//! EDIFACT tokenizer — splits raw bytes into typed tokens.
//!
//! Respects UNA service string advice for non-default delimiters.
//! Uses `memchr` for fast delimiter scanning (no byte-by-byte inner loops).

use crate::{error::EdifactError, model::Span};
use memchr::{memchr, memchr3};

/// EDIFACT service string advice (UNA segment).
///
/// Defaults: `+` (element), `:` (component), `?` (release), space (reserved), `'` (segment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceStringAdvice {
    /// Data element separator (default `+`)
    pub element_sep: u8,
    /// Component data element separator (default `:`)
    pub component_sep: u8,
    /// Release character (default `?`)
    pub release_char: u8,
    /// Decimal notation mark (default `.`; UNA byte 5, ISO 9735-1 §7.1).
    /// Not used by the tokenizer for splitting, but preserved for downstream use.
    pub decimal_mark: u8,
    /// Segment terminator (default `'`)
    pub segment_term: u8,
}

impl Default for ServiceStringAdvice {
    fn default() -> Self {
        Self {
            element_sep: b'+',
            component_sep: b':',
            release_char: b'?',
            decimal_mark: b'.',
            segment_term: b'\'',
        }
    }
}

impl ServiceStringAdvice {
    /// Parse a UNA header from the beginning of an EDIFACT interchange.
    ///
    /// If no UNA is present, returns [`ServiceStringAdvice::default`].
    /// Does not validate that the 6 service characters are mutually distinct;
    /// use [`ServiceStringAdvice::from_bytes_strict`] when that matters.
    pub fn from_bytes(input: &[u8]) -> Self {
        // UNA is 9 bytes: "UNA" + 6 service chars
        if input.len() >= 9 && &input[..3] == b"UNA" {
            Self {
                component_sep: input[3],
                element_sep: input[4],
                decimal_mark: input[5],
                release_char: input[6],
                // input[7] = repetition separator (ISO 9735-4 §3.1; not modelled here)
                segment_term: input[8],
            }
        } else {
            Self::default()
        }
    }

    /// Parse a UNA header and validate that the five active service characters
    /// (`element_sep`, `component_sep`, `decimal_mark`, `release_char`, `segment_term`) are all
    /// mutually distinct and in the printable ASCII range `0x21–0x7E`.
    ///
    /// Returns [`EdifactError::InvalidUna`] if the invariant is violated.
    /// Falls back to [`ServiceStringAdvice::default`] when no UNA is present.
    pub fn from_bytes_strict(input: &[u8]) -> Result<Self, crate::error::EdifactError> {
        let ssa = Self::from_bytes(input);
        if !ssa.is_valid() {
            return Err(crate::error::EdifactError::InvalidUna);
        }
        Ok(ssa)
    }

    /// Return `true` if all five active service characters are mutually distinct
    /// and all fall in the printable ASCII range `0x21–0x7E` (excl. space `0x20`,
    /// control characters `0x00–0x1F`, and `DEL 0x7F`).
    ///
    /// The five characters are `element_sep`, `component_sep`, `decimal_mark`,
    /// `release_char`, and `segment_term`.  All 10 pairwise combinations are
    /// checked.
    ///
    /// Bytes outside `0x21–0x7E` are rejected: high-bytes (`>= 0x80`) would cause
    /// incorrect single-byte tokenization of multi-byte UTF-8 sequences, and DEL
    /// (`0x7F`) is a non-printable control character.
    pub fn is_valid(&self) -> bool {
        let [e, c, d, r, t] = [
            self.element_sep,
            self.component_sep,
            self.decimal_mark,
            self.release_char,
            self.segment_term,
        ];
        // All five must be printable ASCII 0x21–0x7E (excludes high-bytes, control chars,
        // whitespace, and DEL 0x7F) and mutually distinct (10 pairwise checks).
        let printable_ascii = |b: u8| b >= 0x21 && b <= 0x7E;
        printable_ascii(e)
            && printable_ascii(c)
            && printable_ascii(d)
            && printable_ascii(r)
            && printable_ascii(t)
            && e != c
            && e != d
            && e != r
            && e != t
            && c != d
            && c != r
            && c != t
            && d != r
            && d != t
            && r != t
    }
}

