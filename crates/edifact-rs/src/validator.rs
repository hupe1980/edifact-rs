//! Validation pipeline for structural and semantic EDIFACT checks.

use crate::{EdifactError, Segment, ValidationIssue, ValidationReport, ValidationSeverity};

/// A profile rule that can be added to a [`ProfileRulePack`].
///
/// Implement this trait to create reusable, composable profile rules for
/// EDIFACT message validation.
pub trait ProfileRule: Send + Sync {
    /// Evaluate the rule against the given segments.
    ///
    /// Return `Some(issue)` if the rule is violated, or `None` if the segments pass.
    fn evaluate(&self, segments: &[Segment<'_>]) -> Option<ValidationIssue>;
}

struct ClosureProfileRule<F>(F);

impl<F> ProfileRule for ClosureProfileRule<F>
where
    F: for<'a> Fn(&[Segment<'a>]) -> Option<ValidationIssue> + Send + Sync,
{
    fn evaluate(&self, segments: &[Segment<'_>]) -> Option<ValidationIssue> {
        (self.0)(segments)
    }
}

/// A profile/MIG rule pack that can be plugged into `ValidationContext`.
pub struct ProfileRulePack {
    name: String,
    message_types: Vec<String>,
    rules: Vec<Box<dyn ProfileRule + Send + Sync>>,
}

impl ProfileRulePack {
    /// Create an empty rule pack.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            message_types: Vec::new(),
            rules: Vec::new(),
        }
    }

    /// Alias for [`ProfileRulePack::new`] for ergonomic fluent-builder use.
    ///
    /// Because all builder methods (`for_message_type`, `with_rule_fn`, `merge`) are
    /// consuming methods on `ProfileRulePack` itself, no separate builder type is needed:
    ///
    /// ```rust,ignore
    /// let pack = ProfileRulePack::builder("MY-PACK")
    ///     .for_message_type("ORDERS")
    ///     .with_rule_fn(|_segs| None);
    /// ```
    pub fn builder(name: impl Into<String>) -> Self {
        Self::new(name)
    }

    /// Return the pack name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the message types this pack is scoped to.
    pub fn message_types(&self) -> &[String] {
        &self.message_types
    }

    /// Return the number of rules in this pack.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Restrict this pack to one or more EDIFACT message types from the `UNH` segment.
    ///
    /// When a pack has one or more message-type restrictions, its rules are only evaluated
    /// against messages whose `UNH` element 1, component 0 matches one of the registered
    /// types (e.g. `"ORDERS"`, `"INVOIC"`).
    ///
    /// # Silent-skip behaviour
    ///
    /// If the input segments do not contain a `UNH` segment, or if the `UNH` message-type
    /// element is absent, the pack will **silently skip all rules** rather than returning an
    /// error.  This is intentional: without a readable message type the pack cannot
    /// determine whether its rules apply, so it errs on the side of no false positives.
    ///
    /// If you need a hard failure on a missing `UNH`, add a dedicated [`ProfileRule`] that
    /// checks for the segment's presence before other rules run.
    pub fn for_message_type(mut self, message_type: impl Into<String>) -> Self {
        let message_type = message_type.into();
        if !self.message_types.contains(&message_type) {
            self.message_types.push(message_type);
        }
        self
    }

    /// Add one externally authored rule using only public API.
    pub fn with_rule_fn<F>(mut self, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>]) -> Option<ValidationIssue> + Send + Sync + 'static,
    {
        self.rules.push(Box::new(ClosureProfileRule(rule)));
        self
    }

    /// Add a rule that implements [`ProfileRule`].
    pub fn with_rule(mut self, rule: impl ProfileRule + 'static) -> Self {
        self.rules.push(Box::new(rule));
        self
    }

    /// Merge two packs into one combined pack.
    pub fn merge(mut self, mut other: Self) -> Self {
        for message_type in other.message_types.drain(..) {
            if !self.message_types.contains(&message_type) {
                self.message_types.push(message_type);
            }
        }
        self.rules.append(&mut other.rules);
        self
    }
}

