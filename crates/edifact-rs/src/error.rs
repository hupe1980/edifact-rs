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

/// All errors produced by `edifact-rs`.
///
/// # Error Variants
///
/// All variants that include an offset carry byte position information from the input stream.
/// This data enables precise error location reporting in diagnostics.
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
    },

    /// Invalid or malformed segment tag.
    ///
    /// Segment tags must be exactly 3 ASCII uppercase letters.
    #[error("invalid segment tag {0:?}")]
    InvalidSegmentTag(String),

    /// Invalid UNA service string advice.
    ///
    /// If present, the UNA segment must be exactly 9 bytes: `"UNA"` followed by
    /// 6 service characters.  The five active characters (`element_sep`,
    /// `component_sep`, `decimal_mark`, `release_char`, and `segment_term`) must
    /// all be mutually distinct and printable, non-alphanumeric ASCII.  The
    /// **repetition separator** (UNA byte 7) is validated on the same terms
    /// unless it is a space, the conventional "not used" sentinel.
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
        /// Segment tag byte offset.
        offset: usize,
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
        /// Segment start byte offset.
        offset: usize,
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
        /// Segment start byte offset.
        offset: usize,
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
        /// Segment start byte offset.
        offset: usize,
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
        /// Segment start byte offset.
        offset: usize,
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
        /// Segment start byte offset.
        offset: usize,
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

    /// No handler was registered in [`crate::MessageDispatch`] for this message type.
    ///
    /// Returned by [`crate::MessageDispatch::dispatch`] when the message-type
    /// extracted from the `UNH` segment does not match any registered handler
    /// and no fallback was configured.
    #[error("no handler registered for message type {message_type}")]
    UnexpectedMessageType {
        /// The unhandled message type string from the `UNH` segment.
        message_type: String,
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

    /// The interchange syntax identifier (UNB DE 0001) is not a recognised ISO 9735-1 value.
    ///
    /// Valid syntax identifiers are: `UNOA`, `UNOB`, `UNOC`, `UNOD`, `UNOE`, `UNOF`, and
    /// `KECA` (Korean EDI Centre A).  Any other value indicates a non-standard generator
    /// or a corrupted UNB header.
    #[error(
        "unrecognised syntax identifier '{0}': expected UNOA/UNOB/UNOC/UNOD/UNOE/UNOF (or KECA)"
    )]
    UnrecognisedSyntaxIdentifier(String),

    /// A control reference was reused within the scope that requires it to be unique.
    ///
    /// ISO 9735-1 requires the message reference number (`UNH` DE 0062) to be
    /// unique within an interchange, and the group reference number (`UNG`
    /// DE 0048) to be unique within an interchange.  Duplicates make a message
    /// unaddressable: a receiver keying on the reference silently processes one
    /// occurrence and drops the rest.
    #[error("duplicate {tag} reference '{reference}' at byte offset {offset}")]
    DuplicateReference {
        /// Segment tag that carries the duplicated reference (`UNH` or `UNG`).
        tag: String,
        /// The reference value that appeared more than once.
        reference: String,
        /// Byte offset of the duplicate occurrence.
        offset: usize,
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
            Self::UnexpectedMessageType { .. } => "E022",
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
                "UNA must be exactly 9 bytes: 'UNA' followed by 6 distinct, non-whitespace service characters",
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
            Self::ValidationErrors { .. }
            | Self::MessageCountMismatch { .. }
            | Self::SegmentCountMismatch { .. }
            | Self::UnexpectedMessageType { .. }
            | Self::InterchangeTooLarge { .. }
            | Self::UnrecognisedSyntaxIdentifier(_)
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

    fn severity(&self) -> Option<miette::Severity> {
        match self {
            Self::InvalidCodeValue { .. }
            | Self::InvalidComponentCount { .. }
            | Self::QualifierMismatch { .. } => Some(miette::Severity::Warning),
            _ => Some(miette::Severity::Error),
        }
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
            Self::UnexpectedMessageType { message_type } => Some(Box::new(format!(
                "No handler was registered for message type '{message_type}'. \
                 Register a handler with MessageDispatch::on(\"{message_type}\", ...)",
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
            offset: 0,
            suggestion: None,
        };
        assert!(err.recovery_hint().is_some());
    }
}
