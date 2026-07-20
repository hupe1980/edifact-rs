#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(unsafe_code)]

//! `edifact-rs` — zero-copy EDIFACT tokenizer, parser, writer, serde traits,
//! validation engine, and extensible directory support.
//!
//! `edifact-rs` is the main entry point of this workspace. The core parsing,
//! writing, and validation infrastructure is always available. Custom directory
//! validators can be implemented by downstream crates or generated through
//! external build tooling.
//!
//! # Quick start
//! ```
//! use edifact_rs::from_bytes;
//! let input = b"UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'UNZ+0+1'";
//! let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();
//! assert_eq!(segments[0].tag, "UNB");
//! ```
//!
//! # Crate features
//!
//! - `derive` (enabled by default): re-exports the derive macros from
//!   `edifact-rs-derive`.
//! - `diagnostics` (disabled by default): enables rich diagnostic output via `miette`.
//!   When enabled, errors implement `miette::Diagnostic` for enhanced error reporting.
//!   This feature adds an optional dependency and has no impact on parsing performance.
//! - `serde` (disabled by default): derives `Serialize` / `Deserialize` for
//!   [`ValidationReport`], [`ValidationIssue`], and the envelope types, so
//!   reports can be persisted or sent across a queue and read back.
//!
//! Features are additive and independent: enabling any combination changes only
//! which trait impls and re-exports are available, never parsing or validation
//! behaviour.
//!
//! The crate is expected to compile both with defaults and with
//! `--no-default-features` for consumers who only want the core parsing and
//! writing functionality.
//!
//! ## Feature matrix workflows
//!
//! - default features:
//!   `cargo test -p edifact-rs`
//! - no default features:
//!   `cargo test -p edifact-rs --no-default-features`
//! - all features:
//!   `cargo test -p edifact-rs --all-features`
//!
//! # Diagnostic Feature
//!
//! When the `diagnostics` feature is enabled, [`EdifactError`] gains additional
//! traits and methods that enable rich, human-readable error output:
//!
//! ```text
//! Error: invalid delimiter byte 0xAB at offset 42
//!
//!  ╭─ input.edi:2:3
//!  │
//!  2 │ UNB+UNOA:1+....[invalid]...
//!  │         ^^^ invalid byte here
//!  │
//! Error Code: E002
//! Help: The byte 0xAB is not a valid delimiter. Check UNA configuration
//! ```
//!
//! This feature is useful for CLI tools and error reporting, but is not required
//! for applications that handle errors programmatically.
//!
//! # Parse And Text Contracts
//!
//! Parsing in `edifact-rs` is strict and deterministic:
//!
//! - Segment and element text must decode as UTF-8 (`E003` on failure).
//! - Release characters must escape exactly one following byte.
//!   A trailing `?` at end-of-input is rejected (`E019`).
//! - Malformed delimiters and truncated segments are reported with stable
//!   error codes rather than panicking.
//!
//! These contracts apply to both slice-based parsing (`from_bytes`) and
//! reader-based parsing (`from_reader`).
//!
//! ```
//! use edifact_rs::from_reader_collect;
//! use std::io::Cursor;
//!
//! let input = b"UNA:;.? 'BGM;220;test?;value'";
//! let segments = from_reader_collect(Cursor::new(&input[..])).unwrap();
//! assert_eq!(segments.len(), 1);
//! assert_eq!(segments[0].tag, "BGM");
//! assert_eq!(segments[0].element_str(0), Some("220"));
//! assert_eq!(segments[0].element_str(1), Some("test;value"));
//! ```
//!
//! # Validation Quick Start
//!
//! The `Validator` trait and `ValidationContext` provide a flexible framework
//! for building custom validators. Users can generate validators from official
//! UNECE sources or implement their own.
//!
//! See the [`Validator`] trait documentation and the `cookbook_fixture_validation.rs`
//! example for details on creating custom validators.
//!
//! # Custom Profile Packs
//!
//! `ProfileRulePack` is the extension point for downstream MIG/profile crates.
//! Packs can be authored with public APIs only and plugged into a
//! [`ValidationContext`]:
//!
//! ```
//! use edifact_rs::{
//!     from_bytes, ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity,
//! };
//!
//! let segments: Vec<_> = from_bytes(b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'")
//!     .collect::<Result<_, _>>()?;
//!
//! let pack = ProfileRulePack::new("ORDERS-DEMO")
//!     .for_message_type("ORDERS")
//!     .with_stateless_rule_fn(|segments, issues| {
//!         if let Some(bgm) = segments.iter().find(|segment| segment.tag == "BGM") {
//!             if let Some(code) = bgm.get_element(0).and_then(|e| e.get_component(0)) {
//!                 if code == "220" {
//!                     issues.push(
//!                         ValidationIssue::new(
//!                             ValidationSeverity::Warning,
//!                             "demo pack rejects BGM 220 for illustration",
//!                         )
//!                         .with_rule_id("DEMO-P001")
//!                         .with_segment("BGM")
//!                         .with_element_index(0),
//!                     );
//!                 }
//!             }
//!         }
//!     });
//!
//! let report = ValidationContext::builder()
//!     .with_profile_pack(pack)
//!     .build()
//!     .validate_lenient(&segments);
//!
//! assert!(report.has_warnings());
//! let partner_report = report.filter_by_rule_prefix("DEMO-");
//! assert!(partner_report.total_issues() >= 1);
//! # Ok::<(), edifact_rs::EdifactError>(())
//! ```
//!
//! # Async Usage
//!
//! `edifact-rs` does not provide a native `async` API.  All parsing is
//! synchronous and driven by the standard `std::io::Read` / `std::io::BufRead`
//! traits.  The recommended integration pattern with async runtimes is:
//!
//! 1. Use your async runtime's read utilities to read the entire message into a
//!    `Vec<u8>` (e.g. `tokio::io::AsyncReadExt::read_to_end`).
//! 2. Parse the in-memory slice with [`from_bytes`].
//!
//! ```rust,no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // With tokio:
//! // let mut buf = Vec::new();
//! // reader.read_to_end(&mut buf).await?;
//! // let segments: Vec<_> = edifact_rs::from_bytes(&buf).collect::<Result<_, _>>()?;
//! # Ok(())
//! # }
//! ```
// ── core modules ──────────────────────────────────────────────────────────────
pub mod directory_validator;
pub(crate) mod envelope;
/// Error types and validation reporting primitives.
pub(crate) mod error;
pub mod group;
/// Core zero-copy and owned EDIFACT data model types.
pub(crate) mod model;
pub(crate) mod parser;
/// Validation report types: [`ValidationSeverity`], [`ValidationIssue`], [`ValidationReport`].
///
/// These types are also re-exported from the crate root.
pub mod report;
pub(crate) mod tokenizer;
pub(crate) mod validator;
pub(crate) mod writer;

