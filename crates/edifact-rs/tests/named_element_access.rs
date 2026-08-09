#![cfg(feature = "derive")]

//! Code-addressed element access: runtime accessors and the `layout` derive.
//!
//! The property under test throughout is that a *wrong* data element reference
//! fails loudly — as a directory lookup error at runtime, or as a failed `const`
//! assertion at compile time — instead of quietly reading the neighbouring
//! element the way a transposed positional index does.

use edifact_rs::{
    ComponentRef, EdifactDeserialize, EdifactError, EdifactSerialize, ElementRef,
    OwnedComponentRef, OwnedElementRef, OwnedSegmentDef, SegmentDefinition, SegmentLayout, Status,
};

// ── directory tables ──────────────────────────────────────────────────────────
//
// `const` rather than `static`: the derive resolves identifiers during const
// evaluation, and a `const` item cannot read a `static`.

const C082_COMPONENTS: &[ComponentRef] = &[
    ComponentRef::new(1, "3039", Status::Mandatory),
    ComponentRef::new(2, "1131", Status::Conditional),
    ComponentRef::new(3, "3055", Status::Conditional),
];

const NAD_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "3035", Status::Mandatory, 1),
    ElementRef::composite(2, "C082", Status::Conditional, 1, C082_COMPONENTS),
];

pub const NAD: SegmentDefinition = SegmentDefinition::new("NAD", "Name and address", NAD_ELEMENTS);

const C507_COMPONENTS: &[ComponentRef] = &[
    ComponentRef::new(1, "2005", Status::Mandatory),
    ComponentRef::new(2, "2380", Status::Conditional),
    ComponentRef::new(3, "2379", Status::Conditional),
];

const DTM_ELEMENTS: &[ElementRef] = &[ElementRef::composite(
    1,
    "C507",
    Status::Mandatory,
    1,
    C507_COMPONENTS,
)];

pub const DTM: SegmentDefinition = SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);

// Resolution runs at compile time, so these are checked by the build itself.
const _: () = assert!(NAD.element_slot("3035") == 0);
const _: () = assert!(NAD.element_slot("3055") == 1);
const _: () = assert!(NAD.component_slot("3055") == 2);
const _: () = assert!(NAD.component_slot("C082") == 0);
const _: () = assert!(NAD.code_positions("9999") == 0);

// ── derive under a layout ─────────────────────────────────────────────────────

#[derive(Debug, PartialEq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier = "MS", layout = NAD)]
struct SenderParty {
    #[edifact(element = "3039")]
    party_id: String,
    #[edifact(element = "3055")]
    agency: Option<String>,
}

#[derive(Debug, PartialEq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "DTM", layout = DTM)]
struct DocumentDate {
    #[edifact(element = "2005")]
    qualifier: String,
    #[edifact(element = "2380")]
    value: String,
    #[edifact(element = "2379")]
    format: Option<String>,
}

/// A whole-element identifier and a component identifier, both `required`.
///
/// Exists to pin which "missing required" variant each shape reports.
#[derive(Debug, PartialEq, EdifactDeserialize)]
#[edifact(segment = "NAD", layout = NAD)]
struct RequiredShapes {
    #[edifact(element = "3035", required)]
    qualifier: Option<String>,
    #[edifact(element = "3055", required)]
    agency: Option<String>,
}

fn parse(input: &[u8]) -> Vec<edifact_rs::Segment<'_>> {
    edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture must parse")
}

// ── runtime accessors ─────────────────────────────────────────────────────────

#[test]
fn value_by_code_reads_elements_and_composite_components() {
    let segments = parse(b"NAD+MS+9900112233445::293'");
    let nad = &segments[0];

    assert_eq!(nad.value_by_code(&NAD, "3035").unwrap(), Some("MS"));
    assert_eq!(
        nad.value_by_code(&NAD, "3039").unwrap(),
        Some("9900112233445")
    );
    assert_eq!(nad.value_by_code(&NAD, "1131").unwrap(), Some(""));
    assert_eq!(nad.value_by_code(&NAD, "3055").unwrap(), Some("293"));
    // The composite addressed as a whole yields its first component.
    assert_eq!(
        nad.value_by_code(&NAD, "C082").unwrap(),
        Some("9900112233445")
    );
}

