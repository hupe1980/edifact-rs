//! Character-repertoire handling: `UNB` S001 DE 0001 decoding, encoding, and
//! validation.
//!
//! The case that motivates the whole feature: **UTF-8 is not a superset of
//! `UNOC`**. A German `ORDERS` carrying `Müller` is a conformant ISO 8859-1
//! interchange whose `ü` is the single byte `0xFC` — not valid UTF-8, and
//! therefore unparseable by a UTF-8-only reader.

use edifact_rs::{
    Charset, EdifactError, ValidationContext, Writer, decode_interchange, from_bytes,
    from_reader_collect, sniff_charset,
};

/// `UNB+UNOC:3+…+NAD+BY+Müller` with `ü` as the raw Latin-1 byte `0xFC`.
fn unoc_interchange() -> Vec<u8> {
    let mut raw = b"UNB+UNOC:3+SENDER+RECEIVER+200101:0900+IC1'NAD+BY+M".to_vec();
    raw.push(0xFC);
    raw.extend_from_slice(b"ller'UNZ+0+IC1'");
    raw
}

#[test]
fn a_latin1_interchange_is_unparseable_until_it_is_decoded() {
    let raw = unoc_interchange();

    // Straight through the UTF-8 reader: the lone 0xFC is not valid UTF-8.
    let err = from_bytes(&raw)
        .collect::<Result<Vec<_>, _>>()
        .expect_err("0xFC is not valid UTF-8");
    assert!(matches!(err, EdifactError::InvalidText { .. }), "{err:?}");

    // Decoded first: the repertoire comes out of the interchange's own UNB.
    let utf8 = decode_interchange(&raw).expect("decode");
    let segments: Vec<_> = from_bytes(&utf8)
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");
    assert_eq!(segments[1].element_str(1), Some("Müller"));
}

#[test]
fn sniffing_reads_the_syntax_identifier_without_decoding_the_body() {
    // The sender name carries a Latin-1 byte, so anything that parses the whole
    // UNB as UTF-8 before learning the repertoire would fail here.
    let mut raw = b"UNB+UNOC:3+M".to_vec();
    raw.push(0xDC); // Ü
    raw.extend_from_slice(b"LLER+R+200101:0900+IC1'UNZ+0+IC1'");

    assert_eq!(sniff_charset(&raw).unwrap(), Some(Charset::UnoC));
}

#[test]
fn sniffing_honours_a_custom_una() {
    let raw = b"UNA:+.? 'UNB+UNOD:3+S+R+200101:0900+IC1'UNZ+0+IC1'";
    assert_eq!(sniff_charset(raw).unwrap(), Some(Charset::UnoD));

    // component_sep `|`, element_sep `!`, decimal `,`, release `#`, rep `*`,
    // terminator `~` — the composite separators must follow the UNA, not the
    // defaults.
    let exotic = b"UNA|!,#*~UNB!UNOE|3!S!R!200101|0900!IC1~UNZ!0!IC1~";
    assert_eq!(sniff_charset(exotic).unwrap(), Some(Charset::UnoE));
}

