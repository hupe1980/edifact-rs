//! Pass case: segment with an optional composite element field (`#[edifact(composite)]` on `Option<Vec<String>>`).
#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    CompositeElement, Element, EdifactCompositeDeserialize, EdifactCompositeSerialize,
    EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag, EdifactSerialize,
    EventEmitter, OwnedSegment, Segment, composite_element, find_qualified_segment,
    find_qualified_segment_owned, find_segment, find_segment_owned,
};

extern crate self as edifact_rs;

use edifact_rs_derive::{
    EdifactDeserialize as DeriveEdifactDeserialize,
    EdifactSerialize as DeriveEdifactSerialize,
};

/// A segment where the composite element is optional.
#[derive(DeriveEdifactSerialize, DeriveEdifactDeserialize)]
#[edifact(segment = "DTM")]
struct DtmSegment {
    /// Date/time/period composite — present on most DTM segments but optional.
    #[edifact(element = 0, composite)]
    date_time: Option<Vec<String>>,
}

fn main() {
    let _ = std::any::type_name::<DtmSegment>();
}
