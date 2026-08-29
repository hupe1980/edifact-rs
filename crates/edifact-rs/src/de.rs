//! Typed deserialization: EDIFACT segments to Rust values.
//!
//! [`EdifactDeserialize`] maps a slice of parsed [`Segment`]s onto a struct.
//! Implement it by hand or, far more usually, derive it — see
//! [`EdifactDeserialize`][macro@crate::EdifactDeserialize].
//!
//! [`EdifactSegmentTag`] is the companion trait that carries a type's segment
//! tag (and optional qualifier pattern), which is what makes the blanket
//! `impl EdifactDeserialize for Vec<T>` and the streaming helpers possible.
//!
//! # Entry points
//!
//! | Function | Input | Yields |
//! |---|---|---|
//! | [`deserialize`] | `&[u8]` | one `T` |
//! | [`deserialize_str`] | `&str` | one `T` |
//! | [`deserialize_each`] | `&[u8]` | every matching `T`, lazily |
//! | [`deserialize_each_from_reader`] | `impl Read` | every matching `T`, lazily |
//! | [`deserialize_messages`] | `&[u8]` | one `T` per `UNH`/`UNT` message |
//! | [`deserialize_messages_from_reader`] | `impl Read` | one `T` per message |

use crate::{EdifactError, Segment};
use std::borrow::Cow;
use std::io::Read;

// ── traits ────────────────────────────────────────────────────────────────────

