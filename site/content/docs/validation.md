+++
title = "Validation"
description = "The Validator trait, ValidationContext, and the layered envelope / structure / code-list / profile validation pipeline."
weight = 70
+++

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
use edifact_rs::{Validator, ValidationReport, ValidationRuleContext, Segment, validate_each, EdifactError};

struct BgmCodeValidator;

impl Validator for BgmCodeValidator {
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        _context: &ValidationRuleContext<'_>,
    ) {
        validate_each(segments, report, |seg| {
            if seg.tag == "BGM" {
                let code = seg.element_str(0).unwrap_or("");
                if !matches!(code, "220" | "231" | "261") {
                    return Err(EdifactError::InvalidCodeValue {
                        tag: "BGM".to_owned(),
                        element_index: 0,
                        value: code.to_owned(),
                        code_list: "1001".to_owned(),
                        span: seg.span,
                        suggestion: Some("Use 220 (original order), 231 (quote) or 261 (confirmation)"),
                    });
                }
            }
            Ok(())
        });
    }
}
```

The `validate_batch` method receives the **complete** segment slice for the current
validation scope (one `UNH..UNT` window or the whole interchange, depending on how
the context is driven).

`validate_each` is a helper that iterates segments and maps each `Err` result to a
`ValidationIssue` appended to `report`.

The trait also provides `validate_group_batch` (default: no-op) which is called once
per segment-group occurrence when using the group-aware validation path. Override it
to access the isolated segment list for each group instance.

---

## `ValidationContext` and layers

Validators are registered per layer. Layers run in order:

1. **`Structure`** — check mandatory segments, ordering, counts
2. **`CodeList`** — check element values against UNTDID code lists
3. **`Profile`** — check business/MIG rules (see [Profile Packs](@/docs/profile-packs.md))

```rust
use edifact_rs::{
    ValidationContext, ValidationLayer, Validator, ValidationReport,
    ValidationRuleContext, Segment, from_bytes,
};

# struct BgmCodeValidator;
# impl Validator for BgmCodeValidator {
#     fn validate_batch(&self, _: &[Segment<'_>], _: &mut ValidationReport, _: &ValidationRuleContext<'_>) {}
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
use edifact_rs::{ValidationContext, ValidationLayer};

let ctx = ValidationContext::builder()
    .code_list(false)    // skip code list checks
    .structure(false)    // skip structural checks
    .build();
```

Builder toggles: `.structure(bool)`, `.code_list(bool)`, `.profile(bool)`, `.envelope(bool)`.

### Shared `Arc<ProfileRulePack>` (efficient multi-context reuse)

When the same pack is used across many `ValidationContext` instances (e.g. in a
server hot-path), share it via `Arc` to avoid cloning the rule closures:

```rust
use edifact_rs::{ValidationContext, ProfileRulePack};
use std::sync::Arc;

let pack = Arc::new(
    ProfileRulePack::new("ORDERS-RULES")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(|_segs, _issues| {}),
);

// Lightweight clone — closures are not duplicated:
let ctx1 = ValidationContext::builder()
    .with_profile_pack_arc(Arc::clone(&pack))
    .build();

let ctx2 = ValidationContext::builder()
    .with_profile_pack_arc(Arc::clone(&pack))
    .build();
```

### Early abort on first critical issue

```rust
use edifact_rs::ValidationContext;

let ctx = ValidationContext::builder()
    .bail_on_first_critical(true)  // stop after the first Critical-severity issue
    .build();
```

### Built-in envelope validation

The `EnvelopeValidator` checks `UNB`/`UNH`/`UNT`/`UNZ` segment presence, message
counts, and segment counts.  Enable it with `with_envelope_validation()`:

```rust
use edifact_rs::{ValidationContext, from_bytes};

# let segs: Vec<_> = from_bytes(b"UNB+UNOA:1+SENDER:1+RECEIVER:1+200101:1000+1'UNH+1+ORDERS:D:96A:UN'BGM+220+PO-1'UNT+3+1'UNZ+1+1'").collect::<Result<_,_>>()?;
let ctx = ValidationContext::builder()
    .with_envelope_validation()  // adds UNB/UNH/UNT/UNZ structure checks
    .build();
let report = ctx.validate_lenient(&segs);
# Ok::<(), edifact_rs::EdifactError>(())
```

`UNG`/`UNE` functional-group segments are rejected with `E029`
(`FunctionalGroupNotSupported`) since this library does not process legacy
functional groups.

### Per-message reference stamping

When validating individual messages extracted from a multi-message interchange,
use `with_message_ref` to stamp every emitted `ValidationIssue` with the `UNH`
reference (DE 0062).  This makes it easy to map issues back to the originating
message:

```rust
use edifact_rs::{ValidationContext, from_bytes};

