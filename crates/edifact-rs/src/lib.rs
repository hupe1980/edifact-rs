#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(unsafe_code)]

//! `edifact-rs` — zero-copy EDIFACT (ISO 9735) tokenizer, parser, writer, typed
//! (de)serialization, validation engine, and extensible directory support.
//!
//! # Quick start
//! ```
//! use edifact_rs::from_bytes;
//! let input = b"UNB+UNOA:1+SENDER+RECEIVER+200101:0900+1'UNZ+0+1'";
//! let segments: Vec<_> = from_bytes(input).collect::<Result<_, _>>().unwrap();
//! assert_eq!(segments[0].tag, "UNB");
//! ```
//!
//! # One segment type, borrowed or owned
//!
//! [`Segment<'a>`] holds its text as [`Cow<'a, str>`][std::borrow::Cow], which is
//! what lets a single type cover both parsing modes:
//!
//! - [`from_bytes`] yields `Segment<'input>` whose text borrows from the input
//!   ([`Cow::Borrowed`][std::borrow::Cow::Borrowed]) unless a release escape
//!   forced a copy; a segment's own allocation is its element vector.
//! - [`from_reader`] has no buffer to borrow from and yields `Segment<'static>`,
//!   aliased as [`OwnedSegment`], with every non-empty value owned.
//!
//! `Segment` is covariant in `'a`, so `&[OwnedSegment]` is accepted anywhere
//! `&[Segment<'_>]` is wanted. Every API in this crate therefore takes one shape
//! and serves both paths — there are no `_owned` twins, and no conversion step.
//!
//! ```
//! use edifact_rs::{OwnedSegment, Segment};
//!
//! fn count_bgm(segments: &[Segment<'_>]) -> usize {
//!     segments.iter().filter(|s| s.tag == "BGM").count()
//! }
//!
//! let borrowed: Vec<Segment<'_>> =
//!     edifact_rs::from_bytes(b"BGM+220'").collect::<Result<_, _>>()?;
//! let owned: Vec<OwnedSegment> =
//!     edifact_rs::from_reader(std::io::Cursor::new(b"BGM+220'")).collect::<Result<_, _>>()?;
//!
//! assert_eq!(count_bgm(&borrowed), 1);
//! assert_eq!(count_bgm(&owned), 1);
//! # Ok::<(), edifact_rs::EdifactError>(())
//! ```
//!
//! # Crate features
//!
//! - `derive` (enabled by default): re-exports the derive macros from
//!   `edifact-rs-derive` — [`EdifactDeserialize`][macro@EdifactDeserialize] /
//!   [`EdifactSerialize`][macro@EdifactSerialize] for segment and message
//!   structs, and
//!   [`EdifactCompositeDeserialize`][macro@EdifactCompositeDeserialize] /
//!   [`EdifactCompositeSerialize`][macro@EdifactCompositeSerialize] for the
//!   composite-element structs they reference.
//! - `diagnostics` (off by default): [`EdifactError`] implements
//!   `miette::Diagnostic`, for span-annotated CLI output.
//! - `serde` (off by default): `Serialize` / `Deserialize` for
//!   [`ValidationReport`], [`ValidationIssue`], and the envelope types.
//!
//! Features are additive and independent: each changes only which trait impls
//! and re-exports exist, never parsing or validation behaviour.
//!
//! # Parse and text contracts
//!
//! Parsing in `edifact-rs` is strict and deterministic:
//!
//! - A byte order mark and any whitespace before the first service segment are
//!   skipped — ISO 9735 authorises neither, but both arrive constantly.
//! - A segment that ends without its terminator is a **truncation** (`E001`,
//!   [`EdifactError::UnexpectedEof`]):
//!   accepting it would let a file cut off mid-transfer parse as complete.
//! - Segment and element text must decode as UTF-8 (`E003`).
//! - A release character must escape exactly one following byte; a trailing `?`
//!   at end-of-input is rejected (`E019`).
//! - Every [`ReaderConfig`] budget **reports** a violation (`E020`, `E036`)
//!   rather than ending the iterator, which would be indistinguishable from a
//!   clean end of input.
//! - The service characters are discovered the way ISO 9735-1 says a receiver
//!   should discover them: from a leading `UNA` if there is one, otherwise the
//!   §5.1 defaults with the repetition separator resolved from the syntax
//!   version in `UNB` S001 DE 0002 — active as `*` for version 4, inactive for
//!   versions 1–3, where `*` is ordinary data. Override both with
//!   [`ReaderConfig::with_service_string_advice`] when parsing a fragment that
//!   carries neither header.
//! - When the repetition separator is active, repeating data elements are split
//!   into [`Element::repetitions`] rather than left glued into the value
//!   (ISO 9735-1 §8.7).
//!
//! Every one of these contracts applies identically to slice-based parsing
//! ([`from_bytes`]) and reader-based parsing ([`from_reader`]); the two are held
//! to byte-for-byte agreement by a test that runs the same inputs through both.
//!
//! ```
//! use edifact_rs::from_reader;
//! use std::io::Cursor;
//!
//! let input = b"UNA:;.? 'BGM;220;test?;value'";
//! let segments: Vec<_> = from_reader(Cursor::new(&input[..]))
//!     .collect::<Result<Vec<_>, _>>()
//!     .unwrap();
//! assert_eq!(segments.len(), 1);
//! assert_eq!(segments[0].tag, "BGM");
//! assert_eq!(segments[0].element_str(0), Some("220"));
//! assert_eq!(segments[0].element_str(1), Some("test;value"));
//! ```
//!
//! # Validation
//!
//! [`ValidationContext`] runs switchable envelope, syntax, charset, structure,
//! code-list and profile layers into one [`ValidationReport`]. Every [`ValidationIssue`] says which layer found it,
//! which pack produced it and — for the built-in checks — what kind of fault it
//! is ([`FindingKind`]):
//!
//! ```
//! use edifact_rs::{ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity, from_bytes};
//!
//! let pack = ProfileRulePack::new("ORDERS-DEMO")
//!     .for_message_type("ORDERS")
//!     .with_rule_fn(|segments, issues| {
//!         if !segments.iter().any(|s| s.tag == "BGM") {
//!             issues.push(
//!                 ValidationIssue::new(ValidationSeverity::Error, "ORDERS requires a BGM")
//!                     .with_rule_id("DEMO-P001"),
//!             );
//!         }
//!     });
//!
//! let segments: Vec<_> = from_bytes(b"UNH+1+ORDERS:D:96A:UN'DTM+137:1:102'UNT+3+1'")
//!     .collect::<Result<_, _>>()?;
//! let report = ValidationContext::builder().with_profile_pack(pack).build().validate(&segments);
//!
//! assert_eq!(report.filter_by_rule_prefix("DEMO-").total_issues(), 1);
//! let issue = &report.errors()[0];
//! assert_eq!(issue.layer(), Some(edifact_rs::ValidationLayer::Profile));
//! assert_eq!(issue.pack(), Some("ORDERS-DEMO"));
//! # Ok::<(), edifact_rs::EdifactError>(())
//! ```
//!
//! See the [validation](https://hupe1980.github.io/edifact-rs/docs/validation/)
//! and [profile pack](https://hupe1980.github.io/edifact-rs/docs/profile-packs/)
//! guides for the layers, group-scoped rules, and directory validation.
//!
//! # Async usage
//!
//! There is deliberately no native `async` API: parsing is CPU work over a
//! buffer, not I/O, so an async parser would add a runtime dependency and a
//! second copy of every code path to wrap work that never awaits. Read with your
//! runtime, parse synchronously — see the
//! [async integration guide](https://hupe1980.github.io/edifact-rs/docs/async-integration/)
//! for the three patterns, including `spawn_blocking` for multi-gigabyte files.
//!
//! ```rust,no_run
//! # async fn example(mut reader: impl tokio::io::AsyncReadExt + Unpin)
//! # -> Result<(), Box<dyn std::error::Error>> {
//! let mut buf = Vec::new();
//! reader.read_to_end(&mut buf).await?;
//! let segments: Vec<_> = edifact_rs::from_bytes(&buf).collect::<Result<Vec<_>, _>>()?;
//! # let _ = segments;
//! # Ok(())
//! # }
//! ```
// ── core modules ──────────────────────────────────────────────────────────────
/// EDIFACT character repertoires (`UNB` S001 DE 0001) and transcoding.
pub mod charset;
/// `CONTRL` — the ISO 9735-4 syntax and service report message.
pub mod contrl;
pub mod directory_validator;
pub(crate) mod envelope;
/// Error types and validation reporting primitives.
pub(crate) mod error;
pub mod group;
/// Core zero-copy and owned EDIFACT data model types.
pub(crate) mod model;
pub(crate) mod parser;
pub mod recover;
/// Validation report types: [`ValidationSeverity`], [`ValidationIssue`], [`ValidationReport`].
///
/// These types are also re-exported from the crate root.
pub mod report;
/// ISO 9735 service-segment definitions (`UNB`, `UNH`, `UNT`, `UNZ`, `UNG`, `UNE`, `UNS`).
pub mod service;
pub(crate) mod tokenizer;
pub(crate) mod validator;
pub(crate) mod writer;

