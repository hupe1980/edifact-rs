# edifact-rs ⚡

[![crates.io](https://img.shields.io/crates/v/edifact-rs.svg)](https://crates.io/crates/edifact-rs)
[![docs.rs](https://docs.rs/edifact-rs/badge.svg)](https://docs.rs/edifact-rs)
[![CI](https://github.com/hupe1980/edifact-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/edifact-rs/actions)
[![license](https://img.shields.io/crates/l/edifact-rs.svg)](#license)
[![MSRV](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](#msrv-and-edition)

**EDIFACT (ISO 9735) for Rust** — zero-copy parsing, streaming deserialization,
typed derive macros, and composable validation.

📖 **[Guides](https://hupe1980.github.io/edifact-rs/docs/)** ·
🦀 **[API reference](https://docs.rs/edifact-rs)** ·
📦 **[crates.io](https://crates.io/crates/edifact-rs)**

---

## Install

```bash
cargo add edifact-rs
```

`derive` is on by default. Optional features:

```bash
cargo add edifact-rs --features diagnostics   # miette-powered error rendering
cargo add edifact-rs --features serde         # Serialize/Deserialize on reports
cargo add edifact-rs --no-default-features    # core parse + write only
```

## Quick start

Parsing borrows straight from the input — segment tags and component values are
`&str` slices into your buffer:

```rust
use edifact_rs::from_bytes;

let input = b"UNA:+.? 'UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'UNT+3+1'";
let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

let bgm = &segments[1];
assert_eq!(bgm.tag, "BGM");
assert_eq!(bgm.element_str(0), Some("220"));      // document code
assert_eq!(bgm.element_str(1), Some("PO-4711"));  // document number
# Ok::<(), edifact_rs::EdifactError>(())
```

Map segments and whole messages onto structs, dispatching repeated segments by
their qualifier:

```rust
use edifact_rs::{EdifactDeserialize, EdifactSerialize, from_bytes};

#[derive(Debug, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    doc_code: String,
    #[edifact(element = 1)]
    doc_number: String,
}

#[derive(Debug, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier_from = 0)]
struct Nad {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 1)]
    party_id: Option<String>,
}

#[derive(Debug, EdifactDeserialize)]
struct OrderMessage {
    bgm: Option<Bgm>,
    #[edifact(qualifier = "BY")]
    buyer: Option<Nad>,
    #[edifact(qualifier = "SU")]
    supplier: Option<Nad>,
}

let input = b"UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'\
              NAD+BY+4000001000002::9'NAD+SU+4000001000001::9'UNT+5+1'";
let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;
let msg = OrderMessage::edifact_deserialize(&segments)?;

assert_eq!(msg.buyer.unwrap().party_id.as_deref(), Some("4000001000002"));
# Ok::<(), edifact_rs::EdifactError>(())
```

Write it back out with delimiters escaped for you:

```rust
use edifact_rs::to_edifact_string;
# use edifact_rs::EdifactSerialize;
# #[derive(EdifactSerialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] doc_code: String }

let wire = to_edifact_string(&Bgm { doc_code: "220".into() })?;
assert_eq!(wire, "BGM+220'");
# Ok::<(), edifact_rs::EdifactError>(())
```

## What makes it different

EDIFACT is deceptively simple — flat text, a handful of delimiters — which is
why hand-rolled parsers are common and quietly wrong. The delimiters are
redefinable per interchange, any of them may appear *inside* a value when
release-escaped, and syntax version 4 adds a repetition separator that changes
an element's shape rather than its text. `edifact-rs` takes a position on each
of the places that usually goes wrong:

| | |
|---|---|
| **Zero-copy by default** | Tags and values borrow from the input slice. The only per-segment allocation is the element vector; an owned string appears solely where a release escape had to be resolved. |
| **Constant-memory streaming** | Reader iterators yield one segment at a time; message windows group them into `UNH`…`UNT` units. A multi-gigabyte interchange costs one message of peak memory. |
| **Identifiers, not indices** | Address a field by its UN/EDIFACT data element identifier. The derive resolves it during const evaluation, so a stale identifier fails the build instead of reading the element next door. |
| **Repetitions survive the round trip** | The repetition separator splits an element into real occurrences (ISO 9735-1 §8.6) instead of leaving `1*ON` in the value as literal text — read from the `UNA`, or from the syntax version in `UNB` S001 when there is none. `#[edifact(repeat)]` maps those occurrences onto a `Vec<T>` and writes them back byte-for-byte. |
| **The envelope is checked against the standard, not a guess** | `UNB`/`UNG`/`UNH` structure, control-reference pairing, all three control counts, and the two rules no count can catch: an interchange with no content (§7.1) and a message with no body (§7.3). |
| **`CONTRL` is generated, not hand-rolled** | The acknowledgement is built from the validation report, so what you send back cannot disagree with what you found. Each finding lands at the lowest reporting level that can both locate it and legally carry its code (ISO 9735-4 Annex A). |
| **Limits report, never truncate** | Segment, message, and byte budgets raise an error. A budget that quietly ended iteration is indistinguishable from clean end-of-input, so a caller would accept a truncated interchange as a whole one. |
| **Layered validation** | Envelope, structure, code-list, and profile checks write into one report carrying stable error codes, byte spans, and filterable rule identifiers. |
| **Representations are enforced, not just recorded** | `an..35`, `n8`, `a1` — the column every directory prints and partners actually reject on. Length is counted in characters, not bytes (§6), and a numeric value excludes its sign, decimal mark and exponent (§10), so `-123.45` fits `n..5`. Where the syntax versions disagree — `S004` DE 0017 is `n6` in v3 and `n8` in v4 — each is checked against its own version rather than collapsed into a range that fits neither. |
| **A layout can be checked against reality** | A hand-written definition that disagrees with the wire fails silently — it resolves to the wrong component and returns a plausible value. `SegmentLayout::audit` points it at a corpus and separates *disproved* from *unconfirmed*, so thin fixtures cannot masquerade as approval. |
| **Character sets are decoded, not assumed** | UTF-8 is not a superset of `UNOC`, so a conformant German interchange is unparseable as UTF-8. `decode_interchange` reads the repertoire from `UNB` S001 and transcodes — borrowing, not copying, when the payload is already ASCII. |
| **No `unsafe`** | `#![deny(unsafe_code)]`, with property and fuzz tests over parse, write, and validate on every commit. |

### Addressing a field by identifier

Transpose one positional index and you read the wrong data element — and it
still validates clean. Given a segment definition, address the value by its
identifier instead, and a wrong reference becomes a lookup error:

```rust
use edifact_rs::{ComponentRef, ElementRef, SegmentDefinition, Status, from_bytes};

static C082: &[ComponentRef] = &[
    ComponentRef::new(1, "3039", Status::Mandatory),
    ComponentRef::new(2, "1131", Status::Conditional),
    ComponentRef::new(3, "3055", Status::Conditional),
];
static NAD_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "3035", Status::Mandatory, 1),
    ElementRef::composite(2, "C082", Status::Conditional, 1, C082),
];
static NAD: SegmentDefinition = SegmentDefinition::new("NAD", "Name and address", NAD_ELEMENTS);

let segs: Vec<_> = from_bytes(b"NAD+BY+4000001000002::9'").collect::<Result<Vec<_>, _>>()?;

assert_eq!(segs[0].value_by_code(&NAD, "3039")?, Some("4000001000002"));
assert_eq!(segs[0].value_by_code(&NAD, "3055")?, Some("9"));
assert!(segs[0].value_by_code(&NAD, "2380").is_err()); // DE 2380 belongs to DTM
# Ok::<(), edifact_rs::EdifactError>(())
```

The derive performs the same resolution at compile time — see
[Typed Derive](https://hupe1980.github.io/edifact-rs/docs/typed-derive/#element-by-identifier).

## Documentation

Full guides live at **[hupe1980.github.io/edifact-rs](https://hupe1980.github.io/edifact-rs/docs/)**.
Every Rust snippet on the site is compiled and run as part of the test suite, so
none of it can drift from the crate.

| Guide | |
|---|---|
| [Getting Started](https://hupe1980.github.io/edifact-rs/docs/getting-started/) | Install, first parse, feature flags |
| [Core Concepts](https://hupe1980.github.io/edifact-rs/docs/core-concepts/) | Wire format, UNA, release characters, repetitions, Rust type mapping |
| [Character Sets](https://hupe1980.github.io/edifact-rs/docs/character-sets/) | `UNOA`–`UNOK`/`UNOY` decoding, encoding, and repertoire validation |
| [Parsing](https://hupe1980.github.io/edifact-rs/docs/parsing/) | Entry points, byte spans, and the `ReaderConfig` budgets |
| [Writing](https://hupe1980.github.io/edifact-rs/docs/writing/) | `Writer`, escaping, custom UNA, repeating elements |
| [Typed Derive](https://hupe1980.github.io/edifact-rs/docs/typed-derive/) | Every derive attribute, including identifier-addressed fields |
| [Streaming](https://hupe1980.github.io/edifact-rs/docs/streaming/) | Reader iterators, message windows, typed extraction |
| [Validation](https://hupe1980.github.io/edifact-rs/docs/validation/) | `Validator`, `ValidationContext`, the four layers |
| [CONTRL](https://hupe1980.github.io/edifact-rs/docs/contrl/) | Acknowledgements and rejections from a validation report |
| [Profile Packs](https://hupe1980.github.io/edifact-rs/docs/profile-packs/) | Authoring, composing, and filtering business rules |
| [Diagnostics](https://hupe1980.github.io/edifact-rs/docs/diagnostics/) | `miette` integration |
| [Async Integration](https://hupe1980.github.io/edifact-rs/docs/async-integration/) | Bridging to `tokio` |
| [Error Reference](https://hupe1980.github.io/edifact-rs/docs/error-reference/) | Every stable code `E001`–`E046` |
| [Performance](https://hupe1980.github.io/edifact-rs/docs/performance/) | Allocation budgets, benchmarks, tuning |

Runnable cookbooks live in
[`crates/edifact-rs/examples/`](crates/edifact-rs/examples) — try one with
`cargo run --example cookbook_parse_map_validate_write`.

## Scope

`edifact-rs` is the **engine**, not a directory distribution.

It ships the parser, writer, validation pipeline, the table types for segment
definitions, and every **ISO 9735-1 batch service segment** — `UNB`, `UNG`,
`UNH`, `UNT`, `UNE`, `UNZ`, `UNS`, `UNO`, `UNP`, `UGH`, `UGT` — as ready-to-use
layouts in `edifact_rs::service`. Those are fixed by the syntax standard rather
than by a directory release, so there is one correct answer and no version to
pick:

```rust
use edifact_rs::{from_bytes, service};

let segments: Vec<_> = from_bytes(b"UNB+UNOC:3+SENDER+RECEIVER+260101:0900+IC4711'UNZ+0+IC4711'")
    .collect::<Result<Vec<_>, _>>()?;

// DE 0020 by name, not by counting to element 4.
assert_eq!(segments[0].value_by_code(&service::UNB, "0020")?, Some("IC4711"));
# Ok::<(), edifact_rs::EdifactError>(())
```

It also ships **`CONTRL`** (ISO 9735-4) — the acknowledgement you owe a partner —
generated from a validation report rather than assembled by hand:

```rust
use edifact_rs::{Contrl, from_bytes, validate_envelope};

let raw = b"UNB+UNOC:3+SENDER+RECEIVER+260101:0900+IC4711'\
            UNH+MSG1+ORDERS:D:96A:UN'BGM+220+PO-1+9'UNT+3+MSG1'\
            UNZ+1+IC4711'";
let segments: Vec<_> = from_bytes(raw).collect::<Result<Vec<_>, _>>()?;
let validated = validate_envelope(&segments)?;

let wire = Contrl::acknowledgement(&validated)
    .to_interchange_string("UNOC", "3", "260101", "0930", "ACK-1")?;

// Its own interchange, addressed back the way it came.
assert!(wire.starts_with("UNB+UNOC:3+RECEIVER+SENDER+260101:0930+ACK-1'"));
assert!(wire.contains("UCI+IC4711+SENDER+RECEIVER+7'"));
# Ok::<(), edifact_rs::EdifactError>(())
```

It does **not** ship UN/EDIFACT *directory* data — `BGM`, `DTM`, `NAD`, `C507`
and the rest. That is a licensing boundary, not a workload one: the
[UN UNTDID licence](https://service.unece.org/trade/untdid/license.htm) permits
redistributing the Directory unmodified, with its copyright notice, to the
country where you acquired it — and states that you may not modify it and
distribute it. Transcribing it into `const` tables and publishing that on
crates.io is exactly what it forbids.

Supply the subset you touch as `static` tables at compile time, or load them at
startup with `DirectoryValidatorBuilder` — then point
[`SegmentLayout::audit`](https://hupe1980.github.io/edifact-rs/docs/validation/#auditing-a-hand-written-layout)
at your fixtures, because a hand-authored layout that disagrees with the wire
fails silently.

Nor does it cover the remaining parts of ISO 9735: interactive EDI (part 3), the
`AUTACK` and `KEYMAN` security messages (parts 6 and 9), or the security segments
(parts 5 and 7). Packages (§7.9) are recognised and reported rather than parsed
— the object inside a `UNO`…`UNP` pair is arbitrary binary data, not EDIFACT, so
the header tells you its length and the bytes are yours to lift out.

## Development

The [`justfile`](justfile) mirrors CI, so a green `just ci` means a green build:

```bash
just check        # fmt + clippy + tests — the pre-commit gate
just test         # workspace tests, all features
just site-serve   # preview the documentation site with live reload
just ci           # everything CI runs, on this toolchain
```

## MSRV and edition

Rust **1.85**, edition **2024**. The MSRV is enforced by CI on every push and a
raise is treated as a breaking change.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work shall be dual-licensed as above, without any
additional terms or conditions.
