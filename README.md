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
| ✅ **Composable validation** | `ProfileRulePack` with multi-layer, rule-ID-filtered reporting |
| 🩺 **Rich diagnostics** | Optional `miette` integration for human-friendly error output |
| 🛡️ **DOS hardening** | Configurable `max_segment_bytes` guard enforced on all read paths |
| 🏎️ **Allocation-free hot paths** | `SmallVec`, eager `WriterEmitter`, and `edifact_deserialize_owned` |

---

## 📦 Installation

```toml
[dependencies]
edifact-rs = "0.1"

# Optional: derive macros (included by default)
# edifact-rs = { version = "0.4", features = ["derive"] }

# Optional: rich miette diagnostics
# edifact-rs = { version = "0.4", features = ["diagnostics"] }
```

### Feature flags

| Feature | Default | Description |
|---|---|---|
| `derive` | ✅ yes | Re-exports `EdifactDeserialize` / `EdifactSerialize` derive macros |
| `diagnostics` | ❌ no | Adds `miette::Diagnostic` to `EdifactError` for human-readable output |

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
use edifact_rs::ser;

# use edifact_rs::{EdifactSerialize};
# #[derive(EdifactSerialize)]
# #[edifact(segment = "BGM")]
# struct Bgm { #[edifact(element = 0)] doc_code: String }
let bgm = Bgm { doc_code: "220".into() };
let wire = ser::to_string(&bgm)?;
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

// Iterate raw windows:
for window in message_windows_from_reader(interchange.clone()) {
    let segs = window?;
    println!("window: {} segments", segs.len());
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

let document_pack = ProfileRulePack::builder("ORDERS-DOC")
    .for_message_type("ORDERS")
    .with_rule_fn(|segments| {
        let bgm = segments.iter().find(|s| s.tag == "BGM")?;
        let code = bgm.get_element(0)?.get_component(0)?;
        (code == "220").then(|| {
            ValidationIssue::new(ValidationSeverity::Warning, "code 220 requires special handling")
                .with_rule_id("ORDERS-DOC-P001")
                .with_segment("BGM")
                .with_element_index(0)
                .with_suggestion("Check your trading-partner agreement")
        })
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
use edifact_rs::{Validator, ValidationContext, ValidationLayer, ValidationReport, Segment};

struct StructureValidator;
impl Validator for StructureValidator {
    fn validate_batch(&self, _segments: &[Segment<'_>], _report: &mut ValidationReport) {
        // check mandatory segments, ordering, ...
    }
    fn set_message_type(&mut self, _: Option<&str>) {}
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
edifact-rs = { version = "0.4", features = ["diagnostics"] }
```

```
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

```
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
| Owned streaming | `from_reader_iter(reader)` | One `OwnedSegment` per segment; reader not buffered |

**Key types:**

| Type | Description |
|---|---|
| `Segment<'a>` | Zero-copy view with `tag: &'a str` and borrowed elements |
| `OwnedSegment` | Heap-owned copy; `.borrow()` returns O(1) `BorrowedSegment` |
| `BorrowedSegment<'a>` | Zero-allocation view of `OwnedSegment` |
| `EdifactError` | Stable error codes (E001–E020) with byte offsets |
| `ValidationReport` | Collected issues with lenient/strict modes |
| `ProfileRulePack` | Composable, filterable business-rule bundles |

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

### Reader with DOS guard

```rust
use edifact_rs::{ReaderConfig, from_bufread_stream_with_config};
use std::io::BufReader;

let config = ReaderConfig {
    max_segment_bytes: 8_192,
    ..Default::default()
};
let reader = BufReader::new(std::io::Cursor::new(b"BGM+220+test'"));
let segments = from_bufread_stream_with_config(reader, config)?;
# Ok::<(), edifact_rs::EdifactError>(())
```

### Write segments

```rust
use edifact_rs::{Writer, Segment, model::Element};

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

---

## 🌐 Async / tokio integration

`edifact-rs` is intentionally synchronous — EDIFACT parsing is CPU-bound and imposing an async runtime on all users would be wrong. Two clean patterns bridge to async:

**Pattern A** — read into memory, then parse synchronously (recommended for < 1 MB):

```rust,ignore
let bytes = tokio::fs::read("message.edi").await?;
let windows: Vec<_> = edifact_rs::message_windows_bytes(&bytes)
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

```bash
# Run all tests (unit + integration + doc-tests + derive UI tests):
cargo test --workspace --all-features

# Clippy (zero warnings policy):
cargo clippy --all-targets --all-features -- -D warnings

# Benchmarks (criterion + custom):
cargo bench -p edifact-rs

# Fuzz (requires cargo-bolero):
cargo bolero test -p edifact-rs fuzz_parse_write_parse_invariant_small_message
```

---

## 📋 Workspace layout

```
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
├── scripts/                 UNECE source download helpers
├── CHANGELOG.md
├── CONCEPT.md
├── FINDINGS.md              independent code-review findings (24/24 fixed ✅)
└── RELEASE_POLICY.md
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