// ── typed serialization layer ─────────────────────────────────────────────────
pub mod de;
pub(crate) mod event;
pub mod ser;

// ── flat re-exports: core ─────────────────────────────────────────────────────
pub use charset::{Charset, DecodingReader, decode_interchange, decode_reader, sniff_charset};
pub use contrl::{
    Action, ActionPolicy, Contrl, ContrlIdentifier, ContrlLocation, ContrlOptions, RejectionScope,
    ReportingLevel, SyntaxError,
};
pub use envelope::{
    FunctionalGroupEnvelope, GroupIdentifier, InterchangeEnvelope, LenientResult, MessageEnvelope,
    MessageIdentifier, ValidatedInterchange, parse_ung, parse_unh, validate_envelope,
    validate_envelope_lenient,
};
pub use error::{EdifactError, Insignificant, IoError};
pub use group::{
    Descendants, Discriminator, Entry, GroupDef, GroupDefBuilder, GroupResolution, GroupSchema,
    Placement, SegmentGroupIndexed, SegmentPlace, Unplaced, UnplacedReason, group_segments_indexed,
};
pub use model::{Components, Element, OwnedElement, OwnedSegment, Segment, Span};
pub use parser::{
    OwnedSegmentStream, Parser, ReaderConfig, from_bufread, from_bufread_with_config,
    from_reader_with_config,
};
pub use recover::{RecoveredInterchange, RecoveryConfig};
pub use report::severity_for_error;
pub use report::{
    FindingKind, GroupPath, GroupStep, ValidationIssue, ValidationReport, ValidationSeverity,
};
pub use tokenizer::{ServiceStringAdvice, Token, Tokenizer};
pub use validator::{
    CharsetValidator, EnvelopeValidator, PlacementValidator, ProfileRule, ProfileRulePack,
    SyntaxValidator, ValidationContext, ValidationContextBuilder, ValidationLayer,
    ValidationRuleContext, Validator, validate_each,
};
pub use writer::{AsDataElement, DataElement, MessageWriter, Writer};

