#!/usr/bin/env bash
# scripts/check.sh — run all checks that execute in CI locally.
#
# Usage:
#   ./scripts/check.sh              # run everything
#   ./scripts/check.sh --no-bench   # skip benchmarks (faster inner-loop check)
#
# Exit code is non-zero if any step fails. All steps are reported even when one
# fails so you see the full picture in one pass.
#
# Steps mirrored from .github/workflows/ci.yml:
#   1.  cargo check --workspace --all-targets              (msrv-check / feature-matrix)
#   2.  cargo test --workspace --all-targets               (workspace tests)
#   3.  cargo test -p edifact-rs --no-default-features     (feature-matrix)
#   4.  cargo test -p edifact-rs --all-features            (feature-matrix)
#   5.  cargo test -p edifact-rs --all-features --examples (feature-matrix)
#   6.  cargo clippy --all-targets --all-features          (lint gate)
#   7.  cargo doc (RUSTDOCFLAGS=-D warnings)               (docsrs-check proxy)
#   8.  cargo publish --dry-run -p edifact-rs-derive       (release-check)
#   9.  Crate versions match across workspace              (release-check)
#   10. cargo deny check                                   (security/license audit)
#   11. cargo test --benches --no-run                      (bench compile smoke)
#   12. cargo bench bench_core                             (divan, skipped with --no-bench)
#   13. cargo bench bench_criterion smoke                  (criterion, skipped with --no-bench)

set -euo pipefail

# ── helpers ────────────────────────────────────────────────────────────────────

BOLD='\033[1m'
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
RESET='\033[0m'

FAILED_STEPS=()
RUN_BENCH=true

for arg in "$@"; do
  case "$arg" in
    --no-bench) RUN_BENCH=false ;;
    *) echo "Unknown argument: $arg"; exit 1 ;;
  esac
done

step() {
  local label="$1"
  shift
  echo -e "\n${BOLD}▶ ${label}${RESET}"
  echo "  $ $*"
  if "$@"; then
    echo -e "${GREEN}  ✓ ${label}${RESET}"
  else
    echo -e "${RED}  ✗ ${label}${RESET}"
    FAILED_STEPS+=("$label")
  fi
}

# ── change to workspace root ───────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

# ── 1. Workspace check ─────────────────────────────────────────────────────────
step "cargo check --workspace --all-targets" \
  cargo check --workspace --all-targets

# ── 2. Workspace tests ─────────────────────────────────────────────────────────
step "cargo test --workspace --all-targets" \
  cargo test --workspace --all-targets

# ── 3. No-default-features test ────────────────────────────────────────────────
step "cargo test -p edifact-rs --no-default-features" \
  cargo test -p edifact-rs --no-default-features

# ── 4. All-features test ───────────────────────────────────────────────────────
step "cargo test -p edifact-rs --all-features" \
  cargo test -p edifact-rs --all-features

# ── 5. All-features examples test ─────────────────────────────────────────────
step "cargo test -p edifact-rs --all-features --examples" \
  cargo test -p edifact-rs --all-features --examples

# ── 6. Clippy ──────────────────────────────────────────────────────────────────
step "cargo clippy --all-targets --all-features -- -D warnings" \
  cargo clippy --all-targets --all-features -- -D warnings

# ── 7. Docs (stable proxy for docsrs-check) ────────────────────────────────────
# CI uses nightly + --cfg docsrs; locally we use stable without that cfg.
# Set RUSTDOCFLAGS=-D warnings to match the spirit of the CI gate.
step "cargo doc -p edifact-rs --all-features --no-deps" \
  env RUSTDOCFLAGS="-D warnings" cargo doc -p edifact-rs --all-features --no-deps

# ── 8. Publish dry-run (derive only — main crate requires derive to be on crates.io first) ───
step "cargo publish --dry-run -p edifact-rs-derive" \
  cargo publish --dry-run -p edifact-rs-derive

# ── 9. Crate versions match ────────────────────────────────────────────────────
step "Crate versions match across workspace" bash -c '
  set -euo pipefail
  derive_ver=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c "import sys,json; pkgs=json.load(sys.stdin)[\"packages\"]; \
      print(next(p[\"version\"] for p in pkgs if p[\"name\"]==\"edifact-rs-derive\"))")
  main_ver=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c "import sys,json; pkgs=json.load(sys.stdin)[\"packages\"]; \
      print(next(p[\"version\"] for p in pkgs if p[\"name\"]==\"edifact-rs\"))")
  echo "edifact-rs-derive=${derive_ver}  edifact-rs=${main_ver}"
  if [[ "${derive_ver}" != "${main_ver}" ]]; then
    echo "Version mismatch: derive=${derive_ver} main=${main_ver}"
    exit 1
  fi
'

# ── 10. cargo deny ─────────────────────────────────────────────────────────────
# Skip silently if cargo-deny is not installed (it's an optional tool).
if command -v cargo-deny &>/dev/null || cargo deny --version &>/dev/null 2>&1; then
  step "cargo deny check" \
    cargo deny check
else
  echo -e "\n${YELLOW}⏭  cargo deny skipped (cargo-deny not installed)${RESET}"
fi

# ── 11-12. Benchmarks ────────────────────────────────────────────────────────
# Always verify benches compile; only execute them unless --no-bench is passed.
step "cargo test --benches --no-run (smoke-compile)" \
  cargo test -p edifact-rs --benches --no-run

if [[ "$RUN_BENCH" == true ]]; then
  step "cargo bench bench_core (divan)" \
    cargo bench -p edifact-rs --bench bench_core

  step "cargo bench bench_criterion smoke (sample-size 10)" \
    cargo bench -p edifact-rs --bench bench_criterion -- --noplot --sample-size 10
else
  echo -e "\n${YELLOW}⏭  Benchmarks skipped (--no-bench)${RESET}"
fi

# ── Summary ───────────────────────────────────────────────────────────────────

echo ""
if [[ ${#FAILED_STEPS[@]} -eq 0 ]]; then
  echo -e "${GREEN}${BOLD}✓ All checks passed.${RESET}"
  exit 0
else
  echo -e "${RED}${BOLD}✗ ${#FAILED_STEPS[@]} check(s) failed:${RESET}"
  for s in "${FAILED_STEPS[@]}"; do
    echo -e "${RED}  • ${s}${RESET}"
  done
  exit 1
fi
