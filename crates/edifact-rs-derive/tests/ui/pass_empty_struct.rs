#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    Element, EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag,
    EdifactSerialize, EventEmitter, Segment, find_qualified_segment, find_segment};

extern crate self as edifact_rs;
pub use support::edifact_rs::helpers;

use edifact_rs_derive::{EdifactDeserialize as DeriveEdifactDeserialize, EdifactSerialize as DeriveEdifactSerialize};

#[derive(DeriveEdifactSerialize, DeriveEdifactDeserialize)]
struct Empty {}

fn main() {
    let _ = std::any::type_name::<Empty>();
}