// ── flat re-exports: serde ────────────────────────────────────────────────────

/// User-facing deserialization API.
pub use de::{
    CompositeElement, EdifactCompositeDeserialize, EdifactDeserialize, EdifactSegmentTag,
    MessageWindow, MessageWindows, OwnedMessageWindow, composite_element, contiguous_groups,
    deserialize, deserialize_each, deserialize_each_from_reader,
    deserialize_each_from_reader_with_config, deserialize_each_with_config, deserialize_messages,
    deserialize_messages_from_reader, deserialize_messages_from_reader_with_config,
    deserialize_messages_with_config, deserialize_str, find_qualified_segment, find_segment,
    find_segments, find_segments_typed, message_windows, message_windows_from_reader,
    message_windows_from_reader_with_config, message_windows_with_config,
    qualifier_matches_pattern,
};

// ── Proc-macro support ─────────────────────────────────────────────────────────

pub use directory_validator::{
    ComponentRef, DirectoryValidator, DirectoryValidatorBuilder, ElementPath, ElementRef,
    LayoutAudit, LayoutFinding, LayoutSlot, OwnedComponentRef, OwnedElementRef, OwnedSegmentDef,
    Repr, ReprKind, SegmentDefinition, SegmentLayout, Status, audit_directory,
};
#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use edifact_rs_derive::{
    EdifactCompositeDeserialize, EdifactCompositeSerialize, EdifactDeserialize, EdifactSerialize,
};
pub use event::{EdifactEvent, EventEmitter, VecEmitter, WriterEmitter};
pub use ser::{
    DecimalFloat, EdifactCompositeSerialize, EdifactSerialize, emit_sparse_segment, to_bytes,
    to_edifact_string,
};

