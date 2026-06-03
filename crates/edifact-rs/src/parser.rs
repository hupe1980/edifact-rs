//! Streaming EDIFACT parser — wraps a [`Tokenizer`] and assembles [`Segment`]s.

use crate::{
    error::EdifactError,
    model::{Element, OwnedSegment, Segment, Span},
    tokenizer::{Token, Tokenizer},
};
use memchr::memchr2;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::io::{BufRead, BufReader, Read};

fn finish_element<'a>(
    elements: &mut Vec<Element<'a>>,
    current_components: &mut SmallVec<[(Cow<'a, str>, Span); 4]>,
    current_element_start: &mut Option<usize>,
) {
    if let (Some(start), Some((_, last_span))) =
        (current_element_start.take(), current_components.last())
    {
        let last_end = last_span.end;
        elements.push(Element {
            span: Span::new(start, last_end),
            components: std::mem::take(current_components),
        });
    }
}

fn resolve_release(
    val: &str,
    release_char: char,
    start_offset: usize,
) -> Result<Cow<'_, str>, EdifactError> {
    if !val.contains(release_char) {
        return Ok(Cow::Borrowed(val));
    }
    resolve_release_owned(val, release_char, start_offset).map(Cow::Owned)
}

fn resolve_release_owned(
    val: &str,
    release_char: char,
    start_offset: usize,
) -> Result<String, EdifactError> {
    let mut out = String::with_capacity(val.len());
    let mut chars = val.chars();
    while let Some(c) = chars.next() {
        if c == release_char {
            if let Some(escaped) = chars.next() {
                out.push(escaped);
            } else {
                return Err(EdifactError::InvalidReleaseSequence {
                    offset: start_offset + val.len().saturating_sub(1),
                });
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// Streaming parser over a [`Tokenizer`].
///
/// Implements `Iterator<Item = Result<Segment<'a>, EdifactError>>`.
/// Each `next()` call produces one fully-assembled segment.
pub struct Parser<'a> {
    tokenizer: Tokenizer<'a>,
    /// Buffered token from a previous `next()` call (tag peeked ahead).
    peeked: Option<Token<'a>>,
    /// Release character from the active service string advice.
    release_char: char,
}

impl<'a> Parser<'a> {
    /// Construct a parser from a tokenizer.
    pub fn new(tokenizer: Tokenizer<'a>) -> Self {
        let release_char = tokenizer.service_string_advice().release_char as char;
        Self {
            tokenizer,
            peeked: None,
            release_char,
        }
    }
}

impl<'a> Iterator for Parser<'a> {
    type Item = Result<Segment<'a>, EdifactError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Obtain the segment tag (may have been peeked from a previous iteration)
        let (tag, tag_span) = loop {
            let tok = match self.peeked.take() {
                Some(t) => Ok(t),
                None => self.tokenizer.next()?,
            };
            match tok {
                Ok(Token::SegmentTag { value, span }) => break (value, span),
                Ok(Token::SegmentTerminator { .. }) => continue, // stray terminator — tolerated (blank line)
                Ok(Token::DataElement { span, .. }) | Ok(Token::ComponentElement { span, .. }) => {
                    return Some(Err(EdifactError::UnexpectedDataToken {
                        offset: span.start,
                    }));
                }
                Err(e) => return Some(Err(e)),
            }
        };

        let mut elements: Vec<Element<'a>> = Vec::with_capacity(8);
        let mut current_components: SmallVec<[(Cow<'a, str>, Span); 4]> = SmallVec::new();
        let mut current_element_start: Option<usize> = None;
        let mut in_element = false;
        let mut segment_end = tag_span.end;

        loop {
            let tok = match self.tokenizer.next() {
                Some(Ok(t)) => t,
                Some(Err(e)) => return Some(Err(e)),
                None => {
                    // EOF — flush whatever we have
                    if in_element {
                        finish_element(
                            &mut elements,
                            &mut current_components,
                            &mut current_element_start,
                        );
                        if let Some(last) = elements.last() {
                            segment_end = last.span.end;
                        }
                    }
                    break;
                }
            };

            match tok {
                Token::SegmentTag {
                    value: next_tag,
                    span,
                } => {
                    // We consumed the first token of the *next* segment; save it.
                    self.peeked = Some(Token::SegmentTag {
                        value: next_tag,
                        span,
                    });
                    if in_element {
                        finish_element(
                            &mut elements,
                            &mut current_components,
                            &mut current_element_start,
                        );
                        if let Some(last) = elements.last() {
                            segment_end = last.span.end;
                        }
                    }
                    break;
                }
                Token::SegmentTerminator { span } => {
                    if in_element {
                        finish_element(
                            &mut elements,
                            &mut current_components,
                            &mut current_element_start,
                        );
                    }
                    segment_end = span.end;
                    break;
                }
                Token::DataElement { value, span } => {
                    if in_element {
                        finish_element(
                            &mut elements,
                            &mut current_components,
                            &mut current_element_start,
                        );
                    }
                    let resolved = match resolve_release(value, self.release_char, span.start) {
                        Ok(v) => v,
                        Err(error) => return Some(Err(error)),
                    };
                    current_components.push((resolved, span));
                    current_element_start = Some(span.start);
                    in_element = true;
                }
                Token::ComponentElement { value, span } => {
                    if !in_element {
                        // component before any element — treat as first element
                        in_element = true;
                        current_element_start = Some(span.start);
                    }
                    let resolved = match resolve_release(value, self.release_char, span.start) {
                        Ok(v) => v,
                        Err(error) => return Some(Err(error)),
                    };
                    current_components.push((resolved, span));
                }
            }
        }

        Some(Ok(Segment {
            tag,
            span: Span::new(tag_span.start, segment_end),
            tag_span,
            elements,
        }))
    }
}

/// Parse EDIFACT from an arbitrary reader.
///
/// This path is optimized for bounded-memory ingest and returns owned segments,
/// allowing the parser to advance across chunk boundaries without requiring a
/// fully-buffered input slice.
pub fn from_reader<R: Read>(reader: R) -> Result<Vec<OwnedSegment>, EdifactError> {
    from_reader_stream(reader).collect()
}

/// Parse EDIFACT from a buffered reader.
pub fn from_bufread<R: BufRead>(reader: R) -> Result<Vec<OwnedSegment>, EdifactError> {
    from_bufread_stream(reader).collect()
}

/// Configuration for reader-based EDIFACT parsers.
///
/// Pass to [`from_reader_with_config`] or [`from_bufread_stream_with_config`] to
/// override default limits.
///
/// # Example
/// ```
/// use edifact_rs::{ReaderConfig, from_reader_with_config};
///
/// let cfg = ReaderConfig::default().max_segment_bytes(4_096);
/// let segments: Vec<_> = from_reader_with_config(b"BGM+220+1+9'".as_ref(), cfg)
///     .collect::<Result<_, _>>()
///     .unwrap();
/// assert_eq!(segments[0].tag, "BGM");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ReaderConfig {
    /// Maximum allowed segment byte length (excluding the segment terminator).
    ///
    /// If a segment accumulates more bytes than this limit without a terminator
    /// the parser returns [`EdifactError::SegmentTooLong`].  This prevents
    /// unbounded allocation when processing malformed or adversarially crafted
    /// input streams.
    ///
    /// Default: 65 536 bytes (64 KiB).  Real-world EDIFACT segments are almost
    /// always below 4 KiB; consider using a tighter limit for untrusted inputs.
    pub max_segment_bytes: usize,
    /// Maximum number of segments to yield before the stream stops.
    ///
    /// Once this many segments have been produced the segment stream returns
    /// `None`, effectively truncating the message.  Useful for preventing
    /// resource exhaustion when the total segment count in a message is expected
    /// to be bounded.
    ///
    /// Default: `None` (unlimited).
    pub max_segments: Option<usize>,
    /// Maximum total input bytes to consume before the stream stops.
    ///
    /// The budget is checked **before** each segment is read.  If
    /// `bytes_consumed >= max_input_bytes` at that point the stream stops
    /// immediately without reading any further data.  A segment whose bytes
    /// push `bytes_consumed` above the threshold is still yielded (parsing
    /// cannot be abandoned mid-segment), but no subsequent segment will be
    /// started.  Use in combination with `max_segment_bytes` for
    /// defence-in-depth against maliciously large inputs.
    ///
    /// Default: `None` (unlimited).
    pub max_input_bytes: Option<u64>,
    /// Maximum number of EDIFACT messages (UNH/UNT pairs) to process before
    /// the stream stops.
    ///
    /// Once this many complete messages have been yielded the stream returns
    /// `None`. Useful for rate-limiting or sampling large interchanges without
    /// parsing the entire file.
    ///
    /// Default: `None` (unlimited).
    pub max_messages: Option<usize>,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            max_segment_bytes: 65_536,
            max_segments: None,
            max_input_bytes: None,
            max_messages: None,
        }
    }
}

impl ReaderConfig {
    /// Set the maximum segment byte length and return `self`.
    #[must_use]
    pub fn max_segment_bytes(mut self, limit: usize) -> Self {
        self.max_segment_bytes = limit;
        self
    }

    /// Set the maximum number of segments to yield and return `self`.
    #[must_use]
    pub fn max_segments(mut self, limit: usize) -> Self {
        self.max_segments = Some(limit);
        self
    }

    /// Set the maximum total input bytes to consume and return `self`.
    #[must_use]
    pub fn max_input_bytes(mut self, limit: u64) -> Self {
        self.max_input_bytes = Some(limit);
        self
    }

    /// Set the maximum number of EDIFACT messages (UNH/UNT pairs) to yield and return `self`.
    #[must_use]
    pub fn max_messages(mut self, limit: usize) -> Self {
        self.max_messages = Some(limit);
        self
    }
}

/// Streaming state for [`OwnedSegmentStream`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamState {
    /// UNA header not yet scanned; must inspect first bytes.
    Init,
    /// UNA has been scanned (or was absent); streaming segments.
    Running,
    /// A terminal error was encountered; no more items.
    Done,
}

