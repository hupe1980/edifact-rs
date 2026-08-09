//! EDIFACT tokenizer — splits raw bytes into typed tokens.
//!
//! Respects UNA service string advice for non-default delimiters.
//! Uses `memchr` for fast delimiter scanning (no byte-by-byte inner loops).

use crate::{error::EdifactError, model::Span};
use memchr::{memchr, memchr2, memchr3};

/// EDIFACT service string advice — the six characters of the `UNA`
/// (ISO 9735-1 Annex B).
///
/// The five *active* service characters — component separator, element
/// separator, release character, repetition separator, and segment terminator —
/// are what [`is_valid`][Self::is_valid] enforces: printable non-alphanumeric
/// ASCII, mutually distinct, so a collision between the repetition separator and
/// any other delimiter, or a delimiter that would clash with segment-tag
/// characters, is caught at UNA parse time.
///
/// The decimal mark is deliberately **not** in that set; see
/// [`decimal_mark`][Self::decimal_mark].
///
/// # Defaults
///
/// ISO 9735-1 §5.1 fixes the defaults as `:` (component), `+` (element), `?`
/// (release), `*` (repetition), `'` (terminator). Syntax version 4 is the
/// version that defines the repetition separator at all: in versions 1–3 that
/// UNA position is reserved and carries a space. [`Default`] is therefore the
/// version-agnostic reading — everything per §5.1 **except** repetition, which
/// stays inactive until something says the interchange is version 4. See
/// [`for_syntax_version`][Self::for_syntax_version].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceStringAdvice {
    /// Data element separator (default `+`; `UNA` position 020)
    pub element_sep: u8,
    /// Component data element separator (default `:`; `UNA` position 010)
    pub component_sep: u8,
    /// Release character (default `?`; `UNA` position 040)
    pub release_char: u8,
    /// Decimal mark (`UNA` position 030), default `.`.
    ///
    /// **Ignored on receipt.** ISO 9735-1 Annex B keeps this position only for
    /// upward compatibility with earlier syntax versions and states that the
    /// character transferred here "shall be ignored by the recipient"; §10
    /// instead allows the full stop *or* the comma per individual numeric value.
    /// It is therefore neither validated nor used for splitting — it is
    /// preserved so a writer can round-trip the `UNA` it was given, and so
    /// [`DecimalFloat`][crate::ser::DecimalFloat] has a house style to format
    /// with.
    pub decimal_mark: u8,
    /// Repetition separator (`UNA` position 050), introduced by syntax version 4.
    ///
    /// A space (`0x20`) means **not used**: that is what versions 1–3 put in this
    /// reserved position, and version 4 forbids a space here precisely because
    /// the position now carries a real separator.
    ///
    /// When the separator is active the tokenizer splits on it: a data element
    /// carrying `ON:1*ON:2` becomes one element with two repetitions rather than
    /// one repetition whose second component is the literal text `1*ON`.  Use
    /// [`is_repetition_active`][Self::is_repetition_active] to test for this.
    pub repetition_sep: u8,
    /// Segment terminator (default `'`; `UNA` position 060)
    pub segment_term: u8,
}

impl Default for ServiceStringAdvice {
    fn default() -> Self {
        Self {
            element_sep: b'+',
            component_sep: b':',
            release_char: b'?',
            decimal_mark: b'.',
            // Inactive until the interchange is known to be syntax version 4 —
            // see `for_syntax_version`.  Defaulting to the §5.1 asterisk would
            // split every unescaped `*` in a version 3 interchange, where `*` is
            // an ordinary level A character and not a service character at all.
            repetition_sep: b' ',
            segment_term: b'\'',
        }
    }
}

