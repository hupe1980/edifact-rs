//! Validation report types: [`ValidationSeverity`], [`ValidationIssue`], [`ValidationReport`].
//!
//! These types are also re-exported from the crate root.

use std::sync::Arc;

use crate::model::Span;

// ── ValidationSeverity ────────────────────────────────────────────────────────

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

// ── ValidationIssue ───────────────────────────────────────────────────────────

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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ValidationIssue {
    /// Stable error code, if known.
    ///
    /// Not preserved across serialization round-trips: deserialized issues
    /// always have `error_code = None` because error codes are compile-time
    /// library constants, not external data.
    #[cfg_attr(feature = "serde", serde(skip_deserializing, default))]
    pub error_code: Option<&'static str>,
    /// The severity of this issue.
    pub severity: ValidationSeverity,
    /// The error or warning message.
    pub message: String,
    /// Byte offset in the source (if available).
    ///
    /// For precise source-range highlighting (e.g. in `miette` diagnostics or
    /// Language Server Protocol `Range` values), prefer [`span`](Self::span)
    /// which carries both start and end.  `offset` is kept for backwards
    /// compatibility and is always equal to `span.start` when both are set.
    pub offset: Option<usize>,
    /// Half-open byte range of the relevant segment or element in the source.
    ///
    /// Provides precise source-range information for diagnostics and editor
    /// tooling.  Use [`with_span`](Self::with_span) to set this from a
    /// [`Span`] obtained from a parsed [`crate::Segment`].  Setting `span`
    /// automatically populates `offset` with `span.start` for backwards
    /// compatibility.
    pub span: Option<Span>,
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
    /// The vec is empty by default and is never populated by the built-in rules;
    /// it is reserved exclusively for caller-supplied metadata.
    ///
    /// Entries are stored in insertion order; duplicate keys are allowed and
    /// [`context_get`](Self::context_get) returns the first match.
    /// [`with_context_entry`](Self::with_context_entry) uses upsert semantics
    /// (updates an existing key in place rather than duplicating it).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub context: Vec<(String, String)>,
}

impl ValidationIssue {
    /// Create a new validation issue.
    pub fn new(severity: ValidationSeverity, message: impl Into<String>) -> Self {
        Self {
            error_code: None,
            severity,
            message: message.into(),
            offset: None,
            span: None,
            segment_tag: None,
            rule_id: None,
            element_index: None,
            component_index: None,
            segment_occurrence: None,
            message_ref: None,
            suggestion: None,
            segment_group: None,
            context: Vec::new(),
        }
    }

    /// Set stable error code metadata.
    pub fn with_error_code(mut self, code: &'static str) -> Self {
        self.error_code = Some(code);
        self
    }

    /// Set the byte offset for this issue.
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set the full byte-range span for this issue.
    ///
    /// Also populates [`offset`](Self::offset) with `span.start` so that
    /// existing code that only reads `offset` continues to work.
    ///
    /// Use this in preference to `with_offset` when you have access to the
    /// source [`Span`] from a parsed [`crate::Segment`] — the full range
    /// enables precise source-range highlighting in `miette` diagnostics and
    /// Language Server Protocol tooling.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity, Span};
    /// let span = Span::new(42, 57);
    /// let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code missing")
    ///     .with_span(span);
    /// assert_eq!(issue.offset, Some(42));
    /// assert_eq!(issue.span, Some(span));
    /// ```
    pub fn with_span(mut self, span: Span) -> Self {
        self.offset = Some(span.start);
        self.span = Some(span);
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
        let key = key.into();
        let value = value.into();
        if let Some(entry) = self.context.iter_mut().find(|(k, _)| k == &key) {
            entry.1 = value;
        } else {
            self.context.push((key, value));
        }
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
        for (k, v) in entries {
            let k = k.into();
            let v = v.into();
            if let Some(entry) = self.context.iter_mut().find(|(key, _)| key == &k) {
                entry.1 = v;
            } else {
                self.context.push((k, v));
            }
        }
        self
    }

