//! `CONTRL` — the syntax and service report message (ISO 9735-4).
//!
//! `CONTRL` is how one EDI partner tells another what happened to an
//! interchange: that it arrived, that it was syntactically accepted, or that it
//! was rejected and precisely where. It is not an application-level reply —
//! ISO 9735-4 §5.2 is explicit that acknowledging an interchange with `CONTRL`
//! says nothing about whether the business content was agreed to.
//!
//! # The two messages
//!
//! §5.3.1 defines a maximum of two `CONTRL` messages per subject interchange:
//!
//! 1. **Receipt** — optional, sent immediately, action code `8`. Built with
//!    [`Contrl::receipt`].
//! 2. **Acknowledgement or rejection** — sent after the syntax check, action
//!    code `7` or `4`. Built with [`Contrl::acknowledgement`] or
//!    [`Contrl::from_report`].
//!
//! If the subject interchange requested an acknowledgement (`UNB` DE 0031 —
//! [`InterchangeEnvelope::ack_requested`]), the second message is mandatory.
//!
//! # Reporting levels
//!
//! The message nests five reporting levels, each naming a part of the subject
//! interchange and each able to carry **one** error code (§5.3.3):
//!
//! ```text
//! UCI   the interchange          → UNB / UNZ
//!  UCF  a group                  → UNG / UNE
//!   UCM a message or package     → UNH / UNT
//!    UCS a segment               → by position, UNH = 1
//!     UCD a data element         → by position within that segment
//! ```
//!
//! §5.3.3 also requires the *lowest* level that can express an error to be the
//! one that reports it, and forbids repeating the same code further up. This
//! module follows both rules: an issue that names a segment becomes a `UCS`, one
//! that names a data element becomes a `UCD`, and only an issue about the
//! envelope itself reaches `UCI`.
//!
//! # Example
//!
//! ```
//! use edifact_rs::{Contrl, from_bytes, validate_envelope};
//!
//! let raw = b"UNB+UNOC:3+SENDER+RECEIVER+260101:0900+IC4711'\
//!             UNH+MSG1+ORDERS:D:96A:UN'BGM+220+PO-1+9'UNT+3+MSG1'\
//!             UNZ+1+IC4711'";
//! let segments: Vec<_> = from_bytes(raw).collect::<Result<Vec<_>, _>>()?;
//! let validated = validate_envelope(&segments)?;
//!
//! let contrl = Contrl::acknowledgement(&validated).with_message_reference("ACK1");
//! let wire = contrl.to_edifact_string()?;
//!
//! assert!(wire.starts_with("UNH+ACK1+CONTRL:4:1:UN'"));
//! assert!(wire.contains("UCI+IC4711+SENDER+RECEIVER+7'"));
//! # Ok::<(), edifact_rs::EdifactError>(())
//! ```

use crate::envelope::{
    FunctionalGroupEnvelope, InterchangeEnvelope, MessageEnvelope, ValidatedInterchange,
};
use crate::model::{OwnedElement, OwnedSegment, Segment};
use crate::report::{ValidationIssue, ValidationReport};
use crate::{EdifactError, Writer};

// ── action codes (DE 0083) ────────────────────────────────────────────────────

/// `CONTRL` action code — DE 0083, the verdict on one reporting level.
///
/// ISO 9735-4 §5.3.2 restricts which codes may appear in which message: `4` and
/// `7` only after a complete syntax check, `8` only in a receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Action {
    /// `4` — this level and all lower levels rejected.
    Rejected,
    /// `7` — this level acknowledged; every lower level is acknowledged too
    /// unless a reporting level explicitly rejects it.
    Acknowledged,
    /// `8` — interchange received. Only valid in a receipt message.
    Received,
}

impl Action {
    /// The DE 0083 code value.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Rejected => "4",
            Self::Acknowledged => "7",
            Self::Received => "8",
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

// ── reporting levels ──────────────────────────────────────────────────────────

/// One of the five `CONTRL` reporting levels.
///
/// Which level may carry which error code is fixed by ISO 9735-4 Annex A; see
/// [`SyntaxError::permitted_at`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReportingLevel {
    /// `UCI` — the interchange.
    Interchange,
    /// `UCF` — a group.
    Group,
    /// `UCM` — a message or package.
    Message,
    /// `UCS` — a segment.
    Segment,
    /// `UCD` — a data element.
    DataElement,
}

impl ReportingLevel {
    /// The segment tag that carries this level.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Interchange => "UCI",
            Self::Group => "UCF",
            Self::Message => "UCM",
            Self::Segment => "UCS",
            Self::DataElement => "UCD",
        }
    }
}

// ── syntax error codes (DE 0085) ──────────────────────────────────────────────

/// `CONTRL` syntax error code — DE 0085, the nature of one fault.
///
/// The variants are exactly the code set of ISO 9735-4 Annex A, and
/// [`permitted_at`][Self::permitted_at] reproduces that annex's table of which
/// code may be used at which reporting level. Emitting a code at a level the
/// annex forbids produces a `CONTRL` the partner's translator will reject, so
/// [`Contrl`] checks it rather than trusting the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SyntaxError {
    /// `2` — syntax version or level not supported.
    SyntaxVersionNotSupported,
    /// `7` — interchange recipient not actual recipient.
    NotActualRecipient,
    /// `12` — invalid value.
    InvalidValue,
    /// `13` — missing.
    Missing,
    /// `14` — value not supported in this position.
    ValueNotSupportedHere,
    /// `15` — not supported in this position.
    NotSupportedHere,
    /// `16` — too many constituents.
    TooManyConstituents,
    /// `17` — no agreement.
    NoAgreement,
    /// `18` — unspecified error.
    Unspecified,
    /// `20` — character invalid as service character.
    InvalidAsServiceCharacter,
    /// `21` — invalid character(s).
    InvalidCharacters,
    /// `22` — invalid service character(s).
    InvalidServiceCharacters,
    /// `23` — unknown interchange sender.
    UnknownSender,
    /// `24` — too old.
    TooOld,
    /// `25` — test indicator not supported.
    TestIndicatorNotSupported,
    /// `26` — duplicate detected.
    DuplicateDetected,
    /// `28` — references do not match.
    ReferencesDoNotMatch,
    /// `29` — control or octet count does not match number of instances received.
    ControlCountMismatch,
    /// `30` — groups and messages/packages mixed.
    GroupsAndMessagesMixed,
    /// `32` — lower level empty.
    LowerLevelEmpty,
    /// `33` — invalid occurrence outside message, package or group.
    InvalidOccurrenceOutsideMessage,
    /// `35` — too many repetitions.
    TooManyRepetitions,
    /// `36` — too many segment group repetitions.
    TooManyGroupRepetitions,
    /// `37` — invalid type of character(s).
    InvalidCharacterType,
    /// `39` — data element too long.
    DataElementTooLong,
    /// `40` — data element too short.
    DataElementTooShort,
    /// `45` — trailing separator.
    TrailingSeparator,
    /// `46` — character set not supported.
    CharacterSetNotSupported,
    /// `47` — envelope functionality not supported.
    EnvelopeFunctionalityNotSupported,
}

