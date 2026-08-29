use edifact_rs::{
    DirectoryValidator, EdifactError, ElementRef, OwnedElementRef, OwnedSegmentDef,
    SegmentDefinition, Status, ValidationReport, ValidationRuleContext, Validator, from_bytes,
};

static DTM_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "C507", Status::Mandatory, 1),
    ElementRef::new(2, "2380", Status::Conditional, 1),
];

static NAD_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "3035", Status::Mandatory, 1),
    ElementRef::new(2, "C082", Status::Mandatory, 1),
];

static DTM_DEF: SegmentDefinition = SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);

static NAD_DEF: SegmentDefinition = SegmentDefinition::new("NAD", "Name and address", NAD_ELEMENTS);

fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    match tag {
        "DTM" => Some(&DTM_DEF),
        "NAD" => Some(&NAD_DEF),
        _ => None,
    }
}

fn is_code_valid(_de: &str, _code: &str) -> bool {
    true
}

fn suggest_code(_de: &str, _code: &str) -> Option<&'static str> {
    None
}

fn expected_components(tag: &str, element_idx: usize) -> Option<u8> {
    match (tag, element_idx) {
        ("DTM", 0) => Some(3),
        ("NAD", 1) => Some(3),
        _ => None,
    }
}

fn new_validator() -> DirectoryValidator {
    DirectoryValidator::new(
        "TEST",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
    .structure_only()
}

#[test]
fn conformance_accepts_real_world_composite_with_internal_empty_component() {
    let input = b"NAD+BY+4000001000002::9'";
    let segments = from_bytes(input).collect::<Result<Vec<_>, _>>().unwrap();

    let validator = new_validator();
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(report.is_valid(), "expected valid report, got {report:?}");
}

#[test]
fn conformance_accepts_composite_when_first_component_empty_but_later_present() {
    let input = b"NAD+BY+:12345:9'";
    let segments = from_bytes(input).collect::<Result<Vec<_>, _>>().unwrap();

    let validator = new_validator();
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(report.is_valid(), "expected valid report, got {report:?}");
}

#[test]
fn conformance_accepts_trailing_empty_components_when_effective_count_matches() {
    let input = b"DTM+137:20260401:102::'";
    let segments = from_bytes(input).collect::<Result<Vec<_>, _>>().unwrap();

    let validator = new_validator();
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(report.is_valid(), "expected valid report, got {report:?}");
}

#[test]
fn conformance_rejects_mandatory_composite_when_all_components_empty() {
    let input = b"DTM+::'";
    let segments = from_bytes(input).collect::<Result<Vec<_>, _>>().unwrap();

    let validator = new_validator();
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(report.has_errors(), "expected errors, got {report:?}");
    assert!(
        report
            .errors()
            .iter()
            .any(|issue| issue.message.contains("required element")),
        "expected missing-required-element issue, got {report:?}"
    );
}

#[test]
fn conformance_flags_underfilled_composite_component_count() {
    let input = b"DTM+137:20260401'";
    let segments = from_bytes(input).collect::<Result<Vec<_>, _>>().unwrap();

    let validator = new_validator();
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(
        report.has_errors(),
        "expected errors for component count mismatch, got {report:?}"
    );
    assert!(
        report
            .errors()
            .iter()
            .any(|issue| issue.message.contains("expected 3")),
        "expected component-count error, got {report:?}"
    );
}

#[test]
fn conformance_rejects_unknown_tags_when_enforced() {
    let input = b"ZZZ+X'";
    let segments = from_bytes(input).collect::<Result<Vec<_>, _>>().unwrap();

    let validator = new_validator();
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(report.has_errors(), "expected errors, got {report:?}");
    assert!(
        report
            .errors()
            .iter()
            .any(|issue| issue.message.contains("not valid for message type")),
        "expected unknown-segment issue, got {report:?}"
    );
}

#[test]
fn conformance_can_run_structure_checks_without_code_lists() {
    let input = b"NAD+BY+4000001000002::9'DTM+137:20260401:102'";
    let segments = from_bytes(input).collect::<Result<Vec<_>, _>>().unwrap();

    let validator = new_validator();
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(report.is_valid(), "expected valid report, got {report:?}");
}

#[test]
fn conformance_surfaces_parse_errors_before_validation() {
    let input = b"DTM+137:20260401:102?"; // dangling release sequence
    let result = from_bytes(input).collect::<Result<Vec<_>, EdifactError>>();
    assert!(matches!(
        result,
        Err(EdifactError::InvalidReleaseSequence { .. })
    ));
}

#[test]
fn owned_definitions_take_precedence_over_static_lookup() {
    let validator =
        DirectoryValidator::from_owned_definitions(vec![OwnedSegmentDef::new_unchecked(
            "NAD".to_owned(),
            "Name and address (runtime)".to_owned(),
            vec![OwnedElementRef::new_unchecked(
                1,
                "3035".to_owned(),
                Status::Mandatory,
                1,
            )],
        )])
        .with_directory_id("RUNTIME")
        .structure_only();

    let valid_segments = from_bytes(b"NAD+BY'")
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut valid_report = ValidationReport::default();
    validator.validate_batch(
        &valid_segments,
        &mut valid_report,
        &ValidationRuleContext::empty(),
    );
    assert!(
        valid_report.is_valid(),
        "expected runtime definition to win over static lookup"
    );

    let invalid_segments = from_bytes(b"NAD+'").collect::<Result<Vec<_>, _>>().unwrap();
    let mut invalid_report = ValidationReport::default();
    validator.validate_batch(
        &invalid_segments,
        &mut invalid_report,
        &ValidationRuleContext::empty(),
    );
    assert!(
        invalid_report.has_errors(),
        "expected runtime mandatory check to apply"
    );
    assert!(
        invalid_report
            .errors()
            .iter()
            .any(|issue| issue.message.contains("required element")),
        "expected missing required element error, got {invalid_report:?}"
    );
}

#[test]
fn owned_element_ref_try_new_rejects_position_zero() {
    let err = OwnedElementRef::try_new(0, "3035".to_owned(), Status::Mandatory, 1)
        .expect_err("position 0 must be rejected");
    assert!(
        matches!(err, EdifactError::InvalidElementPosition),
        "expected InvalidElementPosition (E025), got {err:?}"
    );
    assert_eq!(err.stable_code(), "E025");
}

#[test]
fn from_owned_definitions_accepts_valid_definitions() {
    // All invariants are enforced at OwnedElementRef/OwnedSegmentDef construction time;
    // from_owned_definitions is now infallible — this just verifies it doesn't panic.
    let _validator =
        DirectoryValidator::from_owned_definitions(vec![OwnedSegmentDef::new_unchecked(
            "BGM".to_owned(),
            "test".to_owned(),
            vec![OwnedElementRef::new_unchecked(
                1,
                "1001".to_owned(),
                Status::Mandatory,
                1,
            )],
        )]);
}

// ── exhaustive per-segment reporting ─────────────────────────────────────────

#[test]
fn every_violation_in_a_segment_is_reported_not_just_the_first() {
    // `NAD` declares two mandatory elements; supplying neither is two distinct
    // faults. The validator used to return on the first `Err`, so a report whose
    // whole purpose is to be exhaustive showed one issue per segment and the
    // caller fixed the message one round-trip at a time.
    let segments: Vec<_> = from_bytes(b"NAD++'")
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");

    let mut report = ValidationReport::default();
    new_validator().validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    let missing: Vec<_> = report
        .errors()
        .iter()
        .filter(|i| i.error_code() == Some("E008"))
        .collect();
    assert_eq!(
        missing.len(),
        2,
        "expected both mandatory elements reported, got {:#?}",
        report.errors()
    );
    assert_eq!(missing[0].element_index, Some(0));
    assert_eq!(missing[1].element_index, Some(1));
}

#[test]
fn faults_from_different_check_families_are_all_reported() {
    // One segment, two unrelated problems: a missing mandatory element (E008)
    // and a composite whose component count does not match (E013).
    let segments: Vec<_> = from_bytes(b"NAD++A:B'")
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");

    let mut report = ValidationReport::default();
    new_validator().validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    let codes: Vec<_> = report
        .iter_issues()
        .filter_map(|i| i.error_code())
        .collect();
    assert!(
        codes.contains(&"E008"),
        "missing element not reported: {codes:?}"
    );
    assert!(
        codes.contains(&"E013"),
        "component-count fault not reported: {codes:?}"
    );
}

// ── representation and repetition enforcement ─────────────────────────────────

mod representation {
    use edifact_rs::{
        ComponentRef, DirectoryValidator, ElementRef, Repr, SegmentDefinition, Status,
        ValidationContext, ValidationLayer, from_bytes, service,
    };

    fn service_report(input: &[u8]) -> edifact_rs::ValidationReport {
        let segments: Vec<_> = from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let validator = DirectoryValidator::new(
            "iso-9735-service",
            service::lookup,
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate(&segments)
    }

    fn codes(report: &edifact_rs::ValidationReport) -> Vec<&str> {
        report
            .iter_issues()
            .filter_map(edifact_rs::ValidationIssue::error_code)
            .collect()
    }

    #[test]
    fn a_non_numeric_control_count_is_rejected() {
        // UNZ DE 0036 is `n..6`.  Nothing previously checked this: the segment
        // has the right arity, so structure validation passed it through.
        let report = service_report(b"UNZ+abc+IC1'");
        assert!(codes(&report).contains(&"E048"), "{report:#?}");
    }

    #[test]
    fn an_oversized_interchange_control_reference_is_rejected() {
        // DE 0020 is `an..14`; this is 20 characters.
        let report = service_report(b"UNZ+1+ABCDEFGHIJKLMNOPQRST'");
        assert!(codes(&report).contains(&"E049"), "{report:#?}");
    }

    #[test]
    fn a_fixed_length_value_that_is_short_is_rejected() {
        // S001 DE 0001 is `a4`; `UNO` is three characters.
        let report = service_report(b"UNB+UNO:3+S+R+260101:0900+IC1'");
        assert!(codes(&report).contains(&"E050"), "{report:#?}");
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        // ISO 9735-1 §6: one graphic character counts once whatever its
        // encoding.  `ü` is two UTF-8 bytes and must not count as two.
        static NAME: &[ElementRef] =
            &[ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::an_up_to(5))];
        static SEG: SegmentDefinition = SegmentDefinition::new("ZZZ", "Test", NAME);

        let segments: Vec<_> = from_bytes("ZZZ+üüüüü'".as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let validator = DirectoryValidator::new(
            "t",
            |tag| (tag == "ZZZ").then_some(&SEG),
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate(&segments);
        assert!(!report.has_errors(), "{report:#?}");
    }

    #[test]
    fn a_numeric_length_excludes_sign_decimal_mark_and_exponent() {
        // ISO 9735-1 §10: the length "shall not include the minus sign, the
        // decimal mark, or the exponent mark and its exponent".  `-123.45` is
        // five characters, so `n..5` admits it.
        static AMOUNT: &[ElementRef] =
            &[ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::n_up_to(5))];
        static SEG: SegmentDefinition = SegmentDefinition::new("ZZZ", "Test", AMOUNT);

        let segments: Vec<_> = from_bytes(b"ZZZ+-123.45'")
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let validator = DirectoryValidator::new(
            "t",
            |tag| (tag == "ZZZ").then_some(&SEG),
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate(&segments);
        assert!(!report.has_errors(), "{report:#?}");
    }

    #[test]
    fn a_plus_sign_or_space_is_not_a_numeric_value() {
        // §10 excludes both explicitly.
        for value in [&b"ZZZ++123'"[..], &b"ZZZ+1 2'"[..]] {
            static AMOUNT: &[ElementRef] =
                &[ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::n_up_to(9))];
            static SEG: SegmentDefinition = SegmentDefinition::new("ZZZ", "Test", AMOUNT);

            let segments: Vec<_> = from_bytes(value)
                .collect::<Result<Vec<_>, _>>()
                .expect("parse");
            let validator = DirectoryValidator::new(
                "t",
                |tag| (tag == "ZZZ").then_some(&SEG),
                |_, _| true,
                |_, _| None,
                |_, _| None,
                None,
            );
            let report = ValidationContext::builder()
                .with_validator(ValidationLayer::Structure, validator)
                .build()
                .validate(&segments);
            // `ZZZ++123` leaves element 0 empty and puts `123` in element 1;
            // `ZZZ+1 2` is a single value with an embedded space.
            assert!(
                report.has_errors(),
                "{:?} must be rejected: {report:#?}",
                std::str::from_utf8(value).unwrap()
            );
        }
    }

    // ── the one position where v4 is not a superset of v3 ────────────────────
    //
    // S004 DE 0017: version 3 transfers YYMMDD (n6), version 4 CCYYMMDD (n8).
    // Collapsing them into `n..8` would validate neither version correctly, so
    // the table declares both and the checker picks by UNB S001 DE 0002.

    #[test]
    fn a_version_3_date_is_checked_as_n6() {
        let ok = service_report(b"UNB+UNOA:3+SENDER+RECEIVER+200101:0900+IC1'");
        assert!(!ok.has_errors(), "YYMMDD must pass under v3: {ok:#?}");

        // A version 4 date in a version 3 interchange is wrong for that version.
        let bad = service_report(b"UNB+UNOA:3+SENDER+RECEIVER+20200101:0900+IC1'");
        assert!(
            codes(&bad).contains(&"E049"),
            "CCYYMMDD must be rejected under v3: {bad:#?}"
        );
    }

    #[test]
    fn a_version_4_date_is_checked_as_n8() {
        let ok = service_report(b"UNB+UNOC:4+SENDER+RECEIVER+20260101:0900+IC1'");
        assert!(!ok.has_errors(), "CCYYMMDD must pass under v4: {ok:#?}");

        // The `n..8` compromise this replaced accepted exactly this — a version
        // 3 date silently passing as a version 4 one.
        let bad = service_report(b"UNB+UNOC:4+SENDER+RECEIVER+260101:0900+IC1'");
        assert!(
            codes(&bad).contains(&"E050"),
            "YYMMDD must be rejected under v4: {bad:#?}"
        );
    }

    #[test]
    fn an_unknown_syntax_version_accepts_either_date_form() {
        // Validating a message window, or any slice with no UNB, cannot know the
        // version.  Guessing would reject conformant data from whichever version
        // was guessed against, so both forms are accepted.
        static S004: &[ComponentRef] = &[ComponentRef::new(1, "0017", Status::Mandatory)
            .with_repr_by_syntax_version(Repr::n(6), Repr::n(8))];
        static ELEMENTS: &[ElementRef] =
            &[ElementRef::composite(1, "S004", Status::Mandatory, 1, S004)];
        static SEG: SegmentDefinition = SegmentDefinition::new("ZZZ", "Test", ELEMENTS);

        for date in [&b"ZZZ+200101'"[..], &b"ZZZ+20200101'"[..]] {
            let segments: Vec<_> = from_bytes(date)
                .collect::<Result<Vec<_>, _>>()
                .expect("parse");
            let validator = DirectoryValidator::new(
                "t",
                |tag| (tag == "ZZZ").then_some(&SEG),
                |_, _| true,
                |_, _| None,
                |_, _| None,
                None,
            );
            let report = ValidationContext::builder()
                .with_validator(ValidationLayer::Structure, validator)
                .build()
                .validate(&segments);
            assert!(
                !report.has_errors(),
                "{:?} must pass with no UNB: {report:#?}",
                std::str::from_utf8(date).unwrap()
            );
        }

        // A length that is neither form is still wrong.
        let segments: Vec<_> = from_bytes(b"ZZZ+2001011'")
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let validator = DirectoryValidator::new(
            "t",
            |tag| (tag == "ZZZ").then_some(&SEG),
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate(&segments);
        assert!(report.has_errors(), "seven digits is neither n6 nor n8");
    }

    #[test]
    fn max_repeat_is_enforced() {
        // Previously dead metadata: every ElementRef carried a maximum and
        // nothing read it, so a definition saying "once" constrained nothing.
        static ONCE: &[ElementRef] = &[ElementRef::new(1, "9999", Status::Mandatory, 1)];
        static SEG: SegmentDefinition = SegmentDefinition::new("ZZZ", "Test", ONCE);

        let segments: Vec<_> = from_bytes(b"UNA:+.?*'ZZZ+a*b*c'")
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let validator = DirectoryValidator::new(
            "t",
            |tag| (tag == "ZZZ").then_some(&SEG),
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate(&segments);
        assert!(
            report.iter_issues().any(|i| i.error_code() == Some("E047")),
            "{report:#?}"
        );
    }

    #[test]
    fn a_definition_without_representations_reports_nothing_new() {
        // A partial table must stay useful rather than become a source of
        // false findings, so unstated positions are simply not checked.
        static BARE: &[ComponentRef] = &[ComponentRef::new(1, "9999", Status::Mandatory)];
        static ELEMENTS: &[ElementRef] =
            &[ElementRef::composite(1, "C999", Status::Mandatory, 1, BARE)];
        static SEG: SegmentDefinition = SegmentDefinition::new("ZZZ", "Test", ELEMENTS);

        let segments: Vec<_> = from_bytes(b"ZZZ+anything at all goes here'")
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let validator = DirectoryValidator::new(
            "t",
            |tag| (tag == "ZZZ").then_some(&SEG),
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate(&segments);
        assert!(!report.has_errors(), "{report:#?}");
    }
}
