#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, OwnedSegment, Segment,
    find_qualified_segment, find_segment, find_segment_owned,
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
    code: String,
}

#[derive(DeriveEdifactSerialize)]
struct Message {
    #[edifact(element = 0)]
    bgm: BgmSegment,
}

fn main() {}