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

impl<'a> EdifactEvent<'a> {
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
/// `StartSegment` → zero or more `Element` / `ComponentElement` → `EndSegment`.
/// In debug builds, any violation of this order (e.g. `Element` before
/// `StartSegment`, or `StartSegment` inside an open segment) panics immediately.
pub struct WriterEmitter<W: Write> {
    writer: crate::Writer<W>,
    #[cfg(debug_assertions)]
    in_segment: bool,
}

impl<W: Write> WriterEmitter<W> {
    /// Create a new `WriterEmitter` with default EDIFACT delimiters.
    pub fn new(inner: W) -> Self {
        Self {
            writer: crate::Writer::new(inner),
            #[cfg(debug_assertions)]
            in_segment: false,
        }
    }

    /// Flush and consume the emitter, returning the underlying writer.
    pub fn finish(self) -> Result<W, EdifactError> {
        self.writer.finish()
    }

    /// Number of complete segments written so far.
    pub fn segment_count(&self) -> u32 {
        self.writer.segment_count()
    }
}

impl<W: Write> EventEmitter for WriterEmitter<W> {
    fn emit(&mut self, event: EdifactEvent<'_>) -> Result<(), EdifactError> {
        match event {
            EdifactEvent::StartSegment { tag } => {
                #[cfg(debug_assertions)]
                {
                    assert!(
                        !self.in_segment,
                        "WriterEmitter: StartSegment emitted while a segment is already open (missing EndSegment)"
                    );
                    self.in_segment = true;
                }
                self.writer.write_tag_only(tag)?;
            }
            EdifactEvent::Element { value } => {
                #[cfg(debug_assertions)]
                assert!(
                    self.in_segment,
                    "WriterEmitter: Element emitted outside of a segment (missing StartSegment)"
                );
                self.writer.write_element_sep()?;
                self.writer.write_escaped(value)?;
            }
            EdifactEvent::ComponentElement { value } => {
                #[cfg(debug_assertions)]
                assert!(
                    self.in_segment,
                    "WriterEmitter: ComponentElement emitted outside of a segment (missing StartSegment)"
                );
                self.writer.write_component_sep()?;
                self.writer.write_escaped(value)?;
            }
            EdifactEvent::EndSegment => {
                #[cfg(debug_assertions)]
                {
                    assert!(
                        self.in_segment,
                        "WriterEmitter: EndSegment emitted while no segment is open (missing StartSegment)"
                    );
                    self.in_segment = false;
                }
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
}
