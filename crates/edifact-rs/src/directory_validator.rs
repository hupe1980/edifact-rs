//! Shared UN/EDIFACT directory validation engine used by D.11A, D.01B and D.96A.

use crate::error::Insignificant;
use crate::validator::{ValidationRuleContext, Validator, report_error};
use crate::{EdifactError, Segment, ValidationIssue, ValidationReport, ValidationSeverity};
use std::sync::Arc;

/// Mandatory/Conditional status of a data element within a segment.
///
/// Marked `#[non_exhaustive]` because UN/EDIFACT also defines Required, Advised,
/// Dependent, and Not-used statuses; adding one must not break downstream `match`
/// arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// Element must be present.
    Mandatory,
    /// Element is optional unless additional rules require it.
    Conditional,
}

/// The character class of a data element value — the `a` / `n` / `an` of a
/// directory's representation column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReprKind {
    /// `a` — alphabetic. Digits are not permitted.
    Alphabetic,
    /// `n` — numeric, in the ISO 6093 forms ISO 9735-1 §10 admits: digits, an
    /// optional leading minus, a decimal mark (`.` or `,`), and an exponent.
    /// The space character and the plus sign are explicitly not allowed.
    Numeric,
    /// `an` — alphanumeric. Any character the interchange's repertoire permits.
    Alphanumeric,
}

impl ReprKind {
    /// The directory's abbreviation for this class.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alphabetic => "a",
            Self::Numeric => "n",
            Self::Alphanumeric => "an",
        }
    }
}

/// A data element's representation — `an..35`, `n8`, `a1` and friends.
///
/// This is the column every UN/EDIFACT directory prints beside a data element,
/// and the thing partners actually reject on: a sender identification of 40
/// characters where the standard says `an..35` is refused at the far end, long
/// after it was sent.
///
/// # Length is counted in characters
///
/// ISO 9735-1 §6: "one graphic character shall be counted as one character,
/// irrespective of the number of bytes/octets required to encode it" — so `ü`
/// is one, not two. §5 excludes the release character from the count, which is
/// automatic here because release sequences are already resolved by the time a
/// value reaches validation.
///
/// For a numeric value §10 excludes more: "the length ... shall not include the
/// minus sign (-), the decimal mark (. or ,), or the exponent mark (E or e) and
/// its exponent". `-123.45` is therefore five characters long, not seven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Repr {
    kind: ReprKind,
    max: u16,
    fixed: bool,
}

impl Repr {
    /// `an{max}` — exactly `max` alphanumeric characters.
    #[must_use]
    pub const fn an(max: u16) -> Self {
        Self {
            kind: ReprKind::Alphanumeric,
            max,
            fixed: true,
        }
    }

    /// `an..{max}` — up to `max` alphanumeric characters.
    #[must_use]
    pub const fn an_up_to(max: u16) -> Self {
        Self {
            kind: ReprKind::Alphanumeric,
            max,
            fixed: false,
        }
    }

    /// `a{max}` — exactly `max` alphabetic characters.
    #[must_use]
    pub const fn a(max: u16) -> Self {
        Self {
            kind: ReprKind::Alphabetic,
            max,
            fixed: true,
        }
    }

    /// `a..{max}` — up to `max` alphabetic characters.
    #[must_use]
    pub const fn a_up_to(max: u16) -> Self {
        Self {
            kind: ReprKind::Alphabetic,
            max,
            fixed: false,
        }
    }

    /// `n{max}` — exactly `max` numeric characters.
    #[must_use]
    pub const fn n(max: u16) -> Self {
        Self {
            kind: ReprKind::Numeric,
            max,
            fixed: true,
        }
    }

    /// `n..{max}` — up to `max` numeric characters.
    #[must_use]
    pub const fn n_up_to(max: u16) -> Self {
        Self {
            kind: ReprKind::Numeric,
            max,
            fixed: false,
        }
    }

    /// The character class.
    #[must_use]
    pub const fn kind(self) -> ReprKind {
        self.kind
    }

    /// The maximum number of characters.
    #[must_use]
    pub const fn max_length(self) -> u16 {
        self.max
    }

    /// The minimum number of characters — `max_length` when fixed, else 1.
    ///
    /// A data element is "present" only when it carries at least one character
    /// (§8.1), so a variable-length minimum is never zero.
    #[must_use]
    pub const fn min_length(self) -> u16 {
        if self.fixed { self.max } else { 1 }
    }

    /// Whether the length is fixed (`an3`) rather than variable (`an..3`).
    #[must_use]
    pub const fn is_fixed(self) -> bool {
        self.fixed
    }

    /// The number of characters `value` contributes toward its length limit.
    ///
    /// For a numeric value this is §10's count, which excludes the sign, the
    /// decimal mark, and the exponent.
    #[must_use]
    pub fn measure(self, value: &str) -> usize {
        match self.kind {
            ReprKind::Numeric => {
                let mantissa = value
                    .split_once(['E', 'e'])
                    .map_or(value, |(mantissa, _exponent)| mantissa);
                mantissa
                    .chars()
                    .filter(|c| !matches!(c, '-' | '.' | ','))
                    .count()
            }
            _ => value.chars().count(),
        }
    }

    /// Whether every character of `value` belongs to this representation's class.
    #[must_use]
    pub fn permits_characters(self, value: &str) -> bool {
        match self.kind {
            ReprKind::Alphanumeric => true,
            ReprKind::Alphabetic => !value.chars().any(|c| c.is_ascii_digit()),
            ReprKind::Numeric => is_iso6093_numeric(value),
        }
    }
}

impl std::fmt::Display for Repr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.fixed {
            write!(f, "{}{}", self.kind.as_str(), self.max)
        } else {
            write!(f, "{}..{}", self.kind.as_str(), self.max)
        }
    }
}

/// Whether `value` is one of the numeric forms ISO 9735-1 §10 admits.
///
/// §10 takes ISO 6093's representations and subtracts: "The space character and
/// plus sign shall not be allowed", and "when a decimal mark is transferred,
/// there shall be at least one digit after the decimal mark" — which is why
/// `1.` and `.` are rejected while `.5` and `2.00` are not.
fn is_iso6093_numeric(value: &str) -> bool {
    let (mantissa, exponent) = match value.split_once(['E', 'e']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (value, None),
    };
    if let Some(exponent) = exponent {
        let digits = exponent.strip_prefix('-').unwrap_or(exponent);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    let digits = mantissa.strip_prefix('-').unwrap_or(mantissa);
    if digits.is_empty() {
        return false;
    }
    match digits.split_once(['.', ',']) {
        Some((integer, fraction)) => {
            // At least one digit after the mark; the integer part may be empty.
            !fraction.is_empty()
                && fraction.chars().all(|c| c.is_ascii_digit())
                && integer.chars().all(|c| c.is_ascii_digit())
        }
        None => digits.chars().all(|c| c.is_ascii_digit()),
    }
}

/// What a value at one position must satisfy.
///
/// Usually a single [`Repr`]. Two only where the syntax versions disagree and
/// the interchange has not said which it is — see
/// [`ComponentRef::with_repr_by_syntax_version`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReprRequirement {
    primary: Repr,
    /// Accepted as well, when the syntax version could not be determined.
    alternative: Option<Repr>,
}

impl ReprRequirement {
    const fn single(repr: Repr) -> Self {
        Self {
            primary: repr,
            alternative: None,
        }
    }

    /// Whether `value` satisfies the class of any accepted representation.
    fn permits_characters(&self, value: &str) -> bool {
        self.primary.permits_characters(value)
            || self
                .alternative
                .is_some_and(|repr| repr.permits_characters(value))
    }

    /// Whether `value`'s length satisfies any accepted representation.
    fn permits_length(&self, value: &str) -> bool {
        let fits = |repr: Repr| {
            let length = repr.measure(value);
            length >= usize::from(repr.min_length()) && length <= usize::from(repr.max_length())
        };
        fits(self.primary) || self.alternative.is_some_and(fits)
    }

    /// `true` when every accepted representation is longer than `value`.
    fn is_too_short(&self, value: &str) -> bool {
        let short = |repr: Repr| repr.measure(value) < usize::from(repr.min_length());
        short(self.primary) && self.alternative.is_none_or(short)
    }

    /// The measured length under the primary representation, for reporting.
    fn measure(&self, value: &str) -> usize {
        self.primary.measure(value)
    }
}

impl std::fmt::Display for ReprRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.alternative {
            Some(alternative) => write!(f, "{} or {alternative}", self.primary),
            None => write!(f, "{}", self.primary),
        }
    }
}

/// Reference to a component data element within a composite data element.
///
/// Composites such as `C507` (DTM date/time/period) are addressed by the
/// identifier of their *own* components (`2005`, `2380`, `2379`), which is what
/// makes code-addressed access — [`Segment::value_by_code`][crate::Segment::value_by_code]
/// and `#[edifact(element = "2005")]` — resolve to the right slot instead of a
/// hand-counted index.
///
/// Fields are private to enforce the one-based position invariant.
#[derive(Debug, Clone, Copy)]
pub struct ComponentRef {
    /// One-based position of the **first** slot this component occupies.
    position: u8,
    /// UN/EDIFACT component data element identifier.
    data_element: &'static str,
    /// Requirement status of the component.
    status: Status,
    /// How many consecutive slots this component occupies; `1` unless the
    /// composite repeats it by design.
    repeat_count: u8,
    /// The directory's representation, when the definition states one.
    repr: Option<Repr>,
    /// The representation from syntax version 4 onward, where it differs.
    repr_from_v4: Option<Repr>,
}

impl ComponentRef {
    /// Construct a `ComponentRef` with compile-time position validation.
    ///
    /// `position` must be ≥ 1 (one-based).  In a `const` context a zero
    /// `position` is a **compile-time error**; at runtime it panics.
    ///
    /// # Panics
    ///
    /// Panics if `position == 0`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ComponentRef, Status};
    ///
    /// const DTM_2005: ComponentRef = ComponentRef::new(1, "2005", Status::Mandatory);
    /// ```
    #[must_use]
    pub const fn new(position: u8, data_element: &'static str, status: Status) -> Self {
        assert!(
            position != 0,
            "ComponentRef position must be >= 1 (one-based)"
        );
        Self {
            position,
            data_element,
            status,
            repeat_count: 1,
            repr: None,
            repr_from_v4: None,
        }
    }

    /// Declare a component the composite repeats by design.
    ///
    /// Several standard composites carry the same data element several times
    /// over: `C080 PARTY NAME` is `3036` five times followed by `3045`, `C059
    /// STREET` is `3042` four times.  Spelling that out as five separate
    /// [`new`][Self::new] entries made the code count as five positions, so
    /// [`resolve_code`][SegmentLayout::resolve_code] reported it
    /// [ambiguous][EdifactError::AmbiguousDataElement] and the component could
    /// not be code-addressed at all — a faithful declaration was punished, and
    /// the only way to use named access was to declare the composite
    /// incompletely and disagree with the directory it claims to model.
    ///
    /// One `repeated` entry is one position, so `3036` resolves to occurrence 1
    /// — what "the party name" means in every real message — while the
    /// definition still records that five slots belong to it.
    ///
    /// `position` is the **first** slot; the next component follows at
    /// `position + repeat_count`.
    ///
    /// # Panics
    ///
    /// Panics if `position == 0` or `repeat_count == 0`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ComponentRef, ElementRef, SegmentDefinition, SegmentLayout, Status};
    ///
    /// // C080 PARTY NAME: 3036 ×5, then 3045 at position 6.
    /// static C080: &[ComponentRef] = &[
    ///     ComponentRef::repeated(1, "3036", Status::Mandatory, 5),
    ///     ComponentRef::new(6, "3045", Status::Conditional),
    /// ];
    /// static NAD_ELEMENTS: &[ElementRef] = &[
    ///     ElementRef::new(1, "3035", Status::Mandatory, 1),
    ///     ElementRef::composite(4, "C080", Status::Conditional, 1, C080),
    /// ];
    /// static NAD: SegmentDefinition = SegmentDefinition::new("NAD", "Name and address", NAD_ELEMENTS);
    ///
    /// // Addressable, and it points at the first occurrence.
    /// let path = NAD.resolve_code("3036")?;
    /// assert_eq!((path.element, path.component), (3, Some(0)));
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[must_use]
    pub const fn repeated(
        position: u8,
        data_element: &'static str,
        status: Status,
        repeat_count: u8,
    ) -> Self {
        assert!(
            position != 0,
            "ComponentRef position must be >= 1 (one-based)"
        );
        assert!(
            repeat_count != 0,
            "ComponentRef repeat_count must be >= 1; use `new` for a component that does not repeat"
        );
        Self {
            position,
            data_element,
            status,
            repeat_count,
            repr: None,
            repr_from_v4: None,
        }
    }

