#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{EdifactError, EdifactEvent, EdifactSerialize, EventEmitter};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactSerialize as DeriveEdifactSerialize;

// A repeated key silently took the last value; it must be an error.
#[derive(DeriveEdifactSerialize)]
#[edifact(segment = "BGM", segment = "RFF")]
struct Bad {
    #[edifact(element = 0)]
    value: String}

fn main() {}