// ── typed serialization layer ─────────────────────────────────────────────────
pub mod de;
pub(crate) mod event;
pub mod ser;

// ── flat re-exports: core ─────────────────────────────────────────────────────
pub use envelope::{
    FunctionalGroupEnvelope, GroupIdentifier, InterchangeEnvelope, LenientResult, MessageEnvelope,
    MessageIdentifier, ValidatedInterchange, parse_ung, parse_unh, validate_envelope,
    validate_envelope_from_owned, validate_envelope_lenient, validate_envelope_lenient_from_owned,
};
pub use error::{EdifactError, IoError};
pub use group::{
    GroupDef, SegmentGroupIndexed, group_owned_segments_indexed, group_segments_indexed,
};
pub use model::{
    BorrowedElement, BorrowedSegment, Element, OwnedElement, OwnedSegment, Segment, Span,
};
pub use parser::{
    OwnedSegmentStream, Parser, ReaderConfig, from_bufread, from_bufread_stream,
    from_bufread_stream_with_config, from_reader_with_config,
};
pub use report::{ValidationIssue, ValidationReport, ValidationSeverity};
pub use tokenizer::{ServiceStringAdvice, Token, Tokenizer};
pub use validator::{
    EnvelopeValidator, ProfileRule, ProfileRulePack, ValidationContext, ValidationContextBuilder,
    ValidationLayer, ValidationRuleContext, Validator, validate_each,
};
pub use writer::{MessageWriter, Writer};

