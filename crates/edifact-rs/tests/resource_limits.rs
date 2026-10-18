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
//! (`from_bufread_with_config`), and every streaming convenience built on them.
//!
//! The bounded-streaming proofs (the second half of this file) feed endless
//! generator readers through each entry point and assert three things: the
//! configured limit stops the stream with `E036` at a stated byte offset, the
//! slice and reader paths report the identical error after the identical
//! items, and peak heap — measured by a counting global allocator — stays
//! below a bound derived from the limits, independent of how much input the
//! generator could still produce.

#[allow(dead_code)]
#[path = "support/counting_alloc.rs"]
mod counting_alloc;

use edifact_rs::{
    EdifactError, OwnedSegment, ReaderConfig, from_bufread_with_config, from_bytes_with_config,
};

#[global_allocator]
static ALLOC: counting_alloc::CountingAlloc = counting_alloc::CountingAlloc;

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

/// Assert that `result` failed with exactly the named limit; returns the
/// reported offset.
#[track_caller]
fn assert_limit<T: std::fmt::Debug>(
    result: Result<T, EdifactError>,
    limit: &str,
    max: u64,
) -> usize {
    match result {
        Err(error) => assert_limit_error(&error, limit, max),
        Ok(value) => panic!("expected LimitExceeded({limit}, {max}), got Ok({value:?})"),
    }
}

