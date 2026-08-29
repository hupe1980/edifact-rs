#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag, EdifactSerialize, Element,
    EventEmitter, Segment, find_qualified_segment, find_segment};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

// Identifiers belong in `element`, which resolves both coordinates.
#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "NAD")]
struct Message {
    #[edifact(element = 1, component = "3055")]
    agency: String}

fn main() {}