// ── flat re-exports: serde ────────────────────────────────────────────────────

/// User-facing deserialization API.
pub use de::{
    CompositeElement, DispatchedMessage, EdifactCompositeDeserialize, EdifactDeserialize,
    EdifactSegmentTag, MessageDispatch, MessageWindow, MessageWindowsIter, MessageWindowsSliceIter,
    OwnedMessageWindow, SegmentAccessor, composite_element, contiguous_groups_by_qualifier,
    contiguous_groups_iter, deserialize, deserialize_all_from_reader, deserialize_all_streaming,
    deserialize_first_from_reader, deserialize_first_streaming, deserialize_messages_bytes,
    deserialize_messages_from_reader, deserialize_str, element_str, find_qualified_segment,
    find_qualified_segment_owned, find_segment, find_segment_owned, find_segment_typed,
    find_segments_iter, find_segments_typed, get_components_iter,
    groups_are_contiguous_by_qualifier, message_windows_from_reader, optional_component,
    optional_element, qualifier_matches_pattern, required_component, required_element,
};

/// Splits a byte slice into [`MessageWindow`] views, one per `UNH`/`UNT` envelope,
/// enabling parallel or lazy per-message processing without copying data.
///
/// # Example
/// ```rust,ignore
/// use edifact_rs::from_bytes_windows;
/// let windows: Vec<_> = from_bytes_windows(input).collect();
/// ```
pub use de::message_windows_bytes as from_bytes_windows;

// ── Proc-macro support ─────────────────────────────────────────────────────────

pub use directory_validator::{
    DirectoryValidator, DirectoryValidatorBuilder, ElementRef, OwnedElementRef, OwnedSegmentDef,
    SegmentDefinition, Status,
};
#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use edifact_rs_derive::{EdifactDeserialize, EdifactSerialize};
pub use event::{EdifactEvent, EventEmitter, OwnedEdifactEvent, VecEmitter, WriterEmitter};
pub use ser::{
    DecimalFloat, DecimalFloatDisplay, EdifactCompositeSerialize, EdifactSerialize, to_bytes,
    to_edifact_string,
};

// ── core free functions ───────────────────────────────────────────────────────

use std::io::{Read, Write};

/// Iterator returned by [`from_bytes`].
pub struct FromBytesIter<'a> {
    parser: Option<parser::Parser<'a>>,
    pending_error: Option<EdifactError>,
    /// Remaining segment allowance (`None` = unlimited).
    segments_remaining: Option<usize>,
    /// Maximum byte budget (`None` = unlimited).
    bytes_remaining: Option<u64>,
    /// Byte offset of the start of the current parse position (approximated
    /// as the sum of previously yielded segment spans — the borrowed tokenizer
    /// does not expose a byte counter, so we track it from `Segment::span`).
    bytes_consumed: u64,
}

/// Iterator returned by [`from_reader`].
pub struct FromReaderIter<R: Read> {
    inner: parser::OwnedSegmentStream<std::io::BufReader<R>>,
}