/// Types that can be deserialized from a slice of EDIFACT segments.
///
/// Implement manually or derive with
/// [`#[derive(EdifactDeserialize)]`][macro@crate::EdifactDeserialize].
///
/// The slice may hold any number of segments; an implementation extracts the
/// ones it cares about and ignores the rest. Both parsing paths are covered by
/// the one signature: `&[OwnedSegment]` coerces to `&[Segment<'_>]`.
pub trait EdifactDeserialize: Sized {
    /// Deserialize `Self` from the provided segment slice.
    ///
    /// # Errors
    ///
    /// Implementation-defined; derived impls report
    /// [`EdifactError::MissingSegment`] for an absent segment,
    /// [`EdifactError::MissingRequiredElement`] for an absent mandatory field,
    /// and [`EdifactError::InvalidFieldValue`] for one that will not parse.
    fn edifact_deserialize(segments: &[Segment<'_>]) -> Result<Self, EdifactError>;
}

/// Types that can be deserialized from a composite EDIFACT element.
///
/// Implement this for custom composite structs used with
/// `#[edifact(composite)]` in derive macros.
pub trait EdifactCompositeDeserialize: Sized {
    /// Deserialize `Self` from a composite element.
    ///
    /// # Errors
    ///
    /// Implementation-defined; typically
    /// [`EdifactError::MissingRequiredComponent`].
    fn edifact_deserialize_composite(composite: CompositeElement<'_>)
    -> Result<Self, EdifactError>;
}

impl EdifactCompositeDeserialize for Vec<String> {
    fn edifact_deserialize_composite(
        composite: CompositeElement<'_>,
    ) -> Result<Self, EdifactError> {
        Ok(composite.iter().map(str::to_owned).collect())
    }
}

/// Companion trait that declares a type's segment tag (and optional qualifier).
///
/// Required for the `Vec<T>` blanket impl and for finding the right segment in
/// a message-level struct deserialization.
pub trait EdifactSegmentTag {
    /// The 3-character EDIFACT segment tag (e.g. `"BGM"`, `"NAD"`).
    const SEGMENT_TAG: &'static str;

    /// Optional qualifier pattern to further constrain segment matching.
    ///
    /// Examples:
    /// - `Some("MS")` for exact qualifier matching.
    /// - `Some("M*")` for wildcard prefix matching (matches `"MS"`, `"MR"`, etc.).
    const QUALIFIER_PATTERN: Option<&'static str> = None;

    /// Return `true` if `seg`'s qualifier matches this type's qualifier pattern.
    fn matches_qualifier(seg: &Segment<'_>) -> bool {
        match Self::QUALIFIER_PATTERN {
            Some(pattern) => qualifier_matches_pattern(seg.element_str(0).unwrap_or(""), pattern),
            None => true,
        }
    }

    /// Return `true` if `seg` is the segment this type maps to.
    ///
    /// Default: the tag matches and, when a qualifier pattern is declared,
    /// element 0 matches it (e.g. `NAD+BY`).
    fn matches_segment(seg: &Segment<'_>) -> bool {
        seg.tag == Self::SEGMENT_TAG && Self::matches_qualifier(seg)
    }
}

// ── blanket impl for Vec<T> ───────────────────────────────────────────────────

/// Deserializes each segment matching `T::matches_segment` as an independent
/// single-segment slice, collecting the results.
impl<T> EdifactDeserialize for Vec<T>
where
    T: EdifactDeserialize + EdifactSegmentTag,
{
    fn edifact_deserialize(segments: &[Segment<'_>]) -> Result<Self, EdifactError> {
        segments
            .iter()
            .filter(|s| T::matches_segment(s))
            .map(|seg| T::edifact_deserialize(std::slice::from_ref(seg)))
            .collect()
    }
}

// ── entry points ──────────────────────────────────────────────────────────────

/// Deserialize a value of type `T` from EDIFACT bytes.
///
/// Buffers every parsed segment into a `Vec<Segment<'_>>` before handing it to
/// `T`. For large interchanges prefer [`deserialize_each`] (one segment at a
/// time) or [`deserialize_messages`] (one message at a time).
///
/// # Errors
///
/// Any parse error, or whatever `T` reports.
pub fn deserialize<T: EdifactDeserialize>(input: &[u8]) -> Result<T, EdifactError> {
    let segments: Vec<Segment<'_>> = crate::from_bytes(input).collect::<Result<_, _>>()?;
    T::edifact_deserialize(&segments)
}

/// Deserialize a value of type `T` from an EDIFACT string.
///
/// # Errors
///
/// As [`deserialize`].
pub fn deserialize_str<T: EdifactDeserialize>(input: &str) -> Result<T, EdifactError> {
    deserialize(input.as_bytes())
}

/// Lazily deserialize every segment in `input` that `T` maps to.
///
/// Non-matching segments are never buffered, so memory stays proportional to one
/// segment rather than the whole interchange. Take the first match with
/// `.next()`; collect them all with `.collect::<Result<Vec<_>, _>>()`.
///
/// # Example
///
#[cfg_attr(feature = "derive", doc = "```")]
#[cfg_attr(not(feature = "derive"), doc = "```ignore")]
/// use edifact_rs::{EdifactDeserialize, deserialize_each};
///
/// #[derive(EdifactDeserialize)]
/// #[edifact(segment = "RFF")]
/// struct Rff {
///     #[edifact(element = 0, component = 1)]
///     number: String,
/// }
///
/// let input = b"UNH+1+ORDERS:D:96A:UN'RFF+ON:A'BGM+220'RFF+ON:B'UNT+5+1'";
/// let refs: Vec<Rff> = deserialize_each(input).collect::<Result<_, _>>()?;
///
/// assert_eq!(refs.len(), 2);
/// assert_eq!(refs[1].number, "B");
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub fn deserialize_each<'a, T>(
    input: &'a [u8],
) -> impl Iterator<Item = Result<T, EdifactError>> + 'a
where
    T: EdifactDeserialize + EdifactSegmentTag + 'a,
{
    deserialize_matching(crate::from_bytes(input))
}

/// Lazily deserialize every segment from a reader that `T` maps to.
///
/// The reader counterpart of [`deserialize_each`], with the same bounded-memory
/// behaviour.
pub fn deserialize_each_from_reader<T, R>(
    reader: R,
) -> impl Iterator<Item = Result<T, EdifactError>>
where
    T: EdifactDeserialize + EdifactSegmentTag,
    R: Read,
{
    deserialize_matching(crate::from_reader(reader))
}

/// Shared driver behind [`deserialize_each`] and [`deserialize_each_from_reader`].
fn deserialize_matching<'a, T, I>(segments: I) -> impl Iterator<Item = Result<T, EdifactError>>
where
    T: EdifactDeserialize + EdifactSegmentTag,
    I: Iterator<Item = Result<Segment<'a>, EdifactError>>,
{
    segments.filter_map(|segment| match segment {
        Ok(segment) if T::matches_segment(&segment) => {
            Some(T::edifact_deserialize(std::slice::from_ref(&segment)))
        }
        Ok(_) => None,
        Err(error) => Some(Err(error)),
    })
}

// ── segment lookup ────────────────────────────────────────────────────────────

/// Find the first segment with the given tag.
pub fn find_segment<'s, 'd>(segments: &'s [Segment<'d>], tag: &str) -> Option<&'s Segment<'d>> {
    segments.iter().find(|s| s.tag == tag)
}

/// Iterate over all segments with the given tag.
pub fn find_segments<'s, 'd: 's>(
    segments: &'s [Segment<'d>],
    tag: &'s str,
) -> impl Iterator<Item = &'s Segment<'d>> {
    segments.iter().filter(move |s| s.tag == tag)
}

