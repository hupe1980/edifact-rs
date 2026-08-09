use crate::directory_validator::{ElementPath, SegmentLayout};
use crate::error::EdifactError;
use smallvec::SmallVec;
use std::borrow::Cow;

/// Reject a layout whose tag does not describe `segment_tag`.
///
/// Resolving `"3055"` against the wrong definition would silently address a
/// different element — the exact failure mode code-addressed access exists to
/// eliminate — so the mismatch is an error rather than a lookup miss.
#[inline]
fn check_layout_tag<L: SegmentLayout + ?Sized>(
    layout: &L,
    segment_tag: &str,
) -> Result<(), EdifactError> {
    if layout.layout_tag() != segment_tag {
        return Err(EdifactError::SegmentLayoutMismatch {
            expected: layout.layout_tag().to_owned(),
            actual: segment_tag.to_owned(),
        });
    }
    Ok(())
}

/// A half-open byte span within an EDIFACT payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Span {
    /// Start byte offset (inclusive).
    pub start: usize,
    /// End byte offset (exclusive).
    pub end: usize,
}

impl Span {
    #[inline]
    /// Construct a span from inclusive start and exclusive end offsets.
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[inline]
    /// Shift the span by `delta` bytes.
    ///
    /// Uses saturating addition to avoid integer overflow on malformed input.
    pub const fn offset(self, delta: usize) -> Self {
        Self {
            start: self.start.saturating_add(delta),
            end: self.end.saturating_add(delta),
        }
    }

    /// Length of the span in bytes.
    ///
    /// In debug builds, asserts `end >= start` (inverted spans are a bug).
    /// In release builds, returns 0 for inverted spans rather than panicking,
    /// so a single corrupt span does not abort an entire validation run.
    #[inline]
    pub fn len(self) -> usize {
        debug_assert!(
            self.end >= self.start,
            "Span::len: end ({}) < start ({})",
            self.end,
            self.start
        );
        self.end.saturating_sub(self.start)
    }

    /// Returns `true` if the span covers zero bytes.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A single EDIFACT segment, borrowing its data from the source input.
///
/// `#[non_exhaustive]`: build one with [`Segment::new`] rather than a struct
/// literal.  Adding `repeats` to [`Element`] in 0.14 broke every downstream
/// literal, and the next field would do it again; a constructor plus builder
/// setters keeps that additive.  The fields stay public, so reading and `..`
/// destructuring are unaffected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Segment<'a> {
    /// Segment tag, usually three uppercase letters.
    pub tag: &'a str,
    /// Span covering the whole segment payload.
    pub span: Span,
    /// Span covering only the segment tag.
    pub tag_span: Span,
    /// Segment elements in positional order.
    pub elements: Vec<Element<'a>>,
}

impl<'a> Segment<'a> {
    #[inline]
    /// Construct a segment with default spans.
    pub fn new(tag: &'a str, elements: Vec<Element<'a>>) -> Self {
        Self {
            tag,
            span: Span::default(),
            tag_span: Span::default(),
            elements,
        }
    }

