# edifact-rs — task runner.
#
# Recipes mirror .github/workflows/ci.yml so a green `just ci` means a green CI.
# When you change a CI job, change the matching recipe in the same commit.
#
#   just              list every recipe
#   just check        fast inner-loop gate (fmt + clippy + tests)
#   just ci           everything CI runs (needs nightly for the docs.rs job)
#   just ci-full      `just ci` plus the MSRV job and the benchmarks
#
# `just VAR=value recipe` overrides a variable, e.g. `just locked= test` to drop
# `--locked` while you are adding a dependency.

set shell := ["bash", "-euo", "pipefail", "-c"]

# The MSRV, kept in step with `rust-version` in Cargo.toml and the msrv-check job.
msrv := "1.85.0"

# CI passes --locked so a stale Cargo.lock fails the build rather than being
# silently rewritten.  Clear it locally while changing dependencies.
locked := "--locked"

# Enables the derive trybuild suite, which is off by default because it is slow.
ui_env := "EDIFACT_UI_TESTS=1"

[private]
default:
    @just --list --unsorted

# ── inner loop ────────────────────────────────────────────────────────────────

# Build the whole workspace.
build:
    cargo build --workspace --all-targets --all-features {{ locked }}

# Type-check without codegen — the fastest signal.
check:
    cargo check --workspace --all-targets --all-features {{ locked }}

# Run the workspace test suite, including the derive UI expectations.
test *ARGS:
    {{ ui_env }} cargo test --workspace --all-features {{ locked }} {{ ARGS }}

# Run one test by name, e.g. `just test-one value_by_code`.
test-one PATTERN:
    {{ ui_env }} cargo test --workspace --all-features {{ locked }} -- {{ PATTERN }} --nocapture

# Doctests only — the guides and README compile through these.
test-docs:
    cargo test -p edifact-rs --all-features --doc {{ locked }}

# Format every crate.
fmt:
    cargo fmt --all

# Mirrors the `lint` CI job.
fmt-check:
    cargo fmt --all --check

# Deny all warnings, as the `lint` CI job does.
clippy:
    cargo clippy --workspace --all-targets --all-features {{ locked }} -- -D warnings

# Format, lint, and test — what to run before every commit.
pre-commit: fmt-check clippy test

# ── site ──────────────────────────────────────────────────────────────────────

# Serve the Zola site with live reload at http://127.0.0.1:1111.
site-serve:
    cd site && zola serve

# Build the Zola site into site/public.
site-build:
    cd site && zola build

# Mirrors the `site` CI job: validates every internal link and anchor.
site-check:
    cd site && zola check --skip-external-links

# ── documentation ─────────────────────────────────────────────────────────────

# Build the public docs with warnings denied.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc -p edifact-rs --all-features --no-deps {{ locked }}

# Build the docs and open them in a browser.
doc-open:
    RUSTDOCFLAGS="-D warnings" cargo doc -p edifact-rs --all-features --no-deps --open {{ locked }}

# The `docsrs-check` CI job.  Nightly, with the `docsrs` cfg docs.rs itself sets.
#
# Run this, not just `doc`: nightly carries rustdoc lints stable does not —
# `redundant_explicit_links` among them — and those are what gate the published
# documentation.  A green `just doc` says nothing about docs.rs.
doc-docsrs:
    RUSTDOCFLAGS="--cfg docsrs -D warnings" cargo +nightly doc -p edifact-rs --all-features --no-deps {{ locked }}

# ── derive UI suite ───────────────────────────────────────────────────────────

# The `test-derive-ui` CI job: compile-fail/compile-pass expectations for the macros.
ui:
    {{ ui_env }} cargo test -p edifact-rs-derive {{ locked }}

# Review the diff before committing.  The expectations must hold ONLY this
# crate's own messages: one that captures a rustc warning or note will drift on
# the next release and bury real regressions.  Silence incidental lints at the
# source (see crates/edifact-rs-derive/tests/ui/support.rs) instead.

[doc("Re-bless the .stderr expectations after an intentional diagnostic change.")]
ui-bless:
    {{ ui_env }} TRYBUILD=overwrite cargo test -p edifact-rs-derive
    @echo
    @echo "Blessed. Now: git diff crates/edifact-rs-derive/tests/ui/"
    @echo "Then confirm both toolchains agree: just ui && just ui-msrv"

# Run the UI suite on the MSRV toolchain, where CI also runs it.
ui-msrv:
    {{ ui_env }} cargo +{{ msrv }} test -p edifact-rs-derive {{ locked }}

# ── feature matrix (the `feature-matrix` CI job) ──────────────────────────────

# No features: the crate must build without the derive and serde layers.
test-no-default:
    cargo test -p edifact-rs --no-default-features {{ locked }}

# All features enabled.
test-all-features:
    cargo test -p edifact-rs --all-features {{ locked }}

