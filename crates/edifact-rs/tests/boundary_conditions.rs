//! Boundary-condition regression locks.
//!
//! These inputs sit at the edges of the parsing contract — empty input, a UNA
//! with no segments, lone delimiters, nested release sequences, multi-byte
//! UTF-8, and segments at exactly the configured size limit.  The behaviour is
//! correct today; these tests pin it so it stays that way.

use edifact_rs::{
    EdifactError, ReaderConfig, ServiceStringAdvice, Writer, from_bytes, from_bytes_with_config,
    segments_to_bytes, validate_envelope,
};

fn parse(input: &[u8]) -> Result<Vec<edifact_rs::Segment<'_>>, EdifactError> {
    from_bytes(input).collect()
}

// ── empty and near-empty input ────────────────────────────────────────────────

#[test]
fn empty_input_yields_no_segments() {
    assert_eq!(parse(b"").expect("empty input is not an error").len(), 0);
    assert_eq!(
        edifact_rs::from_reader(std::io::Cursor::new(b"".as_slice()))
            .collect::<Result<Vec<_>, _>>()
            .expect("empty reader is not an error")
            .len(),
        0
    );
}

#[test]
fn empty_segment_list_fails_envelope_validation() {
    let err = validate_envelope(&[]).expect_err("an empty interchange has no UNB");
    assert!(
        matches!(err, EdifactError::MissingSegment { .. }),
        "{err:?}"
    );
}

#[test]
fn una_only_input_yields_no_segments() {
    // The UNA header is consumed by the tokenizer and is not itself a segment.
    let segs = parse(b"UNA:+.? '").expect("UNA-only input must parse");
    assert!(segs.is_empty(), "UNA must not surface as a segment");
}

#[test]
fn truncated_una_is_not_treated_as_a_service_string_header() {
    // A UNA header is exactly 9 bytes.  Anything shorter is not a header, so the
    // default delimiters stay in force and the bytes are read as an ordinary
    // segment tag — one that here has no terminator, and is rejected as the
    // truncation it is rather than accepted as a complete segment.
    assert!(matches!(
        parse(b"UNA").unwrap_err(),
        EdifactError::UnexpectedEof { .. }
    ));

    // Terminated, it is an ordinary element-less segment …
    let segs = parse(b"UNA'").expect("a terminated 3-letter tag is a segment");
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].tag, "UNA");
    assert!(segs[0].elements.is_empty());

    // … and the default delimiters remain active, so `+` still splits elements.
    let segs = parse(b"UNA+X'").expect("default delimiters still apply");
    assert_eq!(segs[0].element_str(0), Some("X"));
}

// ── lone and stray delimiters ─────────────────────────────────────────────────

#[test]
fn lone_delimiters_are_rejected_with_precise_errors() {
    assert!(matches!(
        parse(b"'").unwrap_err(),
        EdifactError::InvalidDelimiter { offset: 0, .. }
    ));
    assert!(matches!(
        parse(b"+").unwrap_err(),
        EdifactError::InvalidDelimiter { offset: 0, .. }
    ));
    assert!(matches!(
        parse(b"?").unwrap_err(),
        EdifactError::InvalidSegmentTag(_)
    ));
}

#[test]
fn consecutive_terminators_are_rejected_not_silently_skipped() {
    // An empty segment is malformed; it is reported with the offset of the
    // offending terminator rather than being quietly dropped.
    let err = parse(b"BGM+220''RFF+ON:1'").expect_err("empty segment must be rejected");
    assert!(
        matches!(err, EdifactError::InvalidDelimiter { offset: 8, .. }),
        "{err:?}"
    );
}

#[test]
fn inter_segment_whitespace_is_skipped() {
    // Line breaks and indentation between segments are tolerated, unlike empty
    // segments — pretty-printed interchanges are common in practice.
    let segs = parse(b"BGM+220'\r\n  RFF+ON:1'\n").expect("whitespace is skipped");
    assert_eq!(
        segs.iter().map(|s| s.tag()).collect::<Vec<_>>(),
        vec!["BGM", "RFF"]
    );
}

