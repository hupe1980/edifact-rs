use edifact_rs::{
    DirectoryValidator, EdifactError, ElementRef, OwnedElementRef, OwnedSegmentDef,
    SegmentDefinition, Status, ValidationReport, ValidationRuleContext, Validator, from_bytes,
};

static DTM_ELEMENTS: &[ElementRef] = &[
    ElementRef {
        position: 1,
        data_element: "C507",
        status: Status::Mandatory,
        max_repeat: 1,
    },
    ElementRef {
        position: 2,
        data_element: "2380",
        status: Status::Conditional,
        max_repeat: 1,
    },
];

static NAD_ELEMENTS: &[ElementRef] = &[
    ElementRef {
        position: 1,
        data_element: "3035",
        status: Status::Mandatory,
        max_repeat: 1,
    },
    ElementRef {
        position: 2,
        data_element: "C082",
        status: Status::Mandatory,
        max_repeat: 1,
    },
];

static DTM_DEF: SegmentDefinition = SegmentDefinition {
    tag: "DTM",
    name: "Date/time/period",
    elements: DTM_ELEMENTS,
};

static NAD_DEF: SegmentDefinition = SegmentDefinition {
    tag: "NAD",
    name: "Name and address",
    elements: NAD_ELEMENTS,
};

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
    let validator = DirectoryValidator::from_owned_definitions(vec![OwnedSegmentDef::new(
        "NAD".to_owned(),
        "Name and address (runtime)".to_owned(),
        vec![OwnedElementRef::new(
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
    let _validator = DirectoryValidator::from_owned_definitions(vec![OwnedSegmentDef::new(
        "BGM".to_owned(),
        "test".to_owned(),
        vec![OwnedElementRef::new(
            1,
            "1001".to_owned(),
            Status::Mandatory,
            1,
        )],
    )]);
}
