//! `ContrlLocation`: one test per subject-input error code.
//!
//! Every stable code a subject interchange can produce is placed on a known
//! interchange and the full location tuple is asserted — reporting level,
//! message/group, DE 0096, service segment, DE 0098, DE 0104, DE 0085 and the
//! exact flag. Codes that are not faults of the subject input (writer,
//! serializer and definition errors) must have no location at all.
//!
//! Counting rules (ISO 9735-4, BDEW CONTRL MIG 2.0b): DE 0096 counts from
//! `UNH` = 1; DE 0098 counts the segment tag as 1; DE 0104 counts from 1.

use bolero::check;
use edifact_rs::contrl::{ContrlLocation, ReportingLevel};
use edifact_rs::{
    EdifactError, Insignificant, RecoveredInterchange, RecoveryConfig, Span, ValidationReport,
};

/// Items: 0 UNB, 1 UNH, 2 BGM, 3 DTM, 4 UNT, 5 UNZ.
const IC: &[u8] = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
UNH+M1+ORDERS:D:96A:UN'BGM+220+PO1+9'DTM+137:20260101:102'UNT+4+M1'\
UNZ+1+IC1'";

/// Items: 0 UNB, 1 UNG, 2 UNH, 3 BGM, 4 UNT, 5 UNE, 6 UNZ.
const GROUPED: &[u8] = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
UNG+ORDERS+SND+RCV+260101:0900+G1+UN+D:96A'\
UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'\
UNE+1+G1'UNZ+1+IC1'";

fn scan(input: &[u8]) -> RecoveredInterchange<'_> {
    RecoveredInterchange::scan(input, &RecoveryConfig::default())
}

/// The span of item `index`.
fn span(recovered: &RecoveredInterchange<'_>, index: usize) -> Span {
    recovered.items()[index].span()
}

/// The full location tuple, in data-model order.
type Tuple = (
    ReportingLevel,
    Option<usize>,        // message
    Option<u32>,          // DE 0096
    Option<&'static str>, // service segment
    Option<u32>,          // DE 0098
    Option<u32>,          // DE 0104
    &'static str,         // DE 0085
    bool,                 // exact
);

fn tuple(location: &ContrlLocation) -> Tuple {
    assert_invariants(location);
    (
        location.level(),
        location.message(),
        location.segment_position(),
        location.service_segment(),
        location.element_position(),
        location.component_position(),
        location.error().code(),
        location.is_exact(),
    )
}

/// The invariants of every location (contract C5).
#[track_caller]
fn assert_invariants(l: &ContrlLocation) {
    assert!(
        l.error().permitted_at(l.level()),
        "{l:?}: code not permitted at its level"
    );
    match l.level() {
        ReportingLevel::Segment | ReportingLevel::DataElement => {
            assert!(l.message().is_some(), "{l:?}");
            assert!(l.segment_position().is_some(), "{l:?}");
            assert!(l.service_segment().is_none(), "{l:?}");
        }
        _ => assert!(l.segment_position().is_none(), "{l:?}"),
    }
    if l.level() == ReportingLevel::DataElement {
        assert!(l.element_position().is_some(), "{l:?}");
    }
    if l.component_position().is_some() {
        assert!(l.element_position().is_some(), "{l:?}");
    }
    if let Some(service) = l.service_segment() {
        let allowed: &[&str] = match l.level() {
            ReportingLevel::Interchange => &["UNA", "UNB", "UNZ"],
            ReportingLevel::Group => &["UNG", "UNE"],
            ReportingLevel::Message => &["UNH", "UNT"],
            _ => &[],
        };
        assert!(allowed.contains(&service), "{l:?}");
    }
    match l.level() {
        ReportingLevel::Interchange => assert!(l.message().is_none() && l.group().is_none()),
        ReportingLevel::Group => assert!(l.group().is_some() && l.message().is_none()),
        _ => assert!(l.message().is_some()),
    }
}

