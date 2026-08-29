use crate::model::Span;
use thiserror::Error;

/// Wrapper around [`std::io::Error`] that implements [`PartialEq`] by comparing [`std::io::ErrorKind`].
///
/// This allows `EdifactError` to derive `PartialEq` without requiring `std::io::Error: PartialEq`.
#[derive(Debug)]
pub struct IoError(pub(crate) std::io::Error);

impl IoError {
    /// Returns a reference to the underlying [`std::io::Error`].
    pub fn inner(&self) -> &std::io::Error {
        &self.0
    }
}

impl PartialEq for IoError {
    /// Equality is determined by [`std::io::ErrorKind`] only.
    ///
    /// Two `IoError` values with the same kind but different OS-level error codes
    /// (or different messages) will compare as equal.  This is a deliberate
    /// limitation: `std::io::Error` is not `PartialEq`, so kind-based comparison
    /// is the only practical option that lets `EdifactError` derive `PartialEq`.
    fn eq(&self, other: &Self) -> bool {
        self.0.kind() == other.0.kind()
    }
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for IoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<std::io::Error> for IoError {
    fn from(e: std::io::Error) -> Self {
        Self(e)
    }
}

/// Which rule of ISO 9735-1 §9.1 a value violates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Insignificant {
    /// A variable-length numeric value carries leading zeroes.
    ///
    /// "Nevertheless, a single zero before a decimal mark is allowed", so `0.5`
    /// is correct and `00.5` is not.
    LeadingZeroes,
    /// A variable-length alphabetic or alphanumeric value carries trailing
    /// spaces.
    TrailingSpaces,
}

impl std::fmt::Display for Insignificant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::LeadingZeroes => "leading zeroes are not suppressed",
            Self::TrailingSpaces => "trailing spaces are not suppressed",
        })
    }
}