/// Assert that `error` is exactly the named limit; returns its offset.
#[track_caller]
fn assert_limit_error(error: &EdifactError, limit: &str, max: u64) -> usize {
    match error {
        EdifactError::LimitExceeded {
            limit: got_limit,
            max: got_max,
            offset,
            ..
        } => {
            assert_eq!(*got_limit, limit, "wrong limit reported");
            assert_eq!(*got_max, max, "wrong ceiling reported");
            assert_eq!(error.stable_code(), "E036");
            *offset
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

#[test]
fn default_max_segment_bytes_is_64_kib() {
    const LIMIT: usize = 65_536;
    assert_eq!(ReaderConfig::default().max_segment_bytes, LIMIT);

    // The limit counts the segment body from the tag, terminator excluded;
    // `long_segment_msg(n)`'s BGM body is `BGM+` plus `n` filler bytes.
    let at_limit = long_segment_msg(LIMIT - 4);
    let over_limit = long_segment_msg(LIMIT - 3);

    let slice: Vec<_> = edifact_rs::from_bytes(&at_limit)
        .collect::<Result<_, _>>()
        .expect("a segment of exactly the default limit parses on the slice path");
    assert_eq!(slice.len(), 3);
    let owned: Vec<_> = edifact_rs::from_reader(&at_limit[..])
        .collect::<Result<_, _>>()
        .expect("a segment of exactly the default limit parses on the reader path");
    assert_eq!(owned.len(), 3);

    let errors = [
        edifact_rs::from_bytes(&over_limit)
            .collect::<Result<Vec<_>, _>>()
            .map(|_| ())
            .expect_err("one byte over the default limit fails on the slice path"),
        edifact_rs::from_reader(&over_limit[..])
            .collect::<Result<Vec<_>, _>>()
            .map(|_| ())
            .expect_err("one byte over the default limit fails on the reader path"),
    ];
    for error in errors {
        assert!(
            matches!(error, EdifactError::SegmentTooLong { limit, .. } if limit == LIMIT),
            "{error:?}"
        );
        assert_eq!(error.stable_code(), "E020");
    }
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

// ── the recovering scan ──────────────────────────────────────────────────────
//
// The recovering scan resumes after a fault instead of stopping, so it needs
// the same limits plus a fault budget, and it must end with the same error
// kind the strict parser gives when a limit trips.

use edifact_rs::recover::{Fault, Scanned};
use edifact_rs::{RecoveryConfig, scan_recovering};

/// Scan `input`, returning every item.
fn recover(input: &[u8], config: RecoveryConfig) -> Vec<Scanned<'_>> {
    scan_recovering(input, &config).collect()
}

/// The terminal fault that ends `items`, asserting it is the last item.
#[track_caller]
fn terminal<'s>(items: &'s [Scanned<'_>]) -> &'s Fault {
    let fault = items
        .last()
        .and_then(Scanned::as_fault)
        .expect("the scan ends with a fault");
    assert!(fault.is_terminal(), "{fault:?}");
    assert_eq!(
        items
            .iter()
            .filter_map(Scanned::as_fault)
            .filter(|f| f.is_terminal())
            .count(),
        1,
        "exactly one terminal fault"
    );
    fault
}

/// The strict error for `input` under `config`.
fn strict_error(input: &[u8], config: ReaderConfig) -> EdifactError {
    from_bytes_with_config(input, config)
        .find_map(Result::err)
        .expect("the strict parser refuses the input")
}

#[test]
fn recovering_scan_stops_at_fault_budget() {
    // Every byte of `''''…` is a fault. Without a budget, 10 MB would hold ten
    // million errors in memory.
    let input = vec![b'\''; 10 * 1024 * 1024];
    let items = recover(&input, RecoveryConfig::default());
    assert_eq!(items.len(), 1_001);
    let fault = terminal(&items);
    assert_eq!(fault.ordinal(), 1_000);
    assert_limit_error(fault.error(), "max_faults", 1_000);
}

#[test]
fn recovering_scan_reports_each_reader_limit_like_the_strict_parser() {
    let two_messages = concat!(
        "UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'",
        "UNH+1+ORDERS:D:11A:UN'BGM+220+1+9'UNT+3+1'",
        "UNH+2+ORDERS:D:11A:UN'BGM+220+2+9'UNT+3+2'",
        "UNZ+2+1'"
    )
    .as_bytes();
    let configs = [
        ReaderConfig::default().max_segments(2),
        ReaderConfig::default().max_messages(1),
        ReaderConfig::default().max_input_bytes(first_segment_len()),
    ];
    for reader in configs {
        let items = recover(two_messages, RecoveryConfig::new().reader(reader));
        let fault = terminal(&items);
        assert_eq!(
            fault.error(),
            &strict_error(two_messages, reader),
            "{reader:?}"
        );
    }

    let reader = ReaderConfig::default().max_segment_bytes(100);
    let long = long_segment_msg(500);
    let items = recover(&long, RecoveryConfig::new().reader(reader));
    let fault = terminal(&items);
    assert!(matches!(
        fault.error(),
        EdifactError::SegmentTooLong { limit: 100, .. }
    ));
    assert_eq!(fault.error(), &strict_error(&long, reader));
}

#[test]
fn recovering_scan_counts_faults_against_max_segments() {
    // Three faults and a budget of two items: faults are segments on the wire.
    let reader = ReaderConfig::default().max_segments(2);
    let items = recover(b"'''", RecoveryConfig::new().reader(reader));
    assert_eq!(items.len(), 3);
    assert_limit_error(terminal(&items).error(), "max_segments", 2);
}

#[test]
fn recovering_scan_counts_whitespace_against_max_input_bytes() {
    // The strict path accepts 10 MB of whitespace as an empty input; the scan
    // counts every consumed byte and refuses it.
    let input = vec![b' '; 10 * 1024 * 1024];
    let reader = ReaderConfig::default().max_input_bytes(1_000);
    let items = recover(&input, RecoveryConfig::new().reader(reader));
    assert_eq!(items.len(), 1);
    assert_limit_error(terminal(&items).error(), "max_input_bytes", 1_000);
}

#[test]
fn recovering_scan_applies_max_segments_per_message() {
    // Faults are segments on the wire, so a faulted segment inside a message
    // counts against its cap like any other.
    let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220'bad'DTM+137:1:102'UNT+5+1'";
    let reader = ReaderConfig::default().max_segments_per_message(3);
    let items = recover(input, RecoveryConfig::new().reader(reader));
    let fault = terminal(&items);
    let offset = assert_limit_error(fault.error(), "max_segments_per_message", 3);
    assert_eq!(&input[offset..offset + 3], b"DTM");
}

// ══ Bounded streaming ═══════════════════════════════════════════════════════

use std::cell::Cell;
use std::fmt::Debug;
use std::io::{BufReader, Read};
use std::rc::Rc;

use edifact_rs::{
    EdifactDeserialize, EdifactSegmentTag, Segment, deserialize_each, deserialize_each_from_reader,
    deserialize_each_from_reader_with_config, deserialize_each_with_config, deserialize_messages,
    deserialize_messages_from_reader, deserialize_messages_from_reader_with_config,
    deserialize_messages_with_config, from_reader_with_config, message_windows,
    message_windows_from_reader, message_windows_from_reader_with_config,
    message_windows_with_config,
};

const UNB: &[u8] = b"UNB+UNOC:3+SENDER:14+RECEIVER:14+260101:0900+IC1'";
const UNH: &[u8] = b"UNH+1+ORDERS:D:96A:UN'";
const UNZ: &[u8] = b"UNZ+1+IC1'";
/// A 63-byte free-text segment: the repeating unit of the hostile streams.
const FTX: &[u8] = b"FTX+AAA+++THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG AGAIN AN'";

/// Hard stop of a generator whose stream a limit is supposed to end: reaching
/// it means the limit did not trip, and the test fails instead of hanging.
const HARD_STOP: u64 = 256 << 20;
/// Hard stop for streams that buffer a window: a regression there buffers
/// everything it reads, so stop early enough to fail without exhausting memory.
const WINDOW_STOP: u64 = 16 << 20;
/// `BufReader`'s default capacity: how far the reader path may read ahead.
const READ_BUFFER: u64 = 8 * 1024;

/// A reader that serves `header` once and then `unit` over and over, until
/// `stop` bytes have been served in total.
struct Generator {
    header: Vec<u8>,
    unit: Vec<u8>,
    stop: u64,
    served: Rc<Cell<u64>>,
}

impl Generator {
    /// The generator and a handle on how many bytes it has served.
    fn new(header: &[u8], unit: &[u8], stop: u64) -> (Self, Rc<Cell<u64>>) {
        let served = Rc::new(Cell::new(0));
        let generator = Self {
            header: header.to_vec(),
            unit: unit.to_vec(),
            stop,
            served: Rc::clone(&served),
        };
        (generator, served)
    }
}

impl Read for Generator {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let mut pos = self.served.get();
        let want = (out.len() as u64).min(self.stop.saturating_sub(pos)) as usize;
        let mut written = 0;
        while written < want {
            let header_len = self.header.len() as u64;
            let (source, at) = if pos < header_len {
                (&self.header, pos as usize)
            } else {
                (
                    &self.unit,
                    ((pos - header_len) % self.unit.len() as u64) as usize,
                )
            };
            let take = (source.len() - at).min(want - written);
            out[written..written + take].copy_from_slice(&source[at..at + take]);
            written += take;
            pos += take as u64;
        }
        self.served.set(pos);
        Ok(written)
    }
}

/// The `Debug` form of every `Ok` item up to the first error, and that error.
///
/// Asserts that the iterator ends after an error (no item follows a limit, or
/// any other, error).
#[track_caller]
fn outcome<T: Debug>(
    mut iter: impl Iterator<Item = Result<T, EdifactError>>,
) -> (Vec<String>, Option<EdifactError>) {
    let mut items = Vec::new();
    while let Some(item) = iter.next() {
        match item {
            Ok(value) => items.push(format!("{value:?}")),
            Err(error) => {
                assert!(iter.next().is_none(), "an item followed {error:?}");
                assert!(iter.next().is_none(), "an item followed {error:?}");
                return (items, Some(error));
            }
        }
    }
    (items, None)
}

/// Every tag of a message window, as a typed value.
#[derive(Debug, PartialEq)]
struct Tags(Vec<String>);

impl EdifactDeserialize for Tags {
    fn edifact_deserialize(segments: &[Segment<'_>]) -> Result<Self, EdifactError> {
        Ok(Self(segments.iter().map(|s| s.tag().to_owned()).collect()))
    }
}

/// The text of an `FTX` segment, as a typed value.
#[derive(Debug, PartialEq)]
struct Ftx(String);

impl EdifactSegmentTag for Ftx {
    const SEGMENT_TAG: &'static str = "FTX";
}

impl EdifactDeserialize for Ftx {
    fn edifact_deserialize(segments: &[Segment<'_>]) -> Result<Self, EdifactError> {
        Ok(Self(
            segments
                .first()
                .and_then(|s| s.element_str(3))
                .unwrap_or_default()
                .to_owned(),
        ))
    }
}

/// What every entry point gives for one input and configuration.
#[derive(Debug, PartialEq)]
struct Outcomes {
    segments: (Vec<String>, Option<EdifactError>),
    windows: (Vec<String>, Option<EdifactError>),
    messages: (Vec<String>, Option<EdifactError>),
    each: (Vec<String>, Option<EdifactError>),
}

/// Run `input` through every slice entry point and every reader entry point —
/// the reader segments twice, through the default buffer (fast path) and a
/// seven-byte one (slow path) — and assert they all agree, limit errors
/// included (code, limit, ceiling and offset). Returns the common outcome.
#[track_caller]
fn assert_same_on_both_paths(input: &[u8], config: ReaderConfig) -> Outcomes {
    let slice = Outcomes {
        segments: outcome(
            from_bytes_with_config(input, config).map(|r| r.map(Segment::into_owned)),
        ),
        windows: outcome(message_windows_with_config(input, config)),
        messages: outcome(deserialize_messages_with_config::<Tags>(input, config)),
        each: outcome(deserialize_each_with_config::<Ftx>(input, config)),
    };
    let reader = Outcomes {
        segments: outcome(from_reader_with_config(input, config)),
        windows: outcome(message_windows_from_reader_with_config(input, config)),
        messages: outcome(deserialize_messages_from_reader_with_config::<Tags, _>(
            input, config,
        )),
        each: outcome(deserialize_each_from_reader_with_config::<Ftx, _>(
            input, config,
        )),
    };
    assert_eq!(slice, reader, "slice and reader paths disagree");
    let slow = outcome(from_bufread_with_config(
        BufReader::with_capacity(7, input),
        config,
    ));
    assert_eq!(slice.segments, slow, "the reader's slow path disagrees");
    slice
}

/// Byte offset of the `n`th (1-based) segment of a window that starts right
/// after `UNB` with `UNH` followed by `FTX` segments.
fn window_segment_offset(n: usize) -> usize {
    UNB.len() + UNH.len() + (n - 2) * FTX.len()
}

/// `UNB`, `UNH` and `count` `FTX` segments, in memory.
fn unterminated(count: usize) -> Vec<u8> {
    let mut input = [UNB, UNH].concat();
    for _ in 0..count {
        input.extend_from_slice(FTX);
    }
    input
}

#[test]
fn the_generator_unit_is_63_bytes() {
    assert_eq!(FTX.len(), 63);
}

/// Shared body of the three `unterminated_message_*` tests: `UNH` then endless
/// `FTX` through the reader entry point `stream`, and the same bytes in memory
/// through both paths.
#[track_caller]
fn assert_unterminated_message_is_stopped(
    stream: impl FnOnce(Generator, ReaderConfig) -> (Vec<String>, Option<EdifactError>),
) {
    let config = ReaderConfig::default().max_segments_per_message(1_000);
    let (generator, served) = Generator::new(&[UNB, UNH].concat(), FTX, WINDOW_STOP);
    let (_, error) = stream(generator, config);
    let error = error.expect("the per-message cap must stop the stream");
    let offset = assert_limit_error(&error, "max_segments_per_message", 1_000);
    assert_eq!(offset, window_segment_offset(1_001));
    assert!(
        served.get() < WINDOW_STOP,
        "the generator reached its stop without a limit error"
    );

    // The same bytes in memory, through both paths.
    let outcomes = assert_same_on_both_paths(&unterminated(2_000), config);
    assert_eq!(outcomes.windows.1.as_ref(), Some(&error));
    assert_eq!(outcomes.messages.1.as_ref(), Some(&error));
    assert_eq!(outcomes.each.1.as_ref(), Some(&error));
    // The segments before the violation are still handed out: UNB, UNH and
    // 999 FTX.
    assert_eq!(outcomes.segments.0.len(), 1_001);
    assert_eq!(outcomes.each.0.len(), 999);
}

#[test]
fn unterminated_message_message_windows() {
    assert_unterminated_message_is_stopped(|generator, config| {
        let (windows, error) = outcome(message_windows_from_reader_with_config(generator, config));
        assert!(windows.is_empty(), "no partial window");
        (windows, error)
    });
}

#[test]
fn unterminated_message_deserialize_messages() {
    assert_unterminated_message_is_stopped(|generator, config| {
        let (messages, error) = outcome(deserialize_messages_from_reader_with_config::<Tags, _>(
            generator, config,
        ));
        assert!(messages.is_empty(), "no partial message");
        (messages, error)
    });
}

#[test]
fn unterminated_message_deserialize_each() {
    assert_unterminated_message_is_stopped(|generator, config| {
        let (each, error) = outcome(deserialize_each_from_reader_with_config::<Ftx, _>(
            generator, config,
        ));
        assert_eq!(each.len(), 999, "every FTX of the window up to the cap");
        (each, error)
    });
}

/// A three-message interchange inside every limit.
fn three_messages() -> Vec<u8> {
    let mut input = UNB.to_vec();
    for n in 1..=3 {
        input.extend_from_slice(
            format!("UNH+{n}+ORDERS:D:96A:UN'BGM+220+PO-{n}'FTX+AAA+++NOTE {n}'UNT+4+{n}'")
                .as_bytes(),
        );
    }
    input.extend_from_slice(b"UNZ+3+IC1'");
    input
}

#[test]
fn conveniences_match_default_config_variants_within_limits() {
    // The functions without a configuration are the `_with_config` ones under
    // `ReaderConfig::default()`; limits that do not trip change nothing.
    let input = three_messages();
    let generous = ReaderConfig::default()
        .max_segments(1_000)
        .max_messages(10)
        .max_segments_per_message(100)
        .max_input_bytes(1 << 20);
    let defaults = Outcomes {
        segments: outcome(edifact_rs::from_bytes(&input).map(|r| r.map(Segment::into_owned))),
        windows: outcome(message_windows(&input)),
        messages: outcome(deserialize_messages::<Tags>(&input)),
        each: outcome(deserialize_each::<Ftx>(&input)),
    };
    assert_eq!(defaults.windows.0.len(), 3);
    assert_eq!(defaults.each.0.len(), 3);
    assert_eq!(defaults.segments.1, None);
    for config in [ReaderConfig::default(), generous] {
        assert_eq!(assert_same_on_both_paths(&input, config), defaults);
    }
    let reader_defaults = Outcomes {
        segments: outcome(edifact_rs::from_reader(&input[..])),
        windows: outcome(message_windows_from_reader(&input[..])),
        messages: outcome(deserialize_messages_from_reader::<Tags, _>(&input[..])),
        each: outcome(deserialize_each_from_reader::<Ftx, _>(&input[..])),
    };
    assert_eq!(reader_defaults, defaults);
}

#[test]
fn limit_error_terminates_windows_and_typed_iterators() {
    // `outcome` asserts that `next()` is `None` twice after the error, for
    // every iterator `assert_same_on_both_paths` runs: the two segment
    // iterators, both `MessageWindows`, and the four typed iterators.
    let config = ReaderConfig::default().max_segments_per_message(10);
    let outcomes = assert_same_on_both_paths(&unterminated(50), config);
    for (_, error) in [
        &outcomes.segments,
        &outcomes.windows,
        &outcomes.messages,
        &outcomes.each,
    ] {
        assert_limit_error(
            error.as_ref().expect("limit error"),
            "max_segments_per_message",
            10,
        );
    }
}

#[test]
fn max_messages_alone_does_not_bound_a_window() {
    // A finite stream whose only message never ends.
    let stop = (UNB.len() + UNH.len() + 10_000 * FTX.len()) as u64;
    let header = [UNB, UNH].concat();

    // `max_messages(1)` admits the one message — and buffers all of it.
    let (generator, served) = Generator::new(&header, FTX, stop);
    let config = ReaderConfig::default().max_messages(1);
    let (windows, error) = outcome(message_windows_from_reader_with_config(generator, config));
    assert!(windows.is_empty());
    assert_eq!(error.as_ref().map(EdifactError::stable_code), Some("E001"));
    assert_eq!(served.get(), stop, "the whole stream was read");

    // The per-message cap is what stops it.
    let (generator, served) = Generator::new(&header, FTX, stop);
    let config = config.max_segments_per_message(1_000);
    let (_, error) = outcome(message_windows_from_reader_with_config(generator, config));
    let offset = assert_limit_error(&error.expect("E036"), "max_segments_per_message", 1_000);
    assert_eq!(offset, window_segment_offset(1_001));
    assert!(served.get() < stop);
}

/// Peak heap (bytes) of draining `stream` built over a generator that could
/// serve `stop` bytes; the stream's final error is returned too.
fn peak_heap<I, T>(
    header: &[u8],
    unit: &[u8],
    stop: u64,
    stream: impl FnOnce(Generator) -> I,
) -> (usize, usize, Option<EdifactError>)
where
    I: Iterator<Item = Result<T, EdifactError>>,
{
    let (generator, _) = Generator::new(header, unit, stop);
    let ((items, error), stats) = counting_alloc::measure(|| {
        let mut items = 0;
        let mut error = None;
        for item in stream(generator) {
            match item {
                Ok(_) => items += 1,
                Err(e) => error = Some(e),
            }
        }
        (items, error)
    });
    (stats.peak_bytes, items, error)
}

/// `a` and `b` differ by less than 10 %.
#[track_caller]
fn assert_within_ten_percent(a: usize, b: usize) {
    let (low, high) = (a.min(b), a.max(b));
    assert!(
        high - low <= high / 10,
        "peak heap depends on the input length: {a} vs {b} bytes"
    );
}

#[cfg(target_pointer_width = "64")]
#[test]
fn peak_heap_bounded_by_limits_unterminated() {
    // One window of at most 1 000 owned 63-byte segments, the window vector and
    // an 8 KiB read buffer: about 1 MB, asserted below 2 MiB — and the same
    // whether the generator could serve 1 MiB or 10 MiB more.
    let config = ReaderConfig::default().max_segments_per_message(1_000);
    let header = [UNB, UNH].concat();
    let run = |stop: u64| {
        let (peak, windows, error) = peak_heap(&header, FTX, stop, |generator| {
            message_windows_from_reader_with_config(generator, config)
        });
        assert_eq!(windows, 0);
        assert_limit_error(&error.expect("E036"), "max_segments_per_message", 1_000);
        peak
    };
    let short = run(1 << 20);
    let long = run(10 << 20);
    assert!(short <= 2 << 20, "peak heap {short} B exceeds 2 MiB");
    assert_within_ten_percent(short, long);

    // The segment stream underneath holds one segment at a time.
    let (peak, _, error) = peak_heap(&header, FTX, 10 << 20, |generator| {
        deserialize_each_from_reader_with_config::<Ftx, _>(generator, config)
    });
    assert!(error.is_some());
    assert!(peak <= 64 << 10, "peak heap {peak} B exceeds 64 KiB");
}

/// `count` valid three-segment messages after a `UNB`, then (optionally) a
/// message of `last` segments (`UNH`, `last - 2` × `FTX`, `UNT`) and `UNZ`.
/// Returns the input and the offset of the last message's `UNT`.
fn long_lived(count: usize, last: Option<usize>) -> (Vec<u8>, usize) {
    let mut input = UNB.to_vec();
    for n in 0..count {
        input.extend_from_slice(
            format!("UNH+{n}+ORDERS:D:96A:UN'BGM+220+{n}'UNT+3+{n}'").as_bytes(),
        );
    }
    let mut unt = 0;
    if let Some(last) = last {
        input.extend_from_slice(UNH);
        for _ in 0..last - 2 {
            input.extend_from_slice(FTX);
        }
        unt = input.len();
        input.extend_from_slice(format!("UNT+{last}+1'").as_bytes());
    }
    input.extend_from_slice(UNZ);
    (input, unt)
}

#[test]
fn per_message_cap_on_long_lived_stream() {
    // No whole-input limit: only the oversized message is refused, after the
    // 3 000 good ones — which `max_segments` could not do.
    let config = ReaderConfig::default().max_segments_per_message(50);
    let (input, unt) = long_lived(3_000, Some(51));
    let outcomes = assert_same_on_both_paths(&input, config);
    assert_eq!(outcomes.windows.0.len(), 3_000);
    let offset = assert_limit_error(
        outcomes.windows.1.as_ref().expect("E036"),
        "max_segments_per_message",
        50,
    );
    assert_eq!(offset, unt, "at the start of the 51st segment, its UNT");
}

#[test]
fn per_message_cap_exact_fit() {
    let config = ReaderConfig::default().max_segments_per_message(50);
    let (input, _) = long_lived(3, Some(50));
    let outcomes = assert_same_on_both_paths(&input, config);
    assert_eq!(outcomes.windows, (outcomes.windows.0.clone(), None));
    assert_eq!(outcomes.windows.0.len(), 4);
}

#[test]
fn envelope_segments_do_not_count() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'UNG+ORDERS+S+R+260101:0900+G1+UN+D:96A'\
UNH+1+ORDERS:D:96A:UN'BGM+220+1'UNT+3+1'UNE+1+G1'UNZ+1+IC1'";
    let config = ReaderConfig::default().max_segments_per_message(3);
    let outcomes = assert_same_on_both_paths(input, config);
    assert_eq!(outcomes.segments.0.len(), 7);
    assert_eq!(outcomes.segments.1, None);
}

#[test]
fn unh_inside_open_message_restarts_count() {
    let input = b"UNH+1+ORDERS:D:96A:UN'FTX+AAA+++A'FTX+AAA+++B'\
UNH+2+ORDERS:D:96A:UN'FTX+AAA+++C'FTX+AAA+++D'UNT+6+2'";
    let config = ReaderConfig::default().max_segments_per_message(3);
    let outcomes = assert_same_on_both_paths(input, config);
    // The segment stream restarts the count at the second UNH (otherwise it
    // would be the fourth segment), and refuses that message's UNT.
    assert_eq!(outcomes.segments.0.len(), 6);
    let offset = assert_limit_error(
        outcomes.segments.1.as_ref().expect("E036"),
        "max_segments_per_message",
        3,
    );
    assert_eq!(&input[offset..offset + 3], b"UNT");
    // Message windows still report the unclosed first message.
    assert_eq!(
        outcomes.windows.1.as_ref().map(EdifactError::stable_code),
        Some("E011")
    );
}

#[cfg(target_pointer_width = "64")]
#[test]
fn peak_heap_bounded_by_limits_long_lived() {
    let config = ReaderConfig::default().max_segments_per_message(50);
    let unit = format!("{}BGM+220+1'UNT+3+1'", std::str::from_utf8(UNH).unwrap()).into_bytes();
    let run = |messages: u64| {
        let stop = UNB.len() as u64 + messages * unit.len() as u64;
        let (peak, windows, error) = peak_heap(UNB, &unit, stop, |generator| {
            message_windows_from_reader_with_config(generator, config)
        });
        assert_eq!(windows as u64, messages);
        assert_eq!(error, None);
        peak
    };
    let short = run(3_000);
    let long = run(30_000);
    assert!(short <= 64 << 10, "peak heap {short} B exceeds 64 KiB");
    assert_within_ten_percent(short, long);
}

#[test]
fn endless_whitespace_reader_stops() {
    let config = ReaderConfig::default().max_input_bytes(1_000);
    let check_ahead = |ahead: u64,
                       served: &Rc<Cell<u64>>,
                       (items, error): (Vec<String>, Option<EdifactError>)| {
        let offset = assert_limit_error(&error.expect("E036"), "max_input_bytes", 1_000);
        assert_eq!(offset, 1_000);
        assert!(
            served.get() <= 1_000 + ahead,
            "read {} bytes for a budget of 1 000",
            served.get()
        );
        items
    };
    let check = |served: &Rc<Cell<u64>>, result| check_ahead(READ_BUFFER, served, result);

    let (generator, served) = Generator::new(UNB, b" ", HARD_STOP);
    let items = check(&served, outcome(from_reader_with_config(generator, config)));
    assert_eq!(items.len(), 1, "the UNB, then the budget");

    let (generator, served) = Generator::new(UNB, b" ", HARD_STOP);
    check(
        &served,
        outcome(message_windows_from_reader_with_config(generator, config)),
    );
    let (generator, served) = Generator::new(UNB, b" ", HARD_STOP);
    check(
        &served,
        outcome(deserialize_messages_from_reader_with_config::<Tags, _>(
            generator, config,
        )),
    );
    let (generator, served) = Generator::new(UNB, b" ", HARD_STOP);
    check(
        &served,
        outcome(deserialize_each_from_reader_with_config::<Ftx, _>(
            generator, config,
        )),
    );
    // Through the decoding front end the budget counts decoded bytes, which
    // for this ASCII stream are the same bytes; the 4 KiB charset probe is
    // read ahead as well.
    let (generator, served) = Generator::new(UNB, b" ", HARD_STOP);
    let items = check_ahead(
        4_096 + READ_BUFFER,
        &served,
        outcome(edifact_rs::from_reader_decoded_with_config(
            generator, config,
        )),
    );
    assert_eq!(items.len(), 1);
}

#[test]
fn whitespace_slice() {
    let mut input = UNB.to_vec();
    input.resize(10 * 1024 * 1024, b' ');
    let config = ReaderConfig::default().max_input_bytes(1_000);
    let outcomes = assert_same_on_both_paths(&input, config);
    assert_eq!(outcomes.segments.0.len(), 1);
    let offset = assert_limit_error(
        outcomes.segments.1.as_ref().expect("E036"),
        "max_input_bytes",
        1_000,
    );
    assert_eq!(offset, 1_000);
}

#[test]
fn stray_terminators_count() {
    let mut input = UNB.to_vec();
    input.extend(std::iter::repeat_n(b'\'', 5_000));

    // Every stray terminator beyond the budget is consumed input: both paths
    // refuse them at the budget.
    let budget = UNB.len() as u64;
    let outcomes =
        assert_same_on_both_paths(&input, ReaderConfig::default().max_input_bytes(budget));
    assert_eq!(outcomes.segments.0.len(), 1);
    let offset = assert_limit_error(
        outcomes.segments.1.as_ref().expect("E036"),
        "max_input_bytes",
        budget,
    );
    assert_eq!(offset, UNB.len());

    // Stray terminators inside the budget are skipped by the reader, and still
    // counted. (The slice path refuses the first stray terminator with E002, a
    // divergence that predates the byte budget and is not a limit question.)
    let config = ReaderConfig::default().max_input_bytes(1_000);
    let (items, error) = outcome(from_reader_with_config(&input[..], config));
    assert_eq!(items.len(), 1);
    let offset = assert_limit_error(&error.expect("E036"), "max_input_bytes", 1_000);
    assert_eq!(offset, 1_000);
}

#[test]
fn bom_and_una_count() {
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend_from_slice(UNB);
    let outcomes = assert_same_on_both_paths(&bom, ReaderConfig::default().max_input_bytes(2));
    assert!(outcomes.segments.0.is_empty());
    assert_eq!(
        assert_limit_error(
            outcomes.segments.1.as_ref().expect("E036"),
            "max_input_bytes",
            2
        ),
        2
    );

    let una = [b"UNA:+.? '".as_slice(), UNB].concat();
    let outcomes = assert_same_on_both_paths(&una, ReaderConfig::default().max_input_bytes(8));
    assert!(outcomes.segments.0.is_empty());
    assert_eq!(
        assert_limit_error(
            outcomes.segments.1.as_ref().expect("E036"),
            "max_input_bytes",
            8
        ),
        8
    );

    // Both prologues fit when the budget covers them and the UNB.
    for input in [&bom, &una] {
        let config = ReaderConfig::default().max_input_bytes(input.len() as u64);
        assert_eq!(assert_same_on_both_paths(input, config).segments.1, None);
    }
}

#[test]
fn exact_fit_with_trailing_whitespace() {
    let input = [UNB, b"\r\n  \t".as_slice()].concat();
    let exact = ReaderConfig::default().max_input_bytes(input.len() as u64);
    let outcomes = assert_same_on_both_paths(&input, exact);
    assert_eq!(outcomes.segments, (outcomes.segments.0.clone(), None));
    assert_eq!(outcomes.segments.0.len(), 1);

    // One byte less and the trailing whitespace is over budget.
    let short = (input.len() - 1) as u64;
    let outcomes =
        assert_same_on_both_paths(&input, ReaderConfig::default().max_input_bytes(short));
    assert_eq!(outcomes.segments.0.len(), 1);
    assert_eq!(
        assert_limit_error(
            outcomes.segments.1.as_ref().expect("E036"),
            "max_input_bytes",
            short
        ),
        input.len() - 1
    );
}

#[test]
fn malformed_unit_beyond_budget_is_e036() {
    let input = [UNB, b"BGM+1'bgm+broken-segment'".as_slice(), UNZ].concat();
    let bad = UNB.len() + b"BGM+1'".len();

    // The budget ends inside the malformed segment: the stream runs into the
    // budget, whatever is wrong with the bytes beyond it.
    let budget = (bad + 5) as u64;
    let outcomes =
        assert_same_on_both_paths(&input, ReaderConfig::default().max_input_bytes(budget));
    assert_eq!(outcomes.segments.0.len(), 2);
    assert_eq!(
        assert_limit_error(
            outcomes.segments.1.as_ref().expect("E036"),
            "max_input_bytes",
            budget
        ),
        bad + 5
    );

    // With the whole segment inside the budget the syntax error is genuine.
    let budget = input.len() as u64;
    let outcomes =
        assert_same_on_both_paths(&input, ReaderConfig::default().max_input_bytes(budget));
    assert_eq!(
        outcomes.segments.1.as_ref().map(EdifactError::stable_code),
        Some("E006")
    );
}

#[test]
fn segment_too_long_vs_budget_precedence() {
    // A segment longer than `max_segment_bytes` (20) is detected once its
    // first 21 bytes have been read: at byte 6 + 21 = 27.
    let input = [b"BGM+1'FTX+".as_slice(), &[b'X'; 100], b"'"].concat();
    let config = ReaderConfig::default().max_segment_bytes(20);

    // Detection inside the budget: E020.
    let outcomes = assert_same_on_both_paths(&input, config.max_input_bytes(50));
    match outcomes.segments.1 {
        Some(EdifactError::SegmentTooLong {
            offset: 6,
            limit: 20,
        }) => {}
        ref other => panic!("expected E020 at 6, got {other:?}"),
    }
    // Exactly at the detection point: still E020.
    let outcomes = assert_same_on_both_paths(&input, config.max_input_bytes(27));
    assert_eq!(
        outcomes.segments.1.as_ref().map(EdifactError::stable_code),
        Some("E020")
    );

    // The budget ends first: E036.
    for budget in [16_u64, 26] {
        let outcomes = assert_same_on_both_paths(&input, config.max_input_bytes(budget));
        assert_eq!(
            assert_limit_error(
                outcomes.segments.1.as_ref().expect("E036"),
                "max_input_bytes",
                budget
            ),
            budget as usize
        );
    }
}

#[cfg(target_pointer_width = "64")]
#[test]
fn peak_heap_bounded_by_limits_whitespace() {
    let config = ReaderConfig::default().max_input_bytes(1_000);
    let (peak, items, error) = peak_heap(UNB, b" ", HARD_STOP, |generator| {
        from_reader_with_config(generator, config)
    });
    assert_eq!(items, 1);
    assert_limit_error(&error.expect("E036"), "max_input_bytes", 1_000);
    assert!(peak <= 64 << 10, "peak heap {peak} B exceeds 64 KiB");
}