#[test]
fn an_input_without_a_unb_is_returned_untouched() {
    let bare = b"UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'";
    assert_eq!(sniff_charset(bare).unwrap(), None);
    assert!(matches!(
        decode_interchange(bare).unwrap(),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn an_ascii_interchange_is_never_copied() {
    let ascii = b"UNB+UNOC:3+S+R+200101:0900+IC1'BGM+220'UNZ+0+IC1'";
    // UNOC, but every byte is ASCII — the zero-copy path must survive.
    assert!(
        matches!(
            decode_interchange(ascii).unwrap(),
            std::borrow::Cow::Borrowed(_)
        ),
        "an all-ASCII payload must not be reallocated"
    );
}

#[test]
fn stateful_and_multibyte_repertoires_are_refused_rather_than_mis_decoded() {
    for identifier in ["UNOX", "KECA"] {
        let err = Charset::from_syntax_identifier(identifier).unwrap_err();
        assert!(
            matches!(err, EdifactError::UnsupportedCharset { .. }),
            "{identifier}: {err:?}"
        );
        assert_eq!(err.stable_code(), "E039");
    }
    let err = Charset::from_syntax_identifier("NOPE").unwrap_err();
    assert!(matches!(err, EdifactError::UnrecognisedSyntaxIdentifier(_)));
}

#[test]
fn every_supported_repertoire_round_trips_its_own_characters() {
    // One representative non-ASCII character per single-byte repertoire.
    let cases = [
        (Charset::UnoC, 'ü'),
        (Charset::UnoD, 'ř'),
        (Charset::UnoE, 'Я'),
        (Charset::UnoF, 'π'),
        (Charset::UnoG, 'ġ'),
        (Charset::UnoH, 'ā'),
        (Charset::UnoI, 'ب'),
        (Charset::UnoJ, 'א'),
        (Charset::UnoK, 'ğ'),
    ];
    for (charset, ch) in cases {
        let text = format!("A{ch}Z");
        assert!(charset.permits(ch), "{charset} should permit {ch:?}");

        let encoded = charset.encode(&text).expect("encode");
        assert_eq!(encoded.len(), 3, "{charset}: expected one byte per char");

        let decoded = charset.decode(&encoded).expect("decode");
        assert_eq!(decoded, text, "{charset} round-trip");
    }
}

#[test]
fn level_a_and_level_b_repertoires_match_iso_9735() {
    // Level A: upper case, digits, space, and a fixed punctuation set.
    assert!(Charset::UnoA.permits('A'));
    assert!(Charset::UnoA.permits('7'));
    assert!(Charset::UnoA.permits(' '));
    for punctuation in ".,-()/='+:?!\"%&*;<>".chars() {
        assert!(Charset::UnoA.permits(punctuation), "{punctuation:?}");
    }
    assert!(!Charset::UnoA.permits('a'));
    assert!(!Charset::UnoA.permits('#'));
    assert!(!Charset::UnoA.permits('ü'));

    // Level B adds lower case and nothing else.
    assert!(Charset::UnoB.permits('a'));
    assert!(!Charset::UnoB.permits('ü'));
    assert!(!Charset::UnoB.permits('#'));

    // UNOY permits everything.
    assert!(Charset::UnoY.permits('€'));
    assert!(
        Charset::UnoY
            .first_violation("anything at all — 漢字")
            .is_none()
    );
}

#[test]
fn a_character_outside_the_repertoire_reports_its_offset() {
    assert_eq!(Charset::UnoA.first_violation("ORDER"), None);
    assert_eq!(Charset::UnoA.first_violation("ORDer"), Some((3, 'e')));
    // Offsets are byte offsets, so a multi-byte prefix shifts them.
    assert_eq!(Charset::UnoC.first_violation("Mü€"), Some((3, '€')));
}

#[test]
fn the_decoding_reader_matches_whole_slice_decoding() {
    let raw = unoc_interchange();
    let expected = decode_interchange(&raw).expect("decode");

    // Exercise every buffer size, including ones that split a two-byte UTF-8
    // sequence across two `read` calls.
    for capacity in 1..=16 {
        let reader = std::io::BufReader::with_capacity(
            capacity,
            Charset::UnoC.decoding_reader(std::io::Cursor::new(raw.clone())),
        );
        let mut got = Vec::new();
        std::io::Read::read_to_end(&mut { reader }, &mut got).expect("read");
        assert_eq!(got, expected.as_ref(), "capacity {capacity}");
    }
}

#[test]
fn the_decoding_reader_feeds_the_streaming_parser() {
    let raw = unoc_interchange();
    let segments = from_reader_collect(Charset::UnoC.decoding_reader(std::io::Cursor::new(raw)))
        .expect("parse");
    assert_eq!(segments[1].element_str(1), Some("Müller"));
}

#[test]
fn an_undefined_byte_in_a_repertoire_is_an_error_not_a_replacement_character() {
    // 0xA1 is an undefined slot in ISO 8859-6 (UNOI).
    let err = Charset::UnoI
        .decode(b"AB\xA1CD")
        .expect_err("undefined slot");
    assert!(
        matches!(err, EdifactError::InvalidText { offset: 2 }),
        "{err:?}"
    );
}

// ── writer side ───────────────────────────────────────────────────────────────

#[test]
fn a_charset_bound_writer_emits_single_byte_values() {
    let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoC);
    writer
        .write_composites("NAD", &[&["BY"], &["Müller"]])
        .expect("write");
    let wire = writer.finish().expect("finish");

    assert_eq!(wire, b"NAD+BY+M\xFCller'".to_vec());
    // And it reads back through the matching decoder.
    let decoded = Charset::UnoC.transcode_to_utf8(&wire).expect("decode");
    let segments: Vec<_> = from_bytes(&decoded)
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");
    assert_eq!(segments[0].element_str(1), Some("Müller"));
}

#[test]
fn a_value_outside_the_repertoire_is_refused() {
    let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoA);
    let err = writer
        .write_composites("NAD", &[&["BY"], &["Müller"]])
        .expect_err("level A cannot carry ü");
    assert!(
        matches!(
            err,
            EdifactError::CharacterNotInRepertoire {
                character: 'ü', ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(err.stable_code(), "E038");
}

#[test]
fn escaping_survives_encoding() {
    // The escaped byte is a service character (ASCII) but the surrounding text
    // is not — both halves must come out right.
    let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoC);
    writer
        .write_composites("FTX", &[&["Müller+Söhne"]])
        .expect("write");
    let wire = writer.finish().expect("finish");
    assert_eq!(wire, b"FTX+M\xFCller?+S\xF6hne'".to_vec());

    let decoded = Charset::UnoC.transcode_to_utf8(&wire).expect("decode");
    let segments: Vec<_> = from_bytes(&decoded)
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");
    assert_eq!(segments[0].element_str(0), Some("Müller+Söhne"));
}

#[test]
fn a_header_that_lies_about_the_body_is_refused() {
    let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoC);
    let err = writer
        .begin_interchange("UNOA", "3", "S", "R", "200101", "0900", "IC1")
        .expect_err("UNB must not declare a repertoire the writer does not encode");
    assert!(
        matches!(err, EdifactError::CharacterRepertoireMismatch { .. }),
        "{err:?}"
    );
    assert_eq!(err.stable_code(), "E041");

    // The writer's own identifier is always accepted.
    let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoC);
    writer
        .begin_interchange("UNOC", "3", "S", "R", "200101", "0900", "IC1")
        .expect("matching identifier");
}

// ── validation layer ──────────────────────────────────────────────────────────

#[test]
fn the_validator_reports_values_outside_the_declared_repertoire() {
    // UNOA is upper-case only, but the party name is mixed case.
    let segments: Vec<_> = from_bytes(b"UNB+UNOA:3+S+R+200101:0900+IC1'NAD+BY+Acme Ltd'UNZ+0+IC1'")
        .collect::<Result<_, _>>()
        .expect("parse");

    let report = ValidationContext::builder()
        .with_charset_validation()
        .build()
        .validate_lenient(&segments);

    let issue = report
        .errors()
        .iter()
        .find(|i| i.error_code() == Some("E038"))
        .expect("expected a repertoire finding");
    assert_eq!(issue.segment_tag.as_deref(), Some("NAD"));
    assert_eq!(issue.element_index, Some(1));
    // The span points at the offending value, not the whole segment.
    let span = issue.span.expect("span");
    assert_eq!(
        &b"UNB+UNOA:3+S+R+200101:0900+IC1'NAD+BY+Acme Ltd'UNZ+0+IC1'"[span.start..span.end],
        b"Acme Ltd"
    );
}

#[test]
fn a_conformant_interchange_produces_no_repertoire_findings() {
    let segments: Vec<_> = from_bytes(b"UNB+UNOA:3+S+R+200101:0900+IC1'NAD+BY+ACME LTD'UNZ+0+IC1'")
        .collect::<Result<_, _>>()
        .expect("parse");

    let report = ValidationContext::builder()
        .with_charset_validation()
        .build()
        .validate_lenient(&segments);

    assert!(
        !report
            .errors()
            .iter()
            .any(|i| i.error_code() == Some("E038")),
        "{:#?}",
        report.errors()
    );
}

#[test]
fn a_pinned_repertoire_applies_to_message_level_slices() {
    // No UNB, so nothing declares a repertoire — the pinned one still applies.
    let segments: Vec<_> = from_bytes(b"UNH+1+ORDERS:D:96A:UN'NAD+BY+Acme'UNT+3+1'")
        .collect::<Result<_, _>>()
        .expect("parse");

    let unchecked = ValidationContext::builder()
        .with_charset_validation()
        .build()
        .validate_lenient(&segments);
    assert!(
        !unchecked
            .errors()
            .iter()
            .any(|i| i.error_code() == Some("E038"))
    );

    let pinned = ValidationContext::builder()
        .with_charset_validation_for(Charset::UnoA)
        .build()
        .validate_lenient(&segments);
    assert!(
        pinned
            .errors()
            .iter()
            .any(|i| i.error_code() == Some("E038"))
    );
}

#[test]
fn repetitions_are_checked_too() {
    // The second occurrence is the one that violates level A.
    let segments: Vec<_> =
        from_bytes(b"UNA:+.?*'UNB+UNOA:4+S+R+200101:0900+IC1'RFF+ON:AAA*ON:bbb'UNZ+0+IC1'")
            .collect::<Result<_, _>>()
            .expect("parse");

    let report = ValidationContext::builder()
        .with_charset_validation()
        .build()
        .validate_lenient(&segments);

    assert!(
        report
            .errors()
            .iter()
            .any(|i| i.error_code() == Some("E038") && i.segment_tag.as_deref() == Some("RFF")),
        "a violation in a further repetition must still be reported: {:#?}",
        report.errors()
    );
}

// ── non-finite numbers ────────────────────────────────────────────────────────

#[test]
fn non_finite_floats_are_refused() {
    use edifact_rs::{EdifactSerialize, VecEmitter, ser::DecimalFloat};

    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut emitter = VecEmitter::default();
        let err = DecimalFloat(value)
            .edifact_serialize(&mut emitter)
            .expect_err("NaN and infinity have no EDIFACT representation");
        assert!(
            matches!(err, EdifactError::NonFiniteNumber { .. }),
            "{err:?}"
        );
        assert_eq!(err.stable_code(), "E040");
    }

    // f32 is checked on the same terms.
    let mut emitter = VecEmitter::default();
    assert!(
        DecimalFloat(f32::NAN)
            .edifact_serialize(&mut emitter)
            .is_err()
    );

    // Finite values are untouched.
    let mut emitter = VecEmitter::default();
    DecimalFloat(12.5_f64)
        .edifact_serialize(&mut emitter)
        .expect("finite values still serialize");
}

