use edifact_rs::{
    ValidationContext, ValidationLayer, Validator, ValidationReport, Segment, validate_each,
};

const D11A_CONFORMING: &str = include_str!("../tests/fixtures/d11a_conforming.edi");
const D11A_MALFORMED: &str = include_str!("../tests/fixtures/d11a_malformed.edi");

/// Example custom validator for demonstration.
/// In real applications, generate validators with your own build tooling or
/// implement them directly as custom `Validator`s.
struct DemoStructuralValidator;

impl Validator for DemoStructuralValidator {
    fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
        validate_each(segments, report, |_segment| {
            // Implement custom structural validation logic here
            // For example, check required segments, segment sequences, etc.
            Ok(())
        });
    }

    fn set_message_type(&mut self, _msg_type: Option<&str>) {}
}

struct DemoCodeListValidator;

impl Validator for DemoCodeListValidator {
    fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
        validate_each(segments, report, |_segment| {
            // Implement custom code-list validation logic here
            // For example, check codes against allowed value sets
            Ok(())
        });
    }

    fn set_message_type(&mut self, _msg_type: Option<&str>) {}
}

fn run_case(
    name: &str,
    input: &str,
    context: &ValidationContext,
) -> Result<(), edifact_rs::EdifactError> {
    // Parse the fixture into zero-copy segments, then validate.
    let segments: Vec<_> =
        edifact_rs::from_bytes(input.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    let report = context.validate_lenient(&segments);
    println!("report:\n{}", report.render_deterministic());

    println!(
        "case={name} valid={} errors={} warnings={}",
        report.is_valid(),
        report.errors.len(),
        report.warnings.len()
    );

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
    for error in &report.errors {
        println!(
            "error: code={} rule={:?} segment={:?} element={:?} offset={:?} message={}",
            error.error_code.unwrap_or("UNKNOWN"),
            error.rule_id,
            error.segment_tag,
            error.element_index,
            error.offset,
            error.message
        );
    }

    Ok(())
}

fn main() -> Result<(), edifact_rs::EdifactError> {
    let context = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(ValidationLayer::Structure, DemoStructuralValidator)
        .with_validator(ValidationLayer::CodeList, DemoCodeListValidator)
        .build();

    run_case("conforming", D11A_CONFORMING, &context)?;
    run_case("malformed", D11A_MALFORMED, &context)?;

    Ok(())
}
