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
| `Writer::write_segment(seg)` | Round-tripping a parsed or hand-built `Segment` |
| `Writer::write_elements(tag, elements)` | **The general form** — segments mixing simple and composite data elements |
| `Writer::write_composites(tag, elements)` | Every element is a list of components; borrowed *or* owned data |
| `Writer::write_simple(tag, elements)` | Every element is one value — the commonest shape |
| `Writer::with_service_string_advice(w, ssa)` | Custom delimiters, **no** `UNA` emitted |
| `Writer::with_una(w, ssa)` | Custom delimiters, `UNA` written first |

Component boundaries are always **explicit**: no write method infers them by
splitting a string, so a value containing the active component separator is
release-escaped rather than silently promoted to a boundary.

## What the writer will not write

Everything the writer emits reparses. Two cases are refused before a byte
reaches the sink, so a rejected segment leaves nothing half-written:

| Refused | Error |
|---|---|
| A tag that is not three ASCII uppercase letters | `InvalidSegmentTag` |
| A repeating data element with no repetition separator declared | `RepetitionSeparatorNotDeclared` |

A tag is written verbatim — EDIFACT has no way to escape one — so `bgm`,
`BGMX` or `B+M` would produce bytes that do not read back as the segment they
came from:

```rust
use edifact_rs::{EdifactError, Writer};

let mut writer = Writer::new(Vec::new());
let err = writer.write_simple("bgm", &["220"]).unwrap_err();
assert!(matches!(err, EdifactError::InvalidSegmentTag(_)));
```

Delimiters *inside a value* are not a problem — those are release-escaped.

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
writer.write_simple("UNT", &[count.to_string().as_str(), "1"])?;
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
the same four methods (`write_simple`, `write_composites`, `write_elements`,
`write_segment`), so every segment you write inside a message is counted into the
`UNT` DE 0074 total.

---

## `write_simple` and `write_composites` — runtime string data

When building segments from runtime data — database rows, a mapping layer —
these two avoid constructing `Segment` / `Element` values at all.

`write_simple` is the all-simple shape: one value per data element, and a
component separator inside a value stays part of the value.

```rust
use edifact_rs::Writer;

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);

writer.write_simple("BGM", &["220", "PO-4711", "9"])?;
writer.write_simple("FTX", &["AAA", "ACME:INC"])?;   // the `:` is escaped, not split

writer.finish()?;
assert_eq!(buf, b"BGM+220+PO-4711+9'FTX+AAA+ACME?:INC'".to_vec());
# Ok::<(), edifact_rs::EdifactError>(())
```

`write_composites` takes the components explicitly, and its bounds accept
borrowed and owned data through the same call — `&[&[&str]]`, `&[Vec<String>]`,
`&[[String; 3]]`:

```rust
use edifact_rs::Writer;

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);

// Borrowed literals …
writer.write_composites("DTM", &[&["137", "20240101", "102"][..]])?;
// … and owned values built at runtime.
let rff = vec![vec!["ON".to_string(), "PO-4711".to_string()]];
writer.write_composites("RFF", &rff)?;

writer.finish()?;
assert_eq!(buf, b"DTM+137:20240101:102'RFF+ON:PO-4711'".to_vec());
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Custom delimiters and UNA

`Writer::with_una` writes a `UNA` header and then uses those delimiters:

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

// One `&str` per data element; the custom element separator `|` is applied.
writer.write_simple("BGM", &["220", "PO-4711", "9"])?;
writer.finish()?;

let text = String::from_utf8(buf).unwrap();
// UNA header first (9 bytes, including the space "not used" repetition slot),
// then the segment with the custom delimiters.
assert_eq!(text, "UNA;|.? !BGM|220|PO-4711|9!");
# Ok::<(), edifact_rs::EdifactError>(())
```

### Delimiters without a `UNA`

A syntax-version-4 interchange can carry repeating data elements and no `UNA` at
all — the version in `UNB` S001 DE 0002 is what declares the separator. Writing
one back through `with_una` would invent a header the original never had, so
declare the delimiters to the writer directly instead:

```rust
use edifact_rs::{ServiceStringAdvice, Writer, from_bytes};

let input = b"UNB+UNOC:4+S+R+260101:0900+I'RFF+ON:1*ON:2'UNZ+0+I'";
let segments: Vec<_> = from_bytes(input).collect::<Result<Vec<_>, _>>()?;

let ssa = ServiceStringAdvice::for_syntax_version(Some(4));
let mut writer = Writer::with_service_string_advice(Vec::new(), ssa)?;
for segment in &segments {
    writer.write_segment(segment)?;
}
// Byte-for-byte the input, with no UNA invented along the way.
assert_eq!(writer.finish()?, input.to_vec());
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Repeating data elements

ISO 9735-1 §8.6 repetitions are written with the repetition separator from UNA
position 050, so the writer needs a `ServiceStringAdvice` that declares one.
Writing a repeating element through a writer that has none is
`EdifactError::RepetitionSeparatorNotDeclared`, checked before the first byte is
emitted so a rejected segment leaves nothing half-written behind it:

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

The refusal is checked before the first byte leaves the writer, so a rejected
segment writes **nothing**. The writer stays usable: log the error, skip the
segment, and keep going — the next segment starts at a clean boundary rather
than after a dangling `RFF+`.

### Through the event layer

`EdifactEvent::RepeatElement` is the write-side mirror of the parser's
repetition split, so a value that arrives as several occurrences can leave as
several occurrences:

```rust
use edifact_rs::{EdifactEvent, EventEmitter, ServiceStringAdvice, WriterEmitter};

let ssa = ServiceStringAdvice::from_bytes(b"UNA:+.?*'")?;
let mut emitter = WriterEmitter::with_una(Vec::new(), ssa)?;
emitter.emit(EdifactEvent::start("RFF"))?;
emitter.emit(EdifactEvent::element("ON"))?;
emitter.emit(EdifactEvent::component("1"))?;
emitter.emit(EdifactEvent::repeat("ON"))?;  // second occurrence
emitter.emit(EdifactEvent::component("2"))?;
emitter.emit(EdifactEvent::EndSegment)?;

assert_eq!(emitter.finish()?, b"UNA:+.?*'RFF+ON:1*ON:2'".to_vec());
# Ok::<(), edifact_rs::EdifactError>(())
```

The same `E037` refusal applies: a `RepeatElement` under a writer with no
declared separator is rejected before the separator byte is written.

---

## Writing in a non-UTF-8 repertoire

`Writer::with_charset` binds the writer to the repertoire named in `UNB` S001, so
a `UNOC` interchange goes out as ISO 8859-1 rather than as UTF-8 — and a
character the repertoire cannot carry is refused instead of being written as
bytes the receiver decodes as something else.

The typed path reaches it through `WriterEmitter::with_charset`, so a
`#[derive(EdifactSerialize)]` struct is bound the same way:

```rust
use edifact_rs::{Charset, EdifactEvent, EventEmitter, WriterEmitter};

let mut emitter = WriterEmitter::new(Vec::new()).with_charset(Charset::UnoC);
emitter.emit(EdifactEvent::start("NAD"))?;
emitter.emit(EdifactEvent::element("Müller"))?;
emitter.emit(EdifactEvent::EndSegment)?;

// `ü` goes out as the single Latin-1 byte 0xFC, not as two UTF-8 bytes.
assert_eq!(emitter.finish()?, b"NAD+M\xFCller'".to_vec());
# Ok::<(), edifact_rs::EdifactError>(())
```

See [Character Sets](@/docs/character-sets.md) for the full repertoire table.

---

## Character repertoires

`UNB` S001 names the repertoire the payload is written in. A writer that emits
UTF-8 into a `UNOC` interchange produces mojibake at the far end with nothing in
the file to explain it, so bind the writer to the repertoire and let it encode:

```rust
use edifact_rs::{Charset, Writer};

let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoC);
writer.write_composites("NAD", &[&["BY"], &["Müller"]])?;
// `ü` goes out as the single ISO 8859-1 byte 0xFC.
assert_eq!(writer.finish()?, b"NAD+BY+M\xFCller'".to_vec());
# Ok::<(), edifact_rs::EdifactError>(())
```

A character the repertoire cannot carry is refused with
`EdifactError::CharacterNotInRepertoire` (`E038`), and a `UNB` declaring a
repertoire the writer does not encode is refused with
`CharacterRepertoireMismatch` (`E041`) — the header cannot lie about the body.

See [Character Sets](@/docs/character-sets.md) for the read side and for
repertoire validation.

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
w.write_simple("UNB", &["UNOA:1", "SENDER:14", "RECEIVER:14", "200101:0900", "1"])?;
// Message header
w.write_simple("UNH", &["1", "ORDERS:D:96A:UN"])?;
// Body
w.write_simple("BGM", &["220", "PO-4711", "9"])?;
w.write_simple("NAD", &["BY", "4000001::9"])?;
// Message trailer (segment count includes UNH and UNT)
let body_segments = w.segment_count(); // BGM + NAD
let unt_count = body_segments + 2;
w.write_simple("UNT", &[unt_count.to_string().as_str(), "1"])?;
// Interchange trailer
w.write_simple("UNZ", &["1", "1"])?;

w.finish()?;
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Next steps

- [Typed Derive](@/docs/typed-derive.md) — derive `EdifactSerialize` for your structs
- [Parsing](@/docs/parsing.md) — parse EDIFACT input to `Segment` slices
- [Performance](@/docs/performance.md) — allocation budgets and benchmarking tips
