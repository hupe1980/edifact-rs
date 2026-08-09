//! Custom serialization trait for EDIFACT.
//!
//! [`EdifactSerialize`] emits typed EDIFACT events rather than the generic
//! key/value tokens of standard `serde`.  This matches EDIFACT's positional,
//! qualifier-based data model — see `docs/writing.md` for the design rationale.

use crate::EdifactError;
use crate::event::{EdifactEvent, EventEmitter, WriterEmitter};
use std::io::Write;

// ── trait ─────────────────────────────────────────────────────────────────────

/// Types that can serialize themselves to an EDIFACT event stream.
///
/// Implement manually or derive with `#[derive(EdifactSerialize)]` from the
/// `edifact-rs-derive` crate.
pub trait EdifactSerialize {
    /// Serialize `self` by emitting events into `emitter`.
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError>;
}

/// Types that can serialize themselves as a composite EDIFACT element.
///
/// Implement this for custom composite structs used with
/// `#[edifact(composite)]` in derive macros.
pub trait EdifactCompositeSerialize {
    /// Serialize `self` as one composite element into `emitter`.
    fn edifact_serialize_composite<E: EventEmitter>(
        &self,
        emitter: &mut E,
    ) -> Result<(), EdifactError>;
}

impl EdifactCompositeSerialize for Vec<String> {
    fn edifact_serialize_composite<E: EventEmitter>(
        &self,
        emitter: &mut E,
    ) -> Result<(), EdifactError> {
        if self.is_empty() {
            return emitter.emit(EdifactEvent::Element { value: "" });
        }

        emitter.emit(EdifactEvent::Element { value: &self[0] })?;
        for component in self.iter().skip(1) {
            emitter.emit(EdifactEvent::ComponentElement { value: component })?;
        }
        Ok(())
    }
}

// ── blanket impls for scalar types ────────────────────────────────────────────

impl EdifactSerialize for str {
    #[inline]
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
        emitter.emit(EdifactEvent::Element { value: self })
    }
}

impl EdifactSerialize for String {
    #[inline]
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
        emitter.emit(EdifactEvent::Element {
            value: self.as_str(),
        })
    }
}

/// `None` → empty element `""`; `Some(v)` → `v.edifact_serialize(emitter)`.
impl<T: EdifactSerialize> EdifactSerialize for Option<T> {
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
        match self {
            Some(v) => v.edifact_serialize(emitter),
            None => emitter.emit(EdifactEvent::Element { value: "" }),
        }
    }
}

/// Each element is serialized independently (repeated segments for groups).
impl<T: EdifactSerialize> EdifactSerialize for Vec<T> {
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
        for item in self {
            item.edifact_serialize(emitter)?;
        }
        Ok(())
    }
}

/// Each element is serialized independently (repeated segments for groups).
impl<T: EdifactSerialize> EdifactSerialize for [T] {
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
        for item in self {
            item.edifact_serialize(emitter)?;
        }
        Ok(())
    }
}

macro_rules! impl_serialize_int {
    ($($t:ty),+ $(,)?) => {
        $(
            impl EdifactSerialize for $t {
                fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
                    // 42-byte buffer: i128::MIN is 40 chars; 2 spare bytes as safety margin.
                    use std::io::Write as _;
                    let mut buf = [0u8; 42];
                    let mut w: &mut [u8] = &mut buf;
                    if write!(w, "{self}").is_ok() {
                        let written = 42 - w.len();
                        // Display output for all integer/bool types is ASCII-only.
                        let s = std::str::from_utf8(&buf[..written]).map_err(|_| EdifactError::InvalidUtf8)?;
                        emitter.emit(EdifactEvent::Element { value: s })
                    } else {
                        // Extraordinary case: fall back to heap to avoid any panic.
                        let s = format!("{self}");
                        emitter.emit(EdifactEvent::Element { value: &s })
                    }
                }
            }
        )+
    };
}

// Boolean is also bounded (max "false" = 5 bytes).
impl_serialize_int!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, bool
);

// ── decimal-mark-aware float wrapper ─────────────────────────────────────────