/// Token produced by [`Tokenizer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token<'a> {
    /// 3-character segment tag (e.g. `"BGM"`)
    SegmentTag {
        /// Raw tag value.
        value: &'a str,
        /// Source span of the tag.
        span: Span,
    },
    /// Data element value (between element separators)
    DataElement {
        /// Raw element value.
        value: &'a str,
        /// Source span of the element value.
        span: Span,
    },
    /// Component within a composite data element (between component separators)
    ComponentElement {
        /// Raw component value.
        value: &'a str,
        /// Source span of the component value.
        span: Span,
    },
    /// Segment terminator — signals the end of a segment
    SegmentTerminator {
        /// Source span of the segment terminator byte.
        span: Span,
    },
}

#[derive(Debug)]
pub(crate) struct RawSegment {
    pub(crate) bytes: Vec<u8>,
    pub(crate) start_offset: usize,
}

/// Zero-copy tokenizer over a byte slice.
///
/// Yields `Token` values, each borrowing from the original input.
///
/// # Segment size guard
///
/// The default constructor [`Tokenizer::new`] enforces a **64 KiB** per-segment
/// limit, which is sufficient for all well-formed EDIFACT interchanges and guards
/// against adversarially crafted inputs that omit segment terminators.
/// Use [`Tokenizer::with_limit`] to raise or lower this threshold, or
/// [`Tokenizer::unlimited`] to remove it entirely (trusted / pre-validated input only).
pub struct Tokenizer<'a> {
    input: &'a [u8],
    pos: usize,
    ssa: ServiceStringAdvice,
    state: TokState,
    /// Maximum allowed segment byte length (tag + elements, **excluding** the
    /// segment terminator byte itself).  Checked in `read_value` and `read_tag`.
    /// `usize::MAX` = unlimited.
    max_segment_bytes: usize,
    /// Byte position where the current segment started (set in `read_tag`).
    segment_start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokState {
    /// Expecting a segment tag next
    ExpectTag,
    /// Inside a segment; next byte could be element or component sep, release, or terminator
    InSegment,
}

impl<'a> Tokenizer<'a> {
    /// Return the byte offset of the first non-UNA byte in `input`.
    ///
    /// If the input starts with the `UNA` service string advice (first 3
    /// bytes are `b"UNA"`), the UNA header is exactly 9 bytes long and the
    /// first segment tag starts at offset 9.  Otherwise parsing starts at 0.
    #[inline]
    fn una_start_pos(input: &[u8]) -> usize {
        if input.len() >= 9 && &input[..3] == b"UNA" {
            9
        } else {
            0
        }
    }

    /// Construct a tokenizer with the default 64 KiB segment-size limit.
    ///
    /// If a single segment's byte length exceeds 65 536 bytes, the iterator
    /// returns [`EdifactError::SegmentTooLong`].  This guards against
    /// pathological or adversarially crafted inputs that omit segment
    /// terminators and would otherwise cause unbounded scanning.
    ///
    /// Call [`Tokenizer::unlimited`] if you deliberately need to process
    /// segments larger than 64 KiB, or [`Tokenizer::with_limit`] to supply a
    /// custom bound.
    pub fn new(input: &'a [u8], ssa: ServiceStringAdvice) -> Self {
        Self::with_limit(input, ssa, 65_536)
    }