/// Locate `error` on `IC` with an optional message hint.
fn at(error: &EdifactError, message: Option<usize>) -> Tuple {
    let recovered = scan(IC);
    tuple(&recovered.locate(error, message).expect("a subject fault"))
}

/// The location of the only fault of `input`.
fn only_fault(input: &[u8], config: &RecoveryConfig) -> Tuple {
    let recovered = RecoveredInterchange::scan(input, config);
    let locations = recovered.fault_locations();
    assert_eq!(locations.len(), 1, "{:?}", recovered.items());
    tuple(&locations[0])
}

use ReportingLevel::{
    DataElement as UCD, Group as UCF, Interchange as UCI, Message as UCM, Segment as UCS,
};

fn spanned(index: usize) -> Span {
    span(&scan(IC), index)
}

// ── lexical errors (offset-based) ────────────────────────────────────────────

#[test]
fn e001_unexpected_eof_is_placed_at_its_offset() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGM+220'DTM+137:2026";
    assert_eq!(
        only_fault(input, &RecoveryConfig::default()),
        (UCD, Some(0), Some(3), None, Some(2), Some(2), "18", true)
    );
}

#[test]
fn e002_an_empty_segment_is_22_on_element_one() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGM+220''DTM+137'UNT+5+M1'UNZ+1+IC1'";
    assert_eq!(
        only_fault(input, &RecoveryConfig::default()),
        (UCD, Some(0), Some(3), None, Some(1), None, "22", true)
    );
}

#[test]
fn e003_invalid_text_is_21_at_its_element() {
    let input =
        b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGM+220+P\xFF1'UNT+3+M1'UNZ+1+IC1'";
    assert_eq!(
        only_fault(input, &RecoveryConfig::default()),
        (UCD, Some(0), Some(2), None, Some(3), None, "21", true)
    );
}

#[test]
fn e019_a_dangling_release_is_22_at_its_element() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGM+220:A?";
    assert_eq!(
        only_fault(input, &RecoveryConfig::default()),
        (UCD, Some(0), Some(2), None, Some(2), Some(2), "22", true)
    );
}

#[test]
fn e020_an_overlong_segment_is_39_on_the_message() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGM+XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX'UNT+3+M1'UNZ+1+IC1'";
    let config =
        RecoveryConfig::new().reader(edifact_rs::ReaderConfig::default().max_segment_bytes(35));
    assert_eq!(
        only_fault(input, &config),
        (UCM, Some(0), None, None, None, None, "39", true)
    );
}

#[test]
fn e028_a_stray_data_token_is_12_on_the_tag_in_a_message_and_33_outside() {
    let recovered = scan(IC);
    let in_bgm = EdifactError::UnexpectedDataToken {
        offset: span(&recovered, 2).start + 4,
    };
    assert_eq!(
        at(&in_bgm, None),
        (UCD, Some(0), Some(2), None, Some(1), None, "12", true)
    );
    let in_unb = EdifactError::UnexpectedDataToken { offset: 4 };
    // S011 of the UCI: element 2 (S001), component 1 — it is a composite.
    assert_eq!(
        at(&in_unb, None),
        (UCI, None, None, Some("UNB"), Some(2), Some(1), "33", true)
    );
}

// ── tag errors ───────────────────────────────────────────────────────────────

#[test]
fn e006_a_bad_tag_is_12_on_element_one_in_a_message_and_33_outside() {
    let in_body =
        b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGME03+X'UNT+3+M1'UNZ+1+IC1'";
    assert_eq!(
        only_fault(in_body, &RecoveryConfig::default()),
        (UCD, Some(0), Some(2), None, Some(1), None, "12", true)
    );
    let outside =
        b"UNB+UNOC:3+S+R+260101:0900+IC1'BGME03+X'UNH+M1+ORDERS:D:96A:UN'BGM+1'UNT+3+M1'UNZ+1+IC1'";
    assert_eq!(
        only_fault(outside, &RecoveryConfig::default()),
        (UCI, None, None, None, None, None, "33", true)
    );
    // Without a matching fault the tag error cannot be placed.
    assert_eq!(
        at(&EdifactError::InvalidSegmentTag("XX".into()), Some(0)),
        (UCI, None, None, None, None, None, "33", false)
    );
}