/// Streaming iterator over owned segments from a buffered reader.
///
/// # Performance
///
/// A **fast path** uses [`BufRead::fill_buf`] + `memchr` to locate the segment
/// terminator within the OS-level read buffer (typically 8 KB) without any
/// intermediate heap allocation.  The segment bytes are parsed directly from
/// the buffer slice and converted to an [`OwnedSegment`] in a single pass.
///
/// For segments that span read-buffer boundaries the implementation falls back
/// to the byte-accumulation slow path, which allocates a temporary `Vec<u8>`
/// and re-tokenizes — the same behaviour as in older versions of the library.
/// In practice this fallback is rare because the default `BufReader` buffer
/// (8 KB) is far larger than a typical EDIFACT segment (<300 bytes).
///
/// Configure limits via [`ReaderConfig`] and [`from_reader_with_config`] /
/// [`from_bufread_stream_with_config`].
pub struct OwnedSegmentStream<R: BufRead> {
    reader: R,
    ssa: crate::tokenizer::ServiceStringAdvice,
    state: StreamState,
    stream_offset: u64,
    config: ReaderConfig,
    /// Number of segments successfully yielded so far.
    segments_yielded: usize,
    /// Number of complete EDIFACT messages (UNH/UNT pairs) seen so far.
    messages_yielded: usize,
    /// Whether the last segment tag was UNH (inside a message).
    in_message: bool,
    /// Total bytes consumed from the reader (UNA header + segment data).
    bytes_consumed: u64,
}