impl SyntaxError {
    /// The DE 0085 code value.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SyntaxVersionNotSupported => "2",
            Self::NotActualRecipient => "7",
            Self::InvalidValue => "12",
            Self::Missing => "13",
            Self::ValueNotSupportedHere => "14",
            Self::NotSupportedHere => "15",
            Self::TooManyConstituents => "16",
            Self::NoAgreement => "17",
            Self::Unspecified => "18",
            Self::InvalidAsServiceCharacter => "20",
            Self::InvalidCharacters => "21",
            Self::InvalidServiceCharacters => "22",
            Self::UnknownSender => "23",
            Self::TooOld => "24",
            Self::TestIndicatorNotSupported => "25",
            Self::DuplicateDetected => "26",
            Self::ReferencesDoNotMatch => "28",
            Self::ControlCountMismatch => "29",
            Self::GroupsAndMessagesMixed => "30",
            Self::LowerLevelEmpty => "32",
            Self::InvalidOccurrenceOutsideMessage => "33",
            Self::TooManyRepetitions => "35",
            Self::TooManyGroupRepetitions => "36",
            Self::InvalidCharacterType => "37",
            Self::DataElementTooLong => "39",
            Self::DataElementTooShort => "40",
            Self::TrailingSeparator => "45",
            Self::CharacterSetNotSupported => "46",
            Self::EnvelopeFunctionalityNotSupported => "47",
        }
    }

    /// The Annex A code name.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::SyntaxVersionNotSupported => "syntax version or level not supported",
            Self::NotActualRecipient => "interchange recipient not actual recipient",
            Self::InvalidValue => "invalid value",
            Self::Missing => "missing",
            Self::ValueNotSupportedHere => "value not supported in this position",
            Self::NotSupportedHere => "not supported in this position",
            Self::TooManyConstituents => "too many constituents",
            Self::NoAgreement => "no agreement",
            Self::Unspecified => "unspecified error",
            Self::InvalidAsServiceCharacter => "character invalid as service character",
            Self::InvalidCharacters => "invalid character(s)",
            Self::InvalidServiceCharacters => "invalid service character(s)",
            Self::UnknownSender => "unknown interchange sender",
            Self::TooOld => "too old",
            Self::TestIndicatorNotSupported => "test indicator not supported",
            Self::DuplicateDetected => "duplicate detected",
            Self::ReferencesDoNotMatch => "references do not match",
            Self::ControlCountMismatch => {
                "control or octet count does not match number of instances received"
            }
            Self::GroupsAndMessagesMixed => "groups and messages/packages mixed",
            Self::LowerLevelEmpty => "lower level empty",
            Self::InvalidOccurrenceOutsideMessage => {
                "invalid occurrence outside message, package or group"
            }
            Self::TooManyRepetitions => "too many repetitions",
            Self::TooManyGroupRepetitions => "too many segment group repetitions",
            Self::InvalidCharacterType => "invalid type of character(s)",
            Self::DataElementTooLong => "data element too long",
            Self::DataElementTooShort => "data element too short",
            Self::TrailingSeparator => "trailing separator",
            Self::CharacterSetNotSupported => "character set not supported",
            Self::EnvelopeFunctionalityNotSupported => "envelope functionality not supported",
        }
    }

    /// Whether ISO 9735-4 Annex A allows this code at `level`.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::contrl::{ReportingLevel, SyntaxError};
    ///
    /// // "Too many segment group repetitions" is a segment-level finding only.
    /// assert!(SyntaxError::TooManyGroupRepetitions.permitted_at(ReportingLevel::Segment));
    /// assert!(!SyntaxError::TooManyGroupRepetitions.permitted_at(ReportingLevel::Interchange));
    ///
    /// // "Unknown interchange sender" can only be said about the interchange.
    /// assert!(SyntaxError::UnknownSender.permitted_at(ReportingLevel::Interchange));
    /// assert!(!SyntaxError::UnknownSender.permitted_at(ReportingLevel::Message));
    /// ```
    #[must_use]
    pub const fn permitted_at(self, level: ReportingLevel) -> bool {
        use ReportingLevel as L;
        match self {
            // Interchange-only findings.
            Self::SyntaxVersionNotSupported
            | Self::NotActualRecipient
            | Self::InvalidAsServiceCharacter
            | Self::UnknownSender
            | Self::CharacterSetNotSupported => matches!(l_of(level), L::Interchange),
            // Everywhere.
            Self::InvalidValue
            | Self::Missing
            | Self::ValueNotSupportedHere
            | Self::NotSupportedHere
            | Self::TooManyConstituents
            | Self::Unspecified
            | Self::InvalidCharacters => true,
            // Not at the data-element level.
            Self::NoAgreement | Self::TestIndicatorNotSupported => matches!(
                l_of(level),
                L::Interchange | L::Group | L::Message | L::Segment
            ),
            Self::InvalidServiceCharacters => true,
            Self::TooOld => matches!(l_of(level), L::Interchange | L::Group),
            Self::DuplicateDetected
            | Self::ReferencesDoNotMatch
            | Self::ControlCountMismatch
            | Self::GroupsAndMessagesMixed => {
                matches!(l_of(level), L::Interchange | L::Group | L::Message)
            }
            Self::LowerLevelEmpty | Self::InvalidOccurrenceOutsideMessage => {
                matches!(l_of(level), L::Interchange | L::Group)
            }
            Self::TooManyRepetitions => {
                matches!(l_of(level), L::Message | L::Segment | L::DataElement)
            }
            Self::TooManyGroupRepetitions => matches!(l_of(level), L::Segment),
            Self::InvalidCharacterType | Self::DataElementTooLong | Self::DataElementTooShort => {
                matches!(
                    l_of(level),
                    L::Interchange | L::Group | L::Message | L::DataElement
                )
            }
            Self::TrailingSeparator => matches!(
                l_of(level),
                L::Interchange | L::Group | L::Message | L::Segment
            ),
            Self::EnvelopeFunctionalityNotSupported => matches!(l_of(level), L::Group | L::Message),
        }
    }

    /// The `CONTRL` code that best names an [`EdifactError`].
    ///
    /// ISO 9735-4 §5.3.3 asks for the most precise code available and warns
    /// against reaching for a general one when a specific one fits, so this maps
    /// each variant to the narrowest Annex A code that is true of it, falling
    /// back to `18` (unspecified) only where the annex genuinely offers nothing
    /// better.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::contrl::SyntaxError;
    /// use edifact_rs::EdifactError;
    ///
    /// let err = EdifactError::MessageCountMismatch { expected: 2, actual: 1 };
    /// assert_eq!(SyntaxError::for_error(&err), SyntaxError::ControlCountMismatch);
    /// assert_eq!(SyntaxError::for_error(&err).code(), "29");
    /// ```
    #[must_use]
    pub fn for_error(error: &EdifactError) -> Self {
        use EdifactError as E;
        match error {
            E::MessageCountMismatch { .. } | E::SegmentCountMismatch { .. } => {
                Self::ControlCountMismatch
            }
            E::QualifierMismatch { .. } => Self::ReferencesDoNotMatch,
            E::DuplicateReference { .. } => Self::DuplicateDetected,
            E::MissingRequiredElement { .. }
            | E::MissingRequiredComponent { .. }
            | E::MissingSegment { .. } => Self::Missing,
            E::InvalidCodeValue { .. } | E::InvalidFieldValue { .. } => Self::InvalidValue,
            E::InvalidSegmentForMessage { .. } | E::ConditionalRequirementNotMet { .. } => {
                Self::NotSupportedHere
            }
            E::InvalidElementCount { .. } | E::InvalidComponentCount { .. } => {
                Self::TooManyConstituents
            }
            E::UnrecognisedSyntaxIdentifier(_) | E::UnsupportedCharset { .. } => {
                Self::CharacterSetNotSupported
            }
            E::CharacterNotInRepertoire { .. } | E::InvalidText { .. } => Self::InvalidCharacters,
            E::InvalidUna | E::InvalidDelimiter { .. } | E::InvalidReleaseSequence { .. } => {
                Self::InvalidServiceCharacters
            }
            E::EmptyInterchange { .. } | E::EmptyMessage { .. } => Self::LowerLevelEmpty,
            E::SegmentWithoutDataElements { .. } => Self::Missing,
            E::BlankDataElementValue { .. } => Self::InvalidValue,
            E::PackageNotSupported { .. } => Self::EnvelopeFunctionalityNotSupported,
            E::SegmentTooLong { .. } | E::DataElementTooLong { .. } => Self::DataElementTooLong,
            E::DataElementTooShort { .. } => Self::DataElementTooShort,
            E::InvalidCharacterType { .. } => Self::InvalidCharacterType,
            E::TooManyRepetitions { .. } => Self::TooManyRepetitions,
            E::TrailingSeparator { .. } => Self::TrailingSeparator,
            E::GroupsAndMessagesMixed { .. } => Self::GroupsAndMessagesMixed,
            E::InsignificantCharacters { .. } => Self::InvalidValue,
            E::UnexpectedDataToken { .. } | E::InvalidSegmentTag(_) => {
                Self::InvalidOccurrenceOutsideMessage
            }
            _ => Self::Unspecified,
        }
    }

    /// The best `CONTRL` code for a [`ValidationIssue`], via its stable code.
    ///
    /// Works for issues raised by *any* validator, including third-party ones,
    /// because it reads [`ValidationIssue::error_code`] rather than needing the
    /// original [`EdifactError`]. An issue with no stable code — a profile rule
    /// finding, typically — is `18` (unspecified), which is what the annex
    /// provides for a fault it does not name.
    #[must_use]
    pub fn for_issue(issue: &ValidationIssue) -> Self {
        match issue.error_code() {
            Some("E004" | "E005") => Self::ControlCountMismatch,
            Some("E016") => Self::ReferencesDoNotMatch,
            Some("E032") => Self::DuplicateDetected,
            Some("E008" | "E015" | "E021" | "E046") => Self::Missing,
            Some("E014" | "E027" | "E045") => Self::InvalidValue,
            Some("E011" | "E017") => Self::NotSupportedHere,
            Some("E012" | "E013") => Self::TooManyConstituents,
            Some("E031" | "E039") => Self::CharacterSetNotSupported,
            Some("E003" | "E038") => Self::InvalidCharacters,
            Some("E002" | "E007" | "E019") => Self::InvalidServiceCharacters,
            Some("E042" | "E043") => Self::LowerLevelEmpty,
            Some("E044") => Self::EnvelopeFunctionalityNotSupported,
            Some("E020" | "E049") => Self::DataElementTooLong,
            Some("E050") => Self::DataElementTooShort,
            Some("E048") => Self::InvalidCharacterType,
            Some("E047") => Self::TooManyRepetitions,
            Some("E051") => Self::TrailingSeparator,
            Some("E052") => Self::GroupsAndMessagesMixed,
            Some("E053") => Self::InvalidValue,
            Some("E006" | "E028") => Self::InvalidOccurrenceOutsideMessage,
            _ => Self::Unspecified,
        }
    }
}