/// Find the first segment matching `tag` whose element 0 equals `qualifier`.
pub fn find_qualified_segment<'s, 'd>(
    segments: &'s [Segment<'d>],
    tag: &str,
    qualifier: &str,
) -> Option<&'s Segment<'d>> {
    segments
        .iter()
        .find(|s| s.tag == tag && s.element_str(0).unwrap_or("") == qualifier)
}

/// Iterate over every segment that the type `T` maps to (tag plus qualifier pattern).
pub fn find_segments_typed<'s, 'd: 's, T>(
    segments: &'s [Segment<'d>],
) -> impl Iterator<Item = &'s Segment<'d>>
where
    T: EdifactSegmentTag,
{
    segments.iter().filter(|s| T::matches_segment(s))
}

/// Iterate lazily over contiguous runs of segments that `T` maps to.
///
/// Each item is a borrowed sub-slice of `segments` covering one uninterrupted
/// run of matches — the shape a repeating segment group has on the wire.
///
/// # Example
///
/// ```
/// use edifact_rs::{EdifactSegmentTag, contiguous_groups, from_bytes};
///
/// struct Loc;
/// impl EdifactSegmentTag for Loc {
///     const SEGMENT_TAG: &'static str = "LOC";
/// }
///
/// let segments: Vec<_> = from_bytes(b"LOC+1'LOC+2'DTM+137'LOC+3'")
///     .collect::<Result<Vec<_>, _>>()?;
/// let runs: Vec<usize> = contiguous_groups::<Loc>(&segments).map(<[_]>::len).collect();
///
/// assert_eq!(runs, [2, 1]);
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub fn contiguous_groups<'s, 'd, T>(
    segments: &'s [Segment<'d>],
) -> impl Iterator<Item = &'s [Segment<'d>]> + 's
where
    T: EdifactSegmentTag,
{
    let mut idx = 0;
    let len = segments.len();
    std::iter::from_fn(move || {
        while idx < len && !T::matches_segment(&segments[idx]) {
            idx += 1;
        }
        if idx >= len {
            return None;
        }
        let start = idx;
        idx += 1;
        while idx < len && T::matches_segment(&segments[idx]) {
            idx += 1;
        }
        Some(&segments[start..idx])
    })
}

