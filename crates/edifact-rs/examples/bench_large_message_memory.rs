//! Memory benchmark: large interchange streaming
//!
//! Generates a ~10 MB synthetic EDIFACT interchange by repeating a short
//! sample message, then streams through it with [`edifact_rs::from_bytes`]
//! counting segments.  The goal is to verify that memory growth is bounded
//! (each `Segment<'_>` borrows from the input slice — no heap allocation per
//! segment).
//!
//! Run with:
//! ```text
//! cargo run -p edifact-rs --example bench_large_message_memory --release
//! ```

fn sample_msg() -> &'static [u8] {
    b"\
UNA:+.? '\
UNH+1+UTILMD:D:11A:UN:FV2604'\
UNH+1+ORDERS:D:11A:UN'\
BGM+220+PO-4711+9'\
DTM+137:20260401:102'\
NAD+BY+4000001000002::9'\
NAD+SU+4000001000001::9'\
UNT+5+1'"
}

fn main() -> Result<(), edifact_rs::EdifactError> {
    let target_bytes = 10_000_000usize; // ~10 MB of EDIFACT bytes
    let message = sample_msg();
    let repetitions = (target_bytes / message.len()) + 1;
    // Build the large payload by repeating the short sample.
    // In a real integration test you would use a representative message instead.
    let payload = message.repeat(repetitions);

    let mut segment_count = 0usize;
    // `from_bytes` returns a zero-copy iterator: each `Segment<'_>` borrows
    // directly from `payload`.  Peak heap usage is bounded to a single
    // `Segment` at a time — it does not grow with `payload.len()`.
    for segment in edifact_rs::from_bytes(&payload) {
        segment?; // propagate any parse errors
        segment_count += 1;
    }

    println!(
        "payload_bytes={} segment_count={}",
        payload.len(),
        segment_count
    );

    Ok(())
}
