/// Canonical demo ORDERS profile pack used across multiple test files.
///
/// Defines two rules:
/// - `DEMO-P001` (Error): rejects `BGM` document code `220`.
/// - `DEMO-P002` (Warning): flags purchase-order reference `PO-4711` as reserved.
pub fn demo_orders_profile_pack() -> edifact_rs::ProfileRulePack {
    use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity};

    ProfileRulePack::new("ORDERS-DEMO")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(|segments, issues| {
            issues.extend((|| -> Option<ValidationIssue> {
                let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
                let document_code = bgm.get_element(0)?.get_component(0)?;
                (document_code == "220").then(|| {
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        "profile rule DEMO-P001 violated: BGM document code 220 is rejected in this demo pack",
                    )
                    .with_rule_id("DEMO-P001")
                    .with_segment("BGM")
                    .with_element_index(0)
                    .with_suggestion("Use a different BGM document code in this demo pack")
                })
            })());
        })
        .with_stateless_rule_fn(|segments, issues| {
            issues.extend((|| -> Option<ValidationIssue> {
                let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
                let reference = bgm.get_element(1)?.get_component(0)?;
                (reference == "PO-4711").then(|| {
                    ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "profile rule DEMO-P002 warning: purchase-order reference PO-4711 is reserved in this demo pack",
                    )
                    .with_rule_id("DEMO-P002")
                    .with_segment("BGM")
                    .with_element_index(1)
                    .with_suggestion("Use a non-reserved reference in this demo pack")
                })
            })());
        })
}
