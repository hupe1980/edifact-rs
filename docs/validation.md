# Validation ✅

`edifact-rs` provides a layered, composable validation pipeline that separates
structural, code-list, and profile-level checks — each pluggable independently.

---

## Key types

| Type | Role |
|---|---|
| `Validator` | Trait — implement to create a custom validator |
| `ValidationContext` | Orchestrates multiple validators across layers |
| `ValidationLayer` | Enum — `Structure`, `CodeList`, `Profile` |
| `ValidationReport` | Aggregated result — `errors`, `warnings`, `infos` |
| `ValidationIssue` | A single finding — severity, message, rule ID, offsets |
| `ValidationSeverity` | `Critical`, `Error`, `Warning`, `Info` |
| `ProfileRulePack` | Composable bundle of closure-based profile rules |
| `validate_each` | Helper — run a per-segment function over a slice |

---

## The `Validator` trait

Implement `Validator` to encapsulate validation logic:

```rust
use edifact_rs::{Validator, ValidationReport, Segment, validate_each, EdifactError};

struct BgmCodeValidator;

impl Validator for BgmCodeValidator {
    fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
        validate_each(segments, report, |seg| {
            if seg.tag == "BGM" {
                let code = seg.element_str(0).unwrap_or("");
                if !matches!(code, "220" | "231" | "261") {
                    return Err(EdifactError::InvalidCodeValue {
                        tag: "BGM".to_owned(),
                        element_index: 0,
                        value: code.to_owned(),
                        code_list: "1001".to_owned(),
                        offset: seg.span.start,
                        suggestion: Some("Use 220 (original order), 231 (quote) or 261 (confirmation)".to_owned()),
                    });
                }
            }
            Ok(())
        });
    }

    fn set_message_type(&mut self, _msg_type: Option<&str>) {}
}
```

The `validate_batch` method receives the **complete** segment slice for the current
validation scope (one `UNH..UNT` window or the whole interchange, depending on how
the context is driven).

`validate_each` is a helper that iterates segments and maps each `Err` result to a
`ValidationIssue` appended to `report`.

---

## `ValidationContext` and layers

Validators are registered per layer. Layers run in order:

1. **`Structure`** — check mandatory segments, ordering, counts
2. **`CodeList`** — check element values against UNTDID code lists
3. **`Profile`** — check business/MIG rules (see [Profile Packs](profile-packs.md))

```rust
use edifact_rs::{
    ValidationContext, ValidationLayer, Validator, ValidationReport, Segment,
    from_bytes,
};

# struct BgmCodeValidator;
# impl Validator for BgmCodeValidator {
#     fn validate_batch(&self, _: &[Segment<'_>], _: &mut ValidationReport) {}
#     fn set_message_type(&mut self, _: Option<&str>) {}
# }
let segs: Vec<_> = from_bytes(b"UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'UNT+3+1'")
    .collect::<Result<_, _>>()?;

let ctx = ValidationContext::builder()
    .with_message_type("ORDERS")           // passed to set_message_type on each validator
    .with_validator(ValidationLayer::CodeList, BgmCodeValidator)
    .build();

let report = ctx.validate_lenient(&segs);
# Ok::<(), edifact_rs::EdifactError>(())
```

### Disabling layers

```rust
use edifact_rs::ValidationContextBuilder;

let ctx = ValidationContext::builder()
    .disable_layer(ValidationLayer::CodeList) // skip code list checks
    .with_validator(ValidationLayer::Structure, my_struct_validator)
    .build();
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## `validate_lenient` vs `validate_strict`

| Method | On first error | Returns |
|---|---|---|
| `validate_lenient(&segs)` | Continues collecting all issues | `ValidationReport` |
| `validate_strict(&segs)` | Stops at first `Error` or `Critical` | `Result<ValidationReport, EdifactError>` |

```rust
# use edifact_rs::{ValidationContext, from_bytes};
# let segs: Vec<_> = from_bytes(b"BGM+220+PO-4711+9'").collect::<Result<_,_>>()?;
# let ctx = ValidationContext::builder().build();

// Lenient: get all issues
let report = ctx.validate_lenient(&segs);
if !report.is_valid() {
    for issue in &report.errors {
        eprintln!("error [{}]: {}", issue.error_code.unwrap_or("?"), issue.message);
    }
    for warn in &report.warnings {
        eprintln!("warn:  {}", warn.message);
    }
}

// Strict: fail fast
match ctx.validate_strict(&segs) {
    Ok(report) => println!("valid, {} warnings", report.warnings.len()),
    Err(e) => eprintln!("invalid: {e}"),
}
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## `ValidationReport` — working with results

```rust
# use edifact_rs::{ValidationContext, from_bytes};
# let segs: Vec<_> = from_bytes(b"BGM+220+PO-4711+9'").collect::<Result<_,_>>()?;
# let ctx = ValidationContext::builder().build();
let report = ctx.validate_lenient(&segs);

// Overall validity (no errors, no criticals)
println!("valid: {}", report.is_valid());
println!("errors: {}", report.errors.len());
println!("warnings: {}", report.warnings.len());
println!("infos: {}", report.infos.len());
println!("total issues: {}", report.total_issues());

// Deterministic string rendering (useful for snapshots / golden tests)
let text = report.render_deterministic();
println!("{text}");

// Filter by rule ID prefix (for profile-pack namespacing)
let profile_issues = report.filter_by_rule_prefix("ORDERS-DEMO-");
println!("{} ORDERS-DEMO issues", profile_issues.total_issues());

// Get issues for a specific rule (lazy iterator; collect or count as needed)
let count = report.issues_for_rule_id("ORDERS-P001").count();
println!("ORDERS-P001 findings: {count}");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## `ValidationIssue` — building findings

Within a `Validator` or `ProfileRulePack` rule, construct `ValidationIssue` with the
builder API:

```rust
use edifact_rs::{ValidationIssue, ValidationSeverity};