impl ServiceStringAdvice {
    /// Read the service characters an interchange actually uses.
    ///
    /// Two sources, in priority order:
    ///
    /// 1. A leading `UNA`, which states all six characters explicitly.
    /// 2. Otherwise the ISO 9735-1 §5.1 defaults, with the repetition separator
    ///    resolved from the syntax version in `UNB` S001 DE 0002 — see
    ///    [`for_syntax_version`][Self::for_syntax_version].
    ///
    /// # Errors
    ///
    /// [`EdifactError::InvalidUna`] when a `UNA` is present but its active
    /// service characters are not mutually distinct printable non-alphanumeric
    /// ASCII.  See [`is_valid`][Self::is_valid] for the exact rule.
    ///
    /// This is the **safe, default constructor** — always use this for input from
    /// an external source.  For trusted or internal use where delimiter uniqueness
    /// is already guaranteed, use [`from_bytes_unchecked`](Self::from_bytes_unchecked).
    pub fn from_bytes(input: &[u8]) -> Result<Self, crate::error::EdifactError> {
        let ssa = Self::from_bytes_unchecked(input);
        if !ssa.is_valid() {
            return Err(crate::error::EdifactError::InvalidUna);
        }
        Ok(ssa)
    }

    /// Parse a UNA header from the beginning of an EDIFACT interchange **without**
    /// validating delimiter uniqueness or printability.
    ///
    /// When no `UNA` is present the §5.1 defaults apply, with the repetition
    /// separator taken from the syntax version declared in `UNB` S001 DE 0002.
    ///
    /// # When to use
    ///
    /// Use this only for trusted internal data (e.g. round-tripping data where
    /// the UNA invariant is already guaranteed) or in fuzz/property tests that
    /// intentionally explore degenerate delimiter combinations.
    ///
    /// For any external or user-provided input, prefer [`from_bytes`](Self::from_bytes)
    /// which validates delimiter uniqueness and rejects invalid bytes.
    pub fn from_bytes_unchecked(input: &[u8]) -> Self {
        // UNA is 9 bytes: "UNA" + 6 service chars
        if input.len() >= 9 && &input[..3] == b"UNA" {
            Self {
                component_sep: input[3],
                element_sep: input[4],
                decimal_mark: input[5],
                release_char: input[6],
                repetition_sep: input[7],
                segment_term: input[8],
            }
        } else {
            Self::for_syntax_version(sniff_syntax_version(input))
        }
    }

    /// The ISO 9735-1 §5.1 defaults for a given syntax version.
    ///
    /// The repetition separator is the only character the version decides:
    /// version 4 introduced it as `*`, and versions 1–3 have no such service
    /// character at all — that `UNA` position is reserved and carries a space.
    /// Splitting on `*` in a version 3 interchange would corrupt every value
    /// containing one, since `*` is an ordinary level A character there.
    ///
    /// `None` means the version could not be determined (no `UNB`, or an
    /// unreadable one) and is treated as "not version 4".
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::ServiceStringAdvice;
    ///
    /// assert!(ServiceStringAdvice::for_syntax_version(Some(4)).is_repetition_active());
    /// assert!(!ServiceStringAdvice::for_syntax_version(Some(3)).is_repetition_active());
    /// assert!(!ServiceStringAdvice::for_syntax_version(None).is_repetition_active());
    /// ```
    #[must_use]
    pub const fn for_syntax_version(version: Option<u8>) -> Self {
        Self {
            element_sep: b'+',
            component_sep: b':',
            release_char: b'?',
            decimal_mark: b'.',
            repetition_sep: match version {
                Some(4) => b'*',
                _ => b' ',
            },
            segment_term: b'\'',
        }
    }

