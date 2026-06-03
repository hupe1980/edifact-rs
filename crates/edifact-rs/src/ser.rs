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

// Rust's float Display picks the shortest round-trip decimal representation,
// which may be fixed-point or scientific notation depending on the magnitude.
// A 320-byte stack buffer covers all known finite f32/f64 Display forms;
// if the buffer is somehow exceeded we fall back to a heap-allocated String
// so no panic ever escapes to the caller.
//
// NOTE: Rust's Display always uses `.` as the decimal separator. The decimal
// mark configured in `ServiceStringAdvice.decimal_mark` (UNA byte 5) is NOT
// respected by these impls. If your interchange declares a different decimal
// mark (e.g. `,`), wrap the value in a newtype whose `EdifactSerialize` impl
// formats with the correct separator.
macro_rules! impl_serialize_float {
    ($($t:ty),+ $(,)?) => {
        $(
            impl EdifactSerialize for $t {
                fn edifact_serialize<E: EventEmitter>(&self, emitter: &mut E) -> Result<(), EdifactError> {
                    use std::io::Write as _;
                    let mut buf = [0u8; 320];
                    let mut w: &mut [u8] = &mut buf;
                    if write!(w, "{self}").is_ok() {
                        let written = 320 - w.len();
                        // SAFETY: float Display only emits ASCII digits and punctuation.
                        let s = std::str::from_utf8(&buf[..written]).map_err(|_| EdifactError::InvalidUtf8)?;
                        emitter.emit(EdifactEvent::Element { value: s })
                    } else {
                        // Extraordinary case: format via heap to avoid any panic.
                        let s = format!("{self}");
                        emitter.emit(EdifactEvent::Element { value: &s })
                    }
                }
            }
        )+
    };
}

impl_serialize_float!(f32, f64);

// ── decimal-mark-aware float wrapper ─────────────────────────────────────────

/// A float value that serializes using the interchange's configured decimal mark.
///
/// Rust's [`Display`][std::fmt::Display] for `f32`/`f64` always uses `.` as the
/// decimal separator.  EDIFACT interchanges can declare a different decimal mark
/// in the UNA service string — most commonly `,` in German EDI@Energy messages.
/// Bare `f32`/`f64` [`EdifactSerialize`] impls are correct for standard (`.`) interchanges
/// but produce **silent data corruption** for `,` interchanges.
///
/// Wrap a float in `DecimalFloat` when the interchange may use a non-`.` decimal mark:
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
    let mark_char = mark as char;
    if s.contains('.') {
        let replaced = s.replace('.', &mark_char.to_string());
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
        // Rust Display for f64 picks the shortest round-trip form; a 320-byte buffer covers all values.
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
