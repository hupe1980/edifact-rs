#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag, EdifactSerialize, Element,
    EventEmitter, OwnedSegment, Segment, find_qualified_segment, find_segment, find_segment_owned,
};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

// A data element identifier has nothing to resolve against without a layout.
#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "NAD")]
struct Message {
    #[edifact(element = "3035")]
    qualifier: String,
}

fn main() {}
