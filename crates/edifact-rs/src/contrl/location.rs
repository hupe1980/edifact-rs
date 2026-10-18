//! Where a fault sits in the subject interchange, in `CONTRL` coordinates.
//!
//! One mapping from a subject-input [`EdifactError`] (or a
//! [`ValidationIssue`][crate::ValidationIssue]) to the reporting level and
//! positions ISO 9735-4 asks for, shared by [`Contrl::from_report`] and
//! [`RecoveredInterchange::locate`][crate::recover::RecoveredInterchange::locate]
//! so the two paths cannot disagree.
//!
//! [`Contrl::from_report`]: super::Contrl::from_report

use super::{ReportingLevel, SyntaxError};
use crate::model::Span;
use crate::{EdifactError, ServiceStringAdvice};

/// Where a fault sits in the subject interchange, in `CONTRL` terms.
///
/// The coordinates ISO 9735-4 gives a reporting level:
///
/// | Getter | Data element | Meaning |
/// |---|---|---|
/// | [`level`](Self::level) | — | `UCI`, `UCF`, `UCM`, `UCS` or `UCD` |
/// | [`group`](Self::group) | — | index of the group (`UNG`…`UNE`) in document order |
/// | [`message`](Self::message) | — | flat index of the message (`UNH`…`UNT`) in document order, as [`ValidatedInterchange::messages`][crate::ValidatedInterchange] counts them |
/// | [`segment_position`](Self::segment_position) | DE 0096 | segment position in the message, `UNH` = 1, unreadable segments included |
/// | [`service_segment`](Self::service_segment) | DE 0135 (syntax 4) / DE 0013 (syntax 3) | the service segment at fault: `UNA`, `UNB`, `UNZ`, `UNG`, `UNE`, `UNH` or `UNT` |
/// | [`element_position`](Self::element_position) | DE 0098 | data element position in the segment, the tag = 1; repetitions do not advance it |
/// | [`component_position`](Self::component_position) | DE 0104 | component position in the composite, first = 1 |
/// | [`error`](Self::error) | DE 0085 | the syntax error code |
/// | [`is_exact`](Self::is_exact) | — | `false` when the segment was found by tag search |
///
/// # Invariants
///
/// - [`error`](Self::error) is always [permitted](SyntaxError::permitted_at) at
///   [`level`](Self::level): a code ISO 9735-4 Annex A forbids at the lowest
///   locating level climbs (message → group → interchange) until a level
///   carries it, and `18` (unspecified) stands in for a code no reachable level
///   may carry.
/// - `UCS` and `UCD` carry both [`message`](Self::message) and
///   [`segment_position`](Self::segment_position); `UCD` carries
///   [`element_position`](Self::element_position).
/// - [`component_position`](Self::component_position) is only ever set
///   together with [`element_position`](Self::element_position).
/// - [`service_segment`](Self::service_segment) is set only on `UCI`
///   (`UNA`/`UNB`/`UNZ`), `UCF` (`UNG`/`UNE`) and `UCM` (`UNH`/`UNT`). On those
///   levels, [`element_position`](Self::element_position) and
///   [`component_position`](Self::component_position) are the S011 of that
///   level (the erroneous element inside the service segment).
///
/// Built by [`RecoveredInterchange::locate`][crate::recover::RecoveredInterchange::locate]
/// and [`RecoveredInterchange::fault_locations`][crate::recover::RecoveredInterchange::fault_locations];
/// [`Contrl::from_report`][super::Contrl::from_report] places its findings
/// through the same mapping.
///
/// # Example
///
/// ```
/// use edifact_rs::contrl::{ReportingLevel, SyntaxError};
/// use edifact_rs::{RecoveredInterchange, RecoveryConfig};
///
/// // The second segment of the message has a broken tag (`BGME03`).
/// let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
///               UNH+1+ORDERS:D:96A:UN'BGME03+X'UNT+3+1'UNZ+1+IC1'";
/// let recovered = RecoveredInterchange::scan(input, &RecoveryConfig::default());
/// let location = &recovered.fault_locations()[0];
///
/// assert_eq!(location.level(), ReportingLevel::DataElement);
/// assert_eq!(location.message(), Some(0));
/// assert_eq!(location.segment_position(), Some(2));
/// assert_eq!(location.element_position(), Some(1)); // the tag itself
/// assert_eq!(location.error(), SyntaxError::InvalidValue); // DE 0085 `12`
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ContrlLocation {
    level: ReportingLevel,
    group: Option<usize>,
    message: Option<usize>,
    segment_position: Option<u32>,
    service_segment: Option<ServiceSegment>,
    element_position: Option<u32>,
    component_position: Option<u32>,
    error: SyntaxError,
    exact: bool,
}

