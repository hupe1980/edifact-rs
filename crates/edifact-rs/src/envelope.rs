//! EDIFACT envelope validation — UNB / UNG / UNH / UNT / UNE / UNZ.
//!
//! Validates the full ISO 9735-1 interchange structure including optional
//! functional groups (`UNG`/`UNE`).  The public surface is:
//!
//! - [`validate_envelope`] / [`validate_envelope_from_owned`] — fail-fast strict validation
//! - [`validate_envelope_lenient`] / [`validate_envelope_lenient_from_owned`] — collects all errors
//! - [`parse_unh`] — zero-copy parse of UNH identifier fields
//!
//! # UNZ count semantics (ISO 9735-1 §9.2)
//!
//! `UNZ` DE 0036 (the interchange control count) has dual semantics:
//! - **No functional groups**: counts `UNH`/`UNT` message pairs.
//! - **With functional groups**: counts `UNG`/`UNE` group pairs.
//!
//! `validate_envelope` checks the UNZ count against the appropriate unit
//! (groups when groups are present, messages otherwise) and reports
//! [`EdifactError::MessageCountMismatch`] on any discrepancy.

use crate::{
    OwnedSegment,
    error::EdifactError,
    model::{Segment, Span},
};
use std::collections::HashSet;

// ── Sealed segment-access trait ──────────────────────────────────────────────

pub(crate) trait SegmentReader: sealed::Sealed {
    fn tag(&self) -> &str;
    fn span(&self) -> Span;
    fn component(&self, elem_idx: usize, comp_idx: usize) -> Option<&str>;

    fn required_component_field(
        &self,
        elem_idx: usize,
        comp_idx: usize,
    ) -> Result<&str, EdifactError> {
        self.component(elem_idx, comp_idx)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EdifactError::MissingRequiredComponent {
                tag: self.tag().to_owned(),
                element_index: elem_idx,
                component_index: comp_idx,
            })
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::model::Segment<'_> {}
    impl Sealed for crate::OwnedSegment {}
}

impl SegmentReader for Segment<'_> {
    #[inline]
    fn tag(&self) -> &str {
        self.tag
    }
    #[inline]
    fn span(&self) -> Span {
        self.span
    }
    #[inline]
    fn component(&self, elem_idx: usize, comp_idx: usize) -> Option<&str> {
        self.get_element(elem_idx)?.get_component(comp_idx)
    }
}

impl SegmentReader for OwnedSegment {
    #[inline]
    fn tag(&self) -> &str {
        &self.tag
    }
    #[inline]
    fn span(&self) -> Span {
        self.span
    }
    #[inline]
    fn component(&self, elem_idx: usize, comp_idx: usize) -> Option<&str> {
        self.component_str(elem_idx, comp_idx)
    }
}

// ── Public data types ─────────────────────────────────────────────────────────

/// Extracted data from the `UNB` / `UNZ` interchange envelope.
///
/// All standard UNB fields that carry business-relevant information are
/// exposed.  Optional fields that are absent in the source are represented
/// as empty strings (`syntax_version`, qualifiers) or `None` (optional fields).
///
/// UNB element positions (ISO 9735-1 §6.1.1, 0-indexed):
///
/// ```text
/// [0] S001  syntax identifier + version
/// [1] S002  sender id (0004) + qualifier (0007) + internal id (0008)
/// [2] S003  recipient id (0010) + qualifier (0007) + internal id (0014)
/// [3] S004  date + time
/// [4] 0020  interchange control reference
/// [5] S005  recipient password (DE 0022 comp 0)
/// [6] 0026  application reference
/// [7] 0029  processing priority code
/// [8] 0031  acknowledgement request
/// [9] 0032  communications agreement ID
///[10] 0035  test indicator
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InterchangeEnvelope {
    /// Syntax identifier, e.g. `"UNOA"` or `"UNOB"` (UNB S001 DE 0001).
    pub syntax_identifier: String,
    /// Syntax version number, e.g. `"3"` (UNB S001 DE 0002).
    ///
    /// Empty string when the UNB omits the version component.
    pub syntax_version: String,
    /// Interchange sender identification (UNB S002 DE 0004).
    pub sender_id: String,
    /// Interchange sender identification code qualifier (UNB S002 DE 0007).
    ///
    /// Common values: `"14"` (EAN/GLN), `"ZZZ"` (mutually defined).
    /// Empty string when no qualifier is present.
    pub sender_qualifier: String,
    /// Interchange sender internal identification (UNB S002 **DE 0008**), if present.
    ///
    /// An optional address used by some EDI networks to identify the sub-entity
    /// (division, application) within the sender organisation; ISO 9735 version 3
    /// calls it the address for reverse routing.
    ///
    /// This is **DE 0008**, not DE 0014 — 0014 is the recipient-side component in
    /// S003.  See [`service::S002`][crate::service::S002].
    pub sender_routing_address: Option<String>,
    /// Interchange recipient identification (UNB S003 DE 0010).
    pub recipient_id: String,
    /// Interchange recipient identification code qualifier (UNB S003 DE 0007).
    ///
    /// Same values as `sender_qualifier`.  Empty string when absent.
    pub recipient_qualifier: String,
    /// Interchange recipient internal identification (UNB S003 DE 0014), if present.
    ///
    /// The recipient-side counterpart of `sender_routing_address`, which is
    /// DE 0008.  See [`service::S003`][crate::service::S003].
    pub recipient_routing_address: Option<String>,
    /// Interchange date (UNB S004 DE 0017), e.g. `"230401"` (YYMMDD format).
    pub date: String,
    /// Interchange time (UNB S004 DE 0019), e.g. `"0900"` (HHMM format), if present.
    ///
    /// `None` when the UNB time component (DE 0019) is absent.
    pub time: Option<String>,
    /// Interchange control reference (UNB DE 0020).
    pub control_ref: String,
    /// Recipient's reference/password (UNB S005 DE 0022), if present.
    ///
    /// Used in some EDI networks for basic interchange-level authentication.
    /// Empty S005 in the source yields `None`.
    pub recipient_password: Option<String>,
    /// Recipient's reference/password qualifier (UNB S005 DE 0025), if present.
    ///
    /// Qualifies the type of the `recipient_password`.  Example value: `"AA"` (unencoded).
    /// `None` when DE 0025 is absent or empty.
    pub recipient_password_qualifier: Option<String>,
    /// Application reference (UNB DE 0026, element index 6), if present.
    ///
    /// Identifies the division, department, or section of sender or recipient.
    pub app_ref: Option<String>,
    /// Processing priority code (UNB DE 0029, element index 7), if present.
    ///
    /// Indicates the processing priority requested by the sender.
    /// Rarely used in practice; included here for full ISO 9735-1 §6.1.1 compliance.
    pub processing_priority: Option<String>,
    /// Acknowledgement request flag (UNB DE 0031, element index 8).
    ///
    /// `true` when DE 0031 is `"1"`, indicating that the sender requests a
    /// `CONTRL` functional acknowledgement from the recipient.
    pub acknowledgement_request: bool,
    /// Communications agreement identifier (UNB DE 0032, element index 9), if present.
    ///
    /// Identifies the agreement controlling the interchange.
    pub communications_agreement_id: Option<String>,
    /// Test indicator flag (UNB DE 0035, element index 10).
    ///
    /// `true` when DE 0035 is `"1"`.  Test interchanges **must not** be processed
    /// as production data — check [`is_test()`](Self::is_test) before dispatching
    /// messages to business logic, billing, or downstream integrations.
    pub test_indicator: bool,
    /// Interchange unit count declared in `UNZ` DE 0036.
    ///
    /// - When no functional groups are present: count of messages (`UNH`/`UNT` pairs).
    /// - When functional groups are present: count of groups (`UNG`/`UNE` pairs).
    ///
    /// Use [`ValidatedInterchange::messages`] for a flat count of all messages
    /// regardless of group structure.
    pub declared_unit_count: u32,
    /// Actual unit count observed (groups if groups present; messages otherwise).
    pub actual_unit_count: u32,
}

impl InterchangeEnvelope {
    /// Returns `true` when the test indicator (`UNB` DE 0035) is set to `"1"`.
    ///
    /// Production systems must check this flag before dispatching any message
    /// to business logic, billing, or downstream integrations.
    ///
    /// # Example
    ///
    /// ```
    /// // UNB element [10] is the test indicator; "1" means test.
    /// // UNB+UNOA:3+S+R+200101:0900+1++++++1'  ← last element = "1" → is_test() == true
    /// let input = b"UNB+UNOA:3+S+R+200101:0900+CTRL++++++1'\
    ///               UNH+1+ORDERS:D:96A:UN'\
    ///               BGM+220+PO-001+9'\
    ///               UNT+3+1'\
    ///               UNZ+1+CTRL'";
    /// let segs: Vec<_> = edifact_rs::from_bytes(input)
    ///     .collect::<Result<Vec<_>, _>>()
    ///     .unwrap();
    /// let result = edifact_rs::validate_envelope(&segs).unwrap();
    /// assert!(result.interchange.is_test());
    /// ```
    #[inline]
    #[must_use]
    pub fn is_test(&self) -> bool {
        self.test_indicator
    }

    /// Returns `true` when the acknowledgement request flag (UNB DE 0031) is set.
    ///
    /// When `true`, the sender expects a `CONTRL` acknowledgement from the recipient.
    #[inline]
    #[must_use]
    pub fn ack_requested(&self) -> bool {
        self.acknowledgement_request
    }
}

