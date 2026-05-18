# Streaming 🌊

`edifact-rs` provides multiple streaming APIs for processing large EDIFACT
interchanges without loading the entire file into memory. All reader-based APIs are
synchronous (`std::io::Read`) and can be bridged to async runtimes — see
[Async Integration](async-integration.md).

---

## API overview

| API | Source | Output | Memory model |
|---|---|---|---|
| `from_reader_iter(reader)` | `impl Read` | `Iterator<Item = Result<OwnedSegment, _>>` | O(1) — one segment at a time |
| `message_windows_bytes(input)` | `&[u8]` | `Iterator<Item = Result<Vec<OwnedSegment>, _>>` | O(window) — one message window |
| `message_windows_from_reader(reader)` | `impl Read` | `Iterator<Item = Result<Vec<OwnedSegment>, _>>` | O(window) — lazy I/O |
| `deserialize_first_streaming(input)` | `&[u8]` | `Result<T, _>` | Stops at first match |
| `deserialize_all_streaming(input)` | `&[u8]` | `Result<Vec<T>, _>` | Collects matching segments |
| `deserialize_first_from_reader(reader)` | `impl Read` | `Result<T, _>` | Stops at first match |
| `deserialize_all_from_reader(reader)` | `impl Read` | `Result<Vec<T>, _>` | Collects matching segments |
| `deserialize_messages_from_reader(reader)` | `impl Read` | `Iterator<Item = Result<T, _>>` | One typed message per window |

---

## Segment-level streaming

### `from_reader_iter` — raw segment stream

Process one `OwnedSegment` at a time without loading the interchange into memory:

```rust
use edifact_rs::from_reader_iter;
use std::fs::File;

fn main() -> Result<(), edifact_rs::EdifactError> {
    let f = File::open("interchange.edi")?;

    for result in from_reader_iter(f) {
        let seg = result?;
        println!("tag={} elements={}", seg.tag, seg.elements.len());
    }
    Ok(())
}
```

> **Memory**: a single `OwnedSegment` is allocated per iteration. Previous segments
> are dropped before the next one is parsed.

---

## Message windows

A **message window** is the slice of segments between a `UNH` and its matching `UNT`
(inclusive). Envelope segments (`UNB`, `UNZ`, `UNG`, `UNE`) are **skipped**
automatically.

### `message_windows_bytes` — byte-slice source

```rust
use edifact_rs::message_windows_bytes;

let interchange = b"\
    UNB+UNOA:1+S+R+200101:0900+1'\
    UNH+1+ORDERS:D:96A:UN'BGM+220+PO-001+9'UNT+3+1'\
    UNH+2+ORDERS:D:96A:UN'BGM+220+PO-002+9'UNT+3+2'\
    UNZ+2+1'";

for result in message_windows_bytes(interchange) {
    let window: Vec<edifact_rs::OwnedSegment> = result?;
    // window = [UNH, BGM, UNT]
    println!("{} segments in this message", window.len());
}
# Ok::<(), edifact_rs::EdifactError>(())
```

### `message_windows_from_reader` — reader source

```rust
use edifact_rs::message_windows_from_reader;
use std::fs::File;

fn main() -> Result<(), edifact_rs::EdifactError> {
    let f = File::open("multi_message.edi")?;
    for result in message_windows_from_reader(f) {
        let window = result?;
        let msg_ref = window[0].element_str(0).unwrap_or("?");
        println!("message reference: {msg_ref}");
    }
    Ok(())
}
```

> **Error propagation**: if a `UNH` is opened but no `UNT` is found before end of
> input, the unclosed window is silently discarded (the iterator simply ends). Any
> I/O error from the underlying reader surfaces as `EdifactError::Io`.

---

## Typed streaming — extract matching segments

Use `deserialize_first_streaming` / `deserialize_all_streaming` when you only care
about specific segment types in an interchange.

```rust
use edifact_rs::{EdifactDeserialize, deserialize_first_streaming, deserialize_all_streaming};

#[derive(Debug, EdifactDeserialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    doc_code: String,
    #[edifact(element = 1)]
    doc_id: String,
}

let input = b"UNH+1+ORDERS:D:11A:UN'BGM+220+PO-001+9'BGM+231+PO-002+9'UNT+4+1'";

// Stop after the first BGM:
let first: Bgm = deserialize_first_streaming(input)?;
assert_eq!(first.doc_id, "PO-001");

// Collect all BGM segments:
let all: Vec<Bgm> = deserialize_all_streaming(input)?;
assert_eq!(all.len(), 2);
assert_eq!(all[1].doc_id, "PO-002");
# Ok::<(), edifact_rs::EdifactError>(())
```

### Reader variants

```rust
use edifact_rs::{deserialize_first_from_reader, deserialize_all_from_reader};
use std::io::Cursor;

# use edifact_rs::EdifactDeserialize;
# #[derive(Debug, EdifactDeserialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] doc_code: String, #[edifact(element = 1)] doc_id: String }
let input = Cursor::new(b"BGM+220+PO-001+9'BGM+231+PO-002+9'".to_vec());

let first: Bgm = deserialize_first_from_reader(input.clone())?;
let all: Vec<Bgm> = deserialize_all_from_reader(input)?;
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Message-level typed streaming

`deserialize_messages_from_reader` combines message-window iteration with typed
deserialization. Each `UNH..UNT` window is deserialized into a message struct:

```rust
use edifact_rs::{EdifactDeserialize, deserialize_messages_from_reader};
use std::io::Cursor;

