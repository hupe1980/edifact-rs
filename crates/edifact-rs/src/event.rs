//! Event model for EDIFACT (de)serialization.
//!
//! [`EdifactEvent`] is the borrowed, zero-allocation form used during real-time
//! emission.  [`OwnedEdifactEvent`] is the owned form collected by [`VecEmitter`]
//! for testing and introspection — no `Box::leak` anywhere.

use crate::EdifactError;
use std::io::Write;

// ── event types ───────────────────────────────────────────────────────────────

/// A borrowed EDIFACT event emitted during serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EdifactEvent<'a> {
    /// Beginning of a new segment (e.g. `"BGM"`, `"NAD"`).
    StartSegment {
        /// Segment tag.
        tag: &'a str,
    },
    /// A data element value — first (or only) component of a new element.
    Element {
        /// Element text value.
        value: &'a str,
    },
    /// An additional component within the current element.
    ComponentElement {
        /// Component text value.
        value: &'a str,
    },
    /// End of the current segment.
    EndSegment,
}

/// An owned EDIFACT event — for collection and testing (no borrowed lifetimes).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OwnedEdifactEvent {
    /// Owned segment-start event.
    StartSegment {
        /// Segment tag.
        tag: String,
    },
    /// Owned element event.
    Element {
        /// Element text value.
        value: String,
    },
    /// Owned component event.
    ComponentElement {
        /// Component text value.
        value: String,
    },
    /// Owned segment-end event.
    EndSegment,
}

impl EdifactEvent<'_> {
    /// Convert to an owned event, cloning string data.
    pub fn into_owned(self) -> OwnedEdifactEvent {
        match self {
            Self::StartSegment { tag } => OwnedEdifactEvent::StartSegment {
                tag: tag.to_owned(),
            },
            Self::Element { value } => OwnedEdifactEvent::Element {
                value: value.to_owned(),
            },
            Self::ComponentElement { value } => OwnedEdifactEvent::ComponentElement {
                value: value.to_owned(),
            },
            Self::EndSegment => OwnedEdifactEvent::EndSegment,
        }
    }
}

// ── emitter trait ─────────────────────────────────────────────────────────────

/// Trait for any sink that can consume [`EdifactEvent`]s.
pub trait EventEmitter {
    /// Consume one event.
    fn emit(&mut self, event: EdifactEvent<'_>) -> Result<(), EdifactError>;

    /// Return the decimal-mark byte used by the interchange (`b'.'` by default).
    ///
    /// Serializers that format numeric values (e.g. [`crate::ser::DecimalFloat`])
    /// call this to discover whether to emit `12.5` or `12,5`.
    ///
    /// The default implementation returns `b'.'`, which is correct for standard
    /// EDIFACT interchanges that do not declare a UNA service string or that use
    /// the ISO 9735 default.  Override this in emitters backed by a
    /// [`crate::Writer`] with a custom [`crate::tokenizer::ServiceStringAdvice`].
    #[inline]
    fn decimal_mark(&self) -> u8 {
        b'.'
    }
}

// ── VecEmitter ────────────────────────────────────────────────────────────────

/// Collects events into a [`Vec<OwnedEdifactEvent>`].
///
/// Useful for testing and introspection.  Does not leak memory.
#[derive(Debug, Default)]
pub struct VecEmitter {
    /// Collected owned events.
    pub events: Vec<OwnedEdifactEvent>,
}

impl EventEmitter for VecEmitter {
    fn emit(&mut self, event: EdifactEvent<'_>) -> Result<(), EdifactError> {
        self.events.push(event.into_owned());
        Ok(())
    }
}

// ── WriterEmitter ─────────────────────────────────────────────────────────────

/// Internal protocol-state machine for [`WriterEmitter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitterState {
    /// Between segments: no open segment.
    Idle,
    /// A [`EdifactEvent::StartSegment`] has been emitted; no element written yet.
    InSegment,
    /// An [`EdifactEvent::Element`] has been emitted; `ComponentElement` is valid.
    InElement,
}