    /// How many consecutive slots this component occupies.
    ///
    /// `1` for a component declared with [`new`][Self::new].
    #[must_use]
    #[inline]
    pub const fn repeat_count(&self) -> u8 {
        self.repeat_count
    }

    /// One-based component position within the composite.
    #[must_use]
    #[inline]
    pub const fn position(&self) -> u8 {
        self.position
    }

    /// UN/EDIFACT component data element identifier.
    #[must_use]
    #[inline]
    pub const fn data_element(&self) -> &'static str {
        self.data_element
    }

    /// Requirement status of the component.
    #[must_use]
    #[inline]
    pub const fn status(&self) -> Status {
        self.status
    }

    /// Attach the directory's representation, e.g. `an..35`.
    ///
    /// Without one, a definition still resolves identifiers and checks presence;
    /// with one, [`DirectoryValidator`] also checks length and character class —
    /// which is what a partner's translator does before it rejects the file.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{ComponentRef, Repr, Status};
    ///
    /// const SENDER: ComponentRef =
    ///     ComponentRef::new(1, "0004", Status::Mandatory).with_repr(Repr::an_up_to(35));
    /// ```
    #[must_use]
    pub const fn with_repr(mut self, repr: Repr) -> Self {
        self.repr = Some(repr);
        self
    }

    /// The declared representation, if the definition states one.
    #[must_use]
    #[inline]
    pub const fn repr(&self) -> Option<Repr> {
        self.repr
    }
    /// Declare a representation that changed between syntax versions.
    ///
    /// The service directory has exactly one such position: `S004` DE 0017, the
    /// date of preparation. Version 3 transfers `YYMMDD` (`n6`); version 4
    /// widened it to `CCYYMMDD` (`n8`) to be year-2000 correct. It is the only
    /// place where version 4 is *not* a superset of version 3.
    ///
    /// Declaring both keeps each version checked exactly. Collapsing them into
    /// `n..8` would have accepted a six-digit date in a version 4 interchange
    /// and a seven-digit one in either — validating neither version correctly.
    ///
    /// The syntax version comes from `UNB` S001 DE 0002. When it cannot be
    /// determined — validating a bare message window, say — **both** are
    /// accepted, because guessing would reject conformant data.
    #[must_use]
    pub const fn with_repr_by_syntax_version(mut self, up_to_v3: Repr, from_v4: Repr) -> Self {
        self.repr = Some(up_to_v3);
        self.repr_from_v4 = Some(from_v4);
        self
    }

    /// The representation used from syntax version 4 onward, when it differs.
    #[must_use]
    #[inline]
    pub const fn repr_from_v4(&self) -> Option<Repr> {
        self.repr_from_v4
    }
}

/// Reference to a data element within a segment definition.
///
/// Fields are private to enforce the one-based position invariant through the
/// [`ElementRef::new`] constructor.  Use [`ElementRef::new`] for a simple data
/// element and [`ElementRef::composite`] for a composite whose components are
/// themselves named (panics at compile time when `position == 0`).
///
/// Use [`OwnedElementRef`] for runtime-constructed element refs.
#[derive(Debug, Clone, Copy)]
pub struct ElementRef {
    /// One-based element position in the segment definition.
    position: u8,
    /// UN/EDIFACT data element identifier.
    data_element: &'static str,
    /// Requirement status of the element.
    status: Status,
    /// Maximum repetition count for this element.
    max_repeat: u8,
    /// Component definitions when this element is a composite; empty for a
    /// simple data element.
    components: &'static [ComponentRef],
    /// The directory's representation for a *simple* element; composites carry
    /// theirs on each component.
    repr: Option<Repr>,
    /// The representation from syntax version 4 onward, where it differs.
    repr_from_v4: Option<Repr>,
}

impl ElementRef {
    /// Construct an `ElementRef` for a simple data element.
    ///
    /// `position` must be ≥ 1 (one-based).  When called in a `const` context
    /// (e.g. inside a `static` array initialiser), a zero `position` causes a
    /// **compile-time error**.  At runtime it panics.
    ///
    /// Use [`composite`][Self::composite] when the element is a composite whose
    /// components carry their own UN/EDIFACT identifiers.
    ///
    /// # Panics
    ///
    /// Panics if `position == 0`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ElementRef, Status};
    ///
    /// const BGM_1001: ElementRef = ElementRef::new(1, "1001", Status::Mandatory, 1);
    /// ```
    #[must_use]
    pub const fn new(
        position: u8,
        data_element: &'static str,
        status: Status,
        max_repeat: u8,
    ) -> Self {
        assert!(
            position != 0,
            "ElementRef position must be >= 1 (one-based)"
        );
        Self {
            position,
            data_element,
            status,
            max_repeat,
            components: &[],
            repr: None,
            repr_from_v4: None,
        }
    }

    /// Construct an `ElementRef` for a composite data element with named components.
    ///
    /// Declaring components is what lets code-addressed access reach *inside* a
    /// composite: `value_by_code(&DTM, "2380")` resolves to element 1,
    /// component 2 without the caller counting positions.  Declared components
    /// also make the mandatory-component check in [`DirectoryValidator`] active
    /// for this element.
    ///
    /// # Panics
    ///
    /// Panics if `position == 0`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ComponentRef, ElementRef, Status};
    ///
    /// static C507: &[ComponentRef] = &[
    ///     ComponentRef::new(1, "2005", Status::Mandatory),
    ///     ComponentRef::new(2, "2380", Status::Conditional),
    ///     ComponentRef::new(3, "2379", Status::Conditional),
    /// ];
    /// const DTM_C507: ElementRef =
    ///     ElementRef::composite(1, "C507", Status::Mandatory, 1, C507);
    /// ```
    #[must_use]
    pub const fn composite(
        position: u8,
        data_element: &'static str,
        status: Status,
        max_repeat: u8,
        components: &'static [ComponentRef],
    ) -> Self {
        assert!(
            position != 0,
            "ElementRef position must be >= 1 (one-based)"
        );
        Self {
            position,
            data_element,
            status,
            max_repeat,
            components,
            repr: None,
            repr_from_v4: None,
        }
    }

    /// One-based element position in the segment definition.
    #[must_use]
    #[inline]
    pub const fn position(&self) -> u8 {
        self.position
    }

    /// UN/EDIFACT data element identifier.
    #[must_use]
    #[inline]
    pub const fn data_element(&self) -> &'static str {
        self.data_element
    }

    /// Requirement status of the element.
    #[must_use]
    #[inline]
    pub const fn status(&self) -> Status {
        self.status
    }

    /// Maximum repetition count for this element.
    #[must_use]
    #[inline]
    pub const fn max_repeat(&self) -> u8 {
        self.max_repeat
    }

    /// Component definitions; empty when this is a simple data element.
    #[must_use]
    #[inline]
    pub const fn components(&self) -> &'static [ComponentRef] {
        self.components
    }

    /// Attach the directory's representation for a simple data element.
    ///
    /// A composite carries its representations on the components instead, so
    /// this is ignored when `components` is non-empty.
    #[must_use]
    pub const fn with_repr(mut self, repr: Repr) -> Self {
        self.repr = Some(repr);
        self
    }

    /// The declared representation, if the definition states one.
    #[must_use]
    #[inline]
    pub const fn repr(&self) -> Option<Repr> {
        self.repr
    }
    /// Declare a representation that changed between syntax versions.
    ///
    /// The service directory has exactly one such position: `S004` DE 0017, the
    /// date of preparation. Version 3 transfers `YYMMDD` (`n6`); version 4
    /// widened it to `CCYYMMDD` (`n8`) to be year-2000 correct. It is the only
    /// place where version 4 is *not* a superset of version 3.
    ///
    /// Declaring both keeps each version checked exactly. Collapsing them into
    /// `n..8` would have accepted a six-digit date in a version 4 interchange
    /// and a seven-digit one in either — validating neither version correctly.
    ///
    /// The syntax version comes from `UNB` S001 DE 0002. When it cannot be
    /// determined — validating a bare message window, say — **both** are
    /// accepted, because guessing would reject conformant data.
    #[must_use]
    pub const fn with_repr_by_syntax_version(mut self, up_to_v3: Repr, from_v4: Repr) -> Self {
        self.repr = Some(up_to_v3);
        self.repr_from_v4 = Some(from_v4);
        self
    }

    /// The representation used from syntax version 4 onward, when it differs.
    #[must_use]
    #[inline]
    pub const fn repr_from_v4(&self) -> Option<Repr> {
        self.repr_from_v4
    }
}

/// Definition of an EDIFACT segment (tag + element structure).
///
/// Construct with [`SegmentDefinition::new`] rather than a struct literal, so
/// that future fields (max repeat, description, …) are not a breaking change.
#[derive(Debug)]
#[non_exhaustive]
pub struct SegmentDefinition {
    /// Segment tag.
    pub tag: &'static str,
    /// Human-readable segment name.
    pub name: &'static str,
    /// Ordered element definitions.
    pub elements: &'static [ElementRef],
}

/// Byte-wise string equality usable in a `const` context.
///
/// `str::eq` is not `const`, and code resolution has to run at compile time so
/// that a mistyped data element identifier in `#[edifact(element = "3055")]`
/// fails the build rather than reading the wrong slot at runtime.
const fn const_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The resolved position of a UN/EDIFACT data element within a segment.
///
/// Produced by [`SegmentLayout::resolve_code`] and consumed by the `*_at`
/// accessors on [`crate::Segment`], [`crate::BorrowedSegment`] and
/// [`crate::OwnedSegment`].
///
/// Both indices are **zero-based**, matching the positional accessors — the
/// one-based positions used in directory definitions are converted during
/// resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementPath {
    /// Zero-based index of the data element within the segment.
    pub element: usize,
    /// Zero-based index of the component within a composite.
    ///
    /// `None` when the code names the data element itself (a simple element, or
    /// a composite addressed as a whole).  Value lookups treat `None` as
    /// component 0, which is the first — and for a simple element, only —
    /// component.
    pub component: Option<usize>,
}

impl ElementPath {
    /// Path to a whole data element.
    #[must_use]
    #[inline]
    pub const fn element(element: usize) -> Self {
        Self {
            element,
            component: None,
        }
    }

    /// Path to a component within a composite data element.
    #[must_use]
    #[inline]
    pub const fn component(element: usize, component: usize) -> Self {
        Self {
            element,
            component: Some(component),
        }
    }

    /// Zero-based component index, treating "whole element" as component 0.
    #[must_use]
    #[inline]
    pub const fn component_index(&self) -> usize {
        match self.component {
            Some(c) => c,
            None => 0,
        }
    }
}

