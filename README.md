# edifact-rs ⚡

[![crates.io](https://img.shields.io/crates/v/edifact-rs.svg)](https://crates.io/crates/edifact-rs)
[![docs.rs](https://docs.rs/edifact-rs/badge.svg)](https://docs.rs/edifact-rs)
[![CI](https://github.com/hupe1980/edifact-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/edifact-rs/actions)
[![license](https://img.shields.io/crates/l/edifact-rs.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](Cargo.toml)

**EDIFACT for Rust** — zero-copy parsing, streaming deserialization, typed derive macros, composable validation, and rich diagnostics.

---

## ✨ Why edifact-rs?

| | edifact-rs |
|---|---|
| 🚀 **Zero-copy parsing** | Borrows directly from the input `&[u8]` — no intermediate allocations |
| 🔄 **Streaming I/O** | Reader-based APIs process gigabyte interchanges in constant memory |
| 🎯 **Typed mapping** | `#[derive(EdifactDeserialize, EdifactSerialize)]` for segments and messages |
| 🔎 **Named data elements** | Address fields by UN/EDIFACT identifier (`"3055"`), not by hand-counted index — checked at compile time |
| ✅ **Composable validation** | `ProfileRulePack` with multi-layer, rule-ID-filtered reporting |
| 🩺 **Rich diagnostics** | Optional `miette` integration for human-friendly error output |
| 🛡️ **DOS hardening** | Configurable `max_segment_bytes` guard enforced on all read paths |
| 🏎️ **Allocation-free hot paths** | `SmallVec`, eager `WriterEmitter`, and `edifact_deserialize_owned` |

---

## 📦 Installation

```toml
[dependencies]
edifact-rs = "0.12"

# Optional: derive macros (included by default)
# edifact-rs = { version = "0.12", features = ["derive"] }

# Optional: rich miette diagnostics
# edifact-rs = { version = "0.12", features = ["diagnostics"] }
```

### Feature flags

| Feature | Default | Description |
|---|---|---|
| `derive` | ✅ yes | Re-exports `EdifactDeserialize` / `EdifactSerialize` derive macros |
| `diagnostics` | ❌ no | Adds `miette::Diagnostic` to `EdifactError` for human-readable output |
| `serde` | ❌ no | Derives `Serialize` / `Deserialize` for `ValidationReport`, `ValidationIssue`, and the envelope types |

---

## 🚀 Quick start

### Parse bytes (zero-copy)

```rust
use edifact_rs::from_bytes;

let input = b"UNA:+.? 'UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'UNT+3+1'";
let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

assert_eq!(segments[0].tag, "UNH");
let bgm = &segments[1];
assert_eq!(bgm.tag, "BGM");
assert_eq!(bgm.element_str(0), Some("220"));   // document code
assert_eq!(bgm.element_str(1), Some("PO-4711")); // document number
# Ok::<(), edifact_rs::EdifactError>(())
```

### Typed deserialization with derive

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

let input = b"BGM+220+PO-4711+9'";
let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;
let bgm = Bgm::edifact_deserialize(&segments)?;

assert_eq!(bgm.document_name_code, "220");
assert_eq!(bgm.document_number, "PO-4711");
assert_eq!(bgm.function_code.as_deref(), Some("9"));
# Ok::<(), edifact_rs::EdifactError>(())
```

### Map a full message with qualifier-based fields

```rust
use edifact_rs::{EdifactDeserialize, EdifactSerialize, from_bytes};

#[derive(Debug, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier_from = 0)]
struct Nad {
    #[edifact(element = 0)]
    qualifier: String,
    #[edifact(element = 1)]
    party_id: Option<String>,
}

#[derive(Debug, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "BGM")]
struct Bgm {
    #[edifact(element = 0)]
    doc_code: String,
    #[edifact(element = 1)]
    doc_number: String,
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

println!("buyer:    {:?}", msg.buyer.as_ref().and_then(|n| n.party_id.as_deref()));
println!("supplier: {:?}", msg.supplier.as_ref().and_then(|n| n.party_id.as_deref()));
# Ok::<(), edifact_rs::EdifactError>(())
```

### Serialize to wire format

```rust
use edifact_rs::to_edifact_string;

# use edifact_rs::EdifactSerialize;
# #[derive(EdifactSerialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] doc_code: String }
let bgm = Bgm { doc_code: "220".into() };
let wire = to_edifact_string(&bgm)?;
assert_eq!(wire, "BGM+220'");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## 📡 Streaming APIs

### Low-memory typed extraction

Scan a large interchange and extract matching segments without buffering everything:

```rust
use edifact_rs::{EdifactDeserialize, deserialize_first_from_reader, deserialize_all_from_reader};

# #[derive(Debug, EdifactDeserialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] doc_code: String, #[edifact(element = 1)] doc_id: String }
let input = std::io::Cursor::new(
    b"UNH+1+ORDERS:D:11A:UN'BGM+220+PO-001+9'BGM+231+PO-002+9'UNT+4+1'".to_vec()
);

// Stop after the first match — O(1) memory:
let first: Bgm = deserialize_first_from_reader(input.clone())?;
assert_eq!(first.doc_id, "PO-001");

// Collect all matches — only matching segments are kept:
let all: Vec<Bgm> = deserialize_all_from_reader(input)?;
assert_eq!(all.len(), 2);
# Ok::<(), edifact_rs::EdifactError>(())
```

### Message-window streaming (UNH..UNT)

Process multi-message interchanges one window at a time, with O(1) memory per message:

```rust
use edifact_rs::{message_windows_from_reader, deserialize_messages_from_reader, EdifactDeserialize};

# #[derive(Debug, EdifactDeserialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] doc_code: String }
# #[derive(Debug, EdifactDeserialize)]
# struct OrderMessage { bgm: Option<Bgm> }
let interchange = std::io::Cursor::new(b"\
    UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'\
    UNH+1+ORDERS:D:96A:UN'BGM+220+PO-001+9'UNT+3+1'\
    UNH+2+ORDERS:D:96A:UN'BGM+220+PO-002+9'UNT+3+2'\
    UNZ+2+1'".to_vec());

// Iterate raw windows — each window carries type info and the segment slice:
for window in message_windows_from_reader(interchange.clone()) {
    let window = window?;
    println!("type={:?} segments={}",
        window.message_type, window.segments.len());
}

// Or deserialize directly — zero Vec<Segment> allocation per window:
let messages: Vec<OrderMessage> =
    deserialize_messages_from_reader::<OrderMessage, _>(interchange)
        .collect::<Result<_, _>>()?;
assert_eq!(messages.len(), 2);
# Ok::<(), edifact_rs::EdifactError>(())
```

> **Performance note**: `deserialize_messages_from_reader` calls
> `edifact_deserialize_owned`, a method generated by the derive macro that
> works directly on `&[OwnedSegment]` — no intermediate `Vec<Segment<'_>>`
> is ever materialized.

---

## ✅ Validation

### Profile rule packs

Compose business-level validation rules with stable rule IDs that can be filtered and reported independently:

```rust
use edifact_rs::{
    ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity, from_bytes,
};

let segments: Vec<_> =
    from_bytes(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'")
        .collect::<Result<_, _>>()?;

let document_pack = ProfileRulePack::new("ORDERS-DOC")
    .for_message_type("ORDERS")
    .with_stateless_rule_fn(|segments, issues| {
        if let Some(bgm) = segments.iter().find(|s| s.tag == "BGM") {
            if let Some(code) = bgm.get_element(0).and_then(|e| e.get_component(0)) {
                if code == "220" {
                    issues.push(
                        ValidationIssue::new(ValidationSeverity::Warning, "code 220 requires special handling")
                            .with_rule_id("ORDERS-DOC-P001")
                            .with_segment("BGM")
                            .with_element_index(0)
                            .with_suggestion("Check your trading-partner agreement")
                    );
                }
            }
        }
    });

let report = ValidationContext::builder()
    .with_profile_pack(document_pack)
    .build()
    .validate_lenient(&segments);

// Filter by rule namespace:
let doc_issues = report.filter_by_rule_prefix("ORDERS-DOC-");
println!("{} issue(s) from ORDERS-DOC rules", doc_issues.total_issues());
# Ok::<(), edifact_rs::EdifactError>(())
```

### Multi-layer validation

Separate structure, code-list, and profile checks into distinct layers:

```rust
use edifact_rs::{Validator, ValidationContext, ValidationLayer, ValidationReport, ValidationRuleContext, Segment};

struct StructureValidator;
impl Validator for StructureValidator {
    fn validate_batch(&self, _segments: &[Segment<'_>], _report: &mut ValidationReport, _context: &ValidationRuleContext<'_>) {
        // check mandatory segments, ordering, ...
    }
}

let context = ValidationContext::builder()
    .with_message_type("ORDERS")
    .with_validator(ValidationLayer::Structure, StructureValidator)
    .build();
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## 🩺 Diagnostics (optional feature)

Enable the `diagnostics` feature for human-readable, span-annotated error output powered by [`miette`](https://docs.rs/miette):

```toml
edifact-rs = { version = "0.12", features = ["diagnostics"] }
```

```text
Error: invalid code value "999" at offset 42
  ╭─ input.edi:2:5
  │
2 │ BGM+999+PO-4711+9'
  │     ^^^  code "999" is not in code list 1001
  │
Error Code: E007
Help: Use a valid document name code from UNTDID 1001
```

```rust,no_run
use edifact_rs::{from_bytes, ValidationContext};
// With `diagnostics` feature, errors implement miette::Diagnostic.
// Use miette's Report for pretty-printing to the terminal.
```

See [`cookbook_diagnostics.rs`](crates/edifact-rs/examples/cookbook_diagnostics.rs) for a complete example.

---

## 🏗️ Architecture

```text
edifact-rs workspace
│
├── edifact-rs              ← core library
│   ├── tokenizer           zero-copy byte scanning, UNA handling
│   ├── parser              segment assembly, release-char resolution
│   ├── model               Segment / Element / OwnedSegment types
│   ├── writer              streaming wire-format serialization
│   ├── event               EdifactEvent / WriterEmitter (allocation-free)
│   ├── de                  EdifactDeserialize trait + free helpers
│   ├── ser                 EdifactSerialize trait
│   ├── envelope            UNB/UNH/UNT/UNZ validation
│   ├── validator           Validator / ValidationContext / ProfileRulePack
│   └── directory_validator SegmentDefinition / DirectoryValidator
│
└── edifact-rs-derive       proc-macro crate
    └── #[derive(EdifactDeserialize, EdifactSerialize)]
```

**Two parsing modes:**

| Mode | API | Allocation model |
|---|---|---|
| Zero-copy | `from_bytes(input: &[u8])` | Borrows from `input` — no heap for segment data |
| Owned streaming | `from_reader(reader)` | One `OwnedSegment` per segment; reader not buffered |

**Key types:**

| Type | Description |
|---|---|
| `Segment<'a>` | Zero-copy view with `tag: &'a str` and borrowed elements |
| `OwnedSegment` | Heap-owned copy; `.borrow()` returns O(1) `BorrowedSegment` |
| `BorrowedSegment<'a>` | Zero-allocation view of `OwnedSegment` |
| `EdifactError` | Stable error codes (E001–E032) with byte offsets |
| `ValidationReport` | Collected issues with lenient/strict modes |
| `ProfileRulePack` | Composable, filterable business-rule bundles |
| `MessageWindow<'a>` | Zero-copy window: `message_type`, `association_code`, borrowed `segments` |
| `OwnedMessageWindow` | Owned window: heap `message_type`, `association_code`, owned `segments` |

---

## 🔧 Low-level API

### Segment and element access

```rust
use edifact_rs::{from_bytes, find_qualified_segment};

let input = b"NAD+BY+4000001000002::9'NAD+SU+4000001000001::9'";
let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

let buyer = find_qualified_segment(&segs, "NAD", "BY").unwrap();
assert_eq!(buyer.element_str(0), Some("BY"));
assert_eq!(buyer.get_element(1).and_then(|e| e.get_component(0)), Some("4000001000002"));
# Ok::<(), edifact_rs::EdifactError>(())
```

### Access by data element identifier

Positional indices are a silent-misread hazard: transpose one and you read the
wrong data element, and it still validates clean. Given a segment definition,
address the value by its UN/EDIFACT identifier instead — a wrong reference is
then a lookup error against the directory:

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
assert!(segs[0].value_by_code(&NAD, "2380").is_err()); // DE 2380 is a DTM element
# Ok::<(), edifact_rs::EdifactError>(())
```

The derive does the same resolution at **compile time** — see
[Typed Derive](docs/typed-derive.md#element--3055--data-element-identifier):

```rust,ignore
#[derive(EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier = "BY", layout = NAD)]
struct Buyer {
    #[edifact(element = "3039")]
    gln: String,
    #[edifact(element = "3055")]
    agency: Option<String>,
}
```

### Reader with DOS guard

```rust
use edifact_rs::{ReaderConfig, from_bufread_stream_with_config};
use std::io::BufReader;

let config = ReaderConfig {
    max_segment_bytes: 8_192,
    ..Default::default()
};
let reader = BufReader::new(std::io::Cursor::new(b"BGM+220+test'"));
// The stream yields `Result<OwnedSegment, _>`; a segment longer than
// `max_segment_bytes` ends it with `EdifactError::SegmentTooLong`.
let segments: Vec<_> =
    from_bufread_stream_with_config(reader, config).collect::<Result<_, _>>()?;
assert_eq!(segments.len(), 1);
# Ok::<(), edifact_rs::EdifactError>(())
```

### Write segments

```rust
use edifact_rs::{Writer, Segment, Element};

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);
writer.write_segment(&Segment::new(
    "BGM",
    vec![Element::of(&["220"]), Element::of(&["PO-4711"])],
))?;
writer.finish()?;
assert_eq!(buf, b"BGM+220+PO-4711'");
# Ok::<(), edifact_rs::EdifactError>(())
```

Real segments mix simple and composite data elements; `write_elements` (and its
`elements!` shorthand) writes that shape in one call, with component boundaries
explicit so a literal `:` in a value is escaped rather than promoted to a
boundary:

```rust
use edifact_rs::{Writer, elements};

let mut buf: Vec<u8> = Vec::new();
let mut writer = Writer::new(&mut buf);
writer.write_elements("NAD", elements!["MS", ["9900112233445", "", "293"]])?;
writer.write_elements("DTM", elements![["137", "20260101", "102"]])?;
writer.finish()?;
assert_eq!(buf, b"NAD+MS+9900112233445::293'DTM+137:20260101:102'");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## 🌐 Async / tokio integration

`edifact-rs` is intentionally synchronous — EDIFACT parsing is CPU-bound and imposing an async runtime on all users would be wrong. Two clean patterns bridge to async:

**Pattern A** — read into memory, then parse synchronously (recommended for < 1 MB):

```rust,ignore
let bytes = tokio::fs::read("message.edi").await?;
let windows: Vec<_> = edifact_rs::from_bytes_windows(&bytes)
    .collect::<Result<_, _>>()?;
```

**Pattern B** — `spawn_blocking` for large files or blocking sources:

```rust,ignore
let messages = tokio::task::spawn_blocking(move || {
    let f = std::fs::File::open("large.edi")?;
    edifact_rs::message_windows_from_reader(f)
        .collect::<Result<Vec<_>, _>>()
}).await??;
```

---

## 📖 Documentation

**[CHANGELOG.md](CHANGELOG.md)** records every release: new public APIs, new
`ValidationIssue` fields, and any wire or derive behaviour change — so you can
adopt a release deliberately instead of reading the git log.

| Guide | Covers |
|---|---|
| [Getting Started](docs/getting-started.md) | Install, parse, validate, write — end to end |
| [Core Concepts](docs/core-concepts.md) | Segments, elements, spans, borrowed vs owned |
| [Parsing](docs/parsing.md) | Every parse entry point and when to use it |
| [Typed Derive](docs/typed-derive.md) | `#[derive(EdifactDeserialize, EdifactSerialize)]`, `layout`, identifier-based fields |
| [Writing](docs/writing.md) | `Writer`, `write_elements`, custom UNA |
| [Validation](docs/validation.md) | Layers, `ValidationIssue`, code-addressed access |
| [Profile Packs](docs/profile-packs.md) | Composable business-rule bundles |
| [Streaming](docs/streaming.md) | Constant-memory processing of large interchanges |
| [Diagnostics](docs/diagnostics.md) | `miette` integration |
| [Error Reference](docs/error-reference.md) | Every stable code `E001`–`E035` |
| [Performance](docs/performance.md) | Benchmarks and allocation behaviour |
| [Async Integration](docs/async-integration.md) | Bridging to `tokio` |

---

## 📚 Examples

Run any example with `cargo run -p edifact-rs --example <name>`:

| Example | What it shows |
|---|---|
| [`cookbook_parse_map_validate_write`](crates/edifact-rs/examples/cookbook_parse_map_validate_write.rs) | Parse → extract fields → validate → round-trip write |
| [`cookbook_typed_derive`](crates/edifact-rs/examples/cookbook_typed_derive.rs) | Full derive workflow with qualifier-based NAD mapping |
| [`cookbook_typed_streaming`](crates/edifact-rs/examples/cookbook_typed_streaming.rs) | All four streaming extraction APIs |
| [`cookbook_profile_packs`](crates/edifact-rs/examples/cookbook_profile_packs.rs) | Composing and filtering profile rule packs |
| [`cookbook_streamed_progressive_validation`](crates/edifact-rs/examples/cookbook_streamed_progressive_validation.rs) | Per-window validation over reader-based interchange |
| [`cookbook_fixture_validation`](crates/edifact-rs/examples/cookbook_fixture_validation.rs) | Custom `Validator` implementation with fixture data |
| [`cookbook_diagnostics`](crates/edifact-rs/examples/cookbook_diagnostics.rs) | Rich miette diagnostics (`--features diagnostics`) |

---

## 🧪 Testing

Recipes live in the [`justfile`](justfile) and mirror
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) job for job, so a green
`just ci` means a green CI. Install with `cargo install just` (or
`brew install just`), then:

```bash
just                # list every recipe
just pre-commit     # fmt + clippy + tests — run before every commit
just ci             # everything CI runs, on this toolchain
just ci-full        # `just ci` plus the MSRV job and the benchmarks
```

| Recipe | What it covers |
|---|---|
| `just test` | Unit, integration, doc-tests, and the derive UI expectations |
| `just clippy` | Zero-warnings lint gate |
| `just doc` | Public docs with warnings denied |
| `just ui` / `just ui-msrv` | Derive compile-fail suite on stable / MSRV |
| `just ui-bless` | Re-bless `.stderr` after an intentional diagnostic change |
| `just msrv` | Full suite on the MSRV toolchain (`just msrv-install` first) |
| `just deny` | Advisory, licence, and dependency-ban audit |
| `just fuzz` | `bolero` property targets, release build |
| `just bench` / `just bench-smoke` | Divan microbenchmarks / criterion smoke run |
| `just release-check` | Package dry-run and cross-crate version match |

Every recipe is a thin wrapper over `cargo`, so the underlying command is always
visible in the justfile if you would rather run it directly.

---

## 📋 Workspace layout

```text
edifact-rs/
├── crates/
│   ├── edifact-rs/          core library crate
│   │   ├── src/
│   │   ├── examples/        runnable cookbooks
│   │   ├── tests/           integration + conformance tests
│   │   └── benches/         criterion benchmarks
│   └── edifact-rs-derive/   proc-macro crate
│       ├── src/
│       └── tests/ui/        trybuild compile-fail test suite
├── docs/                    guides (compiled as doctests)
└── justfile                 task runner, mirroring CI
```

---

## ⚙️ MSRV and edition

- **Minimum Supported Rust Version**: 1.85
- **Edition**: 2024
- MSRV is enforced by CI on every push.

---

## 📄 License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