impl ContrlLocation {
    /// The reporting level that carries the fault.
    #[must_use]
    pub const fn level(&self) -> ReportingLevel {
        self.level
    }

    /// Index of the group (`UNG`…`UNE`) the fault belongs to, in document order.
    ///
    /// Set on `UCF` and, for a grouped interchange, on every lower level.
    #[must_use]
    pub const fn group(&self) -> Option<usize> {
        self.group
    }

    /// Flat index of the message (`UNH`…`UNT`) the fault belongs to, in
    /// document order across all groups.
    ///
    /// Set on `UCM`, `UCS` and `UCD`.
    #[must_use]
    pub const fn message(&self) -> Option<usize> {
        self.message
    }

    /// DE 0096 — the segment position in the message, `UNH` = 1.
    ///
    /// `Some` exactly when the level is `UCS` or `UCD`.
    #[must_use]
    pub const fn segment_position(&self) -> Option<u32> {
        self.segment_position
    }

    /// DE 0135 (syntax version 4) / DE 0013 (syntax version 3) — the service
    /// segment the fault sits in.
    ///
    /// One of `UNA`, `UNB`, `UNZ` (on `UCI`), `UNG`, `UNE` (on `UCF`), `UNH`,
    /// `UNT` (on `UCM`).
    #[must_use]
    pub const fn service_segment(&self) -> Option<&'static str> {
        match self.service_segment {
            Some(segment) => Some(segment.tag()),
            None => None,
        }
    }

    /// DE 0098 — the data element position, the segment tag = 1.
    ///
    /// On `UCD` the erroneous data element; on `UCI`/`UCF`/`UCM` with a
    /// [`service_segment`](Self::service_segment), the element of S011.
    #[must_use]
    pub const fn element_position(&self) -> Option<u32> {
        self.element_position
    }

    /// DE 0104 — the component position inside the composite, first = 1.
    #[must_use]
    pub const fn component_position(&self) -> Option<u32> {
        self.component_position
    }

    /// DE 0085 — the syntax error code, always permitted at [`level`](Self::level).
    #[must_use]
    pub const fn error(&self) -> SyntaxError {
        self.error
    }

    /// Whether the location was read off the fault itself.
    ///
    /// `false` when the error carries no byte position and the segment was
    /// found by searching the message for the first segment with the error's
    /// tag, or when such an error could only be placed on the interchange.
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        self.exact
    }
}

impl std::fmt::Display for ContrlLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.level.tag())?;
        if let Some(message) = self.message {
            write!(f, " message {message}")?;
        } else if let Some(group) = self.group {
            write!(f, " group {group}")?;
        }
        if let Some(service) = self.service_segment() {
            write!(f, " {service}")?;
        }
        if let Some(position) = self.segment_position {
            write!(f, " segment {position}")?;
        }
        if let Some(element) = self.element_position {
            write!(f, " element {element}")?;
            if let Some(component) = self.component_position {
                write!(f, ":{component}")?;
            }
        }
        write!(f, " code {}", self.error.code())
    }
}

/// The service segments a `CONTRL` level can name (DE 0135 / DE 0013).
///
/// Private so the public getter stays a plain `&'static str`; serialises as the
/// tag (`"UNH"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
enum ServiceSegment {
    #[cfg_attr(feature = "serde", serde(rename = "UNA"))]
    Una,
    #[cfg_attr(feature = "serde", serde(rename = "UNB"))]
    Unb,
    #[cfg_attr(feature = "serde", serde(rename = "UNZ"))]
    Unz,
    #[cfg_attr(feature = "serde", serde(rename = "UNG"))]
    Ung,
    #[cfg_attr(feature = "serde", serde(rename = "UNE"))]
    Une,
    #[cfg_attr(feature = "serde", serde(rename = "UNH"))]
    Unh,
    #[cfg_attr(feature = "serde", serde(rename = "UNT"))]
    Unt,
}

impl ServiceSegment {
    const ALL: [Self; 7] = [
        Self::Una,
        Self::Unb,
        Self::Unz,
        Self::Ung,
        Self::Une,
        Self::Unh,
        Self::Unt,
    ];

    const fn tag(self) -> &'static str {
        match self {
            Self::Una => "UNA",
            Self::Unb => "UNB",
            Self::Unz => "UNZ",
            Self::Ung => "UNG",
            Self::Une => "UNE",
            Self::Unh => "UNH",
            Self::Unt => "UNT",
        }
    }
}