/// Identity helper that keeps [`SyntaxError::permitted_at`] usable in `const`.
///
/// `match` on a `#[non_exhaustive]` enum from inside its own crate is fine; this
/// exists only so the arms below read as `L::Interchange` rather than repeating
/// the full path, without tripping the `const fn` restriction on `use` inside a
/// match guard.
#[inline]
const fn l_of(level: ReportingLevel) -> ReportingLevel {
    level
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code(), self.description())
    }
}

// ── the message ───────────────────────────────────────────────────────────────

/// What part of the subject interchange a finding was traced to.
///
/// Messages and groups are keyed by **position** rather than by their control
/// reference: the reference is ambiguous in exactly the case a `CONTRL` most
/// needs to be precise about, namely when two of them share one and that
/// duplication is the fault being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The `UNG`/`UNE` envelope of the group at this index.
    Group(usize),
    /// A segment inside the message at this flat index across the interchange.
    Message {
        index: usize,
        /// One-based segment position within that message, `UNH` being 1.
        segment_position: u32,
    },
}

/// One finding placed at a `CONTRL` reporting level.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Finding {
    scope: Scope,
    /// One-based data element position within that segment, the tag being 1.
    element_position: Option<u32>,
    /// One-based component position within that data element.
    component_position: Option<u32>,
    /// Whether this finding rejects its level, or merely annotates it.
    ///
    /// §5.3.3 permits reporting an error on a level that is nonetheless
    /// acknowledged, which is exactly what a warning is.
    rejects: bool,
    error: SyntaxError,
}

impl Finding {
    /// The lowest reporting level that can both locate this fault and legally
    /// carry its code.
    ///
    /// §5.3.3 asks for the lowest possible level, but Annex A decides which
    /// levels may carry which code — "duplicate detected", for instance, is
    /// meaningless below the message. Descending past the deepest *permitted*
    /// level would emit a `UCS` with no code at all, which tells the partner
    /// that a segment is wrong without saying how.
    fn level(&self) -> ReportingLevel {
        if let Scope::Group(_) = self.scope {
            return ReportingLevel::Group;
        }
        if self.element_position.is_some() && self.error.permitted_at(ReportingLevel::DataElement) {
            return ReportingLevel::DataElement;
        }
        if self.error.permitted_at(ReportingLevel::Segment) {
            return ReportingLevel::Segment;
        }
        ReportingLevel::Message
    }

    /// The flat message index this finding belongs to, if it is inside a message.
    fn message_index(&self) -> Option<usize> {
        match self.scope {
            Scope::Message { index, .. } => Some(index),
            Scope::Group(_) => None,
        }
    }
}

/// A `CONTRL` message under construction.
///
/// Build one with [`receipt`][Self::receipt], [`acknowledgement`][Self::acknowledgement],
/// or [`from_report`][Self::from_report], then render it with
/// [`segments`][Self::segments] or [`to_edifact_string`][Self::to_edifact_string].
///
/// The message reference defaults to the subject interchange's control reference,
/// which keeps a `CONTRL` traceable to what it answers without the caller having
/// to invent one; override it with
/// [`with_message_reference`][Self::with_message_reference].
#[derive(Debug, Clone)]
pub struct Contrl {
    control_ref: String,
    sender: String,
    sender_qualifier: String,
    recipient: String,
    recipient_qualifier: String,
    action: Action,
    interchange_error: Option<SyntaxError>,
    message_ref: Option<String>,
    /// Reported messages: their flat index in the subject interchange, their
    /// identity, and the verdict on them.
    ///
    /// Populated only when the subject interchange uses **no** groups —
    /// ISO 9735-4 §5.3.1 makes segment groups 1 and 3 mutually exclusive.
    messages: Vec<(usize, MessageEnvelope, Action)>,
    /// Reported groups and, under each, the messages it contains.
    ///
    /// Populated only when the subject interchange **does** use groups.
    groups: Vec<GroupReport>,
    findings: Vec<Finding>,
}