impl<R: Read> Iterator for FromReaderIter<R> {
    type Item = Result<OwnedSegment, EdifactError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<'a> Iterator for FromBytesIter<'a> {
    type Item = Result<Segment<'a>, EdifactError>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(err) = self.pending_error.take() {
            return Some(Err(err));
        }
        // max_segments guard
        if let Some(ref mut remaining) = self.segments_remaining {
            if *remaining == 0 {
                self.parser = None;
                return None;
            }
        }
        // max_input_bytes guard — uses absolute byte offset from the input start.
        // `bytes_consumed` holds `seg.span.end` of the last yielded segment, which
        // is an absolute position in the input slice and therefore naturally includes
        // the 9-byte UNA header and the segment terminator character.
        if let Some(max) = self.bytes_remaining {
            if self.bytes_consumed >= max {
                self.parser = None;
                return None;
            }
        }
        let item = self.parser.as_mut()?.next();
        if let Some(Ok(ref seg)) = item {
            // Decrement segment allowance
            if let Some(ref mut remaining) = self.segments_remaining {
                *remaining = remaining.saturating_sub(1);
            }
            // Track the absolute input position at the end of this segment.
            // `seg.span.end` is the byte offset just past the segment terminator —
            // a monotonically increasing absolute cursor that automatically accounts
            // for the UNA header, element/component separators, and terminators.
            self.bytes_consumed = seg.span.end as u64;
            if let Some(max) = self.bytes_remaining {
                if self.bytes_consumed >= max {
                    self.parser = None;
                }
            }
        }
        item
    }
}

/// Parse `input` bytes into an iterator of [`Segment`]s.
///
/// Borrows directly from `input` — zero allocation for segment data.
///
/// # Segment-size limit
///
/// Applies a default 64 KiB per-segment limit, matching the reader-based path.
/// Use [`from_bytes_with_config`] to override.
pub fn from_bytes(input: &[u8]) -> FromBytesIter<'_> {
    from_bytes_with_config(input, parser::ReaderConfig::default())
}

/// Parse `input` bytes into an iterator of [`Segment`]s with explicit configuration.
///
/// All three [`ReaderConfig`] limits are enforced:
/// - `max_segment_bytes`: returns [`EdifactError::SegmentTooLong`] if a single segment
///   exceeds the threshold.
/// - `max_segments`: stops the iterator after this many segments have been yielded.
/// - `max_input_bytes`: **stop-after** limit — the iterator stops once the cumulative
///   byte position (tracked via `Segment::span.end`) reaches or exceeds this value.
///   The last segment whose `span.end` exceeds the limit is **still returned**;
///   no further segments are fetched after that.  This means at most one segment
///   worth of bytes can be processed beyond the limit, which is sufficient for a
///   DoS guard but is not a strict hard cap.  If your use case requires that every
///   yielded segment fits entirely within `max_input_bytes` bytes, collect and
///   filter the output, or set the limit conservatively below the true boundary.
///
/// Pass `ReaderConfig::default()` to use the default 64 KiB per-segment limit with
/// no segment-count or byte-budget cap.
///
/// # Example
///
/// ```
/// use edifact_rs::{ReaderConfig, from_bytes_with_config};
///
/// let cfg = ReaderConfig::default().max_segment_bytes(128);
/// let result: Result<Vec<_>, _> = from_bytes_with_config(b"BGM+220+1+9'", cfg).collect();
/// assert!(result.is_ok());
/// ```
pub fn from_bytes_with_config(input: &[u8], config: parser::ReaderConfig) -> FromBytesIter<'_> {
    let segments_remaining = config.max_segments;
    let bytes_remaining = config.max_input_bytes;
    match tokenizer::ServiceStringAdvice::from_bytes(input) {
        Ok(ssa) => {
            let t = tokenizer::Tokenizer::with_limit(input, ssa, config.max_segment_bytes);
            FromBytesIter {
                parser: Some(parser::Parser::new(t)),
                pending_error: None,
                segments_remaining,
                bytes_remaining,
                bytes_consumed: 0,
            }
        }
        Err(error) => FromBytesIter {
            parser: None,
            pending_error: Some(error),
            segments_remaining,
            bytes_remaining,
            bytes_consumed: 0,
        },
    }
}

