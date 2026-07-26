mod common;

use edifact_rs::ValidationContext;
use expect_test::expect_file;

const ORDERS_CONFORMING: &str = include_str!("fixtures/orders_conforming.edi");

#[test]
fn snapshot_profile_orders_demo_report_contract() {
    let segments: Vec<_> = edifact_rs::from_bytes(ORDERS_CONFORMING.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture should parse");

    let ctx = ValidationContext::builder()
        .with_profile_pack(common::demo_orders_profile_pack())
        .build();

    let mut report = ctx.validate_lenient(&segments);

    // Freeze snapshot content across parser span changes.
    for issue in report.errors_mut() {
        issue.span = None;
    }
    for issue in report.warnings_mut() {
        issue.span = None;
    }
    for issue in report.infos_mut() {
        issue.span = None;
    }

    let rendered = report.render_deterministic();

    // `expect_file!` loads the snapshot file at compile-time and compares at
    // runtime.  Set `UPDATE_EXPECT=1` (or run `cargo test` with that env var)
    // to auto-update the snapshot file when the output intentionally changes.
    expect_file!["snapshots/profile_orders_demo_report.txt"].assert_eq(&rendered);
}
