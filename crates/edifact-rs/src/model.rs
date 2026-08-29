//! The EDIFACT data model: [`Span`], [`Element`], and [`Segment`].
//!
//! # One segment type, borrowed or owned
//!
//! [`Segment<'a>`] holds its text as [`Cow<'a, str>`], so the *same* type covers
//! both parsing modes:
//!
//! - [`from_bytes`][crate::from_bytes] borrows straight out of the input and
//!   yields `Segment<'input>` — no allocation for segment data.
//! - [`from_reader`][crate::from_reader] cannot borrow from a stream, so it
//!   yields `Segment<'static>`, aliased as [`OwnedSegment`].
//!
//! `Segment` is covariant in `'a`, so a `&[OwnedSegment]` is accepted anywhere a
//! `&[Segment<'_>]` is expected: every API takes one shape and serves both.
//!
//! ```
//! use edifact_rs::{OwnedSegment, Segment};
//!
//! fn count_bgm(segments: &[Segment<'_>]) -> usize {
//!     segments.iter().filter(|s| s.tag == "BGM").count()
//! }
//!
//! let borrowed: Vec<Segment<'_>> =
//!     edifact_rs::from_bytes(b"BGM+220'").collect::<Result<_, _>>()?;
//! let owned: Vec<OwnedSegment> =
//!     edifact_rs::from_reader(std::io::Cursor::new(b"BGM+220'")).collect::<Result<_, _>>()?;
//!
//! assert_eq!(count_bgm(&borrowed), 1);
//! assert_eq!(count_bgm(&owned), 1); // the same function, no conversion
//! # Ok::<(), edifact_rs::EdifactError>(())
//! ```

use crate::directory_validator::{ElementPath, SegmentLayout};
use crate::error::EdifactError;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::str::FromStr;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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

/// Components of one occurrence of a data element, each paired with its span.
pub type Components<'a> = SmallVec<[(Cow<'a, str>, Span); 4]>;

/// A [`Segment`] that owns all of its text.
///
/// Just `Segment<'static>` — the shape [`from_reader`][crate::from_reader]
/// produces, because a stream has no buffer to borrow from. It is accepted
/// anywhere a `&[Segment<'_>]` is wanted.
pub type OwnedSegment = Segment<'static>;

/// An [`Element`] that owns all of its text. See [`OwnedSegment`].
pub type OwnedElement = Element<'static>;

/// A data element, which may have one or more component values.
///
/// `#[non_exhaustive]`: build one with [`Element::of`] (plus
/// [`and_repeat`][Element::and_repeat] / [`with_span`][Element::with_span])
/// rather than a struct literal.
///
/// Uses [`SmallVec`] with an inline capacity of 4 to avoid heap allocation
/// for the common case (≤ 4 components). Each entry is a `(value, span)` pair,
/// guaranteeing that the component string and its byte span stay in sync.
/// A value that contained a release-character sequence is stored as
/// [`Cow::Owned`]; everything else borrows from the input.
///
/// # Repetition (ISO 9735-1 §8.6)
///
/// [`components`][Self::components] holds the **first** occurrence, which is the
/// only one for every interchange that does not declare a repetition separator —
/// that is, virtually all of them. Further occurrences land in
/// [`repeats`][Self::repeats]; read them together with
/// [`repetitions`][Self::repetitions].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Element<'a> {
    /// Span covering the whole element, including every occurrence.
    pub span: Span,
    /// Components of the first occurrence, in positional order.
    pub components: Components<'a>,
    /// Second and subsequent occurrences of this data element.
    ///
    /// Empty — and therefore unallocated — unless the interchange declares a
    /// repetition separator and the element actually repeats.
    pub repeats: Vec<Components<'a>>,
}

impl<'a> Element<'a> {
    /// Build a data element from its component values.
    ///
    /// Accepts anything that converts to `Cow<'a, str>`, so both string literals
    /// and owned `String`s work:
    ///
    /// ```
    /// use edifact_rs::{Element, OwnedElement};
    ///
    /// let borrowed = Element::of(&["4000001000002", "", "9"]);
    /// let owned: OwnedElement = Element::of(&[String::from("BY")]);
    ///
    /// assert_eq!(borrowed.get_component(2), Some("9"));
    /// assert_eq!(owned.get_component(0), Some("BY"));
    /// ```
    pub fn of<S>(components: &[S]) -> Self
    where
        S: Into<Cow<'a, str>> + Clone,
    {
        Self {
            span: Span::default(),
            components: components
                .iter()
                .map(|c| (c.clone().into(), Span::default()))
                .collect(),
            repeats: Vec::new(),
        }
    }

