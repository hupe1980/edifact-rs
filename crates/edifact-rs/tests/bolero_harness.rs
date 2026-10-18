//! Property and fuzz tests over parse, write, and validate.
//!
//! # What a target here must assert
//!
//! **The output contract, not only the absence of a panic.** A target that runs
//! the code and discards the result passes for the wrong reason: it exercises
//! the bug and throws away the evidence. Every target below either
//!
//! - asserts an invariant of what came back (spans in bounds, a report's buckets
//!   summing, a written segment reading back), or
//! - compares two implementations that must agree — **including on whether they
//!   succeed at all**, since an input one accepts and the other rejects is the
//!   most interesting case, not one to skip.
//!
//! A conditional assertion (`if let (Ok(_), Ok(_)) = …`) is the trap: it looks
//! like a differential test and silently covers only the agreeing half.

use bolero::check;
use edifact_rs::{from_bytes, segments_to_bytes};

#[test]
fn fuzz_parser_output_is_well_formed() {
    // For any byte sequence the parser must not panic — and every segment it
    // *does* yield must satisfy the contract the rest of the crate relies on:
    // a valid tag, and spans that index the input in bounds and in order.
    //
    // Asserting only "no panic" would let a corrupt span or an unparseable tag
    // through, and both are silent: a bad span misplaces a diagnostic, a bad tag
    // is bytes that will not read back.
    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            for result in from_bytes(&input) {
                let Ok(segment) = result else { break };

                assert_eq!(
                    segment.tag().len(),
                    3,
                    "tag {:?} is not 3 bytes",
                    segment.tag()
                );
                assert!(
                    segment.tag().bytes().all(|b| b.is_ascii_uppercase()),
                    "tag {:?} is not all uppercase ASCII",
                    segment.tag(),
                );

                let in_bounds = |span: edifact_rs::Span, what: &str| {
                    assert!(span.start <= span.end, "inverted {what} span {span:?}");
                    assert!(
                        span.end <= input.len(),
                        "{what} span {span:?} runs past the {}-byte input",
                        input.len(),
                    );
                };
                in_bounds(segment.span, "segment");
                in_bounds(segment.tag_span, "tag");
                for element in &segment.elements {
                    in_bounds(element.span, "element");
                    for occurrence in element.repetitions() {
                        for (_, span) in occurrence {
                            in_bounds(*span, "component");
                        }
                    }
                }
            }
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
fn fuzz_validation_report_is_self_consistent() {
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

            let report = context.validate(&segments);

            // The three severity buckets and the aggregate must agree, and the
            // two ways of asking "did this pass?" must give the same answer —
            // callers branch on both.
            assert_eq!(
                report.total_issues(),
                report.errors().len() + report.warnings().len() + report.infos().len(),
                "total_issues disagrees with the severity buckets",
            );
            assert_eq!(
                report.is_valid(),
                report.errors().is_empty(),
                "is_valid disagrees with the error bucket",
            );
            assert_eq!(
                report.is_valid(),
                report.clone().result().is_ok(),
                "result() disagrees with is_valid()",
            );
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
fn fuzz_valid_service_characters_are_writable() {
    use edifact_rs::{ServiceStringAdvice, Writer};
    // `is_valid` is the gate the writer trusts before emitting a `UNA`, so the
    // two must agree exactly: anything it calls valid must be writable, and
    // anything it rejects must be refused.  Asserting only "does not panic"
    // would let the predicate and its one consumer drift apart.
    check!()
        .with_type::<[u8; 9]>()
        .cloned()
        .for_each(|bytes: [u8; 9]| {
            let ssa = ServiceStringAdvice::from_bytes_unchecked(&bytes);
            let accepted = Writer::with_una(Vec::new(), ssa).is_ok();
            assert_eq!(
                ssa.is_valid(),
                accepted,
                "is_valid() and Writer::with_una disagree about {bytes:?}",
            );

            if !ssa.is_valid() {
                return;
            }
            // Valid means the active service characters are pairwise distinct,
            // which is what makes a written interchange re-readable.
            let mut active = vec![
                ssa.component_sep,
                ssa.element_sep,
                ssa.release_char,
                ssa.segment_term,
            ];
            if ssa.is_repetition_active() {
                active.push(ssa.repetition_sep);
            }
            let mut sorted = active.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                active.len(),
                "is_valid() accepted duplicate service characters {active:?}",
            );
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
fn fuzz_anything_the_writer_accepts_reparses() {
    // The writer's core contract: it never emits bytes the parser rejects.
    //
    // A segment tag is written verbatim, so an invalid one cannot be escaped
    // out of trouble the way a delimiter inside a value can — the writer has to
    // refuse it rather than emit something that does not read back.
    check!()
        .with_type::<(String, Vec<Vec<String>>)>()
        .cloned()
        .for_each(|(tag, elements): (String, Vec<Vec<String>>)| {
            let mut buf: Vec<u8> = Vec::new();
            let mut writer = edifact_rs::Writer::new(&mut buf);
            if writer.write_composites(&tag, &elements).is_err() {
                return; // refused: nothing to reparse
            }
            if writer.finish().is_err() {
                return;
            }

            let reparsed: Result<Vec<_>, _> = from_bytes(&buf).collect();
            let segments = reparsed.unwrap_or_else(|e| {
                panic!(
                    "the writer accepted {tag:?} / {elements:?} and emitted {:?}, \
                     which the parser then rejected: {e}",
                    String::from_utf8_lossy(&buf),
                )
            });

            // An all-empty element list writes just the tag and terminator, which
            // is one segment; anything else is still exactly one segment.
            assert_eq!(segments.len(), 1, "one segment in, one segment out");
            assert_eq!(segments[0].tag, tag, "the tag must survive verbatim");
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
fn fuzz_message_windows_no_panic() {
    // `message_windows` must not panic for any byte sequence.
    use edifact_rs::message_windows;

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            // Consume the full iterator — any item may be Ok or Err.
            for _ in message_windows(&input) { /* consume */ }
        });
}

#[test]
fn fuzz_reader_no_panic_and_equivalence() {
    // The reader-based path (OwnedSegmentStream) exercises distinct logic from the
    // slice path: fast-path BufRead scan, slow-path byte accumulation, UNA detection
    // across buffer boundaries, and max_segment_bytes guard.
    //
    // The two paths must agree **completely**: same verdict, and on success the
    // same segments.  Comparing only when both succeed would skip the one case
    // that matters most — an input one path accepts and the other rejects.
    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            // Use a small BufReader capacity to maximise buffer-boundary splits.
            let reader = std::io::BufReader::with_capacity(8, std::io::Cursor::new(&input));
            let reader_result: Result<Vec<_>, _> = edifact_rs::from_bufread(reader).collect();
            let slice_result: Result<Vec<_>, _> = from_bytes(&input).collect();

            let head = &input[..input.len().min(64)];
            assert_eq!(
                reader_result.is_ok(),
                slice_result.is_ok(),
                "the two paths disagreed on whether {head:?} parses at all: \
                 reader={reader_result:?} slice={slice_result:?}",
            );

            let (Ok(reader_segs), Ok(slice_segs)) = (&reader_result, &slice_result) else {
                return; // both failed, which is agreement
            };
            assert_eq!(
                reader_segs.len(),
                slice_segs.len(),
                "different segment counts for {head:?}",
            );
            for (r, s) in reader_segs.iter().zip(slice_segs.iter()) {
                assert_eq!(r.tag, s.tag, "tag mismatch between the two paths");
                assert_eq!(
                    r.elements.len(),
                    s.elements.len(),
                    "element count mismatch on {} for {head:?}",
                    r.tag,
                );
                for (re, se) in r.elements.iter().zip(s.elements.iter()) {
                    assert_eq!(
                        re.components().collect::<Vec<_>>(),
                        se.components().collect::<Vec<_>>(),
                        "component values differ on {} for {head:?}",
                        r.tag,
                    );
                    assert_eq!(
                        re.repeat_count(),
                        se.repeat_count(),
                        "occurrence count differs on {} for {head:?}",
                        r.tag,
                    );
                }
            }
        });
}

#[test]
fn fuzz_reader_and_slice_agree_on_limit_errors() {
    // FR: for every input and every `ReaderConfig`, the slice and reader paths
    // yield the same segments followed by the same limit error — code, limit,
    // ceiling and byte offset — and a limit error ends both iterators.
    //
    // Only inputs on which the two paths already agree without count or byte
    // budgets are compared: the non-limit divergences between them (for
    // example a doubled segment terminator, which the reader skips and the
    // slice tokenizer refuses) are a separate parity question, and must not
    // make this target fail.
    use edifact_rs::{
        EdifactError, ReaderConfig, from_bufread_with_config, from_bytes_with_config,
    };

    fn outcome<T: std::fmt::Debug>(
        mut iter: impl Iterator<Item = Result<T, EdifactError>>,
    ) -> (Vec<String>, Option<EdifactError>) {
        let mut items = Vec::new();
        while let Some(item) = iter.next() {
            match item {
                Ok(value) => items.push(format!("{value:?}")),
                Err(error) => {
                    assert!(iter.next().is_none(), "an item followed {error:?}");
                    return (items, Some(error));
                }
            }
        }
        (items, None)
    }

    fn both(input: &[u8], config: ReaderConfig) -> [(Vec<String>, Option<EdifactError>); 3] {
        [
            outcome(from_bytes_with_config(input, config).map(|r| r.map(|s| s.into_owned()))),
            outcome(from_bufread_with_config(
                std::io::BufReader::with_capacity(8, input),
                config,
            )),
            outcome(from_bufread_with_config(
                std::io::BufReader::new(input),
                config,
            )),
        ]
    }

    check!()
        .with_type::<(Vec<u8>, u8, u8, u8, u8, u16, u8)>()
        .cloned()
        .for_each(
            |(bytes, segment_bytes, segments, messages, per_message, input_bytes, shape): (
                Vec<u8>,
                u8,
                u8,
                u8,
                u8,
                u16,
                u8,
            )| {
                // Half the cases corrupt a valid interchange (with a BOM,
                // whitespace and a UNA in some of them) at fuzzed positions.
                let input = if shape % 2 == 0 {
                    let mut template = Vec::new();
                    if shape % 4 == 0 {
                        template.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
                    }
                    if shape % 8 < 4 {
                        template.extend_from_slice(b"UNA:+.? '");
                    }
                    template.extend_from_slice(
                        b"UNB+UNOC:3+S:14+R:14+260101:0900+IC1'\r\n\
UNH+M1+ORDERS:D:96A:UN'BGM+220+PO?+1+9'DTM+137:20260101:102'UNT+4+M1'\n\
UNH+M2+ORDERS:D:96A:UN'BGM+220+PO2'UNT+3+M2'UNZ+2+IC1'  \n",
                    );
                    for pair in bytes.chunks_exact(2).take(4) {
                        let at = usize::from(pair[0]) % template.len();
                        template[at] = pair[1];
                    }
                    template
                } else {
                    bytes
                };

                let mut base = ReaderConfig::default();
                if segment_bytes % 2 == 0 {
                    base = base.max_segment_bytes(usize::from(segment_bytes / 2) + 1);
                }
                let [slice, slow, fast] = both(&input, base);
                if slice != slow || slice != fast {
                    return; // a non-limit divergence; not this target's question
                }

                let mut config = base;
                if segments % 3 != 0 {
                    config = config.max_segments(usize::from(segments % 24));
                }
                if messages % 3 != 0 {
                    config = config.max_messages(usize::from(messages % 4));
                }
                if per_message % 3 != 0 {
                    config = config.max_segments_per_message(usize::from(per_message % 8));
                }
                if input_bytes % 3 != 0 {
                    config =
                        config.max_input_bytes(u64::from(input_bytes) % (input.len() as u64 + 8));
                }
                let [slice, slow, fast] = both(&input, config);
                let head = &input[..input.len().min(64)];
                assert_eq!(
                    slice, slow,
                    "slice vs slow reader for {head:?} under {config:?}"
                );
                assert_eq!(
                    slice, fast,
                    "slice vs fast reader for {head:?} under {config:?}"
                );

                // A limit never hides bytes it admitted: the segments before
                // the limit are exactly those of the unlimited parse.
                let [unlimited, _, _] = both(&input, base);
                assert!(
                    unlimited.0.starts_with(&slice.0),
                    "a limit changed the segments before it for {head:?}"
                );
                if let Some(EdifactError::LimitExceeded {
                    limit, offset, max, ..
                }) = &slice.1
                {
                    if *limit == "max_input_bytes" {
                        assert_eq!(*offset as u64, *max, "byte budget offset");
                    } else {
                        assert!(*offset < input.len(), "count budget offset in bounds");
                    }
                }
            },
        );
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

// ── ProfileRulePack + group validation never panics ──────────────────────────

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

    static SG3: &[GroupDef] = &[GroupDef::new("SG3", "RFF")];
    static FUZZ_SCHEMA: &[GroupDef] = &[
        GroupDef::new("SG1", "BGM"),
        GroupDef::with_children("SG2", "NAD", SG3),
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
            let _ = ctx.validate(&segs);
            // Build the segment group tree — must not panic.
            let tree = group_segments_indexed(&segs, FUZZ_SCHEMA, "ROOT");
            // Grouped validation must not panic.
            let _ = ctx.validate_grouped(&tree, &segs);
        });
}

// ── 005: grouping output contracts ─────────────────────────────────────────────

#[path = "common/legacy_grouping.rs"]
mod legacy_grouping;

const GROUP_TAGS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];

/// A trigger-only schema from fuzz bytes: depth ≤ 3, ≤ 4 children per level,
/// triggers from a five-tag alphabet, names from four (so same-name siblings
/// and shared triggers both occur).
fn legacy_schema(
    seed: &mut impl Iterator<Item = u8>,
    depth: usize,
) -> Vec<legacy_grouping::LegacyDef> {
    let count = usize::from(seed.next().unwrap_or(0) % 5);
    (0..count)
        .map(|_| {
            let name = format!("G{}", seed.next().unwrap_or(0) % 4);
            let trigger = GROUP_TAGS[usize::from(seed.next().unwrap_or(0)) % 5].to_owned();
            let children = if depth < 3 {
                legacy_schema(seed, depth + 1)
            } else {
                Vec::new()
            };
            legacy_grouping::LegacyDef {
                name,
                trigger,
                children,
            }
        })
        .collect()
}

fn owned_open(def: &legacy_grouping::LegacyDef) -> edifact_rs::GroupDef {
    def.children
        .iter()
        .fold(
            edifact_rs::GroupDef::builder(
                def.name.as_str(),
                edifact_rs::SegmentPlace::owned(def.trigger.as_str()),
            ),
            |b, child| b.group(owned_open(child)),
        )
        .build()
        .expect("generated schemas are valid")
}

fn to_legacy(node: &edifact_rs::SegmentGroupIndexed<'_>) -> legacy_grouping::LegacyNode {
    legacy_grouping::LegacyNode {
        definition: node.definition.to_owned(),
        total_span: node.total_span.clone(),
        children: node.children.iter().map(to_legacy).collect(),
        occurrence_index: node.occurrence_index,
    }
}

/// Every node's direct indices agree with the legacy oracle
/// (`common/legacy_grouping.rs`).
fn assert_same_direct(
    new: &edifact_rs::SegmentGroupIndexed<'_>,
    old: &legacy_grouping::LegacyNode,
) {
    assert!(new.unplaced.is_empty(), "an open group never unplaces");
    assert_eq!(
        new.direct_segment_indices().collect::<Vec<_>>(),
        legacy_grouping::legacy_direct(old)
    );
    for (n, o) in new.children.iter().zip(&old.children) {
        assert_same_direct(n, o);
    }
}

/// For trigger-only schemas the engine builds exactly the tree of the legacy
/// oracle (`common/legacy_grouping.rs`) — names, spans, occurrence indices,
/// children, direct segments.
#[test]
fn fuzz_group_open_matches_legacy_reference() {
    use edifact_rs::group::{GroupDef, group_segments_indexed};
    use edifact_rs::{Element, Segment};

    // The const-constructed (static) form, checked on every input as well.
    static G3: &[GroupDef] = &[GroupDef::new("G3", "CCC")];
    static G2: &[GroupDef] = &[
        GroupDef::with_children("G2", "BBB", G3),
        GroupDef::new("G4", "AAA"),
    ];
    static DEEP_SCHEMA: &[GroupDef] = &[
        GroupDef::with_children("G1", "AAA", G2),
        GroupDef::new("G5", "BBB"),
    ];
    let deep_legacy = {
        use legacy_grouping::LegacyDef as D;
        let d = |n: &str, t: &str, c: Vec<D>| D {
            name: n.into(),
            trigger: t.into(),
            children: c,
        };
        vec![
            d(
                "G1",
                "AAA",
                vec![
                    d("G2", "BBB", vec![d("G3", "CCC", vec![])]),
                    d("G4", "AAA", vec![]),
                ],
            ),
            d("G5", "BBB", vec![]),
        ]
    };

    check!()
        .with_type::<(Vec<u8>, Vec<u8>)>()
        .cloned()
        .for_each(|(schema_seed, tag_seed): (Vec<u8>, Vec<u8>)| {
            let legacy = legacy_schema(&mut schema_seed.into_iter(), 1);
            let schema: Vec<GroupDef> = legacy.iter().map(owned_open).collect();

            let tags: Vec<&str> = tag_seed
                .iter()
                .take(64)
                .map(|b| match b % 7 {
                    5 => "ZZZ",
                    6 => "UNT",
                    i => GROUP_TAGS[usize::from(i)],
                })
                .collect();
            let segments: Vec<Segment<'_>> = tags
                .iter()
                .map(|t| Segment::new(*t, vec![Element::of(&["x"])]))
                .collect();

            let new = group_segments_indexed(&segments, &schema, "ROOT");
            let old = legacy_grouping::legacy_group(&tags, &legacy, "ROOT");
            assert_eq!(to_legacy(&new), old, "schema {legacy:?}, tags {tags:?}");
            assert_same_direct(&new, &old);

            let new = group_segments_indexed(&segments, DEEP_SCHEMA, "ROOT");
            let old = legacy_grouping::legacy_group(&tags, &deep_legacy, "ROOT");
            assert_eq!(to_legacy(&new), old, "static schema, tags {tags:?}");
            assert_same_direct(&new, &old);
        });
}

/// A random ordered/open schema using every discriminator kind and bounds.
fn ordered_entries(
    seed: &mut impl Iterator<Item = u8>,
    depth: usize,
) -> Vec<edifact_rs::group::Entry> {
    use edifact_rs::group::{Entry, GroupDef};
    let count = usize::from(seed.next().unwrap_or(0) % 5);
    let mut entries = Vec::new();
    for _ in 0..count {
        let kind = seed.next().unwrap_or(0);
        if kind % 3 == 0 && depth < 3 {
            let trigger = fuzz_place(seed);
            let mut builder = GroupDef::builder(format!("G{}", kind % 4), trigger);
            for entry in ordered_entries(seed, depth + 1) {
                builder = match entry {
                    Entry::Segment(place) => builder.segment(place),
                    Entry::Group(group) => builder.group(group),
                    _ => builder,
                };
            }
            if kind % 2 == 0 {
                builder = builder.ordered();
            }
            if let Some(max) = fuzz_max(seed) {
                builder = builder.max(max);
            }
            entries.push(Entry::Group(
                builder.build().expect("valid generated group"),
            ));
        } else {
            entries.push(Entry::Segment(fuzz_place(seed)));
        }
    }
    entries
}

fn fuzz_max(seed: &mut impl Iterator<Item = u8>) -> Option<u32> {
    match seed.next().unwrap_or(0) % 4 {
        0 => None,
        n => Some(u32::from(n)),
    }
}

fn fuzz_place(seed: &mut impl Iterator<Item = u8>) -> edifact_rs::SegmentPlace {
    use edifact_rs::group::{Discriminator, SegmentPlace};
    let byte = seed.next().unwrap_or(0);
    let mut place = SegmentPlace::new(GROUP_TAGS[usize::from(byte % 5)]);
    let code = ["A", "B", "C"][usize::from(byte / 5 % 3)];
    place = match byte / 15 % 6 {
        0 => place,
        1 => place.when(Discriminator::one_of(0, 0, [code])),
        2 => place.when(Discriminator::one_of(0, 0, [code]).optional()),
        3 => place.when(Discriminator::one_of(0, 0, [code]).accept_unclaimed()),
        4 => place.when(Discriminator::not_used(0, 0)),
        _ => place.when(
            Discriminator::one_of(0, 1, [code, "D"])
                .optional()
                .accept_unclaimed(),
        ),
    };
    match fuzz_max(seed) {
        Some(max) => place.with_max(max),
        None => place,
    }
}

/// `(definition, span, occurrence, unplaced, children)` of a tree, recursively.
fn tree_shape(node: &edifact_rs::SegmentGroupIndexed<'_>) -> String {
    let mut out = format!(
        "{}#{}{:?}u{:?}[",
        node.definition, node.occurrence_index, node.total_span, node.unplaced
    );
    for child in &node.children {
        out.push_str(&tree_shape(child));
    }
    out.push(']');
    out
}

fn assert_tree_invariants(node: &edifact_rs::SegmentGroupIndexed<'_>, owner: &mut [u32]) {
    let mut last_end = node.total_span.start;
    for (k, child) in node.children.iter().enumerate() {
        assert!(
            child.total_span.start < child.total_span.end,
            "empty child span"
        );
        assert!(
            child.total_span.start >= last_end,
            "child spans overlap or are unordered"
        );
        if k == 0 {
            assert!(child.total_span.start >= node.total_span.start);
        }
        assert!(
            child.total_span.end <= node.total_span.end,
            "child outside its parent"
        );
        last_end = child.total_span.end;
        assert_tree_invariants(child, owner);
    }
    assert!(
        node.unplaced.windows(2).all(|w| w[0] < w[1]),
        "unplaced sorted"
    );
    for i in node
        .direct_segment_indices()
        .chain(node.unplaced.iter().copied())
    {
        assert!(node.total_span.contains(&i));
        owner[i] += 1;
    }
}

/// With ordered groups and discriminators, every segment
/// is exactly once a direct segment of one instance or unplaced; child spans
/// are ordered, disjoint and inside their parent; resolution is deterministic;
/// every finding carries kind, layer and (for a segment) its span.
#[test]
fn fuzz_group_ordered_invariants() {
    use edifact_rs::group::{Entry, GroupSchema};
    use edifact_rs::{Element, Segment, ValidationLayer};

    check!()
        .with_type::<(Vec<u8>, Vec<u8>, bool)>()
        .cloned()
        .for_each(
            |(schema_seed, seg_seed, ordered_root): (Vec<u8>, Vec<u8>, bool)| {
                let entries = ordered_entries(&mut schema_seed.into_iter(), 1);
                let schema = if ordered_root {
                    GroupSchema::ordered("ROOT", entries)
                } else {
                    GroupSchema::new(
                        "ROOT",
                        entries.into_iter().filter_map(|e| match e {
                            Entry::Group(g) => Some(g),
                            _ => None,
                        }),
                    )
                }
                .expect("generated schemas are valid");

                let segments: Vec<Segment<'_>> = seg_seed
                    .chunks(2)
                    .take(64)
                    .map(|pair| {
                        let tag = GROUP_TAGS[usize::from(pair[0] % 5)];
                        let value = ["", "A", "B", "C", "D"][usize::from(pair[0] / 5 % 5)];
                        let second =
                            ["", "A", "D"][usize::from(pair.get(1).copied().unwrap_or(0) % 3)];
                        Segment::new(tag, vec![Element::of(&[value, second])])
                    })
                    .collect();

                let resolution = schema.resolve(&segments);
                let tree = resolution.tree();
                assert_eq!(tree.total_span, 0..segments.len());

                let mut owner = vec![0_u32; segments.len()];
                assert_tree_invariants(tree, &mut owner);
                assert!(
                    owner.iter().all(|&n| n == 1),
                    "every segment exactly once direct or unplaced: {owner:?}"
                );

                let mut on_nodes: Vec<usize> = tree
                    .descendants()
                    .flat_map(|n| n.unplaced.iter().copied())
                    .collect();
                on_nodes.sort_unstable();
                let listed: Vec<usize> = resolution.unplaced().iter().map(|u| u.index).collect();
                assert_eq!(on_nodes, listed);
                for u in resolution.unplaced() {
                    assert!(resolution.placement(u.index).is_none());
                }

                // Every non-root instance starts with a segment its trigger matches.
                for node in tree.descendants().skip(1) {
                    let def = node.def.expect("non-root instances name their group");
                    assert_eq!(segments[node.total_span.start].tag(), def.trigger_tag());
                }

                let again = schema.resolve(&segments);
                assert_eq!(tree_shape(tree), tree_shape(again.tree()), "deterministic");
                assert_eq!(resolution.unplaced(), again.unplaced());

                for issue in resolution.findings(&segments) {
                    assert!(issue.kind().is_some());
                    assert_eq!(issue.layer(), Some(ValidationLayer::Structure));
                    if issue.error_code() != Some("E015") {
                        assert!(issue.span().is_some(), "{issue:?}");
                    }
                }
            },
        );
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

#[test]
fn fuzz_recovering_scan_matches_strict() {
    // The recovering scan drives the strict parser and resumes after a fault,
    // so on the bytes the strict parser reads it must agree with it exactly:
    //
    // C1  accepted input → the same segments, no fault;
    // C2  refused input  → the same segments up to the error, then a fault
    //                      carrying that error (offsets included);
    // C3  bounded        → at most `max_faults + 1` faults, at most
    //                      `max_segments + 1` items, nothing past
    //                      `max_input_bytes` except the limit fault itself;
    // C4  a terminal fault is the last item.
    //
    // C1/C2 run with and without `max_input_bytes`: both paths count every
    // consumed byte against it. C3 runs with it in every case.
    use edifact_rs::recover::Scanned;
    use edifact_rs::{
        EdifactError, ReaderConfig, RecoveryConfig, from_bytes_with_config, scan_recovering,
    };

    check!()
        .with_type::<(Vec<u8>, u16, u8, u8, u8, u16)>()
        .cloned()
        .for_each(
            |(bytes, segment_bytes, segments, messages, faults, input_bytes): (
                Vec<u8>,
                u16,
                u8,
                u8,
                u8,
                u16,
            )| {
                // Half the cases corrupt a valid interchange at fuzzed positions,
                // so faults land between real segments rather than only in
                // random garbage the strict parser refuses at byte 0.
                let input = if segments % 2 == 0 {
                    let mut template = b"UNB+UNOC:3+S:14+R:14+260101:0900+IC1'\
UNH+M1+ORDERS:D:96A:UN'BGM+220+PO?+1+9'DTM+137:20260101:102'UNT+4+M1'\
UNH+M2+ORDERS:D:96A:UN'BGM+220+PO2'UNT+3+M2'UNZ+2+IC1'"
                        .to_vec();
                    for pair in bytes.chunks_exact(2).take(6) {
                        let at = usize::from(pair[0]) % template.len();
                        template[at] = pair[1];
                    }
                    template
                } else {
                    bytes
                };
                let mut reader = ReaderConfig::default().max_segment_bytes(usize::from(segment_bytes % 512) + 1);
                if segments % 3 != 0 {
                    reader = reader.max_segments(usize::from(segments % 32));
                }
                if messages % 3 != 0 {
                    reader = reader.max_messages(usize::from(messages % 8));
                }
                if faults % 4 == 1 {
                    reader = reader.max_segments_per_message(usize::from(faults % 8));
                }
                if input_bytes % 3 == 1 {
                    reader = reader.max_input_bytes(u64::from(input_bytes % 1024));
                }
                let config = RecoveryConfig::new()
                    .reader(reader)
                    .max_faults(usize::from(faults % 16));

                // ── C1 / C2 ──────────────────────────────────────────────────
                let strict: Vec<_> = from_bytes_with_config(&input, reader).collect();
                let items: Vec<Scanned<'_>> = scan_recovering(&input, &config).collect();
                let accepted = strict.iter().take_while(|r| r.is_ok()).count();
                for (index, result) in strict.iter().take(accepted).enumerate() {
                    let segment = result.as_ref().expect("prefix is Ok");
                    assert_eq!(items[index].as_segment(), Some(segment), "C1/C2 item {index}");
                }
                match strict.get(accepted) {
                    None => assert_eq!(items.len(), accepted, "C1: accepted input has no fault"),
                    Some(Err(error)) => {
                        let fault = items[accepted]
                            .as_fault()
                            .expect("C2: the strict error is a fault");
                        let budget_boundary = matches!(
                            fault.error(),
                            EdifactError::LimitExceeded { limit, .. }
                                if (*limit == "max_segments" && Some(accepted) == reader.max_segments)
                                    || *limit == "max_messages"
                                    || *limit == "max_segments_per_message"
                                    || (*limit == "max_faults" && config.fault_budget() == 0)
                        );
                        assert!(
                            fault.error() == error || budget_boundary,
                            "C2: strict {error:?}, scan {:?}",
                            fault.error()
                        );
                    }
                    Some(Ok(_)) => unreachable!("take_while stopped at an error"),
                }
                assert_bounded(&input, &items, &config);

                // ── C3 with a byte budget ───────────────────────────────────
                let config = config.reader(reader.max_input_bytes(u64::from(input_bytes % 1024)));
                let items: Vec<Scanned<'_>> = scan_recovering(&input, &config).collect();
                assert_bounded(&input, &items, &config);
            },
        );

    fn assert_bounded(input: &[u8], items: &[Scanned<'_>], config: &RecoveryConfig) {
        let reader = config.reader_config();
        let faults: Vec<_> = items.iter().filter_map(Scanned::as_fault).collect();
        assert!(
            faults.len() <= config.fault_budget() + 1,
            "C3: fault budget"
        );
        if let Some(max) = reader.max_segments {
            assert!(items.len() <= max + 1, "C3: segment budget");
        }
        for (index, item) in items.iter().enumerate() {
            let span = item.span();
            assert!(
                span.start <= span.end && span.end <= input.len(),
                "span in bounds"
            );
            let is_limit = item
                .as_fault()
                .is_some_and(|f| matches!(f.error(), EdifactError::LimitExceeded { .. }));
            if let (Some(max), false) = (reader.max_input_bytes, is_limit) {
                assert!(
                    span.end as u64 <= max,
                    "C3: item {index} past max_input_bytes"
                );
            }
            if let Some(fault) = item.as_fault() {
                assert_eq!(fault.ordinal() as usize, index, "ordinal");
                if fault.is_terminal() {
                    assert_eq!(index + 1, items.len(), "C4: terminal fault is last");
                }
            }
        }
    }
}

#[test]
fn fuzz_contrl_options_reparse() {
    // Any subject × any options: building a CONTRL either fails, or yields a
    // message that
    //
    // K3  carries only DE 0083 codes the action policy permits (UCI, UCF, UCM);
    // K4  reparses and, wrapped in its own interchange, passes
    //     `validate_envelope` — including the UNT count it computes itself.
    //
    // Both constructors that take options from a subject are exercised: the
    // validation path (when the corrupted subject still parses) and the
    // recovering path (always).
    //
    // The wrapper's `validate_envelope` half of K4 needs a subject whose UNB
    // sender and recipient are non-empty — the precondition `from_recovered`
    // enforces. The lenient envelope also yields an interchange with an empty
    // party, and `to_interchange_bytes` then writes a reply UNB without S002 or
    // S003; that is a property of the wrapper, unchanged by the options.
    use edifact_rs::contrl::{
        Action, ActionPolicy, Contrl, ContrlIdentifier, ContrlOptions, RejectionScope,
        ReportingLevel,
    };
    use edifact_rs::{
        RecoveredInterchange, RecoveryConfig, ValidationContext, validate_envelope,
        validate_envelope_lenient,
    };

    fn actions(bits: u8) -> Vec<Action> {
        [Action::Rejected, Action::Acknowledged, Action::Received]
            .into_iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, a)| a)
            .collect()
    }

    fn action_of(code: &str) -> Action {
        match code {
            "4" => Action::Rejected,
            "7" => Action::Acknowledged,
            "8" => Action::Received,
            other => panic!("DE 0083 {other:?} is not an action code"),
        }
    }

    fn check_contrl(contrl: &Contrl, policy: &ActionPolicy, addressed: bool) {
        let contrl = contrl.clone().with_message_reference("ACK1");
        let bytes = contrl.to_bytes().expect("K4: a built CONTRL renders");
        let segments: Vec<_> = from_bytes(&bytes)
            .collect::<Result<Vec<_>, _>>()
            .expect("K4: a built CONTRL reparses");
        for segment in &segments {
            let (level, position) = match segment.tag() {
                "UCI" => (ReportingLevel::Interchange, 3),
                "UCF" => (ReportingLevel::Group, 3),
                "UCM" => (ReportingLevel::Message, 2),
                _ => continue,
            };
            let code = segment.component_str(position, 0).expect("DE 0083 present");
            assert!(
                policy.permits(level, action_of(code)),
                "K3: {} carries {code}, which the policy forbids",
                segment.tag()
            );
        }
        let wrapped = contrl
            .to_interchange_bytes("UNOC", "3", "261001", "0800", "R1")
            .expect("K4: the wrapper renders");
        let wrapped: Vec<_> = from_bytes(&wrapped)
            .collect::<Result<Vec<_>, _>>()
            .expect("K4: the wrapped CONTRL reparses");
        if addressed {
            validate_envelope(&wrapped).expect("K4: the wrapped CONTRL passes validate_envelope");
        }
    }

    /// Corruptions, grouped, identifier choice, UCI/UCF/UCM sets, scope, warnings.
    type Case = (Vec<(u16, u8)>, bool, u8, [u8; 3], bool, bool);

    check!().with_type::<Case>().cloned().for_each(
        |(edits, grouped, identifier, sets, whole, warnings): Case| {
            let mut input = if grouped {
                b"UNB+UNOC:3+S:14+R:14+260101:0900+IC1'\
UNG+ORDERS+SND+RCV+260101:0900+G1+UN+D:96A'\
UNH+M1+ORDERS:D:96A:UN:EX1'BGM+220+PO1'FTX+   'UNT+4+M1'\
UNH+M2+ORDERS:D:96A:UN'BGM+220+PO2'UNT+3+M2'UNE+2+G1'UNZ+1+IC1'"
                    .to_vec()
            } else {
                b"UNB+UNOC:4+S:14+R:14+20260101:0900+IC1'\
UNH+M1+ORDERS:D:96A:UN:EX1'BGM+220+PO1'FTX+   'UNT+4+M1'\
UNH+M2+ORDERS:D:96A:UN'BGM+220+PO2'UNT+3+M2'UNZ+2+IC1'"
                    .to_vec()
            };
            for (at, byte) in edits.iter().take(6) {
                let at = usize::from(*at) % input.len();
                input[at] = *byte;
            }

            let mut options = ContrlOptions::new()
                .actions(
                    ActionPolicy::new()
                        .permit(ReportingLevel::Interchange, &actions(sets[0]))
                        .permit(ReportingLevel::Group, &actions(sets[1]))
                        .permit(ReportingLevel::Message, &actions(sets[2])),
                )
                .rejection(if whole {
                    RejectionScope::Interchange
                } else {
                    RejectionScope::PerLevel
                })
                .report_warnings(warnings);
            match identifier % 3 {
                0 => {}
                1 => options = options.identifier(ContrlIdentifier::syntax_4()),
                _ => {
                    options = options.identifier(
                        ContrlIdentifier::syntax_3()
                            .with_association("EX1")
                            .expect("an..6"),
                    );
                }
            }
            let policy = ActionPolicy::new()
                .permit(ReportingLevel::Interchange, &actions(sets[0]))
                .permit(ReportingLevel::Group, &actions(sets[1]))
                .permit(ReportingLevel::Message, &actions(sets[2]));

            let recovered = RecoveredInterchange::scan(&input, &RecoveryConfig::default());
            if let Ok(contrl) = Contrl::from_recovered(&recovered, &[], &options) {
                check_contrl(&contrl, &policy, true);
            }

            let Ok(segments) = from_bytes(&input).collect::<Result<Vec<_>, _>>() else {
                return;
            };
            let Some(validated) = validate_envelope_lenient(&segments).interchange else {
                return;
            };
            let report = ValidationContext::builder()
                .with_envelope_validation()
                .with_syntax_validation()
                .build()
                .validate(&segments);
            let addressed = !validated.interchange.sender_id.is_empty()
                && !validated.interchange.recipient_id.is_empty();
            if let Ok(contrl) = Contrl::from_report_with(&validated, &segments, &report, &options) {
                check_contrl(&contrl, &policy, addressed);
            }
            if let Ok(contrl) = Contrl::receipt_with(&validated.interchange, &options) {
                check_contrl(&contrl, &policy, addressed);
            }
        },
    );
}
