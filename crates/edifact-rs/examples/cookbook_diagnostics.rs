//! Cookbook: rich diagnostics with `miette`
//!
//! Demonstrates the optional `diagnostics` feature that adds
//! [`miette::Diagnostic`] to [`edifact_rs::EdifactError`].  When enabled,
//! `miette::Report` renders errors with byte-precise source spans and help
//! text — ideal for developer-facing CLIs.
//!
//! Run **without** the feature flag for plain `Display` output:
//! ```text
//! cargo run -p edifact-rs --example cookbook_diagnostics
//! ```
//!
//! Run **with** the feature flag for miette's annotated output:
//! ```text
//! cargo run -p edifact-rs --example cookbook_diagnostics --features diagnostics
//! ```

fn run_validation() -> Result<(), edifact_rs::EdifactError> {
    use edifact_rs::{
        ValidationContext, ValidationLayer, Validator, ValidationReport, Segment, validate_each,
        from_bytes,
    };

    /// Simple demo validator
    struct DemoValidator;

    impl Validator for DemoValidator {
        fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
            validate_each(segments, report, |segment| {
                // Example validation: check for invalid code value
                if segment.tag == "BGM" {
                    if let Some(elem) = segment.get_element(0) {
                        if let Some(code) = elem.get_component(0) {
                            if code == "999" {
                                return Err(edifact_rs::EdifactError::InvalidCodeValue {
                                    tag: "BGM".to_owned(),
                                    element_index: 0,
                                    value: "999".to_owned(),
                                    code_list: "1001".to_owned(),
                                    offset: segment.span.start,
                                    suggestion: None,
                                });
                            }
                        }
                    }
                }
                Ok(())
            });
        }

        fn set_message_type(&mut self, _msg_type: Option<&str>) {}
    }

    // BGM code "999" is intentionally invalid — the validator below will
    // catch it and return an `InvalidCodeValue` error wrapped as a warning.
    let input = b"UNH+1+ORDERS:D:11A:UN'BGM+999+PO-4711+9'UNT+3+1'";
    let segments: Vec<_> = from_bytes(input).collect::<Result<Vec<_>, _>>()?;

    // Register the demo validator in the CodeList layer.
    // `validate_lenient` collects issues into a report instead of failing fast.
    let context = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(ValidationLayer::CodeList, DemoValidator)
        .build();

    let report = context.validate_lenient(&segments);
    // `render_deterministic` always produces the same output for the same
    // report — useful for snapshot tests.
    println!("report:\n{}", report.render_deterministic());
    for warning in &report.warnings {
        println!(
            "warning: code={} rule={:?} segment={:?} element={:?} offset={:?} message={}",
            warning.error_code.unwrap_or("UNKNOWN"),
            warning.rule_id,
            warning.segment_tag,
            warning.element_index,
            warning.offset,
            warning.message
        );
        if let Some(suggestion) = &warning.suggestion {
            println!("suggestion: {suggestion}");
        }
    }
    Ok(())
}

fn main() -> Result<(), edifact_rs::EdifactError> {
    #[cfg(feature = "diagnostics")]
    {
        if let Err(err) = run_validation() {
            let report = miette::Report::new(err);
            eprintln!("{report:?}");
            return Err(edifact_rs::EdifactError::ValidationFailed {
                error_count: 1,
                first_message: "validation failed in diagnostics example".to_owned(),
            });
        }
        Ok(())
    }

    #[cfg(not(feature = "diagnostics"))]
    {
        run_validation()?;
        println!("re-run with --features diagnostics for rich miette output");
        Ok(())
    }
}
