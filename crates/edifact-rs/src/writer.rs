//! EDIFACT writer — serializes [`Segment`]s to wire format.

use crate::{
    charset::Charset, error::EdifactError, model::Segment, tokenizer::ServiceStringAdvice,
};
use std::borrow::Cow;
use std::io::Write;

/// One data element of a segment being written: simple or composite.
///
/// The everyday EDIFACT segment mixes both shapes — `NAD+MS+id::agency`,
/// `DTM+137:20260101:102` — and this enum lets a single call express that
/// without pre-joining components into a string (which loses the distinction
/// between a separator and a literal `:` in a value).
///
/// `From` impls cover the common literals, so `"MS".into()` and
/// `["a", "", "b"].into()` both work; the [`elements!`][crate::elements] macro
/// wraps that up entirely.
///
/// # Example
///
/// ```rust
/// use edifact_rs::{DataElement, Writer};
///
/// let mut w = Writer::new(Vec::new());
/// w.write_elements(
///     "NAD",
///     &[
///         DataElement::Simple("MS"),
///         DataElement::Composite(&["9900112233445", "", "293"]),
///     ],
/// )?;
/// assert_eq!(w.finish()?, b"NAD+MS+9900112233445::293'".to_vec());
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataElement<'a> {
    /// A simple data element — one value, no component separators.
    Simple(&'a str),
    /// A composite data element — components written in order, separated by the
    /// active component separator.  A separator byte *inside* a component value
    /// is escaped rather than promoted to a boundary.
    Composite(&'a [&'a str]),
}

impl<'a> DataElement<'a> {
    /// The components of this element, as a slice.
    #[inline]
    #[must_use]
    pub fn components(&self) -> &[&'a str] {
        match self {
            Self::Simple(value) => std::slice::from_ref(value),
            Self::Composite(components) => components,
        }
    }
}

impl<'a> From<&'a str> for DataElement<'a> {
    #[inline]
    fn from(value: &'a str) -> Self {
        Self::Simple(value)
    }
}

impl<'a> From<&'a [&'a str]> for DataElement<'a> {
    #[inline]
    fn from(components: &'a [&'a str]) -> Self {
        Self::Composite(components)
    }
}

impl<'a, const N: usize> From<&'a [&'a str; N]> for DataElement<'a> {
    #[inline]
    fn from(components: &'a [&'a str; N]) -> Self {
        Self::Composite(components)
    }
}

/// Borrow a value as a [`DataElement`], choosing simple or composite by type.
///
/// A single string borrows as [`DataElement::Simple`]; an array, slice, or `Vec`
/// of strings borrows as [`DataElement::Composite`]. This is what lets the
/// [`elements!`][crate::elements] macro accept both shapes from arbitrary
/// expressions rather than only from literals.
///
/// # Example
///
/// ```rust
/// use edifact_rs::{AsDataElement, DataElement};
///
/// let qualifier = String::from("MS");
/// let party = ["9900112233445", "", "293"];
///
/// assert_eq!(qualifier.as_data_element(), DataElement::Simple("MS"));
/// assert_eq!(
///     party.as_data_element(),
///     DataElement::Composite(&["9900112233445", "", "293"]),
/// );
/// ```
pub trait AsDataElement {
    /// Borrow `self` as a [`DataElement`].
    fn as_data_element(&self) -> DataElement<'_>;
}

impl AsDataElement for str {
    #[inline]
    fn as_data_element(&self) -> DataElement<'_> {
        DataElement::Simple(self)
    }
}

impl AsDataElement for &str {
    #[inline]
    fn as_data_element(&self) -> DataElement<'_> {
        DataElement::Simple(self)
    }
}

impl AsDataElement for String {
    #[inline]
    fn as_data_element(&self) -> DataElement<'_> {
        DataElement::Simple(self.as_str())
    }
}

impl AsDataElement for Cow<'_, str> {
    #[inline]
    fn as_data_element(&self) -> DataElement<'_> {
        DataElement::Simple(self.as_ref())
    }
}

impl<const N: usize> AsDataElement for [&str; N] {
    #[inline]
    fn as_data_element(&self) -> DataElement<'_> {
        DataElement::Composite(self)
    }
}

impl AsDataElement for [&str] {
    #[inline]
    fn as_data_element(&self) -> DataElement<'_> {
        DataElement::Composite(self)
    }
}

impl AsDataElement for Vec<&str> {
    #[inline]
    fn as_data_element(&self) -> DataElement<'_> {
        DataElement::Composite(self)
    }
}

impl AsDataElement for DataElement<'_> {
    #[inline]
    fn as_data_element(&self) -> DataElement<'_> {
        *self
    }
}