/// Directory metadata that maps UN/EDIFACT data element identifiers to positions.
///
/// Implemented by [`SegmentDefinition`] (compile-time tables) and
/// [`OwnedSegmentDef`] (runtime-loaded definitions), so the same code-addressed
/// accessors work against either source.
///
/// # Example
///
/// ```rust
/// use edifact_rs::{ElementRef, SegmentDefinition, SegmentLayout, Status};
///
/// static BGM_ELEMENTS: &[ElementRef] = &[
///     ElementRef::new(1, "C002", Status::Conditional, 1),
///     ElementRef::new(2, "C106", Status::Conditional, 1),
///     ElementRef::new(3, "1225", Status::Conditional, 1),
/// ];
/// static BGM: SegmentDefinition =
///     SegmentDefinition::new("BGM", "Beginning of message", BGM_ELEMENTS);
///
/// let path = BGM.resolve_code("1225")?;
/// assert_eq!(path.element, 2);
/// assert!(BGM.resolve_code("9999").is_err());
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub trait SegmentLayout {
    /// The segment tag this layout describes (e.g. `"NAD"`).
    fn layout_tag(&self) -> &str;

    /// Resolve a UN/EDIFACT data element identifier to a position.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::UnknownDataElement`] when the identifier does not
    /// appear in this definition, and [`EdifactError::AmbiguousDataElement`]
    /// when it appears at more than one position.
    fn resolve_code(&self, data_element: &str) -> Result<ElementPath, EdifactError>;

    /// Every position this layout declares, flattened and in order.
    ///
    /// Implemented by both the compile-time and runtime definitions, so tooling
    /// can walk a layout without knowing which one it holds.
    fn slots(&self) -> Vec<LayoutSlot>;

    /// Check this layout against real messages and report what does not line up.
    ///
    /// Hand-authoring a segment definition has a silent failure mode: a layout
    /// that disagrees with the wire resolves `value_by_code` to the *wrong
    /// component*, returns a plausible value, and every test still passes. There
    /// is no way to notice from inside the program — the definition is the only
    /// thing that says what the positions mean.
    ///
    /// Pointing the definition at a corpus is what breaks that circle. Three
    /// kinds of finding come back, and the third is the one that matters most:
    ///
    /// | Finding | Means |
    /// |---|---|
    /// | [`UndeclaredElement`][LayoutFinding::UndeclaredElement] / [`UndeclaredComponent`][LayoutFinding::UndeclaredComponent] | The wire carries a value the layout has no slot for — the layout is **wrong**. |
    /// | [`MandatoryNeverPopulated`][LayoutFinding::MandatoryNeverPopulated] | A slot the layout calls mandatory is empty everywhere — the status or the position is **wrong**. |
    /// | [`NeverObserved`][LayoutFinding::NeverObserved] | Nothing in the corpus reaches this slot, so **the corpus cannot confirm it**. |
    ///
    /// `NeverObserved` is not a defect. It is the honest answer to "does my
    /// definition match the directory?" when the fixtures are too thin to tell,
    /// and it names exactly which positions to go and check by hand.
    ///
    /// Only segments whose tag matches [`layout_tag`][Self::layout_tag] are
    /// examined; the rest of the slice is ignored, so a whole interchange can be
    /// passed in as-is.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{ComponentRef, ElementRef, SegmentDefinition, SegmentLayout, Status, from_bytes};
    ///
    /// // A hand-authored C507 that stops one component short of the directory.
    /// static C507: &[ComponentRef] = &[
    ///     ComponentRef::new(1, "2005", Status::Mandatory),
    ///     ComponentRef::new(2, "2380", Status::Conditional),
    /// ];
    /// static DTM_ELEMENTS: &[ElementRef] =
    ///     &[ElementRef::composite(1, "C507", Status::Mandatory, 1, C507)];
    /// static DTM: SegmentDefinition =
    ///     SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);
    ///
    /// let corpus: Vec<_> = from_bytes(b"DTM+137:20260101:102'").collect::<Result<Vec<_>, _>>()?;
    /// let audit = DTM.audit(&corpus);
    ///
    /// // The format qualifier `102` has nowhere to go — the layout is short.
    /// assert!(audit.has_contradictions());
    /// assert_eq!(audit.segments_examined(), 1);
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    fn audit(&self, segments: &[crate::Segment<'_>]) -> LayoutAudit {
        audit_layout(self.layout_tag(), &self.slots(), segments)
    }
}

/// One declared position in a [`SegmentLayout`], flattened.
///
/// Produced by [`SegmentLayout::slots`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSlot {
    /// Zero-based data element index within the segment.
    pub element: usize,
    /// Zero-based component index, or `None` for a simple data element.
    pub component: Option<usize>,
    /// The UN/EDIFACT identifier declared at this position.
    pub data_element: String,
    /// Whether the layout calls this position mandatory.
    pub status: Status,
    /// The status of the **enclosing data element**.
    ///
    /// Equal to `status` for a simple data element. For a component it is the
    /// composite's own status, which is what decides whether a mandatory
    /// component is actually required: ISO 9735-1 §8.6 makes it mandatory "if
    /// the composite data element is present", not unconditionally.
    pub element_status: Status,
}

impl LayoutSlot {
    /// Component index treating a simple data element as component 0.
    #[must_use]
    pub fn component_index(&self) -> usize {
        self.component.unwrap_or(0)
    }
}

/// One way a layout and a corpus disagree — or fail to inform each other.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayoutFinding {
    /// A segment carried a populated data element beyond the last one declared.
    UndeclaredElement {
        /// Zero-based index of the undeclared element.
        element: usize,
        /// Byte span of the segment that carried it.
        span: crate::Span,
    },
    /// An element carried a populated component beyond the last one declared.
    UndeclaredComponent {
        /// Zero-based element index.
        element: usize,
        /// Zero-based index of the undeclared component.
        component: usize,
        /// Byte span of the segment that carried it.
        span: crate::Span,
    },
    /// A position the layout calls mandatory was empty in every segment.
    MandatoryNeverPopulated {
        /// The declared position.
        slot: LayoutSlot,
    },
    /// No segment in the corpus populated this position.
    ///
    /// Evidence of nothing rather than evidence of a fault: the corpus is too
    /// thin to confirm or refute the slot.
    NeverObserved {
        /// The declared position.
        slot: LayoutSlot,
    },
}

/// The result of checking a [`SegmentLayout`] against a corpus.
///
/// See [`SegmentLayout::audit`].
#[derive(Debug, Clone, Default)]
pub struct LayoutAudit {
    tag: String,
    segments_examined: usize,
    findings: Vec<LayoutFinding>,
}

impl LayoutAudit {
    /// The segment tag that was audited.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// How many segments in the corpus carried that tag.
    ///
    /// Zero means the audit proved nothing at all — worth asserting on.
    #[must_use]
    pub fn segments_examined(&self) -> usize {
        self.segments_examined
    }

    /// Every finding, in declaration order.
    #[must_use]
    pub fn findings(&self) -> &[LayoutFinding] {
        &self.findings
    }

    /// Findings that mean the layout is **wrong**, as opposed to unconfirmed.
    ///
    /// [`NeverObserved`][LayoutFinding::NeverObserved] is excluded: a corpus
    /// that never reaches a slot says nothing about whether the slot is right.
    pub fn contradictions(&self) -> impl Iterator<Item = &LayoutFinding> {
        self.findings
            .iter()
            .filter(|f| !matches!(f, LayoutFinding::NeverObserved { .. }))
    }

    /// `true` when the corpus contradicts the layout.
    ///
    /// This is the assertion to put in a test: it fails on a layout the wire
    /// disproves, and stays quiet about slots the fixtures simply never exercise.
    #[must_use]
    pub fn has_contradictions(&self) -> bool {
        self.contradictions().next().is_some()
    }

    /// Positions the corpus never reached, in declaration order.
    ///
    /// Each one is a slot to verify against the directory by hand — or a gap to
    /// fill with a fixture.
    pub fn unconfirmed(&self) -> impl Iterator<Item = &LayoutSlot> {
        self.findings.iter().filter_map(|f| match f {
            LayoutFinding::NeverObserved { slot } => Some(slot),
            _ => None,
        })
    }
}

impl std::fmt::Display for LayoutAudit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{}: {} segment(s) examined, {} contradiction(s), {} unconfirmed slot(s)",
            self.tag,
            self.segments_examined,
            self.contradictions().count(),
            self.unconfirmed().count(),
        )?;
        for finding in &self.findings {
            match finding {
                LayoutFinding::UndeclaredElement { element, span } => writeln!(
                    f,
                    "  element {element} is populated at bytes {span} but the layout declares no such element",
                )?,
                LayoutFinding::UndeclaredComponent {
                    element,
                    component,
                    span,
                } => writeln!(
                    f,
                    "  element {element} component {component} is populated at bytes {span} but the layout declares no such component",
                )?,
                LayoutFinding::MandatoryNeverPopulated { slot } => writeln!(
                    f,
                    "  {} is declared mandatory but is empty in every segment",
                    describe_slot(slot),
                )?,
                LayoutFinding::NeverObserved { slot } => writeln!(
                    f,
                    "  {} was never populated — this corpus cannot confirm it",
                    describe_slot(slot),
                )?,
            }
        }
        Ok(())
    }
}

fn describe_slot(slot: &LayoutSlot) -> String {
    match slot.component {
        Some(component) => format!(
            "DE {} (element {}, component {component})",
            slot.data_element, slot.element
        ),
        None => format!("DE {} (element {})", slot.data_element, slot.element),
    }
}

/// Audit every layout a corpus actually exercises, in one call.
///
/// [`SegmentLayout::audit`] answers for one segment. A hand-authored directory
/// has dozens, and the question worth asking is about all of them at once:
/// *which of my definitions does this corpus disprove, and which can it not
/// speak to?*
///
/// Only tags present in the corpus are audited — a definition the fixtures never
/// exercise would produce nothing but `NeverObserved` noise and drown the
/// findings that matter. Ask [`SegmentLayout::audit`] directly for those.
///
/// Results come back in the order the tags first appear, so the report reads in
/// message order.
///
/// # Example
///
/// ```
/// use edifact_rs::{audit_directory, from_bytes, service};
///
/// let corpus: Vec<_> = from_bytes(
///     b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+M1+ORDERS:D:96A:UN'UNT+2+M1'UNZ+1+IC1'",
/// )
/// .collect::<Result<Vec<_>, _>>()?;
///
/// let audits = audit_directory(service::lookup, &corpus);
///
/// // One audit per distinct tag the corpus contains.
/// assert_eq!(audits.len(), 4);
/// // The shipped service tables are not disproved by conformant input.
/// assert!(audits.iter().all(|a| !a.has_contradictions()));
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub fn audit_directory<'a, L, F>(lookup: F, segments: &[crate::Segment<'_>]) -> Vec<LayoutAudit>
where
    L: SegmentLayout + ?Sized + 'a,
    F: Fn(&str) -> Option<&'a L>,
{
    let mut seen: Vec<&str> = Vec::new();
    for segment in segments {
        if !seen.contains(&segment.tag) {
            seen.push(segment.tag);
        }
    }
    seen.into_iter()
        .filter_map(|tag| lookup(tag).map(|layout| layout.audit(segments)))
        .collect()
}

