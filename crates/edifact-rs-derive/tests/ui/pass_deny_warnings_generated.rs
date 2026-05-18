#![deny(warnings)]

#[path = "support.rs"]
#[allow(dead_code)]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, OwnedSegment, Segment, find_qualified_segment,
    find_qualified_segment_owned, find_segment, find_segment_owned,
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
    doc_name_code: String,
    #[edifact(element = 1)]
    doc_id: String,
    #[edifact(element = 2)]
    msg_function: Option<String>,
}

fn main() {
    let value = BgmSegment {
        doc_name_code: "220".to_owned(),
        doc_id: "PO-1".to_owned(),
        msg_function: Some("9".to_owned()),
    };
    let _ = std::hint::black_box(value);
}
