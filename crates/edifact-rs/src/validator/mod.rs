//! Validation pipeline for structural and semantic EDIFACT checks.
//!
//! The validation module is split into three sub-modules:
//!
//! - [`pack`] — [`ProfileRule`], [`ProfileRulePack`], and rule construction helpers.
//! - [`context`] — [`ValidationContext`], [`ValidationContextBuilder`].
//! - This module (`mod.rs`) — [`Validator`] trait, [`ValidationRuleContext`],
//!   [`ValidationLayer`], [`EnvelopeValidator`], and helper functions.

pub mod context;
pub mod pack;

pub use context::{ValidationContext, ValidationContextBuilder};
pub use pack::{ProfileRule, ProfileRulePack};

use crate::{EdifactError, Segment, ValidationIssue, ValidationReport, ValidationSeverity};
use std::any::Any;

/// Typed context injected into profile rule closures at validation time.
///
/// Rules access per-call metadata via [`ValidationRuleContext::metadata`] and
/// the message reference (UNH element 0) via [`ValidationRuleContext::message_ref`].
///
/// # Example
///
/// ```rust,ignore
/// let pack = ProfileRulePack::new("AHB-11001")
///     .with_rule_fn(|segs, ctx, issues| {
///         let Some(pruefid) = ctx.metadata::<Pruefid>() else { return };
///         let msg_ref = ctx.message_ref.unwrap_or("<unknown>");
///     });
/// ```
#[derive(Clone, Copy)]
pub struct ValidationRuleContext<'a> {
    pub(super) metadata: Option<&'a (dyn Any + Send + Sync)>,
    /// Message reference (`UNH` element 0) for this validation call.
    pub message_ref: Option<&'a str>,
    /// EDIFACT message type extracted from `UNH` element 1 component 0.
    ///
    /// Pre-extracted once by [`ValidationContext`] before dispatching to validators,
    /// so each [`ProfileRulePack`] can skip its own `UNH` scan.
    /// `None` when the message type could not be determined (no `UNH`, or when calling
    /// [`ProfileRulePack::validate_batch`] directly without a `ValidationContext`).
    pub message_type: Option<&'a str>,
}

impl<'a> ValidationRuleContext<'a> {
    /// Construct a context with no metadata and no message reference.
    pub fn empty() -> Self {
        Self {
            metadata: None,
            message_ref: None,
            message_type: None,
        }
    }

    /// Construct a context holding a typed metadata reference.
    pub fn new<T: Any + Send + Sync>(value: &'a T) -> Self {
        Self {
            metadata: Some(value as &(dyn Any + Send + Sync)),
            message_ref: None,
            message_type: None,
        }
    }

    /// Attach a message reference to this context (builder-style).
    pub fn with_message_ref(mut self, msg_ref: &'a str) -> Self {
        self.message_ref = Some(msg_ref);
        self
    }

    /// Attach a pre-extracted message type to this context (builder-style).
    pub fn with_message_type(mut self, message_type: &'a str) -> Self {
        self.message_type = Some(message_type);
        self
    }

    /// Downcast the metadata to `T`.  Returns `None` if no metadata was
    /// injected or if the concrete type does not match `T`.
    pub fn metadata<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.metadata?.downcast_ref::<T>()
    }

    /// Return `true` if metadata was provided.
    pub fn has_metadata(&self) -> bool {
        self.metadata.is_some()
    }
}

impl std::fmt::Debug for ValidationRuleContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationRuleContext")
            .field("has_metadata", &self.metadata.is_some())
            .field("message_ref", &self.message_ref)
            .field("message_type", &self.message_type)
            .finish()
    }
}

/// Validation layers used by [`ValidationContext`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationLayer {
    /// Interchange / message envelope checks (`UNB`/`UNH`/`UNT`/`UNZ` counts).
    Envelope,
    /// Directory structure checks (segment presence/order/arity).
    Structure,
    /// Directory code-list checks.
    CodeList,
    /// Downstream profile-pack checks.
    Profile,
}