/// `tag` as a service segment, if it is one of `allowed`.
fn service(tag: Option<&str>, allowed: &[&str]) -> Option<ServiceSegment> {
    let tag = tag.filter(|t| allowed.contains(t))?;
    ServiceSegment::ALL.into_iter().find(|s| s.tag() == tag)
}

// ── what an error says about its own position ────────────────────────────────

/// How an error points into the subject interchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchor<'e> {
    /// A byte offset (lexical errors).
    Offset {
        offset: usize,
        /// Whether DE 0098 / DE 0104 can be counted from the offset.
        element: bool,
    },
    /// A byte range (validation errors on a parsed segment).
    Span(Span),
    /// No position, but the segment tag: the first segment with that tag in the
    /// message the caller names.
    Tag(&'e str),
    /// A segment that is absent: the message the caller names, or the
    /// interchange for `UNB`/`UNZ`.
    Missing(&'e str),
    /// Tied to a service segment by the nature of the error itself.
    Service(&'static str),
    /// About the interchange as a whole.
    Interchange,
    /// No position at all (`E006` without a fault to match, `E038`): the
    /// interchange, not exact.
    Unplaced,
}

/// What an error says about where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Detail<'e> {
    pub(crate) anchor: Anchor<'e>,
    /// Zero-based element index named by the error.
    pub(crate) element_index: Option<usize>,
    /// Zero-based component index named by the error.
    pub(crate) component_index: Option<usize>,
}

impl<'e> Detail<'e> {
    const fn new(anchor: Anchor<'e>) -> Self {
        Self {
            anchor,
            element_index: None,
            component_index: None,
        }
    }

    const fn element(mut self, element: usize) -> Self {
        self.element_index = Some(element);
        self
    }

    const fn component(mut self, component: usize) -> Self {
        self.component_index = Some(component);
        self
    }
}

/// What `error` says about its position, or `None` when it is not a fault of
/// the subject input (writer, serializer and definition errors).
///
/// Written without a wildcard arm, so a new error variant cannot compile until
/// it is classified here.
pub(crate) fn detail_of(error: &EdifactError) -> Option<Detail<'_>> {
    use EdifactError as E;
    let offset = |offset: usize| {
        Detail::new(Anchor::Offset {
            offset,
            element: true,
        })
    };
    Some(match error {
        E::UnexpectedEof { offset: at }
        | E::InvalidDelimiter { offset: at, .. }
        | E::InvalidText { offset: at }
        | E::InvalidReleaseSequence { offset: at }
        | E::UnexpectedDataToken { offset: at } => offset(*at),
        // The whole segment is too long; no single element is at fault.
        E::SegmentTooLong { offset, .. } => Detail::new(Anchor::Offset {
            offset: *offset,
            element: false,
        }),
        E::MessageCountMismatch { .. } => Detail::new(Anchor::Service("UNZ")),
        E::UnrecognisedSyntaxIdentifier(_) | E::UnsupportedCharset { .. } => {
            // UNB S001 DE 0001: element 2, component 1.
            Detail::new(Anchor::Service("UNB")).element(0).component(0)
        }
        E::InterchangeTooLarge { .. } | E::LimitExceeded { .. } | E::EmptyInterchange { .. } => {
            Detail::new(Anchor::Interchange)
        }
        E::InvalidUna => Detail::new(Anchor::Service("UNA")),
        E::InvalidSegmentTag(_) | E::CharacterNotInRepertoire { .. } => {
            Detail::new(Anchor::Unplaced)
        }
        E::MissingRequiredElement { tag, element_index }
        | E::InvalidFieldValue {
            tag, element_index, ..
        } => Detail::new(Anchor::Tag(tag)).element(*element_index),
        E::MissingRequiredComponent {
            tag,
            element_index,
            component_index,
        } => Detail::new(Anchor::Tag(tag))
            .element(*element_index)
            .component(*component_index),
        E::MissingSegment { tag, .. } => Detail::new(Anchor::Missing(tag)),
        E::SegmentCountMismatch { span, .. }
        | E::InvalidSegmentForMessage { span, .. }
        | E::InvalidElementCount { span, .. }
        | E::QualifierMismatch { span, .. }
        | E::DuplicateReference { span, .. }
        | E::EmptyMessage { span, .. }
        | E::PackageNotSupported { span, .. }
        | E::SegmentWithoutDataElements { span, .. }
        | E::SegmentNotPlaced { span, .. }
        | E::TooManySegmentOccurrences { span, .. }
        | E::TooManyGroupOccurrences { span, .. }
        | E::GroupsAndMessagesMixed { span } => Detail::new(Anchor::Span(*span)),
        E::InvalidComponentCount {
            element_index,
            span,
            ..
        }
        | E::InvalidCodeValue {
            element_index,
            span,
            ..
        }
        | E::ConditionalRequirementNotMet {
            element_index,
            span,
            ..
        }
        | E::TooManyRepetitions {
            element_index,
            span,
            ..
        } => Detail::new(Anchor::Span(*span)).element(*element_index),
        E::TrailingSeparator {
            element_index,
            span,
            ..
        } => {
            let detail = Detail::new(Anchor::Span(*span));
            match element_index {
                Some(index) => detail.element(*index),
                None => detail,
            }
        }
        E::BlankDataElementValue {
            element_index,
            component_index,
            span,
            ..
        }
        | E::InvalidCharacterType {
            element_index,
            component_index,
            span,
            ..
        }
        | E::DataElementTooLong {
            element_index,
            component_index,
            span,
            ..
        }
        | E::DataElementTooShort {
            element_index,
            component_index,
            span,
            ..
        }
        | E::InsignificantCharacters {
            element_index,
            component_index,
            span,
            ..
        } => Detail::new(Anchor::Span(*span))
            .element(*element_index)
            .component(*component_index),
        // Not faults of the subject input: nothing to locate.
        E::InvalidUtf8
        | E::Io(_)
        | E::ValidationErrors { .. }
        | E::InvalidEventSequence { .. }
        | E::InvalidElementPosition
        | E::IncompatibleReleaseScopes { .. }
        | E::UnknownDataElement { .. }
        | E::AmbiguousDataElement { .. }
        | E::SegmentLayoutMismatch { .. }
        | E::RepetitionSeparatorNotDeclared
        | E::NonFiniteNumber { .. }
        | E::CharacterRepertoireMismatch { .. }
        // Caller configuration of a `CONTRL`, not a fault of the subject.
        | E::ActionNotPermitted { .. }
        | E::InvalidContrlIdentifier { .. }
        // A malformed group schema is caller configuration as well.
        | E::InvalidGroupSchema { .. } => return None,
    })
}