/// Build a `&[`[`DataElement`]`]` from a mix of simple values and component lists.
///
/// Each entry is an arbitrary expression borrowed through
/// [`AsDataElement`]: a string becomes a simple data element, an array or slice
/// of strings becomes a composite. This is the shorthand for the mixed-segment
/// shape that dominates real EDIFACT:
///
/// ```rust
/// use edifact_rs::{Writer, elements};
///
/// // Runtime values, not just literals — the everyday builder shape.
/// let qualifier = String::from("MS");
/// let gln = "9900112233445";
///
/// let mut w = Writer::new(Vec::new());
/// w.write_elements("NAD", elements![qualifier.as_str(), [gln, "", "293"]])?;
/// w.write_elements("DTM", elements![["137", "20260101", "102"]])?;
/// assert_eq!(
///     w.finish()?,
///     b"NAD+MS+9900112233445::293'DTM+137:20260101:102'".to_vec(),
/// );
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
///
/// Composite components must be string *slices*: a `[String; N]` cannot borrow
/// as `&[&str]` without allocating, so write `[id.as_str(), "", agency]`.
///
/// The expansion borrows temporaries, so the result must be consumed within the
/// same statement — passing it directly as an argument, as above, always is.
#[macro_export]
macro_rules! elements {
    () => {
        &[] as &[$crate::DataElement<'_>]
    };
    ($($element:expr),+ $(,)?) => {
        &[$($crate::AsDataElement::as_data_element(&$element)),+][..]
    };
}

/// Streaming EDIFACT writer.
///
/// Wraps any [`Write`] implementation and serializes segments one at a time.
/// Call [`Writer::finish`] to flush and get the underlying writer back.
///
/// # Wrap unbuffered sinks
///
/// `Writer` issues a separate write for each tag, delimiter, and value chunk, so
/// a segment costs roughly one write per component. Against an in-memory
/// `Vec<u8>` that is free, but against a [`File`][std::fs::File] or a socket each
/// one is a syscall.
///
/// The writer deliberately does **not** buffer internally: an internal buffer
/// would silently discard everything not yet flushed if the writer were dropped
/// without [`finish`][Self::finish]. Wrap the sink instead, which makes the
/// buffering visible and keeps the flush contract in one place:
///
/// ```rust
/// use std::io::BufWriter;
/// use edifact_rs::Writer;
///
/// let sink = Vec::new(); // stands in for a File or TcpStream
/// let mut writer = Writer::new(BufWriter::new(sink));
/// writer.write_raw("BGM", &["220"])?;
/// // `finish` flushes the `Writer` and hands the `BufWriter` back.
/// let buffered = writer.finish()?;
/// assert_eq!(buffered.into_inner().unwrap(), b"BGM+220'".to_vec());
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub struct Writer<W: Write> {
    inner: W,
    ssa: ServiceStringAdvice,
    /// Running count of segments written.  `u64` to prevent silent overflow on
    /// pathological inputs (a `u32` would wrap after ~4 billion segments).
    segment_count: u64,
    /// `segment_count` as of the most recent `UNH`, used by [`Writer::finish_unt`]
    /// to derive a per-message DE 0074 rather than a writer-lifetime total.
    message_start_count: u64,
    /// Whether the segment currently being written incrementally (via the
    /// event-emitter path) is a `UNH`.  The whole-segment methods pass the tag
    /// to `end_segment` directly; the emitter only sees it at `StartSegment`.
    open_segment_is_unh: bool,
    /// Repertoire every value is encoded into, when the writer is bound to one.
    ///
    /// `None` emits UTF-8 unchecked, which is correct for `UNOY` and for any
    /// payload that happens to be ASCII.
    charset: Option<Charset>,
}

/// Return the offset of the first byte in `hay` that must be release-escaped.
///
/// The escape set is the four splitting delimiters plus the repetition separator
/// when the active UNA declares one.  A space at UNA position 7 is the
/// conventional "not used" sentinel and is never escaped.
#[inline]
fn find_escape(ssa: &ServiceStringAdvice, hay: &[u8]) -> Option<usize> {
    let first = memchr::memchr3(ssa.element_sep, ssa.component_sep, ssa.release_char, hay);
    let second = if ssa.repetition_sep == b' ' {
        memchr::memchr(ssa.segment_term, hay)
    } else {
        memchr::memchr2(ssa.segment_term, ssa.repetition_sep, hay)
    };
    match (first, second) {
        (None, None) => None,
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (Some(a), Some(b)) => Some(a.min(b)),
    }
}

impl<W: Write> Writer<W> {
    /// Create a new writer with default EDIFACT delimiters.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            ssa: ServiceStringAdvice::default(),
            segment_count: 0,
            message_start_count: 0,
            open_segment_is_unh: false,
            charset: None,
        }
    }

    /// Bind this writer to a character repertoire.
    ///
    /// Every value is then encoded into `charset` rather than emitted as UTF-8,
    /// and a character the repertoire cannot carry is rejected with
    /// [`EdifactError::CharacterNotInRepertoire`] instead of being written as
    /// bytes the receiver decodes as something else.
    ///
    /// This is the write-side counterpart of
    /// [`decode_interchange`][crate::decode_interchange]: a `UNOC` interchange
    /// must go out as ISO 8859-1, not UTF-8, or `ü` arrives as two mojibake
    /// characters.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{Charset, Writer};
    ///
    /// let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoC);
    /// writer.write_composites("NAD", &[&["BY"], &["Müller"]])?;
    /// // `ü` goes out as the single Latin-1 byte 0xFC.
    /// assert_eq!(writer.finish()?, b"NAD+BY+M\xFCller'".to_vec());
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    ///
    /// A value outside the repertoire is refused:
    ///
    /// ```
    /// use edifact_rs::{Charset, EdifactError, Writer};
    ///
    /// let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoA);
    /// // Level A is upper-case only.
    /// let err = writer.write_composites("NAD", &[&["BY"], &["Müller"]]).unwrap_err();
    /// assert!(matches!(err, EdifactError::CharacterNotInRepertoire { .. }));
    /// ```
    #[must_use]
    pub fn with_charset(mut self, charset: Charset) -> Self {
        self.charset = Some(charset);
        self
    }

    /// The repertoire this writer encodes into, if it is bound to one.
    #[must_use]
    pub fn charset(&self) -> Option<Charset> {
        self.charset
    }

    /// Create a writer with custom delimiters and write a UNA segment first.
    pub fn with_una(mut inner: W, ssa: ServiceStringAdvice) -> Result<Self, EdifactError> {
        // All five active service characters must be mutually distinct, non-whitespace,
        // and within the ASCII range so they never bisect multi-byte UTF-8 sequences.
        if !ssa.is_valid() {
            return Err(EdifactError::InvalidUna);
        }
        // UNA: component_sep, element_sep, decimal_mark, release_char, repetition_sep, segment_term
        let una = [
            b'U',
            b'N',
            b'A',
            ssa.component_sep,
            ssa.element_sep,
            ssa.decimal_mark,
            ssa.release_char,
            ssa.repetition_sep,
            ssa.segment_term,
        ];
        inner.write_all(&una)?;
        Ok(Self {
            inner,
            ssa,
            segment_count: 0,
            message_start_count: 0,
            open_segment_is_unh: false,
            charset: None,
        })
    }

    /// Record the end of a segment: terminator, count, and `UNH` bookkeeping.
    ///
    /// Every emit path funnels through here.  Two of them used to forget the
    /// `UNH` marker, so [`finish_unt`][Self::finish_unt] derived DE 0074 from
    /// the writer-lifetime total whenever a message header happened to be
    /// written with [`write_segment`][Self::write_segment].
    #[inline]
    fn end_segment(&mut self, tag: &str) -> Result<(), EdifactError> {
        self.inner.write_all(&[self.ssa.segment_term])?;
        if tag == "UNH" {
            self.message_start_count = self.segment_count;
        }
        self.segment_count += 1;
        Ok(())
    }

    /// Write a single segment, including any ISO 9735-4 repetitions.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::RepetitionSeparatorNotDeclared`] when the segment
    /// carries a repeating data element but the active service string advice
    /// declares no repetition separator.  The check runs **before** any byte is
    /// written, so a rejected segment leaves nothing behind in the sink — a
    /// half-written `RFF+` would otherwise corrupt the interchange for every
    /// caller that recovers from the error and carries on.
    pub fn write_segment(&mut self, seg: &Segment<'_>) -> Result<(), EdifactError> {
        // The sentinel at UNA position 7 is a space.  Emitting it as a separator
        // would produce output that reads back as a single occurrence whose
        // value contains a space — corrupt, and quietly so.  Refusing is the
        // only honest option, and refusing before the first write is the only
        // one that keeps the sink consistent.
        if !self.ssa.is_repetition_active() && seg.elements.iter().any(|e| !e.repeats.is_empty()) {
            return Err(EdifactError::RepetitionSeparatorNotDeclared);
        }

        self.inner.write_all(seg.tag.as_bytes())?;

        for element in &seg.elements {
            self.inner.write_all(&[self.ssa.element_sep])?;
            for (repetition, components) in element.repetitions().enumerate() {
                if repetition > 0 {
                    self.inner.write_all(&[self.ssa.repetition_sep])?;
                }
                for (i, (component, _)) in components.iter().enumerate() {
                    if i > 0 {
                        self.inner.write_all(&[self.ssa.component_sep])?;
                    }
                    self.write_escaped(component)?;
                }
            }
        }

        self.end_segment(seg.tag)
    }

    /// Write a raw segment from tag + element string slices.
    ///
    /// Each element string is split on the **active component-separator byte** from the
    /// configured [`ServiceStringAdvice`][crate::ServiceStringAdvice] to identify component
    /// boundaries.  The default component separator is `:` (0x3A), but this can differ when a
    /// non-default `UNA` string was used to construct the writer.
    ///
    /// # Delimiter dependency
    ///
    /// Callers that embed the literal `:` character in element strings rely on `:` being
    /// the component separator.  When the writer uses a non-default delimiter set, `:` will
    /// **not** be treated as a component boundary and the segment will be written incorrectly.
    ///
    /// **UTF-8 safety**: EDIFACT syntax requires all delimiter bytes to be single-byte ASCII
    /// characters (values 0x00–0x7F).  Non-ASCII delimiter bytes would bisect multi-byte UTF-8
    /// sequences in data values and produce malformed output.  All fields of
    /// [`ServiceStringAdvice`][crate::ServiceStringAdvice] must therefore hold ASCII byte values.
    ///
    /// To produce correct output regardless of the active delimiter, prefer
    /// [`Self::write_elements`] — it takes component boundaries explicitly and
    /// handles the mixed simple/composite shape that most real segments have.
    /// [`Self::write_segment_parts`] is the equivalent for owned data.
    pub fn write_raw(&mut self, tag: &str, elements: &[&str]) -> Result<(), EdifactError> {
        self.inner.write_all(tag.as_bytes())?;
        let comp_sep = self.ssa.component_sep;
        for el in elements {
            self.inner.write_all(&[self.ssa.element_sep])?;
            // Byte-level split: EDIFACT delimiters are always single bytes.
            let mut parts = el.as_bytes().split(|&b| b == comp_sep);
            if let Some(first) = parts.next() {
                // INVARIANT: input is valid UTF-8 and we split on a single-byte ASCII
                // delimiter, so each part remains a valid UTF-8 slice.
                self.write_escaped(
                    std::str::from_utf8(first).map_err(|_| EdifactError::InvalidUtf8)?,
                )?;
            }
            for part in parts {
                self.inner.write_all(&[comp_sep])?;
                self.write_escaped(
                    std::str::from_utf8(part).map_err(|_| EdifactError::InvalidUtf8)?,
                )?;
            }
        }
        self.end_segment(tag)
    }

    /// Write a segment from a tag and pre-split element/component data.
    ///
    /// `elements` is a slice of elements; each element is a sequence of component strings.
    /// This avoids the lifetime constraints of [`Self::write_segment`] when building
    /// segments from runtime-owned data (e.g. inside [`crate::WriterEmitter`]).
    pub fn write_segment_parts<E>(&mut self, tag: &str, elements: &[E]) -> Result<(), EdifactError>
    where
        E: AsRef<[String]>,
    {
        self.inner.write_all(tag.as_bytes())?;
        for element in elements {
            self.inner.write_all(&[self.ssa.element_sep])?;
            let mut first = true;
            for comp in element.as_ref() {
                if !first {
                    self.inner.write_all(&[self.ssa.component_sep])?;
                }
                first = false;
                self.write_escaped(comp.as_str())?;
            }
        }
        self.end_segment(tag)
    }

    /// Write a segment from a tag and borrowed element/component slices.
    ///
    /// Unlike [`Self::write_raw`], component boundaries are given explicitly
    /// rather than inferred by splitting on the active component separator, so
    /// values containing a literal separator byte are escaped instead of being
    /// silently reinterpreted as a composite boundary.  Unlike
    /// [`Self::write_segment_parts`], no `String` allocation is required.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::Writer;
    /// let mut w = Writer::new(Vec::new());
    /// // The `:` inside the sender id stays part of the value.
    /// w.write_composites("NAD", &[&["MS"][..], &["ACME:INC"][..]])?;
    /// assert_eq!(w.finish()?, b"NAD+MS+ACME?:INC'".to_vec());
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if the underlying writer fails.
    pub fn write_composites(
        &mut self,
        tag: &str,
        elements: &[&[&str]],
    ) -> Result<(), EdifactError> {
        self.inner.write_all(tag.as_bytes())?;
        for element in elements {
            self.inner.write_all(&[self.ssa.element_sep])?;
            for (i, comp) in element.iter().enumerate() {
                if i > 0 {
                    self.inner.write_all(&[self.ssa.component_sep])?;
                }
                self.write_escaped(comp)?;
            }
        }
        self.end_segment(tag)
    }

    /// Write a segment whose data elements mix simple and composite shapes.
    ///
    /// This is the general form of segment emission and the one that matches
    /// how EDIFACT segments are actually specified: `NAD` takes a simple
    /// qualifier followed by a composite party identification, `DTM` takes a
    /// single composite.  [`write_raw`][Self::write_raw] (all-simple, with
    /// separators inferred by splitting) and
    /// [`write_composites`][Self::write_composites] (all-composite) are the two
    /// special cases.
    ///
    /// Component boundaries are explicit, so a value containing the active
    /// component separator is escaped rather than silently promoted to a
    /// boundary.  Nothing is allocated.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edifact_rs::{DataElement, Writer, elements};
    ///
    /// let mut w = Writer::new(Vec::new());
    /// // Explicit form …
    /// w.write_elements(
    ///     "NAD",
    ///     &[DataElement::Simple("MS"), DataElement::Composite(&["ACME:INC", "", "9"])],
    /// )?;
    /// // … or the `elements!` shorthand.
    /// w.write_elements("DTM", elements![["137", "20260101", "102"]])?;
    /// assert_eq!(
    ///     w.finish()?,
    ///     b"NAD+MS+ACME?:INC::9'DTM+137:20260101:102'".to_vec(),
    /// );
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if the underlying writer fails.
    pub fn write_elements(
        &mut self,
        tag: &str,
        elements: &[DataElement<'_>],
    ) -> Result<(), EdifactError> {
        self.inner.write_all(tag.as_bytes())?;
        for element in elements {
            self.inner.write_all(&[self.ssa.element_sep])?;
            for (i, comp) in element.components().iter().enumerate() {
                if i > 0 {
                    self.inner.write_all(&[self.ssa.component_sep])?;
                }
                self.write_escaped(comp)?;
            }
        }
        self.end_segment(tag)
    }

    /// Flush and return the underlying writer.
    pub fn finish(mut self) -> Result<W, EdifactError> {
        self.inner.flush()?;
        Ok(self.inner)
    }

    /// Write the `UNT` segment and return the inner writer.
    ///
    /// The count written into `UNT` DE 0074 covers the current message only:
    /// `UNH`, every segment written since it, and `UNT` itself.  Segments written
    /// before the message's `UNH` — an interchange-level `UNB`, or a preceding
    /// message — are excluded, as EDIFACT requires.
    ///
    /// If no `UNH` has been written, the count falls back to every segment
    /// written so far plus one.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.  Do **not** call [`write_raw`][Self::write_raw] or
    /// [`write_segment`][Self::write_segment] after `finish_unt` — the writer is consumed.
    pub fn finish_unt(mut self, message_ref: &str) -> Result<W, EdifactError> {
        // DE 0074 counts UNH + content + UNT.  `message_start_count` is the
        // absolute segment count immediately after UNH, so content is
        // `segment_count - message_start_count` and the total adds UNH and UNT.
        let count = self.segment_count - self.message_start_count + 1;
        let count_str = count.to_string();
        self.write_composites("UNT", &[&[count_str.as_str()], &[message_ref]])?;
        self.finish()
    }

    /// Returns the total number of segments written so far.
    pub fn segment_count(&self) -> u64 {
        self.segment_count
    }

    /// Returns the active [`ServiceStringAdvice`] (delimiter configuration).
    pub fn service_string_advice(&self) -> ServiceStringAdvice {
        self.ssa
    }

    /// Escape a value string for inclusion in an EDIFACT segment.
    ///
    /// Any character in `value` that matches the active element separator,
    /// component separator, release character, or segment terminator is escaped
    /// by prefixing it with the release character (default `?`).
    ///
    /// Returns a borrowed `Cow::Borrowed(value)` when no escaping is needed,
    /// avoiding an allocation on the fast path.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let writer = Writer::new(std::io::sink());
    /// // '+' must be escaped since it is the default element separator.
    /// assert_eq!(writer.escape_value("price+tax"), "price?+tax");
    /// ```
    pub fn escape_value<'v>(&self, value: &'v str) -> Cow<'v, str> {
        let bytes = value.as_bytes();
        if find_escape(&self.ssa, bytes).is_none() {
            return Cow::Borrowed(value);
        }
        // Built as a `String` from the start.  Assembling a `Vec<u8>` and then
        // re-validating it needed a fallible conversion whose failure branch was
        // unreachable, which is exactly the kind of `expect` that has no business
        // in a library.  Every delimiter is single-byte ASCII (enforced by
        // `ServiceStringAdvice::is_valid`), so each hit lands on a character
        // boundary and both halves of the split are valid `&str`.
        let release = self.ssa.release_char as char;
        let mut out = String::with_capacity(value.len() + 4);
        let mut last = 0;
        while let Some(hit) = find_escape(&self.ssa, &bytes[last..]) {
            let abs = last + hit;
            out.push_str(&value[last..abs]);
            out.push(release);
            out.push(bytes[abs] as char);
            last = abs + 1;
        }
        out.push_str(&value[last..]);
        Cow::Owned(out)
    }
    /// Write only the segment tag bytes — no element separator or terminator.
    ///
    /// Used by [`crate::WriterEmitter`] for eager, zero-allocation event writing.
    #[inline]
    pub(crate) fn write_tag_only(&mut self, tag: &str) -> Result<(), EdifactError> {
        self.inner.write_all(tag.as_bytes())?;
        self.open_segment_is_unh = tag == "UNH";
        Ok(())
    }

    /// Write one element separator byte.
    #[inline]
    pub(crate) fn write_element_sep(&mut self) -> Result<(), EdifactError> {
        self.inner.write_all(&[self.ssa.element_sep])?;
        Ok(())
    }

    /// Write one component separator byte.
    #[inline]
    pub(crate) fn write_component_sep(&mut self) -> Result<(), EdifactError> {
        self.inner.write_all(&[self.ssa.component_sep])?;
        Ok(())
    }

    /// Write the segment terminator and increment the internal segment counter.
    #[inline]
    pub(crate) fn write_segment_term_and_count(&mut self) -> Result<(), EdifactError> {
        let tag = if self.open_segment_is_unh { "UNH" } else { "" };
        self.open_segment_is_unh = false;
        self.end_segment(tag)
    }

    /// Write text, encoding it into the bound repertoire when there is one.
    ///
    /// Callers must only pass slices that start and end on a character boundary.
    /// Every delimiter is single-byte ASCII (enforced by
    /// [`ServiceStringAdvice::is_valid`]), so splitting a value at a delimiter
    /// always satisfies that.
    #[inline]
    fn write_text(&mut self, text: &str) -> Result<(), EdifactError> {
        match self.charset {
            None => self.inner.write_all(text.as_bytes())?,
            Some(charset) => self.inner.write_all(&charset.encode(text)?)?,
        }
        Ok(())
    }

    /// Write a value, escaping any delimiter characters.
    pub(crate) fn write_escaped(&mut self, value: &str) -> Result<(), EdifactError> {
        let release = self.ssa.release_char;
        let bytes = value.as_bytes();
        let mut last = 0;
        let mut pos = 0;
        while pos < bytes.len() {
            let Some(hit) = find_escape(&self.ssa, &bytes[pos..]) else {
                break;
            };
            let abs = pos + hit;
            if abs > last {
                self.write_text(&value[last..abs])?;
            }
            // The escaped byte is a service character, hence ASCII in every
            // repertoire — it needs no encoding pass.
            self.inner.write_all(&[release, bytes[abs]])?;
            last = abs + 1;
            pos = abs + 1;
        }
        self.write_text(&value[last..])
    }

    // ── Interchange envelope helpers ──────────────────────────────────────────

    /// Write a `UNB` interchange header segment.
    ///
    /// Generates:
    /// ```text
    /// UNB+<syntax_id>:<syntax_version>+<sender>+<recipient>+<date>:<time>+<control_ref>'
    /// ```
    ///
    /// Composite components (S001 syntax identifier/version, S004 date/time) are
    /// passed separately rather than pre-joined with `:`, so they are written
    /// with the writer's *active* component separator and so a literal separator
    /// inside `sender`, `recipient`, or `control_ref` is escaped rather than
    /// silently promoted to a component boundary.
    ///
    /// Track the `control_ref` — it must be repeated in the matching
    /// [`end_interchange`](Self::end_interchange) call.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::Writer;
    /// let mut w = Writer::new(Vec::new());
    /// w.begin_interchange("UNOA", "1", "SENDER", "RECEIVER", "200101", "0900", "IC1")?;
    /// assert_eq!(
    ///     w.finish()?,
    ///     b"UNB+UNOA:1+SENDER+RECEIVER+200101:0900+IC1'".to_vec(),
    /// );
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if writing fails.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_interchange(
        &mut self,
        syntax_id: &str,
        syntax_version: &str,
        sender: &str,
        recipient: &str,
        date: &str,
        time: &str,
        control_ref: &str,
    ) -> Result<(), EdifactError> {
        // A header that names one repertoire while the body is encoded in another
        // is the exact silent-corruption failure `with_charset` exists to stop, so
        // a mismatch is refused rather than written.
        if let Some(charset) = self.charset {
            if charset.syntax_identifier() != syntax_id {
                return Err(EdifactError::CharacterRepertoireMismatch {
                    declared: syntax_id.to_owned(),
                    writer: charset.syntax_identifier(),
                });
            }
        }
        self.write_composites(
            "UNB",
            &[
                &[syntax_id, syntax_version],
                &[sender],
                &[recipient],
                &[date, time],
                &[control_ref],
            ],
        )
    }

    /// Write a `UNH` message header and return a [`MessageWriter`] guard.
    ///
    /// The guard tracks the per-message segment count automatically.  Call
    /// [`MessageWriter::finish`] when all message segments have been written — this
    /// writes the matching `UNT` segment with the correct count.  If `finish` is not
    /// called, `Drop` will attempt to write `UNT` as a best-effort fallback (errors
    /// are silently discarded on drop; prefer explicit `finish`).
    ///
    /// Generates:
    /// ```text
    /// UNH+<message_ref>+<message_type>:<version>:<release>:<controlling_agency>'
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if writing the `UNH` segment fails.
    pub fn begin_message<'w>(
        &'w mut self,
        message_ref: &str,
        message_type: &str,
        version: &str,
        release: &str,
        controlling_agency: &str,
    ) -> Result<MessageWriter<'w, W>, EdifactError> {
        // Build S009 as an explicit composite.  Formatting it with a literal `:`
        // and handing it to `write_raw` produced a single collapsed component
        // whenever the writer used a non-default component separator.
        self.write_composites(
            "UNH",
            &[
                &[message_ref],
                &[message_type, version, release, controlling_agency],
            ],
        )?;
        // Capture `segment_count` after writing UNH so `MessageWriter` knows
        // the absolute count that includes UNH.
        let unh_count = self.segment_count;
        Ok(MessageWriter {
            writer: self,
            message_ref: message_ref.to_owned(),
            unh_count,
            finished: false,
        })
    }

    /// Write a `UNZ` interchange trailer segment.
    ///
    /// `message_count` is the number of `UNH`/`UNT` message pairs in the
    /// interchange.  `control_ref` must match the value passed to
    /// [`begin_interchange`](Self::begin_interchange).
    ///
    /// If you used [`begin_message`](Self::begin_message) for every message in the
    /// interchange, `message_count` equals the number of times you called that
    /// method.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if writing fails.
    pub fn end_interchange(
        &mut self,
        message_count: u32,
        control_ref: &str,
    ) -> Result<(), EdifactError> {
        let msg_count_str = message_count.to_string();
        self.write_composites("UNZ", &[&[msg_count_str.as_str()], &[control_ref]])
    }
}