#[test]
fn segment_without_elements_parses() {
    let segs = parse(b"UNZ'UNB+A'").expect("element-less segments are legal syntax");
    assert_eq!(
        segs.iter().map(|s| s.tag()).collect::<Vec<_>>(),
        vec!["UNZ", "UNB"]
    );
    assert!(segs[0].elements.is_empty());
}

// ── release sequences ─────────────────────────────────────────────────────────

#[test]
fn nested_release_sequences_resolve_pairwise() {
    // `??` is one escaped release char; `???+` is an escaped release char
    // followed by an escaped element separator.
    let segs = parse(b"FTX+a??b+c???+d'").expect("nested releases must parse");
    assert_eq!(segs[0].element_str(0), Some("a?b"));
    assert_eq!(segs[0].element_str(1), Some("c?+d"));
}

#[test]
fn release_escaping_every_service_character_round_trips() {
    for value in ["a+b", "a:b", "a?b", "a'b", "?", "??", "+:?'"] {
        let seg = edifact_rs::Segment::new("FTX", vec![edifact_rs::Element::of(&[value])]);
        let bytes = segments_to_bytes(std::slice::from_ref(&seg)).expect("write");
        let back = parse(&bytes).expect("re-parse own output");
        assert_eq!(
            back[0].element_str(0),
            Some(value),
            "round-trip lost data for {value:?} (wire form {:?})",
            String::from_utf8_lossy(&bytes)
        );
    }
}

#[test]
fn dangling_release_at_end_of_input_is_an_error() {
    assert!(matches!(
        parse(b"FTX+abc?").unwrap_err(),
        EdifactError::InvalidReleaseSequence { .. }
    ));
}

// ── zero-length elements and components ───────────────────────────────────────

#[test]
fn empty_elements_and_components_are_preserved() {
    let segs = parse(b"NAD++::X'").expect("empty slots must parse");
    assert_eq!(segs[0].element_str(0), Some(""));
    let composite = segs[0].get_element(1).expect("second element");
    assert_eq!(composite.get_component(0), Some(""));
    assert_eq!(composite.get_component(1), Some(""));
    assert_eq!(composite.get_component(2), Some("X"));
}

// ── non-ASCII / UTF-8 ─────────────────────────────────────────────────────────

#[test]
fn multibyte_utf8_values_round_trip() {
    // The writer inserts single ASCII release bytes into a UTF-8 buffer; a
    // multi-byte value must survive that untouched.
    let value = "Grüße — 北京 🌍";
    let seg = edifact_rs::Segment::new("FTX", vec![edifact_rs::Element::of(&[value])]);
    let bytes = segments_to_bytes(std::slice::from_ref(&seg)).expect("write");
    let back = parse(&bytes).expect("re-parse");
    assert_eq!(back[0].element_str(0), Some(value));
}

#[test]
fn multibyte_utf8_next_to_an_escaped_delimiter_round_trips() {
    let value = "ü+ß:é?ñ'ö";
    let seg = edifact_rs::Segment::new("FTX", vec![edifact_rs::Element::of(&[value])]);
    let bytes = segments_to_bytes(std::slice::from_ref(&seg)).expect("write");
    assert_eq!(
        parse(&bytes).expect("re-parse")[0].element_str(0),
        Some(value)
    );
}

#[test]
fn invalid_utf8_is_rejected_not_silently_replaced() {
    // 0xFF is never valid UTF-8.
    let err = parse(b"FTX+ab\xFFcd'").expect_err("invalid UTF-8 must be rejected");
    assert!(
        matches!(
            err,
            EdifactError::InvalidText { .. } | EdifactError::InvalidSegmentTag(_)
        ),
        "{err:?}"
    );
}

