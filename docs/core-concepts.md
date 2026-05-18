# Core Concepts 🧩

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

```
UNA:+.? '
UNB+UNOA:1+SENDER:14+RECEIVER:14+200101:0900+1'
UNH+1+ORDERS:D:96A:UN+MYREF'
BGM+220+PO-4711+9'
NAD+BY+4000001000002::9'
NAD+SU+4000001000001::9'
UNT+6+1'
UNZ+1+1'
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

```
U N A : + . ?   '
        │ │ │ │ │ └── Segment terminator (default: ' )
        │ │ │ │ └──── Release character   (default: ? )
        │ │ │ └────── Decimal mark        (default: . )
        │ │ └──────── Repetition sep.     (default:   )
        │ └────────── Element separator   (default: + )
        └──────────── Component separator (default: : )
```

`edifact-rs` reads the UNA on the first call to `from_bytes` / `from_reader` and
applies the custom delimiters for all subsequent parsing. If UNA is absent, the six
EDIFACT defaults shown above are used.

> **Security note**: `edifact-rs` fails hard on a malformed UNA (wrong byte count,
> duplicate delimiter bytes) and never silently falls back to defaults. This prevents
> delimiter injection attacks.

---

## Segment structure

A segment has:

1. **Tag** — exactly 3 uppercase ASCII letters (e.g. `BGM`, `NAD`, `UNH`)
2. **Elements** — separated by the element separator (`+`)
3. **Components** — within an element, separated by the component separator (`:`)
4. **Terminator** — marks the end of the segment (`'`)

Example breakdown:

```
BGM  +  220  +  PO-4711  +  9  '
 │       │         │        │
 tag   elem 0   elem 1   elem 2 (all single-component)

NAD + BY + 4000001000002 :: 9 '
 │     │         │          │
 tag  elem 0   elem 1        └── component 1 of elem 1
              │
              component 0 of elem 1
```

Element 0 of `NAD` is `BY` — this is the **qualifier**. `edifact-rs` derive macros
use `qualifier_from = 0` to dispatch different Rust structs for `NAD+BY` vs `NAD+SU`.

---

## Release characters

The **release character** (`?` by default) escapes the next byte, allowing delimiters
to appear as literal text:

```
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

### `Segment<'a>` — zero-copy view

```rust
pub struct Segment<'a> {
    pub tag: &'a str,       // borrows from input
    pub span: Span,         // byte range of the whole segment
    pub tag_span: Span,     // byte range of just the tag
    pub elements: Vec<Element<'a>>,
}
```

`Segment<'a>` **borrows** its tag and all element text directly from the input
`&[u8]`. No heap allocation is needed for the values; only the `Vec<Element>` and
the `SmallVec<[Cow<'a, str>; 4]>` per element are allocated.

### `Element<'a>` — component holder

```rust
pub struct Element<'a> {
    pub span: Span,
    pub components: SmallVec<[Cow<'a, str>; 4]>,  // inline for ≤4 components
    pub component_spans: SmallVec<[Span; 4]>,
}
```

`Cow::Borrowed` is used when the component contains no release sequences.
`Cow::Owned` is used only when an escape was resolved (the decoded string differs
from the raw bytes).

### `OwnedSegment` — heap-owned copy

When parsing from a `Read` source (`from_reader_iter`, `message_windows_from_reader`),
the library can't borrow from the input buffer. It produces `OwnedSegment` instead:

```rust
pub struct OwnedSegment {
    pub tag: String,
    pub elements: Vec<OwnedElement>,
}
```

`OwnedSegment` provides two accessors that avoid extra allocation:

```rust
seg.element_str(n)           // component 0 of element n → Option<&str>
seg.component_str(elem, comp) // specific component → Option<&str>
```

To get a zero-allocation `Segment<'_>` view of an `OwnedSegment`, call:

```rust
let borrowed: BorrowedSegment<'_> = seg.borrow();
```

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
`ORDERS`, `INVOIC`, `UTILMD`). `ValidationContext` and `ProfileRulePack` scope their
rules to a specific message type:

```
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

`edifact-rs` expects all segment text to be valid **UTF-8**. ISO 9735 defines
several character sets (UNOA, UNOB, UNOC/UTF-8, …), but the library operates on
decoded text and rejects invalid byte sequences with `EdifactError::InvalidText`
(error code `E003`).

---

## Further reading

- [Parsing guide](parsing.md) — `from_bytes`, `from_reader`, reader config
- [Typed Derive guide](typed-derive.md) — mapping segments to Rust structs
- [Error Reference](error-reference.md) — all error codes and their meanings
