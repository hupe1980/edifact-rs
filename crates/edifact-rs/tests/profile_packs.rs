use edifact_rs::{ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity};

fn parse_segments(input: &[u8]) -> Vec<edifact_rs::Segment<'_>> {
    edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("expected parse success")
}

#[test]
fn externally_authored_pack_can_validate_a_message_type() {
    let segments = parse_segments(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'");

    let pack = ProfileRulePack::builder("ORDERS-DEMO")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
            let document_code = bgm.get_element(0)?.get_component(0)?;
            (document_code == "220").then(|| {
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "Demo pack rejects BGM 220 for testing external authoring",
                )
                .with_rule_id("DEMO-P001")
                .with_segment("BGM")
                .with_element_index(0)
                .with_suggestion("Use a different document/message name code in this test pack")
            })
        });

    assert_eq!(pack.name(), "ORDERS-DEMO");
    assert_eq!(pack.message_types(), ["ORDERS".to_owned()]);

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_lenient(&segments);

    assert!(report.has_errors());
    assert!(
        report
            .errors
            .iter()
            .any(|issue| issue.rule_id.as_deref() == Some("DEMO-P001"))
    );
}

#[test]
fn merged_packs_accumulate_rules() {
    let segments = parse_segments(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'");

    let document_rule = ProfileRulePack::new("ORDERS-DOC")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
            let document_code = bgm.get_element(0)?.get_component(0)?;
            (document_code == "220").then(|| {
                ValidationIssue::new(ValidationSeverity::Error, "document code rejected")
                    .with_rule_id("DEMO-P001")
            })
        });
    let reference_rule = ProfileRulePack::new("ORDERS-REF")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
            let reference = bgm.get_element(1)?.get_component(0)?;
            (reference == "PO123").then(|| {
                ValidationIssue::new(ValidationSeverity::Warning, "reference rejected")
                    .with_rule_id("DEMO-P002")
            })
        });

    let pack = document_rule.merge(reference_rule);
    assert_eq!(pack.rule_count(), 2);

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_lenient(&segments);

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
fn builder_can_merge_existing_packs() {
    let pack = ProfileRulePack::builder("COMBINED")
        .merge(
            ProfileRulePack::new("ONE")
                .for_message_type("ORDERS")
                .with_rule_fn(|_| {
                    Some(
                        ValidationIssue::new(ValidationSeverity::Info, "rule one")
                            .with_rule_id("DEMO-P010"),
                    )
                }),
        )
        .merge(
            ProfileRulePack::new("TWO")
                .for_message_type("INVOIC")
                .with_rule_fn(|_| {
                    Some(
                        ValidationIssue::new(ValidationSeverity::Info, "rule two")
                            .with_rule_id("DEMO-P011"),
                    )
                }),
        );

    assert_eq!(pack.name(), "COMBINED");
    assert_eq!(pack.rule_count(), 2);
    assert_eq!(pack.message_types(), ["ORDERS".to_owned(), "INVOIC".to_owned()]);
}

#[test]
fn message_type_scoping_prevents_wrong_pack_application() {
    let segments = parse_segments(b"UNH+1+INVOIC:D:96A:UN'BGM+220+INV123+9'UNT+3+1'");

    let pack = ProfileRulePack::new("ORDERS-ONLY")
        .for_message_type("ORDERS")
        .with_rule_fn(|_| {
            Some(
                ValidationIssue::new(ValidationSeverity::Error, "should not run")
                    .with_rule_id("DEMO-P999"),
            )
        });

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_lenient(&segments);

    assert!(
        report.is_valid(),
        "expected scoped pack to be skipped: {report}"
    );
}
