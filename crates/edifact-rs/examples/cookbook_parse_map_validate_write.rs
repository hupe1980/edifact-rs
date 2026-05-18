//! Cookbook: parse → map fields → validate → round-trip write
//!
//! The most common EDIFACT workflow in a single example:
//!
//! 1. **Parse** raw bytes into a zero-copy `Vec<Segment<'_>>`
//! 2. **Map** named parties using `find_qualified_segment`
//! 3. **Validate** using a custom `Validator` registered in a `ValidationContext`
//! 4. **Serialize** the segment list back to EDIFACT wire format with `to_bytes`
//!
//! Run:
//! ```text
//! cargo run -p edifact-rs --example cookbook_parse_map_validate_write
//! ```

use edifact_rs::{
    ValidationContext, ValidationLayer, Validator, ValidationReport, Segment,
    find_qualified_segment, from_bytes, to_bytes, validate_each,
};

/// A simple custom validator that checks for known segment tags
struct SimpleValidator;

impl Validator for SimpleValidator {
    fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
        validate_each(segments, report, |_segment| {
            // Basic structural validation
            // In a real application, you would:
            // 1. Implement custom validation logic specific to your use case
            // 2. Or plug in a generated validator from your own build workflow
            Ok(())
        });
    }

    fn set_message_type(&mut self, _msg_type: Option<&str>) {}
}

fn main() -> Result<(), edifact_rs::EdifactError> {
    // ── 1. Parse ──────────────────────────────────────────────────────────────
    // `from_bytes` returns a lazy iterator of `Segment<'_>`, each borrowing
    // directly from `input` — no heap allocation for segment data.
    let input = b"UNA:+.? 'UNH+1+ORDERS:D:11A:UN'BGM+220+PO-4711+9'NAD+BY+4000001000002::9'NAD+SU+4000001000001::9'UNT+5+1'";

    let segments: Vec<_> = from_bytes(input).collect::<Result<Vec<_>, _>>()?;

    // ── 2. Map fields ─────────────────────────────────────────────────────────
    // `find_qualified_segment` scans for the first NAD whose element 0 matches
    // the qualifier ("BY" = buyer, "SU" = supplier).
    let buyer = find_qualified_segment(&segments, "NAD", "BY")
        .and_then(|segment| segment.element_str(1))
        .unwrap_or("unknown");
    let supplier = find_qualified_segment(&segments, "NAD", "SU")
        .and_then(|segment| segment.element_str(1))
        .unwrap_or("unknown");

    println!("buyer={buyer} supplier={supplier}");

    // ── 3. Validate ───────────────────────────────────────────────────────────
    // `validate_lenient` collects all issues into a report rather than failing
    // on the first error.  Switch to `validate_strict` to get a typed
    // `EdifactError` containing the first failure.
    let validation_context = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(ValidationLayer::Structure, SimpleValidator)
        .build();

    let report = validation_context.validate_lenient(&segments);
    if !report.is_valid() {
        // Promote the first validation error into an `EdifactError` so callers
        // receive a typed error rather than having to inspect the report.
        return Err(edifact_rs::EdifactError::ValidationFailed {
            error_count: report.errors.len(),
            first_message: report
                .errors
                .first()
                .map(|issue| issue.message.clone())
                .unwrap_or_else(|| "unknown validation issue".to_owned()),
        });
    }

    // ── 4. Round-trip write ───────────────────────────────────────────────────
    // `to_bytes` serialises the borrowed segment list back to wire-format bytes.
    // Because segments borrow from `input`, this effectively round-trips the
    // original payload through the parser and writer.
    let output = to_bytes(&segments)?;
    let output_text =
        String::from_utf8(output).map_err(|_| edifact_rs::EdifactError::InvalidUtf8)?;
    println!("roundtrip={output_text}");

    Ok(())
}