/// One `UCF` and the `UCM`s beneath it.
#[derive(Debug, Clone)]
struct GroupReport {
    envelope: FunctionalGroupEnvelope,
    action: Action,
    error: Option<SyntaxError>,
    /// Flat message index, identity, and verdict for each reported message.
    messages: Vec<(usize, MessageEnvelope, Action)>,
}

impl Contrl {
    fn base(interchange: &InterchangeEnvelope, action: Action) -> Self {
        Self {
            control_ref: interchange.control_ref.clone(),
            sender: interchange.sender_id.clone(),
            sender_qualifier: interchange.sender_qualifier.clone(),
            recipient: interchange.recipient_id.clone(),
            recipient_qualifier: interchange.recipient_qualifier.clone(),
            action,
            interchange_error: None,
            message_ref: None,
            messages: Vec::new(),
            groups: Vec::new(),
            findings: Vec::new(),
        }
    }

    /// The receipt message of ISO 9735-4 §5.3.1 — action code `8`.
    ///
    /// Says only that the interchange arrived. It carries no per-message
    /// reporting, because nothing has been checked yet.
    #[must_use]
    pub fn receipt(interchange: &InterchangeEnvelope) -> Self {
        Self::base(interchange, Action::Received)
    }

    /// Acknowledge a clean interchange — action code `7` throughout.
    ///
    /// Every message is acknowledged implicitly by the `UCI`, so no `UCM` is
    /// emitted: §5.3.4 makes explicit acknowledgement of every message redundant
    /// when the interchange level already says so.
    #[must_use]
    pub fn acknowledgement(subject: &ValidatedInterchange) -> Self {
        Self::base(&subject.interchange, Action::Acknowledged)
    }

