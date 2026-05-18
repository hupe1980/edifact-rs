# Typed Derive 🎯

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
#[derive(EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier = "MS")]
struct NadMs {
    #[edifact(element = 1)]
    party_id: Option<String>,
}
```

The generated `matches_segment` impl checks that element 0 equals `"MS"`.
Use wildcard suffix with `*` for prefix matching:

```rust
#[edifact(segment = "NAD", qualifier = "M*")] // matches "MS", "MR", "MT", …
```

### `qualifier_from = N` — dynamic qualifier at runtime

```rust
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
#[derive(EdifactDeserialize)]
struct OrderMessage {
    bgm: Option<Bgm>,
    #[edifact(qualifier = "BY")]
    buyer: Option<Nad>,
    #[edifact(qualifier = "SU")]
    supplier: Option<Nad>,
}
```

---

## Field-level attributes (`#[edifact(...)]` on a field)

### `element = N` — positional element index (0-based)

```rust
#[edifact(element = 2)]
function_code: Option<String>,
```

Maps the field to **component 0 of element N**. This is the most common pattern —
use it for every simple (non-composite) field.

### `element = N, component = C` — composite component

```rust
#[edifact(element = 0, component = 1)]
date_value: String,
```

Maps the field to a specific component within an element.  Use this when an element
carries multiple values (e.g. `DTM` element 0 has qualifier/value/format at
components 0/1/2).

### `composite` — full composite element

```rust
#[derive(EdifactCompositeDeserialize, EdifactCompositeSerialize)]
struct PartyId {
    id: String,
    qualifier: Option<String>,
    code_list: Option<String>,
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

### `group` — repeated segment group

```rust
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
| `element = N` | field | Positional element index (0-based) |
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

These limitations are documented on the derive macro items and surfaced as
compile-time errors with span-accurate messages.

---

## Troubleshooting

### "expected `#[edifact(segment = …)]` or named-field struct"

You applied the derive to a struct without the `segment` attribute and without any
named fields, or to a tuple/unit struct. Add `#[edifact(segment = "TAG")]` or switch
to a named-field struct.

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

- [Streaming](streaming.md) — use `deserialize_messages_from_reader` for reader-based typed extraction
- [Writing](writing.md) — `EdifactSerialize` and the event model
- [Validation](validation.md) — validate typed messages against business rules
