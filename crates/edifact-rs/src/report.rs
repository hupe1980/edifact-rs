//! Validation report types: [`ValidationSeverity`], [`ValidationIssue`],
//! [`ValidationReport`], and the classification and location types an issue
//! carries — [`FindingKind`], [`GroupPath`] and [`GroupStep`].
//!
//! These types are also re-exported from the crate root.

use std::borrow::Cow;
use std::sync::Arc;

use crate::model::Span;
use crate::validator::ValidationLayer;

/// The severity `edifact-rs` assigns to an [`EdifactError`][crate::EdifactError].
///
/// This is the **single** definition of that mapping. It backs
/// [`ValidationReport`] entries and, with the `diagnostics` feature, the
/// `miette::Diagnostic` impl on the error itself — two renderings of one
/// interchange must not disagree about whether it is broken.
///
/// | Variant | Severity | Why |
/// |---|---|---|
/// | [`InvalidCodeValue`][crate::EdifactError::InvalidCodeValue] | `Warning` | Non-standard extension codes are common and often intentional. |
/// | [`BlankDataElementValue`][crate::EdifactError::BlankDataElementValue] | `Warning` | Violates §9.3, but the value is still readable. |
/// | [`TrailingSeparator`][crate::EdifactError::TrailingSeparator] | `Warning` | Violates §8.7.1/§8.7.2; a correct parser reads it anyway. |
/// | [`InsignificantCharacters`][crate::EdifactError::InsignificantCharacters] | `Warning` | Violates §9.1; the value is still readable. |
/// | everything else | `Error` | Structural violations, control-reference mismatches, parse faults. |
///
/// [`QualifierMismatch`][crate::EdifactError::QualifierMismatch] is deliberately
/// *not* a warning: it is only ever raised for `UNZ`/`UNE`/`UNT` control-reference
/// mismatches, which are hard ISO 9735-1 violations. Downgrading it let a spliced
/// or truncated interchange pass `validate(..).result()`.
#[must_use]
pub fn severity_for_error(error: &crate::EdifactError) -> ValidationSeverity {
    use crate::EdifactError as E;
    match error {
        E::InvalidCodeValue { .. }
        | E::BlankDataElementValue { .. }
        | E::TrailingSeparator { .. }
        | E::InsignificantCharacters { .. } => ValidationSeverity::Warning,
        _ => ValidationSeverity::Error,
    }
}

// ── ValidationSeverity ────────────────────────────────────────────────────────

/// Priority level for a validation error or warning.
///
/// `#[non_exhaustive]`: severity levels may be added without a breaking
/// change, so downstream `match`es need a wildcard arm.
///
/// # Ordering
///
/// Comparison follows *severity*, not declaration order:
/// `Info < Warning < Error < Critical`.  The derived ordering said the opposite
/// — it ranked `Critical` lowest, so `issues.max_by_key(|i| i.severity)` picked
/// the least important issue — while [`numeric_level`][Self::numeric_level] said
/// the reverse.  One of the two had to give, and the one that matches the word
/// "severity" wins.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ValidationSeverity {
    /// Structural parse failure; processing cannot continue.
    Critical,
    /// Structural validation failed; message is invalid.
    Error,
    /// Data validation warning (e.g., code-list mismatch); message may be usable.
    Warning,
    /// Informational note; message is valid but noteworthy.
    Info,
}

impl ValidationSeverity {
    /// Return a lowercase ASCII string for this severity level.
    ///
    /// Stable for the four variants.  Because the enum is
    /// `#[non_exhaustive]`, a catch-all arm returns `"unknown"` for any
    /// variant added later.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            #[allow(unreachable_patterns)]
            _ => "unknown",
        }
    }

    /// Return a numeric priority for this severity level.
    ///
    /// Higher values indicate higher severity: `Critical = 3`, `Error = 2`,
    /// `Warning = 1`, `Info = 0`.  This is also the basis of the [`Ord`] impl.
    ///
    /// Because the enum is `#[non_exhaustive]`, any other variant ranks below
    /// `Info`.
    #[must_use]
    pub fn numeric_level(self) -> u8 {
        match self {
            Self::Info => 0,
            Self::Warning => 1,
            Self::Error => 2,
            Self::Critical => 3,
            #[allow(unreachable_patterns)]
            _ => 0,
        }
    }
}

impl Ord for ValidationSeverity {
    /// Orders by [`numeric_level`][Self::numeric_level]: `Info` is least, `Critical` greatest.
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.numeric_level().cmp(&other.numeric_level())
    }
}

impl PartialOrd for ValidationSeverity {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for ValidationSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── FindingKind ───────────────────────────────────────────────────────────────

/// What kind of fault a [`ValidationIssue`] reports, independent of the layer
/// that found it and of its free-text rule identifier.
///
/// A consumer routes on this — "missing" versus "not permitted" versus "wrong
/// code" — without parsing [`rule_id`](ValidationIssue::rule_id) suffixes.
/// Every built-in validator sets it; an issue a caller builds without one keeps
/// [`kind`](ValidationIssue::kind) `None`.
///
/// The families follow the ISO 9735-4 Annex A syntax error codes (DE 0085)
/// without being them: CONTRL vocabulary has no "not permitted by the guide"
/// and no "resource limit", and a kind is meant for every layer, not only for
/// the ones CONTRL can report.
///
/// | Variant | [`as_str`](Self::as_str) | Meaning |
/// |---|---|---|
/// | `Missing` | `missing` | a required segment, group, element or component is absent; minimum occurrences not reached |
/// | `NotPermitted` | `not_permitted` | present where the directory, guide or pack forbids it |
/// | `Code` | `code` | value not in the admitted code list, or the wrong qualifier |
/// | `Representation` | `representation` | character type, length, blank value, insignificant characters, non-finite number, typed field value |
/// | `Count` | `count` | too many occurrences, repetitions, elements or components |
/// | `Placement` | `placement` | the segment fits no place here, or is out of order |
/// | `ControlMismatch` | `control_mismatch` | a trailer count or control reference does not match its header |
/// | `Duplicate` | `duplicate` | a control reference is used twice |
/// | `Envelope` | `envelope` | envelope structure: empty interchange or message, groups and messages mixed |
/// | `Charset` | `charset` | character not in the repertoire, undecodable text, unknown or unsupported repertoire |
/// | `Syntax` | `syntax` | an ISO 9735-1 lexical or syntax fault (delimiter, tag, `UNA`, release character, end of input, trailing separator, segment without data) |
/// | `Limit` | `limit` | a configured resource limit tripped |
/// | `Unsupported` | `unsupported` | a standard feature this crate refuses (packages, …) |
/// | `Other` | `other` | anything else |
///
/// Marked `#[non_exhaustive]`: match with a wildcard arm. With the `serde`
/// feature, variants serialise by name (`"NotPermitted"`, the casing
/// [`ValidationSeverity`] uses), and a name this build does not know — written
/// by a newer release — reads back as [`Other`](Self::Other).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FindingKind {
    /// A required segment, group, element or component is absent.
    Missing,
    /// Present where the directory, guide or pack forbids it.
    NotPermitted,
    /// Value not in the admitted code list, or the wrong qualifier.
    Code,
    /// The value's representation is wrong: character type, length, blank,
    /// insignificant characters, non-finite number, typed field value.
    Representation,
    /// Too many occurrences, repetitions, elements or components.
    Count,
    /// The segment fits no place here, or is out of order.
    Placement,
    /// A trailer count or control reference does not match its header.
    ControlMismatch,
    /// A control reference is used twice.
    Duplicate,
    /// Envelope structure: empty interchange or message, groups and messages mixed.
    Envelope,
    /// Character not in the repertoire, undecodable text, unknown or
    /// unsupported repertoire.
    Charset,
    /// An ISO 9735-1 lexical or syntax fault.
    Syntax,
    /// A configured resource limit tripped.
    Limit,
    /// A standard feature this crate refuses (packages, …).
    Unsupported,
    /// Anything else, and any kind name this build does not know.
    #[cfg_attr(feature = "serde", serde(other))]
    Other,
}

