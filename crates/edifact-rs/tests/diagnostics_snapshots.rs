use edifact_rs::{ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity};

const ORDERS_CONFORMING: &str = include_str!("fixtures/orders_conforming.edi");
const PROFILE_ORDERS_DEMO_REPORT: &str = include_str!("snapshots/profile_orders_demo_report.txt");

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
            (reference == "PO-4711").then(|| {
                ValidationIssue::new(
                    ValidationSeverity::Warning,
                    "profile rule DEMO-P002 warning: purchase-order reference PO-4711 is reserved in this demo pack",
                )
                .with_rule_id("DEMO-P002")
                .with_segment("BGM")
                .with_element_index(1)
                .with_suggestion("Use a non-reserved reference in this demo pack")
            })
        })
}

#[test]
fn snapshot_profile_orders_demo_report_contract() {
    let segments: Vec<_> = edifact_rs::from_bytes(ORDERS_CONFORMING.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture should parse");

    let ctx = ValidationContext::builder()
        .with_profile_pack(demo_orders_profile_pack())
        .build();

    let mut report = ctx.validate_lenient(&segments);

    // Freeze snapshot content across parser span-offset changes.
    for issue in &mut report.errors {
        issue.offset = None;
    }
    for issue in &mut report.warnings {
        issue.offset = None;
    }
    for issue in &mut report.infos {
        issue.offset = None;
    }

    let rendered = report.render_deterministic();
    assert_eq!(rendered.trim_end(), PROFILE_ORDERS_DEMO_REPORT.trim_end());
}
