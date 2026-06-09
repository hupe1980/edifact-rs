use std::sync::Arc;
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
    /// 6 service characters.  The four active characters (element separator,
    /// component separator, release character, and segment terminator) must be
    /// mutually distinct and must not be ASCII whitespace.  The decimal mark and
    /// repetition separator characters are not validated by this check.
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
    /// slot.  Position 0 is reserved and invalid.  Use [`crate::OwnedElementRef::new`]
    /// to catch this at construction time.
    #[error("element definition contains invalid position 0; positions must be >= 1 (one-based)")]
    InvalidElementPosition,

    /// Two [`crate::ProfileRulePack`] values with incompatible release scopes were composed.
    ///
    /// When composing packs via [`crate::ProfileRulePack::extend_from`] or
    /// [`crate::ProfileRulePack::merge_with_override`], both packs must either
    /// share the same release scope or at most one may carry a scope.
    #[error("incompatible release scopes: cannot compose {current:?} with {incoming:?}")]
    #[non_exhaustive]
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

    /// The input contains EDIFACT functional group segments (`UNG`/`UNE`).
    ///
    /// Functional groups are defined in ISO 9735 but are rarely used in practice
    /// and are not supported by this library.  Strip `UNG`/`UNE` wrappers before
    /// calling `validate_envelope`, or process the interchange as raw segments.
    #[error(
        "functional group segments (UNG/UNE) at byte offset {offset} are not supported; \
         strip them before calling validate_envelope"
    )]
    FunctionalGroupNotSupported {
        /// Byte offset of the first `UNG` or `UNE` segment found.
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
            Self::FunctionalGroupNotSupported { .. } => "E029",
            Self::ValidationErrors { .. } => "E030",
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
            Self::FunctionalGroupNotSupported { .. } => Some(
                "Strip UNG/UNE segments before calling validate_envelope, or process the interchange as raw segments",
            ),
            Self::ValidationErrors { .. }
            | Self::MessageCountMismatch { .. }
            | Self::SegmentCountMismatch { .. }
            | Self::UnexpectedMessageType { .. }
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
            Self::FunctionalGroupNotSupported { offset } => Some(Box::new(format!(
                "Functional group segment (UNG/UNE) found at offset {offset}. \
                 Strip UNG/UNE wrappers before calling validate_envelope",
            ))),
            Self::ValidationErrors { error_count, .. } => Some(Box::new(format!(
                "Validation found {error_count} error(s). Inspect the ValidationReport for details",
            ))),
        }
    }
}

// ── validation report ─────────────────────────────────────────────────────────

/// Priority level for a validation error or warning.
///
/// Marked `#[non_exhaustive]` so that adding new severity levels in future
/// releases is not a breaking change for downstream match arms.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ValidationSeverity {
    /// Structural parse failure; processing cannot continue.
    Critical,
    /// Structural validation failed; message is invalid.
    Error,
    /// Data validation warning (e.g., code-list mismatch); message may be usable.
    Warning,
    /// Informational note; message is valid but noteworthy.
    Info,
}

impl ValidationSeverity {
    /// Return a lowercase ASCII string for this severity level.
    ///
    /// Stable for the four known variants.  Because the enum is
    /// `#[non_exhaustive]`, new variants added in future releases are
    /// handled by a catch-all arm that returns `"unknown"` so that
    /// existing code keeps compiling and serialising gracefully.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            #[allow(unreachable_patterns)]
            _ => "unknown",
        }
    }

    /// Return a numeric priority for this severity level.
    ///
    /// Higher values indicate higher severity: `Critical = 3`, `Error = 2`,
    /// `Warning = 1`, `Info = 0`.
    #[must_use]
    pub fn numeric_level(self) -> u8 {
        match self {
            Self::Info => 0,
            Self::Warning => 1,
            Self::Error => 2,
            Self::Critical => 3,
        }
    }
}