/// All errors produced by `edifact-rs`.
///
/// # Positional data
///
/// Two kinds of position information appear in this enum, and the distinction is
/// deliberate:
///
/// * **`offset: usize`** — a single byte position in the input stream.  Used by
///   the lexical variants (`UnexpectedEof`, `InvalidDelimiter`, `InvalidText`,
///   `InvalidReleaseSequence`, `SegmentTooLong`, `UnexpectedDataToken`), where
///   the fault is a *point* in the byte stream and no meaningful end position
///   exists.
/// * **`span: Span`** — a half-open byte range.  Used by every variant produced
///   while validating an already-parsed [`Segment`][crate::Segment], where the
///   exact source range is known.  A [`ValidationIssue`][crate::ValidationIssue]
///   built from such an error carries the full range, so `miette` and LSP
///   tooling can underline the offending segment rather than place a
///   zero-width caret.
///
/// Use `span.start` when only the start position is needed.
#[derive(Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum EdifactError {
    /// Unexpected end of input while parsing.
    ///
    /// This typically occurs when a segment terminator or expected delimiter
    /// is not found before the end of the input stream.
    #[error("unexpected end of input at byte offset {offset}")]
    UnexpectedEof {
        /// Byte offset where the parser exhausted input.
        offset: usize,
    },

    /// Invalid byte encountered in a delimiter context.
    ///
    /// Delimiters must be precisely ASCII characters from the UNA service string advice.
    /// Any other byte is invalid in delimiter position.
    #[error("invalid delimiter byte 0x{byte:02X} at offset {offset}")]
    InvalidDelimiter {
        /// Unexpected delimiter byte.
        byte: u8,
        /// Byte offset where the delimiter was observed.
        offset: usize,
    },

    /// Invalid UTF-8 sequence in parsed text.
    ///
    /// While EDIFACT operates on bytes, segments and elements are expected to contain
    /// valid UTF-8 text. Non-UTF-8 sequences are rejected at parse time.
    #[error("invalid EDIFACT text at byte offset {offset}")]
    InvalidText {
        /// Byte offset where invalid UTF-8 text starts.
        offset: usize,
    },

    /// Invalid release-character escape sequence in parsed text.
    ///
    /// The release character (`?` by default) must be followed by one escaped byte.
    /// A trailing release character without a following byte is malformed.
    #[error("invalid release sequence at byte offset {offset}: dangling release character")]
    InvalidReleaseSequence {
        /// Byte offset of the dangling release character.
        offset: usize,
    },

    /// UNZ interchange message count does not match the number of UNH/UNT pairs found.
    ///
    /// The `UNZ` segment declares the number of messages in the interchange,
    /// but the actual number of `UNH`/`UNT` pairs observed differs.
    #[error("interchange message count mismatch: UNZ declared {expected}, found {actual}")]
    MessageCountMismatch {
        /// Message count declared in the UNZ segment.
        expected: u32,
        /// Actual number of UNH/UNT pairs observed.
        actual: u32,
    },

    /// UNT segment count does not match the actual number of segments in the message.
    ///
    /// The `UNT` segment declares the number of segments in the message (including `UNH`/`UNT`),
    /// but the actual count differs.
    #[error(
        "segment count mismatch in message {message_ref}: UNT declared {expected}, found {actual}"
    )]
    SegmentCountMismatch {
        /// Segment count declared in the UNT segment.
        expected: u32,
        /// Actual number of segments observed.
        actual: u32,
        /// Message reference from the UNH segment.
        message_ref: String,
        /// Byte range of the offending `UNT`.
        ///
        /// This is what places the finding on the *message* rather than on the
        /// interchange — a `CONTRL` built from the report reports it on that
        /// message's `UCM`, and a rendered diagnostic underlines the trailer
        /// that got the count wrong.
        span: Span,
    },

    /// Invalid or malformed segment tag.
    ///
    /// Segment tags must be exactly 3 ASCII uppercase letters.
    #[error("invalid segment tag {0:?}")]
    InvalidSegmentTag(String),

    /// Invalid UNA service string advice.
    ///
    /// If present, the UNA segment must be exactly 9 bytes: `"UNA"` followed by
    /// 6 service characters.  The **active** ones — component separator, element
    /// separator, release character, segment terminator, and the repetition
    /// separator when it is not the "not used" space — must all be mutually
    /// distinct and printable, non-alphanumeric ASCII.
    ///
    /// The **decimal mark is not checked**: ISO 9735-1 Annex B says the character
    /// in that position "shall be ignored by the recipient", and it is the one
    /// position where the standard permits a space.
    #[error("invalid UNA service string advice")]
    InvalidUna,

    /// Missing required element in a segment.
    ///
    /// Certain segments require specific elements to be present. This error indicates
    /// a mandatory element was not found.
    #[error("missing required element {element_index} in segment {tag}")]
    MissingRequiredElement {
        /// Segment tag containing the missing element.
        tag: String,
        /// Zero-based required element index.
        element_index: usize,
    },

    /// Missing required component in a composite element.
    ///
    /// The element is present, but the required component at the given index is absent or empty.
    #[error(
        "missing required component {component_index} in element {element_index} of segment {tag}"
    )]
    MissingRequiredComponent {
        /// Segment tag containing the composite element.
        tag: String,
        /// Zero-based element index of the composite.
        element_index: usize,
        /// Zero-based component index that was absent.
        component_index: usize,
    },

    /// Output serialization produced invalid UTF-8.
    ///
    /// This is an internal consistency error; the writer should never produce non-UTF-8 output.
    /// If this occurs, it indicates a bug in the serialization logic.
    #[error("serialized output contains invalid UTF-8")]
    InvalidUtf8,

    /// I/O error from reading or writing.
    #[error(transparent)]
    Io(#[from] IoError),

    // ── validation variants (E010–E020) ────────────────────────────────────
    /// Segment is not valid for the current message type.
    ///
    /// Structural validation found a segment that should not appear in this message.
    #[error("segment {tag} is not valid for message type {message_type}")]
    InvalidSegmentForMessage {
        /// Segment tag that is not allowed for the message type.
        tag: String,
        /// Message type used for structural validation.
        message_type: String,
        /// Byte range of the offending segment tag.
        span: Span,
    },

    /// Element count in segment exceeds or falls short of directory definition.
    ///
    /// Validation against directory metadata found an element count mismatch.
    #[error("segment {tag} has {actual} elements, expected between {min} and {max}")]
    InvalidElementCount {
        /// Segment tag with wrong arity.
        tag: String,
        /// Minimum allowed element count.
        min: usize,
        /// Maximum allowed element count.
        max: usize,
        /// Actual element count found.
        actual: usize,
        /// Byte range of the offending segment.
        span: Span,
    },

    /// Component count in a composite element is invalid.
    ///
    /// A composite data element does not have the expected number of components.
    #[error("segment {tag} element {element_index} has {actual} components, expected {expected}")]
    InvalidComponentCount {
        /// Segment tag containing the composite.
        tag: String,
        /// Zero-based element index of the composite.
        element_index: usize,
        /// Expected component count.
        expected: u8,
        /// Actual component count found.
        actual: u8,
        /// Byte range of the offending composite element.
        span: Span,
    },

    /// Code-list value is not valid.
    ///
    /// The value appears in a field that should contain a code from a specific code list,
    /// but the value is not in that code list.
    #[error(
        "segment {tag} element {element_index}: '{value}' is not a valid code (code list {code_list})"
    )]
    InvalidCodeValue {
        /// Segment tag containing the invalid value.
        tag: String,
        /// Zero-based element index containing the invalid code.
        element_index: usize,
        /// Invalid code value observed.
        value: String,
        /// Data element code list identifier.
        code_list: String,
        /// Byte range of the offending value.
        span: Span,
        /// Optional remediation suggestion from the code-list lookup function.
        suggestion: Option<&'static str>,
    },

    /// A required segment is missing from the message.
    ///
    /// Structural validation found that a mandatory segment is absent.
    #[error("required segment {tag} is missing from message (position {expected_position})")]
    MissingSegment {
        /// Missing segment tag.
        tag: String,
        /// Human-readable position hint.
        expected_position: String,
    },

    /// Qualifier does not match expected value for segment.
    ///
    /// A qualified segment (e.g., NAD+MS) has a qualifier that does not match expected.
    #[error("segment {tag} has qualifier '{actual}', expected '{expected}'")]
    QualifierMismatch {
        /// Segment tag whose qualifier mismatched.
        tag: String,
        /// Actual qualifier found.
        actual: String,
        /// Expected qualifier value.
        expected: String,
        /// Byte range of the offending segment.
        span: Span,
    },

    /// Conditional requirement not met.
    ///
    /// A segment or element is conditionally required based on another element's value,
    /// but the condition was not satisfied.
    #[error("segment {tag} element {element_index}: conditional requirement not met ({condition})")]
    ConditionalRequirementNotMet {
        /// Segment tag that violated a conditional rule.
        tag: String,
        /// Zero-based element index governed by the condition.
        element_index: usize,
        /// Condition text describing the rule.
        condition: String,
        /// Byte range of the offending segment.
        span: Span,
    },

    /// Validation failed and the full [`ValidationReport`] is preserved.
    ///
    /// Returned by validation helpers when errors are found.  Provides programmatic
    /// access to all issues, warnings, and infos.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// match my_fn() {
    ///     Err(EdifactError::ValidationErrors { report, .. }) => {
    ///         for issue in report.errors() {
    ///             eprintln!("{}", issue);
    ///         }
    ///     }
    ///     other => { /* ... */ }
    /// }
    /// ```
    #[error("validation failed with {error_count} error(s)")]
    ValidationErrors {
        /// Number of error-severity issues in the report.
        error_count: usize,
        /// Full report with all errors, warnings, and infos.
        report: Box<ValidationReport>,
    },

    /// Segment exceeded the configured maximum byte length.
    ///
    /// Returned by reader-based parsers when an unterminated segment accumulates more
    /// bytes than the configured `max_segment_bytes` limit in [`ReaderConfig`].  This
    /// prevents resource exhaustion on adversarially crafted or truncated input that
    /// never emits a segment terminator.
    ///
    /// [`ReaderConfig`]: crate::ReaderConfig
    #[error("segment starting at byte offset {offset} exceeded maximum length of {limit} bytes")]
    SegmentTooLong {
        /// Byte offset where the overlong segment started.
        offset: usize,
        /// Configured maximum segment byte length.
        limit: usize,
    },

    /// An interchange or message contains more segments or messages than can be
    /// represented in a `u32` counter (> 4 294 967 295).
    ///
    /// This is effectively unreachable in practice — no real-world EDIFACT
    /// interchange has billions of segments — but the parser returns this error
    /// rather than silently saturating or wrapping the counter.
    #[error("interchange too large: count {count} exceeds u32::MAX")]
    InterchangeTooLarge {
        /// The count that could not be represented as `u32`.
        count: u64,
    },

    /// An [`crate::EventEmitter`] received events in an invalid sequence.
    ///
    /// This indicates a programming error in the caller's serialization code:
    /// for example, emitting an [`crate::EdifactEvent::Element`] without a prior
    /// [`crate::EdifactEvent::StartSegment`], or emitting
    /// [`crate::EdifactEvent::ComponentElement`] without a preceding
    /// [`crate::EdifactEvent::Element`].
    #[error("invalid event sequence: {message}")]
    InvalidEventSequence {
        /// Description of the protocol violation.
        message: &'static str,
    },

    /// An [`crate::OwnedElementRef`] has `position = 0`, which is never valid.
    ///
    /// Element positions are one-based: position 1 refers to the first element
    /// slot.  Position 0 is reserved and invalid.  Use [`crate::OwnedElementRef::try_new`]
    /// to get a `Result` instead of a panic.
    #[error("element definition contains invalid position 0; positions must be >= 1 (one-based)")]
    InvalidElementPosition,

    /// Two [`crate::ProfileRulePack`] values with incompatible release scopes were composed.
    ///
    /// When composing packs via [`crate::ProfileRulePack::extend_from`] or
    /// [`crate::ProfileRulePack::merge_with_override`], both packs must either
    /// share the same release scope or at most one may carry a scope.
    #[error("incompatible release scopes: cannot compose {current:?} with {incoming:?}")]
    IncompatibleReleaseScopes {
        /// Release scope of the pack being composed into.
        current: String,
        /// Release scope of the pack being composed in.
        incoming: String,
    },

    /// A field value failed semantic validation (e.g. wrong format, out-of-range).
    ///
    /// Distinct from [`InvalidCodeValue`][Self::InvalidCodeValue] which is for
    /// code-list membership checks.  Use this variant when a free-text or numeric
    /// field contains a value that is structurally invalid for its purpose.
    #[error("segment {tag} element {element_index}: invalid field value {value:?}")]
    InvalidFieldValue {
        /// Segment tag that contains the invalid field.
        tag: String,
        /// Zero-based element index of the invalid field.
        element_index: usize,
        /// The invalid value that was observed.
        value: String,
    },

    /// A data or component element token appeared before the first segment tag.
    ///
    /// EDIFACT syntax requires that every data element follows a segment tag.
    /// A data element token encountered before any tag (e.g. after a stray
    /// separator at the start of the stream) is a protocol violation.
    ///
    /// Unlike stray segment terminators (which are tolerated as blank lines),
    /// stray data tokens indicate encoding corruption or a partial write.
    #[error("unexpected data token at byte offset {offset}: data element before segment tag")]
    UnexpectedDataToken {
        /// Byte offset of the stray token.
        offset: usize,
    },

    /// The interchange syntax identifier (`UNB` S001 DE 0001) names no defined
    /// character repertoire.
    ///
    /// DE 0001 is `UN` followed by a two-character repertoire code, so the
    /// defined values are `UNOA` through `UNOK`, `UNOX`, `UNOY`, and `KECA`
    /// (Korean EDI Centre A).  Anything else is a non-standard generator or a
    /// corrupted `UNB`.
    ///
    /// A value that *is* defined but that this crate cannot decode is
    /// [`UnsupportedCharset`][Self::UnsupportedCharset] instead — the two say
    /// different things about whose problem it is.
    #[error("unrecognised syntax identifier '{0}': expected UNOA-UNOK, UNOX, UNOY, or KECA")]
    UnrecognisedSyntaxIdentifier(String),

    /// A control reference was reused within the scope that requires it to be unique.
    ///
    /// ISO 9735-1 requires the message reference number (`UNH` DE 0062) to be
    /// unique within an interchange, and the group reference number (`UNG`
    /// DE 0048) to be unique within an interchange.  Duplicates make a message
    /// unaddressable: a receiver keying on the reference silently processes one
    /// occurrence and drops the rest.
    #[error("duplicate {tag} reference '{reference}' at bytes {span}")]
    DuplicateReference {
        /// Segment tag that carries the duplicated reference (`UNH` or `UNG`).
        tag: String,
        /// The reference value that appeared more than once.
        reference: String,
        /// Byte range of the duplicate occurrence.
        span: Span,
    },

    /// A UN/EDIFACT data element code was not found in the segment definition.
    ///
    /// Produced by the code-addressed accessors
    /// ([`Segment::value_by_code`][crate::Segment::value_by_code] and friends)
    /// when the requested data element identifier does not appear anywhere in
    /// the supplied [`SegmentLayout`][crate::SegmentLayout].  This is the error
    /// that turns a mistyped or stale DE reference into a loud failure instead
    /// of a silent off-by-one read of the wrong element.
    #[error("segment {tag} has no data element {data_element} in its definition")]
    UnknownDataElement {
        /// Segment tag whose definition was searched.
        tag: String,
        /// The data element identifier that was not found.
        data_element: String,
    },

    /// A UN/EDIFACT data element code appears more than once in a segment definition.
    ///
    /// Code-addressed access requires an unambiguous target.  When a directory
    /// genuinely repeats a code (e.g. the same DE used at two positions), address
    /// it positionally with [`Segment::element_str`][crate::Segment::element_str]
    /// or split the definition.
    #[error("segment {tag} defines data element {data_element} at more than one position")]
    AmbiguousDataElement {
        /// Segment tag whose definition was searched.
        tag: String,
        /// The data element identifier that resolved to multiple positions.
        data_element: String,
    },

    /// A configured [`ReaderConfig`][crate::ReaderConfig] resource limit was exceeded.
    ///
    /// Raised by the parsing iterators when the input carries more segments,
    /// messages, or bytes than the caller allowed.  The limit is reported rather
    /// than silently applied: a budget that ends the iterator without an error is
    /// indistinguishable from a clean end of input, so the caller would accept a
    /// **truncated** interchange as complete.
    ///
    /// [`SegmentTooLong`][Self::SegmentTooLong] covers the per-segment size
    /// guard; this variant covers the whole-input budgets.
    #[error("input exceeded the configured {limit} limit of {max}")]
    LimitExceeded {
        /// Name of the limit that tripped: `"max_segments"`, `"max_messages"`,
        /// or `"max_input_bytes"`.
        limit: &'static str,
        /// The configured ceiling.
        max: u64,
    },

    /// A repeating data element was written under a service string advice that
    /// declares no repetition separator.
    ///
    /// ISO 9735-1 §8.6 repetitions can only be expressed when `UNA` position 7
    /// carries a real separator.  With the space "not used" sentinel there is no
    /// byte to write between occurrences, and joining them anyway would emit
    /// output that reads back as a single occurrence — silent data corruption.
    /// Construct the writer with [`Writer::with_una`][crate::Writer::with_una]
    /// and a service string advice whose `repetition_sep` is set.
    #[error(
        "cannot write a repeating data element: the active service string advice declares no repetition separator"
    )]
    RepetitionSeparatorNotDeclared,

    /// A non-finite float was handed to a numeric serializer.
    ///
    /// ISO 9735-1 §10 admits the ISO 6093 numeric representations — digits, an
    /// optional minus sign, a decimal mark, an exponent — and nothing else.
    /// There is no representation for `NaN` or infinity, and `Display` would
    /// emit `NaN` / `inf`, text no receiver can parse and that the crate's own
    /// reader would hand back as a string.
    #[error("non-finite number {value} has no EDIFACT representation")]
    NonFiniteNumber {
        /// The offending value, formatted for the message.
        value: String,
    },

    /// A character cannot be represented in the interchange's declared repertoire.
    ///
    /// The `UNB` S001 DE 0001 syntax identifier names the character repertoire the
    /// payload is written in (`UNOA`, `UNOC`, …).  Writing a character outside it
    /// produces bytes the receiver decodes as something else — or as nothing at
    /// all — so the writer refuses instead.
    ///
    /// Also raised by [`Charset::encode`][crate::Charset::encode].
    #[error(
        "character {character:?} at offset {offset} is not in the {charset} character repertoire"
    )]
    CharacterNotInRepertoire {
        /// The syntax identifier of the repertoire that rejected the character.
        charset: &'static str,
        /// The offending character.
        character: char,
        /// Byte offset of the character within the value it appeared in.
        offset: usize,
    },

    /// A `UNB` was written declaring a repertoire other than the writer's own.
    ///
    /// A header that names `UNOA` while the body goes out as ISO 8859-1 is
    /// unreadable at the far end in exactly the way that is hardest to diagnose,
    /// so [`Writer::begin_interchange`][crate::Writer::begin_interchange] refuses
    /// the combination rather than emitting it.
    #[error("UNB declares repertoire {declared}, but the writer encodes {writer}")]
    CharacterRepertoireMismatch {
        /// Syntax identifier passed to `begin_interchange`.
        declared: String,
        /// Repertoire the writer was bound to with `Writer::with_charset`.
        writer: &'static str,
    },

    /// The interchange declares a character repertoire this crate cannot decode.
    ///
    /// `UNOX` (ISO 2022 code extension) and `KECA` (Korean) are stateful or
    /// multi-byte in ways that break the byte-level delimiter scanning every
    /// other repertoire allows.  They are reported rather than silently
    /// mis-decoded.
    #[error("character repertoire '{syntax_identifier}' is not supported")]
    UnsupportedCharset {
        /// The `UNB` S001 DE 0001 value that could not be handled.
        syntax_identifier: String,
    },

    /// An interchange carries neither a message nor a group.
    ///
    /// ISO 9735-1 §7.1: an interchange "shall contain at least one group, or one
    /// message or one package".  A bare `UNB`/`UNZ` pair is a delivery that says
    /// nothing, and because `UNZ+0` makes the control count agree with the
    /// (absent) content, no count check can catch it.
    #[error("interchange {control_ref} contains no message or group")]
    EmptyInterchange {
        /// Interchange control reference (`UNB` DE 0020) of the empty interchange.
        control_ref: String,
    },

    /// A message carries no segment between its header and trailer.
    ///
    /// ISO 9735-1 §7.3: a message "shall be started and identified by a message
    /// header, shall be terminated by a message trailer, and shall contain at
    /// least one additional segment".
    #[error("message {message_ref} has no segments between UNH and UNT")]
    EmptyMessage {
        /// Message reference number (`UNH` DE 0062) of the empty message.
        message_ref: String,
        /// Byte range of the offending `UNH`.
        span: Span,
    },

    /// The interchange carries a package (`UNO`…`UNP`), which this crate does not
    /// tokenize.
    ///
    /// A package wraps an arbitrary object — ISO 9735-1 §7.9, elaborated by
    /// ISO 9735-8 — whose bytes are not EDIFACT-encoded and whose length is
    /// declared in `UNO` S022 DE 0810.  Feeding those bytes to a tokenizer that
    /// scans for delimiters produces nonsense, so the package is reported instead.
    /// Split the object out of the byte stream using the declared length before
    /// parsing the remainder.
    #[error("segment {tag} opens or closes a package, which this crate does not parse")]
    PackageNotSupported {
        /// The package segment encountered: `UNO` or `UNP`.
        tag: String,
        /// Byte range of the offending segment.
        span: Span,
    },

    /// A data element value consists only of spaces.
    ///
    /// ISO 9735-1 §9.3: "A data element value containing only space(s) shall not
    /// be allowed."  Trailing spaces are insignificant and must be suppressed
    /// (§9.1), so a value that is nothing but spaces is an element that should
    /// have been omitted — and a receiver comparing it against a code list, a
    /// reference, or a previous message will not treat it as absent.
    #[error(
        "segment {tag} element {element_index} component {component_index}: value is only spaces"
    )]
    BlankDataElementValue {
        /// Segment tag containing the blank value.
        tag: String,
        /// Zero-based element index.
        element_index: usize,
        /// Zero-based component index.
        component_index: usize,
        /// Byte range of the offending value.
        span: Span,
    },

    /// A segment carries nothing but its tag.
    ///
    /// ISO 9735-1 §7.5: "A segment shall contain at least one data element in
    /// addition to the segment tag."  §8.5 adds that a conditional segment whose
    /// only content is the tag "shall be omitted in its entirety" — so `ABC'` is
    /// either a mandatory segment that lost its data or a conditional one that
    /// should not have been sent.
    #[error("segment {tag} contains no data element")]
    SegmentWithoutDataElements {
        /// The offending segment tag.
        tag: String,
        /// Byte range of the offending segment.
        span: Span,
    },

    /// A data element occurred more times than its definition allows.
    ///
    /// ISO 9735-1 §7.5: "Each stand-alone or composite data element's position,
    /// status and maximum number of occurrences within the segment structure
    /// shall be stated in the segment specification." Exceeding the stated
    /// maximum is a structural violation, reported by `CONTRL` as code 35.
    #[error("segment {tag} element {element_index} occurs {actual} times, at most {max} allowed")]
    TooManyRepetitions {
        /// Segment tag carrying the over-repeated element.
        tag: String,
        /// Zero-based element index.
        element_index: usize,
        /// Maximum occurrences the definition allows.
        max: u8,
        /// Occurrences actually present.
        actual: usize,
        /// Byte range of the offending element.
        span: Span,
    },

    /// A value's characters do not match its declared representation class.
    ///
    /// A directory types every data element `a` (alphabetic), `n` (numeric), or
    /// `an` (alphanumeric). ISO 9735-1 §10 fixes what "numeric" admits: digits,
    /// an optional minus, a decimal mark, and an exponent — and explicitly not
    /// the space character or a plus sign. Reported by `CONTRL` as code 37.
    #[error(
        "segment {tag} element {element_index} component {component_index}: {value:?} is not {repr}"
    )]
    InvalidCharacterType {
        /// Segment tag containing the value.
        tag: String,
        /// Zero-based element index.
        element_index: usize,
        /// Zero-based component index.
        component_index: usize,
        /// The declared representation, e.g. `n..6`.
        repr: String,
        /// The offending value.
        value: String,
        /// Byte range of the offending value.
        span: Span,
    },

    /// A value is longer than its declared representation allows.
    ///
    /// Length is counted in **characters**, not bytes (ISO 9735-1 §6), and for a
    /// numeric value excludes the sign, the decimal mark, and the exponent
    /// (§10). Reported by `CONTRL` as code 39.
    #[error(
        "segment {tag} element {element_index} component {component_index}: {actual} characters exceeds {repr}"
    )]
    DataElementTooLong {
        /// Segment tag containing the value.
        tag: String,
        /// Zero-based element index.
        element_index: usize,
        /// Zero-based component index.
        component_index: usize,
        /// The declared representation, e.g. `an..35`.
        repr: String,
        /// The measured character count.
        actual: usize,
        /// Byte range of the offending value.
        span: Span,
    },

    /// A value is shorter than its declared fixed-length representation.
    ///
    /// Only a fixed-length representation (`n8`, `a1`) has a minimum above one;
    /// a variable one (`an..35`) is satisfied by any non-empty value. Reported
    /// by `CONTRL` as code 40.
    #[error(
        "segment {tag} element {element_index} component {component_index}: {actual} characters is short of {repr}"
    )]
    DataElementTooShort {
        /// Segment tag containing the value.
        tag: String,
        /// Zero-based element index.
        element_index: usize,
        /// Zero-based component index.
        component_index: usize,
        /// The declared representation, e.g. `n8`.
        repr: String,
        /// The measured character count.
        actual: usize,
        /// Byte range of the offending value.
        span: Span,
    },

    /// A segment or composite ends in separators that carry no value.
    ///
    /// ISO 9735-1 §8.7.1: "If one or more non-repeating composite data elements
    /// or stand-alone data elements at the end of a segment are omitted, the
    /// data element separators which would normally follow them shall also be
    /// omitted." §8.7.2 says the same for components at the end of a composite.
    ///
    /// `BGM+220+'` and `DTM+137:20260101:'` are therefore both malformed, and a
    /// receiver that trims them is being generous rather than correct. Reported
    /// by `CONTRL` as code 45.
    #[error("segment {tag} ends in a separator that carries no value")]
    TrailingSeparator {
        /// Segment tag carrying the trailing separator.
        tag: String,
        /// `Some(index)` when the trailing separators close that element's
        /// composite; `None` when they close the segment itself.
        element_index: Option<usize>,
        /// Byte range of the offending segment or element.
        span: Span,
    },

    /// An interchange mixes groups with ungrouped messages.
    ///
    /// ISO 9735-1 §7.1 lists what an interchange may contain, and every entry is
    /// exclusive: messages, or packages, or groups containing them — never a
    /// group alongside a bare message. A message outside every group has no
    /// group to be counted in, so `UNZ` DE 0036 cannot describe the interchange
    /// at all. Reported by `CONTRL` as code 30.
    #[error("interchange mixes groups with ungrouped messages")]
    GroupsAndMessagesMixed {
        /// Byte range of the segment that revealed the mix.
        span: Span,
    },

    /// A value carries characters ISO 9735-1 §9.1 requires to be suppressed.
    ///
    /// §9.1: "In variable length numeric data elements, leading zeroes shall be
    /// suppressed... In variable length alphabetic and alphanumeric data
    /// elements, trailing spaces shall be suppressed."
    ///
    /// Both are artefacts of a fixed-width source record copied into a
    /// variable-length field. The value is still readable, so this is a warning
    /// — but a receiver comparing `007` against the code `7`, or `"ACME "`
    /// against `"ACME"`, will not match them.
    #[error("segment {tag} element {element_index} component {component_index}: {kind}")]
    InsignificantCharacters {
        /// Segment tag containing the value.
        tag: String,
        /// Zero-based element index.
        element_index: usize,
        /// Zero-based component index.
        component_index: usize,
        /// Which rule of §9.1 was violated.
        kind: Insignificant,
        /// Byte range of the offending value.
        span: Span,
    },

    /// A [`SegmentLayout`][crate::SegmentLayout] was applied to a segment with a different tag.
    ///
    /// Passing the `NAD` definition to a `DTM` segment would resolve codes
    /// against the wrong table, which is exactly the class of mistake that
    /// code-addressed access exists to prevent — so it is rejected up front.
    #[error("segment layout is for {expected}, but the segment is {actual}")]
    SegmentLayoutMismatch {
        /// Tag the layout describes.
        expected: String,
        /// Tag of the segment the layout was applied to.
        actual: String,
    },
}

