//! Shared UN/EDIFACT directory validation engine used by D.11A, D.01B and D.96A.

use crate::validator::{Validator, report_error};
use crate::{EdifactError, Segment, ValidationIssue, ValidationReport, ValidationSeverity};

/// Mandatory/Conditional status of a data element within a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Element must be present.
    Mandatory,
    /// Element is optional unless additional rules require it.
    Conditional,
}

/// Reference to a data element within a segment definition.
#[derive(Debug, Clone, Copy)]
pub struct ElementRef {
    /// One-based element position in the segment definition.
    pub position: u8,
    /// UN/EDIFACT data element identifier.
    pub data_element: &'static str,
    /// Requirement status of the element.
    pub status: Status,
    /// Maximum repetition count for this element.
    pub max_repeat: u8,
}

/// Definition of an EDIFACT segment (tag + element structure).
#[derive(Debug)]
pub struct SegmentDefinition {
    /// Segment tag.
    pub tag: &'static str,
    /// Human-readable segment name.
    pub name: &'static str,
    /// Ordered element definitions.
    pub elements: &'static [ElementRef],
}

type SegmentLookupFn = fn(&str) -> Option<&'static SegmentDefinition>;
type IsCodeValidFn = fn(&str, &str) -> bool;
type SuggestCodeFn = fn(&str, &str) -> Option<&'static str>;
type ExpectedComponentsFn = fn(&str, usize) -> Option<u8>;
type AdditionalStructureRuleFn = fn(&Segment<'_>) -> Result<(), EdifactError>;
/// Returns the `(element_index, component_index, data_element_id)` tuples to
/// validate against a code list for the given segment tag.
type CodeListRulesFn = fn(tag: &str) -> &'static [(usize, usize, &'static str)];

/// Code-list validation rules common to all UN/EDIFACT directory releases.
///
/// Each entry is `(element_index, component_index, data_element_id)`.
/// `element_index` and `component_index` are zero-based.
///
/// Covers the most frequently validated qualifier/code elements across ORDERS,
/// INVOIC, UTILMD, and similar message types.
pub(crate) fn base_code_list_rules(tag: &str) -> &'static [(usize, usize, &'static str)] {
    match tag {
        "BGM" => &[(0, 0, "1001")],
        "DTM" => &[(0, 0, "2005")],
        "NAD" => &[(0, 0, "3035")],
        "QTY" => &[(0, 0, "6063")],
        "RFF" => &[(0, 0, "1153")],
        "MOA" => &[(0, 0, "5025")],
        "PRI" => &[(0, 0, "5125")],
        "LOC" => &[(0, 0, "3227")],
        _ => &[],
    }
}

/// Shared validator implementation that is configured per UN/EDIFACT directory release.
///
/// # Scope and limitations
///
/// `DirectoryValidator` validates individual segment *content* (element counts,
/// component counts, code-list values, and conditional rules) and checks that
/// every *mandatory* segment type is present at least once.  It does **not**
/// validate segment *sequence* or *repetition cardinality* — i.e., it cannot
/// tell you that a `BGM` segment appears more than once, or that a `RFF` group
/// appears in the wrong position.  Full sequence validation requires a
/// state-machine per message type (UN/EDIFACT Segment Tables) which is outside
/// the scope of this implementation.
#[derive(Debug, Clone)]
pub struct DirectoryValidator {
    directory_id: &'static str,
    segment_lookup: SegmentLookupFn,
    is_code_valid: IsCodeValidFn,
    suggest_code: SuggestCodeFn,
    expected_components: ExpectedComponentsFn,
    code_list_rules: CodeListRulesFn,
    additional_structure_rule: Option<AdditionalStructureRuleFn>,
    message_type: Option<String>,
    enforce_known_tags: bool,
    structure_checks: bool,
    code_list_checks: bool,
}