/// Decimal-mark-aware wrapper for `f32` or `f64` serialization.
///
/// Rust's [`Display`][std::fmt::Display] for `f32`/`f64` always uses `.` as the
/// decimal separator.  EDIFACT interchanges can declare a different decimal mark
/// in the UNA service string — `,` is the common alternative.
///
/// # Required for float serialization
///
/// `edifact-rs` intentionally provides **no** blanket `EdifactSerialize` impl for
/// `f32`/`f64`.  This forces callers to make the decimal-mark intent explicit:
///
/// ```
/// use edifact_rs::ser::DecimalFloat;
/// use edifact_rs::{EdifactSerialize, VecEmitter, OwnedEdifactEvent};
///
/// let mut emitter = VecEmitter::default();
/// DecimalFloat(12.5_f64).edifact_serialize(&mut emitter).unwrap();
/// assert!(matches!(&emitter.events[0], OwnedEdifactEvent::Element { value } if value == "12.5"));
/// ```
///
/// When the emitter's [`EventEmitter::decimal_mark`] is `b','`, the output will be `"12,5"`.
///
/// # Supported inner types
///
/// `DecimalFloat<f32>`, `DecimalFloat<f64>`. For any type that implements
/// [`std::fmt::Display`], use [`DecimalFloatDisplay`] instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecimalFloat<T>(pub T);

/// Decimal-mark-aware serializer for any [`std::fmt::Display`] value.
///
/// Like [`DecimalFloat`] but works with any type whose `Display` uses `.` as a
/// decimal point (e.g., `rust_decimal::Decimal`, `bigdecimal::BigDecimal`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecimalFloatDisplay<T: std::fmt::Display>(pub T);

fn serialize_with_decimal_mark<E: EventEmitter>(
    display: &dyn std::fmt::Display,
    emitter: &mut E,
) -> Result<(), EdifactError> {
    use std::io::Write as _;
    let mark = emitter.decimal_mark();

    // Fast path: standard interchange — avoid any string manipulation.
    if mark == b'.' {
        let mut buf = [0u8; 320];
        let mut w: &mut [u8] = &mut buf;
        if write!(w, "{display}").is_ok() {
            let written = 320 - w.len();
            // INVARIANT: float Display output is ASCII-only.
            let s = std::str::from_utf8(&buf[..written]).map_err(|_| EdifactError::InvalidUtf8)?;
            return emitter.emit(EdifactEvent::Element { value: s });
        }
        // Buffer overflow fallback (extraordinarily large exponent).
        let s = format!("{display}");
        return emitter.emit(EdifactEvent::Element { value: &s });
    }

    // Non-standard decimal mark: format as string then replace '.'.
    // INVARIANT: `mark` is ASCII (validated by ServiceStringAdvice::is_valid()).
    let s = format!("{display}");
    if s.contains('.') {
        // Encode `mark` as a 1–4 byte UTF-8 slice on the stack; no heap allocation.
        let mut mark_buf = [0u8; 4];
        let mark_str = (mark as char).encode_utf8(&mut mark_buf);
        let replaced = s.replace('.', mark_str);
        emitter.emit(EdifactEvent::Element { value: &replaced })
    } else {
        emitter.emit(EdifactEvent::Element { value: &s })
    }
}

impl EdifactSerialize for DecimalFloat<f32> {
    #[inline]
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
        serialize_with_decimal_mark(&self.0, emitter)
    }
}

impl EdifactSerialize for DecimalFloat<f64> {
    #[inline]
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
        serialize_with_decimal_mark(&self.0, emitter)
    }
}

impl<T: std::fmt::Display> EdifactSerialize for DecimalFloatDisplay<T> {
    #[inline]
    fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
        serialize_with_decimal_mark(&self.0, emitter)
    }
}

