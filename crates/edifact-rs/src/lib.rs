#![cfg_attr(docsrs, feature(doc_cfg))]

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
//!   A trailing `?` at end-of-input is rejected (`E018`).
//! - Malformed delimiters and truncated segments are reported with stable
//!   error codes rather than panicking.
//!
//! These contracts apply to both slice-based parsing (`from_bytes`) and
//! reader-based parsing (`from_reader`).
//!
//! ```
//! use edifact_rs::from_reader;
//! use std::io::Cursor;
//!
//! let input = b"UNA:;.? 'BGM;220;test?;value'";
//! let segments = from_reader(Cursor::new(&input[..])).unwrap();
//! assert_eq!(segments.len(), 1);
//! assert_eq!(segments[0].tag, "BGM");
//! assert_eq!(segments[0].elements[0].components[0], "220");
//! assert_eq!(segments[0].elements[1].components[0], "test;value");
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
//! let pack = ProfileRulePack::builder("ORDERS-DEMO")
//!     .for_message_type("ORDERS")
//!     .with_rule_fn(|segments| {
//!         let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
//!         let document_code = bgm.get_element(0)?.get_component(0)?;
//!         (document_code == "220").then(|| {
//!             ValidationIssue::new(
//!                 ValidationSeverity::Warning,
//!                 "demo pack rejects BGM 220 for illustration",
//!             )
//!             .with_rule_id("DEMO-P001")
//!             .with_segment("BGM")
//!             .with_element_index(0)
//!         })
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
//!
//! A native zero-copy streaming async API is tracked as a future roadmap item.
// ── core modules ──────────────────────────────────────────────────────────────
pub mod directory_validator;
pub mod envelope;
/// Error types and validation reporting primitives.
pub mod error;
/// Core zero-copy and owned EDIFACT data model types.
pub mod model;
pub mod parser;
pub mod tokenizer;
pub mod validator;
pub mod writer;

// ── typed serialization layer ─────────────────────────────────────────────────
pub mod de;
pub mod event;
pub mod ser;

// ── flat re-exports: core ─────────────────────────────────────────────────────
pub use envelope::validate_envelope;
pub use error::{EdifactError, IoError, ValidationIssue, ValidationReport, ValidationSeverity};
pub use model::{BorrowedElement, BorrowedSegment, Element, OwnedElement, OwnedSegment, Segment, Span};
pub use parser::{
    Parser, ReaderConfig, from_bufread, from_bufread_stream, from_bufread_stream_with_config,
    from_reader_with_config,
};
pub use tokenizer::{ServiceStringAdvice, Tokenizer};
pub use validator::{
    ProfileRule, ProfileRulePack, ValidationContext, ValidationContextBuilder, ValidationLayer,
    Validator, validate_each,
};
pub use writer::Writer;

// ── flat re-exports: serde ────────────────────────────────────────────────────
pub use de::{
    CompositeElement, EdifactCompositeDeserialize, EdifactDeserialize, EdifactSegmentTag,
    MessageWindowsIter, MessageWindowsSliceIter, SegmentAccessor, composite_element,
    contiguous_groups_by_qualifier, deserialize, deserialize_all_from_reader,
    deserialize_all_streaming, deserialize_first_from_reader, deserialize_first_streaming,
    deserialize_messages_bytes, deserialize_messages_from_reader, deserialize_str, element_str,
    find_qualified_segment, find_qualified_segment_owned, find_segment, find_segment_owned,
    find_segment_typed, find_segments_iter, find_segments_typed, get_components_iter,
    groups_are_contiguous_by_qualifier,
    message_windows_bytes, message_windows_from_reader, optional_component, optional_element,
    qualifier_matches_pattern, required_component, required_element,
};
#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use edifact_rs_derive::{EdifactDeserialize, EdifactSerialize};
pub use event::{EdifactEvent, EventEmitter, OwnedEdifactEvent, VecEmitter, WriterEmitter};
pub use ser::{EdifactCompositeSerialize, EdifactSerialize, to_string};
pub use directory_validator::{DirectoryValidator, ElementRef, SegmentDefinition, Status};

// ── core free functions ───────────────────────────────────────────────────────

use std::io::{Read, Write};

/// Iterator returned by [`from_bytes`].
pub struct FromBytesIter<'a> {
    parser: Option<parser::Parser<'a>>,
    pending_error: Option<EdifactError>,
}

/// Iterator returned by [`from_reader_iter`].
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
        self.parser.as_mut()?.next()
    }
}

/// Parse `input` bytes into an iterator of [`Segment`]s.
///
/// Borrows directly from `input` — zero allocation for segment data.
pub fn from_bytes(input: &[u8]) -> FromBytesIter<'_> {
    match tokenizer::ServiceStringAdvice::from_bytes_strict(input) {
        Ok(ssa) => {
            let t = tokenizer::Tokenizer::new(input, ssa);
            FromBytesIter {
                parser: Some(parser::Parser::new(t)),
                pending_error: None,
            }
        }
        Err(error) => FromBytesIter {
            parser: None,
            pending_error: Some(error),
        },
    }
}

/// Parse a reader into owned segments.
///
/// # Errors
///
/// Returns an error if the input contains malformed EDIFACT syntax,
/// invalid UTF-8 segment text, dangling release sequences, or underlying I/O failures.
pub fn from_reader<R: Read>(reader: R) -> Result<Vec<OwnedSegment>, EdifactError> {
    parser::from_reader(reader)
}

/// Parse a reader into owned segments as a streaming iterator.
///
/// This keeps memory bounded by yielding segments incrementally instead of
/// materializing the full interchange up front.
pub fn from_reader_iter<R: Read>(reader: R) -> FromReaderIter<R> {
    FromReaderIter {
        inner: parser::from_reader_stream(reader),
    }
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
pub fn to_bytes<'a, 'b, I>(segments: I) -> Result<Vec<u8>, EdifactError>
where
    'b: 'a,
    I: IntoIterator<Item = &'a Segment<'b>>,
{
    let mut buf = Vec::new();
    to_writer(&mut buf, segments)?;
    Ok(buf)
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