#[test]
fn e007_an_invalid_una_is_20_on_the_uci_naming_una() {
    let input = b"UNA:::.? 'UNB+UNOC:3+S+R+260101:0900+IC1'UNZ+0+IC1'";
    assert_eq!(
        only_fault(input, &RecoveryConfig::default()),
        (UCI, None, None, Some("UNA"), None, None, "20", true)
    );
}

// ── envelope control errors ──────────────────────────────────────────────────

#[test]
fn e004_a_message_count_mismatch_is_29_on_the_uci_naming_unz() {
    let error = EdifactError::MessageCountMismatch {
        expected: 2,
        actual: 1,
    };
    assert_eq!(
        at(&error, None),
        (UCI, None, None, Some("UNZ"), None, None, "29", true)
    );
}

#[test]
fn e005_a_segment_count_mismatch_is_29_on_the_ucm_naming_unt() {
    let error = EdifactError::SegmentCountMismatch {
        expected: 9,
        actual: 4,
        message_ref: "M1".into(),
        span: spanned(4),
    };
    assert_eq!(
        at(&error, None),
        (UCM, Some(0), None, Some("UNT"), None, None, "29", true)
    );
}

#[test]
fn e016_mismatched_control_references_are_28_on_their_trailer() {
    let unz = EdifactError::QualifierMismatch {
        tag: "UNZ".into(),
        actual: "IC2".into(),
        expected: "IC1".into(),
        span: spanned(5),
    };
    assert_eq!(
        at(&unz, None),
        (UCI, None, None, Some("UNZ"), None, None, "28", true)
    );

    let unt = EdifactError::QualifierMismatch {
        tag: "UNT".into(),
        actual: "M2".into(),
        expected: "M1".into(),
        span: spanned(4),
    };
    assert_eq!(
        at(&unt, None),
        (UCM, Some(0), None, Some("UNT"), None, None, "28", true)
    );

    let recovered = scan(GROUPED);
    let une = EdifactError::QualifierMismatch {
        tag: "UNE".into(),
        actual: "G2".into(),
        expected: "G1".into(),
        span: span(&recovered, 5),
    };
    let location = recovered.locate(&une, None).expect("located");
    assert_invariants(&location);
    assert_eq!(location.level(), UCF);
    assert_eq!(location.group(), Some(0));
    assert_eq!(location.service_segment(), Some("UNE"));
    assert_eq!(location.error().code(), "28");
}

#[test]
fn e032_a_duplicate_reference_is_26_on_its_header() {
    let error = EdifactError::DuplicateReference {
        tag: "UNH".into(),
        reference: "M1".into(),
        span: spanned(1),
    };
    assert_eq!(
        at(&error, None),
        (UCM, Some(0), None, Some("UNH"), None, None, "26", true)
    );

    let recovered = scan(GROUPED);
    let ung = EdifactError::DuplicateReference {
        tag: "UNG".into(),
        reference: "G1".into(),
        span: span(&recovered, 1),
    };
    let location = recovered.locate(&ung, None).expect("located");
    assert_eq!(
        (
            location.level(),
            location.group(),
            location.service_segment()
        ),
        (UCF, Some(0), Some("UNG"))
    );
    assert_eq!(location.error().code(), "26");
}

#[test]
fn e042_an_empty_interchange_is_32_on_the_uci() {
    let error = EdifactError::EmptyInterchange {
        control_ref: "IC1".into(),
    };
    assert_eq!(
        at(&error, None),
        (UCI, None, None, None, None, None, "32", true)
    );
}