/// Match a qualifier value against an exact or wildcard pattern.
///
/// Rules:
/// - If `pattern` contains `*`, it is treated as a glob wildcard (e.g. `"M*"` matches `"MS"`, `"MR"`).
/// - If no wildcard is present, exact match is required.
///
/// Prefix matching without an explicit `*` is deliberately *not* supported: `"M"`
/// matches only `"M"`, not `"MS"`. Use `"M*"` for prefix semantics.
///
/// Patterns with more than three wildcard-separated gaps (four or more `*`) are
/// rejected outright, guarding against pathological O(n·m) matching.
pub fn qualifier_matches_pattern(value: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }

    if !pattern.contains('*') {
        return value == pattern;
    }

    // Fast path: single wildcard (dominant case — e.g. "M*" or "*:MS").
    // The length test is what stops the prefix and the suffix from overlapping:
    // `value.len() >= prefix.len() + suffix.len()` is exactly the condition that
    // leaves a (possibly empty) gap between them for `*` to cover.
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        if !suffix.contains('*') {
            return value.len() >= prefix.len() + suffix.len()
                && value.starts_with(prefix)
                && value.ends_with(suffix);
        }
    }

    // General multi-wildcard path.
    let parts: smallvec::SmallVec<[&str; 4]> = pattern.split('*').collect();

    // EDIFACT qualifier patterns use at most one or two wildcards; four is a
    // generous ceiling.  Anything beyond is a programming error or adversarial
    // input — reject immediately rather than backtrack.
    if parts.len() > 4 {
        return false;
    }

    let prefix = parts[0];
    let suffix = parts[parts.len() - 1];

    if !value.starts_with(prefix) || !value.ends_with(suffix) {
        return false;
    }

    let mid_start = prefix.len();
    let mid_end = value.len().saturating_sub(suffix.len());

    if mid_start > mid_end {
        return parts[1..parts.len() - 1].iter().all(|p| p.is_empty());
    }

    let mut remaining = &value[mid_start..mid_end];

    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        match remaining.find(part) {
            Some(idx) => remaining = &remaining[idx + part.len()..],
            None => return false,
        }
    }

    true
}

// ── composite elements ────────────────────────────────────────────────────────

/// A composite data element, flattened to its component strings.
///
/// Holds borrowed `&'a str` references to the underlying data — no string
/// copies are made. Up to four component pointers are stored inline, so the
/// common case is allocation-free.
pub struct CompositeElement<'a> {
    components: smallvec::SmallVec<[&'a str; 4]>,
}

impl<'a> CompositeElement<'a> {
    /// Build a composite view over a slice of component values.
    pub fn from_slice(components: &'a [Cow<'a, str>]) -> Self {
        Self {
            components: components.iter().map(|c| c.as_ref()).collect(),
        }
    }

    /// Crate-private constructor for direct `&str` components.
    pub(crate) fn from_strs(components: smallvec::SmallVec<[&'a str; 4]>) -> Self {
        Self { components }
    }

    /// Get the component at index `i`, or `None` if absent.
    pub fn get(&self, i: usize) -> Option<&'a str> {
        self.components.get(i).copied()
    }

    /// Get the component at index `i`, or `""` if absent.
    pub fn get_or_empty(&self, i: usize) -> &'a str {
        self.get(i).unwrap_or("")
    }

    /// Number of components.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// Returns `true` when the composite carries no components.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Iterate over all component values.
    pub fn iter(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.components.iter().copied()
    }
}

/// View element `idx` of `seg` as a [`CompositeElement`].
pub fn composite_element<'a, 'd: 'a>(
    seg: &'a Segment<'d>,
    idx: usize,
) -> Option<CompositeElement<'a>> {
    seg.elements
        .get(idx)
        .map(|elem| CompositeElement::from_strs(elem.components().collect()))
}

// ── message-window streaming ──────────────────────────────────────────────────

/// A complete `UNH..UNT` message, lifted out of an interchange.
///
/// Produced by [`message_windows`] and [`message_windows_from_reader`].
/// `message_type` and `association_code` are read off the `UNH` at construction
/// time, so routing logic never has to traverse `segments` itself.
///
/// `segments` holds the **full** window, `UNH` and `UNT` included, so that
/// envelope-aware consumers can reach them; [`body`][Self::body] is the view
/// without them.
#[derive(Debug, Clone)]
pub struct MessageWindow<'a> {
    /// EDIFACT message type from `UNH` element 1, component 0 (DE 0065).
    pub message_type: Option<Cow<'a, str>>,
    /// Association-assigned code from `UNH` element 1, component 4 (DE 0057).
    pub association_code: Option<Cow<'a, str>>,
    /// All segments in this window, from `UNH` through `UNT` inclusive.
    pub segments: Vec<Segment<'a>>,
}

/// A [`MessageWindow`] that owns its text — what the reader path produces.
pub type OwnedMessageWindow = MessageWindow<'static>;

