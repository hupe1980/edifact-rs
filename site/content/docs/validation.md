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
| `ValidationLayer` | Enum — `Envelope`, `Syntax`, `Charset`, `Structure`, `CodeList`, `Profile`; registration key and an issue's producing stage |
| `ValidationReport` | Aggregated result — `errors`, `warnings`, `infos` |
| `ValidationIssue` | A single finding — severity, message, rule ID, kind, layer, pack, location |
| `FindingKind` | What kind of fault — `Missing`, `NotPermitted`, `Code`, `Representation`, `Count`, `Placement`, … |
| `GroupPath` / `GroupStep` | Occurrence path to the group instance holding a finding, e.g. `SG4[0]/SG8[1]` |
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

let report = ctx.validate(&segs);
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
        .with_rule_fn(|_segs, _issues| {}),
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
    .bail_on_first_critical(true)  // skip later validators once a Critical appears
    .build();
```

### Built-in envelope validation

The `EnvelopeValidator` checks the whole ISO 9735-1 interchange structure —
`UNB`/`UNZ`, `UNG`/`UNE` groups, `UNH`/`UNT` messages, the control references
that tie each pair together, and the three control counts. Enable it with
`with_envelope_validation()`:

```rust
use edifact_rs::{ValidationContext, from_bytes};

# let segs: Vec<_> = from_bytes(b"UNB+UNOA:1+SENDER:1+RECEIVER:1+200101:1000+1'UNH+1+ORDERS:D:96A:UN'BGM+220+PO-1'UNT+3+1'UNZ+1+1'").collect::<Result<_,_>>()?;
let ctx = ValidationContext::builder()
    .with_envelope_validation()  // adds UNB/UNG/UNH/UNT/UNE/UNZ structure checks
    .build();
let report = ctx.validate(&segs);
# Ok::<(), edifact_rs::EdifactError>(())
```

Groups (`UNG`/`UNE`) are parsed and validated natively: nesting is rejected, the
`UNE` reference must repeat the `UNG` one, and `UNZ` DE 0036 is checked against
the *group* count rather than the message count — which is what Annex C.3.4
requires once groups are in play.

Two structural rules have no count to give them away and so are worth calling
out. An interchange with no message and no group (§7.2) raises `E042`, and a
message with nothing between its `UNH` and `UNT` (§7.4) raises `E043`. In both
cases the declared counts agree with the content, so every other check passes.

### Syntax validation

`SyntaxValidator` checks the rules that hold for **every** interchange from every
partner in every directory, so it needs no configuration:

| Rule | Source | Code |
|---|---|---|
| A segment carries at least one data element besides its tag | §7.6, §8.6 | `E046` |
| No data element value is made only of spaces | §9.4 | `E045` (warning) |
| No segment or composite ends in a separator with nothing after it | §8.8.2, §8.8.3 | `E051` (warning) |

All three are artefacts a hand-rolled writer produces by accident — an `ABC'`
where a conditional segment should have been dropped entirely, a fixed-width
source field copied across with its padding intact, or a loop that emits a
separator before checking whether another value follows. None trips a count
check, and all are rejected downstream.

The trailing-separator rule is narrower than it first looks. An **interior**
omission must *keep* its separator (§8.8.2), so `BGM+220++9'` is correct
and is not reported; and `BGM+'` is how §8.5 spells a mandatory segment with no
data to carry. Only a separator with nothing after it at all is a fault.

```rust
use edifact_rs::{ValidationContext, from_bytes};

let segments: Vec<_> = from_bytes(b"FTX+   'DTM'").collect::<Result<Vec<_>, _>>()?;

let report = ValidationContext::builder()
    .with_syntax_validation()
    .build()
    .validate(&segments);

assert_eq!(report.errors()[0].error_code(), Some("E046"));   // DTM has no data element
assert_eq!(report.warnings()[0].error_code(), Some("E045")); // FTX value is only spaces
# Ok::<(), edifact_rs::EdifactError>(())
```

Syntax findings carry `layer() == Some(ValidationLayer::Syntax)`; charset findings
from `with_charset_validation()` carry `ValidationLayer::Charset`. Both stages are
switched by the envelope switch and see the whole slice, like the envelope
validator — they are separate layers only so an issue says which stage found it.

Note that `DTM+'` is *not* `E046`: an empty data element is present, which is how
EDIFACT spells a mandatory segment that has no data to carry (§8.5).

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
let report = ctx.validate(&segs);
// Every issue in `report` will have `issue.message_ref == Some("MSG-42")`
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Collecting issues, and turning them into a `Result`

