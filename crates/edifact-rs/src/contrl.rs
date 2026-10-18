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
//! A fault in a service segment is reported on that segment's own level and
//! named there (DE 0135 / DE 0013, then S011 when the element is known):
//! `UCM+…+4+29+UNT` for a wrong `UNT` count. A `UCM` copies the subject's
//! DE 0062 and S009 in full, DE 0057 included.
//!
//! # Configuration
//!
//! The `CONTRL`'s own `UNH` S009 follows the subject's syntax version —
//! `CONTRL:4:1:UN` for syntax 4, `CONTRL:D:3:UN` for syntax 1 to 3 — unless
//! [`ContrlOptions`] names a [`ContrlIdentifier`]. The same options restrict the
//! action codes each level may carry ([`ActionPolicy`]), choose how far a
//! rejection reaches ([`RejectionScope`]) and whether warnings are reported.
//! Community rules — a guide that admits fewer codes or rejects whole
//! interchanges — are expressed this way; the module ships no community preset.
//! Pass options to [`Contrl::receipt_with`], [`Contrl::acknowledgement_with`],
//! [`Contrl::from_report_with`] or [`Contrl::from_recovered`].
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
//! // A syntax 3 subject is answered with `CONTRL:D:3:UN`; syntax 4 with `4:1:UN`.
//! assert!(wire.starts_with("UNH+ACK1+CONTRL:D:3:UN'"));
//! assert!(wire.contains("UCI+IC4711+SENDER+RECEIVER+7'"));
//! # Ok::<(), edifact_rs::EdifactError>(())
//! ```

pub(crate) mod location;

pub use location::ContrlLocation;
pub(crate) use location::{Anchor, Detail, Target, anchor_for_code, detail_of, place};

