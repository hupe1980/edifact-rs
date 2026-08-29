#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{EdifactError, EdifactEvent, EdifactSerialize, EventEmitter};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactSerialize as DeriveEdifactSerialize;

// element index far beyond the UN/EDIFACT maximum — must be rejected at compile
// time rather than making rustc emit one statement per slot.
#[derive(DeriveEdifactSerialize)]
#[edifact(segment = "BGM")]
struct Bad {
    #[edifact(element = 200000)]
    value: String}

fn main() {}
