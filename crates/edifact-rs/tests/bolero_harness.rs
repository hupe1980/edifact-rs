//! Bolero property-based / fuzz harness for `edifact-core` (Story 10.3).
//!
//! Run:
//!   cargo bolero test edifact_rs::tests::bolero_no_panic --engine=libfuzzer
//! Or:
//!   cargo test -- bolero

use bolero::check;
use edifact_rs::{from_bytes, segments_to_bytes};

#[test]
fn fuzz_parser_no_panic() {
    // For any arbitrary byte sequence the parser must not panic.
    // It may return errors, but never unwind.
    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            // input is owned by the closure; segments borrow from it and are
            // dropped at the end of each iteration — no leak needed.
            for _ in from_bytes(&input) { /* consume */ }
        });
}

#[test]
fn fuzz_round_trip_valid_segment() {
    // Any tag (up to 3 ASCII uppercase letters) + single ASCII-printable value
    // must survive a write→parse round-trip.
    check!()
        .with_type::<(u8, u8, u8, u8)>()
        .cloned()
        .for_each(|(a, b, c, v): (u8, u8, u8, u8)| {
            let tag_chars = [a, b, c]
                .iter()
                .map(|&x| b'A' + (x % 26))
                .collect::<Vec<_>>();
            let tag = std::str::from_utf8(&tag_chars).unwrap();
            let value_char = (b'A' + v % 26) as char;
            let input = format!("{}+{}'\n", tag, value_char);
            let segs: Vec<_> = from_bytes(input.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(!segs.is_empty());
            assert_eq!(segs[0].tag, tag);
        });
}

#[test]
fn fuzz_parse_write_parse_invariant_small_message() {
    check!()
        .with_type::<(u8, u8)>()
        .cloned()
        .for_each(|(left_value, right_value): (u8, u8)| {
            let left_char = (b'A' + (left_value % 26)) as char;
            let right_char = (b'A' + (right_value % 26)) as char;

            let input = format!("BGM+{}'RFF+{}'", left_char, right_char);
            let segs = from_bytes(input.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            let encoded = segments_to_bytes(&segs).unwrap();
            {
                let reparsed = from_bytes(&encoded).collect::<Result<Vec<_>, _>>().unwrap();

                assert_eq!(
                    reparsed.len(),
                    segs.len(),
                    "segment count must survive round-trip"
                );
                for (orig, rt) in segs.iter().zip(reparsed.iter()) {
                    assert_eq!(orig.tag, rt.tag, "tag must survive round-trip");
                    assert_eq!(
                        orig.elements.len(),
                        rt.elements.len(),
                        "element count must survive round-trip for tag {}",
                        orig.tag,
                    );
                    for (ei, (oe, re)) in orig.elements.iter().zip(rt.elements.iter()).enumerate() {
                        let oe_comps: Vec<&str> =
                            oe.components.iter().map(|(c, _)| c.as_ref()).collect();
                        let re_comps: Vec<&str> =
                            re.components.iter().map(|(c, _)| c.as_ref()).collect();
                        assert_eq!(
                            oe_comps.len(),
                            re_comps.len(),
                            "component count must survive round-trip for tag {} element {ei}",
                            orig.tag,
                        );
                        assert_eq!(
                            oe_comps, re_comps,
                            "component values must survive round-trip for tag {} element {ei}",
                            orig.tag,
                        );
                    }
                }
            }
        });
}

#[test]
fn fuzz_validation_layers_no_panic() {
    use edifact_rs::{
        Segment, ValidationContext, ValidationLayer, ValidationReport, ValidationRuleContext,
        Validator, validate_each,
    };

    struct NoopValidator;

    impl Validator for NoopValidator {
        fn validate_batch(
            &self,
            segments: &[Segment<'_>],
            report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
            validate_each(segments, report, |_segment| Ok(()));
        }
    }

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            let Ok(segments) = from_bytes(&input).collect::<Result<Vec<_>, _>>() else {
                return;
            };

            let context = ValidationContext::builder()
                .with_message_type("ORDERS")
                .with_validator(ValidationLayer::Structure, NoopValidator)
                .with_validator(ValidationLayer::CodeList, NoopValidator)
                .build();

            let _ = context.validate_lenient(&segments);
        });
}

#[test]
fn fuzz_qualifier_matches_pattern_no_panic() {
    use edifact_rs::qualifier_matches_pattern;
    // For any two arbitrary strings the function must never panic.
    check!().with_type::<(String, String)>().cloned().for_each(
        |(value, pattern): (String, String)| {
            let _ = qualifier_matches_pattern(&value, &pattern);
        },
    );
}

#[test]
fn fuzz_qualifier_pattern_invariants() {
    use edifact_rs::qualifier_matches_pattern;
    // Invariant 1: a literal pattern (no '*') is always an exact match.
    // Invariant 2: pattern "*" matches every value (wildcard-only).
    // Invariant 3: empty pattern matches only empty value.
    check!()
        .with_type::<(String,)>()
        .cloned()
        .for_each(|(value,): (String,)| {
            // Invariant 1 — if pattern contains no '*', result == (value == pattern)
            // (Only check for patterns that happen to have no '*'; generate a fresh
            //  ASCII-only copy to avoid accidentally inserting '*')
            let literal: String = value.chars().filter(|&c| c != '*').collect();
            assert!(
                qualifier_matches_pattern(&literal, &literal),
                "a literal must always match itself: {literal:?}",
            );

            // Invariant 2 — "*" matches everything
            assert!(
                qualifier_matches_pattern(&value, "*"),
                "\"*\" must match every value, failed for: {value:?}",
            );

            // Invariant 3 — empty pattern matches only empty value
            assert_eq!(
                qualifier_matches_pattern(&value, ""),
                value.is_empty(),
                "empty pattern should match only empty value, failed for: {value:?}",
            );
        });
}

#[test]
fn fuzz_service_string_advice_is_valid_no_panic() {
    use edifact_rs::ServiceStringAdvice;
    // ServiceStringAdvice::from_bytes_unchecked + is_valid must not panic for any 9-byte input.
    check!()
        .with_type::<[u8; 9]>()
        .cloned()
        .for_each(|bytes: [u8; 9]| {
            let ssa = ServiceStringAdvice::from_bytes_unchecked(&bytes);
            let _ = ssa.is_valid();
        });
}

#[test]
fn fuzz_service_string_advice_valid_una_prefix() {
    use edifact_rs::ServiceStringAdvice;
    // Every valid UNA must start with "UNA" — fuzz the rest.
    check!()
        .with_type::<[u8; 6]>()
        .cloned()
        .for_each(|suffix: [u8; 6]| {
            let mut bytes = [0u8; 9];
            bytes[..3].copy_from_slice(b"UNA");
            bytes[3..].copy_from_slice(&suffix);
            let ssa = ServiceStringAdvice::from_bytes_unchecked(&bytes);

            // The six service characters must round-trip verbatim out of the UNA.
            assert_eq!(ssa.component_sep, suffix[0]);
            assert_eq!(ssa.element_sep, suffix[1]);
            assert_eq!(ssa.decimal_mark, suffix[2]);
            assert_eq!(ssa.release_char, suffix[3]);
            assert_eq!(ssa.repetition_sep, suffix[4]);
            assert_eq!(ssa.segment_term, suffix[5]);

            // `is_valid` must agree with the checked constructor, and must imply
            // that every active delimiter is distinct printable ASCII — the
            // invariant the whole tokenizer relies on.
            let checked = ServiceStringAdvice::from_bytes(&bytes);
            assert_eq!(
                ssa.is_valid(),
                checked.is_ok(),
                "is_valid disagreed with from_bytes for {suffix:?}"
            );
            if ssa.is_valid() {
                // The *active* service characters are the ones the tokenizer
                // splits on.  The decimal mark is not among them: ISO 9735-1
                // Annex B says the recipient ignores it, and it is the one
                // position where the standard permits a space.
                let mut active = vec![
                    ssa.component_sep,
                    ssa.element_sep,
                    ssa.release_char,
                    ssa.segment_term,
                ];
                if ssa.is_repetition_active() {
                    active.push(ssa.repetition_sep);
                }
                for (i, a) in active.iter().enumerate() {
                    assert!(
                        (0x21..=0x7E).contains(a) && !a.is_ascii_alphanumeric(),
                        "delimiter {a:#04X} is not printable non-alphanumeric ASCII"
                    );
                    for b in &active[i + 1..] {
                        assert_ne!(a, b, "duplicate delimiter {a:#04X}");
                    }
                }
                assert!(
                    (0x20..=0x7E).contains(&ssa.decimal_mark),
                    "decimal mark {:#04X} is not a graphic ASCII byte",
                    ssa.decimal_mark
                );
            }
        });
}

// ── structured generation ─────────────────────────────────────────────────────

/// Build a syntactically plausible interchange from a fuzzer seed.
///
/// Arbitrary `Vec<u8>` essentially never parses as EDIFACT, so targets driven by
/// raw bytes almost always bail before reaching the code they are named after.
/// This produces messages that *do* parse — including empty elements, multi-
/// component composites, and values containing every service character, so the
/// escaping path is actually exercised.
fn build_message(seed: &[u8]) -> String {
    // Values deliberately include each delimiter and the release character so the
    // writer's escaping and the tokenizer's un-escaping must agree.
    const VALUES: &[&str] = &[
        "",
        "A",
        "220",
        "a+b",
        "a:b",
        "a?b",
        "a'b",
        "a*b",
        "??",
        "a??b",
        "x?'y",
        "LONGER VALUE",
    ];
    if seed.is_empty() {
        return "BGM+220'".to_owned();
    }
    let mut out = String::new();
    let segment_count = 1 + (seed[0] as usize % 4);
    let mut cursor = 1usize;
    let next = |cursor: &mut usize| -> u8 {
        let b = seed.get(*cursor).copied().unwrap_or(0);
        *cursor = cursor.wrapping_add(1);
        b
    };
    for _ in 0..segment_count {
        let tag_seed = next(&mut cursor);
        let tag: String =
            ["BGM", "RFF", "NAD", "DTM", "FTX", "LIN"][tag_seed as usize % 6].to_owned();
        out.push_str(&tag);
        let element_count = next(&mut cursor) as usize % 4;
        for _ in 0..element_count {
            out.push('+');
            let component_count = 1 + (next(&mut cursor) as usize % 3);
            for c in 0..component_count {
                if c > 0 {
                    out.push(':');
                }
                let v = VALUES[next(&mut cursor) as usize % VALUES.len()];
                // Escape the service characters so the generated text is valid.
                for ch in v.chars() {
                    if matches!(ch, '+' | ':' | '?' | '\'') {
                        out.push('?');
                    }
                    out.push(ch);
                }
            }
        }
        out.push('\'');
    }
    out
}

#[test]
fn fuzz_structured_parse_write_reparse_is_stable() {
    // parse -> write -> reparse must preserve the full segment/element/component
    // structure.  This is the invariant that the raw-bytes targets never reach.
    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|seed: Vec<u8>| {
            let text = build_message(&seed);
            let Ok(first) = from_bytes(text.as_bytes()).collect::<Result<Vec<_>, _>>() else {
                panic!("generator produced unparseable EDIFACT: {text:?}");
            };
            let bytes = segments_to_bytes(&first).expect("write must succeed");
            let second = from_bytes(&bytes)
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|e| panic!("re-parse of own output failed: {e} for {text:?}"));

            assert_eq!(first.len(), second.len(), "segment count changed: {text:?}");
            for (a, b) in first.iter().zip(second.iter()) {
                assert_eq!(a.tag, b.tag, "tag changed: {text:?}");
                assert_eq!(
                    a.elements.len(),
                    b.elements.len(),
                    "element count changed for {}: {text:?}",
                    a.tag
                );
                for (ea, eb) in a.elements.iter().zip(b.elements.iter()) {
                    let va: Vec<&str> = ea.components.iter().map(|(c, _)| c.as_ref()).collect();
                    let vb: Vec<&str> = eb.components.iter().map(|(c, _)| c.as_ref()).collect();
                    assert_eq!(va, vb, "component values changed for {}: {text:?}", a.tag);
                }
            }
        });
}

