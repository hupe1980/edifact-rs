//! Integration tests for `ReaderConfig` resource-limit enforcement.
//!
//! The contract under test: every limit is a **hard cap that reports an error**.
//! A budget that merely ended the iterator would be indistinguishable from a
//! clean end of input, so `collect::<Result<Vec<_>, _>>()` would accept a
//! truncated interchange as a complete one. Input that ends exactly at a limit
//! is not a violation.
//!
//! Both parsing front ends are covered: the borrowed slice path
//! (`from_bytes_with_config`) and the owned reader path
//! (`from_bufread_with_config`).

use edifact_rs::{
    EdifactError, OwnedSegment, ReaderConfig, from_bufread_with_config, from_bytes_with_config,
};

// UNB + UNH + BGM + UNT + UNZ = 5 segments.
const FIVE_SEGMENT_MSG: &[u8] = concat!(
    "UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'",
    "UNH+1+ORDERS:D:11A:UN'",
    "BGM+220+12345+9'",
    "UNT+2+1'",
    "UNZ+1+1'"
)
.as_bytes();

/// Byte length of the leading `UNB` segment, terminator included.
fn first_segment_len() -> u64 {
    FIVE_SEGMENT_MSG
        .iter()
        .position(|&b| b == b'\'')
        .map(|p| p + 1)
        .expect("fixture has a terminator") as u64
}

// A single very long segment (BGM with a filler element).
fn long_segment_msg(element_len: usize) -> Vec<u8> {
    let filler = "X".repeat(element_len);
    format!("UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'BGM+{filler}'UNZ+1+1'").into_bytes()
}

/// Collect through the owned reader path.
fn read(input: &[u8], config: ReaderConfig) -> Result<Vec<OwnedSegment>, EdifactError> {
    from_bufread_with_config(std::io::Cursor::new(input.to_vec()), config).collect()
}

/// Collect through the borrowed slice path, keeping only the tags so the result
/// does not borrow the input.
fn slice_tags(input: &[u8], config: ReaderConfig) -> Result<Vec<String>, EdifactError> {
    from_bytes_with_config(input, config)
        .map(|r| r.map(|s| s.tag().to_owned()))
        .collect()
}

/// Assert that `result` failed with exactly the named limit.
#[track_caller]
fn assert_limit<T: std::fmt::Debug>(result: Result<T, EdifactError>, limit: &str, max: u64) {
    match result {
        Err(EdifactError::LimitExceeded {
            limit: got_limit,
            max: got_max,
        }) => {
            assert_eq!(got_limit, limit, "wrong limit reported");
            assert_eq!(got_max, max, "wrong ceiling reported");
        }
        other => panic!("expected LimitExceeded({limit}, {max}), got {other:?}"),
    }
}

// ── max_segments ─────────────────────────────────────────────────────────────

#[test]
fn max_segments_exceeded_is_an_error_on_both_paths() {
    let config = ReaderConfig::default().max_segments(2);
    assert_limit(read(FIVE_SEGMENT_MSG, config), "max_segments", 2);
    assert_limit(slice_tags(FIVE_SEGMENT_MSG, config), "max_segments", 2);
}

#[test]
fn max_segments_exactly_at_the_limit_succeeds() {
    // The fixture has 5 segments; a budget of exactly 5 is not a violation.
    let config = ReaderConfig::default().max_segments(5);
    assert_eq!(read(FIVE_SEGMENT_MSG, config).expect("exact fit").len(), 5);
    assert_eq!(
        slice_tags(FIVE_SEGMENT_MSG, config)
            .expect("exact fit")
            .len(),
        5
    );
}

#[test]
fn max_segments_larger_than_message_yields_all() {
    let config = ReaderConfig::default().max_segments(100);
    assert_eq!(
        read(FIVE_SEGMENT_MSG, config).expect("under budget").len(),
        5
    );
}

#[test]
fn segments_before_the_limit_are_still_delivered() {
    // A caller iterating manually keeps everything up to the violation, which is
    // what makes the error recoverable rather than all-or-nothing.
    let config = ReaderConfig::default().max_segments(2);
    let mut iter = from_bytes_with_config(FIVE_SEGMENT_MSG, config);
    assert_eq!(iter.next().unwrap().unwrap().tag, "UNB");
    assert_eq!(iter.next().unwrap().unwrap().tag, "UNH");
    assert!(matches!(
        iter.next(),
        Some(Err(EdifactError::LimitExceeded { .. }))
    ));
    // The iterator is finished after reporting the violation.
    assert!(iter.next().is_none());
}

