//! Memory benchmark: large interchange streaming
//!
//! Builds a ~10 MB synthetic interchange and streams it with
//! [`edifact_rs::from_bytes`], counting segments. The point is that peak heap
//! usage is bounded by one `Segment` — each borrows from the input slice — and
//! does not grow with the payload.
//!
//! The payload is a **well-formed** interchange: one `UNA`, one `UNB`, one
//! `UNH`, a repeated body, and trailers whose declared counts match what was
//! written. A previous version of this example repeated a whole fragment
//! including its `UNA`, which put a service string advice in the middle of the
//! stream where it is an ordinary (and invalid) segment tag — so the example
//! failed on its own input, and validating the result was impossible.
//!
//! Run with:
//! ```text
//! cargo run -p edifact-rs --example bench_large_message_memory --release
//! ```

/// One repetition of the message body — four segments, no envelope.
const BODY: &[u8] = b"\
BGM+220+PO-4711+9'\
DTM+137:20260401:102'\
NAD+BY+4000001000002::9'\
NAD+SU+4000001000001::9'";

/// Segments per `BODY` repetition.
const BODY_SEGMENTS: usize = 4;

/// Build a single well-formed interchange of roughly `target_bytes`.
///
/// Returns the payload and the number of segments it contains, so the count the
/// parser reports can be checked against what was written rather than merely
/// printed.
fn interchange(target_bytes: usize) -> (Vec<u8>, usize) {
    let header = b"UNA:+.? 'UNB+UNOA:1+SENDER+RECEIVER+260401:0900+IC1'UNH+1+ORDERS:D:11A:UN'";
    let repetitions = target_bytes / BODY.len() + 1;

    let mut payload = Vec::with_capacity(target_bytes + header.len() + 64);
    payload.extend_from_slice(header);
    for _ in 0..repetitions {
        payload.extend_from_slice(BODY);
    }

    // UNT DE 0074 counts UNH + body + UNT; UNZ DE 0036 counts messages.
    let body_segments = repetitions * BODY_SEGMENTS;
    payload.extend_from_slice(format!("UNT+{}+1'", body_segments + 2).as_bytes());
    payload.extend_from_slice(b"UNZ+1+IC1'");

    // UNB + UNH + body + UNT + UNZ.  The UNA is service string advice, not a segment.
    (payload, body_segments + 4)
}

fn main() -> Result<(), edifact_rs::EdifactError> {
    let (payload, expected_segments) = interchange(10_000_000);

    let mut segment_count = 0usize;
    // `from_bytes` returns a zero-copy iterator: each `Segment<'_>` borrows
    // directly from `payload`.  Peak heap usage is bounded to a single segment
    // at a time — it does not grow with `payload.len()`.
    for segment in edifact_rs::from_bytes(&payload) {
        segment?; // propagate any parse error
        segment_count += 1;
    }

    assert_eq!(
        segment_count, expected_segments,
        "the parser must see exactly the segments that were written",
    );

    // A second pass proves the synthetic payload is a conformant interchange and
    // not merely one this parser tolerates: the trailers' declared counts are
    // checked against what is actually there.
    let segments: Vec<_> = edifact_rs::from_bytes(&payload).collect::<Result<Vec<_>, _>>()?;
    let validated = edifact_rs::validate_envelope(&segments)?;

    println!(
        "payload_bytes={} segment_count={} messages={}",
        payload.len(),
        segment_count,
        validated.message_count(),
    );

    Ok(())
}