    /// Return `true` if all **active** service characters are mutually distinct
    /// and printable, non-alphanumeric ASCII.
    ///
    /// The active set is the component separator, element separator, release
    /// character, segment terminator, and — when it is not the space "not used"
    /// sentinel — the repetition separator.  Each must be in `0x21..=0x7E`
    /// excluding `0-9A-Za-z`, and all must differ pairwise.
    ///
    /// Alphanumerics are excluded because segment tags are always three ASCII
    /// uppercase letters written verbatim (a tag cannot be escaped).  A delimiter
    /// such as `N` would make `NAD` unrepresentable — the writer would emit a
    /// premature terminator and the result would not reparse.  High bytes
    /// (`>= 0x80`) are rejected because they would bisect multi-byte UTF-8
    /// sequences, and DEL (`0x7F`) is a control character.
    ///
    /// The **decimal mark is not checked at all**: ISO 9735-1 Annex B states that
    /// the character in that position "shall be ignored by the recipient", and is
    /// the one position where the standard permits a space.  Rejecting a `UNA`
    /// over a character the standard tells receivers to ignore would fail
    /// conformant interchanges for nothing.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::ServiceStringAdvice;
    ///
    /// // A duplicated *active* character is fatal …
    /// assert!(ServiceStringAdvice::from_bytes(b"UNA::.? '").is_err());
    /// // … but the ignored decimal-mark slot may hold anything, even a space.
    /// assert!(ServiceStringAdvice::from_bytes(b"UNA:+ ? '")?.is_repetition_active() == false);
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    pub fn is_valid(&self) -> bool {
        let printable_ascii = |b: u8| (0x21..=0x7E).contains(&b) && !b.is_ascii_alphanumeric();
        // The decimal mark is ignored on receipt, so the only requirement is
        // that it stay a single graphic ASCII byte — Annex B types it `an1`, and
        // space is explicitly permitted in this one position.
        if !(0x20..=0x7E).contains(&self.decimal_mark) {
            return false;
        }
        let active: [u8; 5] = [
            self.component_sep,
            self.element_sep,
            self.release_char,
            self.segment_term,
            self.repetition_sep,
        ];
        // The repetition separator occupies the last slot and drops out of both
        // checks when it holds the "not used" space.
        let active = &active[..if self.is_repetition_active() { 5 } else { 4 }];
        active.iter().all(|&b| printable_ascii(b))
            && (0..active.len()).all(|i| active[i + 1..].iter().all(|&other| active[i] != other))
    }

    /// Returns `true` when this interchange declares a usable repetition
    /// separator (`UNA` position 050, syntax version 4).
    ///
    /// A space there means "not used" — the reserved value carried by syntax
    /// versions 1–3 — so it reports `false` and the tokenizer never splits on it.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::ServiceStringAdvice;
    ///
    /// assert!(!ServiceStringAdvice::default().is_repetition_active());
    /// assert!(ServiceStringAdvice::from_bytes(b"UNA:+.?*'")?.is_repetition_active());
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[inline]
    #[must_use]
    pub const fn is_repetition_active(&self) -> bool {
        self.repetition_sep != b' '
    }
}