// ── core free functions ───────────────────────────────────────────────────────

use std::io::{Read, Write};

/// Iterator returned by [`from_bytes`].
pub struct FromBytesIter<'a> {
    parser: Option<parser::Parser<'a>>,
    pending_error: Option<EdifactError>,
    /// The whole input, including any bytes beyond `max_input_bytes`.
    input: &'a [u8],
    /// Service characters in force (terminator and release character are
    /// needed to tell where an over-budget unit ends).
    ssa: tokenizer::ServiceStringAdvice,
    max_segment_bytes: usize,
    /// `Some(max)` when the input is longer than `max_input_bytes`: the parser
    /// then only ever sees `input[..max]`.
    truncated_at: Option<usize>,
    /// End of the last segment handed out, or of the prologue.
    last_end: usize,
    /// Count budgets with their running totals.
    budget: parser::Budget,
}

/// Iterator returned by [`from_reader`].
///
/// # Resource bounds
///
/// As [`from_reader`]: only `max_segment_bytes` (64 KiB) applies. Memory is
/// one segment plus an 8 KiB read buffer; time is unbounded on input that
/// never ends. Use [`from_reader_with_config`] for untrusted input.
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
            self.parser = None;
            return Some(Err(err));
        }
        let parser = self.parser.as_mut()?;
        // The parser sees only the bytes within `max_input_bytes`, so every
        // segment it yields ends inside the budget; what is left to decide
        // here is what its end of input and its errors mean.
        match parser.next() {
            None => {
                self.parser = None;
                // Bytes beyond the budget exist but were never examined: the
                // reader path would have had to consume them, so this is the
                // byte budget, whatever those bytes are (trailing whitespace
                // and stray terminators included).
                self.truncated_at
                    .map(|max| Err(parser::input_bytes_exceeded(max as u64)))
            }
            Some(Err(error)) => {
                self.parser = None;
                Some(Err(self.over_budget(error)))
            }
            Some(Ok(seg)) => {
                // Limits are checked against a segment that is actually
                // available, so an input ending exactly at the limit finishes
                // cleanly instead of being reported as a violation.
                if let Err(error) = self.budget.admit_segment(&seg.tag, seg.tag_span.start) {
                    self.parser = None;
                    return Some(Err(error));
                }
                self.budget.account(&seg.tag);
                self.last_end = seg.span.end;
                Some(Ok(seg))
            }
        }
    }
}

impl FromBytesIter<'_> {
    /// The answer for a parse `error` raised over a budget-truncated input.
    ///
    /// The reader path checks each unit — the segment after `last_end`, with
    /// its terminator — against `max_input_bytes` *before* parsing it. The
    /// slice parser ran over the truncated input instead, so an error from a
    /// unit whose terminator lies beyond the budget is reported as the budget,
    /// exactly as the reader reports it. Two exceptions keep the genuine
    /// error: the unit ends inside the budget, or the segment is detectably
    /// longer than `max_segment_bytes` within the budget (`E020` comes first in
    /// the byte stream).
    fn over_budget(&self, error: EdifactError) -> EdifactError {
        let Some(max) = self.truncated_at else {
            return error;
        };
        if matches!(error, EdifactError::SegmentTooLong { .. }) {
            // Only detectable inside the truncated input, so inside the budget.
            return error;
        }
        let from = self.last_end.min(max);
        let unit = &self.input[from..max];
        if parser::find_unescaped_term(unit, self.ssa.segment_term, self.ssa.release_char).is_some()
        {
            return error;
        }
        let start = from
            + unit
                .iter()
                .take_while(|&&b| parser::is_inter_segment_ws(b))
                .count();
        if start
            .saturating_add(self.max_segment_bytes)
            .saturating_add(1)
            <= max
        {
            // A release character in the budget's last byte escapes a byte
            // the truncated parser cannot see; the full segment is longer
            // than `max_segment_bytes`, which is what the reader reports.
            if matches!(error, EdifactError::InvalidReleaseSequence { offset } if offset + 1 == max)
            {
                return EdifactError::SegmentTooLong {
                    offset: start,
                    limit: self.max_segment_bytes,
                };
            }
            return error;
        }
        parser::input_bytes_exceeded(max as u64)
    }
}

