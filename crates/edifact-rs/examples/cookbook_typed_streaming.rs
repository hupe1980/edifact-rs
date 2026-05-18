//! Cookbook: typed segment streaming
//!
//! Demonstrates all four typed extraction APIs:
//!
//! | API | Source | Stops after first? |
//! |---|---|---|
//! | `deserialize_first_streaming` | `&[u8]` | yes |
//! | `deserialize_all_streaming` | `&[u8]` | no |
//! | `deserialize_first_from_reader` | `impl Read` | yes |
//! | `deserialize_all_from_reader` | `impl Read` | no |
//!
//! All four APIs scan the segment stream and collect segments that match
//! the target type's `#[edifact(segment = "TAG")]` attribute.  Non-matching
//! segments are skipped without allocation.
//!
//! For message-window streaming (`UNH..UNT`), see
//! `cookbook_message_window_streaming.rs`.
//!
//! Run:
//! ```text
//! cargo run -p edifact-rs --example cookbook_typed_streaming
//! ```

use edifact_rs::{
    EdifactDeserialize, deserialize_all_from_reader, deserialize_all_streaming,
    deserialize_first_from_reader, deserialize_first_streaming,
};

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
    // The input contains two BGM segments separated by a non-BGM segment.
    // All four APIs skip non-matching segments without buffering them.
    let input =
        b"UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'RFF+ON:PO-4711'BGM+231+PO-4712+9'UNT+5+1'";

    // ── Bytes-based APIs ──────────────────────────────────────────────────────
    // `deserialize_first_streaming` stops scanning after the first BGM match;
    // memory usage is O(1) regardless of the interchange size.
    let first: Bgm = deserialize_first_streaming(input)?;
    assert_eq!(first.doc_id, "PO-4711");

    // `deserialize_all_streaming` collects all matching BGM segments.
    let all: Vec<Bgm> = deserialize_all_streaming(input)?;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].doc_id, "PO-4711");
    assert_eq!(all[1].doc_id, "PO-4712");

    // ── Reader-based APIs ─────────────────────────────────────────────────────
    // Identical semantics to the bytes variants, but accept any `impl Read`.
    // Use these when the payload comes from a file or network stream.
    let first_reader: Bgm = deserialize_first_from_reader(std::io::Cursor::new(input.to_vec()))?;
    assert_eq!(first_reader.doc_id, "PO-4711");

    let all_reader: Vec<Bgm> = deserialize_all_from_reader(std::io::Cursor::new(input.to_vec()))?;
    assert_eq!(all_reader.len(), 2);

    println!("first={:?}", first_reader);
    println!("all={:?}", all_reader);

    Ok(())
}