/// Shared implementation behind [`SegmentLayout::audit`].
fn audit_layout(tag: &str, slots: &[LayoutSlot], segments: &[crate::Segment<'_>]) -> LayoutAudit {
    let mut audit = LayoutAudit {
        tag: tag.to_owned(),
        segments_examined: 0,
        findings: Vec::new(),
    };

    // Highest declared index per element, so "beyond the layout" is decidable.
    let declared_elements = slots.iter().map(|s| s.element + 1).max().unwrap_or(0);
    let mut declared_components: Vec<usize> = vec![0; declared_elements];
    for slot in slots {
        let width = slot.component_index() + 1;
        if width > declared_components[slot.element] {
            declared_components[slot.element] = width;
        }
    }

    let mut populated: Vec<Vec<bool>> = declared_components
        .iter()
        .map(|width| vec![false; *width])
        .collect();

    for segment in segments.iter().filter(|s| s.tag == tag) {
        audit.segments_examined += 1;
        for (element_index, element) in segment.elements.iter().enumerate() {
            // Every occurrence counts: a repeating element populates the same
            // declared positions each time (ISO 9735-1 §8.6).
            for occurrence in element.repetitions() {
                for (component_index, (value, _)) in occurrence.iter().enumerate() {
                    // A trailing empty component is how EDIFACT spells "absent"
                    // (§8.7.2), so only a populated one is evidence of anything.
                    if value.is_empty() {
                        continue;
                    }
                    if element_index >= declared_elements {
                        push_once(
                            &mut audit.findings,
                            LayoutFinding::UndeclaredElement {
                                element: element_index,
                                span: segment.span,
                            },
                        );
                        continue;
                    }
                    if component_index >= declared_components[element_index] {
                        push_once(
                            &mut audit.findings,
                            LayoutFinding::UndeclaredComponent {
                                element: element_index,
                                component: component_index,
                                span: segment.span,
                            },
                        );
                        continue;
                    }
                    populated[element_index][component_index] = true;
                }
            }
        }
    }

    // Whether any position of each element was populated — which is exactly
    // ISO 9735-1 §8.1's definition of a composite being "present".
    let element_populated: Vec<bool> = populated
        .iter()
        .map(|components| components.iter().any(|seen| *seen))
        .collect();

    for slot in slots {
        if populated[slot.element][slot.component_index()] {
            continue;
        }
        // §8.6: "A mandatory component data element in a composite data element
        // shall be present **if the composite data element is present**."  A
        // conditional composite that the corpus never carries therefore says
        // nothing about its mandatory components — reporting them as violations
        // would condemn every optional composite in the definition.
        let required_here = slot.status == Status::Mandatory
            && (slot.component.is_none()
                || slot.element_status == Status::Mandatory
                || element_populated[slot.element]);
        audit.findings.push(if required_here {
            LayoutFinding::MandatoryNeverPopulated { slot: slot.clone() }
        } else {
            LayoutFinding::NeverObserved { slot: slot.clone() }
        });
    }

    audit
}

/// Record a finding unless an equivalent one is already present.
///
/// A corpus of 360 fixtures would otherwise report the same undeclared
/// component 360 times, burying every other finding.
fn push_once(findings: &mut Vec<LayoutFinding>, finding: LayoutFinding) {
    let duplicate = findings.iter().any(|existing| match (existing, &finding) {
        (
            LayoutFinding::UndeclaredElement { element: a, .. },
            LayoutFinding::UndeclaredElement { element: b, .. },
        ) => a == b,
        (
            LayoutFinding::UndeclaredComponent {
                element: a,
                component: c,
                ..
            },
            LayoutFinding::UndeclaredComponent {
                element: b,
                component: d,
                ..
            },
        ) => a == b && c == d,
        _ => false,
    });
    if !duplicate {
        findings.push(finding);
    }
}

impl SegmentDefinition {
    /// Create a segment definition.
    ///
    /// `const` so directory tables can still be built at compile time despite
    /// the `#[non_exhaustive]` attribute blocking external struct literals.
    #[must_use]
    pub const fn new(
        tag: &'static str,
        name: &'static str,
        elements: &'static [ElementRef],
    ) -> Self {
        Self {
            tag,
            name,
            elements,
        }
    }

    /// Number of positions in this definition that carry `data_element`.
    ///
    /// `0` means unknown, `1` means unambiguously addressable, and anything
    /// larger means the identifier is repeated and cannot be code-addressed.
    /// `const`, so a derive macro can assert on it at compile time.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use edifact_rs::{ElementRef, SegmentDefinition, Status};
    /// # static E: &[ElementRef] = &[ElementRef::new(1, "3035", Status::Mandatory, 1)];
    /// static NAD: SegmentDefinition = SegmentDefinition::new("NAD", "Name and address", E);
    /// const _: () = assert!(NAD.code_positions("3035") == 1);
    /// ```
    #[must_use]
    pub const fn code_positions(&self, data_element: &str) -> usize {
        let mut hits = 0;
        let mut i = 0;
        while i < self.elements.len() {
            let el = &self.elements[i];
            if const_str_eq(el.data_element, data_element) {
                hits += 1;
            }
            let mut c = 0;
            while c < el.components.len() {
                if const_str_eq(el.components[c].data_element, data_element) {
                    hits += 1;
                }
                c += 1;
            }
            i += 1;
        }
        hits
    }

    /// Zero-based element index for `data_element`, resolved at compile time.
    ///
    /// # Panics
    ///
    /// Panics when the identifier is unknown or appears at more than one
    /// position.  In a `const` context — which is how the derive macro uses it —
    /// that panic is a **compile error**, so a mistyped identifier can never
    /// reach runtime.  Guard with [`code_positions`][Self::code_positions] for a
    /// message that names the offending field.
    #[must_use]
    pub const fn element_slot(&self, data_element: &str) -> usize {
        // Two asserts rather than one: a const panic message cannot be
        // formatted, so naming the identifier is impossible — but saying which
        // of the two problems occurred is not, and it is the part that decides
        // what the author has to change.
        assert!(
            self.code_positions(data_element) != 0,
            "this segment definition declares no such data element identifier — check it against the directory"
        );
        assert!(
            self.code_positions(data_element) == 1,
            "this data element identifier is declared at more than one position; address it positionally, or declare the repeat with ComponentRef::repeated"
        );
        let mut i = 0;
        while i < self.elements.len() {
            let el = &self.elements[i];
            if const_str_eq(el.data_element, data_element) {
                return el.position as usize - 1;
            }
            let mut c = 0;
            while c < el.components.len() {
                if const_str_eq(el.components[c].data_element, data_element) {
                    return el.position as usize - 1;
                }
                c += 1;
            }
            i += 1;
        }
        unreachable!()
    }

    /// Zero-based component index for `data_element`, resolved at compile time.
    ///
    /// Returns `0` when the identifier names a data element rather than a
    /// component inside a composite — component 0 is the first (and for a simple
    /// element, only) component, so the same accessor works for both shapes.
    ///
    /// # Panics
    ///
    /// Panics when the identifier is unknown or appears at more than one
    /// position; see [`element_slot`][Self::element_slot].
    #[must_use]
    pub const fn component_slot(&self, data_element: &str) -> usize {
        assert!(
            self.code_positions(data_element) != 0,
            "this segment definition declares no such data element identifier — check it against the directory"
        );
        assert!(
            self.code_positions(data_element) == 1,
            "this data element identifier is declared at more than one position; address it positionally, or declare the repeat with ComponentRef::repeated"
        );
        let mut i = 0;
        while i < self.elements.len() {
            let el = &self.elements[i];
            if const_str_eq(el.data_element, data_element) {
                return 0;
            }
            let mut c = 0;
            while c < el.components.len() {
                if const_str_eq(el.components[c].data_element, data_element) {
                    return el.components[c].position as usize - 1;
                }
                c += 1;
            }
            i += 1;
        }
        unreachable!()
    }

    /// `true` when `data_element` names a component *inside* a composite rather
    /// than a data element of the segment.
    ///
    /// Lets a caller — the derive macro, in practice — pick the right
    /// "missing required" error variant without a second lookup:
    /// [`EdifactError::MissingRequiredComponent`] rather than
    /// [`EdifactError::MissingRequiredElement`]. `component_slot` alone cannot
    /// answer this, because a code naming the *first* component of a composite
    /// also resolves to component index 0.
    ///
    /// Returns `false` for an unknown identifier; pair with
    /// [`code_positions`][Self::code_positions] when that case matters.
    #[must_use]
    pub const fn code_is_component(&self, data_element: &str) -> bool {
        let mut i = 0;
        while i < self.elements.len() {
            let el = &self.elements[i];
            let mut c = 0;
            while c < el.components.len() {
                if const_str_eq(el.components[c].data_element, data_element) {
                    return true;
                }
                c += 1;
            }
            i += 1;
        }
        false
    }
}

impl SegmentLayout for SegmentDefinition {
    #[inline]
    fn layout_tag(&self) -> &str {
        self.tag
    }

    fn resolve_code(&self, data_element: &str) -> Result<ElementPath, EdifactError> {
        // One pass, not four: this runs per lookup on hot validation paths, and
        // composing the `const` helpers would rescan the table for each of the
        // count, the element index, and the component index.
        let mut hits = 0usize;
        let mut found = None;
        for el in self.elements {
            if el.data_element == data_element {
                hits += 1;
                found.get_or_insert(ElementPath::element(el.position as usize - 1));
            }
            for comp in el.components {
                if comp.data_element == data_element {
                    hits += 1;
                    found.get_or_insert(ElementPath::component(
                        el.position as usize - 1,
                        comp.position as usize - 1,
                    ));
                }
            }
        }
        resolve_outcome(self.tag, data_element, hits, found)
    }

    fn slots(&self) -> Vec<LayoutSlot> {
        let mut out = Vec::new();
        for element in self.elements {
            if element.components.is_empty() {
                out.push(LayoutSlot {
                    element: element.position as usize - 1,
                    component: None,
                    data_element: element.data_element.to_owned(),
                    status: element.status,
                    element_status: element.status,
                });
                continue;
            }
            for component in element.components {
                out.push(LayoutSlot {
                    element: element.position as usize - 1,
                    component: Some(component.position as usize - 1),
                    data_element: component.data_element.to_owned(),
                    status: component.status,
                    element_status: element.status,
                });
            }
        }
        out
    }
}

/// Turn a resolution scan's `(hit count, first match)` into a `Result`.
///
/// Shared by both [`SegmentLayout`] impls so the static and runtime tables
/// cannot drift on which condition maps to which error.
fn resolve_outcome(
    tag: &str,
    data_element: &str,
    hits: usize,
    found: Option<ElementPath>,
) -> Result<ElementPath, EdifactError> {
    match (hits, found) {
        (1, Some(path)) => Ok(path),
        (0, _) => Err(EdifactError::UnknownDataElement {
            tag: tag.to_owned(),
            data_element: data_element.to_owned(),
        }),
        _ => Err(EdifactError::AmbiguousDataElement {
            tag: tag.to_owned(),
            data_element: data_element.to_owned(),
        }),
    }
}

/// Owned runtime equivalent of [`ElementRef`].
///
/// Used by [`DirectoryValidatorBuilder`] and [`DirectoryValidator::from_owned_definitions`]
/// to construct validators from data that is not available at compile time (e.g. loaded
/// from JSON or a database at startup).
///
/// Use [`OwnedElementRef::new_unchecked`] for compile-time-known positions (panics on invalid
/// input, no error handling noise) or [`OwnedElementRef::try_new`] when the position
/// comes from an external source and you need a `Result`. Fields are private to prevent
/// bypassing the position invariant through struct-literal syntax.
#[derive(Debug, Clone)]
pub struct OwnedElementRef {
    /// One-based element position.
    position: u8,
    /// UN/EDIFACT data element identifier.
    data_element: String,
    /// Requirement status.
    status: Status,
    /// Maximum repetition count.
    max_repeat: u8,
    /// The directory's representation for a *simple* element.
    repr: Option<Repr>,
    /// The representation from syntax version 4 onward, where it differs.
    repr_from_v4: Option<Repr>,
    /// Component definitions when this element is a composite; empty for a
    /// simple data element.
    components: Vec<OwnedComponentRef>,
}

/// Owned runtime equivalent of [`ComponentRef`].
///
/// Attach these to an [`OwnedElementRef`] with
/// [`OwnedElementRef::with_components`] so that runtime-loaded definitions
/// support code-addressed access into composites, exactly like compile-time
/// [`SegmentDefinition`] tables do.
#[derive(Debug, Clone)]
pub struct OwnedComponentRef {
    /// One-based position of the first slot this component occupies.
    position: u8,
    /// UN/EDIFACT component data element identifier.
    data_element: String,
    /// Requirement status.
    status: Status,
    /// The directory's representation, when the definition states one.
    repr: Option<Repr>,
    /// The representation from syntax version 4 onward, where it differs.
    repr_from_v4: Option<Repr>,
    /// How many consecutive slots this component occupies.
    repeat_count: u8,
}

impl OwnedComponentRef {
    /// Construct an owned component reference.
    ///
    /// # Panics
    ///
    /// Panics if `position` is `0` (positions are one-based).
    pub fn new_unchecked(position: u8, data_element: String, status: Status) -> Self {
        assert!(
            position != 0,
            "OwnedComponentRef::new_unchecked: position must be >= 1 (one-based), got 0"
        );
        Self {
            position,
            data_element,
            status,
            repeat_count: 1,
            repr: None,
            repr_from_v4: None,
        }
    }

    /// Runtime counterpart of [`ComponentRef::repeated`].
    ///
    /// # Panics
    ///
    /// Panics if `position == 0` or `repeat_count == 0`.
    #[must_use]
    pub fn repeated(position: u8, data_element: String, status: Status, repeat_count: u8) -> Self {
        assert!(
            position != 0,
            "OwnedComponentRef::repeated: position must be >= 1 (one-based), got 0"
        );
        assert!(
            repeat_count != 0,
            "OwnedComponentRef::repeated: repeat_count must be >= 1"
        );
        Self {
            position,
            data_element,
            status,
            repeat_count,
            repr: None,
            repr_from_v4: None,
        }
    }

    /// How many consecutive slots this component occupies.
    #[inline]
    #[must_use]
    pub fn repeat_count(&self) -> u8 {
        self.repeat_count
    }

    /// Construct an owned component reference, returning an error for position `0`.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::InvalidElementPosition`] if `position` is `0`.
    pub fn try_new(
        position: u8,
        data_element: String,
        status: Status,
    ) -> Result<Self, EdifactError> {
        if position == 0 {
            return Err(EdifactError::InvalidElementPosition);
        }
        Ok(Self {
            position,
            data_element,
            status,
            repeat_count: 1,
            repr: None,
            repr_from_v4: None,
        })
    }

    /// One-based component position (always >= 1).
    #[inline]
    pub fn position(&self) -> u8 {
        self.position
    }

    /// UN/EDIFACT component data element identifier.
    #[inline]
    pub fn data_element(&self) -> &str {
        &self.data_element
    }

    /// Requirement status of this component.
    #[inline]
    pub fn status(&self) -> Status {
        self.status
    }

    /// Attach the directory's representation, e.g. `an..35`.
    #[must_use]
    pub fn with_repr(mut self, repr: Repr) -> Self {
        self.repr = Some(repr);
        self
    }

    /// The declared representation, if the definition states one.
    #[inline]
    #[must_use]
    pub fn repr(&self) -> Option<Repr> {
        self.repr
    }

    /// Declare a representation that changed between syntax versions.
    ///
    /// See [`ComponentRef::with_repr_by_syntax_version`].
    #[must_use]
    pub fn with_repr_by_syntax_version(mut self, up_to_v3: Repr, from_v4: Repr) -> Self {
        self.repr = Some(up_to_v3);
        self.repr_from_v4 = Some(from_v4);
        self
    }

    /// The representation used from syntax version 4 onward, when it differs.
    #[inline]
    #[must_use]
    pub fn repr_from_v4(&self) -> Option<Repr> {
        self.repr_from_v4
    }
}

/// Owned runtime equivalent of [`SegmentDefinition`].
///
/// Used by [`DirectoryValidatorBuilder`] and [`DirectoryValidator::from_owned_definitions`].
///
/// Use [`OwnedSegmentDef::new_unchecked`] for compile-time-known tags (panics on invalid input,
/// no error handling noise) or [`OwnedSegmentDef::try_new`] when the tag comes from
/// an external source and you need a `Result`. Fields are private to prevent bypassing
/// the tag invariant through struct-literal syntax.
#[derive(Debug, Clone)]
pub struct OwnedSegmentDef {
    /// Segment tag (e.g. `"BGM"`).
    tag: String,
    /// Human-readable segment name.
    name: String,
    /// Ordered element definitions.
    elements: Vec<OwnedElementRef>,
}

impl OwnedSegmentDef {
    /// Construct an owned segment definition.
    ///
    /// This is the ergonomic constructor for compile-time-known tags (e.g.
    /// `"BGM"`, `"UNH"`).  It panics immediately on invalid input so that
    /// call sites with literal tag strings require no `.unwrap()` / `.expect()`
    /// boilerplate.
    ///
    /// Use [`try_new`][Self::try_new] instead when the tag originates from an
    /// external source (user input, config file, database) and you need a
    /// `Result` to propagate errors gracefully.
    ///
    /// # Panics
    ///
    /// Panics if `tag` is not exactly three ASCII uppercase letters.
    pub fn new_unchecked(tag: String, name: String, elements: Vec<OwnedElementRef>) -> Self {
        assert!(
            tag.len() == 3 && tag.bytes().all(|b| b.is_ascii_uppercase()),
            "OwnedSegmentDef::new_unchecked: tag must be exactly three ASCII uppercase letters, got {tag:?}"
        );
        Self {
            tag,
            name,
            elements,
        }
    }

    /// Construct an owned segment definition, returning an error for invalid tags.
    ///
    /// Prefer this over [`new_unchecked`][Self::new_unchecked] when the tag comes from an external
    /// source (user input, config file, database) and you want to handle the
    /// error without panicking.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::InvalidSegmentTag`] if `tag` is not exactly three
    /// ASCII uppercase letters.
    pub fn try_new(
        tag: String,
        name: String,
        elements: Vec<OwnedElementRef>,
    ) -> Result<Self, EdifactError> {
        if tag.len() != 3 || !tag.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(EdifactError::InvalidSegmentTag(tag));
        }
        Ok(Self {
            tag,
            name,
            elements,
        })
    }

    /// Segment tag (e.g. `"BGM"`).
    #[inline]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Human-readable segment name.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Element definitions for this segment.
    #[inline]
    pub fn elements(&self) -> &[OwnedElementRef] {
        &self.elements
    }

    /// Number of positions in this definition that carry `data_element`.
    ///
    /// Runtime counterpart of [`SegmentDefinition::code_positions`].
    #[must_use]
    pub fn code_positions(&self, data_element: &str) -> usize {
        self.elements
            .iter()
            .map(|el| {
                usize::from(el.data_element == data_element)
                    + el.components
                        .iter()
                        .filter(|c| c.data_element == data_element)
                        .count()
            })
            .sum()
    }
}