    /// Return the element at position `n` (0-indexed), if it exists.
    #[inline]
    pub fn get_element(&self, n: usize) -> Option<&Element<'a>> {
        self.elements.get(n)
    }

    /// Shorthand: get component 0 of element `n` — the most common access pattern.
    #[inline]
    pub fn element_str(&self, n: usize) -> Option<&str> {
        self.elements.get(n)?.get_component(0)
    }

    /// Get component `comp` of element `elem` (both 0-based), or `None` if absent.
    ///
    /// Mirrors [`OwnedSegment::component_str`], eliminating the need to chain
    /// `get_element(elem)?.get_component(comp)` in rule closures.
    #[inline]
    pub fn component_str(&self, elem: usize, comp: usize) -> Option<&str> {
        self.elements.get(elem)?.get_component(comp)
    }

    /// Return the byte span of the element at position `n`, if it exists.
    #[inline]
    pub fn element_span(&self, n: usize) -> Option<Span> {
        Some(self.elements.get(n)?.span)
    }

    // ── code-addressed access ─────────────────────────────────────────────────

    /// Read the value at an already-resolved [`ElementPath`].
    ///
    /// Use this when the same path is reused across many segments — resolve once
    /// with [`SegmentLayout::resolve_code`], then read without repeating the
    /// lookup.
    #[inline]
    pub fn value_at(&self, path: ElementPath) -> Option<&str> {
        self.elements
            .get(path.element)?
            .get_component(path.component_index())
    }

    /// Byte span of the value at an already-resolved [`ElementPath`].
    #[inline]
    pub fn span_at(&self, path: ElementPath) -> Option<Span> {
        let element = self.elements.get(path.element)?;
        match path.component {
            Some(c) => element.component_span(c),
            None => Some(element.span),
        }
    }

    /// Read a value by its UN/EDIFACT data element identifier.
    ///
    /// Positional access (`seg.element_str(4)`) fails silently when the index is
    /// wrong: it reads a different, usually still-plausible value.  Code-addressed
    /// access cannot — a stale or mistyped identifier is a
    /// [`EdifactError::UnknownDataElement`], checked against the directory.
    ///
    /// `Ok(None)` means the identifier is valid for this segment but the value is
    /// absent from *this* instance, which is the normal state for a conditional
    /// element.
    ///
    /// # Performance
    ///
    /// Each call scans the layout for the identifier. That is a handful of short
    /// string comparisons and fine for one-off reads, but when pulling the same
    /// identifier out of many segments, resolve once with
    /// [`SegmentLayout::resolve_code`] and read with [`value_at`](Self::value_at).
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{ComponentRef, ElementRef, SegmentDefinition, Status};
    ///
    /// static C507: &[ComponentRef] = &[
    ///     ComponentRef::new(1, "2005", Status::Mandatory),
    ///     ComponentRef::new(2, "2380", Status::Conditional),
    ///     ComponentRef::new(3, "2379", Status::Conditional),
    /// ];
    /// static DTM_ELEMENTS: &[ElementRef] =
    ///     &[ElementRef::composite(1, "C507", Status::Mandatory, 1, C507)];
    /// static DTM: SegmentDefinition =
    ///     SegmentDefinition::new("DTM", "Date/time/period", DTM_ELEMENTS);
    ///
    /// let segments: Vec<_> = edifact_rs::from_bytes(b"DTM+137:20260101:102'")
    ///     .collect::<Result<Vec<_>, _>>()?;
    /// let dtm = &segments[0];
    ///
    /// assert_eq!(dtm.value_by_code(&DTM, "2380")?, Some("20260101"));
    /// // A data element that this segment does not define is a hard error,
    /// // not a wrong-but-quiet read.
    /// assert!(dtm.value_by_code(&DTM, "3055").is_err());
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::SegmentLayoutMismatch`] when `layout` describes a
    /// different segment tag, [`EdifactError::UnknownDataElement`] when the
    /// identifier is not in the definition, and
    /// [`EdifactError::AmbiguousDataElement`] when it appears more than once.
    pub fn value_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<&str>, EdifactError> {
        check_layout_tag(layout, self.tag)?;
        Ok(self.value_at(layout.resolve_code(data_element)?))
    }

    /// Byte span of a value addressed by its UN/EDIFACT data element identifier.
    ///
    /// Use this to attach a precise [`Span`] to a
    /// [`ValidationIssue`][crate::ValidationIssue] without hand-counting indices.
    ///
    /// # Errors
    ///
    /// As [`value_by_code`][Self::value_by_code].
    pub fn span_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<Span>, EdifactError> {
        check_layout_tag(layout, self.tag)?;
        Ok(self.span_at(layout.resolve_code(data_element)?))
    }

    /// Return the whole [`Element`] addressed by a data element identifier.
    ///
    /// When the identifier names a component inside a composite, the enclosing
    /// composite element is returned.
    ///
    /// # Errors
    ///
    /// As [`value_by_code`][Self::value_by_code].
    pub fn element_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<&Element<'a>>, EdifactError> {
        check_layout_tag(layout, self.tag)?;
        let path = layout.resolve_code(data_element)?;
        Ok(self.elements.get(path.element))
    }
}

/// Components of one repetition of a data element, each paired with its span.
pub type Components<'a> = SmallVec<[(Cow<'a, str>, Span); 4]>;

/// Components of one repetition of an owned data element.
pub type OwnedComponents = SmallVec<[(String, Span); 4]>;

/// A data element, which may have one or more component values.
///
/// `#[non_exhaustive]`: build one with [`Element::of`] (plus
/// [`and_repeat`][Element::and_repeat] / [`with_span`][Element::with_span])
/// rather than a struct literal.
///
/// Uses [`SmallVec`] with an inline capacity of 4 to avoid heap allocation
/// for the common case (≤ 4 components).  Component values borrow from the
/// original input; if the value contained a release-character sequence the
/// resolved string is stored as an owned [`Cow::Owned`] variant instead of
/// using `Box::leak`.
///
/// Each entry is a `(value, span)` pair, guaranteeing that the component
/// string and its byte span are always in sync.
///
/// # Repetition (ISO 9735-4 §3.1)
///
/// [`components`][Self::components] holds the **first** repetition, which is the
/// only one for every interchange that does not declare a repetition separator
/// in its `UNA` — that is, virtually all of them.  Further repetitions land in
/// [`repeats`][Self::repeats]; read them together with
/// [`repetitions`][Self::repetitions].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Element<'a> {
    /// Span covering the whole element, including every repetition.
    pub span: Span,
    /// Components of the first repetition, in positional order.
    pub components: Components<'a>,
    /// Second and subsequent repetitions of this data element.
    ///
    /// Empty — and therefore unallocated — unless the interchange declares a
    /// repetition separator and the element actually repeats.
    pub repeats: Vec<Components<'a>>,
}