impl From<std::io::Error> for EdifactError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(IoError(e))
    }
}

impl EdifactError {
    /// Stable diagnostic code for this error variant.
    #[must_use]
    pub const fn stable_code(&self) -> &'static str {
        match self {
            Self::UnexpectedEof { .. } => "E001",
            Self::InvalidDelimiter { .. } => "E002",
            Self::InvalidText { .. } => "E003",
            Self::MessageCountMismatch { .. } => "E004",
            Self::SegmentCountMismatch { .. } => "E005",
            Self::InvalidSegmentTag(_) => "E006",
            Self::InvalidUna => "E007",
            Self::MissingRequiredElement { .. } => "E008",
            Self::InvalidUtf8 => "E009",
            Self::Io(_) => "E010",
            Self::InvalidSegmentForMessage { .. } => "E011",
            Self::InvalidElementCount { .. } => "E012",
            Self::InvalidComponentCount { .. } => "E013",
            Self::InvalidCodeValue { .. } => "E014",
            Self::MissingSegment { .. } => "E015",
            Self::QualifierMismatch { .. } => "E016",
            Self::ConditionalRequirementNotMet { .. } => "E017",
            // E018 is permanently retired (was ValidationFailed, removed in 0.8.0)
            Self::InvalidReleaseSequence { .. } => "E019",
            Self::SegmentTooLong { .. } => "E020",
            Self::MissingRequiredComponent { .. } => "E021",
            // E022 is permanently retired (was UnexpectedMessageType, removed
            // in 0.17.0 along with the type-erased MessageDispatch it served).
            Self::InterchangeTooLarge { .. } => "E023",
            Self::InvalidEventSequence { .. } => "E024",
            Self::InvalidElementPosition => "E025",
            Self::IncompatibleReleaseScopes { .. } => "E026",
            Self::InvalidFieldValue { .. } => "E027",
            Self::UnexpectedDataToken { .. } => "E028",
            // E029 is permanently retired (was FunctionalGroupNotSupported, removed when
            // full UNG/UNE support was added — functional groups are now parsed natively)
            Self::ValidationErrors { .. } => "E030",
            Self::UnrecognisedSyntaxIdentifier(_) => "E031",
            Self::DuplicateReference { .. } => "E032",
            Self::UnknownDataElement { .. } => "E033",
            Self::AmbiguousDataElement { .. } => "E034",
            Self::SegmentLayoutMismatch { .. } => "E035",
            Self::LimitExceeded { .. } => "E036",
            Self::RepetitionSeparatorNotDeclared => "E037",
            Self::CharacterNotInRepertoire { .. } => "E038",
            Self::UnsupportedCharset { .. } => "E039",
            Self::NonFiniteNumber { .. } => "E040",
            Self::CharacterRepertoireMismatch { .. } => "E041",
            Self::EmptyInterchange { .. } => "E042",
            Self::EmptyMessage { .. } => "E043",
            Self::PackageNotSupported { .. } => "E044",
            Self::BlankDataElementValue { .. } => "E045",
            Self::SegmentWithoutDataElements { .. } => "E046",
            Self::TooManyRepetitions { .. } => "E047",
            Self::InvalidCharacterType { .. } => "E048",
            Self::DataElementTooLong { .. } => "E049",
            Self::DataElementTooShort { .. } => "E050",
            Self::TrailingSeparator { .. } => "E051",
            Self::GroupsAndMessagesMixed { .. } => "E052",
            Self::InsignificantCharacters { .. } => "E053",
        }
    }

    /// Stable recovery hint for common malformed input and validation cases.
    #[must_use]
    pub fn recovery_hint(&self) -> Option<&'static str> {
        match self {
            Self::UnexpectedEof { .. } => {
                Some("Ensure every segment ends with the configured segment terminator")
            }
            Self::InvalidDelimiter { .. } => {
                Some("Check UNA service string advice and delimiter bytes in the payload")
            }
            Self::InvalidText { .. } => {
                Some("Input must be valid UTF-8 text for segment and element values")
            }
            Self::InvalidReleaseSequence { .. } => {
                Some("Release character must escape one following byte; trailing '?' is invalid")
            }
            Self::InvalidSegmentTag(_) => Some("Segment tags must be 3 ASCII uppercase letters"),
            Self::InvalidUna => Some(
                "UNA must be exactly 9 bytes: 'UNA' followed by 6 service characters, of which the active five must be distinct and non-whitespace",
            ),
            Self::MissingRequiredElement { .. } => {
                Some("Provide all mandatory elements for the segment per directory rules")
            }
            Self::MissingRequiredComponent { .. } => Some(
                "Provide all mandatory components for the composite element per directory rules",
            ),
            Self::InvalidSegmentForMessage { .. } => {
                Some("Remove unsupported segment or switch to the correct message type")
            }
            Self::InvalidElementCount { .. } => {
                Some("Adjust the segment element count to the allowed min/max range")
            }
            Self::InvalidComponentCount { .. } => {
                Some("Fix composite element arity to match the expected component count")
            }
            Self::InvalidCodeValue { .. } => {
                Some("Use a value from the referenced code list for this element")
            }
            Self::MissingSegment { .. } => {
                Some("Insert the required segment at the expected position")
            }
            Self::QualifierMismatch { .. } => {
                Some("Set the segment qualifier to the expected value")
            }
            Self::ConditionalRequirementNotMet { .. } => {
                Some("When the condition is met, include the conditionally required element")
            }
            Self::SegmentTooLong { limit, .. } => {
                let _ = limit; // used in the error message; hint is generic
                Some("Increase max_segment_bytes in ReaderConfig or reject the input as malformed")
            }
            Self::InvalidEventSequence { .. } => {
                Some("Emit StartSegment before Element, and Element before ComponentElement")
            }
            Self::InvalidElementPosition => Some(
                "Set element position to a value >= 1; positions are one-based (1 = first element slot)",
            ),
            Self::IncompatibleReleaseScopes { .. } => Some(
                "Only compose ProfileRulePack values that share the same release scope, or where at most one has a release scope set",
            ),
            Self::InvalidFieldValue { .. } => Some(
                "Correct the field value to match the expected format or range for this element",
            ),
            Self::UnexpectedDataToken { .. } => Some(
                "A data element appeared before any segment tag; check for partial writes or encoding corruption",
            ),
            Self::DuplicateReference { .. } => Some(
                "Assign a unique control reference to every UNH (DE 0062) and UNG (DE 0048) within an interchange",
            ),
            Self::UnknownDataElement { .. } => Some(
                "Check the data element identifier against the segment definition; the directory is the source of truth",
            ),
            Self::AmbiguousDataElement { .. } => Some(
                "The code appears at more than one position; address the element positionally instead",
            ),
            Self::SegmentLayoutMismatch { .. } => {
                Some("Resolve codes against the segment definition whose tag matches the segment")
            }
            Self::LimitExceeded { .. } => {
                Some("Raise the corresponding ReaderConfig limit, or reject the input as oversized")
            }
            Self::RepetitionSeparatorNotDeclared => Some(
                "Build the writer with Writer::with_una and a ServiceStringAdvice whose repetition_sep is set",
            ),
            Self::CharacterNotInRepertoire { .. } => Some(
                "Transliterate the value into the declared repertoire, or declare a wider one in UNB S001 (UNOC for Latin-1, UNOY for UTF-8)",
            ),
            Self::UnsupportedCharset { .. } => Some(
                "UNOX and KECA are not supported; ask the partner for UNOC or UNOY, or transcode the interchange before parsing",
            ),
            Self::NonFiniteNumber { .. } => Some(
                "EDIFACT has no representation for NaN or infinity; check the calculation, or omit the element",
            ),
            Self::CharacterRepertoireMismatch { .. } => Some(
                "Pass the writer's own syntax identifier to begin_interchange, or bind the writer to the repertoire the header declares",
            ),
            Self::EmptyInterchange { .. } => Some(
                "An interchange must carry at least one message or group; send nothing rather than an empty envelope",
            ),
            Self::EmptyMessage { .. } => Some(
                "A message needs at least one segment between UNH and UNT; omit the message entirely if it has no content",
            ),
            Self::PackageNotSupported { .. } => Some(
                "Split the object out of the byte stream using the length in UNO S022 DE 0810, then parse the remaining segments",
            ),
            Self::BlankDataElementValue { .. } => Some(
                "Omit the data element instead of sending spaces; trailing spaces are insignificant and must be suppressed",
            ),
            Self::SegmentWithoutDataElements { .. } => {
                Some("Supply the segment's data, or omit the segment entirely if it is conditional")
            }
            Self::TooManyRepetitions { .. } => Some(
                "Reduce the occurrences to the definition's maximum, or correct the definition if the directory allows more",
            ),
            Self::InvalidCharacterType { .. } => Some(
                "Send a value of the declared class: `n` admits digits, an optional minus, a decimal mark and an exponent — never a space or a plus sign",
            ),
            Self::DataElementTooLong { .. } => Some(
                "Shorten the value to the declared maximum; length is counted in characters, and a numeric value excludes its sign, decimal mark and exponent",
            ),
            Self::DataElementTooShort { .. } => Some(
                "Pad the value to the declared fixed length, or correct the definition if the directory declares it variable",
            ),
            Self::TrailingSeparator { .. } => Some(
                "Stop emitting separators once the last value has been written; ISO 9735-1 §8.7.1 and §8.7.2 require trailing ones to be omitted",
            ),
            Self::GroupsAndMessagesMixed { .. } => Some(
                "Put every message inside a group, or none of them; ISO 9735-1 §7.1 does not allow both in one interchange",
            ),
            Self::InsignificantCharacters { .. } => Some(
                "Suppress the insignificant characters before sending: leading zeroes in a variable-length numeric value, trailing spaces in a variable-length text one",
            ),
            Self::UnrecognisedSyntaxIdentifier(_) => Some(
                "UNB S001 DE 0001 must name a defined repertoire: UNOA-UNOK, UNOX, UNOY, or KECA",
            ),
            Self::ValidationErrors { .. }
            | Self::MessageCountMismatch { .. }
            | Self::SegmentCountMismatch { .. }
            | Self::InterchangeTooLarge { .. }
            | Self::InvalidUtf8
            | Self::Io(_) => None,
        }
    }
}