/// Pluggable validator for parsed EDIFACT segments.
///
/// The primary contract is [`validate_batch`](Validator::validate_batch), which processes an
/// entire segment sequence and appends issues to a [`ValidationReport`].
pub trait Validator: Send + Sync {
    /// Validate a full segment set and append issues to `report`.
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
    );

    /// Validate a segment-group tree and append issues to `report`.
    ///
    /// Called by [`ValidationContext::validate_lenient_grouped`] in addition to
    /// [`validate_batch`](Validator::validate_batch).  Validators that only perform
    /// flat segment checks (e.g. [`EnvelopeValidator`]) can leave this as the
    /// default no-op; only validators with group-scoped rules (typically
    /// [`ProfileRulePack`] with at least one group rule) need to override it.
    ///
    /// The default implementation is a no-op so that adding this method to the
    /// trait is not a breaking change for external `Validator` implementors.
    fn validate_group_batch(
        &self,
        _root: &crate::group::SegmentGroupIndexed,
        _all_segments: &[Segment<'_>],
        _report: &mut ValidationReport,
        _context: &ValidationRuleContext<'_>,
    ) {
    }

    /// Returns `true` if this validator has any group-scoped rules.
    ///
    /// Used by [`crate::ValidationContext`] to short-circuit the
    /// group-tree walk when no validator in the context has group rules,
    /// avoiding the cost of allocating a borrowed slice for nothing.
    ///
    /// The default implementation returns `false`.
    fn has_group_rules(&self) -> bool {
        false
    }

    /// Configure message-type metadata for validators that support explicit scoping.
    fn set_message_type(&mut self, _message_type: Option<&str>) {}

    /// Create a `Box<dyn Validator>` clone of this validator for context forking.
    ///
    /// Return `Some(boxed_clone)` for validators that support cheap forking
    /// (e.g. those backed by `Arc` data, like [`ProfileRulePack`]).
    ///
    /// Return `None` for validators that cannot be forked (e.g. stateful validators
    /// without `Clone`).  Returning `None` causes the validator to be silently
    /// **excluded** from forked contexts — forking is used by
    /// [`crate::ValidationContext::validate_lenient_grouped`] to validate
    /// each group in isolation, so omitting a non-forkable validator from the
    /// forked context is safer than panicking.
    fn fork(&self) -> Option<Box<dyn Validator + Send + Sync>> {
        None
    }
}

/// Helper for per-segment validators: iterates `segments`, calls `f` for each one,
/// and converts any `Err` into report entries.
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
///
/// # Severity mapping
///
/// The mapping from `EdifactError` variant to `ValidationSeverity` is:
///
/// | Variant | Severity |
/// |---|---|
/// | `InvalidCodeValue` | `Warning` |
/// | `QualifierMismatch` | `Warning` |
/// | *(everything else)* | `Error` |
///
/// Rationale: code-list and qualifier mismatches indicate a value that *could*
/// be intentional (non-standard extension codes are common in practice).  All
/// structural violations — missing segments, wrong element counts, parse errors
/// — are hard errors.
pub(crate) fn report_error(report: &mut ValidationReport, err: EdifactError) {
    let issue = issue_from_error(err);
    match issue.severity {
        ValidationSeverity::Critical | ValidationSeverity::Error => report.add_error(issue),
        ValidationSeverity::Warning => report.add_warning(issue),
        ValidationSeverity::Info => report.add_info(issue),
    }
}

// ── EnvelopeValidator ─────────────────────────────────────────────────────────

/// Built-in validator for EDIFACT interchange envelope structure.
///
/// Checks `UNB`/`UNH`/`UNT`/`UNZ` segment presence, message counts, and
/// segment counts.  Registered by
/// [`ValidationContextBuilder::with_envelope_validation`].
pub struct EnvelopeValidator;

