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

// ── ISO 9735-1 §8.7 trailing separators and §9.1 insignificant characters ─────

mod suppression_rules {
    use edifact_rs::{
        ComponentRef, DirectoryValidator, ElementRef, Insignificant, Repr, SegmentDefinition,
        Status, ValidationContext, ValidationLayer, from_bytes,
    };

    fn syntax_codes(input: &[u8]) -> Vec<String> {
        let segments: Vec<_> = from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        ValidationContext::builder()
            .with_syntax_validation()
            .build()
            .validate_lenient(&segments)
            .iter_issues()
            .filter_map(|i| i.error_code().map(str::to_owned))
            .collect()
    }

    #[test]
    fn a_segment_ending_in_an_empty_element_is_a_trailing_separator() {
        // §8.7.1: separators that would follow omitted trailing data elements
        // "shall also be omitted".
        assert!(syntax_codes(b"BGM+220+'").contains(&"E051".to_owned()));
        // …but a segment that simply stops is correct.
        assert!(!syntax_codes(b"BGM+220'").contains(&"E051".to_owned()));
    }

    #[test]
    fn a_composite_ending_in_an_empty_component_is_a_trailing_separator() {
        // §8.7.2 is the component-level counterpart.
        assert!(syntax_codes(b"DTM+137:20260101:'").contains(&"E051".to_owned()));
        assert!(!syntax_codes(b"DTM+137:20260101'").contains(&"E051".to_owned()));
    }

    #[test]
    fn an_omitted_element_in_the_middle_keeps_its_separator_and_is_not_reported() {
        // §8.7.1 Figure 1: an interior omission *must* retain its separator, so
        // reporting it would be telling the sender to corrupt the segment.
        let codes = syntax_codes(b"BGM+220++9'");
        assert!(!codes.contains(&"E051".to_owned()), "{codes:?}");
    }

    #[test]
    fn a_single_empty_element_is_an_omitted_element_not_a_trailing_separator() {
        // §8.4: a mandatory segment with no data to carry is transferred as
        // `ABC'`; one empty element is that shape, not a stray separator.
        let codes = syntax_codes(b"BGM+'");
        assert!(!codes.contains(&"E051".to_owned()), "{codes:?}");
    }

    /// `DirectoryValidator::new` takes a plain `fn` for the lookup, so the
    /// definition has to reach it through a `static` rather than a capture.
    /// Each representation under test gets its own.
    macro_rules! directory_report {
        ($input:expr, $repr:expr) => {{
            static ELEMENTS: &[ElementRef] = &[$repr];
            static SEG: SegmentDefinition = SegmentDefinition::new("ZZZ", "Test", ELEMENTS);
            report_against($input, |tag| (tag == "ZZZ").then_some(&SEG))
        }};
    }

    fn report_against(
        input: &[u8],
        lookup: fn(&str) -> Option<&'static SegmentDefinition>,
    ) -> Vec<String> {
        let segments: Vec<_> = from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let validator =
            DirectoryValidator::new("t", lookup, |_, _| true, |_, _| None, |_, _| None, None);
        ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate_lenient(&segments)
            .iter_issues()
            .filter_map(|i| i.error_code().map(str::to_owned))
            .collect()
    }

    #[test]
    fn leading_zeroes_in_a_variable_numeric_value_are_reported() {
        // §9.1: "In variable length numeric data elements, leading zeroes shall
        // be suppressed."
        assert!(
            directory_report!(
                b"ZZZ+007'",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::n_up_to(6))
            )
            .contains(&"E053".to_owned())
        );
        assert!(
            !directory_report!(
                b"ZZZ+7'",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::n_up_to(6))
            )
            .contains(&"E053".to_owned())
        );
    }

    #[test]
    fn a_single_zero_before_a_decimal_mark_is_allowed() {
        // §9.1 says so explicitly, so `0.5` must not be reported — while `00.5`
        // still is.
        assert!(
            !directory_report!(
                b"ZZZ+0.5'",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::n_up_to(6))
            )
            .contains(&"E053".to_owned())
        );
        assert!(
            directory_report!(
                b"ZZZ+00.5'",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::n_up_to(6))
            )
            .contains(&"E053".to_owned())
        );
        // A bare zero is a significant value, not a leading one.
        assert!(
            !directory_report!(
                b"ZZZ+0'",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::n_up_to(6))
            )
            .contains(&"E053".to_owned())
        );
    }

    #[test]
    fn trailing_spaces_in_a_variable_text_value_are_reported() {
        // §9.1: "In variable length alphabetic and alphanumeric data elements,
        // trailing spaces shall be suppressed."
        assert!(
            directory_report!(
                b"ZZZ+ACME '",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::an_up_to(10))
            )
            .contains(&"E053".to_owned())
        );
        assert!(
            !directory_report!(
                b"ZZZ+ACME'",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::an_up_to(10))
            )
            .contains(&"E053".to_owned())
        );
    }

    #[test]
    fn a_fixed_length_value_is_exempt_from_suppression() {
        // §9.1 governs *variable* length elements only: a fixed-length numeric
        // field is zero-padded by design, and a fixed text one space-padded.
        assert!(
            !directory_report!(
                b"ZZZ+007'",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::n(3))
            )
            .contains(&"E053".to_owned())
        );
        assert!(
            !directory_report!(
                b"ZZZ+AC '",
                ElementRef::new(1, "9999", Status::Mandatory, 1).with_repr(Repr::an(4))
            )
            .contains(&"E053".to_owned())
        );
    }

    #[test]
    fn suppression_findings_are_warnings_not_errors() {
        // The value is still readable; a partner may or may not care.
        let segments: Vec<_> = from_bytes(b"BGM+220+'")
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let report = ValidationContext::builder()
            .with_syntax_validation()
            .build()
            .validate_lenient(&segments);
        assert!(!report.has_errors(), "{report:#?}");
        assert!(report.has_warnings());
    }

    #[test]
    fn the_insignificant_kind_is_exposed_on_the_error() {
        // The two rules need different fixes, so the variant distinguishes them.
        assert_eq!(
            Insignificant::LeadingZeroes.to_string(),
            "leading zeroes are not suppressed"
        );
        assert_eq!(
            Insignificant::TrailingSpaces.to_string(),
            "trailing spaces are not suppressed"
        );
    }

    #[test]
    fn a_composite_component_definition_still_drives_the_check() {
        static C507: &[ComponentRef] = &[
            ComponentRef::new(1, "2005", Status::Mandatory).with_repr(Repr::an_up_to(3)),
            ComponentRef::new(2, "2380", Status::Conditional).with_repr(Repr::an_up_to(35)),
        ];
        static ELEMENTS: &[ElementRef] =
            &[ElementRef::composite(1, "C507", Status::Mandatory, 1, C507)];
        static DTM: SegmentDefinition = SegmentDefinition::new("DTM", "Date/time", ELEMENTS);

        let segments: Vec<_> = from_bytes(b"DTM+137:20260101 '")
            .collect::<Result<Vec<_>, _>>()
            .expect("parse");
        let validator = DirectoryValidator::new(
            "t",
            |tag| (tag == "DTM").then_some(&DTM),
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate_lenient(&segments);
        assert!(
            report.iter_issues().any(|i| i.error_code() == Some("E053")),
            "{report:#?}"
        );
    }
}