#[test]
fn unknown_data_element_is_an_error_not_a_wrong_read() {
    let segments = parse(b"NAD+MS+9900112233445::293'");
    let nad = &segments[0];

    // DE 2380 belongs to DTM, not NAD.  Positionally this would have read
    // *something*; by code it cannot.
    let err = nad.value_by_code(&NAD, "2380").unwrap_err();
    assert!(
        matches!(&err, EdifactError::UnknownDataElement { tag, data_element }
            if tag == "NAD" && data_element == "2380"),
        "expected UnknownDataElement, got {err:?}"
    );
    assert_eq!(err.stable_code(), "E033");
}

#[test]
fn applying_a_layout_to_the_wrong_segment_is_rejected() {
    let segments = parse(b"NAD+MS+9900112233445::293'");
    let err = segments[0].value_by_code(&DTM, "2005").unwrap_err();
    assert!(
        matches!(&err, EdifactError::SegmentLayoutMismatch { expected, actual }
            if expected == "DTM" && actual == "NAD"),
        "expected SegmentLayoutMismatch, got {err:?}"
    );
}

#[test]
fn ambiguous_data_element_is_rejected() {
    // A directory that repeats a code cannot be code-addressed for it.
    const REPEATED: &[ElementRef] = &[
        ElementRef::new(1, "1153", Status::Mandatory, 1),
        ElementRef::new(2, "1153", Status::Conditional, 1),
    ];
    const RFF: SegmentDefinition = SegmentDefinition::new("RFF", "Reference", REPEATED);

    let err = RFF.resolve_code("1153").unwrap_err();
    assert!(
        matches!(&err, EdifactError::AmbiguousDataElement { data_element, .. } if data_element == "1153"),
        "expected AmbiguousDataElement, got {err:?}"
    );
    assert_eq!(err.stable_code(), "E034");
}

#[test]
fn span_by_code_points_at_the_addressed_value() {
    let input = b"NAD+MS+9900112233445::293'";
    let segments = parse(input);
    let span = segments[0]
        .span_by_code(&NAD, "3055")
        .unwrap()
        .expect("DE 3055 is present");
    assert_eq!(&input[span.start..span.end], b"293");
}

#[test]
fn owned_and_borrowed_segments_resolve_identically() {
    let input = b"NAD+MS+9900112233445::293'";
    let owned: Vec<edifact_rs::OwnedSegment> =
        edifact_rs::from_reader_collect(std::io::Cursor::new(input)).expect("fixture must parse");

    assert_eq!(owned[0].value_by_code(&NAD, "3055").unwrap(), Some("293"));
    assert_eq!(
        owned[0].borrow().value_by_code(&NAD, "3039").unwrap(),
        Some("9900112233445")
    );
    assert_eq!(
        owned[0].span_by_code(&NAD, "3055").unwrap(),
        owned[0].borrow().span_by_code(&NAD, "3055").unwrap(),
    );
}

#[test]
fn runtime_loaded_definitions_resolve_the_same_codes() {
    // Directories loaded from JSON at startup get the same addressing as
    // compile-time tables.
    let def = OwnedSegmentDef::new_unchecked(
        "NAD".to_owned(),
        "Name and address".to_owned(),
        vec![
            OwnedElementRef::new_unchecked(1, "3035".to_owned(), Status::Mandatory, 1),
            OwnedElementRef::new_unchecked(2, "C082".to_owned(), Status::Conditional, 1)
                .with_components(vec![
                    OwnedComponentRef::new_unchecked(1, "3039".to_owned(), Status::Mandatory),
                    OwnedComponentRef::new_unchecked(2, "1131".to_owned(), Status::Conditional),
                    OwnedComponentRef::new_unchecked(3, "3055".to_owned(), Status::Conditional),
                ]),
        ],
    );

    let segments = parse(b"NAD+MS+9900112233445::293'");
    assert_eq!(
        segments[0].value_by_code(&def, "3055").unwrap(),
        Some("293")
    );
    assert_eq!(
        def.resolve_code("3039").unwrap(),
        NAD.resolve_code("3039").unwrap()
    );
    assert!(segments[0].value_by_code(&def, "2380").is_err());
}

