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

    let mut report = ctx.validate(&segments);

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

/// Each issue renders as its own labelled diagnostic under the report.
#[cfg(feature = "diagnostics")]
#[test]
fn miette_renders_one_labelled_diagnostic_per_issue() {
    use edifact_rs::{FindingKind, GroupPath, GroupStep, ProfileRulePack, ValidationIssue};
    use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme};

    const SOURCE: &str = "UNH+1+ORDERS:D:96A:UN'BGM+220+PO-4711+9'DTM+999:20240101:102'UNT+4+1'";
    let segments: Vec<_> = edifact_rs::from_bytes(SOURCE.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture should parse");
    let pack = ProfileRulePack::new("ORDERS-DEMO")
        .require_qualifier("DTM", 0, 0, "137", "DEMO-DTM-CODE")
        .with_rule_fn(|segments, issues| {
            let bgm = segments.iter().find(|s| s.tag == "BGM").expect("BGM");
            issues.push(
                ValidationIssue::new(
                    edifact_rs::ValidationSeverity::Warning,
                    "reserved reference",
                )
                .with_rule_id("DEMO-P002")
                .with_segment("BGM")
                .with_span(bgm.get_element(1).expect("element").span)
                .with_kind(FindingKind::NotPermitted)
                .with_group_path(GroupPath::new(GroupStep::new("SG1", 0)).expect("path"))
                .with_suggestion("Use a non-reserved reference"),
            );
        });
    let mut report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate(&segments);
    // `require_qualifier` reports no span; give it the DTM's so it can be labelled.
    let dtm_span = segments[2].span;
    report.errors_mut()[0].span = Some(dtm_span);

    let related: Vec<&dyn Diagnostic> = report.related().expect("related").collect();
    assert_eq!(related.len(), 2);
    assert_eq!(
        related[0].code().expect("code").to_string(),
        "DEMO-DTM-CODE"
    );
    assert_eq!(related[0].severity(), Some(miette::Severity::Error));
    assert_eq!(related[1].code().expect("code").to_string(), "DEMO-P002");
    assert_eq!(related[1].severity(), Some(miette::Severity::Warning));
    assert_eq!(
        related[1].help().expect("help").to_string(),
        "Use a non-reserved reference"
    );
    let label = |d: &dyn Diagnostic| {
        let labels: Vec<_> = d.labels().expect("labels").collect();
        assert_eq!(labels.len(), 1);
        (
            labels[0].offset(),
            labels[0].len(),
            labels[0].label().map(str::to_owned),
        )
    };
    assert_eq!(
        label(related[0]),
        (
            dtm_span.start,
            dtm_span.len(),
            Some("code at DTM".to_owned())
        )
    );
    let bgm_reference = segments[1].get_element(1).expect("element").span;
    assert_eq!(
        label(related[1]),
        (
            bgm_reference.start,
            bgm_reference.len(),
            Some("not_permitted at SG1[0]/BGM".to_owned())
        )
    );

    // And a real render with the source attached shows both, underlined.
    let rendered_report = miette::Report::new(report).with_source_code(SOURCE);
    let mut out = String::new();
    GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
        .render_report(&mut out, rendered_report.as_ref())
        .expect("render");
    for needle in [
        "DEMO-DTM-CODE",
        "code at DTM",
        "DEMO-P002",
        "not_permitted at SG1[0]/BGM",
        "Use a non-reserved reference",
    ] {
        assert!(out.contains(needle), "{needle} missing from:\n{out}");
    }
}
