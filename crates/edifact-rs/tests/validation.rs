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
                    tag: segment.tag().to_owned(),
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

    let report = ctx.validate(&segments);
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
                        tag: segment.tag().to_owned(),
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

    let report = ctx.validate(&segments);
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

    let report = ctx.validate(&segments);
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
            .validate(&segments)
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
            .validate(&segments)
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
            .validate(&segments);
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
            .validate(&segments);
        assert!(
            report.iter_issues().any(|i| i.error_code() == Some("E053")),
            "{report:#?}"
        );
    }
}

// ── 019: exact positions, layer and kind on every built-in issue ─────────────

mod issue_model {
    use edifact_rs::{
        DirectoryValidator, ElementRef, FindingKind, ProfileRulePack, SegmentDefinition, Status,
        ValidationContext, ValidationIssue, ValidationLayer, ValidationReport, from_bytes, service,
    };

    fn parse(input: &[u8]) -> Vec<edifact_rs::Segment<'_>> {
        from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("fixture should parse")
    }

    fn codes(report: &ValidationReport) -> Vec<&str> {
        report
            .iter_issues()
            .filter_map(ValidationIssue::error_code)
            .collect()
    }

    fn by_code<'r>(report: &'r ValidationReport, code: &str) -> &'r ValidationIssue {
        report
            .iter_issues()
            .find(|i| i.error_code() == Some(code))
            .unwrap_or_else(|| panic!("no {code} in {report:#?}"))
    }

    /// Every issue carries a layer and a kind; returns the report for more checks.
    fn assert_classified(report: ValidationReport) -> ValidationReport {
        assert!(report.has_any_issues(), "fixture produced no issues");
        for issue in report.iter_issues() {
            assert!(issue.layer().is_some(), "no layer: {issue:#?}");
            assert!(issue.kind().is_some(), "no kind: {issue:#?}");
        }
        report
    }

    #[test]
    fn seventy_thousand_segment_occurrence_is_exact() {
        // MSCONS-shaped: UNH, BGM, LIN, then 70 000 QTY of which only the last
        // carries the wrong qualifier.  A u16 field saturated at 65 535 here.
        let mut input = String::from("UNH+1+MSCONS:D:04B:UN'BGM+7+1'LIN+1'");
        for _ in 0..69_999 {
            input.push_str("QTY+220:1'");
        }
        input.push_str("QTY+999:1'UNT+70004+1'");
        let segments = parse(input.as_bytes());
        assert_eq!(segments.len(), 70_004);

        let pack = ProfileRulePack::new("MSCONS-DEMO").require_qualifier(
            "QTY",
            0,
            0,
            "220",
            "MSCONS-QTY-CODE",
        );
        let report = ValidationContext::builder()
            .with_profile_pack(pack)
            .build()
            .validate(&segments);

        assert_eq!(report.total_issues(), 1, "{report:#?}");
        let issue = &report.errors()[0];
        assert_eq!(issue.segment_occurrence(), Some(69_999));
        // UNH = 1, BGM = 2, LIN = 3, first QTY = 4, so QTY #70 000 = 70 003.
        assert_eq!(issue.segment_position(), Some(70_003));
        assert_eq!(
            segments[70_002].get_element(0).unwrap().get_component(0),
            Some("999")
        );
        assert_eq!(issue.element_index(), Some(0));
        assert_eq!(issue.component_index(), Some(0));
        assert_eq!(issue.kind(), Some(FindingKind::Code));
        assert_eq!(issue.layer(), Some(ValidationLayer::Profile));
        assert_eq!(issue.pack(), Some("MSCONS-DEMO"));
    }

    #[test]
    fn every_builtin_issue_has_layer_and_kind() {
        // Envelope: wrong UNT count and reference, wrong UNZ count and reference.
        let envelope = |input: &[u8]| {
            let report = assert_classified(
                ValidationContext::builder()
                    .with_envelope_validation()
                    .build()
                    .validate(&parse(input)),
            );
            for issue in report.iter_issues() {
                assert_eq!(issue.layer(), Some(ValidationLayer::Envelope), "{issue:#?}");
            }
            report
        };
        let report = envelope(
            b"UNB+UNOC:3+S+R+200101:0900+IC1'UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+5+2'UNZ+2+IC2'",
        );
        for code in ["E004", "E005", "E016"] {
            assert!(codes(&report).contains(&code), "{code}: {report:#?}");
        }
        assert_eq!(
            by_code(&report, "E004").kind(),
            Some(FindingKind::ControlMismatch)
        );
        let report = envelope(b"UNB+UNOC:3+S+R+200101:0900+IC1'UNZ+0+IC1'");
        assert_eq!(by_code(&report, "E042").kind(), Some(FindingKind::Envelope));
        let report =
            envelope(b"UNB+UNOC:3+S+R+200101:0900+IC1'UNH+1+ORDERS:D:96A:UN'UNT+2+1'UNZ+1+IC1'");
        assert_eq!(by_code(&report, "E043").kind(), Some(FindingKind::Envelope));
        let report = envelope(
            b"UNB+UNOC:3+S+R+200101:0900+IC1'UNG+ORDERS+S+R+200101:0900+G1+UN+D:96A'\
              UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'UNE+1+G1'\
              UNH+2+ORDERS:D:96A:UN'BGM+220'UNT+3+2'UNZ+2+IC1'",
        );
        assert_eq!(by_code(&report, "E052").kind(), Some(FindingKind::Envelope));

        // Syntax: blank value, segment without data, trailing separator.
        let report = assert_classified(
            ValidationContext::builder()
                .with_syntax_validation()
                .build()
                .validate(&parse(
                    b"UNH+1+ORDERS:D:96A:UN'FTX+   'DTM'NAD+BY+'UNT+5+1'",
                )),
        );
        for (code, kind, position) in [
            ("E045", FindingKind::Representation, 2),
            ("E046", FindingKind::Syntax, 3),
            ("E051", FindingKind::Syntax, 4),
        ] {
            let issue = by_code(&report, code);
            assert_eq!(issue.layer(), Some(ValidationLayer::Syntax), "{code}");
            assert_eq!(issue.kind(), Some(kind), "{code}");
            assert_eq!(issue.segment_position(), Some(position), "{code}");
        }

        // Charset: a lower-case letter in UNOA.  The slice starts at UNB, so
        // there is no message position to report.
        let report = assert_classified(
            ValidationContext::builder()
                .with_charset_validation()
                .build()
                .validate(&parse(
                    b"UNB+UNOA:3+S+R+200101:0900+1'NAD+BY+Acme Ltd'UNZ+0+1'",
                )),
        );
        let issue = by_code(&report, "E038");
        assert_eq!(issue.layer(), Some(ValidationLayer::Charset));
        assert_eq!(issue.kind(), Some(FindingKind::Charset));
        assert_eq!(issue.segment_position(), None);

        // Directory: code list, missing element, component count, plus the
        // two hand-built structure findings (missing and out-of-order segment).
        static DTM_ELEMENTS: &[ElementRef] = &[
            ElementRef::new(1, "C507", Status::Mandatory, 1),
            ElementRef::new(2, "2380", Status::Conditional, 1),
        ];
        static NAD_ELEMENTS: &[ElementRef] = &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Mandatory, 1),
        ];
        static DTM_DEF: SegmentDefinition =
            SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);
        static NAD_DEF: SegmentDefinition =
            SegmentDefinition::new("NAD", "Name and address", NAD_ELEMENTS);
        let directory = DirectoryValidator::new(
            "TEST",
            |tag| match tag {
                "DTM" => Some(&DTM_DEF),
                "NAD" => Some(&NAD_DEF),
                _ => None,
            },
            |de, code| !(de == "3035" && code == "XX"),
            |_, _| None,
            |tag, element| (tag == "DTM" && element == 0).then_some(3),
            None,
        )
        .with_required_segments(|_| &["UNH", "BGM", "DTM", "FTX", "UNT"]);
        let segments =
            parse(b"UNH+1+ORDERS:D:96A:UN'NAD+XX'DTM+137:20240101:102:9'BGM+220'UNT+5+1'");
        let report = assert_classified(
            ValidationContext::builder()
                .with_validator(ValidationLayer::Structure, directory)
                .build()
                .validate(&segments),
        );
        let code_list = by_code(&report, "E014");
        assert_eq!(code_list.layer(), Some(ValidationLayer::CodeList));
        assert_eq!(code_list.kind(), Some(FindingKind::Code));
        assert_eq!(code_list.segment_position(), Some(2));
        let missing_element = by_code(&report, "E008");
        assert_eq!(missing_element.layer(), Some(ValidationLayer::Structure));
        assert_eq!(missing_element.kind(), Some(FindingKind::Missing));
        let component_count = by_code(&report, "E013");
        assert_eq!(component_count.layer(), Some(ValidationLayer::Structure));
        assert_eq!(component_count.kind(), Some(FindingKind::Count));
        assert_eq!(component_count.segment_position(), Some(3));
        let missing_segment = report
            .iter_issues()
            .find(|i| i.error_code().is_none() && i.segment_tag() == Some("FTX"))
            .expect("missing FTX finding");
        assert_eq!(missing_segment.layer(), Some(ValidationLayer::Structure));
        assert_eq!(missing_segment.kind(), Some(FindingKind::Missing));
        let out_of_order = report
            .iter_issues()
            .find(|i| i.kind() == Some(FindingKind::Placement) && i.error_code().is_none())
            .unwrap_or_else(|| panic!("out-of-order finding: {report:#?}"));
        assert_eq!(out_of_order.segment_tag(), Some("DTM"));
        assert_eq!(out_of_order.error_code(), None, "kind only, no code");
        assert_eq!(out_of_order.segment_position(), Some(3));

        // Directory representation checks with the service directory.
        let service_directory = DirectoryValidator::new(
            "iso-9735-service",
            service::lookup,
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = assert_classified(
            ValidationContext::builder()
                .with_validator(ValidationLayer::Structure, service_directory)
                .build()
                .validate(&parse(b"UNZ+abc+IC1234567890123'")),
        );
        assert_eq!(
            by_code(&report, "E048").kind(),
            Some(FindingKind::Representation)
        );
        assert_eq!(
            by_code(&report, "E049").kind(),
            Some(FindingKind::Representation)
        );
        assert_eq!(
            by_code(&report, "E048").layer(),
            Some(ValidationLayer::Structure)
        );

        // Profile helpers.
        let pack = ProfileRulePack::new("PROFILE")
            .require_segment("FTX", "P-FTX-M")
            .forbid_segment("DTM", "P-DTM-X")
            .require_qualifier("BGM", 0, 0, "380", "P-BGM-CODE");
        let report = assert_classified(
            ValidationContext::builder()
                .with_profile_pack(pack)
                .build()
                .validate(&parse(
                    b"UNH+1+ORDERS:D:96A:UN'BGM+220'DTM+137:20240101:102'UNT+4+1'",
                )),
        );
        for (rule, kind, position) in [
            ("P-FTX-M", FindingKind::Missing, None),
            ("P-DTM-X", FindingKind::NotPermitted, Some(3)),
            ("P-BGM-CODE", FindingKind::Code, Some(2)),
        ] {
            let issue = report.issues_for_rule_id(rule).next().expect(rule);
            assert_eq!(issue.layer(), Some(ValidationLayer::Profile), "{rule}");
            assert_eq!(issue.kind(), Some(kind), "{rule}");
            assert_eq!(issue.pack(), Some("PROFILE"), "{rule}");
            assert_eq!(issue.segment_position(), position, "{rule}");
        }
    }

    #[test]
    fn fork_exclusion_notice_has_kind_other_and_no_layer() {
        struct NotForkable;
        impl edifact_rs::Validator for NotForkable {
            fn validate_batch(
                &self,
                _segments: &[edifact_rs::Segment<'_>],
                _report: &mut ValidationReport,
                _context: &edifact_rs::ValidationRuleContext<'_>,
            ) {
            }
        }
        let report = ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, NotForkable)
            .build()
            .fork_with_message_ref("1")
            .validate(&parse(b"UNH+1+ORDERS:D:96A:UN'UNT+2+1'"));
        let notice = &report.infos()[0];
        assert_eq!(
            notice.rule_id(),
            Some("edifact-rs::fork::excluded-validator")
        );
        assert_eq!(notice.kind(), Some(FindingKind::Other));
        assert_eq!(notice.layer(), None, "static issues are not stamped");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn reads_legacy_issue_json() {
        // A compatibility fixture in the legacy serialised form
        // (`serde_json::to_string_pretty` of an issue with element_index 1 and
        // segment_occurrence 3): the fields it lacks must default to `None`.
        let json = include_str!("fixtures/issue_v0_17.json");
        let issue: ValidationIssue = serde_json::from_str(json).expect("legacy JSON must load");
        assert_eq!(issue.error_code(), Some("E014"));
        assert_eq!(issue.element_index(), Some(1));
        assert_eq!(issue.segment_occurrence(), Some(3));
        assert_eq!(issue.component_index(), None);
        assert_eq!(issue.segment_position(), None);
        assert_eq!(issue.group_path(), None);
        assert_eq!(issue.layer(), None);
        assert_eq!(issue.kind(), None);
        assert_eq!(issue.pack(), None);
    }
}
