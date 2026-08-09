//! ISO 9735-4 §3.1 repeating data elements.
//!
//! Syntax version 4 lets a `UNA` declare a repetition separator at position 7,
//! turning `RFF+ON:1*ON:2` into one data element with two occurrences. Before
//! repetition was implemented the separator was stored but never acted on, so
//! that segment parsed as a single occurrence whose second component was the
//! literal text `1*ON` — wrong data, delivered without a word of warning.
//!
//! A space at UNA position 7 is the "not used" sentinel, which is why every
//! interchange without an explicit separator keeps the flat behaviour.

use edifact_rs::{
    EdifactError, Element, Segment, ServiceStringAdvice, Writer, from_bytes, from_bytes_owned,
    from_reader_collect, segments_to_bytes,
};

/// `UNA` declaring `*` as the repetition separator; everything else default.
const REP_UNA: &str = "UNA:+.?*'";

fn parse(input: &[u8]) -> Vec<Segment<'_>> {
    from_bytes(input)
        .collect::<Result<_, _>>()
        .expect("input must parse")
}

// ── parsing ──────────────────────────────────────────────────────────────────

#[test]
fn a_declared_separator_splits_the_element_into_repetitions() {
    let input = format!("{REP_UNA}RFF+ON:1*ON:2*ON:3'");
    let segments = parse(input.as_bytes());
    let rff = segments[0].get_element(0).expect("RFF has one element");

    assert_eq!(rff.repeat_count(), 3);
    let values: Vec<Vec<&str>> = rff
        .repetitions()
        .map(|components| components.iter().map(|(c, _)| c.as_ref()).collect())
        .collect();
    assert_eq!(values, vec![["ON", "1"], ["ON", "2"], ["ON", "3"]]);
}

#[test]
fn the_first_repetition_is_what_positional_accessors_see() {
    // Every existing accessor keeps reading occurrence 0, so code written
    // against non-repeating interchanges behaves identically.
    let input = format!("{REP_UNA}RFF+ON:1*ON:2'");
    let segments = parse(input.as_bytes());
    assert_eq!(segments[0].component_str(0, 0), Some("ON"));
    assert_eq!(segments[0].component_str(0, 1), Some("1"));
}

#[test]
fn without_a_declared_separator_the_byte_is_ordinary_data() {
    // No UNA at all: position 7 defaults to the space sentinel.
    let segments = parse(b"RFF+ON:1*ON:2'");
    let rff = segments[0].get_element(0).unwrap();
    assert_eq!(rff.repeat_count(), 1, "no separator declared, no split");
    assert_eq!(rff.get_component(1), Some("1*ON"));
}

#[test]
fn an_escaped_separator_stays_inside_the_value() {
    let input = format!("{REP_UNA}FTX+a?*b'");
    let segments = parse(input.as_bytes());
    let ftx = segments[0].get_element(0).unwrap();
    assert_eq!(ftx.repeat_count(), 1);
    assert_eq!(ftx.get_component(0), Some("a*b"));
}

#[test]
fn repetitions_carry_their_own_spans() {
    let input = format!("{REP_UNA}RFF+A*B'");
    let segments = parse(input.as_bytes());
    let rff = segments[0].get_element(0).unwrap();

    let first = rff.repetition(0).unwrap()[0].1;
    let second = rff.repetition(1).unwrap()[0].1;
    assert_eq!(&input[first.start..first.end], "A");
    assert_eq!(&input[second.start..second.end], "B");
    // The element span covers every repetition, not just the first.
    assert_eq!(rff.span.end, second.end);
}