There is **one** validate method per shape. It always runs every enabled
validator and always returns a `ValidationReport` — validation reports, it does
not decide for you. When you want a `Result`, ask the report for one:

| Call | Returns |
|---|---|
| `ctx.validate(&segs)` | `ValidationReport` — every issue found |
| `ctx.validate(&segs).result()` | `Result<ValidationReport, ValidationReport>` — `Err` when any `Error`/`Critical` was found |
| `ctx.validate_with(&segs, &meta)` | as `validate`, with typed per-call metadata for rule closures |
| `ctx.validate_grouped(&tree, &segs)` | as `validate`, plus the group-aware pass |

Both parsing paths go into the same call: `&[OwnedSegment]` coerces to
`&[Segment<'_>]`, so a window straight off `message_windows_from_reader` needs no
conversion.

```rust
# use edifact_rs::{ValidationContext, from_bytes};
# let segs: Vec<_> = from_bytes(b"BGM+220+PO-4711+9'").collect::<Result<_,_>>()?;
# let ctx = ValidationContext::builder().build();

// Lenient: collect all issues even when errors are present
let report = ctx.validate(&segs);
if !report.is_valid() {
    for issue in report.errors() {
        eprintln!("error [{}]: {}", issue.error_code().unwrap_or("?"), issue.message);
    }
    for warn in report.warnings() {
        eprintln!("warn:  {}", warn.message);
    }
}

// `.result()` turns the same report into a Result when that is what you want.
match ctx.validate(&segs).result() {
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
let report = ctx.validate(&segs);

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
| `.with_element_index(n)` | `usize` | Element index (0-based; CONTRL DE 0098 is `n + 2`); stored as `u16`, a larger value leaves the field `None` |
| `.with_component_index(n)` | `usize` | Component index (0-based; CONTRL DE 0104 is `n + 1`); stored as `u16`, a larger value leaves the field `None` |
| `.with_error_code(code)` | `impl Into<Cow<'static, str>>` | Stable error code string (e.g. `"E007"`); a `&'static str` costs no allocation, and the field round-trips through `serde` |
| `.with_suggestion(text)` | `&str` | Human-friendly remediation hint |
| `.with_span(span)` | `Span` | Byte range of the issue in the input — the only positional field; read `issue.span.map(\|s\| s.start)` or `issue.start_offset()` for the start alone |
| `.with_segment_occurrence(n)` | `usize` | Zero-based occurrence among segments with the same tag; stored as `u32` |
| `.with_segment_position(n)` | `usize` | One-based position of the segment in its message, `UNH` = 1 (CONTRL DE 0096); stored as `u32` |
| `.with_segment_group(name)` | `impl Into<Arc<str>>` | Name of the innermost segment group (e.g. `"SG5"`) — set automatically by group-scoped rules |
| `.with_group_path(path)` | `GroupPath` | Occurrence path to the group instance (e.g. `SG4[0]/SG8[1]`) — set automatically by group-scoped rules; also fills an empty `segment_group` |
| `.with_layer(layer)` | `ValidationLayer` | Stage that produced the issue — filled automatically from the registration layer when left unset |
| `.with_kind(kind)` | `FindingKind` | What kind of fault this is |
| `.with_pack(name)` | `impl Into<Arc<str>>` | Pack that produced the issue — set automatically by `ProfileRulePack` |
| `.with_message_ref(r)` | `impl Into<String>` | `UNH` reference (DE 0062) — usually set automatically via `ValidationContextBuilder::with_message_ref` |
| `.with_context_entry(k, v)` | `(impl Into<String>, impl Into<String>)` | Insert a single key-value pair into the domain metadata map |
| `.with_context_entries(iter)` | `impl IntoIterator<Item=(K,V)>` | Bulk-insert domain metadata; duplicate keys overwrite |

Positional builders take the `usize` you already hold and never clamp: a value
that does not fit the field leaves it `None`, because a clamped index points at a
plausible but wrong segment. `segment_occurrence` and `segment_position` are
different coordinates — the 70 000th `QTY` of an `MSCONS` is occurrence `69_999`,
and if it follows `UNH`, `BGM` and `LIN` its position is `70_003`:

```rust
use edifact_rs::{FindingKind, ValidationIssue, ValidationLayer, ValidationSeverity};

let index_in_message: usize = 70_002; // zero-based index into a slice starting at UNH
let issue = ValidationIssue::new(ValidationSeverity::Error, "QTY qualifier must be 220")
    .with_segment("QTY")
    .with_segment_occurrence(69_999)
    .with_segment_position(index_in_message + 1)
    .with_element_index(0)
    .with_component_index(0);