#[test]
fn utf8_split_across_a_reader_chunk_boundary_is_handled() {
    let input = "FTX+Grüße北京'".as_bytes();
    for capacity in 1..=input.len() {
        let reader = std::io::BufReader::with_capacity(capacity, std::io::Cursor::new(input));
        let segs = edifact_rs::from_reader(reader)
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|e| panic!("failed at buffer capacity {capacity}: {e}"));
        assert_eq!(segs[0].elements[0].components[0].0, "Grüße北京");
    }
}

// ── segment size limits ───────────────────────────────────────────────────────

/// Build `BGM+<padding>'` whose body (tag + elements, excluding the terminator)
/// is exactly `body_len` bytes.
fn segment_of_body_len(body_len: usize) -> Vec<u8> {
    let mut v = b"BGM+".to_vec();
    v.resize(body_len, b'X');
    v.push(b'\'');
    v
}

#[test]
fn segment_at_exactly_the_limit_is_accepted() {
    let limit = 1_024;
    let input = segment_of_body_len(limit);
    let cfg = ReaderConfig::default().max_segment_bytes(limit);
    from_bytes_with_config(&input, cfg)
        .collect::<Result<Vec<_>, _>>()
        .expect("a segment of exactly max_segment_bytes must be accepted");
}

#[test]
fn segment_one_byte_over_the_limit_is_rejected() {
    let limit = 1_024;
    let input = segment_of_body_len(limit + 1);
    let cfg = ReaderConfig::default().max_segment_bytes(limit);
    let err = from_bytes_with_config(&input, cfg)
        .collect::<Result<Vec<_>, _>>()
        .expect_err("one byte over the limit must be rejected");
    assert!(matches!(err, EdifactError::SegmentTooLong { limit: l, .. } if l == limit));
}

#[test]
fn slice_and_reader_paths_agree_at_the_limit_boundary() {
    // The two paths used to disagree by one byte, so a segment of exactly
    // `max_segment_bytes` parsed or failed depending only on whether it happened
    // to straddle a read-buffer boundary.
    let limit = 512;
    for body_len in [limit - 1, limit, limit + 1] {
        let input = segment_of_body_len(body_len);
        let cfg = ReaderConfig::default().max_segment_bytes(limit);
        let slice_ok = from_bytes_with_config(&input, cfg)
            .collect::<Result<Vec<_>, _>>()
            .is_ok();
        let reader_ok = edifact_rs::from_bufread_with_config(
            std::io::BufReader::with_capacity(64, std::io::Cursor::new(&input)),
            cfg,
        )
        .collect::<Result<Vec<_>, _>>()
        .is_ok();
        assert_eq!(
            slice_ok, reader_ok,
            "slice and reader paths disagreed at body_len={body_len} (limit={limit})"
        );
    }
}

#[test]
fn default_segment_limit_is_enforced_on_unterminated_input() {
    // 64 KiB default: an unterminated segment must fail rather than scan on.
    let mut input = b"BGM+".to_vec();
    input.resize(200_000, b'X');
    let err = parse(&input).expect_err("unterminated oversized segment must be rejected");
    assert!(
        matches!(err, EdifactError::SegmentTooLong { .. }),
        "{err:?}"
    );
}

// ── custom UNA edge cases ─────────────────────────────────────────────────────

#[test]
fn alphanumeric_delimiters_are_rejected() {
    // A letter delimiter would collide with segment-tag characters, which are
    // written verbatim and cannot be escaped.
    let ssa = ServiceStringAdvice {
        segment_term: b'N',
        ..ServiceStringAdvice::default()
    };
    assert!(!ssa.is_valid());
    assert!(Writer::with_una(Vec::new(), ssa).is_err());
}

#[test]
fn duplicate_delimiters_are_rejected() {
    let err = parse(b"UNA::.? 'BGM:220'").expect_err("component_sep == element_sep");
    assert!(matches!(err, EdifactError::InvalidUna));
}

#[test]
fn space_repetition_separator_sentinel_is_accepted() {
    // Space at UNA position 7 means "not used".
    parse(b"UNA:+.? 'BGM+220'").expect("the conventional UNA must parse");
}