/// Parse `input` bytes into an iterator of [`Segment`]s.
///
/// Borrows its text directly from `input`. A segment costs one allocation for
/// its element vector (at its exact length; none for a segment without
/// elements), one per value that contained a release sequence and so had to be
/// unescaped into an owned `String`, and occasionally one to grow the parser's
/// reusable element buffer. Elements that do not repeat allocate nothing for
/// repetitions. Asserted by the `allocation_budget` integration test.
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
/// Every [`ReaderConfig`] limit is enforced as a **hard cap that yields an error**,
/// never as a silent stop:
///
/// - `max_segment_bytes` — [`EdifactError::SegmentTooLong`] when a single segment
///   exceeds the threshold.
/// - `max_segments`, `max_messages`, `max_segments_per_message`,
///   `max_input_bytes` — [`EdifactError::LimitExceeded`] when the input carries
///   more than the budget, at the start of the offending segment (or at the
///   byte budget itself).
///
/// `max_input_bytes` counts every byte of `input` the parser would consume,
/// trailing whitespace and stray terminators included: bytes beyond it are
/// never examined, and their mere presence is the violation. The reader path
/// gives the same answer for the same bytes.
///
/// A budget that merely ended the iterator would be indistinguishable from a clean
/// end of input, so a caller collecting into a `Vec` would silently accept a
/// **truncated** interchange as a complete one.  Input that ends exactly at a limit
/// is not a violation and finishes normally.
///
/// Pass `ReaderConfig::default()` for the default 64 KiB per-segment limit with no
/// segment-count, message-count, or byte budget.
///
/// # Example
///
/// ```
/// use edifact_rs::{EdifactError, ReaderConfig, from_bytes_with_config};
///
/// // Exactly at the limit: fine.
/// let cfg = ReaderConfig::default().max_segments(1);
/// assert!(from_bytes_with_config(b"BGM+220'", cfg).collect::<Result<Vec<_>, _>>().is_ok());
///
/// // One segment too many: a loud error, not a quiet truncation.
/// let err = from_bytes_with_config(b"BGM+220'DTM+137'", cfg)
///     .collect::<Result<Vec<_>, _>>()
///     .unwrap_err();
/// assert!(matches!(
///     err,
///     EdifactError::LimitExceeded { limit: "max_segments", max: 1, offset: 8, .. }
/// ));
/// ```
pub fn from_bytes_with_config(input: &[u8], config: parser::ReaderConfig) -> FromBytesIter<'_> {
    // A malformed `UNA` is rejected even when the caller supplied its own
    // service characters: the input is broken either way, and silently parsing
    // past a nine-byte header nobody validated would be the worse answer.
    let discovered = tokenizer::ServiceStringAdvice::from_bytes(input);
    let resolved = match (config.service_string_advice, discovered) {
        (_, Err(error)) => Err(error),
        (Some(override_ssa), Ok(_)) => Ok(override_ssa),
        (None, Ok(ssa)) => Ok(ssa),
    };
    // Every consumed byte counts against `max_input_bytes`: the parser is given
    // only the bytes within the budget, and the iterator reports the budget
    // when it needs a byte beyond it (see `FromBytesIter::over_budget`).
    let truncated_at = config
        .max_input_bytes
        .filter(|&max| input.len() as u64 > max)
        // `max < input.len()`, so it fits in `usize`.
        .map(|max| max as usize);
    let scanned = truncated_at.map_or(input, |max| &input[..max]);
    // The prologue — byte order mark, leading whitespace, `UNA` — is one unit:
    // the reader reads all of it before the first segment.
    let start = tokenizer::prologue_len(input);
    let has_una = input.len() >= start + 9 && &input[start..start + 3] == b"UNA";
    let prologue_end = if has_una { start + 9 } else { start };
    let ssa = resolved
        .as_ref()
        .map_or_else(|_| tokenizer::ServiceStringAdvice::default(), |ssa| *ssa);
    let (parser, pending_error) = match (truncated_at, resolved) {
        (Some(max), _) if prologue_end > max => {
            // A `UNA` whose three letters are not all inside the budget cannot
            // be told from a segment by a reader, which then reports what it
            // would for a segment: an overlong tag before the budget.
            let error = if has_una
                && start + 3 > max
                && max.saturating_sub(start) > config.max_segment_bytes
            {
                EdifactError::SegmentTooLong {
                    offset: start,
                    limit: config.max_segment_bytes,
                }
            } else {
                parser::input_bytes_exceeded(max as u64)
            };
            (None, Some(error))
        }
        (_, Ok(ssa)) => {
            let t = tokenizer::Tokenizer::with_limit(scanned, ssa, config.max_segment_bytes);
            (Some(parser::Parser::new(t)), None)
        }
        (_, Err(error)) => (None, Some(error)),
    };
    FromBytesIter {
        parser,
        pending_error,
        input,
        ssa,
        max_segment_bytes: config.max_segment_bytes,
        truncated_at,
        last_end: prologue_end.min(input.len()),
        budget: parser::Budget::new(&config),
    }
}