impl FindingKind {
    /// A stable lowercase identifier, e.g. `"not_permitted"`.
    ///
    /// Stable across releases for every existing variant; used by
    /// [`ValidationReport::render_deterministic`] and the `miette` labels.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::NotPermitted => "not_permitted",
            Self::Code => "code",
            Self::Representation => "representation",
            Self::Count => "count",
            Self::Placement => "placement",
            Self::ControlMismatch => "control_mismatch",
            Self::Duplicate => "duplicate",
            Self::Envelope => "envelope",
            Self::Charset => "charset",
            Self::Syntax => "syntax",
            Self::Limit => "limit",
            Self::Unsupported => "unsupported",
            Self::Other => "other",
        }
    }

    /// The kind of fault an [`EdifactError`][crate::EdifactError] reports.
    ///
    /// This is the **single** mapping from errors to kinds, the counterpart of
    /// [`severity_for_error`]: every issue the built-in validators derive from
    /// an error carries exactly this kind. It is total — written without a
    /// wildcard arm, so a new error variant cannot compile until it is
    /// classified.
    ///
    /// | Code | Kind |
    /// |---|---|
    /// | `E001` `E002` `E006` `E007` `E019` `E028` `E037` `E046` `E051` | `Syntax` |
    /// | `E003` `E009` `E031` `E038` `E039` `E041` | `Charset` |
    /// | `E004` `E005` `E016` | `ControlMismatch` |
    /// | `E008` `E015` `E017` `E021` | `Missing` |
    /// | `E011` `E068` | `Placement` |
    /// | `E012` `E013` `E047` `E069` `E070` | `Count` |
    /// | `E014` | `Code` |
    /// | `E020` `E023` `E036` | `Limit` |
    /// | `E027` `E040` `E045` `E048` `E049` `E050` `E053` | `Representation` |
    /// | `E032` | `Duplicate` |
    /// | `E042` `E043` `E052` | `Envelope` |
    /// | `E044` | `Unsupported` |
    /// | `E010` `E024` `E025` `E026` `E030` `E033` `E034` `E035` `E066` `E067` `E071` | `Other` |
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{EdifactError, FindingKind};
    ///
    /// let err = EdifactError::MissingRequiredElement { tag: "BGM".into(), element_index: 1 };
    /// assert_eq!(FindingKind::for_error(&err), FindingKind::Missing);
    /// ```
    #[must_use]
    pub fn for_error(error: &crate::EdifactError) -> FindingKind {
        use crate::EdifactError as E;
        match error {
            E::UnexpectedEof { .. }
            | E::InvalidDelimiter { .. }
            | E::InvalidSegmentTag(_)
            | E::InvalidUna
            | E::InvalidReleaseSequence { .. }
            | E::UnexpectedDataToken { .. }
            | E::RepetitionSeparatorNotDeclared
            | E::SegmentWithoutDataElements { .. }
            | E::TrailingSeparator { .. } => Self::Syntax,
            E::InvalidText { .. }
            | E::InvalidUtf8
            | E::UnrecognisedSyntaxIdentifier(_)
            | E::CharacterNotInRepertoire { .. }
            | E::UnsupportedCharset { .. }
            | E::CharacterRepertoireMismatch { .. } => Self::Charset,
            E::MessageCountMismatch { .. }
            | E::SegmentCountMismatch { .. }
            | E::QualifierMismatch { .. } => Self::ControlMismatch,
            E::MissingRequiredElement { .. }
            | E::MissingSegment { .. }
            | E::ConditionalRequirementNotMet { .. }
            | E::MissingRequiredComponent { .. } => Self::Missing,
            E::InvalidSegmentForMessage { .. } | E::SegmentNotPlaced { .. } => Self::Placement,
            E::InvalidElementCount { .. }
            | E::InvalidComponentCount { .. }
            | E::TooManyRepetitions { .. }
            | E::TooManySegmentOccurrences { .. }
            | E::TooManyGroupOccurrences { .. } => Self::Count,
            E::InvalidCodeValue { .. } => Self::Code,
            E::SegmentTooLong { .. } | E::InterchangeTooLarge { .. } | E::LimitExceeded { .. } => {
                Self::Limit
            }
            E::InvalidFieldValue { .. }
            | E::NonFiniteNumber { .. }
            | E::BlankDataElementValue { .. }
            | E::InvalidCharacterType { .. }
            | E::DataElementTooLong { .. }
            | E::DataElementTooShort { .. }
            | E::InsignificantCharacters { .. } => Self::Representation,
            E::DuplicateReference { .. } => Self::Duplicate,
            E::EmptyInterchange { .. }
            | E::EmptyMessage { .. }
            | E::GroupsAndMessagesMixed { .. } => Self::Envelope,
            E::PackageNotSupported { .. } => Self::Unsupported,
            E::Io(_)
            | E::InvalidEventSequence { .. }
            | E::InvalidElementPosition
            | E::IncompatibleReleaseScopes { .. }
            | E::ValidationErrors { .. }
            | E::UnknownDataElement { .. }
            | E::AmbiguousDataElement { .. }
            | E::SegmentLayoutMismatch { .. }
            | E::ActionNotPermitted { .. }
            | E::InvalidContrlIdentifier { .. }
            | E::InvalidGroupSchema { .. } => Self::Other,
        }
    }
}

impl std::fmt::Display for FindingKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── GroupPath ─────────────────────────────────────────────────────────────────

/// One step of a [`GroupPath`]: a segment group name and which occurrence of
/// that group it is.
///
/// `occurrence` is zero-based and counted **per name among the children of one
/// parent instance** — the same counter as
/// [`SegmentGroupIndexed::occurrence_index`][crate::SegmentGroupIndexed::occurrence_index].
/// So the second `SG8` under the first `SG4` is `SG8[1]`, whatever other groups
/// sit between them, and two places in a guide that share one group name count
/// together.
///
/// With the `serde` feature a step serialises as
/// `{"group": "SG8", "occurrence": 1}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GroupStep {
    /// Group name as the grouping schema calls it, e.g. `"SG8"`.
    #[cfg_attr(feature = "serde", serde(rename = "group"))]
    pub name: Arc<str>,
    /// Zero-based occurrence among same-named siblings.
    pub occurrence: u32,
}

impl GroupStep {
    /// A step for occurrence `occurrence` (zero-based) of group `name`.
    ///
    /// Takes the `usize` a caller holds — a loop index or
    /// [`SegmentGroupIndexed::occurrence_index`][crate::SegmentGroupIndexed::occurrence_index]
    /// — so no caller casts. Returns `None` when the occurrence does not fit
    /// `u32`; it is never clamped, because a clamped occurrence names a
    /// different, plausible group instance. Parser output cannot reach that
    /// bound: an interchange with more than `u32::MAX` segments is refused
    /// with `E023`.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, occurrence: usize) -> Option<Self> {
        Some(Self {
            name: name.into(),
            occurrence: u32::try_from(occurrence).ok()?,
        })
    }
}

impl std::fmt::Display for GroupStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}[{}]", self.name, self.occurrence)
    }
}

/// The occurrence path from the outermost segment group to the group instance
/// that holds a finding, e.g. `SG4[0]/SG8[1]`.
///
/// A group name alone does not locate anything in a message where the same
/// group repeats thousands of times; the path does. It is immutable and holds
/// its steps behind one [`Arc`], so the many issues found in one group
/// instance share a single allocation and cloning is a reference-count bump.
///
/// A path always has at least one step. The message root has no path:
/// [`ValidationIssue::group_path`] is then `None`, never an empty path.
///
/// [`Display`](std::fmt::Display) joins the steps with `/`, each as
/// `name[occurrence]`. That form is for people: a group name containing `/` or
/// `[` renders ambiguously. With the `serde` feature the path serialises as a
/// sequence of [`GroupStep`] objects, which loses nothing.
///
/// # Example
///
/// ```
/// use edifact_rs::{GroupPath, GroupStep};
///
/// let sg4 = GroupPath::new(GroupStep::new("SG4", 0)).unwrap();
/// let sg8 = sg4.child("SG8", 1).unwrap();
/// assert_eq!(sg8.to_string(), "SG4[0]/SG8[1]");
/// assert_eq!(sg8.depth(), 2);
/// assert_eq!(&*sg8.last().name, "SG8");
/// assert!(GroupPath::new(Vec::new()).is_none()); // the root has no path
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupPath(Arc<[GroupStep]>);

impl GroupPath {
    /// A path from its steps, outermost first.
    ///
    /// Returns `None` for zero steps: the message root is expressed by having
    /// no path at all.
    #[must_use]
    pub fn new(steps: impl IntoIterator<Item = GroupStep>) -> Option<Self> {
        let steps: Arc<[GroupStep]> = steps.into_iter().collect();
        (!steps.is_empty()).then_some(Self(steps))
    }

    /// The path one level deeper: this path followed by occurrence
    /// `occurrence` of group `name`.
    ///
    /// Allocates one new step list; `self` is unchanged. Returns `None` when
    /// `occurrence` does not fit `u32` (see [`GroupStep::new`]).
    #[must_use]
    pub fn child(&self, name: impl Into<Arc<str>>, occurrence: usize) -> Option<Self> {
        let step = GroupStep::new(name, occurrence)?;
        Some(Self(self.0.iter().cloned().chain([step]).collect()))
    }

    /// The steps, outermost first. Never empty.
    #[must_use]
    pub fn steps(&self) -> &[GroupStep] {
        &self.0
    }

    /// The innermost step — the group instance that holds the finding.
    #[must_use]
    pub fn last(&self) -> &GroupStep {
        // The constructors refuse an empty step list.
        &self.0[self.0.len() - 1]
    }

    /// Number of steps (nesting depth). Always at least 1.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.0.len()
    }
}

impl std::fmt::Display for GroupPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, step) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            std::fmt::Display::fmt(step, f)?;
        }
        Ok(())
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for GroupPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&*self.0, serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for GroupPath {
    /// Reads a sequence of steps; an empty sequence is refused, because a path
    /// is never empty.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let steps = <Vec<GroupStep> as serde::Deserialize>::deserialize(deserializer)?;
        GroupPath::new(steps)
            .ok_or_else(|| serde::de::Error::custom("a group path has at least one step"))
    }
}

// ── ValidationIssue ───────────────────────────────────────────────────────────