assert_eq!(issue.segment_position(), Some(70_003));
assert_eq!(issue.with_element_index(70_000).element_index(), None); // not 65_535
```

### Classifying findings: layer, kind and pack

Every issue says what kind of fault it is, which stage found it and, for profile
packs, which pack — as structured fields, so routing never parses a rule id:

| Field | Type | Filled by |
|---|---|---|
| `kind()` | `Option<FindingKind>` | Every built-in validator; errors converted to issues use `FindingKind::for_error`. A caller's issue without a kind stays `None`. |
| `layer()` | `Option<ValidationLayer>` | Built-in validators set their own; `ValidationContext` fills the registration layer into every issue left without one. Explicit wins. |
| `pack()` | `Option<&str>` | `ProfileRulePack` stamps its name on every issue it emits without one. |
| `group_path()` | `Option<&GroupPath>` | The group pass of `validate_grouped`, for issues from group rules. |

The built-in classification:

| Validator | Layer | Kind |
|---|---|---|
| `EnvelopeValidator` | `Envelope` | `FindingKind::for_error` (`ControlMismatch`, `Envelope`, `Duplicate`, …) |
| `SyntaxValidator` | `Syntax` | `Representation` (`E045`), `Syntax` (`E046`, `E051`) |
| `CharsetValidator` | `Charset` | `Charset` |
| `DirectoryValidator` | `CodeList` for `E014`, `Structure` otherwise | `FindingKind::for_error`; missing required segment → `Missing`, out-of-order segment → `Placement` |
| `ProfileRulePack` helpers | `Profile` (from the context) | `require_segment*` → `Missing`, `forbid_segment*` → `NotPermitted`, `require_qualifier*` → `Code` |

```rust
use edifact_rs::{FindingKind, ProfileRulePack, ValidationContext, ValidationLayer, from_bytes};

let segs: Vec<_> = from_bytes(b"UNH+1+ORDERS:D:96A:UN'BGM+220'DTM+137:20240101:102'UNT+4+1'")
    .collect::<Result<_, _>>()?;
let report = ValidationContext::builder()
    .with_syntax_validation()
    .with_profile_pack(ProfileRulePack::new("AHB-ORDERS").forbid_segment("DTM", "AHB-DTM-X"))
    .build()
    .validate(&segs);

let issue = &report.errors()[0];
assert_eq!(issue.layer(), Some(ValidationLayer::Profile));
assert_eq!(issue.kind(), Some(FindingKind::NotPermitted));
assert_eq!(issue.pack(), Some("AHB-ORDERS"));
assert_eq!(issue.segment_position(), Some(3)); // UNH = 1
// Syntax, envelope and charset findings answer with a CONTRL; the rest do not.
let for_contrl = report.iter_issues().filter(|i| {
    matches!(
        i.layer(),
        Some(ValidationLayer::Envelope | ValidationLayer::Syntax | ValidationLayer::Charset)
    )
});
assert_eq!(for_contrl.count(), 0);
# Ok::<(), edifact_rs::EdifactError>(())
```

With the `serde` feature, optional fields serialise only when set and read back
as `None` when absent; an unknown kind reads as `FindingKind::Other`.

### Rule ID naming convention

`rule_id` is an identifier: it names a rule so a report can be filtered and a
rule overridden. Layer, kind and pack have their own fields (above). A
structured, namespaced id still helps people reading a report:

```text
"<PACK>-<SCOPE>-<TAG>-<STATUS>"
 ^^^^^^                          — the pack / profile that owns the rule
        ^^^^^^^                  — a process identifier, group name, or other discriminator
                ^^^^^            — the affected segment
                      ^^^^^^^^   — M / C / … status or short discriminator
```

Example: `"PROFILE-4711-BGM-M"` — pack `PROFILE`, scope `4711`, segment `BGM`,
mandatory (`M`). Domain detail that has no structured field, such as the scope,
can be recovered with a split:

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
            .with_rule_fn(|segs, issues| {
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
    let borrowed: Vec<_> = window.segments.iter().map(|s| s.clone()).collect();
    let report = ctx.validate(&borrowed);
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
message. First define a `[GroupDef]` schema, build a `SegmentGroupIndexed` tree
with `group_segments_indexed`, then pass the tree to `validate_grouped`:

```rust
use edifact_rs::{
    ValidationContext, ProfileRulePack,
    group::{GroupDef, group_segments_indexed},
    from_bytes,
};