#[test]
fn e043_an_empty_message_is_13_on_its_ucm_never_32() {
    let error = EdifactError::EmptyMessage {
        message_ref: "M1".into(),
        span: spanned(1),
    };
    assert_eq!(
        at(&error, None),
        (UCM, Some(0), None, Some("UNH"), None, None, "13", true)
    );
}

#[test]
fn e044_a_package_is_47_on_the_message() {
    let error = EdifactError::PackageNotSupported {
        tag: "UNO".into(),
        span: spanned(3),
    };
    assert_eq!(
        at(&error, None),
        (UCM, Some(0), None, None, None, None, "47", true)
    );
}

#[test]
fn e052_groups_and_messages_mixed_is_30_on_the_header() {
    let error = EdifactError::GroupsAndMessagesMixed { span: spanned(1) };
    assert_eq!(
        at(&error, None),
        (UCM, Some(0), None, Some("UNH"), None, None, "30", true)
    );
}

// ── interchange-wide errors ──────────────────────────────────────────────────

#[test]
fn e023_e036_size_errors_are_18_on_the_uci() {
    let too_large = EdifactError::InterchangeTooLarge { count: u64::MAX };
    assert_eq!(
        at(&too_large, None),
        (UCI, None, None, None, None, None, "18", true)
    );
    // `LimitExceeded` is `#[non_exhaustive]`: obtain one from the parser.
    let limit = edifact_rs::from_bytes_with_config(
        b"BGM+220'DTM+137'",
        edifact_rs::ReaderConfig::default().max_segments(1),
    )
    .find_map(Result::err)
    .expect("one segment over the budget");
    assert_eq!(
        at(&limit, None),
        (UCI, None, None, None, None, None, "18", true)
    );
}

#[test]
fn e031_e039_repertoire_errors_are_46_on_unb_s001() {
    let unrecognised = EdifactError::UnrecognisedSyntaxIdentifier("UNOQ".into());
    assert_eq!(
        at(&unrecognised, None),
        (UCI, None, None, Some("UNB"), Some(2), Some(1), "46", true)
    );
    let unsupported = EdifactError::UnsupportedCharset {
        syntax_identifier: "UNOX".into(),
    };
    assert_eq!(
        at(&unsupported, None),
        (UCI, None, None, Some("UNB"), Some(2), Some(1), "46", true)
    );
}

#[test]
fn e038_a_character_outside_the_repertoire_is_21_on_the_uci() {
    let error = EdifactError::CharacterNotInRepertoire {
        charset: "UNOA",
        character: 'ä',
        offset: 3,
    };
    assert_eq!(
        at(&error, None),
        (UCI, None, None, None, None, None, "21", false)
    );
}

// ── span-less errors, placed by tag ──────────────────────────────────────────

#[test]
fn e008_a_missing_element_is_13_on_the_first_segment_with_its_tag() {
    let error = EdifactError::MissingRequiredElement {
        tag: "BGM".into(),
        element_index: 1,
    };
    assert_eq!(
        at(&error, Some(0)),
        (UCD, Some(0), Some(2), None, Some(3), None, "13", false)
    );
    // Without the message it was raised for, only the interchange is known.
    assert_eq!(
        at(&error, None),
        (UCI, None, None, None, None, None, "13", false)
    );

    // A service segment's element is S011 on the UCM.
    let in_unh = EdifactError::MissingRequiredElement {
        tag: "UNH".into(),
        element_index: 1,
    };
    assert_eq!(
        at(&in_unh, Some(0)),
        (UCM, Some(0), None, Some("UNH"), Some(3), None, "13", false)
    );
}

