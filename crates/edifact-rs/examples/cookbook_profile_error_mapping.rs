//! Cookbook: mapping profile rule violations to structured diagnostics
//!
//! Shows two complementary patterns for downstream crates building domain
//! validators on top of `edifact-rs`:
//!
//! ## Pattern A — `ProfileRulePack` rules mapped to application error codes
//!
//! Use `ValidationReport::filter_by_rule_prefix` and `issues_for_rule_id` to
//! extract rule-specific findings from a report and map them to your own error
//! type without coupling to `ValidationIssue`.
//!
//! ## Pattern B — custom `Validator` trait implementation
//!
//! For complex validation logic that needs shared state across segments (e.g.,
//! reference counting, cross-segment consistency), implement the `Validator`
//! trait directly and register it alongside a `ProfileRulePack`.

use edifact_rs::{
    ProfileRulePack, ValidationContext, ValidationIssue, ValidationLayer, ValidationReport,
    ValidationSeverity, Validator, from_bytes,
};

// ── Application-level error type ──────────────────────────────────────────────

/// Domain-level error codes produced by downstream validation.
///
/// Downstream crates typically define their own richer error taxonomy and map
/// `ValidationIssue` rule IDs here rather than returning `ValidationIssue`
/// directly to application code.
#[derive(Debug, PartialEq, Eq)]
enum OrdersViolation {
    /// Document function code is not accepted in this trading flow.
    UnsupportedFunctionCode { code: String },
    /// Mandatory purchase-order reference is blank.
    MissingPoReference,
}

// ── Pattern A: ProfileRulePack + rule-ID mapping ───────────────────────────────

fn build_orders_pack() -> ProfileRulePack {
    let function_code_pack = ProfileRulePack::builder("ORDERS-FUNCTION-CODE")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let bgm = segments.iter().find(|s| s.tag == "BGM")?;
            let func = bgm.get_element(2)?.get_component(0)?;
            // Only codes 9 (original) and 1 (cancellation) are accepted.
            (!matches!(func, "9" | "1")).then(|| {
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    format!("unsupported BGM function code '{func}'"),
                )
                .with_rule_id("ORDERS-P001-FUNC")
                .with_segment("BGM")
                .with_element_index(2)
                .with_suggestion("Use function code 9 (original) or 1 (cancellation)")
            })
        });

    let reference_pack = ProfileRulePack::builder("ORDERS-PO-REF")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let bgm = segments.iter().find(|s| s.tag == "BGM")?;
            let reference = bgm.get_element(1)?.get_component(0)?;
            reference.is_empty().then(|| {
                ValidationIssue::new(ValidationSeverity::Error, "BGM purchase-order reference is empty")
                    .with_rule_id("ORDERS-P002-REF")
                    .with_segment("BGM")
                    .with_element_index(1)
                    .with_suggestion("Populate BGM element 1 with the buyer's PO reference number")
            })
        });

    ProfileRulePack::builder("ORDERS-COMBINED")
        .merge(function_code_pack)
        .merge(reference_pack)
}

/// Map a `ValidationReport` to an application-level violation list.
///
/// Downstream crates use `filter_by_rule_prefix` and `issues_for_rule_id` to
/// decouple their domain model from `ValidationIssue`.
fn extract_violations(report: &ValidationReport) -> Vec<OrdersViolation> {
    let mut violations = Vec::new();

    // Map profile rule IDs to domain violation types.
    for issue in report.filter_by_rule_prefix("ORDERS-P001").iter_issues() {
        // Rule ORDERS-P001-FUNC carries the disallowed code in the message text.
        let code = issue
            .message
            .split('\'')
            .nth(1)
            .unwrap_or("?")
            .to_owned();
        violations.push(OrdersViolation::UnsupportedFunctionCode { code });
    }
    if report.issues_for_rule_id("ORDERS-P002-REF").next().is_some() {
        violations.push(OrdersViolation::MissingPoReference);
    }

    violations
}

// ── Pattern B: custom Validator implementation ────────────────────────────────

/// A stateful validator that enforces an agreed segment-count ceiling.
///
/// `ProfileRulePack` closures receive a complete segment slice, but a custom
/// `Validator` implementation is useful when the logic is complex, needs
/// access to injected configuration, or must be tested in isolation.
struct MaxSegmentValidator {
    /// Maximum accepted segment count in a single message.
    limit: u32,
}

impl Validator for MaxSegmentValidator {
    fn validate_batch(
        &self,
        segments: &[edifact_rs::Segment<'_>],
        report: &mut ValidationReport,
    ) {
        let count = segments.len() as u32;
        if count > self.limit {
            report.add_warning(
                ValidationIssue::new(
                    ValidationSeverity::Warning,
                    format!("message has {count} segments; agreed maximum is {}", self.limit),
                )
                .with_rule_id("ORDERS-P099-SEGCOUNT")
                .with_suggestion(format!(
                    "Review message structure — maximum agreed count is {}",
                    self.limit
                )),
            );
        }
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<(), edifact_rs::EdifactError> {
    // ── valid message ─────────────────────────────────────────────────────────
    let valid_input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO-001+9'UNT+3+1'";
    let valid_segments: Vec<_> = from_bytes(valid_input).collect::<Result<_, _>>()?;
    let report = ValidationContext::builder()
        .with_profile_pack(build_orders_pack())
        .with_validator(ValidationLayer::Profile, MaxSegmentValidator { limit: 100 })
        .build()
        .validate_lenient(&valid_segments);
    assert!(!report.has_errors(), "valid message should produce no errors");
    println!("valid message: no violations");

    // ── message with unsupported function code ────────────────────────────────
    let bad_func_input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO-002+5'UNT+3+1'";
    let bad_func_segments: Vec<_> = from_bytes(bad_func_input).collect::<Result<_, _>>()?;
    let bad_func_report = ValidationContext::builder()
        .with_profile_pack(build_orders_pack())
        .build()
        .validate_lenient(&bad_func_segments);
    let violations = extract_violations(&bad_func_report);
    println!("function-code violations: {violations:?}");
    assert_eq!(
        violations,
        vec![OrdersViolation::UnsupportedFunctionCode { code: "5".to_owned() }]
    );

    // ── rule-ID based filtering (Pattern A) ───────────────────────────────────
    let orders_issues = bad_func_report.filter_by_rule_prefix("ORDERS-");
    println!(
        "ORDERS-scoped issues: {}  |  total: {}",
        orders_issues.total_issues(),
        bad_func_report.total_issues()
    );

    // ── custom validator (Pattern B) ──────────────────────────────────────────
    let tiny_limit_report = ValidationContext::builder()
        .with_profile_pack(build_orders_pack())
        .with_validator(ValidationLayer::Profile, MaxSegmentValidator { limit: 2 })
        .build()
        .validate_lenient(&valid_segments);
    assert!(
        tiny_limit_report.has_warnings(),
        "expected a segment-count warning"
    );
    let seg_count_issues: Vec<_> = tiny_limit_report.issues_for_rule_id("ORDERS-P099-SEGCOUNT").collect();
    println!("segment-count issue: {}", seg_count_issues[0].message);

    println!("All profile error-mapping examples passed.");
    Ok(())
}