// Schema: SG5 starts at LIN and contains an SG6 sub-group starting at QTY.
static SG6: &[GroupDef] = &[GroupDef::new("SG6", "QTY")];
static SCHEMA: &[GroupDef] = &[GroupDef::with_children("SG5", "LIN", SG6)];

let segs: Vec<_> = from_bytes(
    b"UNH+1+ORDERS:D:96A:UN'\
      LIN+1'QTY+21:10'\
      LIN+2'\
      UNT+5+1'"
).collect::<Result<_, _>>()?;

// Build the indexed group tree (O(n × reachable places), no segment clones):
let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

let pack = ProfileRulePack::new("ORDERS")
    .for_message_type("ORDERS")
    // Require QTY inside every SG5 occurrence:
    .require_segment_in_group("SG5", "QTY", "ORDERS-SG5-QTY-M");

let ctx = ValidationContext::builder()
    .with_profile_pack(pack)
    .build();

// `validate_grouped` runs the flat pass, then the group pass:
let report = ctx.validate_grouped(&tree, &segs);
println!("{} error(s)", report.errors().len());

// The second SG5 (LIN+2) has no QTY; the issue says which occurrence.
let issue = &report.errors()[0];
assert_eq!(issue.segment_group(), Some("SG5"));
assert_eq!(issue.group_path().map(|p| p.to_string()).as_deref(), Some("SG5[1]"));
# Ok::<(), edifact_rs::EdifactError>(())
```

Group-rule issues carry a `group_path` (see
[Profile Packs](@/docs/profile-packs.md)); `render_deterministic` prints it with
the segment tag as `[path=SG4[0]/SG8[1]/RFF]`.

Segments read from a reader go into the same call unchanged, and `.result()`
converts the report exactly as it does for the flat pass.

### Schemas built at runtime

`GroupDef::new` and `GroupDef::with_children` are `const` over `'static` data,
so a `static` table costs nothing. A schema read out of a MIG at startup goes
through `GroupDef::builder`, which owns its strings (shared behind `Arc`, so
cloning a group is O(1)) — no borrowed arena, no `Box::leak`:

```rust
use edifact_rs::group::{GroupDef, SegmentPlace, group_segments_indexed};
use edifact_rs::from_bytes;

// Stand-in for names read out of a MIG file at startup.
let names: Vec<String> = vec!["SG5".into(), "LIN".into()];
let schema = vec![GroupDef::builder(names[0].as_str(), SegmentPlace::owned(names[1].as_str())).build()?];
drop(names); // the schema owns what it needs

let segs: Vec<_> = from_bytes(b"UNH+1+ORDERS:D:96A:UN'LIN+1'UNT+3+1'")
    .collect::<Result<_, _>>()?;
let tree = group_segments_indexed(&segs, &schema, "ROOT");

assert_eq!(tree.children[0].definition, "SG5");
# Ok::<(), edifact_rs::EdifactError>(())
```

A nested child slice in a `static` must be a named `static` of its own
(`static SG6: &[GroupDef] = …` above): written inline as
`with_children("SG5", "LIN", &[GroupDef::new(..)])` it fails with `E0493`,
because `GroupDef` can own shared data.

### Open and ordered groups

A group that declares only its trigger and child groups is **open**: every
segment that triggers nothing is a direct segment of it. That is all the
`const` constructors build.

A group built with `GroupDef::builder` that declares member segments with
`.segment(..)` — or is marked `.ordered()` — is **ordered**, the way an
implementation guide (MIG) lists it: trigger first, then member places and
child groups in guide order. Its entries are walked front to back. Each
`SegmentPlace` and group can carry:

- `min` / `max` occurrences per parent instance (default 0 / unbounded);
- an `id` (e.g. the MIG segment number), returned with placements and findings;
- `Discriminator`s on a segment place, all of which must match:
  `one_of(element, component, codes)` (strict: an absent value or another code
  does not match), `.optional()` (an absent value cannot rule the place out),
  `.accept_unclaimed()` (a code that no place of the same tag admits there still
  matches — a code fault, not a misplacement), and `not_used(element, component)`
  (a value there rules the place out). Positions are zero-based and read the
  element's first repetition.

A segment that fits no entry of an ordered group is **unplaced**: skipped,
recorded on the instance it was skipped in (`SegmentGroupIndexed::unplaced`,
excluded from `direct_segment_indices`), with a reason — `OutOfOrder` (it fits a
place already passed), `NotHere` (it fits a place elsewhere in the schema) or
`Unknown`. It is never attached to a wrong group.