impl std::fmt::Display for InterchangeEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{sender} -> {recipient} [{ctrl}] ({syntax}:{ver})",
            sender = self.sender_id,
            recipient = self.recipient_id,
            ctrl = self.control_ref,
            syntax = self.syntax_identifier,
            ver = self.syntax_version,
        )
    }
}

/// Extracted data from a single `UNH` / `UNT` message envelope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// Common access reference (UNH DE 0068, element index 2), if present.
    ///
    /// A reference shared across related messages or exchanges on the same network
    /// path.  Used by some EDI network profiles to
    /// correlate messages that belong to a single business transaction.
    /// `None` when element \[2\] is absent or empty.
    pub common_access_ref: Option<String>,
    /// Sequence of transfers (UNH S010 DE 0070, element index 3), if present.
    ///
    /// When a large message is split across multiple interchanges, this is the
    /// 1-based index of this segment within the sequence.  `None` when the message
    /// is not split (element \[3\] absent).
    pub sequence_of_transfers: Option<u32>,
    /// Transfer position indicator (UNH S010 DE 0073, element index 3 comp 1), if present.
    ///
    /// Values per ISO 9735-1 §6.2.3: `"C"` = continuation, `"F"` = first, `"L"` = last.
    /// `None` when element \[3\] is absent.
    pub transfer_position: Option<String>,
    /// Declared segment count from `UNT`.
    pub declared_segment_count: u32,
    /// Actual segment count between this `UNH` and its `UNT`.
    pub actual_segment_count: u32,
}

impl std::fmt::Display for MessageEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{msg_type}:{ver}:{rel} ref={msg_ref} seg={actual}/{declared}",
            msg_type = self.message_type,
            ver = self.version,
            rel = self.release,
            msg_ref = self.message_ref,
            actual = self.actual_segment_count,
            declared = self.declared_segment_count,
        )
    }
}

/// Extracted data from a single `UNG` / `UNE` functional group envelope.
///
/// ISO 9735-1 §8 defines optional functional groups that may wrap one or more
/// `UNH`/`UNT` message pairs.  This type carries the parsed fields from both
/// the `UNG` header and its matching `UNE` trailer, plus the validated messages.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FunctionalGroupEnvelope {
    /// Functional group identification (UNG DE 0038), e.g. `"ORDERS"`.
    pub group_id: String,
    /// Application sender's identification (UNG S006 DE 0040).
    pub app_sender: String,
    /// Application sender identification code qualifier (UNG S006 DE 0007).
    ///
    /// Empty string when no qualifier is present.
    pub app_sender_qualifier: String,
    /// Application recipient's identification (UNG S007 DE 0044).
    pub app_recipient: String,
    /// Application recipient identification code qualifier (UNG S007 DE 0007).
    ///
    /// Empty string when no qualifier is present.
    pub app_recipient_qualifier: String,
    /// Date of preparation (UNG S004 DE 0017), e.g. `"200101"` (YYMMDD format).
    pub date: String,
    /// Time of preparation (UNG S004 DE 0019), e.g. `"0900"` (HHMM format), if present.
    pub time: Option<String>,
    /// Functional group reference number (UNG DE 0048). Must match `UNE` DE 0048.
    pub group_ref: String,
    /// Controlling agency, coded (UNG DE 0051), e.g. `"UN"`.
    pub controlling_agency: String,
    /// Message version number, e.g. `"D"`.
    pub version: String,
    /// Message release number, e.g. `"96A"`.
    pub release: String,
    /// Declared message count from `UNE` DE 0060.
    pub declared_message_count: u32,
    /// Actual number of `UNH`/`UNT` pairs found within this group.
    pub actual_message_count: u32,
    /// Messages contained within this functional group.
    pub messages: Vec<MessageEnvelope>,
}

/// Fully validated interchange structure returned by [`validate_envelope`].
///
/// Provides both hierarchical (group → message) and flat (all messages) access
/// so that callers who do not care about group boundaries can use
/// [`messages`](Self::messages) directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ValidatedInterchange {
    /// Interchange-level envelope data (from `UNB`/`UNZ`).
    pub interchange: InterchangeEnvelope,
    /// Functional groups, when the interchange uses `UNG`/`UNE` wrappers.
    ///
    /// Empty when messages appear directly under the interchange (the common
    /// case for most modern EDIFACT implementations).
    pub functional_groups: Vec<FunctionalGroupEnvelope>,
    /// Flat list of all messages in the interchange.
    ///
    /// When functional groups are present this contains the same messages as
    /// the nested `messages` fields inside each [`FunctionalGroupEnvelope`].
    pub messages: Vec<MessageEnvelope>,
}

impl ValidatedInterchange {
    /// Returns `true` if this interchange uses `UNG`/`UNE` functional group wrappers.
    #[inline]
    #[must_use]
    pub fn has_functional_groups(&self) -> bool {
        !self.functional_groups.is_empty()
    }

    /// Total number of `UNH`/`UNT` message pairs across all groups.
    ///
    /// Equivalent to `self.messages.len()` but communicates intent clearly.
    #[inline]
    #[must_use]
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Iterate over all messages in the interchange.
    ///
    /// Equivalent to `self.messages.iter()` but communicates intent clearly
    /// and is stable regardless of future internal layout changes.
    #[inline]
    pub fn iter_messages(&self) -> impl Iterator<Item = &MessageEnvelope> {
        self.messages.iter()
    }

    /// Find the first message whose `message_ref` equals `reference`.
    ///
    /// Useful for locating a specific message in an interchange with multiple
    /// messages after calling `validate_envelope`.
    ///
    /// Returns `None` if no message with that reference exists.
    #[inline]
    #[must_use]
    pub fn find_message(&self, reference: &str) -> Option<&MessageEnvelope> {
        self.messages.iter().find(|m| m.message_ref == reference)
    }

    /// Collect all messages of a given type (e.g. `"ORDERS"`, `"INVOIC"`).
    ///
    /// Returns a `Vec` of references to matching messages in document order.
    /// Returns an empty `Vec` when the interchange contains no messages of
    /// the requested type.
    ///
    /// Prefer [`iter_messages_by_type`](Self::iter_messages_by_type) in tight loops
    /// to avoid the allocation.
    #[must_use]
    pub fn messages_by_type(&self, message_type: &str) -> Vec<&MessageEnvelope> {
        self.messages
            .iter()
            .filter(|m| m.message_type == message_type)
            .collect()
    }

    /// Iterate over all messages of a given type without allocating.
    ///
    /// Zero-allocation alternative to [`messages_by_type`](Self::messages_by_type).
    ///
    /// The bound `'q: 's` means the `message_type` string reference must outlive the
    /// borrow of `self`.  In practice this is always satisfied when passing a string
    /// literal (`&'static str`) or any string whose lifetime is at least as long as
    /// the `ValidatedInterchange` reference.  For short-lived computed strings, use
    /// [`messages_by_type`](Self::messages_by_type) which collects eagerly and releases the string reference
    /// immediately.
    #[inline]
    pub fn iter_messages_by_type<'s, 'q: 's>(
        &'s self,
        message_type: &'q str,
    ) -> impl Iterator<Item = &'s MessageEnvelope> + 's {
        self.messages
            .iter()
            .filter(move |m| m.message_type == message_type)
    }
}

impl std::fmt::Display for ValidatedInterchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{ic} messages={n}",
            ic = self.interchange,
            n = self.messages.len(),
        )
    }
}

impl std::fmt::Display for FunctionalGroupEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{gid} sender={sender} recipient={recip} [{gref}] ({agency}) msgs={actual}/{declared}",
            gid = self.group_id,
            sender = self.app_sender,
            recip = self.app_recipient,
            gref = self.group_ref,
            agency = self.controlling_agency,
            actual = self.actual_message_count,
            declared = self.declared_message_count,
        )
    }
}

/// Parsed identifier fields from a `UNH` segment.
///
/// All string slices borrow from the input bytes so they live as long as the
/// original byte buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageIdentifier<'a> {
    /// Message reference number (UNH DE 0062, element index 0).
    pub message_ref: &'a str,
    pub message_type: &'a str,
    pub version: &'a str,
    pub release: &'a str,
    pub controlling_agency: &'a str,
    /// Association assigned code (UNH S009 DE 0057).
    ///
    /// Matches `MessageEnvelope::association_code` for the same message.
    pub association_code: &'a str,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Extract identifier fields from a `UNH` segment (zero allocation).
pub fn parse_unh<'a>(unh: &'a Segment<'a>) -> Result<MessageIdentifier<'a>, EdifactError> {
    // Element [0]: DE 0062 — message reference number (required, simple DE)
    let message_ref = unh
        .get_element(0)
        .and_then(|e| e.get_component(0))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| EdifactError::MissingRequiredComponent {
            tag: "UNH".to_owned(),
            element_index: 0,
            component_index: 0,
        })?;
    // Element [1]: S009 composite — message type, version, release, agency, association
    let elem = unh
        .get_element(1)
        .ok_or_else(|| EdifactError::MissingRequiredElement {
            tag: "UNH".to_owned(),
            element_index: 1,
        })?;
    let message_type =
        elem.get_component(0)
            .ok_or_else(|| EdifactError::MissingRequiredComponent {
                tag: "UNH".to_owned(),
                element_index: 1,
                component_index: 0,
            })?;
    Ok(MessageIdentifier {
        message_ref,
        message_type,
        version: elem.get_component(1).unwrap_or(""),
        release: elem.get_component(2).unwrap_or(""),
        controlling_agency: elem.get_component(3).unwrap_or(""),
        association_code: elem.get_component(4).unwrap_or(""),
    })
}