/// Parse a reader into a lazy iterator of [`OwnedSegment`]s.
///
/// Returns a [`FromReaderIter`] that parses and yields segments on demand,
/// keeping memory bounded. Use [`from_reader_collect`] to eagerly materialise
/// all segments into a `Vec`.
///
/// # Errors
///
/// Each `next()` call yields `Some(Ok(segment))` for a successfully parsed
/// segment, `Some(Err(EdifactError))` for a parse or I/O failure, and `None`
/// when the end of the stream has been reached.
pub fn from_reader<R: Read>(reader: R) -> FromReaderIter<R> {
    FromReaderIter {
        inner: parser::from_reader_stream(reader),
    }
}

/// Parse a reader into an owned `Vec` of all segments.
///
/// Eagerly collects the full interchange into memory. If you only need a
/// subset of segments, prefer [`from_reader`] (lazy iterator) to avoid
/// unnecessary allocations.
///
/// # Errors
///
/// Returns an error if the input contains malformed EDIFACT syntax,
/// invalid UTF-8 segment text, dangling release sequences, or underlying I/O failures.
pub fn from_reader_collect<R: Read>(reader: R) -> Result<Vec<OwnedSegment>, EdifactError> {
    parser::from_reader(reader)
}

/// Parse `input` bytes eagerly into an iterator of [`OwnedSegment`]s.
///
/// Unlike [`from_bytes`] (which yields borrowed [`Segment`]s tied to the input
/// lifetime), every segment returned here is fully owned.  This is convenient
/// when you need to store or return segments without retaining a reference to
/// the original byte slice.
///
/// # Example
///
/// ```
/// let segs: Vec<edifact_rs::OwnedSegment> = edifact_rs::from_bytes_owned(b"BGM+220+1+9'")
///     .collect::<Result<_, _>>()
///     .unwrap();
/// assert_eq!(segs[0].tag, "BGM");
/// ```
pub fn from_bytes_owned(
    input: &[u8],
) -> impl Iterator<Item = Result<OwnedSegment, EdifactError>> + '_ {
    from_bytes(input).map(|r| r.map(OwnedSegment::from))
}

/// Parse `input` bytes eagerly into an iterator of [`OwnedSegment`]s with a
/// custom [`ReaderConfig`].
///
/// Identical to [`from_bytes_owned`] but applies the limits and settings from
/// `config` (e.g. `max_segment_bytes`, `max_segments`, `max_input_bytes`).
///
/// # Example
///
/// ```
/// use edifact_rs::ReaderConfig;
/// let config = ReaderConfig::default().max_segments(10);
/// let segs: Vec<edifact_rs::OwnedSegment> = edifact_rs::from_bytes_owned_with_config(
///     b"BGM+220+1+9'",
///     config,
/// )
/// .collect::<Result<_, _>>()
/// .unwrap();
/// assert_eq!(segs[0].tag, "BGM");
/// ```
pub fn from_bytes_owned_with_config(
    input: &[u8],
    config: ReaderConfig,
) -> impl Iterator<Item = Result<OwnedSegment, EdifactError>> + '_ {
    from_bytes_with_config(input, config).map(|r| r.map(OwnedSegment::from))
}

/// Serialize `segments` to an [`std::io::Write`] implementation.
///
/// # Errors
///
/// Returns an error if writing fails or if segment serialization fails.
pub fn to_writer<'a, 'b, W, I>(w: W, segments: I) -> Result<(), EdifactError>
where
    'b: 'a,
    W: Write,
    I: IntoIterator<Item = &'a Segment<'b>>,
{
    let mut wr = writer::Writer::new(w);
    for seg in segments {
        wr.write_segment(seg)?;
    }
    wr.finish().map(|_| ())
}

/// Serialize `segments` to an owned `Vec<u8>`.
///
/// # Errors
///
/// Returns an error if serialization fails.
pub fn segments_to_bytes<'a, 'b, I>(segments: I) -> Result<Vec<u8>, EdifactError>
where
    'b: 'a,
    I: IntoIterator<Item = &'a Segment<'b>>,
{
    let mut buf = Vec::new();
    to_writer(&mut buf, segments)?;
    Ok(buf)
}