impl<'a> MessageWindow<'a> {
    /// The message **body**: everything between `UNH` and `UNT`, exclusive.
    ///
    /// [`segments`][Self::segments] deliberately includes the service segments so
    /// that envelope-aware consumers can read them, but they are exactly what a
    /// body-oriented pass does not want. In particular
    /// [`group_segments_indexed`][crate::group_segments_indexed] is driven by
    /// trigger tags alone, so a trailing `UNT` lands inside whichever group ran
    /// last — pass `body()` and it cannot.
    ///
    /// Missing service segments are tolerated: a window that somehow lacks its
    /// `UNH` or `UNT` yields whatever it does have, rather than panicking.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::message_windows;
    ///
    /// let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+A+9'UNT+3+1'";
    /// let windows: Vec<_> = message_windows(input).collect::<Result<Vec<_>, _>>()?;
    ///
    /// assert_eq!(windows[0].segments.len(), 3); // UNH, BGM, UNT
    /// assert_eq!(
    ///     windows[0].body().iter().map(edifact_rs::Segment::tag).collect::<Vec<_>>(),
    ///     ["BGM"],
    /// );
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[must_use]
    pub fn body(&self) -> &[Segment<'a>] {
        let start = usize::from(self.segments.first().is_some_and(|s| s.tag == "UNH"));
        let end = self.segments.len().saturating_sub(usize::from(
            self.segments.last().is_some_and(|s| s.tag == "UNT"),
        ));
        self.segments.get(start..end).unwrap_or(&[])
    }

    /// Build a window from a completed segment buffer, reading the `UNH` metadata.
    fn from_segments(segments: Vec<Segment<'a>>) -> Self {
        let unh = segments.first().filter(|s| s.tag == "UNH");
        let component = |idx: usize| -> Option<Cow<'a, str>> {
            unh?.elements
                .get(1)?
                .components
                .get(idx)
                .map(|(c, _)| c.clone())
                .filter(|c| !c.is_empty())
        };
        Self {
            message_type: component(0),
            association_code: component(4),
            segments,
        }
    }
}

/// Groups a segment stream into per-message `UNH..UNT` windows.
///
/// One iterator for both parsing paths: wrap [`from_bytes`][crate::from_bytes]
/// and the windows borrow from the input; wrap
/// [`from_reader`][crate::from_reader] and they own their text. Envelope
/// segments outside any `UNH..UNT` pair are skipped.
///
/// # Errors
///
/// - An inner-iterator error is forwarded immediately and iteration stops.
/// - A `UNH` seen while a prior window is still open (missing `UNT`) is an error.
/// - Input that ends with a window still open yields
///   [`EdifactError::UnexpectedEof`] before returning `None`, so a truncated
///   stream can never be mistaken for a complete one.
pub struct MessageWindows<'a, I> {
    inner: I,
    buf: Vec<Segment<'a>>,
    in_message: bool,
    /// Set after any terminal condition so later `next()` calls return `None`.
    done: bool,
}

impl<'a, I> MessageWindows<'a, I>
where
    I: Iterator<Item = Result<Segment<'a>, EdifactError>>,
{
    /// Wrap any segment iterator as a message-window iterator.
    pub fn new(inner: I) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            in_message: false,
            done: false,
        }
    }
}

impl<'a, I> Iterator for MessageWindows<'a, I>
where
    I: Iterator<Item = Result<Segment<'a>, EdifactError>>,
{
    type Item = Result<MessageWindow<'a>, EdifactError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            let segment = match self.inner.next() {
                Some(Ok(s)) => s,
                Some(Err(e)) => {
                    self.done = true;
                    return Some(Err(e));
                }
                None => {
                    self.done = true;
                    // A window that opened but never closed means the stream was
                    // truncated — surfacing it as an error is what stops a caller
                    // from accepting a partial message as a whole one.
                    if self.in_message && !self.buf.is_empty() {
                        self.in_message = false;
                        let offset = self.buf.last().map(|s| s.span.end).unwrap_or(0);
                        return Some(Err(EdifactError::UnexpectedEof { offset }));
                    }
                    return None;
                }
            };

            match segment.tag() {
                "UNH" => {
                    if self.in_message {
                        self.buf.clear();
                        self.in_message = false;
                        self.done = true;
                        return Some(Err(EdifactError::InvalidSegmentForMessage {
                            tag: "UNH".to_owned(),
                            message_type: "ENVELOPE".to_owned(),
                            span: segment.span,
                        }));
                    }
                    self.buf.clear();
                    self.in_message = true;
                    self.buf.push(segment);
                }
                "UNT" if self.in_message => {
                    self.buf.push(segment);
                    self.in_message = false;
                    let segments = std::mem::take(&mut self.buf);
                    return Some(Ok(MessageWindow::from_segments(segments)));
                }
                _ if self.in_message => self.buf.push(segment),
                // Envelope segment outside a window — skip.
                _ => {}
            }
        }
    }
}

