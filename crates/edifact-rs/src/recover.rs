//! Recovering scan — read what can be read from an interchange that does not
//! parse, and say where every fault is.
//!
//! The strict parser ([`from_bytes`][crate::from_bytes]) stops at the first
//! fault, which is the right default: a truncated or corrupted interchange must
//! never pass for a complete one. But a receiver still owes the sender a
//! `CONTRL` for exactly that interchange (ISO 9735-4 §5.3), addressed to the
//! `UNB` parties and pointing at the faulty segment — and neither is available
//! once the parser has given up.
//!
//! [`RecoveredInterchange::scan`] (or the lazy [`scan_recovering`][crate::scan_recovering])
//! runs the **same** parser over the input, and where it refuses a segment
//! records a [`Fault`] and resumes after that segment's terminator:
//!
//! - every segment it yields is byte-for-byte the segment the strict parser
//!   yields for the same bytes — same tag, values and absolute spans;
//! - the first fault carries the strict parser's error;
//! - every [`ReaderConfig`] limit applies, every consumed byte counts against
//!   `max_input_bytes`, and a fault budget
//!   ([`RecoveryConfig::max_faults`], default 1 000) bounds the work done on
//!   hostile input.
//!
//! From the scan, [`RecoveredInterchange`] reads the [`InterchangeAddress`]
//! (`UNB` sender, recipient, control reference), the message and group windows
//! with their identity, and places every fault — or any other
//! [`EdifactError`] about the same input — as a
//! [`ContrlLocation`]. A recovered interchange is never a
//! [`ValidatedInterchange`][crate::ValidatedInterchange]: it exists to report
//! on input, not to process it.
//!
//! # Resynchronisation
//!
//! The segment terminator is the only boundary ISO 9735-1 defines, so after a
//! fault the scan resumes at the byte after the next segment terminator that is
//! not preceded by the release character, searching at most `max_segment_bytes`
//! bytes from the faulted segment's start. A fault is **terminal** — the scan
//! ends with it — when no such terminator can be found, and for the kinds that
//! leave nothing to resynchronise on: an invalid `UNA` (`E007`, the delimiters
//! are unknown), an unterminated final segment (`E001`), a segment longer than
//! `max_segment_bytes` (`E020`) and any exceeded limit (`E036`).
//!
//! # Example
//!
//! ```
//! use edifact_rs::{RecoveredInterchange, RecoveryConfig};
//! use edifact_rs::contrl::{ReportingLevel, SyntaxError};
//!
//! // The second message's BGM tag is broken: `BGME03` is no segment tag.
//! let input = b"UNB+UNOC:3+4012345000023:500+9900357000004:500+261001:0700+IC9'\
//!               UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E03+X'UNT+3+1'\
//!               UNH+2+UTILMD:D:11A:UN:S2.1'BGME03+X'UNT+3+2'\
//!               UNZ+2+IC9'";
//! let recovered = RecoveredInterchange::scan(input, &RecoveryConfig::default());
//!
//! // Who the CONTRL goes back to.
//! let address = recovered.address().expect("UNB is readable");
//! assert_eq!(address.sender_id(), "4012345000023");
//! assert_eq!(address.control_ref(), "IC9");
//!
//! // Which message is broken, and where.
//! let identity = recovered.messages()[1].identity().expect("UNH is readable");
//! assert_eq!(identity.message_ref(), "2");
//! let location = &recovered.fault_locations()[0];
//! assert_eq!(location.message(), Some(1));
//! assert_eq!(location.segment_position(), Some(2)); // UNH = 1
//! assert_eq!(location.element_position(), Some(1)); // the tag
//! assert_eq!(location.error(), SyntaxError::InvalidValue); // `12`
//! assert_eq!(location.level(), ReportingLevel::DataElement);
//! ```

use std::sync::Arc;

use crate::EdifactError;
use crate::contrl::{
    Anchor, ContrlLocation, Detail, SyntaxError, Target, detail_of, location, place,
};
use crate::model::{Segment, Span};
use crate::parser::{Budget, Parser, ReaderConfig, find_unescaped_term, input_bytes_exceeded};
use crate::tokenizer::{ServiceStringAdvice, Tokenizer, prologue_len};

// ── configuration ────────────────────────────────────────────────────────────

/// Limits for a recovering scan: the strict parser's [`ReaderConfig`] plus a
/// fault budget.
///
/// Every `ReaderConfig` limit applies exactly as in
/// [`from_bytes_with_config`][crate::from_bytes_with_config], with two
/// additions that keep recovery bounded:
///
/// - faults count against `max_segments` and, inside a message, against
///   `max_segments_per_message`; a faulted `UNH` counts against `max_messages`;
/// - every consumed byte — prologue, whitespace between segments, faulted
///   bytes, trailing whitespace — counts against `max_input_bytes`.
///
/// The fault budget stops input such as `''''…`, where every byte is a fault:
/// once [`fault_budget`](Self::fault_budget) faults have been recorded, the next
/// one is a terminal [`EdifactError::LimitExceeded`] with
/// `limit: "max_faults"`. The default of 1 000 exceeds anything a `CONTRL` can
/// report (ISO 9735-4 allows at most 999 segment reports per message).
///
/// # Example
///
/// ```
/// use edifact_rs::{ReaderConfig, RecoveryConfig};
///
/// let config = RecoveryConfig::new()
///     .reader(ReaderConfig::default().max_segments(10_000))
///     .max_faults(100);
/// assert_eq!(config.fault_budget(), 100);
/// assert_eq!(config.reader_config().max_segments, Some(10_000));
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RecoveryConfig {
    reader: ReaderConfig,
    max_faults: usize,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            reader: ReaderConfig::default(),
            max_faults: 1_000,
        }
    }
}

