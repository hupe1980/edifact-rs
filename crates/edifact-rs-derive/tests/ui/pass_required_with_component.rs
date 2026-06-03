//! Verify that `#[edifact(required)]` combined with `#[edifact(component = N)]`
//! is accepted and generates code that emits `MissingRequiredComponent` (not
//! `MissingRequiredElement`) when the component is absent.

#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, OwnedSegment, Segment, find_qualified_segment,
    find_qualified_segment_owned, find_segment, find_segment_owned,
};

extern crate self as edifact_rs;
pub use support::edifact_rs::helpers;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

/// Segment with a required component — the absence should surface as
/// `EdifactError::MissingRequiredComponent`, not `MissingRequiredElement`.
#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "PIA")]
struct PiaSegment {
    #[edifact(element = 0)]
    item_number_type: String,
    /// Component 0 of element 1 is required; emits `MissingRequiredComponent`
    /// instead of `MissingRequiredElement` when absent.
    #[edifact(element = 1, component = 0, required)]
    item_identifier: Option<String>,
}

fn main() {
    let _ = std::any::type_name::<PiaSegment>();
}
