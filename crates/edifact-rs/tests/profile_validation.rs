mod common;

use edifact_rs::ValidationContext;

const ORDERS_CONFORMING: &str = include_str!("fixtures/orders_conforming.edi");
#[test]
fn custom_profile_pack_reports_rule_ids_for_orders_fixture() {
    let segments: Vec<_> = edifact_rs::from_bytes(ORDERS_CONFORMING.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture should parse");

    let ctx = ValidationContext::builder()
        .with_profile_pack(common::demo_orders_profile_pack())
        .build();

    let report = ctx.validate_lenient(&segments);
    assert!(report.has_errors(), "expected profile errors");
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
fn custom_profile_pack_is_skipped_for_other_message_types() {
    let segments: Vec<_> =
        edifact_rs::from_bytes(b"UNH+1+INVOIC:D:96A:UN'BGM+220+INV-1+9'UNT+3+1'")
            .collect::<Result<Vec<_>, _>>()
            .expect("fixture should parse");

    let ctx = ValidationContext::builder()
        .with_profile_pack(common::demo_orders_profile_pack())
        .build();

    let report = ctx.validate_lenient(&segments);
    assert!(
        report.is_valid(),
        "expected scoped pack to be skipped: {report}"
    );
}

#[test]
fn custom_profile_pack_strict_mode_fails_for_error_level_issues() {
    let segments: Vec<_> = edifact_rs::from_bytes(ORDERS_CONFORMING.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture should parse");

    let ctx = ValidationContext::builder()
        .with_profile_pack(common::demo_orders_profile_pack())
        .build();

    let result = ctx.validate_strict(&segments);
    assert!(result.is_err(), "strict profile validation should fail");
}

// TEST 7.1: with_message_type call-order independence
//
// The same message-type filter must be passed to validators whether
// `with_message_type` is called before or after `with_profile_pack`.
#[test]
fn profile_pack_message_type_set_before_pack_matches_set_after() {
    let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'";
    let segments: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture should parse");

    // builder order 1: message_type THEN pack
    let ctx_before = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_profile_pack(common::demo_orders_profile_pack())
        .build();

    // builder order 2: pack THEN message_type
    let ctx_after = ValidationContext::builder()
        .with_profile_pack(common::demo_orders_profile_pack())
        .with_message_type("ORDERS")
        .build();

    let report_before = ctx_before.validate_lenient(&segments);
    let report_after = ctx_after.validate_lenient(&segments);

    // Both configurations must produce the same number of errors and warnings.
    assert_eq!(
        report_before.errors().len(),
        report_after.errors().len(),
        "error count must be the same regardless of with_message_type call order"
    );
    assert_eq!(
        report_before.warnings().len(),
        report_after.warnings().len(),
        "warning count must be the same regardless of with_message_type call order"
    );
}