impl SegmentLayout for OwnedSegmentDef {
    #[inline]
    fn layout_tag(&self) -> &str {
        &self.tag
    }

    fn resolve_code(&self, data_element: &str) -> Result<ElementPath, EdifactError> {
        let mut hits = 0usize;
        let mut found = None;
        for el in &self.elements {
            if el.data_element == data_element {
                hits += 1;
                found.get_or_insert(ElementPath::element(el.position as usize - 1));
            }
            for comp in &el.components {
                if comp.data_element == data_element {
                    hits += 1;
                    found.get_or_insert(ElementPath::component(
                        el.position as usize - 1,
                        comp.position as usize - 1,
                    ));
                }
            }
        }
        resolve_outcome(&self.tag, data_element, hits, found)
    }

    fn slots(&self) -> Vec<LayoutSlot> {
        let mut out = Vec::new();
        for element in &self.elements {
            if element.components.is_empty() {
                out.push(LayoutSlot {
                    element: element.position as usize - 1,
                    component: None,
                    data_element: element.data_element.clone(),
                    status: element.status,
                    element_status: element.status,
                });
                continue;
            }
            for component in &element.components {
                out.push(LayoutSlot {
                    element: element.position as usize - 1,
                    component: Some(component.position as usize - 1),
                    data_element: component.data_element.clone(),
                    status: component.status,
                    element_status: element.status,
                });
            }
        }
        out
    }
}

impl OwnedElementRef {
    /// Construct an owned element reference.
    ///
    /// This is the ergonomic constructor for compile-time-known positions.
    /// It panics immediately on invalid input so that call sites with literal
    /// position numbers require no `.unwrap()` / `.expect()` boilerplate.
    ///
    /// Use [`try_new`][Self::try_new] instead when the position originates from
    /// an external source (user input, config file, database) and you need a
    /// `Result` to propagate errors gracefully.
    ///
    /// # Panics
    ///
    /// Panics if `position` is `0` (positions are one-based).
    pub fn new_unchecked(
        position: u8,
        data_element: String,
        status: Status,
        max_repeat: u8,
    ) -> Self {
        assert!(
            position != 0,
            "OwnedElementRef::new_unchecked: position must be >= 1 (one-based), got 0"
        );
        Self {
            position,
            data_element,
            status,
            max_repeat,
            repr: None,
            repr_from_v4: None,
            components: Vec::new(),
        }
    }

    /// Construct an owned element reference, returning an error for position `0`.
    ///
    /// Prefer this over [`new_unchecked`][Self::new_unchecked] when the position comes from an
    /// external source (user input, config file, database) and you want to
    /// handle the error without panicking.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::InvalidElementPosition`] if `position` is `0`.
    pub fn try_new(
        position: u8,
        data_element: String,
        status: Status,
        max_repeat: u8,
    ) -> Result<Self, EdifactError> {
        if position == 0 {
            return Err(EdifactError::InvalidElementPosition);
        }
        Ok(Self {
            position,
            data_element,
            status,
            max_repeat,
            repr: None,
            repr_from_v4: None,
            components: Vec::new(),
        })
    }

    /// Attach component definitions, marking this element as a composite.
    ///
    /// Declared components make code-addressed access resolve *into* the
    /// composite and activate the mandatory-component check in
    /// [`DirectoryValidator`].
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{OwnedComponentRef, OwnedElementRef, Status};
    ///
    /// let dtm = OwnedElementRef::new_unchecked(1, "C507".to_owned(), Status::Mandatory, 1)
    ///     .with_components(vec![
    ///         OwnedComponentRef::new_unchecked(1, "2005".to_owned(), Status::Mandatory),
    ///         OwnedComponentRef::new_unchecked(2, "2380".to_owned(), Status::Conditional),
    ///     ]);
    /// assert_eq!(dtm.components().len(), 2);
    /// ```
    #[must_use]
    pub fn with_components(mut self, components: Vec<OwnedComponentRef>) -> Self {
        self.components = components;
        self
    }

    /// Component definitions; empty when this is a simple data element.
    #[inline]
    pub fn components(&self) -> &[OwnedComponentRef] {
        &self.components
    }

    /// One-based element position (always >= 1).
    #[inline]
    pub fn position(&self) -> u8 {
        self.position
    }

    /// UN/EDIFACT data element identifier.
    #[inline]
    pub fn data_element(&self) -> &str {
        &self.data_element
    }

    /// Requirement status of this element.
    #[inline]
    pub fn status(&self) -> Status {
        self.status
    }

    /// Maximum repetition count for this element.
    #[inline]
    pub fn max_repeat(&self) -> u8 {
        self.max_repeat
    }

    /// Attach the directory's representation for a simple data element.
    #[must_use]
    pub fn with_repr(mut self, repr: Repr) -> Self {
        self.repr = Some(repr);
        self
    }

    /// The declared representation, if the definition states one.
    #[inline]
    #[must_use]
    pub fn repr(&self) -> Option<Repr> {
        self.repr
    }

    /// Declare a representation that changed between syntax versions.
    ///
    /// See [`ComponentRef::with_repr_by_syntax_version`].
    #[must_use]
    pub fn with_repr_by_syntax_version(mut self, up_to_v3: Repr, from_v4: Repr) -> Self {
        self.repr = Some(up_to_v3);
        self.repr_from_v4 = Some(from_v4);
        self
    }