/// Emit one segment from `(element_index, component_index, value)` triples.
///
/// Values may arrive in any order and need not cover every slot: the triples
/// are sorted, gaps are filled with empty elements and components, and the
/// result is a single well-formed segment.
///
/// This exists because a derive that addresses fields by UN/EDIFACT data element
/// identifier (`#[edifact(element = "3055")]`) only learns the numeric slots
/// during *const evaluation*, after the macro has already expanded — so it
/// cannot lay out the emit order at expansion time the way a positional derive
/// can.  Handing the resolved slots to this function moves the ordering to
/// runtime, where they are known.
///
/// `parts` is taken by mutable reference because it is sorted in place; the
/// caller keeps the allocation.
///
/// If two triples claim the same slot the **first** one wins and the rest are
/// dropped — emitting both would shift every following component by one and
/// silently corrupt the segment's positions. (The derive cannot produce a
/// duplicate: it rejects colliding slots at compile time.)
///
/// # Example
///
/// ```rust
/// use edifact_rs::{VecEmitter, emit_sparse_segment};
/// use std::borrow::Cow;
///
/// let mut parts = vec![
///     (1, 2, Cow::Borrowed("293")),
///     (0, 0, Cow::Borrowed("MS")),
///     (1, 0, Cow::Borrowed("9900112233445")),
/// ];
/// let mut emitter = VecEmitter::default();
/// emit_sparse_segment(&mut emitter, "NAD", &mut parts)?;
/// // Slot (1, 1) was never supplied, so it is emitted as an empty component:
/// // NAD+MS+9900112233445::293'
/// assert_eq!(emitter.events.len(), 6);
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
///
/// # Errors
///
/// Propagates any error returned by the emitter.
pub fn emit_sparse_segment<E: EventEmitter>(
    emitter: &mut E,
    tag: &str,
    parts: &mut [(usize, usize, std::borrow::Cow<'_, str>)],
) -> Result<(), EdifactError> {
    parts.sort_by_key(|(element, component, _)| (*element, *component));

    emitter.emit(EdifactEvent::StartSegment { tag })?;

    // `parts` is sorted, so the last entry carries the highest element index and
    // one linear walk covers every slot.
    let mut cursor = 0usize;
    let element_count = parts.last().map_or(0, |(element, _, _)| *element + 1);
    for element in 0..element_count {
        // Component 0 opens the element; every later component extends it.
        let mut next_component = 0usize;
        let mut opened = false;
        while cursor < parts.len() && parts[cursor].0 == element {
            let (_, component, value) = &parts[cursor];
            if *component < next_component {
                // A slot already emitted: first value wins.  Emitting this one
                // too would push every later component one position right.
                cursor += 1;
                continue;
            }
            // Fill any skipped component slots so positions stay meaningful.
            while next_component < *component {
                let event = if opened {
                    EdifactEvent::ComponentElement { value: "" }
                } else {
                    EdifactEvent::Element { value: "" }
                };
                emitter.emit(event)?;
                opened = true;
                next_component += 1;
            }
            let event = if opened {
                EdifactEvent::ComponentElement { value }
            } else {
                EdifactEvent::Element { value }
            };
            emitter.emit(event)?;
            opened = true;
            next_component += 1;
            cursor += 1;
        }
        if !opened {
            // No value for this element at all — emit an empty placeholder so
            // the following elements keep their positions.
            emitter.emit(EdifactEvent::Element { value: "" })?;
        }
    }

    emitter.emit(EdifactEvent::EndSegment)
}

/// Serialize `value` to the given [`Write`] implementation.
pub fn to_writer<T, W>(inner: W, value: &T) -> Result<(), EdifactError>
where
    T: EdifactSerialize,
    W: Write,
{
    let mut emitter = WriterEmitter::new(inner);
    value.edifact_serialize(&mut emitter)?;
    emitter.finish().map(|_| ())
}

/// Serialize `value` to an owned `Vec<u8>`.
pub fn to_bytes<T: EdifactSerialize>(value: &T) -> Result<Vec<u8>, EdifactError> {
    let mut buf = Vec::new();
    to_writer(&mut buf, value)?;
    Ok(buf)
}

/// Serialize `value` to a UTF-8 `String`.
///
/// # Allocations
///
/// Allocates one `Vec<u8>` via [`to_bytes`].  The subsequent conversion to
/// `String` reuses that allocation in-place via [`String::from_utf8`] — no
/// second allocation occurs.  When you only need raw bytes (e.g. for a network
/// write), prefer [`to_bytes`] directly.
pub fn to_edifact_string<T: EdifactSerialize>(value: &T) -> Result<String, EdifactError> {
    let bytes = to_bytes(value)?;
    String::from_utf8(bytes).map_err(|_| EdifactError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{OwnedEdifactEvent, VecEmitter};
    use std::borrow::Cow;

    /// Render a sparse-emit result to wire bytes, which is what the shape of
    /// the event stream actually has to produce.
    fn sparse_to_wire(parts: &mut [(usize, usize, Cow<'_, str>)], tag: &str) -> String {
        let mut buf = Vec::new();
        {
            let mut emitter = crate::WriterEmitter::new(&mut buf);
            emit_sparse_segment(&mut emitter, tag, parts).expect("emit");
            emitter.finish().expect("finish");
        }
        String::from_utf8(buf).expect("utf-8")
    }

    #[test]
    fn emit_sparse_segment_fills_gaps_in_elements_and_components() {
        let mut parts = vec![
            (1, 2, Cow::Borrowed("293")),
            (0, 0, Cow::Borrowed("MS")),
            (3, 1, Cow::Borrowed("late")),
        ];
        // Element 2 is absent entirely, element 3 skips component 0, and
        // element 1 skips component 1 — every gap holds its position.
        assert_eq!(sparse_to_wire(&mut parts, "NAD"), "NAD+MS+::293++:late'");
    }

    #[test]
    fn emit_sparse_segment_first_value_wins_on_a_duplicate_slot() {
        // Emitting both would push `293` from component 2 to component 3 and
        // silently corrupt every later position.
        let mut parts = vec![
            (0, 0, Cow::Borrowed("MS")),
            (1, 0, Cow::Borrowed("first")),
            (1, 0, Cow::Borrowed("second")),
            (1, 2, Cow::Borrowed("293")),
        ];
        assert_eq!(sparse_to_wire(&mut parts, "NAD"), "NAD+MS+first::293'");
    }

    #[test]
    fn emit_sparse_segment_with_no_parts_writes_a_bare_tag() {
        assert_eq!(sparse_to_wire(&mut [], "UNS"), "UNS'");
    }

    struct BgmSegment {
        doc_name_code: String,
        pruef_id: String,
        msg_function: Option<String>,
    }

    impl EdifactSerialize for BgmSegment {
        fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
            emitter.emit(EdifactEvent::StartSegment { tag: "BGM" })?;
            emitter.emit(EdifactEvent::Element {
                value: &self.doc_name_code,
            })?;
            emitter.emit(EdifactEvent::Element {
                value: &self.pruef_id,
            })?;
            self.msg_function.edifact_serialize(emitter)?;
            emitter.emit(EdifactEvent::EndSegment)?;
            Ok(())
        }
    }

    #[test]
    fn vec_emitter_captures_segment_events() {
        let seg = BgmSegment {
            doc_name_code: "E03".to_owned(),
            pruef_id: "11042".to_owned(),
            msg_function: None,
        };
        let mut emitter = VecEmitter::default();
        seg.edifact_serialize(&mut emitter).unwrap();

        assert_eq!(
            emitter.events[0],
            OwnedEdifactEvent::StartSegment {
                tag: "BGM".to_owned()
            }
        );
        assert_eq!(emitter.events.last(), Some(&OwnedEdifactEvent::EndSegment));
    }

    #[test]
    fn to_bytes_produces_valid_edifact() {
        let seg = BgmSegment {
            doc_name_code: "E03".to_owned(),
            pruef_id: "11042".to_owned(),
            msg_function: Some("9".to_owned()),
        };
        let bytes = to_bytes(&seg).unwrap();
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), "BGM+E03+11042+9'");
    }

    #[test]
    fn option_none_emits_empty_element() {
        let val: Option<String> = None;
        let mut emitter = VecEmitter::default();
        val.edifact_serialize(&mut emitter).unwrap();
        assert_eq!(
            emitter.events[0],
            OwnedEdifactEvent::Element {
                value: String::new()
            }
        );
    }

    #[test]
    fn option_some_emits_value() {
        let val: Option<String> = Some("TEST".to_owned());
        let mut emitter = VecEmitter::default();
        val.edifact_serialize(&mut emitter).unwrap();
        assert_eq!(
            emitter.events[0],
            OwnedEdifactEvent::Element {
                value: "TEST".to_owned()
            }
        );
    }

    #[test]
    fn integer_types_serialize_without_alloc() {
        let mut emitter = VecEmitter::default();
        42u32.edifact_serialize(&mut emitter).unwrap();
        assert_eq!(
            emitter.events[0],
            OwnedEdifactEvent::Element {
                value: "42".to_owned()
            }
        );
        // i128::MIN should fit exactly in the 40-byte buffer
        let mut emitter2 = VecEmitter::default();
        i128::MIN.edifact_serialize(&mut emitter2).unwrap();
        assert_eq!(
            emitter2.events[0],
            OwnedEdifactEvent::Element {
                value: "-170141183460469231731687303715884105728".to_owned()
            }
        );
    }

    #[test]
    fn float_extremes_do_not_panic() {
        use super::DecimalFloat;
        // Rust Display for f64 picks the shortest round-trip form; a 320-byte buffer covers all values.
        let mut emitter = VecEmitter::default();
        DecimalFloat(f64::MAX)
            .edifact_serialize(&mut emitter)
            .unwrap();
        let s = match &emitter.events[0] {
            OwnedEdifactEvent::Element { value } => value.clone(),
            _ => panic!("expected Element event"),
        };
        assert!(!s.is_empty());
        // f32::MAX too
        let mut emitter2 = VecEmitter::default();
        DecimalFloat(f32::MAX)
            .edifact_serialize(&mut emitter2)
            .unwrap();
        assert!(matches!(
            &emitter2.events[0],
            OwnedEdifactEvent::Element { .. }
        ));
    }

    #[test]
    fn vec_serializes_each_item() {
        let segments = vec![
            BgmSegment {
                doc_name_code: "E03".to_owned(),
                pruef_id: "11042".to_owned(),
                msg_function: None,
            },
            BgmSegment {
                doc_name_code: "E01".to_owned(),
                pruef_id: "11043".to_owned(),
                msg_function: None,
            },
        ];
        let bytes = to_bytes(&segments).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains("BGM+E03+11042"));
        assert!(s.contains("BGM+E01+11043"));
    }
}