let issue = ValidationIssue::new(
    ValidationSeverity::Error,
    "BGM document code 999 is not accepted",
)
.with_rule_id("ORDERS-BGM-001")        // stable ID for filtering / mapping
.with_segment("BGM")                    // which segment tag
.with_element_index(0)                  // which element
.with_error_code("E007")                // EDIFACT or application error code
.with_suggestion("Use code 220, 231, or 261")
.with_offset(42);                       // byte offset in the interchange
```

| Builder method | Type | Purpose |
|---|---|---|
| `.with_rule_id(id)` | `&str` | Stable ID for filtering and mapping |
| `.with_segment(tag)` | `&str` | Segment tag where the issue was found |
| `.with_element_index(n)` | `usize` | Element index (0-based) |
| `.with_error_code(code)` | `&str` | Stable error code string (e.g. `"E007"`) |
| `.with_suggestion(text)` | `&str` | Human-friendly remediation hint |
| `.with_offset(n)` | `usize` | Byte offset of the issue in the input |

---

## Custom validator implementations

For complex validation that needs shared state across segments (e.g. reference
counting, cross-segment consistency), implement `Validator` as a struct:

```rust
use edifact_rs::{Validator, ValidationReport, ValidationIssue, ValidationSeverity, Segment};

struct ReferenceConsistencyValidator {
    message_type: Option<String>,
}

impl Validator for ReferenceConsistencyValidator {
    fn set_message_type(&mut self, mt: Option<&str>) {
        self.message_type = mt.map(str::to_owned);
    }

    fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
        // Find the UNH reference
        let unh_ref = segments
            .iter()
            .find(|s| s.tag == "UNH")
            .and_then(|s| s.element_str(0))
            .unwrap_or("");

        // Find the UNT reference
        let unt_ref = segments
            .iter()
            .find(|s| s.tag == "UNT")
            .and_then(|s| s.element_str(1))
            .unwrap_or("");

        if unh_ref != unt_ref {
            report.add_error(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    format!("UNH reference '{unh_ref}' does not match UNT reference '{unt_ref}'"),
                )
                .with_rule_id("ENVELOPE-REF-PARITY")
                .with_segment("UNT")
                .with_element_index(1),
            );
        }
    }
}
```

See [`cookbook_fixture_validation.rs`](../crates/edifact-rs/examples/cookbook_fixture_validation.rs)
for a complete validator with fixture-based test data.

---

## Progressive streaming validation

Validate each `UNH..UNT` window as it arrives from the reader:

```rust
use edifact_rs::{
    ValidationContext, ProfileRulePack, ValidationIssue, ValidationSeverity,
    message_windows_from_reader,
};
use std::io::Cursor;

let input = Cursor::new(b"\
    UNH+1+ORDERS:D:11A:UN'BGM+220+PO-001+9'UNT+3+1'\
    UNH+2+ORDERS:D:11A:UN'BGM+220+PO-002+9'UNT+3+2'".to_vec());

let ctx = ValidationContext::builder()
    .with_profile_pack(
        ProfileRulePack::builder("ORDERS-REQUIRED")
            .for_message_type("ORDERS")
            .with_rule_fn(|segs| {
                segs.iter().any(|s| s.tag == "BGM").then(|| ()).xor(Some(())).map(|_| {
                    ValidationIssue::new(ValidationSeverity::Error, "BGM is required")
                        .with_rule_id("ORDERS-REQ-BGM")
                })
            }),
    )
    .build();

for result in message_windows_from_reader(input) {
    let window = result?;
    let borrowed: Vec<_> = window.iter().map(|s| s.as_borrowed()).collect();
    let report = ctx.validate_lenient(&borrowed);
    println!(
        "message {}: {} error(s)",
        window[0].element_str(0).unwrap_or("?"),
        report.errors.len()
    );
}
# Ok::<(), edifact_rs::EdifactError>(())
```

See [`cookbook_streamed_progressive_validation.rs`](../crates/edifact-rs/examples/cookbook_streamed_progressive_validation.rs)
for a complete example.

---

## Directory validator

`DirectoryValidator` provides structural validation against a user-supplied segment
definition dictionary:

```rust
use edifact_rs::{DirectoryValidator, SegmentDefinition, ElementDefinition};

let mut validator = DirectoryValidator::new();
validator.register(SegmentDefinition {
    tag: "BGM".to_owned(),
    elements: vec![
        ElementDefinition { min_length: 1, max_length: 3, is_mandatory: true },
        ElementDefinition { min_length: 1, max_length: 35, is_mandatory: true },
        ElementDefinition { min_length: 1, max_length: 3, is_mandatory: false },
    ],
});
```

> **Scope note**: `DirectoryValidator` validates element presence and length within
> individual segments. It does not enforce full EDIFACT message grammar (conditional
> segment groups, repeat counts). Use `ProfileRulePack` for those cross-segment rules.

---

## Next steps

- [Profile Packs](profile-packs.md) — composable business-rule bundles
- [Error Reference](error-reference.md) — stable codes for `ValidationIssue.error_code`
- [Diagnostics](diagnostics.md) — human-readable error rendering