impl<'a> Element<'a> {
    /// Return the component at position `n` (0-indexed) of the first repetition.
    #[inline]
    pub fn get_component(&self, n: usize) -> Option<&str> {
        self.components.get(n).map(|(c, _)| c.as_ref())
    }

    /// Number of repetitions of this data element — always at least 1.
    #[inline]
    pub fn repeat_count(&self) -> usize {
        1 + self.repeats.len()
    }

    /// Components of repetition `n` (0-indexed), if it exists.
    #[inline]
    pub fn repetition(&self, n: usize) -> Option<&[(Cow<'a, str>, Span)]> {
        match n {
            0 => Some(&self.components),
            _ => self.repeats.get(n - 1).map(|r| r.as_slice()),
        }
    }

    /// Iterate over every repetition of this element, first one included.
    ///
    /// # Example
    ///
    /// ```
    /// // `UNA` byte 7 declares `*` as the repetition separator.
    /// let segments: Vec<_> = edifact_rs::from_bytes(b"UNA:+.?*'RFF+ON:1*ON:2'")
    ///     .collect::<Result<Vec<_>, _>>()?;
    /// let rff = segments[0].get_element(0).unwrap();
    ///
    /// let refs: Vec<&str> = rff
    ///     .repetitions()
    ///     .map(|components| components[1].0.as_ref())
    ///     .collect();
    /// assert_eq!(refs, ["1", "2"]);
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[inline]
    pub fn repetitions(&self) -> impl Iterator<Item = &[(Cow<'a, str>, Span)]> {
        std::iter::once(self.components.as_slice()).chain(self.repeats.iter().map(|r| r.as_slice()))
    }

    /// Return the component at position `n`, or `""` if absent.
    #[inline]
    pub fn component_or_empty(&self, n: usize) -> &str {
        self.components
            .get(n)
            .map(|(c, _)| c.as_ref())
            .unwrap_or("")
    }

    /// Return the byte span of the component at position `n`, if it exists.
    #[inline]
    pub fn component_span(&self, n: usize) -> Option<Span> {
        self.components.get(n).map(|(_, s)| *s)
    }

    /// Convenience constructor: wraps string literals as borrowed components.
    ///
    /// Useful in tests and when constructing segments for writing.
    pub fn of(components: &[&'a str]) -> Self {
        Self {
            span: Span::default(),
            components: components
                .iter()
                .copied()
                .map(|c| (Cow::Borrowed(c), Span::default()))
                .collect(),
            repeats: Vec::new(),
        }
    }

    /// Set the span covering this element.
    ///
    /// Parsed elements carry real spans; hand-built ones default to
    /// [`Span::default`] and only need this when the caller is synthesising
    /// input for diagnostics.
    #[must_use]
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = span;
        self
    }

    /// Append a further repetition of this data element (ISO 9735-4 §3.1).
    ///
    /// Useful when building segments for [`Writer::write_segment`][crate::Writer::write_segment];
    /// the writer joins repetitions with the active repetition separator.
    #[must_use]
    pub fn and_repeat(mut self, components: &[&'a str]) -> Self {
        self.repeats.push(
            components
                .iter()
                .copied()
                .map(|c| (Cow::Borrowed(c), Span::default()))
                .collect(),
        );
        self
    }
}

/// Owned data element used by reader-based parsing APIs.
///
/// Each entry in `components` is a `(value, span)` pair, keeping the string
/// and its byte span structurally in sync.
///
/// `#[non_exhaustive]`: build one with [`OwnedElement::of`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OwnedElement {
    /// Span covering the whole element, including every repetition.
    pub span: Span,
    /// Components of the first repetition, in positional order.
    pub components: OwnedComponents,
    /// Second and subsequent repetitions (ISO 9735-4 §3.1); usually empty.
    pub repeats: Vec<OwnedComponents>,
}

impl OwnedElement {
    /// Build an owned data element from its component values.
    ///
    /// The owned counterpart of [`Element::of`].  Spans default to
    /// [`Span::default`]; set the element span with
    /// [`with_span`][Self::with_span] when synthesising input for diagnostics.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{OwnedElement, OwnedSegment};
    ///
    /// let segment = OwnedSegment::new(
    ///     "NAD",
    ///     vec![
    ///         OwnedElement::of(&["BY"]),
    ///         OwnedElement::of(&["4000001000002", "", "9"]),
    ///     ],
    /// );
    /// assert_eq!(segment.component_str(1, 2), Some("9"));
    /// ```
    #[must_use]
    pub fn of<S: AsRef<str>>(components: &[S]) -> Self {
        Self {
            span: Span::default(),
            components: components
                .iter()
                .map(|c| (c.as_ref().to_owned(), Span::default()))
                .collect(),
            repeats: Vec::new(),
        }
    }

    /// Set the span covering this element.
    #[must_use]
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = span;
        self
    }

    /// Append a further repetition of this data element (ISO 9735-4 §3.1).
    ///
    /// The owned counterpart of [`Element::and_repeat`].
    #[must_use]
    pub fn and_repeat<S: AsRef<str>>(mut self, components: &[S]) -> Self {
        self.repeats.push(
            components
                .iter()
                .map(|c| (c.as_ref().to_owned(), Span::default()))
                .collect(),
        );
        self
    }

    #[inline]
    /// Shift all stored spans by `delta` bytes.
    pub fn offset(mut self, delta: usize) -> Self {
        self.offset_in_place(delta);
        self
    }

    /// Shift all stored spans by `delta` bytes, in place.
    ///
    /// Every repetition is shifted, not just the first: the reader parses each
    /// segment from a zero-based slice and then rebases it onto the stream, so
    /// a repetition left unshifted points into a different segment entirely.
    #[inline]
    pub fn offset_in_place(&mut self, delta: usize) {
        self.span = self.span.offset(delta);
        for (_, span) in &mut self.components {
            *span = span.offset(delta);
        }
        for repeat in &mut self.repeats {
            for (_, span) in repeat {
                *span = span.offset(delta);
            }
        }
    }

    /// Number of repetitions of this data element — always at least 1.
    #[inline]
    pub fn repeat_count(&self) -> usize {
        1 + self.repeats.len()
    }

    /// Components of repetition `n` (0-indexed), if it exists.
    #[inline]
    pub fn repetition(&self, n: usize) -> Option<&[(String, Span)]> {
        match n {
            0 => Some(&self.components),
            _ => self.repeats.get(n - 1).map(|r| r.as_slice()),
        }
    }

    /// Iterate over every repetition of this element, first one included.
    #[inline]
    pub fn repetitions(&self) -> impl Iterator<Item = &[(String, Span)]> {
        std::iter::once(self.components.as_slice()).chain(self.repeats.iter().map(|r| r.as_slice()))
    }
}

impl<'a> From<Element<'a>> for OwnedElement {
    fn from(value: Element<'a>) -> Self {
        fn own(components: Components<'_>) -> OwnedComponents {
            components
                .into_iter()
                .map(|(c, s)| (c.into_owned(), s))
                .collect()
        }
        Self {
            span: value.span,
            components: own(value.components),
            repeats: value.repeats.into_iter().map(own).collect(),
        }
    }
}

/// Owned segment used by reader-based parsing APIs.
///
/// `#[non_exhaustive]`: build one with [`OwnedSegment::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OwnedSegment {
    /// Segment tag, usually three uppercase letters.
    pub tag: String,
    /// Span covering the whole segment payload.
    pub span: Span,
    /// Span covering only the segment tag.
    pub tag_span: Span,
    /// Owned segment elements in positional order.
    pub elements: Vec<OwnedElement>,
}