impl<R: BufRead> OwnedSegmentStream<R> {
    fn new(reader: R) -> Self {
        Self::with_config(reader, ReaderConfig::default())
    }

    fn with_config(reader: R, config: ReaderConfig) -> Self {
        Self {
            reader,
            ssa: crate::tokenizer::ServiceStringAdvice::default(),
            state: StreamState::Init,
            stream_offset: 0,
            config,
            segments_yielded: 0,
            messages_yielded: 0,
            in_message: false,
            bytes_consumed: 0,
        }
    }
}

// ── fast-path helpers ─────────────────────────────────────────────────────────

/// Outcome of a single-buffer segment extraction attempt.
enum FastSegment {
    /// Segment parsed; second field = bytes to consume (content + terminator).
    Parsed(OwnedSegment, usize),
    /// Only whitespace or an isolated terminator; bytes to skip and continue.
    Skip(usize),
    /// Terminator not present in the current buffer; caller must use slow path.
    NeedMore,
    /// Buffer is empty — no more input.
    Eof,
    /// Parse error.
    Err(EdifactError),
}

/// Return the byte offset of the first **unescaped** occurrence of `term` in `buf`.
///
/// A byte is *escaped* when it is immediately preceded by `release` (e.g.
/// `?'` escapes `'`).  Two consecutive release chars cancel each other, so
/// `??'` contains an *unescaped* `'`.
///
/// # Complexity
///
/// O(n) in the length of `buf`.  `memchr2` is used to fast-scan past bytes
/// that are neither `release` nor `term`, so SIMD acceleration applies on
/// platforms where `memchr` provides it.
fn find_unescaped_term(buf: &[u8], term: u8, release: u8) -> Option<usize> {
    let mut i = 0;
    while i < buf.len() {
        // Fast-skip to the next byte that might be a release char or terminator.
        let rel = memchr2(release, term, &buf[i..])?;
        let pos = i + rel;
        if buf[pos] == release {
            // Release char: the next byte is escaped — skip both.
            i = pos + 2;
        } else {
            // Unescaped terminator found.
            return Some(pos);
        }
    }
    None
}

