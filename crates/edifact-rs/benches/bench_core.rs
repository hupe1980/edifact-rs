//! divan micro-benchmarks for `edifact-rs`.
//!
//! Run with:
//!   cargo bench -p edifact-rs --bench bench_core
//!
//! Every closure returns its result and every input is passed through
//! `black_box`.  Without both, LLVM is free to delete the measured work: divan
//! black-boxes the value a closure *returns*, so a closure ending in `let _ = …`
//! measures nothing.  Fixtures are also hoisted out of the timed region — the
//! sample message is a compile-time literal and would otherwise be constant-folded
//! into the measurement.

use divan::{Bencher, black_box};
use edifact_rs::{
    ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity, from_bytes,
    from_reader_collect, segments_to_bytes, validate_envelope,
};
mod bench_data;
use bench_data::{one_mb, sample_msg, sample_segments};

fn main() {
    divan::main();
}

// ── tokenizer benchmarks ───────────────────────────────────────────────────────

#[divan::bench]
fn bench_tokenize_small(b: Bencher) {
    let data = sample_msg();
    b.bench(|| {
        let input = black_box(data);
        let ssa = edifact_rs::ServiceStringAdvice::from_bytes_unchecked(input);
        edifact_rs::Tokenizer::new(input, ssa).count()
    });
}

#[divan::bench]
fn bench_tokenize_1mb(b: Bencher) {
    let data = one_mb();
    b.bench(|| {
        let input = black_box(data);
        let ssa = edifact_rs::ServiceStringAdvice::from_bytes_unchecked(input);
        edifact_rs::Tokenizer::new(input, ssa).count()
    });
}

// ── parser benchmarks ──────────────────────────────────────────────────────────

#[divan::bench]
fn bench_parse_small(b: Bencher) {
    let data = sample_msg();
    b.bench(|| {
        from_bytes(black_box(data))
            .collect::<Result<Vec<_>, _>>()
            .expect("bench fixture must be valid EDIFACT")
    });
}

#[divan::bench]
fn bench_parse_1mb(b: Bencher) {
    let data = one_mb();
    b.bench(|| {
        from_bytes(black_box(data))
            .collect::<Result<Vec<_>, _>>()
            .expect("bench fixture must be valid EDIFACT")
    });
}

#[divan::bench]
fn bench_parse_reader_1mb(b: Bencher) {
    let data = one_mb();
    b.bench(|| {
        from_reader_collect(std::io::Cursor::new(black_box(data)))
            .expect("bench fixture must be valid EDIFACT")
    });
}

// ── writer / serialize benchmarks ─────────────────────────────────────────────

#[divan::bench]
fn bench_serialize_sample_message(b: Bencher) {
    let segments = sample_segments();
    b.bench(|| {
        segments_to_bytes(black_box(&segments))
            .expect("serialization of known-good segments must not fail")
    });
}

/// Escape-heavy payload: every value carries delimiters, so the writer takes the
/// allocating `Cow::Owned` path on every component rather than the borrow path
/// the other benchmarks measure.
#[divan::bench]
fn bench_serialize_escape_heavy(b: Bencher) {
    use edifact_rs::{Element, Segment};
    let segments: Vec<Segment<'static>> = (0..64)
        .map(|_| {
            Segment::new(
                "FTX",
                vec![
                    Element::of(&["a+b", "c:d"]),
                    Element::of(&["e?f", "g'h"]),
                    Element::of(&["plain value"]),
                ],
            )
        })
        .collect();
    b.bench(|| segments_to_bytes(black_box(&segments)).expect("escape-heavy write"));
}

// ── round-trip benchmark ───────────────────────────────────────────────────────

#[divan::bench]
fn bench_roundtrip_small(b: Bencher) {
    let data = sample_msg();
    b.bench(|| {
        let segs: Vec<_> = from_bytes(black_box(data))
            .collect::<Result<Vec<_>, _>>()
            .expect("bench fixture must be valid EDIFACT");
        segments_to_bytes(&segs).expect("serialization of known-good segments must not fail")
    });
}

// ── validation benchmarks ─────────────────────────────────────────────────────
//
// These measure real validation work.  Benchmarking a no-op validator only
// measures dispatch and report overhead, which is not the number anyone tuning
// this library needs.

#[divan::bench]
fn bench_validate_envelope(b: Bencher) {
    let data = sample_msg();
    let segs: Vec<_> = from_bytes(data)
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture");
    b.bench(|| validate_envelope(black_box(&segs)).is_ok());
}

/// A profile pack with enough rules to be representative of a real MIG profile.
///
/// Each rule sweeps the whole segment slice rather than stopping at the first
/// match.  A `find`-and-return rule short-circuits after a couple of comparisons,
/// so a benchmark built on those measures dispatch overhead and reports the same
/// number for a 200-byte message and a 1 MB one.
fn realistic_pack() -> ProfileRulePack {
    let mut pack = ProfileRulePack::new("BENCH-PROFILE").for_message_type("ORDERS");
    for (i, tag) in ["BGM", "DTM", "NAD", "LIN", "QTY", "PRI"]
        .iter()
        .enumerate()
    {
        let tag = *tag;
        pack = pack.with_stateless_rule_fn(move |segments, issues| {
            // Full sweep: count occurrences and check each one's qualifier,
            // mirroring a cardinality + code-value rule from a real MIG.
            let mut seen = 0usize;
            for seg in segments.iter().filter(|s| s.tag == tag) {
                seen += 1;
                if seg.get_element(0).and_then(|e| e.get_component(0)) == Some("") {
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Warning,
                            format!("{tag} element 0 is empty"),
                        )
                        .with_segment(tag),
                    );
                }
            }
            if seen == 0 {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!("required segment {tag} missing"),
                    )
                    .with_segment(tag)
                    .with_rule_id(format!("BENCH-{i:03}")),
                );
            }
        });
    }
    pack
}

#[divan::bench]
fn bench_validate_profile_pack_small(b: Bencher) {
    let data = sample_msg();
    let segs: Vec<_> = from_bytes(data)
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture");
    let ctx = ValidationContext::builder()
        .with_envelope_validation()
        .with_message_type("ORDERS")
        .with_profile_pack(realistic_pack())
        .build();
    b.bench(|| ctx.validate_lenient(black_box(&segs)).total_issues());
}

#[divan::bench]
fn bench_validate_profile_pack_1mb(b: Bencher) {
    let data = one_mb();
    let segs: Vec<_> = from_bytes(data)
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture");
    let ctx = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_profile_pack(realistic_pack())
        .build();
    b.bench(|| ctx.validate_lenient(black_box(&segs)).total_issues());
}

/// End-to-end: parse then validate, which is what a real ingest pipeline does.
#[divan::bench]
fn bench_parse_and_validate_1mb(b: Bencher) {
    let data = one_mb();
    let ctx = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_profile_pack(realistic_pack())
        .build();
    b.bench(|| {
        let segs: Vec<_> = from_bytes(black_box(data))
            .collect::<Result<Vec<_>, _>>()
            .expect("bench fixture must be valid EDIFACT");
        ctx.validate_lenient(&segs).total_issues()
    });
}