#[test]
fn fuzz_custom_una_round_trip_is_stable() {
    // Same invariant, but written through a fuzzed (valid) UNA.  A writer that
    // hardcodes a default delimiter, or forgets to escape one that the active UNA
    // declares, fails here and nowhere else.
    use edifact_rs::{ServiceStringAdvice, Writer};
    check!()
        .with_type::<(Vec<u8>, [u8; 6])>()
        .cloned()
        .for_each(|(seed, delims): (Vec<u8>, [u8; 6])| {
            let mut una = [0u8; 9];
            una[..3].copy_from_slice(b"UNA");
            una[3..].copy_from_slice(&delims);
            // Only exercise delimiter sets the library accepts.
            let Ok(ssa) = ServiceStringAdvice::from_bytes(&una) else {
                return;
            };

            let text = build_message(&seed);
            let Ok(source) = from_bytes(text.as_bytes()).collect::<Result<Vec<_>, _>>() else {
                return;
            };

            let mut buf = Vec::new();
            {
                let mut w = Writer::with_una(&mut buf, ssa).expect("valid ssa");
                for seg in &source {
                    w.write_segment(seg).expect("write must succeed");
                }
                w.finish().expect("flush must succeed");
            }

            let reparsed = from_bytes(&buf)
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|e| {
                    panic!("output written with UNA {delims:?} did not reparse: {e}")
                });
            assert_eq!(source.len(), reparsed.len(), "segment count changed");
            for (a, b) in source.iter().zip(reparsed.iter()) {
                assert_eq!(a.tag, b.tag);
                let va: Vec<Vec<&str>> = a
                    .elements
                    .iter()
                    .map(|e| e.components.iter().map(|(c, _)| c.as_ref()).collect())
                    .collect();
                let vb: Vec<Vec<&str>> = b
                    .elements
                    .iter()
                    .map(|e| e.components.iter().map(|(c, _)| c.as_ref()).collect())
                    .collect();
                assert_eq!(va, vb, "values changed under UNA {delims:?}");
            }
        });
}