/// Try to parse one segment directly from the `BufRead` buffer.
///
/// This function borrows `reader` only for the duration of the call.  After it
/// returns the caller is free to call `reader.consume(n)`.
fn try_fast_segment<R: BufRead>(
    reader: &mut R,
    ssa: crate::tokenizer::ServiceStringAdvice,
    seg_start: usize,
    max_segment_bytes: usize,
) -> FastSegment {
    let buf = match reader.fill_buf() {
        Ok(b) => b,
        Err(e) => return FastSegment::Err(e.into()),
    };

    if buf.is_empty() {
        return FastSegment::Eof;
    }

    let Some(pos) = find_unescaped_term(buf, ssa.segment_term, ssa.release_char) else {
        return FastSegment::NeedMore;
    };

    // Enforce the segment-size guard *before* any allocation.
    // `pos` is the index of the terminator byte, so the segment body is `buf[..pos]`.
    if pos > max_segment_bytes {
        return FastSegment::Err(EdifactError::SegmentTooLong {
            offset: seg_start,
            limit: max_segment_bytes,
        });
    }

    // `buf[..pos]` is the segment content without the terminator.
    let seg_bytes = &buf[..pos];

    // Skip isolated terminators / pure-whitespace slots between segments.
    if seg_bytes
        .iter()
        .all(|&b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
    {
        return FastSegment::Skip(pos + 1);
    }

    // Parse directly from the buffer slice — zero intermediate allocation.
    // Include the terminator byte so the parser sees a `SegmentTerminator`
    // token and records a span that is consistent with the `from_bytes` path.
    // Use `with_limit(max_segment_bytes)` so the tokenizer respects the caller's
    // configured limit; `Tokenizer::new` would impose a hard 64 KiB cap that
    // could reject segments already allowed by a larger `max_segment_bytes`.
    let tok = Tokenizer::with_limit(&buf[..pos + 1], ssa, max_segment_bytes);
    let mut parser_iter = Parser::new(tok);
    match parser_iter.next() {
        None => FastSegment::Skip(pos + 1),
        Some(Err(e)) => FastSegment::Err(e),
        Some(Ok(s)) => FastSegment::Parsed(OwnedSegment::from(s).offset(seg_start), pos + 1),
    }
    // `buf` borrow released here — `reader.consume()` is safe to call in the caller.
}

// ── Iterator impl ─────────────────────────────────────────────────────────────

impl<R: BufRead> Iterator for OwnedSegmentStream<R> {
    type Item = Result<OwnedSegment, EdifactError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.state == StreamState::Done {
            return None;
        }

        // Check segment count limit before attempting to read the next segment.
        if let Some(max) = self.config.max_segments {
            if self.segments_yielded >= max {
                self.state = StreamState::Done;
                return None;
            }
        }

        // Check byte budget before attempting to read the next segment.
        if let Some(max) = self.config.max_input_bytes {
            if self.bytes_consumed >= max {
                self.state = StreamState::Done;
                return None;
            }
        }

        // Check message count limit before attempting to read the next segment.
        if let Some(max) = self.config.max_messages {
            if self.messages_yielded >= max {
                self.state = StreamState::Done;
                return None;
            }
        }

        loop {
            // ── Fast path (after UNA has been consumed) ───────────────────
            if self.state == StreamState::Running {
                let seg_start = self.stream_offset;
                match try_fast_segment(
                    &mut self.reader,
                    self.ssa,
                    seg_start as usize,
                    self.config.max_segment_bytes,
                ) {
                    FastSegment::Parsed(seg, n) => {
                        let n = n as u64;
                        self.reader.consume(n as usize);
                        self.stream_offset += n;
                        self.bytes_consumed = self.stream_offset;
                        self.segments_yielded += 1;
                        // Track message boundaries for max_messages enforcement.
                        // Only count a UNT that closes a UNH we already saw; a bare
                        // UNT without a preceding UNH is malformed and must not inflate
                        // the counter (matches the documented UNH/UNT-pair semantics).
                        if seg.tag == "UNT" {
                            if self.in_message {
                                self.messages_yielded += 1;
                            }
                            self.in_message = false;
                        } else if seg.tag == "UNH" {
                            self.in_message = true;
                        }
                        // Eagerly mark Done if the byte budget was exhausted by
                        // this segment so the next next() call returns None
                        // without a redundant read attempt.
                        if let Some(max) = self.config.max_input_bytes {
                            if self.bytes_consumed >= max {
                                self.state = StreamState::Done;
                            }
                        }
                        return Some(Ok(seg));
                    }
                    FastSegment::Skip(n) => {
                        let n = n as u64;
                        self.reader.consume(n as usize);
                        self.stream_offset += n;
                        self.bytes_consumed = self.stream_offset;
                        continue;
                    }
                    FastSegment::Eof => return None,
                    FastSegment::Err(e) => {
                        self.state = StreamState::Done;
                        return Some(Err(e));
                    }
                    FastSegment::NeedMore => {
                        // Segment spans buffer boundary — fall through to slow path.
                    }
                }
            }

            // ── Slow path: byte accumulation (also handles UNA header) ────
            let mut scanned = self.state != StreamState::Init;
            // `read_next_raw_segment` tracks offset as `usize` for segment
            // start positions; sync back to the `u64` field afterward.
            let mut slow_offset: usize = self.stream_offset as usize;
            let mut raw = match read_next_raw_segment(
                &mut self.reader,
                &mut self.ssa,
                &mut scanned,
                &mut slow_offset,
                self.config.max_segment_bytes,
            ) {
                Ok(Some(r)) => r,
                Ok(None) => return None,
                Err(e) => {
                    self.state = StreamState::Done;
                    return Some(Err(e));
                }
            };
            self.stream_offset = slow_offset as u64;
            if scanned {
                self.state = StreamState::Running;
            }
            self.bytes_consumed = self.stream_offset;

            raw.bytes.push(self.ssa.segment_term);
            // Use `with_limit` so the configured max_segment_bytes is honoured on the
            // slow path as well; `Tokenizer::new` would impose a hard 64 KiB cap that
            // could reject segments the caller explicitly permitted via ReaderConfig.
            let tok = Tokenizer::with_limit(
                raw.bytes.as_slice(),
                self.ssa,
                self.config.max_segment_bytes,
            );
            let mut parser_iter = Parser::new(tok);
            match parser_iter.next() {
                Some(Ok(s)) => {
                    self.segments_yielded += 1;
                    let seg = OwnedSegment::from(s).offset(raw.start_offset);
                    if seg.tag == "UNT" {
                        if self.in_message {
                            self.messages_yielded += 1;
                        }
                        self.in_message = false;
                    } else if seg.tag == "UNH" {
                        self.in_message = true;
                    }
                    return Some(Ok(seg));
                }
                Some(Err(e)) => {
                    self.state = StreamState::Done;
                    return Some(Err(e));
                }
                None => {} // Empty segment — loop back.
            }
        }
    }
}

