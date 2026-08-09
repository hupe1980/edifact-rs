+++
title = "Parsing"
description = "Zero-copy slice parsing, streaming readers, byte spans, UNA handling, and the ReaderConfig budgets that guard against oversized input."
weight = 30
+++

This guide covers every entry point for reading EDIFACT data — byte slices, readers,
custom delimiter configuration, and envelope-level helpers.

---

## Entry points overview

| Function | Input | Output type | When to use |
|---|---|---|---|
| `from_bytes(input)` | `&[u8]` | `impl Iterator<Item = Result<Segment<'_>, _>>` | In-memory buffer (fastest path) |
| `from_reader_collect(reader)` | `impl Read` | `Result<Vec<OwnedSegment>, _>` | Eagerly collect all segments from a reader |
| `from_reader(reader)` | `impl Read` | `FromReaderIter<R>` | Lazy iterator — one segment at a time |
| `from_bufread_stream_with_config(reader, config)` | `impl BufRead` | `Result<Vec<OwnedSegment>, _>` | DOS guard, custom limits |

---

## Zero-copy byte-slice parsing

```rust
use edifact_rs::from_bytes;

let input: &[u8] = b"UNA:+.? 'BGM+220+PO-4711+9'NAD+BY+4000001::9'";

// collect eagerly
let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

// or iterate lazily — no allocations until each segment is consumed
for result in from_bytes(input) {
    let seg = result?;
    println!("{}", seg.tag);
}
# Ok::<(), edifact_rs::EdifactError>(())
```

`from_bytes` is the fastest path:
- The tag `&str` and component `&str` slices **borrow directly from `input`**.
- A `Vec<Element>` per segment and a `SmallVec<[Cow<'_, str>; 4]>` per element are
  the only heap allocations.
- `Cow::Owned` is only created for components that contain a resolved release
  character (the decoded string differs from the raw bytes).

---

## Accessing segment data

### `element_str(n)` — the most common pattern

```rust
# use edifact_rs::from_bytes;
# let input: &[u8] = b"UNA:+.? 'BGM+220+PO-4711+9'NAD+BY+4000001::9'";
# let segments: Vec<_> = from_bytes(input).collect::<Result<Vec<_>, _>>()?;
let bgm = &segments[0];

// component 0 of element n — covers the vast majority of EDIFACT fields
assert_eq!(bgm.element_str(0), Some("220"));
assert_eq!(bgm.element_str(99), None); // out-of-bounds → None
# Ok::<(), edifact_rs::EdifactError>(())
```

### `get_element(n)` + `get_component(c)` — composite fields

```rust
# use edifact_rs::from_bytes;
# let input: &[u8] = b"UNA:+.? 'BGM+220+PO-4711+9'NAD+BY+4000001::9'";
# let segments: Vec<_> = from_bytes(input).collect::<Result<Vec<_>, _>>()?;
// NAD element 1 is composite: party_id : qualifier : code_list_qual
let nad = &segments[1];
let party_id = nad
    .get_element(1)
    .and_then(|e| e.get_component(0))
    .unwrap_or("unknown");
let code = nad
    .get_element(1)
    .and_then(|e| e.get_component(2))
    .unwrap_or("");
assert_eq!(party_id, "4000001");
assert_eq!(code, "9");
# Ok::<(), edifact_rs::EdifactError>(())
```

### `component_or_empty(n)` — avoid `Option` unwrapping

```rust
# use edifact_rs::from_bytes;
# let input: &[u8] = b"UNA:+.? 'BGM+220+PO-4711+9'NAD+BY+4000001::9'";
# let segments: Vec<_> = from_bytes(input).collect::<Result<Vec<_>, _>>()?;
# let seg = &segments[1];
let elem = seg.get_element(1).unwrap();
let val = elem.component_or_empty(0); // "" if absent, no panic
assert_eq!(val, "4000001");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Byte spans

Every `Segment`, `Element`, and component carries a `Span { start, end }` pointing
into the **original input slice**.

```rust
# use edifact_rs::from_bytes;
# let input: &[u8] = b"UNA:+.? 'BGM+220+PO-4711+9'NAD+BY+4000001::9'";
# let segments: Vec<_> = from_bytes(input).collect::<Result<Vec<_>, _>>()?;
let seg = &segments[0];
println!("segment spans bytes {}..{}", seg.span.start, seg.span.end);