/// Parsed identifier fields from a `UNG` segment.
///
/// All string slices borrow from the input bytes so they live as long as the
/// original byte buffer.  Use this for zero-allocation group routing in streaming
/// scenarios where you need to inspect group identity without full validation.
///
/// # UNG element positions (ISO 9735-1 §8, 0-indexed)
///
/// ```text
/// [0] DE 0038  functional group identification
/// [1] S006     application sender id + qualifier (comp 0 / comp 1)
/// [2] S007     application recipient id + qualifier (comp 0 / comp 1)
/// [3] S004     date + time (comp 0 / comp 1)
/// [4] DE 0048  group reference number
/// [5] DE 0051  controlling agency
/// [6] S008     version + release (comp 0 / comp 1)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GroupIdentifier<'a> {
    /// Functional group identification (UNG DE 0038), e.g. `"ORDERS"`.
    pub group_id: &'a str,
    /// Application sender identification (UNG S006 DE 0040).
    pub app_sender: &'a str,
    /// Application sender identification code qualifier (UNG S006 DE 0007).
    pub app_sender_qualifier: &'a str,
    /// Application recipient identification (UNG S007 DE 0044).
    pub app_recipient: &'a str,
    /// Application recipient identification code qualifier (UNG S007 DE 0007).
    pub app_recipient_qualifier: &'a str,
    /// Group reference number (UNG DE 0048).
    pub group_ref: &'a str,
    /// Controlling agency (UNG DE 0051), e.g. `"UN"`.
    pub controlling_agency: &'a str,
    /// Message version number from S008 (UNG DE 0052), e.g. `"D"`.
    pub version: &'a str,
    /// Message release number from S008 (UNG DE 0054), e.g. `"96A"`.
    pub release: &'a str,
}

/// Extract identifier fields from a `UNG` segment (zero allocation).
///
/// The symmetric counterpart to [`parse_unh`] for streaming scenarios that need
/// to inspect or route functional groups before full validation.
pub fn parse_ung<'a>(ung: &'a Segment<'a>) -> Result<GroupIdentifier<'a>, EdifactError> {
    let group_id = ung
        .get_element(0)
        .and_then(|e| e.get_component(0))
        .unwrap_or("");
    let app_sender = ung
        .get_element(1)
        .and_then(|e| e.get_component(0))
        .unwrap_or("");
    let app_sender_qualifier = ung
        .get_element(1)
        .and_then(|e| e.get_component(1))
        .unwrap_or("");
    let app_recipient = ung
        .get_element(2)
        .and_then(|e| e.get_component(0))
        .unwrap_or("");
    let app_recipient_qualifier = ung
        .get_element(2)
        .and_then(|e| e.get_component(1))
        .unwrap_or("");
    let group_ref = ung
        .get_element(4)
        .and_then(|e| e.get_component(0))
        .ok_or_else(|| EdifactError::MissingRequiredComponent {
            tag: "UNG".to_owned(),
            element_index: 4,
            component_index: 0,
        })?;
    let controlling_agency = ung
        .get_element(5)
        .and_then(|e| e.get_component(0))
        .unwrap_or("");
    let s008 = ung.get_element(6);
    let version = s008.as_ref().and_then(|e| e.get_component(0)).unwrap_or("");
    let release = s008.as_ref().and_then(|e| e.get_component(1)).unwrap_or("");
    Ok(GroupIdentifier {
        group_id,
        app_sender,
        app_sender_qualifier,
        app_recipient,
        app_recipient_qualifier,
        group_ref,
        controlling_agency,
        version,
        release,
    })
}

