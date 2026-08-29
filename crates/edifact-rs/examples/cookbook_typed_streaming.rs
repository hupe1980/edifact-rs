//! Cookbook: typed segment streaming
//!
//! Two functions cover typed segment extraction, one per input shape:
//!
//! | API | Source |
//! |---|---|
//! | `deserialize_each` | `&[u8]` |
//! | `deserialize_each_from_reader` | `impl Read` |
//!
//! Both are **lazy iterators**, so "first match" and "all matches" are just
//! `.next()` and `.collect()` — there is no separate entry point for each.
//! Non-matching segments are skipped without being buffered, so peak memory is
//! one segment regardless of interchange size.
//!
//! For message-window streaming (`UNH..UNT`), see
//! `cookbook_message_window_streaming.rs`.
//!
//! Run:
//! ```text
//! cargo run -p edifact-rs --example cookbook_typed_streaming
//! ```

use edifact_rs::{EdifactDeserialize, deserialize_each, deserialize_each_from_reader};

/// BGM — Beginning of Message.
/// The derive macro scans for segments with tag "BGM" and maps element 0/1/2.
#[derive(Debug, PartialEq, EdifactDeserialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    doc_code: String,
    #[edifact(element = 1)]
    doc_id: String,
    #[edifact(element = 2)] // absent → `None`, present → `Some`
    function: Option<String>,
}

fn main() -> Result<(), edifact_rs::EdifactError> {
    // The input holds two BGM segments separated by a non-BGM segment.
    let input =
        b"UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'RFF+ON:PO-4711'BGM+231+PO-4712+9'UNT+5+1'";

    // ── From a byte slice ─────────────────────────────────────────────────────
    // `.next()` stops scanning at the first match — nothing past it is parsed.
    let first: Bgm = deserialize_each(input)
        .next()
        .transpose()?
        .expect("input carries a BGM");
    assert_eq!(first.doc_id, "PO-4711");

    // `.collect()` drains the iterator and yields every match.
    let all: Vec<Bgm> = deserialize_each(input).collect::<Result<_, _>>()?;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].doc_id, "PO-4711");
    assert_eq!(all[1].doc_id, "PO-4712");

    // ── From any `impl Read` ──────────────────────────────────────────────────
    // Identical semantics; reach for this when the payload comes from a file or
    // a socket rather than memory.
    let all_reader: Vec<Bgm> = deserialize_each_from_reader(std::io::Cursor::new(input.to_vec()))
        .collect::<Result<_, _>>()?;
    assert_eq!(all_reader, all);

    println!("first={first:?}");
    println!("all={all_reader:?}");

    Ok(())
}
