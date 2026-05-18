# Writing ✍️

This guide covers every way to produce EDIFACT output — from typed structs to
raw segment construction and custom delimiter configuration.

---

## Overview of write APIs

| API | Best for |
|---|---|
| `ser::to_string(value)` | Quick serialization of a single derived struct |
| `ser::to_bytes(segments)` | Round-trip a parsed `Vec<Segment<'_>>` |
| `to_bytes(segments)` | Free function alias for `ser::to_bytes` |
| `Writer::write_segment(seg)` | Streaming segment-by-segment output |
| `Writer::write_raw(tag, elements)` | Build segments from runtime string data |
| `Writer::with_una(w, ssa)` | Output with custom UNA service string |

---

## Serialize a typed struct

```rust
use edifact_rs::{EdifactSerialize, ser};

#[derive(edifact_rs::EdifactSerialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    doc_code: String,
    #[edifact(element = 1)]
    doc_number: String,
    #[edifact(element = 2)]
    function_code: Option<String>,
}

let bgm = Bgm {
    doc_code: "220".into(),
    doc_number: "PO-4711".into(),
    function_code: Some("9".into()),
};

let output = ser::to_string(&bgm)?;
assert_eq!(output, "BGM+220+PO-4711+9'");

// or as bytes:
let bytes = ser::to_bytes(&bgm)?;
# Ok::<(), edifact_rs::EdifactError>(())
```

`None` fields produce empty elements in their positional slot:

```rust
let bgm_no_func = Bgm {
    doc_code: "220".into(),
    doc_number: "PO-4711".into(),
    function_code: None,
};
let out = ser::to_string(&bgm_no_func)?;
assert_eq!(out, "BGM+220+PO-4711+'");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Round-trip: parse then write

```rust
use edifact_rs::{from_bytes, to_bytes};

let input = b"UNA:+.? 'BGM+220+PO-4711+9'NAD+BY+4000001::9'";
let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

let output = to_bytes(&segs)?;
// output matches input byte-for-byte (UNA is preserved if present)
# Ok::<(), edifact_rs::EdifactError>(())
```

> **Note**: `to_bytes` uses the **default** EDIFACT delimiters when serializing
> `Segment<'_>` slices. If the original had a custom UNA you should use `Writer`
> directly to preserve the custom service string.

---

## Streaming writer

`Writer<W>` writes one segment at a time to any `Write` implementation:

```rust
use edifact_rs::{Writer, Segment, model::Element};

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);

writer.write_segment(&Segment::new("BGM", vec![
    Element::of(&["220"]),
    Element::of(&["PO-4711"]),
    Element::of(&["9"]),
]))?;

writer.write_segment(&Segment::new("NAD", vec![
    Element::of(&["BY"]),
    Element::of(&["4000001", "", "9"]), // composite element
]))?;

writer.finish()?;

let text = String::from_utf8(buf).unwrap();
assert_eq!(text, "BGM+220+PO-4711+9'NAD+BY+4000001::9'");
# Ok::<(), edifact_rs::EdifactError>(())
```

`Writer::finish()` flushes the underlying writer and returns it.

### Segment count tracking

`Writer` maintains an internal segment counter that is incremented on every
`write_segment` call. Retrieve it with `writer.segment_count()` to fill the `UNT`
segment's count field:

```rust
# use edifact_rs::{Writer, Segment, model::Element};
# let mut buf: Vec<u8> = Vec::new();
# let mut writer = Writer::new(&mut buf);
// ... write body segments ...

let count = writer.segment_count() + 2; // +2 for UNH and UNT themselves
writer.write_raw("UNT", &[&count.to_string(), "1"])?;
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## `write_raw` — runtime string data

When building segments from runtime data (e.g., database values), use `write_raw`
to avoid constructing `Segment` / `Element` objects:

```rust
use edifact_rs::Writer;

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);

// write_raw(tag, &[elements]) — components inside an element separated by ':'
writer.write_raw("DTM", &["137:20240101:102"])?;
writer.write_raw("RFF", &["ON:PO-4711"])?;

writer.finish()?;
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Custom delimiters and UNA

To write with non-default delimiters, create the writer with `Writer::with_una`:

```rust
use edifact_rs::{Writer, tokenizer::ServiceStringAdvice};

let ssa = ServiceStringAdvice {
    component_sep: b';',
    element_sep:   b'|',
    decimal_mark:  b'.',
    release_char:  b'?',
    segment_term:  b'!',
};

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::with_una(&mut buf, ssa)?;

writer.write_raw("BGM", &["220|PO-4711|9"])?;
writer.finish()?;

let text = String::from_utf8(buf).unwrap();
// UNA written first, then segments with custom delimiters:
assert!(text.starts_with("UNA;|.?!"));
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Escape handling

`Writer` **automatically escapes** any character in a component value that collides
with the current delimiter set. You never need to pre-escape data:

```rust
use edifact_rs::{Writer, Segment, model::Element};

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);

// "+" is the element separator — the writer escapes it automatically
writer.write_segment(&Segment::new("FTX", vec![
    Element::of(&["AAI"]),
    Element::of(&["Price: 100+VAT"]),  // '+' will be escaped as '?+'
]))?;
writer.finish()?;

let text = String::from_utf8(buf).unwrap();
assert_eq!(text, "FTX+AAI+Price: 100?+VAT'");
# Ok::<(), edifact_rs::EdifactError>(())
```

Characters escaped by default:
- `'` (segment terminator)
- `+` (element separator)
- `:` (component separator)
- `?` (the release character itself)

---

## Event-based writing (`WriterEmitter`)

For advanced use cases — such as writing segments produced by `EdifactSerialize`
derived types to a `Write` sink without buffering — use `WriterEmitter`:

```rust
use edifact_rs::{ser, WriterEmitter, EdifactSerialize, Writer};

# #[derive(EdifactSerialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] code: String }
let bgm = Bgm { code: "220".into() };

let mut buf: Vec<u8> = Vec::new();
let writer = Writer::new(&mut buf);
let mut emitter = WriterEmitter::new(writer);

bgm.edifact_serialize(&mut emitter)?;

let (inner_writer, _) = emitter.into_inner();
let _ = inner_writer.finish();
# Ok::<(), edifact_rs::EdifactError>(())
```

`WriterEmitter` is **allocation-free** per event: it writes directly to the
underlying `Write` on each `EdifactEvent` without buffering components in a `String`.

---

## Writing a full interchange

```rust
use edifact_rs::{Writer, Segment, model::Element};
use std::io::Cursor;

let mut buf: Vec<u8> = Vec::new();
let mut w = Writer::new(&mut buf);

// Interchange header
w.write_raw("UNB", &["UNOA:1", "SENDER:14", "RECEIVER:14", "200101:0900", "1"])?;
// Message header
w.write_raw("UNH", &["1", "ORDERS:D:96A:UN"])?;
// Body
w.write_raw("BGM", &["220", "PO-4711", "9"])?;
w.write_raw("NAD", &["BY", "4000001::9"])?;
// Message trailer (segment count includes UNH and UNT)
let body_segments = w.segment_count(); // BGM + NAD
let unt_count = body_segments + 2;
w.write_raw("UNT", &[&unt_count.to_string(), "1"])?;
// Interchange trailer
w.write_raw("UNZ", &["1", "1"])?;

w.finish()?;
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Next steps

- [Typed Derive](typed-derive.md) — derive `EdifactSerialize` for your structs
- [Parsing](parsing.md) — parse EDIFACT input to `Segment` slices
- [Performance](performance.md) — allocation budgets and benchmarking tips