/// RAII guard for a single EDIFACT message within an interchange.
///
/// Obtained from [`Writer::begin_message`].  Writes `UNH` on creation and
/// `UNT` (with the correct per-message segment count) when [`finish`](Self::finish)
/// is called or the guard is dropped.
///
/// Always prefer calling [`finish`](Self::finish) explicitly so that write
/// errors can be propagated.  The `Drop` impl writes `UNT` as a best-effort
/// fallback but silently discards I/O errors.
///
/// # Example
///
/// ```rust,no_run
/// # use edifact_rs::{Writer, Segment};
/// # fn example() -> Result<(), edifact_rs::EdifactError> {
/// let mut writer = Writer::new(Vec::new());
/// writer.begin_interchange("UNOA", "1", "SENDER", "RECEIVER", "200101", "0900", "1")?;
/// {
///     let mut msg = writer.begin_message("1", "ORDERS", "D", "96A", "UN")?;
///     msg.write_raw("BGM", &["220", "PO001", "9"])?;
///     msg.finish()?;
/// }
/// writer.end_interchange(1, "1")?;
/// # Ok(())
/// # }
/// ```
pub struct MessageWriter<'w, W: Write> {
    writer: &'w mut Writer<W>,
    message_ref: String,
    /// Absolute segment count immediately after `UNH` was written.
    unh_count: u64,
    /// Set to `true` once `finish()` has been called to prevent a double-write
    /// from the `Drop` impl.
    finished: bool,
}