// ── derive round-trip ─────────────────────────────────────────────────────────

#[test]
fn code_addressed_derive_deserializes_from_the_right_slots() {
    let segments = parse(b"NAD+MS+9900112233445::293'");
    let party = SenderParty::edifact_deserialize(&segments).expect("deserialize");
    assert_eq!(party.party_id, "9900112233445");
    assert_eq!(party.agency.as_deref(), Some("293"));
}

#[test]
fn code_addressed_derive_round_trips() {
    let segments = parse(b"NAD+MS+9900112233445::293'DTM+137:20260101:102'");

    let party = SenderParty::edifact_deserialize(&segments).expect("deserialize NAD");
    assert_eq!(
        edifact_rs::to_edifact_string(&party).expect("serialize NAD"),
        "NAD+MS+9900112233445::293'"
    );

    let date = DocumentDate::edifact_deserialize(&segments).expect("deserialize DTM");
    assert_eq!(date.qualifier, "137");
    assert_eq!(date.value, "20260101");
    assert_eq!(date.format.as_deref(), Some("102"));
    assert_eq!(
        edifact_rs::to_edifact_string(&date).expect("serialize DTM"),
        "DTM+137:20260101:102'"
    );
}

#[test]
fn code_addressed_derive_matches_the_owned_path() {
    let input = b"NAD+MS+9900112233445::293'";
    let owned: Vec<edifact_rs::OwnedSegment> =
        edifact_rs::from_reader_collect(std::io::Cursor::new(input)).expect("parse");
    let borrowed = parse(input);

    assert_eq!(
        SenderParty::edifact_deserialize_owned(&owned).expect("owned"),
        SenderParty::edifact_deserialize(&borrowed).expect("borrowed"),
    );
}

#[test]
fn required_reports_the_variant_matching_what_the_field_addresses() {
    // A whole-element identifier must report E008, a component identifier E021.
    // Both resolve to component index 0 for the first component, so a naive
    // "has a component index" test picks the wrong code — and downstream
    // routing keyed on `error_code` then takes the wrong branch.
    let missing_qualifier = parse(b"NAD++9900112233445::293'");
    let err = RequiredShapes::edifact_deserialize(&missing_qualifier).unwrap_err();
    assert!(
        matches!(&err, EdifactError::MissingRequiredElement { tag, element_index }
            if tag == "NAD" && *element_index == 0),
        "DE 3035 names a whole element, expected MissingRequiredElement, got {err:?}"
    );
    assert_eq!(err.stable_code(), "E008");

    let missing_agency = parse(b"NAD+MS+9900112233445'");
    let err = RequiredShapes::edifact_deserialize(&missing_agency).unwrap_err();
    assert!(
        matches!(&err, EdifactError::MissingRequiredComponent { tag, element_index, component_index }
            if tag == "NAD" && *element_index == 1 && *component_index == 2),
        "DE 3055 names a component, expected MissingRequiredComponent, got {err:?}"
    );
    assert_eq!(err.stable_code(), "E021");
}

#[test]
fn required_variant_agrees_across_borrowed_and_owned_paths() {
    let input = b"NAD++9900112233445::293'";
    let owned: Vec<edifact_rs::OwnedSegment> =
        edifact_rs::from_reader_collect(std::io::Cursor::new(input)).expect("parse");
    let borrowed = parse(input);

    let from_owned = RequiredShapes::edifact_deserialize_owned(&owned).unwrap_err();
    let from_borrowed = RequiredShapes::edifact_deserialize(&borrowed).unwrap_err();
    assert_eq!(from_owned.stable_code(), from_borrowed.stable_code());
    assert_eq!(from_owned, from_borrowed);
}