/// Split EDIFACT bytes into one [`MessageWindow`] per `UNH`/`UNT` pair.
///
/// Segment text borrows from `input`. Envelope segments (`UNB`, `UNZ`, …) are
/// skipped automatically.
///
/// # Example
///
/// ```
/// use edifact_rs::message_windows;
///
/// let input = b"UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'\
///               UNH+1+ORDERS:D:96A:UN'BGM+220+PO-001+9'UNT+3+1'\
///               UNZ+1+1'";
///
/// let windows: Vec<_> = message_windows(input).collect::<Result<Vec<_>, _>>()?;
///
/// assert_eq!(windows.len(), 1);
/// assert_eq!(windows[0].message_type.as_deref(), Some("ORDERS"));
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub fn message_windows(input: &[u8]) -> MessageWindows<'_, crate::FromBytesIter<'_>> {
    MessageWindows::new(crate::from_bytes(input))
}

/// Split a reader into one [`OwnedMessageWindow`] per `UNH`/`UNT` pair.
///
/// Reads lazily: only enough input to complete one window is consumed per
/// [`Iterator::next`] call, so peak memory is one message, not one interchange.
pub fn message_windows_from_reader<R: Read>(
    reader: R,
) -> MessageWindows<'static, crate::FromReaderIter<R>> {
    MessageWindows::new(crate::from_reader(reader))
}

/// Deserialize one `T` per `UNH`/`UNT` message in `input`.
///
/// # Example
///
#[cfg_attr(feature = "derive", doc = "```")]
#[cfg_attr(not(feature = "derive"), doc = "```ignore")]
/// use edifact_rs::{EdifactDeserialize, deserialize_messages};
///
/// #[derive(EdifactDeserialize)]
/// #[edifact(segment = "BGM")]
/// struct Bgm {
///     #[edifact(element = 1)]
///     number: String,
/// }
///
/// let input = b"UNB+UNOA:1+S+R+200101:0900+1'\
///               UNH+1+ORDERS:D:96A:UN'BGM+220+PO-1+9'UNT+3+1'\
///               UNH+2+ORDERS:D:96A:UN'BGM+220+PO-2+9'UNT+3+2'\
///               UNZ+2+1'";
///
/// let orders: Vec<Bgm> = deserialize_messages(input).collect::<Result<_, _>>()?;
/// assert_eq!(orders[1].number, "PO-2");
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub fn deserialize_messages<'a, T>(
    input: &'a [u8],
) -> impl Iterator<Item = Result<T, EdifactError>> + 'a
where
    T: EdifactDeserialize + 'a,
{
    message_windows(input).map(|window| T::edifact_deserialize(&window?.segments))
}

