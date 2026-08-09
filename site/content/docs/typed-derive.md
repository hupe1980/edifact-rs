+++
title = "Typed Derive"
description = "Map EDIFACT segments and messages onto Rust structs with #[derive(EdifactDeserialize, EdifactSerialize)] and address fields by UN/EDIFACT data element identifier."
weight = 50
+++

`edifact-rs` ships first-class derive macros that map EDIFACT segments and messages
to plain Rust structs. Add `#[derive(EdifactDeserialize, EdifactSerialize)]` and the
macros generate efficient, span-accurate code — no hand-written parsing loops.

---

## Quick example

```rust
use edifact_rs::{EdifactDeserialize, EdifactSerialize, from_bytes};

#[derive(Debug, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    document_name_code: String,
    #[edifact(element = 1)]
    document_number: String,
    #[edifact(element = 2)]
    function_code: Option<String>,
}

let segs: Vec<_> = from_bytes(b"BGM+220+PO-4711+9'").collect::<Result<_, _>>()?;
let bgm = Bgm::edifact_deserialize(&segs)?;

assert_eq!(bgm.document_name_code, "220");
assert_eq!(bgm.document_number, "PO-4711");
assert_eq!(bgm.function_code.as_deref(), Some("9"));
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Struct-level attributes (`#[edifact(...)]` on the struct)

### `segment = "TAG"` — declare a segment struct

```rust
# use edifact_rs::{EdifactDeserialize, EdifactSerialize};
#[derive(EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "DTM")]
struct Dtm {
    #[edifact(element = 0, component = 0)]
    qualifier: String,
    #[edifact(element = 0, component = 1)]
    value: String,
    #[edifact(element = 0, component = 2)]
    format: String,
}
```

When `segment = "TAG"` is present, the struct is treated as a **segment struct**:
- `EdifactDeserialize::edifact_deserialize` finds the first segment with tag `"TAG"`.
- `EdifactSerialize` emits a single segment.
- `EdifactSegmentTag::SEGMENT_TAG` is set to `"TAG"`.

If `segment` is **absent**, the struct is treated as a **message struct** (see below).

### `qualifier = "VALUE"` — fixed qualifier matching

```rust
# use edifact_rs::{EdifactDeserialize, EdifactSerialize};
#[derive(EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier = "MS")]
struct NadMs {
    #[edifact(element = 1)]
    party_id: Option<String>,
}
```

The generated `matches_segment` impl checks that element 0 equals `"MS"`.
Use wildcard suffix with `*` for prefix matching:

```text
#[edifact(segment = "NAD", qualifier = "M*")]   // matches "MS", "MR", "MT", …
```

### `qualifier_from = N` — dynamic qualifier at runtime

```rust
# use edifact_rs::{EdifactDeserialize, EdifactSerialize};
#[derive(EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier_from = 0)]
struct Nad {
    #[edifact(element = 0)]
    qualifier: String,       // ← this field holds the runtime qualifier
    #[edifact(element = 1)]
    party_id: Option<String>,
}
```

`qualifier_from = 0` means "the qualifier comes from element 0 at runtime" — the
struct can represent any `NAD` qualifier. This is the pattern for message-level
structs that hold multiple qualifier variants in separate fields:

```rust
# use edifact_rs::{EdifactDeserialize, EdifactSerialize};
# #[derive(Debug, EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] code: String }
# #[derive(Debug, EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "NAD")]
# struct Nad { #[edifact(element = 0)] qualifier: String }
#[derive(EdifactDeserialize)]
struct OrderMessage {
    bgm: Option<Bgm>,
    #[edifact(qualifier = "BY")]
    buyer: Option<Nad>,
    #[edifact(qualifier = "SU")]
    supplier: Option<Nad>,
}
```

### `layout = PATH` — resolve fields by UN/EDIFACT data element identifier