impl RecoveryConfig {
    /// The default configuration: `ReaderConfig::default()` and a budget of
    /// 1 000 faults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Use these strict-parser limits (and service string advice override).
    #[must_use]
    pub fn reader(mut self, reader: ReaderConfig) -> Self {
        self.reader = reader;
        self
    }

    /// Allow at most `limit` faults; the next one ends the scan with
    /// `LimitExceeded { limit: "max_faults", .. }`.
    #[must_use]
    pub fn max_faults(mut self, limit: usize) -> Self {
        self.max_faults = limit;
        self
    }

    /// The strict-parser limits in force.
    #[must_use]
    pub fn reader_config(&self) -> &ReaderConfig {
        &self.reader
    }

    /// The fault budget.
    #[must_use]
    pub fn fault_budget(&self) -> usize {
        self.max_faults
    }
}

// ── scan items ───────────────────────────────────────────────────────────────

/// One item of a recovering scan: a segment the strict parser accepted, or a
/// fault where it refused one.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Scanned<'a> {
    /// A segment, exactly as [`from_bytes`][crate::from_bytes] yields it
    /// (zero-copy, absolute spans).
    Segment(Segment<'a>),
    /// Bytes the parser refused.
    Fault(Fault),
}

impl<'a> Scanned<'a> {
    /// The bytes this item covers in the input.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Segment(segment) => segment.span,
            Self::Fault(fault) => fault.span,
        }
    }

    /// The segment tag: the parsed one, or a fault's [`raw_tag`](Fault::raw_tag).
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        match self {
            Self::Segment(segment) => Some(segment.tag()),
            Self::Fault(fault) => fault.raw_tag(),
        }
    }

    /// The segment, if this item is one.
    #[must_use]
    pub fn as_segment(&self) -> Option<&Segment<'a>> {
        match self {
            Self::Segment(segment) => Some(segment),
            Self::Fault(_) => None,
        }
    }

    /// The fault, if this item is one.
    #[must_use]
    pub fn as_fault(&self) -> Option<&Fault> {
        match self {
            Self::Segment(_) => None,
            Self::Fault(fault) => Some(fault),
        }
    }
}

/// A segment the strict parser refused, and why.
///
/// # Example
///
/// ```
/// use edifact_rs::scan_recovering;
/// use edifact_rs::RecoveryConfig;
///
/// let input = b"UNH+1+ORDERS:D:96A:UN'BGME03+X'UNT+3+1'";
/// let fault = scan_recovering(input, &RecoveryConfig::default())
///     .find_map(|item| item.as_fault().cloned())
///     .expect("one fault");
/// assert_eq!(fault.error().stable_code(), "E006");
/// assert_eq!(fault.ordinal(), 1);
/// assert_eq!(&input[fault.span().start..fault.span().end], b"BGME03+X'");
/// assert!(!fault.is_terminal()); // the scan went on to read UNT
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Fault {
    error: Arc<EdifactError>,
    span: Span,
    ordinal: u32,
    raw_tag: Option<String>,
    terminal: bool,
}

impl Fault {
    /// The strict parser's error for this segment; offsets in it are absolute.
    #[must_use]
    pub fn error(&self) -> &EdifactError {
        &self.error
    }

    /// The bytes discarded: from the segment's first byte through its
    /// terminator, or to the end of the input when the fault is terminal —
    /// but never past `max_input_bytes`, except for the limit fault itself.
    #[must_use]
    pub fn span(&self) -> Span {
        self.span
    }

    /// Zero-based position among all scanned items, segments and faults alike.
    #[must_use]
    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    /// The segment tag as written, when it is one to three ASCII uppercase
    /// letters before the first unreleased element separator or terminator.
    ///
    /// This is what lets a faulted `UNB`, `UNH` or `UNT` still open or close
    /// its window.
    #[must_use]
    pub fn raw_tag(&self) -> Option<&str> {
        self.raw_tag.as_deref()
    }

    /// Whether the scan ended with this fault.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }
}

// ── the scan ─────────────────────────────────────────────────────────────────

/// Lazy recovering scan; returned by [`scan_recovering`][crate::scan_recovering].
///
/// Yields every segment the strict parser accepts and a [`Fault`] for every
/// segment it refuses, in wire order. Never panics; ends after a terminal
/// fault. See the [module documentation](self).
pub struct RecoveringScan<'a> {
    input: &'a [u8],
    ssa: ServiceStringAdvice,
    config: RecoveryConfig,
    parser: Option<Parser<'a>>,
    /// Faults waiting to be yielded (an invalid `UNA`, or the terminal
    /// follow-up of a fault that could not be resynchronised).
    pending: Vec<Fault>,
    items: u64,
    faults: usize,
    /// The strict parser's count and byte budgets, with faults counted as
    /// segments.
    budget: Budget,
    /// End of the last item, or the end of the prologue.
    cursor: usize,
    done: bool,
}

impl std::fmt::Debug for RecoveringScan<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveringScan")
            .field("cursor", &self.cursor)
            .field("items", &self.items)
            .field("faults", &self.faults)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

/// Whether `error` leaves nothing to resynchronise on.
fn is_terminal_kind(error: &EdifactError) -> bool {
    matches!(
        error,
        EdifactError::UnexpectedEof { .. }
            | EdifactError::InvalidUna
            | EdifactError::SegmentTooLong { .. }
            | EdifactError::LimitExceeded { .. }
    )
}