/// Writes EDIFACT events directly to any [`Write`] implementation.
///
/// Each event is written to the underlying writer immediately — no intermediate
/// buffering of element strings occurs, so no heap allocation is required per
/// event.  This makes `WriterEmitter` suitable for high-throughput serialization
/// of large EDIFACT messages.
///
/// # Protocol
///
/// Events must arrive in the order produced by [`crate::EdifactSerialize`]:
/// `StartSegment` → zero or more (`Element` → zero or more `ComponentElement`) → `EndSegment`.
///
/// Any violation of this protocol returns
/// [`EdifactError::InvalidEventSequence`] immediately.  Violations are
/// detected in both debug and release builds.
pub struct WriterEmitter<W: Write> {
    writer: crate::Writer<W>,
    state: EmitterState,
}

impl<W: Write> WriterEmitter<W> {
    /// Create a new `WriterEmitter` with default EDIFACT delimiters.
    pub fn new(inner: W) -> Self {
        Self {
            writer: crate::Writer::new(inner),
            state: EmitterState::Idle,
        }
    }

    /// Create a new `WriterEmitter` with custom delimiters, writing a UNA header first.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::InvalidUna`] when `ssa.is_valid()` is false.
    pub fn with_una(
        inner: W,
        ssa: crate::tokenizer::ServiceStringAdvice,
    ) -> Result<Self, crate::EdifactError> {
        Ok(Self {
            writer: crate::Writer::with_una(inner, ssa)?,
            state: EmitterState::Idle,
        })
    }

    /// Flush and consume the emitter, returning the underlying writer.
    pub fn finish(self) -> Result<W, EdifactError> {
        self.writer.finish()
    }

    /// Number of complete segments written so far.
    pub fn segment_count(&self) -> u64 {
        self.writer.segment_count()
    }

    /// Return the active [`ServiceStringAdvice`][crate::tokenizer::ServiceStringAdvice].
    ///
    /// Callers can use this to format values (e.g., floats) using the correct
    /// decimal-mark character configured in the UNA header.
    pub fn service_string_advice(&self) -> crate::tokenizer::ServiceStringAdvice {
        self.writer.service_string_advice()
    }
}

impl<W: Write> EventEmitter for WriterEmitter<W> {
    #[inline]
    fn decimal_mark(&self) -> u8 {
        self.writer.service_string_advice().decimal_mark
    }