impl Validator for ProfileRulePack {
    fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
        let message_type = segments
            .iter()
            .find(|segment| segment.tag == "UNH")
            .and_then(|segment| segment.get_element(1))
            .and_then(|element| element.get_component(0));
        if !self.message_types.is_empty()
            && !message_type.is_some_and(|mt| self.message_types.iter().any(|t| t == mt))
        {
            return;
        }

        for rule in &self.rules {
            if let Some(issue) = rule.evaluate(segments) {
                match issue.severity {
                    ValidationSeverity::Critical | ValidationSeverity::Error => {
                        report.add_error(issue);
                    }
                    ValidationSeverity::Warning => {
                        report.add_warning(issue);
                    }
                    ValidationSeverity::Info => {
                        report.add_info(issue);
                    }
                }
            }
        }
    }
}

impl std::fmt::Debug for ProfileRulePack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileRulePack")
            .field("name", &self.name)
            .field("message_types", &self.message_types)
            .field("rule_count", &self.rules.len())
            .finish()
    }
}

/// Validation layers used by [`ValidationContext`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationLayer {
    /// Directory structure checks (segment presence/order/arity).
    Structure,
    /// Directory code-list checks.
    CodeList,
    /// Downstream profile-pack checks.
    Profile,
}

struct LayeredValidator {
    layer: ValidationLayer,
    validator: Box<dyn Validator + Send + Sync>,
}

/// Runtime validation context for progressive layered validation.
pub struct ValidationContext {
    validators: Vec<LayeredValidator>,
    structure_enabled: bool,
    code_list_enabled: bool,
    profile_enabled: bool,
    message_type: Option<String>,
}

/// Builder for [`ValidationContext`].
#[must_use = "call `.build()` to produce a `ValidationContext`"]
pub struct ValidationContextBuilder {
    inner: ValidationContext,
}

impl Default for ValidationContextBuilder {
    /// Default context builder has all layers enabled, same as [`ValidationContextBuilder::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationContextBuilder {
    /// Create a new context builder with all layers enabled.
    pub fn new() -> Self {
        Self {
            inner: ValidationContext {
                validators: Vec::new(),
                structure_enabled: true,
                code_list_enabled: true,
                profile_enabled: true,
                message_type: None,
            },
        }
    }

    /// Set message type metadata for downstream validators.
    pub fn with_message_type(mut self, message_type: impl Into<String>) -> Self {
        self.inner.message_type = Some(message_type.into());
        let configured = self.inner.message_type.as_deref();
        for layered in &mut self.inner.validators {
            layered.validator.set_message_type(configured);
        }
        self
    }

    /// Enable/disable structure validators.
    pub fn structure(mut self, enabled: bool) -> Self {
        self.inner.structure_enabled = enabled;
        self
    }

    /// Enable/disable code-list validators.
    pub fn code_list(mut self, enabled: bool) -> Self {
        self.inner.code_list_enabled = enabled;
        self
    }

    /// Enable/disable profile validators.
    pub fn profile(mut self, enabled: bool) -> Self {
        self.inner.profile_enabled = enabled;
        self
    }

    /// Add a validator assigned to `layer`.
    pub fn with_validator<V>(mut self, layer: ValidationLayer, mut validator: V) -> Self
    where
        V: Validator + 'static,
    {
        validator.set_message_type(self.inner.message_type.as_deref());
        self.inner.validators.push(LayeredValidator {
            layer,
            validator: Box::new(validator),
        });
        self
    }

    /// Add a profile rule pack to the profile layer.
    pub fn with_profile_pack(mut self, mut pack: ProfileRulePack) -> Self {
        pack.set_message_type(self.inner.message_type.as_deref());
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Profile,
            validator: Box::new(pack),
        });
        self
    }

    /// Finalize builder and create context.
    #[must_use = "call `.validate_lenient()` or `.validate_strict()` on the resulting context"]
    pub fn build(self) -> ValidationContext {
        self.inner
    }
}