#[cfg(feature = "diagnostics")]
#[cfg_attr(docsrs, doc(cfg(feature = "diagnostics")))]
impl miette::Diagnostic for EdifactError {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(self.stable_code()))
    }

    /// Mirrors the severity the validation pipeline assigns.
    ///
    /// The two must agree: a `miette` render that calls a control-reference
    /// mismatch a warning while [`ValidationReport`] files it as an error tells
    /// the operator and the program two different things about the same
    /// interchange. [`crate::ValidationReport`] is the source of truth, and this
    /// delegates to it.
    fn severity(&self) -> Option<miette::Severity> {
        Some(match crate::report::severity_for_error(self) {
            crate::ValidationSeverity::Info => miette::Severity::Advice,
            crate::ValidationSeverity::Warning => miette::Severity::Warning,
            crate::ValidationSeverity::Error | crate::ValidationSeverity::Critical => {
                miette::Severity::Error
            }
        })
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        match self {
            // Static text — no allocation needed.
            Self::InvalidUna => Some(Box::new(
                "UNA segment must be exactly 9 bytes: 'UNA' + 6 service characters. See EDIFACT spec",
            )),
            Self::InvalidUtf8 => Some(Box::new(
                "Internal error: serialized output contains invalid UTF-8. Please report this as a bug",
            )),
            // Dynamic help text.
            Self::UnexpectedEof { offset } => Some(Box::new(format!(
                "Check that all segments are terminated with the segment terminator (usually '). \
                 Reached end at offset {offset}",
            ))),
            Self::InvalidDelimiter { byte, offset } => Some(Box::new(format!(
                "The byte 0x{byte:02X} at offset {offset} is not a valid delimiter. \
                 Check UNA configuration",
            ))),
            Self::InvalidText { offset } => Some(Box::new(format!(
                "The byte sequence at offset {offset} contains invalid UTF-8. \
                 Ensure input is valid UTF-8",
            ))),
            Self::InvalidReleaseSequence { offset } => Some(Box::new(format!(
                "Release character at offset {offset} is dangling. \
                 Ensure '?' is followed by an escaped byte",
            ))),
            Self::MessageCountMismatch { expected, actual } => Some(Box::new(format!(
                "UNZ declares {expected} message(s) but {actual} UNH/UNT pair(s) were found. \
                 Check the UNZ message count",
            ))),
            Self::SegmentCountMismatch {
                expected,
                actual,
                message_ref,
                ..
            } => Some(Box::new(format!(
                "UNT for message {message_ref} declares {expected} segment(s) but {actual} were found. \
                 Check the UNT segment count",
            ))),
            Self::InvalidSegmentTag(tag) => Some(Box::new(format!(
                "Segment tag '{tag}' must be exactly 3 ASCII uppercase letters",
            ))),
            Self::MissingRequiredElement { tag, element_index } => Some(Box::new(format!(
                "Segment {tag} requires element at index {element_index}",
            ))),
            Self::MissingRequiredComponent {
                tag,
                element_index,
                component_index,
            } => Some(Box::new(format!(
                "Segment {tag} element {element_index} requires component at index {component_index}",
            ))),
            Self::Io(e) => Some(Box::new(format!("I/O error: {e}"))),
            Self::InvalidSegmentForMessage {
                tag, message_type, ..
            } => Some(Box::new(format!(
                "Segment {tag} should not appear in a {message_type} message. \
                 Check the directory definition",
            ))),
            Self::InvalidElementCount {
                tag,
                min,
                max,
                actual,
                ..
            } => Some(Box::new(format!(
                "Segment {tag} should have between {min} and {max} elements, but has {actual}. \
                 Check segment structure",
            ))),
            Self::InvalidComponentCount {
                tag,
                element_index,
                expected,
                actual,
                ..
            } => Some(Box::new(format!(
                "In segment {tag}, element {element_index} should have {expected} components \
                     but has {actual}. Check element structure",
            ))),
            Self::InvalidCodeValue {
                tag,
                element_index,
                value,
                code_list,
                ..
            } => Some(Box::new(format!(
                "Value '{value}' in segment {tag} element {element_index} is not in the \
                     {code_list} code list. Check the directory for valid codes",
            ))),
            Self::MissingSegment {
                tag,
                expected_position,
            } => Some(Box::new(format!(
                "Segment {tag} is required at position {expected_position} but is missing. \
                 Add this segment to the message",
            ))),
            Self::QualifierMismatch {
                tag,
                actual,
                expected,
                ..
            } => Some(Box::new(format!(
                "Segment {tag} has qualifier '{actual}' but expected '{expected}'. \
                 Check the segment's first component",
            ))),
            Self::ConditionalRequirementNotMet {
                tag,
                element_index,
                condition,
                ..
            } => Some(Box::new(format!(
                "In segment {tag}, element {element_index} is conditionally required when: \
                     {condition}. Check if the condition is met",
            ))),
            Self::SegmentTooLong { offset, limit } => Some(Box::new(format!(
                "Segment starting at byte offset {offset} exceeds the {limit}-byte limit. \
                 Use ReaderConfig::max_segment_bytes to adjust the limit if needed, \
                 or verify the input for a missing segment terminator",
            ))),
            Self::InterchangeTooLarge { count } => Some(Box::new(format!(
                "Interchange contains {count} items which exceeds the u32::MAX limit. \
                 This is an extremely unusual input; verify the message is not corrupted.",
            ))),
            Self::InvalidEventSequence { message } => Some(Box::new(format!(
                "Event sequence violation: {message}. \
                 Check that StartSegment is emitted before Element, and Element before ComponentElement.",
            ))),
            Self::InvalidElementPosition => Some(Box::new(
                "Element positions must be >= 1 (one-based). \
                 Ensure no OwnedElementRef is constructed with position == 0",
            )),
            Self::IncompatibleReleaseScopes { current, incoming } => Some(Box::new(format!(
                "Release scope {current:?} and {incoming:?} are incompatible. \
                 Only compose ProfileRulePack values that share the same release scope, \
                 or where at most one carries a release scope",
            ))),
            Self::InvalidFieldValue {
                tag,
                element_index,
                value,
            } => Some(Box::new(format!(
                "Segment {tag} element {element_index} has invalid value '{value}'. \
                 Check the expected format or range for this field",
            ))),
            Self::UnexpectedDataToken { offset } => Some(Box::new(format!(
                "Data element at offset {offset} appeared before any segment tag. \
                 Check for partial writes or encoding corruption",
            ))),
            Self::ValidationErrors { error_count, .. } => Some(Box::new(format!(
                "Validation found {error_count} error(s). Inspect the ValidationReport for details",
            ))),
            Self::UnrecognisedSyntaxIdentifier(id) => Some(Box::new(format!(
                "Syntax identifier '{id}' is not defined in ISO 9735-1. \
                 Valid values are UNOA, UNOB, UNOC, UNOD, UNOE, UNOF (or KECA for KEC-A profile)",
            ))),
            Self::DuplicateReference { tag, reference, .. } => Some(Box::new(format!(
                "Reference '{reference}' is used by more than one {tag} in this interchange; \
                 each must be unique so receivers can address messages unambiguously",
            ))),
            Self::UnknownDataElement { tag, data_element } => Some(Box::new(format!(
                "Segment {tag} does not define data element {data_element}. \
                 Check the identifier against the directory definition for {tag}",
            ))),
            Self::AmbiguousDataElement { tag, data_element } => Some(Box::new(format!(
                "Segment {tag} defines data element {data_element} at more than one position, \
                 so code-addressed access cannot pick one; use a positional accessor",
            ))),
            Self::SegmentLayoutMismatch { expected, actual } => Some(Box::new(format!(
                "The supplied layout describes segment {expected} but was applied to {actual}. \
                 Look up the definition by the segment's own tag",
            ))),
            Self::RepetitionSeparatorNotDeclared => Some(Box::new(
                "UNA position 7 holds the space \"not used\" sentinel, so repeating data \
                 elements cannot be expressed. Use Writer::with_una with a repetition_sep",
            )),
            Self::CharacterNotInRepertoire {
                charset,
                character,
                offset,
            } => Some(Box::new(format!(
                "The character {character:?} at offset {offset} has no representation in {charset}. \
                 Transliterate it, or declare a wider repertoire in UNB S001 DE 0001",
            ))),
            Self::UnsupportedCharset { syntax_identifier } => Some(Box::new(format!(
                "'{syntax_identifier}' is stateful or multi-byte, so byte-level delimiter scanning \
                 would be unsound. Transcode the interchange to UNOC or UNOY before parsing",
            ))),
            Self::CharacterRepertoireMismatch { declared, writer } => Some(Box::new(format!(
                "The UNB declares {declared} but the writer encodes {writer}. \
                 The receiver would decode the body with the wrong table",
            ))),
            Self::NonFiniteNumber { value } => Some(Box::new(format!(
                "The value {value} is not finite. EDIFACT numeric data elements have no \
                 representation for NaN or infinity",
            ))),
            Self::LimitExceeded { limit, max } => Some(Box::new(format!(
                "The input exceeds the configured {limit} limit of {max}. \
                 Raise it via ReaderConfig if the input is legitimate, or reject the input",
            ))),
            Self::EmptyInterchange { control_ref } => Some(Box::new(format!(
                "Interchange {control_ref} carries no message and no group. \
                 ISO 9735-1 §7.1 requires at least one; send nothing rather than an empty envelope",
            ))),
            Self::EmptyMessage { message_ref, .. } => Some(Box::new(format!(
                "Message {message_ref} has nothing between UNH and UNT. \
                 ISO 9735-1 §7.3 requires at least one additional segment",
            ))),
            Self::PackageNotSupported { tag, .. } => Some(Box::new(format!(
                "{tag} opens or closes a package, whose object is arbitrary binary data \
                 rather than EDIFACT. Split it out using the length in UNO S022 DE 0810, \
                 then parse the remaining segments",
            ))),
            Self::BlankDataElementValue {
                tag,
                element_index,
                component_index,
                ..
            } => Some(Box::new(format!(
                "Segment {tag} element {element_index} component {component_index} holds only \
                 spaces. ISO 9735-1 §9.3 forbids that — omit the element instead",
            ))),
            Self::SegmentWithoutDataElements { tag, .. } => Some(Box::new(format!(
                "Segment {tag} carries only its tag. ISO 9735-1 §7.5 requires at least one \
                 data element; §8.5 says a conditional segment with no data is omitted entirely",
            ))),
            Self::TooManyRepetitions {
                tag,
                element_index,
                max,
                actual,
                ..
            } => Some(Box::new(format!(
                "Segment {tag} element {element_index} occurs {actual} times but its definition \
                 allows at most {max}",
            ))),
            Self::InvalidCharacterType {
                repr, value, tag, ..
            } => Some(Box::new(format!(
                "Segment {tag}: {value:?} is not a valid {repr} value. ISO 9735-1 §10 admits \
                 digits, an optional minus sign, a decimal mark and an exponent — the space \
                 character and the plus sign are not allowed",
            ))),
            Self::DataElementTooLong {
                repr, actual, tag, ..
            } => Some(Box::new(format!(
                "Segment {tag}: the value is {actual} characters, but the directory declares \
                 {repr}. Length is counted in characters rather than bytes",
            ))),
            Self::DataElementTooShort {
                repr, actual, tag, ..
            } => Some(Box::new(format!(
                "Segment {tag}: the value is {actual} characters, but the directory declares the \
                 fixed length {repr}",
            ))),
            Self::TrailingSeparator {
                tag, element_index, ..
            } => Some(Box::new(match element_index {
                Some(index) => format!(
                    "Segment {tag} element {index} ends in a component separator with no value \
                     after it. ISO 9735-1 §8.7.2 requires trailing component separators to be \
                     omitted",
                ),
                None => format!(
                    "Segment {tag} ends in a data element separator with no value after it. \
                     ISO 9735-1 §8.7.1 requires trailing data element separators to be omitted",
                ),
            })),
            Self::GroupsAndMessagesMixed { .. } => Some(Box::new(
                "ISO 9735-1 §7.1 lists what an interchange may contain, and the entries are \
                 exclusive: groups containing messages, or bare messages — never both, because a \
                 message outside every group cannot be counted in UNZ DE 0036",
            )),
            Self::InsignificantCharacters { tag, kind, .. } => Some(Box::new(format!(
                "Segment {tag}: {kind}. ISO 9735-1 §9.1 requires insignificant characters to be \
                 suppressed before transfer",
            ))),
        }
    }
}

// ── validation report ─────────────────────────────────────────────────────────

pub use crate::report::ValidationReport;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_hint_exists_for_common_malformed_cases() {
        let err = EdifactError::InvalidReleaseSequence { offset: 10 };
        assert!(err.recovery_hint().is_some());

        let err = EdifactError::InvalidCodeValue {
            tag: "BGM".to_owned(),
            element_index: 0,
            value: "X".to_owned(),
            code_list: "1001".to_owned(),
            span: Span::new(0, 9),
            suggestion: None,
        };
        assert!(err.recovery_hint().is_some());
    }
}