#[test]
fn declared_components_cap_a_composite_arity() {
    use edifact_rs::{DirectoryValidator, ValidationReport, Validator};

    // C082 declares three components; a fourth is a structural error the
    // directory can now catch on its own.
    const DEFS: &[SegmentDefinition] = &[NAD];
    let validator = DirectoryValidator::from_definitions(DEFS).enforce_known_tags(false);

    let too_many = parse(b"NAD+MS+9900112233445::293:EXTRA'");
    let mut report = ValidationReport::default();
    validator.validate_batch(
        &too_many,
        &mut report,
        &edifact_rs::ValidationRuleContext::empty(),
    );
    assert!(
        report
            .errors()
            .iter()
            .any(|i| i.error_code() == Some("E013")),
        "expected InvalidComponentCount, got {}",
        report.render_deterministic()
    );

    // Fewer components than declared is normal: conditional components may be
    // omitted, and trailing empties are stripped per ISO 9735-1 §3.3.
    let fewer = parse(b"NAD+MS+9900112233445'");
    let mut report = ValidationReport::default();
    validator.validate_batch(
        &fewer,
        &mut report,
        &edifact_rs::ValidationRuleContext::empty(),
    );
    assert!(
        !report.has_errors(),
        "omitting conditional components must stay valid, got {}",
        report.render_deterministic()
    );
}

#[test]
fn absent_optional_component_serializes_as_an_empty_slot() {
    // A missing trailing component must still hold its position, exactly as the
    // positional derive emits it.
    let party = SenderParty {
        party_id: "9900112233445".to_owned(),
        agency: None,
    };
    assert_eq!(
        edifact_rs::to_edifact_string(&party).expect("serialize"),
        "NAD+MS+9900112233445::'"
    );
}

// ── composites that repeat a data element by design ───────────────────────────

/// `C080 PARTY NAME` carries `3036` five times, then `3045`.
///
/// Declared as five separate `ComponentRef::new` entries, `code_positions` counts
/// five positions and `3036` resolves as ambiguous — so a *faithful* declaration
/// made the component unaddressable, and the only way to use named access was to
/// declare the composite incompletely.
const C080_FAITHFUL: &[ComponentRef] = &[
    ComponentRef::repeated(1, "3036", Status::Mandatory, 5),
    ComponentRef::new(6, "3045", Status::Conditional),
];
const NAD_C080_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "3035", Status::Mandatory, 1),
    ElementRef::composite(2, "C080", Status::Conditional, 1, C080_FAITHFUL),
];
const NAD_C080: SegmentDefinition =
    SegmentDefinition::new("NAD", "Name and address", NAD_C080_ELEMENTS);

#[test]
fn a_by_design_repeat_stays_addressable_and_points_at_the_first_occurrence() {
    assert_eq!(
        NAD_C080.code_positions("3036"),
        1,
        "one entry, one position"
    );

    let path = NAD_C080.resolve_code("3036").expect("addressable");
    assert_eq!((path.element, path.component), (1, Some(0)));

    let segments: Vec<_> = edifact_rs::from_bytes(b"NAD+BY+ACME:GMBH:::+X'")
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");
    assert_eq!(
        segments[0].value_by_code(&NAD_C080, "3036").unwrap(),
        Some("ACME"),
    );
    // The later components keep the positions the repeat pushed them to.
    assert_eq!(NAD_C080.resolve_code("3045").unwrap().component, Some(5));
}

#[test]
fn the_repeat_count_is_recorded_rather_than_lost() {
    let c080 = NAD_C080.elements[1];
    assert_eq!(c080.components()[0].repeat_count(), 5);
    assert_eq!(c080.components()[1].repeat_count(), 1);
}

#[test]
fn a_repeated_composite_is_not_capped_at_its_entry_count() {
    // Six values in the composite: the arity check must count slots (6), not
    // declared entries (2), or a conformant C080 is rejected.
    let segments: Vec<_> = edifact_rs::from_bytes(b"NAD+BY+A:B:C:D:E:F'")
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");

    let validator =
        edifact_rs::DirectoryValidator::from_definitions(std::slice::from_ref(&NAD_C080_STATIC));
    let report = edifact_rs::ValidationContext::builder()
        .with_validator(edifact_rs::ValidationLayer::Structure, validator)
        .build()
        .validate_lenient(&segments);

    assert!(
        !report
            .errors()
            .iter()
            .any(|i| i.error_code() == Some("E013")),
        "six components must fit a composite declaring 5+1 slots: {:#?}",
        report.errors(),
    );
}

static NAD_C080_STATIC: SegmentDefinition = NAD_C080;
