//! Cookbook: message-window streaming
//!
//! Demonstrates the [`MessageWindowsIter`] API for iterating over
//! `UNH..UNT` message windows without buffering the entire interchange.
//!
//! # Key APIs
//!
//! - [`message_windows_bytes`] — byte-slice source, produces owned windows
//! - [`message_windows_from_reader`] — reader source, lazy per-window I/O
//! - [`deserialize_messages_from_reader`] — reader source + typed deserialisation
//!
//! All three variants skip envelope segments (`UNB`, `UNZ`, `UNG`, `UNE`)
//! automatically and yield only the `UNH..UNT` windows.

use edifact_rs::{
    EdifactDeserialize, EdifactSerialize, OwnedSegment, Segment, message_windows_bytes,
    message_windows_from_reader,
};

// ── 1. Manual approach: inspect windows as raw segment slices ─────────────────

fn count_messages_in_interchange(input: &[u8]) -> usize {
    message_windows_bytes(input)
        .filter_map(Result::ok)
        .count()
}

// ── 2. Typed message struct via derive macros ─────────────────────────────────

/// A minimal ORDERS message representation.
///
/// In a real codebase this would be generated from the EDIFACT directory.
#[cfg(feature = "derive")]
#[derive(Debug, EdifactDeserialize, EdifactSerialize, PartialEq)]
#[edifact(segment = "BGM")]
struct BgmSegment {
    #[edifact(element = 0)]
    doc_code: String,
    #[edifact(element = 1)]
    doc_id: Option<String>,
}

#[cfg(feature = "derive")]
#[derive(Debug, EdifactDeserialize, PartialEq)]
struct OrdersMessage {
    bgm: Option<BgmSegment>,
}

// ── 3. Typed streaming over a reader ─────────────────────────────────────────

#[cfg(feature = "derive")]
fn collect_order_ids(input: &[u8]) -> Vec<Option<String>> {
    use std::io::Cursor;

    // reader variant — identical ergonomics, lazy I/O
    let reader = Cursor::new(input);
    message_windows_from_reader(reader)
        .filter_map(Result::ok)
        .map(|window| {
            // Each window is a Vec<OwnedSegment>; borrow to deserialise.
            let borrowed: Vec<Segment<'_>> = window.iter().map(OwnedSegment::as_borrowed).collect();
            OrdersMessage::edifact_deserialize(&borrowed)
                .ok()
                .and_then(|m| m.bgm.into_iter().next())
                .and_then(|bgm| bgm.doc_id)
        })
        .collect()
}

// ── 4. Error propagation — missing UNT is surfaced immediately ────────────────

fn demonstrate_error_propagation() {
    // Interchange where the second message has no UNT — window iterator errors.
    let broken = b"UNB+UNOA:1+S+R+200101:0900+1'\
                   UNH+1+ORDERS:D:96A:UN'BGM+220+OK+9'UNT+3+1'\
                   UNH+2+ORDERS:D:96A:UN'BGM+220+BROKEN+9'";
    // Collecting all windows: the last window is never closed (no UNT), so it
    // is simply not yielded (iteration ends when the inner iterator ends).
    let windows: Vec<_> = message_windows_bytes(broken)
        .collect::<Result<_, _>>()
        .unwrap();
    // Only the first, complete window is returned.
    assert_eq!(windows.len(), 1);
    println!("Received {} complete window(s) before end-of-stream", windows.len());

    // A UNH-while-in-flight scenario (no UNT before next UNH) is an error:
    let double_unh = b"UNH+1+ORDERS:D:96A:UN'BGM+220+X+9'\
                       UNH+2+ORDERS:D:96A:UN'BGM+220+Y+9'UNT+3+2'";
    let results: Vec<_> = message_windows_bytes(double_unh).collect();
    assert!(results.iter().any(|r| r.is_err()), "expected an error for double-UNH");
    println!("Double-UNH correctly produces an error");
}

fn main() {
    let interchange = b"\
        UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'\
        UNH+1+ORDERS:D:96A:UN'\
        BGM+220+PO-001+9'\
        UNT+3+1'\
        UNH+2+ORDERS:D:96A:UN'\
        BGM+220+PO-002+9'\
        UNT+3+2'\
        UNZ+2+1'";

    // ── raw window iteration ──────────────────────────────────────────────────
    let count = count_messages_in_interchange(interchange);
    println!("Interchange contains {count} message(s)");
    assert_eq!(count, 2);

    // Inspect each window directly
    for (i, window) in message_windows_bytes(interchange)
        .enumerate()
        .map(|(i, r)| (i, r.unwrap()))
    {
        let unh = &window[0];
        let msg_ref = unh
            .elements
            .first()
            .and_then(|e| e.components.first())
            .map(|c| c.as_ref())
            .unwrap_or("?");
        println!("Window {}: msg_ref={msg_ref}, segments={}", i + 1, window.len());
    }

    // ── typed streaming ───────────────────────────────────────────────────────
    #[cfg(feature = "derive")]
    {
        let order_ids = collect_order_ids(interchange);
        println!("Order IDs: {order_ids:?}");
        assert_eq!(order_ids.len(), 2);
    }

    demonstrate_error_propagation();
    println!("All message-window streaming examples passed.");
}