impl<'a> RecoveringScan<'a> {
    pub(crate) fn new(input: &'a [u8], config: &RecoveryConfig) -> Self {
        let config = *config;
        // Delimiters are resolved exactly as `from_bytes_with_config` resolves
        // them, including refusing a malformed UNA even under an override.
        let discovered = ServiceStringAdvice::from_bytes(input);
        let mut scan = Self {
            input,
            ssa: config
                .reader
                .service_string_advice
                .unwrap_or_else(|| ServiceStringAdvice::from_bytes_unchecked(input)),
            config,
            parser: None,
            pending: Vec::new(),
            items: 0,
            faults: 0,
            budget: Budget::new(&config.reader),
            cursor: prologue_len(input),
            done: false,
        };
        match discovered {
            Ok(_) => {
                let tokenizer =
                    Tokenizer::with_limit(input, scan.ssa, config.reader.max_segment_bytes);
                scan.parser = Some(Parser::new(tokenizer));
            }
            Err(error) => {
                let start = prologue_len(input);
                let end = start.saturating_add(9).min(input.len());
                scan.pending.push(Fault {
                    error: Arc::new(error),
                    span: Span::new(start, end),
                    ordinal: 0,
                    raw_tag: Some("UNA".to_owned()),
                    terminal: true,
                });
            }
        }
        scan
    }

    /// The service characters the scan reads the input with.
    pub(crate) fn service_string_advice(&self) -> ServiceStringAdvice {
        self.ssa
    }

    fn ordinal(&self) -> u32 {
        // Items are bounded by the input length (every item consumes at least
        // one byte), so a `u32` overflow needs a > 4 GiB input of one-byte
        // faults *and* a fault budget above 4 billion; saturate rather than wrap.
        u32::try_from(self.items).unwrap_or(u32::MAX)
    }

    /// The limit an item tagged `tag`, from byte `start` to byte `end`, would
    /// exceed, if any.
    ///
    /// Same order as the strict parser: the byte budget for the item's unit
    /// first (the strict path checks it before parsing), then the count
    /// budgets, then — for a fault — the fault budget.
    fn exceeded(
        &self,
        tag: Option<&str>,
        start: usize,
        end: usize,
        fault: bool,
    ) -> Option<EdifactError> {
        if let Err(error) = self.budget.admit_bytes(end as u64) {
            return Some(error);
        }
        if let Err(error) = self.budget.admit_segment(tag.unwrap_or(""), start) {
            return Some(error);
        }
        if fault && self.faults >= self.config.max_faults {
            return Some(EdifactError::LimitExceeded {
                limit: "max_faults",
                max: self.config.max_faults as u64,
                offset: start,
            });
        }
        None
    }

    /// Count an item against the budgets, mirroring the strict iterator.
    fn account(&mut self, tag: Option<&str>, end: usize, fault: bool) {
        self.items += 1;
        self.cursor = self.cursor.max(end);
        if fault {
            self.faults += 1;
        }
        self.budget.account(tag.unwrap_or(""));
    }

    /// Emit `fault` (or the limit it would exceed), stopping the scan when it
    /// is terminal.
    fn emit_fault(&mut self, mut fault: Fault) -> Scanned<'a> {
        let already_a_limit = matches!(*fault.error, EdifactError::LimitExceeded { .. });
        // An oversized segment detected inside the byte budget is what the
        // strict parser reports, even though the fault runs on past the budget
        // (it reaches the end of the input): the strict parser reads only the
        // first `max_segment_bytes + 1` bytes of it.
        let end = match *fault.error {
            EdifactError::SegmentTooLong { offset, limit } => fault
                .span
                .end
                .min(offset.saturating_add(limit).saturating_add(1)),
            _ => fault.span.end,
        };
        match self
            .exceeded(fault.raw_tag(), fault.span.start, end, true)
            .filter(|_| !already_a_limit)
        {
            Some(limit) => {
                fault.error = Arc::new(limit);
                fault.terminal = true;
            }
            // Kept as `E020`: it discards nothing past the byte budget.
            None if end < fault.span.end => {
                if let Some(max) = self.budget.max_input_bytes() {
                    let max = usize::try_from(max).unwrap_or(usize::MAX);
                    fault.span = Span::new(fault.span.start, fault.span.end.min(max));
                }
            }
            None => {}
        }
        fault.ordinal = self.ordinal();
        self.account(fault.raw_tag.as_deref(), fault.span.end, true);
        if fault.terminal {
            self.parser = None;
            self.pending.clear();
            self.done = true;
        }
        Scanned::Fault(fault)
    }

    /// Record the strict error for the segment starting at `start` and decide
    /// where to resume.
    fn fault_at(&mut self, start: usize, error: EdifactError) -> Scanned<'a> {
        let input = self.input;
        let raw_tag = raw_tag(&input[start..], &self.ssa);
        if is_terminal_kind(&error) {
            return self.emit_fault(Fault {
                error: Arc::new(error),
                span: Span::new(start, input.len()),
                ordinal: 0,
                raw_tag,
                terminal: true,
            });
        }

        // The terminator may sit at index `max_segment_bytes` (the limit
        // excludes the terminator itself), so the window is one byte longer.
        let max = self.config.reader.max_segment_bytes;
        let window_end = start.saturating_add(max).saturating_add(1).min(input.len());
        match find_unescaped_term(
            &input[start..window_end],
            self.ssa.segment_term,
            self.ssa.release_char,
        ) {
            Some(term) => {
                let resume = start + term + 1;
                self.parser = Some(Parser::new(Tokenizer::resume_at(
                    input, self.ssa, max, resume,
                )));
                self.emit_fault(Fault {
                    error: Arc::new(error),
                    span: Span::new(start, resume),
                    ordinal: 0,
                    raw_tag,
                    terminal: false,
                })
            }
            // The input ends inside this segment: nothing to resume at.
            None if window_end == input.len() => self.emit_fault(Fault {
                error: Arc::new(error),
                span: Span::new(start, input.len()),
                ordinal: 0,
                raw_tag,
                terminal: true,
            }),
            // No terminator within the segment budget: report the fault, then
            // end the scan with the overlong segment.
            None => {
                self.parser = None;
                self.pending.push(Fault {
                    error: Arc::new(EdifactError::SegmentTooLong {
                        offset: start,
                        limit: max,
                    }),
                    span: Span::new(start, input.len()),
                    ordinal: 0,
                    raw_tag: raw_tag.clone(),
                    terminal: true,
                });
                self.emit_fault(Fault {
                    error: Arc::new(error),
                    span: Span::new(start, window_end),
                    ordinal: 0,
                    raw_tag,
                    terminal: false,
                })
            }
        }
    }
}