#[test]
fn a_utf8_payload_survives_the_decoding_reader() {
    // `UNOY` and the ASCII-subset repertoires have an empty high half, so mapping
    // a byte through them has no answer.  The reader must pass them through
    // rather than reject them — `decode_reader` wraps every stream, including the
    // ones that need no decoding at all.
    let raw = "UNB+UNOY:4+S+R+260101:0900+IC1'NAD+BY+Müller'UNZ+0+IC1'".as_bytes();
    for charset in [Charset::UnoY, Charset::UnoA, Charset::UnoB] {
        let segments =
            from_reader_collect(charset.decoding_reader(std::io::Cursor::new(raw))).expect("parse");
        assert_eq!(segments[1].element_str(1), Some("Müller"), "{charset}");
    }
}

#[test]
fn decode_reader_discovers_the_repertoire_from_the_stream() {
    use edifact_rs::decode_reader;

    let raw = unoc_interchange();
    let segments = from_reader_collect(decode_reader(std::io::Cursor::new(raw)).expect("sniff"))
        .expect("parse");
    assert_eq!(segments[1].element_str(1), Some("Müller"));

    // A UTF-8 interchange goes through untouched …
    let utf8 = "UNB+UNOY:4+S+R+260101:0900+IC1'NAD+BY+Müller'UNZ+0+IC1'".as_bytes();
    let segments = from_reader_collect(decode_reader(std::io::Cursor::new(utf8)).expect("sniff"))
        .expect("parse");
    assert_eq!(segments[1].element_str(1), Some("Müller"));

    // … and so does a bare message with no UNB to sniff.
    let bare = b"UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'";
    let segments =
        from_reader_collect(decode_reader(std::io::Cursor::new(&bare[..])).expect("sniff"))
            .expect("parse");
    assert_eq!(segments.len(), 3);
}

#[test]
fn decode_reader_handles_a_stream_that_returns_one_byte_at_a_time() {
    use edifact_rs::decode_reader;

    /// A reader that never returns more than one byte per call, which is what
    /// the probe loop has to tolerate — a short read must not truncate the UNB.
    struct Trickle(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for Trickle {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if out.is_empty() {
                return Ok(0);
            }
            std::io::Read::read(&mut self.0, &mut out[..1])
        }
    }

    let segments = from_reader_collect(
        decode_reader(Trickle(std::io::Cursor::new(unoc_interchange()))).expect("sniff"),
    )
    .expect("parse");
    assert_eq!(segments[1].element_str(1), Some("Müller"));
}