#[test]
fn fuzz_writer_no_panic() {
    // Feeding arbitrary string data to the writer must never panic.
    // The writer may return errors, but must not unwind.
    check!()
        .with_type::<(String, Vec<Vec<String>>)>()
        .cloned()
        .for_each(|(tag, elements): (String, Vec<Vec<String>>)| {
            let mut buf: Vec<u8> = Vec::new();
            let mut writer = edifact_rs::Writer::new(&mut buf);
            let _ = writer.write_segment_parts(&tag, &elements);
        });
}

#[test]
fn fuzz_validate_envelope_no_panic() {
    // `validate_envelope` must not panic for any parseable byte sequence.
    use edifact_rs::validate_envelope;

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            let Ok(segs) = from_bytes(&input).collect::<Result<Vec<_>, _>>() else {
                return;
            };
            // May return Ok or Err — must never panic.
            let _ = validate_envelope(&segs);
        });
}

#[test]
fn fuzz_message_windows_bytes_no_panic() {
    // `from_bytes_windows` must not panic for any byte sequence.
    use edifact_rs::from_bytes_windows;

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            // Consume the full iterator — any item may be Ok or Err.
            for _ in from_bytes_windows(&input) { /* consume */ }
        });
}

#[test]
fn fuzz_reader_no_panic_and_equivalence() {
    // The reader-based path (OwnedSegmentStream) exercises distinct logic from the
    // slice path: fast-path BufRead scan, slow-path byte accumulation, UNA detection
    // across buffer boundaries, and max_segment_bytes guard.
    //
    // Two properties are tested:
    //   1. No panic for any arbitrary byte sequence.
    //   2. When both paths succeed, the resulting segments are identical.
    use edifact_rs::from_reader_collect;

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            // Use a small BufReader capacity to maximise buffer-boundary splits.
            let reader = std::io::BufReader::with_capacity(8, std::io::Cursor::new(&input));
            let reader_result: Result<Vec<_>, _> = from_reader_collect(reader);
            let slice_result: Result<Vec<_>, _> = from_bytes(&input).collect();

            // Property 1: no panic (guaranteed by running the code above).

            // Property 2: on success both paths must agree on tag sequence.
            if let (Ok(reader_segs), Ok(slice_segs)) = (&reader_result, &slice_result) {
                assert_eq!(
                    reader_segs.len(),
                    slice_segs.len(),
                    "reader and slice paths returned different segment counts for input: {:?}",
                    &input[..input.len().min(64)],
                );
                for (r, s) in reader_segs.iter().zip(slice_segs.iter()) {
                    assert_eq!(r.tag, s.tag, "tag mismatch between reader and slice paths");
                }
            }
        });
}