/// Scan `input` without stopping at the first fault.
///
/// Yields every segment [`from_bytes_with_config`] would yield — the same
/// segments, with the same absolute spans — and, where the strict parser stops,
/// a [`Fault`][recover::Fault] carrying the strict error, then resumes after the
/// faulted segment's terminator. Every [`ReaderConfig`] limit in
/// `config` applies, plus a fault budget; a terminal fault ends the scan.
///
/// This is the lazy form of [`RecoveredInterchange::scan`], which also derives
/// the `UNB` addressing, the message windows and the `CONTRL` location of every
/// fault. See the [`recover`] module for the full contract.
///
/// # Example
///
/// ```
/// use edifact_rs::recover::Scanned;
/// use edifact_rs::{RecoveryConfig, scan_recovering};
///
/// let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220'\xFF'UNT+4+1'";
/// let items: Vec<Scanned<'_>> = scan_recovering(input, &RecoveryConfig::default()).collect();
///
/// let tags: Vec<_> = items.iter().map(|item| item.tag()).collect();
/// assert_eq!(tags, [Some("UNH"), Some("BGM"), None, Some("UNT")]);
/// assert_eq!(items[2].as_fault().unwrap().error().stable_code(), "E006");
/// ```
pub fn scan_recovering<'a>(
    input: &'a [u8],
    config: &RecoveryConfig,
) -> recover::RecoveringScan<'a> {
    recover::RecoveringScan::new(input, config)
}

/// Parse a reader into a lazy iterator of [`OwnedSegment`]s.
///
/// Returns a [`FromReaderIter`] that parses and yields segments on demand,
/// keeping memory bounded. `.collect::<Result<Vec<_>, _>>()` when you do want
/// them all in memory.
///
/// # Resource bounds
///
/// Only the default `max_segment_bytes` (64 KiB) applies. Memory is one
/// segment plus an 8 KiB read buffer; nothing bounds how much input is read or
/// how long that takes. For untrusted input use [`from_reader_with_config`]
/// with `max_input_bytes`.
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