/// Zero-allocation view of an [`OwnedElement`].
///
/// Implements the same accessor methods as [`Element`] without constructing
/// any intermediate `SmallVec` or `Cow` values.  Use this when you hold an
/// `&OwnedSegment` reference and want to inspect element data without the
/// `Vec<Element>` allocation that [`OwnedSegment::as_borrowed`] incurs.
///
/// Construct via `BorrowedElement::from(&owned_element)` or through
/// [`BorrowedSegment::get_element`].
#[derive(Debug, Clone, Copy)]
pub struct BorrowedElement<'a>(pub(crate) &'a OwnedElement);

impl<'a> From<&'a OwnedElement> for BorrowedElement<'a> {
    #[inline]
    fn from(elem: &'a OwnedElement) -> Self {
        BorrowedElement(elem)
    }
}

impl<'a> BorrowedElement<'a> {
    /// Return the component at position `n` (0-indexed), if it exists.
    #[inline]
    pub fn get_component(&self, n: usize) -> Option<&'a str> {
        self.0.components.get(n).map(|(s, _)| s.as_str())
    }

    /// Return the component at position `n`, or `""` if absent.
    #[inline]
    pub fn component_or_empty(&self, n: usize) -> &'a str {
        self.0
            .components
            .get(n)
            .map(|(s, _)| s.as_str())
            .unwrap_or("")
    }

    /// Return the byte span of the component at position `n`, if it exists.
    #[inline]
    pub fn component_span(&self, n: usize) -> Option<Span> {
        self.0.components.get(n).map(|(_, s)| *s)
    }

    /// The byte span covering the whole element.
    #[inline]
    pub fn span(&self) -> Span {
        self.0.span
    }

    /// Number of components in this element.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.components.len()
    }

    /// Returns `true` if this element has no components.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.components.is_empty()
    }

    /// Iterate over all component strings.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &'a str> {
        self.0.components.iter().map(|(c, _)| c.as_str())
    }

    /// Number of repetitions of this data element — always at least 1.
    #[inline]
    pub fn repeat_count(&self) -> usize {
        self.0.repeat_count()
    }

    /// Components of repetition `n` (0-indexed), if it exists.
    #[inline]
    pub fn repetition(&self, n: usize) -> Option<&'a [(String, Span)]> {
        match n {
            0 => Some(&self.0.components),
            _ => self.0.repeats.get(n - 1).map(|r| r.as_slice()),
        }
    }

    /// Iterate over every repetition of this element, first one included.
    #[inline]
    pub fn repetitions(&self) -> impl Iterator<Item = &'a [(String, Span)]> {
        std::iter::once(self.0.components.as_slice())
            .chain(self.0.repeats.iter().map(|r| r.as_slice()))
    }
}