/// Validate the EDIFACT interchange envelope (fail-fast, borrowed-segment path).
///
/// Supports direct-message interchanges and functional-group interchanges
/// (ISO 9735-1 §8).  Returns [`ValidatedInterchange`] on success.
pub fn validate_envelope(segments: &[Segment<'_>]) -> Result<ValidatedInterchange, EdifactError> {
    validate_envelope_impl(segments)
}

/// Validate the EDIFACT interchange envelope (fail-fast, owned-segment path).
pub fn validate_envelope_from_owned(
    segments: &[OwnedSegment],
) -> Result<ValidatedInterchange, EdifactError> {
    validate_envelope_impl(segments)
}

/// Result of a lenient envelope validation — carries both a (possibly partial)
/// interchange and the full list of collected errors.
///
/// Returned by [`validate_envelope_lenient`] and [`validate_envelope_lenient_from_owned`].
///
/// # Semantics
///
/// | Condition | `interchange` | `errors` |
/// |-----------|---------------|----------|
/// | Structurally valid, all counts correct | `Some(result)` | empty |
/// | Structurally parseable but count violations | `Some(partial)` | non-empty |
/// | Missing `UNB`/`UNZ`, stray segments, etc. | `None` | non-empty |
#[derive(Debug)]
#[non_exhaustive]
pub struct LenientResult {
    /// The parsed interchange, if extraction was structurally possible.
    pub interchange: Option<ValidatedInterchange>,
    /// All errors collected during validation, in discovery order.
    pub errors: Vec<EdifactError>,
}

impl LenientResult {
    /// Returns `true` if no errors were detected and the interchange is fully valid.
    #[inline]
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns `true` if one or more errors were collected.
    ///
    /// The readable inverse of [`is_valid`](Self::is_valid).
    /// A partial interchange may still be present even when `has_errors()` returns `true`.
    #[inline]
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Convert into a `Result`, returning the interchange on success or the errors on failure.
    ///
    /// The partial interchange (when present alongside errors) is discarded on the
    /// `Err` path.  Use the fields directly when you need both simultaneously.
    ///
    /// This conversion is total.  Because [`errors`](Self::errors) is a public
    /// field, callers may legitimately filter out violations they tolerate before
    /// converting; if that leaves no errors but also no interchange, the result is
    /// an empty `Err` rather than a panic.
    pub fn into_strict(self) -> Result<ValidatedInterchange, Vec<EdifactError>> {
        match self.interchange {
            Some(interchange) if self.errors.is_empty() => Ok(interchange),
            _ => Err(self.errors),
        }
    }
}

/// Validate the EDIFACT envelope and collect **all** errors rather than stopping
/// at the first failure (borrowed-segment path).
///
/// Returns a [`LenientResult`] whose `interchange` field is:
///
/// - `Some(result)` with empty `errors` when fully valid.
/// - `Some(partial)` with non-empty `errors` when only count violations were found —
///   lets diagnostic tooling display the actual interchange structure.
/// - `None` when the interchange is structurally broken beyond recovery
///   (missing `UNB`/`UNZ`, stray segments, etc.).
pub fn validate_envelope_lenient(segments: &[Segment<'_>]) -> LenientResult {
    validate_envelope_lenient_impl(segments)
}

/// Lenient validation over an owned-segment slice — collects all errors.
///
/// See [`validate_envelope_lenient`] for full semantics.
pub fn validate_envelope_lenient_from_owned(segments: &[OwnedSegment]) -> LenientResult {
    validate_envelope_lenient_impl(segments)
}

// ── Core implementation ───────────────────────────────────────────────────────

/// Collector for **recoverable** envelope violations.
///
/// Extraction distinguishes two error classes:
///
/// * *Recoverable* — the violation is recorded here and extraction substitutes a
///   fallback value, so later checks still run.  Control-reference mismatches,
///   missing mandatory components, and unparseable counts are all recoverable.
/// * *Fatal* — the structure cannot be interpreted at all (no `UNB`/`UNZ`, a
///   stray segment outside any message, an unterminated message).  These are
///   still returned via `Err` and abort extraction.
///
/// Strict and lenient validation share one implementation over this sink, which
/// is what keeps them from diverging: strict reports the first error the sink
/// collected, lenient reports all of them.
#[derive(Default)]
struct ErrorSink {
    errors: Vec<EdifactError>,
}

impl ErrorSink {
    #[inline]
    fn push(&mut self, error: EdifactError) {
        self.errors.push(error);
    }

    /// Record a recoverable failure and continue with `fallback`.
    #[inline]
    fn recover<T>(&mut self, result: Result<T, EdifactError>, fallback: T) -> T {
        match result {
            Ok(value) => value,
            Err(error) => {
                self.errors.push(error);
                fallback
            }
        }
    }

    /// Read a mandatory component, recording `MissingRequiredComponent` if absent.
    #[inline]
    fn required<S: SegmentReader>(&mut self, seg: &S, element: usize, component: usize) -> String {
        self.recover(
            seg.required_component_field(element, component)
                .map(str::to_owned),
            String::new(),
        )
    }
}

/// Shared extraction used by both the strict and the lenient entry points.
///
/// Returns the interchange when the structure was interpretable at all, plus
/// every violation found in discovery order.
fn validate_envelope_collecting<S: SegmentReader>(
    segments: &[S],
) -> (Option<ValidatedInterchange>, Vec<EdifactError>) {
    let mut sink = ErrorSink::default();

    let mut interchange_env = match extract_interchange(segments, &mut sink) {
        Ok(env) => env,
        Err(fatal) => {
            sink.push(fatal);
            return (None, sink.errors);
        }
    };

    let inner = if segments.len() >= 2 {
        &segments[1..segments.len() - 1]
    } else {
        &[]
    };

    let (functional_groups, messages) = match extract_content(inner, &mut sink) {
        Ok(pair) => pair,
        Err(fatal) => {
            sink.push(fatal);
            return (None, sink.errors);
        }
    };

    // UNZ unit count semantics (ISO 9735-1 §9.2):
    //   with groups    → counts groups
    //   without groups → counts messages
    let actual_unit_count = if functional_groups.is_empty() {
        messages.len()
    } else {
        functional_groups.len()
    };
    interchange_env.actual_unit_count = sink.recover(
        u32::try_from(actual_unit_count).map_err(|_| EdifactError::InterchangeTooLarge {
            count: actual_unit_count as u64,
        }),
        u32::MAX,
    );

    if interchange_env.declared_unit_count != interchange_env.actual_unit_count {
        sink.push(EdifactError::MessageCountMismatch {
            expected: interchange_env.declared_unit_count,
            actual: interchange_env.actual_unit_count,
        });
    }

    for msg in &messages {
        if msg.declared_segment_count != msg.actual_segment_count {
            sink.push(EdifactError::SegmentCountMismatch {
                expected: msg.declared_segment_count,
                actual: msg.actual_segment_count,
                message_ref: msg.message_ref.clone(),
            });
        }
    }

    (
        Some(ValidatedInterchange {
            interchange: interchange_env,
            functional_groups,
            messages,
        }),
        sink.errors,
    )
}

fn validate_envelope_impl<S: SegmentReader>(
    segments: &[S],
) -> Result<ValidatedInterchange, EdifactError> {
    match validate_envelope_collecting(segments) {
        (Some(result), errors) if errors.is_empty() => Ok(result),
        (_, mut errors) => Err(errors
            .drain(..)
            .next()
            .unwrap_or(EdifactError::MissingSegment {
                tag: "UNB".to_owned(),
                expected_position: "first segment of interchange".to_owned(),
            })),
    }
}

fn validate_envelope_lenient_impl<S: SegmentReader>(segments: &[S]) -> LenientResult {
    let (interchange, errors) = validate_envelope_collecting(segments);
    LenientResult {
        interchange,
        errors,
    }
}

// ── Interchange extraction ────────────────────────────────────────────────────

fn extract_interchange<S: SegmentReader>(
    segments: &[S],
    sink: &mut ErrorSink,
) -> Result<InterchangeEnvelope, EdifactError> {
    if segments.first().map(|s| s.tag()) != Some("UNB") {
        return Err(EdifactError::MissingSegment {
            tag: "UNB".to_owned(),
            expected_position: "first segment of interchange".to_owned(),
        });
    }
    if segments.last().map(|s| s.tag()) != Some("UNZ") {
        return Err(EdifactError::MissingSegment {
            tag: "UNZ".to_owned(),
            expected_position: "last segment of interchange".to_owned(),
        });
    }

    let unb = &segments[0];
    let unz = &segments[segments.len() - 1];

    let syntax_identifier = sink.required(unb, 0, 0);
    let syntax_version = unb.component(0, 1).unwrap_or("").to_owned();

    // Validate DE 0001 against the ISO 9735-1 §3.1 list of defined syntax identifiers.
    const VALID_SYNTAX_IDS: &[&str] = &["UNOA", "UNOB", "UNOC", "UNOD", "UNOE", "UNOF", "KECA"];
    if !VALID_SYNTAX_IDS.contains(&syntax_identifier.as_str()) {
        sink.push(EdifactError::UnrecognisedSyntaxIdentifier(
            syntax_identifier.clone(),
        ));
    }

    let sender_id = sink.required(unb, 1, 0);
    let sender_qualifier = unb.component(1, 1).unwrap_or("").to_owned();
    // UNB S002 comp[2]: DE 0008 — sender internal identification
    let sender_routing_address = unb
        .component(1, 2)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let recipient_id = sink.required(unb, 2, 0);
    let recipient_qualifier = unb.component(2, 1).unwrap_or("").to_owned();
    // UNB S003 comp[2]: DE 0014 — recipient internal identification
    let recipient_routing_address = unb
        .component(2, 2)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let date = sink.required(unb, 3, 0);
    let time_raw = unb.component(3, 1).unwrap_or("");
    let time = if time_raw.is_empty() {
        None
    } else {
        Some(time_raw.to_owned())
    };

    let control_ref = sink.required(unb, 4, 0);

    // UNB element [5]: S005 — recipient's reference/password (DE 0022, comp 0) + qualifier (DE 0025, comp 1)
    let recipient_password = unb
        .component(5, 0)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let recipient_password_qualifier = unb
        .component(5, 1)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // UNB element [6]: DE 0026 — application reference
    let app_ref = unb
        .component(6, 0)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // UNB element [7]: DE 0029 — processing priority code
    let processing_priority = unb
        .component(7, 0)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // UNB element [8]: DE 0031 — acknowledgement request ("1" = requested)
    let acknowledgement_request = unb.component(8, 0) == Some("1");
    // UNB element [9]: DE 0032 — communications agreement ID
    let communications_agreement_id = unb
        .component(9, 0)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // UNB element [10]: DE 0035 — test indicator ("1" = test)
    let test_indicator = unb.component(10, 0) == Some("1");

    let unz_control_ref = sink.required(unz, 1, 0);
    if unz_control_ref != control_ref {
        sink.push(EdifactError::QualifierMismatch {
            tag: "UNZ".to_owned(),
            actual: unz_control_ref,
            expected: control_ref.clone(),
            span: unz.span(),
        });
    }

    let declared_unit_count_raw = sink.required(unz, 0, 0);
    let declared_unit_count: u32 = sink.recover(
        declared_unit_count_raw
            .parse()
            .map_err(|_| EdifactError::InvalidText {
                offset: unz.span().start,
            }),
        0,
    );

    Ok(InterchangeEnvelope {
        syntax_identifier,
        syntax_version,
        sender_id,
        sender_qualifier,
        sender_routing_address,
        recipient_id,
        recipient_qualifier,
        recipient_routing_address,
        date,
        time,
        control_ref,
        recipient_password,
        recipient_password_qualifier,
        app_ref,
        processing_priority,
        acknowledgement_request,
        communications_agreement_id,
        test_indicator,
        declared_unit_count,
        actual_unit_count: 0,
    })
}

// ── Content extraction ────────────────────────────────────────────────────────

fn extract_content<S: SegmentReader>(
    inner: &[S],
    sink: &mut ErrorSink,
) -> Result<(Vec<FunctionalGroupEnvelope>, Vec<MessageEnvelope>), EdifactError> {
    // A UNG as the first inner segment means the interchange uses functional groups.
    // Checking only the first tag is O(1) and correct: if UNG is present it must
    // always be first; a stray UNE without a preceding UNG is caught downstream.
    if inner.first().is_some_and(|s| s.tag() == "UNG") {
        let groups = extract_with_groups(inner, sink)?;
        let messages = groups
            .iter()
            .flat_map(|g| g.messages.iter().cloned())
            .collect();
        Ok((groups, messages))
    } else {
        let mut seen_refs = HashSet::new();
        let messages = extract_messages_flat(inner, sink, &mut seen_refs)?;
        Ok((vec![], messages))
    }
}

/// Find the index of the `UNE` that closes the `UNG` opened just before `start`.
fn find_matching_une<S: SegmentReader>(
    segments: &[S],
    start: usize,
) -> Result<usize, EdifactError> {
    for (offset, seg) in segments[start..].iter().enumerate() {
        match seg.tag() {
            "UNE" => return Ok(start + offset),
            "UNG" => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: "UNG".to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    span: seg.span(),
                });
            }
            _ => {}
        }
    }
    Err(EdifactError::MissingSegment {
        tag: "UNE".to_owned(),
        expected_position: "end of functional group".to_owned(),
    })
}