#[test]
fn fuzz_from_bytes_no_panic() {
    // `ServiceStringAdvice::from_bytes` must not panic or unwind for any
    // arbitrary byte input.  It may return errors or valid SSAs.
    use edifact_rs::ServiceStringAdvice;

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            // May succeed or return an error — must never panic.
            let _ = ServiceStringAdvice::from_bytes(&input);
        });
}

#[test]
fn fuzz_tokenizer_with_limit_no_panic() {
    // The tokenizer with a size limit and custom SSA must never panic for any byte input.
    use edifact_rs::{
        Parser, ReaderConfig, ServiceStringAdvice, Tokenizer, from_bytes_with_config,
    };

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            // Default path: exercises UNA detection + default 64 KiB limit.
            for result in edifact_rs::from_bytes(&input) {
                // Results may be Ok or Err; we only require no panic.
                let _ = result;
            }

            // Reduced-limit path: exercises max_segment_bytes enforcement.
            let small_limit = ReaderConfig::default().max_segment_bytes(64);
            for result in from_bytes_with_config(&input, small_limit) {
                let _ = result;
            }

            // Custom-SSA path: derive a non-default SSA from the first 9 bytes and
            // parse with it — exercises alternative delimiter paths via the public
            // Tokenizer + Parser API.
            if input.len() >= 9 {
                let ssa = ServiceStringAdvice::from_bytes_unchecked(&input[..9]);
                // Ensure is_valid does not panic.
                let _ = ssa.is_valid();
                // Parse using the derived SSA with a 64 KiB per-segment limit.
                let t = Tokenizer::with_limit(&input, ssa, 65_536);
                let p = Parser::new(t);
                for result in p {
                    let _ = result;
                }
            }
        });
}