#[test]
fn e015_a_missing_segment_is_13_on_the_message() {
    let error = EdifactError::MissingSegment {
        tag: "BGM".into(),
        expected_position: "after UNH".into(),
    };
    assert_eq!(
        at(&error, Some(0)),
        (UCM, Some(0), None, None, None, None, "13", false)
    );
    let unz = EdifactError::MissingSegment {
        tag: "UNZ".into(),
        expected_position: "last".into(),
    };
    assert_eq!(
        at(&unz, None),
        (UCI, None, None, Some("UNZ"), None, None, "13", false)
    );
}

#[test]
fn e021_a_missing_component_is_13_at_its_component() {
    let error = EdifactError::MissingRequiredComponent {
        tag: "DTM".into(),
        element_index: 0,
        component_index: 2,
    };
    assert_eq!(
        at(&error, Some(0)),
        (UCD, Some(0), Some(3), None, Some(2), Some(3), "13", false)
    );
}

#[test]
fn e027_an_invalid_field_value_is_12_at_its_element() {
    let error = EdifactError::InvalidFieldValue {
        tag: "DTM".into(),
        element_index: 0,
        value: "x".into(),
    };
    assert_eq!(
        at(&error, Some(0)),
        (UCD, Some(0), Some(3), None, Some(2), None, "12", false)
    );
}

// ── validation errors with a span ────────────────────────────────────────────

#[test]
fn e011_a_segment_out_of_place_is_15_on_the_ucs() {
    let error = EdifactError::InvalidSegmentForMessage {
        tag: "DTM".into(),
        message_type: "ORDERS".into(),
        span: spanned(3),
    };
    assert_eq!(
        at(&error, None),
        (UCS, Some(0), Some(3), None, None, None, "15", true)
    );
}

#[test]
fn e012_too_many_elements_is_16_on_the_ucs() {
    let error = EdifactError::InvalidElementCount {
        tag: "BGM".into(),
        min: 1,
        max: 2,
        actual: 3,
        span: spanned(2),
    };
    assert_eq!(
        at(&error, None),
        (UCS, Some(0), Some(2), None, None, None, "16", true)
    );
}

#[test]
fn e013_too_many_components_is_16_on_the_ucd() {
    let error = EdifactError::InvalidComponentCount {
        tag: "DTM".into(),
        element_index: 0,
        expected: 2,
        actual: 3,
        span: spanned(3),
    };
    assert_eq!(
        at(&error, None),
        (UCD, Some(0), Some(3), None, Some(2), None, "16", true)
    );
}

#[test]
fn e014_an_invalid_code_is_12_on_the_ucd() {
    let error = EdifactError::InvalidCodeValue {
        tag: "BGM".into(),
        element_index: 0,
        value: "999".into(),
        code_list: "1001".into(),
        span: spanned(2),
        suggestion: None,
    };
    assert_eq!(
        at(&error, None),
        (UCD, Some(0), Some(2), None, Some(2), None, "12", true)
    );
}

#[test]
fn e017_a_conditional_requirement_is_15_on_the_ucd() {
    let error = EdifactError::ConditionalRequirementNotMet {
        tag: "BGM".into(),
        element_index: 1,
        condition: "D1".into(),
        span: spanned(2),
    };
    assert_eq!(
        at(&error, None),
        (UCD, Some(0), Some(2), None, Some(3), None, "15", true)
    );
}

#[test]
fn e045_a_blank_value_is_12_on_the_ucd_with_its_component() {
    let error = EdifactError::BlankDataElementValue {
        tag: "BGM".into(),
        element_index: 1,
        component_index: 0,
        span: spanned(2),
    };
    assert_eq!(
        at(&error, None),
        (UCD, Some(0), Some(2), None, Some(3), Some(1), "12", true)
    );
}

#[test]
fn e046_a_segment_without_data_is_13_on_the_ucs() {
    let error = EdifactError::SegmentWithoutDataElements {
        tag: "DTM".into(),
        span: spanned(3),
    };
    assert_eq!(
        at(&error, None),
        (UCS, Some(0), Some(3), None, None, None, "13", true)
    );
}

