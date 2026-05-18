#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, Segment, find_qualified_segment, find_segment,
};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactSerialize as DeriveEdifactSerialize;

#[derive(DeriveEdifactSerialize)]
#[edifact(segment = "RFF", qualifier = "AGI")]
struct RffSegment {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 1)]
    reference: String,
}

fn main() {}
