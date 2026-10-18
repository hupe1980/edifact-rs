+++
title = "Performance"
description = "Allocation budget, memory bounds per entry point, the benchmark suite, and ReaderConfig tuning — each figure with the test that asserts it."
weight = 120
+++

## Allocation per segment

`from_bytes` borrows every tag and value from the input; only the element vector
is allocated:

```rust
let input: &[u8] = b"BGM+220+PO-4711+9'";
// One allocation (the element vector); text is borrowed.
let seg = edifact_rs::from_bytes(input).next().unwrap().unwrap();
assert_eq!(seg.tag, "BGM");
```

For one segment with `E` data elements, `X` release-escaped values and `V`
non-empty component values:

| Path | Allocator calls |
|---|---|
| `from_bytes` | `[E ≥ 1] + X` — the element vector, allocated once at its exact length, plus one owned string per escaped value |
| `from_reader` | `[E ≥ 1] + 1 + V` — the element vector, the tag and every non-empty value are owned |

- `from_bytes` adds one call when its reusable element buffer grows: at the
  first segment with elements, then once per doubling of the largest element
  count.
- Components use `SmallVec<[_; 4]>` (with `union`): up to four components per
  element need no allocation of their own; a fifth spills to the heap.
- `Element::repeats` stays unallocated unless the element actually repeats.
- A value is `Cow::Owned` only when a release escape had to be resolved
  (`?+` → `+`); otherwise it is `Cow::Borrowed`.

---

## Memory by entry point

| API | Holds at a time | Bound |
|---|---|---|
| `from_bytes` | one segment | the input slice is yours; streaming peak ≤ 2 704 B on the 1 MB fixture |
| `from_bytes(..).collect()` | every segment | peak ≤ 25 217 536 B for the 964 250 B fixture (26.2 × input) |
| `from_reader`, `from_bufread` | one segment and an 8 KiB read buffer | a segment is at most `max_segment_bytes` (64 KiB by default); streaming peak ≤ 9 347 B on the 1 MB fixture |
| `deserialize_each_from_reader` | one segment and one `T` | as `from_reader` |
| `message_windows_from_reader`, `deserialize_messages_from_reader` | one `UNH`…`UNT` window | unbounded without limits; the `_with_config` variants with `max_segments_per_message` bound it, independent of stream length |
| `WriterEmitter<W>` | nothing — each event goes straight to `W` | 0 allocations per event; it does not buffer, so wrap a file or socket in `BufWriter` |
| `to_bytes` / `to_edifact_string` | the output | the returned buffer |