/// Serialize a slice of [`OwnedSegment`]s to an owned `Vec<u8>`.
///
/// Convenience wrapper around [`to_writer`] that accepts owned segments
/// directly.  Each segment is converted to its borrowed form on the fly
/// and written immediately — no intermediate `Vec<Segment<'_>>` is
/// allocated, so peak memory stays proportional to one segment at a time
/// rather than the full slice.
///
/// # Errors
///
/// Returns an error if serialization fails.
pub fn segments_to_bytes_owned(segments: &[OwnedSegment]) -> Result<Vec<u8>, EdifactError> {
    let mut buf = Vec::new();
    let mut wr = writer::Writer::new(&mut buf);
    for seg in segments {
        wr.write_segment(&seg.as_borrowed())?;
    }
    wr.finish()?;
    Ok(buf)
}

/// Validate the envelope structure of an owned-segment slice.
///
/// Convenience wrapper that accepts `&[OwnedSegment]` without requiring a
/// manual conversion to borrowed segments.  Unlike the previous implementation,
/// no intermediate `Vec<Segment<'_>>` is allocated — segments are read directly.
///
/// # Errors
///
/// Returns an error if the envelope is structurally invalid.
pub fn validate_envelope_owned(
    segments: &[OwnedSegment],
) -> Result<ValidatedInterchange, EdifactError> {
    envelope::validate_envelope_from_owned(segments)
}

/// Lenient envelope validation over owned segments — collects all errors.
///
/// Convenience wrapper around [`validate_envelope_lenient_from_owned`].
/// Returns a [`LenientResult`] with `Some(result)` and empty errors on success.
/// On count-only violations, returns `Some(partial)` with errors.
/// On structural failures, returns `None` with errors.
pub fn validate_envelope_lenient_owned(segments: &[OwnedSegment]) -> LenientResult {
    envelope::validate_envelope_lenient_from_owned(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_bytes_rejects_invalid_una() {
        let err = from_bytes(b"UNA::.? 'BGM:220'")
            .collect::<Result<Vec<_>, _>>()
            .expect_err("invalid UNA should fail slice parsing");
        assert!(matches!(err, EdifactError::InvalidUna));
    }
}

/// Compiles and runs every ```` ```rust ```` block in the `docs/` guides as a
/// doctest.
///
/// The guides drifted from the API — snippets referenced private module paths
/// and methods that did not exist — because nothing ever compiled them. Wiring
/// them in here means a rename that breaks a guide breaks the build.
///
/// Blocks that genuinely cannot run (they need a live socket, a real directory
/// file, or a downstream crate) should be marked ```` ```rust,ignore ```` or
/// ```` ```rust,no_run ```` in the guide itself.
#[cfg(doctest)]
mod doc_guides {
    macro_rules! guide {
        ($name:ident, $path:literal) => {
            #[doc = include_str!($path)]
            pub struct $name;
        };
    }

    guide!(CoreConcepts, "../../../docs/core-concepts.md");
    guide!(Parsing, "../../../docs/parsing.md");
    guide!(ProfilePacks, "../../../docs/profile-packs.md");
    guide!(Validation, "../../../docs/validation.md");

    // Guides whose examples use the derive macros.
    #[cfg(feature = "derive")]
    guide!(AsyncIntegration, "../../../docs/async-integration.md");
    #[cfg(feature = "derive")]
    guide!(ErrorReference, "../../../docs/error-reference.md");
    #[cfg(feature = "derive")]
    guide!(GettingStarted, "../../../docs/getting-started.md");
    #[cfg(feature = "derive")]
    guide!(Performance, "../../../docs/performance.md");
    #[cfg(feature = "derive")]
    guide!(Streaming, "../../../docs/streaming.md");
    #[cfg(feature = "derive")]
    guide!(TypedDerive, "../../../docs/typed-derive.md");
    #[cfg(feature = "derive")]
    guide!(Writing, "../../../docs/writing.md");

    // The diagnostics guide's examples use `miette` types.
    #[cfg(feature = "diagnostics")]
    guide!(Diagnostics, "../../../docs/diagnostics.md");
}