impl ValidationContext {
    /// Start building a validation context.
    pub fn builder() -> ValidationContextBuilder {
        ValidationContextBuilder::new()
    }

    /// Execute validators in lenient mode for enabled layers.
    pub fn validate_lenient(&self, segments: &[Segment<'_>]) -> ValidationReport {
        let mut report = ValidationReport::default();
        for lv in &self.validators {
            if self.layer_enabled(lv.layer) {
                lv.validator.validate_batch(segments, &mut report);
            }
        }
        report
    }

    /// Execute validators in strict mode for enabled layers.
    pub fn validate_strict(
        &self,
        segments: &[Segment<'_>],
    ) -> Result<ValidationReport, EdifactError> {
        let report = self.validate_lenient(segments);
        if report.has_errors() {
            let first_message = report
                .errors
                .first()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "unknown validation failure".to_owned());
            return Err(EdifactError::ValidationFailed {
                error_count: report.errors.len(),
                first_message,
            });
        }
        Ok(report)
    }

    /// Message type metadata associated with this context, if provided.
    pub fn message_type(&self) -> Option<&str> {
        self.message_type.as_deref()
    }

    fn layer_enabled(&self, layer: ValidationLayer) -> bool {
        match layer {
            ValidationLayer::Structure => self.structure_enabled,
            ValidationLayer::CodeList => self.code_list_enabled,
            ValidationLayer::Profile => self.profile_enabled,
        }
    }
}

/// Pluggable validator for parsed EDIFACT segments.
///
/// The primary contract is [`validate_batch`](Validator::validate_batch), which processes an
/// entire segment sequence and appends issues to a [`ValidationReport`].
///
/// For validators that work segment-by-segment, the convenience function
/// [`validate_each`] iterates over the slice and calls a per-segment closure,
/// so you only need to implement `validate_batch`:
///
/// ```rust,ignore
/// fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
///     validate_each(segments, report, |seg| {
///         // return Ok(()) or Err(EdifactError::...)
///         Ok(())
///     });
/// }
/// ```
pub trait Validator: Send + Sync {
    /// Validate a full segment set and append issues to `report`.
    fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport);

    /// Configure message-type metadata for validators that support explicit scoping.
    fn set_message_type(&mut self, _message_type: Option<&str>) {}
}