impl std::fmt::Display for ValidationSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A structured validation issue.
///
/// Marked `#[non_exhaustive]` so that new diagnostic fields (e.g. `segment_group`)
/// can be added in future releases without breaking downstream code that constructs
/// issues via struct literals.  Always use [`ValidationIssue::new`] + builder
/// methods (`with_*`) rather than constructing directly.
///
/// ## Rule ID prefix convention
///
/// The `rule_id` field doubles as a lightweight metadata carrier when no full
/// `context` map is needed.  Use a namespaced, structured prefix so consumers can
/// extract domain-specific information without parsing the human-readable message:
///
/// ```text
/// "<PACK>-<SCOPE>-<TAG>-<STATUS>"
///  ^^^^^^^^                        — identifies the pack / profile (e.g. "AHB-13001")
///              ^^^^^^^             — identifies the rule scope (e.g. "SG5", "BGM")
///                      ^^^         — identifies the affected segment
///                          ^^^^^^^  — M/C/... status or short discriminator
/// ```
///
/// Example: `"AHB-13001-BGM-M"` encodes the AHB process identifier (`13001`),
/// the affected segment (`BGM`), and the mandatory status (`M`).  Downstream code
/// can extract the PID with a simple string split:
///
/// ```rust
/// # let rule_id = "AHB-13001-BGM-M";
/// if let Some(pid) = rule_id.strip_prefix("AHB-").and_then(|s| s.splitn(2, '-').next()) {
///     println!("process identifier: {pid}"); // "13001"
/// }
/// ```
///
/// For truly arbitrary domain metadata, use the [`context`](Self::context) map and
/// `with_context_entry`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ValidationIssue {
    /// Stable error code, if known.
    pub error_code: Option<&'static str>,
    /// The severity of this issue.
    pub severity: ValidationSeverity,
    /// The error or warning message.
    pub message: String,
    /// Byte offset in the source (if available).
    pub offset: Option<usize>,
    /// Segment tag involved (if known).
    pub segment_tag: Option<String>,
    /// Profile/MIG rule identifier, if applicable.
    ///
    /// By convention, rule IDs are namespaced hierarchically so that downstream
    /// code can extract domain-specific metadata (pack name, process ID, rule scope)
    /// from the string.  See the [`ValidationIssue`] type-level docs for the
    /// recommended naming convention.
    pub rule_id: Option<String>,
    /// Element index (0-based), if known.
    ///
    /// `u8` is sufficient: EDIFACT segments have at most 99 data elements per
    /// the UN/EDIFACT standard, so an index fits comfortably in one byte.
    pub element_index: Option<u8>,
    /// Component index (0-based), if known.
    ///
    /// `u8` is sufficient: composite data elements have at most 99 components
    /// per the UN/EDIFACT standard.
    pub component_index: Option<u8>,
    /// Zero-based occurrence index among segments with the same tag in the message.
    ///
    /// When multiple segments share the same tag (e.g. repeated `DTM` lines),
    /// this field indicates which occurrence (0 = first) was the source of
    /// this issue.  `None` when occurrence tracking is not available for this rule.
    pub segment_occurrence: Option<u16>,
    /// Message reference (`UNH` element 0, DE 0062) that this issue belongs to.
    ///
    /// Populated automatically when the context was built with
    /// `ValidationContextBuilder::with_message_ref`.  Useful in batch processing
    /// where many messages are validated and issues from different messages must
    /// be correlated back to the originating `UNH`/`UNT` envelope.
    pub message_ref: Option<String>,
    /// Suggested remediation (if available).
    pub suggestion: Option<String>,
    /// Segment group (e.g. `"SG6"`) in which the issue occurred, if known.
    ///
    /// Populated by group-aware rule functions when they evaluate sub-slices of a
    /// [`crate::group::SegmentGroupIndexed`] tree.  `None` for flat-segment rules
    /// that do not have group context.
    pub segment_group: Option<Arc<str>>,
    /// Arbitrary domain-specific key-value metadata attached to this issue.
    ///
    /// Use this for information that does not fit into the structured fields above
    /// — for example the PID a downstream MIG crate is validating against, a
    /// trading-partner identifier, or a document UUID:
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity};
    /// let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
    ///     .with_rule_id("AHB-13001-BGM-M")
    ///     .with_context_entry("pid", "13001")
    ///     .with_context_entry("partner", "9900123456789");
    /// assert_eq!(issue.context_get("pid"), Some("13001"));
    /// ```
    ///
    /// The map is empty by default and is never populated by the built-in rules;
    /// it is reserved exclusively for caller-supplied metadata.
    #[cfg_attr(
        feature = "serde",
        serde(skip_serializing_if = "std::collections::HashMap::is_empty")
    )]
    pub context: std::collections::HashMap<String, String>,
}