#[test]
fn a_separator_before_any_element_is_rejected() {
    // There is nothing to repeat, so this is a protocol violation rather than a
    // silently-empty element.
    let input = format!("{REP_UNA}RFF*A'");
    let err = from_bytes(input.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect_err("a leading repetition separator is malformed");
    // `RFF*A` reads as a five-character tag, which fails before the separator
    // is ever reached — either way the input is refused, never mis-parsed.
    assert!(
        matches!(
            err,
            EdifactError::InvalidSegmentTag(_) | EdifactError::UnexpectedDataToken { .. }
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn the_reader_path_agrees_with_the_slice_path() {
    let input = format!("{REP_UNA}RFF+ON:1*ON:2'");

    let owned = from_reader_collect(std::io::Cursor::new(input.as_bytes()))
        .expect("reader path must parse");
    let borrowed = parse(input.as_bytes());

    assert_eq!(owned.len(), borrowed.len());
    assert_eq!(owned[0].elements[0].repeat_count(), 2);
    let owned_values: Vec<Vec<&str>> = owned[0].elements[0]
        .repetitions()
        .map(|r| r.iter().map(|(c, _)| c.as_str()).collect())
        .collect();
    assert_eq!(owned_values, vec![["ON", "1"], ["ON", "2"]]);
}

#[test]
fn owned_conversion_preserves_repetitions() {
    let input = format!("{REP_UNA}RFF+ON:1*ON:2'");
    let owned: Vec<_> = from_bytes_owned(input.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");
    assert_eq!(owned[0].elements[0].repeat_count(), 2);
    // …and converting back to a borrowed view keeps them.
    assert_eq!(owned[0].as_borrowed().elements[0].repeat_count(), 2);
    assert_eq!(owned[0].borrow().get_element(0).unwrap().repeat_count(), 2);
}

// ── writing and round-trip ───────────────────────────────────────────────────

fn rep_ssa() -> ServiceStringAdvice {
    ServiceStringAdvice {
        repetition_sep: b'*',
        ..ServiceStringAdvice::default()
    }
}

#[test]
fn the_writer_emits_repetitions_with_the_active_separator() {
    let mut buf = Vec::new();
    {
        let mut w = Writer::with_una(&mut buf, rep_ssa()).expect("valid UNA");
        let seg = Segment::new(
            "RFF",
            vec![Element::of(&["ON", "1"]).and_repeat(&["ON", "2"])],
        );
        w.write_segment(&seg).expect("write");
    }
    let out = String::from_utf8(buf).expect("utf-8");
    assert!(out.ends_with("RFF+ON:1*ON:2'"), "got {out}");
}

#[test]
fn a_repeating_segment_round_trips_through_its_own_output() {
    let mut buf = Vec::new();
    {
        let mut w = Writer::with_una(&mut buf, rep_ssa()).expect("valid UNA");
        let seg = Segment::new(
            "RFF",
            vec![
                Element::of(&["ON", "1"])
                    .and_repeat(&["ON", "2"])
                    .and_repeat(&["ON", "3"]),
            ],
        );
        w.write_segment(&seg).expect("write");
    }

    let reparsed = parse(&buf);
    let element = reparsed[0].get_element(0).unwrap();
    assert_eq!(element.repeat_count(), 3);
    assert_eq!(element.repetition(2).unwrap()[1].0.as_ref(), "3");
}

#[test]
fn a_value_containing_the_separator_survives_a_round_trip() {
    // The writer escapes `*` inside a value; the tokenizer must not then treat
    // the unescaped result as a repetition boundary.
    let mut buf = Vec::new();
    {
        let mut w = Writer::with_una(&mut buf, rep_ssa()).expect("valid UNA");
        w.write_segment(&Segment::new("FTX", vec![Element::of(&["a*b"])]))
            .expect("write");
    }
    let reparsed = parse(&buf);
    let ftx = reparsed[0].get_element(0).unwrap();
    assert_eq!(ftx.repeat_count(), 1);
    assert_eq!(ftx.get_component(0), Some("a*b"));
}

#[test]
fn writing_a_repetition_without_a_declared_separator_is_refused() {
    // With the space sentinel there is no byte to write between occurrences.
    // Emitting the space anyway produced `RFF+ON:1 ON:2'`, which reads back as a
    // *single* occurrence whose second component is `1 ON` — corrupt output from
    // a call that reported success.
    let seg = Segment::new(
        "RFF",
        vec![Element::of(&["ON", "1"]).and_repeat(&["ON", "2"])],
    );
    let err = segments_to_bytes(std::slice::from_ref(&seg))
        .expect_err("a repetition needs a declared separator");
    assert!(
        matches!(err, EdifactError::RepetitionSeparatorNotDeclared),
        "unexpected error: {err:?}"
    );

    // A non-repeating segment is unaffected.
    let plain = Segment::new("RFF", vec![Element::of(&["ON", "1"])]);
    assert_eq!(
        segments_to_bytes(std::slice::from_ref(&plain)).expect("write"),
        b"RFF+ON:1'".to_vec()
    );
}

/// The reader parses each segment from its own zero-based slice and then rebases
/// the spans onto the stream.  `OwnedSegment::offset` walked `components` but
/// not `repeats`, so every occurrence after the first kept its segment-relative
/// span and pointed into the wrong part of the input.
#[test]
fn reader_rebases_repetition_spans_onto_the_stream() {
    let input = b"UNA:+.?*'RFF+ON:1*ON:2'RFF+AAA:9*AAA:8'";

    let borrowed: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("slice parse");
    let owned: Vec<edifact_rs::OwnedSegment> =
        edifact_rs::from_reader_collect(std::io::Cursor::new(&input[..])).expect("reader parse");

    assert_eq!(borrowed.len(), owned.len());
    for (b, o) in borrowed.iter().zip(&owned) {
        for (be, oe) in b.elements.iter().zip(&o.elements) {
            assert_eq!(be.span, oe.span, "element span for {}", b.tag);
            for (rep, (br, or)) in be.repeats.iter().zip(&oe.repeats).enumerate() {
                for (bc, oc) in br.iter().zip(or.iter()) {
                    assert_eq!(
                        bc.1,
                        oc.1,
                        "{} repetition {} component span",
                        b.tag,
                        rep + 1
                    );
                }
            }
        }
    }

    // And the span must actually address the bytes it claims to.
    let second = owned[0].elements[0].repeats[0][1].1;
    assert_eq!(&input[second.start..second.end], b"2");
}

/// Rejecting a repeating element mid-write left `RFF+` in the sink, so a caller
/// that logged the error and carried on emitted a corrupt interchange.
#[test]
fn a_rejected_repetition_writes_no_bytes_at_all() {
    use edifact_rs::Writer;

    let seg = Segment::new(
        "RFF",
        vec![Element::of(&["ON", "1"]).and_repeat(&["ON", "2"])],
    );
    let mut buf = Vec::new();
    {
        let mut writer = Writer::new(&mut buf);
        let err = writer
            .write_segment(&seg)
            .expect_err("a repetition needs a declared separator");
        assert!(matches!(err, EdifactError::RepetitionSeparatorNotDeclared));
        // The writer stays usable, and the next segment is not corrupted by a
        // dangling `RFF+` prefix.
        writer
            .write_segment(&Segment::new("BGM", vec![Element::of(&["220"])]))
            .expect("write after a rejected segment");
    }
    assert_eq!(buf, b"BGM+220'");
}
