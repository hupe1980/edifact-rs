//! Cookbook: async / tokio integration
//!
//! `edifact-rs` is a synchronous library — it exposes `std::io::Read`-based
//! streaming APIs, not async ones.  This is intentional: EDIFACT parsing is
//! CPU-bound after the first read, and adding an async runtime dependency would
//! impose it on every downstream user.
//!
//! This example shows the two canonical patterns for integrating with a
//! tokio-based application **without** adding async to the core crate:
//!
//! ## Pattern A — read into memory, parse synchronously
//!
//! ```text
//! let bytes = tokio::fs::read("payload.edi").await?;
//! let segments = edifact_rs::from_bytes(&bytes).collect::<Result<Vec<_>, _>>()?;
//! ```
//!
//! Best when: the payload fits comfortably in memory (typical for UN/EDIFACT
//! messages, which are usually < 1 MB).
//!
//! ## Pattern B — spawn_blocking for large/streaming payloads
//!
//! ```text
//! let path = path.clone();
//! let messages = tokio::task::spawn_blocking(move || {
//!     let f = std::fs::File::open(&path)?;
//!     edifact_rs::message_windows_from_reader(f)
//!         .collect::<Result<Vec<_>, _>>()
//! }).await??;
//! ```
//!
//! Best when: the file is large or originates from a blocking I/O source.
//! `spawn_blocking` moves the work onto a dedicated thread pool so the async
//! runtime is not blocked.
//!
//! ## Pattern C — channel bridge
//!
//! Parse on a dedicated thread and send results across a `tokio::sync::mpsc`
//! channel to an async consumer.  See [`channel_bridge`] below.

use std::io::Cursor;

use edifact_rs::{EdifactError, OwnedSegment, message_windows_from_reader};

// ── Pattern A: buffer, then parse ─────────────────────────────────────────────

async fn pattern_a_parse_from_bytes(edi_bytes: Vec<u8>) -> Result<usize, EdifactError> {
    // Zero async I/O: the bytes are already in memory.
    let segments: Vec<_> = edifact_rs::from_bytes(&edi_bytes)
        .collect::<Result<_, _>>()?;
    Ok(segments.len())
}

// ── Pattern B: spawn_blocking ─────────────────────────────────────────────────

async fn pattern_b_spawn_blocking(edi_bytes: Vec<u8>) -> Result<Vec<Vec<OwnedSegment>>, EdifactError> {
    tokio::task::spawn_blocking(move || {
        let cursor = Cursor::new(edi_bytes);
        message_windows_from_reader(cursor).collect::<Result<_, _>>()
    })
    .await
    // JoinError from spawn_blocking (panic in the worker thread)
    .map_err(|e| EdifactError::ValidationFailed {
        error_count: 1,
        first_message: format!("spawn_blocking panicked: {e}"),
    })?
}

// ── Pattern C: channel bridge ─────────────────────────────────────────────────

/// Parse an EDIFACT interchange in a blocking thread and stream `UNH..UNT`
/// windows to an async consumer through a channel.
async fn channel_bridge(
    edi_bytes: Vec<u8>,
) -> Result<Vec<Vec<OwnedSegment>>, EdifactError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Vec<OwnedSegment>, EdifactError>>(8);

    // Producer: blocking thread
    tokio::task::spawn_blocking(move || {
        let cursor = Cursor::new(edi_bytes);
        for window in message_windows_from_reader(cursor) {
            if tx.blocking_send(window).is_err() {
                break; // consumer dropped — stop early
            }
        }
    });

    // Consumer: async side
    let mut messages: Vec<Vec<OwnedSegment>> = Vec::new();
    while let Some(window) = rx.recv().await {
        messages.push(window?);
    }
    Ok(messages)
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), EdifactError> {
    let interchange = b"\
        UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'\
        UNH+1+ORDERS:D:96A:UN'\
        BGM+220+PO-001+9'\
        UNT+3+1'\
        UNH+2+ORDERS:D:96A:UN'\
        BGM+220+PO-002+9'\
        UNT+3+2'\
        UNZ+2+1'";

    // A: parse from bytes
    let segment_count = pattern_a_parse_from_bytes(interchange.to_vec()).await?;
    println!("Pattern A: {segment_count} segments");

    // B: spawn_blocking
    let windows_b = pattern_b_spawn_blocking(interchange.to_vec()).await?;
    println!("Pattern B: {} message window(s)", windows_b.len());
    assert_eq!(windows_b.len(), 2);

    // C: channel bridge
    let windows_c = channel_bridge(interchange.to_vec()).await?;
    println!("Pattern C: {} message window(s)", windows_c.len());
    assert_eq!(windows_c.len(), 2);

    println!("All async integration patterns passed.");
    Ok(())
}
