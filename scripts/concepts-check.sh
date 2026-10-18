#!/usr/bin/env bash
# Published files must not cite the gitignored planning paths; with a local
# planning tree, every roadmap item must own exactly one spec.
set -euo pipefail

cd "$(dirname "$0")/.."
failed=0
# Everything published: repository docs, crate sources and the site.
paths=(README.md CHANGELOG.md crates site/content)
if leaks=$(grep -rnE '(^|[^A-Za-z0-9_./-]|\.\./)(concepts/|specs/|\.specify/|\.claude/)' "${paths[@]}"); then
    printf 'Published files cite unavailable internal paths:\n%s\n' "$leaks" >&2
    failed=1
else
    result=$?
    if [[ "$result" -ne 1 ]]; then
        printf 'Could not scan published files (exit %s).\n' "$result" >&2
        exit "$result"
    fi
fi

roadmap=concepts/ROADMAP.md
if [[ ! -f "$roadmap" && ! -d specs ]]; then
    [[ "$failed" -eq 0 ]] || exit 1
    printf '%s\n' 'concepts-check: local planning absent; public boundary passed.'
    exit 0
fi
if [[ ! -f "$roadmap" || ! -d specs ]]; then
    printf '%s\n' 'Incomplete local planning: both roadmap and specs are required.' >&2
    exit 1
fi

if ! named=$(awk '
    function finish() {
        if (item && (names != 1 || discharges != 1)) {
            print "Roadmap item needs one specification and discharge: " item > "/dev/stderr"
            bad = 1
        }
    }
    {
        text = $0; sub(/^ */, "", text)
        if (text ~ /^(```+|~~~+)/) {
            marker = substr(text, 1, 1); token = text
            sub(marker == "`" ? /[^`].*$/ : /[^~].*$/, "", token)
            if (!fenced) {
                fenced = 1; fence_marker = marker; fence_length = length(token)
            } else if (marker == fence_marker && length(token) >= fence_length && text ~ /^(`+|~+)[ \t]*$/) {
                fenced = 0
            }
            next
        }
    }
    fenced { next }
    /^## / { finish(); item = ""; next }
    /^### / {
        finish(); item = $0; names = 0; discharges = 0
        if ($0 !~ /^### `#[a-z0-9-]+` /) {
            print "Roadmap item needs a stable anchor: " $0 > "/dev/stderr"
            bad = 1
        }
        next
    }
    /^\*\*Specified as:\*\* `[0-9][0-9][0-9]-[a-z0-9-]+`$/ {
        if (!item) bad = 1
        names++
        split($0, parts, "`"); print parts[2]
        next
    }
    /^\*\*Discharge:\*\* .+/ { if (!item) bad = 1; discharges++ }
    END { finish(); if (bad) exit 1 }
' "$roadmap"); then
    exit 1
fi

duplicates=$(printf '%s\n' "$named" | sort | uniq -d)
if [[ -n "$duplicates" ]]; then
    printf 'Specifications owned by multiple items:\n%s\n' "$duplicates" >&2
    failed=1
fi
while IFS= read -r feature; do
    [[ -n "$feature" ]] || continue
    if [[ ! -s "specs/$feature/spec.md" ]]; then
        printf 'Missing specification: %s\n' "$feature" >&2
        failed=1
    fi
done <<< "$named"
for directory in specs/[0-9][0-9][0-9]-*/; do
    [[ -d "$directory" ]] || continue
    feature=$(basename "$directory")
    if ! grep -Fxq "$feature" <<< "$named"; then
        printf 'Specification has no roadmap owner: %s\n' "$feature" >&2
        failed=1
    fi
done
[[ "$failed" -eq 0 ]] || exit 1
printf '%s\n' 'concepts-check: roadmap ownership and public boundary passed.'