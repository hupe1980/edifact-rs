//! Streaming EDIFACT parser — wraps a [`Tokenizer`] and assembles [`Segment`]s.

use crate::{
    error::EdifactError,
    model::{Components, Element, OwnedSegment, Segment, Span},
    tokenizer::{Token, Tokenizer},
};
use memchr::memchr2;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::io::{BufRead, BufReader, Read};

/// In-progress data element: the components of the repetition currently being
/// read, plus every repetition already closed by a repetition separator.
///
/// The first closed repetition has a slot of its own, so `repeats` only ever
/// receives the second and later ones. An element that does not repeat — every
/// element of an interchange without a repetition separator — therefore never
/// allocates a repetition vector.
struct PendingElement<'a> {
    start: Option<usize>,
    components: Components<'a>,
    /// The first repetition, once a repetition separator has closed it.
    first: Option<Components<'a>>,
    /// Closed repetitions after the first; unallocated until there is one.
    repeats: Vec<Components<'a>>,
}

impl<'a> PendingElement<'a> {
    fn new() -> Self {
        Self {
            start: None,
            components: SmallVec::new(),
            first: None,
            repeats: Vec::new(),
        }
    }

    #[inline]
    fn is_open(&self) -> bool {
        self.start.is_some()
    }

    /// Open a new element (or, when already open, a further repetition of it).
    #[inline]
    fn open(&mut self, start: usize) {
        if self.start.is_none() {
            self.start = Some(start);
        }
    }

    /// Close the current repetition and begin the next one.
    #[inline]
    fn end_repetition(&mut self) {
        let closed = std::mem::take(&mut self.components);
        if self.first.is_none() {
            self.first = Some(closed);
        } else {
            self.repeats.push(closed);
        }
    }

    /// Close the element and push it onto `elements`, returning its end offset.
    fn flush(&mut self, elements: &mut Vec<Element<'a>>) -> Option<usize> {
        let start = self.start.take()?;
        let current = std::mem::take(&mut self.components);
        let first = self.first.take();
        let mut repeats = std::mem::take(&mut self.repeats);
        // The element ends at the last component of the last *non-empty*
        // repetition; a trailing repetition separator with no value after it
        // contributes no span. Repetitions in reverse order: the current one,
        // the closed later ones, then the first.
        let end = std::iter::once(&current)
            .chain(repeats.iter().rev())
            .chain(first.iter())
            .find_map(|rep| rep.last().map(|(_, span)| span.end))?;
        let components = match first {
            None => current,
            Some(first) => {
                repeats.push(current);
                first
            }
        };
        elements.push(Element {
            span: Span::new(start, end),
            components,
            repeats,
        });
        Some(end)
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
    // The unescaped output is shorter than the input (every release pair loses
    // one byte), so `val.len()` is an upper bound: the value costs exactly one
    // allocation, never a regrowth.
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
    /// Reusable element buffer: the elements of the segment being read are
    /// collected here, then moved into a vector of exactly the right length.
    /// Grows geometrically and is kept for the whole parse, so a segment
    /// with elements costs one allocation once the buffer is warm.
    scratch: Vec<Element<'a>>,
    /// Reader-path hint: when `Some(n)`, each segment's element vector is
    /// allocated once with capacity `n` and `scratch` is not used.
    element_capacity: Option<usize>,
}

impl<'a> Parser<'a> {
    /// Construct a parser from a tokenizer.
    pub fn new(tokenizer: Tokenizer<'a>) -> Self {
        let release_char = tokenizer.service_string_advice().release_char as char;
        Self {
            tokenizer,
            peeked: None,
            release_char,
            scratch: Vec::new(),
            element_capacity: None,
        }
    }

    /// A parser for one already-located segment whose element count is known
    /// to be at most `element_capacity`.
    ///
    /// The reader path builds a parser per segment, so a reusable buffer would
    /// cost an extra allocation each time; the number of element-separator
    /// bytes in the segment is an upper bound instead (exact unless a separator
    /// is release-escaped), and the element vector is allocated once at that
    /// capacity — or not at all for a segment without elements.
    pub(crate) fn with_element_capacity(tokenizer: Tokenizer<'a>, element_capacity: usize) -> Self {
        Self {
            element_capacity: Some(element_capacity),
            ..Self::new(tokenizer)
        }
    }
}

impl Parser<'_> {
    /// Absolute byte position of the underlying tokenizer.
    ///
    /// Between two `next()` calls this is the byte just past the previous
    /// segment's terminator, which is where the next segment (after any
    /// inter-segment whitespace) begins — the recovering scan relies on that to
    /// find the start of a segment the parser refused.
    pub(crate) fn byte_position(&self) -> usize {
        self.tokenizer.position()
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
                Ok(Token::DataElement { span, .. })
                | Ok(Token::ComponentElement { span, .. })
                | Ok(Token::RepeatElement { span, .. }) => {
                    return Some(Err(EdifactError::UnexpectedDataToken {
                        offset: span.start,
                    }));
                }
                Err(e) => return Some(Err(e)),
            }
        };

        // `Element` is a large struct (inline component storage), so the
        // element vector a segment keeps must not carry slack. With a capacity
        // hint (reader path) it is allocated once at that bound; otherwise the
        // elements go into the reusable `scratch` buffer and are moved into an
        // exact-length vector at the terminator. Either way a segment keeps one
        // element-vector allocation, and none without elements; the slice path
        // additionally pays when `scratch` has to grow (asserted by the
        // `allocation_budget` integration test). An error
        // return drops the taken scratch; the next segment re-grows it.
        let mut elements: Vec<Element<'a>> = match self.element_capacity {
            Some(capacity) => Vec::with_capacity(capacity),
            None => {
                let mut scratch = std::mem::take(&mut self.scratch);
                scratch.clear();
                scratch
            }
        };
        let mut pending = PendingElement::new();
        let mut segment_end = tag_span.end;

        loop {
            let tok = match self.tokenizer.next() {
                Some(Ok(t)) => t,
                Some(Err(e)) => return Some(Err(e)),
                None => {
                    // Input ran out mid-segment: no terminator arrived.
                    //
                    // Yielding what was read would let a truncated file parse as
                    // a complete one — a silent data-integrity failure in a
                    // format whose trailers declare counts.
                    if let Some(end) = pending.flush(&mut elements) {
                        segment_end = end;
                    }
                    return Some(Err(EdifactError::UnexpectedEof {
                        offset: segment_end,
                    }));
                }
            };

            match tok {
                Token::SegmentTag {
                    value: next_tag,
                    span,
                } => {
                    // Not reachable through the current tokenizer, which only
                    // leaves `InSegment` on a terminator (handled below) or on
                    // an error it reports first.  Kept so the match stays total
                    // and a future tokenizer change cannot silently splice two
                    // segments together: a tag where a terminator belongs means
                    // the current segment was never closed.  The token is saved
                    // so a caller that continues sees the next segment intact.
                    self.peeked = Some(Token::SegmentTag {
                        value: next_tag,
                        span,
                    });
                    if let Some(end) = pending.flush(&mut elements) {
                        segment_end = end;
                    }
                    return Some(Err(EdifactError::UnexpectedEof {
                        offset: segment_end,
                    }));
                }
                Token::SegmentTerminator { span } => {
                    pending.flush(&mut elements);
                    segment_end = span.end;
                    break;
                }
                Token::DataElement { value, span } => {
                    pending.flush(&mut elements);
                    let resolved = match resolve_release(value, self.release_char, span.start) {
                        Ok(v) => v,
                        Err(error) => return Some(Err(error)),
                    };
                    pending.open(span.start);
                    pending.components.push((resolved, span));
                }
                Token::ComponentElement { value, span } => {
                    // A component before any element opens the first one.
                    let resolved = match resolve_release(value, self.release_char, span.start) {
                        Ok(v) => v,
                        Err(error) => return Some(Err(error)),
                    };
                    pending.open(span.start);
                    pending.components.push((resolved, span));
                }
                Token::RepeatElement { value, span } => {
                    let resolved = match resolve_release(value, self.release_char, span.start) {
                        Ok(v) => v,
                        Err(error) => return Some(Err(error)),
                    };
                    // A repetition separator before any element is malformed:
                    // there is nothing to repeat.
                    if !pending.is_open() {
                        return Some(Err(EdifactError::UnexpectedDataToken {
                            offset: span.start,
                        }));
                    }
                    pending.end_repetition();
                    pending.components.push((resolved, span));
                }
            }
        }

        if self.element_capacity.is_none() {
            let mut exact = Vec::with_capacity(elements.len());
            exact.append(&mut elements);
            self.scratch = std::mem::replace(&mut elements, exact);
        }

        Some(Ok(Segment {
            tag: Cow::Borrowed(tag),
            span: Span::new(tag_span.start, segment_end),
            tag_span,
            elements,
        }))
    }
}