/// Helper for per-segment validators: iterates `segments`, calls `f` for each one,
/// and converts any `Err` into report entries.
///
/// Use this in `validate_batch` implementations that work segment-by-segment:
///
/// ```rust,ignore
/// fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
///     validate_each(segments, report, |seg| { /* ... */ Ok(()) });
/// }
/// ```
pub fn validate_each<F>(segments: &[Segment<'_>], report: &mut ValidationReport, mut f: F)
where
    F: FnMut(&Segment<'_>) -> Result<(), EdifactError>,
{
    for segment in segments {
        if let Err(err) = f(segment) {
            report_error(report, err);
        }
    }
}

/// Convert a low-level validation error to a user-facing issue and append it.
pub(crate) fn report_error(report: &mut ValidationReport, err: EdifactError) {
    let issue = issue_from_error(err);
    match issue.severity {
        ValidationSeverity::Critical | ValidationSeverity::Error => report.add_error(issue),
        ValidationSeverity::Warning => report.add_warning(issue),
        ValidationSeverity::Info => report.add_info(issue),
    }
}

fn issue_from_error(err: EdifactError) -> ValidationIssue {
    let code = err.stable_code();
    let mut issue = ValidationIssue::new(severity_for(&err), err.to_string()).with_error_code(code);
    let default_hint = err.recovery_hint();

    match err {
        EdifactError::InvalidSegmentForMessage { tag, offset, .. } => {
            issue = issue.with_segment(tag).with_offset(offset);
        }
        EdifactError::InvalidElementCount { tag, offset, .. } => {
            issue = issue.with_segment(tag).with_offset(offset);
        }
        EdifactError::InvalidComponentCount {
            tag,
            element_index,
            offset,
            ..
        } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(element_index as u8)
                .with_offset(offset);
        }
        EdifactError::InvalidCodeValue {
            tag,
            element_index,
            offset,
            suggestion,
            ..
        } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(element_index as u8)
                .with_offset(offset);
            if let Some(s) = suggestion {
                issue = issue.with_suggestion(s);
            }
        }
        EdifactError::MissingSegment { tag, .. } => {
            issue = issue.with_segment(tag);
        }
        EdifactError::QualifierMismatch { tag, offset, .. } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(0)
                .with_offset(offset);
        }
        EdifactError::ConditionalRequirementNotMet {
            tag,
            element_index,
            offset,
            ..
        } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(element_index as u8)
                .with_offset(offset);
        }
        EdifactError::MissingRequiredElement { tag, element_index } => {
            issue = issue.with_segment(tag).with_element_index(element_index as u8);
        }
        EdifactError::MissingRequiredComponent {
            tag,
            element_index,
            component_index,
        } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(element_index as u8)
                .with_component_index(component_index as u8);
        }
        EdifactError::InvalidReleaseSequence { offset }
        | EdifactError::InvalidDelimiter { offset, .. }
        | EdifactError::InvalidText { offset }
        | EdifactError::UnexpectedEof { offset } => {
            issue = issue.with_offset(offset);
        }
        _ => {}
    }

    if issue.suggestion.is_none() {
        if let Some(hint) = default_hint {
            issue = issue.with_suggestion(hint);
        }
    }

    issue
}