/// Deserialize one `T` per `UNH`/`UNT` message read from `reader`.
///
/// The highest-level streaming API: one `T` per message, reading only as much
/// input as each window needs.
pub fn deserialize_messages_from_reader<T, R>(
    reader: R,
) -> impl Iterator<Item = Result<T, EdifactError>>
where
    T: EdifactDeserialize,
    R: Read,
{
    message_windows_from_reader(reader).map(|window| T::edifact_deserialize(&window?.segments))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── manual test impl ──────────────────────────────────────────────────────
    #[derive(Debug, PartialEq)]
    struct BgmSegment {
        doc_name_code: String,
        pruef_id: String,
        msg_function: Option<String>,
    }

    impl EdifactSegmentTag for BgmSegment {
        const SEGMENT_TAG: &'static str = "BGM";
    }

    struct NadM;

    impl EdifactSegmentTag for NadM {
        const SEGMENT_TAG: &'static str = "NAD";
        const QUALIFIER_PATTERN: Option<&'static str> = Some("M*");
    }

    impl EdifactDeserialize for BgmSegment {
        fn edifact_deserialize(segments: &[Segment<'_>]) -> Result<Self, EdifactError> {
            let seg = find_segment(segments, "BGM").ok_or_else(|| {
                EdifactError::MissingRequiredElement {
                    tag: "BGM".to_owned(),
                    element_index: 0,
                }
            })?;
            Ok(Self {
                doc_name_code: seg.element_str(0).unwrap_or("").to_owned(),
                pruef_id: seg.element_str(1).unwrap_or("").to_owned(),
                msg_function: seg.optional_element(2).map(str::to_owned),
            })
        }
    }

    #[test]
    fn deserialize_single_segment() {
        let input = b"BGM+E03+11042+9'";
        let bgm: BgmSegment = deserialize(input).unwrap();
        assert_eq!(bgm.doc_name_code, "E03");
        assert_eq!(bgm.pruef_id, "11042");
        assert_eq!(bgm.msg_function, Some("9".to_owned()));
    }

    #[test]
    fn deserialize_each_is_lazy_over_both_sources() {
        let input = b"BGM+E03+11042+9'RFF+AA:1'BGM+E01+11043+9'";

        let from_slice: Vec<BgmSegment> =
            deserialize_each(input).collect::<Result<_, _>>().unwrap();
        let from_reader: Vec<BgmSegment> =
            deserialize_each_from_reader(std::io::Cursor::new(&input[..]))
                .collect::<Result<_, _>>()
                .unwrap();

        assert_eq!(from_slice, from_reader);
        assert_eq!(from_slice.len(), 2);
        assert_eq!(from_slice[1].pruef_id, "11043");
    }

    #[test]
    fn deserialize_each_stops_at_the_first_match_when_asked() {
        let input = b"UNH+1+ORDERS:D:11A:UN'BGM+E03+11042+9'UNT+3+1'";
        let first: BgmSegment = deserialize_each(input).next().unwrap().unwrap();
        assert_eq!(first.pruef_id, "11042");
    }

    #[test]
    fn qualifier_patterns_match_the_documented_way() {
        assert!(qualifier_matches_pattern("MS", "M*"));
        assert!(!qualifier_matches_pattern("MS", "M"));
        assert!(qualifier_matches_pattern("MS", "MS"));
        assert!(qualifier_matches_pattern("", ""));
        // Adversarial patterns are refused rather than backtracked.
        assert!(!qualifier_matches_pattern("aaaa", "*a*a*a*a*"));
    }

    #[test]
    fn typed_qualifier_matching_filters_by_element_zero() {
        let segments: Vec<Segment<'_>> = crate::from_bytes(b"NAD+MS+1'NAD+BY+2'NAD+MR+3'")
            .collect::<Result<_, _>>()
            .unwrap();
        let matched: Vec<&str> = find_segments_typed::<NadM>(&segments)
            .map(|s| s.element_str(1).unwrap())
            .collect();
        assert_eq!(matched, ["1", "3"]);
    }

    #[test]
    fn message_windows_agree_across_both_parsing_paths() {
        let input = b"UNB+UNOA:1+S+R+200101:0900+1'\
                      UNH+1+ORDERS:D:96A:UN'BGM+220+A+9'UNT+3+1'\
                      UNH+2+ORDERS:D:96A:UN'BGM+220+B+9'UNT+3+2'\
                      UNZ+2+1'";

        let sliced: Vec<_> = message_windows(input)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let streamed: Vec<_> = message_windows_from_reader(std::io::Cursor::new(&input[..]))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(sliced.len(), 2);
        assert_eq!(sliced.len(), streamed.len());
        for (a, b) in sliced.iter().zip(&streamed) {
            assert_eq!(a.message_type, b.message_type);
            assert_eq!(a.body().len(), b.body().len());
        }
    }

    #[test]
    fn a_truncated_window_is_an_error_not_a_short_message() {
        let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+A+9'";
        let err = message_windows(input)
            .collect::<Result<Vec<_>, _>>()
            .expect_err("an unclosed UNH must not pass as a complete message");
        assert!(matches!(err, EdifactError::UnexpectedEof { .. }));
    }
}
