use smallvec::SmallVec;
use std::borrow::Cow;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

/// A data element, which may have one or more component values.
///
/// Uses [`SmallVec`] with an inline capacity of 4 to avoid heap allocation
/// for the common case (≤ 4 components).  Component values borrow from the
/// original input; if the value contained a release-character sequence the
/// resolved string is stored as an owned [`Cow::Owned`] variant instead of
/// using `Box::leak`.
///
/// Each entry is a `(value, span)` pair, guaranteeing that the component
/// string and its byte span are always in sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element<'a> {
    /// Span covering the whole element.
    pub span: Span,
    /// Element components in positional order, each paired with its byte span.
    pub components: SmallVec<[(Cow<'a, str>, Span); 4]>,
}

impl<'a> Element<'a> {
    /// Return the component at position `n` (0-indexed), if it exists.
    #[inline]
    pub fn get_component(&self, n: usize) -> Option<&str> {
        self.components.get(n).map(|(c, _)| c.as_ref())
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
        }
    }
}

/// Owned data element used by reader-based parsing APIs.
///
/// Each entry in `components` is a `(value, span)` pair, keeping the string
/// and its byte span structurally in sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedElement {
    /// Span covering the whole element.
    pub span: Span,
    /// Owned element components in positional order, each paired with its byte span.
    pub components: SmallVec<[(String, Span); 4]>,
}

impl OwnedElement {
    #[inline]
    /// Shift all stored spans by `delta` bytes.
    pub fn offset(mut self, delta: usize) -> Self {
        self.span = self.span.offset(delta);
        for (_, span) in &mut self.components {
            *span = span.offset(delta);
        }
        self
    }
}

impl<'a> From<Element<'a>> for OwnedElement {
    fn from(value: Element<'a>) -> Self {
        Self {
            span: value.span,
            components: value
                .components
                .into_iter()
                .map(|(c, s)| (c.into_owned(), s))
                .collect(),
        }
    }
}

/// Owned segment used by reader-based parsing APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// let seg = OwnedSegment {
///     tag: "BGM".into(),
///     span: Span::new(0, 3),
///     tag_span: Span::new(0, 3),
///     elements: vec![],
/// };
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
}

impl OwnedSegment {
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
    pub fn offset(mut self, delta: usize) -> Self {
        self.span = self.span.offset(delta);
        self.tag_span = self.tag_span.offset(delta);
        for element in &mut self.elements {
            element.span = element.span.offset(delta);
            for (_, span) in &mut element.components {
                *span = span.offset(delta);
            }
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
                .map(|elem| Element {
                    span: elem.span,
                    components: elem
                        .components
                        .iter()
                        .map(|(c, s)| (Cow::Borrowed(c.as_str()), *s))
                        .collect(),
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