/// The position-free anchor a stable code implies, for an issue without a span.
///
/// Mirrors the position-free rows of [`detail_of`], for findings that arrive as
/// a [`ValidationIssue`][crate::ValidationIssue] rather than as the error.
pub(crate) fn anchor_for_code(code: Option<&str>) -> Detail<'static> {
    match code {
        Some("E004") => Detail::new(Anchor::Service("UNZ")),
        Some("E031" | "E039") => Detail::new(Anchor::Service("UNB")).element(0).component(0),
        Some("E007") => Detail::new(Anchor::Service("UNA")),
        _ => Detail::new(Anchor::Interchange),
    }
}

// ── placing a resolved anchor ────────────────────────────────────────────────

/// The structure around the segment an anchor resolved to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Target<'t> {
    /// Tag of the anchor segment, when known.
    pub(crate) tag: Option<&'t str>,
    /// The message window around it: its index and, when the anchor is a
    /// segment of it, DE 0096. `(index, None)` places the fault on the message
    /// as a whole.
    pub(crate) message: Option<(usize, Option<u32>)>,
    /// The group window around it.
    pub(crate) group: Option<usize>,
    /// DE 0098, when known.
    pub(crate) element_position: Option<u32>,
    /// DE 0104, when known (only with `element_position`).
    pub(crate) component_position: Option<u32>,
    /// Whether the anchor was read off the fault rather than searched for.
    pub(crate) exact: bool,
}

impl<'t> Target<'t> {
    /// A target on the interchange as a whole.
    pub(crate) const fn interchange(exact: bool) -> Self {
        Self {
            tag: None,
            message: None,
            group: None,
            element_position: None,
            component_position: None,
            exact,
        }
    }

    /// Element and component positions from zero-based indices.
    pub(crate) fn with_indices(mut self, element: Option<usize>, component: Option<usize>) -> Self {
        // DE 0098 counts the tag as position 1, so a zero-based element index is
        // two positions further along; DE 0104 counts from 1.
        self.element_position = element
            .and_then(|e| u32::try_from(e).ok())
            .and_then(|e| e.checked_add(2));
        self.component_position = self
            .element_position
            .and(component)
            .and_then(|c| u32::try_from(c).ok())
            .and_then(|c| c.checked_add(1));
        self
    }
}

