# Benchmarking

This crate provides two benchmark suites:

- `bench_core` (divan): quick local iteration and micro-benchmark feedback.
- `bench_criterion` (criterion): statistical suite used by CI.

## Run locally

Quick micro-benchmarks:

```bash
cargo bench -p edifact-rs --bench bench_core
```

Criterion suite:

```bash
cargo bench -p edifact-rs --bench bench_criterion -- --noplot
```

Fast smoke configuration (matches CI intent):

```bash
cargo bench -p edifact-rs --bench bench_criterion -- --noplot --sample-size 10
```

Save baseline for comparison (main/release workflow):

```bash
cargo bench -p edifact-rs --bench bench_criterion -- --noplot --sample-size 50 --warm-up-time 2 --measurement-time 8 --save-baseline main
```

Compare against baseline (PR regression check):

```bash
cargo bench -p edifact-rs --bench bench_criterion -- --noplot --sample-size 20 --baseline main
```

Large-message memory probe:

```bash
cargo run -p edifact-rs --release --example bench_large_message_memory
```

## What is measured

- Tokenizer throughput (small + ~1 MB payload)
- Parser throughput (small + ~1 MB payload)
- Reader-based parse throughput (~1 MB payload)
- Writer throughput (representative ORDERS message)
- Validation: the per-call dispatch floor, two profile-pack shapes, and a
  ~1 MB interchange end to end
- Grouping: `direct_segment_indices` and ordered `GroupSchema::resolve` over a
  flat message with 10 k / 20 k / 40 k child groups (both linear; baseline in
  `baselines/grouping`)

### Fixtures

Two ~1 MB fixtures; pick by whether the benchmark reads the envelope.

- `one_mb()` — repeated interchanges. For the byte-level suites (tokenizer,
  parser, reader, writer), which never look at the envelope.
- `one_mb_interchange()` — one conformant interchange: a single `UNB`, many
  `UNH`/`UNT` messages, a matching `UNZ`. Required by anything that validates —
  `validate_envelope` aborts at the second `UNB` in the other fixture.

## Policy

- Use `bench_core` when iterating on tight loops or parser internals.
- Use `bench_criterion` for performance sign-off and CI smoke checks.
- Pushes to `main` and release tags save a full `main` baseline as a CI artifact.
- Pull requests run the suite, then print `critcmp main pr --threshold 5`
  against the latest `main` artifact: every benchmark that moved by more than
  5 %. The comparison is **informational** — critcmp exits 0 when a benchmark
  regressed, so the job fails only if critcmp itself errors. It is skipped when
  no `main` artifact exists or no benchmark names match.
- No CI job measures memory. `bench_large_message_memory` is a local probe; the
  committed allocation and peak-heap figures are asserted by the
  `allocation_budget` and `resource_limits` tests (see the Performance guide).
- Treat a reported regression as a blocker unless there is an explicit,
  documented exception.
- Check a new benchmark's throughput figure is physically plausible before
  trusting its trend — a benchmark over a no-op validator measures dispatch, not
  validation.
- Prune the `baselines/` directory of any benchmark you rename or re-fixture, in
  the same commit: `critcmp` ignores a baseline with no matching benchmark, so a
  stale one is a comparison that silently never happens.

## Committed baselines

`baselines/` holds Criterion baselines named `m5`, kept as comparison
references. The machine and toolchain behind the `tokenizer`, `parser`, `reader`
and `validation` runs were not recorded, so the guides quote no timings from
them. `baselines/grouping` was recorded on 2026-10-18 on an Apple M4 Pro
(macOS, rustc 1.99.0) under heavy machine load: its ratios between sizes are
meaningful, its absolute times are not. Re-record on a quiet machine before
using it as a gate.
