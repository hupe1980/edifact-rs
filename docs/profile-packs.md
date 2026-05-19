# Profile Packs 📦

`ProfileRulePack` is the primary extension point for downstream MIG/profile crates
and trading-partner–specific validation logic. Packs are authored using only public
APIs, can be composed from multiple sources, and plugged into any `ValidationContext`.

---

## What is a profile pack?

A **profile rule pack** is a named collection of validation rules scoped to one or
more EDIFACT message types. Each rule is a closure that receives a segment slice and
returns `Some(ValidationIssue)` if the rule is violated, or `None` if it passes.

Packs can be:
- **Authored** in downstream crates (your application or a MIG library)
- **Merged** from multiple sources into a combined pack
- **Filtered** by rule ID prefix for fine-grained reporting
- **Scoped** to specific message types (`ORDERS`, `INVOIC`, …)

---

## Creating a pack

```rust
use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity};

let pack = ProfileRulePack::builder("ORDERS-RULES")
    .for_message_type("ORDERS")
    .with_rule_fn(|segments| {
        // Find the BGM segment
        let bgm = segments.iter().find(|s| s.tag == "BGM")?;
        let code = bgm.get_element(0)?.get_component(0)?;

        // Rule: only codes 220 (original) and 231 (quote) are accepted
        (!matches!(code, "220" | "231")).then(|| {
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("unsupported BGM document code '{code}'"),
            )
            .with_rule_id("ORDERS-BGM-P001")
            .with_segment("BGM")
            .with_element_index(0)
            .with_suggestion("Use document code 220 (original order) or 231 (quotation)")
        })
    })
    .with_rule_fn(|segments| {
        // Rule: a buyer NAD is mandatory
        let has_buyer = segments
            .iter()
            .filter(|s| s.tag == "NAD")
            .any(|s| s.element_str(0) == Some("BY"));
        (!has_buyer).then(|| {
            ValidationIssue::new(ValidationSeverity::Error, "buyer NAD+BY is required")
                .with_rule_id("ORDERS-NAD-P001")
                .with_segment("NAD")
                .with_element_index(0)
        })
    });
```

---

## Plugging into `ValidationContext`

```rust
use edifact_rs::{ValidationContext, from_bytes};

# use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity};
# let pack = ProfileRulePack::builder("ORDERS-RULES").for_message_type("ORDERS");
let segs: Vec<_> = from_bytes(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO-4711+9'UNT+3+1'")
    .collect::<Result<_, _>>()?;

let report = ValidationContext::builder()
    .with_profile_pack(pack)
    .build()
    .validate_lenient(&segs);

println!("{} error(s)", report.errors.len());
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Merging packs

Use `.merge(other)` to combine packs from multiple sources into a single pack that
can be registered once:

```rust
use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity};

let document_pack = ProfileRulePack::builder("ORDERS-DOCUMENT")
    .for_message_type("ORDERS")
    .with_rule_fn(|segs| {
        // document code rules …
        None
    });

let reference_pack = ProfileRulePack::builder("ORDERS-REFERENCE")
    .for_message_type("ORDERS")
    .with_rule_fn(|segs| {
        // reference rules …
        None
    });

let partner_pack = ProfileRulePack::builder("ACME-PARTNER")
    .for_message_type("ORDERS")
    .with_rule_fn(|segs| {
        // trading-partner–specific rules …
        None
    });

// Merge all three into one combined pack:
let combined = ProfileRulePack::builder("ORDERS-COMBINED")
    .merge(document_pack)
    .merge(reference_pack)
    .merge(partner_pack);
```

> **Note**: `.merge(other)` consumes both packs and returns a new one. The combined
> pack applies rules from all merged packs in order.

---

## Scoping by message type

Use `.for_message_type("TYPE")` to restrict a pack to a specific message type.
Packs check the `UNH` segment element 1 component 0 (the message identifier):

```rust
# use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity};

// This pack's rules run ONLY when the message type is "INVOIC"
let invoic_pack = ProfileRulePack::builder("INVOIC-RULES")
    .for_message_type("INVOIC")
    .with_rule_fn(|segs| {
        // Will not run for ORDERS, UTILMD, etc.
        None
    });

// Without for_message_type, rules run for all message types:
let universal_pack = ProfileRulePack::builder("UNIVERSAL")
    .with_rule_fn(|segs| None);
```

Call `.for_message_type` multiple times to include multiple types:

```rust
# use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity};
let multi_pack = ProfileRulePack::builder("TRADE-DOCS")
    .for_message_type("ORDERS")
    .for_message_type("ORDRSP")
    .for_message_type("INVOIC")
    .with_rule_fn(|segs| None);
