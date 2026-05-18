//! Pass case: message struct with an optional group field (`#[edifact(group)]`).
#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, OwnedSegment, Segment, find_qualified_segment,
    find_qualified_segment_owned, find_segment, find_segment_owned, find_segments_typed,
};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "LIN")]
struct LinSegment {
    #[edifact(element = 0)]
    line_number: String,
}

/// Message with an optional repeating group and an optional scalar segment.
#[derive(DeriveEdifactDeserialize)]
struct OrdersMessage {
    #[edifact(group)]
    lines: Vec<LinSegment>,
    last_line: Option<LinSegment>,
}

fn main() {
    let _ = std::any::type_name::<OrdersMessage>();
}
