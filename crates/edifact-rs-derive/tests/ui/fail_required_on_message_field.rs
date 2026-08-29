//! Fail case: `#[edifact(required)]` is not supported on message struct fields.
//!
//! The `required` attribute applies only to segment struct element fields.  On a
//! message struct the attribute would be silently ignored, so the macro rejects
//! it at compile time with a clear diagnostic.
#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{
    EdifactDeserialize, EdifactError, EdifactEvent, EdifactSegmentTag, EdifactSerialize,
    EventEmitter, Segment, find_qualified_segment,
    find_segment};

extern crate self as edifact_rs;
pub use support::edifact_rs::helpers;

use edifact_rs_derive::{
    EdifactDeserialize as DeriveEdifactDeserialize,
    EdifactSerialize as DeriveEdifactSerialize};

#[derive(DeriveEdifactSerialize, DeriveEdifactDeserialize)]
#[edifact(segment = "BGM")]
struct BgmSegment {
    #[edifact(element = 0)]
    doc_id: String}

/// Message struct — `#[edifact(required)]` on an `Option` field here should be
/// rejected because the semantics are unimplemented and the attribute would be
/// silently ignored.
#[derive(DeriveEdifactDeserialize)]
struct OrdersMessage {
    #[edifact(required)]
    bgm: Option<BgmSegment>}

fn main() {}