/// Parse EDIFACT from a buffered reader as a streaming iterator.
pub fn from_bufread_stream<R: BufRead>(reader: R) -> OwnedSegmentStream<R> {
    OwnedSegmentStream::new(reader)
}

/// Parse EDIFACT from a buffered reader as a streaming iterator with custom config.
pub fn from_bufread_stream_with_config<R: BufRead>(
    reader: R,
    config: ReaderConfig,
) -> OwnedSegmentStream<R> {
    OwnedSegmentStream::with_config(reader, config)
}

/// Parse EDIFACT from an arbitrary reader as a streaming iterator.
pub fn from_reader_stream<R: Read>(reader: R) -> OwnedSegmentStream<BufReader<R>> {
    from_bufread_stream(BufReader::new(reader))
}

/// Parse EDIFACT from an arbitrary reader as a streaming iterator with custom config.
///
/// # Example
/// ```
/// use edifact_rs::{ReaderConfig, from_reader_with_config};
///
/// let cfg = ReaderConfig::default().max_segment_bytes(4_096);
/// let segs: Vec<_> = from_reader_with_config(b"BGM+220+1+9'".as_ref(), cfg)
///     .collect::<Result<_, _>>()
///     .unwrap();
/// assert_eq!(segs[0].tag, "BGM");
/// ```
pub fn from_reader_with_config<R: Read>(
    reader: R,
    config: ReaderConfig,
) -> OwnedSegmentStream<BufReader<R>> {
    from_bufread_stream_with_config(BufReader::new(reader), config)
}

