//! Integration tests verifying `ReaderConfig` resource-limit enforcement.
//!
//! These tests ensure that `max_segments`, `max_input_bytes`, and
//! `max_segment_bytes` stop the stream early and return an error (or stop
//! cleanly) rather than silently producing a partial result or hanging.

use edifact_rs::{EdifactError, ReaderConfig, from_bufread_stream_with_config};

// A minimal valid EDIFACT interchange with 5 segments separated by `'`.
// UNB + UNH + BGM + UNT + UNZ  = 5 segments
const FIVE_SEGMENT_MSG: &[u8] = concat!(
    "UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'",
    "UNH+1+ORDERS:D:11A:UN'",
    "BGM+220+12345+9'",
    "UNT+2+1'",
    "UNZ+1+1'"
)
.as_bytes();

// A single very long segment (BGM with a 200-byte filler element).
fn long_segment_msg(element_len: usize) -> Vec<u8> {
    let filler = "X".repeat(element_len);
    format!("UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'BGM+{filler}'UNZ+1+1'").into_bytes()
}

// ── max_segments ─────────────────────────────────────────────────────────────

#[test]
fn max_segments_stops_stream_at_limit() {
    let config = ReaderConfig::default().max_segments(2);
    let segments: Vec<_> =
        from_bufread_stream_with_config(std::io::Cursor::new(FIVE_SEGMENT_MSG), config)
            .collect::<Result<_, _>>()
            .expect("should not error, just stop early");

    assert_eq!(
        segments.len(),
        2,
        "stream must stop after exactly 2 segments"
    );
}

#[test]
fn max_segments_one_yields_only_first_segment() {
    let config = ReaderConfig::default().max_segments(1);
    let segments: Vec<_> =
        from_bufread_stream_with_config(std::io::Cursor::new(FIVE_SEGMENT_MSG), config)
            .collect::<Result<_, _>>()
            .expect("should not error");

    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].tag, "UNB");
}

#[test]
fn max_segments_larger_than_message_yields_all() {
    let config = ReaderConfig::default().max_segments(100);
    let segments: Vec<_> =
        from_bufread_stream_with_config(std::io::Cursor::new(FIVE_SEGMENT_MSG), config)
            .collect::<Result<_, _>>()
            .expect("should not error");

    assert_eq!(segments.len(), 5, "all segments should be yielded");
}

// ── max_input_bytes ───────────────────────────────────────────────────────────

#[test]
fn max_input_bytes_stops_before_end() {
    // Allow exactly the first segment's bytes so the
    // cursor stops before reading the second segment.
    // UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1' = 41 bytes
    let first_seg_len = FIVE_SEGMENT_MSG
        .iter()
        .position(|&b| b == b'\'')
        .map(|p| p + 1)
        .unwrap_or(0) as u64;
    let config = ReaderConfig::default().max_input_bytes(first_seg_len);
    let segments: Vec<_> =
        from_bufread_stream_with_config(std::io::Cursor::new(FIVE_SEGMENT_MSG), config)
            .collect::<Result<_, _>>()
            .expect("should not error");

    // Only the first segment (UNB) should be yielded.
    assert_eq!(
        segments.len(),
        1,
        "only UNB should fit within {first_seg_len} bytes"
    );
    assert_eq!(segments[0].tag, "UNB");
}

#[test]
fn max_input_bytes_zero_yields_nothing() {
    let config = ReaderConfig::default().max_input_bytes(0);
    let segments: Vec<_> =
        from_bufread_stream_with_config(std::io::Cursor::new(FIVE_SEGMENT_MSG), config)
            .collect::<Result<_, _>>()
            .expect("should not error");

    assert!(segments.is_empty(), "zero byte budget yields no segments");
}

// ── max_segment_bytes ─────────────────────────────────────────────────────────

#[test]
fn max_segment_bytes_errors_on_oversized_segment() {
    // Create a segment that is 500 bytes long, then set limit to 100 bytes.
    let msg = long_segment_msg(500);
    let config = ReaderConfig::default().max_segment_bytes(100);

    let results: Vec<_> =
        from_bufread_stream_with_config(std::io::Cursor::new(&msg), config).collect();

    let has_segment_too_long = results
        .iter()
        .any(|r| matches!(r, Err(EdifactError::SegmentTooLong { .. })));
    assert!(
        has_segment_too_long,
        "expected SegmentTooLong error for 500-byte segment with 100-byte limit, got: {results:?}"
    );
}

#[test]
fn max_segment_bytes_passes_for_short_segments() {
    let config = ReaderConfig::default().max_segment_bytes(512);
    let segments: Vec<_> =
        from_bufread_stream_with_config(std::io::Cursor::new(FIVE_SEGMENT_MSG), config)
            .collect::<Result<_, _>>()
            .expect("all segments are short; should succeed");

    assert_eq!(segments.len(), 5);
}

// ── combined limits ───────────────────────────────────────────────────────────

#[test]
fn combined_max_segments_and_max_input_bytes_respects_tighter_limit() {
    // Compute the first segment's byte length so the byte budget is tight.
    let first_seg_len = FIVE_SEGMENT_MSG
        .iter()
        .position(|&b| b == b'\'')
        .map(|p| p + 1)
        .unwrap_or(0) as u64;
    // max_segments=5 but max_input_bytes covers only the first segment.
    let config = ReaderConfig::default()
        .max_segments(5)
        .max_input_bytes(first_seg_len);
    let segments: Vec<_> =
        from_bufread_stream_with_config(std::io::Cursor::new(FIVE_SEGMENT_MSG), config)
            .collect::<Result<_, _>>()
            .expect("should not error");

    // Byte limit fires first — only UNB should be yielded.
    assert_eq!(
        segments.len(),
        1,
        "byte limit should truncate before segment limit"
    );
}
