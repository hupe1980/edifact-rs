# Getting Started 🚀

This guide walks you from zero to a working `edifact-rs` integration in under five minutes.

---

## Prerequisites

- **Rust 1.85 or later** (edition 2024)
- A Cargo workspace or binary / library project

Check your toolchain:

```bash
rustup show        # active toolchain
rustup update      # upgrade to latest stable
```

---

## 1. Add the dependency

```toml
[dependencies]
edifact-rs = "0.1"
```

The `derive` feature is enabled by default, which re-exports
`EdifactDeserialize` and `EdifactSerialize` derive macros from `edifact-rs-derive`.

### Optional features

| Feature | Default | What it adds |
|---|---|---|
| `derive` | ✅ yes | Proc-macro derive for typed structs |
| `diagnostics` | ❌ no | `miette::Diagnostic` on `EdifactError` — human-friendly output |

Enable diagnostics:

```toml
[dependencies]
edifact-rs = { version = "0.1", features = ["diagnostics"] }
```

Disable derive macros (core parsing only):

```toml
[dependencies]
edifact-rs = { version = "0.1", default-features = false }
```

---

## 2. Parse your first EDIFACT message

```rust
use edifact_rs::from_bytes;

fn main() -> Result<(), edifact_rs::EdifactError> {
    // Minimal ORDERS interchange (UNA is optional)
    let input = b"UNA:+.? '\
                  UNH+1+ORDERS:D:11A:UN'\
                  BGM+220+PO-4711+9'\
                  NAD+BY+4000001000002::9'\
                  UNT+4+1'";

    let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

    for seg in &segments {
        println!("{} ({} elements)", seg.tag, seg.elements.len());
    }
    // UNH (2 elements)
    // BGM (3 elements)
    // NAD (2 elements)
    // UNT (2 elements)

    Ok(())
}
```

`from_bytes` returns an **iterator** of `Result<Segment<'_>, EdifactError>`.
Each `Segment` borrows directly from `input` — zero heap allocation for tag and element text.

---

## 3. Access segment data

```rust
use edifact_rs::from_bytes;

fn main() -> Result<(), edifact_rs::EdifactError> {
    let input = b"BGM+220+PO-4711+9'";
    let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;
    let bgm = &segs[0];

    // element_str(n) — shorthand for component 0 of element n
    assert_eq!(bgm.element_str(0), Some("220"));
    assert_eq!(bgm.element_str(1), Some("PO-4711"));

    // get_element(n) then get_component(c) for composite access
    let doc_code = bgm
        .get_element(0)
        .and_then(|e| e.get_component(0))
        .unwrap_or_default();
    assert_eq!(doc_code, "220");

    Ok(())
}
```

---

## 4. Typed mapping with derive macros

Instead of accessing elements by index, declare a struct:

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

fn main() -> Result<(), edifact_rs::EdifactError> {
    let input = b"BGM+220+PO-4711+9'";
    let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;
    let bgm = Bgm::edifact_deserialize(&segs)?;

    println!("doc_code={}", bgm.document_name_code);
    println!("doc_number={}", bgm.document_number);
    Ok(())
}
```

→ Full derive reference: [Typed Derive](typed-derive.md)

---

## 5. Validate a message

```rust
use edifact_rs::{
    ValidationContext, ValidationLayer, Validator, ValidationReport, Segment,
    from_bytes,
};

struct MyValidator;

impl Validator for MyValidator {
    fn validate_batch(&self, _segments: &[Segment<'_>], _report: &mut ValidationReport) {
        // your validation logic here
    }
    fn set_message_type(&mut self, _: Option<&str>) {}
}

fn main() -> Result<(), edifact_rs::EdifactError> {
    let input = b"UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'UNT+3+1'";
    let segs: Vec<_> = from_bytes(input).collect::<Result<_, _>>()?;

    let ctx = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(ValidationLayer::Structure, MyValidator)
        .build();

    let report = ctx.validate_lenient(&segs);
    if report.is_valid() {
        println!("✅ valid");
    } else {
        for issue in &report.errors {
            eprintln!("❌ {}", issue.message);
        }
    }
    Ok(())
}
```

→ Full validation guide: [Validation](validation.md)

---

## 6. Process a reader (large files)

```rust
use edifact_rs::from_reader_iter;
use std::fs::File;

fn main() -> Result<(), edifact_rs::EdifactError> {
    let f = File::open("interchange.edi")?;
    for result in from_reader_iter(f) {
        let segment = result?;
        println!("{}", segment.tag);
    }
    Ok(())
}
```

`from_reader_iter` parses one `OwnedSegment` at a time without buffering the whole file.

→ Full streaming guide: [Streaming](streaming.md)

---

## 7. Next steps

| Goal | Guide |
|---|---|
| Understand UNA, delimiters, release chars | [Core Concepts](core-concepts.md) |
| Parse byte slices efficiently | [Parsing](parsing.md) |
| Write EDIFACT output | [Writing](writing.md) |
| Derive typed structs for segments and messages | [Typed Derive](typed-derive.md) |
| Stream multi-message interchanges | [Streaming](streaming.md) |
| Add business-rule validation | [Profile Packs](profile-packs.md) |
| Pretty-print errors in a CLI | [Diagnostics](diagnostics.md) |
| Integrate with async / tokio | [Async Integration](async-integration.md) |
