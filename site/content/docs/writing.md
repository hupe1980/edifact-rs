+++
title = "Writing"
description = "Serialize segments back to the EDIFACT wire format with Writer, automatic delimiter escaping, custom UNA service strings, and repeating data elements."
weight = 40
+++

This guide covers every way to produce EDIFACT output — from typed structs to
raw segment construction and custom delimiter configuration.

---

## Overview of write APIs

| API | Best for |
|---|---|
| `to_edifact_string(value)` | Quick serialization of a single derived struct |
| `ser::to_bytes(segments)` | Round-trip a parsed `Vec<Segment<'_>>` |
| `to_bytes(segments)` | Free function alias for `ser::to_bytes` |
| `Writer::write_segment(seg)` | Streaming segment-by-segment output |
| `Writer::write_elements(tag, elements)` | **The general form** — segments mixing simple and composite data elements |
| `Writer::write_composites(tag, elements)` | Segments whose every element is a composite |
| `Writer::write_raw(tag, elements)` | All-simple segments, component boundaries inferred by splitting |
| `Writer::write_segment_parts(tag, elements)` | Same as `write_elements`, for owned `String` data |
| `Writer::with_una(w, ssa)` | Output with custom UNA service string |

---

## Serialize a typed struct

```rust
use edifact_rs::{EdifactSerialize, ser, to_edifact_string};

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

let output = to_edifact_string(&bgm)?;
assert_eq!(output, "BGM+220+PO-4711+9'");

// or as bytes:
let bytes = ser::to_bytes(&bgm)?;
# Ok::<(), edifact_rs::EdifactError>(())
```

`None` fields produce empty elements in their positional slot:

```rust
# use edifact_rs::to_edifact_string;
# #[derive(edifact_rs::EdifactSerialize)]
# #[edifact(segment = "BGM")]
# struct Bgm {
#     #[edifact(element = 0)]
#     doc_code: String,
#     #[edifact(element = 1)]
#     doc_number: String,
#     #[edifact(element = 2)]
#     function_code: Option<String>,
# }
# 
# let bgm = Bgm {
#     doc_code: "220".into(),
#     doc_number: "PO-4711".into(),
#     function_code: Some("9".into()),
# };
let bgm_no_func = Bgm {
    doc_code: "220".into(),
    doc_number: "PO-4711".into(),
    function_code: None,
};
let out = to_edifact_string(&bgm_no_func)?;
assert_eq!(out, "BGM+220+PO-4711+'");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Round-trip: parse then write

```rust
use edifact_rs::{from_bytes, segments_to_bytes};

let input = b"UNA:+.? 'BGM+220+PO-4711+9'NAD+BY+4000001::9'";
let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

let output = segments_to_bytes(&segs)?;
assert_eq!(output, b"BGM+220+PO-4711+9'NAD+BY+4000001::9'");
# Ok::<(), edifact_rs::EdifactError>(())
```

`segments_to_bytes` is the entry point for a slice of parsed `Segment`s;
`ser::to_bytes` is for a value that implements `EdifactSerialize`.

> **Note**: `segments_to_bytes` writes with the **default** EDIFACT delimiters and
> emits no `UNA` header, so the round-trip above is not byte-for-byte when the
> input carried one. Use `Writer::with_una` to preserve a custom service string.

---

## Streaming writer

`Writer<W>` writes one segment at a time to any `Write` implementation:

```rust
use edifact_rs::{Writer, Segment, Element};

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
# use edifact_rs::{Writer, Segment, Element};
# let mut buf: Vec<u8> = Vec::new();
# let mut writer = Writer::new(&mut buf);
// ... write body segments ...

let count = writer.segment_count() + 2; // +2 for UNH and UNT themselves
writer.write_raw("UNT", &[&count.to_string(), "1"])?;
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## `write_elements` — mixed simple and composite elements

Most real EDIFACT segments mix the two shapes: `NAD` takes a simple qualifier
followed by a composite party identification, `DTM` takes a single composite.
`write_elements` expresses that directly, with component boundaries given
explicitly rather than inferred:

```rust
use edifact_rs::{DataElement, Writer};

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);

writer.write_elements("NAD", &[
    DataElement::Simple("MS"),
    DataElement::Composite(&["9900112233445", "", "293"]),
])?;
writer.write_elements("DTM", &[
    DataElement::Composite(&["137", "20260101", "102"]),
])?;

writer.finish()?;
assert_eq!(
    String::from_utf8(buf).unwrap(),
    "NAD+MS+9900112233445::293'DTM+137:20260101:102'",
);
# Ok::<(), edifact_rs::EdifactError>(())
```

The `elements!` macro is shorthand for the same thing. Each entry is an ordinary
expression borrowed through the `AsDataElement` trait: a string becomes a simple
data element, an array/slice/`Vec` of strings becomes a composite. Runtime values
work exactly like literals, which is the point — builders rarely have literals:

```rust
use edifact_rs::{Writer, elements};

let qualifier = String::from("MS");
let gln = "9900112233445";

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);

writer.write_elements("NAD", elements![qualifier.as_str(), [gln, "", "293"]])?;
writer.write_elements("DTM", elements![["137", "20260101", "102"]])?;

writer.finish()?;
assert_eq!(
    String::from_utf8(buf).unwrap(),
    "NAD+MS+9900112233445::293'DTM+137:20260101:102'",
);
# Ok::<(), edifact_rs::EdifactError>(())
```