impl Validator for EnvelopeValidator {
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        _ctx: &ValidationRuleContext<'_>,
    ) {
        if let Err(e) = crate::envelope::validate_envelope(segments) {
            report_error(report, e);
        }
    }

    fn fork(&self) -> Option<Box<dyn Validator + Send + Sync>> {
        Some(Box::new(EnvelopeValidator))
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
                .with_element_index(u8::try_from(element_index).unwrap_or(u8::MAX))
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
                .with_element_index(u8::try_from(element_index).unwrap_or(u8::MAX))
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
                .with_element_index(u8::try_from(element_index).unwrap_or(u8::MAX))
                .with_offset(offset);
        }
        EdifactError::MissingRequiredElement { tag, element_index } => {
            issue = issue.with_segment(tag);
            if let Ok(idx) = u8::try_from(element_index) {
                issue = issue.with_element_index(idx);
            }
        }
        EdifactError::MissingRequiredComponent {
            tag,
            element_index,
            component_index,
        } => {
            issue = issue.with_segment(tag);
            if let Ok(ei) = u8::try_from(element_index) {
                issue = issue.with_element_index(ei);
            }
            if let Ok(ci) = u8::try_from(component_index) {
                issue = issue.with_component_index(ci);
            }
        }
        EdifactError::InvalidReleaseSequence { offset }
        | EdifactError::InvalidDelimiter { offset, .. }
        | EdifactError::InvalidText { offset }
        | EdifactError::UnexpectedEof { offset }
        | EdifactError::UnexpectedDataToken { offset } => {
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
        ProfileRulePack::new("ORDERS-DEMO")
            .for_message_type("ORDERS")
            .with_stateless_rule_fn(|segments, issues| {
                issues.extend((|| -> Option<ValidationIssue> {
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
                })());
            })
            .with_stateless_rule_fn(|segments, issues| {
                issues.extend((|| -> Option<ValidationIssue> {
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
                })());
            })
    }

    struct RejectBgm;

    struct WarnBgm;

    impl Validator for RejectBgm {
        fn validate_batch(
            &self,
            segments: &[Segment<'_>],
            report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
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
        fn validate_batch(
            &self,
            segments: &[Segment<'_>],
            report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
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
        RejectBgm.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());
        assert!(report.has_errors());
        assert_eq!(report.errors().len(), 1);
    }

    #[test]
    fn strict_fails_on_errors() {
        let segments = vec![test_segment("BGM")];
        let mut report = ValidationReport::default();
        RejectBgm.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());
        assert!(report.has_errors());
        assert_eq!(report.errors().len(), 1);
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
        assert_eq!(report.warnings().len(), 1);
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
        assert!(result.is_err());
        assert!(result.unwrap_err().has_errors());
    }

    #[test]
    fn report_error_applies_default_recovery_hint() {
        let mut report = ValidationReport::default();
        report_error(
            &mut report,
            EdifactError::InvalidReleaseSequence { offset: 9 },
        );

        let issue = report
            .errors()
            .first()
            .expect("expected one issue in the report");
        let hint = issue
            .suggestion
            .as_deref()
            .expect("expected default hint to be set");
        assert!(hint.contains("Release character"));
        assert_eq!(issue.error_code, Some("E019"));
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

        let issue = report.errors().first().expect("expected one issue");
        assert_eq!(issue.error_code, Some("E021"));
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
                .errors()
                .iter()
                .any(|issue| issue.rule_id.as_deref() == Some("DEMO-P001"))
        );
        assert!(
            report
                .warnings()
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
        assert!(result.is_err());
        assert!(result.unwrap_err().has_errors());
    }

    // ── bail_on_first_error ──────────────────────────────────────────────────

    /// A rule that emits two error-severity issues (one per DTM segment).
    fn two_dtm_errors_rule() -> ProfileRulePack {
        ProfileRulePack::new("TEST-BAIL")
            .with_stateless_rule_fn(|segments, issues| {
                // Rule A: emits one error per DTM segment.
                for seg in segments.iter().filter(|s| s.tag == "DTM") {
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            format!("DTM error at offset {}", seg.span.start),
                        )
                        .with_rule_id("BAIL-R1")
                        .with_segment("DTM"),
                    );
                }
            })
            .with_stateless_rule_fn(|segments, issues| {
                // Rule B: never fires; used to verify bail skips this rule.
                for seg in segments.iter().filter(|s| s.tag == "BGM") {
                    issues.push(
                        ValidationIssue::new(ValidationSeverity::Error, "BGM error")
                            .with_rule_id("BAIL-R2")
                            .with_segment(seg.tag),
                    );
                }
            })
    }

    #[test]
    fn bail_on_first_error_fires_at_rule_invocation_granularity() {
        // Two DTM segments → Rule A emits 2 errors for them.
        // With bail, Rule B (BGM check) must NOT run.
        let input =
            b"UNH+1+ORDERS:D:96A:UN'BGM+220+9'DTM+137:20240101:102'DTM+163:20240201:102'UNT+5+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse failed");

        let pack_with_bail = two_dtm_errors_rule().with_bail_on_first_error(true);
        let ctx = ValidationContext::builder()
            .with_profile_pack(pack_with_bail)
            .build();
        let report = ctx.validate_lenient(&segments);

        // Rule A fires: both DTM errors are in the report (the whole rule invocation
        // runs to completion before bail is checked).
        assert_eq!(
            report
                .errors()
                .iter()
                .filter(|i| i.rule_id.as_deref() == Some("BAIL-R1"))
                .count(),
            2,
            "both DTM errors from Rule A should be present"
        );
        // Bail fired after Rule A: Rule B (BGM) must be skipped.
        assert_eq!(
            report
                .errors()
                .iter()
                .filter(|i| i.rule_id.as_deref() == Some("BAIL-R2"))
                .count(),
            0,
            "Rule B should have been skipped by bail"
        );
    }

    #[test]
    fn bail_disabled_runs_all_rules() {
        let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+9'DTM+137:20240101:102'UNT+4+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse failed");

        let pack_no_bail = two_dtm_errors_rule(); // bail_on_first_error defaults to false
        let ctx = ValidationContext::builder()
            .with_profile_pack(pack_no_bail)
            .build();
        let report = ctx.validate_lenient(&segments);

        // Both rules run: one DTM error from Rule A, one BGM error from Rule B.
        assert_eq!(
            report
                .errors()
                .iter()
                .filter(|i| i.rule_id.as_deref() == Some("BAIL-R1"))
                .count(),
            1
        );
        assert_eq!(
            report
                .errors()
                .iter()
                .filter(|i| i.rule_id.as_deref() == Some("BAIL-R2"))
                .count(),
            1
        );
    }

    // ── message_ref in ValidationRuleContext ─────────────────────────────────

    #[test]
    fn message_ref_is_visible_inside_rule_closure() {
        let input = b"UNH+MSG001+ORDERS:D:96A:UN'BGM+220+9'UNT+3+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse failed");

        let pack = ProfileRulePack::new("MSG-REF-TEST").with_rule_fn(|_segs, ctx, issues| {
            if let Some(mref) = ctx.message_ref {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Info,
                        format!("validating message {mref}"),
                    )
                    .with_rule_id("CTX-REF"),
                );
            }
        });

        let ctx = ValidationContext::builder()
            .with_profile_pack(pack)
            .with_message_ref("MSG001")
            .build();

        let report = ctx.validate_lenient(&segments);
        let info = report
            .infos()
            .iter()
            .find(|i| i.rule_id.as_deref() == Some("CTX-REF"))
            .expect("expected info issue from CTX-REF rule");
        assert!(info.message.contains("MSG001"));
        // The message_ref is also stamped onto the issue itself.
        assert_eq!(info.message_ref.as_deref(), Some("MSG001"));
    }
}