/// A structured validation issue.
///
/// `#[non_exhaustive]`: fields may be added without a breaking change, so build
/// one with [`ValidationIssue::new`] and the `with_*` methods rather than a
/// struct literal.
///
/// ## What, where and who
///
/// An issue answers three questions with structured fields, so a consumer never
/// has to parse [`rule_id`](Self::rule_id) or [`message`](Self::message):
///
/// | Question | Fields |
/// |---|---|
/// | What kind of fault? | [`kind`](Self::kind) ([`FindingKind`]), [`error_code`](Self::error_code), [`severity`](Self::severity) |
/// | Which stage and which pack found it? | [`layer`](Self::layer) ([`ValidationLayer`]), [`pack`](Self::pack) |
/// | Where? | [`span`](Self::span) (bytes), [`segment_position`](Self::segment_position) (DE 0096), [`segment_occurrence`](Self::segment_occurrence), [`element_index`](Self::element_index) / [`component_index`](Self::component_index) (DE 0098 / DE 0104), [`group_path`](Self::group_path), [`segment_tag`](Self::segment_tag) |
///
/// Positional builders take the `usize` a caller already holds and never
/// clamp: a value that does not fit the field leaves it `None`, because a
/// clamped index is a plausible *wrong* location.
///
/// ```rust
/// # use edifact_rs::{FindingKind, ValidationIssue, ValidationLayer, ValidationSeverity};
/// let segment_index = 70_002_usize; // zero-based index into the message slice
/// let issue = ValidationIssue::new(ValidationSeverity::Error, "QTY qualifier must be 220")
///     .with_segment("QTY")
///     .with_segment_position(segment_index + 1) // UNH = 1
///     .with_segment_occurrence(69_999)          // the 70 000th QTY
///     .with_element_index(0)
///     .with_layer(ValidationLayer::Profile)
///     .with_kind(FindingKind::Code)
///     .with_pack("MSCONS-MIG");
/// assert_eq!(issue.segment_position(), Some(70_003));
/// assert_eq!(issue.kind(), Some(FindingKind::Code));
/// assert_eq!(issue.with_element_index(70_000).element_index(), None); // never clamped
/// ```
///
/// ## Rule ID naming convention
///
/// [`rule_id`](Self::rule_id) is an identifier: it names the rule so a report
/// can be filtered and a rule overridden. It is not where layer, kind or pack
/// live — read [`layer`](Self::layer), [`kind`](Self::kind) and
/// [`pack`](Self::pack) for those.  A namespaced id still helps people reading
/// a report:
///
/// ```text
/// "<PACK>-<SCOPE>-<TAG>-<STATUS>"
///  ^^^^^^                          — the pack / profile that owns the rule
///         ^^^^^^^                  — a process identifier, group name, or other discriminator
///                 ^^^^^            — the affected segment
///                       ^^^^^^^^   — M / C / … status or short discriminator
/// ```
///
/// Example: `"PROFILE-4711-BGM-M"` names the pack `PROFILE`, the scope `4711`,
/// the affected segment `BGM`, and the mandatory status `M`.  Domain detail with
/// no structured field, such as the scope, can be recovered with a plain string
/// split:
///
/// ```rust
/// let rule_id = "PROFILE-4711-BGM-M";
/// let scope = rule_id.strip_prefix("PROFILE-").and_then(|s| s.split('-').next());
/// assert_eq!(scope, Some("4711"));
/// ```
///
/// For truly arbitrary domain metadata, use the [`context`](Self::context) map and
/// `with_context_entry`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ValidationIssue {
    /// Stable error code, if known.
    ///
    /// Library-produced codes are `&'static str` constants (`"E014"`, …), so the
    /// [`Cow`] borrows and costs nothing to construct.  Callers may also supply
    /// an owned code from an external rule catalogue.  Either way the field
    /// **round-trips through serialization**: a report persisted to an audit
    /// store and read back can still be filtered and routed on `error_code`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub error_code: Option<Cow<'static, str>>,
    /// The severity of this issue.
    pub severity: ValidationSeverity,
    /// The error or warning message.
    pub message: String,
    /// Half-open byte range of the relevant segment, element, or component.
    ///
    /// The single source of byte position for an issue — read `span.start` when
    /// only the start offset is needed.  Set it with [`with_span`](Self::with_span)
    /// from a [`Span`] carried by a parsed [`crate::Segment`], `Element`, or
    /// component, which is what gives `miette` diagnostics and Language Server
    /// Protocol `Range` values their precision.
    ///
    /// Issues derived from a purely lexical fault (a dangling release character,
    /// unexpected end of input) carry a zero-width span at the offending byte:
    /// there is no meaningful end position for a point diagnostic.
    pub span: Option<Span>,
    /// Segment tag involved (if known).
    pub segment_tag: Option<String>,
    /// Profile/MIG rule identifier, if applicable.
    ///
    /// An identifier only: it names the rule for filtering
    /// ([`ValidationReport::filter_by_rule_id`]) and overriding. Layer, kind and
    /// producing pack have their own fields — [`layer`](Self::layer),
    /// [`kind`](Self::kind), [`pack`](Self::pack) — and are not encoded here.
    /// See the [`ValidationIssue`] type-level docs for a naming convention.
    pub rule_id: Option<String>,
    /// Zero-based data element index within the segment, if known.
    ///
    /// The segment tag is not counted, so ISO 9735-4 DE 0098 (data element
    /// position, tag = 1) is `element_index + 2`; that is what
    /// [`Contrl::from_report`][crate::Contrl::from_report] writes. `u16` holds
    /// every index a segment can have: an index cannot exceed the segment's
    /// byte length, which the default 65 536-byte segment limit keeps within
    /// `u16`, and DE 0098 itself is `n..3`. Set with
    /// [`with_element_index`](Self::with_element_index), which leaves the field
    /// `None` rather than clamp a value that does not fit.
    pub element_index: Option<u16>,
    /// Zero-based component index within the composite, if known.
    ///
    /// ISO 9735-4 DE 0104 (component data element position, first = 1) is
    /// `component_index + 1`. Widths and overflow as for
    /// [`element_index`](Self::element_index).
    pub component_index: Option<u16>,
    /// Zero-based occurrence among segments **with the same tag** in the
    /// message, if known.
    ///
    /// When several segments share a tag (repeated `DTM` lines), this says which
    /// one (0 = first). It is not the segment's position in the message — that
    /// is [`segment_position`](Self::segment_position). `u32`: a message can
    /// hold more than 65 535 segments of one tag (large `MSCONS`), and the
    /// parser refuses more than `u32::MAX` segments with `E023`.
    pub segment_occurrence: Option<u32>,
    /// One-based position of the segment within its message, `UNH` = 1, if known.
    ///
    /// This is ISO 9735-4 DE 0096 (segment position in message body) — the
    /// coordinate a CONTRL `UCS` reports. The built-in validators that walk
    /// segments themselves set it when their input slice starts at `UNH`, and
    /// leave it `None` otherwise rather than guess. Findings converted from an
    /// error that carries no segment index keep relying on [`span`](Self::span),
    /// from which CONTRL derives the same coordinate.
    ///
    /// Together with [`element_index`](Self::element_index) and
    /// [`component_index`](Self::component_index) it is the issue's whole
    /// CONTRL location; a producer that maps its own faults to a location
    /// fills these three fields and does not add others.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub segment_position: Option<u32>,
    /// Message reference (`UNH` element 0, DE 0062) that this issue belongs to.
    ///
    /// Populated automatically when the context was built with
    /// `ValidationContextBuilder::with_message_ref`.  Useful in batch processing
    /// where many messages are validated and issues from different messages must
    /// be correlated back to the originating `UNH`/`UNT` envelope.
    pub message_ref: Option<String>,
    /// Suggested remediation (if available).
    pub suggestion: Option<String>,
    /// Segment group (e.g. `"SG6"`) in which the issue occurred, if known.
    ///
    /// Populated by group-aware rule functions when they evaluate sub-slices of a
    /// [`crate::group::SegmentGroupIndexed`] tree.  `None` for flat-segment rules
    /// that do not have group context.  It names only the innermost group; see
    /// [`group_path`](Self::group_path) for which occurrence of it.
    pub segment_group: Option<Arc<str>>,
    /// Occurrence path to the group instance holding the finding, e.g.
    /// `SG4[0]/SG8[1]`, if known.
    ///
    /// `None` at the message root — never an empty path. The profile group pass
    /// stamps it on every issue a group rule emits without one. Setting it with
    /// [`with_group_path`](Self::with_group_path) also fills an empty
    /// [`segment_group`](Self::segment_group).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub group_path: Option<GroupPath>,
    /// Validation stage that produced the issue, if known.
    ///
    /// Built-in validators set it themselves. [`ValidationContext`][crate::ValidationContext]
    /// fills it with the layer a validator was registered under for every issue
    /// that validator left without one; an explicit layer is kept. An issue
    /// built outside a context, or added with
    /// [`with_static_issue`][crate::ValidationContextBuilder::with_static_issue],
    /// keeps whatever its builder set.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub layer: Option<ValidationLayer>,
    /// What kind of fault this is, if known.
    ///
    /// Every built-in validator sets it — issues derived from an
    /// [`EdifactError`][crate::EdifactError] use [`FindingKind::for_error`].
    /// An issue a caller builds without one stays `None`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub kind: Option<FindingKind>,
    /// Name of the [`ProfileRulePack`][crate::ProfileRulePack] that produced
    /// the issue, if any.
    ///
    /// A pack stamps its name on every issue it emits without one, so a
    /// consumer running several packs (a guide pack, an application pack)
    /// tells their findings apart without a rule-id prefix.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub pack: Option<Arc<str>>,
    /// Arbitrary domain-specific key-value metadata attached to this issue.
    ///
    /// Use this for information that does not fit into the structured fields above
    /// — for example the process identifier a downstream profile crate is validating against, a
    /// trading-partner identifier, or a document UUID:
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity};
    /// let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
    ///     .with_rule_id("PROFILE-4711-BGM-M")
    ///     .with_context_entry("pid", "4711")
    ///     .with_context_entry("partner", "9900123456789");
    /// assert_eq!(issue.context_get("pid"), Some("4711"));
    /// ```
    ///
    /// The vec is empty by default and is never populated by the built-in rules;
    /// it is reserved exclusively for caller-supplied metadata.
    ///
    /// Entries are stored in insertion order; duplicate keys are allowed and
    /// [`context_get`](Self::context_get) returns the first match.
    /// [`with_context_entry`](Self::with_context_entry) uses upsert semantics
    /// (updates an existing key in place rather than duplicating it).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub context: Vec<(String, String)>,
}

impl ValidationIssue {
    /// Create a new validation issue.
    pub fn new(severity: ValidationSeverity, message: impl Into<String>) -> Self {
        Self {
            error_code: None,
            severity,
            message: message.into(),
            span: None,
            segment_tag: None,
            rule_id: None,
            element_index: None,
            component_index: None,
            segment_occurrence: None,
            segment_position: None,
            message_ref: None,
            suggestion: None,
            segment_group: None,
            group_path: None,
            layer: None,
            kind: None,
            pack: None,
            context: Vec::new(),
        }
    }

    /// Set stable error code metadata.
    ///
    /// Accepts both a `&'static str` library constant (no allocation) and an
    /// owned `String` from an external rule catalogue.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity};
    /// let from_const = ValidationIssue::new(ValidationSeverity::Error, "bad code")
    ///     .with_error_code("E014");
    /// let from_owned = ValidationIssue::new(ValidationSeverity::Error, "bad code")
    ///     .with_error_code(format!("PROFILE-{}", 4711));
    /// assert_eq!(from_const.error_code(), Some("E014"));
    /// assert_eq!(from_owned.error_code(), Some("PROFILE-4711"));
    /// ```
    pub fn with_error_code(mut self, code: impl Into<Cow<'static, str>>) -> Self {
        self.error_code = Some(code.into());
        self
    }

    /// Set the byte-range span for this issue.
    ///
    /// [`span`](Self::span) is the only positional field on a
    /// `ValidationIssue`; read `issue.span.map(|s| s.start)` when you need the
    /// start offset alone.  Prefer the narrowest span you have — a component
    /// span over an element span over a segment span — since that is what
    /// diagnostics underline.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity, Span};
    /// let span = Span::new(42, 57);
    /// let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code missing")
    ///     .with_span(span);
    /// assert_eq!(issue.span, Some(span));
    /// assert_eq!(issue.span.map(|s| s.start), Some(42));
    /// ```
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Set the segment tag for this issue.
    pub fn with_segment(mut self, tag: impl Into<String>) -> Self {
        self.segment_tag = Some(tag.into());
        self
    }