let elem = seg.get_element(0).unwrap();
println!("element spans bytes {}..{}", elem.span.start, elem.span.end);

if let Some(span) = elem.component_span(0) {
    let raw = &input[span.start..span.end];
    assert_eq!(raw, b"220");
}
# Ok::<(), edifact_rs::EdifactError>(())
```

Spans are stable across all parsing modes and are used by the `diagnostics` feature
to show source-annotated error messages.

---

## UNA handling

`from_bytes` and `from_reader` both check for a `UNA` prefix automatically:

- **UNA present**: the six custom service characters are extracted and applied.
- **UNA absent**: EDIFACT defaults (`+`, `:`, `.`, ` `, `?`, `'`) are used.
- **Malformed UNA**: parsing fails immediately with `EdifactError::InvalidUna`.

```rust
# use edifact_rs::from_bytes;
// Custom delimiters via UNA: `;` component, `|` element, `!` terminator.
let custom = b"UNA;|.? !BGM|220|PO-4711|9!";
//                ││   │└── segment terminator
//                ││   └─── repetition separator (space = not used)
//                │└─────── element separator
//                └──────── component separator
let segs: Vec<_> = from_bytes(custom).collect::<Result<_, _>>()?;
assert_eq!(segs[0].tag, "BGM");
assert_eq!(segs[0].element_str(0), Some("220"));
assert_eq!(segs[0].element_str(1), Some("PO-4711"));
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Reader-based parsing

### `from_reader_collect` — read all at once

```rust,no_run
use edifact_rs::from_reader_collect;
use std::fs::File;

let f = File::open("message.edi")?;
let segments = from_reader_collect(f)?; // Vec<OwnedSegment>
# Ok::<(), edifact_rs::EdifactError>(())
```

### `from_reader` — streaming, one segment at a time

```rust,no_run
use edifact_rs::from_reader;
use std::fs::File;

let f = File::open("large_interchange.edi")?;
for result in from_reader(f) {
    let seg = result?;               // OwnedSegment
    println!("{}", seg.tag);
}
# Ok::<(), edifact_rs::EdifactError>(())
```

`from_reader` is O(1) memory — it yields one `OwnedSegment` and then
immediately drops the internal buffer before reading the next segment.

---

## DoS hardening with `ReaderConfig`

`ReaderConfig` carries four independent budgets. Every one of them is a **hard cap
that reports an error** — none of them ever ends the iterator quietly.

| Field | Default | Raised as |
|---|---|---|
| `max_segment_bytes` | 65 536 (64 KiB) | `SegmentTooLong { offset, limit }` (E020) |
| `max_segments` | unlimited | `LimitExceeded { limit: "max_segments", max }` (E036) |
| `max_messages` | unlimited | `LimitExceeded { limit: "max_messages", max }` (E036) |
| `max_input_bytes` | unlimited | `LimitExceeded { limit: "max_input_bytes", max }` (E036) |

The same config applies to both front ends: `from_bytes_with_config` for the
borrowed slice path and `from_reader_with_config` /
`from_bufread_stream_with_config` for the owned reader path.

```rust
use edifact_rs::{EdifactError, ReaderConfig, from_bytes_with_config};

let config = ReaderConfig::default()
    .max_segment_bytes(8_192)   // reject any single segment over 8 KiB
    .max_segments(10_000)       // …and any input with more than 10 000 segments
    .max_input_bytes(1 << 20);  // …or more than 1 MiB in total

let segments: Vec<_> = from_bytes_with_config(b"BGM+220+test'", config)
    .collect::<Result<_, _>>()?;