Shared tags are where this matters. With `SG4(IDE){SG6(RFF), SG8(SEQ)}` all
open, the `RFF+Z19` that belongs to `SG8` opens a second `SG6` — nothing says
otherwise, and open schemas build that tree. Declaring `SG8`'s members
fixes it:

```rust
use edifact_rs::group::{GroupDef, SegmentPlace, group_segments_indexed};

let sg8 = GroupDef::builder("SG8", SegmentPlace::new("SEQ"))
    .segment(SegmentPlace::new("RFF"))
    .segment(SegmentPlace::new("CCI"))
    .build()?;
let schema = [GroupDef::builder("SG4", SegmentPlace::new("IDE"))
    .group(GroupDef::new("SG6", "RFF"))
    .group(sg8)
    .build()?];

let segments: Vec<_> = edifact_rs::from_bytes(b"IDE+24+X'RFF+Z13:1'SEQ+Z01'RFF+Z19:ABC'CCI+Z30'")
    .collect::<Result<_, _>>()?;
let tree = group_segments_indexed(&segments, &schema, "ROOT");
let sg4 = &tree.children[0];
assert_eq!(
    sg4.children.iter().map(|g| (g.definition, g.total_span.clone())).collect::<Vec<_>>(),
    [("SG6", 1..2), ("SG8", 2..5)],
);
# Ok::<(), edifact_rs::EdifactError>(())
```

### Placements and findings: `GroupSchema`

`GroupSchema::new(root, groups)` (open root) or `GroupSchema::ordered(root,
entries)` (ordered root — declare `UNT` as its last entry, or it is unplaced)
checks a schema once — tags are three upper-case letters, `min <= max`,
`max >= 1`, `one_of` lists a code, at most 64 nested groups; otherwise
`E071 InvalidGroupSchema` — and `resolve(&segments)` returns a
`GroupResolution`: the tree, `placement(i)` (the place, group and occurrence of
segment `i`), `unplaced()`, `group_path(i)`, and `findings(&segments)`:

| Finding | Code | Kind |
|---|---|---|
| unplaced segment | `E068 SegmentNotPlaced` | `placement` |
| a place beyond its `max` | `E069 TooManySegmentOccurrences` | `count` |
| a group beyond its `max` | `E070 TooManyGroupOccurrences` | `count` |
| a place or group below its `min` | `E015 MissingSegment` | `missing` |

Every finding has layer `structure`, the `group_path` of its instance, the
segment's span and — when the slice starts at `UNH` — its `segment_position`; a
place `id` is in the context under `place`. `PlacementValidator` runs the same
resolution inside a `ValidationContext`:

```rust
use std::sync::Arc;
use edifact_rs::group::{Discriminator, Entry, GroupDef, GroupSchema, SegmentPlace};
use edifact_rs::{FindingKind, PlacementValidator, ValidationContext, ValidationLayer};

let malo = |code: &str, id: &str| {
    GroupDef::builder("SG5", SegmentPlace::new("LOC").with_id(id).when(Discriminator::one_of(0, 0, [code])))
        .ordered()
        .build()
};
let sg4 = GroupDef::builder("SG4", SegmentPlace::new("IDE"))
    .group(malo("Z16", "00047")?)
    .group(malo("Z22", "00048")?)
    .ordered()
    .build()?;
let schema = GroupSchema::ordered(
    "UTILMD",
    [
        Entry::Segment(SegmentPlace::new("UNH").with_min(1).with_max(1)),
        Entry::Group(sg4),
        Entry::Segment(SegmentPlace::new("UNT").with_min(1).with_max(1)),
    ],
)?;

let segments: Vec<_> = edifact_rs::from_bytes(
    b"UNH+1+UTILMD:D:11A:UN'IDE+24+V1'LOC+Z22+1'LOC+Z16+2'UNT+5+1'",
)
.collect::<Result<_, _>>()?;

let resolution = schema.resolve(&segments);
assert_eq!(resolution.placement(2).and_then(|p| p.place.id()), Some("00048"));

let report = ValidationContext::builder()
    .with_validator(ValidationLayer::Structure, PlacementValidator::new(Arc::new(schema)))
    .build()
    .validate(&segments);
// LOC+Z16 after LOC+Z22 is out of guide order.
let issue = &report.errors()[0];
assert_eq!(issue.kind(), Some(FindingKind::Placement));
assert_eq!(issue.error_code(), Some("E068"));
assert_eq!(issue.segment_position(), Some(4));
# Ok::<(), edifact_rs::EdifactError>(())
```

Resolution is `O(n × reachable places)` time and `O(n)` extra memory, with no
recursion over the input; `direct_segment_indices` is linear in the span plus
the number of children.

### How a tag is resolved