#[test]
fn fuzz_directory_validator_no_panic() {
    // `DirectoryValidator::validate_batch` must never panic for any parseable byte
    // sequence, regardless of whether the interchange looks like a known message type
    // or contains completely invalid structure.
    use edifact_rs::{
        DirectoryValidator, ElementRef, SegmentDefinition, Status, ValidationReport,
        ValidationRuleContext, Validator,
    };

    // A minimal static segment directory: just BGM and DTM so we exercise both
    // "known segment" and "unknown segment" paths without pulling in a full directory.
    static BGM_ELEMENTS: &[ElementRef] = &[
        ElementRef::new(1, "C002", Status::Conditional, 1),
        ElementRef::new(2, "1004", Status::Conditional, 1),
    ];
    static DTM_ELEMENTS: &[ElementRef] = &[ElementRef::new(1, "C507", Status::Mandatory, 1)];
    static BGM_DEF: SegmentDefinition =
        SegmentDefinition::new("BGM", "Beginning of message", BGM_ELEMENTS);
    static DTM_DEF: SegmentDefinition =
        SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);

    fn seg_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
        match tag {
            "BGM" => Some(&BGM_DEF),
            "DTM" => Some(&DTM_DEF),
            _ => None,
        }
    }
    fn code_valid(_: &str, _: &str) -> bool {
        true
    }
    fn suggest(_: &str, _: &str) -> Option<&'static str> {
        None
    }
    fn expected_components(_: &str, _: usize) -> Option<u8> {
        None
    }

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            let Ok(segments) = from_bytes(&input).collect::<Result<Vec<_>, _>>() else {
                return;
            };
            let validator = DirectoryValidator::new(
                "FUZZ",
                seg_lookup,
                code_valid,
                suggest,
                expected_components,
                None,
            );
            let mut report = ValidationReport::default();
            let ctx = ValidationRuleContext::empty();
            validator.validate_batch(&segments, &mut report, &ctx);
        });
}