impl ValidationIssue {
    /// Create a new validation issue.
    pub fn new(severity: ValidationSeverity, message: impl Into<String>) -> Self {
        Self {
            error_code: None,
            severity,
            message: message.into(),
            offset: None,
            segment_tag: None,
            rule_id: None,
            element_index: None,
            component_index: None,
            segment_occurrence: None,
            message_ref: None,
            suggestion: None,
            segment_group: None,
            context: std::collections::HashMap::new(),
        }
    }

    /// Set stable error code metadata.
    pub fn with_error_code(mut self, code: &'static str) -> Self {
        self.error_code = Some(code);
        self
    }

    /// Set the offset for this issue.
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set the segment tag for this issue.
    pub fn with_segment(mut self, tag: impl Into<String>) -> Self {
        self.segment_tag = Some(tag.into());
        self
    }

    /// Set the profile/MIG rule identifier for this issue.
    pub fn with_rule_id(mut self, rule_id: impl Into<String>) -> Self {
        self.rule_id = Some(rule_id.into());
        self
    }

    /// Set the element index (0-based) for this issue.
    pub fn with_element_index(mut self, element_index: u8) -> Self {
        self.element_index = Some(element_index);
        self
    }

    /// Set the component index (0-based) for this issue.
    pub fn with_component_index(mut self, component_index: u8) -> Self {
        self.component_index = Some(component_index);
        self
    }

    /// Set a suggestion for resolving this issue.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Set the zero-based occurrence index for this issue.
    ///
    /// Use this when the same segment tag appears multiple times in a message
    /// and you want to identify which occurrence is affected.
    pub fn with_segment_occurrence(mut self, occurrence: u16) -> Self {
        self.segment_occurrence = Some(occurrence);
        self
    }

    /// Set the message reference (`UNH` element 0) for this issue.
    ///
    /// Use this to correlate an issue back to a specific message in a
    /// multi-message interchange.
    pub fn with_message_ref(mut self, message_ref: impl Into<String>) -> Self {
        self.message_ref = Some(message_ref.into());
        self
    }

    /// Set the segment group (e.g. `"SG6"`) in which this issue occurred.
    ///
    /// Use this from group-aware rule functions that evaluate a sub-slice of a
    /// [`crate::group::SegmentGroupIndexed`] tree so that consumers can identify
    /// the exact group occurrence without re-reading the raw message.
    pub fn with_segment_group(mut self, group: impl Into<Arc<str>>) -> Self {
        self.segment_group = Some(group.into());
        self
    }

    /// Insert a single key-value entry into the domain-specific [`context`](Self::context) map.
    ///
    /// Calling this multiple times accumulates entries; duplicate keys overwrite
    /// the previous value.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity};
    /// let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
    ///     .with_rule_id("AHB-13001-BGM-M")
    ///     .with_context_entry("pid", "13001")
    ///     .with_context_entry("partner", "9900123456789");
    ///
    /// assert_eq!(issue.context_get("pid"), Some("13001"));
    /// assert_eq!(issue.context_get("partner"), Some("9900123456789"));
    /// ```
    pub fn with_context_entry(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    /// Extend the domain-specific [`context`](Self::context) map from an iterator of
    /// `(key, value)` pairs.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity};
    /// let meta = [("pid", "13001"), ("partner", "9900123456789")];
    /// let issue = ValidationIssue::new(ValidationSeverity::Error, "test")
    ///     .with_context_entries(meta);
    ///
    /// assert_eq!(issue.context_get("pid"), Some("13001"));
    /// ```
    pub fn with_context_entries<K, V, I>(mut self, entries: I) -> Self
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
    {
        self.context
            .extend(entries.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Look up a value in the domain-specific [`context`](Self::context) map.
    #[must_use]
    #[inline]
    pub fn context_get(&self, key: &str) -> Option<&str> {
        self.context.get(key).map(String::as_str)
    }

    /// Short label for the severity level, suitable for display.
    #[must_use]
    pub fn severity_label(&self) -> &'static str {
        match self.severity {
            ValidationSeverity::Critical => "CRITICAL",
            ValidationSeverity::Error => "ERROR",
            ValidationSeverity::Warning => "WARNING",
            ValidationSeverity::Info => "INFO",
        }
    }

    // ── Getters ───────────────────────────────────────────────────────────────

    /// Stable error code, if available.
    #[must_use]
    #[inline]
    pub fn error_code(&self) -> Option<&'static str> {
        self.error_code
    }

    /// Byte offset in the source, if available.
    #[must_use]
    #[inline]
    pub fn offset(&self) -> Option<usize> {
        self.offset
    }

    /// Segment tag involved in this issue, if known.
    #[must_use]
    #[inline]
    pub fn segment_tag(&self) -> Option<&str> {
        self.segment_tag.as_deref()
    }

    /// Profile/MIG rule identifier, if applicable.
    #[must_use]
    #[inline]
    pub fn rule_id(&self) -> Option<&str> {
        self.rule_id.as_deref()
    }

    /// Zero-based element index, if known.
    #[must_use]
    #[inline]
    pub fn element_index(&self) -> Option<u8> {
        self.element_index
    }

    /// Zero-based component index, if known.
    #[must_use]
    #[inline]
    pub fn component_index(&self) -> Option<u8> {
        self.component_index
    }

    /// Zero-based occurrence index among same-tag segments, if known.
    #[must_use]
    #[inline]
    pub fn segment_occurrence(&self) -> Option<u16> {
        self.segment_occurrence
    }

    /// Message reference (`UNH` element 0), if set.
    #[must_use]
    #[inline]
    pub fn message_ref(&self) -> Option<&str> {
        self.message_ref.as_deref()
    }

    /// Suggested remediation, if available.
    #[must_use]
    #[inline]
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }

    /// Segment group (e.g. `"SG6"`) in which the issue occurred, if known.
    #[must_use]
    #[inline]
    pub fn segment_group(&self) -> Option<&str> {
        self.segment_group.as_deref()
    }
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.severity_label(), self.message)
    }
}

impl std::error::Error for ValidationIssue {}

/// A collection of validation results: errors, warnings, and info.
///
/// Enables batch validation where all issues are collected instead of failing on the first error.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ValidationReport {
    /// Critical and error-level issues.
    pub(crate) errors: Vec<ValidationIssue>,
    /// Warning-level issues.
    pub(crate) warnings: Vec<ValidationIssue>,
    /// Informational notes.
    pub(crate) infos: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// Returns all error-level [`ValidationIssue`]s in this report.
    pub fn errors(&self) -> &[ValidationIssue] {
        &self.errors
    }

    /// Returns all error-level [`ValidationIssue`]s mutably.
    pub fn errors_mut(&mut self) -> &mut [ValidationIssue] {
        &mut self.errors
    }

    /// Returns all warning-level [`ValidationIssue`]s in this report.
    pub fn warnings(&self) -> &[ValidationIssue] {
        &self.warnings
    }

    /// Returns all warning-level [`ValidationIssue`]s mutably.
    pub fn warnings_mut(&mut self) -> &mut [ValidationIssue] {
        &mut self.warnings
    }

    /// Returns all informational [`ValidationIssue`]s in this report.
    pub fn infos(&self) -> &[ValidationIssue] {
        &self.infos
    }

    /// Returns all informational [`ValidationIssue`]s mutably.
    pub fn infos_mut(&mut self) -> &mut [ValidationIssue] {
        &mut self.infos
    }
    /// Add an error to the report.
    pub fn add_error(&mut self, issue: ValidationIssue) {
        self.errors.push(issue);
    }

    /// Add a warning to the report.
    pub fn add_warning(&mut self, issue: ValidationIssue) {
        self.warnings.push(issue);
    }

    /// Add an info message to the report.
    pub fn add_info(&mut self, issue: ValidationIssue) {
        self.infos.push(issue);
    }

    /// Check if the report has any errors.
    pub fn has_errors(&self) -> bool {
        !self.errors().is_empty()
    }

    /// Check if the report has any warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings().is_empty()
    }

    /// Get the total count of all issues.
    pub fn total_issues(&self) -> usize {
        self.errors().len() + self.warnings().len() + self.infos().len()
    }

    /// Check if the validation passed (no errors, but may have warnings).
    pub fn is_valid(&self) -> bool {
        self.errors().is_empty()
    }

    /// Convert to a `Result`.
    ///
    /// Returns `Ok(self)` when there are no errors.  Returns `Err(self)` when
    /// there is at least one error-level issue, **preserving warnings and infos**
    /// in the `Err` variant so callers can inspect the full report.
    pub fn result(self) -> Result<Self, Self> {
        if self.is_valid() { Ok(self) } else { Err(self) }
    }

    /// Iterate over all issues in severity buckets: errors, warnings, then infos.
    pub fn iter_issues(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.errors()
            .iter()
            .chain(self.warnings().iter())
            .chain(self.infos().iter())
    }

    /// Return `true` if the report contains any issues (errors, warnings, or infos).
    pub fn has_any_issues(&self) -> bool {
        !self.errors().is_empty() || !self.warnings().is_empty() || !self.infos().is_empty()
    }

    /// Drain all issues from `other` into `self`.
    ///
    /// Issues are appended in severity order: errors, warnings, infos.
    /// `other` is left empty after this call.
    pub fn merge(&mut self, mut other: ValidationReport) {
        self.errors.append(&mut other.errors);
        self.warnings.append(&mut other.warnings);
        self.infos.append(&mut other.infos);
    }

    /// Iterate over all issues matching an exact profile/MIG rule identifier.
    ///
    /// Searches errors, warnings, and infos in that order.  Returns a lazy
    /// iterator; collect into `Vec` if you need random access.
    pub fn issues_for_rule_id<'a>(
        &'a self,
        rule_id: &'a str,
    ) -> impl Iterator<Item = &'a ValidationIssue> + 'a {
        self.iter_issues()
            .filter(move |issue| issue.rule_id.as_deref() == Some(rule_id))
    }

    /// Return a cloned report filtered by `pred`.
    fn filter_report<F>(&self, pred: F) -> Self
    where
        F: Fn(&ValidationIssue) -> bool,
    {
        Self {
            errors: self.errors().iter().filter(|i| pred(i)).cloned().collect(),
            warnings: self
                .warnings()
                .iter()
                .filter(|i| pred(i))
                .cloned()
                .collect(),
            infos: self.infos().iter().filter(|i| pred(i)).cloned().collect(),
        }
    }

    /// Return a cloned report containing only issues with an exact rule identifier.
    pub fn filter_by_rule_id(&self, rule_id: &str) -> Self {
        self.filter_report(|issue| issue.rule_id.as_deref() == Some(rule_id))
    }

    /// Return a cloned report containing only issues whose rule identifier starts with `prefix`.
    pub fn filter_by_rule_prefix(&self, prefix: &str) -> Self {
        self.filter_report(|issue| {
            issue
                .rule_id
                .as_deref()
                .is_some_and(|id| id.starts_with(prefix))
        })
    }

    /// Return a cloned report containing only issues that reference `segment_tag`.
    ///
    /// Issues whose `segment_tag` field does not match are dropped; the severity
    /// buckets (errors / warnings / infos) are preserved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ValidationReport, ValidationIssue, ValidationSeverity};
    ///
    /// let mut report = ValidationReport::default();
    /// report.add_error(
    ///     ValidationIssue::new(ValidationSeverity::Error, "BGM missing")
    ///         .with_segment("BGM"),
    /// );
    /// report.add_error(
    ///     ValidationIssue::new(ValidationSeverity::Error, "NAD missing")
    ///         .with_segment("NAD"),
    /// );
    /// let bgm_issues = report.for_segment("BGM");
    /// assert_eq!(bgm_issues.errors().len(), 1);
    /// assert_eq!(bgm_issues.errors()[0].segment_tag.as_deref(), Some("BGM"));
    /// ```
    pub fn for_segment(&self, segment_tag: &str) -> Self {
        self.filter_report(|issue| issue.segment_tag.as_deref() == Some(segment_tag))
    }

    /// Return a deterministic, stable text representation for snapshots and logs.
    pub fn render_deterministic(&self) -> String {
        fn sorted_refs(issues: &[ValidationIssue]) -> Vec<&ValidationIssue> {
            let mut refs: Vec<&ValidationIssue> = issues.iter().collect();
            refs.sort_by(|left, right| {
                left.offset
                    .unwrap_or(usize::MAX)
                    .cmp(&right.offset.unwrap_or(usize::MAX))
                    .then_with(|| {
                        left.segment_tag
                            .as_deref()
                            .unwrap_or("")
                            .cmp(right.segment_tag.as_deref().unwrap_or(""))
                    })
                    .then_with(|| {
                        left.rule_id
                            .as_deref()
                            .unwrap_or("")
                            .cmp(right.rule_id.as_deref().unwrap_or(""))
                    })
                    .then_with(|| {
                        left.element_index
                            .unwrap_or(u8::MAX)
                            .cmp(&right.element_index.unwrap_or(u8::MAX))
                    })
                    .then_with(|| {
                        left.component_index
                            .unwrap_or(u8::MAX)
                            .cmp(&right.component_index.unwrap_or(u8::MAX))
                    })
                    .then_with(|| {
                        left.error_code
                            .unwrap_or("")
                            .cmp(right.error_code.unwrap_or(""))
                    })
                    .then_with(|| left.message.cmp(&right.message))
            });
            refs
        }

        fn render_issue_line(out: &mut String, issue: &ValidationIssue) {
            use std::fmt::Write as _;
            out.push_str("    - ");
            out.push_str(&issue.message);
            if let Some(code) = issue.error_code {
                out.push_str(" [");
                out.push_str(code);
                out.push(']');
            }
            if let Some(seg) = &issue.segment_tag {
                out.push_str(" [segment=");
                out.push_str(seg);
                out.push(']');
            }
            if let Some(rule_id) = &issue.rule_id {
                out.push_str(" [rule=");
                out.push_str(rule_id);
                out.push(']');
            }
            if let Some(element_index) = issue.element_index {
                write!(out, " [element={element_index}]").ok();
            }
            if let Some(component_index) = issue.component_index {
                write!(out, " [component={component_index}]").ok();
            }
            if let Some(offset) = issue.offset {
                write!(out, " [offset={offset}]").ok();
            }
            if let Some(suggestion) = &issue.suggestion {
                out.push_str(" [hint=");
                out.push_str(suggestion);
                out.push(']');
            }
        }

        use std::fmt::Write as _;
        let mut out = String::from("Validation Report:");
        let errors = sorted_refs(self.errors());
        let warnings = sorted_refs(self.warnings());
        let infos = sorted_refs(self.infos());

        if !errors.is_empty() {
            write!(out, "\n  Errors ({})", errors.len()).ok();
            for issue in &errors {
                out.push('\n');
                render_issue_line(&mut out, issue);
            }
        }
        if !warnings.is_empty() {
            write!(out, "\n  Warnings ({})", warnings.len()).ok();
            for issue in &warnings {
                out.push('\n');
                render_issue_line(&mut out, issue);
            }
        }
        if !infos.is_empty() {
            write!(out, "\n  Info ({})", infos.len()).ok();
            for issue in &infos {
                out.push('\n');
                render_issue_line(&mut out, issue);
            }
        }

        out
    }
}