fn severity_for(err: &EdifactError) -> ValidationSeverity {
    match err {
        EdifactError::InvalidCodeValue { .. } | EdifactError::QualifierMismatch { .. } => {
            ValidationSeverity::Warning
        }
        _ => ValidationSeverity::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Element;

    fn demo_orders_profile_pack() -> ProfileRulePack {
        ProfileRulePack::builder("ORDERS-DEMO")
            .for_message_type("ORDERS")
            .with_rule_fn(|segments| {
                let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
                let document_code = bgm.get_element(0)?.get_component(0)?;
                (document_code == "220").then(|| {
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        "profile rule DEMO-P001 violated: BGM document code 220 is rejected in this demo pack",
                    )
                    .with_rule_id("DEMO-P001")
                    .with_segment("BGM")
                    .with_element_index(0)
                    .with_suggestion("Use a different BGM document code in this demo pack")
                })
            })
            .with_rule_fn(|segments| {
                let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
                let reference = bgm.get_element(1)?.get_component(0)?;
                (reference == "PO123").then(|| {
                    ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "profile rule DEMO-P002 warning: purchase-order reference PO123 is reserved in this demo pack",
                    )
                    .with_rule_id("DEMO-P002")
                    .with_segment("BGM")
                    .with_element_index(1)
                    .with_suggestion("Use a non-reserved reference in this demo pack")
                })
            })
    }

    struct RejectBgm;

    struct WarnBgm;

    impl Validator for RejectBgm {
        fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
            validate_each(segments, report, |segment| {
                if segment.tag == "BGM" {
                    return Err(EdifactError::InvalidSegmentForMessage {
                        tag: "BGM".to_owned(),
                        message_type: "TEST".to_owned(),
                        offset: segment.tag_span.start,
                    });
                }
                Ok(())
            });
        }
    }

    impl Validator for WarnBgm {
        fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
            validate_each(segments, report, |segment| {
                if segment.tag == "BGM" {
                    return Err(EdifactError::InvalidCodeValue {
                        tag: "BGM".to_owned(),
                        element_index: 0,
                        value: "XXX".to_owned(),
                        code_list: "1001".to_owned(),
                        offset: segment.span.start,
                        suggestion: None,
                    });
                }
                Ok(())
            });
        }
    }

    fn test_segment(tag: &'static str) -> Segment<'static> {
        Segment {
            tag,
            span: crate::Span::new(0, 0),
            tag_span: crate::Span::new(0, 0),
            elements: vec![Element::of(&["x"])],
        }
    }

    #[test]
    fn lenient_collects_issues() {
        let segments = vec![test_segment("UNH"), test_segment("BGM")];
        let mut report = ValidationReport::default();
        RejectBgm.validate_batch(&segments, &mut report);
        assert!(report.has_errors());
        assert_eq!(report.errors.len(), 1);
    }

    #[test]
    fn strict_fails_on_errors() {
        let segments = vec![test_segment("BGM")];
        let mut report = ValidationReport::default();
        RejectBgm.validate_batch(&segments, &mut report);
        assert!(report.has_errors());
        assert_eq!(report.errors.len(), 1);
    }

    #[test]
    fn context_builder_respects_layer_toggles() {
        let segments = vec![test_segment("BGM")];
        let ctx = ValidationContext::builder()
            .structure(false)
            .with_validator(ValidationLayer::Structure, RejectBgm)
            .with_validator(ValidationLayer::CodeList, WarnBgm)
            .build();

        let report = ctx.validate_lenient(&segments);
        assert!(!report.has_errors());
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn context_strict_fails_when_structure_enabled() {
        let segments = vec![test_segment("BGM")];
        let ctx = ValidationContext::builder()
            .with_message_type("ORDERS")
            .with_validator(ValidationLayer::Structure, RejectBgm)
            .build();

        assert_eq!(ctx.message_type(), Some("ORDERS"));
        let result = ctx.validate_strict(&segments);
        assert!(matches!(result, Err(EdifactError::ValidationFailed { .. })));
    }

    #[test]
    fn report_error_applies_default_recovery_hint() {
        let mut report = ValidationReport::default();
        report_error(
            &mut report,
            EdifactError::InvalidReleaseSequence { offset: 9 },
        );

        let issue = report
            .errors
            .first()
            .expect("expected one issue in the report");
        let hint = issue
            .suggestion
            .as_deref()
            .expect("expected default hint to be set");
        assert!(hint.contains("Release character"));
        assert_eq!(issue.error_code, Some("E020"));
    }

    #[test]
    fn missing_required_component_maps_metadata_to_issue() {
        let mut report = ValidationReport::default();
        report_error(
            &mut report,
            EdifactError::MissingRequiredComponent {
                tag: "BGM".to_owned(),
                element_index: 2,
                component_index: 1,
            },
        );

        let issue = report
            .errors
            .first()
            .expect("expected one issue");
        assert_eq!(issue.error_code, Some("E009"));
        assert_eq!(issue.segment_tag.as_deref(), Some("BGM"));
        assert_eq!(issue.element_index, Some(2));
        assert_eq!(issue.component_index, Some(1));
    }

    #[test]
    fn profile_pack_lenient_collects_profile_rule_issues() {
        let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("expected parse success");

        let ctx = ValidationContext::builder()
            .with_profile_pack(demo_orders_profile_pack())
            .build();

        let report = ctx.validate_lenient(&segments);
        assert!(report.has_errors());
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.rule_id.as_deref() == Some("DEMO-P001"))
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|issue| issue.rule_id.as_deref() == Some("DEMO-P002"))
        );
    }

    #[test]
    fn profile_pack_strict_fails_when_profile_errors_exist() {
        let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("expected parse success");

        let ctx = ValidationContext::builder()
            .with_profile_pack(demo_orders_profile_pack())
            .build();
        let result = ctx.validate_strict(&segments);
        assert!(matches!(result, Err(EdifactError::ValidationFailed { .. })));
    }
}