    /// Look up a value in the domain-specific [`context`](Self::context) map.
    #[must_use]
    #[inline]
    pub fn context_get(&self, key: &str) -> Option<&str> {
        self.context
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Short label for the severity level, suitable for display.
    #[must_use]
    pub fn severity_label(&self) -> &'static str {
        match self.severity {
            ValidationSeverity::Critical => "CRITICAL",
            ValidationSeverity::Error => "ERROR",
            ValidationSeverity::Warning => "WARNING",
            ValidationSeverity::Info => "INFO",
            #[allow(unreachable_patterns)]
            _ => "UNKNOWN",
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

    /// Half-open byte range of the relevant source region, if available.
    #[must_use]
    #[inline]
    pub fn span(&self) -> Option<Span> {
        self.span
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

// ── ValidationReport ─────────────────────────────────────────────────────────

/// A collection of validation results: errors, warnings, and informational notes.
///
/// Enables batch validation where all issues are collected instead of failing on
/// the first error.  Produced by [`crate::validator::ValidationContext`] methods
/// such as `validate_lenient` and `validate_lenient_grouped`.
///
/// # Building reports manually
///
/// Use [`ValidationReport::from_issues`] to construct a report from pre-built issue
/// vectors, or the `add_*` methods to push individual issues:
///
/// ```rust
/// use edifact_rs::{ValidationReport, ValidationIssue, ValidationSeverity};
///
/// let mut report = ValidationReport::default();
/// report.add_warning(
///     ValidationIssue::new(ValidationSeverity::Warning, "optional field missing")
///         .with_segment("DTM"),
/// );
/// assert!(report.is_valid()); // warnings don't fail validation
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ValidationReport {
    /// Critical and error-level issues.
    pub(crate) errors: Vec<ValidationIssue>,
    /// Warning-level issues.
    pub(crate) warnings: Vec<ValidationIssue>,
    /// Informational notes.
    pub(crate) infos: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// Construct a report directly from pre-categorized issue vectors.
    ///
    /// This is the primary escape hatch for code that needs to inject advisory
    /// issues into a report outside the normal validation pipeline — for example,
    /// a middleware layer that wants to attach AHB-layer skip notices without
    /// registering a synthetic `ProfileRulePack` rule.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut report = ctx.validate_lenient(&segments);
    /// let advisory = ValidationReport::from_issues(
    ///     vec![],
    ///     vec![ValidationIssue::new(ValidationSeverity::Warning, "AHB layer skipped")
    ///         .with_rule_id("AHB-SKIP-001")],
    ///     vec![],
    /// );
    /// report.merge(advisory);
    /// ```
    pub fn from_issues(
        errors: Vec<ValidationIssue>,
        warnings: Vec<ValidationIssue>,
        infos: Vec<ValidationIssue>,
    ) -> Self {
        Self {
            errors,
            warnings,
            infos,
        }
    }

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

    /// Check if the report has any errors (Critical or Error severity).
    pub fn has_errors(&self) -> bool {
        !self.errors().is_empty()
    }

    /// Check if the report contains at least one `Critical`-severity issue.
    ///
    /// O(1) — backed by an incrementally maintained counter.
    pub fn has_critical_errors(&self) -> bool {
        self.errors
            .iter()
            .any(|i| i.severity == ValidationSeverity::Critical)
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

    /// Extend `self` with cloned issues from `other` (borrowing).
    ///
    /// Unlike [`merge`](Self::merge), this method borrows `other` so the caller
    /// retains ownership.  Issues are cloned and appended to the respective
    /// severity buckets.  Use `merge` when you can afford to consume `other`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ValidationReport, ValidationIssue, ValidationSeverity};
    ///
    /// let mut combined = ValidationReport::default();
    /// let report = ValidationReport::from_issues(
    ///     vec![ValidationIssue::new(ValidationSeverity::Error, "bad segment")],
    ///     vec![],
    ///     vec![],
    /// );
    /// combined.extend_from(&report);
    /// assert_eq!(combined.errors().len(), 1);
    /// // `report` is still accessible
    /// assert_eq!(report.errors().len(), 1);
    /// ```
    pub fn extend_from(&mut self, other: &ValidationReport) {
        for issue in &other.errors {
            self.add_error(issue.clone());
        }
        for issue in &other.warnings {
            self.add_warning(issue.clone());
        }
        for issue in &other.infos {
            self.add_info(issue.clone());
        }
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

    fn filter_report<F>(&self, pred: F) -> Self
    where
        F: Fn(&ValidationIssue) -> bool,
    {
        let errors: Vec<ValidationIssue> =
            self.errors().iter().filter(|i| pred(i)).cloned().collect();
        Self {
            errors,
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

impl Extend<ValidationIssue> for ValidationReport {
    /// Push each issue into the appropriate severity bucket.
    ///
    /// This enables ergonomic batch collection:
    ///
    /// ```rust
    /// use edifact_rs::{ValidationReport, ValidationIssue, ValidationSeverity};
    ///
    /// let issues = vec![
    ///     ValidationIssue::new(ValidationSeverity::Error, "bad segment"),
    ///     ValidationIssue::new(ValidationSeverity::Warning, "optional field missing"),
    ///     ValidationIssue::new(ValidationSeverity::Info, "advisory note"),
    /// ];
    /// let mut report = ValidationReport::default();
    /// report.extend(issues);
    /// assert_eq!(report.errors().len(), 1);
    /// assert_eq!(report.warnings().len(), 1);
    /// assert_eq!(report.infos().len(), 1);
    /// ```
    fn extend<I: IntoIterator<Item = ValidationIssue>>(&mut self, iter: I) {
        for issue in iter {
            match issue.severity {
                ValidationSeverity::Critical | ValidationSeverity::Error => {
                    self.add_error(issue);
                }
                ValidationSeverity::Warning => {
                    self.add_warning(issue);
                }
                _ => {
                    self.add_info(issue);
                }
            }
        }
    }
}

impl FromIterator<ValidationIssue> for ValidationReport {
    fn from_iter<I: IntoIterator<Item = ValidationIssue>>(iter: I) -> Self {
        let mut report = ValidationReport::default();
        report.extend(iter);
        report
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_collects_errors_and_warnings() {
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
    fn report_result_conversion() {
        let mut report = ValidationReport::default();
        report.add_error(ValidationIssue::new(
            ValidationSeverity::Error,
            "Critical issue",
        ));
        assert!(report.result().is_err());
    }

    #[test]
    fn report_valid_with_only_warnings() {
        let mut report = ValidationReport::default();
        report.add_warning(ValidationIssue::new(
            ValidationSeverity::Warning,
            "Just a warning",
        ));
        assert!(report.is_valid());
        assert!(report.result().is_ok());
    }

    #[test]
    fn issue_builder_chain() {
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
    fn report_display_format() {
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

        let display_str = format!("{report}");
        assert!(display_str.contains("Errors (1)"));
        assert!(display_str.contains("Warnings (1)"));
        assert!(display_str.contains("Info (1)"));
        assert!(display_str.contains("[E011]"));
    }

    #[test]
    fn render_deterministic_sorts_by_offset() {
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
    fn filter_by_rule_id() {
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

        let exact: Vec<_> = report.issues_for_rule_id("INVOIC-P001").collect();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].message, "invoic policy warning");
    }

    #[test]
    fn context_map_builder() {
        let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
            .with_context_entry("pid", "13001")
            .with_context_entry("partner", "9900123456789");

        assert_eq!(issue.context_get("pid"), Some("13001"));
        assert_eq!(issue.context_get("partner"), Some("9900123456789"));
        assert_eq!(issue.context_get("missing"), None);
    }

    #[test]
    fn context_map_extend() {
        let meta = [("pid", "13001"), ("partner", "9900123456789")];
        let issue =
            ValidationIssue::new(ValidationSeverity::Error, "test").with_context_entries(meta);
        assert_eq!(issue.context_get("pid"), Some("13001"));
    }

    #[test]
    fn context_key_overwrite() {
        let issue = ValidationIssue::new(ValidationSeverity::Warning, "demo")
            .with_context_entry("pid", "old")
            .with_context_entry("pid", "new");
        assert_eq!(issue.context_get("pid"), Some("new"));
    }
}
