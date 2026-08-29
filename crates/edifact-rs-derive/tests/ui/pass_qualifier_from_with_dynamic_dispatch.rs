/// Trybuild pass: `qualifier_from = N` on a segment struct used in a message
/// must compile cleanly.  The qualifier element index can be any valid u32.
#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, Segment, find_qualified_segment, find_segment};

extern crate self as edifact_rs;
pub use support::edifact_rs::helpers;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

/// Qualifier is taken from element 0, component 0 at runtime.
#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "RFF", qualifier_from = 0)]
struct RffAny {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 0, component = 1)]
    value: Option<String>}

/// Qualifier is taken from element 1 (non-zero index).
#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "NAD", qualifier_from = 0)]
struct NadAny {
    #[edifact(element = 0)]
    party_qualifier: String,
    #[edifact(element = 1)]
    party_id: Option<String>}

/// A message struct that maps dynamically-qualified segments.
#[derive(DeriveEdifactDeserialize)]
struct Message {
    #[edifact(qualifier = "ON")]
    order_reference: Option<RffAny>,
    #[edifact(qualifier = "BY")]
    buyer: Option<NadAny>}

fn main() {
    let _ = std::any::type_name::<Message>();
}
