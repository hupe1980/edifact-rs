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