For each segment, innermost open group first:

1. **The innermost group's own candidates.** Open: its child-group triggers, in
   declaration order. Ordered: its current entry, then later entries — the first
   that matches wins, except that a place already at its `max` yields to a later
   entry that also matches.
2. **The reachable places of each enclosing group, innermost first.** Open: all
   its child-group triggers. Ordered: its current and later entries. A match
   closes the groups inside it.
3. Otherwise the segment is a direct segment of an open group, or unplaced in an
   ordered one.

Step 1 comes first because the same trigger tag routinely appears at more than
one level. `UTILMD` names its message-level parties in SG2 and a Vorgang's
parties in SG12 inside SG4 — both triggered by `NAD`:

```rust
use edifact_rs::group::{GroupDef, group_segments_indexed};

static SG12: &[GroupDef] = &[GroupDef::new("SG12", "NAD")];
static SCHEMA: &[GroupDef] = &[
    GroupDef::new("SG2", "NAD"),
    GroupDef::with_children("SG4", "IDE", SG12),
];

let segments: Vec<_> = edifact_rs::from_bytes(
    b"BGM+E01'NAD+MS+SENDER'NAD+MR+RECEIVER'IDE+24+V1'NAD+Z09+KUNDE'DTM+92:20260101:102'",
)
.collect::<Result<Vec<_>, _>>()?;

let tree = group_segments_indexed(&segments, SCHEMA, "ROOT");

// The two message-level parties are SG2 …
assert_eq!(
    tree.children.iter().map(|g| g.definition).collect::<Vec<_>>(),
    ["SG2", "SG2", "SG4"],
);
// … and the party inside the Vorgang nests as SG12, rather than reopening SG2.
let sg4 = tree.children.last().unwrap();
assert_eq!(sg4.children[0].definition, "SG12");
# Ok::<(), edifact_rs::EdifactError>(())
```

A group ends at the first segment the current branch cannot consume, never at
one an outer branch could also have consumed — the other order would make SG12
unreachable from any input.

A group's own trigger is not among its candidates, so a repeated trigger opens
the **next occurrence** rather than nesting.

### Reading groups back

The tree stores index ranges, not copies. `segments(&all)` resolves a group
against the slice it was built from, `descendants()` walks the subtree in
document order, and `find(name)` yields every group with a given name at any
depth:

```rust
use edifact_rs::group::{GroupDef, group_segments_indexed};

static SG12: &[GroupDef] = &[GroupDef::new("SG12", "NAD")];
static SCHEMA: &[GroupDef] = &[
    GroupDef::new("SG2", "NAD"),
    GroupDef::with_children("SG4", "IDE", SG12),
];

let segments: Vec<_> = edifact_rs::from_bytes(
    b"NAD+MS+SENDER'IDE+24+V1'NAD+Z09+KUNDE'IDE+24+V2'NAD+VY+PARTY'",
)
.collect::<Result<Vec<_>, _>>()?;
let tree = group_segments_indexed(&segments, SCHEMA, "ROOT");

// Every SG12 in the message, whichever Vorgang it belongs to.
let parties: Vec<&str> = tree
    .find("SG12")
    .filter_map(|group| group.segments(&segments).first())
    .filter_map(|nad| nad.element_str(0))
    .collect();
assert_eq!(parties, ["Z09", "VY"]);
# Ok::<(), edifact_rs::EdifactError>(())
```

### What a group actually spans

In an open group, a tag that triggers nothing is a direct segment of whichever
group is open when it arrives. Nothing stops the last group of a message at `UNT`, so the trailer
lands inside whichever group ran last.

`MessageWindow::segments` deliberately includes `UNH` and `UNT` so that
envelope-aware consumers can read them; pass `MessageWindow::body()` — which is
exactly the segments between them — when the group boundaries matter.

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

## Data element representations

Every UN/EDIFACT directory prints a representation beside each data element, and
it is what partners actually reject on: a sender identification of 40 characters
where the standard says `an..35` is refused at the far end, long after it was
sent. Attach it with `with_repr` and `DirectoryValidator` checks it:

```rust
use edifact_rs::{ComponentRef, ElementRef, Repr, SegmentDefinition, Status};

static C507: &[ComponentRef] = &[
    ComponentRef::new(1, "2005", Status::Mandatory).with_repr(Repr::an_up_to(3)),
    ComponentRef::new(2, "2380", Status::Conditional).with_repr(Repr::an_up_to(35)),
    ComponentRef::new(3, "2379", Status::Conditional).with_repr(Repr::an_up_to(3)),
];
static DTM_ELEMENTS: &[ElementRef] =
    &[ElementRef::composite(1, "C507", Status::Mandatory, 1, C507)];
static DTM: SegmentDefinition = SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);
```