/// Place a fault with DE 0085 `code` (the code [`SyntaxError::for_error`] or
/// [`SyntaxError::for_issue`] gives) and the stable code it came from.
///
/// Implements the level algorithm: a service segment's own level; otherwise the
/// deepest of `UCD`/`UCS` that Annex A permits for the code; otherwise climbing
/// `UCM` → `UCF` → `UCI`. The override table is exhaustive:
///
/// | Stable code | Where | DE 0085 |
/// |---|---|---|
/// | `E006`, `E028` | inside a message body | `12` at DE 0098 = 1 (the tag is the invalid value) |
/// | `E007` | `UCI`, service segment `UNA` | `20` |
/// | `E043` | `UCM`, service segment `UNH` | `13` (the body segments are missing) |
pub(crate) fn place(target: Target<'_>, code: SyntaxError, stable: Option<&str>) -> ContrlLocation {
    let exact = target.exact;
    let base = move || ContrlLocation {
        level: ReportingLevel::Interchange,
        group: None,
        message: None,
        segment_position: None,
        service_segment: None,
        element_position: None,
        component_position: None,
        error: code,
        exact,
    };

    // ISO 9735-4 Annex A: "character invalid as service character" is the
    // honest code for a UNA whose service characters cannot be used.
    if stable == Some("E007") {
        return ContrlLocation {
            service_segment: Some(ServiceSegment::Una),
            error: SyntaxError::InvalidAsServiceCharacter,
            ..base()
        };
    }

    let climb = |code: SyntaxError| -> ContrlLocation {
        match target.group {
            Some(group) if code.permitted_at(ReportingLevel::Group) => ContrlLocation {
                level: ReportingLevel::Group,
                group: Some(group),
                error: code,
                ..base()
            },
            _ => interchange(base(), None, None, None, code),
        }
    };

    if let Some((message, position)) = target.message {
        let service_segment = service(target.tag, &["UNH", "UNT"]);
        let on_message = |code: SyntaxError, service_segment: Option<ServiceSegment>| {
            let (element, component) = if service_segment.is_some() {
                (target.element_position, target.component_position)
            } else {
                (None, None)
            };
            ContrlLocation {
                level: ReportingLevel::Message,
                group: target.group,
                message: Some(message),
                service_segment,
                element_position: element,
                component_position: component,
                error: code,
                ..base()
            }
        };
        let Some(position) = position.filter(|_| service_segment.is_none()) else {
            // The message's own header or trailer, or the message as a whole.
            let code = if stable == Some("E043") {
                SyntaxError::Missing
            } else {
                code
            };
            return if code.permitted_at(ReportingLevel::Message) {
                on_message(code, service_segment)
            } else {
                climb(code)
            };
        };

        // A segment of the message body.
        let (code, element, component) = if matches!(stable, Some("E006" | "E028")) {
            (SyntaxError::InvalidValue, Some(1), None)
        } else {
            (code, target.element_position, target.component_position)
        };
        let in_segment = ContrlLocation {
            level: ReportingLevel::Segment,
            group: target.group,
            message: Some(message),
            segment_position: Some(position),
            error: code,
            ..base()
        };
        if let Some(element) = element {
            if code.permitted_at(ReportingLevel::DataElement) {
                return ContrlLocation {
                    level: ReportingLevel::DataElement,
                    element_position: Some(element),
                    component_position: component,
                    ..in_segment
                };
            }
        }
        if code.permitted_at(ReportingLevel::Segment) {
            return in_segment;
        }
        if code.permitted_at(ReportingLevel::Message) {
            return on_message(code, None);
        }
        return climb(code);
    }

    if let Some(group) = target.group {
        let service_segment = service(target.tag, &["UNG", "UNE"]);
        if code.permitted_at(ReportingLevel::Group) {
            let (element, component) = if service_segment.is_some() {
                (target.element_position, target.component_position)
            } else {
                (None, None)
            };
            return ContrlLocation {
                level: ReportingLevel::Group,
                group: Some(group),
                service_segment,
                element_position: element,
                component_position: component,
                ..base()
            };
        }
        return interchange(base(), None, None, None, code);
    }

    let service_segment = service(target.tag, &["UNA", "UNB", "UNZ"]);
    let (element, component) = if service_segment.is_some() {
        (target.element_position, target.component_position)
    } else {
        (None, None)
    };
    interchange(base(), service_segment, element, component, code)
}