/// Configuration for the EDIFACT parsers: resource limits and an optional
/// service string advice override.
///
/// Every parsing entry point that has a `_with_config` form takes one — the
/// slice and reader segment iterators ([`from_bytes_with_config`][crate::from_bytes_with_config],
/// [`from_reader_with_config`], [`from_bufread_with_config`]), the decoding
/// variants, the message-window and typed conveniences
/// ([`message_windows_from_reader_with_config`][crate::message_windows_from_reader_with_config]
/// and friends) and the recovering scan.
///
/// The struct is `#[non_exhaustive]`: start from [`ReaderConfig::default`]
/// and chain the builder methods. Fields stay public for reading.
///
/// # Resource bounds
///
/// | Field | Default | Violation |
/// |---|---|---|
/// | `max_segment_bytes` | 65 536 | `E020 SegmentTooLong` |
/// | `max_segments` | unlimited | `E036 LimitExceeded`, `limit = "max_segments"` |
/// | `max_messages` | unlimited | `E036`, `limit = "max_messages"` |
/// | `max_segments_per_message` | unlimited | `E036`, `limit = "max_segments_per_message"` |
/// | `max_input_bytes` | unlimited | `E036`, `limit = "max_input_bytes"` |
///
/// Every limit is reported as an error item, after which the iterator ends;
/// input that ends exactly at a limit is not a violation. The slice and reader
/// paths report the same limit error at the same offset for the same input.
///
/// - **Memory.** A segment iterator holds one segment (at most
///   `max_segment_bytes`) plus, on the reader path, one read buffer, whatever
///   the count limits are. A [`MessageWindows`][crate::MessageWindows] iterator
///   additionally buffers one message: at most `max_segments_per_message`
///   segments of at most `max_segment_bytes` each. **Without
///   `max_segments_per_message` a message window is unbounded** — a `UNH`
///   that never sees its `UNT` is buffered until the input ends;
///   `max_messages` does not change that.
/// - **CPU.** Only `max_input_bytes` bounds the work done on input that yields
///   no segments (for example an endless run of whitespace); every consumed
///   byte — byte order mark, `UNA`, whitespace, stray terminators, segments —
///   counts against it.
///
/// The `resource_limits` integration tests assert each of these bounds,
/// including the peak heap of a window under `max_segments_per_message`.
///
/// # Example
/// ```
/// use edifact_rs::{ReaderConfig, from_reader_with_config};
///
/// let cfg = ReaderConfig::default()
///     .max_segment_bytes(4_096)
///     .max_segments_per_message(10_000)
///     .max_input_bytes(64 << 20);
/// let segments: Vec<_> = from_reader_with_config(b"BGM+220+1+9'".as_ref(), cfg)
///     .collect::<Result<_, _>>()
///     .unwrap();
/// assert_eq!(segments[0].tag, "BGM");
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ReaderConfig {
    /// Maximum allowed segment byte length (excluding the segment terminator).
    ///
    /// If a segment accumulates more bytes than this limit without a terminator
    /// the parser returns [`EdifactError::SegmentTooLong`].  This prevents
    /// unbounded allocation when processing malformed or adversarially crafted
    /// input streams.
    ///
    /// Default: 65 536 bytes (64 KiB).  For untrusted input, set the limit to
    /// the largest segment your partners actually send.
    pub max_segment_bytes: usize,
    /// Maximum number of segments the input may contain.
    ///
    /// A segment beyond this budget yields [`EdifactError::LimitExceeded`]
    /// at the segment's start.
    /// Input that ends exactly at the limit is not a violation.
    ///
    /// This counts the whole input, so on a long-lived stream it eventually
    /// stops valid traffic; bound each message with
    /// [`max_segments_per_message`][Self::max_segments_per_message] instead.
    ///
    /// Default: `None` (unlimited).
    pub max_segments: Option<usize>,
    /// Maximum number of input bytes the parser may consume.
    ///
    /// **Every** consumed byte counts: a byte order mark, the `UNA`,
    /// whitespace, stray segment terminators, segments and their terminators.
    /// Input is consumed in units — the prologue, a run of whitespace or stray
    /// terminators, one segment with its terminator — and a unit whose end
    /// lies beyond the budget yields [`EdifactError::LimitExceeded`] at
    /// `offset = max` *instead of* the unit, so no data beyond the cap is ever
    /// parsed or handed to the caller. Input that ends exactly at the budget,
    /// trailing whitespace included, is not a violation.
    ///
    /// When a segment is both longer than `max_segment_bytes` and crosses
    /// this budget, whichever is detected first in the byte stream wins:
    /// `E020` when the segment's first `max_segment_bytes + 1` bytes lie within
    /// the budget, `E036` otherwise.
    ///
    /// On the decoding paths the budget counts decoded (UTF-8) bytes; a
    /// Latin-1 interchange can be up to twice as long after decoding.
    ///
    /// Default: `None` (unlimited).
    pub max_input_bytes: Option<u64>,
    /// Maximum number of EDIFACT messages (`UNH`/`UNT` pairs) the input may contain.
    ///
    /// The `UNH` of a message beyond this budget yields
    /// [`EdifactError::LimitExceeded`] at its start. It does not bound the size
    /// of a message: see
    /// [`max_segments_per_message`][Self::max_segments_per_message].
    ///
    /// Default: `None` (unlimited).
    pub max_messages: Option<usize>,
    /// Maximum number of segments in any one message, from `UNH` through `UNT`
    /// inclusive.
    ///
    /// The first segment that would exceed it yields
    /// [`EdifactError::LimitExceeded`] with
    /// `limit = "max_segments_per_message"` at that segment's start. Envelope
    /// segments outside a message (`UNB`, `UNG`, `UNE`, `UNZ`) do not
    /// count, and a `UNH` restarts the count even inside an unclosed message.
    /// A message of exactly this many segments is not a violation.
    ///
    /// This is the limit that bounds a [`MessageWindows`][crate::MessageWindows]
    /// buffer, and the one to set on a long-lived stream.
    ///
    /// Default: `None` (unlimited).
    pub max_segments_per_message: Option<usize>,
    /// Service characters to use instead of reading them from the input.
    ///
    /// `None` — the default — discovers them the way ISO 9735-1 says a receiver
    /// should: from a leading `UNA` if there is one, otherwise the §5.1 defaults
    /// with the repetition separator resolved from `UNB` S001 DE 0002.
    ///
    /// Set this when the input is a **fragment** that carries neither: a single
    /// message lifted out of an interchange has no `UNA` and no `UNB`, so nothing
    /// in it records the delimiters its interchange declared, and parsing it with
    /// the defaults silently mis-splits every value.
    ///
    /// Default: `None`.
    pub service_string_advice: Option<crate::tokenizer::ServiceStringAdvice>,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            max_segment_bytes: 65_536,
            max_segments: None,
            max_input_bytes: None,
            max_messages: None,
            max_segments_per_message: None,
            service_string_advice: None,
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

    /// Set the maximum number of segments per message (`UNH` through `UNT`
    /// inclusive) and return `self`.
    ///
    /// See [`max_segments_per_message`][Self::max_segments_per_message].
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{EdifactError, ReaderConfig, from_bytes_with_config};
    ///
    /// let config = ReaderConfig::default().max_segments_per_message(2);
    /// let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220'DTM+137:20260101:102'UNT+4+1'";
    /// let err = from_bytes_with_config(input, config)
    ///     .collect::<Result<Vec<_>, _>>()
    ///     .unwrap_err();
    /// assert!(matches!(
    ///     err,
    ///     EdifactError::LimitExceeded { limit: "max_segments_per_message", max: 2, offset: 30, .. }
    /// ));
    /// ```
    #[must_use]
    pub fn max_segments_per_message(mut self, limit: usize) -> Self {
        self.max_segments_per_message = Some(limit);
        self
    }

    /// Parse with these service characters instead of discovering them.
    ///
    /// See [`service_string_advice`][Self::service_string_advice].
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{ReaderConfig, ServiceStringAdvice, from_bytes_with_config};
    ///
    /// // A message lifted out of an interchange whose UNA declared `;` and `~`.
    /// let ssa = ServiceStringAdvice::from_bytes(b"UNA:;.? ~")?;
    /// let config = ReaderConfig::default().with_service_string_advice(ssa);
    ///
    /// let segments: Vec<_> = from_bytes_with_config(b"BGM;220;PO-4711~", config)
    ///     .collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(segments[0].element_str(1), Some("PO-4711"));
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[must_use]
    pub fn with_service_string_advice(
        mut self,
        ssa: crate::tokenizer::ServiceStringAdvice,
    ) -> Self {
        self.service_string_advice = Some(ssa);
        self
    }
}