impl<'a> Iterator for RecoveringScan<'a> {
    type Item = Scanned<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if !self.pending.is_empty() {
            let fault = self.pending.remove(0);
            return Some(self.emit_fault(fault));
        }
        let Some(parser) = self.parser.as_mut() else {
            self.done = true;
            return None;
        };
        let before = parser.byte_position();
        match parser.next() {
            None => {
                self.parser = None;
                self.done = true;
                // Trailing whitespace is consumed input too.
                let end = self.input.len();
                if let Some(max) = self.config.reader.max_input_bytes {
                    if end as u64 > max {
                        let start = self.cursor.min(end);
                        return Some(self.emit_fault(Fault {
                            error: Arc::new(input_bytes_exceeded(max)),
                            span: Span::new(start, end),
                            ordinal: 0,
                            raw_tag: None,
                            terminal: true,
                        }));
                    }
                }
                None
            }
            Some(Ok(segment)) => {
                if let Some(limit) = self.exceeded(
                    Some(segment.tag()),
                    segment.tag_span.start,
                    segment.span.end,
                    false,
                ) {
                    let tag = Some(segment.tag().to_owned()).filter(|t| t.len() <= 3);
                    return Some(self.emit_fault(Fault {
                        error: Arc::new(limit),
                        span: Span::new(segment.span.start, self.input.len()),
                        ordinal: 0,
                        raw_tag: tag,
                        terminal: true,
                    }));
                }
                self.account(Some(segment.tag()), segment.span.end, false);
                Some(Scanned::Segment(segment))
            }
            Some(Err(error)) => {
                let start = skip_whitespace(self.input, before);
                Some(self.fault_at(start, error))
            }
        }
    }
}

impl std::iter::FusedIterator for RecoveringScan<'_> {}

/// First byte at or after `from` that is not inter-segment whitespace — the
/// same set the tokenizer skips.
fn skip_whitespace(input: &[u8], from: usize) -> usize {
    let mut pos = from.min(input.len());
    while pos < input.len() && matches!(input[pos], b' ' | b'\t' | b'\r' | b'\n') {
        pos += 1;
    }
    pos
}

/// The tag of a raw segment: 1–3 ASCII uppercase letters before the first
/// element separator or terminator.
fn raw_tag(segment: &[u8], ssa: &ServiceStringAdvice) -> Option<String> {
    let end = segment
        .iter()
        .position(|&b| b == ssa.element_sep || b == ssa.segment_term)
        .unwrap_or(segment.len());
    let tag = &segment[..end];
    (!tag.is_empty() && tag.len() <= 3 && tag.iter().all(u8::is_ascii_uppercase))
        .then(|| String::from_utf8_lossy(tag).into_owned())
}

/// Component `component` of data element `element` (both zero-based, the tag
/// not counted) of a raw segment, release characters resolved.
///
/// The first occurrence of a repeating element is read. `None` when the value
/// is absent, empty, or not valid UTF-8 — an unreadable value is never guessed.
fn raw_component(
    segment: &[u8],
    ssa: &ServiceStringAdvice,
    element: usize,
    component: usize,
) -> Option<String> {
    let repetition = ssa.is_repetition_active().then_some(ssa.repetition_sep);
    // Position 0 is the tag; data element `element` is position `element + 1`.
    let wanted = element + 1;
    let mut position = 0usize;
    let mut comp = 0usize;
    let mut first_occurrence = true;
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < segment.len() {
        let b = segment[i];
        let collecting = position == wanted && comp == component && first_occurrence;
        if b == ssa.release_char {
            if let Some(&escaped) = segment.get(i + 1) {
                if collecting {
                    out.push(escaped);
                }
            }
            i += 2;
            continue;
        }
        if b == ssa.segment_term {
            break;
        }
        if b == ssa.element_sep {
            if position == wanted {
                break;
            }
            position += 1;
            comp = 0;
            first_occurrence = true;
        } else if b == ssa.component_sep {
            comp += 1;
        } else if Some(b) == repetition {
            first_occurrence = false;
        } else if collecting {
            out.push(b);
        }
        i += 1;
    }
    String::from_utf8(out).ok().filter(|v| !v.is_empty())
}

// ── the recovered interchange ────────────────────────────────────────────────