# let segs: Vec<_> = from_bytes(b"UNH+MSG-42+ORDERS:D:96A:UN'BGM+220+PO-1'UNT+3+MSG-42'").collect::<Result<_,_>>()?;
let ctx = ValidationContext::builder()
    .with_message_type("ORDERS")
    .with_message_ref("MSG-42")  // DE 0062 from the UNH segment
    .build();
let report = ctx.validate_lenient(&segs);
// Every issue in `report` will have `issue.message_ref == Some("MSG-42")`
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## `validate_lenient` vs `validate_strict`

| Method | On first error | Returns |
|---|---|---|
| `validate_lenient(&segs)` | Continues collecting all issues | `ValidationReport` |
| `validate_strict(&segs)` | Runs all validators, returns `Err(report)` if any `Error`/`Critical` found | `Result<ValidationReport, ValidationReport>` |

```rust
# use edifact_rs::{ValidationContext, from_bytes};
# let segs: Vec<_> = from_bytes(b"BGM+220+PO-4711+9'").collect::<Result<_,_>>()?;
# let ctx = ValidationContext::builder().build();

// Lenient: collect all issues even when errors are present
let report = ctx.validate_lenient(&segs);
if !report.is_valid() {
    for issue in report.errors() {
        eprintln!("error [{}]: {}", issue.error_code().unwrap_or("?"), issue.message);
    }
    for warn in report.warnings() {
        eprintln!("warn:  {}", warn.message);
    }
}

// Strict: run all validators; get Err(report) when any Error/Critical found
match ctx.validate_strict(&segs) {
    Ok(report) => println!("valid, {} warnings", report.warnings().len()),
    Err(report) => {
        for issue in report.errors() {
            eprintln!("error [{}]: {}", issue.error_code().unwrap_or("?"), issue.message);
        }
    }
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
println!("errors: {}", report.errors().len());
println!("warnings: {}", report.warnings().len());
println!("infos: {}", report.infos().len());
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
use edifact_rs::{Span, ValidationIssue, ValidationSeverity};

let issue = ValidationIssue::new(
    ValidationSeverity::Error,
    "BGM document code 999 is not accepted",
)
.with_rule_id("ORDERS-BGM-001")        // stable ID for filtering / mapping
.with_segment("BGM")                    // which segment tag
.with_element_index(0)                  // which element
.with_error_code("E007")                // EDIFACT or application error code
.with_suggestion("Use code 220, 231, or 261")
.with_span(Span::new(42, 60));          // byte range of the offending region
```

| Builder method | Type | Purpose |
|---|---|---|
| `.with_rule_id(id)` | `&str` | Stable ID for filtering and mapping |
| `.with_segment(tag)` | `&str` | Segment tag where the issue was found |
| `.with_element_index(n)` | `usize` | Element index (0-based) |
| `.with_error_code(code)` | `impl Into<Cow<'static, str>>` | Stable error code string (e.g. `"E007"`); a `&'static str` costs no allocation, and the field round-trips through `serde` |
| `.with_suggestion(text)` | `&str` | Human-friendly remediation hint |
| `.with_span(span)` | `Span` | Byte range of the issue in the input — the only positional field; read `issue.span.map(\|s\| s.start)` or `issue.start_offset()` for the start alone |
| `.with_segment_occurrence(n)` | `u16` | Zero-based occurrence among segments with the same tag |
| `.with_segment_group(name)` | `impl Into<String>` | Name of the segment group instance (e.g. `"SG5"`) — set automatically by group-scoped rules |
| `.with_message_ref(r)` | `impl Into<String>` | `UNH` reference (DE 0062) — usually set automatically via `ValidationContextBuilder::with_message_ref` |
| `.with_context_entry(k, v)` | `(impl Into<String>, impl Into<String>)` | Insert a single key-value pair into the domain metadata map |
| `.with_context_entries(iter)` | `impl IntoIterator<Item=(K,V)>` | Bulk-insert domain metadata; duplicate keys overwrite |

### Rule ID prefix convention

`rule_id` doubles as a lightweight metadata carrier. Use a structured,
namespaced prefix so downstream code can extract domain identifiers without
parsing the human-readable message:

```text
"<PACK>-<SCOPE>-<TAG>-<STATUS>"
 ^^^^^^                          — the pack / profile that owns the rule
        ^^^^^^^                  — a process identifier, group name, or other discriminator
                ^^^^^            — the affected segment
                      ^^^^^^^^   — M / C / … status or short discriminator
```

Example: `"PROFILE-4711-BGM-M"` — pack `PROFILE`, scope `4711`, segment `BGM`,
mandatory (`M`). Recover the scope:

```rust
let rule_id = "PROFILE-4711-BGM-M";
let scope = rule_id.strip_prefix("PROFILE-").and_then(|s| s.split('-').next());
assert_eq!(scope, Some("4711"));
```

For arbitrary domain metadata use `with_context_entry` instead:

```rust
use edifact_rs::{Span, ValidationIssue, ValidationSeverity};

let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
    .with_rule_id("PROFILE-4711-BGM-M")
    .with_context_entry("pid", "4711")
    .with_context_entry("partner", "9900123456789");

assert_eq!(issue.context_get("pid"), Some("4711"));
```

---

## Custom validator implementations

For complex validation that needs shared state across segments (e.g. reference
counting, cross-segment consistency), implement `Validator` as a struct:

```rust
use edifact_rs::{Validator, ValidationReport, ValidationRuleContext, ValidationIssue, ValidationSeverity, Segment};

struct ReferenceConsistencyValidator;

impl Validator for ReferenceConsistencyValidator {
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        _context: &ValidationRuleContext<'_>,
    ) {
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

See [`cookbook_fixture_validation.rs`](https://github.com/hupe1980/edifact-rs/tree/main/crates/edifact-rs/examples/cookbook_fixture_validation.rs)
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
        ProfileRulePack::new("ORDERS-REQUIRED")
            .for_message_type("ORDERS")
            .with_stateless_rule_fn(|segs, issues| {
                if !segs.iter().any(|s| s.tag == "BGM") {
                    issues.push(
                        ValidationIssue::new(ValidationSeverity::Error, "BGM is required")
                            .with_rule_id("ORDERS-REQ-BGM"),
                    );
                }
            }),
    )
    .build();

for result in message_windows_from_reader(input) {
    let window = result?;
    let borrowed: Vec<_> = window.segments.iter().map(|s| s.as_borrowed()).collect();
    let report = ctx.validate_lenient(&borrowed);
    println!(
        "message {:?}: {} error(s)",
        window.message_type,
        report.errors().len()
    );
}
# Ok::<(), edifact_rs::EdifactError>(())
```

See [`cookbook_streamed_progressive_validation.rs`](https://github.com/hupe1980/edifact-rs/tree/main/crates/edifact-rs/examples/cookbook_streamed_progressive_validation.rs)
for a complete example.

---

## Group-aware validation

Group-aware validation fires `ProfileRulePack` group rules once per segment-group
occurrence (e.g. once per `SG5` instance) rather than once across the entire
message. First define a `&'static [GroupDef]` schema, build a `SegmentGroupIndexed`
tree with `group_segments_indexed`, then pass the tree to `validate_lenient_grouped`:

```rust
use edifact_rs::{
    ValidationContext, ProfileRulePack,
    group::{GroupDef, group_segments_indexed},
    from_bytes,
};

// Schema: SG5 starts at LIN and contains an SG6 sub-group starting at QTY.
// GroupDef is a plain struct with &'static [GroupDef] children — use a static.
static SCHEMA: &[GroupDef] = &[GroupDef {
    name: "SG5",
    trigger: "LIN",
    children: &[GroupDef {
        name: "SG6",
        trigger: "QTY",
        children: &[],
    }],
}];

let segs: Vec<_> = from_bytes(
    b"UNH+1+ORDERS:D:96A:UN'\
      LIN+1'QTY+21:10'\
      LIN+2'\
      UNT+5+1'"
).collect::<Result<_, _>>()?;

// Build the indexed group tree (O(n × schema_depth), no segment clones):
let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

let pack = ProfileRulePack::new("ORDERS")
    .for_message_type("ORDERS")
    // Require QTY inside every SG5 occurrence:
    .require_segment_in_group("SG5", "QTY", "ORDERS-SG5-QTY-M");

let ctx = ValidationContext::builder()
    .with_profile_pack(pack)
    .build();

// validate_lenient_grouped runs the flat pass then the group pass:
let report = ctx.validate_lenient_grouped(&tree, &segs);
println!("{} error(s)", report.errors().len());
# Ok::<(), edifact_rs::EdifactError>(())
```

For owned segments (e.g. from `message_windows_from_reader`), use the `_owned`
variants:

| Method | Args | Segment type | Mode |
|---|---|---|---|
| `validate_lenient_grouped(root, segs)` | `(&SegmentGroupIndexed, &[Segment])` | borrowed | Collect all issues |
| `validate_strict_grouped(root, segs)` | `(&SegmentGroupIndexed, &[Segment])` | borrowed | `Err` on first error/critical |
| `validate_lenient_grouped_owned(root, segs)` | `(&SegmentGroupIndexed, &[OwnedSegment])` | owned | Collect all issues |
| `validate_strict_grouped_owned(root, segs)` | `(&SegmentGroupIndexed, &[OwnedSegment])` | owned | `Err` on first error/critical |

See [Profile Packs — Group-scoped rules](@/docs/profile-packs.md#group-scoped-rules) for
how to build group rules.

---

## Directory validator

`DirectoryValidator` provides structural validation against a user-supplied segment
definition dictionary:

```rust
use edifact_rs::{DirectoryValidatorBuilder, OwnedSegmentDef, OwnedElementRef, Status};

let validator = DirectoryValidatorBuilder::new("CUSTOM-D96A")
    .add_segment(
        OwnedSegmentDef::new_unchecked(
            "BGM".to_owned(),
            "Beginning of message".to_owned(),
            vec![
                OwnedElementRef::new_unchecked(1, "1001".to_owned(), Status::Conditional, 1),
                OwnedElementRef::new_unchecked(2, "1004".to_owned(), Status::Conditional, 1),
                OwnedElementRef::new_unchecked(3, "1225".to_owned(), Status::Conditional, 1),
            ],
        ),
    )
    .build();
```

Declaring a composite's components with `ElementRef::composite` (or
`OwnedElementRef::with_components`) does three things: it activates the
mandatory-**component** check, it caps the composite's arity (more components
than the directory declares is `E013`; fewer is normal, since conditional
components may be omitted), and it makes the composite's contents reachable by
identifier — see below. Definitions that declare no components behave exactly as
before, so this is inert for any directory table that has not opted in.