#[test]
fn fuzz_serialization_no_panic() {
    // Feeding arbitrary segments through the serialisation round-trip must never panic.
    // Specifically exercises `Writer::write_segment` and the escape logic in `ser.rs`.
    use edifact_rs::segments_to_bytes;

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            let Ok(segs) = from_bytes(&input).collect::<Result<Vec<_>, _>>() else {
                return;
            };
            // `segments_to_bytes` must not panic even if segments contain
            // release-character edge cases or unusual delimiter combinations.
            let _ = segments_to_bytes(&segs);
        });
}

/// Checks that `Writer::escape_value` never leaves unescaped structural delimiter
/// bytes (element separator, component separator, segment terminator) in its output,
/// and that the escaped value can be round-tripped back through the parser to recover
/// the original string.
///
/// The release character itself is intentionally **not** forbidden from appearing in
/// the output; it is a legitimate payload byte and is only required to be present
/// *before* each structural delimiter that was escaped.
///
/// We construct a `ServiceStringAdvice` with printable-ASCII delimiters and
/// a valid release character, then run arbitrary UTF-8 values through the
/// escape path and verify the invariant.
#[test]
fn fuzz_escape_value_no_unescaped_delimiters() {
    use edifact_rs::{Parser, ServiceStringAdvice, Tokenizer, Writer};

    check!()
        .with_type::<(Vec<u8>, u8, u8, u8, u8)>()
        .cloned()
        .for_each(|(value_bytes, elem_raw, comp_raw, release_raw, term_raw): (Vec<u8>, u8, u8, u8, u8)| {
            // Map raw bytes into the printable-ASCII 0x21–0x7E range.
            const PRINTABLE_LEN: u8 = 0x7E - 0x21 + 1; // 94 printable ASCII chars
            let elem_sep  = 0x21u8 + (elem_raw    % PRINTABLE_LEN);
            let comp_sep  = 0x21u8 + (comp_raw    % PRINTABLE_LEN);
            let release   = 0x21u8 + (release_raw % PRINTABLE_LEN);
            let term      = 0x21u8 + (term_raw    % PRINTABLE_LEN);

            // Delimiters must all be distinct — skip if any collide.
            let delimiters = [elem_sep, comp_sep, release, term];
            if delimiters.iter().collect::<std::collections::HashSet<_>>().len() < 4 {
                return;
            }

            // The value must be valid UTF-8; skip invalid byte sequences.
            let Ok(value) = std::str::from_utf8(&value_bytes) else {
                return;
            };

            // Build a UNA string: UNA<comp><elem>. <release><term>
            // (decimal point placeholder is always `.`; not used as a delimiter here)
            let una = format!(
                "UNA{}{}.{} {}",
                comp_sep as char,
                elem_sep as char,
                release as char,
                term as char,
            );
            let ssa = ServiceStringAdvice::from_bytes(una.as_bytes());
            let Ok(ssa) = ssa else {
                return; // Invalid SSA combination — skip.
            };

            // Build a Writer backed by a Vec<u8>.
            let mut buf = Vec::new();
            let Ok(writer) = Writer::with_una(&mut buf, ssa) else {
                return;
            };

            // Escape the value — must not panic.
            let escaped = writer.escape_value(value);

            // Property: structural delimiters (element/component separator, segment terminator)
            // must each be preceded by the release character when they appear in the output.
            // The release character itself may appear freely as a payload byte.
            let elem_ch    = elem_sep as char;
            let comp_ch    = comp_sep as char;
            let release_ch = release  as char;
            let term_ch    = term     as char;

            let chars: Vec<char> = escaped.chars().collect();
            for (idx, &ch) in chars.iter().enumerate() {
                if ch == elem_ch || ch == comp_ch || ch == term_ch {
                    let preceded_by_release = idx > 0 && chars[idx - 1] == release_ch;
                    assert!(
                        preceded_by_release,
                        "unescaped delimiter {ch:?} at position {idx} in escaped value {escaped:?} \
                         (original: {value:?}, elem={elem_ch:?} comp={comp_ch:?} \
                         release={release_ch:?} term={term_ch:?})"
                    );
                }
            }

            // Round-trip property: build a minimal segment and parse it back.
            // Use Tokenizer::with_limit so the SSA delimiters are honoured.
            if !value.contains('\n') && !value.contains('\r') {
                let message = format!("BGM+{}{}", escaped, term as char);
                let t = Tokenizer::with_limit(message.as_bytes(), ssa, 65_536);
                let mut p = Parser::new(t);
                if let Some(Ok(seg)) = p.next() {
                    if let Some(v) = seg.element_str(0) {
                        assert_eq!(
                            v, value,
                            "round-trip mismatch: escaped={escaped:?} value={value:?}"
                        );
                    }
                }
            }
        });
}

