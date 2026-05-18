# Performance ⚡

`edifact-rs` is designed for high-throughput, low-allocation EDIFACT processing.
This guide explains the key design decisions, how to measure them, and how to tune
for production workloads.

---

## Zero-copy parsing

`from_bytes(input: &[u8])` is the fastest entry point. It yields `Segment<'a>`
values that borrow directly from the input slice:

```rust
let input: &[u8] = b"BGM+220+PO-4711+9'";
// No heap allocations for the segment itself
let seg = edifact_rs::from_bytes(input).next().unwrap().unwrap();
assert_eq!(seg.tag, "BGM");
```

- **`Segment<'a>`** borrows the tag, element values, and component slices from `input`
- **`Cow::Borrowed`** is returned for components that contain no release characters — zero copy
- **`Cow::Owned`** is returned only when a component contains escape sequences (e.g. `?+` → `+`)

For a 1 MB interchange, `from_bytes` typically completes in < 5 ms on a modern CPU.

---

## `SmallVec` inline storage

Elements and components use `SmallVec<[T; 4]>` (with the `union` feature enabled).
This means segments with ≤ 4 components per element avoid all heap allocations
entirely — which covers the large majority of real-world EDIFACT segments.

---

## `from_reader_iter` — O(1) memory streaming

`from_reader_iter(reader)` is the reader-based API with minimal memory overhead:

```rust
use edifact_rs::from_reader_iter;

let f = std::fs::File::open("large.edi")?;
for seg in from_reader_iter(f) {
    let seg = seg?;  // OwnedSegment (segment-sized allocation only)
    // process and drop — O(1) peak memory
}
# Ok::<(), edifact_rs::EdifactError>(())
```

It holds at most one `OwnedSegment` in memory at a time. Use this instead of
`from_reader` when processing interchanges that are larger than available RAM.

---

## `edifact_deserialize_owned` — zero-batch typed extraction

`deserialize_messages_from_reader::<T, R>` never materializes a `Vec<Segment<'_>>`
for the whole interchange. It uses `edifact_deserialize_owned` internally to
deserialize each `UNH..UNT` window as a typed value, then immediately drops the
window segments:

```rust
use edifact_rs::{deserialize_messages_from_reader, EdifactDeserialize};

# #[derive(Debug, EdifactDeserialize)]
# struct OrderMessage {}
let f = std::fs::File::open("interchange.edi")?;
for order in deserialize_messages_from_reader::<OrderMessage, _>(f) {
    let order = order?;
    // the window's raw segments have already been freed
}
# Ok::<(), edifact_rs::EdifactError>(())
```

This achieves O(1) peak memory even when an interchange contains thousands of messages.

---

## `WriterEmitter` — zero-alloc output

`WriterEmitter<W>` writes EDIFACT directly to any `std::io::Write` without
accumulating a `Vec<u8>`. Each `emit_*` call flushes bytes immediately:

```rust
use edifact_rs::WriterEmitter;

let mut out = Vec::<u8>::new();
let mut emitter = WriterEmitter::new(&mut out);
emitter.emit_segment_start("BGM")?;
emitter.emit_element("220")?;
emitter.emit_segment_end()?;
# Ok::<(), edifact_rs::EdifactError>(())
```

Use `WriterEmitter` over `ser::to_bytes` or `Writer` when generating large interchanges
that you want to pipe directly to a file or socket.

---

## Parsing modes and memory comparison

| API | Input | Peak memory | Notes |
|---|---|---|---|
| `from_bytes` | `&[u8]` | O(1) — zero copy | Fastest; requires full buffer |
| `from_reader` | `impl Read` | O(n) segments | Collects all segments into `Vec` |
| `from_reader_iter` | `impl Read` | O(1) | One segment at a time |
| `message_windows_bytes` | `&[u8]` | O(window) | One UNH..UNT window at a time |
| `message_windows_from_reader` | `impl Read` | O(window) | Reader-based windows |
| `deserialize_messages_from_reader` | `impl Read` | O(1) typed | Zero raw-segment buffer |

---

## Benchmarks

The benchmark suite uses both **divan** (micro-benchmarks) and **Criterion** (statistical):

```bash
# Divan micro-benchmarks
cargo bench -p edifact-rs --bench bench_core

# Criterion statistical benchmarks (with HTML reports)
cargo bench -p edifact-rs --bench bench_criterion
```

Criterion outputs are saved to `target/criterion/`. Open
`target/criterion/report/index.html` in a browser for a full comparison dashboard.

### Benchmark groups (Criterion)

| Group | Measures |
|---|---|
| `tokenizer/small` | Tokenization throughput on a single message (~450 bytes) |
| `tokenizer/1mb` | Tokenization throughput on a 1 MB interchange |
| `parser/small` | Parse + collect on a single message |
| `parser/1mb` | Parse + collect on 1 MB |
| `reader/1mb` | `from_reader` on 1 MB (reader overhead) |
| `writer/utilmd_message` | Serialize a UTILMD-sized message |
| `validation/d11a_structure` | Structure validation on D.11A rules |

---

## Fuzz testing

The crate integrates [bolero](https://docs.rs/bolero) for fuzz/property testing:

```bash
# Run the bolero harness (requires cargo-bolero)
cargo bolero test -p edifact-rs bolero_harness

# With libFuzzer (requires nightly + cargo-fuzz)
cargo +nightly bolero test -p edifact-rs bolero_harness --engine libfuzzer
```

The harness feeds arbitrary bytes to `from_bytes` and asserts that the parser never
panics — only returns `Ok` or `Err(EdifactError)`.

---

## Profiling tips

### flamegraph

```bash
cargo flamegraph -p edifact-rs --bench bench_criterion -- \
  --bench --profile-time=10 parser/1mb
```

### perf stat (Linux)

```bash
cargo build --release -p edifact-rs --example bench_large_message_memory
perf stat ./target/release/examples/bench_large_message_memory
```

### Memory usage with heaptrack

```bash
cargo build --release -p edifact-rs --example bench_large_message_memory
heaptrack ./target/release/examples/bench_large_message_memory
heaptrack_gui heaptrack.*.gz
```

---

## Tuning `ReaderConfig`

The reader-based APIs accept a `ReaderConfig` that controls the DOS guard:

```rust
use edifact_rs::{from_bufread_stream_with_config, ReaderConfig};

let config = ReaderConfig {
    max_segment_bytes: 256 * 1024,  // default: 512 KB
};
let cursor = std::io::Cursor::new(b"...");
let _iter = from_bufread_stream_with_config(cursor, config);
```

Setting `max_segment_bytes` too low will cause `E020 SegmentTooLong` on large but
legitimate segments (e.g. free-text `FTX` segments). 512 KB is a safe default for
most deployments.

---

## Memory budget summary

| Scenario | Typical peak allocation |
|---|---|
| Parse 1 MB interchange, collect all | ~3–4× input size (segments + `SmallVec` inlining) |
| Parse 1 MB, `from_reader_iter` (streaming) | ~4 KB (one segment buffer) |
| Typed streaming, `deserialize_messages_from_reader` | ~4 KB + sizeof(T) |
| Write 1 MB interchange via `WriterEmitter` | ~8 KB (internal buffer) |
| Write 1 MB interchange via `ser::to_bytes` | ~1× output size |

---

## Next steps

- [Streaming](streaming.md) — memory-efficient streaming APIs
- [Async Integration](async-integration.md) — `spawn_blocking` patterns
- [Error Reference](error-reference.md) — `E020 SegmentTooLong` and `ReaderConfig`
