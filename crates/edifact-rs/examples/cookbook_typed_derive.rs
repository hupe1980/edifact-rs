//! Cookbook: typed segment and message mapping with derive macros
//!
//! Shows the full `#[derive(EdifactDeserialize, EdifactSerialize)]` workflow:
//!
//! - Map individual segments to Rust structs with `#[edifact(segment = "TAG")]`
//! - Map elements to fields with `#[edifact(element = N)]`
//! - Route qualified segments (e.g. `NAD+BY`, `NAD+SU`) with `qualifier_from`
//!   on the struct and `#[edifact(qualifier = "Q")]` on message fields
//! - Serialise a typed struct back to wire format with `ser::to_edifact_string`
//!
//! Run:
//! ```text
//! cargo run -p edifact-rs --example cookbook_typed_derive
//! ```

use edifact_rs::{EdifactDeserialize, EdifactSerialize, from_bytes, ser};

// ── Segment structs ────────────────────────────────────────────────────────────

/// BGM — Beginning of Message.
/// `#[edifact(segment = "BGM")]` binds this struct to the BGM tag.
/// Fields are mapped by element index; `Option<T>` handles absent elements.
#[derive(Debug, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    doc_code: String,
    #[edifact(element = 1)]
    doc_id: String,
    #[edifact(element = 2)] // absent elements deserialise to `None`
    function: Option<String>,
}

/// NAD — Name and Address.
/// `qualifier_from = 0` tells the macro which element holds the party qualifier
/// ("BY" = buyer, "SU" = supplier, etc.).  At the message level, fields
/// annotated with `#[edifact(qualifier = "Q")]` are filled only when the
/// qualifier matches.
#[derive(Debug, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier_from = 0)]
struct Nad {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 1)]
    party_id: Option<String>,
}

// ── Message struct ─────────────────────────────────────────────────────────────

/// A minimal ORDERS message that collects one BGM and two qualified NADs.
///
/// Unrecognised or unmatched segments are silently skipped — the derive
/// implementation only advances through the segment slice, never backwards.
#[derive(Debug, EdifactDeserialize)]
struct MinimalOrderMessage {
    bgm: Option<Bgm>,
    #[edifact(qualifier = "BY")] // matches NAD+BY segments
    buyer: Option<Nad>,
    #[edifact(qualifier = "SU")] // matches NAD+SU segments
    supplier: Option<Nad>,
}

fn main() -> Result<(), edifact_rs::EdifactError> {
    let input = b"UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'NAD+BY+4000001000002::9'NAD+SU+4000001000001::9'UNT+5+1'";
    // Zero-copy parse: each `Segment<'_>` borrows from `input`.
    let segments: Vec<_> = from_bytes(input).collect::<Result<Vec<_>, _>>()?;

    // Deserialise the full message in one call.
    let message = MinimalOrderMessage::edifact_deserialize(&segments)?;
    println!("parsed={message:?}");

    if let Some(buyer) = &message.buyer {
        println!("buyer={}", buyer.party_id.as_deref().unwrap_or("unknown"));
    }
    if let Some(supplier) = &message.supplier {
        println!(
            "supplier={}",
            supplier.party_id.as_deref().unwrap_or("unknown")
        );
    }

    // Serialise a single typed segment back to wire format.
    // `ser::to_edifact_string` returns a `String` including the segment terminator.
    let bgm = message.bgm.as_ref().expect("BGM expected in sample");
    let encoded = ser::to_edifact_string(bgm)?;
    println!("bgm={encoded}"); // e.g. "BGM+220+PO-4711+9'"

    Ok(())
}