/// Who a `CONTRL` for a recovered interchange goes back to: the subject's
/// `UNB` addressing.
///
/// Read from the first `UNB` of the scan, parsed or faulted, release characters
/// resolved. Present only when the sender (DE 0004), recipient (DE 0010) and
/// control reference (DE 0020) are all readable: without them no `CONTRL` can be
/// addressed, and a plausible but invented header is worse than none.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InterchangeAddress {
    syntax_identifier: String,
    syntax_version: String,
    sender_id: String,
    sender_qualifier: String,
    recipient_id: String,
    recipient_qualifier: String,
    control_ref: String,
}

impl InterchangeAddress {
    /// `UNB` S001 DE 0001, e.g. `UNOC`; empty when unreadable.
    #[must_use]
    pub fn syntax_identifier(&self) -> &str {
        &self.syntax_identifier
    }

    /// `UNB` S001 DE 0002, e.g. `3`; empty when unreadable.
    #[must_use]
    pub fn syntax_version(&self) -> &str {
        &self.syntax_version
    }

    /// `UNB` S002 DE 0004 — the interchange sender.
    #[must_use]
    pub fn sender_id(&self) -> &str {
        &self.sender_id
    }

    /// `UNB` S002 DE 0007 — the sender's code qualifier; empty when absent.
    #[must_use]
    pub fn sender_qualifier(&self) -> &str {
        &self.sender_qualifier
    }

    /// `UNB` S003 DE 0010 — the interchange recipient.
    #[must_use]
    pub fn recipient_id(&self) -> &str {
        &self.recipient_id
    }

    /// `UNB` S003 DE 0007 — the recipient's code qualifier; empty when absent.
    #[must_use]
    pub fn recipient_qualifier(&self) -> &str {
        &self.recipient_qualifier
    }

    /// `UNB` DE 0020 — the interchange control reference.
    #[must_use]
    pub fn control_ref(&self) -> &str {
        &self.control_ref
    }
}

/// The identity of a subject message — what a `UCM` names it by.
///
/// Read from the message's `UNH`, parsed or faulted, release characters
/// resolved. Owned (unlike [`MessageIdentifier`][crate::MessageIdentifier])
/// because a faulted `UNH` has no parsed values to borrow.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageIdentity {
    message_ref: String,
    message_type: String,
    version: String,
    release: String,
    controlling_agency: String,
    association_code: String,
}

impl MessageIdentity {
    /// `UNH` DE 0062 — the message reference number.
    #[must_use]
    pub fn message_ref(&self) -> &str {
        &self.message_ref
    }

    /// `UNH` S009 DE 0065 — the message type.
    #[must_use]
    pub fn message_type(&self) -> &str {
        &self.message_type
    }

    /// `UNH` S009 DE 0052 — the version; empty when absent.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// `UNH` S009 DE 0054 — the release; empty when absent.
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }

    /// `UNH` S009 DE 0051 — the controlling agency; empty when absent.
    #[must_use]
    pub fn controlling_agency(&self) -> &str {
        &self.controlling_agency
    }

    /// `UNH` S009 DE 0057 — the association assigned code; empty when absent.
    #[must_use]
    pub fn association_code(&self) -> &str {
        &self.association_code
    }
}

/// One message (`UNH`…`UNT`) of a recovered interchange.
///
/// Opens at an item tagged `UNH` (parsed or faulted) and closes at the next
/// `UNT`. A message whose trailer never arrives closes at the next `UNH`, `UNG`,
/// `UNE` or `UNZ` with [`last`](Self::last) `None` — the missing trailer is
/// never invented.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MessageWindow {
    first: u32,
    last: Option<u32>,
    end: u32,
    group: Option<usize>,
    identity: Option<MessageIdentity>,
}

impl MessageWindow {
    /// Ordinal of the `UNH` item.
    #[must_use]
    pub fn first(&self) -> u32 {
        self.first
    }

    /// Ordinal of the `UNT` item; `None` when the message was never closed.
    #[must_use]
    pub fn last(&self) -> Option<u32> {
        self.last
    }

    /// Index of the group the message sits in, if the interchange is grouped.
    #[must_use]
    pub fn group(&self) -> Option<usize> {
        self.group
    }

    /// DE 0062 and S009 of the message; `None` when DE 0062 or DE 0065 is
    /// unreadable.
    #[must_use]
    pub fn identity(&self) -> Option<&MessageIdentity> {
        self.identity.as_ref()
    }
}

/// One group (`UNG`…`UNE`) of a recovered interchange.
///
/// Opens at an item tagged `UNG` and closes at the next `UNE`, or with
/// [`last`](Self::last) `None` at the next `UNG` or `UNZ`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GroupWindow {
    first: u32,
    last: Option<u32>,
    end: u32,
    group_ref: Option<String>,
}

impl GroupWindow {
    /// Ordinal of the `UNG` item.
    #[must_use]
    pub fn first(&self) -> u32 {
        self.first
    }

    /// Ordinal of the `UNE` item; `None` when the group was never closed.
    #[must_use]
    pub fn last(&self) -> Option<u32> {
        self.last
    }

    /// `UNG` DE 0048 — the group reference number, when readable.
    #[must_use]
    pub fn group_ref(&self) -> Option<&str> {
        self.group_ref.as_deref()
    }
}