// ── budgets ──────────────────────────────────────────────────────────────────

/// The [`EdifactError::LimitExceeded`] for the byte budget `max`.
///
/// The offset is the budget itself — the first byte outside it — which both
/// parse paths can report without examining that byte.
pub(crate) fn input_bytes_exceeded(max: u64) -> EdifactError {
    EdifactError::LimitExceeded {
        limit: "max_input_bytes",
        max,
        offset: usize::try_from(max).unwrap_or(usize::MAX),
    }
}

/// The count and byte budgets of a [`ReaderConfig`] with the running totals
/// they are checked against.
///
/// One type for both segment iterators — [`FromBytesIter`][crate::FromBytesIter]
/// and [`OwnedSegmentStream`] — and the recovering scan, so that the slice and
/// reader paths report the same limit at the same offset by construction
/// rather than by two parallel implementations.
///
/// Check order for a segment: [`admit_bytes`](Self::admit_bytes) for the unit
/// that carries it (before parsing), then [`admit_segment`](Self::admit_segment)
/// (`max_segments`, `max_messages`, `max_segments_per_message`, in that order),
/// then [`account`](Self::account) once the segment is handed out.
#[derive(Debug, Clone)]
pub(crate) struct Budget {
    max_segments: Option<usize>,
    max_messages: Option<usize>,
    max_segments_per_message: Option<usize>,
    max_input_bytes: Option<u64>,
    /// Segments handed out so far.
    segments: usize,
    /// Complete `UNH`…`UNT` messages so far.
    messages: usize,
    /// Whether a `UNH` has been handed out without its `UNT`.
    in_message: bool,
    /// Segments of the open message, its `UNH` included.
    message_segments: usize,
}

impl Budget {
    pub(crate) fn new(config: &ReaderConfig) -> Self {
        Self {
            max_segments: config.max_segments,
            max_messages: config.max_messages,
            max_segments_per_message: config.max_segments_per_message,
            max_input_bytes: config.max_input_bytes,
            segments: 0,
            messages: 0,
            in_message: false,
            message_segments: 0,
        }
    }

    /// The byte budget, if one is set.
    #[inline]
    pub(crate) fn max_input_bytes(&self) -> Option<u64> {
        self.max_input_bytes
    }

    /// Admit a consumed unit that ends at absolute byte `unit_end`.
    ///
    /// A unit ending beyond the byte budget is refused with
    /// `LimitExceeded { limit: "max_input_bytes", offset: max, .. }`; a unit
    /// ending exactly at it is fine.
    #[inline]
    pub(crate) fn admit_bytes(&self, unit_end: u64) -> Result<(), EdifactError> {
        match self.max_input_bytes {
            Some(max) if unit_end > max => Err(input_bytes_exceeded(max)),
            _ => Ok(()),
        }
    }

    /// Whether a segment-size violation detected once byte `detected_at`
    /// (exclusive end) has been read falls inside the byte budget — that is,
    /// whether `E020` is seen before `E036` in the byte stream.
    #[inline]
    pub(crate) fn sees_byte(&self, detected_at: u64) -> bool {
        self.max_input_bytes.is_none_or(|max| detected_at <= max)
    }