```

---

## Filtering reports by rule ID prefix

Assign stable, namespaced rule IDs and filter them independently:

```rust
# use edifact_rs::{ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity, from_bytes};
let segs: Vec<_> = from_bytes(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO-4711+9'UNT+3+1'")
    .collect::<Result<_, _>>()?;

let pack = ProfileRulePack::builder("ORDERS")
    .for_message_type("ORDERS")
    .with_rule_fn(|segs| {
        Some(ValidationIssue::new(ValidationSeverity::Warning, "demo warning")
            .with_rule_id("ORDERS-DOC-P001"))
    })
    .with_rule_fn(|segs| {
        Some(ValidationIssue::new(ValidationSeverity::Info, "demo info")
            .with_rule_id("ORDERS-REF-P001"))
    });

let report = ValidationContext::builder()
    .with_profile_pack(pack)
    .build()
    .validate_lenient(&segs);

// All findings:
println!("total: {}", report.total_issues());

// Only document-level findings:
let doc_issues = report.filter_by_rule_prefix("ORDERS-DOC-");
println!("document issues: {}", doc_issues.total_issues());

// Only reference-level findings:
let ref_issues = report.filter_by_rule_prefix("ORDERS-REF-");
println!("reference issues: {}", ref_issues.total_issues());

// Exact rule ID lookup (returns a lazy iterator; use count() or collect()):
let p001_count = report.issues_for_rule_id("ORDERS-DOC-P001").count();
println!("ORDERS-DOC-P001 findings: {p001_count}");
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Mapping violations to application errors

Instead of returning `ValidationIssue` to your application layer, map rule IDs to
your own domain error type:

```rust
use edifact_rs::{ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity, from_bytes};

#[derive(Debug)]
enum TradeError {
    UnsupportedDocumentCode(String),
    MissingBuyerNad,
}

fn validate_orders(input: &[u8]) -> Result<(), Vec<TradeError>> {
    let segs: Vec<_> = from_bytes(input)
        .collect::<Result<_, _>>()
        .map_err(|_| vec![])?;

    let pack = ProfileRulePack::builder("ORDERS")
        .for_message_type("ORDERS")
        .with_rule_fn(|segs| {
            let code = segs.iter()
                .find(|s| s.tag == "BGM")
                .and_then(|s| s.element_str(0))?;
            (!matches!(code, "220" | "231")).then(|| {
                ValidationIssue::new(ValidationSeverity::Error, format!("code {code}"))
                    .with_rule_id("ORDERS-DOC-P001")
            })
        })
        .with_rule_fn(|segs| {
            let has_buyer = segs.iter()
                .filter(|s| s.tag == "NAD")
                .any(|s| s.element_str(0) == Some("BY"));
            (!has_buyer).then(|| {
                ValidationIssue::new(ValidationSeverity::Error, "missing NAD+BY")
                    .with_rule_id("ORDERS-NAD-P001")
            })
        });

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_lenient(&segs);

    if report.is_valid() {
        return Ok(());
    }

    let errors: Vec<TradeError> = report
        .errors
        .iter()
        .filter_map(|issue| match issue.rule_id.as_deref() {
            Some("ORDERS-DOC-P001") => {
                Some(TradeError::UnsupportedDocumentCode(issue.message.clone()))
            }
            Some("ORDERS-NAD-P001") => Some(TradeError::MissingBuyerNad),
            _ => None,
        })
        .collect();

    Err(errors)
}
```

See [`cookbook_profile_error_mapping.rs`](../crates/edifact-rs/examples/cookbook_profile_error_mapping.rs)
for a complete example with a custom `Validator` implementation alongside profile packs.

---

## Implementing `ProfileRule` as a struct

For rules with shared state or complex initialization, implement the `ProfileRule`
trait directly:

```rust
use edifact_rs::{ProfileRule, ProfileRulePack, ValidationIssue, ValidationSeverity, Segment};

struct AllowedCodesRule {
    allowed: Vec<String>,
}

impl ProfileRule for AllowedCodesRule {
    fn evaluate(&self, segments: &[Segment<'_>]) -> Option<ValidationIssue> {
        let bgm = segments.iter().find(|s| s.tag == "BGM")?;
        let code = bgm.element_str(0)?;
        (!self.allowed.iter().any(|a| a == code)).then(|| {
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("code '{code}' is not in the allowed list"),
            )
            .with_rule_id("ORDERS-ALLOWED-CODE")
        })
    }
}

let pack = ProfileRulePack::builder("ORDERS")
    .with_rule(AllowedCodesRule {
        allowed: vec!["220".into(), "231".into()],
    });
```

`ProfileRule` requires `Send + Sync` so packs can be shared across threads (e.g.
when validating in a `spawn_blocking` worker).

---

## Pack introspection

```rust
# use edifact_rs::ProfileRulePack;
let pack = ProfileRulePack::builder("MY-PACK")
    .for_message_type("ORDERS")
    .with_rule_fn(|_| None)
    .with_rule_fn(|_| None);

println!("name:        {}", pack.name());           // "MY-PACK"
println!("rule count:  {}", pack.rule_count());     // 2
println!("types:       {:?}", pack.message_types()); // ["ORDERS"]
```

---

## Best practices

1. **Use stable rule IDs** — prefix with your pack name: `"ORDERS-DOC-P001"`, not `"P001"`.
   This allows downstream code to filter and map rules independently.
2. **Return `None` for passing rules** — rules are evaluated on every call; returning
   `None` is free.
3. **Scope packs to message types** — unscoped packs run for every message, even
   when the logic is type-specific.
4. **Prefer `.merge()`** over registering many packs — a single merged pack is
   semantically cleaner and slightly more efficient (one iteration over segments).
5. **Keep rules small and focused** — one rule per concern makes reporting and
   debugging easier.

---

## Next steps

- [Validation](validation.md) — multi-layer `ValidationContext` and the `Validator` trait
- [Streaming](streaming.md) — progressive per-window validation over reader streams
- [Diagnostics](diagnostics.md) — human-friendly rendering of `ValidationReport`