/// Parse a byte slice, decoding it from the repertoire its own `UNB` declares.
///
/// [`decode_interchange`] followed by [`from_bytes`], in one call that a
/// caller cannot forget to make. Forgetting is the failure mode worth designing
/// against: a `UNOC` corpus stored as UTF-8 parses fine, so the tests pass and
/// the first *conformant* counterparty message — the one with `ü` as the single
/// byte `0xFC` — is rejected as invalid text.
///
/// Segments are owned because the decoded buffer is this function's, not the
/// caller's: an ISO 8859-1 payload has to be transcoded to exist as UTF-8 at
/// all. When the payload is already ASCII or `UNOY`, decoding borrows and copies
/// nothing, but the segments are still owned — reach for
/// [`decode_interchange`] plus [`from_bytes`] when you want to keep the
/// zero-copy path and hold the buffer yourself.
///
/// # Errors
///
/// As [`decode_interchange`], plus any parse error.
///
/// # Example
///
/// ```
/// // A conformant UNOC interchange: `Müller` is `4D FC 6C 6C 65 72`.
/// let mut raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'NAD+BY+M".to_vec();
/// raw.push(0xFC);
/// raw.extend_from_slice(b"ller'UNZ+0+IC1'");
///
/// // Parsing it directly fails — it is not UTF-8, and it never claimed to be.
/// assert!(edifact_rs::from_bytes(&raw).collect::<Result<Vec<_>, _>>().is_err());
///
/// let segments = edifact_rs::from_bytes_decoded(&raw)?;
/// assert_eq!(segments[1].element_str(1), Some("Müller"));
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub fn from_bytes_decoded(input: &[u8]) -> Result<Vec<OwnedSegment>, EdifactError> {
    from_bytes_decoded_with_config(input, ReaderConfig::default())
}

/// [`from_bytes_decoded`] with explicit [`ReaderConfig`] limits.
///
/// # Errors
///
/// As [`from_bytes_decoded`].
pub fn from_bytes_decoded_with_config(
    input: &[u8],
    config: ReaderConfig,
) -> Result<Vec<OwnedSegment>, EdifactError> {
    let decoded = charset::decode_interchange(input)?;
    from_bytes_with_config(&decoded, config)
        .map(|r| r.map(|segment| segment.into_owned()))
        .collect()
}

/// Parse a reader, decoding it from the repertoire the stream's own `UNB`
/// declares — **lazily**.
///
/// [`decode_reader`] has to read far enough to find the `UNB` before it can
/// answer, so it returns a `Result` — and a `?` on it turns a lazy pipeline
/// eager, forcing the caller to box the iterator or wrap the error in a
/// one-item chain. This does the sniff on the first `next()` instead, so the
/// signature stays a plain `Iterator` and a decode failure arrives as its first
/// item, exactly like a parse failure does.
///
/// # Resource bounds
///
/// Only the default `max_segment_bytes` (64 KiB) applies. Memory is one
/// segment, the 4 KiB sniff probe and two 8 KiB buffers; nothing bounds how
/// much input is read. For untrusted input use
/// [`from_reader_decoded_with_config`].
///
/// # Example
///
/// ```
/// let mut raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'NAD+BY+M".to_vec();
/// raw.push(0xFC);
/// raw.extend_from_slice(b"ller'UNZ+0+IC1'");
///
/// // No `?` before the loop: the pipeline stays lazy.
/// let segments: Vec<_> = edifact_rs::from_reader_decoded(std::io::Cursor::new(raw))
///     .collect::<Result<Vec<_>, _>>()?;
/// assert_eq!(segments[1].element_str(1), Some("Müller"));
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub fn from_reader_decoded<R: Read>(reader: R) -> DecodingSegmentStream<R> {
    from_reader_decoded_with_config(reader, ReaderConfig::default())
}

/// [`from_reader_decoded`] with explicit [`ReaderConfig`] limits.
///
/// # Resource bounds
///
/// Every limit in `config` applies to the **decoded** UTF-8 stream: a Latin-1
/// interchange can be up to twice as long after decoding, so `max_input_bytes`
/// admits at least half as many raw bytes. Memory is one segment of at most
/// `max_segment_bytes`, the 4 KiB sniff probe and two 8 KiB buffers.
pub fn from_reader_decoded_with_config<R: Read>(
    reader: R,
    config: ReaderConfig,
) -> DecodingSegmentStream<R> {
    DecodingSegmentStream {
        state: DecodingState::Pending(reader),
        config,
    }
}