Iterate rather than `.collect()` when the interchange may not fit in memory.
For a stream from an untrusted source, set the budgets described in
[Parsing → DoS hardening](@/docs/parsing.md#dos-hardening-with-readerconfig):

```rust
use edifact_rs::{ReaderConfig, message_windows_from_reader_with_config};

let config = ReaderConfig::default()
    .max_segments_per_message(10_000) // bounds each window
    .max_input_bytes(64 << 20);       // bounds the whole stream
let input = std::io::Cursor::new(b"UNH+1+ORDERS:D:96A:UN'BGM+220+1'UNT+3+1'".to_vec());
for window in message_windows_from_reader_with_config(input, config) {
    let window = window?;
    assert_eq!(window.segments.len(), 3);
}
# Ok::<(), edifact_rs::EdifactError>(())
```

`WriterEmitter` writes EDIFACT to any `std::io::Write` one event at a time:

```rust
use edifact_rs::{EdifactEvent, EventEmitter, WriterEmitter};

let mut out = Vec::<u8>::new();
let mut emitter = WriterEmitter::new(&mut out);
emitter.emit(EdifactEvent::start("BGM"))?;
emitter.emit(EdifactEvent::element("220"))?;
emitter.emit(EdifactEvent::element("PO-4711"))?;
emitter.emit(EdifactEvent::EndSegment)?;
assert_eq!(out, b"BGM+220+PO-4711'");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Where these numbers come from

The 1 MB fixture is `one_mb()` in `benches/bench_data.rs` (964 250 B, 44 011
segments). Peak figures are 64-bit and are asserted as ceilings: a test fails if
the measured value rises above the figure, or falls more than 20 % below it.

| Claim | Test |
|---|---|
| Per-segment allocator calls on both paths | `allocation_budget`: `orders_fixture_slice_budget`, `orders_fixture_reader_budget`, `escaped_value_costs_one_allocation` |
| `repeats` unallocated, element vector at exact length | `allocation_budget`: `repeats_unallocated_on_both_paths` |
| Collect peak 25 217 536 B | `allocation_budget`: `one_mb_peak_heap_slice_collect` |
| `from_bytes` streaming peak 2 704 B | `allocation_budget`: `one_mb_peak_heap_slice_stream` |
| `from_reader` streaming peak 9 347 B (8 KiB `BufReader` included) | `allocation_budget`: `one_mb_peak_heap_reader_stream` |
| `WriterEmitter` allocates nothing per event | `allocation_budget`: `writer_emitter_allocates_nothing_per_event` |
| `parse_unh`, `parse_ung`, `iter_messages_by_type` allocate nothing | `allocation_budget`: `parse_unh_allocates_nothing`, `parse_ung_allocates_nothing`, `messages_by_type_iter_allocates_nothing` |
| Default `max_segment_bytes` is 64 KiB | `resource_limits`: `default_max_segment_bytes_is_64_kib` |
| A window is bounded by `max_segments_per_message`, its peak independent of stream length | `resource_limits`: `unterminated_message_message_windows`, `unterminated_message_deserialize_messages`, `unterminated_message_deserialize_each`, `per_message_cap_on_long_lived_stream`, `peak_heap_bounded_by_limits_unterminated`, `peak_heap_bounded_by_limits_long_lived` |
| `max_input_bytes` stops endless whitespace | `resource_limits`: `endless_whitespace_reader_stops`, `whitespace_slice`, `peak_heap_bounded_by_limits_whitespace` |

`docs_claims` checks that the figures on this page match the committed
constants in `allocation_budget`.

---

## Benchmarks

**divan** for quick micro-benchmarks, **Criterion** for statistics and baselines:

```bash
cargo bench -p edifact-rs --bench bench_core
cargo bench -p edifact-rs --bench bench_criterion
```

Criterion writes its reports to `target/criterion/report/index.html`.

| Benchmark | Measures |
|---|---|
| `tokenizer/small`, `tokenizer/1mb` | Tokenizer throughput |
| `parser/small`, `parser/1mb` | `from_bytes` parse and collect |
| `reader/1mb` | `from_reader` parse and collect |
| `reader/parse_reader_chunked` | Reader path across read-buffer boundaries |
| `writer/sample_message` | Serialising a message |
| `validation/context_dispatch_floor` | Per-call `ValidationContext` dispatch over a no-op validator |
| `validation/validate_profile_custom_pack` | One `ProfileRulePack` |
| `validation/validate_profile_composed_packs` | Composed packs (`extend_from`) |
| `validation/parse_large_message`, `validation/validate_large_message` | Parse and validate a 1 MB interchange |
| `grouping/direct_indices/*`, `grouping/resolve_ordered/*` | Group traversal and ordered resolution at 10 k / 20 k / 40 k child groups |

How CI runs and compares them is described in
[`benches/README.md`](https://github.com/hupe1980/edifact-rs/tree/main/crates/edifact-rs/benches/README.md).

---

## Property and fuzz testing

The [bolero](https://docs.rs/bolero) harness covers parse, write, validate,
grouping and CONTRL invariants. `cargo test` runs it in random-exploration
mode; with `cargo-bolero` it runs under a coverage-guided engine:

```bash
cargo bolero test -p edifact-rs bolero_harness
cargo +nightly bolero test -p edifact-rs bolero_harness --engine libfuzzer
```

---

## Profiling

```bash
# flamegraph
cargo flamegraph -p edifact-rs --bench bench_criterion -- \
  --bench --profile-time=10 parser/1mb

# heap profile of the large-message probe
cargo build --release -p edifact-rs --example bench_large_message_memory
heaptrack ./target/release/examples/bench_large_message_memory
```

---

## Tuning `ReaderConfig`

```rust
use edifact_rs::{ReaderConfig, from_bufread_with_config};

// Budgets you do not name keep their defaults.
let config = ReaderConfig::default().max_segment_bytes(256 * 1024);
let cursor = std::io::Cursor::new(b"...");
let _iter = from_bufread_with_config(cursor, config);
```

A `max_segment_bytes` below your partners' longest segment (a long `FTX`, say)
raises `E020 SegmentTooLong`. The other budgets are unset by default and raise
`E036 LimitExceeded`; see
[Parsing → DoS hardening](@/docs/parsing.md#dos-hardening-with-readerconfig).

---

## Next steps

- [Streaming](@/docs/streaming.md) — reader iterators and message windows
- [Async Integration](@/docs/async-integration.md) — `spawn_blocking` patterns
- [Error Reference](@/docs/error-reference.md) — `E020` and `E036`