assert_eq!(segments.len(), 1);
# Ok::<(), edifact_rs::EdifactError>(())
```

### Why the limits are errors and not a quiet stop

A budget that merely returned `None` is indistinguishable from a clean end of
input. The idiomatic call —

```rust,ignore
let segments: Vec<_> = from_bytes_with_config(input, config).collect::<Result<_, _>>()?;
```

— would then succeed on a **truncated** interchange, and everything downstream
would treat a fragment as the whole message. Reporting the violation is the only
outcome a caller cannot accidentally ignore:

```rust
use edifact_rs::{EdifactError, ReaderConfig, from_bytes_with_config};

let config = ReaderConfig::default().max_segments(1);
let err = from_bytes_with_config(b"BGM+220'DTM+137'", config)
    .collect::<Result<Vec<_>, _>>()
    .unwrap_err();
assert!(matches!(
    err,
    EdifactError::LimitExceeded { limit: "max_segments", max: 1 },
));
```

Input that ends *exactly* at a limit is not a violation and finishes normally.
Segments parsed before the violation are still delivered, so a caller iterating
manually can keep the partial result and decide what to do with it.

`max_input_bytes` is a true cap: a segment whose end offset would pass the budget
is never handed out at all.

---

## Envelope helpers

Use `find_segment` and `find_qualified_segment` to locate specific segments without
iterating manually:

```rust
use edifact_rs::{find_segment, find_qualified_segment, from_bytes};

let input = b"UNH+1+ORDERS:D:11A:UN'\
              BGM+220+PO-4711+9'\
              NAD+BY+4000001::9'\
              NAD+SU+4000002::9'\
              UNT+5+1'";
let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

// First segment with tag "BGM"
let bgm = find_segment(&segs, "BGM").expect("BGM not found");
assert_eq!(bgm.element_str(1), Some("PO-4711"));

// First NAD where element 0 == "BY"
let buyer_nad = find_qualified_segment(&segs, "NAD", "BY").expect("NAD BY not found");
assert_eq!(buyer_nad.element_str(1), Some("4000001"));

// Same for OwnedSegment slices (reader path)
use edifact_rs::{find_segment_owned, find_qualified_segment_owned};
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Error handling

All parse errors carry stable codes and byte offsets:

```rust
use edifact_rs::{from_bytes, EdifactError};

let bad_input = b"BGM+incomplete"; // no segment terminator
match from_bytes(bad_input).collect::<Result<Vec<_>, _>>() {
    Ok(_) => println!("parsed"),
    Err(EdifactError::UnexpectedEof { offset }) => {
        eprintln!("input ended at byte {offset}");
    }
    Err(e) => eprintln!("error: {e}"),
}
```

See the [Error Reference](@/docs/error-reference.md) for a complete list of all variants
and their stable codes (E001–E037).

---

## Strict vs. lenient envelope validation

`from_bytes`, `from_reader`, and `from_reader_collect` are **lenient by default** — they parse all
segments and surface body segments even when `UNB`/`UNZ` or `UNH`/`UNT` reference
parity is wrong. Use `validate_envelope` to enforce parity explicitly:

```rust
use edifact_rs::{from_bytes, validate_envelope};

let input = b"UNB+UNOA:1+SENDER+RECEIVER+200101:0900+IC1'\
              UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'\
              UNZ+1+IC1'";
let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;
validate_envelope(&segs)?; // returns Err on the first parity violation
# Ok::<(), edifact_rs::EdifactError>(())
```

To collect **every** violation instead of stopping at the first, use
`validate_envelope_lenient`, which returns a `LenientResult` carrying both the
parsed interchange (when it could be recovered) and the full error list:

```rust
use edifact_rs::{from_bytes, validate_envelope_lenient};

// UNZ references a different control reference *and* UNT declares a wrong count.
let input = b"UNB+UNOA:1+SENDER+RECEIVER+200101:0900+IC1'\
              UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+99+1'\
              UNZ+1+IC2'";
let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;
let result = validate_envelope_lenient(&segs);
assert!(result.errors.len() >= 2); // both problems reported
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Next steps

- [Writing](@/docs/writing.md) — serialize segments back to EDIFACT bytes
- [Typed Derive](@/docs/typed-derive.md) — map segments to strongly-typed Rust structs
- [Streaming](@/docs/streaming.md) — process multi-message interchanges lazily
