use edifact_rs::{ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity};

const ORDERS_CONFORMING: &str = include_str!("fixtures/orders_conforming.edi");

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
            })
        })
        .with_rule_fn(|segments| {
            let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
            let reference = bgm.get_element(1)?.get_component(0)?;
            (reference == "PO-4711").then(|| {
                ValidationIssue::new(
                    ValidationSeverity::Warning,
                    "profile rule DEMO-P002 warning: purchase-order reference PO-4711 is reserved in this demo pack",
                )
                .with_rule_id("DEMO-P002")
                .with_segment("BGM")
                .with_element_index(1)
            })
        })
}

#[test]
fn custom_profile_pack_reports_rule_ids_for_orders_fixture() {
    let segments: Vec<_> = edifact_rs::from_bytes(ORDERS_CONFORMING.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture should parse");

    let ctx = ValidationContext::builder()
        .with_profile_pack(demo_orders_profile_pack())
        .build();

    let report = ctx.validate_lenient(&segments);
    assert!(report.has_errors(), "expected profile errors");
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
fn custom_profile_pack_is_skipped_for_other_message_types() {
    let segments: Vec<_> =
        edifact_rs::from_bytes(b"UNH+1+INVOIC:D:96A:UN'BGM+220+INV-1+9'UNT+3+1'")
            .collect::<Result<Vec<_>, _>>()
            .expect("fixture should parse");

    let ctx = ValidationContext::builder()
        .with_profile_pack(demo_orders_profile_pack())
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
        .with_profile_pack(demo_orders_profile_pack())
        .build();

    let result = ctx.validate_strict(&segments);
    assert!(result.is_err(), "strict profile validation should fail");
}
