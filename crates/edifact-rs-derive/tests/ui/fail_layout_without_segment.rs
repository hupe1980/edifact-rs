#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag, EdifactSerialize, Element,
    EventEmitter, OwnedSegment, Segment, find_qualified_segment, find_segment, find_segment_owned,
};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

// `layout` describes one segment, so it needs `segment` to say which.
#[derive(DeriveEdifactDeserialize)]
#[edifact(layout = SOME_LAYOUT)]
struct Message {
    doc_name_code: String,
}

fn main() {}