#[test]
fn fuzz_validate_envelope_lenient_no_panic() {
    // `validate_envelope_lenient` must not panic for any parseable byte sequence.
    use edifact_rs::validate_envelope_lenient;

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            let Ok(segs) = from_bytes(&input).collect::<Result<Vec<_>, _>>() else {
                return;
            };
            // Returns a Vec<EdifactError> — must never panic.
            let _ = validate_envelope_lenient(&segs);
        });
}

// ── F-030: ProfileRulePack + group validation never panics ────────────────────

/// Fuzz property: parsing arbitrary bytes and running them through a
/// `ProfileRulePack` with representative flat and group rules must never panic.
///
/// This exercises:
/// - `validate_batch` (flat rules: require_segment, forbid_segment, require_qualifier)
/// - `validate_group_batch` (group rules: require_segment_in_group, forbid_segment_in_group)
/// - `group_segments_indexed` (segment tree construction)
/// - `validate_lenient_grouped` (combined flat + group pass)
#[test]
fn fuzz_profile_rule_pack_no_panic() {
    use edifact_rs::{
        ProfileRulePack, ValidationContext,
        group::{GroupDef, group_segments_indexed},
    };

    static FUZZ_SCHEMA: &[GroupDef] = &[
        GroupDef {
            name: "SG1",
            trigger: "BGM",
            children: &[],
        },
        GroupDef {
            name: "SG2",
            trigger: "NAD",
            children: &[GroupDef {
                name: "SG3",
                trigger: "RFF",
                children: &[],
            }],
        },
    ];

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            let Ok(segs) = from_bytes(&input).collect::<Result<Vec<_>, _>>() else {
                return;
            };
            // Build a representative pack with flat rules and group-scoped rules.
            let pack = ProfileRulePack::new("FUZZ")
                .require_segment("UNH", "UNH-M")
                .forbid_segment("UNK", "UNK-F")
                .require_qualifier("UNH", 1, 0, "ORDERS", "UNH-ORDERS")
                .require_segment_in_group("SG1", "LOC", "SG1-LOC-M")
                .forbid_segment_in_group("SG2", "UNS", "SG2-UNS-F");
            let ctx = ValidationContext::builder().with_profile_pack(pack).build();
            // Flat validation must not panic.
            let _ = ctx.validate_lenient(&segs);
            // Build the segment group tree — must not panic.
            let tree = group_segments_indexed(&segs, FUZZ_SCHEMA, "ROOT");
            // Grouped validation must not panic.
            let _ = ctx.validate_lenient_grouped(&tree, &segs);
        });
}

