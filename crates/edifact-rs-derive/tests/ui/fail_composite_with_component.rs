#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactCompositeDeserialize, EdifactCompositeSerialize, EdifactDeserialize,
    EdifactError, EdifactEvent, EdifactSegmentTag, EdifactSerialize, EventEmitter, Segment,
    composite_element, find_qualified_segment, find_segment,
};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactSerialize as DeriveEdifactSerialize;

#[derive(DeriveEdifactSerialize)]
#[edifact(segment = "RFF")]
struct RffSegment {
    #[edifact(element = 0, component = 1, composite)]
    reference: Vec<String>,
}

fn main() {}