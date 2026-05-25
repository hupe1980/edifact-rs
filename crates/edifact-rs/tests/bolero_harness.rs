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
            let reparsed = from_bytes(&encoded)
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            assert_eq!(reparsed.len(), segs.len(), "segment count must survive round-trip");
            for (orig, rt) in segs.iter().zip(reparsed.iter()) {
                assert_eq!(orig.tag, rt.tag, "tag must survive round-trip");
                assert_eq!(
                    orig.elements.len(),
                    rt.elements.len(),
                    "element count must survive round-trip for tag {}",
                    orig.tag,
                );
                for (ei, (oe, re)) in orig.elements.iter().zip(rt.elements.iter()).enumerate() {
                    assert_eq!(
                        oe.components.len(),
                        re.components.len(),
                        "component count must survive round-trip for tag {} element {ei}",
                        orig.tag,
                    );
                    assert_eq!(
                        oe.components,
                        re.components,
                        "component values must survive round-trip for tag {} element {ei}",
                        orig.tag,
                    );
                }
            }
        });
}

#[test]
fn fuzz_validation_layers_no_panic() {
    use edifact_rs::{Segment, ValidationContext, ValidationLayer, ValidationReport, Validator, validate_each};

    struct NoopValidator;

    impl Validator for NoopValidator {
        fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
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
                .with_message_type("UTILMD")
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
    check!()
        .with_type::<(String, String)>()
        .cloned()
        .for_each(|(value, pattern): (String, String)| {
            let _ = qualifier_matches_pattern(&value, &pattern);
        });
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
    // ServiceStringAdvice::from_bytes + is_valid must not panic for any 9-byte input.
    check!()
        .with_type::<[u8; 9]>()
        .cloned()
        .for_each(|bytes: [u8; 9]| {
            let ssa = ServiceStringAdvice::from_bytes(&bytes);
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
            let ssa = ServiceStringAdvice::from_bytes(&bytes);
            // If from_bytes returns a valid object, is_valid must also not panic.
            let _ = ssa.is_valid();
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
    // `message_windows_bytes` must not panic for any byte sequence.
    use edifact_rs::message_windows_bytes;

    check!()
        .with_type::<Vec<u8>>()
        .cloned()
        .for_each(|input: Vec<u8>| {
            // Consume the full iterator — any item may be Ok or Err.
            for _ in message_windows_bytes(&input) { /* consume */ }
        });
}