fn read_next_raw_segment<R: BufRead>(
    reader: &mut R,
    ssa: &mut crate::tokenizer::ServiceStringAdvice,
    scanned_header: &mut bool,
    stream_offset: &mut usize,
    max_segment_bytes: usize,
) -> Result<Option<crate::tokenizer::RawSegment>, EdifactError> {
    loop {
        let Some((first_offset, first)) = read_next_non_ws_byte(reader, stream_offset)? else {
            return Ok(None);
        };

        if !*scanned_header && first == b'U' {
            let second = read_required_byte(reader, stream_offset)?;
            let third = read_required_byte(reader, stream_offset)?;
            if second == b'N' && third == b'A' {
                let mut una = [0u8; 9];
                una[0] = b'U';
                una[1] = b'N';
                una[2] = b'A';
                for slot in una.iter_mut().skip(3) {
                    *slot = read_required_byte(reader, stream_offset)?;
                }
                *ssa = crate::tokenizer::ServiceStringAdvice {
                    component_sep: una[3],
                    element_sep: una[4],
                    decimal_mark: una[5],
                    release_char: una[6],
                    segment_term: una[8],
                };
                if !ssa.is_valid() {
                    return Err(EdifactError::InvalidUna);
                }
                *scanned_header = true;
                continue;
            }

            *scanned_header = true;
            return read_remainder_of_segment(
                reader,
                ssa,
                crate::tokenizer::RawSegment {
                    bytes: vec![first, second, third],
                    start_offset: first_offset,
                },
                stream_offset,
                max_segment_bytes,
            );
        }

        *scanned_header = true;
        return read_remainder_of_segment(
            reader,
            ssa,
            crate::tokenizer::RawSegment {
                bytes: vec![first],
                start_offset: first_offset,
            },
            stream_offset,
            max_segment_bytes,
        );
    }
}

fn read_remainder_of_segment<R: BufRead>(
    reader: &mut R,
    ssa: &crate::tokenizer::ServiceStringAdvice,
    mut out: crate::tokenizer::RawSegment,
    stream_offset: &mut usize,
    max_segment_bytes: usize,
) -> Result<Option<crate::tokenizer::RawSegment>, EdifactError> {
    let mut escaped = false;
    loop {
        if out.bytes.len() >= max_segment_bytes {
            return Err(EdifactError::SegmentTooLong {
                offset: out.start_offset,
                limit: max_segment_bytes,
            });
        }
        let Some(byte) = read_next_byte(reader, stream_offset)? else {
            return if out.bytes.is_empty() {
                Ok(None)
            } else if escaped {
                Err(EdifactError::InvalidReleaseSequence {
                    offset: out.start_offset + out.bytes.len().saturating_sub(1),
                })
            } else {
                Err(EdifactError::UnexpectedEof {
                    offset: out.start_offset + out.bytes.len(),
                })
            };
        };

        if !escaped && byte == ssa.segment_term {
            return Ok(Some(out));
        }

        if !escaped && byte == ssa.release_char {
            escaped = true;
            out.bytes.push(byte);
            continue;
        }

        escaped = false;
        out.bytes.push(byte);
    }
}

fn read_next_byte<R: BufRead>(
    reader: &mut R,
    stream_offset: &mut usize,
) -> Result<Option<u8>, EdifactError> {
    let buf = reader.fill_buf()?;
    if buf.is_empty() {
        return Ok(None);
    }

    let byte = buf[0];
    reader.consume(1);
    *stream_offset += 1;
    Ok(Some(byte))
}

fn read_required_byte<R: BufRead>(
    reader: &mut R,
    stream_offset: &mut usize,
) -> Result<u8, EdifactError> {
    read_next_byte(reader, stream_offset)?.ok_or(EdifactError::UnexpectedEof {
        offset: *stream_offset,
    })
}

