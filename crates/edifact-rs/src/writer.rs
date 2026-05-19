//! EDIFACT writer — serializes [`Segment`]s to wire format.

use crate::{error::EdifactError, model::Segment, tokenizer::ServiceStringAdvice};
use std::io::Write;

/// Streaming EDIFACT writer.
///
/// Wraps any [`Write`] implementation and serializes segments one at a time.
/// Call [`Writer::finish`] to flush and get the underlying writer back.
pub struct Writer<W: Write> {
    inner: W,
    ssa: ServiceStringAdvice,
    segment_count: u32,
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
        // UNA: component_sep, element_sep, decimal_mark, release_char, space, segment_term
        let una = [
            b'U',
            b'N',
            b'A',
            ssa.component_sep,
            ssa.element_sep,
            ssa.decimal_mark,
            ssa.release_char,
            b' ',
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
            for component in &element.components {
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
                // SAFETY: input is valid UTF-8 and we split on a single-byte delimiter,
                // so each part remains a valid UTF-8 slice.
                self.write_escaped(std::str::from_utf8(first).map_err(|_| EdifactError::InvalidUtf8)?)?;
            }
            for part in parts {
                self.inner.write_all(&[comp_sep])?;
                self.write_escaped(std::str::from_utf8(part).map_err(|_| EdifactError::InvalidUtf8)?)?;
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
    pub fn write_segment_parts<E>(
        &mut self,
        tag: &str,
        elements: &[E],
    ) -> Result<(), EdifactError>
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

    /// Number of segments written so far.
    pub fn segment_count(&self) -> u32 {
        self.segment_count
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
        let mut pos = 0;
        while pos < bytes.len() {
            // Find next byte that needs escaping
            let end = bytes[pos..]
                .iter()
                .position(|&b| b == elem || b == comp || b == release || b == term)
                .map(|r| pos + r)
                .unwrap_or(bytes.len());
            if end > pos {
                self.inner.write_all(&bytes[pos..end])?;
            }
            if end < bytes.len() {
                self.inner.write_all(&[release, bytes[end]])?;
                pos = end + 1;
            } else {
                break;
            }
        }
        Ok(())
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
        let rt: Vec<crate::OwnedSegment> =
            crate::parser::from_reader(std::io::Cursor::new(&bytes))
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

        // Custom UNA: comp_sep=|  elem_sep=!  esc=?  dec_mark=,  seg_term=~
        let ssa = ServiceStringAdvice {
            component_sep: b'|',
            element_sep: b'!',
            release_char: b'?',
            decimal_mark: b',',
            segment_term: b'~',
        };

        let buf = Vec::new();
        let mut writer = Writer::with_una(buf, ssa).expect("writer creation failed");

        // write_segment_parts: pre-split; no hard-coded `:` in element strings
        writer
            .write_segment_parts("BGM", &[vec!["220".to_owned(), "SUB1".to_owned()], vec!["PO1".to_owned()]])
            .expect("write failed");

        let out = writer.finish().expect("finish failed");
        let s = std::str::from_utf8(&out).unwrap();

        // Output must use `!` as element separator, `|` as component separator, `~` as terminator.
        // The writer also emits a UNA header when with_una is used.
        assert!(s.contains("BGM"), "BGM segment missing: {s}");
        // Slice after UNA so assertions target segment output, not UNA header bytes.
        let after_una = s.find("BGM").map(|i| &s[i..]).unwrap_or(s);
        assert!(after_una.contains('!'), "missing element sep in segment: {after_una}");
        assert!(after_una.contains('|'), "missing component sep in segment: {after_una}");
        assert!(after_una.ends_with('~'), "missing segment term in segment: {after_una}");
        // Decimal mark appears in the UNA header (no decimal-bearing values in this segment).
        assert!(s.contains(','), "missing decimal mark in UNA: {s}");
        assert!(!s.contains('+'), "default element sep leaked: {s}");
        assert!(!s.contains(':'), "default component sep leaked: {s}");
        // segment_term '~' is not the default; ensure no default ' leaks (UNA itself aside)
        assert!(!after_una.contains('\''), "default segment term leaked after UNA: {after_una}");
    }
}