impl<W: Write> MessageWriter<'_, W> {
    /// Write a segment within this message.
    ///
    /// Delegates to [`Writer::write_raw`].
    pub fn write_raw(&mut self, tag: &str, elements: &[&str]) -> Result<(), EdifactError> {
        self.writer.write_raw(tag, elements)
    }

    /// Write a segment mixing simple and composite data elements within this message.
    ///
    /// Delegates to [`Writer::write_elements`] — the general form, and the one
    /// to reach for when a segment is not uniformly simple or uniformly
    /// composite.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if the underlying writer fails.
    pub fn write_elements(
        &mut self,
        tag: &str,
        elements: &[DataElement<'_>],
    ) -> Result<(), EdifactError> {
        self.writer.write_elements(tag, elements)
    }

    /// Write a segment from borrowed element/component slices within this message.
    ///
    /// Delegates to [`Writer::write_composites`].
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if the underlying writer fails.
    pub fn write_composites(
        &mut self,
        tag: &str,
        elements: &[&[&str]],
    ) -> Result<(), EdifactError> {
        self.writer.write_composites(tag, elements)
    }

    /// Write a segment from pre-split, owned element/component data within this message.
    ///
    /// Delegates to [`Writer::write_segment_parts`].
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if the underlying writer fails.
    pub fn write_segment_parts<E>(&mut self, tag: &str, elements: &[E]) -> Result<(), EdifactError>
    where
        E: AsRef<[String]>,
    {
        self.writer.write_segment_parts(tag, elements)
    }

    /// Write a fully-typed segment within this message.
    ///
    /// Delegates to [`Writer::write_segment`].
    pub fn write_segment(&mut self, seg: &Segment<'_>) -> Result<(), EdifactError> {
        self.writer.write_segment(seg)
    }

    /// Compute the per-message segment count and write `UNT`, consuming the guard.
    ///
    /// The count written into `UNT` DE 0074 includes `UNH`, all content segments,
    /// and `UNT` itself — matching the EDIFACT standard.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if writing the `UNT` segment fails.
    pub fn finish(mut self) -> Result<(), EdifactError> {
        self.write_unt()?;
        self.finished = true;
        Ok(())
    }

    fn write_unt(&mut self) -> Result<(), EdifactError> {
        // Segments since UNH: writer.segment_count - unh_count (content only).
        // Total = 1 (UNH) + content + 1 (UNT) = content + 2.
        let count = self.writer.segment_count - self.unh_count + 2;
        let count_str = count.to_string();
        self.writer.write_composites(
            "UNT",
            &[&[count_str.as_str()], &[self.message_ref.as_str()]],
        )
    }
}

