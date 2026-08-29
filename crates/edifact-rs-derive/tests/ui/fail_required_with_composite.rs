//! Fail case: `#[edifact(required)]` cannot be combined with `#[edifact(composite)]`.
//!
//! The `required` attribute is only enforced in the scalar element/component
//! deserialization path; composite fields do not implement its semantics.  The
//! macro must reject this combination at compile time.
#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    CompositeElement, EdifactCompositeDeserialize, EdifactCompositeSerialize, EdifactDeserialize,
    EdifactError, EdifactEvent, EdifactSegmentTag, EdifactSerialize, EventEmitter,
    Segment, composite_element, find_qualified_segment,
    find_segment};

extern crate self as edifact_rs;
pub use support::edifact_rs::helpers;

use edifact_rs_derive::EdifactDeserialize as DeriveEdifactDeserialize;

#[derive(DeriveEdifactDeserialize)]
#[edifact(segment = "DTM")]
struct DtmSegment {
    #[edifact(element = 0, composite, required)]
    date_time: Option<Vec<String>>}

fn main() {}