# Compile and run the cookbook examples.
test-examples:
    cargo test -p edifact-rs --all-features --examples {{ locked }}

# `cargo test` skips `harness = false` bench targets, so build them explicitly.
build-benches:
    cargo test -p edifact-rs --benches --no-run {{ locked }}

# Every feature-matrix entry, in CI's order.
feature-matrix: check test test-no-default test-all-features ui test-examples build-benches

# ── MSRV (the `msrv-check` CI job) ───────────────────────────────────────────

# Install the MSRV toolchain if it is missing.
msrv-install:
    rustup toolchain install {{ msrv }} --profile minimal

# Install the nightly toolchain the `docsrs-check` job needs.
nightly-install:
    rustup toolchain install nightly --profile minimal

# Run the full workspace suite on the MSRV toolchain.
msrv:
    {{ ui_env }} cargo +{{ msrv }} test --workspace --all-features {{ locked }} --no-fail-fast

# ── audits and release (the `deny` and `release-check` CI jobs) ───────────────

# --all-features decides whether the optional miette subtree is in the graph;
# without it the audit covers a graph CI never sees.

[doc("Advisory, licence, and dependency-ban audit.")]
deny:
    cargo deny --all-features check advisories bans licenses sources

# Install cargo-deny if it is missing.
deny-install:
    cargo install cargo-deny --locked

# On a clean tree this is byte-for-byte the `release-check` CI job.  With
# uncommitted work it adds --allow-dirty and packages the working tree instead,
# because otherwise `just ci` would be unusable mid-change — the one time you
# most want to know whether a new file made it into the package.

[doc("Verify the derive crate packages cleanly.")]
publish-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "$(git status --porcelain 2>/dev/null)" ]]; then
        echo "note: working tree is dirty — packaging it as-is (--allow-dirty)"
        cargo publish --dry-run -p edifact-rs-derive --allow-dirty
    else
        cargo publish --dry-run -p edifact-rs-derive {{ locked }}
    fi

# They are released as a pair, and the main crate depends on the derive by
# exact version.

[doc("Verify both crates carry the same version.")]
versions-match:
    #!/usr/bin/env bash
    set -euo pipefail
    read_version() {
        cargo metadata --no-deps --format-version 1 \
            | python3 -c "import sys,json;print(next(p['version'] for p in json.load(sys.stdin)['packages'] if p['name']=='$1'))"
    }
    derive_ver=$(read_version edifact-rs-derive)
    main_ver=$(read_version edifact-rs)
    echo "edifact-rs-derive=${derive_ver}  edifact-rs=${main_ver}"
    if [[ "${derive_ver}" != "${main_ver}" ]]; then
        echo "Version mismatch: derive=${derive_ver} main=${main_ver}" >&2
        exit 1
    fi

# Everything the `release-check` CI job verifies.
release-check: publish-check versions-match

# ── property tests and benchmarks ────────────────────────────────────────────

# The `fuzz-smoke` CI job: bolero targets in release, with a long random sweep.
fuzz ITERATIONS="50000":
    BOLERO_RANDOM_ITERATIONS="{{ ITERATIONS }}" cargo test -p edifact-rs --release --test bolero_harness {{ locked }}

# Divan microbenchmarks.
bench:
    cargo bench -p edifact-rs --bench bench_core

# The criterion smoke run CI uses on pull requests.
bench-smoke:
    cargo bench -p edifact-rs --bench bench_criterion -- --noplot --sample-size 10

# Record the current criterion numbers under a named baseline.
bench-baseline NAME="main":
    cargo bench -p edifact-rs --bench bench_criterion -- --noplot --sample-size 50 --warm-up-time 2 --measurement-time 8 --save-baseline {{ NAME }}

# Compare two recorded baselines; needs `cargo install critcmp --locked`.
bench-compare FROM="main" TO="pr":
    critcmp {{ FROM }} {{ TO }} --threshold 5

# ── aggregates ───────────────────────────────────────────────────────────────

# Benchmarks and the MSRV job are not included — see `ci-full`.

[doc("Everything CI runs, on this toolchain.")]
ci: fmt-check clippy feature-matrix doc doc-docsrs deny release-check fuzz

# `ci` plus the MSRV job and the benchmark smoke run: the full pre-release gate.
ci-full: ci msrv bench-smoke

# ── housekeeping ─────────────────────────────────────────────────────────────

# Refresh Cargo.lock.
update:
    cargo update

# Remove build artifacts.
clean:
    cargo clean

# Report the toolchains these recipes use.
toolchains:
    @echo "default: $(rustc --version)"
    @echo "msrv:    $(rustc +{{ msrv }} --version 2>/dev/null || echo 'not installed — run: just msrv-install')"
    @echo "nightly: $(rustc +nightly --version 2>/dev/null || echo 'not installed — run: just nightly-install')"