/// Zero-allocation view of an [`OwnedSegment`].
///
/// Implements the same accessor methods as [`Segment`] without constructing
/// a `Vec<Element>`.  Use this when you hold an `&OwnedSegment` reference and
/// want to read data without the allocations incurred by
/// [`OwnedSegment::as_borrowed`].
///
/// # Construction
///
/// The idiomatic way to obtain a `BorrowedSegment` is via [`OwnedSegment::borrow`]
/// or the [`From`] impl:
///
/// ```rust
/// use edifact_rs::{BorrowedSegment, OwnedSegment, Span};
///
/// let seg = OwnedSegment::new("BGM", vec![]).with_spans(Span::new(0, 3), Span::new(0, 3));
/// let borrowed = BorrowedSegment::from(&seg);
/// assert_eq!(borrowed.tag(), "BGM");
/// ```
///
/// The `'a` lifetime is tied to the referent — you cannot outlive the
/// `OwnedSegment` you borrowed from.
#[derive(Debug, Clone, Copy)]
pub struct BorrowedSegment<'a>(pub(crate) &'a OwnedSegment);

impl<'a> From<&'a OwnedSegment> for BorrowedSegment<'a> {
    #[inline]
    fn from(seg: &'a OwnedSegment) -> Self {
        BorrowedSegment(seg)
    }
}

