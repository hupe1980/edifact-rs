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

- Tokenizer throughput (small + ~1MB payload)
- Parser throughput (small + ~1MB payload)
- Reader-based parse throughput (~1MB payload)
- Writer throughput (representative D.11A message)
- D.11A structural validation throughput

## Policy

- Use `bench_core` when iterating on tight loops or parser internals.
- Use `bench_criterion` for performance sign-off and CI smoke checks.
- Main/release runs publish a `main` criterion baseline artifact for PR comparisons.
- Pull requests compare against the latest main baseline and fail when criterion reports statistically significant regressions.
- Main/release runs enforce a large-message memory ceiling using `/usr/bin/time -v` and `bench_large_message_memory`.
- Treat measurable regressions as blockers unless there is an explicit, documented exception.