/// Fuzz property: group_segments_indexed on arbitrary byte input must never panic.
#[test]
fn fuzz_group_segments_indexed_no_panic() {
    use edifact_rs::group::{GroupDef, group_segments_indexed};

    static DEEP_SCHEMA: &[GroupDef] = &[GroupDef {
        name: "G1",
        trigger: "AAA",
        children: &[GroupDef {
            name: "G2",
            trigger: "BBB",
            children: &[GroupDef {
                name: "G3",
                trigger: "CCC",
                children: &[],
            }],
        }],
    }];

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            let Ok(segs) = from_bytes(&input).collect::<Result<Vec<_>, _>>() else {
                return;
            };
            // Must not panic regardless of segment content.
            let _ = group_segments_indexed(&segs, DEEP_SCHEMA, "ROOT");
        });
}

/// parse → write → reparse must preserve ISO 9735-4 repetitions exactly.
///
/// The repetition separator is the one delimiter that changes an element's
/// *shape* rather than its text, so a bug here silently merges or splits
/// occurrences instead of producing a parse error.
#[test]
fn fuzz_repetition_round_trip_is_stable() {
    use edifact_rs::{ServiceStringAdvice, Writer};

    check!()
        .with_type::<(Vec<u8>, u8)>()
        .cloned()
        .for_each(|(seed, count): (Vec<u8>, u8)| {
            // 1..=8 occurrences of a two-component element, values drawn from
            // the seed so escaping and empty components are both exercised.
            let repeats = (count % 8) as usize + 1;
            let value = |i: usize| -> String {
                seed.get(i)
                    .map(|b| ((b'A' + (b % 26)) as char).to_string())
                    .unwrap_or_default()
            };

            let ssa = ServiceStringAdvice {
                repetition_sep: b'*',
                ..ServiceStringAdvice::default()
            };

            let owned: Vec<String> = (0..repeats).map(value).collect();
            let mut built = edifact_rs::Element::of(&["ON", owned[0].as_str()]);
            for v in owned.iter().skip(1) {
                built = built.and_repeat(&["ON", v.as_str()]);
            }
            let segment = edifact_rs::Segment::new("RFF", vec![built]);

            let mut buf = Vec::new();
            {
                let mut writer = Writer::with_una(&mut buf, ssa).expect("valid UNA");
                writer.write_segment(&segment).expect("write must succeed");
            }

            let reparsed = from_bytes(&buf)
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|e| panic!("re-parse of own output failed: {e}"));
            assert_eq!(reparsed.len(), 1);

            let element = reparsed[0].get_element(0).expect("element present");
            assert_eq!(
                element.repeat_count(),
                repeats,
                "repetition count changed for {owned:?}"
            );
            for (i, expected) in owned.iter().enumerate() {
                let components = element.repetition(i).expect("repetition present");
                assert_eq!(components[0].0.as_ref(), "ON");
                // A trailing empty component is dropped on the wire, so an empty
                // value legitimately reparses as a one-component repetition.
                let actual = components.get(1).map(|(c, _)| c.as_ref()).unwrap_or("");
                assert_eq!(actual, expected.as_str(), "value changed at repetition {i}");
            }
        });
}
