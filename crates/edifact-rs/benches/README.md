# Benchmarking

This crate provides two benchmark suites:

- `bench_core` (divan): quick local iteration and micro-benchmark feedback.
- `bench_criterion` (criterion): release-grade statistical benchmark suite for CI/regression discipline.

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
- Main/release runs publish a `main` criterion baseline artifact for PR comparisons.
- Pull requests compare against the latest main baseline and fail when criterion reports statistically significant regressions.
- Main/release runs enforce a large-message memory ceiling using `/usr/bin/time -v` and `bench_large_message_memory`.
- Treat measurable regressions as blockers unless there is an explicit, documented exception.
- Check a new benchmark's throughput figure is physically plausible before
  trusting its trend — a benchmark over a no-op validator measures dispatch, not
  validation.
- Prune the `baselines/` directory of any benchmark you rename or re-fixture, in
  the same commit: `critcmp` ignores a baseline with no matching benchmark, so a
  stale one is a comparison that silently never happens.