    fn emit(&mut self, event: EdifactEvent<'_>) -> Result<(), EdifactError> {
        match event {
            EdifactEvent::StartSegment { tag } => {
                if self.state != EmitterState::Idle {
                    return Err(EdifactError::InvalidEventSequence {
                        message: "StartSegment emitted while a segment is already open; emit EndSegment first",
                    });
                }
                self.state = EmitterState::InSegment;
                self.writer.write_tag_only(tag)?;
            }
            EdifactEvent::Element { value } => {
                if self.state == EmitterState::Idle {
                    return Err(EdifactError::InvalidEventSequence {
                        message: "Element emitted outside of a segment; emit StartSegment first",
                    });
                }
                self.state = EmitterState::InElement;
                self.writer.write_element_sep()?;
                self.writer.write_escaped(value)?;
            }
            EdifactEvent::ComponentElement { value } => {
                if self.state != EmitterState::InElement {
                    return Err(EdifactError::InvalidEventSequence {
                        message: "ComponentElement emitted without a preceding Element in the same segment",
                    });
                }
                self.writer.write_component_sep()?;
                self.writer.write_escaped(value)?;
            }
            EdifactEvent::EndSegment => {
                if self.state == EmitterState::Idle {
                    return Err(EdifactError::InvalidEventSequence {
                        message: "EndSegment emitted while no segment is open; emit StartSegment first",
                    });
                }
                self.state = EmitterState::Idle;
                self.writer.write_segment_term_and_count()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_emitter_no_memory_leak() {
        let mut e = VecEmitter::default();
        e.emit(EdifactEvent::StartSegment { tag: "BGM" }).unwrap();
        e.emit(EdifactEvent::Element { value: "E03" }).unwrap();
        e.emit(EdifactEvent::EndSegment).unwrap();
        assert_eq!(
            e.events[0],
            OwnedEdifactEvent::StartSegment {
                tag: "BGM".to_owned()
            }
        );
        assert_eq!(
            e.events[1],
            OwnedEdifactEvent::Element {
                value: "E03".to_owned()
            }
        );
    }

    #[test]
    fn writer_emitter_produces_valid_edifact() {
        let mut buf = Vec::new();
        {
            let mut e = WriterEmitter::new(&mut buf);
            e.emit(EdifactEvent::StartSegment { tag: "BGM" }).unwrap();
            e.emit(EdifactEvent::Element { value: "E03" }).unwrap();
            e.emit(EdifactEvent::Element { value: "11042" }).unwrap();
            e.emit(EdifactEvent::EndSegment).unwrap();
            e.finish().unwrap();
        }
        assert_eq!(buf, b"BGM+E03+11042'");
    }

    #[test]
    fn writer_emitter_handles_components() {
        let mut buf = Vec::new();
        {
            let mut e = WriterEmitter::new(&mut buf);
            e.emit(EdifactEvent::StartSegment { tag: "NAD" }).unwrap();
            e.emit(EdifactEvent::Element { value: "MS" }).unwrap();
            e.emit(EdifactEvent::Element {
                value: "9900112233445",
            })
            .unwrap();
            e.emit(EdifactEvent::ComponentElement { value: "" })
                .unwrap();
            e.emit(EdifactEvent::ComponentElement { value: "293" })
                .unwrap();
            e.emit(EdifactEvent::EndSegment).unwrap();
            e.finish().unwrap();
        }
        let s = std::str::from_utf8(&buf).unwrap();
        assert_eq!(s, "NAD+MS+9900112233445::293'");
    }

    // ── protocol-violation tests (BUG 2.1) ───────────────────────────────────

    #[test]
    fn writer_emitter_element_before_start_segment_is_err() {
        let mut e = WriterEmitter::new(Vec::<u8>::new());
        let err = e.emit(EdifactEvent::Element { value: "X" }).unwrap_err();
        assert!(
            matches!(err, crate::EdifactError::InvalidEventSequence { .. }),
            "expected InvalidEventSequence, got {err:?}"
        );
    }

    #[test]
    fn writer_emitter_component_before_element_is_err() {
        let mut e = WriterEmitter::new(Vec::<u8>::new());
        e.emit(EdifactEvent::StartSegment { tag: "BGM" }).unwrap();
        let err = e
            .emit(EdifactEvent::ComponentElement { value: "X" })
            .unwrap_err();
        assert!(
            matches!(err, crate::EdifactError::InvalidEventSequence { .. }),
            "expected InvalidEventSequence, got {err:?}"
        );
    }

    #[test]
    fn writer_emitter_double_start_segment_is_err() {
        let mut e = WriterEmitter::new(Vec::<u8>::new());
        e.emit(EdifactEvent::StartSegment { tag: "BGM" }).unwrap();
        let err = e
            .emit(EdifactEvent::StartSegment { tag: "DTM" })
            .unwrap_err();
        assert!(
            matches!(err, crate::EdifactError::InvalidEventSequence { .. }),
            "expected InvalidEventSequence, got {err:?}"
        );
    }

    #[test]
    fn writer_emitter_end_segment_without_start_is_err() {
        let mut e = WriterEmitter::new(Vec::<u8>::new());
        let err = e.emit(EdifactEvent::EndSegment).unwrap_err();
        assert!(
            matches!(err, crate::EdifactError::InvalidEventSequence { .. }),
            "expected InvalidEventSequence, got {err:?}"
        );
    }
}