Points the struct at a `SegmentDefinition`, which unlocks the code-addressed
form of `element` described [below](#element-by-identifier).
The path must name a **`const`** item (a `const` initialiser cannot read a
`static`), because identifiers are resolved during const evaluation:

```rust
use edifact_rs::{
    ComponentRef, EdifactDeserialize, EdifactSerialize, ElementRef, SegmentDefinition, Status,
};

const C082: &[ComponentRef] = &[
    ComponentRef::new(1, "3039", Status::Mandatory),
    ComponentRef::new(2, "1131", Status::Conditional),
    ComponentRef::new(3, "3055", Status::Conditional),
];
const NAD_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "3035", Status::Mandatory, 1),
    ElementRef::composite(2, "C082", Status::Conditional, 1, C082),
];
pub const NAD: SegmentDefinition =
    SegmentDefinition::new("NAD", "Name and address", NAD_ELEMENTS);

#[derive(EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier = "MS", layout = NAD)]
struct SenderParty {
    #[edifact(element = "3039")]
    party_id: String,
    #[edifact(element = "3055")]
    agency: Option<String>,
}
```

`layout` requires `segment`, since a layout describes exactly one segment.

---

## Field-level attributes (`#[edifact(...)]` on a field)

### `element = N` — positional element index (0-based)

```text
#[edifact(element = 2)]
function_code: Option<String>,
```

Maps the field to **component 0 of element N**. This is the most common pattern —
use it for every simple (non-composite) field.

### `element = N, component = C` — composite component

```text
#[edifact(element = 0, component = 1)]
date_value: String,
```

Maps the field to a specific component within an element.  Use this when an element
carries multiple values (e.g. `DTM` element 0 has qualifier/value/format at
components 0/1/2).

### `element = "3055"` — data element identifier {#element-by-identifier}

Under a struct-level `layout`, `element` also accepts a UN/EDIFACT data element
identifier instead of an index:

```text
#[edifact(element = "3055")]
agency: Option<String>,
```

The identifier is resolved against the layout **at compile time**, and it
resolves *both* coordinates: a code naming a component inside a composite (like
`3055` in `C082`) yields that element *and* component position, so no index is
written by hand anywhere.

This is the point of the attribute. A transposed positional index reads the
wrong data element and still validates clean — the most dangerous failure mode
in this domain. An identifier cannot fail that way:

| Mistake | Positional form | Code form |
|---|---|---|
| Identifier not in this segment | reads a neighbouring element | **compile error** |
| Identifier defined at two positions | — | **compile error** |
| Two fields claiming one slot | compile error | **compile error** |
| Wrong segment's definition | — | compile error (or `E035` at runtime) |

Identifier resolution also picks the right diagnostic for `#[edifact(required)]`:
an identifier naming a whole data element reports `MissingRequiredElement`
(`E008`), one naming a component inside a composite reports
`MissingRequiredComponent` (`E021`) — a distinction a bare component index
cannot make, because the first component of a composite also sits at index 0.

`component = N` may still accompany a code, but only one that names a whole data
element — combining it with a code that already names a component is a compile
error.

For runtime lookups against the same metadata, see
[`Segment::value_by_code`](@/docs/validation.md#code-addressed-element-access).

### `composite` — full composite element

```rust
# use edifact_rs::{
#     EdifactCompositeDeserialize, EdifactCompositeSerialize, EdifactDeserialize, EdifactSerialize,
# };
#[derive(EdifactCompositeDeserialize, EdifactCompositeSerialize)]
struct PartyId {
    id: String,
    code_list: Option<String>,
    agency: Option<String>,
}

#[derive(EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD")]
struct Nad {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 1, composite)]
    party: Option<PartyId>,
}
```

`composite` hands the entire `CompositeElement` to the field's
`EdifactCompositeDeserialize` impl instead of extracting a single string.

Field *n* of the composite struct maps to component *n*, so `NAD+BY+4711::9`
fills `id = "4711"`, `code_list = None`, `agency = Some("9")`. Add
`#[edifact(component = N)]` to a field to pin it to a specific component
regardless of declaration order. A bare `String` component is mandatory —
an absent or empty value raises `MissingRequiredComponent` (`E021`) — while
`Option<String>` is not.

### `group` — repeated segment group

```rust
# use edifact_rs::{EdifactDeserialize, EdifactSerialize};
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "BGM")]
# #[derive(Debug)] struct Bgm { #[edifact(element = 0)] code: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "DTM")]
# #[derive(Debug)] struct Dtm { #[edifact(element = 0)] value: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "NAD")]
# #[derive(Debug)] struct Nad { #[edifact(element = 0)] qualifier: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "LIN")]
# #[derive(Debug)] struct Lin { #[edifact(element = 0)] line_no: String }
#[derive(EdifactDeserialize)]
struct OrderMessage {
    bgm: Option<Bgm>,
    #[edifact(group)]
    lines: Vec<Lin>,     // every LIN segment in the message
}
```

`group` on a `Vec<T>` field collects all matching segments where
`T::matches_segment` returns `true`.  `T` must implement `EdifactSegmentTag`.

### `qualifier = "VALUE"` — message-field qualifier filter

```rust
# use edifact_rs::{EdifactDeserialize, EdifactSerialize};
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "BGM")]
# #[derive(Debug)] struct Bgm { #[edifact(element = 0)] code: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "DTM")]
# #[derive(Debug)] struct Dtm { #[edifact(element = 0)] value: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "NAD")]
# #[derive(Debug)] struct Nad { #[edifact(element = 0)] qualifier: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "LIN")]
# #[derive(Debug)] struct Lin { #[edifact(element = 0)] line_no: String }
#[derive(EdifactDeserialize)]
struct OrderMessage {
    #[edifact(qualifier = "BY")]
    buyer: Option<Nad>,
    #[edifact(qualifier = "SU")]
    supplier: Option<Nad>,
}
```

Within a message struct, `qualifier` on a field restricts which `Nad` segment
(identified by element 0) is mapped to that field.

---

## Message structs

A struct **without** `#[edifact(segment = "TAG")]` is a **message struct**.
Each field maps to a segment type:

```rust
# use edifact_rs::{EdifactDeserialize, EdifactSerialize};
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "BGM")]
# #[derive(Debug)] struct Bgm { #[edifact(element = 0)] code: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "DTM")]
# #[derive(Debug)] struct Dtm { #[edifact(element = 0)] value: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "NAD")]
# #[derive(Debug)] struct Nad { #[edifact(element = 0)] qualifier: String }
# #[derive(EdifactDeserialize, EdifactSerialize)]
# #[edifact(segment = "LIN")]
# #[derive(Debug)] struct Lin { #[edifact(element = 0)] line_no: String }
# use edifact_rs::from_bytes;
# let input = b"BGM+220'DTM+137'NAD+BY'NAD+SU'LIN+1'";
#[derive(Debug, EdifactDeserialize)]
struct OrderMessage {
    bgm: Option<Bgm>,                // finds first BGM segment
    dtm: Option<Dtm>,                // finds first DTM segment
    #[edifact(qualifier = "BY")]
    buyer: Option<Nad>,              // finds NAD where element 0 == "BY"
    #[edifact(qualifier = "SU")]
    supplier: Option<Nad>,           // finds NAD where element 0 == "SU"
    #[edifact(group)]
    lines: Vec<Lin>,                 // collects all LIN segments
}

let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;
let msg = OrderMessage::edifact_deserialize(&segs)?;
# Ok::<(), edifact_rs::EdifactError>(())
```

Field rules:
- `Option<T>` — field is optional; returns `None` if the segment is not found.
- `T` (non-optional) — field is required; returns `EdifactError::MissingSegment` if absent.
- `Vec<T>` with `#[edifact(group)]` — zero or more matches.

---

## Attribute summary table

| Attribute | Target | Description |
|---|---|---|
| `segment = "TAG"` | struct | Declares a segment struct with the given 3-letter tag |
| `qualifier = "Q"` | struct | Filter — segment must have element 0 == `"Q"` (supports `*` suffix wildcard) |
| `qualifier_from = N` | struct | Element N holds the runtime qualifier value |
| `layout = PATH` | struct | `const SegmentDefinition` that code-addressed `element` attributes resolve against |
| `element = N` | field | Positional element index (0-based) |
| `element = "DE"` | field | UN/EDIFACT data element identifier, resolved against `layout` at compile time |
| `component = C` | field | Component index within the element (use with `element`) |
| `composite` | field | Map the whole element to `EdifactCompositeDeserialize` |
| `group` | field | Collect all matching segments into a `Vec<T>` |
| `qualifier = "Q"` | field (message struct) | Filter which qualifier variant to bind to this field |

---

## Generated trait implementations

For a **segment struct** `#[edifact(segment = "TAG")]` the macro generates:

| Trait | Method | Notes |
|---|---|---|
| `EdifactDeserialize` | `edifact_deserialize(&[Segment<'_>])` | Finds first matching segment |
| `EdifactDeserialize` | `edifact_deserialize_owned(&[OwnedSegment])` | Zero-alloc override for reader paths |
| `EdifactSerialize` | `edifact_serialize(&mut E)` | Emits one `StartSegment` .. `EndSegment` event sequence |
| `EdifactSegmentTag` | `SEGMENT_TAG` | The `"TAG"` string as a const |
| `EdifactSegmentTag` | `QUALIFIER_PATTERN` | `Some("Q")` or `None` |
| `EdifactSegmentTag` | `matches_segment(seg)` | Checks tag and qualifier |
| `EdifactSegmentTag` | `matches_owned_segment(seg)` | Same for `OwnedSegment` |

---

## Limitations

- **No generic type parameters**: structs with `<T>` type params are rejected at
  compile time.
- **No lifetime parameters**: use `String` (owned) instead of `&str` (borrowed)
  in derive-annotated structs.
- **Named fields only**: tuple structs and unit structs are not supported.
- **Single segment per struct**: one derive struct maps to one EDIFACT segment type.
  Nested message structs handle multi-segment composition.
- **`layout` must be a `const`**: a `const` initialiser cannot read a `static`, and
  identifier resolution happens in const evaluation. Declare directory tables as
  `const SegmentDefinition` / `const &[ElementRef]`.

These limitations are documented on the derive macro items and surfaced as
compile-time errors with span-accurate messages.

---

## Troubleshooting

### "expected `#[edifact(segment = …)]` or named-field struct"

You applied the derive to a struct without the `segment` attribute and without any
named fields, or to a tuple/unit struct. Add `#[edifact(segment = "TAG")]` or switch
to a named-field struct.

### "data element {code} is not defined exactly once in the segment layout"

The identifier in `#[edifact(element = "…")]` either does not appear in the
`layout`, or appears at more than one position. Check it against the directory
definition for that segment; this is the compile-time counterpart of the
runtime `E033` / `E034` errors.

### "qualifier conflicts with qualifier_from"

You used both `qualifier = "…"` and `qualifier_from = N` on the same struct. Use
only one — `qualifier` for a fixed compile-time filter, `qualifier_from` for a
runtime value stored in a field.

### "`EdifactSegmentTag` is not implemented for `MyStruct`"

The `Vec<T>` blanket impl of `EdifactDeserialize` requires `T: EdifactSegmentTag`.
This trait is auto-generated for segment structs (those with `#[edifact(segment = "TAG")]`).
Message structs (no `segment` attribute) do not get `EdifactSegmentTag`.

### Field type must be `String`, `Option<String>`, or a type implementing `FromStr`

The derive macro calls `FromStr::from_str` for non-String field types. Ensure the
type implements `std::str::FromStr` and that its error type implements
`std::fmt::Display`.

---

## Next steps

- [Streaming](@/docs/streaming.md) — use `deserialize_messages_from_reader` for reader-based typed extraction
- [Writing](@/docs/writing.md) — `EdifactSerialize` and the event model
- [Validation](@/docs/validation.md) — validate typed messages against business rules