use crate::envelope::{
    FunctionalGroupEnvelope, InterchangeEnvelope, MessageEnvelope, ValidatedInterchange,
};
use crate::model::{OwnedElement, OwnedSegment, Segment};
use crate::recover::{MessageIdentity, RecoveredInterchange};
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
            E::TooManyRepetitions { .. } | E::TooManySegmentOccurrences { .. } => {
                Self::TooManyRepetitions
            }
            E::TooManyGroupOccurrences { .. } => Self::TooManyGroupRepetitions,
            // A segment the group schema has no place for is a segment that is
            // not supported in this position, as `E011` is for a directory.
            E::SegmentNotPlaced { .. } => Self::NotSupportedHere,
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
    /// finding, for example — is `18` (unspecified), which is what the annex
    /// provides for a fault it does not name.
    #[must_use]
    pub fn for_issue(issue: &ValidationIssue) -> Self {
        match issue.error_code() {
            Some("E004" | "E005") => Self::ControlCountMismatch,
            Some("E016") => Self::ReferencesDoNotMatch,
            Some("E032") => Self::DuplicateDetected,
            Some("E008" | "E015" | "E021" | "E046") => Self::Missing,
            Some("E014" | "E027" | "E045") => Self::InvalidValue,
            Some("E011" | "E017" | "E068") => Self::NotSupportedHere,
            Some("E012" | "E013") => Self::TooManyConstituents,
            Some("E031" | "E039") => Self::CharacterSetNotSupported,
            Some("E003" | "E038") => Self::InvalidCharacters,
            Some("E002" | "E007" | "E019") => Self::InvalidServiceCharacters,
            Some("E042" | "E043") => Self::LowerLevelEmpty,
            Some("E044") => Self::EnvelopeFunctionalityNotSupported,
            Some("E020" | "E049") => Self::DataElementTooLong,
            Some("E050") => Self::DataElementTooShort,
            Some("E048") => Self::InvalidCharacterType,
            Some("E047" | "E069") => Self::TooManyRepetitions,
            Some("E070") => Self::TooManyGroupRepetitions,
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

// ── configuration ─────────────────────────────────────────────────────────────

/// The `UNH` S009 of the `CONTRL` message itself.
///
/// DE 0065 is always `CONTRL` (ISO 9735-4 §5.4.1); this type holds the rest:
/// version (DE 0052), release (DE 0054), controlling agency (DE 0051) and an
/// optional association assigned code (DE 0057). It renders as the second `UNH`
/// element, `CONTRL:<0052>:<0054>:<0051>[:<0057>]`.
///
/// Every `CONTRL` constructor derives the identifier from the subject
/// interchange's syntax version (`UNB` S001 DE 0002) unless
/// [`ContrlOptions::identifier`] names one — see
/// [`for_syntax_version`](Self::for_syntax_version).
///
/// # Example
///
/// ```
/// use edifact_rs::contrl::ContrlIdentifier;
///
/// assert_eq!(ContrlIdentifier::for_syntax_version("4"), ContrlIdentifier::syntax_4());
/// assert_eq!(ContrlIdentifier::for_syntax_version("3"), ContrlIdentifier::syntax_3());
///
/// // A community's association assigned code is caller configuration.
/// let id = ContrlIdentifier::syntax_3().with_association("EX1")?;
/// assert_eq!(id.association(), Some("EX1"));
///
/// // DE 0057 is an..6.
/// assert!(ContrlIdentifier::syntax_3().with_association("SEVENCH").is_err());
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ContrlIdentifier {
    version: String,
    release: String,
    agency: String,
    association: Option<String>,
}

impl ContrlIdentifier {
    /// `CONTRL:4:1:UN` — the identifier ISO 9735-4 gives for syntax version 4.
    #[must_use]
    pub fn syntax_4() -> Self {
        Self {
            version: "4".to_owned(),
            release: "1".to_owned(),
            agency: "UN".to_owned(),
            association: None,
        }
    }

    /// `CONTRL:D:3:UN` — the identifier used with syntax versions 1 to 3.
    #[must_use]
    pub fn syntax_3() -> Self {
        Self {
            version: "D".to_owned(),
            release: "3".to_owned(),
            agency: "UN".to_owned(),
            association: None,
        }
    }

    /// The identifier for a subject interchange of syntax version `version`
    /// (`UNB` S001 DE 0002).
    ///
    /// `"4"` gives [`syntax_4`](Self::syntax_4); `"1"`, `"2"` and `"3"` give
    /// [`syntax_3`](Self::syntax_3). Anything else — empty, unknown, a future
    /// version — gives [`syntax_4`](Self::syntax_4): the `CONTRL` is the vehicle
    /// for reporting code `2` "syntax version or level not supported", so it is
    /// built rather than refused.
    #[must_use]
    pub fn for_syntax_version(version: &str) -> Self {
        match version {
            "1" | "2" | "3" => Self::syntax_3(),
            _ => Self::syntax_4(),
        }
    }

    /// An identifier with explicit version, release and agency.
    ///
    /// # Errors
    ///
    /// [`EdifactError::InvalidContrlIdentifier`] naming the data element when a
    /// value is empty or longer than three characters (DE 0052, DE 0054 and
    /// DE 0051 are mandatory `an..3` in `service::S009`).
    pub fn new(
        version: impl Into<String>,
        release: impl Into<String>,
        agency: impl Into<String>,
    ) -> Result<Self, EdifactError> {
        Ok(Self {
            version: identifier_component("0052", version.into(), 3)?,
            release: identifier_component("0054", release.into(), 3)?,
            agency: identifier_component("0051", agency.into(), 3)?,
            association: None,
        })
    }

    /// Add an association assigned code (DE 0057).
    ///
    /// # Errors
    ///
    /// [`EdifactError::InvalidContrlIdentifier`] naming `0057` when `code` is
    /// empty or longer than six characters (`an..6`).
    pub fn with_association(mut self, code: impl Into<String>) -> Result<Self, EdifactError> {
        self.association = Some(identifier_component("0057", code.into(), 6)?);
        Ok(self)
    }

    /// DE 0052 — message version number.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// DE 0054 — message release number.
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }

    /// DE 0051 — controlling agency.
    #[must_use]
    pub fn agency(&self) -> &str {
        &self.agency
    }

    /// DE 0057 — association assigned code, when set.
    #[must_use]
    pub fn association(&self) -> Option<&str> {
        self.association.as_deref()
    }

    /// The S009 components as written: `CONTRL`, 0052, 0054, 0051[, 0057].
    fn components(&self) -> Vec<String> {
        let mut out = vec![
            "CONTRL".to_owned(),
            self.version.clone(),
            self.release.clone(),
            self.agency.clone(),
        ];
        if let Some(association) = &self.association {
            out.push(association.clone());
        }
        out
    }
}

/// Validate one S009 component: non-empty, at most `max` characters.
fn identifier_component(
    data_element: &'static str,
    value: String,
    max: usize,
) -> Result<String, EdifactError> {
    let length = value.chars().count();
    if length == 0 || length > max {
        return Err(EdifactError::InvalidContrlIdentifier {
            data_element,
            value,
        });
    }
    Ok(value)
}

/// Which DE 0083 action codes each reporting level may carry.
///
/// ISO 9735-4 admits `4`, `7` and `8` (the last only in a receipt); a
/// community guide may admit fewer. The default admits every code. A `CONTRL`
/// constructed with [`ContrlOptions`] whose policy forbids an action it would
/// need fails with [`EdifactError::ActionNotPermitted`] — the code is never
/// emitted and never rewritten into a permitted one.
///
/// # Example
///
/// ```
/// use edifact_rs::contrl::{Action, ActionPolicy, ReportingLevel};
///
/// let policy = ActionPolicy::new()
///     .permit(ReportingLevel::Interchange, &[Action::Acknowledged, Action::Rejected])
///     .permit(ReportingLevel::Message, &[Action::Rejected]);
///
/// assert!(!policy.permits(ReportingLevel::Interchange, Action::Received));
/// assert!(!policy.permits(ReportingLevel::Message, Action::Acknowledged));
/// assert!(policy.permits(ReportingLevel::Group, Action::Acknowledged)); // unrestricted
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ActionPolicy {
    interchange: u8,
    group: u8,
    message: u8,
}

/// Every action code, as a bit set.
const ALL_ACTIONS: u8 = 0b111;

/// The bit of `action` in an [`ActionPolicy`] set.
const fn action_bit(action: Action) -> u8 {
    match action {
        Action::Rejected => 0b001,
        Action::Acknowledged => 0b010,
        Action::Received => 0b100,
    }
}

impl Default for ActionPolicy {
    fn default() -> Self {
        Self {
            interchange: ALL_ACTIONS,
            group: ALL_ACTIONS,
            message: ALL_ACTIONS,
        }
    }
}

impl ActionPolicy {
    /// A policy that permits every action at every level.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Permit exactly `actions` at `level` (`UCI`, `UCF` or `UCM`), replacing
    /// what was permitted there before.
    ///
    /// `UCS` and `UCD` carry no DE 0083, so for
    /// [`ReportingLevel::Segment`] and [`ReportingLevel::DataElement`] this is a
    /// no-op.
    #[must_use]
    pub fn permit(mut self, level: ReportingLevel, actions: &[Action]) -> Self {
        let set = actions.iter().fold(0, |set, a| set | action_bit(*a));
        match level {
            ReportingLevel::Interchange => self.interchange = set,
            ReportingLevel::Group => self.group = set,
            ReportingLevel::Message => self.message = set,
            ReportingLevel::Segment | ReportingLevel::DataElement => {}
        }
        self
    }

    /// Whether `action` may appear at `level`.
    ///
    /// Always `true` for `UCS` and `UCD`, which carry no action code.
    #[must_use]
    pub fn permits(&self, level: ReportingLevel, action: Action) -> bool {
        let set = match level {
            ReportingLevel::Interchange => self.interchange,
            ReportingLevel::Group => self.group,
            ReportingLevel::Message => self.message,
            ReportingLevel::Segment | ReportingLevel::DataElement => return true,
        };
        set & action_bit(action) != 0
    }
}

/// How far a rejection reaches up the reporting levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum RejectionScope {
    /// ISO 9735-4 §5.3.2: a rejected `UCM` or `UCF` leaves the levels above it
    /// at `7` — "acknowledged, next lower level acknowledged if not explicitly
    /// rejected". Only the faulty message or group is rejected.
    #[default]
    PerLevel,
    /// The whole interchange is rejected when any part of it is: every level
    /// above a rejected one carries `4` without DE 0085, and only rejected
    /// messages and groups get a `UCM`/`UCF`. For communities whose guide
    /// either acknowledges an interchange or rejects all of it.
    Interchange,
}

/// How a `CONTRL` is built: its identifier, the action codes it may carry,
/// how far rejections reach, and whether warnings are reported.
///
/// The default reproduces ISO 9735-4 with nothing restricted: identifier
/// derived from the subject's syntax version, every action permitted,
/// [`RejectionScope::PerLevel`], warnings reported. Pass options to
/// [`Contrl::receipt_with`], [`Contrl::acknowledgement_with`],
/// [`Contrl::from_report_with`] or [`Contrl::from_recovered`].
///
/// # Example
///
/// A community that answers syntax 3 with an association code, admits only `7`
/// and `4` on `UCI` and only `4` on `UCM`, and rejects the whole interchange
/// when anything is wrong:
///
/// ```
/// use edifact_rs::contrl::{
///     Action, ActionPolicy, ContrlIdentifier, ContrlOptions, RejectionScope, ReportingLevel,
/// };
///
/// let options = ContrlOptions::new()
///     .identifier(ContrlIdentifier::syntax_3().with_association("2.0b")?)
///     .actions(
///         ActionPolicy::new()
///             .permit(ReportingLevel::Interchange, &[Action::Acknowledged, Action::Rejected])
///             .permit(ReportingLevel::Message, &[Action::Rejected]),
///     )
///     .rejection(RejectionScope::Interchange)
///     .report_warnings(false);
/// # let _ = options;
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ContrlOptions {
    identifier: Option<ContrlIdentifier>,
    actions: ActionPolicy,
    rejection: RejectionScope,
    report_warnings: bool,
}