    /// Set the profile/MIG rule identifier for this issue.
    pub fn with_rule_id(mut self, rule_id: impl Into<String>) -> Self {
        self.rule_id = Some(rule_id.into());
        self
    }

    /// Set the zero-based data element index (DE 0098 − 2) for this issue.
    ///
    /// Takes the `usize` a caller holds. A value above `u16::MAX` leaves
    /// [`element_index`](Self::element_index) `None` — it is never clamped.
    pub fn with_element_index(mut self, element_index: usize) -> Self {
        self.element_index = u16::try_from(element_index).ok();
        self
    }

    /// Set the zero-based component index (DE 0104 − 1) for this issue.
    ///
    /// A value above `u16::MAX` leaves
    /// [`component_index`](Self::component_index) `None` — it is never clamped.
    pub fn with_component_index(mut self, component_index: usize) -> Self {
        self.component_index = u16::try_from(component_index).ok();
        self
    }

    /// Set a suggestion for resolving this issue.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Set the zero-based occurrence among same-tag segments for this issue.
    ///
    /// Use this when the same segment tag appears multiple times in a message
    /// and you want to identify which occurrence is affected.  For the
    /// segment's position in the message, use
    /// [`with_segment_position`](Self::with_segment_position) instead.  A
    /// value above `u32::MAX` leaves the field `None` — it is never clamped.
    pub fn with_segment_occurrence(mut self, occurrence: usize) -> Self {
        self.segment_occurrence = u32::try_from(occurrence).ok();
        self
    }

    /// Set the one-based position of the segment in its message, `UNH` = 1
    /// (ISO 9735-4 DE 0096).
    ///
    /// For a zero-based index `i` into a slice that starts at `UNH`, pass
    /// `i + 1`.  A value above `u32::MAX` leaves the field `None` — it is never
    /// clamped.
    pub fn with_segment_position(mut self, position: usize) -> Self {
        self.segment_position = u32::try_from(position).ok();
        self
    }

    /// Set the occurrence path to the group instance holding this issue.
    ///
    /// Also sets [`segment_group`](Self::segment_group) to the path's innermost
    /// group name when it is not already set.
    pub fn with_group_path(mut self, path: GroupPath) -> Self {
        if self.segment_group.is_none() {
            self.segment_group = Some(Arc::clone(&path.last().name));
        }
        self.group_path = Some(path);
        self
    }

    /// Set the validation stage that produced this issue.
    pub fn with_layer(mut self, layer: ValidationLayer) -> Self {
        self.layer = Some(layer);
        self
    }

    /// Set the kind of fault this issue reports.
    pub fn with_kind(mut self, kind: FindingKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Set the name of the pack that produced this issue.
    pub fn with_pack(mut self, pack: impl Into<Arc<str>>) -> Self {
        self.pack = Some(pack.into());
        self
    }

    /// Set the message reference (`UNH` element 0) for this issue.
    ///
    /// Use this to correlate an issue back to a specific message in a
    /// multi-message interchange.
    pub fn with_message_ref(mut self, message_ref: impl Into<String>) -> Self {
        self.message_ref = Some(message_ref.into());
        self
    }

    /// Set the segment group (e.g. `"SG6"`) in which this issue occurred.
    ///
    /// Use this from group-aware rule functions that evaluate a sub-slice of a
    /// [`crate::group::SegmentGroupIndexed`] tree so that consumers can identify
    /// the exact group occurrence without re-reading the raw message.
    pub fn with_segment_group(mut self, group: impl Into<Arc<str>>) -> Self {
        self.segment_group = Some(group.into());
        self
    }

    /// Insert a single key-value entry into the domain-specific [`context`](Self::context) map.
    ///
    /// Calling this multiple times accumulates entries; duplicate keys overwrite
    /// the previous value.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity};
    /// let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
    ///     .with_rule_id("PROFILE-4711-BGM-M")
    ///     .with_context_entry("pid", "4711")
    ///     .with_context_entry("partner", "9900123456789");
    ///
    /// assert_eq!(issue.context_get("pid"), Some("4711"));
    /// assert_eq!(issue.context_get("partner"), Some("9900123456789"));
    /// ```
    pub fn with_context_entry(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        let value = value.into();
        if let Some(entry) = self.context.iter_mut().find(|(k, _)| k == &key) {
            entry.1 = value;
        } else {
            self.context.push((key, value));
        }
        self
    }

    /// Extend the domain-specific [`context`](Self::context) map from an iterator of
    /// `(key, value)` pairs.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use edifact_rs::{ValidationIssue, ValidationSeverity};
    /// let meta = [("pid", "4711"), ("partner", "9900123456789")];
    /// let issue = ValidationIssue::new(ValidationSeverity::Error, "test")
    ///     .with_context_entries(meta);
    ///
    /// assert_eq!(issue.context_get("pid"), Some("4711"));
    /// ```
    pub fn with_context_entries<K, V, I>(mut self, entries: I) -> Self
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
    {
        for (k, v) in entries {
            let k = k.into();
            let v = v.into();
            if let Some(entry) = self.context.iter_mut().find(|(key, _)| key == &k) {
                entry.1 = v;
            } else {
                self.context.push((k, v));
            }
        }
        self
    }

    /// Look up a value in the domain-specific [`context`](Self::context) map.
    #[must_use]
    #[inline]
    pub fn context_get(&self, key: &str) -> Option<&str> {
        self.context
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Short label for the severity level, suitable for display.
    #[must_use]
    pub fn severity_label(&self) -> &'static str {
        match self.severity {
            ValidationSeverity::Critical => "CRITICAL",
            ValidationSeverity::Error => "ERROR",
            ValidationSeverity::Warning => "WARNING",
            ValidationSeverity::Info => "INFO",
            #[allow(unreachable_patterns)]
            _ => "UNKNOWN",
        }
    }

    // ── Getters ───────────────────────────────────────────────────────────────

    /// Stable error code, if available.
    #[must_use]
    #[inline]
    pub fn error_code(&self) -> Option<&str> {
        self.error_code.as_deref()
    }

    /// Half-open byte range of the relevant source region, if available.
    #[must_use]
    #[inline]
    pub fn span(&self) -> Option<Span> {
        self.span
    }

    /// Start byte offset of [`span`](Self::span), if available.
    ///
    /// Convenience for consumers that only need a position, not a range.
    #[must_use]
    #[inline]
    pub fn start_offset(&self) -> Option<usize> {
        self.span.map(|s| s.start)
    }

    /// Segment tag involved in this issue, if known.
    #[must_use]
    #[inline]
    pub fn segment_tag(&self) -> Option<&str> {
        self.segment_tag.as_deref()
    }

    /// Profile/MIG rule identifier, if applicable.
    #[must_use]
    #[inline]
    pub fn rule_id(&self) -> Option<&str> {
        self.rule_id.as_deref()
    }

    /// Zero-based element index, if known.
    #[must_use]
    #[inline]
    pub fn element_index(&self) -> Option<u16> {
        self.element_index
    }

    /// Zero-based component index, if known.
    #[must_use]
    #[inline]
    pub fn component_index(&self) -> Option<u16> {
        self.component_index
    }

    /// Zero-based occurrence index among same-tag segments, if known.
    #[must_use]
    #[inline]
    pub fn segment_occurrence(&self) -> Option<u32> {
        self.segment_occurrence
    }

    /// One-based position of the segment in its message (`UNH` = 1, DE 0096), if known.
    #[must_use]
    #[inline]
    pub fn segment_position(&self) -> Option<u32> {
        self.segment_position
    }

    /// Occurrence path to the group instance holding the issue, if known.
    #[must_use]
    #[inline]
    pub fn group_path(&self) -> Option<&GroupPath> {
        self.group_path.as_ref()
    }

    /// Validation stage that produced the issue, if known.
    #[must_use]
    #[inline]
    pub fn layer(&self) -> Option<ValidationLayer> {
        self.layer
    }

    /// Kind of fault, if known.
    #[must_use]
    #[inline]
    pub fn kind(&self) -> Option<FindingKind> {
        self.kind
    }

    /// Name of the pack that produced the issue, if any.
    #[must_use]
    #[inline]
    pub fn pack(&self) -> Option<&str> {
        self.pack.as_deref()
    }

    /// Message reference (`UNH` element 0), if set.
    #[must_use]
    #[inline]
    pub fn message_ref(&self) -> Option<&str> {
        self.message_ref.as_deref()
    }

    /// Suggested remediation, if available.
    #[must_use]
    #[inline]
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }

    /// Segment group (e.g. `"SG6"`) in which the issue occurred, if known.
    #[must_use]
    #[inline]
    pub fn segment_group(&self) -> Option<&str> {
        self.segment_group.as_deref()
    }
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.severity_label(), self.message)
    }
}

impl std::error::Error for ValidationIssue {}

// ── ValidationReport ─────────────────────────────────────────────────────────

