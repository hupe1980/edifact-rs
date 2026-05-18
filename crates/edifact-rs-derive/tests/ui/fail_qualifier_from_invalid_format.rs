#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, Segment, find_qualified_segment, find_segment,
};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "RFF", qualifier_from = "0")]
struct Reference {
    #[edifact(element = 0)]
    qualifier: String,
}

fn main() {}