/// An interchange read by the recovering scan: every segment that parses, a
/// [`Fault`] for every one that does not, and the structure around them.
///
/// Answers what a `CONTRL` for a broken interchange needs: the
/// [`address`](Self::address), the [`messages`](Self::messages) with their
/// identity, and a [`ContrlLocation`] for every fault
/// ([`fault_locations`](Self::fault_locations)) or for any other error about
/// the same input ([`locate`](Self::locate)). See the
/// [module documentation](self) for an example.
///
/// Memory is bounded by the limits: at most `max_segments + 1` items when that
/// limit is set, at most `max_faults + 1` faults.
///
/// The input must be the bytes the interchange was scanned from; locations are
/// counts (segment, element, component), so transcoding a non-UTF-8 repertoire
/// first with [`decode_interchange`][crate::decode_interchange] does not change
/// them.
#[derive(Debug, Clone)]
pub struct RecoveredInterchange<'a> {
    input: &'a [u8],
    ssa: ServiceStringAdvice,
    items: Vec<Scanned<'a>>,
    complete: bool,
    address: Option<InterchangeAddress>,
    messages: Vec<MessageWindow>,
    groups: Vec<GroupWindow>,
}

impl<'a> RecoveredInterchange<'a> {
    /// Scan `input` and derive its structure.
    ///
    /// Never fails: a fault is data, not an error. Check
    /// [`is_complete`](Self::is_complete) to learn whether the scan reached the
    /// end of the input.
    #[must_use]
    pub fn scan(input: &'a [u8], config: &RecoveryConfig) -> Self {
        let scan = RecoveringScan::new(input, config);
        let ssa = scan.service_string_advice();
        let items: Vec<Scanned<'a>> = scan.collect();
        let complete = !items
            .iter()
            .any(|item| item.as_fault().is_some_and(Fault::is_terminal));
        let mut recovered = Self {
            input,
            ssa,
            items,
            complete,
            address: None,
            messages: Vec::new(),
            groups: Vec::new(),
        };
        recovered.address = recovered.read_address();
        recovered.build_windows();
        recovered
    }

    /// Every scanned item, in wire order.
    #[must_use]
    pub fn items(&self) -> &[Scanned<'a>] {
        &self.items
    }

    /// The faults, in wire order.
    pub fn faults(&self) -> impl Iterator<Item = &Fault> {
        self.items.iter().filter_map(Scanned::as_fault)
    }

    /// Whether the scan reached the end of the input (no terminal fault).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// The service characters the input was read with.
    #[must_use]
    pub fn service_string_advice(&self) -> ServiceStringAdvice {
        self.ssa
    }

    /// The `UNB` addressing, when sender, recipient and control reference are
    /// all readable.
    #[must_use]
    pub fn address(&self) -> Option<&InterchangeAddress> {
        self.address.as_ref()
    }

    /// The message windows, in document order.
    ///
    /// The index into this slice is what
    /// [`ContrlLocation::message`][crate::contrl::ContrlLocation::message] and
    /// the `message` argument of [`locate`](Self::locate) refer to.
    #[must_use]
    pub fn messages(&self) -> &[MessageWindow] {
        &self.messages
    }

    /// The group windows, in document order.
    #[must_use]
    pub fn groups(&self) -> &[GroupWindow] {
        &self.groups
    }

    /// Place `error` — a fault of this scan, or any other error about the same
    /// input — as a `CONTRL` location.
    ///
    /// - An error equal to one of this scan's faults is placed on that fault's
    ///   segment.
    /// - An error with a byte offset or span is placed on the item containing
    ///   it; DE 0098 / DE 0104 of a lexical error are counted from the raw
    ///   bytes.
    /// - An error with neither (`MissingRequiredElement`,
    ///   `MissingRequiredComponent`, `InvalidFieldValue`, `MissingSegment`) is
    ///   placed only with `message`, the index of the message it was raised for:
    ///   on the first segment of that message with the error's tag, marked
    ///   [not exact](ContrlLocation::is_exact). Without `message` it lands on the
    ///   interchange.
    ///
    /// Returns `None` for an error that is not a fault of the subject input
    /// (writer, serializer and definition errors such as `E009`, `E010`, `E024`
    /// …).
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{EdifactError, RecoveredInterchange, RecoveryConfig};
    /// use edifact_rs::contrl::ReportingLevel;
    ///
    /// let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
    ///               UNH+1+ORDERS:D:96A:UN'BGM+220'DTM+137:20260101:102'UNT+4+1'UNZ+1+IC1'";
    /// let recovered = RecoveredInterchange::scan(input, &RecoveryConfig::default());
    ///
    /// // A typed layer found BGM element 1 missing in message 0.
    /// let error = EdifactError::MissingRequiredElement { tag: "BGM".into(), element_index: 1 };
    /// let location = recovered.locate(&error, Some(0)).expect("a subject fault");
    /// assert_eq!(location.level(), ReportingLevel::DataElement);
    /// assert_eq!(location.segment_position(), Some(2));
    /// assert_eq!(location.element_position(), Some(3));
    /// assert_eq!(location.error().code(), "13");
    /// assert!(!location.is_exact());
    /// ```
    #[must_use]
    pub fn locate(&self, error: &EdifactError, message: Option<usize>) -> Option<ContrlLocation> {
        let detail = detail_of(error)?;
        let code = SyntaxError::for_error(error);
        let stable = Some(error.stable_code());
        // One of our own faults: its segment is the anchor.
        if let Some(index) = self
            .items
            .iter()
            .position(|item| item.as_fault().is_some_and(|f| f.error() == error))
        {
            return Some(place(self.fault_target(index, &detail), code, stable));
        }
        let target = self.target(&detail, message);
        Some(place(target, code, stable))
    }