    /// Return the component at position `n` (0-indexed) of the first occurrence.
    #[inline]
    pub fn get_component(&self, n: usize) -> Option<&str> {
        self.components.get(n).map(|(c, _)| c.as_ref())
    }

    /// Return the component at position `n`, or `""` if absent.
    #[inline]
    pub fn component_or_empty(&self, n: usize) -> &str {
        self.get_component(n).unwrap_or("")
    }

    /// Return the byte span of the component at position `n`, if it exists.
    #[inline]
    pub fn component_span(&self, n: usize) -> Option<Span> {
        self.components.get(n).map(|(_, s)| *s)
    }

    /// Iterate over the component values of the first occurrence.
    #[inline]
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.components.iter().map(|(c, _)| c.as_ref())
    }

    /// Number of occurrences of this data element — always at least 1.
    #[inline]
    pub fn repeat_count(&self) -> usize {
        1 + self.repeats.len()
    }

    /// Components of occurrence `n` (0-indexed), if it exists.
    #[inline]
    pub fn repetition(&self, n: usize) -> Option<&[(Cow<'a, str>, Span)]> {
        match n {
            0 => Some(&self.components),
            _ => self.repeats.get(n - 1).map(|r| r.as_slice()),
        }
    }

    /// Iterate over every occurrence of this element, the first one included.
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

    /// Read component `n` from every occurrence of this element (ISO 9735-1 §8.6).
    ///
    /// One item per occurrence, using `""` where an occurrence omits the
    /// component: §8.7.3 makes occurrence position significant, so dropping the
    /// empty ones would shift every later value into the wrong slot.
    #[inline]
    pub fn repeated_component(&self, n: usize) -> impl Iterator<Item = &str> {
        self.repetitions()
            .map(move |occurrence| occurrence.get(n).map_or("", |(c, _)| c.as_ref()))
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

    /// Append a further occurrence of this data element (ISO 9735-1 §8.6).
    ///
    /// Useful when building segments for
    /// [`Writer::write_segment`][crate::Writer::write_segment]; the writer joins
    /// occurrences with the active repetition separator.
    #[must_use]
    pub fn and_repeat<S>(mut self, components: &[S]) -> Self
    where
        S: Into<Cow<'a, str>> + Clone,
    {
        self.repeats.push(
            components
                .iter()
                .map(|c| (c.clone().into(), Span::default()))
                .collect(),
        );
        self
    }

    /// Shift every stored span by `delta` bytes, in place.
    ///
    /// Every occurrence is shifted, not just the first: the reader parses each
    /// segment from a zero-based slice and then rebases it onto the stream, so
    /// an occurrence left unshifted points into a different segment entirely.
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

    /// Detach this element from the input buffer, cloning any borrowed text.
    #[must_use]
    pub fn into_owned(self) -> OwnedElement {
        fn own(components: Components<'_>) -> Components<'static> {
            components
                .into_iter()
                .map(|(c, s)| (Cow::Owned(c.into_owned()), s))
                .collect()
        }
        Element {
            span: self.span,
            components: own(self.components),
            repeats: self.repeats.into_iter().map(own).collect(),
        }
    }
}

/// A single EDIFACT segment.
///
/// Borrows its text from the parsed input where it can, and owns it where it
/// cannot: [`from_bytes`][crate::from_bytes] yields `Segment<'input>`, while
/// [`from_reader`][crate::from_reader] yields `Segment<'static>` — aliased as
/// [`OwnedSegment`]. Covariance in `'a` means one is accepted wherever the other
/// is, so every API in this crate takes a single shape.
///
/// `#[non_exhaustive]`: build one with [`Segment::new`] rather than a struct
/// literal, so a future field stays additive. The fields stay public, so reading
/// and `..` destructuring are unaffected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Segment<'a> {
    /// Segment tag — three ASCII uppercase letters for anything this crate parses.
    pub tag: Cow<'a, str>,
    /// Span covering the whole segment payload.
    pub span: Span,
    /// Span covering only the segment tag.
    pub tag_span: Span,
    /// Segment elements in positional order.
    pub elements: Vec<Element<'a>>,
}

impl<'a> Segment<'a> {
    /// Build a segment from a tag and its data elements.
    ///
    /// Spans default to [`Span::default`], which is what a segment synthesised
    /// from a non-EDIFACT source should carry — there is no input to point at.
    /// Use [`with_spans`][Self::with_spans] when there is.
    ///
    /// The tag is not checked here; it is checked when the segment is written.
    /// A tag is emitted verbatim, so one the parser would reject is refused as
    /// [`EdifactError::InvalidSegmentTag`] by
    /// [`Writer::write_segment`][crate::Writer::write_segment] rather than
    /// written out as bytes that do not read back.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{Element, Segment, segments_to_bytes};
    ///
    /// let segment = Segment::new("BGM", vec![Element::of(&["220"])]);
    /// assert_eq!(segments_to_bytes(&[segment])?, b"BGM+220'".to_vec());
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[inline]
    pub fn new(tag: impl Into<Cow<'a, str>>, elements: Vec<Element<'a>>) -> Self {
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

    /// The segment tag as a plain `&str`.
    ///
    /// `segment.tag` compares directly against a string literal
    /// (`segment.tag == "BGM"`); this is for the places that need a `&str`, such
    /// as a `match`.
    #[inline]
    pub fn tag(&self) -> &str {
        self.tag.as_ref()
    }

    /// Return the element at position `n` (0-indexed), if it exists.
    #[inline]
    pub fn get_element(&self, n: usize) -> Option<&Element<'a>> {
        self.elements.get(n)
    }

    /// Shorthand: component 0 of element `n` — the most common access pattern.
    #[inline]
    pub fn element_str(&self, n: usize) -> Option<&str> {
        self.elements.get(n)?.get_component(0)
    }

    /// Get component `comp` of element `elem` (both 0-based), or `None` if absent.
    #[inline]
    pub fn component_str(&self, elem: usize, comp: usize) -> Option<&str> {
        self.elements.get(elem)?.get_component(comp)
    }

    /// Return the byte span of the element at position `n`, if it exists.
    #[inline]
    pub fn element_span(&self, n: usize) -> Option<Span> {
        Some(self.elements.get(n)?.span)
    }

    /// Read component `component` from every occurrence of element `element`.
    ///
    /// Yields nothing when the element is absent. See
    /// [`Element::repeated_component`].
    ///
    /// # Example
    ///
    /// ```
    /// // `UNA` position 050 declares `*` as the repetition separator.
    /// let segments: Vec<_> = edifact_rs::from_bytes(b"UNA:+.?*'RFF+ON:1*ON:2*ON:3'")
    ///     .collect::<Result<Vec<_>, _>>()?;
    ///
    /// let references: Vec<&str> = segments[0].repeated_component(0, 1).collect();
    /// assert_eq!(references, ["1", "2", "3"]);
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[inline]
    pub fn repeated_component(
        &self,
        element: usize,
        component: usize,
    ) -> impl Iterator<Item = &str> {
        self.elements
            .get(element)
            .into_iter()
            .flat_map(move |elem| elem.repeated_component(component))
    }

    /// Shift every stored span by `delta` bytes.
    #[inline]
    #[must_use]
    pub fn offset(mut self, delta: usize) -> Self {
        self.span = self.span.offset(delta);
        self.tag_span = self.tag_span.offset(delta);
        for element in &mut self.elements {
            element.offset_in_place(delta);
        }
        self
    }

    /// Detach this segment from the input buffer, cloning any borrowed text.
    ///
    /// Use it to keep a segment alive past the buffer it was parsed from.
    #[must_use]
    pub fn into_owned(self) -> OwnedSegment {
        Segment {
            tag: Cow::Owned(self.tag.into_owned()),
            span: self.span,
            tag_span: self.tag_span,
            elements: self.elements.into_iter().map(Element::into_owned).collect(),
        }
    }

    // ── checked field access ──────────────────────────────────────────────────

    /// Read element `idx`, treating an empty value as absent.
    ///
    /// EDIFACT lets an element be syntactically present but empty (`SEG++'`).
    /// A mandatory data element must carry a value, so this reports
    /// [`EdifactError::MissingRequiredElement`] for both cases.
    ///
    /// # Errors
    ///
    /// [`EdifactError::MissingRequiredElement`] when the element is absent or empty.
    pub fn required_element(&self, idx: usize) -> Result<&str, EdifactError> {
        self.optional_element(idx)
            .ok_or_else(|| EdifactError::MissingRequiredElement {
                tag: self.tag.clone().into_owned(),
                element_index: idx,
            })
    }

    /// Read element `idx`, treating an empty value as absent.
    #[inline]
    pub fn optional_element(&self, idx: usize) -> Option<&str> {
        self.element_str(idx).filter(|s| !s.is_empty())
    }

    /// Read component `comp` of element `elem`, treating an empty value as absent.
    ///
    /// # Errors
    ///
    /// [`EdifactError::MissingRequiredElement`] when the element itself is
    /// absent, and [`EdifactError::MissingRequiredComponent`] when the element is
    /// present but the component is absent or empty. The distinction matters:
    /// the first says the segment is too short, the second that one composite is
    /// incomplete.
    pub fn required_component(&self, elem: usize, comp: usize) -> Result<&str, EdifactError> {
        let element =
            self.elements
                .get(elem)
                .ok_or_else(|| EdifactError::MissingRequiredElement {
                    tag: self.tag.clone().into_owned(),
                    element_index: elem,
                })?;
        element
            .get_component(comp)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EdifactError::MissingRequiredComponent {
                tag: self.tag.clone().into_owned(),
                element_index: elem,
                component_index: comp,
            })
    }

    /// Read component `comp` of element `elem`, treating an empty value as absent.
    #[inline]
    pub fn optional_component(&self, elem: usize, comp: usize) -> Option<&str> {
        self.component_str(elem, comp).filter(|s| !s.is_empty())
    }

    /// Read element `idx` and parse it into `T`.
    ///
    /// # Errors
    ///
    /// As [`required_element`][Self::required_element], plus
    /// [`EdifactError::InvalidText`] when the value does not parse.
    pub fn parsed_element<T: FromStr>(&self, idx: usize) -> Result<T, EdifactError> {
        let raw = self.required_element(idx)?;
        raw.parse::<T>().map_err(|_| EdifactError::InvalidText {
            offset: self
                .element_span(idx)
                .map(|s| s.start)
                .unwrap_or(self.span.start),
        })
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
    /// wrong: it reads a different, usually still-plausible value. Code-addressed
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
        check_layout_tag(layout, &self.tag)?;
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
        check_layout_tag(layout, &self.tag)?;
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
        check_layout_tag(layout, &self.tag)?;
        let path = layout.resolve_code(data_element)?;
        Ok(self.elements.get(path.element))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_segments_pass_where_borrowed_ones_are_expected() {
        fn tags<'s>(segments: &'s [Segment<'_>]) -> Vec<&'s str> {
            segments.iter().map(Segment::tag).collect()
        }

        let owned: Vec<OwnedSegment> =
            crate::from_reader(std::io::Cursor::new(b"BGM+220'UNT+2+1'"))
                .collect::<Result<_, _>>()
                .expect("reader parse");
        // The point of the unified model: no conversion, no `_owned` twin.
        assert_eq!(tags(&owned), ["BGM", "UNT"]);
    }

    #[test]
    fn into_owned_outlives_the_input_buffer() {
        let segment = {
            let input = b"BGM+220+PO-4711'".to_vec();
            let parsed: Vec<Segment<'_>> = crate::from_bytes(&input)
                .collect::<Result<_, _>>()
                .expect("parse");
            parsed.into_iter().next().unwrap().into_owned()
        };
        assert_eq!(segment.element_str(1), Some("PO-4711"));
    }

    #[test]
    fn required_accessors_treat_empty_as_absent() {
        let segments: Vec<Segment<'_>> = crate::from_bytes(b"NAD++::'")
            .collect::<Result<_, _>>()
            .expect("parse");
        let nad = &segments[0];
        assert!(matches!(
            nad.required_element(0),
            Err(EdifactError::MissingRequiredElement { .. })
        ));
        // Element 1 exists but its components are empty …
        assert!(matches!(
            nad.required_component(1, 0),
            Err(EdifactError::MissingRequiredComponent { .. })
        ));
        // … while element 5 does not exist at all.
        assert!(matches!(
            nad.required_component(5, 0),
            Err(EdifactError::MissingRequiredElement { .. })
        ));
    }
}
