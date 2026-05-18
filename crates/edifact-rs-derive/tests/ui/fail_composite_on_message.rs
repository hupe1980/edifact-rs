#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactCompositeDeserialize, EdifactCompositeSerialize, EdifactDeserialize,
    EdifactError, EdifactEvent, EdifactSegmentTag, EdifactSerialize, EventEmitter, Segment,
    composite_element, find_qualified_segment, find_segment,
};

extern crate self as edifact_rs;

use edifact_rs_derive::{
    EdifactDeserialize as DeriveEdifactDeserialize,
    EdifactSerialize as DeriveEdifactSerialize,
};

#[derive(DeriveEdifactSerialize, DeriveEdifactDeserialize)]
#[edifact(segment = "RFF")]
struct RffSegment {
    #[edifact(element = 0, composite)]
    reference: Vec<String>,
}

#[derive(DeriveEdifactSerialize)]
struct Message {
    #[edifact(composite)]
    rff: RffSegment,
}

fn main() {}