//! EDIFACT envelope validation (Story 2.4).
//!
//! Validates UNB / UNH / UNT / UNZ envelope segment structure and count
//! consistency — independently of business-rule (AHB) validation.

use crate::{error::EdifactError, model::Segment};

/// Extracted data from the `UNB` / `UNZ` interchange envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterchangeEnvelope {
    /// Syntax identifier, e.g. `"UNOA"`.
    pub syntax_identifier: String,
    /// Interchange sender identification.
    pub sender_id: String,
    /// Interchange recipient identification.
    pub recipient_id: String,
    /// Interchange date-time string as found in the source.
    pub datetime: String,
    /// Interchange control reference.
    pub control_ref: String,
    /// Declared message (functional group) count from `UNZ`.
    pub declared_message_count: u32,
    /// Actual message count encountered between `UNB` and `UNZ`.
    pub actual_message_count: u32,
}

/// Extracted data from a single `UNH` / `UNT` message envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageEnvelope {
    /// Message reference from `UNH` element 0.
    pub message_ref: String,
    /// EDIFACT message type, e.g. `"ORDERS"`.
    pub message_type: String,
    /// Version number, e.g. `"D"`.
    pub version: String,
    /// Release number, e.g. `"11A"`.
    pub release: String,
    /// Controlling agency code, e.g. `"UN"`.
    pub controlling_agency: String,
    /// Association assigned code (MIG version), e.g. `"FV2510"`.
    pub association_code: String,
    /// Declared segment count from `UNT`.
    pub declared_segment_count: u32,
    /// Actual segment count between this `UNH` and its `UNT`.
    pub actual_segment_count: u32,
}

/// Validates the EDIFACT interchange envelope for the given segments.
///
/// Checks:
/// - `UNB` is present (first meaningful segment)
/// - `UNZ` is present (last segment) with correct message count
/// - Each `UNH` is paired with a `UNT` carrying a matching segment count
/// - `UNZ` message count matches the number of `UNH`/`UNT` pairs found
///
/// Returns `Ok((interchange_env, message_envs))` on success,
/// or `Err(EdifactError::MessageCountMismatch)` / `Err(EdifactError::SegmentCountMismatch)` on
/// count discrepancies.
///
/// # Limitations
///
/// Functional group segments (`UNG`/`UNE`) are **not supported**.  If the
/// input contains `UNG` or `UNE` segments they will be treated as regular
/// message segments and may cause an [`EdifactError::InvalidSegmentForMessage`]
/// error or incorrect segment counting.  Strip functional-group wrappers
/// before calling this function.
pub fn validate_envelope(
    segments: &[Segment<'_>],
) -> Result<(InterchangeEnvelope, Vec<MessageEnvelope>), EdifactError> {
    let mut interchange_env = extract_interchange(segments)?;
    let message_envs = extract_messages(segments)?;
    interchange_env.actual_message_count = u32::try_from(message_envs.len())
        .unwrap_or(u32::MAX);

    // Cross-check UNZ declared count vs. actual UNH/UNT pair count
    if interchange_env.declared_message_count != interchange_env.actual_message_count {
        return Err(EdifactError::MessageCountMismatch {
            expected: interchange_env.declared_message_count,
            actual: interchange_env.actual_message_count,
        });
    }

    // Cross-check each UNT segment count vs. actual count
    for msg in &message_envs {
        if msg.declared_segment_count != msg.actual_segment_count {
            return Err(EdifactError::SegmentCountMismatch {
                expected: msg.declared_segment_count,
                actual: msg.actual_segment_count,
                message_ref: msg.message_ref.clone(),
            });
        }
    }

    Ok((interchange_env, message_envs))
}

fn extract_interchange(segments: &[Segment<'_>]) -> Result<InterchangeEnvelope, EdifactError> {
    if segments.first().map(|segment| segment.tag) != Some("UNB") {
        return Err(EdifactError::MissingSegment {
            tag: "UNB".to_owned(),
            expected_position: "first segment of interchange".to_owned(),
        });
    }

    if segments.last().map(|segment| segment.tag) != Some("UNZ") {
        return Err(EdifactError::MissingSegment {
            tag: "UNZ".to_owned(),
            expected_position: "last segment of interchange".to_owned(),
        });
    }

    let unb = &segments[0];
    let unz = &segments[segments.len() - 1];

    let syntax_identifier = required_component(unb, 0, 0)?.to_owned();

    let sender_id = required_component(unb, 1, 0)?.to_owned();

    let recipient_id = required_component(unb, 2, 0)?.to_owned();

    // Element 3: date/time composite
    let date = required_component(unb, 3, 0)?;
    let time = unb
        .get_element(3)
        .and_then(|e| e.get_component(1))
        .unwrap_or("");
    let datetime = if time.is_empty() {
        date.to_owned()
    } else {
        format!("{date}:{time}")
    };

    let control_ref = required_component(unb, 4, 0)?.to_owned();
    let unz_control_ref = required_component(unz, 1, 0)?;
    if unz_control_ref != control_ref {
        return Err(EdifactError::QualifierMismatch {
            tag: "UNZ".to_owned(),
            actual: unz_control_ref.to_owned(),
            expected: control_ref,
            offset: unz.span.start,
        });
    }

    let declared_message_count: u32 = required_component(unz, 0, 0)?
        .parse()
        .map_err(|_| EdifactError::InvalidText {
            offset: unz.span.start,
        })?;

    Ok(InterchangeEnvelope {
        syntax_identifier,
        sender_id,
        recipient_id,
        datetime,
        control_ref,
        declared_message_count,
        actual_message_count: 0,
    })
}

/// Thin shim that forwards to [`crate::de::required_component`].
#[inline]
fn required_component<'a>(
    segment: &'a Segment<'_>,
    element_index: usize,
    component_index: usize,
) -> Result<&'a str, EdifactError> {
    crate::de::required_component(segment, element_index, component_index)
}