impl<'a> BorrowedSegment<'a> {
    /// The segment tag (e.g. `"BGM"`).
    #[inline]
    pub fn tag(&self) -> &'a str {
        &self.0.tag
    }

    /// Byte span covering the whole segment.
    #[inline]
    pub fn span(&self) -> Span {
        self.0.span
    }

    /// Byte span covering only the segment tag.
    #[inline]
    pub fn tag_span(&self) -> Span {
        self.0.tag_span
    }

    /// Return the element at position `n` (0-indexed), if it exists.
    #[inline]
    pub fn get_element(&self, n: usize) -> Option<BorrowedElement<'a>> {
        self.0.elements.get(n).map(BorrowedElement)
    }

    /// Shorthand: first component of element `n` — the most common access pattern.
    #[inline]
    pub fn element_str(&self, n: usize) -> Option<&'a str> {
        self.0
            .elements
            .get(n)?
            .components
            .first()
            .map(|(c, _)| c.as_str())
    }

    /// Get component `comp` of element `elem` (both 0-based), or `None` if absent.
    ///
    /// Mirrors [`OwnedSegment::component_str`].
    #[inline]
    pub fn component_str(&self, elem: usize, comp: usize) -> Option<&'a str> {
        self.0
            .elements
            .get(elem)?
            .components
            .get(comp)
            .map(|(c, _)| c.as_str())
    }

    /// Return the byte span of the element at position `n`, if it exists.
    #[inline]
    pub fn element_span(&self, n: usize) -> Option<Span> {
        Some(self.0.elements.get(n)?.span)
    }

    /// Iterate over all elements as zero-allocation views.
    #[inline]
    pub fn elements(&self) -> impl Iterator<Item = BorrowedElement<'a>> {
        self.0.elements.iter().map(BorrowedElement)
    }

    // ── code-addressed access ─────────────────────────────────────────────────

    /// Read the value at an already-resolved [`ElementPath`].
    #[inline]
    pub fn value_at(&self, path: ElementPath) -> Option<&'a str> {
        self.0
            .elements
            .get(path.element)?
            .components
            .get(path.component_index())
            .map(|(c, _)| c.as_str())
    }

    /// Byte span of the value at an already-resolved [`ElementPath`].
    #[inline]
    pub fn span_at(&self, path: ElementPath) -> Option<Span> {
        let element = self.0.elements.get(path.element)?;
        match path.component {
            Some(c) => element.components.get(c).map(|(_, s)| *s),
            None => Some(element.span),
        }
    }

    /// Read a value by its UN/EDIFACT data element identifier.
    ///
    /// Zero-allocation counterpart of [`Segment::value_by_code`].
    ///
    /// # Errors
    ///
    /// As [`Segment::value_by_code`].
    pub fn value_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<&'a str>, EdifactError> {
        check_layout_tag(layout, &self.0.tag)?;
        Ok(self.value_at(layout.resolve_code(data_element)?))
    }

    /// Byte span of a value addressed by its UN/EDIFACT data element identifier.
    ///
    /// # Errors
    ///
    /// As [`Segment::value_by_code`].
    pub fn span_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<Span>, EdifactError> {
        check_layout_tag(layout, &self.0.tag)?;
        Ok(self.span_at(layout.resolve_code(data_element)?))
    }

    /// Return the whole element addressed by a data element identifier.
    ///
    /// When the identifier names a component inside a composite, the enclosing
    /// composite element is returned.
    ///
    /// # Errors
    ///
    /// As [`Segment::value_by_code`].
    pub fn element_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<BorrowedElement<'a>>, EdifactError> {
        check_layout_tag(layout, &self.0.tag)?;
        let path = layout.resolve_code(data_element)?;
        Ok(self.0.elements.get(path.element).map(BorrowedElement))
    }
}

