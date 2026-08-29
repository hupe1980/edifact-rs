+++
title = "Core Concepts"
description = "The EDIFACT wire format — segments, data elements, components, the UNA service string advice, release characters, and repetition — mapped onto the Rust types."
weight = 20
+++

This guide explains the EDIFACT wire format and maps it to the Rust types exposed
by `edifact-rs`. Understanding this makes every other guide easier to follow.

---

## What is EDIFACT?

**EDIFACT** (Electronic Data Interchange For Administration, Commerce and Transport)
is the UN's international standard for structured business message interchange,
defined in ISO 9735. It is widely used in supply chains, logistics, energy markets,
healthcare, and banking.

An EDIFACT **interchange** is a flat text document composed of **segments**
separated by a single terminator character (usually `'`). Each segment carries
**data elements** separated by `+`, and each element may contain **component values**
separated by `:`.

---

## Wire format anatomy

```text
UNA:+.? '
UNB+UNOA:3+SENDER:14+RECEIVER:14+260101:0900+IC4711'
UNH+MSG1+ORDERS:D:96A:UN+MYREF'
BGM+220+PO-4711+9'
NAD+BY+4000001000002::9'
NAD+SU+4000001000001::9'
UNT+5+MSG1'
UNZ+1+IC4711'
```

| Part | Purpose |
|---|---|
| `UNA:+.? '` | Service string advice — defines the 6 special characters |
| `UNB` | Interchange header (sender, receiver, date, reference) |
| `UNH` | Message header — starts one functional message |
| Body segments | Payload (`BGM`, `NAD`, `LIN`, `MOA`, …) |
| `UNT` | Message trailer — declares segment count and reference |
| `UNZ` | Interchange trailer — declares message count and reference |

A single interchange may contain **many UNH..UNT pairs** (multi-message interchange).

---

## The UNA service string advice

The `UNA` segment is always exactly **9 bytes**: the literal `UNA` followed by six
service characters in fixed positions:

```text
U N A : + . ?   '
      │ │ │ │ │ └── Segment terminator   (default: ' )
      │ │ │ │ └──── Repetition separator (`*`, or space = "not used")
      │ │ │ └────── Release character    (default: ? )
      │ │ └──────── Decimal mark         (ignored on receipt)
      │ └────────── Element separator    (default: + )
      └──────────── Component separator  (default: : )
```

ISO 9735-1 numbers these positions `010`–`060`; the byte offsets below are the
same six characters counted from the start of the input.

| UNA byte | Position | Purpose | Default | Splits input? |
|---|---|---|---|---|
| 3 | 010 | Component data element separator | `:` | yes |
| 4 | 020 | Data element separator | `+` | yes |
| 5 | 030 | Decimal mark | `.` | no — see below |
| 6 | 040 | Release (escape) character | `?` | escapes the next byte |
| 7 | 050 | Repetition separator | `*` | yes, **when not a space** |
| 8 | 060 | Segment terminator | `'` | yes |

`edifact-rs` reads the UNA on the first call to `from_bytes` / `from_reader` and
applies those delimiters to everything that follows.

**The decimal mark is ignored.** ISO 9735-1 Annex B keeps position 030 only for
upward compatibility with earlier syntax versions and says the character
transferred there "shall be ignored by the recipient" — §10 instead allows the
full stop *or* the comma per individual numeric value. It is therefore the one
position where a space is legal, the one that need not be distinct from the
others, and the only one `edifact-rs` does not validate. The value is still kept,
so a writer can round-trip the UNA it was handed and `DecimalFloat` has a house
style to format with.