    /// Check the count budgets for a segment tagged `tag` starting at `start`,
    /// before it is handed out.
    ///
    /// Checking a segment that has actually been read — rather than refusing
    /// to read once a count is reached — is what lets input that ends exactly
    /// at a limit finish cleanly.
    pub(crate) fn admit_segment(&self, tag: &str, start: usize) -> Result<(), EdifactError> {
        let exceeded = |limit: &'static str, max: usize| EdifactError::LimitExceeded {
            limit,
            max: max as u64,
            offset: start,
        };
        if let Some(max) = self.max_segments {
            if self.segments >= max {
                return Err(exceeded("max_segments", max));
            }
        }
        // Only a `UNH` opens a message, so only a `UNH` can push the count past
        // the budget.  Testing every segment would trip on the interchange
        // trailer, which belongs to no message.
        if let Some(max) = self.max_messages {
            if tag == "UNH" && self.messages >= max {
                return Err(exceeded("max_messages", max));
            }
        }
        if let Some(max) = self.max_segments_per_message {
            let position = if tag == "UNH" {
                1
            } else if self.in_message {
                self.message_segments + 1
            } else {
                0
            };
            if position > max {
                return Err(exceeded("max_segments_per_message", max));
            }
        }
        Ok(())
    }

    /// Record a segment that has been handed out.
    ///
    /// Only a `UNT` that closes a `UNH` already seen advances the message
    /// counter; a bare `UNT` is malformed and must not inflate it.
    pub(crate) fn account(&mut self, tag: &str) {
        self.segments += 1;
        if tag == "UNH" {
            self.in_message = true;
            self.message_segments = 1;
        } else if self.in_message {
            if tag == "UNT" {
                self.messages += 1;
                self.in_message = false;
                self.message_segments = 0;
            } else {
                self.message_segments += 1;
            }
        }
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
/// terminator within the reader's buffer (8 KiB, `std`'s `BufReader` default
/// capacity, for [`from_reader`](crate::from_reader)) without any intermediate
/// heap allocation.  The segment bytes are parsed directly from
/// the buffer slice and converted to an [`OwnedSegment`] in a single pass.
///
/// Owned text costs one allocation for the tag and one per non-empty component
/// value (an escaped value is unescaped once and moved, not copied again); the
/// element list is one allocation sized by the segment's element-separator
/// count, and an element that does not repeat allocates nothing for
/// repetitions. The `allocation_budget` integration test enforces these counts.
///
/// A segment that straddles the read buffer's boundary — and the first one
/// after a `UNA` — takes the slow path instead: its bytes are accumulated into
/// a temporary `Vec<u8>` and tokenized from there.
///
/// Configure limits via [`ReaderConfig`] and [`from_reader_with_config`] /
/// [`from_bufread_with_config`].
///
/// # Resource bounds
///
/// Every [`ReaderConfig`] limit applies (`max_segment_bytes`, `max_segments`,
/// `max_messages`, `max_segments_per_message`, `max_input_bytes`); a violation
/// is an error item, after which the stream ends. With
/// `ReaderConfig::default()` only `max_segment_bytes` (64 KiB) is set.
///
/// - **Memory:** one segment of at most `max_segment_bytes` plus the reader's
///   buffer, independent of the input length and of the count limits.
/// - **CPU:** bounded by `max_input_bytes`, which counts every consumed byte —
///   byte order mark, `UNA`, whitespace, stray terminators and segments. The
///   check runs inside the byte-skipping loops, so an endless run of
///   whitespace stops within one read buffer of the budget. Without
///   `max_input_bytes` such input holds memory at the bound above but runs for
///   unbounded time (the `resource_limits` test `endless_whitespace_reader_stops`
///   covers the bounded case).
pub struct OwnedSegmentStream<R: BufRead> {
    reader: R,
    ssa: crate::tokenizer::ServiceStringAdvice,
    state: StreamState,
    stream_offset: u64,
    config: ReaderConfig,
    /// Count and byte budgets with their running totals.
    budget: Budget,
    /// Whether the delimiters are already settled — because the caller supplied
    /// them, or because a `UNA` stated them.  While this is `false` the `UNB`'s
    /// own syntax version still gets to choose the repetition separator.
    delimiters_settled: bool,
}

impl<R: BufRead> OwnedSegmentStream<R> {
    fn new(reader: R) -> Self {
        Self::with_config(reader, ReaderConfig::default())
    }

    fn with_config(reader: R, config: ReaderConfig) -> Self {
        let (ssa, delimiters_settled) = match config.service_string_advice {
            Some(ssa) => (ssa, true),
            None => (crate::tokenizer::ServiceStringAdvice::default(), false),
        };
        Self {
            reader,
            ssa,
            state: StreamState::Init,
            stream_offset: 0,
            config,
            budget: Budget::new(&config),
            delimiters_settled,
        }
    }

    /// Adopt the repetition separator implied by a `UNB`'s syntax version.
    ///
    /// Only reached when nothing has already settled the delimiters: a `UNA`
    /// states all six explicitly, and a caller-supplied advice is by definition
    /// the last word.  Version 4 is the version that has a repetition separator
    /// at all, and its default is `*` (ISO 9735-1 §5.1).
    ///
    /// The `UNB` itself has no repeating data elements, so re-reading it under
    /// the updated advice would not change a thing.
    fn adopt_syntax_version(&mut self, unb: &OwnedSegment) {
        self.delimiters_settled = true;
        if unb.component_str(0, 1) == Some("4") {
            self.ssa.repetition_sep = b'*';
        }
    }

    /// Hand out a segment that has been read and parsed, after checking the
    /// count budgets against it.
    fn hand_out(&mut self, seg: OwnedSegment) -> Result<OwnedSegment, EdifactError> {
        self.budget.admit_segment(&seg.tag, seg.tag_span.start)?;
        if !self.delimiters_settled && seg.tag == "UNB" {
            self.adopt_syntax_version(&seg);
        }
        self.budget.account(&seg.tag);
        Ok(seg)
    }

    /// End the stream with `error`.
    fn fail(&mut self, error: EdifactError) -> Option<Result<OwnedSegment, EdifactError>> {
        self.state = StreamState::Done;
        Some(Err(error))
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
pub(crate) fn find_unescaped_term(buf: &[u8], term: u8, release: u8) -> Option<usize> {
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

/// Whether `b` is inter-segment whitespace — the set the tokenizer skips.
#[inline]
pub(crate) fn is_inter_segment_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

/// Try to parse one segment directly from the `BufRead` buffer.
///
/// The buffer is handled in two steps: **locate** the unit (leading
/// whitespace, the segment, its terminator) and check it against the size and
/// byte budgets, then **parse** it. A unit that ends beyond `max_input_bytes`
/// is refused before any of it is parsed, so the answer never depends on a
/// syntax error sitting in over-budget bytes.
///
/// This function borrows `reader` only for the duration of the call.  After it
/// returns the caller is free to call `reader.consume(n)`.
fn try_fast_segment<R: BufRead>(
    reader: &mut R,
    ssa: crate::tokenizer::ServiceStringAdvice,
    stream_offset: u64,
    max_segment_bytes: usize,
    budget: &Budget,
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

    // Saturate rather than wrap on 32-bit targets; Span offsets are `usize`
    // so streams > 4 GiB on 32-bit produce clamped (but monotonic) diagnostic
    // positions.
    let seg_start = stream_offset.min(usize::MAX as u64) as usize;

    // Enforce the segment-size guard *before* any allocation.
    // `pos` is the index of the terminator byte, so the segment body is `buf[..pos]`.
    if pos > max_segment_bytes {
        // The slow path and the slice tokenizer detect an oversized segment
        // once its first `max_segment_bytes + 1` bytes (from the tag) have
        // been read; when the byte budget ends before that point, the budget
        // is what the stream runs into first.
        let lead = buf[..pos]
            .iter()
            .take_while(|&&b| is_inter_segment_ws(b))
            .count() as u64;
        let detected_at = stream_offset
            .saturating_add(lead)
            .saturating_add(max_segment_bytes as u64)
            .saturating_add(1);
        if budget.sees_byte(detected_at) {
            return FastSegment::Err(EdifactError::SegmentTooLong {
                offset: seg_start,
                limit: max_segment_bytes,
            });
        }
    }

    // The byte budget, for the whole unit including its terminator.
    if let Err(error) = budget.admit_bytes(stream_offset.saturating_add(pos as u64 + 1)) {
        return FastSegment::Err(error);
    }

    // `buf[..pos]` is the segment content without the terminator.
    let seg_bytes = &buf[..pos];

    // Skip isolated terminators / pure-whitespace slots between segments.
    if seg_bytes.iter().all(|&b| is_inter_segment_ws(b)) {
        return FastSegment::Skip(pos + 1);
    }

    // Parse directly from the buffer slice — zero intermediate allocation.
    // Include the terminator byte so the parser sees a `SegmentTerminator`
    // token and records a span that is consistent with the `from_bytes` path.
    // `for_segment` (not `with_limit`) because this slice is one bare segment:
    // the whole-interchange constructors would mistake a segment tagged `UNA`
    // for a service string advice and skip nine bytes of it.  The configured
    // `max_segment_bytes` is passed through so the tokenizer never applies a
    // tighter cap than the caller allowed.
    let tok = Tokenizer::for_segment(&buf[..pos + 1], ssa, max_segment_bytes);
    let element_capacity = memchr::memchr_iter(ssa.element_sep, seg_bytes).count();
    let mut parser_iter = Parser::with_element_capacity(tok, element_capacity);
    match parser_iter.next() {
        None => FastSegment::Skip(pos + 1),
        Some(Err(e)) => FastSegment::Err(e),
        Some(Ok(s)) => FastSegment::Parsed(s.into_owned().offset(seg_start), pos + 1),
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

        loop {
            // ── Fast path (after UNA has been consumed) ───────────────────
            if self.state == StreamState::Running {
                match try_fast_segment(
                    &mut self.reader,
                    self.ssa,
                    self.stream_offset,
                    self.config.max_segment_bytes,
                    &self.budget,
                ) {
                    FastSegment::Parsed(seg, n) => {
                        self.reader.consume(n);
                        self.stream_offset += n as u64;
                        return match self.hand_out(seg) {
                            Ok(seg) => Some(Ok(seg)),
                            Err(error) => self.fail(error),
                        };
                    }
                    FastSegment::Skip(n) => {
                        self.reader.consume(n);
                        self.stream_offset += n as u64;
                        continue;
                    }
                    FastSegment::Eof => return None,
                    FastSegment::Err(e) => return self.fail(e),
                    FastSegment::NeedMore => {
                        // Segment spans buffer boundary — fall through to slow path.
                    }
                }
            }

            // ── Slow path: byte accumulation (also handles UNA header) ────
            if self.state == StreamState::Init {
                // Strip a byte order mark before anything tries to read a tag
                // out of it.  The slice path does the same (see
                // `tokenizer::prologue_len`), and the two must agree or the
                // identical file parses only from one of them.
                match self.reader.fill_buf() {
                    Ok(buf) if buf.starts_with(&crate::tokenizer::UTF8_BOM) => {
                        // The mark is consumed input like any other.
                        if let Err(error) = self.budget.admit_bytes(self.stream_offset + 3) {
                            return self.fail(error);
                        }
                        self.reader.consume(3);
                        self.stream_offset += 3;
                    }
                    Ok(_) => {}
                    Err(error) => return self.fail(error.into()),
                }
            }
            let mut scanned = self.state != StreamState::Init;
            // `read_next_raw_segment` tracks offset as `usize` for segment
            // start positions; sync back to the `u64` field afterward.
            // Saturate rather than wrap on 32-bit targets (same rationale as
            // the fast-path cast).
            let mut cursor = SlowCursor {
                offset: self.stream_offset.min(usize::MAX as u64) as usize,
                max_input_bytes: self.budget.max_input_bytes(),
            };
            let read = read_next_raw_segment(
                &mut self.reader,
                &mut self.ssa,
                &mut scanned,
                &mut cursor,
                self.config.max_segment_bytes,
                &mut self.delimiters_settled,
                self.config.service_string_advice.is_some(),
            );
            self.stream_offset = cursor.offset as u64;
            let mut raw = match read {
                Ok(Some(r)) => r,
                Ok(None) => return None,
                Err(e) => return self.fail(e),
            };
            if scanned {
                self.state = StreamState::Running;
            }

            raw.bytes.push(self.ssa.segment_term);
            // `for_segment`, and with the configured `max_segment_bytes`: this
            // buffer holds one bare segment, so neither the UNA skip nor the
            // 64 KiB default of `Tokenizer::new` applies.
            let tok = Tokenizer::for_segment(
                raw.bytes.as_slice(),
                self.ssa,
                self.config.max_segment_bytes,
            );
            let element_capacity =
                memchr::memchr_iter(self.ssa.element_sep, raw.bytes.as_slice()).count();
            let mut parser_iter = Parser::with_element_capacity(tok, element_capacity);
            match parser_iter.next() {
                Some(Ok(s)) => {
                    let seg = s.into_owned().offset(raw.start_offset);
                    return match self.hand_out(seg) {
                        Ok(seg) => Some(Ok(seg)),
                        Err(error) => self.fail(error),
                    };
                }
                Some(Err(e)) => return self.fail(e),
                None => {} // Empty segment — loop back.
            }
        }
    }
}

/// Parse an already-buffered reader into a lazy iterator of [`OwnedSegment`]s.
///
/// The same as [`from_reader`][crate::from_reader] without the internal
/// [`BufReader`] wrapper, for callers who already hold a [`BufRead`] and would
/// otherwise pay for a second layer of buffering.
///
/// # Resource bounds
///
/// `ReaderConfig::default()`: only `max_segment_bytes` (64 KiB) applies.
/// Memory is one segment plus `reader`'s buffer; time is unbounded on input
/// that never ends (for example endless whitespace). Use
/// [`from_bufread_with_config`] with `max_input_bytes` for untrusted input.
///
/// # Errors
///
/// Each `next()` yields a parse or I/O failure as `Some(Err(_))`.
pub fn from_bufread<R: BufRead>(reader: R) -> OwnedSegmentStream<R> {
    OwnedSegmentStream::new(reader)
}

/// [`from_bufread`] with explicit [`ReaderConfig`] limits.
///
/// # Resource bounds
///
/// Every limit in `config` applies (see [`ReaderConfig`]). Memory is one
/// segment of at most `max_segment_bytes` plus `reader`'s buffer; consumption
/// stops within one buffer of `max_input_bytes`.
pub fn from_bufread_with_config<R: BufRead>(
    reader: R,
    config: ReaderConfig,
) -> OwnedSegmentStream<R> {
    OwnedSegmentStream::with_config(reader, config)
}

/// Parse EDIFACT from an arbitrary reader as a streaming iterator.
///
/// # Resource bounds
///
/// As [`from_bufread`]: only `max_segment_bytes` (64 KiB) applies; memory is
/// one segment plus an 8 KiB read buffer.
pub fn from_reader_stream<R: Read>(reader: R) -> OwnedSegmentStream<BufReader<R>> {
    from_bufread(BufReader::new(reader))
}

/// Parse EDIFACT from an arbitrary reader as a streaming iterator with custom config.
///
/// # Resource bounds
///
/// Every limit in `config` applies (see [`ReaderConfig`]). Memory is one
/// segment of at most `max_segment_bytes` plus an 8 KiB read buffer; at most
/// `max_input_bytes` plus one buffer (8 KiB) is read from `reader`.
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
    from_bufread_with_config(BufReader::new(reader), config)
}

/// Position of the slow path, with the byte budget it may not read past.
struct SlowCursor {
    /// Absolute offset of the next byte to read.
    offset: usize,
    /// `max_input_bytes`: reading a byte at or beyond it is `E036`.
    max_input_bytes: Option<u64>,
}

#[allow(clippy::too_many_arguments)]
fn read_next_raw_segment<R: BufRead>(
    reader: &mut R,
    ssa: &mut crate::tokenizer::ServiceStringAdvice,
    scanned_header: &mut bool,
    cursor: &mut SlowCursor,
    max_segment_bytes: usize,
    delimiters_settled: &mut bool,
    advice_overridden: bool,
) -> Result<Option<crate::tokenizer::RawSegment>, EdifactError> {
    loop {
        let Some((first_offset, first)) = read_next_non_ws_byte(reader, cursor)? else {
            return Ok(None);
        };

        // A stray terminator is a unit of its own: it ends where it stands.
        // Reading on to the next terminator would swallow the following
        // segment into it, and count those bytes against the budget as part
        // of a unit the slice path never forms. Parsing the empty segment
        // reports the same `InvalidDelimiter` as before.
        if first == ssa.segment_term {
            *scanned_header = true;
            return Ok(Some(crate::tokenizer::RawSegment {
                bytes: Vec::new(),
                start_offset: first_offset,
            }));
        }

        if !*scanned_header && first == b'U' {
            // The `UNA` lookahead reads ahead of the size guard in
            // `read_remainder_of_segment`. When the byte budget stops it, a
            // segment already longer than `max_segment_bytes` is what the
            // stream ran into first, exactly as without the lookahead.
            let size_first = |error: EdifactError, read: usize| match error {
                EdifactError::LimitExceeded { .. } if read > max_segment_bytes => {
                    EdifactError::SegmentTooLong {
                        offset: first_offset,
                        limit: max_segment_bytes,
                    }
                }
                other => other,
            };
            // Read no further than the bytes still spell `UNA`: a byte that
            // does not may already be this segment's terminator.
            let second = read_required_byte(reader, cursor).map_err(|e| size_first(e, 1))?;
            if second != b'N' {
                *scanned_header = true;
                return finish_segment(
                    reader,
                    ssa,
                    &[first, second],
                    first_offset,
                    cursor,
                    max_segment_bytes,
                );
            }
            let third = read_required_byte(reader, cursor).map_err(|e| size_first(e, 2))?;
            if third == b'A' {
                let mut una = [0u8; 9];
                una[0] = b'U';
                una[1] = b'N';
                una[2] = b'A';
                for slot in una.iter_mut().skip(3) {
                    *slot = read_required_byte(reader, cursor)?;
                }
                let declared = crate::tokenizer::ServiceStringAdvice {
                    component_sep: una[3],
                    element_sep: una[4],
                    decimal_mark: una[5],
                    release_char: una[6],
                    repetition_sep: una[7],
                    segment_term: una[8],
                };
                if !declared.is_valid() {
                    return Err(EdifactError::InvalidUna);
                }
                // A malformed UNA is still an error when the caller supplied its
                // own advice — the input is broken either way — but the caller's
                // choice, not the header's, is what parsing then uses.
                if !advice_overridden {
                    *ssa = declared;
                }
                *delimiters_settled = true;
                *scanned_header = true;
                continue;
            }

            *scanned_header = true;
            return finish_segment(
                reader,
                ssa,
                &[first, second, third],
                first_offset,
                cursor,
                max_segment_bytes,
            );
        }

        *scanned_header = true;
        return finish_segment(
            reader,
            ssa,
            &[first],
            first_offset,
            cursor,
            max_segment_bytes,
        );
    }
}

/// Read the rest of a segment whose first bytes, `prefix`, are already
/// consumed.
///
/// The prefix goes through the same rules as every later byte — release
/// character, unescaped terminator, size guard — so the segment ends at the
/// same terminator the fast path's `find_unescaped_term` finds.
fn finish_segment<R: BufRead>(
    reader: &mut R,
    ssa: &crate::tokenizer::ServiceStringAdvice,
    prefix: &[u8],
    start_offset: usize,
    cursor: &mut SlowCursor,
    max_segment_bytes: usize,
) -> Result<Option<crate::tokenizer::RawSegment>, EdifactError> {
    let mut out = crate::tokenizer::RawSegment {
        bytes: Vec::with_capacity(prefix.len()),
        start_offset,
    };
    let mut escaped = false;
    for &byte in prefix {
        if out.bytes.len() > max_segment_bytes {
            return Err(EdifactError::SegmentTooLong {
                offset: out.start_offset,
                limit: max_segment_bytes,
            });
        }
        if !escaped && byte == ssa.segment_term {
            return Ok(Some(out));
        }
        escaped = !escaped && byte == ssa.release_char;
        out.bytes.push(byte);
    }
    read_remainder_of_segment(reader, ssa, out, escaped, cursor, max_segment_bytes)
}

fn read_remainder_of_segment<R: BufRead>(
    reader: &mut R,
    ssa: &crate::tokenizer::ServiceStringAdvice,
    mut out: crate::tokenizer::RawSegment,
    mut escaped: bool,
    cursor: &mut SlowCursor,
    max_segment_bytes: usize,
) -> Result<Option<crate::tokenizer::RawSegment>, EdifactError> {
    loop {
        // Strictly greater-than, matching the fast path (`pos > max_segment_bytes`)
        // and the tokenizer guard.  Using `>=` here made a segment of exactly
        // `max_segment_bytes` bytes parse on the fast path but fail on the slow
        // path, so identical input succeeded or failed depending only on whether
        // it happened to straddle a read-buffer boundary.
        if out.bytes.len() > max_segment_bytes {
            return Err(EdifactError::SegmentTooLong {
                offset: out.start_offset,
                limit: max_segment_bytes,
            });
        }
        let Some(byte) = read_next_byte(reader, cursor)? else {
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

/// Read one byte, or `None` at the end of the input.
///
/// This is where the slow path enforces `max_input_bytes`: a byte at or past
/// the budget that is actually there is `E036` (offset = the budget), while
/// input that ends exactly at the budget is a clean end. Every slow-path
/// reader — the whitespace loop, the `UNA` and the segment body — goes through
/// here, so no run of bytes can be consumed without being counted.
fn read_next_byte<R: BufRead>(
    reader: &mut R,
    cursor: &mut SlowCursor,
) -> Result<Option<u8>, EdifactError> {
    let buf = reader.fill_buf()?;
    if buf.is_empty() {
        return Ok(None);
    }
    if let Some(max) = cursor.max_input_bytes {
        if cursor.offset as u64 >= max {
            return Err(input_bytes_exceeded(max));
        }
    }

    let byte = buf[0];
    reader.consume(1);
    // Saturating add: on 32-bit targets the offset is a `usize` clamped from a
    // `u64` field.  Plain `+= 1` would wrap to 0 once the counter reaches
    // `usize::MAX` and corrupt subsequent span diagnostics.
    cursor.offset = cursor.offset.saturating_add(1);
    Ok(Some(byte))
}

fn read_required_byte<R: BufRead>(
    reader: &mut R,
    cursor: &mut SlowCursor,
) -> Result<u8, EdifactError> {
    read_next_byte(reader, cursor)?.ok_or(EdifactError::UnexpectedEof {
        offset: cursor.offset,
    })
}

fn read_next_non_ws_byte<R: BufRead>(
    reader: &mut R,
    cursor: &mut SlowCursor,
) -> Result<Option<(usize, u8)>, EdifactError> {
    loop {
        let current_offset = cursor.offset;
        let Some(byte) = read_next_byte(reader, cursor)? else {
            return Ok(None);
        };
        if !is_inter_segment_ws(byte) {
            return Ok(Some((current_offset, byte)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::ServiceStringAdvice;

    fn parse_all(input: &[u8]) -> Vec<Segment<'_>> {
        let ssa = ServiceStringAdvice::from_bytes_unchecked(input);
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
        let segments: Vec<_> =
            super::from_bufread(std::io::BufReader::new(std::io::Cursor::new(input)))
                .collect::<Result<_, _>>()
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
        let parsed = from_bufread(reader)
            .collect::<Result<Vec<_>, _>>()
            .expect("reader parsing should succeed");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].tag, "BGM");
        assert_eq!(parsed[0].elements[1].components[0].0, "test+value");
        assert_eq!(parsed[1].tag, "UNT");
    }

    #[test]
    fn from_reader_without_una_uses_default_delimiters() {
        let input = b"BGM+220+X'UNT+2+1'";
        let parsed = crate::from_reader(std::io::Cursor::new(input))
            .collect::<Result<Vec<_>, _>>()
            .expect("reader parsing should succeed");
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
        let err = crate::from_reader(std::io::Cursor::new(input))
            .collect::<Result<Vec<_>, _>>()
            .expect_err("expected dangling release from reader path");
        assert!(matches!(err, EdifactError::InvalidReleaseSequence { .. }));
    }

    #[test]
    fn a_segment_tagged_una_parses_the_same_on_both_paths() {
        // `UNA` is three ASCII uppercase letters, so it is a syntactically legal
        // segment tag.  The reader re-tokenizes each segment from its own slice,
        // where the whole-interchange "skip nine bytes of service string advice"
        // heuristic is wrong: it ate the tag and everything after it, and the
        // identical bytes that parsed cleanly through `from_bytes` came back as
        // `InvalidSegmentTag` through a reader.
        let input = b"BGM+220'UNA+XXXXXX'BGM+221'";

        let from_slice: Vec<_> = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("slice path");
        let from_reader = crate::from_reader(std::io::Cursor::new(&input[..]))
            .collect::<Result<Vec<_>, _>>()
            .expect("reader path must agree");

        assert_eq!(
            from_slice.iter().map(|s| s.tag()).collect::<Vec<_>>(),
            from_reader.iter().map(|s| s.tag()).collect::<Vec<_>>(),
        );
        assert_eq!(from_reader[1].element_str(0), Some("XXXXXX"));
    }

    #[test]
    fn the_reader_adopts_the_repetition_separator_from_the_unb_syntax_version() {
        // The slice path resolves this before tokenizing; the reader can only
        // learn it once the UNB has been parsed.  Both must agree.
        let input = b"UNB+UNOC:4+S+R+260101:0900+IC1'RFF+ON:1*ON:2'UNZ+0+IC1'";

        let from_slice: Vec<_> = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("slice path");
        let from_reader = crate::from_reader(std::io::Cursor::new(&input[..]))
            .collect::<Result<Vec<_>, _>>()
            .expect("reader path");

        assert_eq!(from_slice[1].get_element(0).unwrap().repeat_count(), 2);
        assert_eq!(from_reader[1].elements[0].repeat_count(), 2);
    }

    #[test]
    fn an_explicit_service_string_advice_parses_a_fragment_with_no_header() {
        // A message lifted out of an interchange carries neither UNA nor UNB, so
        // nothing in it records the delimiters — the caller has to supply them.
        let ssa = ServiceStringAdvice::from_bytes(b"UNA:;.? ~").expect("UNA");
        let config = ReaderConfig::default().with_service_string_advice(ssa);

        let from_slice: Vec<_> = crate::from_bytes_with_config(b"BGM;220;PO-4711~", config)
            .collect::<Result<Vec<_>, _>>()
            .expect("slice path");
        assert_eq!(from_slice[0].element_str(1), Some("PO-4711"));

        let from_reader: Vec<_> =
            from_reader_with_config(std::io::Cursor::new(b"BGM;220;PO-4711~"), config)
                .collect::<Result<Vec<_>, _>>()
                .expect("reader path");
        assert_eq!(from_reader[0].element_str(1), Some("PO-4711"));
    }

    #[test]
    fn an_explicit_service_string_advice_outranks_the_una_in_the_input() {
        let ssa = ServiceStringAdvice::default();
        let config = ReaderConfig::default().with_service_string_advice(ssa);
        // The UNA declares `;`, the caller insists on `+`.
        let input = b"UNA:;.? 'BGM+220'";

        for segments in [
            crate::from_bytes_with_config(input, config)
                .map(|r| r.map(crate::Segment::into_owned))
                .collect::<Result<Vec<_>, _>>()
                .expect("slice path"),
            from_reader_with_config(std::io::Cursor::new(&input[..]), config)
                .collect::<Result<Vec<_>, _>>()
                .expect("reader path"),
        ] {
            assert_eq!(segments[0].element_str(0).unwrap(), "220");
        }
    }

    /// The slice and reader paths must agree byte for byte: the same file has
    /// to parse — or fail — identically however it is handed in.
    #[test]
    fn both_parsing_paths_agree_on_every_edge_case() {
        fn bom(rest: &[u8]) -> Vec<u8> {
            let mut v = crate::tokenizer::UTF8_BOM.to_vec();
            v.extend_from_slice(rest);
            v
        }

        let cases: Vec<Vec<u8>> = vec![
            b"BGM+220".to_vec(),                           // truncated: no terminator
            b"BGM+220'".to_vec(),                          // the ordinary case
            b"BGM+220'\r\n".to_vec(),                      // trailing whitespace
            b"\n  UNA:+.? 'BGM+220'".to_vec(),             // UNA behind whitespace
            bom(b"UNA:+.? 'BGM+220'"),                     // UNA behind a BOM
            bom(b"UNB+UNOA:1+S+R+200101:0900+1'UNZ+0+1'"), // UNB behind a BOM
            b"UNB+UNOC:4+S+R+260101:0900+I'RFF+ON:1*ON:2'UNZ+0+I'".to_vec(),
            b"FTX+AAA++dangling?".to_vec(), // dangling release
            b"UNA::.? 'BGM:220'".to_vec(),  // malformed UNA
        ];

        for input in cases {
            let readable = String::from_utf8_lossy(&input).into_owned();
            let sliced: Result<Vec<OwnedSegment>, _> = crate::from_bytes(&input)
                .map(|r| r.map(|s| s.into_owned()))
                .collect();
            let streamed: Result<Vec<OwnedSegment>, _> =
                crate::from_reader(std::io::Cursor::new(input.clone())).collect();
            assert_eq!(
                format!("{sliced:?}"),
                format!("{streamed:?}"),
                "slice and reader paths disagree on {readable:?}",
            );
        }
    }

    #[test]
    fn a_segment_with_no_terminator_is_rejected_not_accepted() {
        // Accepting it would let a file cut off mid-transfer parse as a
        // complete interchange whose UNZ count happens to agree.
        for input in [
            &b"BGM+220"[..],
            &b"UNB+UNOA:1+S+R+200101:0900+1'UNZ+0+1"[..],
        ] {
            let err = crate::from_bytes(input)
                .collect::<Result<Vec<_>, _>>()
                .expect_err("an unterminated segment must not parse");
            assert!(
                matches!(err, EdifactError::UnexpectedEof { .. }),
                "expected UnexpectedEof for {:?}, got {err:?}",
                std::str::from_utf8(input).unwrap(),
            );
        }
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_segment() {
        let mut input = crate::tokenizer::UTF8_BOM.to_vec();
        input.extend_from_slice(b"UNB+UNOA:1+S+R+200101:0900+1'UNZ+0+1'");
        let segments: Vec<_> = crate::from_bytes(&input)
            .collect::<Result<Vec<_>, _>>()
            .expect("a BOM is prologue, not a segment tag");
        assert_eq!(
            segments.iter().map(Segment::tag).collect::<Vec<_>>(),
            ["UNB", "UNZ"],
        );
    }

    // ── Budget ───────────────────────────────────────────────────────────────

    fn limit_of(result: Result<(), EdifactError>) -> Option<(&'static str, u64, usize)> {
        match result {
            Ok(()) => None,
            Err(EdifactError::LimitExceeded {
                limit, max, offset, ..
            }) => Some((limit, max, offset)),
            Err(other) => panic!("unexpected {other:?}"),
        }
    }

    /// Admit and account each tag in turn, returning the first refusal.
    fn feed(budget: &mut Budget, tags: &[&str]) -> Option<(usize, (&'static str, u64, usize))> {
        for (index, tag) in tags.iter().enumerate() {
            if let Some(limit) = limit_of(budget.admit_segment(tag, index * 10)) {
                return Some((index, limit));
            }
            budget.account(tag);
        }
        None
    }

    #[test]
    fn budget_bytes_refuse_a_unit_ending_past_the_limit_at_the_limit() {
        let budget = Budget::new(&ReaderConfig::default().max_input_bytes(100));
        assert_eq!(
            limit_of(budget.admit_bytes(100)),
            None,
            "exactly at the limit"
        );
        assert_eq!(
            limit_of(budget.admit_bytes(101)),
            Some(("max_input_bytes", 100, 100))
        );
        assert!(budget.sees_byte(100));
        assert!(!budget.sees_byte(101));
        let unlimited = Budget::new(&ReaderConfig::default());
        assert_eq!(limit_of(unlimited.admit_bytes(u64::MAX)), None);
        assert!(unlimited.sees_byte(u64::MAX));
    }

    #[test]
    fn budget_max_segments_counts_every_segment() {
        let mut budget = Budget::new(&ReaderConfig::default().max_segments(2));
        assert_eq!(
            feed(&mut budget, &["UNB", "UNH", "BGM"]),
            Some((2, ("max_segments", 2, 20)))
        );
    }

    #[test]
    fn budget_max_messages_trips_only_on_a_unh() {
        let mut budget = Budget::new(&ReaderConfig::default().max_messages(1));
        // The trailer after a message exactly at the budget is fine.
        assert_eq!(
            feed(&mut budget, &["UNB", "UNH", "BGM", "UNT", "UNZ"]),
            None
        );
        assert_eq!(
            feed(&mut budget, &["UNH"]),
            Some((0, ("max_messages", 1, 0)))
        );
    }

    #[test]
    fn budget_a_bare_unt_does_not_count_as_a_message() {
        let mut budget = Budget::new(&ReaderConfig::default().max_messages(1));
        assert_eq!(feed(&mut budget, &["UNT", "UNT", "UNH", "UNT"]), None);
    }

    #[test]
    fn budget_per_message_counts_unh_through_unt_inclusive() {
        let config = ReaderConfig::default().max_segments_per_message(3);
        // Exactly three: UNH, BGM, UNT.
        let mut budget = Budget::new(&config);
        assert_eq!(feed(&mut budget, &["UNH", "BGM", "UNT"]), None);
        // Four: the UNT is the fourth segment of the message.
        let mut budget = Budget::new(&config);
        assert_eq!(
            feed(&mut budget, &["UNH", "BGM", "DTM", "UNT"]),
            Some((3, ("max_segments_per_message", 3, 30)))
        );
    }

    #[test]
    fn budget_per_message_ignores_segments_outside_a_message() {
        let mut budget = Budget::new(&ReaderConfig::default().max_segments_per_message(2));
        // UNT closes the message; what follows belongs to no message.
        assert_eq!(
            feed(
                &mut budget,
                &["UNB", "UNG", "UNH", "UNT", "UNE", "UNZ", "XXX", "YYY"]
            ),
            None
        );
    }

    #[test]
    fn budget_a_unh_inside_an_open_message_restarts_the_count() {
        let mut budget = Budget::new(&ReaderConfig::default().max_segments_per_message(2));
        // Without the restart the second UNH would be the third segment.
        assert_eq!(feed(&mut budget, &["UNH", "BGM", "UNH", "BGM"]), None);
        assert_eq!(
            feed(&mut budget, &["DTM"]),
            Some((0, ("max_segments_per_message", 2, 0)))
        );
    }

    #[test]
    fn budget_checks_count_limits_in_a_fixed_order() {
        let config = ReaderConfig::default()
            .max_segments(1)
            .max_messages(0)
            .max_segments_per_message(0);
        let budget = Budget::new(&config);
        // All three would trip on a first UNH; `max_segments` is not yet
        // reached, so `max_messages` is reported before the per-message cap.
        assert_eq!(
            limit_of(budget.admit_segment("UNH", 7)),
            Some(("max_messages", 0, 7))
        );
        let config = ReaderConfig::default().max_segments(0).max_messages(0);
        assert_eq!(
            limit_of(Budget::new(&config).admit_segment("UNH", 7)),
            Some(("max_segments", 0, 7))
        );
    }

    #[test]
    fn from_reader_rejects_invalid_una() {
        let input = b"UNA::.? 'BGM:220'";
        let err = crate::from_reader(std::io::Cursor::new(input))
            .collect::<Result<Vec<_>, _>>()
            .expect_err("invalid UNA should fail reader parsing");
        assert!(matches!(err, EdifactError::InvalidUna));
    }
}