impl Default for ContrlOptions {
    fn default() -> Self {
        Self {
            identifier: None,
            actions: ActionPolicy::default(),
            rejection: RejectionScope::default(),
            report_warnings: true,
        }
    }
}

impl ContrlOptions {
    /// The default options (see the type documentation).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Use `id` as the `CONTRL`'s own `UNH` S009 instead of deriving it from
    /// the subject's syntax version.
    #[must_use]
    pub fn identifier(mut self, id: ContrlIdentifier) -> Self {
        self.identifier = Some(id);
        self
    }

    /// Restrict the action codes each level may carry.
    #[must_use]
    pub fn actions(mut self, policy: ActionPolicy) -> Self {
        self.actions = policy;
        self
    }

    /// Choose how far a rejection reaches.
    #[must_use]
    pub fn rejection(mut self, scope: RejectionScope) -> Self {
        self.rejection = scope;
        self
    }

    /// Report warnings (`true`, the default) or leave them out entirely.
    ///
    /// A warning annotates a level it does not reject (§5.3.3), which yields a
    /// `UCM` with action `7`; a guide that admits only `4` on `UCM` cannot
    /// express that, so it turns warnings off.
    #[must_use]
    pub fn report_warnings(mut self, report: bool) -> Self {
        self.report_warnings = report;
        self
    }

    /// The identifier for a subject of syntax version `syntax_version`.
    fn resolve_identifier(&self, syntax_version: &str) -> ContrlIdentifier {
        self.identifier
            .clone()
            .unwrap_or_else(|| ContrlIdentifier::for_syntax_version(syntax_version))
    }
}

// ── the message ───────────────────────────────────────────────────────────────

/// One finding placed at a `CONTRL` reporting level.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Finding {
    /// Where the finding sits — the same mapping
    /// [`RecoveredInterchange::locate`][crate::recover::RecoveredInterchange::locate]
    /// uses, so the two paths cannot place a fault differently.
    location: ContrlLocation,
    /// Whether this finding rejects its level, or merely annotates it.
    ///
    /// §5.3.3 permits reporting an error on a level that is nonetheless
    /// acknowledged, which is exactly what a warning is.
    rejects: bool,
    /// Whether the `UCI` states this finding's code. `false` when `18` only
    /// stands in for a code the `UCI` may not carry: the `UCI` then states the
    /// rejection without a code.
    states_code: bool,
}

impl Finding {
    /// The reporting level that carries this finding.
    fn level(&self) -> ReportingLevel {
        self.location.level()
    }

    /// The flat message index this finding belongs to, if it is inside a message.
    fn message_index(&self) -> Option<usize> {
        match self.level() {
            ReportingLevel::Message | ReportingLevel::Segment | ReportingLevel::DataElement => {
                self.location.message()
            }
            _ => None,
        }
    }
}

/// DE 0085 on a `UCI`, `UCF` or `UCM` and, for a fault in a service segment,
/// DE 0135 (syntax 4) / DE 0013 (syntax 3) and S011 after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LevelError {
    code: SyntaxError,
    service_segment: Option<&'static str>,
    element: Option<u32>,
    component: Option<u32>,
}

impl LevelError {
    /// A code with nothing more precise.
    const fn code(code: SyntaxError) -> Self {
        Self {
            code,
            service_segment: None,
            element: None,
            component: None,
        }
    }

    /// The code, service segment and S011 a location gives its level.
    fn of(location: &ContrlLocation) -> Self {
        let service_segment = location.service_segment();
        Self {
            code: location.error(),
            service_segment,
            // S011 belongs to the service segment; without one there is none.
            element: service_segment.and(location.element_position()),
            component: service_segment.and(location.component_position()),
        }
    }

    /// Append DE 0085, then DE 0135/0013 and S011 when known.
    ///
    /// The positions follow `service::UCI`/`UCF` (5, 6, 7) and `service::UCM`
    /// (4, 5, 6): the service segment directly after DE 0085, S011 after it.
    fn push_onto(&self, elements: &mut Vec<OwnedElement>) {
        elements.push(OwnedElement::of(&[self.code.code()]));
        if let Some(tag) = self.service_segment {
            elements.push(OwnedElement::of(&[tag]));
            if let Some(element) = self.element {
                let mut s011 = vec![element.to_string()];
                if let Some(component) = self.component {
                    s011.push(component.to_string());
                }
                elements.push(OwnedElement::of(&s011));
            }
        }
    }
}

/// What a `UCM` names a message by: DE 0062 and S009, copied from the subject.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageName {
    reference: String,
    message_type: String,
    version: String,
    release: String,
    agency: String,
    /// DE 0057; empty when the subject has none.
    association: String,
}

impl From<&MessageEnvelope> for MessageName {
    fn from(message: &MessageEnvelope) -> Self {
        Self {
            reference: message.message_ref.clone(),
            message_type: message.message_type.clone(),
            version: message.version.clone(),
            release: message.release.clone(),
            agency: message.controlling_agency.clone(),
            association: message.association_code.clone(),
        }
    }
}

impl From<&MessageIdentity> for MessageName {
    fn from(identity: &MessageIdentity) -> Self {
        Self {
            reference: identity.message_ref().to_owned(),
            message_type: identity.message_type().to_owned(),
            version: identity.version().to_owned(),
            release: identity.release().to_owned(),
            agency: identity.controlling_agency().to_owned(),
            association: identity.association_code().to_owned(),
        }
    }
}

/// What a `UCF` names a group by: DE 0048, S006 and S007, copied from the subject.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupName {
    reference: String,
    sender: String,
    sender_qualifier: String,
    recipient: String,
    recipient_qualifier: String,
}

impl From<&FunctionalGroupEnvelope> for GroupName {
    fn from(group: &FunctionalGroupEnvelope) -> Self {
        Self {
            reference: group.group_ref.clone(),
            sender: group.app_sender.clone(),
            sender_qualifier: group.app_sender_qualifier.clone(),
            recipient: group.app_recipient.clone(),
            recipient_qualifier: group.app_recipient_qualifier.clone(),
        }
    }
}

/// The structure of a subject interchange, as far as a `CONTRL` needs it.
struct Shape {
    /// Each message by flat index: its name (`None` when unreadable) and the
    /// group it sits in.
    messages: Vec<(Option<MessageName>, Option<usize>)>,
    /// Each group by index: its name (`None` when unreadable). Empty when the
    /// subject uses no groups.
    groups: Vec<Option<GroupName>>,
}