    /// One location per fault, in wire order.
    #[must_use]
    pub fn fault_locations(&self) -> Vec<ContrlLocation> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let fault = item.as_fault()?;
                let detail = detail_of(fault.error())?;
                Some(place(
                    self.fault_target(index, &detail),
                    SyntaxError::for_error(fault.error()),
                    Some(fault.error().stable_code()),
                ))
            })
            .collect()
    }

    // ── internals ────────────────────────────────────────────────────────

    /// The target for a fault at item `index`.
    fn fault_target<'s>(&'s self, index: usize, detail: &Detail<'s>) -> Target<'s> {
        match detail.anchor {
            // Offset-bearing errors are placed by their offset, which also
            // gives DE 0098 / DE 0104.
            Anchor::Offset { .. } | Anchor::Span(_) => self.target(detail, None),
            // An invalid UNA, a limit, an unplaced tag error: the fault's own
            // segment, when it has one.
            Anchor::Service(_) | Anchor::Interchange => self.target(detail, None),
            _ => self.item_target(index).unwrap_or(Target::interchange(true)),
        }
    }

    /// Resolve `detail` to a target, using `message` for span-less errors.
    fn target<'s>(&'s self, detail: &Detail<'s>, message: Option<usize>) -> Target<'s> {
        let message = message.filter(|m| *m < self.messages.len());
        match detail.anchor {
            Anchor::Offset { offset, element } => {
                let Some(index) = self.item_at(offset) else {
                    return Target::interchange(true);
                };
                let Some(target) = self.item_target(index) else {
                    return Target::interchange(true);
                };
                if !element {
                    return target;
                }
                let span = self.items[index].span();
                let (element, component) = location::element_position(
                    &self.input[span.start..span.end],
                    offset.saturating_sub(span.start),
                    &self.ssa,
                );
                Target {
                    element_position: element,
                    component_position: component,
                    ..target
                }
            }
            Anchor::Span(span) => self
                .item_at(span.start)
                .and_then(|index| self.item_target(index))
                .unwrap_or(Target::interchange(true))
                .with_indices(detail.element_index, detail.component_index),
            Anchor::Tag(tag) => {
                let found = if matches!(tag, "UNB" | "UNZ" | "UNG" | "UNE") {
                    self.first_with_tag(tag, 0..self.items.len())
                } else {
                    message.and_then(|m| {
                        let window = &self.messages[m];
                        self.first_with_tag(tag, window.first as usize..window.end as usize + 1)
                    })
                };
                let target = match (found, message) {
                    (Some(index), _) => self.item_target(index),
                    // The tag does not occur: the message as a whole.
                    (None, Some(m)) => Some(Target {
                        tag: Some(tag).filter(|t| matches!(*t, "UNH" | "UNT")),
                        message: Some((m, None)),
                        group: self.messages[m].group,
                        ..Target::interchange(false)
                    }),
                    (None, None) => None,
                };
                Target {
                    exact: false,
                    ..target.unwrap_or(Target::interchange(false))
                }
                .with_indices(detail.element_index, detail.component_index)
            }
            Anchor::Missing(tag) => match message {
                Some(m) => Target {
                    tag: Some(tag).filter(|t| matches!(*t, "UNH" | "UNT")),
                    message: Some((m, None)),
                    group: self.messages[m].group,
                    ..Target::interchange(false)
                },
                None => Target {
                    tag: Some(tag).filter(|t| matches!(*t, "UNB" | "UNZ")),
                    ..Target::interchange(false)
                },
            },
            Anchor::Service(tag) => self
                .first_with_tag(tag, 0..self.items.len())
                .and_then(|index| self.item_target(index))
                .unwrap_or(Target {
                    tag: Some(tag),
                    ..Target::interchange(true)
                })
                .with_indices(detail.element_index, detail.component_index),
            Anchor::Interchange => Target::interchange(true),
            Anchor::Unplaced => Target::interchange(false),
        }
    }

    /// Index of the item containing byte `offset`: the last one starting at or
    /// before it.
    fn item_at(&self, offset: usize) -> Option<usize> {
        match self
            .items
            .binary_search_by(|item| item.span().start.cmp(&offset))
        {
            Ok(exact) => Some(exact),
            Err(0) => None,
            Err(next) => Some(next - 1),
        }
    }

    /// First item in `range` whose tag is `tag`.
    fn first_with_tag(&self, tag: &str, range: std::ops::Range<usize>) -> Option<usize> {
        let end = range.end.min(self.items.len());
        (range.start.min(end)..end).find(|&i| self.items[i].tag() == Some(tag))
    }

    /// The structure around item `index`.
    fn item_target(&self, index: usize) -> Option<Target<'_>> {
        let item = self.items.get(index)?;
        let ordinal = u32::try_from(index).ok()?;
        let message = self
            .messages
            .iter()
            .position(|w| (w.first..=w.end).contains(&ordinal))
            .map(|m| {
                let position = u32::try_from(index - self.messages[m].first as usize + 1).ok();
                (m, position)
            });
        let group = match message {
            Some((m, _)) => self.messages[m].group,
            None => self
                .groups
                .iter()
                .position(|g| (g.first..=g.end).contains(&ordinal)),
        };
        // A message position that does not fit DE 0096 cannot be reported on a
        // UCS; the message as a whole still can.
        Some(Target {
            tag: item.tag(),
            message,
            group,
            ..Target::interchange(true)
        })
    }

    /// The `UNB` addressing from the first `UNB` item.
    fn read_address(&self) -> Option<InterchangeAddress> {
        let item = self.items.iter().find(|item| item.tag() == Some("UNB"))?;
        let get = |element: usize, component: usize| -> Option<String> {
            match item {
                Scanned::Segment(segment) => segment
                    .component_str(element, component)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned),
                Scanned::Fault(fault) => raw_component(
                    &self.input[fault.span.start..fault.span.end],
                    &self.ssa,
                    element,
                    component,
                ),
            }
        };
        Some(InterchangeAddress {
            sender_id: get(1, 0)?,
            recipient_id: get(2, 0)?,
            control_ref: get(4, 0)?,
            syntax_identifier: get(0, 0).unwrap_or_default(),
            syntax_version: get(0, 1).unwrap_or_default(),
            sender_qualifier: get(1, 1).unwrap_or_default(),
            recipient_qualifier: get(2, 1).unwrap_or_default(),
        })
    }

    /// `UNG` S006 DE 0040 and DE 0007, S007 DE 0044 and DE 0007 of group
    /// `group`, each empty when unreadable — what a `UCF` copies.
    pub(crate) fn group_parties(&self, group: usize) -> [String; 4] {
        let Some(window) = self.groups.get(group) else {
            return Default::default();
        };
        let index = window.first as usize;
        let get = |element, component| self.value(index, element, component).unwrap_or_default();
        [get(1, 0), get(1, 1), get(2, 0), get(2, 1)]
    }

    /// Read component `component` of element `element` of item `index`.
    fn value(&self, index: usize, element: usize, component: usize) -> Option<String> {
        match &self.items[index] {
            Scanned::Segment(segment) => segment
                .component_str(element, component)
                .filter(|v| !v.is_empty())
                .map(str::to_owned),
            Scanned::Fault(fault) => raw_component(
                &self.input[fault.span.start..fault.span.end],
                &self.ssa,
                element,
                component,
            ),
        }
    }

    /// The identity of the message whose `UNH` is item `index`.
    fn identity(&self, index: usize) -> Option<MessageIdentity> {
        let get = |element, component| self.value(index, element, component);
        Some(MessageIdentity {
            message_ref: get(0, 0)?,
            message_type: get(1, 0)?,
            version: get(1, 1).unwrap_or_default(),
            release: get(1, 2).unwrap_or_default(),
            controlling_agency: get(1, 3).unwrap_or_default(),
            association_code: get(1, 4).unwrap_or_default(),
        })
    }

    /// One pass over the items: open on `UNH`/`UNG`, close on `UNT`/`UNE`,
    /// close implicitly on the next opener or `UNZ`.
    fn build_windows(&mut self) {
        let mut messages: Vec<MessageWindow> = Vec::new();
        let mut groups: Vec<GroupWindow> = Vec::new();
        let mut open_message: Option<usize> = None;
        let mut open_group: Option<usize> = None;

        for index in 0..self.items.len() {
            let Ok(ordinal) = u32::try_from(index) else {
                break;
            };
            let tag = self.items[index].tag().map(str::to_owned);
            // Every item inside an open window extends it.
            if let Some(m) = open_message {
                messages[m].end = ordinal;
            }
            if let Some(g) = open_group {
                groups[g].end = ordinal;
            }
            match tag.as_deref() {
                Some("UNH") => {
                    if let Some(m) = open_message.take() {
                        messages[m].end = ordinal - 1;
                    }
                    open_message = Some(messages.len());
                    messages.push(MessageWindow {
                        first: ordinal,
                        last: None,
                        end: ordinal,
                        group: open_group,
                        identity: self.identity(index),
                    });
                }
                Some("UNT") => {
                    if let Some(m) = open_message.take() {
                        messages[m].last = Some(ordinal);
                    }
                }
                Some("UNG") => {
                    if let Some(m) = open_message.take() {
                        messages[m].end = ordinal - 1;
                    }
                    if let Some(g) = open_group.take() {
                        groups[g].end = ordinal - 1;
                    }
                    open_group = Some(groups.len());
                    groups.push(GroupWindow {
                        first: ordinal,
                        last: None,
                        end: ordinal,
                        group_ref: self.value(index, 4, 0),
                    });
                }
                Some("UNE") => {
                    if let Some(m) = open_message.take() {
                        messages[m].end = ordinal - 1;
                    }
                    if let Some(g) = open_group.take() {
                        groups[g].last = Some(ordinal);
                    }
                }
                Some("UNZ") => {
                    if let Some(m) = open_message.take() {
                        messages[m].end = ordinal - 1;
                    }
                    if let Some(g) = open_group.take() {
                        groups[g].end = ordinal - 1;
                    }
                }
                _ => {}
            }
        }
        self.messages = messages;
        self.groups = groups;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_components_resolve_release_characters() {
        let ssa = ServiceStringAdvice::default();
        let unb = b"UNB+UNOC:3+SEND?+ER:14+RECV+260101:0900+IC?'1'";
        assert_eq!(raw_component(unb, &ssa, 1, 0).as_deref(), Some("SEND+ER"));
        assert_eq!(raw_component(unb, &ssa, 1, 1).as_deref(), Some("14"));
        assert_eq!(raw_component(unb, &ssa, 4, 0).as_deref(), Some("IC'1"));
        assert_eq!(raw_component(unb, &ssa, 2, 1), None);
        assert_eq!(raw_component(unb, &ssa, 9, 0), None);
    }

    #[test]
    fn raw_tags_are_one_to_three_uppercase_letters() {
        let ssa = ServiceStringAdvice::default();
        assert_eq!(raw_tag(b"UNH+1'", &ssa).as_deref(), Some("UNH"));
        assert_eq!(raw_tag(b"BGME03+X'", &ssa), None);
        assert_eq!(raw_tag(b"''", &ssa), None);
        assert_eq!(raw_tag(b"bgm+1'", &ssa), None);
    }
}
