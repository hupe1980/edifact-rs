# edifact-rs Documentation 📚

Welcome to the **edifact-rs** documentation hub. These guides cover everything from
first install to production-grade performance tuning.

---

## 🗺️ Navigation

| Guide | Description |
|---|---|
| [Getting Started](getting-started.md) | Install, first parse, feature flags |
| [Core Concepts](core-concepts.md) | EDIFACT wire format, segments, delimiters, UNA |
| [Parsing](parsing.md) | `from_bytes`, `from_reader`, spans, UNA handling |
| [Writing](writing.md) | `Writer`, `ser::to_string`, escape rules |
| [Typed Derive](typed-derive.md) | `#[derive(EdifactDeserialize, EdifactSerialize)]` and all attributes |
| [Streaming](streaming.md) | Reader iterators, message windows, low-memory extraction |
| [Validation](validation.md) | `Validator` trait, `ValidationContext`, multi-layer pipelines |
| [Profile Packs](profile-packs.md) | `ProfileRulePack`, rule authoring, merging, filtering |
| [Diagnostics](diagnostics.md) | `diagnostics` feature, miette integration |
| [Async Integration](async-integration.md) | Tokio patterns A / B / C |
| [Error Reference](error-reference.md) | All `EdifactError` variants and stable codes E001–E020 |
| [Performance](performance.md) | Zero-copy, allocation budgets, benchmarking |

---

## 🚀 Quick links

- **New to edifact-rs?** → [Getting Started](getting-started.md)
- **Want typed structs from EDIFACT?** → [Typed Derive](typed-derive.md)
- **Processing large files?** → [Streaming](streaming.md)
- **Adding business rules?** → [Profile Packs](profile-packs.md)
- **Getting a compile error from derive?** → [Typed Derive § Troubleshooting](typed-derive.md#troubleshooting)
- **Integrating with tokio?** → [Async Integration](async-integration.md)

---

## 📦 Runnable examples

All examples live in [`crates/edifact-rs/examples/`](../crates/edifact-rs/examples/) and
can be run with:

```bash
cargo run -p edifact-rs --example <name>
```

| Example | Guide |
|---|---|
| `cookbook_parse_map_validate_write` | [Parsing](parsing.md) · [Validation](validation.md) |
| `cookbook_typed_derive` | [Typed Derive](typed-derive.md) |
| `cookbook_typed_streaming` | [Streaming](streaming.md) |
| `cookbook_message_window_streaming` | [Streaming § Message windows](streaming.md#message-windows) |
| `cookbook_profile_packs` | [Profile Packs](profile-packs.md) |
| `cookbook_profile_error_mapping` | [Profile Packs § Error mapping](profile-packs.md#mapping-violations-to-application-errors) |
| `cookbook_streamed_progressive_validation` | [Validation § Progressive](validation.md#progressive-streaming-validation) |
| `cookbook_fixture_validation` | [Validation § Custom validators](validation.md#custom-validator-implementations) |
| `cookbook_diagnostics` | [Diagnostics](diagnostics.md) |
| `cookbook_async_tokio_integration` | [Async Integration](async-integration.md) |

---

## 🔗 External links

- [crates.io](https://crates.io/crates/edifact-rs)
- [docs.rs API reference](https://docs.rs/edifact-rs)
- [GitHub repository](https://github.com/hupe1980/edifact-rs)
- [UNECE EDIFACT standards](https://unece.org/trade/uncefact/unedifact)
