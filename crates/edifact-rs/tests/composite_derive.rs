//! `#[derive(EdifactCompositeDeserialize)]` / `#[derive(EdifactCompositeSerialize)]`.
//!
//! `#[edifact(element = N, composite)]` has always required the field type to
//! implement the composite serde traits, and the guides showed a derive for
//! them — but only the hand-written `Vec<String>` impl existed, so every example
//! in the docs failed to compile. These tests pin the derives that close that gap.

#![cfg(feature = "derive")]

use edifact_rs::{
    CompositeElement, EdifactCompositeDeserialize, EdifactCompositeSerialize, EdifactDeserialize,
    EdifactError, EdifactSerialize, from_bytes, to_edifact_string,
};

#[derive(Debug, PartialEq, EdifactCompositeDeserialize, EdifactCompositeSerialize)]
struct PartyId {
    id: String,
    code_list: Option<String>,
    agency: Option<String>,
}

#[derive(Debug, PartialEq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD")]
struct Nad {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 1, composite)]
    party: Option<PartyId>,
}

fn parse_nad(input: &[u8]) -> Nad {
    let segments: Vec<_> = from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");
    Nad::edifact_deserialize(&segments).expect("deserialize")
}

#[test]
fn a_composite_deserializes_field_by_component() {
    let nad = parse_nad(b"NAD+BY+4000001000002::9'");
    assert_eq!(nad.qualifier, "BY");
    assert_eq!(
        nad.party,
        Some(PartyId {
            id: "4000001000002".into(),
            code_list: None, // the empty middle component reads as absent
            agency: Some("9".into()),
        })
    );
}

#[test]
fn a_composite_serializes_back_to_the_same_bytes() {
    let nad = Nad {
        qualifier: "BY".into(),
        party: Some(PartyId {
            id: "4000001000002".into(),
            code_list: None,
            agency: Some("9".into()),
        }),
    };
    assert_eq!(
        to_edifact_string(&nad).expect("serialize"),
        "NAD+BY+4000001000002::9'"
    );
}

#[test]
fn a_composite_round_trips() {
    let wire = b"NAD+SU+4000001000001:16:9'";
    let parsed = parse_nad(wire);
    let written = to_edifact_string(&parsed).expect("serialize");
    assert_eq!(written.as_bytes(), wire);
    assert_eq!(parse_nad(written.as_bytes()), parsed);
}

#[test]
fn a_trailing_optional_component_may_be_omitted_entirely() {
    let nad = parse_nad(b"NAD+BY+4000001000002'");
    let party = nad.party.expect("composite present");
    assert_eq!(party.id, "4000001000002");
    assert_eq!(party.code_list, None);
    assert_eq!(party.agency, None);
}

#[test]
fn a_missing_required_component_is_reported_as_such() {
    // `id` is a bare `String`, so an empty first component is a hard error
    // rather than a silently empty struct field.
    let composite = CompositeElement::from_slice(&[
        std::borrow::Cow::Borrowed(""),
        std::borrow::Cow::Borrowed("9"),
    ]);
    let err = PartyId::edifact_deserialize_composite(composite)
        .expect_err("an empty mandatory component must fail");
    assert!(
        matches!(
            err,
            EdifactError::MissingRequiredComponent {
                component_index: 0,
                ..
            }
        ),
        "unexpected error: {err:?}"
    );
}

// ── explicit component indices ────────────────────────────────────────────────

#[derive(Debug, PartialEq, EdifactCompositeDeserialize, EdifactCompositeSerialize)]
struct DateTime {
    #[edifact(component = 1)]
    value: String,
    #[edifact(component = 0)]
    qualifier: String,
    #[edifact(component = 2)]
    format: Option<String>,
}

#[derive(Debug, PartialEq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "DTM")]
struct Dtm {
    #[edifact(element = 0, composite)]
    c507: Option<DateTime>,
}

#[test]
fn component_indices_override_declaration_order() {
    let segments: Vec<_> = from_bytes(b"DTM+137:20260101:102'")
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");
    let dtm = Dtm::edifact_deserialize(&segments).expect("deserialize");
    let c507 = dtm.c507.as_ref().expect("composite present");

    // Declaration order is value, qualifier, format — the attributes decide.
    assert_eq!(c507.qualifier, "137");
    assert_eq!(c507.value, "20260101");
    assert_eq!(c507.format.as_deref(), Some("102"));

    // …and the writer puts them back where they belong.
    assert_eq!(
        to_edifact_string(&dtm).expect("serialize"),
        "DTM+137:20260101:102'"
    );
}