fn extract_with_groups<S: SegmentReader>(
    inner: &[S],
    sink: &mut ErrorSink,
) -> Result<Vec<FunctionalGroupEnvelope>, EdifactError> {
    let mut groups: Vec<FunctionalGroupEnvelope> = Vec::new();
    // DE 0048 must be unique within the interchange (ISO 9735-1 §8); DE 0062
    // must be unique across the whole interchange, so the set spans all groups.
    let mut seen_group_refs: HashSet<String> = HashSet::new();
    let mut seen_message_refs: HashSet<String> = HashSet::new();
    let mut i = 0;

    while i < inner.len() {
        let seg = &inner[i];
        match seg.tag() {
            "UNG" => {
                let ung_idx = i;
                let une_idx = find_matching_une(inner, ung_idx + 1)?;

                let ung = &inner[ung_idx];
                let group_id = ung.component(0, 0).unwrap_or("").to_owned();
                let app_sender = ung.component(1, 0).unwrap_or("").to_owned();
                let app_sender_qualifier = ung.component(1, 1).unwrap_or("").to_owned();
                let app_recipient = ung.component(2, 0).unwrap_or("").to_owned();
                let app_recipient_qualifier = ung.component(2, 1).unwrap_or("").to_owned();
                let date = ung.component(3, 0).unwrap_or("").to_owned();
                let time_raw = ung.component(3, 1).unwrap_or("");
                let time = if time_raw.is_empty() {
                    None
                } else {
                    Some(time_raw.to_owned())
                };
                // UNG DE 0048 — group reference number (mandatory per ISO 9735-1 §8)
                let group_ref = sink.required(ung, 4, 0);
                if !seen_group_refs.insert(group_ref.clone()) {
                    sink.push(EdifactError::DuplicateReference {
                        tag: "UNG".to_owned(),
                        reference: group_ref.clone(),
                        span: ung.span(),
                    });
                }
                let controlling_agency = ung.component(5, 0).unwrap_or("").to_owned();
                // UNG S008 — version (DE 0052, comp 0) + release (DE 0054, comp 1)
                // S008 is always at element index [6]; there is no element [7] in ISO 9735-1 §8.
                let version = ung.component(6, 0).unwrap_or("").to_owned();
                let release = ung.component(6, 1).unwrap_or("").to_owned();

                let une = &inner[une_idx];
                let declared_str = sink.required(une, 0, 0);
                let declared_message_count: u32 = sink.recover(
                    declared_str.parse().map_err(|_| EdifactError::InvalidText {
                        offset: une.span().start,
                    }),
                    0,
                );
                let une_ref = sink.required(une, 1, 0);
                if une_ref != group_ref {
                    sink.push(EdifactError::QualifierMismatch {
                        tag: "UNE".to_owned(),
                        actual: une_ref,
                        expected: group_ref.clone(),
                        span: une.span(),
                    });
                }

                let group_content = &inner[ung_idx + 1..une_idx];
                let messages = extract_messages_flat(group_content, sink, &mut seen_message_refs)?;
                let actual_message_count = sink.recover(
                    u32::try_from(messages.len()).map_err(|_| EdifactError::InterchangeTooLarge {
                        count: messages.len() as u64,
                    }),
                    u32::MAX,
                );

                if declared_message_count != actual_message_count {
                    sink.push(EdifactError::MessageCountMismatch {
                        expected: declared_message_count,
                        actual: actual_message_count,
                    });
                }

                groups.push(FunctionalGroupEnvelope {
                    group_id,
                    app_sender,
                    app_sender_qualifier,
                    app_recipient,
                    app_recipient_qualifier,
                    date,
                    time,
                    group_ref,
                    controlling_agency,
                    version,
                    release,
                    declared_message_count,
                    actual_message_count,
                    messages,
                });
                i = une_idx + 1;
            }
            "UNE" => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: "UNE".to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    span: seg.span(),
                });
            }
            "UNH" => {
                // Mixing direct messages with functional groups is invalid.
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: "UNH".to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    span: seg.span(),
                });
            }
            _ => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: seg.tag().to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    span: seg.span(),
                });
            }
        }
    }

    Ok(groups)
}