impl Shape {
    fn of_validated(subject: &ValidatedInterchange) -> Self {
        if subject.functional_groups.is_empty() {
            return Self {
                messages: subject
                    .messages
                    .iter()
                    .map(|m| (Some(MessageName::from(m)), None))
                    .collect(),
                groups: Vec::new(),
            };
        }
        // `ValidatedInterchange::messages` is the concatenation of the groups'
        // message lists in document order, which is the flat index findings
        // are keyed by.
        let mut messages = Vec::new();
        let mut groups = Vec::new();
        for (index, group) in subject.functional_groups.iter().enumerate() {
            messages.extend(
                group
                    .messages
                    .iter()
                    .map(|m| (Some(MessageName::from(m)), Some(index))),
            );
            groups.push(Some(GroupName::from(group)));
        }
        Self { messages, groups }
    }

    fn of_recovered(recovered: &RecoveredInterchange<'_>) -> Self {
        Self {
            messages: recovered
                .messages()
                .iter()
                .map(|w| (w.identity().map(MessageName::from), w.group()))
                .collect(),
            groups: (0..recovered.groups().len())
                .map(|g| {
                    let reference = recovered.groups()[g].group_ref()?.to_owned();
                    let [sender, sender_qualifier, recipient, recipient_qualifier] =
                        recovered.group_parties(g);
                    Some(GroupName {
                        reference,
                        sender,
                        sender_qualifier,
                        recipient,
                        recipient_qualifier,
                    })
                })
                .collect(),
        }
    }

    /// Whether a `UCM` (and, in a grouped subject, the `UCF` above it) can
    /// name message `index`.
    fn names_message(&self, index: usize) -> bool {
        let Some((name, group)) = self.messages.get(index) else {
            return false;
        };
        name.is_some() && (self.groups.is_empty() || group.is_some_and(|g| self.names_group(g)))
    }

    /// Whether a `UCF` can name group `index`.
    fn names_group(&self, index: usize) -> bool {
        self.groups.get(index).is_some_and(Option::is_some)
    }
}

/// A `CONTRL` message under construction.
///
/// Build one with [`receipt`][Self::receipt], [`acknowledgement`][Self::acknowledgement],
/// or [`from_report`][Self::from_report] — or their `_with` forms, which take
/// [`ContrlOptions`] — or, for an interchange that does not parse, with
/// [`from_recovered`][Self::from_recovered]. Render it with
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
    identifier: ContrlIdentifier,
    action: Action,
    interchange_error: Option<LevelError>,
    message_ref: Option<String>,
    /// Reported messages, in document order.
    ///
    /// Populated only when the subject interchange uses **no** groups —
    /// ISO 9735-4 §5.3.1 makes segment groups 1 and 3 mutually exclusive.
    messages: Vec<MessageReport>,
    /// Reported groups and, under each, the messages it contains.
    ///
    /// Populated only when the subject interchange **does** use groups.
    groups: Vec<GroupReport>,
    /// Findings below the interchange level, for the `UCS`/`UCD` reports.
    findings: Vec<Finding>,
}

/// One `UCM`.
#[derive(Debug, Clone)]
struct MessageReport {
    /// Flat index of the message in the subject interchange.
    index: usize,
    name: MessageName,
    action: Action,
    error: Option<LevelError>,
}

/// One `UCF` and the `UCM`s beneath it.
#[derive(Debug, Clone)]
struct GroupReport {
    name: GroupName,
    action: Action,
    error: Option<LevelError>,
    messages: Vec<MessageReport>,
}

impl Contrl {
    fn base(interchange: &InterchangeEnvelope, action: Action, options: &ContrlOptions) -> Self {
        Self {
            control_ref: interchange.control_ref.clone(),
            sender: interchange.sender_id.clone(),
            sender_qualifier: interchange.sender_qualifier.clone(),
            recipient: interchange.recipient_id.clone(),
            recipient_qualifier: interchange.recipient_qualifier.clone(),
            identifier: options.resolve_identifier(&interchange.syntax_version),
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
        Self::base(interchange, Action::Received, &ContrlOptions::default())
    }

    /// [`receipt`](Self::receipt) under `options`.
    ///
    /// # Errors
    ///
    /// [`EdifactError::ActionNotPermitted`] when the policy does not admit `8`
    /// on `UCI`.
    pub fn receipt_with(
        interchange: &InterchangeEnvelope,
        options: &ContrlOptions,
    ) -> Result<Self, EdifactError> {
        Self::base(interchange, Action::Received, options).checked(options)
    }

    /// Acknowledge a clean interchange — action code `7` throughout.
    ///
    /// Every message is acknowledged implicitly by the `UCI`, so no `UCM` is
    /// emitted: §5.3.4 makes explicit acknowledgement of every message redundant
    /// when the interchange level already says so.
    #[must_use]
    pub fn acknowledgement(subject: &ValidatedInterchange) -> Self {
        Self::base(
            &subject.interchange,
            Action::Acknowledged,
            &ContrlOptions::default(),
        )
    }

    /// [`acknowledgement`](Self::acknowledgement) under `options`.
    ///
    /// # Errors
    ///
    /// [`EdifactError::ActionNotPermitted`] when the policy does not admit `7`
    /// on `UCI`.
    pub fn acknowledgement_with(
        subject: &ValidatedInterchange,
        options: &ContrlOptions,
    ) -> Result<Self, EdifactError> {
        Self::base(&subject.interchange, Action::Acknowledged, options).checked(options)
    }

    /// Report the outcome of a validation run.
    ///
    /// `segments` is the subject interchange as parsed — it is what turns an
    /// issue's byte span into the segment position `UCS` DE 0096 needs, so
    /// findings land at the right place rather than all piling up on the `UCI`.
    ///
    /// # Where a rejection lands
    ///
    /// Action code `4` rejects a level and everything below it, so it goes on
    /// the lowest level at fault; the levels above carry `7` (§5.3.2):
    ///
    /// - A fault in the interchange envelope rejects the `UCI`; nothing follows.
    /// - A fault in a group's `UNG`/`UNE` rejects that group's `UCF`.
    /// - A fault inside a message rejects only that message's `UCM`.
    ///
    /// [`RejectionScope::Interchange`] (via [`from_report_with`](Self::from_report_with))
    /// rejects the whole interchange instead.
    ///
    /// Warnings are reported but reject nothing, which matches the way
    /// [`ValidationReport`] already separates the two.
    ///
    /// # Placement
    ///
    /// Each finding is placed by the same mapping
    /// [`RecoveredInterchange::locate`][crate::RecoveredInterchange::locate]
    /// uses, so a fault reported from a validation run and the same fault read
    /// from a broken interchange land on the same level — see
    /// [`ContrlLocation`] for the counting rules and the Annex A overrides. A
    /// code is never written at a level Annex A forbids for it. A fault in a
    /// service segment names it on its level — `UCM+…+4+29+UNT` — followed by
    /// S011 when the element is known.
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
        Self::reporting(subject, segments, report, &ContrlOptions::default())
    }