// ── max_input_bytes ───────────────────────────────────────────────────────────

#[test]
fn max_input_bytes_exceeded_is_an_error_on_both_paths() {
    let limit = first_segment_len();
    let config = ReaderConfig::default().max_input_bytes(limit);
    assert_limit(read(FIVE_SEGMENT_MSG, config), "max_input_bytes", limit);
    assert_limit(
        slice_tags(FIVE_SEGMENT_MSG, config),
        "max_input_bytes",
        limit,
    );
}

#[test]
fn max_input_bytes_never_yields_a_segment_past_the_cap() {
    // The old "stop-after" semantics handed the caller a segment whose bytes
    // already ran past the budget. A hard cap must not.
    let limit = first_segment_len();
    let config = ReaderConfig::default().max_input_bytes(limit);
    let mut iter = from_bytes_with_config(FIVE_SEGMENT_MSG, config);
    let unb = iter.next().unwrap().expect("UNB fits exactly");
    assert_eq!(unb.tag, "UNB");
    assert!(unb.span.end as u64 <= limit);
    assert!(matches!(
        iter.next(),
        Some(Err(EdifactError::LimitExceeded { .. }))
    ));
}

#[test]
fn max_input_bytes_covering_the_whole_input_succeeds() {
    let config = ReaderConfig::default().max_input_bytes(FIVE_SEGMENT_MSG.len() as u64);
    assert_eq!(read(FIVE_SEGMENT_MSG, config).expect("exact fit").len(), 5);
}

#[test]
fn max_input_bytes_zero_rejects_any_input() {
    let config = ReaderConfig::default().max_input_bytes(0);
    assert_limit(read(FIVE_SEGMENT_MSG, config), "max_input_bytes", 0);
    assert_limit(slice_tags(FIVE_SEGMENT_MSG, config), "max_input_bytes", 0);
}

// ── max_messages ──────────────────────────────────────────────────────────────

#[test]
fn max_messages_exceeded_is_an_error() {
    let two_messages = concat!(
        "UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'",
        "UNH+1+ORDERS:D:11A:UN'BGM+220+1+9'UNT+3+1'",
        "UNH+2+ORDERS:D:11A:UN'BGM+220+2+9'UNT+3+2'",
        "UNZ+2+1'"
    )
    .as_bytes();

    let config = ReaderConfig::default().max_messages(1);
    assert_limit(read(two_messages, config), "max_messages", 1);
    assert_limit(slice_tags(two_messages, config), "max_messages", 1);

    // Two messages under a budget of two is not a violation.
    let config = ReaderConfig::default().max_messages(2);
    // UNB + (UNH BGM UNT) + (UNH BGM UNT) + UNZ
    assert_eq!(read(two_messages, config).expect("exact fit").len(), 8);
}

// ── max_segment_bytes ─────────────────────────────────────────────────────────

#[test]
fn max_segment_bytes_errors_on_oversized_segment() {
    let msg = long_segment_msg(500);
    let config = ReaderConfig::default().max_segment_bytes(100);

    let results: Vec<_> = from_bufread_with_config(std::io::Cursor::new(&msg), config).collect();

    assert!(
        results
            .iter()
            .any(|r| matches!(r, Err(EdifactError::SegmentTooLong { .. }))),
        "expected SegmentTooLong for a 500-byte segment with a 100-byte limit, got: {results:?}"
    );
}

#[test]
fn max_segment_bytes_passes_for_short_segments() {
    let config = ReaderConfig::default().max_segment_bytes(512);
    assert_eq!(read(FIVE_SEGMENT_MSG, config).expect("all short").len(), 5);
}

// ── combined limits ───────────────────────────────────────────────────────────

#[test]
fn the_tighter_of_two_limits_is_the_one_reported() {
    let limit = first_segment_len();
    let config = ReaderConfig::default()
        .max_segments(5)
        .max_input_bytes(limit);
    // Five segments are allowed, but the byte budget covers only the first.
    assert_limit(read(FIVE_SEGMENT_MSG, config), "max_input_bytes", limit);
}
