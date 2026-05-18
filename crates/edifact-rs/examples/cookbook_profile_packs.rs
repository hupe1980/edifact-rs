//! Cookbook: composing and filtering profile rule packs
//!
//! [`ProfileRulePack`] is the primary extension point for downstream MIG/profile
//! crates.  This example shows how to:
//!
//! - Build packs from closures with `.with_rule_fn`
//! - Restrict a pack to a specific message type with `.for_message_type`
//! - Merge multiple packs into one with `.merge`
//! - Assign stable rule IDs and filter by prefix with `filter_by_rule_prefix`
//!
//! Run:
//! ```text
//! cargo run -p edifact-rs --example cookbook_profile_packs
//! ```

use edifact_rs::{
    ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity, from_bytes,
};

fn main() -> Result<(), edifact_rs::EdifactError> {
    // Parse a minimal ORDERS interchange.
    let segments: Vec<_> =
        from_bytes(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'").collect::<Result<_, _>>()?;

    // ── Pack 1: document-code rule ────────────────────────────────────────────
    // Each rule closure receives the full message segment slice and returns
    // `Some(ValidationIssue)` if the rule fires, or `None` if it passes.
    // The rule ID should be stable and namespaced so downstream code can filter
    // or map it independently.
    let document_pack = ProfileRulePack::builder("ORDERS-DOCUMENT")
        .for_message_type("ORDERS") // only run for ORDERS messages
        .with_rule_fn(|segments| {
            let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
            let document_code = bgm.get_element(0)?.get_component(0)?;
            (document_code == "220").then(|| {
                ValidationIssue::new(
                    ValidationSeverity::Warning,
                    "document code 220 is only allowed in a special trading-partner flow",
                )
                .with_rule_id("ORDERS-DEMO-P001")
                .with_segment("BGM")
                .with_element_index(0)
                .with_suggestion("Use a trading-partner-specific document code in this example")
            })
        });

    // ── Pack 2: reference rule ────────────────────────────────────────────────
    let reference_pack = ProfileRulePack::builder("ORDERS-REFERENCE")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
            let reference = bgm.get_element(1)?.get_component(0)?;
            (reference == "PO123").then(|| {
                ValidationIssue::new(
                    ValidationSeverity::Info,
                    "demo rule observed the sample purchase-order reference",
                )
                .with_rule_id("ORDERS-DEMO-P002")
                .with_segment("BGM")
                .with_element_index(1)
            })
        });

    // ── Merge and validate ────────────────────────────────────────────────────
    // `.merge` combines both packs into one; rules run in declaration order.
    let pack = ProfileRulePack::builder("ORDERS-COMBINED")
        .merge(document_pack)
        .merge(reference_pack);

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_lenient(&segments);

    println!("{}", report.render_deterministic());

    // ── Filter by rule prefix ─────────────────────────────────────────────────
    // `filter_by_rule_prefix` returns a view of the report containing only
    // issues whose rule ID starts with the given prefix — useful when multiple
    // packs are merged and you want per-pack summaries.
    let partner_only = report.filter_by_rule_prefix("ORDERS-DEMO-P");
    println!(
        "partner rule summary: {} issue(s)",
        partner_only.total_issues()
    );

    Ok(())
}
