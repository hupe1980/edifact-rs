use thiserror::Error;

/// Wrapper around [`std::io::Error`] that implements [`PartialEq`] by comparing [`std::io::ErrorKind`].
///
/// This allows `EdifactError` to derive `PartialEq` without requiring `std::io::Error: PartialEq`.
#[derive(Debug)]
pub struct IoError(pub std::io::Error);

impl PartialEq for IoError {
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
    #[error("segment count mismatch in message {message_ref}: UNT declared {expected}, found {actual}")]
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
    /// If present, the UNA segment must be exactly 9 bytes: "UNA" followed by 6 service characters.
    #[error("invalid UNA service string advice: must be exactly 9 bytes")]
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

    /// Aggregate validation failure from strict validation mode.
    #[error("validation failed with {error_count} issue(s); first issue: {first_message}")]
    ValidationFailed {
        /// Number of collected validation issues.
        error_count: usize,
        /// First issue message for quick context.
        first_message: String,
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
            Self::ValidationFailed { .. } => "E018",
            Self::InvalidReleaseSequence { .. } => "E019",
            Self::SegmentTooLong { .. } => "E020",
            Self::MissingRequiredComponent { .. } => "E021",
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
            Self::InvalidUna => {
                Some("UNA must be exactly 9 bytes: 'UNA' followed by 6 service characters")
            }
            Self::MissingRequiredElement { .. } => {
                Some("Provide all mandatory elements for the segment per directory rules")
            }
            Self::MissingRequiredComponent { .. } => {
                Some("Provide all mandatory components for the composite element per directory rules")
            }
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
            Self::ValidationFailed { .. }
            | Self::MessageCountMismatch { .. }
            | Self::SegmentCountMismatch { .. }
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
            Self::SegmentCountMismatch { expected, actual, message_ref } => Some(Box::new(format!(
                "UNT for message {message_ref} declares {expected} segment(s) but {actual} were found. \
                 Check the UNT segment count",
            ))),
            Self::InvalidSegmentTag(tag) => Some(Box::new(format!(
                "Segment tag '{tag}' must be exactly 3 ASCII uppercase letters",
            ))),
            Self::MissingRequiredElement { tag, element_index } => Some(Box::new(format!(
                "Segment {tag} requires element at index {element_index}",
            ))),
            Self::MissingRequiredComponent { tag, element_index, component_index } => {
                Some(Box::new(format!(
                    "Segment {tag} element {element_index} requires component at index {component_index}",
                )))
            }
            Self::Io(e) => Some(Box::new(format!("I/O error: {e}"))),
            Self::InvalidSegmentForMessage { tag, message_type, .. } => Some(Box::new(format!(
                "Segment {tag} should not appear in a {message_type} message. \
                 Check the directory definition",
            ))),
            Self::InvalidElementCount { tag, min, max, actual, .. } => Some(Box::new(format!(
                "Segment {tag} should have between {min} and {max} elements, but has {actual}. \
                 Check segment structure",
            ))),
            Self::InvalidComponentCount { tag, element_index, expected, actual, .. } => {
                Some(Box::new(format!(
                    "In segment {tag}, element {element_index} should have {expected} components \
                     but has {actual}. Check element structure",
                )))
            }
            Self::InvalidCodeValue { tag, element_index, value, code_list, .. } => {
                Some(Box::new(format!(
                    "Value '{value}' in segment {tag} element {element_index} is not in the \
                     {code_list} code list. Check the directory for valid codes",
                )))
            }
            Self::MissingSegment { tag, expected_position } => Some(Box::new(format!(
                "Segment {tag} is required at position {expected_position} but is missing. \
                 Add this segment to the message",
            ))),
            Self::QualifierMismatch { tag, actual, expected, .. } => Some(Box::new(format!(
                "Segment {tag} has qualifier '{actual}' but expected '{expected}'. \
                 Check the segment's first component",
            ))),
            Self::ConditionalRequirementNotMet { tag, element_index, condition, .. } => {
                Some(Box::new(format!(
                    "In segment {tag}, element {element_index} is conditionally required when: \
                     {condition}. Check if the condition is met",
                )))
            }
            Self::ValidationFailed { error_count, first_message } => Some(Box::new(format!(
                "Validation found {error_count} issue(s). Start by fixing: {first_message}",
            ))),
            Self::SegmentTooLong { offset, limit } => Some(Box::new(format!(
                "Segment starting at byte offset {offset} exceeds the {limit}-byte limit. \
                 Use ReaderConfig::max_segment_bytes to adjust the limit if needed, \
                 or verify the input for a missing segment terminator",
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

/// A structured validation issue.
#[derive(Debug, Clone, PartialEq)]
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
    /// Suggested remediation (if available).
    pub suggestion: Option<String>,
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
            suggestion: None,
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
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    /// Critical and error-level issues.
    pub errors: Vec<ValidationIssue>,
    /// Warning-level issues.
    pub warnings: Vec<ValidationIssue>,
    /// Informational notes.
    pub infos: Vec<ValidationIssue>,
}

impl ValidationReport {
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
        !self.errors.is_empty()
    }

    /// Check if the report has any warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Get the total count of all issues.
    pub fn total_issues(&self) -> usize {
        self.errors.len() + self.warnings.len() + self.infos.len()
    }

    /// Check if the validation passed (no errors, but may have warnings).
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Convert to a `Result`.
    ///
    /// Returns `Ok(self)` when there are no errors.  Returns `Err(self)` when
    /// there is at least one error-level issue, **preserving warnings and infos**
    /// in the `Err` variant so callers can inspect the full report.
    pub fn result(self) -> Result<Self, Self> {
        if self.is_valid() {
            Ok(self)
        } else {
            Err(self)
        }
    }

    /// Iterate over all issues in severity buckets: errors, warnings, then infos.
    pub fn iter_issues(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.errors
            .iter()
            .chain(self.warnings.iter())
            .chain(self.infos.iter())
    }

    /// Iterate over all issues in severity order.  Alias for [`iter_issues`][Self::iter_issues].
    pub fn issues(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.iter_issues()
    }

    /// Return `true` if the report contains any issues (errors, warnings, or infos).
    pub fn has_any_issues(&self) -> bool {
        !self.errors.is_empty() || !self.warnings.is_empty() || !self.infos.is_empty()
    }

    /// Iterate over all issues matching an exact profile/MIG rule identifier.
    ///
    /// Searches errors, warnings, and infos in that order.  Returns a lazy
    /// iterator; collect into `Vec` if you need random access.
    pub fn issues_for_rule_id(&self, rule_id: &str) -> impl Iterator<Item = &ValidationIssue> + '_ {
        let rule_id = rule_id.to_owned();
        self.iter_issues()
            .filter(move |issue| issue.rule_id.as_deref() == Some(&rule_id))
    }

    /// Return a cloned report filtered by `pred`.
    fn filter_report<F>(&self, pred: F) -> Self
    where
        F: Fn(&ValidationIssue) -> bool,
    {
        Self {
            errors: self.errors.iter().filter(|i| pred(i)).cloned().collect(),
            warnings: self.warnings.iter().filter(|i| pred(i)).cloned().collect(),
            infos: self.infos.iter().filter(|i| pred(i)).cloned().collect(),
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
        let errors = sorted_refs(&self.errors);
        let warnings = sorted_refs(&self.warnings);
        let infos = sorted_refs(&self.infos);

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
            self.errors.len(),
            self.warnings.len(),
            self.infos.len()
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
        assert_eq!(only_orders_block.errors.len(), 1);
        assert!(only_orders_block.warnings.is_empty());
        assert!(only_orders_block.infos.is_empty());

        let orders_family = report.filter_by_rule_prefix("ORDERS-");
        assert_eq!(orders_family.total_issues(), 2);
        assert!(orders_family.has_errors());
        assert!(!orders_family.has_warnings());

        let exact: Vec<_> = report.issues_for_rule_id("INVOIC-P001").collect();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].message, "invoic policy warning");
    }
}