#[cfg(feature = "diagnostics")]
impl miette::Diagnostic for ValidationReport {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new("VALIDATION"))
    }

    fn severity(&self) -> Option<miette::Severity> {
        if self.has_errors() {
            Some(miette::Severity::Error)
        } else if self.has_warnings() {
            Some(miette::Severity::Warning)
        } else {
            Some(miette::Severity::Advice)
        }
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        let msg = format!(
            "Validation found {} error(s), {} warning(s), {} info(s)",
            self.errors().len(),
            self.warnings().len(),
            self.infos().len()
        );
        Some(Box::new(msg))
    }
}

impl std::fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.render_deterministic())
    }
}

impl std::error::Error for ValidationReport {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_report_collects_errors() {
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "Test error")
                .with_segment("BGM")
                .with_offset(42),
        );
        report.add_warning(ValidationIssue::new(
            ValidationSeverity::Warning,
            "Test warning",
        ));

        assert!(report.has_errors());
        assert!(report.has_warnings());
        assert_eq!(report.total_issues(), 2);
        assert!(!report.is_valid());
    }

    #[test]
    fn validation_report_result_conversion() {
        let mut report = ValidationReport::default();
        report.add_error(ValidationIssue::new(
            ValidationSeverity::Error,
            "Critical issue",
        ));

        let result = report.result();
        assert!(result.is_err());
    }

    #[test]
    fn validation_report_passes_when_no_errors() {
        let mut report = ValidationReport::default();
        report.add_warning(ValidationIssue::new(
            ValidationSeverity::Warning,
            "Just a warning",
        ));

        assert!(report.is_valid());
        assert!(report.result().is_ok());
    }

    #[test]
    fn validation_issue_builder() {
        let issue = ValidationIssue::new(ValidationSeverity::Warning, "test message")
            .with_error_code("E013")
            .with_offset(100)
            .with_segment("NAD")
            .with_rule_id("DEMO-P001")
            .with_element_index(1)
            .with_component_index(2)
            .with_suggestion("Check element count");

        assert_eq!(issue.error_code, Some("E013"));
        assert_eq!(issue.message, "test message");
        assert_eq!(issue.offset, Some(100));
        assert_eq!(issue.segment_tag, Some("NAD".to_owned()));
        assert_eq!(issue.rule_id, Some("DEMO-P001".to_owned()));
        assert_eq!(issue.element_index, Some(1));
        assert_eq!(issue.component_index, Some(2));
        assert_eq!(issue.suggestion, Some("Check element count".to_owned()));
    }

    #[test]
    fn validation_report_display() {
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "Error 1")
                .with_error_code("E011")
                .with_offset(8),
        );
        report.add_warning(ValidationIssue::new(
            ValidationSeverity::Warning,
            "Warning 1",
        ));
        report.add_info(ValidationIssue::new(ValidationSeverity::Info, "Info 1"));

        let display_str = format!("{}", report);
        assert!(display_str.contains("Errors (1)"));
        assert!(display_str.contains("Warnings (1)"));
        assert!(display_str.contains("Info (1)"));
        assert!(display_str.contains("[E011]"));
    }

    #[test]
    fn validation_report_render_is_deterministic() {
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "later")
                .with_segment("BGM")
                .with_offset(20),
        );
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "earlier")
                .with_segment("UNH")
                .with_offset(1),
        );

        let rendered = report.render_deterministic();
        let first = rendered.find("earlier").expect("missing first issue");
        let second = rendered.find("later").expect("missing second issue");
        assert!(first < second, "expected deterministic sort by offset");
    }

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

    #[test]
    fn validation_report_can_filter_by_rule_id() {
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "orders policy blocked")
                .with_rule_id("ORDERS-P001"),
        );
        report.add_warning(
            ValidationIssue::new(ValidationSeverity::Warning, "invoic policy warning")
                .with_rule_id("INVOIC-P001"),
        );
        report.add_info(
            ValidationIssue::new(ValidationSeverity::Info, "orders policy info")
                .with_rule_id("ORDERS-P002"),
        );

        let only_orders_block = report.filter_by_rule_id("ORDERS-P001");
        assert_eq!(only_orders_block.errors().len(), 1);
        assert!(only_orders_block.warnings().is_empty());
        assert!(only_orders_block.infos().is_empty());

        let orders_family = report.filter_by_rule_prefix("ORDERS-");
        assert_eq!(orders_family.total_issues(), 2);
        assert!(orders_family.has_errors());
        assert!(!orders_family.has_warnings());

        let exact: Vec<_> = report.issues_for_rule_id("INVOIC-P001").collect();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].message, "invoic policy warning");
    }

    #[test]
    fn validation_issue_context_map_builder() {
        let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
            .with_rule_id("AHB-13001-BGM-M")
            .with_context_entry("pid", "13001")
            .with_context_entry("partner", "9900123456789");

        assert_eq!(issue.context_get("pid"), Some("13001"));
        assert_eq!(issue.context_get("partner"), Some("9900123456789"));
        assert_eq!(issue.context_get("missing"), None);
    }

    #[test]
    fn validation_issue_context_map_extend() {
        let meta = [("pid", "13001"), ("partner", "9900123456789")];
        let issue =
            ValidationIssue::new(ValidationSeverity::Error, "test").with_context_entries(meta);

        assert_eq!(issue.context_get("pid"), Some("13001"));
        assert_eq!(issue.context_get("partner"), Some("9900123456789"));
    }

    #[test]
    fn validation_issue_context_overwrite() {
        let issue = ValidationIssue::new(ValidationSeverity::Warning, "demo")
            .with_context_entry("pid", "old")
            .with_context_entry("pid", "new");
        assert_eq!(issue.context_get("pid"), Some("new"));
    }

    #[test]
    fn validation_issue_empty_context_not_cloned_into_severity_display() {
        // Ensure that a default-constructed issue has an empty context map.
        let issue = ValidationIssue::new(ValidationSeverity::Info, "no context");
        assert!(issue.context.is_empty());
    }

    #[test]
    fn rule_id_prefix_convention_pid_extraction() {
        // Validate the documented rule_id prefix convention: "AHB-<pid>-<tag>-<status>"
        let rule_id = "AHB-13001-BGM-M";
        let pid = rule_id
            .strip_prefix("AHB-")
            .and_then(|s| s.splitn(2, '-').next());
        assert_eq!(pid, Some("13001"));
    }
}