**The repetition separator depends on the syntax version.** Version 4 introduced
it and §5.1 makes `*` its default; versions 1–3 have no such service character at
all, and their UNA carries a space in that position. With no UNA to say
otherwise, `edifact-rs` reads the version from `UNB` S001 DE 0002 and activates
`*` only for version 4 — splitting on `*` in a version 3 interchange would
corrupt every value containing one, because there `*` is an ordinary level A
character. See [Repeating data elements](#repeating-data-elements) below.

For a fragment that carries neither a UNA nor a UNB — a single message lifted out
of an interchange — nothing records the delimiters, so supply them:

```rust
use edifact_rs::{ReaderConfig, ServiceStringAdvice, from_bytes_with_config};

let ssa = ServiceStringAdvice::from_bytes(b"UNA:;.? ~")?;
let config = ReaderConfig::default().with_service_string_advice(ssa);

let segments: Vec<_> = from_bytes_with_config(b"BGM;220;PO-4711~", config)
    .collect::<Result<Vec<_>, _>>()?;
assert_eq!(segments[0].element_str(1), Some("PO-4711"));
# Ok::<(), edifact_rs::EdifactError>(())
```

> **Security note**: `edifact-rs` fails hard on a malformed UNA — wrong byte
> count, a duplicated *active* delimiter, an alphanumeric delimiter — and never
> silently falls back to defaults. This prevents delimiter injection attacks.

---

## Segment structure

A segment has:

1. **Tag** — exactly 3 uppercase ASCII letters (e.g. `BGM`, `NAD`, `UNH`)
2. **Elements** — separated by the element separator (`+`)
3. **Components** — within an element, separated by the component separator (`:`)
4. **Terminator** — marks the end of the segment (`'`)

Example breakdown:

```text
BGM  +  220  +  PO-4711  +  9  '
 │       │         │        │
 tag   elem 0   elem 1   elem 2 (all single-component)

NAD+BY+4000001000002::9'
 │   │        │         │
 │   │        │         └── component 2 of element 1
 │   │        └──────────── component 0 of element 1
 │   └───────────────────── element 0 (one component)
 └───────────────────────── tag
```

The `::` is not a typo. Component 1 is *omitted*, and its position is held by the
separator that would have followed it (ISO 9735-1 §8.7.2) — so the agency
qualifier stays at component 2 rather than sliding into component 1.

Element 0 of `NAD` is `BY` — this is the **qualifier**. `edifact-rs` derive macros
use `qualifier_from = 0` to dispatch different Rust structs for `NAD+BY` vs `NAD+SU`.

---

## Release characters

The **release character** (`?` by default) escapes the next byte, allowing delimiters
to appear as literal text:

```text
BGM+Test?+value'
         ^^
         '?+' means a literal '+', not an element separator
```

`edifact-rs` resolves release sequences during parsing and stores the decoded value.
The raw escape is never visible to API consumers — you receive `"Test+value"` as a
plain `&str`.

A trailing `?` at end-of-input (with no following byte) is **malformed** and causes
`EdifactError::InvalidReleaseSequence` (error code `E018`).

---

## Rust type mapping

### One segment type, borrowed or owned

`edifact-rs` has a **single** segment type. `Segment<'a>` holds its text as
`Cow<'a, str>`, which covers both parsing modes:

```text
pub struct Segment<'a> {
    pub tag: Cow<'a, str>,            // borrowed from the input, or owned
    pub span: Span,                   // byte range of the whole segment
    pub tag_span: Span,               // byte range of just the tag
    pub elements: Vec<Element<'a>>,
}
```

- `from_bytes` borrows straight out of the input buffer and yields
  `Segment<'input>` — no allocation for segment data.
- `from_reader` has no buffer to borrow from, so it yields `Segment<'static>`,
  which is aliased as **`OwnedSegment`**.

`Segment` is covariant in `'a`, so a `&[OwnedSegment]` is accepted anywhere a
`&[Segment<'_>]` is wanted. Every function in the crate therefore takes one
shape and works with both — there are no `_owned` twins to remember, and no
conversion step between the two paths:

```rust
use edifact_rs::{OwnedSegment, Segment};

fn count_bgm(segments: &[Segment<'_>]) -> usize {
    segments.iter().filter(|s| s.tag == "BGM").count()
}

let borrowed: Vec<Segment<'_>> =
    edifact_rs::from_bytes(b"BGM+220'").collect::<Result<_, _>>()?;
let owned: Vec<OwnedSegment> =
    edifact_rs::from_reader(std::io::Cursor::new(b"BGM+220'")).collect::<Result<_, _>>()?;

assert_eq!(count_bgm(&borrowed), 1);
assert_eq!(count_bgm(&owned), 1);   // same function, no conversion
# Ok::<(), edifact_rs::EdifactError>(())
```

To keep a segment past the buffer it was parsed from, call
`Segment::into_owned()`.

`segment.tag` compares directly against a string literal (`segment.tag == "BGM"`);
`segment.tag()` is the `&str` for the places that need one, such as a `match`.

### `Element<'a>` — component holder

```text
pub struct Element<'a> {
    pub span: Span,
    pub components: Components<'a>,       // (value, span) — inline for ≤4 components
    pub repeats: Vec<Components<'a>>,     // further occurrences; usually empty
}
```

`Cow::Borrowed` is used when the component contains no release sequences.
`Cow::Owned` is used only when an escape was resolved (the decoded string differs
from the raw bytes) or when the segment came from a reader.

### Reading values

Every accessor lives on `Segment`, so it is available on both parsing paths:

| Method | Returns |
|---|---|
| `element_str(n)` | component 0 of element `n` |
| `component_str(elem, comp)` | one specific component |
| `get_element(n)` | the whole `Element` |
| `optional_element(n)` / `optional_component(e, c)` | as above, but empty counts as absent |
| `required_element(n)` / `required_component(e, c)` | `Err` when absent or empty |
| `parsed_element::<T>(n)` | parsed into `T`, `Err` when absent or unparseable |
| `repeated_component(elem, comp)` | one component across every occurrence |
| `value_by_code(&layout, "3055")` | addressed by data-element identifier |

### Repeating data elements

ISO 9735-1 §8.6 lets one data element occur several times in a single slot,
separated by the repetition separator — UNA position 050, or `*` by default in a
syntax version 4 interchange:

```text
RFF+ON:1*ON:2*ON:3'
    └──┬─┘ └──┬─┘ └──┬─┘
       0      1      2     ← three occurrences of element 0
```

`components` always holds occurrence 0, so every positional accessor —
`element_str`, `component_str`, `value_by_code`, and the derive macros — keeps
reading the first occurrence and behaves identically on the interchanges that do
not use the feature. The remaining occurrences live in `repeats`:

```rust
// `UNA` byte 7 declares `*` as the repetition separator.
let segments: Vec<_> = edifact_rs::from_bytes(b"UNA:+.?*'RFF+ON:1*ON:2'")
    .collect::<Result<Vec<_>, _>>()?;
let rff = segments[0].get_element(0).unwrap();

assert_eq!(rff.repeat_count(), 2);
assert_eq!(rff.get_component(1), Some("1"));                  // occurrence 0
assert_eq!(rff.repetition(1).unwrap()[1].0.as_ref(), "2");    // occurrence 1

// Or read one component across every occurrence at once:
let all: Vec<&str> = segments[0].repeated_component(0, 1).collect();
assert_eq!(all, ["1", "2"]);
# Ok::<(), edifact_rs::EdifactError>(())
```

Without an active separator the byte is ordinary data: `RFF+ON:1*ON:2'` in a
syntax version 3 interchange yields the single component `1*ON`. A value that
legitimately contains the separator is release-escaped by the writer and
unescaped on the way back in, so `a?*b` round-trips as `a*b`.

### `Span` — byte position

```rust
pub struct Span { pub start: usize, pub end: usize }
```

Every `Segment`, `Element`, and component carries a `Span` into the original input.
Diagnostics use these spans to show precise error locations.

---

## Envelope vs. body segments

EDIFACT distinguishes **envelope** segments from **body** segments:

| Segment | Role |
|---|---|
| `UNA` | Service string advice (optional, always first) |
| `UNB` | Interchange header — mandatory outer wrapper |
| `UNZ` | Interchange trailer |
| `UNG` | Functional group header (optional) |
| `UNE` | Functional group trailer (optional) |
| `UNH` | Message header — begins a logical message |
| `UNT` | Message trailer |
| Everything else | Message body (`BGM`, `NAD`, `LIN`, …) |

`edifact-rs` validates envelope reference parity (`UNB`↔`UNZ` and `UNH`↔`UNT`) and
rejects envelope control tags that appear in message body positions.

---

## Message types

The `UNH` segment element 1, component 0 carries the **message type** (e.g.
`ORDERS`, `INVOIC`, `ORDERS`). `ValidationContext` and `ProfileRulePack` scope their
rules to a specific message type:

```text
UNH + 1 + ORDERS : D : 96A : UN '
             ^^^   ^   ^^^   ^^
             type  dir rel   org
```

| Component | Meaning |
|---|---|
| 0 | Message type (`ORDERS`, `INVOIC`, …) |
| 1 | Message version number (`D` = draft) |
| 2 | Message release number (`96A`, `11A`, …) |
| 3 | Controlling agency (`UN`) |

---

## Character set

`edifact-rs` operates on **UTF-8** text throughout, and rejects invalid byte
sequences with `EdifactError::InvalidText` (`E003`).

That is not the whole story, because **UTF-8 is not a superset of `UNOC`**. `UNB`
S001 component 1 names the repertoire the payload is written in, and `UNOC` is
ISO 8859-1 — where `ü` is the single byte `0xFC`, which is not valid UTF-8. A
conformant German interchange therefore has to be decoded before it can be
parsed:

```rust
use edifact_rs::{decode_interchange, from_bytes};

let mut raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'NAD+BY+M".to_vec();
raw.push(0xFC);
raw.extend_from_slice(b"ller'UNZ+0+IC1'");

let utf8 = decode_interchange(&raw)?;                     // reads UNB S001
let segments: Vec<_> = from_bytes(&utf8).collect::<Result<Vec<_>, _>>()?;
assert_eq!(segments[1].element_str(1), Some("Müller"));
# Ok::<(), edifact_rs::EdifactError>(())
```

`decode_interchange` copies nothing when the payload is already ASCII or `UNOY`,
so it is safe to call on every input. See
[Character Sets](@/docs/character-sets.md) for the streaming decoder, the write
side, and repertoire validation.

---

## Further reading

- [Parsing guide](@/docs/parsing.md) — `from_bytes`, `from_reader`, reader config
- [Typed Derive guide](@/docs/typed-derive.md) — mapping segments to Rust structs
- [Error Reference](@/docs/error-reference.md) — all error codes and their meanings
