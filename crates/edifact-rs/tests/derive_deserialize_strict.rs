#![cfg(feature = "derive")]

use edifact_rs::{EdifactDeserialize, EdifactError, from_bytes};

#[allow(dead_code)]
#[derive(Debug, EdifactDeserialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    document_name_code: String,
    #[edifact(element = 1)]
    document_number: String,
}

#[derive(Debug, EdifactDeserialize)]
struct MinimalMessage {
    bgm: Option<Bgm>,
}

#[test]
fn optional_message_field_is_none_when_segment_absent() {
    let input = b"UNH+1+ORDERS:D:11A:UN'UNT+2+1'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let message = MinimalMessage::edifact_deserialize(&segments).unwrap();
    assert!(message.bgm.is_none());
}

#[test]
fn optional_message_field_does_not_swallow_segment_errors() {
    let input = b"UNH+1+ORDERS:D:11A:UN'BGM+220'UNT+3+1'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let result = MinimalMessage::edifact_deserialize(&segments);
    assert!(matches!(
        result,
        Err(EdifactError::MissingRequiredElement {
            tag,
            element_index: 1,
        }) if tag == "BGM"
    ));
}

#[test]
fn non_optional_segment_field_requires_non_empty_value() {
    let input = b"BGM+220+'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let result = Bgm::edifact_deserialize(&segments);
    assert!(matches!(
        result,
        Err(EdifactError::MissingRequiredElement {
            tag,
            element_index: 1,
        }) if tag == "BGM"
    ));
}

// ── #[edifact(required)] on Option<T> ────────────────────────────────────────

/// Segment where element 2 is `Option<String>` but annotated `required`.
/// Element 1 is a plain optional without the attribute.
#[derive(Debug, EdifactDeserialize)]
#[edifact(segment = "TST")]
struct TstRequired {
    #[edifact(element = 0)]
    first: String,
    #[edifact(element = 1)]
    optional_second: Option<String>,
    #[edifact(element = 2, required)]
    mandatory_opt: Option<String>,
}

#[test]
fn required_option_succeeds_when_element_present() {
    // All three elements present.
    let input = b"TST+A+B+C'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let tst = TstRequired::edifact_deserialize(&segments).unwrap();
    assert_eq!(tst.first, "A");
    assert_eq!(tst.optional_second, Some("B".to_owned()));
    assert_eq!(tst.mandatory_opt, Some("C".to_owned()));
}

#[test]
fn required_option_fails_when_element_absent() {
    // Only two elements — element 2 missing.
    let input = b"TST+A+B'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let err = TstRequired::edifact_deserialize(&segments)
        .expect_err("expected Err when required element is absent");
    assert!(
        matches!(
            err,
            EdifactError::MissingRequiredElement { ref tag, element_index: 2 } if tag == "TST"
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn required_option_fails_when_element_empty_string() {
    // Element 2 is explicitly empty.
    let input = b"TST+A+B+'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let err = TstRequired::edifact_deserialize(&segments)
        .expect_err("expected Err when required element is empty string");
    assert!(
        matches!(
            err,
            EdifactError::MissingRequiredElement { ref tag, element_index: 2 } if tag == "TST"
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn unrequired_option_still_returns_none_when_absent() {
    // element 1 (optional_second) absent, element 2 present.
    let input = b"TST+A++C'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let tst = TstRequired::edifact_deserialize(&segments).unwrap();
    assert_eq!(tst.optional_second, None);
    assert_eq!(tst.mandatory_opt, Some("C".to_owned()));
}

// ── #[edifact(required)] combined with #[edifact(component = N)] ─────────────

/// Segment where a specific component within a composite element is required.
/// Absence must surface as `MissingRequiredComponent` (E021), not the
/// coarser `MissingRequiredElement` (E008).
#[derive(Debug, EdifactDeserialize)]
#[edifact(segment = "PIA")]
struct PiaSegment {
    #[edifact(element = 0)]
    item_number_type: String,
    /// Component 0 of element 1 is mandatory; generates `MissingRequiredComponent`.
    #[edifact(element = 1, component = 0, required)]
    item_identifier: Option<String>,
    /// Component 1 of element 1 is optional; generates `None` when absent.
    #[edifact(element = 1, component = 1)]
    item_scheme: Option<String>,
}

#[test]
fn required_component_succeeds_when_component_present() {
    // PIA+1+ITEM1:IN'  → element 0 = "1", element 1 component 0 = "ITEM1", component 1 = "IN"
    let input = b"PIA+1+ITEM1:IN'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let pia = PiaSegment::edifact_deserialize(&segments).unwrap();
    assert_eq!(pia.item_number_type, "1");
    assert_eq!(pia.item_identifier, Some("ITEM1".to_owned()));
    assert_eq!(pia.item_scheme, Some("IN".to_owned()));
}

#[test]
fn required_component_fails_with_missing_required_component_when_element_absent() {
    // Element 1 entirely absent — component 0 of element 1 is required.
    let input = b"PIA+1'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let err = PiaSegment::edifact_deserialize(&segments)
        .expect_err("expected Err when required component element is absent");
    assert!(
        matches!(
            err,
            EdifactError::MissingRequiredComponent {
                ref tag,
                element_index: 1,
                component_index: 0,
            } if tag == "PIA"
        ),
        "expected MissingRequiredComponent, got: {err:?}"
    );
}

#[test]
fn required_component_fails_with_missing_required_component_when_component_empty() {
    // Element 1 present but component 0 is empty: PIA+1+:IN'
    let input = b"PIA+1+:IN'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let err = PiaSegment::edifact_deserialize(&segments)
        .expect_err("expected Err when required component is empty string");
    assert!(
        matches!(
            err,
            EdifactError::MissingRequiredComponent {
                ref tag,
                element_index: 1,
                component_index: 0,
            } if tag == "PIA"
        ),
        "expected MissingRequiredComponent, got: {err:?}"
    );
}

#[test]
fn optional_component_returns_none_when_absent() {
    // Component 1 absent — item_scheme should be None.
    let input = b"PIA+1+ITEM1'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();

    let pia = PiaSegment::edifact_deserialize(&segments).unwrap();
    assert_eq!(pia.item_identifier, Some("ITEM1".to_owned()));
    assert_eq!(pia.item_scheme, None);
}