/// Lazy iterator returned by [`from_reader_decoded`].
///
/// Sniffs the interchange's repertoire on the first `next()`, so constructing it
/// cannot fail and the caller keeps a plain `Iterator`.
///
/// # Resource bounds
///
/// As [`from_reader_decoded_with_config`]: the configured limits count decoded
/// bytes; memory is one segment plus the probe and two 8 KiB buffers.
pub struct DecodingSegmentStream<R: Read> {
    state: DecodingState<R>,
    config: ReaderConfig,
}

type DecodedReader<R> = charset::DecodingReader<std::io::Chain<std::io::Cursor<Vec<u8>>, R>>;

enum DecodingState<R: Read> {
    /// Nothing read yet; the repertoire is still unknown.
    Pending(R),
    /// Repertoire resolved; segments are streaming.
    Running(Box<parser::OwnedSegmentStream<std::io::BufReader<DecodedReader<R>>>>),
    /// Terminated, by exhaustion or by a decode failure already reported.
    Done,
}

impl<R: Read> Iterator for DecodingSegmentStream<R> {
    type Item = Result<OwnedSegment, EdifactError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.state {
                DecodingState::Done => return None,
                DecodingState::Running(stream) => return stream.next(),
                DecodingState::Pending(_) => {
                    let DecodingState::Pending(reader) =
                        std::mem::replace(&mut self.state, DecodingState::Done)
                    else {
                        unreachable!("guarded by the match arm")
                    };
                    // The sniff happens here rather than at construction, which
                    // is what keeps the signature a plain `Iterator`.
                    match charset::decode_reader(reader) {
                        Ok(decoded) => {
                            self.state = DecodingState::Running(Box::new(
                                parser::from_reader_with_config(decoded, self.config),
                            ));
                        }
                        Err(error) => return Some(Err(error)),
                    }
                }
            }
        }
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
/// Accepts segments from either parsing path: `&[OwnedSegment]` coerces to
/// `&[Segment<'_>]`.
///
/// # Errors
///
/// Returns an error if serialization fails.
///
/// # Example
///
/// ```
/// let segments: Vec<_> = edifact_rs::from_bytes(b"BGM+220+PO-1'")
///     .collect::<Result<Vec<_>, _>>()?;
/// assert_eq!(edifact_rs::segments_to_bytes(&segments)?, b"BGM+220+PO-1'".to_vec());
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
pub fn segments_to_bytes<'a, 'b, I>(segments: I) -> Result<Vec<u8>, EdifactError>
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

/// Compiles and runs every ```` ```rust ```` block in the published guides as a
/// doctest.
///
/// A rename that breaks a guide breaks the build.
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

    guide!(
        CharacterSets,
        "../../../site/content/docs/character-sets.md"
    );
    guide!(Contrl, "../../../site/content/docs/contrl.md");
    guide!(CoreConcepts, "../../../site/content/docs/core-concepts.md");
    guide!(Parsing, "../../../site/content/docs/parsing.md");
    guide!(ProfilePacks, "../../../site/content/docs/profile-packs.md");
    guide!(Validation, "../../../site/content/docs/validation.md");

    // Guides whose examples use the derive macros.
    #[cfg(feature = "derive")]
    guide!(
        AsyncIntegration,
        "../../../site/content/docs/async-integration.md"
    );
    #[cfg(feature = "derive")]
    guide!(
        ErrorReference,
        "../../../site/content/docs/error-reference.md"
    );
    #[cfg(feature = "derive")]
    guide!(
        GettingStarted,
        "../../../site/content/docs/getting-started.md"
    );
    #[cfg(feature = "derive")]
    guide!(Performance, "../../../site/content/docs/performance.md");
    #[cfg(feature = "derive")]
    guide!(Streaming, "../../../site/content/docs/streaming.md");
    #[cfg(feature = "derive")]
    guide!(TypedDerive, "../../../site/content/docs/typed-derive.md");
    #[cfg(feature = "derive")]
    guide!(Writing, "../../../site/content/docs/writing.md");

    // The diagnostics guide's examples use `miette` types.
    #[cfg(feature = "diagnostics")]
    guide!(Diagnostics, "../../../site/content/docs/diagnostics.md");

    // The README is the crate's front page on docs.rs and crates.io, and drifts
    // for exactly the same reason the guides did.
    #[cfg(feature = "derive")]
    guide!(Readme, "../../../README.md");
}