An `expected_components` hook, when set, still wins for that element: it is an
exact count, whereas declared components are an upper bound.

> **Scope note**: `DirectoryValidator` validates element presence and length within
> individual segments. It does not enforce full EDIFACT message grammar (conditional
> segment groups, repeat counts). Use `ProfileRulePack` for those cross-segment rules.

---

## Code-addressed element access

Positional accessors (`seg.element_str(4)`, `seg.component_str(1, 2)`) address
data by index. A transposed index reads the wrong data element and still
validates clean — silently. Given a `SegmentLayout` — implemented by both
`SegmentDefinition` (compile-time tables) and `OwnedSegmentDef` (runtime-loaded
definitions) — the same data can be addressed by its UN/EDIFACT identifier
instead, and a wrong reference becomes a directory lookup error:

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
static NAD: SegmentDefinition =
    SegmentDefinition::new("NAD", "Name and address", NAD_ELEMENTS);

let segments: Vec<_> = from_bytes(b"NAD+MS+9900112233445::293'").collect::<Result<Vec<_>, _>>()?;
let nad = &segments[0];

assert_eq!(nad.value_by_code(&NAD, "3039")?, Some("9900112233445"));
assert_eq!(nad.value_by_code(&NAD, "3055")?, Some("293"));

// DE 2380 belongs to DTM, not NAD — an error, not a wrong-but-quiet read.
assert!(nad.value_by_code(&NAD, "2380").is_err());
# Ok::<(), edifact_rs::EdifactError>(())
```

| Method | On | Returns |
|---|---|---|
| `value_by_code(layout, de)` | `Segment`, `BorrowedSegment`, `OwnedSegment` | `Result<Option<&str>, EdifactError>` |
| `span_by_code(layout, de)` | `Segment`, `BorrowedSegment`, `OwnedSegment` | `Result<Option<Span>, EdifactError>` — attach to a `ValidationIssue` with `with_span` |
| `element_by_code(layout, de)` | `Segment`, `BorrowedSegment`, `OwnedSegment` | `Result<Option<Element>, EdifactError>` — the enclosing composite when `de` names a component |
| `SegmentLayout::resolve_code(de)` | `SegmentDefinition`, `OwnedSegmentDef` | `Result<ElementPath, EdifactError>` — resolve once, then read many segments with `value_at` / `span_at` |

Three distinct failures are reported rather than silently tolerated:
`UnknownDataElement` (`E033`), `AmbiguousDataElement` (`E034`) when a directory
repeats a code, and `SegmentLayoutMismatch` (`E035`) when a layout is applied to
a segment with a different tag.

The derive has the same addressing under `#[edifact(layout = ...)]`, where
resolution happens at compile time — see
[Typed Derive](@/docs/typed-derive.md#element-by-identifier).

---

## Next steps

- [Profile Packs](@/docs/profile-packs.md) — composable business-rule bundles
- [Error Reference](@/docs/error-reference.md) — stable codes for `ValidationIssue::error_code`
- [Diagnostics](@/docs/diagnostics.md) — human-readable error rendering