fn extract_messages(segments: &[Segment<'_>]) -> Result<Vec<MessageEnvelope>, EdifactError> {
    let mut messages: Vec<MessageEnvelope> = Vec::new();
    let mut in_message = false;
    let mut msg_start_idx: usize = 0;
    let mut current_unh: Option<&Segment<'_>> = None;

    for (i, seg) in segments[1..segments.len() - 1].iter().enumerate() {
        match seg.tag {
            "UNH" => {
                if in_message {
                    return Err(EdifactError::InvalidSegmentForMessage {
                        tag: "UNH".to_owned(),
                        message_type: "ENVELOPE".to_owned(),
                        offset: seg.span.start,
                    });
                }
                in_message = true;
                msg_start_idx = i;
                current_unh = Some(seg);
            }
            "UNT" if in_message => {
                let unh = current_unh
                    .take()
                    .ok_or(EdifactError::InvalidSegmentForMessage {
                        tag: "UNT".to_owned(),
                        message_type: "ENVELOPE".to_owned(),
                        offset: seg.span.start,
                    })?;

                let message_ref = required_component(unh, 0, 0)?.to_owned();

                let message_type = required_component(unh, 1, 0)?.to_owned();
                let version = required_component(unh, 1, 1)?.to_owned();
                let release = required_component(unh, 1, 2)?.to_owned();
                let controlling_agency = required_component(unh, 1, 3)?.to_owned();
                let association_code = unh
                    .get_element(1)
                    .and_then(|e| e.get_component(4))
                    .unwrap_or("")
                    .to_owned();

                let declared_segment_count: u32 = required_component(seg, 0, 0)?
                    .parse()
                    .map_err(|_| EdifactError::InvalidText {
                        offset: seg.span.start,
                    })?;
                let unt_ref = required_component(seg, 1, 0)?;
                if unt_ref != message_ref {
                    return Err(EdifactError::QualifierMismatch {
                        tag: "UNT".to_owned(),
                        actual: unt_ref.to_owned(),
                        expected: message_ref.clone(),
                        offset: seg.span.start,
                    });
                }

                // actual count = segments from UNH (inclusive) to UNT (inclusive)
                let actual_segment_count = u32::try_from(i - msg_start_idx + 1)
                    .unwrap_or(u32::MAX);

                in_message = false;
                messages.push(MessageEnvelope {
                    message_ref,
                    message_type,
                    version,
                    release,
                    controlling_agency,
                    association_code,
                    declared_segment_count,
                    actual_segment_count,
                });
            }
            "UNT" => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: "UNT".to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    offset: seg.span.start,
                });
            }
            "UNB" | "UNZ" if in_message => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: seg.tag.to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    offset: seg.span.start,
                });
            }
            _ if !in_message => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: seg.tag.to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    offset: seg.span.start,
                });
            }
            _ => {}
        }
    }

    if in_message {
        return Err(EdifactError::MissingSegment {
            tag: "UNT".to_owned(),
            expected_position: "end of message group".to_owned(),
        });
    }

    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse test fixtures into an owned-segment vec (no memory leaks).
    fn parse(input: &[u8]) -> Vec<crate::OwnedSegment> {
        crate::from_reader(std::io::Cursor::new(input)).expect("parse failed")
    }

    /// Parse then validate: convenience wrapper for tests that only need the result.
    fn parse_and_validate(
        input: &[u8],
    ) -> Result<(InterchangeEnvelope, Vec<MessageEnvelope>), EdifactError> {
        let owned = parse(input);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        validate_envelope(&segs)
    }

    const VALID_INTERCHANGE: &[u8] =
        b"UNA:+.? 'UNB+UNOA:3+SENDER::293+RECEIVER::293+230401:0900+00001'UNH+00001+ORDERS:D:11A:UN:EAN010'BGM+220+PO-4711+9'DTM+137:20230401:102'UNT+4+00001'UNZ+1+00001'";

    #[test]
    fn valid_envelope_parses_ok() {
        let (interchange, messages) =
            parse_and_validate(VALID_INTERCHANGE).expect("envelope should be valid");
        assert_eq!(interchange.sender_id, "SENDER");
        assert_eq!(interchange.recipient_id, "RECEIVER");
        assert_eq!(interchange.control_ref, "00001");
        assert_eq!(interchange.declared_message_count, 1);
        assert_eq!(interchange.actual_message_count, 1);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_type, "ORDERS");
        assert_eq!(messages[0].association_code, "EAN010");
        assert_eq!(messages[0].declared_segment_count, 4);
        assert_eq!(messages[0].actual_segment_count, 4); // UNH + BGM + DTM + UNT
    }

    #[test]
    fn unt_count_mismatch_returns_err() {
        // UNT declares 99 segments but only 4 are present
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'DTM+137:20200101:102'UNT+99+1'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(
                result,
                Err(EdifactError::SegmentCountMismatch { expected: 99, .. })
            ),
            "expected SegmentCountMismatch, got {result:?}"
        );
    }

    #[test]
    fn unz_count_mismatch_returns_err() {
        // UNZ declares 2 messages but only 1 UNH/UNT pair is present
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+2+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(
                result,
                Err(EdifactError::MessageCountMismatch {
                    expected: 2,
                    actual: 1
                })
            ),
            "expected MessageCountMismatch(2,1), got {result:?}"
        );
    }

    #[test]
    fn missing_unb_returns_err() {
        let input = b"UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(result.is_err());
    }

    #[test]
    fn extracts_una_interchange_correctly() {
        // Test that UNA does not interfere with envelope field extraction
        let (env, _) = parse_and_validate(VALID_INTERCHANGE).unwrap();
        // UNA is parsed by tokenizer; UNB field extraction must be correct
        assert_eq!(env.syntax_identifier, "UNOA");
        assert_eq!(env.datetime, "230401:0900");
    }

    #[test]
    fn dangling_unh_without_unt_returns_err() {
        let input =
            b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(matches!(result, Err(EdifactError::MissingSegment { ref tag, .. }) if tag == "UNT"));
    }

    #[test]
    fn stray_segment_outside_message_returns_err() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'BGM+999+PO-2+9'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(matches!(result, Err(EdifactError::InvalidSegmentForMessage { .. })));
    }

    #[test]
    fn missing_unb_sender_component_returns_err() {
        let input = b"UNB+UNOA:3++R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+1+1'";
        let result = parse_and_validate(input);
        // Element 1 (sender) exists but is empty ("+") — component 0 is absent.
        assert!(
            matches!(result, Err(EdifactError::MissingRequiredComponent { ref tag, element_index: 1, component_index: 0 }) if tag == "UNB"),
            "expected MissingRequiredComponent for empty sender, got: {result:?}"
        );
    }

    #[test]
    fn nested_unh_without_closing_previous_message_returns_err() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNH+2+ORDERS:D:11A:UN:EAN010'UNT+3+2'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::InvalidSegmentForMessage { ref tag, .. }) if tag == "UNH"),
            "expected InvalidSegmentForMessage(UNH), got {result:?}"
        );
    }

    #[test]
    fn unt_message_reference_must_match_unh() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+999'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(matches!(result, Err(EdifactError::QualifierMismatch { tag, .. }) if tag == "UNT"));
    }

    #[test]
    fn unz_control_reference_must_match_unb() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+1+999'";
        let result = parse_and_validate(input);
        assert!(matches!(result, Err(EdifactError::QualifierMismatch { tag, .. }) if tag == "UNZ"));
    }

    #[test]
    fn missing_unh_message_type_components_return_err() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A'BGM+220+PO-1+9'UNT+3+1'UNZ+1+1'";
        let result = parse_and_validate(input);
        // UNH element 1 = "ORDERS:D:11A" — component 3 (controlling agency) is absent.
        assert!(
            matches!(result, Err(EdifactError::MissingRequiredComponent { ref tag, element_index: 1, component_index: 3 }) if tag == "UNH"),
            "expected MissingRequiredComponent for truncated UNH message type, got: {result:?}"
        );
    }

    #[test]
    fn nested_unz_inside_message_returns_err() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'UNZ+1+1'UNT+2+1'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(matches!(result, Err(EdifactError::InvalidSegmentForMessage { tag, .. }) if tag == "UNZ"));
    }

    // ── UNG/UNE functional-group regression guard ────────────────────────────
    //
    // ISO 9735-1 defines optional functional groups (UNG/UNE) that may wrap
    // one or more UNH/UNT pairs.  `validate_envelope` currently documents that
    // UNG/UNE are NOT supported (see module doc at line ~62).  These tests
    // assert the *documented* behaviour: UNG/UNE-wrapped interchanges must
    // not silently produce incorrect counts — they must return an explicit error.

    #[test]
    fn envelope_with_ung_returns_explicit_error() {
        // A UNG segment appearing between UNB and UNH is not a recognized
        // envelope segment — validate_envelope must reject it explicitly.
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+S+R+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+1'\
                      UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            result.is_err(),
            "UNG/UNE is documented as unsupported; must return an error, not silently produce wrong counts"
        );
        // The error must identify the offending segment (UNG or UNE), not some
        // unrelated internal failure.
        assert!(
            matches!(result, Err(EdifactError::InvalidSegmentForMessage { ref tag, .. }) if tag == "UNG" || tag == "UNE"),
            "expected InvalidSegmentForMessage for UNG or UNE, got {result:?}"
        );
    }
}