impl DirectoryValidator {
    /// Create a validator for a specific directory release with injected lookup/check hooks.
    pub fn new(
        directory_id: &'static str,
        segment_lookup: SegmentLookupFn,
        is_code_valid: IsCodeValidFn,
        suggest_code: SuggestCodeFn,
        expected_components: ExpectedComponentsFn,
        additional_structure_rule: Option<AdditionalStructureRuleFn>,
    ) -> Self {
        Self {
            directory_id,
            segment_lookup,
            is_code_valid,
            suggest_code,
            expected_components,
            code_list_rules: base_code_list_rules,
            additional_structure_rule,
            message_type: None,
            enforce_known_tags: true,
            structure_checks: true,
            code_list_checks: true,
        }
    }

    /// Override the code-list rules function.
    ///
    /// Directories can supply a directory-specific implementation that extends or
    /// replaces the base rules from [`base_code_list_rules`].
    pub fn with_code_list_rules(mut self, f: CodeListRulesFn) -> Self {
        self.code_list_rules = f;
        self
    }

    /// Enable only structure checks and disable code-list checks.
    pub fn structure_only(mut self) -> Self {
        self.structure_checks = true;
        self.code_list_checks = false;
        self
    }

    /// Enable only code-list checks and disable structure checks.
    pub fn code_list_only(mut self) -> Self {
        self.structure_checks = false;
        self.code_list_checks = true;
        self
    }

    /// Configure whether unknown segment tags should be rejected.
    pub fn enforce_known_tags(mut self, enforce: bool) -> Self {
        self.enforce_known_tags = enforce;
        self
    }

    fn detect_message_type(&self, segments: &[Segment<'_>]) -> Option<String> {
        if let Some(explicit) = self.message_type.as_deref() {
            return Some(explicit.to_owned());
        }

        segments
            .iter()
            .find(|s| s.tag == "UNH")
            .and_then(|s| s.get_element(1))
            .and_then(|e| e.get_component(0))
            .map(str::to_owned)
    }

    /// Return the list of segment tags that are mandatory for `message_type`.
    ///
    /// **Coverage**: only `UTILMD`, `ORDERS`, and `INVOIC` have message-type-specific
    /// mandatory segments hard-coded.  All other message types fall back to the
    /// generic set `["UNH", "UNT"]`.
    ///
    /// The returned tags are checked via a presence test only — ordering and
    /// repetition constraints are *not* validated.  Unknown message types always
    /// return the generic set, never an empty slice, so envelope segments are
    /// always required regardless of message type.
    fn required_segments_for(message_type: &str) -> &'static [&'static str] {
        match message_type {
            "UTILMD" | "ORDERS" | "INVOIC" => &["UNH", "BGM", "UNT"],
            _ => &["UNH", "UNT"],
        }
    }

    /// Count the non-trailing-empty components in element `element_idx` of `seg`.
    ///
    /// Per ISO 9735-1 §3.3 ("Trailing empty component data elements may be omitted"),
    /// a sender is not required to transmit trailing empty components; this function
    /// therefore strips them before checking against the expected count so that
    /// conformant messages with omitted trailing components are still accepted.
    ///
    /// # Examples
    ///
    /// - `DTM+137:20200101:` has three declared components but only 2 non-empty → effective=2
    /// - `NAD+MS++::293` has a composite with 3 components, last two empty → effective=1
    fn effective_component_count(seg: &Segment<'_>, element_idx: usize) -> Option<u8> {
        let elem = seg.elements.get(element_idx)?;
        let mut count = elem.components.len();
        while count > 0 && elem.components[count - 1].as_ref().is_empty() {
            count -= 1;
        }
        debug_assert!(
            count <= u8::MAX as usize,
            "effective_component_count: element has >255 components, which is invalid EDIFACT"
        );
        Some(count as u8)
    }