fn read_next_non_ws_byte<R: BufRead>(
    reader: &mut R,
    stream_offset: &mut usize,
) -> Result<Option<(usize, u8)>, EdifactError> {
    loop {
        let current_offset = *stream_offset;
        let Some(byte) = read_next_byte(reader, stream_offset)? else {
            return Ok(None);
        };
        if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            return Ok(Some((current_offset, byte)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::ServiceStringAdvice;

    fn parse_all(input: &[u8]) -> Vec<Segment<'_>> {
        let ssa = ServiceStringAdvice::from_bytes(input);
        let tok = Tokenizer::new(input, ssa);
        Parser::new(tok)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse failed")
    }

    #[test]
    fn parses_unb_unz() {
        let input = b"UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'UNZ+0+1'";
        let segs = parse_all(input);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].tag, "UNB");
        assert_eq!(segs[1].tag, "UNZ");
        assert_eq!(segs[0].tag_span, Span::new(0, 3));
        assert_eq!(segs[0].span, Span::new(0, 41));
    }

    #[test]
    fn element_access() {
        let input = b"BGM+220+ORDER123+9'";
        let segs = parse_all(input);
        assert_eq!(segs[0].element_str(0), Some("220"));
        assert_eq!(segs[0].element_str(1), Some("ORDER123"));
    }

    #[test]
    fn component_access() {
        let input = b"DTM+137:20200101:102'";
        let segs = parse_all(input);
        let dtm = &segs[0];
        assert_eq!(dtm.get_element(0).unwrap().get_component(0), Some("137"));
        assert_eq!(
            dtm.get_element(0).unwrap().get_component(1),
            Some("20200101")
        );
        assert_eq!(dtm.get_element(0).unwrap().get_component(2), Some("102"));
    }

    #[test]
    fn release_char_resolved() {
        let input = b"FTX+AAA++test?+value'";
        let segs = parse_all(input);
        assert_eq!(segs[0].element_str(2), Some("test+value"));
        assert_eq!(
            segs[0].get_element(2).unwrap().component_span(0),
            Some(Span::new(9, 20))
        );
    }

    #[test]
    fn reader_path_preserves_custom_una_delimiters() {
        let input = b"UNA:;.? 'BGM;220;test?;value'";
        let segments = super::from_bufread(std::io::BufReader::new(std::io::Cursor::new(input)))
            .expect("reader parse should succeed");
        let bgm = segments
            .iter()
            .find(|segment| segment.tag == "BGM")
            .expect("BGM segment should be present");
        assert_eq!(bgm.elements[0].components[0].0, "220");
        assert_eq!(bgm.elements[1].components[0].0, "test;value");
    }

    #[test]
    fn arbitrary_bytes_no_panic() {
        // This is the stable no-panic property — arbitrary input must not panic
        let garbage: &[u8] = b"\xff\x00\x01\x02ABC+++'''???";
        let _ = crate::from_bytes(garbage).collect::<Vec<_>>();
    }

    #[test]
    fn from_reader_handles_chunk_boundaries() {
        let input = b"UNA:+.? 'BGM+220+test?+value'UNT+2+1'";
        let reader = std::io::BufReader::with_capacity(5, std::io::Cursor::new(input));
        let parsed = from_bufread(reader).expect("reader parsing should succeed");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].tag, "BGM");
        assert_eq!(parsed[0].elements[1].components[0].0, "test+value");
        assert_eq!(parsed[1].tag, "UNT");
    }

    #[test]
    fn from_reader_without_una_uses_default_delimiters() {
        let input = b"BGM+220+X'UNT+2+1'";
        let parsed =
            from_reader(std::io::Cursor::new(input)).expect("reader parsing should succeed");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].tag, "BGM");
        assert_eq!(parsed[0].elements[0].components[0].0, "220");
        assert_eq!(parsed[1].span, Span::new(10, 18));
    }

    #[test]
    fn dangling_release_sequence_is_error() {
        let input = b"FTX+AAA++dangling?";
        let err = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect_err("expected dangling release to fail");

        assert!(matches!(err, EdifactError::InvalidReleaseSequence { .. }));
    }

    #[test]
    fn from_reader_reports_dangling_release_sequence() {
        let input = b"FTX+AAA++dangling?";
        let err = from_reader(std::io::Cursor::new(input))
            .expect_err("expected dangling release from reader path");
        assert!(matches!(err, EdifactError::InvalidReleaseSequence { .. }));
    }

    #[test]
    fn from_reader_rejects_invalid_una() {
        let input = b"UNA::.? 'BGM:220'";
        let err = from_reader(std::io::Cursor::new(input))
            .expect_err("invalid UNA should fail reader parsing");
        assert!(matches!(err, EdifactError::InvalidUna));
    }
}
