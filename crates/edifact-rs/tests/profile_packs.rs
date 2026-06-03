use edifact_rs::{ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity};

fn parse_segments(input: &[u8]) -> Vec<edifact_rs::Segment<'_>> {
    edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("expected parse success")
}

#[test]
fn externally_authored_pack_can_validate_a_message_type() {
    let segments = parse_segments(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'");

    let pack = ProfileRulePack::new("ORDERS-DEMO")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(|segments| {
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
    assert_eq!(pack.message_types().collect::<Vec<_>>(), ["ORDERS"]);

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_lenient(&segments);

    assert!(report.has_errors());
    assert!(
        report
            .errors()
            .iter()
            .any(|issue| issue.rule_id.as_deref() == Some("DEMO-P001"))
    );
}

#[test]
fn merged_packs_accumulate_rules() {
    let segments = parse_segments(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'");

    let document_rule = ProfileRulePack::new("ORDERS-DOC")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(|segments| {
            let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
            let document_code = bgm.get_element(0)?.get_component(0)?;
            (document_code == "220").then(|| {
                ValidationIssue::new(ValidationSeverity::Error, "document code rejected")
                    .with_rule_id("DEMO-P001")
            })
        });
    let reference_rule = ProfileRulePack::new("ORDERS-REF")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(|segments| {
            let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
            let reference = bgm.get_element(1)?.get_component(0)?;
            (reference == "PO123").then(|| {
                ValidationIssue::new(ValidationSeverity::Warning, "reference rejected")
                    .with_rule_id("DEMO-P002")
            })
        });

    let pack = document_rule
        .merge(reference_rule)
        .expect("compatible packs");
    assert_eq!(pack.rule_count(), 2);

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_lenient(&segments);

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
fn builder_can_merge_existing_packs() {
    let pack = ProfileRulePack::new("COMBINED")
        .merge(
            ProfileRulePack::new("ONE")
                .for_message_type("ORDERS")
                .with_stateless_rule_fn(|_| {
                    Some(
                        ValidationIssue::new(ValidationSeverity::Info, "rule one")
                            .with_rule_id("DEMO-P010"),
                    )
                }),
        )
        .expect("compatible packs")
        .merge(
            ProfileRulePack::new("TWO")
                .for_message_type("INVOIC")
                .with_stateless_rule_fn(|_| {
                    Some(
                        ValidationIssue::new(ValidationSeverity::Info, "rule two")
                            .with_rule_id("DEMO-P011"),
                    )
                }),
        )
        .expect("compatible packs");

    assert_eq!(pack.name(), "COMBINED");
    assert_eq!(pack.rule_count(), 2);
    assert_eq!(
        pack.message_types().collect::<Vec<_>>(),
        ["INVOIC", "ORDERS"]
    );
}

#[test]
fn message_type_scoping_prevents_wrong_pack_application() {
    let segments = parse_segments(b"UNH+1+INVOIC:D:96A:UN'BGM+220+INV123+9'UNT+3+1'");

    let pack = ProfileRulePack::new("ORDERS-ONLY")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(|_| {
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

#[test]
fn merge_with_override_replaces_named_rules_in_place() {
    let segments = parse_segments(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'");

    let base = ProfileRulePack::new("BASE")
        .for_message_type("ORDERS")
        .with_named_stateless_rule_fn("RULE-1", |_| {
            Some(ValidationIssue::new(ValidationSeverity::Info, "base first"))
        })
        .with_named_stateless_rule_fn("RULE-2", |_| {
            Some(ValidationIssue::new(
                ValidationSeverity::Info,
                "base second",
            ))
        });

    let override_pack = ProfileRulePack::new("OVERRIDE")
        .for_message_type("ORDERS")
        .with_named_stateless_rule_fn("RULE-1", |_| {
            Some(ValidationIssue::new(
                ValidationSeverity::Info,
                "override first",
            ))
        });

    let pack = base
        .merge_with_override(override_pack)
        .expect("compatible packs");
    assert_eq!(pack.rule_count(), 2);

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_lenient(&segments);

    assert_eq!(report.infos().len(), 2);
    assert_eq!(report.infos()[0].message, "override first");
    assert_eq!(report.infos()[1].message, "base second");
}

#[test]
fn release_scoping_requires_matching_association_code() {
    let matching = parse_segments(b"UNH+1+ORDERS:D:96A:UN:5.5.3a'BGM+220+PO123+9'UNT+3+1'");
    let mismatching = parse_segments(b"UNH+1+ORDERS:D:96A:UN:5.5.4'BGM+220+PO123+9'UNT+3+1'");

    let build_pack = || {
        ProfileRulePack::new("ORDERS-553A")
            .for_message_type("ORDERS")
            .for_release("5.5.3a")
            .with_stateless_rule_fn(|_| {
                Some(
                    ValidationIssue::new(ValidationSeverity::Error, "release-specific rule fired")
                        .with_rule_id("DEMO-P100"),
                )
            })
    };

    let matching_report = ValidationContext::builder()
        .with_profile_pack(build_pack())
        .build()
        .validate_lenient(&matching);
    assert!(matching_report.has_errors());

    let mismatching_report = ValidationContext::builder()
        .with_profile_pack(build_pack())
        .build()
        .validate_lenient(&mismatching);
    assert!(
        mismatching_report.is_valid(),
        "expected release mismatch to skip pack"
    );
}

#[test]
fn pack_composition_preserves_compatible_release_scope() {
    let base = ProfileRulePack::new("BASE")
        .for_message_type("ORDERS")
        .for_release("5.5.3a")
        .with_stateless_rule_fn(|_| None);

    let delta = ProfileRulePack::new("DELTA")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(|_| None);

    let merged = base.merge(delta).expect("compatible packs");
    assert_eq!(merged.release(), Some("5.5.3a"));

    let extended = ProfileRulePack::new("EXTENDED")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(|_| None)
        .extend_from(&ProfileRulePack::new("BASE2").for_release("5.5.3a"))
        .expect("compatible packs");
    assert_eq!(extended.release(), Some("5.5.3a"));
}