Three things follow, and each maps onto a `CONTRL` code:

| Violation | Code | `CONTRL` |
|---|---|---|
| Wrong character class | [`E048`](@/docs/error-reference.md#e048-invalidcharactertype) | 37 |
| Longer than the maximum | [`E049`](@/docs/error-reference.md#e049-dataelementtoolong) | 39 |
| Shorter than a fixed length | [`E050`](@/docs/error-reference.md#e050-dataelementtooshort) | 40 |

Positions with no declared representation are simply not checked, so a partial
table stays useful instead of becoming a source of false findings.

### Length is counted in characters

ISO 9735-1 §6: "one graphic character shall be counted as one character,
irrespective of the number of bytes/octets required to encode it." A `UNOC` `ü`
is one character, not the two bytes UTF-8 needs for it. The release character is
excluded too (§5), which is automatic here because release sequences are resolved
before validation.

A **numeric** value excludes more. §10: "the length of a numeric data element
value shall not include the minus sign (-), the decimal mark (. or ,), or the
exponent mark (E or e) and its exponent" — so `-123.45` is five characters and
fits `n..5`.

### What `n` admits

§10 takes ISO 6093's forms and subtracts: "The space character and plus sign
shall not be allowed", and "when a decimal mark is transferred, there shall be at
least one digit after the decimal mark". So `2`, `2.00`, `0.5`, `.5` and `-3E4`
are numeric; `1.`, `.`, `+1` and `1 2` are not.

### The shipped service tables carry theirs

`edifact_rs::service` declares the representations from ISO 9735-1 Annex C, so
`UNZ+abc+IC1'` (DE 0036 is `n..6`) and a 20-character `UNB` DE 0020 (`an..14`)
are rejected with no directory involved.

### The one position that changed between syntax versions

`S004` DE 0017, the date of preparation, is the **only** place in the service
directory where version 4 is not a superset of version 3. Version 3 transfers
`YYMMDD` (`n6`); version 4 widened it to `CCYYMMDD` (`n8`) to be year-2000
correct.

Collapsing the two into `n..8` would validate *neither* version: a six-digit date
would pass in a version 4 interchange, and a seven-digit one in either. So the
table declares both, and the checker picks by the version in `UNB` S001 DE 0002:

```rust
use edifact_rs::{ComponentRef, Repr, Status};

const DATE: ComponentRef = ComponentRef::new(1, "0017", Status::Mandatory)
    .with_repr_by_syntax_version(Repr::n(6), Repr::n(8));
```

`UNB+UNOA:3+…+200101:0900+…` passes and `…+20200101:0900+…` is rejected; under
`UNOC:4` it is the other way round.

When the version cannot be determined — validating a bare message window, which
carries no `UNB` — **both** forms are accepted, because guessing would reject
conformant data from whichever version was guessed against. A length that is
neither, such as seven digits, is still rejected.

### Suppression of insignificant characters

With a representation attached, `DirectoryValidator` also applies §9.2: leading
zeroes must be suppressed in a **variable-length** numeric value, trailing spaces
in a variable-length text one. Both raise
[`E053`](@/docs/error-reference.md#e053-insignificantcharacters) as a warning.

Two exemptions come straight from the clause. "Nevertheless, a single zero before
a decimal mark is allowed", so `0.5` passes and `00.5` does not. And a
*fixed*-length element is exempt entirely — `n3` is zero-padded by design.

### Occurrence limits

`ElementRef`'s `max_repeat` is enforced as well, raising
[`E047`](@/docs/error-reference.md#e047-toomanyrepetitions) (`CONTRL` 35) when a
repeating data element occurs more often than the definition allows.

---

## Auditing a hand-written layout

Authoring a `SegmentDefinition` by hand has a silent failure mode. A layout that
disagrees with the wire resolves `value_by_code` to the **wrong component**,
returns a plausible value, and every test still passes — the definition is the
only thing in the program that says what the positions mean, so nothing inside
the program can contradict it.

`SegmentLayout::audit` breaks that circle by pointing the definition at real
messages:

```rust
use edifact_rs::{ComponentRef, ElementRef, SegmentDefinition, SegmentLayout, Status, from_bytes};

// A hand-authored C507 that stops one component short of the directory.
static C507: &[ComponentRef] = &[
    ComponentRef::new(1, "2005", Status::Mandatory),
    ComponentRef::new(2, "2380", Status::Conditional),
];
static DTM_ELEMENTS: &[ElementRef] =
    &[ElementRef::composite(1, "C507", Status::Mandatory, 1, C507)];
static DTM: SegmentDefinition = SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);

let corpus: Vec<_> = from_bytes(b"DTM+137:20260101:102'").collect::<Result<Vec<_>, _>>()?;
let audit = DTM.audit(&corpus);

// The format qualifier `102` has nowhere to go — the layout is short.
assert!(audit.has_contradictions());
# Ok::<(), edifact_rs::EdifactError>(())
```

Three kinds of finding come back, and the distinction between them is the point:

| Finding | Means |
|---|---|
| `UndeclaredElement` / `UndeclaredComponent` | The wire carries a value the layout has no slot for — the layout is **wrong**. |
| `MandatoryNeverPopulated` | A slot the layout calls mandatory is empty everywhere — the status or the position is **wrong**. |
| `NeverObserved` | Nothing in the corpus reaches this slot — the corpus **cannot confirm it**. |

`NeverObserved` is not a defect, and `has_contradictions` deliberately excludes
it. It is the honest answer to "does my definition match the directory?" when the
fixtures are too thin to say, and it names exactly which positions still need a
human to check them against the directory — or a fixture that reaches them:

```rust
# use edifact_rs::{ComponentRef, ElementRef, SegmentDefinition, SegmentLayout, Status, from_bytes};
# static C507: &[ComponentRef] = &[
#     ComponentRef::new(1, "2005", Status::Mandatory),
#     ComponentRef::new(2, "2380", Status::Conditional),
#     ComponentRef::new(3, "2379", Status::Conditional),
# ];
# static DTM_ELEMENTS: &[ElementRef] =
#     &[ElementRef::composite(1, "C507", Status::Mandatory, 1, C507)];
# static DTM: SegmentDefinition = SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);
// No fixture in this corpus carries a format qualifier.
let corpus: Vec<_> = from_bytes(b"DTM+137:20260101'").collect::<Result<Vec<_>, _>>()?;
let audit = DTM.audit(&corpus);

assert!(!audit.has_contradictions());          // nothing is disproved …
let pending: Vec<&str> = audit.unconfirmed().map(|s| s.data_element.as_str()).collect();
assert_eq!(pending, ["2379"]);                 // … but 2379 is still unverified
# Ok::<(), edifact_rs::EdifactError>(())
```

The assertion to put in a test is `!audit.has_contradictions()`, together with
`audit.segments_examined() > 0` — an audit over a corpus with none of the segment
proves nothing, and a green test that silently proved nothing is the failure this
whole facility exists to prevent. `LayoutAudit` also implements `Display`, so
printing it gives a report you can paste into a review.

Findings are deduplicated per position, so a corpus of hundreds of fixtures
reports each disagreement once rather than once per message.

### Auditing a whole directory

`audit_directory` answers the question a hand-authored directory actually raises
— *which of my definitions does this corpus disprove, and which can it not speak
to?* — in one call:

```rust
use edifact_rs::{audit_directory, from_bytes, service};

let corpus: Vec<_> = from_bytes(
    b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'UNT+2+M1'UNZ+1+IC1'",
)
.collect::<Result<Vec<_>, _>>()?;

for audit in audit_directory(service::lookup, &corpus) {
    assert!(!audit.has_contradictions(), "{audit}");
}
# Ok::<(), edifact_rs::EdifactError>(())
```

Only tags the corpus contains are audited. A definition the fixtures never
exercise would produce nothing but `NeverObserved` entries and bury the findings
that matter — ask `SegmentLayout::audit` directly for those.

### Mandatory components of an absent composite

ISO 9735-1 §8.7 makes a mandatory component required "**if** the composite data
element is present", not unconditionally — and the audit follows that. `UNB` S005
component 1 (DE 0022, the recipient password) is mandatory *inside* a composite
that is itself conditional, so a conformant `UNB` with no password does not
violate its own layout. It comes back as `NeverObserved` instead, which is the
truthful answer: the corpus has nothing to say about it.

Once the composite *is* present, its mandatory components are required again, and
a missing one is a contradiction.

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
| `value_by_code(layout, de)` | `Segment` (and therefore `OwnedSegment`) | `Result<Option<&str>, EdifactError>` |
| `span_by_code(layout, de)` | `Segment` | `Result<Option<Span>, EdifactError>` — attach to a `ValidationIssue` with `with_span` |
| `element_by_code(layout, de)` | `Segment` | `Result<Option<&Element>, EdifactError>` — the enclosing composite when `de` names a component |
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