    /// The representation used from syntax version 4 onward, when it differs.
    #[inline]
    #[must_use]
    pub fn repr_from_v4(&self) -> Option<Repr> {
        self.repr_from_v4
    }
}

type SegmentLookupFn = Arc<dyn Fn(&str) -> Option<&'static SegmentDefinition> + Send + Sync>;
type IsCodeValidFn = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;
type SuggestCodeFn = Arc<dyn Fn(&str, &str) -> Option<&'static str> + Send + Sync>;
type ExpectedComponentsFn = Arc<dyn Fn(&str, usize) -> Option<u8> + Send + Sync>;
type AdditionalStructureRuleRefFn = fn(&Segment<'_>) -> Result<(), EdifactError>;
type AdditionalStructureRuleFn =
    Arc<dyn Fn(&Segment<'_>) -> Result<(), EdifactError> + Send + Sync>;
/// Returns the `(element_index, component_index, data_element_id)` tuples to
/// validate against a code list for the given segment tag.
type CodeListRulesFn = Arc<dyn Fn(&str) -> &'static [(usize, usize, &'static str)] + Send + Sync>;
/// Returns the mandatory segment tags for a given EDIFACT message type.
///
/// The slice should contain every tag that must appear at least once in a
/// conformant message of the given type.  The tags are also used to check
/// canonical ordering — their relative order in the returned slice is taken
/// as the expected order in the message.
type RequiredSegmentsFn = Arc<dyn Fn(&str) -> &'static [&'static str] + Send + Sync>;

