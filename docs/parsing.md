# Parsing 🔍

This guide covers every entry point for reading EDIFACT data — byte slices, readers,
custom delimiter configuration, and envelope-level helpers.

---

## Entry points overview

| Function | Input | Output type | When to use |
|---|---|---|---|
| `from_bytes(input)` | `&[u8]` | `impl Iterator<Item = Result<Segment<'_>, _>>` | In-memory buffer (fastest path) |
| `from_reader(reader)` | `impl Read` | `Result<Vec<OwnedSegment>, _>` | Small files or `Cursor<Vec<u8>>` |
| `from_reader_iter(reader)` | `impl Read` | `impl Iterator<Item = Result<OwnedSegment, _>>` | Streaming, large files |
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
let bgm = &segments[0];

// component 0 of element n — covers the vast majority of EDIFACT fields
assert_eq!(bgm.element_str(0), Some("220"));
assert_eq!(bgm.element_str(99), None); // out-of-bounds → None
```

### `get_element(n)` + `get_component(c)` — composite fields

```rust
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
```

### `component_or_empty(n)` — avoid `Option` unwrapping

```rust
let elem = seg.get_element(1).unwrap();
let val = elem.component_or_empty(0); // "" if absent, no panic
```

---

## Byte spans

Every `Segment`, `Element`, and component carries a `Span { start, end }` pointing
into the **original input slice**.

```rust
let seg = &segments[0];
println!("segment spans bytes {}..{}", seg.span.start, seg.span.end);

let elem = seg.get_element(0).unwrap();
println!("element spans bytes {}..{}", elem.span.start, elem.span.end);

if let Some(span) = elem.component_span(0) {
    let raw = &input[span.start..span.end];
    println!("raw bytes: {:?}", raw);
}
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
// Custom delimiters via UNA
let custom = b"UNA;|.? !'BGM;220;PO-4711;9!";
//               ^^                         ^
//               comp sep = ;               term = !
let segs: Vec<_> = from_bytes(custom).collect::<Result<_, _>>()?;
assert_eq!(segs[0].tag, "BGM");
assert_eq!(segs[0].element_str(0), Some("220"));
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Reader-based parsing

### `from_reader` — read all at once

```rust
use edifact_rs::from_reader;
use std::fs::File;

let f = File::open("message.edi")?;
let segments = from_reader(f)?; // Vec<OwnedSegment>
# Ok::<(), edifact_rs::EdifactError>(())
```

### `from_reader_iter` — streaming, one segment at a time

```rust
use edifact_rs::from_reader_iter;
use std::fs::File;

let f = File::open("large_interchange.edi")?;
for result in from_reader_iter(f) {
    let seg = result?;               // OwnedSegment
    println!("{}", seg.tag);
}
# Ok::<(), edifact_rs::EdifactError>(())
```

`from_reader_iter` is O(1) memory — it yields one `OwnedSegment` and then
immediately drops the internal buffer before reading the next segment.

---

## DOS hardening with `ReaderConfig`

The `max_segment_bytes` guard prevents a malicious payload from exhausting memory
by sending an extremely long segment. It is enforced on **both** the fast path
(when the segment fits in the OS read buffer) and the slow path.

```rust
use edifact_rs::{ReaderConfig, from_bufread_stream_with_config};
use std::io::BufReader;

let config = ReaderConfig {
    max_segment_bytes: 8_192, // reject any segment > 8 KiB
    ..Default::default()
};

let reader = BufReader::new(std::io::Cursor::new(b"BGM+220+test'"));
let segments = from_bufread_stream_with_config(reader, config)?;
# Ok::<(), edifact_rs::EdifactError>(())
```

Exceeding the limit returns `EdifactError::SegmentTooLong { offset, limit }`.

The default limit is **65 536 bytes** per segment (64 KiB), which comfortably
covers all real-world EDIFACT messages.

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

See the [Error Reference](error-reference.md) for a complete list of all variants
and their stable codes (E001–E020).

---

## Strict vs. lenient envelope validation

`from_bytes` and `from_reader` are **lenient by default** — they parse all
segments and surface body segments even when `UNB`/`UNZ` or `UNH`/`UNT` reference
parity is wrong. Use `validate_envelope` to enforce parity explicitly:

```rust
use edifact_rs::{from_bytes, envelope};

let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;
envelope::validate(&segs)?; // returns Err if parity is violated
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Next steps

- [Writing](writing.md) — serialize segments back to EDIFACT bytes
- [Typed Derive](typed-derive.md) — map segments to strongly-typed Rust structs
- [Streaming](streaming.md) — process multi-message interchanges lazily
