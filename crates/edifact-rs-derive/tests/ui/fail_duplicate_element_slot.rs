#[path = "support.rs"]
mod support;

pub use support::edifact_rs::{EdifactError, EdifactEvent, EdifactSerialize, EventEmitter};

extern crate self as edifact_rs;

use edifact_rs_derive::EdifactSerialize as DeriveEdifactSerialize;

// Two fields claiming the same slot would silently drop one on serialize.
#[derive(DeriveEdifactSerialize)]
#[edifact(segment = "BGM")]
struct Bad {
    #[edifact(element = 1)]
    first: String,
    #[edifact(element = 1)]
    second: String,
}

fn main() {}