#[derive(Debug, EdifactDeserialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    doc_code: String,
    #[edifact(element = 1)]
    doc_id: String,
}

#[derive(Debug, EdifactDeserialize)]
struct OrderMessage {
    bgm: Option<Bgm>,
}

let interchange = Cursor::new(b"\
    UNB+UNOA:1+S+R+200101:0900+1'\
    UNH+1+ORDERS:D:96A:UN'BGM+220+PO-001+9'UNT+3+1'\
    UNH+2+ORDERS:D:96A:UN'BGM+220+PO-002+9'UNT+3+2'\
    UNZ+2+1'".to_vec());

let messages: Vec<OrderMessage> =
    deserialize_messages_from_reader::<OrderMessage, _>(interchange)
        .collect::<Result<_, _>>()?;

assert_eq!(messages.len(), 2);
assert_eq!(messages[0].bgm.as_ref().unwrap().doc_id, "PO-001");
assert_eq!(messages[1].bgm.as_ref().unwrap().doc_id, "PO-002");
# Ok::<(), edifact_rs::EdifactError>(())
```

### Zero-alloc owned deserialization path

`deserialize_messages_from_reader` calls `T::edifact_deserialize_owned(&window)`
rather than converting `OwnedSegment` → `Segment<'_>`. The `#[derive(EdifactDeserialize)]`
macro generates an override of `edifact_deserialize_owned` that accesses
`OwnedSegment::element_str` and `OwnedSegment::component_str` directly — **no
intermediate `Vec<Segment<'_>>` is allocated**.

This makes the reader path allocate at most:
- One `Vec<OwnedSegment>` per message window (released after deserialization)
- The deserialized `T` value itself

---

## Progressive (per-window) validation

Combine `message_windows_from_reader` with `ValidationContext` to validate each
message as it arrives, without buffering the whole interchange:

```rust
use edifact_rs::{
    ValidationContext, ProfileRulePack, ValidationIssue, ValidationSeverity,
    from_reader_iter, message_windows_from_reader, OwnedSegment,
};
use std::io::Cursor;

let input = Cursor::new(b"\
    UNH+1+ORDERS:D:11A:UN'BGM+220+PO-001+9'UNT+3+1'\
    UNH+2+ORDERS:D:11A:UN'BGM+220+PO-002+9'UNT+3+2'".to_vec());

let pack = ProfileRulePack::builder("ORDERS-PROGRESSIVE")
    .for_message_type("ORDERS")
    .with_rule_fn(|segs| {
        let has_bgm = segs.iter().any(|s| s.tag == "BGM");
        (!has_bgm).then(|| {
            ValidationIssue::new(
                ValidationSeverity::Error,
                "every ORDERS message must contain a BGM segment",
            )
            .with_rule_id("ORDERS-P001")
        })
    });

let ctx = ValidationContext::builder()
    .with_profile_pack(pack)
    .build();

for result in message_windows_from_reader(input) {
    let window = result?;
    // Borrow the owned window for validation
    let borrowed: Vec<_> = window.iter().map(|s| s.as_borrowed()).collect();
    let report = ctx.validate_lenient(&borrowed);
    if !report.is_valid() {
        for e in &report.errors {
            eprintln!("❌ {}", e.message);
        }
    }
}
# Ok::<(), edifact_rs::EdifactError>(())
```

See the full example in [`cookbook_streamed_progressive_validation.rs`](../crates/edifact-rs/examples/cookbook_streamed_progressive_validation.rs).

---

## Manual window assembly with `from_reader_iter`

For full control over window boundaries (e.g. custom grouping logic), use the raw
segment iterator:

```rust
use edifact_rs::{from_reader_iter, OwnedSegment};
use std::io::Cursor;

let input = Cursor::new(b"\
    UNH+1+ORDERS:D:11A:UN'\
    BGM+220+PO-001+9'\
    UNT+3+1'".to_vec());

let mut current: Vec<OwnedSegment> = Vec::new();
let mut in_message = false;

for result in from_reader_iter(input) {
    let seg = result?;
    match seg.tag.as_str() {
        "UNH" => {
            current.clear();
            in_message = true;
        }
        "UNT" => {
            if in_message {
                current.push(seg);
                println!("window complete: {} segments", current.len());
                in_message = false;
            }
            continue;
        }
        _ => {}
    }
    if in_message {
        current.push(seg);
    }
}
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Memory budget summary

| Scenario | Peak heap usage |
|---|---|
| `from_bytes` on a 1 MB slice | One `Vec<Element<'_>>` per segment (tags borrow from slice) |
| `from_reader_iter` on a 1 GB file | ~O(1 segment) at any time |
| `message_windows_from_reader` on 100 messages of 20 segments each | O(20 segments) — one window at a time |
| `deserialize_messages_from_reader` typed | O(20 segments) window + O(1 typed struct) |

---

## Next steps

- [Validation](validation.md) — validate windows and messages
- [Async Integration](async-integration.md) — bridging to tokio
- [Performance](performance.md) — allocation analysis and benchmarks