    /// Construct a tokenizer with **no** segment-size limit.
    ///
    /// # Security
    ///
    /// This constructor imposes **no upper bound** on how many bytes a single
    /// segment may consume.  For untrusted or adversarially crafted input a
    /// missing segment terminator can cause the tokenizer to scan the entire
    /// input before returning an error.  Prefer [`Tokenizer::new`] (64 KiB
    /// limit) or [`Tokenizer::with_limit`] for untrusted sources.
    pub fn unlimited(input: &'a [u8], ssa: ServiceStringAdvice) -> Self {
        Self {
            input,
            pos: Self::una_start_pos(input),
            ssa,
            state: TokState::ExpectTag,
            max_segment_bytes: usize::MAX,
            segment_start: 0,
        }
    }

    /// Construct a tokenizer with a segment-size limit.
    ///
    /// If a single segment's byte length (from the start of the tag to the end
    /// of the last value, not including the terminator itself) exceeds `limit`,
    /// the iterator returns [`EdifactError::SegmentTooLong`].
    ///
    /// # Examples
    ///
    /// ```
    /// use edifact_rs::{ServiceStringAdvice, Tokenizer};
    ///
    /// let input = b"BGM+220+PO-4711+9'";
    /// let ssa = ServiceStringAdvice::default();
    /// let tokens: Vec<_> = Tokenizer::with_limit(input, ssa, 64)
    ///     .collect::<Result<_, _>>()
    ///     .unwrap();
    /// assert!(!tokens.is_empty());
    /// ```
    pub fn with_limit(input: &'a [u8], ssa: ServiceStringAdvice, max_segment_bytes: usize) -> Self {
        Self {
            input,
            pos: Self::una_start_pos(input),
            ssa,
            state: TokState::ExpectTag,
            max_segment_bytes,
            segment_start: 0,
        }
    }

    /// Current byte position in the input.
    #[inline]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Return the service string advice active for this tokenizer.
    #[inline]
    pub fn service_string_advice(&self) -> ServiceStringAdvice {
        self.ssa
    }

    /// Consume leading whitespace / CR / LF between segments (not inside data values).
    fn skip_inter_segment_whitespace(&mut self) {
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    /// Read a field value starting at `self.pos`, advancing past the value.
    ///
    /// Recognises the release character (`?` by default) and returns the raw
    /// slice including release sequences. The parser layer resolves them.
    ///
    /// Uses `memchr3` to bulk-scan over non-special bytes between hits, only
    /// falling back to a per-byte step when a release character is encountered.
    fn read_value(&mut self) -> Result<(&'a str, Span), EdifactError> {
        let start = self.pos;
        let (elem, comp, release, term) = (
            self.ssa.element_sep,
            self.ssa.component_sep,
            self.ssa.release_char,
            self.ssa.segment_term,
        );
        loop {
            let remaining = &self.input[self.pos..];
            if remaining.is_empty() {
                break;
            }
            // Scan for release OR a value-terminating delimiter.
            // memchr3 can hold three bytes; we combine elem/comp/release.
            // A separate memchr finds term so we take the nearest hit.
            let hit_ect = memchr3(elem, comp, release, remaining);
            let hit_term = memchr(term, remaining);
            let hit = match (hit_ect, hit_term) {
                (None, None) => {
                    self.pos += remaining.len();
                    break;
                }
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (Some(a), Some(b)) => a.min(b),
            };
            let b = remaining[hit];
            if b == release {
                // A release char must be followed by exactly one escaped byte.
                // If it is the last byte in the buffer the sequence is malformed.
                if remaining.len() - hit == 1 {
                    return Err(EdifactError::InvalidReleaseSequence {
                        offset: self.pos + hit,
                    });
                }
                // Skip release char + the escaped byte.
                self.pos += hit + 2;
                continue;
            }
            // b is elem, comp, or term — end of value.
            self.pos += hit;
            break;
        }
        let span = Span::new(start, self.pos);
        let value = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| EdifactError::InvalidText { offset: start })?;
        // Enforce the per-segment byte-length guard.
        if self.pos - self.segment_start > self.max_segment_bytes {
            return Err(EdifactError::SegmentTooLong {
                offset: self.segment_start,
                limit: self.max_segment_bytes,
            });
        }
        Ok((value, span))
    }

