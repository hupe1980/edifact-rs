mod common;

use edifact_rs::ValidationContext;

const ORDERS_CONFORMING: &str = include_str!("fixtures/orders_conforming.edi");
const PROFILE_ORDERS_DEMO_REPORT: &str = include_str!("snapshots/profile_orders_demo_report.txt");

#[test]
fn snapshot_profile_orders_demo_report_contract() {
    let segments: Vec<_> = edifact_rs::from_bytes(ORDERS_CONFORMING.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture should parse");

    let ctx = ValidationContext::builder()
        .with_profile_pack(common::demo_orders_profile_pack())
        .build();

    let mut report = ctx.validate_lenient(&segments);

    // Freeze snapshot content across parser span-offset changes.
    for issue in report.errors_mut() {
        issue.offset = None;
    }
    for issue in report.warnings_mut() {
        issue.offset = None;
    }
    for issue in report.infos_mut() {
        issue.offset = None;
    }

    let rendered = report.render_deterministic();
    assert_eq!(rendered.trim_end(), PROFILE_ORDERS_DEMO_REPORT.trim_end());
}
