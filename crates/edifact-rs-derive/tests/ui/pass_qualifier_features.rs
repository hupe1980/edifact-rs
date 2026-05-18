#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, OwnedSegment, Segment, find_qualified_segment,
    find_qualified_segment_owned, find_segment, find_segment_owned,
};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "NAD")]
struct NadSegment {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 1)]
    party_id: Option<String>,
}

#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "RFF", qualifier_from = 0)]
struct RffSegment {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 1)]
    value: Option<String>,
}

#[derive(DeriveEdifactDeserialize)]
struct Message {
    #[edifact(qualifier = "MS")]
    market_sender: Option<NadSegment>,
    #[edifact(qualifier = "MR")]
    market_receiver: Option<NadSegment>,
    reference: Option<RffSegment>,
}

fn main() {
    let _ = std::any::type_name::<Message>();
}
