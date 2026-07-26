use edifact_rs::{
    EdifactError, Segment, ValidationContext, ValidationLayer, ValidationReport,
    ValidationRuleContext, Validator, validate_each,
};

/// A simple mock validator for testing the Validator trait and ValidationContext.
/// This demonstrates how to implement a custom validator without hardcoded directory data.
struct SimpleStructureValidator;

impl Validator for SimpleStructureValidator {
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        _context: &ValidationRuleContext<'_>,
    ) {
        validate_each(segments, report, |segment| {
            // Simple validation: require UNH to have a message type component
            if segment.tag == "UNH" && segment.get_element(1).is_none() {
                return Err(EdifactError::MissingRequiredElement {
                    tag: "UNH".to_owned(),
                    element_index: 1,
                });
            }
            // Reject segments starting with Z (reserved for user)
            if segment.tag.starts_with('Z') {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: segment.tag.to_owned(),
                    message_type: "GENERIC".to_owned(),
                    span: segment.span,
                });
            }
            Ok(())
        });
    }

    fn set_message_type(&mut self, _msg_type: Option<&str>) {
        // For simple validator, we don't need message type-specific logic
    }
}

#[test]
fn validator_trait_passes_for_valid_segments() {
    let input = b"UNH+1+ORDERS:D:11A:UN'BGM+E03+11042+9'UNT+3+1'";
    let segments: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let validator = SimpleStructureValidator;
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(!report.has_errors());
}

#[test]
fn validator_trait_fails_for_segments_with_errors() {
    let input = b"UNH+INCOMPLETE'ZZZ+X'";
    let segments: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let validator = SimpleStructureValidator;
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    // Should have errors for both missing UNH component and unknown segment
    assert!(report.has_errors());
}

#[test]
fn validator_rejects_unknown_segments() {
    let input = b"ZZZ+X'";
    let segments: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let validator = SimpleStructureValidator;
    let mut report = ValidationReport::default();
    validator.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());

    assert!(
        report
            .errors()
            .iter()
            .any(|i| i.message.contains("segment"))
    );
}

#[test]
fn context_can_disable_code_list_layer() {
    struct MockCodeListValidator;

    impl Validator for MockCodeListValidator {
        fn validate_batch(
            &self,
            segments: &[Segment<'_>],
            report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
            validate_each(segments, report, |segment| {
                if segment.tag == "BGM" {
                    return Err(EdifactError::InvalidCodeValue {
                        tag: "BGM".to_owned(),
                        element_index: 0,
                        value: "E3".to_owned(),
                        code_list: "1001".to_owned(),
                        span: segment.span,
                        suggestion: None,
                    });
                }
                Ok(())
            });
        }

        fn set_message_type(&mut self, _msg_type: Option<&str>) {}
    }

    let input = b"BGM+E3+11042+9'";
    let segments: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let ctx = ValidationContext::builder()
        .with_validator(ValidationLayer::CodeList, MockCodeListValidator)
        .code_list(false)
        .build();

    let report = ctx.validate_lenient(&segments);
    assert!(report.warnings().is_empty());
}

#[test]
fn validation_context_supports_multiple_validators() {
    struct ValidatorA;
    struct ValidatorB;

    impl Validator for ValidatorA {
        fn validate_batch(
            &self,
            segments: &[Segment<'_>],
            report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
            validate_each(segments, report, |segment| {
                if segment.tag.starts_with('Z') {
                    return Err(EdifactError::InvalidSegmentForMessage {
                        tag: segment.tag.to_owned(),
                        message_type: "GENERIC".to_owned(),
                        span: segment.span,
                    });
                }
                Ok(())
            });
        }

        fn set_message_type(&mut self, _msg_type: Option<&str>) {}
    }

    impl Validator for ValidatorB {
        fn validate_batch(
            &self,
            _segments: &[Segment<'_>],
            _report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
            // ValidatorB does nothing
        }

        fn set_message_type(&mut self, _msg_type: Option<&str>) {}
    }

    let input = b"BGM+E03+11042+9'ZZZ+X'";
    let segments: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let ctx = ValidationContext::builder()
        .with_validator(ValidationLayer::Structure, ValidatorA)
        .with_validator(ValidationLayer::CodeList, ValidatorB)
        .build();

    let report = ctx.validate_lenient(&segments);
    assert!(report.has_errors());
}

#[test]
fn validation_context_propagates_message_type() {
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct MessageTypeCapturingValidator(Arc<Mutex<Option<String>>>);

    impl Validator for MessageTypeCapturingValidator {
        fn validate_batch(
            &self,
            _segments: &[Segment<'_>],
            _report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
        }

        fn set_message_type(&mut self, msg_type: Option<&str>) {
            *self.0.lock().unwrap() = msg_type.map(|s| s.to_owned());
        }
    }

    let captured_message_type = Arc::new(Mutex::new(None));
    let input = b"UNH+1+ORDERS:D:11A:UN'UNT+2+1'";
    let segments: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let ctx = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_validator(
            ValidationLayer::Structure,
            MessageTypeCapturingValidator(captured_message_type.clone()),
        )
        .build();

    let report = ctx.validate_lenient(&segments);
    assert!(!report.has_errors());
    assert_eq!(
        *captured_message_type.lock().unwrap(),
        Some("ORDERS".to_owned())
    );
}

// TEST 7.2: segments between UNB and first UNH are rejected
//
// `validate_envelope` must return `InvalidSegmentForMessage` when any
// application segment appears between UNB and the first UNH.
#[test]
fn envelope_rejects_segment_between_unb_and_unh() {
    use edifact_rs::{EdifactError, validate_envelope};

    // Insert a BGM between UNB and UNH (not inside a message envelope).
    let input = b"UNB+UNOB:1+SENDER+RECEIVER+200101:0000+1'BGM+220+1+9'UNH+1+ORDERS:D:96A:UN'UNT+2+1'UNZ+1+1'";
    let segments: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("should tokenise");

    let result = validate_envelope(&segments);
    assert!(
        matches!(result, Err(EdifactError::InvalidSegmentForMessage { ref tag, .. }) if tag == "BGM"),
        "expected InvalidSegmentForMessage for BGM between UNB and UNH, got: {result:?}"
    );
}