#[test]
fn e047_too_many_repetitions_is_35_on_the_ucd() {
    let error = EdifactError::TooManyRepetitions {
        tag: "BGM".into(),
        element_index: 0,
        max: 1,
        actual: 2,
        span: spanned(2),
    };
    assert_eq!(
        at(&error, None),
        (UCD, Some(0), Some(2), None, Some(2), None, "35", true)
    );
}

#[test]
fn e048_e049_e050_representation_errors_are_on_the_ucd() {
    let wrong_type = EdifactError::InvalidCharacterType {
        tag: "DTM".into(),
        element_index: 0,
        component_index: 1,
        repr: "n..35".into(),
        value: "x".into(),
        span: spanned(3),
    };
    assert_eq!(
        at(&wrong_type, None),
        (UCD, Some(0), Some(3), None, Some(2), Some(2), "37", true)
    );
    let too_long = EdifactError::DataElementTooLong {
        tag: "BGM".into(),
        element_index: 1,
        component_index: 0,
        repr: "an..2".into(),
        actual: 3,
        span: spanned(2),
    };
    assert_eq!(
        at(&too_long, None),
        (UCD, Some(0), Some(2), None, Some(3), Some(1), "39", true)
    );
    let too_short = EdifactError::DataElementTooShort {
        tag: "BGM".into(),
        element_index: 0,
        component_index: 0,
        repr: "an4".into(),
        actual: 3,
        span: spanned(2),
    };
    assert_eq!(
        at(&too_short, None),
        (UCD, Some(0), Some(2), None, Some(2), Some(1), "40", true)
    );
}

#[test]
fn e051_a_trailing_separator_is_45_on_the_ucs() {
    // Annex A does not permit 45 on UCD, so even a composite's trailing
    // separator is reported on the segment.
    for element_index in [None, Some(1)] {
        let error = EdifactError::TrailingSeparator {
            tag: "BGM".into(),
            element_index,
            span: spanned(2),
        };
        assert_eq!(
            at(&error, None),
            (UCS, Some(0), Some(2), None, None, None, "45", true)
        );
    }
}

#[test]
fn e053_insignificant_characters_are_12_on_the_ucd() {
    let error = EdifactError::InsignificantCharacters {
        tag: "BGM".into(),
        element_index: 1,
        component_index: 0,
        kind: Insignificant::TrailingSpaces,
        span: spanned(2),
    };
    assert_eq!(
        at(&error, None),
        (UCD, Some(0), Some(2), None, Some(3), Some(1), "12", true)
    );
}

// ── grouped interchanges ─────────────────────────────────────────────────────

#[test]
fn a_message_inside_a_group_carries_the_group_index() {
    let recovered = scan(GROUPED);
    let error = EdifactError::BlankDataElementValue {
        tag: "BGM".into(),
        element_index: 0,
        component_index: 0,
        span: span(&recovered, 3),
    };
    let location = recovered.locate(&error, None).expect("located");
    assert_invariants(&location);
    assert_eq!(location.group(), Some(0));
    assert_eq!(location.message(), Some(0));
    assert_eq!(location.segment_position(), Some(2));
}

#[test]
fn a_fault_in_ung_is_reported_on_the_ucf() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
UNG+ORDERS+SND+RCV+260101:0900+G\xFF1+UN+D:96A'\
UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNE+1+G1'UNZ+1+IC1'";
    let location = &scan(input).fault_locations()[0];
    assert_invariants(location);
    assert_eq!(
        (
            location.level(),
            location.group(),
            location.service_segment()
        ),
        (UCF, Some(0), Some("UNG"))
    );
    assert_eq!(
        (location.element_position(), location.error().code()),
        (Some(6), "21")
    );
}

// ── not a subject fault ──────────────────────────────────────────────────────