/// A `UCI` location, falling back to `18` (unspecified) for a code Annex A
/// forbids even there.
fn interchange(
    base: ContrlLocation,
    service_segment: Option<ServiceSegment>,
    element_position: Option<u32>,
    component_position: Option<u32>,
    code: SyntaxError,
) -> ContrlLocation {
    let error = if code.permitted_at(ReportingLevel::Interchange) {
        code
    } else {
        SyntaxError::Unspecified
    };
    ContrlLocation {
        level: ReportingLevel::Interchange,
        group: None,
        message: None,
        segment_position: None,
        service_segment,
        element_position,
        component_position,
        error,
        ..base
    }
}

// ── counting DE 0098 / DE 0104 from raw bytes ────────────────────────────────

/// DE 0098 and DE 0104 of byte `at` inside `segment` (the raw bytes of one
/// segment, starting at its tag).
///
/// The tag is element 1 and every unreleased element separator before `at`
/// adds one; repetition separators start a new occurrence of the same element
/// and do not. DE 0104 is the component count inside the current occurrence and
/// is given only when the element is a composite **on the wire** — a component
/// separator appears somewhere in it.
pub(crate) fn element_position(
    segment: &[u8],
    at: usize,
    ssa: &ServiceStringAdvice,
) -> (Option<u32>, Option<u32>) {
    let repetition = ssa.is_repetition_active().then_some(ssa.repetition_sep);
    let mut element = 1usize;
    let mut component = 1usize;
    let mut composite = false;
    let upto = at.min(segment.len());
    let mut i = 0usize;
    while i < upto {
        let b = segment[i];
        if b == ssa.release_char {
            i += 2;
            continue;
        }
        if b == ssa.segment_term {
            break;
        }
        if b == ssa.element_sep {
            element += 1;
            component = 1;
            composite = false;
        } else if b == ssa.component_sep {
            component += 1;
            composite = true;
        } else if Some(b) == repetition {
            component = 1;
        }
        i += 1;
    }
    // A component separator after `at` in the same element also makes it a
    // composite.
    let mut j = i.max(upto);
    while j < segment.len() {
        let b = segment[j];
        if b == ssa.release_char {
            j += 2;
            continue;
        }
        if b == ssa.element_sep || b == ssa.segment_term {
            break;
        }
        if b == ssa.component_sep {
            composite = true;
        }
        j += 1;
    }
    let element = u32::try_from(element).ok();
    let component = element
        .and_then(|_| composite.then(|| u32::try_from(component).ok()))
        .flatten();
    (element, component)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssa() -> ServiceStringAdvice {
        ServiceStringAdvice::default()
    }

    #[test]
    fn the_tag_is_element_one() {
        assert_eq!(element_position(b"BGM+220'", 0, &ssa()), (Some(1), None));
    }

    #[test]
    fn a_composite_on_the_wire_gets_a_component_position() {
        // `X?` … the offset of `X` in `BGM+E03+X:A'` is 8.
        assert_eq!(
            element_position(b"BGM+E03+X:A'", 8, &ssa()),
            (Some(3), Some(1))
        );
        assert_eq!(
            element_position(b"BGM+E03+X:A'", 10, &ssa()),
            (Some(3), Some(2))
        );
        // A simple element has no DE 0104.
        assert_eq!(element_position(b"BGM+E03+X'", 8, &ssa()), (Some(3), None));
    }

    #[test]
    fn released_separators_do_not_count() {
        assert_eq!(element_position(b"FTX+A?+B+C'", 9, &ssa()), (Some(3), None));
    }

    #[test]
    fn repetitions_do_not_advance_the_element() {
        let v4 = ServiceStringAdvice::for_syntax_version(Some(4));
        assert_eq!(element_position(b"RFF+A*B+C'", 6, &v4), (Some(2), None));
        assert_eq!(element_position(b"RFF+A*B+C'", 8, &v4), (Some(3), None));
    }

    #[test]
    fn a_body_tag_fault_is_value_12_on_the_tag() {
        let target = Target {
            tag: None,
            message: Some((1, Some(2))),
            group: None,
            element_position: None,
            component_position: None,
            exact: true,
        };
        let location = place(
            target,
            SyntaxError::InvalidOccurrenceOutsideMessage,
            Some("E006"),
        );
        assert_eq!(location.level(), ReportingLevel::DataElement);
        assert_eq!(location.element_position(), Some(1));
        assert_eq!(location.error(), SyntaxError::InvalidValue);
    }
}