> Composite components must be string *slices*: a `[String; N]` cannot borrow as
> `&[&str]` without allocating, so write `[id.as_str(), "", agency]`.

Because boundaries are explicit, a value containing a literal component
separator is **escaped** rather than silently promoted to a boundary — which is
the failure mode of pre-joining components into one string:

```rust
# use edifact_rs::{Writer, elements};
# let mut buf: Vec<u8> = Vec::new();
# let mut writer = Writer::new(&mut buf);
writer.write_elements("NAD", elements!["MS", "ACME:INC"])?;
writer.finish()?;
// The `:` stays inside the value:
assert_eq!(String::from_utf8(buf).unwrap(), "NAD+MS+ACME?:INC'");
# Ok::<(), edifact_rs::EdifactError>(())
```

`MessageWriter` — the `UNH`/`UNT` guard from `Writer::begin_message` — carries
the same methods (`write_elements`, `write_composites`, `write_segment_parts`,
`write_raw`, `write_segment`), so every segment you write inside a message is
counted into the `UNT` DE 0074 total.

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
// Caveat: the split is on the *active* component separator, and a literal `:`
// inside a value becomes a boundary. Prefer `write_elements` when either
// matters.

writer.finish()?;
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Custom delimiters and UNA

To write with non-default delimiters, create the writer with `Writer::with_una`:

```rust
use edifact_rs::{Writer, ServiceStringAdvice};

let ssa = ServiceStringAdvice {
    component_sep: b';',
    element_sep:   b'|',
    decimal_mark:  b'.',
    release_char:  b'?',
    repetition_sep: b' ',   // 0x20 = 'not used'
    segment_term:  b'!',
};

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::with_una(&mut buf, ssa)?;

// One `&str` per data element; `write_raw` splits each on the *component*
// separator, which this UNA sets to `;`.
writer.write_raw("BGM", &["220", "PO-4711", "9"])?;
writer.finish()?;

let text = String::from_utf8(buf).unwrap();
// UNA header first (9 bytes, including the space "not used" repetition slot),
// then the segment with the custom delimiters.
assert_eq!(text, "UNA;|.? !BGM|220|PO-4711|9!");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Repeating data elements

ISO 9735-4 §3.1 repetitions are written with the repetition separator from UNA
position 7, so the writer needs a `ServiceStringAdvice` that declares one:

```rust
use edifact_rs::{Element, Segment, ServiceStringAdvice, Writer};

let ssa = ServiceStringAdvice { repetition_sep: b'*', ..Default::default() };
let mut buf: Vec<u8> = Vec::new();
{
    let mut writer = Writer::with_una(&mut buf, ssa)?;
    let rff = Segment::new(
        "RFF",
        vec![Element::of(&["ON", "1"]).and_repeat(&["ON", "2"])],
    );
    writer.write_segment(&rff)?;
}
assert!(String::from_utf8(buf).unwrap().ends_with("RFF+ON:1*ON:2'"));
# Ok::<(), edifact_rs::EdifactError>(())
```

Writing a repeating element through a writer that has **no** declared separator
is refused with `EdifactError::RepetitionSeparatorNotDeclared` (`E037`) rather
than silently joined with the space sentinel — which would read back as a single
occurrence.

---

## Escape handling

`Writer` **automatically escapes** any character in a component value that collides
with the current delimiter set. You never need to pre-escape data:

```rust
use edifact_rs::{Writer, Segment, Element};

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);

// Both `:` and `+` are delimiters — the writer escapes each of them.
writer.write_segment(&Segment::new("FTX", vec![
    Element::of(&["AAI"]),
    Element::of(&["Price: 100+VAT"]),  // ':' → '?:'  and  '+' → '?+'
]))?;
writer.finish()?;

let text = String::from_utf8(buf).unwrap();
assert_eq!(text, "FTX+AAI+Price?: 100?+VAT'");
# Ok::<(), edifact_rs::EdifactError>(())
```

Characters escaped by default:
- `'` (segment terminator)
- `+` (element separator)
- `:` (component separator)
- `?` (the release character itself)

Plus the repetition separator, when the active `UNA` declares one. The space
"not used" sentinel at UNA position 7 is never escaped.

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
// `WriterEmitter::new` takes the `io::Write` sink directly and constructs its
// own `Writer` internally — do not wrap the sink in a `Writer` first.
let mut emitter = WriterEmitter::new(&mut buf);

bgm.edifact_serialize(&mut emitter)?;

// `finish` flushes and hands back the sink that was passed in.
let _sink = emitter.finish()?;
# Ok::<(), edifact_rs::EdifactError>(())
```

`WriterEmitter` is **allocation-free** per event: it writes directly to the
underlying `Write` on each `EdifactEvent` without buffering components in a `String`.

---

## Writing a full interchange

```rust
use edifact_rs::{Writer, Segment, Element};
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

- [Typed Derive](@/docs/typed-derive.md) — derive `EdifactSerialize` for your structs
- [Parsing](@/docs/parsing.md) — parse EDIFACT input to `Segment` slices
- [Performance](@/docs/performance.md) — allocation budgets and benchmarking tips