    fn validate_component_counts(&self, seg: &Segment<'_>) -> Result<(), EdifactError> {
        for idx in 0..seg.elements.len() {
            if let Some(expected) = (self.expected_components)(seg.tag, idx) {
                let actual = Self::effective_component_count(seg, idx).unwrap_or(0);
                if actual != expected {
                    return Err(EdifactError::InvalidComponentCount {
                        tag: seg.tag.to_owned(),
                        element_index: idx,
                        expected,
                        actual,
                        offset: seg.span.start,
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_code_lists(&self, seg: &Segment<'_>) -> Result<(), EdifactError> {
        let rules = (self.code_list_rules)(seg.tag);

        for (elem_idx, comp_idx, de) in rules {
            let value = seg
                .get_element(*elem_idx)
                .and_then(|e| e.get_component(*comp_idx))
                .unwrap_or("");
            if !value.is_empty() && !(self.is_code_valid)(de, value) {
                let suggestion = (self.suggest_code)(de, value);
                return Err(EdifactError::InvalidCodeValue {
                    tag: seg.tag.to_owned(),
                    element_index: *elem_idx,
                    value: value.to_owned(),
                    code_list: (*de).to_owned(),
                    offset: seg.span.start,
                    suggestion,
                });
            }
        }

        Ok(())
    }
}

impl DirectoryValidator {
    fn validate_segment(&self, seg: &Segment<'_>) -> Result<(), EdifactError> {
        if !self.structure_checks && !self.code_list_checks {
            return Ok(());
        }

        let Some(def) = (self.segment_lookup)(seg.tag) else {
            if self.structure_checks && self.enforce_known_tags {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: seg.tag.to_owned(),
                    message_type: self
                        .message_type
                        .clone()
                        .unwrap_or_else(|| self.directory_id.to_owned()),
                    offset: seg.tag_span.start,
                });
            }
            return Ok(());
        };

        let max_elements = def.elements.len();
        let min_elements = def
            .elements
            .iter()
            .rposition(|e| e.status == Status::Mandatory)
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let actual = seg.elements.len();

        if self.structure_checks && (actual < min_elements || actual > max_elements) {
            return Err(EdifactError::InvalidElementCount {
                tag: seg.tag.to_owned(),
                min: min_elements,
                max: max_elements,
                actual,
                offset: seg.span.start,
            });
        }

        if self.structure_checks {
            for element in def
                .elements
                .iter()
                .filter(|e| e.status == Status::Mandatory)
            {
                let idx = (element.position as usize).saturating_sub(1);
                let is_present = seg
                    .elements
                    .get(idx)
                    .is_some_and(|elem| elem.components.iter().any(|c| !c.as_ref().is_empty()));
                if !is_present {
                    return Err(EdifactError::MissingRequiredElement {
                        tag: seg.tag.to_owned(),
                        element_index: idx,
                    });
                }
            }
            self.validate_component_counts(seg)?;

            if let Some(rule) = self.additional_structure_rule {
                rule(seg)?;
            }
        }

        if self.code_list_checks {
            self.validate_code_lists(seg)?;
        }

        Ok(())
    }
}

impl Validator for DirectoryValidator {
    fn set_message_type(&mut self, message_type: Option<&str>) {
        self.message_type = message_type.map(str::to_owned);
    }

    fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
        for seg in segments {
            if let Err(err) = self.validate_segment(seg) {
                report_error(report, err);
            }
        }

        if self.structure_checks {
            if let Some(message_type) = self.detect_message_type(segments) {
                for required_tag in Self::required_segments_for(&message_type) {
                    if segments.iter().all(|s| s.tag != *required_tag) {
                        report.add_error(
                            ValidationIssue::new(
                                ValidationSeverity::Error,
                                format!(
                                    "required segment {} missing for message type {}",
                                    required_tag, message_type
                                ),
                            )
                            .with_segment(*required_tag)
                            .with_suggestion("Add the mandatory segment at the correct position"),
                        );
                    }
                }

                let seq = Self::required_segments_for(&message_type);
                let mut last_idx = None;
                for tag in seq {
                    if let Some(idx) = segments.iter().position(|s| s.tag == *tag) {
                        if let Some(prev) = last_idx {
                            if idx < prev {
                                report.add_error(
                                    ValidationIssue::new(
                                        ValidationSeverity::Error,
                                        format!(
                                            "segment sequence violation for message type {}: '{}' appears out of order",
                                            message_type, tag
                                        ),
                                    )
                                    .with_segment(*tag)
                                    .with_suggestion(
                                        "Ensure required segments follow UN/EDIFACT canonical order",
                                    ),
                                );
                            }
                        }
                        last_idx = Some(idx);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_ELEMENTS: &[ElementRef] = &[ElementRef {
        position: 1,
        data_element: "C507",
        status: Status::Mandatory,
        max_repeat: 1,
    }];

    static TEST_SEGMENT: SegmentDefinition = SegmentDefinition {
        tag: "TST",
        name: "Test segment",
        elements: TEST_ELEMENTS,
    };

    fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
        match tag {
            "TST" => Some(&TEST_SEGMENT),
            _ => None,
        }
    }

    fn code_valid(_de: &str, _code: &str) -> bool {
        true
    }

    fn suggest_code(_de: &str, _code: &str) -> Option<&'static str> {
        None
    }

    fn expected_components(_tag: &str, _idx: usize) -> Option<u8> {
        None
    }

    #[test]
    fn mandatory_composite_present_when_any_component_non_empty() {
        let input = b"TST+:ABC'";
        let segments: Vec<_> = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse should succeed");

        let validator = DirectoryValidator::new(
            "TEST",
            segment_lookup,
            code_valid,
            suggest_code,
            expected_components,
            None,
        );

        let mut report = ValidationReport::default();
        validator.validate_batch(&segments, &mut report);
        assert!(!report.has_errors());
    }

    // ── effective_component_count (ISO 9735-1 §3.3 trailing-empty-component trim) ──

    fn parse_single(input: &[u8]) -> crate::model::Segment<'static> {
        // SAFETY: intentional leak — test inputs are small and bounded per call.
        // `Segment<'static>` is needed so the returned value is not tied to a local
        // buffer; the allocation is bounded by test count, not message size.
        let leaked: &'static [u8] = Box::leak(input.to_vec().into_boxed_slice());
        crate::from_bytes(leaked)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse should succeed")
            .into_iter()
            .next()
            .expect("at least one segment")
    }

    #[test]
    fn trailing_empty_component_stripped_from_dtm() {
        // DTM+137:20200101: has three components in element 0; the third is empty.
        // ISO 9735-1 §3.3 says trailing empty components may be omitted,
        // so effective count should be 2.
        let seg = parse_single(b"DTM+137:20200101:'");
        let count = DirectoryValidator::effective_component_count(&seg, 0);
        assert_eq!(count, Some(2), "trailing empty component should be stripped");
    }

    #[test]
    fn all_empty_components_result_in_zero() {
        // NAD+MS++: → element 2 is ":" with two empty components → effective=0
        let seg = parse_single(b"NAD+MS++:'");
        let count = DirectoryValidator::effective_component_count(&seg, 2);
        assert_eq!(count, Some(0), "all-empty composite should have effective count 0");
    }

    #[test]
    fn non_empty_component_not_stripped() {
        // DTM+137:20200101:102 — all three components are non-empty
        let seg = parse_single(b"DTM+137:20200101:102'");
        let count = DirectoryValidator::effective_component_count(&seg, 0);
        assert_eq!(count, Some(3), "no components should be stripped when all non-empty");
    }

    #[test]
    fn with_code_list_rules_overrides_base() {
        // Override code-list rules to require element 0 of TST to be a specific code.
        fn custom_rules(tag: &str) -> &'static [(usize, usize, &'static str)] {
            match tag {
                "TST" => &[(0, 0, "CUSTOM_DE")],
                _ => &[],
            }
        }
        fn custom_code_valid(_de: &str, code: &str) -> bool {
            code == "VALID"
        }
        fn no_suggestion(_de: &str, _code: &str) -> Option<&'static str> {
            None
        }

        let input = b"TST+INVALID'";
        let segments: Vec<_> = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse should succeed");

        let validator = DirectoryValidator::new(
            "TEST",
            segment_lookup,
            custom_code_valid,
            no_suggestion,
            expected_components,
            None,
        )
        .with_code_list_rules(custom_rules);

        let mut report = ValidationReport::default();
        validator.validate_batch(&segments, &mut report);
        assert!(
            report.has_warnings(),
            "INVALID is not in the custom code list so validation must warn"
        );
    }
}
