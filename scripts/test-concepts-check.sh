#!/usr/bin/env bash
# Regression cases for scripts/concepts-check.sh, run against a throwaway tree.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts" "$fixture/crates/edifact-rs/src" "$fixture/site/content"
cp "$root/scripts/concepts-check.sh" "$fixture/scripts/"
touch "$fixture/README.md" "$fixture/CHANGELOG.md"

expect_pass() {
    bash "$fixture/scripts/concepts-check.sh" > /dev/null
}
expect_fail() {
    if bash "$fixture/scripts/concepts-check.sh" > /dev/null 2>&1; then
        printf 'Expected guard rejection: %s\n' "$1" >&2
        exit 1
    fi
}
valid_roadmap() {
    printf '%s\n' '# Roadmap' '## Active' '### `#example` Example' \
        '**Specified as:** `001-example`' '**Discharge:** Evidence is recorded.' > "$fixture/concepts/ROADMAP.md"
}

expect_pass
printf '%s\n' '[Unavailable](concepts/ROADMAP.md)' > "$fixture/README.md"
expect_fail 'public leak without local planning'
printf '' > "$fixture/README.md"
printf '%s\n' '// see specs/001-example/spec.md' > "$fixture/crates/edifact-rs/src/lib.rs"
expect_fail 'leak from crate source'
printf '%s\n' '[Core concepts](/docs/core-concepts/)' > "$fixture/crates/edifact-rs/src/lib.rs"
expect_pass
printf '%s\n' '[x](../.specify/memory/constitution.md)' > "$fixture/site/content/_index.md"
expect_fail 'leak from site content'
printf '' > "$fixture/site/content/_index.md"
mkdir -p "$fixture/concepts"
expect_pass
valid_roadmap
expect_fail 'partial planning tree'
mkdir -p "$fixture/specs/001-example"
printf '%s\n' '# Specification' > "$fixture/specs/001-example/spec.md"
expect_pass
printf '%s\n' '### `#duplicate` Duplicate' '**Specified as:** `001-example`' \
    '**Discharge:** Same owner is invalid.' >> "$fixture/concepts/ROADMAP.md"
expect_fail 'duplicate specification owner'
valid_roadmap
rm "$fixture/specs/001-example/spec.md"
expect_fail 'missing specification'
printf '%s\n' '# Specification' > "$fixture/specs/001-example/spec.md"
mkdir -p "$fixture/specs/002-orphan"
expect_fail 'orphan specification'
rmdir "$fixture/specs/002-orphan"
printf '%s\n' '# Roadmap' '## Active' '### `#example` Example' \
    '**Specified as:** `001-example`' > "$fixture/concepts/ROADMAP.md"
expect_fail 'missing discharge'
printf '%s\n' '# Roadmap' '## Active' '### Example without anchor' \
    '**Specified as:** `001-example`' '**Discharge:** Evidence is recorded.' > "$fixture/concepts/ROADMAP.md"
expect_fail 'missing stable anchor'
valid_roadmap
printf '%s\n' '````markdown' '```' '### `#phantom` Example only' \
    '**Specified as:** `999-phantom`' '````' >> "$fixture/concepts/ROADMAP.md"
expect_pass
printf '%s\n' '~~~markdown' '### `#phantom` Unclosed example' >> "$fixture/concepts/ROADMAP.md"
expect_pass
printf '%s\n' 'concepts-check regression cases passed.'
