# Diagnostics 🩺

The `diagnostics` feature adds rich, span-annotated error output to `EdifactError`
via the [`miette`](https://docs.rs/miette) crate. It is **opt-in** — enabling it has
no effect on parsing performance and adds no overhead when errors are handled
programmatically.

---

## Enabling the feature

```toml
[dependencies]
edifact-rs = { version = "0.4", features = ["diagnostics"] }
```

You also need `miette` in your dependencies to render errors:

```toml
[dependencies]
miette = { version = "7", features = ["fancy"] }
```

---

## What it does

When `diagnostics` is enabled:

- `EdifactError` implements `miette::Diagnostic`
- Every error carries a **byte-level source span** pointing into the input
- `miette::Report` renders errors with annotated source context:

```
Error: invalid code value "999" at offset 42
  ╭─ input.edi:2:5
  │
2 │ BGM+999+PO-4711+9'
  │     ^^^ code "999" is not in code list 1001
  ╰──
Error Code: E007
Help: Use a valid document name code from UNTDID 1001
```

---

## Rendering errors

### In a binary / CLI

```rust
fn main() -> miette::Result<()> {
    let input = b"UNH+1+ORDERS:D:11A:UN'BGM+999+PO-4711+9'UNT+3+1'";
    let segs: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<_, _>>()?;  // EdifactError auto-converts to miette::Report
    Ok(())
}
```

When you use `miette::Result` as the return type of `main`, `miette` automatically
renders any `EdifactError` with full source context.

### Explicit rendering

```rust
fn validate(input: &[u8]) {
    use edifact_rs::{from_bytes, EdifactError, ValidationContext};

    match from_bytes(input).collect::<Result<Vec<_>, _>>() {
        Ok(segs) => {
            let ctx = ValidationContext::builder().build();
            let report = ctx.validate_lenient(&segs);
            println!("{}", report.render_deterministic());
        }
        Err(err) => {
            // miette::Report wraps the error and renders with full context
            let report = miette::Report::new(err);
            eprintln!("{report:?}");
        }
    }
}
```

### Propagating through anyhow

```rust
use anyhow::Context;

fn parse_edi(input: &[u8]) -> anyhow::Result<Vec<edifact_rs::OwnedSegment>> {
    edifact_rs::from_reader(std::io::Cursor::new(input))
        .context("failed to parse EDIFACT interchange")
}
```

---

## Validation report rendering

`ValidationReport::render_deterministic()` always produces the same output for the
same input, making it suitable for snapshot tests:

```rust
use edifact_rs::{from_bytes, ValidationContext, ValidationLayer, Validator, ValidationReport, Segment};

# struct DemoValidator;
# impl Validator for DemoValidator {
#     fn validate_batch(&self, _: &[Segment<'_>], _: &mut ValidationReport) {}
#     fn set_message_type(&mut self, _: Option<&str>) {}
# }
let segs: Vec<_> = from_bytes(b"BGM+220+PO-4711+9'").collect::<Result<_, _>>()?;
let ctx = ValidationContext::builder()
    .with_validator(ValidationLayer::Structure, DemoValidator)
    .build();

let report = ctx.validate_lenient(&segs);
let rendered = report.render_deterministic();

// Use in snapshot tests (insta, expect-test, etc.)
println!("{rendered}");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Error structure (with `diagnostics`)

Each `EdifactError` variant that includes a byte offset exposes it as a miette
`SourceSpan`:

| Variant | Span source |
|---|---|
| `UnexpectedEof { offset }` | Single byte at `offset` |
| `InvalidDelimiter { byte, offset }` | Single byte at `offset` |
| `InvalidText { offset }` | Start of invalid byte sequence |
| `InvalidReleaseSequence { offset }` | The dangling `?` character |
| `InvalidCodeValue { offset, … }` | The element start position |
| `SegmentTooLong { offset, … }` | Start of the oversized segment |

---

## Checking at runtime whether diagnostics are enabled

The feature is a compile-time gate. If you need to branch on it at runtime (e.g. in
tests), use `cfg`:

```rust
fn setup_error_handler() {
    #[cfg(feature = "diagnostics")]
    {
        // install miette's panic hook for better output in tests
        miette::set_panic_hook();
    }
}
```

---

## Example

See [`cookbook_diagnostics.rs`](../crates/edifact-rs/examples/cookbook_diagnostics.rs)
for a full example that triggers a code-value validation error and renders it with
`miette::Report`:

```bash
cargo run -p edifact-rs --example cookbook_diagnostics --features diagnostics
```

Sample output (with `miette`'s `fancy` feature):

```
  × invalid code value "999" in BGM element 0
   ╭─[<anonymous>:1:5]
 1 │ BGM+999+PO-4711+9'
   ·     ─┬─
   ·      ╰── value "999" is not valid for code list 1001
   ╰────
  help: Use a valid document name code from UNTDID 1001
```

---

## Feature interaction

| Scenario | Result |
|---|---|
| `diagnostics` disabled (default) | `EdifactError` implements `std::error::Error` + `Display` + `Debug` |
| `diagnostics` enabled | Also implements `miette::Diagnostic`; adds no runtime overhead when errors don't occur |
| `diagnostics` + `miette` fancy | Full ANSI-colored, span-annotated terminal output |
| `diagnostics` + no `miette` in deps | You can still construct `miette::Report::new(err)` because `edifact_rs` re-exports the `miette` dependency |

---

## Next steps

- [Error Reference](error-reference.md) — all error codes and their span fields
- [Validation](validation.md) — `ValidationReport::render_deterministic()`
- [Getting Started](getting-started.md) — feature flag installation