/// Read the syntax version number (`UNB` S001 DE 0002) out of raw bytes.
///
/// Deliberately byte-level and deliberately tiny: this runs *before* the
/// delimiters are settled, so it can only assume what ISO 9735-1 §6 guarantees —
/// that everything up to and including S001 is ISO/IEC 646 — and the §5.1
/// default separators, which are the only ones in play when no `UNA` said
/// otherwise.
///
/// Returns `None` for input with no readable `UNB` S001.
fn sniff_syntax_version(input: &[u8]) -> Option<u8> {
    let mut pos = 0;
    while pos < input.len() && matches!(input[pos], b' ' | b'\t' | b'\r' | b'\n') {
        pos += 1;
    }
    // `UNB+` — the element separator is the §5.1 default, because a UNA that
    // changed it would have been used instead of this function.
    if input.len() < pos + 4 || &input[pos..pos + 3] != b"UNB" || input[pos + 3] != b'+' {
        return None;
    }
    // S001 = `<identifier>:<version>[:…]`; the version is component 2.
    let s001 = &input[pos + 4..];
    let end = s001
        .iter()
        .position(|&b| b == b'+' || b == b'\'')
        .unwrap_or(s001.len());
    let mut components = s001[..end].split(|&b| b == b':');
    let _identifier = components.next()?;
    match components.next()? {
        [digit @ b'1'..=b'9'] => Some(digit - b'0'),
        _ => None,
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
    /// First component of a further repetition of the current data element
    /// (ISO 9735-1 §8.6).
    ///
    /// Only produced when the active [`ServiceStringAdvice`] declares a
    /// repetition separator — see
    /// [`is_repetition_active`][ServiceStringAdvice::is_repetition_active].
    RepeatElement {
        /// Raw value of the repetition's first component.
        value: &'a str,
        /// Source span of the value.
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
    ///
    /// Only correct for a slice that starts at the head of an interchange.
    /// A slice holding a single already-delimited segment must use
    /// [`Tokenizer::for_segment`], because `UNA` is also a syntactically valid
    /// segment tag and skipping nine bytes of it corrupts the parse.
    #[inline]
    fn una_start_pos(input: &[u8]) -> usize {
        if input.len() >= 9 && &input[..3] == b"UNA" {
            9
        } else {
            0
        }
    }

    /// Construct a tokenizer over a slice that holds **one already-delimited
    /// segment**, with no interchange header to skip.
    ///
    /// The whole-interchange constructors treat a leading `b"UNA"` as the
    /// service string advice and jump nine bytes past it.  The reader paths
    /// re-tokenize each segment from its own slice, where that heuristic is
    /// wrong: `UNA` is three ASCII uppercase letters and therefore a legal
    /// segment tag, so `UNA+XXXXXX'` parsed cleanly from a byte slice but was
    /// rejected as `InvalidSegmentTag` when the identical bytes arrived through
    /// a reader.
    #[must_use]
    pub fn for_segment(
        input: &'a [u8],
        ssa: ServiceStringAdvice,
        max_segment_bytes: usize,
    ) -> Self {
        Self {
            input,
            pos: 0,
            ssa,
            state: TokState::ExpectTag,
            max_segment_bytes,
            segment_start: 0,
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
    /// # Security warning
    ///
    /// This constructor imposes **no upper bound** on how many bytes a single
    /// segment may consume.  For untrusted or adversarially crafted input a
    /// missing segment terminator can cause the tokenizer to scan the entire
    /// input before returning an error.  Prefer [`Tokenizer::new`] (64 KiB
    /// limit) or [`Tokenizer::with_limit`] for untrusted sources.
    #[must_use]
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
    /// Offset of the next segment terminator — or repetition separator, when the
    /// interchange declares one — at or after `from`, searching within `window`.
    ///
    /// `memchr` tops out at three needles and `read_value` already spends those
    /// on the element separator, component separator, and release character, so
    /// the remaining one or two needles are searched separately and cached.
    #[inline]
    fn find_stop(&self, window: &[u8]) -> Option<usize> {
        if self.ssa.is_repetition_active() {
            memchr2(self.ssa.segment_term, self.ssa.repetition_sep, window)
        } else {
            memchr(self.ssa.segment_term, window)
        }
    }

    fn read_value(&mut self) -> Result<(&'a str, Span), EdifactError> {
        let start = self.pos;
        let (elem, comp, release) = (
            self.ssa.element_sep,
            self.ssa.component_sep,
            self.ssa.release_char,
        );
        // Absolute cap on how far this value may extend before the per-segment
        // byte guard trips.  Bounding the scan window here (rather than only
        // checking the length after the loop) keeps adversarial input that omits
        // every delimiter from forcing a scan across the whole remaining input.
        let scan_end = self
            .segment_start
            .saturating_add(self.max_segment_bytes)
            .saturating_add(1)
            .min(self.input.len());

        // Absolute offset of the next stop byte (segment terminator, plus the
        // repetition separator when active) at or after the current search
        // origin.  `memchr3` below rescans only the bytes it actually consumes,
        // but a naive re-search per iteration would rescan the whole tail on
        // every release sequence, making a value such as `?a?a?a…` quadratic.
        // Caching the hit keeps this search amortised linear: each rescan starts
        // past the previous hit, so the scanned regions are disjoint.
        let mut stop_hit = self
            .find_stop(&self.input[self.pos..scan_end])
            .map(|i| self.pos + i);

        loop {
            if self.pos >= scan_end {
                break;
            }
            let remaining = &self.input[self.pos..scan_end];
            // Refresh the cached stop position once the cursor has moved past it
            // (only happens when a release sequence escaped a stop byte).
            if stop_hit.is_some_and(|t| t < self.pos) {
                stop_hit = self.find_stop(remaining).map(|i| self.pos + i);
            }
            let hit_ect = memchr3(elem, comp, release, remaining);
            let hit_stop = stop_hit.map(|t| t - self.pos);
            let hit = match (hit_ect, hit_stop) {
                (None, None) => {
                    self.pos = scan_end;
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
                if self.pos + hit + 1 >= self.input.len() {
                    return Err(EdifactError::InvalidReleaseSequence {
                        offset: self.pos + hit,
                    });
                }
                // Skip release char + the escaped byte.
                self.pos += hit + 2;
                continue;
            }
            // b is elem, comp, rep, or term — end of value.
            self.pos += hit;
            break;
        }
        // The size guard is checked *before* UTF-8 validation.  `scan_end` can
        // cut a multi-byte sequence in half, and reporting that as `InvalidText`
        // blamed the payload for what is really an oversized segment.
        if self.pos - self.segment_start > self.max_segment_bytes {
            return Err(EdifactError::SegmentTooLong {
                offset: self.segment_start,
                limit: self.max_segment_bytes,
            });
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
        // A segment tag is terminated by the element separator or segment terminator.
        // Bound the scan to max_segment_bytes + 1 so adversarial input with no delimiters
        // cannot force memchr to scan arbitrarily large buffers before we return an error.
        let input_remaining = &self.input[self.pos..];
        let scan_limit = self
            .max_segment_bytes
            .saturating_add(1)
            .min(input_remaining.len());
        let remaining = &input_remaining[..scan_limit];
        // Take the *nearest* of the two terminating delimiters.  Searching for
        // the element separator first and only falling back to the segment
        // terminator would run straight past the terminator of an element-less
        // segment (`UNZ'…`) and swallow the following segment's tag.
        let end = memchr2(self.ssa.element_sep, self.ssa.segment_term, remaining)
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
                    } else if self.ssa.is_repetition_active() && b == self.ssa.repetition_sep {
                        self.pos += 1;
                        let (value, span) = match self.read_value() {
                            Ok(value) => value,
                            Err(error) => return Some(Err(error)),
                        };
                        return Some(Ok(Token::RepeatElement { value, span }));
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
        let ssa = ServiceStringAdvice::from_bytes_unchecked(input);
        Tokenizer::new(input, ssa)
            .collect::<Result<Vec<_>, _>>()
            .expect("tokenize failed")
    }

    #[test]
    fn syntax_version_4_activates_the_default_repetition_separator() {
        // ISO 9735-1 §5.1: `*` is the default repetition separator, and version
        // 4 is the version that has one.  Without a UNA, the only thing that can
        // say so is UNB S001 DE 0002.
        let v4 = ServiceStringAdvice::from_bytes(b"UNB+UNOC:4+S+R+260101:0900+IC1'").unwrap();
        assert!(v4.is_repetition_active());
        assert_eq!(v4.repetition_sep, b'*');

        let v3 = ServiceStringAdvice::from_bytes(b"UNB+UNOC:3+S+R+260101:0900+IC1'").unwrap();
        assert!(!v3.is_repetition_active());

        // No UNB at all — a bare message — stays conservative.
        let fragment = ServiceStringAdvice::from_bytes(b"BGM+220'").unwrap();
        assert!(!fragment.is_repetition_active());
    }

    #[test]
    fn a_una_overrides_the_syntax_version_default() {
        // The UNA states all six characters explicitly, so a version 4
        // interchange that declares the "not used" space really means it.
        let input = b"UNA:+.? 'UNB+UNOC:4+S+R+260101:0900+IC1'";
        let ssa = ServiceStringAdvice::from_bytes(input).unwrap();
        assert!(!ssa.is_repetition_active());
    }

    #[test]
    fn version_4_repetitions_parse_without_a_una() {
        let input = b"UNB+UNOC:4+S+R+260101:0900+IC1'RFF+ON:1*ON:2'UNZ+0+IC1'";
        let segments: Vec<_> = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let rff = segments[1].get_element(0).unwrap();
        assert_eq!(rff.repeat_count(), 2);
        assert_eq!(rff.repetition(1).unwrap()[1].0, "2");
    }

    #[test]
    fn a_version_3_asterisk_stays_data() {
        // `*` is an ordinary level A character in syntax version 3; splitting on
        // it would corrupt the value.
        let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'FTX+AAA+2*3'UNZ+0+IC1'";
        let segments: Vec<_> = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(segments[1].element_str(1), Some("2*3"));
    }

    #[test]
    fn the_ignored_decimal_mark_slot_never_invalidates_a_una() {
        // Annex B: the character in position 030 "shall be ignored by the
        // recipient", and it is the one position where a space is allowed.
        for una in [&b"UNA:+ ? '"[..], &b"UNA:+,? '"[..], &b"UNA:+:? '"[..]] {
            assert!(
                ServiceStringAdvice::from_bytes(una).is_ok(),
                "{:?} must parse",
                std::str::from_utf8(una).unwrap()
            );
        }
        // An *active* character duplicated is still fatal.
        assert!(ServiceStringAdvice::from_bytes(b"UNA:+.: '").is_err());
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
    fn element_less_segment_does_not_swallow_the_next_tag() {
        // `read_tag` must stop at the *nearest* of element-separator and
        // segment-terminator.  Scanning for `+` first would run past the `'`
        // and produce the bogus tag "UNZ'UNB".
        let segs: Vec<_> = crate::from_bytes(b"UNZ'UNB+A'")
            .collect::<Result<Vec<_>, _>>()
            .expect("element-less segment must parse");
        assert_eq!(
            segs.iter().map(|s| s.tag).collect::<Vec<_>>(),
            vec!["UNZ", "UNB"]
        );
        assert!(segs[0].elements.is_empty());
    }

    #[test]
    fn release_heavy_value_is_bounded_by_the_segment_guard() {
        // A value consisting solely of release sequences and no delimiter must
        // trip the per-segment guard rather than scanning the whole input once
        // per release sequence (which was quadratic).
        let mut input = b"BGM+".to_vec();
        input.extend(std::iter::repeat_n(b"?a".as_slice(), 200_000).flatten());
        let err = crate::from_bytes(&input)
            .collect::<Result<Vec<_>, _>>()
            .expect_err("oversized segment must be rejected");
        assert!(
            matches!(err, EdifactError::SegmentTooLong { .. }),
            "expected SegmentTooLong, got {err:?}"
        );
    }

    #[test]
    fn an_oversized_segment_is_reported_as_such_even_with_multi_byte_text() {
        // The scan window can cut a multi-byte sequence in half.  Validating
        // UTF-8 before the size guard blamed the payload (`InvalidText`) for
        // what is really an oversized segment, sending the reader hunting for an
        // encoding problem that does not exist.
        let mut input = b"BGM+".to_vec();
        input.extend(std::iter::repeat_n("ä".as_bytes(), 200_000).flatten());
        let err = crate::from_bytes(&input)
            .collect::<Result<Vec<_>, _>>()
            .expect_err("oversized segment must be rejected");
        assert!(
            matches!(err, EdifactError::SegmentTooLong { .. }),
            "expected SegmentTooLong, got {err:?}"
        );
    }

    #[test]
    fn multi_byte_text_within_the_limit_still_parses() {
        let segs: Vec<_> = crate::from_bytes("FTX+Grüße aus Köln'".as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("valid UTF-8 must parse");
        assert_eq!(segs[0].element_str(0), Some("Grüße aus Köln"));
    }

    #[test]
    fn escaped_terminator_inside_a_value_is_not_a_segment_break() {
        // Exercises the cached-terminator refresh path: the first `'` is escaped,
        // so the scan must resume past it and find the real terminator.
        let segs: Vec<_> = crate::from_bytes(b"FTX+a?'b+c'")
            .collect::<Result<Vec<_>, _>>()
            .expect("escaped terminator must parse");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].element_str(0), Some("a'b"));
        assert_eq!(segs[0].element_str(1), Some("c"));
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