    /// [`from_report`](Self::from_report) under `options`.
    ///
    /// # Errors
    ///
    /// [`EdifactError::ActionNotPermitted`] for the first action code (`UCI`,
    /// then each `UCF` and `UCM` in document order) the policy does not admit.
    pub fn from_report_with(
        subject: &ValidatedInterchange,
        segments: &[Segment<'_>],
        report: &ValidationReport,
        options: &ContrlOptions,
    ) -> Result<Self, EdifactError> {
        Self::reporting(subject, segments, report, options).checked(options)
    }

    /// Build a `CONTRL` for an interchange read by the recovering scan.
    ///
    /// The answer to an interchange that does not parse: addressed from the
    /// recovered `UNB`, with every fault of the scan and every location in
    /// `extra` (findings from another layer, for example
    /// [`RecoveredInterchange::locate`] of a typed-layer error) reported as a
    /// rejection of its level, under the same rules and options as
    /// [`from_report_with`](Self::from_report_with).
    ///
    /// - A fault in a message whose `UNH` identity cannot be read (or in a
    ///   group whose reference cannot be read) cannot be named by a `UCM`
    ///   (`UCF`): it rejects the `UCI`, without DE 0085.
    /// - When the scan ended on a terminal fault (an invalid `UNA`, a limit, an
    ///   unterminated segment), the rest of the interchange was never checked,
    ///   so the `UCI` carries `4` whatever the scope — never `7`.
    ///
    /// # Errors
    ///
    /// - [`EdifactError::MissingSegment`] with tag `UNB` when the scan has no
    ///   readable addressing ([`RecoveredInterchange::address`] is `None`): a
    ///   `CONTRL` with invented parties is worse than none.
    /// - [`EdifactError::ActionNotPermitted`] as for
    ///   [`from_report_with`](Self::from_report_with).
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::contrl::{Contrl, ContrlOptions};
    /// use edifact_rs::{RecoveredInterchange, RecoveryConfig};
    ///
    /// // The second segment of the message has a broken tag (`BGME03`).
    /// let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
    ///             UNH+1+ORDERS:D:96A:UN'BGME03+X'UNT+3+1'UNZ+1+IC1'";
    /// let recovered = RecoveredInterchange::scan(raw, &RecoveryConfig::default());
    ///
    /// let wire = Contrl::from_recovered(&recovered, &[], &ContrlOptions::default())?
    ///     .with_message_reference("ACK1")
    ///     .to_edifact_string()?;
    /// assert_eq!(
    ///     wire,
    ///     "UNH+ACK1+CONTRL:D:3:UN'UCI+IC1+S+R+7'\
    ///      UCM+1+ORDERS:D:96A:UN+4'UCS+2'UCD+12+1'UNT+6+ACK1'"
    /// );
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    pub fn from_recovered(
        recovered: &RecoveredInterchange<'_>,
        extra: &[ContrlLocation],
        options: &ContrlOptions,
    ) -> Result<Self, EdifactError> {
        let address = recovered
            .address()
            .ok_or_else(|| EdifactError::MissingSegment {
                tag: "UNB".to_owned(),
                expected_position: "interchange header with readable sender, recipient and \
                                     control reference"
                    .to_owned(),
            })?;
        let mut contrl = Self {
            control_ref: address.control_ref().to_owned(),
            sender: address.sender_id().to_owned(),
            sender_qualifier: address.sender_qualifier().to_owned(),
            recipient: address.recipient_id().to_owned(),
            recipient_qualifier: address.recipient_qualifier().to_owned(),
            identifier: options.resolve_identifier(address.syntax_version()),
            action: Action::Acknowledged,
            interchange_error: None,
            message_ref: None,
            messages: Vec::new(),
            groups: Vec::new(),
            findings: Vec::new(),
        };
        let findings = recovered
            .fault_locations()
            .into_iter()
            .chain(extra.iter().cloned())
            .map(|location| Finding {
                location,
                rejects: true,
                states_code: true,
            })
            .collect();
        contrl.assemble(
            findings,
            &Shape::of_recovered(recovered),
            !recovered.is_complete(),
            options,
        );
        contrl.checked(options)
    }

    /// The shared body of [`from_report`](Self::from_report) and
    /// [`from_report_with`](Self::from_report_with), without the policy check.
    fn reporting(
        subject: &ValidatedInterchange,
        segments: &[Segment<'_>],
        report: &ValidationReport,
        options: &ContrlOptions,
    ) -> Self {
        let mut contrl = Self::base(&subject.interchange, Action::Acknowledged, options);

        // Boundaries let a byte span be resolved to the deepest structure that
        // contains it: a message first, then the group around it.
        let messages = message_boundaries(segments);
        let groups = group_boundaries(segments);

        let findings = report
            .errors()
            .iter()
            .map(|i| (i, true))
            .chain(report.warnings().iter().map(|i| (i, false)))
            .map(|(issue, rejects)| {
                let error = SyntaxError::for_issue(issue);
                let location = locate_issue(segments, &messages, &groups, issue, error);
                // `18` stands in on the location for a code the UCI may not
                // carry; the UCI then states the rejection without a code.
                let stand_in = location.level() == ReportingLevel::Interchange
                    && location.error() == SyntaxError::Unspecified
                    && error != SyntaxError::Unspecified;
                Finding {
                    location,
                    rejects,
                    states_code: !stand_in,
                }
            })
            .collect();

        contrl.assemble(findings, &Shape::of_validated(subject), false, options);
        contrl
    }

    /// Assign every level its verdict and select what is reported.
    ///
    /// `forced` rejects the `UCI` whatever else is found (a scan that ended
    /// early).
    fn assemble(
        &mut self,
        findings: Vec<Finding>,
        shape: &Shape,
        forced: bool,
        options: &ContrlOptions,
    ) {
        // A rejection that no lower level can name rejects the UCI, without a
        // code: the fault is real, but there is nothing to point at.
        let mut unattributed = forced;
        let mut interchange_rejected = false;
        let mut lower = Vec::new();

        for finding in findings {
            if !finding.rejects && !options.report_warnings {
                continue;
            }
            // §5.3.3: report at the lowest level that can express the fault, and
            // never repeat the code higher up.
            match finding.level() {
                ReportingLevel::Interchange => {
                    interchange_rejected |= finding.rejects;
                    if self.interchange_error.is_none() && finding.states_code {
                        self.interchange_error = Some(LevelError::of(&finding.location));
                    }
                }
                ReportingLevel::Group => {
                    if finding
                        .location
                        .group()
                        .is_some_and(|g| shape.names_group(g))
                    {
                        lower.push(finding);
                    } else {
                        unattributed |= finding.rejects;
                    }
                }
                _ => {
                    if finding
                        .message_index()
                        .is_some_and(|m| shape.names_message(m))
                    {
                        lower.push(finding);
                    } else {
                        unattributed |= finding.rejects;
                    }
                }
            }
        }

        // A UCI rejected for its own fault already rejects everything under it
        // (code 4), so anything further would either repeat that or contradict
        // it.
        if interchange_rejected {
            self.action = Action::Rejected;
            return;
        }
        self.findings = lower;

        let whole = options.rejection == RejectionScope::Interchange || unattributed;
        let message_reports: Vec<Option<MessageReport>> = (0..shape.messages.len())
            .map(|index| self.message_verdict(index, shape))
            .collect();
        let group_faults: Vec<Option<(bool, LevelError)>> = (0..shape.groups.len())
            .map(|index| self.group_verdict(index))
            .collect();

        let any_rejected = unattributed
            || message_reports
                .iter()
                .flatten()
                .any(|m| m.action == Action::Rejected)
            || group_faults.iter().flatten().any(|(rejected, _)| *rejected);
        // Whole-interchange scope: every level above a rejected one carries 4
        // without DE 0085, and only what is rejected is reported beneath it — a
        // `7` under a `4` would contradict it.
        let uci_rejected = whole && any_rejected;
        if uci_rejected {
            self.action = Action::Rejected;
            self.interchange_error = None;
        }
        let keep = |report: &MessageReport| !uci_rejected || report.action == Action::Rejected;

        if shape.groups.is_empty() {
            self.messages = message_reports.into_iter().flatten().filter(keep).collect();
            return;
        }

        let mut message_reports = message_reports;
        let mut reports = Vec::new();
        for (index, name) in shape.groups.iter().enumerate() {
            let Some(name) = name else { continue };
            let fault = group_faults[index];
            let own_rejected = fault.is_some_and(|(rejected, _)| rejected);

            // A UCF rejected for its own fault rejects every message under it,
            // so it carries no UCM — exactly as a rejected UCI carries no UCF.
            let messages: Vec<MessageReport> = if own_rejected {
                Vec::new()
            } else {
                shape
                    .messages
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, group))| *group == Some(index))
                    .filter_map(|(m, _)| message_reports[m].take())
                    .filter(keep)
                    .collect()
            };
            let child_rejected = messages.iter().any(|m| m.action == Action::Rejected);
            let rejected = own_rejected || (whole && child_rejected);

            if fault.is_none() && messages.is_empty() {
                continue; // The UCI's acknowledgement already covers it.
            }
            if uci_rejected && !rejected {
                continue;
            }
            reports.push(GroupReport {
                name: name.clone(),
                action: if rejected {
                    Action::Rejected
                } else {
                    Action::Acknowledged
                },
                // Rejected only because a message under it was: no code of its own.
                error: fault
                    .filter(|(own, _)| *own || !rejected)
                    .map(|(_, error)| error),
                messages,
            });
        }
        self.groups = reports;
    }

    /// The `UCM` for message `index`, or `None` when nothing is reported on it.
    ///
    /// A clean message needs none: the level above already acknowledged it, and
    /// §5.3.4 makes the redundant `UCM` pointless.
    fn message_verdict(&self, index: usize, shape: &Shape) -> Option<MessageReport> {
        let mut found = false;
        let mut rejected = false;
        let mut error = None;
        for finding in self
            .findings
            .iter()
            .filter(|f| f.message_index() == Some(index))
        {
            found = true;
            rejected |= finding.rejects;
            // A fault that no lower level may carry is reported on the UCM's
            // DE 0085.
            if error.is_none() && finding.level() == ReportingLevel::Message {
                error = Some(LevelError::of(&finding.location));
            }
        }
        if !found {
            return None;
        }
        Some(MessageReport {
            index,
            name: shape.messages[index].0.clone()?,
            action: if rejected {
                Action::Rejected
            } else {
                Action::Acknowledged
            },
            error,
        })
    }

    /// Whether group `index` has a fault of its own: whether it rejects, and
    /// the first one's code.
    fn group_verdict(&self, index: usize) -> Option<(bool, LevelError)> {
        let mut faults = self
            .findings
            .iter()
            .filter(|f| f.level() == ReportingLevel::Group && f.location.group() == Some(index));
        let first = faults.next()?;
        let rejected = first.rejects || faults.any(|f| f.rejects);
        Some((rejected, LevelError::of(&first.location)))
    }

    /// Check every action code against the policy.
    fn checked(self, options: &ContrlOptions) -> Result<Self, EdifactError> {
        let policy = options.actions;
        let check = |level: ReportingLevel, action: Action| {
            if policy.permits(level, action) {
                Ok(())
            } else {
                Err(EdifactError::ActionNotPermitted {
                    segment_tag: level.tag(),
                    action: action.code(),
                })
            }
        };
        check(ReportingLevel::Interchange, self.action)?;
        for message in &self.messages {
            check(ReportingLevel::Message, message.action)?;
        }
        for group in &self.groups {
            check(ReportingLevel::Group, group.action)?;
            for message in &group.messages {
                check(ReportingLevel::Message, message.action)?;
            }
        }
        Ok(self)
    }

    /// Set the `UNH` message reference (DE 0062) of the `CONTRL` itself.
    ///
    /// Defaults to the subject interchange's control reference.
    #[must_use]
    pub fn with_message_reference(mut self, reference: impl Into<String>) -> Self {
        self.message_ref = Some(reference.into());
        self
    }

    /// Set the `UNH` S009 of the `CONTRL` itself, replacing the one derived
    /// from the subject (or given in [`ContrlOptions`]).
    #[must_use]
    pub fn with_identifier(mut self, id: ContrlIdentifier) -> Self {
        self.identifier = id;
        self
    }

    /// The `UNH` S009 this `CONTRL` will carry.
    #[must_use]
    pub fn identifier(&self) -> &ContrlIdentifier {
        &self.identifier
    }

    /// Record an interchange-level error on the `UCI`.
    ///
    /// Ignored when `error` is not permitted at the interchange level by
    /// ISO 9735-4 Annex A — a `CONTRL` carrying a code its level may not use is
    /// one the partner's translator rejects, which helps nobody.
    #[must_use]
    pub fn with_interchange_error(mut self, error: SyntaxError) -> Self {
        if error.permitted_at(ReportingLevel::Interchange) {
            self.interchange_error = Some(LevelError::code(error));
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

        // UNH — DE 0065 is fixed to CONTRL by ISO 9735-4 §5.4.1; the rest of
        // S009 is the identifier.
        out.push(OwnedSegment::new(
            "UNH",
            vec![
                OwnedElement::of(std::slice::from_ref(&reference)),
                OwnedElement::of(&self.identifier.components()),
            ],
        ));

        // UCI — interchange level.
        let mut uci = vec![
            OwnedElement::of(std::slice::from_ref(&self.control_ref)),
            party(&self.sender, &self.sender_qualifier),
            party(&self.recipient, &self.recipient_qualifier),
            OwnedElement::of(&[self.action.code()]),
        ];
        if let Some(error) = &self.interchange_error {
            error.push_onto(&mut uci);
        }
        out.push(OwnedSegment::new("UCI", uci));

        // SG1 (ungrouped) or SG3 (grouped) — never both, per §5.3.1.
        for message in &self.messages {
            out.extend(self.message_report(message));
        }
        for group in &self.groups {
            let mut ucf = vec![
                OwnedElement::of(std::slice::from_ref(&group.name.reference)),
                party(&group.name.sender, &group.name.sender_qualifier),
                party(&group.name.recipient, &group.name.recipient_qualifier),
                OwnedElement::of(&[group.action.code()]),
            ];
            if let Some(error) = &group.error {
                error.push_onto(&mut ucf);
            }
            out.push(OwnedSegment::new("UCF", ucf));
            for message in &group.messages {
                out.extend(self.message_report(message));
            }
        }

        let count = (out.len() + 1).to_string();
        out.push(OwnedSegment::new(
            "UNT",
            vec![OwnedElement::of(&[count]), OwnedElement::of(&[reference])],
        ));
        out
    }

    /// Emit one `UCM` and the `UCS`/`UCD` pairs beneath it.
    fn message_report(&self, report: &MessageReport) -> Vec<OwnedSegment> {
        let name = &report.name;
        // ISO 9735-4 identifies the message by copying DE 0062 and S009 —
        // including DE 0057 when the subject carries one.
        let mut s009 = vec![
            name.message_type.clone(),
            name.version.clone(),
            name.release.clone(),
            name.agency.clone(),
        ];
        if !name.association.is_empty() {
            s009.push(name.association.clone());
        }
        let mut ucm = vec![
            OwnedElement::of(std::slice::from_ref(&name.reference)),
            OwnedElement::of(&s009),
            OwnedElement::of(&[report.action.code()]),
        ];
        if let Some(error) = &report.error {
            error.push_onto(&mut ucm);
        }
        let mut out = vec![OwnedSegment::new("UCM", ucm)];
        out.extend(self.segment_reports(report.index));
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
            let Some(position) = finding.location.segment_position() else {
                // Findings on the UCM itself; `message_report` placed them.
                continue;
            };
            // §5.3.3: no more than one reporting level per referenced level, so a
            // second finding on the same segment extends it with a UCD rather
            // than opening a second UCS for it.
            if !reported_positions.contains(&position) {
                reported_positions.push(position);
                let position_text = position.to_string();
                let mut ucs = vec![OwnedElement::of(&[position_text])];
                // The UCS states the code only when the segment itself is the
                // deepest level that can carry it.
                if level == ReportingLevel::Segment {
                    ucs.push(OwnedElement::of(&[finding.location.error().code()]));
                }
                out.push(OwnedSegment::new("UCS", ucs));
            }

            if level == ReportingLevel::DataElement {
                let element_text = finding
                    .location
                    .element_position()
                    .expect("DataElement level implies a known element position")
                    .to_string();
                let mut identification = vec![element_text];
                if let Some(component) = finding.location.component_position() {
                    identification.push(component.to_string());
                }
                out.push(OwnedSegment::new(
                    "UCD",
                    vec![
                        OwnedElement::of(&[finding.location.error().code()]),
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
        crate::segments_to_bytes(&self.segments())
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
    /// because the reply goes back the way it came; each keeps its
    /// identification code qualifier (DE 0007).
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
        // The reply goes back the way it came: the subject's recipient sends it,
        // and each party keeps its identification code qualifier (S002/S003
        // DE 0007), which `Writer::begin_interchange` has no slot for.
        let sender = party_components(&self.recipient, &self.recipient_qualifier);
        let recipient = party_components(&self.sender, &self.sender_qualifier);
        writer.write_composites(
            "UNB",
            &[
                &[syntax_identifier, syntax_version][..],
                &sender[..],
                &recipient[..],
                &[date, time][..],
                &[control_reference][..],
            ],
        )?;
        for segment in self.segments() {
            writer.write_segment(&segment)?;
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

/// The components of an S002/S003 party, omitting an absent qualifier.
fn party_components<'a>(id: &'a str, qualifier: &'a str) -> Vec<&'a str> {
    if qualifier.is_empty() {
        vec![id]
    } else {
        vec![id, qualifier]
    }
}

/// Build an S002/S003-shaped element, omitting an absent qualifier.
fn party(id: &str, qualifier: &str) -> OwnedElement {
    if qualifier.is_empty() {
        OwnedElement::of(&[id.to_owned()])
    } else {
        OwnedElement::of(&[id.to_owned(), qualifier.to_owned()])
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

/// Place a [`ValidationIssue`] on the subject interchange.
///
/// Going through the byte span rather than through a dedicated field on
/// [`ValidationIssue`] is what makes this work for findings from *any* validator,
/// including third-party ones that know nothing about `CONTRL`.
///
/// The segment holding the span's start is the anchor; a message wins over the
/// group around it, because §5.3.3 wants the lowest level that can express the
/// fault. An offset in neither — the `UNB`, the `UNZ`, or the gap between
/// structures — makes the finding the interchange's own. An issue without a
/// span is placed by its stable code alone (`E004` on `UNZ`, `E031`/`E039` on
/// `UNB`, the rest on the interchange).
///
/// `UCS` DE 0096 counts from the `UNH` as position 1, so the position is the
/// segment's offset from its message header plus one.
fn locate_issue(
    segments: &[Segment<'_>],
    messages: &[(usize, usize)],
    groups: &[(usize, usize)],
    issue: &ValidationIssue,
    error: SyntaxError,
) -> ContrlLocation {
    let stable = issue.error_code();
    let detail = match issue.span {
        Some(span) => Detail {
            anchor: Anchor::Span(span),
            element_index: issue.element_index.map(usize::from),
            component_index: issue.component_index.map(usize::from),
        },
        None => anchor_for_code(stable),
    };
    let target = match detail.anchor {
        Anchor::Span(span) => segment_target(segments, messages, groups, span.start),
        Anchor::Service(tag) => Some(Target {
            tag: Some(tag),
            ..Target::interchange(true)
        }),
        _ => Some(Target::interchange(true)),
    };
    let target = target
        .unwrap_or(Target::interchange(true))
        .with_indices(detail.element_index, detail.component_index);
    place(target, error, stable)
}

/// The segment containing byte `offset` and the structure around it.
///
/// Segments are in source order, so the containing one is the last whose span
/// starts at or before the offset. `None` when the offset precedes every
/// segment.
fn segment_target<'s>(
    segments: &'s [Segment<'_>],
    messages: &[(usize, usize)],
    groups: &[(usize, usize)],
    offset: usize,
) -> Option<Target<'s>> {
    let index = match segments.binary_search_by(|segment| segment.span.start.cmp(&offset)) {
        Ok(exact) => exact,
        Err(0) => return None,
        Err(next) => next - 1,
    };
    let message = messages
        .iter()
        .enumerate()
        .find(|(_, (start, end))| (*start..=*end).contains(&index))
        .map(|(message_index, (start, _))| (message_index, u32::try_from(index - start + 1).ok()));
    let group = groups
        .iter()
        .position(|(start, end)| (*start..=*end).contains(&index));
    Some(Target {
        tag: Some(segments[index].tag()),
        message,
        group,
        ..Target::interchange(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &[u8]) -> Vec<OwnedSegment> {
        crate::from_bytes(input)
            .map(|r| r.map(|s| s.into_owned()))
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
        let validated = crate::validate_envelope(&owned).expect("valid subject");

        let contrl = Contrl::acknowledgement(&validated).with_message_reference("ACK1");
        let wire = contrl.to_edifact_string().expect("render");

        assert_eq!(
            wire,
            "UNH+ACK1+CONTRL:D:3:UN'UCI+IC4711+SENDER:14+RECEIVER:14+7'UNT+3+ACK1'"
        );

        // And it is itself a well-formed CONTRL by the crate's own tables.
        let reparsed = parse(wire.as_bytes());
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
            .validate(&reparsed);
        assert!(!report.has_errors(), "{:#?}", report.errors());
    }

    #[test]
    fn a_receipt_carries_action_8_and_nothing_else() {
        let raw =
            b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNZ+1+IC1'";
        let owned = parse(raw);
        let validated = crate::validate_envelope(&owned).expect("valid subject");

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
        let validated = crate::validate_envelope(&owned).expect("valid subject");
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
        let validated = crate::validate_envelope(&owned).expect("valid subject");

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
        let validated = crate::validate_envelope_lenient(&owned)
            .interchange
            .expect("subject must be structurally interpretable");
        let report = crate::ValidationContext::builder()
            .with_envelope_validation()
            .with_syntax_validation()
            .build()
            .validate(&owned);
        Contrl::from_report(&validated, &owned, &report)
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
            "UNH+IC1+CONTRL:D:3:UN'\
             UCI+IC1+S+R+7'\
             UCM+M1+ORDERS:D:96A:UN+4+26+UNH'\
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
        assert_eq!(
            wire,
            "UNH+IC1+CONTRL:D:3:UN'UCI+IC1+S+R+4+28+UNZ'UNT+3+IC1'"
        );
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
            "UNH+IC1+CONTRL:D:3:UN'\
             UCI+IC1+S+R+7'\
             UCM+M1+ORDERS:D:96A:UN+4+29+UNT'\
             UNT+4+IC1'"
        );
    }

    #[test]
    fn a_finding_lands_at_the_lowest_level_annex_a_permits() {
        // "Duplicate detected" is legal at UCI/UCF/UCM and nowhere lower, so it
        // must stop at the message even though the fault has a segment position.
        let in_body = Target {
            tag: Some("BGM"),
            message: Some((0, Some(2))),
            element_position: Some(2),
            ..Target::interchange(true)
        };
        let duplicate = place(in_body, SyntaxError::DuplicateDetected, None);
        assert_eq!(duplicate.level(), ReportingLevel::Message);

        // "Invalid value" is legal everywhere, so it descends all the way.
        let invalid = place(in_body, SyntaxError::InvalidValue, None);
        assert_eq!(invalid.level(), ReportingLevel::DataElement);

        // …and stops at the segment when there is no data element to name.
        let segment_only = Target {
            element_position: None,
            ..in_body
        };
        let segment_only = place(segment_only, SyntaxError::InvalidValue, None);
        assert_eq!(segment_only.level(), ReportingLevel::Segment);

        // "Too many segment group repetitions" is UCS-only: it cannot descend to
        // UCD even with an element position, and must not climb to UCM.
        let group_repetitions = place(in_body, SyntaxError::TooManyGroupRepetitions, None);
        assert_eq!(group_repetitions.level(), ReportingLevel::Segment);
    }

    #[test]
    fn an_empty_message_is_reported_as_missing_never_as_lower_level_empty() {
        // ISO 9735-4 Annex A permits `32` (lower level empty) only on UCI/UCF.
        // The body segments are what is missing, so the UCM says `13`.
        let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
                    UNH+M1+ORDERS:D:96A:UN'UNT+2+M1'\
                    UNZ+1+IC1'";
        let contrl = report_for(raw);
        let wire = contrl.to_edifact_string().expect("render");
        assert!(wire.contains("UCM+M1+ORDERS:D:96A:UN+4+13+UNH'"), "{wire}");
        assert!(!wire.contains("+32'"), "{wire}");
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
        assert_eq!(validated.messages[0].version, "D");
        assert_eq!(validated.messages[0].release, "3");
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
            "UNH+IC1+CONTRL:D:3:UN'\
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
            "UNH+IC1+CONTRL:D:3:UN'\
             UCI+IC1+S+R+7'\
             UCF+GRP1+SND+RCV+4+28+UNE'\
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
            "UNH+IC1+CONTRL:D:3:UN'UCI+IC1+S+R+7'UNT+3+IC1'"
        );
    }

    #[test]
    fn group_placement_errors_map_to_their_annex_a_codes() {
        use EdifactError as E;
        let span = crate::Span::new(0, 3);
        let cases = [
            (
                E::SegmentNotPlaced {
                    tag: "RFF".to_owned(),
                    reason: crate::group::UnplacedReason::NotHere,
                    span,
                },
                SyntaxError::NotSupportedHere,
                "15",
            ),
            (
                E::TooManySegmentOccurrences {
                    tag: "DTM".to_owned(),
                    max: 1,
                    span,
                },
                SyntaxError::TooManyRepetitions,
                "35",
            ),
            (
                E::TooManyGroupOccurrences {
                    group: "SG8".to_owned(),
                    max: 2,
                    span,
                },
                SyntaxError::TooManyGroupRepetitions,
                "36",
            ),
        ];
        for (error, expected, code) in cases {
            assert_eq!(SyntaxError::for_error(&error), expected, "{error}");
            assert_eq!(expected.code(), code);
            // The issue path reads the stable code, not the error, and must agree.
            let issue = ValidationIssue::new(crate::ValidationSeverity::Error, error.to_string())
                .with_error_code(error.stable_code());
            assert_eq!(SyntaxError::for_issue(&issue), expected, "{error}");
        }
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