impl OwnedSegment {
    /// Build an owned segment from a tag and its data elements.
    ///
    /// The owned counterpart of [`Segment::new`].  Spans default to
    /// [`Span::default`], which is what a segment synthesised from a non-EDIFACT
    /// source should carry — there is no input to point at.  Use
    /// [`with_spans`][Self::with_spans] when there is.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{OwnedElement, OwnedSegment, segments_to_bytes_owned};
    ///
    /// let segment = OwnedSegment::new("BGM", vec![OwnedElement::of(&["220"])]);
    /// assert_eq!(segments_to_bytes_owned(&[segment])?, b"BGM+220'".to_vec());
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[must_use]
    pub fn new(tag: impl Into<String>, elements: Vec<OwnedElement>) -> Self {
        Self {
            tag: tag.into(),
            span: Span::default(),
            tag_span: Span::default(),
            elements,
        }
    }

    /// Set the segment and tag spans.
    #[must_use]
    pub fn with_spans(mut self, span: Span, tag_span: Span) -> Self {
        self.span = span;
        self.tag_span = tag_span;
        self
    }

    /// Get the first component of element `n`, or `None` if absent.
    ///
    /// This is the zero-allocation equivalent of `as_borrowed().element_str(n)`.
    /// Used internally by [`crate::find_segment_owned`] and the derived
    /// [`crate::EdifactDeserialize::edifact_deserialize_owned`] implementations.
    #[inline]
    pub fn element_str(&self, n: usize) -> Option<&str> {
        self.elements
            .get(n)?
            .components
            .first()
            .map(|(s, _)| s.as_str())
    }

    /// Get component `comp` of element `elem`, or `None` if absent.
    ///
    /// Zero-allocation equivalent of `as_borrowed().get_element(elem)?.get_component(comp)`.
    #[inline]
    pub fn component_str(&self, elem: usize, comp: usize) -> Option<&str> {
        self.elements
            .get(elem)?
            .components
            .get(comp)
            .map(|(s, _)| s.as_str())
    }

    #[inline]
    /// Shift all stored spans by `delta` bytes.
    ///
    /// Delegates to [`OwnedElement::offset_in_place`] rather than walking the
    /// components inline: an inline walk shifted `components` but silently left
    /// `repeats` at their segment-relative offsets, so on the reader path every
    /// repetition after the first pointed at the wrong bytes.
    pub fn offset(mut self, delta: usize) -> Self {
        self.span = self.span.offset(delta);
        self.tag_span = self.tag_span.offset(delta);
        for element in &mut self.elements {
            element.offset_in_place(delta);
        }
        self
    }

