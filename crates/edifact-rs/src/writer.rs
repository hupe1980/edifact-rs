//! EDIFACT writer — serializes [`Segment`]s to wire format.

use crate::{error::EdifactError, model::Segment, tokenizer::ServiceStringAdvice};
use std::borrow::Cow;
use std::io::Write;

/// Streaming EDIFACT writer.
///
/// Wraps any [`Write`] implementation and serializes segments one at a time.
/// Call [`Writer::finish`] to flush and get the underlying writer back.
pub struct Writer<W: Write> {
    inner: W,
    ssa: ServiceStringAdvice,
    /// Running count of segments written.  `u64` to prevent silent overflow on
    /// pathological inputs (a `u32` would wrap after ~4 billion segments).
    segment_count: u64,
}

impl<W: Write> Writer<W> {
    /// Create a new writer with default EDIFACT delimiters.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            ssa: ServiceStringAdvice::default(),
            segment_count: 0,
        }
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
        })
    }

    /// Write a single segment.
    pub fn write_segment(&mut self, seg: &Segment<'_>) -> Result<(), EdifactError> {
        // Tag
        self.inner.write_all(seg.tag.as_bytes())?;

        for element in &seg.elements {
            // Element separator
            self.inner.write_all(&[self.ssa.element_sep])?;
            let mut first_component = true;
            for (component, _) in &element.components {
                if !first_component {
                    self.inner.write_all(&[self.ssa.component_sep])?;
                }
                first_component = false;
                self.write_escaped(component)?;
            }
        }

        // Segment terminator
        self.inner.write_all(&[self.ssa.segment_term])?;
        self.segment_count += 1;
        Ok(())
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
    /// [`Self::write_segment_parts`] which accepts pre-split component slices.
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
        self.inner.write_all(&[self.ssa.segment_term])?;
        self.segment_count += 1;
        Ok(())
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
        self.inner.write_all(&[self.ssa.segment_term])?;
        self.segment_count += 1;
        Ok(())
    }

    /// Flush and return the underlying writer.
    pub fn finish(mut self) -> Result<W, EdifactError> {
        self.inner.flush()?;
        Ok(self.inner)
    }

    /// Write the `UNT` segment and return the inner writer.
    ///
    /// The segment count written into `UNT` element 1 (DE 0074) is the number of
    /// segments already written **plus one** for the `UNT` segment itself, which
    /// EDIFACT requires to be included in the count alongside `UNH`.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.  Do **not** call [`write_raw`][Self::write_raw] or
    /// [`write_segment`][Self::write_segment] after `finish_unt` — the writer is consumed.
    pub fn finish_unt(mut self, message_ref: &str) -> Result<W, EdifactError> {
        // DE 0074: count includes UNH and UNT themselves.
        let count = self.segment_count + 1;
        let count_str = count.to_string();
        self.write_raw("UNT", &[count_str.as_str(), message_ref])?;
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
        let (elem, comp, release, term) = (
            self.ssa.element_sep,
            self.ssa.component_sep,
            self.ssa.release_char,
            self.ssa.segment_term,
        );
        let bytes = value.as_bytes();
        let needs_escape = bytes
            .iter()
            .any(|&b| b == elem || b == comp || b == release || b == term);
        if !needs_escape {
            return Cow::Borrowed(value);
        }
        let mut out = Vec::with_capacity(value.len() + 4);
        let mut last = 0;
        let mut pos = 0;
        while pos < bytes.len() {
            let remaining = &bytes[pos..];
            let hit_ecr = memchr::memchr3(elem, comp, release, remaining);
            let hit_t = memchr::memchr(term, remaining);
            let hit = match (hit_ecr, hit_t) {
                (None, None) => break,
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (Some(a), Some(b)) => a.min(b),
            };
            let abs = pos + hit;
            out.extend_from_slice(&bytes[last..abs]);
            out.push(release);
            out.push(bytes[abs]);
            last = abs + 1;
            pos = abs + 1;
        }
        out.extend_from_slice(&bytes[last..]);
        // SAFETY:
        //   1. `value` is a valid `&str`, so `bytes` is valid UTF-8 to start.
        //   2. `self.ssa.release_char` is a single-byte ASCII value (0x21–0x7E),
        //      enforced at construction time by `ServiceStringAdvice::is_valid()`
        //      (called in `Writer::with_una`; the default SSA hardcodes `?` = 0x3F).
        //      Inserting a single ASCII byte cannot split or corrupt a multi-byte
        //      UTF-8 sequence, because ASCII bytes always have the high bit clear
        //      while continuation bytes of multi-byte sequences always have the high
        //      bit set (0x80–0xBF).
        //   3. All other bytes are copied verbatim from the valid UTF-8 source.
        Cow::Owned(
            String::from_utf8(out).expect(
                "escape_value: output is not valid UTF-8; this is a bug in the escape logic",
            ),
        )
    }
    /// Write only the segment tag bytes — no element separator or terminator.
    ///
    /// Used by [`crate::WriterEmitter`] for eager, zero-allocation event writing.
    #[inline]
    pub(crate) fn write_tag_only(&mut self, tag: &str) -> Result<(), EdifactError> {
        self.inner.write_all(tag.as_bytes())?;
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
        self.inner.write_all(&[self.ssa.segment_term])?;
        self.segment_count += 1;
        Ok(())
    }

    /// Write a value, escaping any delimiter characters.
    pub(crate) fn write_escaped(&mut self, value: &str) -> Result<(), EdifactError> {
        let (elem, comp, release, term) = (
            self.ssa.element_sep,
            self.ssa.component_sep,
            self.ssa.release_char,
            self.ssa.segment_term,
        );
        let bytes = value.as_bytes();
        let mut last = 0;
        let mut pos = 0;
        while pos < bytes.len() {
            // Use memchr3 for three delimiters + memchr for the fourth to avoid
            // a manual byte-by-byte scan.
            let remaining = &bytes[pos..];
            let hit_ecr = memchr::memchr3(elem, comp, release, remaining);
            let hit_t = memchr::memchr(term, remaining);
            let hit = match (hit_ecr, hit_t) {
                (None, None) => break,
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (Some(a), Some(b)) => a.min(b),
            };
            let abs = pos + hit;
            if abs > last {
                self.inner.write_all(&bytes[last..abs])?;
            }
            self.inner.write_all(&[release, bytes[abs]])?;
            last = abs + 1;
            pos = abs + 1;
        }
        self.inner.write_all(&bytes[last..])?;
        Ok(())
    }

    // ── Interchange envelope helpers ──────────────────────────────────────────

    /// Write a `UNB` interchange header segment.
    ///
    /// Generates:
    /// ```text
    /// UNB+<syntax_id>+<sender>+<recipient>+<datetime>+<control_ref>'
    /// ```
    ///
    /// The caller is responsible for:
    /// - Formatting `syntax_id` as a composite (e.g. `"UNOA:1"` for UN/EDIFACT syntax
    ///   version 1 of set A).
    /// - Formatting `datetime` as a composite date-time (e.g. `"200101:0900"`).
    ///
    /// Track the `control_ref` — it must be repeated in the matching
    /// [`end_interchange`](Self::end_interchange) call.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError`] if writing fails.
    pub fn begin_interchange(
        &mut self,
        syntax_id: &str,
        sender: &str,
        recipient: &str,
        datetime: &str,
        control_ref: &str,
    ) -> Result<(), EdifactError> {
        self.write_raw(
            "UNB",
            &[syntax_id, sender, recipient, datetime, control_ref],
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
        let msg_id = format!("{message_type}:{version}:{release}:{controlling_agency}");
        self.write_raw("UNH", &[message_ref, &msg_id])?;
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
        self.write_raw("UNZ", &[&msg_count_str, control_ref])
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
/// writer.begin_interchange("UNOA:1", "SENDER", "RECEIVER", "200101:0900", "1")?;
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

impl<'w, W: Write> MessageWriter<'w, W> {
    /// Write a segment within this message.
    ///
    /// Delegates to [`Writer::write_raw`].
    pub fn write_raw(&mut self, tag: &str, elements: &[&str]) -> Result<(), EdifactError> {
        self.writer.write_raw(tag, elements)
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
        self.writer
            .write_raw("UNT", &[&count_str, &self.message_ref])
    }
}

impl<'w, W: Write> Drop for MessageWriter<'w, W> {
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