/// A collection of validation results: errors, warnings, and informational notes.
///
/// Enables batch validation where all issues are collected instead of failing on
/// the first error.  Produced by
/// [`ValidationContext::validate`][crate::ValidationContext::validate] and
/// [`ValidationContext::validate_grouped`][crate::ValidationContext::validate_grouped].
///
/// # Building reports manually
///
/// Use [`ValidationReport::from_issues`] to construct a report from pre-built issue
/// vectors, or the `add_*` methods to push individual issues:
///
/// ```rust
/// use edifact_rs::{ValidationReport, ValidationIssue, ValidationSeverity};
///
/// let mut report = ValidationReport::default();
/// report.add_warning(
///     ValidationIssue::new(ValidationSeverity::Warning, "optional field missing")
///         .with_segment("DTM"),
/// );
/// assert!(report.is_valid()); // warnings don't fail validation
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ValidationReport {
    /// Critical and error-level issues.
    pub(crate) errors: Vec<ValidationIssue>,
    /// Warning-level issues.
    pub(crate) warnings: Vec<ValidationIssue>,
    /// Informational notes.
    pub(crate) infos: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// Construct a report directly from pre-categorized issue vectors.
    ///
    /// This is the primary escape hatch for code that needs to inject advisory
    /// issues into a report outside the normal validation pipeline — for example,
    /// a middleware layer that wants to attach profile-layer skip notices without
    /// registering a synthetic `ProfileRulePack` rule.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut report = ctx.validate(&segments);
    /// let advisory = ValidationReport::from_issues(
    ///     vec![],
    ///     vec![ValidationIssue::new(ValidationSeverity::Warning, "profile layer skipped")
    ///         .with_rule_id("PROFILE-SKIP-001")],
    ///     vec![],
    /// );
    /// report.merge(advisory);
    /// ```
    pub fn from_issues(
        errors: Vec<ValidationIssue>,
        warnings: Vec<ValidationIssue>,
        infos: Vec<ValidationIssue>,
    ) -> Self {
        Self {
            errors,
            warnings,
            infos,
        }
    }

    /// Returns all error-level [`ValidationIssue`]s in this report.
    pub fn errors(&self) -> &[ValidationIssue] {
        &self.errors
    }

    /// Returns all error-level [`ValidationIssue`]s mutably.
    pub fn errors_mut(&mut self) -> &mut [ValidationIssue] {
        &mut self.errors
    }

    /// Returns all warning-level [`ValidationIssue`]s in this report.
    pub fn warnings(&self) -> &[ValidationIssue] {
        &self.warnings
    }

    /// Returns all warning-level [`ValidationIssue`]s mutably.
    pub fn warnings_mut(&mut self) -> &mut [ValidationIssue] {
        &mut self.warnings
    }

    /// Returns all informational [`ValidationIssue`]s in this report.
    pub fn infos(&self) -> &[ValidationIssue] {
        &self.infos
    }

    /// Returns all informational [`ValidationIssue`]s mutably.
    pub fn infos_mut(&mut self) -> &mut [ValidationIssue] {
        &mut self.infos
    }

    /// Add an error to the report.
    pub fn add_error(&mut self, issue: ValidationIssue) {
        self.errors.push(issue);
    }

    /// Add a warning to the report.
    pub fn add_warning(&mut self, issue: ValidationIssue) {
        self.warnings.push(issue);
    }

    /// Add an info message to the report.
    pub fn add_info(&mut self, issue: ValidationIssue) {
        self.infos.push(issue);
    }

    /// Check if the report has any errors (Critical or Error severity).
    pub fn has_errors(&self) -> bool {
        !self.errors().is_empty()
    }

    /// Check if the report contains at least one `Critical`-severity issue.
    ///
    /// Linear in the number of error-severity issues, which is the bucket that
    /// can hold them.
    pub fn has_critical_errors(&self) -> bool {
        self.errors
            .iter()
            .any(|i| i.severity == ValidationSeverity::Critical)
    }

    /// Check if the report has any warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings().is_empty()
    }

    /// Get the total count of all issues.
    pub fn total_issues(&self) -> usize {
        self.errors().len() + self.warnings().len() + self.infos().len()
    }

    /// Check if the validation passed (no errors, but may have warnings).
    pub fn is_valid(&self) -> bool {
        self.errors().is_empty()
    }

    /// Convert to a `Result`.
    ///
    /// Returns `Ok(self)` when there are no errors.  Returns `Err(self)` when
    /// there is at least one error-level issue, **preserving warnings and infos**
    /// in the `Err` variant so callers can inspect the full report.
    pub fn result(self) -> Result<Self, Self> {
        if self.is_valid() { Ok(self) } else { Err(self) }
    }

    /// Iterate over all issues in severity buckets: errors, warnings, then infos.
    pub fn iter_issues(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.errors()
            .iter()
            .chain(self.warnings().iter())
            .chain(self.infos().iter())
    }

    /// Return `true` if the report contains any issues (errors, warnings, or infos).
    pub fn has_any_issues(&self) -> bool {
        !self.errors().is_empty() || !self.warnings().is_empty() || !self.infos().is_empty()
    }

    /// Drain all issues from `other` into `self`.
    ///
    /// Issues are appended in severity order: errors, warnings, infos.
    /// `other` is left empty after this call.
    pub fn merge(&mut self, mut other: ValidationReport) {
        self.errors.append(&mut other.errors);
        self.warnings.append(&mut other.warnings);
        self.infos.append(&mut other.infos);
    }

    /// Extend `self` with cloned issues from `other` (borrowing).
    ///
    /// Unlike [`merge`](Self::merge), this method borrows `other` so the caller
    /// retains ownership.  Issues are cloned and appended to the respective
    /// severity buckets.  Use `merge` when you can afford to consume `other`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ValidationReport, ValidationIssue, ValidationSeverity};
    ///
    /// let mut combined = ValidationReport::default();
    /// let report = ValidationReport::from_issues(
    ///     vec![ValidationIssue::new(ValidationSeverity::Error, "bad segment")],
    ///     vec![],
    ///     vec![],
    /// );
    /// combined.extend_from(&report);
    /// assert_eq!(combined.errors().len(), 1);
    /// // `report` is still accessible
    /// assert_eq!(report.errors().len(), 1);
    /// ```
    pub fn extend_from(&mut self, other: &ValidationReport) {
        for issue in &other.errors {
            self.add_error(issue.clone());
        }
        for issue in &other.warnings {
            self.add_warning(issue.clone());
        }
        for issue in &other.infos {
            self.add_info(issue.clone());
        }
    }

    /// Iterate over all issues matching an exact profile/MIG rule identifier.
    ///
    /// Searches errors, warnings, and infos in that order.  Returns a lazy
    /// iterator; collect into `Vec` if you need random access.
    pub fn issues_for_rule_id<'a>(
        &'a self,
        rule_id: &'a str,
    ) -> impl Iterator<Item = &'a ValidationIssue> + 'a {
        self.iter_issues()
            .filter(move |issue| issue.rule_id.as_deref() == Some(rule_id))
    }

    fn filter_report<F>(&self, pred: F) -> Self
    where
        F: Fn(&ValidationIssue) -> bool,
    {
        let errors: Vec<ValidationIssue> =
            self.errors().iter().filter(|i| pred(i)).cloned().collect();
        Self {
            errors,
            warnings: self
                .warnings()
                .iter()
                .filter(|i| pred(i))
                .cloned()
                .collect(),
            infos: self.infos().iter().filter(|i| pred(i)).cloned().collect(),
        }
    }

    /// Return a cloned report containing only issues with an exact rule identifier.
    pub fn filter_by_rule_id(&self, rule_id: &str) -> Self {
        self.filter_report(|issue| issue.rule_id.as_deref() == Some(rule_id))
    }

    /// Return a cloned report containing only issues whose rule identifier starts with `prefix`.
    pub fn filter_by_rule_prefix(&self, prefix: &str) -> Self {
        self.filter_report(|issue| {
            issue
                .rule_id
                .as_deref()
                .is_some_and(|id| id.starts_with(prefix))
        })
    }

    /// Return a cloned report containing only issues that reference `segment_tag`.
    ///
    /// Issues whose `segment_tag` field does not match are dropped; the severity
    /// buckets (errors / warnings / infos) are preserved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ValidationReport, ValidationIssue, ValidationSeverity};
    ///
    /// let mut report = ValidationReport::default();
    /// report.add_error(
    ///     ValidationIssue::new(ValidationSeverity::Error, "BGM missing")
    ///         .with_segment("BGM"),
    /// );
    /// report.add_error(
    ///     ValidationIssue::new(ValidationSeverity::Error, "NAD missing")
    ///         .with_segment("NAD"),
    /// );
    /// let bgm_issues = report.for_segment("BGM");
    /// assert_eq!(bgm_issues.errors().len(), 1);
    /// assert_eq!(bgm_issues.errors()[0].segment_tag.as_deref(), Some("BGM"));
    /// ```
    pub fn for_segment(&self, segment_tag: &str) -> Self {
        self.filter_report(|issue| issue.segment_tag.as_deref() == Some(segment_tag))
    }

    /// Return a deterministic, stable text representation for snapshots and logs.
    ///
    /// One line per issue, grouped by severity bucket and sorted by span start,
    /// segment tag, rule id, element and component index, error code and
    /// message, then by segment position, occurrence, layer, kind, pack and
    /// group path. Each line is the message followed by the tokens that are
    /// set, in this order: `[CODE]`, `[segment=…]`, `[rule=…]`, `[element=…]`,
    /// `[component=…]`, `[occurrence=…]`, `[position=…]`, `[path=…]` (the group
    /// path, followed by `/TAG` when the segment tag is known, e.g.
    /// `[path=SG4[0]/SG8[1]/RFF]`), `[layer=…]`, `[kind=…]`, `[pack=…]`,
    /// `[span=…]`, `[hint=…]`.
    pub fn render_deterministic(&self) -> String {
        /// `None` sorts after every value.
        fn none_last<T: Ord>(left: Option<T>, right: Option<T>) -> std::cmp::Ordering {
            match (left, right) {
                (Some(l), Some(r)) => l.cmp(&r),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        }

        /// Group path as a comparable key: step names, then occurrences.
        fn path_key(issue: &ValidationIssue) -> Option<Vec<(&str, u32)>> {
            issue.group_path.as_ref().map(|path| {
                path.steps()
                    .iter()
                    .map(|step| (&*step.name, step.occurrence))
                    .collect()
            })
        }

        fn sorted_refs(issues: &[ValidationIssue]) -> Vec<&ValidationIssue> {
            let mut refs: Vec<&ValidationIssue> = issues.iter().collect();
            refs.sort_by(|left, right| {
                left.start_offset()
                    .unwrap_or(usize::MAX)
                    .cmp(&right.start_offset().unwrap_or(usize::MAX))
                    .then_with(|| {
                        left.segment_tag
                            .as_deref()
                            .unwrap_or("")
                            .cmp(right.segment_tag.as_deref().unwrap_or(""))
                    })
                    .then_with(|| {
                        left.rule_id
                            .as_deref()
                            .unwrap_or("")
                            .cmp(right.rule_id.as_deref().unwrap_or(""))
                    })
                    .then_with(|| none_last(left.element_index, right.element_index))
                    .then_with(|| none_last(left.component_index, right.component_index))
                    .then_with(|| {
                        left.error_code()
                            .unwrap_or("")
                            .cmp(right.error_code().unwrap_or(""))
                    })
                    .then_with(|| left.message.cmp(&right.message))
                    .then_with(|| none_last(left.segment_position, right.segment_position))
                    .then_with(|| none_last(left.segment_occurrence, right.segment_occurrence))
                    .then_with(|| {
                        none_last(
                            left.layer.map(|l| l.as_str()),
                            right.layer.map(|l| l.as_str()),
                        )
                    })
                    .then_with(|| {
                        none_last(
                            left.kind.map(FindingKind::as_str),
                            right.kind.map(FindingKind::as_str),
                        )
                    })
                    .then_with(|| none_last(left.pack(), right.pack()))
                    .then_with(|| none_last(path_key(left), path_key(right)))
            });
            refs
        }

        fn render_issue_line(out: &mut String, issue: &ValidationIssue) {
            use std::fmt::Write as _;
            out.push_str("    - ");
            out.push_str(&issue.message);
            if let Some(code) = issue.error_code() {
                out.push_str(" [");
                out.push_str(code);
                out.push(']');
            }
            if let Some(seg) = &issue.segment_tag {
                out.push_str(" [segment=");
                out.push_str(seg);
                out.push(']');
            }
            if let Some(rule_id) = &issue.rule_id {
                out.push_str(" [rule=");
                out.push_str(rule_id);
                out.push(']');
            }
            if let Some(element_index) = issue.element_index {
                write!(out, " [element={element_index}]").ok();
            }
            if let Some(component_index) = issue.component_index {
                write!(out, " [component={component_index}]").ok();
            }
            if let Some(occurrence) = issue.segment_occurrence {
                write!(out, " [occurrence={occurrence}]").ok();
            }
            if let Some(position) = issue.segment_position {
                write!(out, " [position={position}]").ok();
            }
            if let Some(path) = &issue.group_path {
                write!(out, " [path={path}").ok();
                if let Some(seg) = &issue.segment_tag {
                    out.push('/');
                    out.push_str(seg);
                }
                out.push(']');
            }
            if let Some(layer) = issue.layer {
                write!(out, " [layer={}]", layer.as_str()).ok();
            }
            if let Some(kind) = issue.kind {
                write!(out, " [kind={kind}]").ok();
            }
            if let Some(pack) = &issue.pack {
                write!(out, " [pack={pack}]").ok();
            }
            if let Some(span) = issue.span {
                write!(out, " [span={span}]").ok();
            }
            if let Some(suggestion) = &issue.suggestion {
                out.push_str(" [hint=");
                out.push_str(suggestion);
                out.push(']');
            }
        }

        use std::fmt::Write as _;
        let mut out = String::from("Validation Report:");
        let errors = sorted_refs(self.errors());
        let warnings = sorted_refs(self.warnings());
        let infos = sorted_refs(self.infos());

        if !errors.is_empty() {
            write!(out, "\n  Errors ({})", errors.len()).ok();
            for issue in &errors {
                out.push('\n');
                render_issue_line(&mut out, issue);
            }
        }
        if !warnings.is_empty() {
            write!(out, "\n  Warnings ({})", warnings.len()).ok();
            for issue in &warnings {
                out.push('\n');
                render_issue_line(&mut out, issue);
            }
        }
        if !infos.is_empty() {
            write!(out, "\n  Info ({})", infos.len()).ok();
            for issue in &infos {
                out.push('\n');
                render_issue_line(&mut out, issue);
            }
        }

        out
    }
}