#[test]
fn errors_that_are_not_subject_faults_have_no_location() {
    let recovered = scan(IC);
    let errors = [
        EdifactError::InvalidUtf8,
        EdifactError::from(std::io::Error::other("io")),
        EdifactError::InvalidEventSequence { message: "x" },
        EdifactError::InvalidElementPosition,
        EdifactError::IncompatibleReleaseScopes {
            current: "a".into(),
            incoming: "b".into(),
        },
        EdifactError::ValidationErrors {
            error_count: 0,
            report: Box::new(ValidationReport::default()),
        },
        EdifactError::UnknownDataElement {
            tag: "BGM".into(),
            data_element: "9999".into(),
        },
        EdifactError::AmbiguousDataElement {
            tag: "BGM".into(),
            data_element: "1001".into(),
        },
        EdifactError::SegmentLayoutMismatch {
            expected: "NAD".into(),
            actual: "DTM".into(),
        },
        EdifactError::RepetitionSeparatorNotDeclared,
        EdifactError::NonFiniteNumber {
            value: "NaN".into(),
        },
        EdifactError::CharacterRepertoireMismatch {
            declared: "UNOA".into(),
            writer: "UNOC",
        },
        EdifactError::ActionNotPermitted {
            segment_tag: "UCI",
            action: "8",
        },
        EdifactError::InvalidContrlIdentifier {
            data_element: "0057",
            value: "TOOLONG".into(),
        },
        EdifactError::InvalidGroupSchema {
            group: "SG5".into(),
            reason: "max is 0",
        },
    ];
    let codes: Vec<_> = errors.iter().map(EdifactError::stable_code).collect();
    assert_eq!(
        codes,
        [
            "E009", "E010", "E024", "E025", "E026", "E030", "E033", "E034", "E035", "E037", "E040",
            "E041", "E066", "E067", "E071"
        ]
    );
    for error in &errors {
        assert_eq!(recovered.locate(error, Some(0)), None, "{error:?}");
    }
}

// ── property: every location satisfies the invariants (contract C5) ─────────

#[test]
fn every_location_from_fuzzed_input_satisfies_the_invariants() {
    // Corrupt a valid grouped and ungrouped interchange at fuzzed positions, so
    // faults land in every kind of segment rather than mostly in garbage.
    check!()
        .with_type::<(bool, Vec<(u16, u8)>)>()
        .cloned()
        .for_each(|(grouped, edits): (bool, Vec<(u16, u8)>)| {
            let mut input = if grouped {
                GROUPED.to_vec()
            } else {
                IC.to_vec()
            };
            for (at, byte) in edits.iter().take(8) {
                let at = usize::from(*at) % input.len();
                input[at] = *byte;
            }
            let recovered = scan(&input);
            for location in recovered.fault_locations() {
                assert_invariants(&location);
            }
            // Every fault, located again through `locate`, agrees.
            for fault in recovered.faults() {
                let location = recovered
                    .locate(fault.error(), None)
                    .expect("subject fault");
                assert_invariants(&location);
            }
            // A span-less error with a message hint, on whatever survived.
            for message in 0..recovered.messages().len() {
                for tag in ["BGM", "UNH", "UNT", "DTM"] {
                    let error = EdifactError::MissingRequiredComponent {
                        tag: tag.into(),
                        element_index: 0,
                        component_index: 1,
                    };
                    let location = recovered.locate(&error, Some(message)).expect("located");
                    assert_invariants(&location);
                    assert!(!location.is_exact());
                }
            }
        });
}

#[cfg(feature = "serde")]
#[test]
fn a_location_round_trips_through_serde() {
    let location = scan(IC)
        .locate(
            &EdifactError::MissingRequiredElement {
                tag: "UNH".into(),
                element_index: 1,
            },
            Some(0),
        )
        .expect("located");
    let json = serde_json::to_string(&location).expect("serialise");
    assert!(json.contains("\"UNH\""), "{json}");
    let back: ContrlLocation = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(back, location);
}
