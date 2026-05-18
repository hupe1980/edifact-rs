#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, OwnedSegment, Segment, find_qualified_segment,
    find_qualified_segment_owned, find_segment, find_segment_owned,
};

extern crate self as edifact_rs;

use edifact_rs_derive::{
    EdifactDeserialize as DeriveEdifactDeserialize,
    EdifactSerialize as DeriveEdifactSerialize,
};

#[derive(DeriveEdifactSerialize, DeriveEdifactDeserialize)]
#[edifact(segment = "BGM")]
struct BgmSegment {
    #[edifact(element = 0)]
    doc_name_code: String,
    #[edifact(element = 1)]
    doc_id: String,
    #[edifact(element = 2)]
    msg_function: Option<String>,
}

fn main() {
    let _ = std::any::type_name::<BgmSegment>();
}