#[cfg(feature = "diagnostics")]
#[cfg_attr(docsrs, doc(cfg(feature = "diagnostics")))]
impl miette::Diagnostic for ValidationIssue {
    /// The error code, else the rule id.
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.error_code()
            .or(self.rule_id())
            .map(|code| Box::new(code) as Box<dyn std::fmt::Display + 'a>)
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            ValidationSeverity::Critical | ValidationSeverity::Error => miette::Severity::Error,
            ValidationSeverity::Warning => miette::Severity::Warning,
            ValidationSeverity::Info => miette::Severity::Advice,
        })
    }

    /// The suggestion, if any.
    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.suggestion()
            .map(|hint| Box::new(hint) as Box<dyn std::fmt::Display + 'a>)
    }

    /// One label at [`span`](ValidationIssue::span), naming the kind and the
    /// group path (with the segment tag) when they are known.
    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        let span = self.span?;
        let location = match (&self.group_path, &self.segment_tag) {
            (Some(path), Some(tag)) => Some(format!("{path}/{tag}")),
            (Some(path), None) => Some(path.to_string()),
            (None, Some(tag)) => Some(tag.clone()),
            (None, None) => None,
        };
        let text = match (self.kind, location) {
            (Some(kind), Some(location)) => Some(format!("{kind} at {location}")),
            (Some(kind), None) => Some(kind.as_str().to_owned()),
            (None, location) => location,
        };
        Some(Box::new(std::iter::once(miette::LabeledSpan::new(
            text,
            span.start,
            // Not `Span::len`: that debug-asserts, and a caller-built issue
            // may carry an inverted span.
            span.end.saturating_sub(span.start),
        ))))
    }
}

#[cfg(feature = "diagnostics")]
#[cfg_attr(docsrs, doc(cfg(feature = "diagnostics")))]
impl miette::Diagnostic for ValidationReport {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new("VALIDATION"))
    }

    fn severity(&self) -> Option<miette::Severity> {
        if self.has_errors() {
            Some(miette::Severity::Error)
        } else if self.has_warnings() {
            Some(miette::Severity::Warning)
        } else {
            Some(miette::Severity::Advice)
        }
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        let msg = format!(
            "Validation found {} error(s), {} warning(s), {} info(s)",
            self.errors().len(),
            self.warnings().len(),
            self.infos().len()
        );
        Some(Box::new(msg))
    }

    /// Every issue as its own diagnostic, in bucket order: errors, warnings,
    /// infos. Attach the source with `miette::Report::with_source_code` and
    /// each issue is underlined where it occurred.
    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn miette::Diagnostic> + 'a>> {
        if !self.has_any_issues() {
            return None;
        }
        Some(Box::new(
            self.iter_issues()
                .map(|issue| issue as &dyn miette::Diagnostic),
        ))
    }
}

impl std::fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.render_deterministic())
    }
}

impl std::error::Error for ValidationReport {}

impl Extend<ValidationIssue> for ValidationReport {
    /// Push each issue into the appropriate severity bucket.
    ///
    /// This enables ergonomic batch collection:
    ///
    /// ```rust
    /// use edifact_rs::{ValidationReport, ValidationIssue, ValidationSeverity};
    ///
    /// let issues = vec![
    ///     ValidationIssue::new(ValidationSeverity::Error, "bad segment"),
    ///     ValidationIssue::new(ValidationSeverity::Warning, "optional field missing"),
    ///     ValidationIssue::new(ValidationSeverity::Info, "advisory note"),
    /// ];
    /// let mut report = ValidationReport::default();
    /// report.extend(issues);
    /// assert_eq!(report.errors().len(), 1);
    /// assert_eq!(report.warnings().len(), 1);
    /// assert_eq!(report.infos().len(), 1);
    /// ```
    fn extend<I: IntoIterator<Item = ValidationIssue>>(&mut self, iter: I) {
        for issue in iter {
            match issue.severity {
                ValidationSeverity::Critical | ValidationSeverity::Error => {
                    self.add_error(issue);
                }
                ValidationSeverity::Warning => {
                    self.add_warning(issue);
                }
                _ => {
                    self.add_info(issue);
                }
            }
        }
    }
}

impl FromIterator<ValidationIssue> for ValidationReport {
    fn from_iter<I: IntoIterator<Item = ValidationIssue>>(iter: I) -> Self {
        let mut report = ValidationReport::default();
        report.extend(iter);
        report
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn severity_orders_by_severity_not_declaration_order() {
        use ValidationSeverity::*;
        // The derived ordering ranked `Critical` lowest, so `max_by_key` on a
        // report returned the least important issue.
        assert!(Critical > Error);
        assert!(Error > Warning);
        assert!(Warning > Info);

        let mut levels = vec![Warning, Critical, Info, Error];
        levels.sort();
        assert_eq!(levels, vec![Info, Warning, Error, Critical]);
        assert_eq!(levels.iter().copied().max(), Some(Critical));
    }

    use super::*;

    #[test]
    fn report_collects_errors_and_warnings() {
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "Test error")
                .with_segment("BGM")
                .with_span(Span::new(42, 57)),
        );
        report.add_warning(ValidationIssue::new(
            ValidationSeverity::Warning,
            "Test warning",
        ));