/// Internal enum that unifies lookup results from static and owned segment definitions.
///
/// Allows `validate_segment` to handle both code-generated (`&'static`) and
/// runtime-constructed ([`OwnedSegmentDef`]) definitions without duplication.
enum SegmentDefRef<'a> {
    Static(&'static SegmentDefinition),
    Owned(&'a OwnedSegmentDef),
}

impl SegmentDefRef<'_> {
    /// Returns the highest defined element position (one-based → used directly as
    /// the maximum zero-based slot count for element-count validation).
    ///
    /// For owned definitions the highest `position` value may exceed the number
    /// of entries in the `elements` vec when positions are non-consecutive.
    fn max_element_position(&self) -> usize {
        match self {
            Self::Static(d) => d
                .elements
                .iter()
                .map(|e| e.position as usize)
                .max()
                .unwrap_or(0),
            Self::Owned(d) => d
                .elements
                .iter()
                .map(|e| e.position as usize)
                .max()
                .unwrap_or(0),
        }
    }

    /// Returns the highest position number among mandatory elements (one-based).
    ///
    /// This equals the minimum number of elements that must be present in a
    /// segment: if the highest-positioned mandatory element is at position 5,
    /// the segment must supply at least 5 elements.
    fn last_mandatory_position(&self) -> usize {
        match self {
            Self::Static(d) => d
                .elements
                .iter()
                .filter(|e| e.status == Status::Mandatory)
                .map(|e| e.position as usize)
                .max()
                .unwrap_or(0),
            Self::Owned(d) => d
                .elements
                .iter()
                .filter(|e| e.status == Status::Mandatory)
                .map(|e| e.position as usize)
                .max()
                .unwrap_or(0),
        }
    }

    /// Iterate over mandatory element positions without heap allocation.
    ///
    /// Calls `f(zero_based_index, data_element_id)` for each element whose
    /// status is [`Status::Mandatory`].  Returns `Err` immediately if `f`
    /// returns `Err`, short-circuiting the remaining elements.
    fn for_each_mandatory_position<E, F>(&self, mut f: F) -> Result<(), E>
    where
        F: FnMut(usize, &str) -> Result<(), E>,
    {
        match self {
            Self::Static(d) => {
                for e in d.elements.iter().filter(|e| e.status == Status::Mandatory) {
                    f((e.position as usize).saturating_sub(1), e.data_element)?;
                }
            }
            Self::Owned(d) => {
                for e in d.elements.iter().filter(|e| e.status == Status::Mandatory) {
                    f(
                        (e.position as usize).saturating_sub(1),
                        e.data_element.as_str(),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Iterate over mandatory *component* positions without heap allocation.
    ///
    /// Calls `f(element_index, component_index, data_element_id)` — both indices
    /// zero-based — for every declared component whose status is
    /// [`Status::Mandatory`].  Definitions that declare no components (the shape
    /// every pre-0.13 directory table had) yield nothing, so this check is
    /// inert until a directory opts in by declaring composites with
    /// [`ElementRef::composite`].
    fn for_each_mandatory_component<E, F>(&self, mut f: F) -> Result<(), E>
    where
        F: FnMut(usize, usize, &str) -> Result<(), E>,
    {
        match self {
            Self::Static(d) => {
                for e in d.elements {
                    for c in e
                        .components
                        .iter()
                        .filter(|c| c.status == Status::Mandatory)
                    {
                        f(
                            (e.position as usize).saturating_sub(1),
                            (c.position as usize).saturating_sub(1),
                            c.data_element,
                        )?;
                    }
                }
            }
            Self::Owned(d) => {
                for e in &d.elements {
                    for c in e
                        .components
                        .iter()
                        .filter(|c| c.status == Status::Mandatory)
                    {
                        f(
                            (e.position as usize).saturating_sub(1),
                            (c.position as usize).saturating_sub(1),
                            c.data_element.as_str(),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Number of declared component **slots** for the element at zero-based `index`.
    ///
    /// A component declared with [`ComponentRef::repeated`] occupies several
    /// slots, so this sums repeat counts rather than counting entries: counting
    /// entries would cap `C080` at two components and reject the four extra
    /// `3036` occurrences the composite is defined to carry.
    ///
    /// `None` when the element is not defined, or is defined without
    /// components — in which case its arity is not constrained by the layout.
    /// The declared maximum occurrence count for the element at `index`.
    fn max_repeat_at(&self, index: usize) -> Option<u8> {
        let position = u8::try_from(index.checked_add(1)?).ok()?;
        match self {
            Self::Static(d) => d
                .elements
                .iter()
                .find(|e| e.position == position)
                .map(ElementRef::max_repeat),
            Self::Owned(d) => d
                .elements
                .iter()
                .find(|e| e.position == position)
                .map(OwnedElementRef::max_repeat),
        }
    }

    /// The representation required at `(element, component)`, if any.
    ///
    /// A composite states its representations on the components; a simple data
    /// element states one on the element itself and only at component 0.
    ///
    /// `syntax_version` selects between the two forms of a position that changed
    /// between versions; `None` accepts either, because guessing would reject
    /// conformant data.
    fn repr_at(
        &self,
        element: usize,
        component: usize,
        syntax_version: Option<u8>,
    ) -> Option<ReprRequirement> {
        let position = u8::try_from(element.checked_add(1)?).ok()?;
        let component_position = u8::try_from(component.checked_add(1)?).ok()?;
        match self {
            Self::Static(d) => {
                let element = d.elements.iter().find(|e| e.position == position)?;
                if element.components.is_empty() {
                    return if component == 0 {
                        select_repr(element.repr(), element.repr_from_v4(), syntax_version)
                    } else {
                        None
                    };
                }
                let component_ref = element.components.iter().find(|c| {
                    // A component declared with `repeated` spans several
                    // consecutive slots, all of the same representation.
                    let first = c.position();
                    let last = first.saturating_add(c.repeat_count().saturating_sub(1));
                    (first..=last).contains(&component_position)
                })?;
                select_repr(
                    component_ref.repr(),
                    component_ref.repr_from_v4(),
                    syntax_version,
                )
            }
            Self::Owned(d) => {
                let element = d.elements.iter().find(|e| e.position == position)?;
                if element.components.is_empty() {
                    return if component == 0 {
                        select_repr(
                            OwnedElementRef::repr(element),
                            OwnedElementRef::repr_from_v4(element),
                            syntax_version,
                        )
                    } else {
                        None
                    };
                }
                let component_ref = element.components.iter().find(|c| {
                    let first = c.position();
                    let last = first.saturating_add(c.repeat_count().saturating_sub(1));
                    (first..=last).contains(&component_position)
                })?;
                select_repr(
                    component_ref.repr(),
                    component_ref.repr_from_v4(),
                    syntax_version,
                )
            }
        }
    }

    fn declared_component_count(&self, index: usize) -> Option<u8> {
        let position = u8::try_from(index.checked_add(1)?).ok()?;
        let count: u32 = match self {
            Self::Static(d) => d
                .elements
                .iter()
                .find(|e| e.position == position)
                .map(|e| e.components.iter().map(|c| u32::from(c.repeat_count)).sum())?,
            Self::Owned(d) => d
                .elements
                .iter()
                .find(|e| e.position == position)
                .map(|e| e.components.iter().map(|c| u32::from(c.repeat_count)).sum())?,
        };
        if count == 0 {
            return None;
        }
        u8::try_from(count).ok()
    }
}

/// Read the syntax version number from `UNB` S001 DE 0002.
///
/// `None` when the slice carries no readable `UNB` — a message window, say.
fn detect_syntax_version(segments: &[Segment<'_>]) -> Option<u8> {
    segments
        .iter()
        .find(|s| s.tag == "UNB")
        .and_then(|unb| unb.component_str(0, 1))
        .and_then(|version| version.parse().ok())
}

/// Pick the representation that applies to `syntax_version`.
///
/// Version 4 onward uses `from_v4` when the definition states one. An unknown
/// version accepts either, because a definition that distinguishes them is
/// distinguishing a real incompatibility — guessing would reject conformant data
/// from whichever version we guessed against.
fn select_repr(
    base: Option<Repr>,
    from_v4: Option<Repr>,
    syntax_version: Option<u8>,
) -> Option<ReprRequirement> {
    match (base, from_v4) {
        (_, None) => base.map(ReprRequirement::single),
        (None, Some(v4)) => Some(ReprRequirement::single(v4)),
        (Some(base), Some(v4)) => Some(match syntax_version {
            Some(version) if version >= 4 => ReprRequirement::single(v4),
            Some(_) => ReprRequirement::single(base),
            None => ReprRequirement {
                primary: base,
                alternative: Some(v4),
            },
        }),
    }
}

/// Default required-segments mapping used when no custom function is provided.
///
/// Returns the universal minimum: every EDIFACT message must begin with `UNH`
/// and end with `UNT`.  Message-type-specific mandatory segments (such as
/// `BGM` for ORDERS/INVOIC) must be enforced by a
/// [`ProfileRulePack`][crate::ProfileRulePack] or a custom
/// [`DirectoryValidatorBuilder::with_required_segments`] function to avoid
/// false positives for message types that do not require `BGM`.
fn default_required_segments(_message_type: &str) -> &'static [&'static str] {
    &["UNH", "UNT"]
}

/// Code-list validation rules common to all UN/EDIFACT directory releases.
///
/// Each entry is `(element_index, component_index, data_element_id)`.
/// `element_index` and `component_index` are zero-based.
///
/// Covers the most frequently validated qualifier/code elements across ORDERS,
/// INVOIC, and similar message types.
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
#[derive(Clone)]
pub struct DirectoryValidator {
    directory_id: String,
    segment_lookup: SegmentLookupFn,
    /// Runtime-owned segment definitions (from builder / JSON / DB).
    ///
    /// When `Some`, takes precedence over `segment_lookup` for tag resolution.
    owned_defs: Option<Arc<Vec<OwnedSegmentDef>>>,
    /// Tag -> index into `owned_defs`.  Without this, `resolve_def` was a linear
    /// scan per segment, making validation O(n_segments x n_definitions).
    owned_index: Option<Arc<std::collections::HashMap<String, usize>>>,
    is_code_valid: IsCodeValidFn,
    suggest_code: SuggestCodeFn,
    expected_components: ExpectedComponentsFn,
    code_list_rules: CodeListRulesFn,
    additional_structure_rule: Option<AdditionalStructureRuleFn>,
    /// Configurable mapping from message type to required segment tags.
    required_segments: RequiredSegmentsFn,
    message_type: Option<String>,
    enforce_known_tags: bool,
    structure_checks: bool,
    code_list_checks: bool,
}

impl std::fmt::Debug for DirectoryValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryValidator")
            .field("directory_id", &self.directory_id)
            .field("message_type", &self.message_type)
            .field("enforce_known_tags", &self.enforce_known_tags)
            .field("structure_checks", &self.structure_checks)
            .field("code_list_checks", &self.code_list_checks)
            .finish_non_exhaustive()
    }
}

impl DirectoryValidator {
    /// Create a validator for a specific directory release with injected lookup/check hooks.
    pub fn new(
        directory_id: &'static str,
        segment_lookup: fn(&str) -> Option<&'static SegmentDefinition>,
        is_code_valid: fn(&str, &str) -> bool,
        suggest_code: fn(&str, &str) -> Option<&'static str>,
        expected_components: fn(&str, usize) -> Option<u8>,
        additional_structure_rule: Option<AdditionalStructureRuleRefFn>,
    ) -> Self {
        Self {
            directory_id: directory_id.to_owned(),
            segment_lookup: Arc::new(segment_lookup),
            owned_defs: None,
            owned_index: None,
            is_code_valid: Arc::new(is_code_valid),
            suggest_code: Arc::new(suggest_code),
            expected_components: Arc::new(expected_components),
            code_list_rules: Arc::new(base_code_list_rules),
            additional_structure_rule: additional_structure_rule
                .map(|f| Arc::new(f) as AdditionalStructureRuleFn),
            required_segments: Arc::new(default_required_segments),
            message_type: None,
            enforce_known_tags: true,
            structure_checks: true,
            code_list_checks: true,
        }
    }

    /// Create a validator from a static slice of [`SegmentDefinition`]s.
    ///
    /// This is the preferred constructor when code-generating directory data as
    /// a `static` array: no manual fn-pointer boilerplate is required.
    ///
    /// Code-list checks are **disabled** by default (the built-in `is_code_valid`
    /// always returns `true`).  Call [`with_code_list_rules`][Self::with_code_list_rules]
    /// to register directory-specific rules that actually validate code values.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// static MY_SEGMENTS: &[SegmentDefinition] = &[ /* … */ ];
    ///
    /// let validator = DirectoryValidator::from_definitions(MY_SEGMENTS)
    ///     .with_code_list_rules(my_code_list_rules);
    /// ```
    pub fn from_definitions(definitions: &'static [SegmentDefinition]) -> Self {
        let lookup_map: std::collections::HashMap<&'static str, &'static SegmentDefinition> =
            definitions.iter().map(|d| (d.tag, d)).collect();
        let lookup_map = Arc::new(lookup_map);
        Self {
            directory_id: "custom".to_owned(),
            segment_lookup: Arc::new(move |tag: &str| lookup_map.get(tag).copied()),
            owned_defs: None,
            owned_index: None,
            is_code_valid: Arc::new(|_de: &str, _code: &str| true),
            suggest_code: Arc::new(|_de: &str, _code: &str| None),
            expected_components: Arc::new(|_tag: &str, _idx: usize| None),
            code_list_rules: Arc::new(base_code_list_rules),
            additional_structure_rule: None,
            required_segments: Arc::new(default_required_segments),
            message_type: None,
            enforce_known_tags: true,
            structure_checks: true,
            code_list_checks: false,
        }
    }

    /// Create a validator from a runtime-owned collection of segment definitions.
    ///
    /// Use this (or [`DirectoryValidatorBuilder`]) when segment definitions are
    /// loaded from an external source at startup (JSON, database, YAML, …) rather
    /// than being known at compile time.
    ///
    /// Code-list checks are **disabled** by default; enable them by chaining
    /// [`with_code_list_rules`][Self::with_code_list_rules] and setting
    /// `is_code_valid` via a custom [`new`][Self::new] call or by subclassing
    /// the builder.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let defs = vec![
    ///     OwnedSegmentDef::new_unchecked(
    ///         "BGM".to_owned(),
    ///         "Beginning of message".to_owned(),
    ///         vec![OwnedElementRef::new_unchecked(1, "C002".to_owned(), Status::Mandatory, 1)],
    ///     ),
    /// ];
    /// let validator = DirectoryValidator::from_owned_definitions(defs)
    ///     .with_directory_id("runtime-profile");
    /// ```
    pub fn from_owned_definitions(definitions: Vec<OwnedSegmentDef>) -> Self {
        Self {
            directory_id: "custom".to_owned(),
            // The static lookup is never consulted when `owned_defs` is `Some`.
            segment_lookup: Arc::new(|_| None),
            owned_index: Some(Arc::new(
                definitions
                    .iter()
                    .enumerate()
                    .map(|(i, d)| (d.tag.clone(), i))
                    .collect(),
            )),
            owned_defs: Some(Arc::new(definitions)),
            is_code_valid: Arc::new(|_de: &str, _code: &str| true),
            suggest_code: Arc::new(|_de: &str, _code: &str| None),
            expected_components: Arc::new(|_tag: &str, _idx: usize| None),
            code_list_rules: Arc::new(base_code_list_rules),
            additional_structure_rule: None,
            required_segments: Arc::new(default_required_segments),
            message_type: None,
            enforce_known_tags: true,
            structure_checks: true,
            code_list_checks: false,
        }
    }

    /// Set the directory identifier string (used in error messages).
    pub fn with_directory_id(mut self, id: impl Into<String>) -> Self {
        self.directory_id = id.into();
        self
    }

    /// Override the code-list rules function.
    ///
    /// Directories can supply a directory-specific implementation that extends or
    /// replaces the base rules from `base_code_list_rules`.
    pub fn with_code_list_rules(
        mut self,
        f: impl Fn(&str) -> &'static [(usize, usize, &'static str)] + Send + Sync + 'static,
    ) -> Self {
        self.code_list_rules = Arc::new(f);
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

    /// Override the required-segments mapping used for structural validation.
    ///
    /// The supplied function receives an EDIFACT message type string (e.g. `"ORDERS"`)
    /// and must return a `'static` slice of segment tags that are mandatory for that
    /// type.  The tags are checked both for *presence* and for *canonical ordering*
    /// within the message.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// fn my_required_segments(msg_type: &str) -> &'static [&'static str] {
    ///     match msg_type {
    ///         "DESADV" => &["UNH", "BGM", "SHP", "UNT"],
    ///         "INVOIC" => &["UNH", "BGM", "MOA", "UNT"],
    ///         _ => &["UNH", "UNT"],
    ///     }
    /// }
    ///
    /// let validator = DirectoryValidator::from_definitions(DEFS)
    ///     .with_required_segments(my_required_segments);
    /// ```
    pub fn with_required_segments(
        mut self,
        f: impl Fn(&str) -> &'static [&'static str] + Send + Sync + 'static,
    ) -> Self {
        self.required_segments = Arc::new(f);
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

    /// Count the non-trailing-empty components in element `element_idx` of `seg`.
    ///
    /// Per ISO 9735-1 §8.7.2 ("Trailing empty component data elements may be omitted"),
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
        while count > 0 && elem.components[count - 1].0.as_ref().is_empty() {
            count -= 1;
        }
        u8::try_from(count).ok()
    }

    fn collect_component_count_issues(
        &self,
        seg: &Segment<'_>,
        def: &SegmentDefRef<'_>,
        out: &mut Vec<EdifactError>,
    ) {
        for idx in 0..seg.elements.len() {
            let actual = Self::effective_component_count(seg, idx).unwrap_or(0);
            // The `expected_components` hook is an exact count and wins when set.
            if let Some(expected) = (self.expected_components)(seg.tag, idx) {
                if actual != expected {
                    out.push(EdifactError::InvalidComponentCount {
                        tag: seg.tag.to_owned(),
                        element_index: idx,
                        expected,
                        actual,
                        span: seg.element_span(idx).unwrap_or(seg.span),
                    });
                }
                continue;
            }
            // Otherwise a composite that declares its components caps them:
            // more components than the directory defines is a structural error,
            // while fewer is normal (conditional components may be omitted).
            if let Some(declared) = def.declared_component_count(idx) {
                if actual > declared {
                    out.push(EdifactError::InvalidComponentCount {
                        tag: seg.tag.to_owned(),
                        element_index: idx,
                        expected: declared,
                        actual,
                        span: seg.element_span(idx).unwrap_or(seg.span),
                    });
                }
            }
        }
    }

    /// Enforce each element's declared maximum number of occurrences.
    ///
    /// `max_repeat` had been carried on every `ElementRef` since the type
    /// existed, exposed by a getter, and read by nothing — so a definition that
    /// said "this element occurs once" constrained nothing at all, and a caller
    /// who wrote it believed otherwise.
    fn collect_repetition_issues(
        &self,
        seg: &Segment<'_>,
        def: &SegmentDefRef<'_>,
        out: &mut Vec<EdifactError>,
    ) {
        for (index, element) in seg.elements.iter().enumerate() {
            let Some(max) = def.max_repeat_at(index) else {
                continue;
            };
            // A declared maximum of zero would forbid the element outright,
            // which is what `Status` is for; treat it as "unconstrained".
            if max == 0 {
                continue;
            }
            let actual = element.repeat_count();
            if actual > usize::from(max) {
                out.push(EdifactError::TooManyRepetitions {
                    tag: seg.tag.to_owned(),
                    element_index: index,
                    max,
                    actual,
                    span: element.span,
                });
            }
        }
    }

    /// Check every populated value against its declared representation.
    ///
    /// Only positions the definition actually states a representation for are
    /// checked, so a partial table stays useful rather than becoming a source of
    /// false findings.
    fn collect_representation_issues(
        &self,
        seg: &Segment<'_>,
        def: &SegmentDefRef<'_>,
        syntax_version: Option<u8>,
        out: &mut Vec<EdifactError>,
    ) {
        for (element_index, element) in seg.elements.iter().enumerate() {
            for occurrence in element.repetitions() {
                for (component_index, (value, span)) in occurrence.iter().enumerate() {
                    // An empty value is an absent one (§8.1); its presence is
                    // the mandatory check's business, not the representation's.
                    if value.is_empty() {
                        continue;
                    }
                    let Some(repr) = def.repr_at(element_index, component_index, syntax_version)
                    else {
                        continue;
                    };
                    if !repr.permits_characters(value) {
                        out.push(EdifactError::InvalidCharacterType {
                            tag: seg.tag.to_owned(),
                            element_index,
                            component_index,
                            repr: repr.to_string(),
                            value: value.to_string(),
                            span: *span,
                        });
                        // The length of a value that is not of the declared
                        // class is not meaningful — §10's numeric count in
                        // particular assumes a numeric value.
                        continue;
                    }
                    self.collect_insignificant_characters(
                        seg,
                        element_index,
                        component_index,
                        value,
                        *span,
                        repr.primary,
                        out,
                    );
                    if repr.permits_length(value) {
                        continue;
                    }
                    let actual = repr.measure(value);
                    out.push(if repr.is_too_short(value) {
                        EdifactError::DataElementTooShort {
                            tag: seg.tag.to_owned(),
                            element_index,
                            component_index,
                            repr: repr.to_string(),
                            actual,
                            span: *span,
                        }
                    } else {
                        EdifactError::DataElementTooLong {
                            tag: seg.tag.to_owned(),
                            element_index,
                            component_index,
                            repr: repr.to_string(),
                            actual,
                            span: *span,
                        }
                    });
                }
            }
        }
    }

    /// Report characters ISO 9735-1 §9.1 requires the sender to suppress.
    ///
    /// Only **variable length** elements are covered, which is what §9.1 says:
    /// a fixed-length numeric field is padded with leading zeroes by design, and
    /// a fixed-length text one with trailing spaces.
    #[allow(clippy::too_many_arguments)]
    fn collect_insignificant_characters(
        &self,
        seg: &Segment<'_>,
        element_index: usize,
        component_index: usize,
        value: &str,
        span: crate::Span,
        repr: Repr,
        out: &mut Vec<EdifactError>,
    ) {
        if repr.is_fixed() {
            return;
        }
        let kind = match repr.kind() {
            ReprKind::Numeric => {
                let digits = value.strip_prefix('-').unwrap_or(value);
                // "Nevertheless, a single zero before a decimal mark is
                // allowed", so `0.5` is correct and only `00…` is not.
                let leading_zeroes = digits.starts_with('0')
                    && digits.len() > 1
                    && !digits.starts_with("0.")
                    && !digits.starts_with("0,");
                if !leading_zeroes {
                    return;
                }
                Insignificant::LeadingZeroes
            }
            ReprKind::Alphabetic | ReprKind::Alphanumeric => {
                if !value.ends_with(' ') {
                    return;
                }
                Insignificant::TrailingSpaces
            }
        };
        out.push(EdifactError::InsignificantCharacters {
            tag: seg.tag.to_owned(),
            element_index,
            component_index,
            kind,
            span,
        });
    }

    fn collect_code_list_issues(&self, seg: &Segment<'_>, out: &mut Vec<EdifactError>) {
        for (elem_idx, comp_idx, de) in (self.code_list_rules)(seg.tag) {
            let value = seg
                .get_element(*elem_idx)
                .and_then(|e| e.get_component(*comp_idx))
                .unwrap_or("");
            if !value.is_empty() && !(self.is_code_valid)(de, value) {
                let suggestion = (self.suggest_code)(de, value);
                // Point at the offending *value*, not the whole segment, so
                // rendered diagnostics underline the code that failed.
                let span = seg
                    .get_element(*elem_idx)
                    .and_then(|e| e.component_span(*comp_idx))
                    .unwrap_or(seg.span);
                out.push(EdifactError::InvalidCodeValue {
                    tag: seg.tag.to_owned(),
                    element_index: *elem_idx,
                    value: value.to_owned(),
                    code_list: (*de).to_owned(),
                    span,
                    suggestion,
                });
            }
        }
    }
}

impl DirectoryValidator {
    fn resolve_def<'a>(&'a self, tag: &str) -> Option<SegmentDefRef<'a>> {
        if let Some(owned) = &self.owned_defs {
            let index = self.owned_index.as_ref()?;
            owned.get(*index.get(tag)?).map(SegmentDefRef::Owned)
        } else {
            (self.segment_lookup)(tag).map(SegmentDefRef::Static)
        }
    }

    /// Check one segment, appending **every** violation found to `out`.
    ///
    /// Reporting continues past the first fault: a segment missing two mandatory
    /// elements and carrying an invalid code is three findings, and a validator
    /// whose whole purpose is an exhaustive report has no business hiding two of
    /// them.  Only the checks that cannot proceed without a resolved definition
    /// short-circuit.
    fn collect_segment_issues(
        &self,
        seg: &Segment<'_>,
        syntax_version: Option<u8>,
        out: &mut Vec<EdifactError>,
    ) {
        if !self.structure_checks && !self.code_list_checks {
            return;
        }

        let Some(def) = self.resolve_def(seg.tag) else {
            if self.structure_checks && self.enforce_known_tags {
                out.push(EdifactError::InvalidSegmentForMessage {
                    tag: seg.tag.to_owned(),
                    message_type: self
                        .message_type
                        .clone()
                        .unwrap_or_else(|| self.directory_id.clone()),
                    span: seg.tag_span,
                });
            }
            // Without a definition there is nothing further to check against.
            return;
        };

        if self.structure_checks {
            let max_elements = def.max_element_position();
            let min_elements = def.last_mandatory_position();
            let actual = seg.elements.len();
            if actual < min_elements || actual > max_elements {
                out.push(EdifactError::InvalidElementCount {
                    tag: seg.tag.to_owned(),
                    min: min_elements,
                    max: max_elements,
                    actual,
                    span: seg.span,
                });
            }

            def.for_each_mandatory_position::<std::convert::Infallible, _>(|idx, _de| {
                let is_present = seg.elements.get(idx).is_some_and(|elem| {
                    elem.components.iter().any(|(c, _)| !c.as_ref().is_empty())
                });
                if !is_present {
                    out.push(EdifactError::MissingRequiredElement {
                        tag: seg.tag.to_owned(),
                        element_index: idx,
                    });
                }
                Ok(())
            })
            .unwrap_or_else(|never| match never {});

            // Mandatory *components* inside declared composites.  Only fires for
            // definitions built with `ElementRef::composite` / `with_components`;
            // an element that is absent entirely is already reported above as a
            // missing element, so only present elements are checked here.
            def.for_each_mandatory_component::<std::convert::Infallible, _>(
                |elem_idx, comp_idx, _de| {
                    let Some(elem) = seg.elements.get(elem_idx) else {
                        return Ok(());
                    };
                    let present = elem
                        .get_component(comp_idx)
                        .is_some_and(|value| !value.is_empty());
                    if !present {
                        out.push(EdifactError::MissingRequiredComponent {
                            tag: seg.tag.to_owned(),
                            element_index: elem_idx,
                            component_index: comp_idx,
                        });
                    }
                    Ok(())
                },
            )
            .unwrap_or_else(|never| match never {});

            self.collect_component_count_issues(seg, &def, out);
            self.collect_repetition_issues(seg, &def, out);
            self.collect_representation_issues(seg, &def, syntax_version, out);

            if let Some(rule) = &self.additional_structure_rule {
                if let Err(error) = rule(seg) {
                    out.push(error);
                }
            }
        }

        if self.code_list_checks {
            self.collect_code_list_issues(seg, out);
        }
    }
}

impl Validator for DirectoryValidator {
    fn set_message_type(&mut self, message_type: Option<&str>) {
        self.message_type = message_type.map(str::to_owned);
    }

    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        _context: &ValidationRuleContext<'_>,
    ) {
        // The syntax version decides which form of a version-dependent
        // representation applies; `UNB` S001 DE 0002 is where it is stated.
        let syntax_version = detect_syntax_version(segments);
        let mut issues = Vec::new();
        for seg in segments {
            self.collect_segment_issues(seg, syntax_version, &mut issues);
            for err in issues.drain(..) {
                report_error(report, err);
            }
        }

        if self.structure_checks {
            if let Some(message_type) = self.detect_message_type(segments) {
                // One pass recording each tag's first index answers both the
                // presence and the ordering question.  The previous shape ran two
                // full scans *per required tag* and invoked `required_segments`
                // twice, which is O(|required| x n) on every batch.
                let mut first_index: std::collections::HashMap<&str, usize> =
                    std::collections::HashMap::with_capacity(segments.len());
                for (i, seg) in segments.iter().enumerate() {
                    first_index.entry(seg.tag).or_insert(i);
                }

                let required = (self.required_segments)(&message_type);
                for required_tag in required {
                    if !first_index.contains_key(*required_tag) {
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

                let mut last_idx = None;
                for tag in required {
                    if let Some(&idx) = first_index.get(*tag) {
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

// ── DirectoryValidatorBuilder ─────────────────────────────────────────────────

/// Builder for [`DirectoryValidator`] using runtime-owned segment definitions.
///
/// Use this when segment definitions are loaded from an external source at
/// startup (JSON, database, YAML, …) rather than being available as `static`
/// arrays at compile time.
///
/// # Example
///
/// ```rust,ignore
/// let validator = DirectoryValidatorBuilder::new("my-profile")
///     .add_segment(
///         OwnedSegmentDef::new_unchecked(
///             "BGM".to_owned(),
///             "Beginning of message".to_owned(),
///             vec![OwnedElementRef::new_unchecked(1, "C002".to_owned(), Status::Mandatory, 1)],
///         ),
///     )
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct DirectoryValidatorBuilder {
    directory_id: Option<String>,
    segments: Vec<OwnedSegmentDef>,
}

impl DirectoryValidatorBuilder {
    /// Create a new builder with the given directory identifier.
    ///
    /// The identifier is used in error messages; set a human-readable value
    /// such as `"ORDERS-MIG-5.5"` or `"custom-profile"`.
    pub fn new(directory_id: impl Into<String>) -> Self {
        Self {
            directory_id: Some(directory_id.into()),
            segments: Vec::new(),
        }
    }

    /// Add a segment definition to the builder.
    ///
    /// Definitions can be added in any order; the resulting validator looks
    /// them up by tag at validation time.
    pub fn add_segment(mut self, def: OwnedSegmentDef) -> Self {
        self.segments.push(def);
        self
    }

    /// Extend the builder with multiple segment definitions at once.
    pub fn add_segments(mut self, defs: impl IntoIterator<Item = OwnedSegmentDef>) -> Self {
        self.segments.extend(defs);
        self
    }

    /// Build the [`DirectoryValidator`].
    ///
    /// Returns a validator backed by the accumulated [`OwnedSegmentDef`]s.
    /// Code-list checks are disabled by default; chain
    /// [`DirectoryValidator::with_code_list_rules`] on the returned value to
    /// enable them.
    pub fn build(self) -> DirectoryValidator {
        let mut validator = DirectoryValidator::from_owned_definitions(self.segments);
        if let Some(id) = self.directory_id {
            validator.directory_id = id;
        }
        validator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_ELEMENTS: &[ElementRef] = &[ElementRef::new(1, "C507", Status::Mandatory, 1)];

    static TEST_SEGMENT: SegmentDefinition =
        SegmentDefinition::new("TST", "Test segment", TEST_ELEMENTS);

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
        validator.validate_batch(
            &segments,
            &mut report,
            &crate::validator::ValidationRuleContext::empty(),
        );
        assert!(!report.has_errors());
    }

    // ── effective_component_count (ISO 9735-1 §8.7.2 trailing-empty-component trim) ──

    fn parse_single(input: &[u8]) -> crate::OwnedSegment {
        crate::from_reader_collect(std::io::Cursor::new(input))
            .expect("parse should succeed")
            .into_iter()
            .next()
            .expect("at least one segment")
    }

    #[test]
    fn trailing_empty_component_stripped_from_dtm() {
        // DTM+137:20200101: has three components in element 0; the third is empty.
        // ISO 9735-1 §8.7.2 says trailing empty components may be omitted,
        // so effective count should be 2.
        let owned = parse_single(b"DTM+137:20200101:'");
        let seg = owned.as_borrowed();
        let count = DirectoryValidator::effective_component_count(&seg, 0);
        assert_eq!(
            count,
            Some(2),
            "trailing empty component should be stripped"
        );
    }

    #[test]
    fn all_empty_components_result_in_zero() {
        // NAD+MS++: → element 2 is ":" with two empty components → effective=0
        let owned = parse_single(b"NAD+MS++:'");
        let seg = owned.as_borrowed();
        let count = DirectoryValidator::effective_component_count(&seg, 2);
        assert_eq!(
            count,
            Some(0),
            "all-empty composite should have effective count 0"
        );
    }

    #[test]
    fn non_empty_component_not_stripped() {
        // DTM+137:20200101:102 — all three components are non-empty
        let owned = parse_single(b"DTM+137:20200101:102'");
        let seg = owned.as_borrowed();
        let count = DirectoryValidator::effective_component_count(&seg, 0);
        assert_eq!(
            count,
            Some(3),
            "no components should be stripped when all non-empty"
        );
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
        validator.validate_batch(
            &segments,
            &mut report,
            &crate::validator::ValidationRuleContext::empty(),
        );
        assert!(
            report.has_warnings(),
            "INVALID is not in the custom code list so validation must warn"
        );
    }
}