/// Extract `UNH`/`UNT` message pairs from a flat slice (no UNB/UNZ/UNG/UNE expected).
fn extract_messages_flat<S: SegmentReader>(
    segments: &[S],
    sink: &mut ErrorSink,
    seen_refs: &mut HashSet<String>,
) -> Result<Vec<MessageEnvelope>, EdifactError> {
    let mut messages: Vec<MessageEnvelope> = Vec::new();
    // Index of the `UNH` that opened the message currently being read.  A single
    // `Option` carries the whole "are we inside a message" state, so the index
    // cannot be read while absent.
    let mut unh_idx: Option<usize> = None;

    for (i, seg) in segments.iter().enumerate() {
        match seg.tag() {
            "UNH" => {
                if unh_idx.is_some() {
                    return Err(EdifactError::InvalidSegmentForMessage {
                        tag: "UNH".to_owned(),
                        message_type: "ENVELOPE".to_owned(),
                        span: seg.span(),
                    });
                }
                unh_idx = Some(i);
            }
            "UNT" if unh_idx.is_some() => {
                let msg_start_idx = unh_idx.take().expect("guarded by the match arm");
                let unh = &segments[msg_start_idx];

                let message_ref = sink.required(unh, 0, 0);
                if !seen_refs.insert(message_ref.clone()) {
                    sink.push(EdifactError::DuplicateReference {
                        tag: "UNH".to_owned(),
                        reference: message_ref.clone(),
                        span: unh.span(),
                    });
                }
                let message_type = sink.required(unh, 1, 0);
                let version = sink.required(unh, 1, 1);
                let release = sink.required(unh, 1, 2);
                let controlling_agency = sink.required(unh, 1, 3);
                let association_code = unh.component(1, 4).unwrap_or("").to_owned();
                // UNH element [2]: DE 0068 — common access reference (optional)
                let common_access_ref = unh
                    .component(2, 0)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                // UNH element [3]: S010 composite — sequence of transfers (optional)
                // comp[0] = DE 0070 (sequence number), comp[1] = DE 0073 (position indicator)
                let sequence_of_transfers = unh
                    .component(3, 0)
                    .filter(|s| !s.is_empty())
                    .and_then(|s| s.parse::<u32>().ok());
                let transfer_position = unh
                    .component(3, 1)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);

                let declared_raw = sink.required(seg, 0, 0);
                let declared_segment_count: u32 = sink.recover(
                    declared_raw.parse().map_err(|_| EdifactError::InvalidText {
                        offset: seg.span().start,
                    }),
                    0,
                );
                let unt_ref = sink.required(seg, 1, 0);
                if unt_ref != message_ref {
                    sink.push(EdifactError::QualifierMismatch {
                        tag: "UNT".to_owned(),
                        actual: unt_ref,
                        expected: message_ref.clone(),
                        span: seg.span(),
                    });
                }

                let segment_span = i - msg_start_idx + 1;
                let actual_segment_count = sink.recover(
                    u32::try_from(segment_span).map_err(|_| EdifactError::InterchangeTooLarge {
                        count: segment_span as u64,
                    }),
                    u32::MAX,
                );

                messages.push(MessageEnvelope {
                    message_ref,
                    message_type,
                    version,
                    release,
                    controlling_agency,
                    association_code,
                    common_access_ref,
                    sequence_of_transfers,
                    transfer_position,
                    declared_segment_count,
                    actual_segment_count,
                });
            }
            "UNT" => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: "UNT".to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    span: seg.span(),
                });
            }
            "UNB" | "UNZ" | "UNG" | "UNE" if unh_idx.is_some() => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: seg.tag().to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    span: seg.span(),
                });
            }
            _ if unh_idx.is_none() => {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: seg.tag().to_owned(),
                    message_type: "ENVELOPE".to_owned(),
                    span: seg.span(),
                });
            }
            _ => {}
        }
    }

    if unh_idx.is_some() {
        return Err(EdifactError::MissingSegment {
            tag: "UNT".to_owned(),
            expected_position: "end of message group".to_owned(),
        });
    }

    Ok(messages)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &[u8]) -> Vec<crate::OwnedSegment> {
        crate::from_reader_collect(std::io::Cursor::new(input)).expect("parse failed")
    }

    fn parse_and_validate(input: &[u8]) -> Result<ValidatedInterchange, EdifactError> {
        let owned = parse(input);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        validate_envelope(&segs)
    }

    fn parse_and_validate_lenient(input: &[u8]) -> LenientResult {
        let owned = parse(input);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        validate_envelope_lenient(&segs)
    }

    #[test]
    fn lenient_collects_every_violation_not_just_the_first() {
        // Two independent violations: a UNZ control-reference mismatch and a
        // UNT segment-count mismatch.  The lenient path used to abort on the
        // first and report only one.
        let input = b"UNB+UNOA:1+S+R+200101:0900+CTRL1'\
                      UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+99+1'\
                      UNZ+1+CTRL2'";
        let result = parse_and_validate_lenient(input);
        assert!(
            result.errors.len() >= 2,
            "expected both violations, got {:?}",
            result.errors
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, EdifactError::QualifierMismatch { tag, .. } if tag == "UNZ")),
            "missing UNZ control-ref mismatch in {:?}",
            result.errors
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, EdifactError::SegmentCountMismatch { .. })),
            "missing UNT segment-count mismatch in {:?}",
            result.errors
        );
    }

    #[test]
    fn strict_reports_the_first_error_lenient_collects() {
        // Strict and lenient share one implementation, so strict's error must
        // always be the head of the lenient error list.
        for input in [
            &b"UNB+UNOA:1+S+R+200101:0900+C1'UNH+1+ORDERS:D:96A:UN'UNT+2+1'UNZ+9+C1'"[..],
            &b"UNB+UNOA:1+S+R+200101:0900+C1'UNH+1+ORDERS:D:96A:UN'UNT+99+1'UNZ+1+C1'"[..],
            &b"UNB+UNOA:1+S+R+200101:0900+C1'UNH+1+ORDERS:D:96A:UN'UNT+2+9'UNZ+1+C1'"[..],
        ] {
            let strict = parse_and_validate(input);
            let lenient = parse_and_validate_lenient(input);
            match strict {
                Err(e) => assert_eq!(
                    Some(&e),
                    lenient.errors.first(),
                    "strict/lenient diverged for {:?}",
                    std::str::from_utf8(input).unwrap()
                ),
                Ok(_) => assert!(lenient.errors.is_empty()),
            }
        }
    }

    #[test]
    fn duplicate_message_references_are_rejected() {
        let input = b"UNB+UNOA:1+S+R+200101:0900+C1'\
                      UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'\
                      UNH+1+ORDERS:D:96A:UN'BGM+221'UNT+3+1'\
                      UNZ+2+C1'";
        let err = parse_and_validate(input).expect_err("duplicate UNH refs must fail");
        assert!(
            matches!(&err, EdifactError::DuplicateReference { tag, reference, .. }
                     if tag == "UNH" && reference == "1"),
            "got {err:?}"
        );
    }

    #[test]
    fn distinct_message_references_are_accepted() {
        let input = b"UNB+UNOA:1+S+R+200101:0900+C1'\
                      UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'\
                      UNH+2+ORDERS:D:96A:UN'BGM+221'UNT+3+2'\
                      UNZ+2+C1'";
        parse_and_validate(input).expect("distinct refs must validate");
    }

    #[test]
    fn into_strict_is_total_after_the_caller_filters_errors() {
        // `errors` is a public field, so filtering tolerated violations before
        // converting must not panic.
        let input = b"UNB+UNOA:1+S+R+200101:0900+C1'UNZ+1+C2'";
        let mut lenient = parse_and_validate_lenient(input);
        lenient.errors.clear();
        // Either outcome is acceptable; the contract is only that it does not panic.
        let _ = lenient.into_strict();
    }

    fn parse_and_validate_owned(input: &[u8]) -> Result<ValidatedInterchange, EdifactError> {
        validate_envelope_from_owned(&parse(input))
    }

    const VALID_INTERCHANGE: &[u8] =
        b"UNA:+.? 'UNB+UNOA:3+SENDER::293+RECEIVER::293+230401:0900+00001'UNH+00001+ORDERS:D:11A:UN:EAN010'BGM+220+PO-4711+9'DTM+137:20230401:102'UNT+4+00001'UNZ+1+00001'";

    #[test]
    fn valid_envelope_parses_ok() {
        let result = parse_and_validate(VALID_INTERCHANGE).expect("envelope should be valid");
        assert_eq!(result.interchange.sender_id, "SENDER");
        assert_eq!(result.interchange.sender_qualifier, ""); // no qualifier in fixture
        assert_eq!(result.interchange.recipient_id, "RECEIVER");
        assert_eq!(result.interchange.recipient_qualifier, "");
        assert_eq!(result.interchange.syntax_identifier, "UNOA");
        assert_eq!(result.interchange.syntax_version, "3");
        assert_eq!(result.interchange.control_ref, "00001");
        assert_eq!(result.interchange.declared_unit_count, 1);
        assert_eq!(result.interchange.actual_unit_count, 1);
        assert!(!result.interchange.is_test());
        assert!(!result.has_functional_groups());
        assert_eq!(result.message_count(), 1);
        assert_eq!(result.messages[0].message_type, "ORDERS");
        assert_eq!(result.messages[0].association_code, "EAN010");
        assert_eq!(result.messages[0].declared_segment_count, 4);
        assert_eq!(result.messages[0].actual_segment_count, 4);
    }

    #[test]
    fn valid_envelope_parses_ok_owned_path() {
        let result = parse_and_validate_owned(VALID_INTERCHANGE).expect("envelope should be valid");
        assert_eq!(result.interchange.sender_id, "SENDER");
        assert_eq!(result.interchange.actual_unit_count, 1);
        assert_eq!(result.messages[0].declared_segment_count, 4);
    }

    #[test]
    fn unt_count_mismatch_returns_err() {
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
        assert!(parse_and_validate(input).is_err());
    }

    #[test]
    fn extracts_una_interchange_correctly() {
        let result = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert_eq!(result.interchange.syntax_identifier, "UNOA");
        assert_eq!(result.interchange.syntax_version, "3");
        assert_eq!(result.interchange.date, "230401");
        assert_eq!(result.interchange.time.as_deref(), Some("0900"));
    }

    #[test]
    fn sender_and_recipient_qualifiers_extracted() {
        // GLN-qualified partners: 1234567890123:14 — qualifier at S002 comp 1
        let input = b"UNB+UNOA:3+1234567890123:14+9876543210987:14+200101:0900+1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("GLN-qualified UNB must parse ok");
        assert_eq!(r.interchange.sender_id, "1234567890123");
        assert_eq!(r.interchange.sender_qualifier, "14");
        assert_eq!(r.interchange.recipient_id, "9876543210987");
        assert_eq!(r.interchange.recipient_qualifier, "14");
    }

    // UNB DE 0026 (app_ref) is at element index 6 (ISO 9735-1 §6.1.1):
    // [4]=control_ref [5]=S005/password [6]=0026/app_ref [7]=0029 [8]=0031/ack [9]=0032/comms [10]=0035/test

    #[test]
    fn test_indicator_parsed_from_unb() {
        // DE 0035 (test indicator) is at element index 10 (ISO 9735-1).
        // UNB+...+1 (ctrl) + (S005) + (0026) + (0029) + (0031) + (0032) + 1 (0035)
        //                    [5]       [6]       [7]       [8]       [9]     [10]
        let input = b"UNB+UNOA:3+S+R+200101:0900+1++++++1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("test-flagged UNB must parse ok");
        assert!(
            r.interchange.test_indicator,
            "test_indicator should be true"
        );
        assert!(r.interchange.is_test(), "is_test() convenience must agree");
    }

    #[test]
    fn no_test_indicator_defaults_false() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert!(!r.interchange.test_indicator);
        assert!(!r.interchange.is_test());
    }

    #[test]
    fn app_ref_extracted_when_present() {
        // DE 0026 (app_ref) is at element index 6; element [5] (S005 password) is empty.
        let input = b"UNB+UNOA:3+S+R+200101:0900+1++MYAPP'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNB with app_ref must parse ok");
        assert_eq!(r.interchange.app_ref.as_deref(), Some("MYAPP"));
    }

    #[test]
    fn app_ref_is_none_when_absent() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert!(r.interchange.app_ref.is_none());
    }

    #[test]
    fn recipient_password_extracted_when_present() {
        // DE 0022 (recipient password) is at element index 5 (S005 comp 0).
        let input = b"UNB+UNOA:3+S+R+200101:0900+1+MYPASS'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNB with password must parse ok");
        assert_eq!(r.interchange.recipient_password.as_deref(), Some("MYPASS"));
    }

    #[test]
    fn recipient_password_is_none_when_absent() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert!(r.interchange.recipient_password.is_none());
    }

    #[test]
    fn acknowledgement_request_flag_parsed() {
        // DE 0031 (ack request) at element index 8; set to "1".
        // Elements: [5]S005="" [6]0026="" [7]0029="" [8]0031="1"
        let input = b"UNB+UNOA:3+S+R+200101:0900+1++++1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNB with ack-request must parse ok");
        assert!(r.interchange.acknowledgement_request);
        assert!(r.interchange.ack_requested());
    }

    #[test]
    fn acknowledgement_request_defaults_false() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert!(!r.interchange.acknowledgement_request);
        assert!(!r.interchange.ack_requested());
    }

    #[test]
    fn communications_agreement_id_extracted() {
        // DE 0032 at element index 9; elements [5]-[8] empty.
        let input = b"UNB+UNOA:3+S+R+200101:0900+1+++++AGREEMENT-1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNB with comms-agreement must parse ok");
        assert_eq!(
            r.interchange.communications_agreement_id.as_deref(),
            Some("AGREEMENT-1")
        );
    }

    #[test]
    fn dangling_unh_without_unt_returns_err() {
        let input =
            b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::MissingSegment { ref tag, .. }) if tag == "UNT")
        );
    }

    #[test]
    fn stray_segment_outside_message_returns_err() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'BGM+999+PO-2+9'UNZ+1+1'";
        assert!(matches!(
            parse_and_validate(input),
            Err(EdifactError::InvalidSegmentForMessage { .. })
        ));
    }

    #[test]
    fn missing_unb_sender_component_returns_err() {
        let input = b"UNB+UNOA:3++R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+1+1'";
        let result = parse_and_validate(input);
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
        assert!(
            matches!(result, Err(EdifactError::QualifierMismatch { ref tag, .. }) if tag == "UNT")
        );
    }

    #[test]
    fn unz_control_reference_must_match_unb() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+1+999'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::QualifierMismatch { ref tag, .. }) if tag == "UNZ")
        );
    }

    #[test]
    fn missing_unh_message_type_components_return_err() {
        let input =
            b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A'BGM+220+PO-1+9'UNT+3+1'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::MissingRequiredComponent { ref tag, element_index: 1, component_index: 3 }) if tag == "UNH"),
            "got: {result:?}"
        );
    }

    #[test]
    fn nested_unz_inside_message_returns_err() {
        let input =
            b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'UNZ+1+1'UNT+2+1'UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::InvalidSegmentForMessage { ref tag, .. }) if tag == "UNZ")
        );
    }

    #[test]
    fn lenient_returns_partial_result_on_count_mismatch() {
        // UNZ says 2 but only 1 message — lenient mode must return Some(partial)
        // along with the error, not None.
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+2+1'";
        let owned = parse(input);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        let lenient = validate_envelope_lenient(&segs);
        let result = lenient.interchange;
        let errors = lenient.errors;
        assert!(
            result.is_some(),
            "lenient mode must return Some even on count mismatch"
        );
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                EdifactError::MessageCountMismatch {
                    expected: 2,
                    actual: 1
                }
            ),
            "expected MessageCountMismatch(2,1), got {:?}",
            errors[0]
        );
        let partial = result.unwrap();
        assert_eq!(partial.messages.len(), 1);
        assert_eq!(partial.interchange.actual_unit_count, 1);
        assert_eq!(partial.interchange.declared_unit_count, 2);
    }

    #[test]
    fn lenient_returns_none_on_structural_error() {
        // Missing UNB — no structure at all, expect None
        let input = b"UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+1+1'";
        let owned = parse(input);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        let lenient = validate_envelope_lenient(&segs);
        let result = lenient.interchange;
        let errors = lenient.errors;
        assert!(result.is_none(), "missing UNB must yield None");
        assert!(!errors.is_empty());
    }

    #[test]
    fn message_count_convenience_method() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert_eq!(r.message_count(), r.messages.len());
        assert_eq!(r.message_count(), 1);
    }

    #[test]
    fn interchange_with_single_functional_group_parses_ok() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+S+R+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+1'\
                      UNZ+1+1'";
        let result = parse_and_validate(input).expect("single-group interchange must parse ok");
        assert!(result.has_functional_groups());
        assert_eq!(result.functional_groups.len(), 1);
        let g = &result.functional_groups[0];
        assert_eq!(g.group_id, "ORDERS");
        assert_eq!(g.group_ref, "1");
        assert_eq!(g.controlling_agency, "UN");
        assert_eq!(g.declared_message_count, 1);
        assert_eq!(g.actual_message_count, 1);
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].message_type, "ORDERS");
        assert_eq!(result.interchange.declared_unit_count, 1);
        assert_eq!(result.interchange.actual_unit_count, 1);
    }

    #[test]
    fn interchange_with_multi_message_group_parses_ok() {
        // One group containing 2 messages — UNZ = 1 group, UNE = 2 messages.
        // This is the key case that strip_functional_group_segments breaks.
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+S+R+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNH+2+ORDERS:D:96A:UN'\
                      BGM+220+PO-002+9'\
                      UNT+3+2'\
                      UNE+2+1'\
                      UNZ+1+1'";
        let result = parse_and_validate(input).expect("multi-message group must parse ok");
        assert!(result.has_functional_groups());
        assert_eq!(result.functional_groups.len(), 1);
        assert_eq!(result.functional_groups[0].actual_message_count, 2);
        assert_eq!(result.messages.len(), 2);
        // UNZ = 1 group (not 2 messages): ISO 9735-1 §9.2
        assert_eq!(result.interchange.actual_unit_count, 1);
        assert_eq!(result.interchange.declared_unit_count, 1);
    }

    #[test]
    fn interchange_with_multiple_groups_parses_ok() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+2'\
                      UNG+ORDERS+S+R+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+1'\
                      UNG+INVOIC+S+R+200101:0900+2+UN+D:96A'\
                      UNH+2+INVOIC:D:96A:UN'\
                      BGM+380+INV-001+9'\
                      UNT+3+2'\
                      UNE+1+2'\
                      UNZ+2+2'";
        let result = parse_and_validate(input).expect("multi-group interchange must parse ok");
        assert_eq!(result.functional_groups.len(), 2);
        assert_eq!(result.functional_groups[0].group_id, "ORDERS");
        assert_eq!(result.functional_groups[1].group_id, "INVOIC");
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.interchange.declared_unit_count, 2);
        assert_eq!(result.interchange.actual_unit_count, 2);
    }

    #[test]
    fn ung_une_count_mismatch_returns_err() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+S+R+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+2+1'\
                      UNZ+1+1'";
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
    fn une_without_ung_returns_err() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNE+1+1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::InvalidSegmentForMessage { ref tag, .. }) if tag == "UNE"),
            "expected InvalidSegmentForMessage(UNE), got {result:?}"
        );
    }

    #[test]
    fn ung_without_une_returns_err() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+S+R+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::MissingSegment { ref tag, .. }) if tag == "UNE"),
            "expected MissingSegment(UNE), got {result:?}"
        );
    }

    #[test]
    fn une_group_ref_must_match_ung() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+S+R+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+999'\
                      UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::QualifierMismatch { ref tag, .. }) if tag == "UNE"),
            "expected QualifierMismatch(UNE), got {result:?}"
        );
    }

    // ── New-field tests (ISO 9735-1 completeness) ─────────────────────────────

    #[test]
    fn processing_priority_extracted_when_present() {
        // DE 0029 at element index 7: [5]=S005="" [6]=0026="" [7]=0029="A"
        let input = b"UNB+UNOA:3+S+R+200101:0900+1+++A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNB with processing_priority must parse ok");
        assert_eq!(r.interchange.processing_priority.as_deref(), Some("A"));
    }

    #[test]
    fn processing_priority_is_none_when_absent() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert!(r.interchange.processing_priority.is_none());
    }

    #[test]
    fn sender_routing_address_extracted_when_present() {
        // S002: sender_id:qualifier:routing  → comp[2] = routing address
        let input = b"UNB+UNOA:3+SENDER:14:ROUTEA+RECIP+200101:0900+1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNB with sender routing must parse ok");
        assert_eq!(
            r.interchange.sender_routing_address.as_deref(),
            Some("ROUTEA")
        );
        assert!(r.interchange.recipient_routing_address.is_none());
    }

    #[test]
    fn recipient_routing_address_extracted_when_present() {
        // S003: recipient_id:qualifier:routing → comp[2] = routing address
        let input = b"UNB+UNOA:3+SENDER+RECIP:14:ROUTEB+200101:0900+1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNB with recipient routing must parse ok");
        assert!(r.interchange.sender_routing_address.is_none());
        assert_eq!(
            r.interchange.recipient_routing_address.as_deref(),
            Some("ROUTEB")
        );
    }

    #[test]
    fn routing_address_is_none_when_absent() {
        // Plain S+R without sub-components — no routing addresses in S002/S003
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).unwrap();
        assert!(r.interchange.sender_routing_address.is_none());
        assert!(r.interchange.recipient_routing_address.is_none());
    }

    #[test]
    fn common_access_ref_extracted_when_present() {
        // UNH element [2] (DE 0068): common access reference
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNH+1+ORDERS:D:96A:UN+COMREF1'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNH with common_access_ref must parse ok");
        assert_eq!(r.messages[0].common_access_ref.as_deref(), Some("COMREF1"));
    }

    #[test]
    fn common_access_ref_is_none_when_absent() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert!(r.messages[0].common_access_ref.is_none());
    }

    #[test]
    fn parse_unh_includes_message_ref() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNH+REF42+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+REF42'\
                      UNZ+1+1'";
        // Validate the high-level API which exercises parse_unh internally;
        // the message_ref field on MessageEnvelope should equal the UNH DE 0062 value.
        let r = parse_and_validate(input).expect("must parse ok");
        assert_eq!(r.messages[0].message_ref, "REF42");
        assert_eq!(r.messages[0].message_type, "ORDERS");
    }

    #[test]
    fn iter_messages_by_type_returns_matching_messages() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+CTRL2'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNH+2+INVOIC:D:96A:UN'\
                      BGM+380+INV-001+9'\
                      UNT+3+2'\
                      UNZ+2+CTRL2'";
        let r = parse_and_validate(input).expect("two-message interchange must parse ok");
        let orders: Vec<_> = r.iter_messages_by_type("ORDERS").collect();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].message_ref, "1");
        let invoices: Vec<_> = r.iter_messages_by_type("INVOIC").collect();
        assert_eq!(invoices.len(), 1);
        assert_eq!(invoices[0].message_ref, "2");
        let none: Vec<_> = r.iter_messages_by_type("DESADV").collect();
        assert!(none.is_empty());
    }

    #[test]
    fn display_interchange_envelope() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        let s = r.interchange.to_string();
        assert!(s.contains("SENDER"), "Display must include sender_id");
        assert!(s.contains("UNOA"), "Display must include syntax_identifier");
    }

    #[test]
    fn display_message_envelope() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        let s = r.messages[0].to_string();
        assert!(s.contains("ORDERS"), "Display must include message_type");
        assert!(s.contains("ref="), "Display must include message ref label");
    }

    #[test]
    fn display_validated_interchange() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        let s = r.to_string();
        assert!(
            s.contains("messages="),
            "Display must include message count label"
        );
    }

    // ── DE 0001 syntax identifier validation ─────────────────────────────────

    #[test]
    fn unrecognised_syntax_identifier_returns_err() {
        // DE 0001 "XXXX" is not in the ISO 9735-1 §3.1 defined list.
        let input = b"UNB+XXXX:3+S+R+200101:0900+1'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result, Err(EdifactError::UnrecognisedSyntaxIdentifier(ref id)) if id == "XXXX"),
            "expected UnrecognisedSyntaxIdentifier(\"XXXX\"), got {result:?}"
        );
    }

    #[test]
    fn all_valid_syntax_identifiers_accepted() {
        for id in &["UNOA", "UNOB", "UNOC", "UNOD", "UNOE", "UNOF", "KECA"] {
            let input = format!(
                "UNB+{id}:3+S+R+200101:0900+1'UNH+1+ORDERS:D:96A:UN'BGM+220+PO-001+9'UNT+3+1'UNZ+1+1'"
            );
            let result = parse_and_validate(input.as_bytes());
            assert!(
                result.is_ok(),
                "syntax id '{id}' should be accepted, got {result:?}"
            );
        }
    }

    // ── UNB S005 password qualifier ───────────────────────────────────────────

    #[test]
    fn recipient_password_qualifier_extracted_when_present() {
        // S005: MYPASS:AA — comp[0]=password, comp[1]=qualifier (DE 0025)
        let input = b"UNB+UNOA:3+S+R+200101:0900+1+MYPASS:AA'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNB with password+qualifier must parse ok");
        assert_eq!(r.interchange.recipient_password.as_deref(), Some("MYPASS"));
        assert_eq!(
            r.interchange.recipient_password_qualifier.as_deref(),
            Some("AA")
        );
    }

    #[test]
    fn recipient_password_qualifier_is_none_when_absent() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert!(r.interchange.recipient_password_qualifier.is_none());
    }

    // ── UNH S010 sequence-of-transfers ───────────────────────────────────────

    #[test]
    fn sequence_of_transfers_extracted_when_present() {
        // UNH element [2] = common access ref, element [3] = S010 (seq:position)
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNH+1+ORDERS:D:96A:UN++2:C'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNH with S010 must parse ok");
        assert_eq!(r.messages[0].sequence_of_transfers, Some(2));
        assert_eq!(r.messages[0].transfer_position.as_deref(), Some("C"));
    }

    #[test]
    fn sequence_of_transfers_none_when_absent() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        assert!(r.messages[0].sequence_of_transfers.is_none());
        assert!(r.messages[0].transfer_position.is_none());
    }

    // ── UNG S006/S007 application qualifiers ─────────────────────────────────

    #[test]
    fn ung_app_sender_and_recipient_qualifiers_extracted() {
        // UNG: group_id + S006(app_sender:qualifier) + S007(app_recip:qualifier) + ...
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+APPSEND:ZZZ+APPRECV:14+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("UNG with qualifiers must parse ok");
        let g = &r.functional_groups[0];
        assert_eq!(g.app_sender, "APPSEND");
        assert_eq!(g.app_sender_qualifier, "ZZZ");
        assert_eq!(g.app_recipient, "APPRECV");
        assert_eq!(g.app_recipient_qualifier, "14");
    }

    #[test]
    fn ung_qualifiers_empty_when_absent() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+S+R+200101:0900+1+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+1'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).unwrap();
        let g = &r.functional_groups[0];
        assert_eq!(g.app_sender_qualifier, "");
        assert_eq!(g.app_recipient_qualifier, "");
    }

    // ── LenientResult methods ─────────────────────────────────────────────────

    #[test]
    fn lenient_result_is_valid_true_on_clean_interchange() {
        let owned = parse(VALID_INTERCHANGE);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        let r = validate_envelope_lenient(&segs);
        assert!(r.is_valid());
        assert!(r.errors.is_empty());
        assert!(r.interchange.is_some());
    }

    #[test]
    fn lenient_result_into_strict_ok_path() {
        let owned = parse(VALID_INTERCHANGE);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        let r = validate_envelope_lenient(&segs);
        let strict = r.into_strict();
        assert!(
            strict.is_ok(),
            "into_strict() should succeed for valid interchange"
        );
        assert_eq!(strict.unwrap().messages.len(), 1);
    }

    #[test]
    fn lenient_result_into_strict_err_path() {
        // Count mismatch → into_strict() returns Err with the error
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+2+1'";
        let owned = parse(input);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        let r = validate_envelope_lenient(&segs);
        assert!(!r.is_valid());
        let strict = r.into_strict();
        assert!(strict.is_err());
        let errors = strict.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            EdifactError::MessageCountMismatch {
                expected: 2,
                actual: 1
            }
        ));
    }

    // ── Direct parse_unh / parse_ung API ─────────────────────────────────────

    #[test]
    fn parse_unh_direct_extracts_all_s009_fields() {
        // parse_unh is called internally by extract_messages_flat; all S009 fields
        // it extracts surface in the resulting MessageEnvelope.  We verify them here
        // rather than calling parse_unh(&seg) from a Vec<Segment<'_>>, which would
        // conflict with the SmallVec-based Element drop-check (see API docs).
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNH+REF99+ORDERS:D:96A:UN:EAN010'\
                      BGM+220+PO-001+9'\
                      UNT+3+REF99'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("must parse ok");
        let msg = &r.messages[0];
        assert_eq!(msg.message_ref, "REF99");
        assert_eq!(msg.message_type, "ORDERS");
        assert_eq!(msg.version, "D");
        assert_eq!(msg.release, "96A");
        assert_eq!(msg.controlling_agency, "UN");
        assert_eq!(msg.association_code, "EAN010");
    }

    #[test]
    fn parse_ung_direct_extracts_identifier_fields() {
        // parse_ung fields surface via the FunctionalGroupEnvelope returned by validation.
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+APPSEND:ZZZ+APPRECV:14+200101:0900+GRP01+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+GRP01'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("must parse ok");
        let g = &r.functional_groups[0];
        assert_eq!(g.group_id, "ORDERS");
        assert_eq!(g.app_sender, "APPSEND");
        assert_eq!(g.app_sender_qualifier, "ZZZ");
        assert_eq!(g.app_recipient, "APPRECV");
        assert_eq!(g.app_recipient_qualifier, "14");
        assert_eq!(g.group_ref, "GRP01");
        assert_eq!(g.controlling_agency, "UN");
        assert_eq!(g.version, "D");
        assert_eq!(g.release, "96A");
    }

    // ── find_message convenience method ──────────────────────────────────────

    #[test]
    fn find_message_returns_correct_message_by_ref() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+CTRL2'\
                      UNH+REF-A+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+REF-A'\
                      UNH+REF-B+INVOIC:D:96A:UN'\
                      BGM+380+INV-001+9'\
                      UNT+3+REF-B'\
                      UNZ+2+CTRL2'";
        let r = parse_and_validate(input).expect("two-message interchange must parse ok");
        let msg_a = r.find_message("REF-A");
        assert!(msg_a.is_some());
        assert_eq!(msg_a.unwrap().message_type, "ORDERS");
        let msg_b = r.find_message("REF-B");
        assert!(msg_b.is_some());
        assert_eq!(msg_b.unwrap().message_type, "INVOIC");
        assert!(r.find_message("MISSING").is_none());
    }

    // ── UNG missing mandatory group_ref ──────────────────────────────────────

    #[test]
    fn ung_missing_group_ref_returns_err() {
        // UNG with element [4] (group ref) empty — must error, not silently use ""
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+S+R+200101:0900++UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+'\
                      UNZ+1+1'";
        let result = parse_and_validate(input);
        assert!(
            matches!(result,
                Err(EdifactError::MissingRequiredComponent { ref tag, element_index: 4, component_index: 0 })
                if tag == "UNG"
            ),
            "expected MissingRequiredComponent for empty UNG group_ref, got {result:?}"
        );
    }

    // ── Edge case and ergonomics tests ────────────────────────────────────────

    #[test]
    fn empty_segment_list_returns_missing_unb() {
        // Contract: validate_envelope(&[]) must return MissingSegment{UNB},
        // not panic or return Ok.
        let result = validate_envelope(&[]);
        assert!(
            matches!(result, Err(EdifactError::MissingSegment { ref tag, .. }) if tag == "UNB"),
            "expected MissingSegment(UNB) for empty input, got {result:?}"
        );
    }

    #[test]
    fn single_segment_only_unb_returns_missing_unz() {
        // Only UNB, no UNZ — should fail with MissingSegment{UNZ}.
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'";
        let owned = parse(input);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        let result = validate_envelope(&segs);
        assert!(
            matches!(result, Err(EdifactError::MissingSegment { ref tag, .. }) if tag == "UNZ"),
            "expected MissingSegment(UNZ) for UNB-only input, got {result:?}"
        );
    }

    #[test]
    fn display_validated_interchange_includes_count() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        let s = r.to_string();
        // Must include the numeric count, not just the label
        assert!(
            s.contains("messages=1"),
            "Display must include count '1': {s}"
        );
    }

    #[test]
    fn display_interchange_envelope_contains_arrow() {
        let r = parse_and_validate(VALID_INTERCHANGE).unwrap();
        let s = r.interchange.to_string();
        // Must use ASCII arrow, not Unicode →
        assert!(s.contains("->"), "Display must use ASCII '->' arrow: {s}");
        assert!(
            !s.contains('\u{2192}'),
            "Display must not use Unicode → arrow: {s}"
        );
    }

    #[test]
    fn display_functional_group_envelope() {
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'\
                      UNG+ORDERS+APPSEND+APPRECV+200101:0900+GRP01+UN+D:96A'\
                      UNH+1+ORDERS:D:96A:UN'\
                      BGM+220+PO-001+9'\
                      UNT+3+1'\
                      UNE+1+GRP01'\
                      UNZ+1+1'";
        let r = parse_and_validate(input).expect("must parse ok");
        let g = &r.functional_groups[0];
        let s = g.to_string();
        assert!(s.contains("ORDERS"), "Display must include group_id: {s}");
        assert!(
            s.contains("APPSEND"),
            "Display must include app_sender: {s}"
        );
        assert!(
            s.contains("APPRECV"),
            "Display must include app_recipient: {s}"
        );
        assert!(s.contains("GRP01"), "Display must include group_ref: {s}");
        assert!(
            s.contains("msgs="),
            "Display must include message count label: {s}"
        );
        assert!(
            s.contains("1/1"),
            "Display must include actual/declared counts: {s}"
        );
    }

    #[test]
    fn lenient_has_errors_is_inverse_of_is_valid() {
        let owned = parse(VALID_INTERCHANGE);
        let segs: Vec<Segment<'_>> = owned.iter().map(crate::OwnedSegment::as_borrowed).collect();
        let valid = validate_envelope_lenient(&segs);
        assert!(valid.is_valid());
        assert!(!valid.has_errors());

        // Count mismatch: is_valid() == false, has_errors() == true
        let input = b"UNB+UNOA:3+S+R+200101:0900+1'UNH+1+ORDERS:D:11A:UN:EAN010'BGM+220+PO-1+9'UNT+3+1'UNZ+2+1'";
        let owned2 = parse(input);
        let segs2: Vec<Segment<'_>> = owned2
            .iter()
            .map(crate::OwnedSegment::as_borrowed)
            .collect();
        let invalid = validate_envelope_lenient(&segs2);
        assert!(!invalid.is_valid());
        assert!(invalid.has_errors());
    }
}
