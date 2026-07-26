//! Shared UN/EDIFACT directory validation engine used by D.11A, D.01B and D.96A.

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
    /// One-based component position within the composite.
    position: u8,
    /// UN/EDIFACT component data element identifier.
    data_element: &'static str,
    /// Requirement status of the component.
    status: Status,
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
        }
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
        assert!(
            self.code_positions(data_element) == 1,
            "data element identifier is unknown or ambiguous in this segment definition"
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
            self.code_positions(data_element) == 1,
            "data element identifier is unknown or ambiguous in this segment definition"
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
    /// One-based component position within the composite.
    position: u8,
    /// UN/EDIFACT component data element identifier.
    data_element: String,
    /// Requirement status.
    status: Status,
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
        }
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

    /// Number of declared components for the element at zero-based `index`.
    ///
    /// `None` when the element is not defined, or is defined without
    /// components — in which case its arity is not constrained by the layout.
    fn declared_component_count(&self, index: usize) -> Option<u8> {
        let position = u8::try_from(index.checked_add(1)?).ok()?;
        let count = match self {
            Self::Static(d) => d
                .elements
                .iter()
                .find(|e| e.position == position)
                .map(|e| e.components.len())?,
            Self::Owned(d) => d
                .elements
                .iter()
                .find(|e| e.position == position)
                .map(|e| e.components.len())?,
        };
        if count == 0 {
            return None;
        }
        u8::try_from(count).ok()
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
        while count > 0 && elem.components[count - 1].0.as_ref().is_empty() {
            count -= 1;
        }
        u8::try_from(count).ok()
    }

    fn validate_component_counts(
        &self,
        seg: &Segment<'_>,
        def: &SegmentDefRef<'_>,
    ) -> Result<(), EdifactError> {
        for idx in 0..seg.elements.len() {
            let actual = Self::effective_component_count(seg, idx).unwrap_or(0);
            // The `expected_components` hook is an exact count and wins when set.
            if let Some(expected) = (self.expected_components)(seg.tag, idx) {
                if actual != expected {
                    return Err(EdifactError::InvalidComponentCount {
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
                    return Err(EdifactError::InvalidComponentCount {
                        tag: seg.tag.to_owned(),
                        element_index: idx,
                        expected: declared,
                        actual,
                        span: seg.element_span(idx).unwrap_or(seg.span),
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
                // Point at the offending *value*, not the whole segment, so
                // rendered diagnostics underline the code that failed.
                let span = seg
                    .get_element(*elem_idx)
                    .and_then(|e| e.component_span(*comp_idx))
                    .unwrap_or(seg.span);
                return Err(EdifactError::InvalidCodeValue {
                    tag: seg.tag.to_owned(),
                    element_index: *elem_idx,
                    value: value.to_owned(),
                    code_list: (*de).to_owned(),
                    span,
                    suggestion,
                });
            }
        }

        Ok(())
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

    fn validate_segment(&self, seg: &Segment<'_>) -> Result<(), EdifactError> {
        if !self.structure_checks && !self.code_list_checks {
            return Ok(());
        }

        let Some(def) = self.resolve_def(seg.tag) else {
            if self.structure_checks && self.enforce_known_tags {
                return Err(EdifactError::InvalidSegmentForMessage {
                    tag: seg.tag.to_owned(),
                    message_type: self
                        .message_type
                        .clone()
                        .unwrap_or_else(|| self.directory_id.clone()),
                    span: seg.tag_span,
                });
            }
            return Ok(());
        };

        let max_elements = def.max_element_position();
        let min_elements = def.last_mandatory_position();
        let actual = seg.elements.len();

        if self.structure_checks && (actual < min_elements || actual > max_elements) {
            return Err(EdifactError::InvalidElementCount {
                tag: seg.tag.to_owned(),
                min: min_elements,
                max: max_elements,
                actual,
                span: seg.span,
            });
        }

        if self.structure_checks {
            def.for_each_mandatory_position(|idx, _de| {
                let is_present = seg.elements.get(idx).is_some_and(|elem| {
                    elem.components.iter().any(|(c, _)| !c.as_ref().is_empty())
                });
                if !is_present {
                    return Err(EdifactError::MissingRequiredElement {
                        tag: seg.tag.to_owned(),
                        element_index: idx,
                    });
                }
                Ok(())
            })?;
            // Mandatory *components* inside declared composites.  Only fires for
            // definitions built with `ElementRef::composite` / `with_components`;
            // an element that is absent entirely is already reported above as a
            // missing element, so only present elements are checked here.
            def.for_each_mandatory_component(|elem_idx, comp_idx, _de| {
                let Some(elem) = seg.elements.get(elem_idx) else {
                    return Ok(());
                };
                let present = elem
                    .get_component(comp_idx)
                    .is_some_and(|value| !value.is_empty());
                if !present {
                    return Err(EdifactError::MissingRequiredComponent {
                        tag: seg.tag.to_owned(),
                        element_index: elem_idx,
                        component_index: comp_idx,
                    });
                }
                Ok(())
            })?;
            self.validate_component_counts(seg, &def)?;

            if let Some(rule) = &self.additional_structure_rule {
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

    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        _context: &ValidationRuleContext<'_>,
    ) {
        for seg in segments {
            if let Err(err) = self.validate_segment(seg) {
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
    /// such as `"UTILMD-5.5.3a"` or `"custom-profile"`.
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

    // ── effective_component_count (ISO 9735-1 §3.3 trailing-empty-component trim) ──

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
        // ISO 9735-1 §3.3 says trailing empty components may be omitted,
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