    /// Report the outcome of a validation run.
    ///
    /// `segments` is the subject interchange as parsed — it is what turns an
    /// issue's byte span into the segment position `UCS` DE 0096 needs, so
    /// findings land at the right place rather than all piling up on the `UCI`.
    ///
    /// # Where a rejection lands
    ///
    /// Action code `4` means "this level **and all lower levels** rejected", so
    /// putting it on the `UCI` because one message was bad would reject every
    /// other message in the interchange too. §5.3.2 pairs it with code `7` —
    /// "this level acknowledged, next lower level acknowledged **if not
    /// explicitly rejected**" — and that is the combination used here:
    ///
    /// - A fault in the interchange envelope itself rejects the `UCI`, and
    ///   nothing follows, because everything below is implicitly rejected.
    /// - A fault in a group's `UNG`/`UNE` rejects that group's `UCF`.
    /// - A fault inside a message leaves both above it at `7` and rejects only
    ///   that message's `UCM`.
    ///
    /// Warnings are reported but reject nothing, which matches the way
    /// [`ValidationReport`] already separates the two.
    ///
    /// # Grouped interchanges
    ///
    /// §5.3.1 makes segment groups 1 and 3 mutually exclusive: a subject that
    /// uses `UNG`/`UNE` is reported through `UCF`, and one that does not is
    /// reported through `UCM` directly under the `UCI`. Which shape you get
    /// follows the subject — there is nothing to configure.
    #[must_use]
    pub fn from_report(
        subject: &ValidatedInterchange,
        segments: &[Segment<'_>],
        report: &ValidationReport,
    ) -> Self {
        let mut contrl = Self::base(&subject.interchange, Action::Acknowledged);

        // Boundaries let a byte span be resolved to the deepest structure that
        // contains it: a message first, then the group around it.
        let messages = message_boundaries(segments);
        let groups = group_boundaries(segments);

        for (issue, rejects) in report
            .errors()
            .iter()
            .map(|i| (i, true))
            .chain(report.warnings().iter().map(|i| (i, false)))
        {
            let error = SyntaxError::for_issue(issue);
            let located = issue
                .span
                .and_then(|span| locate(segments, &messages, &groups, span.start));

            // §5.3.3: report at the lowest level that can express the fault, and
            // never repeat the code higher up.  An issue that resolves to no
            // structure at all is about the interchange envelope.
            let Some(scope) = located else {
                if rejects {
                    contrl.action = Action::Rejected;
                }
                if contrl.interchange_error.is_none()
                    && error.permitted_at(ReportingLevel::Interchange)
                {
                    contrl.interchange_error = Some(error);
                }
                continue;
            };

            contrl.findings.push(Finding {
                scope,
                // DE 0098 counts the segment tag as position 1, so a zero-based
                // element index is two positions further along.
                element_position: issue.element_index.map(|i| u32::from(i) + 2),
                component_position: issue.component_index.map(|i| u32::from(i) + 1),
                rejects,
                error,
            });
        }

        // A rejected UCI already rejects everything under it (code 4), so
        // anything further would either repeat that or contradict it.
        if contrl.action == Action::Rejected {
            contrl.findings.clear();
            return contrl;
        }

        if subject.functional_groups.is_empty() {
            contrl.messages = contrl.reported_messages(subject.messages.iter().enumerate());
        } else {
            contrl.build_group_reports(subject);
        }

        contrl
    }

    /// Select the messages that need a `UCM`, with their verdicts.
    ///
    /// A clean message needs none: the level above already acknowledged it, and
    /// §5.3.4 makes the redundant `UCM` pointless.
    fn reported_messages<'m>(
        &self,
        candidates: impl Iterator<Item = (usize, &'m MessageEnvelope)>,
    ) -> Vec<(usize, MessageEnvelope, Action)> {
        let mut out = Vec::new();
        for (index, message) in candidates {
            let mut has_finding = false;
            let mut rejected = false;
            for finding in &self.findings {
                if finding.message_index() == Some(index) {
                    has_finding = true;
                    rejected |= finding.rejects;
                }
            }
            if !has_finding {
                continue;
            }
            out.push((
                index,
                message.clone(),
                if rejected {
                    Action::Rejected
                } else {
                    Action::Acknowledged
                },
            ));
        }
        out
    }

    /// Build segment group 3: one `UCF` per group, each with its own messages.
    fn build_group_reports(&mut self, subject: &ValidatedInterchange) {
        // `ValidatedInterchange::messages` is the concatenation of the groups'
        // message lists in document order, so a running counter converts a
        // per-group position into the flat index findings are keyed by.
        let mut flat = 0usize;
        let mut reports = Vec::new();

        for (group_index, group) in subject.functional_groups.iter().enumerate() {
            let start = flat;
            flat += group.messages.len();

            let group_fault = self
                .findings
                .iter()
                .find(|f| f.scope == Scope::Group(group_index));
            let rejected = group_fault.is_some_and(|f| f.rejects);

            // A rejected UCF rejects every message under it, so it carries no
            // UCM — exactly as a rejected UCI carries no UCF.
            let messages = if rejected {
                Vec::new()
            } else {
                self.reported_messages(
                    group
                        .messages
                        .iter()
                        .enumerate()
                        .map(|(offset, message)| (start + offset, message)),
                )
            };

            if group_fault.is_none() && messages.is_empty() {
                continue; // The UCI's acknowledgement already covers it.
            }

            reports.push(GroupReport {
                envelope: group.clone(),
                action: if rejected {
                    Action::Rejected
                } else {
                    Action::Acknowledged
                },
                error: group_fault
                    .map(|f| f.error)
                    .filter(|e| e.permitted_at(ReportingLevel::Group)),
                messages,
            });
        }
        self.groups = reports;
    }

    /// Set the `UNH` message reference (DE 0062) of the `CONTRL` itself.
    ///
    /// Defaults to the subject interchange's control reference.
    #[must_use]
    pub fn with_message_reference(mut self, reference: impl Into<String>) -> Self {
        self.message_ref = Some(reference.into());
        self
    }

    /// Record an interchange-level error on the `UCI`.
    ///
    /// Ignored when `error` is not permitted at the interchange level by
    /// ISO 9735-4 Annex A — a `CONTRL` carrying a code its level may not use is
    /// one the partner's translator rejects, which helps nobody.
    #[must_use]
    pub fn with_interchange_error(mut self, error: SyntaxError) -> Self {
        if error.permitted_at(ReportingLevel::Interchange) {
            self.interchange_error = Some(error);
        }
        self
    }

    /// The verdict this message carries at the interchange level.
    #[must_use]
    pub const fn action(&self) -> Action {
        self.action
    }

    /// The `UNH` message reference this `CONTRL` will carry.
    #[must_use]
    pub fn message_reference(&self) -> &str {
        self.message_ref.as_deref().unwrap_or(&self.control_ref)
    }

    /// Render the message as segments, `UNH` through `UNT`.
    ///
    /// `UNT` DE 0074 is computed from what was actually emitted, so the count is
    /// right by construction rather than by the caller remembering to update it.
    #[must_use]
    pub fn segments(&self) -> Vec<OwnedSegment> {
        let mut out = Vec::new();
        let reference = self.message_reference().to_owned();

        // UNH — the message type is fixed by ISO 9735-4 §5.4.1.
        out.push(OwnedSegment::new(
            "UNH",
            vec![
                OwnedElement::of(&[reference.as_str()]),
                OwnedElement::of(&["CONTRL", "4", "1", "UN"]),
            ],
        ));

        // UCI — interchange level.
        let mut uci = vec![
            OwnedElement::of(&[self.control_ref.as_str()]),
            party(&self.sender, &self.sender_qualifier),
            party(&self.recipient, &self.recipient_qualifier),
            OwnedElement::of(&[self.action.code()]),
        ];
        if let Some(error) = self.interchange_error {
            uci.push(OwnedElement::of(&[error.code()]));
        }
        out.push(OwnedSegment::new("UCI", uci));

        // SG1 (ungrouped) or SG3 (grouped) — never both, per §5.3.1.
        for (index, message, action) in &self.messages {
            out.extend(self.message_report(*index, message, *action));
        }
        for group in &self.groups {
            let mut ucf = vec![
                OwnedElement::of(&[group.envelope.group_ref.as_str()]),
                party(
                    &group.envelope.app_sender,
                    &group.envelope.app_sender_qualifier,
                ),
                party(
                    &group.envelope.app_recipient,
                    &group.envelope.app_recipient_qualifier,
                ),
                OwnedElement::of(&[group.action.code()]),
            ];
            if let Some(error) = group.error {
                ucf.push(OwnedElement::of(&[error.code()]));
            }
            out.push(OwnedSegment::new("UCF", ucf));
            for (index, message, action) in &group.messages {
                out.extend(self.message_report(*index, message, *action));
            }
        }

        let count = (out.len() + 1).to_string();
        out.push(OwnedSegment::new(
            "UNT",
            vec![
                OwnedElement::of(&[count.as_str()]),
                OwnedElement::of(&[reference.as_str()]),
            ],
        ));
        out
    }

    /// Emit one `UCM` and the `UCS`/`UCD` pairs beneath it.
    fn message_report(
        &self,
        index: usize,
        message: &MessageEnvelope,
        action: Action,
    ) -> Vec<OwnedSegment> {
        let mut ucm = vec![
            OwnedElement::of(&[message.message_ref.as_str()]),
            OwnedElement::of(&[
                message.message_type.as_str(),
                message.version.as_str(),
                message.release.as_str(),
                message.controlling_agency.as_str(),
            ]),
            OwnedElement::of(&[action.code()]),
        ];
        // A fault that no lower level may carry is reported here, on DE 0085.
        if let Some(finding) = self
            .findings
            .iter()
            .find(|f| f.message_index() == Some(index) && f.level() == ReportingLevel::Message)
        {
            ucm.push(OwnedElement::of(&[finding.error.code()]));
        }
        let mut out = vec![OwnedSegment::new("UCM", ucm)];
        out.extend(self.segment_reports(index));
        out
    }

    /// Emit the `UCS`/`UCD` pairs (segment group 2 or 5) for one message.
    ///
    /// Findings that belong on the `UCM` are skipped here —
    /// [`message_report`][Self::message_report] has already placed them.
    fn segment_reports(&self, message_index: usize) -> Vec<OwnedSegment> {
        let mut out = Vec::new();
        let mut reported_positions: Vec<u32> = Vec::new();

        for finding in self
            .findings
            .iter()
            .filter(|f| f.message_index() == Some(message_index))
        {
            let level = finding.level();
            let Scope::Message {
                segment_position: position,
                ..
            } = finding.scope
            else {
                continue;
            };
            if level == ReportingLevel::Message {
                continue;
            }
            // §5.3.3: no more than one reporting level per referenced level, so a
            // second finding on the same segment extends it with a UCD rather
            // than opening a second UCS for it.
            if !reported_positions.contains(&position) {
                reported_positions.push(position);
                let position_text = position.to_string();
                let mut ucs = vec![OwnedElement::of(&[position_text.as_str()])];
                // The UCS states the code only when the segment itself is the
                // deepest level that can carry it.
                if level == ReportingLevel::Segment {
                    ucs.push(OwnedElement::of(&[finding.error.code()]));
                }
                out.push(OwnedSegment::new("UCS", ucs));
            }

            if level == ReportingLevel::DataElement {
                let element_text = finding
                    .element_position
                    .expect("DataElement level implies a known element position")
                    .to_string();
                let mut identification = vec![element_text];
                if let Some(component) = finding.component_position {
                    identification.push(component.to_string());
                }
                out.push(OwnedSegment::new(
                    "UCD",
                    vec![
                        OwnedElement::of(&[finding.error.code()]),
                        OwnedElement::of(&identification),
                    ],
                ));
            }
        }
        out
    }

    /// Render the message to EDIFACT bytes, without an interchange envelope.
    ///
    /// # Errors
    ///
    /// Propagates any writer failure.
    pub fn to_bytes(&self) -> Result<Vec<u8>, EdifactError> {
        crate::segments_to_bytes_owned(&self.segments())
    }

    /// Render the message to an EDIFACT string, without an interchange envelope.
    ///
    /// # Errors
    ///
    /// As [`to_bytes`][Self::to_bytes], plus [`EdifactError::InvalidUtf8`].
    pub fn to_edifact_string(&self) -> Result<String, EdifactError> {
        String::from_utf8(self.to_bytes()?).map_err(|_| EdifactError::InvalidUtf8)
    }

    /// Wrap the message in its own interchange and render it.
    ///
    /// ISO 9735-4 §5.3 requires a `CONTRL` to travel in an interchange of its
    /// own and never inside a group, so this is the form to actually send. The
    /// sender and recipient are swapped relative to the subject interchange,
    /// because the reply goes back the way it came.
    ///
    /// # Example
    ///
    /// ```
    /// # use edifact_rs::{Contrl, from_bytes, validate_envelope};
    /// # let raw = b"UNB+UNOC:3+SENDER+RECEIVER+260101:0900+IC4711'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNZ+1+IC4711'";
    /// # let segments: Vec<_> = from_bytes(raw).collect::<Result<Vec<_>, _>>()?;
    /// # let validated = validate_envelope(&segments)?;
    /// let wire = Contrl::acknowledgement(&validated)
    ///     .to_interchange_string("UNOC", "3", "260101", "0930", "ACK-1")?;
    ///
    /// // The reply is addressed back to the original sender.
    /// assert!(wire.starts_with("UNB+UNOC:3+RECEIVER+SENDER+260101:0930+ACK-1'"));
    /// assert!(wire.ends_with("UNZ+1+ACK-1'"));
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Propagates any writer failure.
    pub fn to_interchange_bytes(
        &self,
        syntax_identifier: &str,
        syntax_version: &str,
        date: &str,
        time: &str,
        control_reference: &str,
    ) -> Result<Vec<u8>, EdifactError> {
        let mut writer = Writer::new(Vec::new());
        writer.begin_interchange(
            syntax_identifier,
            syntax_version,
            // The reply goes back the way it came.
            &self.recipient,
            &self.sender,
            date,
            time,
            control_reference,
        )?;
        for segment in self.segments() {
            writer.write_segment(&segment.as_borrowed())?;
        }
        writer.end_interchange(1, control_reference)?;
        writer.finish()
    }

    /// String form of [`to_interchange_bytes`][Self::to_interchange_bytes].
    ///
    /// # Errors
    ///
    /// As [`to_interchange_bytes`][Self::to_interchange_bytes], plus
    /// [`EdifactError::InvalidUtf8`].
    pub fn to_interchange_string(
        &self,
        syntax_identifier: &str,
        syntax_version: &str,
        date: &str,
        time: &str,
        control_reference: &str,
    ) -> Result<String, EdifactError> {
        let bytes = self.to_interchange_bytes(
            syntax_identifier,
            syntax_version,
            date,
            time,
            control_reference,
        )?;
        String::from_utf8(bytes).map_err(|_| EdifactError::InvalidUtf8)
    }
}

/// Build an S002/S003-shaped element, omitting an absent qualifier.
fn party(id: &str, qualifier: &str) -> OwnedElement {
    if qualifier.is_empty() {
        OwnedElement::of(&[id])
    } else {
        OwnedElement::of(&[id, qualifier])
    }
}

/// `(first segment index, last segment index)` per message, in document order.
///
/// The order matches [`ValidatedInterchange::messages`], which is what lets a
/// finding be keyed by message index rather than by a reference that may repeat.
fn message_boundaries(segments: &[Segment<'_>]) -> Vec<(usize, usize)> {
    spans_between(segments, "UNH", "UNT")
}

/// `(first segment index, last segment index)` per group, in document order.
///
/// The order matches [`ValidatedInterchange::functional_groups`].
fn group_boundaries(segments: &[Segment<'_>]) -> Vec<(usize, usize)> {
    spans_between(segments, "UNG", "UNE")
}

/// Index ranges of each `open`…`close` pair, inclusive of both ends.
fn spans_between(segments: &[Segment<'_>], open: &str, close: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (index, segment) in segments.iter().enumerate() {
        if segment.tag == open {
            start = Some(index);
        } else if segment.tag == close {
            if let Some(from) = start.take() {
                out.push((from, index));
            }
        }
    }
    out
}

/// Resolve a byte offset to the deepest structure that contains it.
///
/// Going through the byte span rather than through a dedicated field on
/// [`ValidationIssue`] is what makes this work for findings from *any* validator,
/// including third-party ones that know nothing about `CONTRL`.
///
/// A message wins over the group around it, because §5.3.3 wants the lowest
/// level that can express the fault. `None` means the offset fell in neither —
/// the `UNB`, the `UNZ`, or the gap between structures — which makes it the
/// interchange's own.
///
/// `UCS` DE 0096 counts from the `UNH` as position 1, so the position is the
/// segment's offset from its message header plus one.
fn locate(
    segments: &[Segment<'_>],
    messages: &[(usize, usize)],
    groups: &[(usize, usize)],
    offset: usize,
) -> Option<Scope> {
    // Segments are in source order, so the containing one is the last whose
    // span starts at or before the offset.
    let index = match segments.binary_search_by(|segment| segment.span.start.cmp(&offset)) {
        Ok(exact) => exact,
        Err(0) => return None,
        Err(next) => next - 1,
    };
    if let Some((message_index, (start, _))) = messages
        .iter()
        .enumerate()
        .find(|(_, (start, end))| (*start..=*end).contains(&index))
    {
        return u32::try_from(index - start + 1)
            .ok()
            .map(|segment_position| Scope::Message {
                index: message_index,
                segment_position,
            });
    }
    groups
        .iter()
        .position(|(start, end)| (*start..=*end).contains(&index))
        .map(Scope::Group)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &[u8]) -> Vec<OwnedSegment> {
        crate::from_bytes_owned(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse")
    }

    #[test]
    fn annex_a_permits_a_code_only_where_the_table_says() {
        // Spot-checks straight out of the Annex A grid.
        assert!(SyntaxError::NotActualRecipient.permitted_at(ReportingLevel::Interchange));
        assert!(!SyntaxError::NotActualRecipient.permitted_at(ReportingLevel::Group));
        assert!(SyntaxError::TooManyGroupRepetitions.permitted_at(ReportingLevel::Segment));
        assert!(!SyntaxError::TooManyGroupRepetitions.permitted_at(ReportingLevel::DataElement));
        assert!(SyntaxError::LowerLevelEmpty.permitted_at(ReportingLevel::Group));
        assert!(!SyntaxError::LowerLevelEmpty.permitted_at(ReportingLevel::Message));
        assert!(SyntaxError::EnvelopeFunctionalityNotSupported.permitted_at(ReportingLevel::Group));
        assert!(
            !SyntaxError::EnvelopeFunctionalityNotSupported
                .permitted_at(ReportingLevel::Interchange)
        );
        // "Invalid value" is usable at every level.
        for level in [
            ReportingLevel::Interchange,
            ReportingLevel::Group,
            ReportingLevel::Message,
            ReportingLevel::Segment,
            ReportingLevel::DataElement,
        ] {
            assert!(SyntaxError::InvalidValue.permitted_at(level), "{level:?}");
        }
    }

    #[test]
    fn every_code_is_permitted_somewhere() {
        // A code no level may carry would be unreachable, which would mean the
        // table above is wrong rather than merely strict.
        for error in ALL_ERRORS {
            assert!(
                [
                    ReportingLevel::Interchange,
                    ReportingLevel::Group,
                    ReportingLevel::Message,
                    ReportingLevel::Segment,
                    ReportingLevel::DataElement,
                ]
                .iter()
                .any(|level| error.permitted_at(*level)),
                "{error:?} is permitted nowhere"
            );
        }
    }

    #[test]
    fn codes_are_unique() {
        let mut codes: Vec<&str> = ALL_ERRORS.iter().map(|e| e.code()).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(before, codes.len(), "duplicate DE 0085 code");
    }

    const ALL_ERRORS: &[SyntaxError] = &[
        SyntaxError::SyntaxVersionNotSupported,
        SyntaxError::NotActualRecipient,
        SyntaxError::InvalidValue,
        SyntaxError::Missing,
        SyntaxError::ValueNotSupportedHere,
        SyntaxError::NotSupportedHere,
        SyntaxError::TooManyConstituents,
        SyntaxError::NoAgreement,
        SyntaxError::Unspecified,
        SyntaxError::InvalidAsServiceCharacter,
        SyntaxError::InvalidCharacters,
        SyntaxError::InvalidServiceCharacters,
        SyntaxError::UnknownSender,
        SyntaxError::TooOld,
        SyntaxError::TestIndicatorNotSupported,
        SyntaxError::DuplicateDetected,
        SyntaxError::ReferencesDoNotMatch,
        SyntaxError::ControlCountMismatch,
        SyntaxError::GroupsAndMessagesMixed,
        SyntaxError::LowerLevelEmpty,
        SyntaxError::InvalidOccurrenceOutsideMessage,
        SyntaxError::TooManyRepetitions,
        SyntaxError::TooManyGroupRepetitions,
        SyntaxError::InvalidCharacterType,
        SyntaxError::DataElementTooLong,
        SyntaxError::DataElementTooShort,
        SyntaxError::TrailingSeparator,
        SyntaxError::CharacterSetNotSupported,
        SyntaxError::EnvelopeFunctionalityNotSupported,
    ];

    #[test]
    fn an_acknowledgement_validates_against_the_shipped_layouts() {
        let raw = b"UNB+UNOC:3+SENDER:14+RECEIVER:14+260101:0900+IC4711'\
                    UNH+MSG1+ORDERS:D:96A:UN'BGM+220+PO-1+9'UNT+3+MSG1'\
                    UNZ+1+IC4711'";
        let owned = parse(raw);
        let segments: Vec<_> = owned.iter().map(OwnedSegment::as_borrowed).collect();
        let validated = crate::validate_envelope(&segments).expect("valid subject");

        let contrl = Contrl::acknowledgement(&validated).with_message_reference("ACK1");
        let wire = contrl.to_edifact_string().expect("render");

        assert_eq!(
            wire,
            "UNH+ACK1+CONTRL:4:1:UN'UCI+IC4711+SENDER:14+RECEIVER:14+7'UNT+3+ACK1'"
        );

        // And it is itself a well-formed CONTRL by the crate's own tables.
        let reparsed = parse(wire.as_bytes());
        let borrowed: Vec<_> = reparsed.iter().map(OwnedSegment::as_borrowed).collect();
        let validator = crate::DirectoryValidator::new(
            "iso-9735-4",
            crate::service::lookup,
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = crate::ValidationContext::builder()
            .with_validator(crate::ValidationLayer::Structure, validator)
            .build()
            .validate_lenient(&borrowed);
        assert!(!report.has_errors(), "{:#?}", report.errors());
    }

    #[test]
    fn a_receipt_carries_action_8_and_nothing_else() {
        let raw =
            b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNZ+1+IC1'";
        let owned = parse(raw);
        let segments: Vec<_> = owned.iter().map(OwnedSegment::as_borrowed).collect();
        let validated = crate::validate_envelope(&segments).expect("valid subject");

        let wire = Contrl::receipt(&validated.interchange)
            .to_edifact_string()
            .expect("render");
        assert!(wire.contains("UCI+IC1+S+R+8'"), "{wire}");
        assert_eq!(
            Contrl::receipt(&validated.interchange).action(),
            Action::Received
        );
    }

    #[test]
    fn the_message_reference_defaults_to_the_subject_control_reference() {
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC-42'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNZ+1+IC-42'";
        let owned = parse(raw);
        let segments: Vec<_> = owned.iter().map(OwnedSegment::as_borrowed).collect();
        let validated = crate::validate_envelope(&segments).expect("valid subject");
        assert_eq!(
            Contrl::acknowledgement(&validated).message_reference(),
            "IC-42"
        );
    }

    #[test]
    fn a_forbidden_interchange_level_code_is_not_recorded() {
        let raw =
            b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNZ+1+IC1'";
        let owned = parse(raw);
        let segments: Vec<_> = owned.iter().map(OwnedSegment::as_borrowed).collect();
        let validated = crate::validate_envelope(&segments).expect("valid subject");

        // Annex A: "too many segment group repetitions" is UCS-only.
        let wire = Contrl::acknowledgement(&validated)
            .with_interchange_error(SyntaxError::TooManyGroupRepetitions)
            .to_edifact_string()
            .expect("render");
        assert!(wire.contains("UCI+IC1+S+R+7'"), "{wire}");
    }

    /// Parse, validate, and build a CONTRL from the result.
    ///
    /// The lenient path, because that is the only one that yields an interchange
    /// *and* its faults — which is exactly what a CONTRL reports.
    fn report_for(raw: &[u8]) -> Contrl {
        let owned = parse(raw);
        let segments: Vec<_> = owned.iter().map(OwnedSegment::as_borrowed).collect();
        let validated = crate::validate_envelope_lenient(&segments)
            .interchange
            .expect("subject must be structurally interpretable");
        let report = crate::ValidationContext::builder()
            .with_envelope_validation()
            .with_syntax_validation()
            .build()
            .validate_lenient(&segments);
        Contrl::from_report(&validated, &segments, &report)
    }

    #[test]
    fn a_data_element_fault_is_reported_at_the_ucd_level() {
        // `FTX+   ` is a value of nothing but spaces — ISO 9735-1 §9.3, which
        // the syntax validator raises as a warning with a span and a component
        // index.  That is enough to place it precisely.
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNH+M1+ORDERS:D:96A:UN'FTX+   'UNT+3+M1'\
                    UNZ+1+IC1'";
        let contrl = report_for(raw);

        // A warning does not reject: the interchange and the message stay
        // acknowledged, and the finding is still reported (§5.3.3).
        assert_eq!(contrl.action(), Action::Acknowledged);
        let wire = contrl.to_edifact_string().expect("render");

        // FTX is the second segment of the message, counting UNH as 1.
        // Element 0 is DE position 2 (the tag is position 1); component 0 is 1.
        assert!(wire.contains("UCM+M1+ORDERS:D:96A:UN+7'"), "{wire}");
        assert!(wire.contains("UCS+2'"), "{wire}");
        assert!(wire.contains("UCD+12+2:1'"), "{wire}");
    }

    #[test]
    fn a_rejected_message_is_named_by_its_ucm() {
        // Two messages share a UNH reference — a hard ISO 9735-1 violation with
        // a span on the offending UNH, so it lands on that message.
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'\
                    UNH+M1+ORDERS:D:96A:UN'BGM+221'UNT+3+M1'\
                    UNZ+2+IC1'";
        let contrl = report_for(raw);

        // The *interchange* envelope is fine — the fault is inside it. Code 4 on
        // the UCI would reject every other message too (§5.3.2), so the UCI
        // stays at 7 and only the offending message is explicitly rejected.
        assert_eq!(contrl.action(), Action::Acknowledged);
        let wire = contrl.to_edifact_string().expect("render");

        // The duplicate belongs to the *second* message, and Annex A does not
        // allow code 26 below the message level — so it is reported on that
        // UCM's DE 0085 rather than pushed into a UCS that could not carry it.
        // The first message needs no UCM at all: the UCI's 7 covers it.
        assert_eq!(
            wire,
            "UNH+IC1+CONTRL:4:1:UN'\
             UCI+IC1+S+R+7'\
             UCM+M1+ORDERS:D:96A:UN+4+26'\
             UNT+4+IC1'"
        );
    }

    #[test]
    fn an_envelope_fault_rejects_the_interchange_and_emits_no_ucm() {
        // A UNZ control reference that does not match the UNB is the envelope's
        // own fault, so code 4 on the UCI is right — and it implicitly rejects
        // every message, which is why no UCM follows.
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'\
                    UNZ+1+IC-OTHER'";
        let contrl = report_for(raw);

        assert_eq!(contrl.action(), Action::Rejected);
        let wire = contrl.to_edifact_string().expect("render");
        assert_eq!(wire, "UNH+IC1+CONTRL:4:1:UN'UCI+IC1+S+R+4+28'UNT+3+IC1'");
    }

    #[test]
    fn a_count_mismatch_is_reported_on_the_message_that_got_it_wrong() {
        // UNT DE 0074 is the trailer's fault, so the finding belongs to that
        // message — not to the interchange, which is otherwise sound.
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+9+M1'\
                    UNZ+1+IC1'";
        let contrl = report_for(raw);

        assert_eq!(contrl.action(), Action::Acknowledged);
        let wire = contrl.to_edifact_string().expect("render");
        // Code 29 (control count mismatch) is not permitted below the message,
        // so it is reported on the UCM.
        assert_eq!(
            wire,
            "UNH+IC1+CONTRL:4:1:UN'\
             UCI+IC1+S+R+7'\
             UCM+M1+ORDERS:D:96A:UN+4+29'\
             UNT+4+IC1'"
        );
    }

    #[test]
    fn a_finding_lands_at_the_lowest_level_annex_a_permits() {
        // "Duplicate detected" is legal at UCI/UCF/UCM and nowhere lower, so it
        // must stop at the message even though the fault has a segment position.
        let duplicate = Finding {
            scope: Scope::Message {
                index: 0,
                segment_position: 1,
            },
            element_position: Some(2),
            component_position: None,
            rejects: true,
            error: SyntaxError::DuplicateDetected,
        };
        assert_eq!(duplicate.level(), ReportingLevel::Message);

        // "Invalid value" is legal everywhere, so it descends all the way.
        let invalid = Finding {
            error: SyntaxError::InvalidValue,
            ..duplicate.clone()
        };
        assert_eq!(invalid.level(), ReportingLevel::DataElement);

        // …and stops at the segment when there is no data element to name.
        let segment_only = Finding {
            element_position: None,
            ..invalid.clone()
        };
        assert_eq!(segment_only.level(), ReportingLevel::Segment);

        // "Too many segment group repetitions" is UCS-only: it cannot descend to
        // UCD even with an element position, and must not climb to UCM.
        let group_repetitions = Finding {
            error: SyntaxError::TooManyGroupRepetitions,
            ..duplicate
        };
        assert_eq!(group_repetitions.level(), ReportingLevel::Segment);
    }

    #[test]
    fn a_generated_contrl_reparses_and_counts_its_own_segments() {
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNH+M1+ORDERS:D:96A:UN'FTX+   'UNT+3+M1'\
                    UNZ+1+IC1'";
        let contrl = report_for(raw);

        let wire = contrl
            .to_interchange_string("UNOC", "3", "260101", "0930", "ACK-1")
            .expect("render");

        // The generated interchange must satisfy the crate's own envelope rules,
        // including the UNT count this module computes for itself.
        let segments: Vec<_> = crate::from_bytes(wire.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("generated CONTRL must reparse");
        let validated =
            crate::validate_envelope(&segments).expect("generated CONTRL must validate");
        assert_eq!(validated.messages.len(), 1);
        assert_eq!(validated.messages[0].message_type, "CONTRL");
        assert_eq!(validated.messages[0].version, "4");
        assert_eq!(validated.messages[0].release, "1");
        assert_eq!(
            validated.messages[0].declared_segment_count,
            validated.messages[0].actual_segment_count
        );
    }

    #[test]
    fn a_grouped_interchange_is_reported_through_ucf() {
        // §5.3.1: segment groups 1 and 3 are mutually exclusive — a subject that
        // uses UNG/UNE is reported through UCF, never through a bare UCM.
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNG+ORDERS+SND:14+RCV:14+260101:0900+GRP1+UN+D:96A'\
                    UNH+M1+ORDERS:D:96A:UN'FTX+   'UNT+3+M1'\
                    UNE+1+GRP1'\
                    UNZ+1+IC1'";
        let contrl = report_for(raw);

        assert_eq!(contrl.action(), Action::Acknowledged);
        let wire = contrl.to_edifact_string().expect("render");

        // The UCF names the group by its own reference and application parties,
        // and the message report nests underneath it.
        assert_eq!(
            wire,
            "UNH+IC1+CONTRL:4:1:UN'\
             UCI+IC1+S+R+7'\
             UCF+GRP1+SND:14+RCV:14+7'\
             UCM+M1+ORDERS:D:96A:UN+7'\
             UCS+2'\
             UCD+12+2:1'\
             UNT+7+IC1'"
        );
    }

    #[test]
    fn a_group_envelope_fault_rejects_only_that_group() {
        // The UNE reference does not match its UNG.  That is the *group's*
        // envelope, not the interchange's — so the UCI stays at 7 and the UCF
        // carries the rejection, which implicitly rejects the messages inside it.
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNG+ORDERS+SND+RCV+260101:0900+GRP1+UN+D:96A'\
                    UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'\
                    UNE+1+GRP-OTHER'\
                    UNZ+1+IC1'";
        let contrl = report_for(raw);

        assert_eq!(contrl.action(), Action::Acknowledged);
        let wire = contrl.to_edifact_string().expect("render");
        assert_eq!(
            wire,
            "UNH+IC1+CONTRL:4:1:UN'\
             UCI+IC1+S+R+7'\
             UCF+GRP1+SND+RCV+4+28'\
             UNT+4+IC1'"
        );
    }

    #[test]
    fn a_clean_grouped_interchange_needs_no_ucf() {
        // The UCI's code 7 already acknowledges every group and message under
        // it (§5.3.4); a UCF that only repeats that is noise.
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNG+ORDERS+SND+RCV+260101:0900+GRP1+UN+D:96A'\
                    UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'\
                    UNE+1+GRP1'\
                    UNZ+1+IC1'";
        let contrl = report_for(raw);
        assert_eq!(
            contrl.to_edifact_string().expect("render"),
            "UNH+IC1+CONTRL:4:1:UN'UCI+IC1+S+R+7'UNT+3+IC1'"
        );
    }

    #[test]
    fn error_mapping_picks_the_narrowest_annex_a_code() {
        use EdifactError as E;
        let cases: [(EdifactError, SyntaxError); 6] = [
            (
                E::MessageCountMismatch {
                    expected: 1,
                    actual: 2,
                },
                SyntaxError::ControlCountMismatch,
            ),
            (
                E::DuplicateReference {
                    tag: "UNH".to_owned(),
                    reference: "1".to_owned(),
                    span: crate::Span::new(0, 1),
                },
                SyntaxError::DuplicateDetected,
            ),
            (
                E::UnsupportedCharset {
                    syntax_identifier: "UNOX".to_owned(),
                },
                SyntaxError::CharacterSetNotSupported,
            ),
            (E::InvalidUna, SyntaxError::InvalidServiceCharacters),
            (
                E::EmptyInterchange {
                    control_ref: "IC1".to_owned(),
                },
                SyntaxError::LowerLevelEmpty,
            ),
            (
                E::PackageNotSupported {
                    tag: "UNO".to_owned(),
                    span: crate::Span::new(0, 1),
                },
                SyntaxError::EnvelopeFunctionalityNotSupported,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(SyntaxError::for_error(&error), expected, "{error:?}");
        }
    }
}