    #[inline]
    /// View this owned segment as a borrowed [`Segment`].
    ///
    /// **Performance note**: allocates a `Vec<Element<'_>>` on every call.
    /// When only individual field access is needed, prefer
    /// [`OwnedSegment::borrow`] → [`BorrowedSegment`] which is O(1).
    /// `as_borrowed` remains necessary when the callee requires `&[Segment<'_>]`.
    pub fn as_borrowed(&self) -> Segment<'_> {
        Segment {
            tag: self.tag.as_str(),
            span: self.span,
            tag_span: self.tag_span,
            elements: self
                .elements
                .iter()
                .map(|elem| {
                    fn borrow(components: &OwnedComponents) -> Components<'_> {
                        components
                            .iter()
                            .map(|(c, s)| (Cow::Borrowed(c.as_str()), *s))
                            .collect()
                    }
                    Element {
                        span: elem.span,
                        components: borrow(&elem.components),
                        repeats: elem.repeats.iter().map(borrow).collect(),
                    }
                })
                .collect(),
        }
    }

    /// Return a zero-allocation view of this segment.
    ///
    /// Unlike [`as_borrowed`][OwnedSegment::as_borrowed], this is `O(1)` and
    /// performs no heap allocation.  The view cannot be passed to APIs that
    /// require `&[Segment<'_>]`; use [`as_borrowed`][OwnedSegment::as_borrowed]
    /// for those call sites.
    #[inline]
    pub fn borrow(&self) -> BorrowedSegment<'_> {
        BorrowedSegment(self)
    }

    // ── code-addressed access ─────────────────────────────────────────────────

    /// Read the value at an already-resolved [`ElementPath`].
    #[inline]
    pub fn value_at(&self, path: ElementPath) -> Option<&str> {
        self.elements
            .get(path.element)?
            .components
            .get(path.component_index())
            .map(|(s, _)| s.as_str())
    }

    /// Byte span of the value at an already-resolved [`ElementPath`].
    #[inline]
    pub fn span_at(&self, path: ElementPath) -> Option<Span> {
        let element = self.elements.get(path.element)?;
        match path.component {
            Some(c) => element.components.get(c).map(|(_, s)| *s),
            None => Some(element.span),
        }
    }

    /// Read a value by its UN/EDIFACT data element identifier.
    ///
    /// Owned-storage counterpart of [`Segment::value_by_code`]; allocates nothing.
    ///
    /// # Errors
    ///
    /// As [`Segment::value_by_code`].
    pub fn value_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<&str>, EdifactError> {
        check_layout_tag(layout, &self.tag)?;
        Ok(self.value_at(layout.resolve_code(data_element)?))
    }

    /// Byte span of a value addressed by its UN/EDIFACT data element identifier.
    ///
    /// # Errors
    ///
    /// As [`Segment::value_by_code`].
    pub fn span_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<Span>, EdifactError> {
        check_layout_tag(layout, &self.tag)?;
        Ok(self.span_at(layout.resolve_code(data_element)?))
    }

    /// Return the whole [`OwnedElement`] addressed by a data element identifier.
    ///
    /// When the identifier names a component inside a composite, the enclosing
    /// composite element is returned.
    ///
    /// # Errors
    ///
    /// As [`Segment::value_by_code`].
    pub fn element_by_code<L: SegmentLayout + ?Sized>(
        &self,
        layout: &L,
        data_element: &str,
    ) -> Result<Option<&OwnedElement>, EdifactError> {
        check_layout_tag(layout, &self.tag)?;
        let path = layout.resolve_code(data_element)?;
        Ok(self.elements.get(path.element))
    }
}

impl<'a> From<Segment<'a>> for OwnedSegment {
    fn from(value: Segment<'a>) -> Self {
        Self {
            tag: value.tag.to_string(),
            span: value.span,
            tag_span: value.tag_span,
            elements: value.elements.into_iter().map(OwnedElement::from).collect(),
        }
    }
}
