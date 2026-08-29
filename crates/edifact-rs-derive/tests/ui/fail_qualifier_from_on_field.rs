/// Trybuild fail: `qualifier_from` combined with `composite` on a field is
/// invalid (qualifier_from is a segment-level attribute, not a field-level one).
///
/// This test verifies that an attempt to use `qualifier_from` in a field-level
/// position produces a clear compile error.
#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, Segment, find_qualified_segment,
    find_segment};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

/// `qualifier_from` must not appear on a field attribute — only on the struct.
#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "RFF")]
struct Reference {
    #[edifact(element = 0, qualifier_from = 1)]
    qualifier: String}

fn main() {}
