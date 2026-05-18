use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use edifact_rs::{
    ProfileRulePack, ServiceStringAdvice, Tokenizer, ValidationContext, ValidationIssue,
    ValidationLayer, ValidationReport, ValidationSeverity, Validator, from_bytes, from_reader,
    to_bytes,
};
use std::io::{Cursor, Read};

mod bench_data;
use bench_data::{one_mb, sample_msg, sample_segments};

fn bench_tokenizer(c: &mut Criterion) {
    let mut group = c.benchmark_group("tokenizer");

    let small = sample_msg();
    group.throughput(Throughput::Bytes(small.len() as u64));
    group.bench_with_input(BenchmarkId::new("small", small.len()), &small, |b, data| {
        b.iter(|| {
            let ssa = ServiceStringAdvice::from_bytes(data);
            let tokens: Vec<_> = Tokenizer::new(black_box(data), ssa).collect();
            black_box(tokens);
        });
    });

    let large = one_mb();
    group.throughput(Throughput::Bytes(large.len() as u64));
    group.bench_with_input(BenchmarkId::new("1mb", large.len()), &large, |b, data| {
        b.iter(|| {
            let ssa = ServiceStringAdvice::from_bytes(data);
            let tokens: Vec<_> = Tokenizer::new(black_box(data), ssa).collect();
            black_box(tokens);
        });
    });

    group.finish();
}

fn bench_parser(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser");

    let small = sample_msg();
    group.throughput(Throughput::Bytes(small.len() as u64));
    group.bench_with_input(BenchmarkId::new("small", small.len()), &small, |b, data| {
        b.iter(|| {
            let segments = from_bytes(black_box(data))
                .collect::<Result<Vec<_>, _>>()
                .expect("bench fixture must be valid EDIFACT");
            black_box(segments);
        });
    });

    let large = one_mb();
    group.throughput(Throughput::Bytes(large.len() as u64));
    group.bench_with_input(BenchmarkId::new("1mb", large.len()), &large, |b, data| {
        b.iter(|| {
            let segments = from_bytes(black_box(data))
                .collect::<Result<Vec<_>, _>>()
                .expect("bench fixture must be valid EDIFACT");
            black_box(segments);
        });
    });

    group.finish();
}

fn bench_reader(c: &mut Criterion) {
    let mut group = c.benchmark_group("reader");

    let data = one_mb();
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("1mb", |b| {
        b.iter(|| {
            let cursor = std::io::Cursor::new(data);
            let segments = from_reader(cursor).expect("bench fixture must be valid EDIFACT");
            black_box(segments);
        });
    });

    group.bench_function("parse_reader_chunked", |b| {
        b.iter(|| {
            let reader = ChunkedReader::new(data, 4 * 1024);
            let segments = from_reader(reader).expect("bench fixture must be valid EDIFACT");
            black_box(segments);
        });
    });

    group.finish();
}

fn bench_writer(c: &mut Criterion) {
    let mut group = c.benchmark_group("writer");

    let segments = sample_segments();
    let bytes = to_bytes(&segments).expect("serialization of known-good segments must not fail");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("sample_message", |b| {
        b.iter(|| {
            let out = to_bytes(black_box(&segments)).expect("serialization of known-good segments must not fail");
            black_box(out);
        });
    });

    group.finish();
}

fn bench_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("validation");

    struct NoopValidator;

    impl Validator for NoopValidator {
        fn validate_batch(
            &self,
            _segments: &[edifact_rs::Segment<'_>],
            _report: &mut ValidationReport,
        ) {
        }
    }

    let segments = from_bytes(sample_msg())
        .collect::<Result<Vec<_>, _>>()
        .expect("bench fixture must be valid EDIFACT");
    let structure_context = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(ValidationLayer::Structure, NoopValidator)
        .build();
    let custom_pack = ProfileRulePack::builder("bench-custom-pack")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let has_bgm = segments.iter().any(|seg| seg.tag == "BGM");
            if has_bgm {
                None
            } else {
                Some(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        "BGM segment missing in ORDERS message",
                    )
                    .with_rule_id("bench.custom.bgm_required"),
                )
            }
        });
    let profile_context = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(ValidationLayer::Structure, NoopValidator)
        .with_profile_pack(custom_pack)
        .build();

    let pack_a = ProfileRulePack::builder("bench-pack-a")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let has_dtm = segments.iter().any(|seg| seg.tag == "DTM");
            if has_dtm {
                None
            } else {
                Some(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        "DTM segment missing in ORDERS message",
                    )
                    .with_rule_id("bench.pack_a.dtm_required"),
                )
            }
        });
    let pack_b = ProfileRulePack::builder("bench-pack-b")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let has_nad = segments.iter().any(|seg| seg.tag == "NAD");
            if has_nad {
                None
            } else {
                Some(
                    ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "NAD segment missing in ORDERS message",
                    )
                    .with_rule_id("bench.pack_b.nad_required"),
                )
            }
        });
    let composed_profile_context = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(ValidationLayer::Structure, NoopValidator)
        .with_profile_pack(pack_a.merge(pack_b))
        .build();

    group.bench_function("validate_structure_orders", |b| {
        b.iter(|| {
            let report = structure_context.validate_lenient(black_box(&segments));
            black_box(report);
        });
    });

    group.bench_function("validate_profile_custom_pack", |b| {
        b.iter(|| {
            let report = profile_context.validate_lenient(black_box(&segments));
            black_box(report);
        });
    });

    group.bench_function("validate_profile_composed_packs", |b| {
        b.iter(|| {
            let report = composed_profile_context.validate_lenient(black_box(&segments));
            black_box(report);
        });
    });

    let large_data = one_mb();
    let large_segments = from_bytes(large_data)
        .collect::<Result<Vec<_>, _>>()
        .expect("bench fixture must be valid EDIFACT");
    let large_context = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(ValidationLayer::Structure, NoopValidator)
        .build();
    group.throughput(Throughput::Bytes(large_data.len() as u64));
    group.bench_function("parse_large_message", |b| {
        b.iter(|| {
            let parsed = from_bytes(black_box(large_data))
                .collect::<Result<Vec<_>, _>>()
                .expect("bench fixture must be valid EDIFACT");
            let report = large_context.validate_lenient(black_box(&parsed));
            black_box(report);
        });
    });

    group.bench_function("validate_large_message", |b| {
        b.iter(|| {
            let report = large_context.validate_lenient(black_box(&large_segments));
            black_box(report);
        });
    });

    group.finish();
}

struct ChunkedReader<'a> {
    data: &'a [u8],
    cursor: Cursor<&'a [u8]>,
    chunk_size: usize,
}

impl<'a> ChunkedReader<'a> {
    fn new(data: &'a [u8], chunk_size: usize) -> Self {
        Self {
            data,
            cursor: Cursor::new(data),
            chunk_size,
        }
    }
}

impl Read for ChunkedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cursor.position() as usize >= self.data.len() {
            return Ok(0);
        }
        let max = buf.len().min(self.chunk_size);
        self.cursor.read(&mut buf[..max])
    }
}

criterion_group!(
    benches,
    bench_tokenizer,
    bench_parser,
    bench_reader,
    bench_writer,
    bench_validation
);
criterion_main!(benches);