        assert!(report.has_errors());
        assert!(report.has_warnings());
        assert_eq!(report.total_issues(), 2);
        assert!(!report.is_valid());
    }

    #[test]
    fn report_result_conversion() {
        let mut report = ValidationReport::default();
        report.add_error(ValidationIssue::new(
            ValidationSeverity::Error,
            "Critical issue",
        ));
        assert!(report.result().is_err());
    }

    #[test]
    fn report_valid_with_only_warnings() {
        let mut report = ValidationReport::default();
        report.add_warning(ValidationIssue::new(
            ValidationSeverity::Warning,
            "Just a warning",
        ));
        assert!(report.is_valid());
        assert!(report.result().is_ok());
    }

    #[test]
    fn issue_builder_chain() {
        let issue = ValidationIssue::new(ValidationSeverity::Warning, "test message")
            .with_error_code("E013")
            .with_span(Span::new(100, 118))
            .with_segment("NAD")
            .with_rule_id("DEMO-P001")
            .with_element_index(1)
            .with_component_index(2)
            .with_suggestion("Check element count");

        assert_eq!(issue.error_code(), Some("E013"));
        assert_eq!(issue.message, "test message");
        assert_eq!(issue.span, Some(Span::new(100, 118)));
        assert_eq!(issue.start_offset(), Some(100));
        assert_eq!(issue.segment_tag, Some("NAD".to_owned()));
        assert_eq!(issue.rule_id, Some("DEMO-P001".to_owned()));
        assert_eq!(issue.element_index, Some(1));
        assert_eq!(issue.component_index, Some(2));
        assert_eq!(issue.suggestion, Some("Check element count".to_owned()));
    }

    #[test]
    fn report_display_format() {
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "Error 1")
                .with_error_code("E011")
                .with_span(Span::new(8, 20)),
        );
        report.add_warning(ValidationIssue::new(
            ValidationSeverity::Warning,
            "Warning 1",
        ));
        report.add_info(ValidationIssue::new(ValidationSeverity::Info, "Info 1"));

        let display_str = format!("{report}");
        assert!(display_str.contains("Errors (1)"));
        assert!(display_str.contains("Warnings (1)"));
        assert!(display_str.contains("Info (1)"));
        assert!(display_str.contains("[E011]"));
    }

    #[test]
    fn render_deterministic_sorts_by_span_start() {
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "later")
                .with_segment("BGM")
                .with_span(Span::new(20, 30)),
        );
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "earlier")
                .with_segment("UNH")
                .with_span(Span::new(1, 19)),
        );

        let rendered = report.render_deterministic();
        let first = rendered.find("earlier").expect("missing first issue");
        let second = rendered.find("later").expect("missing second issue");
        assert!(first < second, "expected deterministic sort by span start");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn error_code_survives_a_serde_round_trip() {
        // A persisted report that loses its codes cannot be filtered or routed
        // on them after reload — the reason the field is owned on the wire.
        let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
            .with_error_code("E014")
            .with_span(Span::new(9, 22))
            .with_rule_id("PROFILE-4711-BGM-M");
        let json = serde_json::to_string(&issue).expect("serialize");
        let back: ValidationIssue = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.error_code(), Some("E014"));
        assert_eq!(back.span, Some(Span::new(9, 22)));
        assert_eq!(back, issue);
    }

    #[test]
    fn filter_by_rule_id() {
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "orders policy blocked")
                .with_rule_id("ORDERS-P001"),
        );
        report.add_warning(
            ValidationIssue::new(ValidationSeverity::Warning, "invoic policy warning")
                .with_rule_id("INVOIC-P001"),
        );
        report.add_info(
            ValidationIssue::new(ValidationSeverity::Info, "orders policy info")
                .with_rule_id("ORDERS-P002"),
        );

        let only_orders_block = report.filter_by_rule_id("ORDERS-P001");
        assert_eq!(only_orders_block.errors().len(), 1);
        assert!(only_orders_block.warnings().is_empty());
        assert!(only_orders_block.infos().is_empty());

        let orders_family = report.filter_by_rule_prefix("ORDERS-");
        assert_eq!(orders_family.total_issues(), 2);

        let exact: Vec<_> = report.issues_for_rule_id("INVOIC-P001").collect();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].message, "invoic policy warning");
    }

    #[test]
    fn context_map_builder() {
        let issue = ValidationIssue::new(ValidationSeverity::Error, "BGM code invalid")
            .with_context_entry("pid", "4711")
            .with_context_entry("partner", "9900123456789");

        assert_eq!(issue.context_get("pid"), Some("4711"));
        assert_eq!(issue.context_get("partner"), Some("9900123456789"));
        assert_eq!(issue.context_get("missing"), None);
    }

    #[test]
    fn context_map_extend() {
        let meta = [("pid", "4711"), ("partner", "9900123456789")];
        let issue =
            ValidationIssue::new(ValidationSeverity::Error, "test").with_context_entries(meta);
        assert_eq!(issue.context_get("pid"), Some("4711"));
    }

    #[test]
    fn context_key_overwrite() {
        let issue = ValidationIssue::new(ValidationSeverity::Warning, "demo")
            .with_context_entry("pid", "old")
            .with_context_entry("pid", "new");
        assert_eq!(issue.context_get("pid"), Some("new"));
    }

    // ── 019: positions, kinds, paths ─────────────────────────────────────────

    #[test]
    fn positional_builders_take_usize_and_never_clamp() {
        let element: usize = 3;
        let component: usize = 1;
        let occurrence: usize = 69_999;
        let index: usize = 70_002;
        let issue = ValidationIssue::new(ValidationSeverity::Error, "x")
            .with_element_index(element)
            .with_component_index(component)
            .with_segment_occurrence(occurrence)
            .with_segment_position(index + 1);
        assert_eq!(issue.element_index(), Some(3));
        assert_eq!(issue.component_index(), Some(1));
        assert_eq!(issue.segment_occurrence(), Some(69_999));
        assert_eq!(issue.segment_position(), Some(70_003));

        // Out of range leaves the field `None` — never `MAX`.
        let issue = issue
            .with_element_index(70_000)
            .with_component_index(usize::from(u16::MAX) + 1);
        assert_eq!(issue.element_index(), None);
        assert_eq!(issue.component_index(), None);
        assert_eq!(
            ValidationIssue::new(ValidationSeverity::Error, "x")
                .with_element_index(usize::from(u16::MAX))
                .element_index(),
            Some(u16::MAX),
            "the largest value that fits is kept as is"
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn occurrence_and_position_beyond_u32_are_none() {
        let too_big = u32::MAX as usize + 1;
        let issue = ValidationIssue::new(ValidationSeverity::Error, "x")
            .with_segment_occurrence(too_big)
            .with_segment_position(too_big);
        assert_eq!(issue.segment_occurrence(), None);
        assert_eq!(issue.segment_position(), None);
        assert!(GroupStep::new("SG4", too_big).is_none());
    }

    #[test]
    fn issue_size_is_reported() {
        // Prints size_of::<ValidationIssue>() under --nocapture.
        let size = std::mem::size_of::<ValidationIssue>();
        println!("size_of::<ValidationIssue>() = {size}");
        #[cfg(target_pointer_width = "64")]
        assert!(size <= 272, "ValidationIssue grew to {size} bytes");
    }

    #[test]
    fn group_path_display_steps_and_root() {
        let path = GroupPath::new(GroupStep::new("SG4", 0))
            .and_then(|p| p.child("SG8", 1))
            .expect("path");
        assert_eq!(path.to_string(), "SG4[0]/SG8[1]");
        assert_eq!(path.depth(), 2);
        assert_eq!(path.steps()[0], GroupStep::new("SG4", 0).unwrap());
        assert_eq!(&*path.last().name, "SG8");
        assert_eq!(path.last().occurrence, 1);
        assert!(GroupPath::new(Vec::new()).is_none());

        // A clone shares the step list.
        let clone = path.clone();
        assert!(std::ptr::eq(path.steps(), clone.steps()));
    }

    #[test]
    fn with_group_path_fills_an_empty_segment_group_only() {
        let path = GroupPath::new(GroupStep::new("SG8", 2)).unwrap();
        let filled =
            ValidationIssue::new(ValidationSeverity::Error, "x").with_group_path(path.clone());
        assert_eq!(filled.segment_group(), Some("SG8"));
        assert_eq!(filled.group_path(), Some(&path));

        let kept = ValidationIssue::new(ValidationSeverity::Error, "x")
            .with_segment_group("CUSTOM")
            .with_group_path(path);
        assert_eq!(kept.segment_group(), Some("CUSTOM"));
    }

    #[test]
    fn render_deterministic_shows_the_new_tokens() {
        let path = GroupPath::new(GroupStep::new("SG4", 0))
            .and_then(|p| p.child("SG8", 1))
            .unwrap();
        let mut report = ValidationReport::default();
        report.add_error(
            ValidationIssue::new(ValidationSeverity::Error, "RFF missing")
                .with_segment("RFF")
                .with_segment_occurrence(2)
                .with_segment_position(17)
                .with_group_path(path)
                .with_layer(ValidationLayer::Profile)
                .with_kind(FindingKind::Missing)
                .with_pack("AHB-UTILMD"),
        );
        let rendered = report.render_deterministic();
        assert!(
            rendered.contains(
                "RFF missing [segment=RFF] [occurrence=2] [position=17] \
                 [path=SG4[0]/SG8[1]/RFF] [layer=profile] [kind=missing] [pack=AHB-UTILMD]"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn render_deterministic_sorts_on_new_keys_after_existing_ones() {
        let mut report = ValidationReport::default();
        for position in [9_usize, 3, 5] {
            report.add_error(
                ValidationIssue::new(ValidationSeverity::Error, "same")
                    .with_segment("QTY")
                    .with_segment_position(position),
            );
        }
        let rendered = report.render_deterministic();
        let at = |p: &str| rendered.find(p).unwrap();
        assert!(at("[position=3]") < at("[position=5]"));
        assert!(at("[position=5]") < at("[position=9]"));
    }

    /// One value of every `EdifactError` variant this crate can construct.
    fn every_error() -> Vec<(crate::EdifactError, FindingKind)> {
        use crate::EdifactError as E;
        use FindingKind as K;
        let span = Span::new(0, 1);
        let tag = || "QTY".to_owned();
        vec![
            (E::UnexpectedEof { offset: 0 }, K::Syntax),
            (
                E::InvalidDelimiter {
                    byte: b'x',
                    offset: 0,
                },
                K::Syntax,
            ),
            (E::InvalidText { offset: 0 }, K::Charset),
            (E::InvalidReleaseSequence { offset: 0 }, K::Syntax),
            (
                E::MessageCountMismatch {
                    expected: 1,
                    actual: 2,
                },
                K::ControlMismatch,
            ),
            (
                E::SegmentCountMismatch {
                    expected: 1,
                    actual: 2,
                    message_ref: "1".into(),
                    span,
                },
                K::ControlMismatch,
            ),
            (E::InvalidSegmentTag("x".into()), K::Syntax),
            (E::InvalidUna, K::Syntax),
            (
                E::MissingRequiredElement {
                    tag: tag(),
                    element_index: 1,
                },
                K::Missing,
            ),
            (
                E::MissingRequiredComponent {
                    tag: tag(),
                    element_index: 1,
                    component_index: 0,
                },
                K::Missing,
            ),
            (E::InvalidUtf8, K::Charset),
            (E::Io(std::io::Error::other("x").into()), K::Other),
            (
                E::InvalidSegmentForMessage {
                    tag: tag(),
                    message_type: "ORDERS".into(),
                    span,
                },
                K::Placement,
            ),
            (
                E::InvalidElementCount {
                    tag: tag(),
                    min: 1,
                    max: 2,
                    actual: 3,
                    span,
                },
                K::Count,
            ),
            (
                E::InvalidComponentCount {
                    tag: tag(),
                    element_index: 0,
                    expected: 2,
                    actual: 3,
                    span,
                },
                K::Count,
            ),
            (
                E::InvalidCodeValue {
                    tag: tag(),
                    element_index: 0,
                    value: "999".into(),
                    code_list: "6063".into(),
                    span,
                    suggestion: None,
                },
                K::Code,
            ),
            (
                E::MissingSegment {
                    tag: tag(),
                    expected_position: "after LIN".into(),
                },
                K::Missing,
            ),
            (
                E::QualifierMismatch {
                    tag: "UNT".into(),
                    actual: "2".into(),
                    expected: "1".into(),
                    span,
                },
                K::ControlMismatch,
            ),
            (
                E::ConditionalRequirementNotMet {
                    tag: tag(),
                    element_index: 0,
                    condition: "x".into(),
                    span,
                },
                K::Missing,
            ),
            (
                E::ValidationErrors {
                    error_count: 0,
                    report: Box::default(),
                },
                K::Other,
            ),
            (
                E::SegmentTooLong {
                    offset: 0,
                    limit: 1,
                },
                K::Limit,
            ),
            (E::InterchangeTooLarge { count: 1 }, K::Limit),
            (E::InvalidEventSequence { message: "x" }, K::Other),
            (E::InvalidElementPosition, K::Other),
            (
                E::IncompatibleReleaseScopes {
                    current: "a".into(),
                    incoming: "b".into(),
                },
                K::Other,
            ),
            (
                E::InvalidFieldValue {
                    tag: tag(),
                    element_index: 0,
                    value: "x".into(),
                },
                K::Representation,
            ),
            (E::UnexpectedDataToken { offset: 0 }, K::Syntax),
            (E::UnrecognisedSyntaxIdentifier("UNOZ".into()), K::Charset),
            (
                E::DuplicateReference {
                    tag: "UNH".into(),
                    reference: "1".into(),
                    span,
                },
                K::Duplicate,
            ),
            (
                E::UnknownDataElement {
                    tag: tag(),
                    data_element: "9999".into(),
                },
                K::Other,
            ),
            (
                E::AmbiguousDataElement {
                    tag: tag(),
                    data_element: "6063".into(),
                },
                K::Other,
            ),
            (
                E::LimitExceeded {
                    limit: "x",
                    max: 1,
                    offset: 0,
                },
                K::Limit,
            ),
            (E::RepetitionSeparatorNotDeclared, K::Syntax),
            (
                E::NonFiniteNumber {
                    value: "NaN".into(),
                },
                K::Representation,
            ),
            (
                E::CharacterNotInRepertoire {
                    charset: "UNOA",
                    character: 'a',
                    offset: 0,
                },
                K::Charset,
            ),
            (
                E::CharacterRepertoireMismatch {
                    declared: "UNOA".into(),
                    writer: "UNOC",
                },
                K::Charset,
            ),
            (
                E::UnsupportedCharset {
                    syntax_identifier: "KECA".into(),
                },
                K::Charset,
            ),
            (
                E::EmptyInterchange {
                    control_ref: "1".into(),
                },
                K::Envelope,
            ),
            (
                E::EmptyMessage {
                    message_ref: "1".into(),
                    span,
                },
                K::Envelope,
            ),
            (
                E::PackageNotSupported {
                    tag: "UNO".into(),
                    span,
                },
                K::Unsupported,
            ),
            (
                E::BlankDataElementValue {
                    tag: tag(),
                    element_index: 0,
                    component_index: 0,
                    span,
                },
                K::Representation,
            ),
            (
                E::SegmentWithoutDataElements { tag: tag(), span },
                K::Syntax,
            ),
            (
                E::TooManyRepetitions {
                    tag: tag(),
                    element_index: 0,
                    max: 1,
                    actual: 2,
                    span,
                },
                K::Count,
            ),
            (
                E::InvalidCharacterType {
                    tag: tag(),
                    element_index: 0,
                    component_index: 0,
                    repr: "n..3".into(),
                    value: "x".into(),
                    span,
                },
                K::Representation,
            ),
            (
                E::DataElementTooLong {
                    tag: tag(),
                    element_index: 0,
                    component_index: 0,
                    repr: "an..3".into(),
                    actual: 4,
                    span,
                },
                K::Representation,
            ),
            (
                E::DataElementTooShort {
                    tag: tag(),
                    element_index: 0,
                    component_index: 0,
                    repr: "an3".into(),
                    actual: 2,
                    span,
                },
                K::Representation,
            ),
            (
                E::TrailingSeparator {
                    tag: tag(),
                    element_index: None,
                    span,
                },
                K::Syntax,
            ),
            (E::GroupsAndMessagesMixed { span }, K::Envelope),
            (
                E::InsignificantCharacters {
                    tag: tag(),
                    element_index: 0,
                    component_index: 0,
                    kind: crate::Insignificant::LeadingZeroes,
                    span,
                },
                K::Representation,
            ),
            (
                E::SegmentLayoutMismatch {
                    expected: "a".into(),
                    actual: "b".into(),
                },
                K::Other,
            ),
            (
                E::ActionNotPermitted {
                    segment_tag: "UCI",
                    action: "8",
                },
                K::Other,
            ),
            (
                E::InvalidContrlIdentifier {
                    data_element: "0057",
                    value: "TOOLONG".into(),
                },
                K::Other,
            ),
            (
                E::SegmentNotPlaced {
                    tag: tag(),
                    reason: crate::group::UnplacedReason::Unknown,
                    span,
                },
                K::Placement,
            ),
            (
                E::TooManySegmentOccurrences {
                    tag: tag(),
                    max: 1,
                    span,
                },
                K::Count,
            ),
            (
                E::TooManyGroupOccurrences {
                    group: "SG5".into(),
                    max: 1,
                    span,
                },
                K::Count,
            ),
            (
                E::InvalidGroupSchema {
                    group: "SG5".into(),
                    reason: "max is 0",
                },
                K::Other,
            ),
        ]
    }

    #[test]
    fn finding_kind_for_every_error() {
        let all = every_error();
        // One entry per stable code: the 53 codes E001–E053 minus the three
        // retired ones (E018, E022, E029), plus E066–E071.
        let mut codes: Vec<&str> = all.iter().map(|(e, _)| e.stable_code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 56, "every EdifactError variant must be listed");
        assert_eq!(all.len(), 56);
        for (error, kind) in &all {
            assert_eq!(
                FindingKind::for_error(error),
                *kind,
                "{} ({error:?})",
                error.stable_code()
            );
        }
    }

    #[test]
    fn finding_kind_and_layer_strings_are_stable() {
        let kinds = [
            (FindingKind::Missing, "missing"),
            (FindingKind::NotPermitted, "not_permitted"),
            (FindingKind::Code, "code"),
            (FindingKind::Representation, "representation"),
            (FindingKind::Count, "count"),
            (FindingKind::Placement, "placement"),
            (FindingKind::ControlMismatch, "control_mismatch"),
            (FindingKind::Duplicate, "duplicate"),
            (FindingKind::Envelope, "envelope"),
            (FindingKind::Charset, "charset"),
            (FindingKind::Syntax, "syntax"),
            (FindingKind::Limit, "limit"),
            (FindingKind::Unsupported, "unsupported"),
            (FindingKind::Other, "other"),
        ];
        for (kind, text) in kinds {
            assert_eq!(kind.as_str(), text);
            assert_eq!(kind.to_string(), text);
        }
        let layers = [
            (ValidationLayer::Envelope, "envelope"),
            (ValidationLayer::Syntax, "syntax"),
            (ValidationLayer::Charset, "charset"),
            (ValidationLayer::Structure, "structure"),
            (ValidationLayer::CodeList, "code_list"),
            (ValidationLayer::Profile, "profile"),
        ];
        for (layer, text) in layers {
            assert_eq!(layer.as_str(), text);
        }
    }

    #[cfg(feature = "serde")]
    fn full_issue() -> ValidationIssue {
        ValidationIssue::new(ValidationSeverity::Warning, "every field")
            .with_error_code("E014")
            .with_span(Span::new(3, 9))
            .with_segment("RFF")
            .with_rule_id("AHB-RFF-CODE")
            .with_element_index(1)
            .with_component_index(2)
            .with_segment_occurrence(70_000)
            .with_segment_position(70_004)
            .with_message_ref("MSG1")
            .with_suggestion("fix it")
            .with_group_path(
                GroupPath::new(GroupStep::new("SG4", 0))
                    .and_then(|p| p.child("SG8", 1))
                    .unwrap(),
            )
            .with_layer(ValidationLayer::CodeList)
            .with_kind(FindingKind::Code)
            .with_pack("AHB-UTILMD")
            .with_context_entry("pid", "55001")
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_round_trips_every_field() {
        let issue = full_issue();
        let json = serde_json::to_string(&issue).expect("serialize");
        let back: ValidationIssue = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, issue);

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["layer"], "CodeList");
        assert_eq!(value["kind"], "Code");
        assert_eq!(value["pack"], "AHB-UTILMD");
        assert_eq!(value["segment_position"], 70_004);
        assert_eq!(
            value["group_path"],
            serde_json::json!([
                {"group": "SG4", "occurrence": 0},
                {"group": "SG8", "occurrence": 1}
            ])
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_omits_unset_new_fields() {
        let json = serde_json::to_string(&ValidationIssue::new(ValidationSeverity::Info, "x"))
            .expect("serialize");
        for field in ["segment_position", "group_path", "layer", "kind", "pack"] {
            assert!(!json.contains(field), "{field} in {json}");
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_unknown_kind_reads_as_other() {
        let json = r#"{"severity":"Error","message":"x","span":null,"segment_tag":null,
            "rule_id":null,"element_index":null,"component_index":null,
            "segment_occurrence":null,"message_ref":null,"suggestion":null,
            "segment_group":null,"kind":"SomethingNewer"}"#;
        let issue: ValidationIssue = serde_json::from_str(json).expect("deserialize");
        assert_eq!(issue.kind(), Some(FindingKind::Other));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_refuses_an_empty_group_path() {
        let json = r#"{"severity":"Error","message":"x","span":null,"segment_tag":null,
            "rule_id":null,"element_index":null,"component_index":null,
            "segment_occurrence":null,"message_ref":null,"suggestion":null,
            "segment_group":null,"group_path":[]}"#;
        assert!(serde_json::from_str::<ValidationIssue>(json).is_err());
    }
}