    /// Fast scan for the segment tag (exactly 3 ASCII uppercase letters).
    fn read_tag(&mut self) -> Result<Option<Token<'a>>, EdifactError> {
        self.skip_inter_segment_whitespace();
        if self.pos >= self.input.len() {
            return Ok(None);
        }
        let start = self.pos;
        // A segment tag is terminated by the element separator or segment terminator.
        // Bound the scan to max_segment_bytes + 1 so adversarial input with no delimiters
        // cannot force memchr to scan arbitrarily large buffers before we return an error.
        let input_remaining = &self.input[self.pos..];
        let scan_limit = self
            .max_segment_bytes
            .saturating_add(1)
            .min(input_remaining.len());
        let remaining = &input_remaining[..scan_limit];
        let end = memchr(self.ssa.element_sep, remaining)
            .or_else(|| memchr(self.ssa.segment_term, remaining))
            .unwrap_or(remaining.len());

        if end == 0 {
            // First byte is already a delimiter — tag is zero-length, which is invalid.
            let byte = self.input[self.pos];
            self.pos += 1;
            return Err(EdifactError::InvalidDelimiter {
                byte,
                offset: start,
            });
        }

        // Enforce the per-segment byte-length guard in read_tag as well.
        // Without this check, adversarial input with no delimiters could cause
        // memchr to scan the entire remaining buffer (potentially hundreds of MB).
        if end > self.max_segment_bytes {
            // Advance past the offending bytes so the iterator can continue.
            self.pos = start + end;
            return Err(EdifactError::SegmentTooLong {
                offset: start,
                limit: self.max_segment_bytes,
            });
        }
        let tag_bytes = &self.input[start..start + end];
        // Always advance pos so errors cannot cause an infinite retry loop.
        self.pos = start + end;
        // Record segment start for the size-limit check in read_value.
        self.segment_start = start;
        let tag = std::str::from_utf8(tag_bytes)
            .map_err(|_| EdifactError::InvalidSegmentTag(format!("{tag_bytes:?}")))?;
        if tag.len() != 3 || !tag.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(EdifactError::InvalidSegmentTag(tag.to_owned()));
        }
        self.state = TokState::InSegment;
        Ok(Some(Token::SegmentTag {
            value: tag,
            span: Span::new(start, start + end),
        }))
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Result<Token<'a>, EdifactError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.pos >= self.input.len() {
                return None;
            }

            match self.state {
                TokState::ExpectTag => {
                    return match self.read_tag() {
                        Ok(Some(tok)) => Some(Ok(tok)),
                        Ok(None) => None,
                        Err(e) => Some(Err(e)),
                    };
                }
                TokState::InSegment => {
                    let b = self.input[self.pos];
                    let (elem, comp, term) = (
                        self.ssa.element_sep,
                        self.ssa.component_sep,
                        self.ssa.segment_term,
                    );

                    if b == term {
                        let start = self.pos;
                        self.pos += 1;
                        self.state = TokState::ExpectTag;
                        return Some(Ok(Token::SegmentTerminator {
                            span: Span::new(start, self.pos),
                        }));
                    } else if b == elem {
                        self.pos += 1;
                        let (value, span) = match self.read_value() {
                            Ok(value) => value,
                            Err(error) => return Some(Err(error)),
                        };
                        // Peek: is the *next* byte a component sep?
                        // We emit DataElement for the leading sub-element regardless;
                        // subsequent components within the same element are ComponentElement.
                        return Some(Ok(Token::DataElement { value, span }));
                    } else if b == comp {
                        self.pos += 1;
                        let (value, span) = match self.read_value() {
                            Ok(value) => value,
                            Err(error) => return Some(Err(error)),
                        };
                        return Some(Ok(Token::ComponentElement { value, span }));
                    } else if b == b'\r' || b == b'\n' {
                        self.pos += 1;
                        // inter-element whitespace inside a segment — skip
                        continue;
                    } else {
                        // Unexpected byte inside a segment — skip it and report.
                        let offset = self.pos;
                        self.pos += 1; // always advance to prevent infinite retry loop
                        self.state = TokState::ExpectTag;
                        return Some(Err(EdifactError::InvalidDelimiter { byte: b, offset }));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: &[u8]) -> Vec<Token<'_>> {
        let ssa = ServiceStringAdvice::from_bytes(input);
        Tokenizer::new(input, ssa)
            .collect::<Result<Vec<_>, _>>()
            .expect("tokenize failed")
    }

    #[test]
    fn minimal_unb_unz() {
        let input = b"UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'UNZ+0+1'";
        let toks = tokens(input);
        assert!(matches!(toks[0], Token::SegmentTag { value: "UNB", .. }));
        // should end with UNZ terminator
        assert!(matches!(toks.last(), Some(Token::SegmentTerminator { .. })));
    }

    #[test]
    fn release_character_not_a_delimiter() {
        // `?+` inside a value must NOT produce a DataElement split
        let input = b"BGM+220+test?+value'";
        let toks = tokens(input);
        // Elements after BGM tag: "220", "test?+value"
        let vals: Vec<_> = toks
            .iter()
            .filter_map(|t| {
                if let Token::DataElement { value, .. } = t {
                    Some(*value)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(vals, vec!["220", "test?+value"]);
    }

    #[test]
    fn custom_una_delimiters() {
        // UNA with `;` as element sep
        let input = b"UNA:;.? 'BGM;220;hello'";
        let toks = tokens(input);
        assert!(matches!(toks[0], Token::SegmentTag { value: "BGM", .. }));
        let vals: Vec<_> = toks
            .iter()
            .filter_map(|t| {
                if let Token::DataElement { value, .. } = t {
                    Some(*value)
                } else {
                    None
                }
            })
            .collect();
        assert!(vals.contains(&"220"));
    }

    #[test]
    fn tokens_expose_spans() {
        let input = b"BGM+220+ABC'";
        let toks = tokens(input);
        assert!(matches!(
            toks[0],
            Token::SegmentTag {
                value: "BGM",
                span: Span { start: 0, end: 3 }
            }
        ));
        assert!(matches!(
            toks[1],
            Token::DataElement {
                value: "220",
                span: Span { start: 4, end: 7 }
            }
        ));
    }

    #[test]
    fn truncated_input_does_not_panic() {
        let input = b"UNB+UNOA:1"; // no terminator
        let _: Vec<_> = Tokenizer::new(input, ServiceStringAdvice::default()).collect();
        // must not panic regardless of result
    }

    #[test]
    fn invalid_segment_tags_are_rejected() {
        for input in [
            &b"bgm+220+'"[..],
            &b"ABCDE+220+'"[..],
            &b"BGM1+220+'"[..],
            &b"BGM +220+'"[..],
            &b" BG+220+'"[..],
        ] {
            let result = Tokenizer::new(input, ServiceStringAdvice::default())
                .collect::<Result<Vec<_>, _>>();
            assert!(result.is_err(), "expected tag rejection for {input:?}");
        }
    }

    #[test]
    fn chunked_reader_parses_via_parser() {
        // The reader tokenizer path was removed; verify the equivalent via the parser.
        let input = b"UNA:+.? 'BGM+220+test?+value'UNT+2+1'";
        let segments =
            crate::parser::from_bufread(std::io::BufReader::new(std::io::Cursor::new(input)))
                .expect("parser should succeed");
        assert!(segments.iter().any(|s| s.tag == "BGM"));
        // The release sequence '?+' inside 'test?+value' should survive in the element.
        let bgm = segments.iter().find(|s| s.tag == "BGM").unwrap();
        let raw_val = bgm
            .elements
            .get(1)
            .and_then(|e| e.components.first())
            .map(|(s, _)| s.as_str());
        assert_eq!(raw_val, Some("test+value"));
    }
}
