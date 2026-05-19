//! Custom serialization trait for EDIFACT.
//!
//! [`EdifactSerialize`] emits typed EDIFACT events rather than the generic
//! key/value tokens of standard `serde`.  This matches EDIFACT's positional,
//! qualifier-based data model — see `SPIKE_NOTES.md` for the design rationale.

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
                    // Stack buffer: 40 bytes covers i128::MIN
                    // ("-170141183460469231731687303715884105728") exactly.
                    // Integer Display lengths are bounded; floats must NOT use this path.
                    let mut buf = [0u8; 40];
                    let s = {
                        use std::io::Write as _;
                        let mut cursor = std::io::Cursor::new(&mut buf[..]);
                        // SAFETY: integer Display is bounded; buf is large enough.
                        write!(cursor, "{}", self).expect("integer format into fixed buffer");
                        let len = cursor.position() as usize;
                        std::str::from_utf8(&buf[..len]).expect("integer output is valid UTF-8")
                    };
                    emitter.emit(EdifactEvent::Element { value: s })
                }
            }
        )+
    };
}

// Boolean is also bounded (max "false" = 5 bytes).
impl_serialize_int!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, bool
);

// f32/f64 Display length is unbounded (e.g. f64::MAX formats to 309 chars).
// Use heap allocation to avoid a panic on extreme values.
macro_rules! impl_serialize_float {
    ($($t:ty),+ $(,)?) => {
        $(
            impl EdifactSerialize for $t {
                fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
                    let s = self.to_string();
                    emitter.emit(EdifactEvent::Element { value: &s })
                }
            }
        )+
    };
}

impl_serialize_float!(f32, f64);

// ── public API ────────────────────────────────────────────────────────────────

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
        // f64::MAX Display is 309 chars; must not overflow fixed buffer.
        let mut emitter = VecEmitter::default();
        f64::MAX.edifact_serialize(&mut emitter).unwrap();
        let s = match &emitter.events[0] {
            OwnedEdifactEvent::Element { value } => value.clone(),
            _ => panic!("expected Element event"),
        };
        assert!(!s.is_empty());
        // f32::MAX too
        let mut emitter2 = VecEmitter::default();
        f32::MAX.edifact_serialize(&mut emitter2).unwrap();
        assert!(matches!(&emitter2.events[0], OwnedEdifactEvent::Element { .. }));
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