impl<W: Write> Drop for MessageWriter<'_, W> {
    fn drop(&mut self) {
        if !self.finished {
            // Best-effort: write UNT; errors cannot be propagated from drop.
            let _ = self.write_unt();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Element;

    /// A non-default UNA whose delimiters share no byte with the defaults.
    fn exotic_ssa() -> ServiceStringAdvice {
        ServiceStringAdvice {
            component_sep: b'|',
            element_sep: b'!',
            decimal_mark: b',',
            release_char: b'#',
            repetition_sep: b'*',
            segment_term: b'~',
        }
    }

    #[test]
    fn unh_composite_uses_the_active_component_separator() {
        // `begin_message` used to `format!` the S009 composite with a literal
        // `:`, collapsing it into one component under a custom UNA.
        let mut buf = Vec::new();
        {
            let mut w = Writer::with_una(&mut buf, exotic_ssa()).unwrap();
            let msg = w
                .begin_message("1", "ORDERS", "D", "96A", "UN")
                .expect("UNH");
            msg.finish().expect("UNT");
        }
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("UNH!1!ORDERS|D|96A|UN~"),
            "S009 must use `|`, got {out}"
        );
    }

    #[test]
    fn round_trips_through_a_custom_una() {
        // The library must be able to re-read its own output verbatim.
        let mut buf = Vec::new();
        {
            let mut w = Writer::with_una(&mut buf, exotic_ssa()).unwrap();
            w.begin_interchange("UNOA", "1", "SENDER", "RECEIVER", "200101", "0900", "IC1")
                .unwrap();
            let mut msg = w.begin_message("1", "ORDERS", "D", "96A", "UN").unwrap();
            msg.write_raw("BGM", &["220"]).unwrap();
            msg.finish().unwrap();
            w.end_interchange(1, "IC1").unwrap();
        }
        let segs: Vec<_> = crate::from_bytes(&buf)
            .collect::<Result<Vec<_>, _>>()
            .expect("own output must reparse");
        let unh = segs.iter().find(|s| s.tag == "UNH").unwrap();
        assert_eq!(unh.get_element(1).unwrap().get_component(0), Some("ORDERS"));
        assert_eq!(unh.get_element(1).unwrap().get_component(2), Some("96A"));
        crate::validate_envelope(&segs).expect("own output must pass envelope validation");
    }

    #[test]
    fn finish_unt_counts_only_the_current_message() {
        // `finish_unt` used the writer-lifetime segment total, so a preceding
        // UNB inflated DE 0074 and the interchange failed its own validation.
        let mut buf = Vec::new();
        {
            let mut w = Writer::new(&mut buf);
            w.begin_interchange("UNOA", "1", "S", "R", "200101", "0900", "IC1")
                .unwrap();
            w.write_composites("UNH", &[&["1"], &["ORDERS", "D", "96A", "UN"]])
                .unwrap();
            w.write_raw("BGM", &["220"]).unwrap();
            w.finish_unt("1").unwrap();
        }
        let out = String::from_utf8(buf).unwrap();
        // UNH + BGM + UNT == 3
        assert!(out.contains("UNT+3+1'"), "expected UNT+3, got {out}");
    }

    #[test]
    fn repetition_separator_is_escaped_when_declared() {
        let mut buf = Vec::new();
        {
            let mut w = Writer::with_una(
                &mut buf,
                ServiceStringAdvice {
                    repetition_sep: b'*',
                    ..ServiceStringAdvice::default()
                },
            )
            .unwrap();
            w.write_composites("FTX", &[&["a*b"]]).unwrap();
        }
        let out = String::from_utf8(buf).unwrap();
        assert!(out.ends_with("FTX+a?*b'"), "rep-sep unescaped in {out}");
    }

    #[test]
    fn repetition_separator_sentinel_is_not_escaped() {
        // Space at UNA position 7 means "not used" and must never be escaped.
        let w = Writer::new(std::io::sink());
        assert_eq!(w.escape_value("a b"), "a b");
    }

    #[test]
    fn write_composites_escapes_a_literal_component_separator() {
        let mut buf = Vec::new();
        {
            let mut w = Writer::new(&mut buf);
            w.write_composites("NAD", &[&["MS"], &["ACME:INC"]])
                .unwrap();
        }
        let segs: Vec<_> = crate::from_bytes(&buf)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        // The `:` stays inside the value instead of splitting the element.
        assert_eq!(
            segs[0].get_element(1).unwrap().get_component(0),
            Some("ACME:INC")
        );
    }

    #[test]
    fn write_elements_mixes_simple_and_composite() {
        let mut buf = Vec::new();
        {
            let mut w = Writer::new(&mut buf);
            w.write_elements(
                "NAD",
                &[
                    DataElement::Simple("MS"),
                    DataElement::Composite(&["9900112233445", "", "293"]),
                ],
            )
            .unwrap();
        }
        assert_eq!(buf, b"NAD+MS+9900112233445::293'");
    }

    #[test]
    fn elements_macro_matches_the_explicit_form() {
        let mut macro_buf = Vec::new();
        {
            let mut w = Writer::new(&mut macro_buf);
            w.write_elements("NAD", elements!["MS", ["ACME", "", "9"]])
                .unwrap();
            w.write_elements("DTM", elements![["137", "20260101", "102"]])
                .unwrap();
        }
        let mut explicit_buf = Vec::new();
        {
            let mut w = Writer::new(&mut explicit_buf);
            w.write_elements(
                "NAD",
                &[
                    DataElement::Simple("MS"),
                    DataElement::Composite(&["ACME", "", "9"]),
                ],
            )
            .unwrap();
            w.write_elements(
                "DTM",
                &[DataElement::Composite(&["137", "20260101", "102"])],
            )
            .unwrap();
        }
        assert_eq!(macro_buf, explicit_buf);
        assert_eq!(macro_buf, b"NAD+MS+ACME::9'DTM+137:20260101:102'");
    }

    #[test]
    fn elements_macro_accepts_arbitrary_expressions() {
        // Builders emit runtime values, not literals.  A `tt`-based macro only
        // matched single-token entries, so `qualifier.as_str()` failed to parse
        // — which is precisely the shape this macro exists for.
        let qualifier = String::from("MS");
        let gln = "9900112233445";
        let dtm: Vec<&str> = vec!["137", "20260101", "102"];

        let mut buf = Vec::new();
        {
            let mut w = Writer::new(&mut buf);
            w.write_elements("NAD", elements![qualifier.as_str(), [gln, "", "293"]])
                .unwrap();
            w.write_elements("DTM", elements![dtm]).unwrap();
            w.write_elements("FTX", elements![qualifier]).unwrap();
            w.write_elements("UNS", elements![]).unwrap();
        }
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "NAD+MS+9900112233445::293'DTM+137:20260101:102'FTX+MS'UNS'"
        );
    }

    #[test]
    fn write_elements_escapes_a_literal_component_separator() {
        // The `:` stays inside the value instead of splitting the element —
        // the failure mode of pre-joining components into one string.
        let mut buf = Vec::new();
        {
            let mut w = Writer::new(&mut buf);
            w.write_elements(
                "NAD",
                &[DataElement::Simple("MS"), DataElement::Simple("ACME:INC")],
            )
            .unwrap();
        }
        let segs: Vec<_> = crate::from_bytes(&buf)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            segs[0].get_element(1).unwrap().get_component(0),
            Some("ACME:INC")
        );
    }

    #[test]
    fn write_elements_uses_the_active_component_separator() {
        let mut buf = Vec::new();
        {
            let mut w = Writer::with_una(&mut buf, exotic_ssa()).unwrap();
            w.write_elements("DTM", elements![["137", "20260101", "102"]])
                .unwrap();
        }
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.ends_with("DTM!137|20260101|102~"),
            "expected custom delimiters, got {out}"
        );
    }

    #[test]
    fn message_writer_counts_write_elements_segments() {
        // `MessageWriter` had no mixed-emit delegate, so callers dropped to the
        // raw writer and their segments escaped the UNT DE 0074 count.
        let mut buf = Vec::new();
        {
            let mut w = Writer::new(&mut buf);
            let mut msg = w.begin_message("1", "ORDERS", "D", "96A", "UN").unwrap();
            msg.write_elements("NAD", elements!["MS", ["ACME", "", "9"]])
                .unwrap();
            msg.write_composites("DTM", &[&["137", "20260101", "102"]])
                .unwrap();
            msg.finish().unwrap();
        }
        let out = String::from_utf8(buf).unwrap();
        // UNH + NAD + DTM + UNT == 4
        assert!(out.contains("UNT+4+1'"), "expected UNT+4, got {out}");
    }

    #[test]
    fn write_segment_records_unh_for_the_unt_count() {
        // `write_segment` and `write_segment_parts` did not record the UNH
        // marker, so `finish_unt` fell back to the writer-lifetime total and
        // DE 0074 came out inflated by every preceding interchange segment.
        let mut buf = Vec::new();
        {
            let mut w = Writer::new(&mut buf);
            w.begin_interchange("UNOA", "1", "S", "R", "200101", "0900", "IC1")
                .unwrap();
            w.write_segment(&Segment::new(
                "UNH",
                vec![
                    Element::of(&["1"]),
                    Element::of(&["ORDERS", "D", "96A", "UN"]),
                ],
            ))
            .unwrap();
            w.write_segment(&Segment::new("BGM", vec![Element::of(&["220"])]))
                .unwrap();
            w.finish_unt("1").unwrap();
        }
        let out = String::from_utf8(buf).unwrap();
        // UNH + BGM + UNT == 3, not 4 (which would count the UNB).
        assert!(out.contains("UNT+3+1'"), "expected UNT+3, got {out}");
    }

    #[test]
    fn write_segment_parts_records_unh_for_the_unt_count() {
        let mut buf = Vec::new();
        {
            let mut w = Writer::new(&mut buf);
            w.begin_interchange("UNOA", "1", "S", "R", "200101", "0900", "IC1")
                .unwrap();
            w.write_segment_parts(
                "UNH",
                &[
                    vec!["1".to_owned()],
                    vec![
                        "ORDERS".to_owned(),
                        "D".to_owned(),
                        "96A".to_owned(),
                        "UN".to_owned(),
                    ],
                ],
            )
            .unwrap();
            w.write_raw("BGM", &["220"]).unwrap();
            w.finish_unt("1").unwrap();
        }
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("UNT+3+1'"), "expected UNT+3, got {out}");
    }

    #[test]
    fn escape_value_handles_multi_byte_text() {
        // The old implementation assembled a `Vec<u8>` and re-validated it with
        // an `expect`.  Escaping around non-ASCII text is the case that made
        // that conversion look fallible in the first place.
        let w = Writer::new(std::io::sink());
        assert_eq!(w.escape_value("Grüße+Köln"), "Grüße?+Köln");
        assert_eq!(w.escape_value("Grüße"), "Grüße");
        assert!(matches!(w.escape_value("plain"), Cow::Borrowed("plain")));
    }

    #[test]
    fn write_and_parse_simple_segment() {
        let segs: Vec<Segment<'static>> = vec![Segment::new(
            "BGM",
            vec![Element::of(&["220"]), Element::of(&["ORDER123"])],
        )];
        let bytes = crate::segments_to_bytes(&segs).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.starts_with("BGM+220+ORDER123'"));
    }

    #[test]
    fn release_char_escaped() {
        let segs: Vec<Segment<'static>> = vec![Segment::new(
            "FTX",
            vec![Element::of(&["value+with+delimiters"])],
        )];
        let bytes = crate::segments_to_bytes(&segs).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        // The `+` in the value must be escaped as `?+`
        assert!(s.contains("?+"), "escape missing: {s}");
    }

    #[test]
    fn round_trip_preserves_values() {
        let segs: Vec<Segment<'static>> = vec![
            Segment::new(
                "UNB",
                vec![
                    Element::of(&["UNOA", "1"]),
                    Element::of(&["SENDER"]),
                    Element::of(&["RECEIVER"]),
                ],
            ),
            Segment::new("UNZ", vec![Element::of(&["0"]), Element::of(&["1"])]),
        ];
        let bytes = crate::segments_to_bytes(&segs).unwrap();
        let rt: Vec<crate::OwnedSegment> = crate::parser::from_reader(std::io::Cursor::new(&bytes))
            .expect("round-trip parse failed");
        assert_eq!(rt[0].tag, "UNB");
        assert_eq!(rt[0].as_borrowed().element_str(0), Some("UNOA"));
        assert_eq!(rt[1].tag, "UNZ");
    }

    /// Verify that `Writer::with_una` uses the configured delimiters throughout,
    /// and that `write_segment_parts` (the delimiter-agnostic API) produces correct
    /// component separators even with a non-default UNA.
    #[test]
    fn with_una_non_default_delimiters() {
        use crate::tokenizer::ServiceStringAdvice;

        // Custom UNA: comp_sep=|  elem_sep=!  esc=?  dec_mark=,  rep_sep=*  seg_term=~
        let ssa = ServiceStringAdvice {
            component_sep: b'|',
            element_sep: b'!',
            release_char: b'?',
            decimal_mark: b',',
            repetition_sep: b'*',
            segment_term: b'~',
        };

        let buf = Vec::new();
        let mut writer = Writer::with_una(buf, ssa).expect("writer creation failed");

        // write_segment_parts: pre-split; no hard-coded `:` in element strings
        writer
            .write_segment_parts(
                "BGM",
                &[
                    vec!["220".to_owned(), "SUB1".to_owned()],
                    vec!["PO1".to_owned()],
                ],
            )
            .expect("write failed");

        let out = writer.finish().expect("finish failed");
        let s = std::str::from_utf8(&out).unwrap();

        // Output must use `!` as element separator, `|` as component separator, `~` as terminator.
        // The writer also emits a UNA header when with_una is used.
        assert!(s.contains("BGM"), "BGM segment missing: {s}");
        // Slice after UNA so assertions target segment output, not UNA header bytes.
        let after_una = s.find("BGM").map(|i| &s[i..]).unwrap_or(s);
        assert!(
            after_una.contains('!'),
            "missing element sep in segment: {after_una}"
        );
        assert!(
            after_una.contains('|'),
            "missing component sep in segment: {after_una}"
        );
        assert!(
            after_una.ends_with('~'),
            "missing segment term in segment: {after_una}"
        );
        // Decimal mark appears in the UNA header (no decimal-bearing values in this segment).
        assert!(s.contains(','), "missing decimal mark in UNA: {s}");
        assert!(!s.contains('+'), "default element sep leaked: {s}");
        assert!(!s.contains(':'), "default component sep leaked: {s}");
        // segment_term '~' is not the default; ensure no default ' leaks (UNA itself aside)
        assert!(
            !after_una.contains('\''),
            "default segment term leaked after UNA: {after_una}"
        );
    }
}
