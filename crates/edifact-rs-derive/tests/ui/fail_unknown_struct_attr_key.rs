#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, Segment, find_qualified_segment, find_segment,
};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "BGM", unknown = "X")]
struct Message {
    #[edifact(element = 0)]
    doc_name_code: String,
}

fn main() {}
