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

    /// Parse a UNA header and validate that the four active service characters
    /// (`element_sep`, `component_sep`, `release_char`, `segment_term`) are all
    /// mutually distinct and are not ASCII whitespace (`CR`, `LF`, space, tab).
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

    /// Return `true` if the four active service characters are mutually distinct
    /// and none is ASCII whitespace (`CR`, `LF`, space, tab).
    pub fn is_valid(&self) -> bool {
        let [e, c, r, t] = [
            self.element_sep,
            self.component_sep,
            self.release_char,
            self.segment_term,
        ];
        let no_ws = |b: u8| !matches!(b, b' ' | b'\t' | b'\r' | b'\n');
        // All must be non-whitespace and mutually distinct (6 pairwise checks).
        no_ws(e) && no_ws(c) && no_ws(r) && no_ws(t)
            && e != c && e != r && e != t
            && c != r && c != t
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
/// Yields [`Token`] values, each borrowing from the original input.
pub struct Tokenizer<'a> {
    input: &'a [u8],
    pos: usize,
    ssa: ServiceStringAdvice,
    state: TokState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokState {
    /// Expecting a segment tag next
    ExpectTag,
    /// Inside a segment; next byte could be element or component sep, release, or terminator
    InSegment,
}

impl<'a> Tokenizer<'a> {
    /// Construct a zero-copy tokenizer over `input` with explicit service-string advice.
    pub fn new(input: &'a [u8], ssa: ServiceStringAdvice) -> Self {
        // Skip past the UNA segment if present
        let pos = if input.len() >= 9 && &input[..3] == b"UNA" {
            9
        } else {
            0
        };
        Self {
            input,
            pos,
            ssa,
            state: TokState::ExpectTag,
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
        Ok((value, span))
    }

    /// Fast scan for the segment tag (exactly 3 ASCII uppercase letters).
    fn read_tag(&mut self) -> Result<Option<Token<'a>>, EdifactError> {
        self.skip_inter_segment_whitespace();
        if self.pos >= self.input.len() {
            return Ok(None);
        }
        let start = self.pos;
        // A segment tag is terminated by the element separator, segment terminator, or CR/LF.
        // Use memchr for the element sep; fall back to a short scan (tags are ≤ 6 bytes).
        let remaining = &self.input[self.pos..];
        let end = memchr(self.ssa.element_sep, remaining)
            .or_else(|| memchr(self.ssa.segment_term, remaining))
            .unwrap_or(remaining.len());

        if end == 0 {
            // First byte is already a delimiter — tag is zero-length, which is invalid.
            let byte = self.input[self.pos];
            self.pos += 1;
            return Err(EdifactError::InvalidDelimiter { byte, offset: start });
        }

        let tag_bytes = &self.input[start..start + end];
        // Always advance pos so errors cannot cause an infinite retry loop.
        self.pos = start + end;
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
        let raw_val = bgm.elements.get(1).and_then(|e| e.components.first()).map(|s| s.as_str());
        assert_eq!(raw_val, Some("test+value"));
    }
}
