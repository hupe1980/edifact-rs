//! divan micro-benchmarks for `edifact-rs`.
//!
//! Run with:
//!   cargo bench -p edifact-rs --bench bench_core
//!
//! Save a baseline:
//!   cargo bench -p edifact-rs --bench bench_core

use divan::Bencher;
use edifact_rs::{from_bytes, from_reader, to_bytes};
mod bench_data;
use bench_data::{one_mb, sample_msg, sample_segments};

fn main() {
    divan::main();
}

// ── tokenizer benchmarks ───────────────────────────────────────────────────────

#[divan::bench]
fn bench_tokenize_small(b: Bencher) {
    b.bench(|| {
        let ssa = edifact_rs::ServiceStringAdvice::from_bytes(sample_msg());
        let _: Vec<_> = edifact_rs::Tokenizer::new(sample_msg(), ssa).collect();
    });
}

#[divan::bench]
fn bench_tokenize_1mb(b: Bencher) {
    let data = one_mb();
    b.bench(|| {
        let ssa = edifact_rs::ServiceStringAdvice::from_bytes(data);
        let _: Vec<_> = edifact_rs::Tokenizer::new(data, ssa).collect();
    });
}

// ── parser benchmarks ──────────────────────────────────────────────────────────

#[divan::bench]
fn bench_parse_small(b: Bencher) {
    b.bench(|| {
        let _ = from_bytes(sample_msg())
            .collect::<Result<Vec<_>, _>>()
            .expect("bench fixture must be valid EDIFACT");
    });
}

#[divan::bench]
fn bench_parse_1mb(b: Bencher) {
    let data = one_mb();
    b.bench(|| {
        let _ = from_bytes(data).collect::<Result<Vec<_>, _>>().expect("bench fixture must be valid EDIFACT");
    });
}

#[divan::bench]
fn bench_parse_reader_1mb(b: Bencher) {
    let data = one_mb();
    b.bench(|| {
        let cursor = std::io::Cursor::new(data);
        let _ = from_reader(cursor).expect("bench fixture must be valid EDIFACT");
    });
}

// ── writer / serialize benchmarks ─────────────────────────────────────────────

#[divan::bench]
fn bench_serialize_sample_message(b: Bencher) {
    let segments = sample_segments();

    b.bench(|| {
        let _ = to_bytes(&segments).expect("serialization of known-good segments must not fail");
    });
}

// ── round-trip benchmark ───────────────────────────────────────────────────────

#[divan::bench]
fn bench_roundtrip_small(b: Bencher) {
    b.bench(|| {
        let segs: Vec<_> = from_bytes(sample_msg())
            .collect::<Result<Vec<_>, _>>()
            .expect("bench fixture must be valid EDIFACT");
        let _bytes = to_bytes(&segs).expect("serialization of known-good segments must not fail");
    });
}
