#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, OwnedSegment, Segment, find_qualified_segment,
    find_qualified_segment_owned, find_segment, find_segment_owned,
};

extern crate self as edifact_rs;

use edifact_rs_derive::{EdifactDeserialize as DeriveEdifactDeserialize, EdifactSerialize as DeriveEdifactSerialize};

#[derive(DeriveEdifactSerialize, DeriveEdifactDeserialize)]
struct Empty {}

fn main() {
    let _ = std::any::type_name::<Empty>();
}