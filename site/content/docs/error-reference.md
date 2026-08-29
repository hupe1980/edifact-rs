+++
title = "Error Reference"
description = "Every EdifactError variant with its stable code E001-E053, the fields it carries, when it fires, and how to fix it."
weight = 110
+++

All errors returned by `edifact-rs` are variants of `EdifactError`. Every variant
carries a stable, semver-protected code (`E001`–`E053`) accessible via
`err.stable_code()`. The enum is marked `#[non_exhaustive]` so future variants can
be added without breaking existing match arms.

## Where an error points

Two kinds of positional data appear, and the distinction is deliberate:

- **`offset: usize`** — a single byte position, carried by the *lexical* variants
  where the fault is a point in the stream and no end position exists.
- **`span: Span`** — a half-open byte range, carried by every variant raised
  while validating an already-parsed segment. A `ValidationIssue` built from one
  of these keeps the full range, so `miette` and LSP tooling underline the
  offending region instead of placing a zero-width caret.

`ValidationIssue` itself has a single positional field, `span`; read
`issue.span.map(|s| s.start)` (or `issue.start_offset()`) when only the start is
needed.

---

## Quick lookup table

| Code | Variant | Source | Span field |
|---|---|---|---|
| E001 | `UnexpectedEof` | Parser | `offset` |
| E002 | `InvalidDelimiter` | Parser | `offset` |
| E003 | `InvalidText` | Parser | `offset` |
| E004 | `MessageCountMismatch` | Envelope validator | — |
| E005 | `SegmentCountMismatch` | Envelope validator | — |
| E006 | `InvalidSegmentTag` | Parser | — |
| E007 | `InvalidUna` | Parser | — |
| E008 | `MissingRequiredElement` | Deserializer | — |
| E009 | `InvalidUtf8` | Writer | — |
| E010 | `Io` | Reader / Writer | — |
| E011 | `InvalidSegmentForMessage` | Directory validator | `span` |
| E012 | `InvalidElementCount` | Directory validator | `span` |
| E013 | `InvalidComponentCount` | Directory validator | `span` |
| E014 | `InvalidCodeValue` | Directory validator | `span` |
| E015 | `MissingSegment` | Directory validator | — |
| E016 | `QualifierMismatch` | Typed deserializer | `span` |
| E017 | `ConditionalRequirementNotMet` | Profile validator | `span` |
| E019 | `InvalidReleaseSequence` | Parser | `offset` |
| E020 | `SegmentTooLong` | Reader parser | `offset` |
| E021 | `MissingRequiredComponent` | Deserializer | — |
| E023 | `InterchangeTooLarge` | Envelope builder | — |
| E024 | `InvalidEventSequence` | Event emitter | — |
| E025 | `InvalidElementPosition` | Directory builder | — |
| E026 | `IncompatibleReleaseScopes` | Profile pack composer | — |
| E027 | `InvalidFieldValue` | Typed deserializer | — |
| E028 | `UnexpectedDataToken` | Parser | `offset` |
| E030 | `ValidationErrors` | Profile / directory validator | — |
| E031 | `UnrecognisedSyntaxIdentifier` | Envelope validator | — |
| E032 | `DuplicateReference` | Envelope validator | `span` |
| E033 | `UnknownDataElement` | Code-addressed access | — |
| E034 | `AmbiguousDataElement` | Code-addressed access | — |
| E035 | `SegmentLayoutMismatch` | Code-addressed access | — |
| E036 | `LimitExceeded` | Parser (`ReaderConfig` budgets) | — |
| E037 | `RepetitionSeparatorNotDeclared` | Writer | — |
| E038 | `CharacterNotInRepertoire` | Writer / `Charset::encode` | — |
| E039 | `UnsupportedCharset` | `Charset::from_syntax_identifier` | — |
| E040 | `NonFiniteNumber` | `DecimalFloat` serialization | — |
| E041 | `CharacterRepertoireMismatch` | `Writer::begin_interchange` | — |
| E042 | `EmptyInterchange` | Envelope validator | — |
| E043 | `EmptyMessage` | Envelope validator | `span` |
| E044 | `PackageNotSupported` | Envelope validator | `span` |
| E045 | `BlankDataElementValue` | Syntax validator | `span` |
| E046 | `SegmentWithoutDataElements` | Syntax validator | `span` |
| E047 | `TooManyRepetitions` | Directory validator | `span` |
| E048 | `InvalidCharacterType` | Directory validator | `span` |
| E049 | `DataElementTooLong` | Directory validator | `span` |
| E050 | `DataElementTooShort` | Directory validator | `span` |
| E051 | `TrailingSeparator` | Syntax validator | `span` |
| E052 | `GroupsAndMessagesMixed` | Envelope validator | `span` |
| E053 | `InsignificantCharacters` | Directory validator | `span` |

---

## Variant details

### E001 — `UnexpectedEof`

```text
unexpected end of input at byte offset {offset}
```

**When**: Parser exhausted input before finding a mandatory delimiter (segment
terminator, element separator, etc.).

**Fields**: `offset: usize` — byte position where input ended.

**Fix**: Ensure every segment ends with the configured segment terminator. Check that
the payload was not truncated.

---

### E002 — `InvalidDelimiter`

```text
invalid delimiter byte 0x{byte:02X} at offset {offset}
```

**When**: A byte in a delimiter position (e.g. after parsing the UNA) is not a valid
ASCII delimiter.

**Fields**: `byte: u8`, `offset: usize`.

**Fix**: Check the UNA service string advice and the delimiter bytes in the payload.

---

### E003 — `InvalidText`

```text
invalid EDIFACT text at byte offset {offset}
```

**When**: A segment or element contains a non-UTF-8 byte sequence.

**Fields**: `offset: usize`.

**Fix**: Decode the interchange first. `UNOC`–`UNOK` are single-byte ISO 8859
repertoires whose high bytes are not valid UTF-8; `decode_interchange` reads the
repertoire from `UNB` S001 and converts, copying nothing when the payload is
already ASCII or `UNOY`. See [Character Sets](@/docs/character-sets.md).

Also raised by `Charset::decode` for a byte that falls in a slot the repertoire
leaves undefined.

---

### E004 — `MessageCountMismatch`

```text
interchange message count mismatch: UNZ declared {expected}, found {actual}
```

**When**: The `UNZ` segment declares a message count that does not match the number
of `UNH`/`UNT` pairs observed.

**Fields**: `expected: u32`, `actual: u32`.

**Fix**: Regenerate the `UNZ` segment with the correct count, or check for missing /
extra `UNH..UNT` pairs.

---

### E005 — `SegmentCountMismatch`

```text
segment count mismatch in message {message_ref}: UNT declared {expected}, found {actual}
```

**When**: The `UNT` segment's element 1 (segment count, inclusive of `UNH`/`UNT`)
does not match the actual count.

**Fields**: `expected: u32`, `actual: u32`, `message_ref: String`.

**Fix**: Recount segments and update `UNT` element 1.

---

### E006 — `InvalidSegmentTag`

```text
invalid segment tag {0:?}
```

**When**: A segment tag that is not exactly 3 ASCII uppercase letters is encountered.

**Fields**: `String` — the bad tag text.

**Fix**: Segment tags must match `[A-Z]{3}`. Check for leading/trailing whitespace or
lowercase letters.

---

### E007 — `InvalidUna`

```text
invalid UNA service string advice: must be exactly 9 bytes
```

**When**: A `UNA` segment is present but is not exactly 9 bytes (`UNA` + 6 service
characters).

**Fix**: The UNA must be exactly: `UNA:+.? '` (9 bytes with the default delimiters).

---

### E008 — `MissingRequiredElement`

```text
missing required element {element_index} in segment {tag}
```

**When**: The typed deserializer (`#[derive(EdifactDeserialize)]`) expected a
mandatory element that was absent.

**Fields**: `tag: String`, `element_index: usize`.

**Fix**: Add the missing element to the segment, or mark the field `Option<T>` if it
is truly optional.

---

### E009 — `InvalidUtf8`

```text
serialized output contains invalid UTF-8
```

**When**: An internal consistency check in the serializer detected non-UTF-8 output.
This should never occur in correct usage — file a bug if you see it.

---

### E010 — `Io`

```text
(transparent — wraps std::io::Error)
```

**When**: Any I/O error from `std::io::Read` / `std::io::Write`.

**Fields**: wraps `IoError(std::io::Error)`.

**Fix**: Check file permissions, disk space, or network connectivity depending on
the underlying I/O source.

---

### E011 — `InvalidSegmentForMessage`

```text
segment {tag} is not valid for message type {message_type}
```

**When**: Directory validation found a segment that is not permitted in the current
message type.

**Fields**: `tag: String`, `message_type: String`, `span: Span`.

**Fix**: Remove the unsupported segment or switch to the correct message type.

---

### E012 — `InvalidElementCount`

```text
segment {tag} has {actual} elements, expected between {min} and {max}
```

**When**: Directory validation found an element count outside the allowed `[min, max]`
range.

**Fields**: `tag`, `min`, `max`, `actual`, `span: Span`.

**Fix**: Adjust the element count to within the directory-defined bounds.

---

### E013 — `InvalidComponentCount`

```text
segment {tag} element {element_index} has {actual} components, expected {expected}
```

**When**: Either of two checks in `DirectoryValidator`:

1. An `expected_components` hook is registered for the element and the observed
   count differs — the hook is an **exact** count.
2. No hook applies, but the element is a composite whose components the directory
   declares (`ElementRef::composite` / `OwnedElementRef::with_components`), and
   the segment supplies **more** components than declared. Supplying fewer stays
   valid: conditional components may be omitted, and trailing empty components
   are stripped first per ISO 9735-1 §8.7.2.

**Fields**: `tag`, `element_index`, `expected: u8` (the exact or maximum count),
`actual: u8`, `span: Span`.

**Fix**: Fix the composite element arity to match the directory definition.

---

### E014 — `InvalidCodeValue`

```text
segment {tag} element {element_index}: '{value}' is not a valid code (code list {code_list})
```

**When**: A field that should hold a code-list value contains an unrecognised code.

**Fields**: `tag`, `element_index`, `value`, `code_list`, `span: Span`, `suggestion: Option<&'static str>`.

**Fix**: Use a valid code from the referenced code list. If `suggestion` is present,
it will contain a remediation hint.

---

### E015 — `MissingSegment`

```text
required segment {tag} is missing from message (position {expected_position})
```

**When**: Structural validation determined a mandatory segment is absent.

**Fields**: `tag: String`, `expected_position: String` (human-readable position hint).

**Fix**: Add the missing segment at the indicated position.

---

### E016 — `QualifierMismatch`

```text
segment {tag} has qualifier '{actual}', expected '{expected}'
```

**When**: A qualified-segment mapping (e.g. `NAD+BY`) found a qualifier that does
not match the expected value.

**Fields**: `tag`, `actual`, `expected`, `span: Span`.

**Fix**: Correct the qualifier or use the right qualified segment type.

---

### E017 — `ConditionalRequirementNotMet`

```text
segment {tag} element {element_index}: conditional requirement not met ({condition})
```

**When**: An element that is required by a conditional rule (e.g. "required when
element 0 is `BY`") is absent.

**Fields**: `tag`, `element_index`, `condition: String`, `span: Span`.

**Fix**: Provide the conditionally required element, or remove the element that
triggered the condition.

---

### E019 — `InvalidReleaseSequence`

```text
invalid release sequence at byte offset {offset}: dangling release character
```

**When**: A release character (`?` by default) appears at the end of input with no
following byte to escape.

**Fields**: `offset: usize`.

**Fix**: Remove the trailing release character, or add the character that should be
escaped.

---

### E020 — `SegmentTooLong`

```text
segment starting at byte offset {offset} exceeded maximum length of {limit} bytes
```

**When**: A reader-based parser accumulated more bytes than the `max_segment_bytes`
limit in `ReaderConfig` without encountering a segment terminator. This is a DOS
guard against adversarially crafted or truncated input.

**Fields**: `offset: usize`, `limit: usize`.

**Fix**: Increase `ReaderConfig::max_segment_bytes` if the segment is legitimately
large, or investigate why the terminator is missing.

---

### E021 — `MissingRequiredComponent`

```text
missing required component {component_index} in element {element_index} of segment {tag}
```

**When**: The typed deserializer expected a mandatory component within a composite
element that was present but did not contain the required component.

**Fields**: `tag: String`, `element_index: usize`, `component_index: usize`.

**Fix**: Provide the missing component inside the composite element, or mark the
field `Option<T>` if it is truly optional.

---

### E023 — `InterchangeTooLarge`

```text
interchange too large: count {count} exceeds u32::MAX
```

**When**: An interchange being built has accumulated more than 4 294 967 295 items.
This is effectively unreachable on real-world data; it usually indicates corrupted
or synthetic input.

**Fields**: `count: u64`.

**Fix**: Verify the input is not corrupted. If genuinely processing very large
interchanges, partition the input into smaller batches.

---

### E024 — `InvalidEventSequence`

```text
invalid event sequence: {message}
```

**When**: An `EventEmitter` received events in an invalid order — for example an
`Element` event before `StartSegment`, or a `ComponentElement` event before
`Element`.

**Fields**: `message: &'static str` describing the violation.

**Fix**: Emit `StartSegment` before `Element`, and `Element` before
`ComponentElement`. This error always indicates a programming mistake in the
caller's serialization code.

---

### E025 — `InvalidElementPosition`

```text
element definition contains invalid position 0; positions must be >= 1 (one-based)
```

**When**: An `OwnedElementRef` was constructed with `position = 0`. Element
positions are one-based — position 1 is the first element slot. Position 0 is
never valid.

**Fix**: Use `OwnedElementRef::try_new(position, ...)` which returns
`Err(EdifactError::InvalidElementPosition)` instead of panicking. For trusted
literal position values use `OwnedElementRef::new_unchecked`, which panics
immediately rather than returning this error.

---

### E026 — `IncompatibleReleaseScopes`

```text
incompatible release scopes: cannot compose {current:?} with {incoming:?}
```

**When**: Two `ProfileRulePack` values with different release scopes were
composed via `merge`, `extend_from`, or `merge_with_override`. Both packs must
either share the same release scope or at most one may carry a scope.

**Fields**: `current: String` (scope on the receiving pack), `incoming: String`
(scope on the pack being merged in).

**Fix**: Ensure both packs target the same release with
`ProfileRulePack::for_release`, or remove the scope from one pack before
composing.

---

### E027 — `InvalidFieldValue`

```text
segment {tag} element {element_index}: invalid field value "{value}"
```

**When**: The typed deserializer found a qualifier element that was present but
held an empty or otherwise invalid value.

**Fields**: `tag: String`, `element_index: usize`, `value: String`.

**Fix**: Check that the qualifier element in segment `tag` at position
`element_index` contains a recognised non-empty value.

---

### E028 — `UnexpectedDataToken`

```text
unexpected data token at byte offset {offset}: data element before segment tag
```

**When**: The parser encountered a data-element or component-element token
before reading the first segment tag.  This usually indicates a partial write,
a missing segment tag, or encoding corruption.

**Fields**: `offset: usize`.

**Fix**: Verify the input starts with a valid segment tag (three uppercase
ASCII letters) and that no data or component separators appear before it.

---

### E030 — `ValidationErrors`

```text
validation failed with {error_count} error(s)
```

**When**: Constructed explicitly to promote a `ValidationReport` that contains at
least one error-severity issue into an `EdifactError` — typically inside application
code or library helpers that need to return `Result<_, EdifactError>` rather than a
bare report.  Note that `ValidationReport::result` itself returns
`Result<ValidationReport, ValidationReport>` (the `Err` arm carries the full report)
and does **not** produce this variant automatically; callers must wrap it themselves
when needed.

**Fields**: `error_count: usize`, `report: Box<ValidationReport>`.

**Fix**: Inspect `report` for the full list of issues with locations, rule IDs, and
suggested fixes. Call `ValidationContext::validate` if you want validation to always return a
report rather than an error.

---

## Matching errors

Because `EdifactError` is `#[non_exhaustive]`, always include a wildcard arm:

```rust
use edifact_rs::EdifactError;

fn handle(err: EdifactError) {
    match err {
        EdifactError::UnexpectedEof { offset } => {
            eprintln!("E001 truncated input at byte {offset}");
        }
        EdifactError::InvalidCodeValue { value, code_list, .. } => {
            eprintln!("E014 bad code '{value}' in list {code_list}");
        }
        EdifactError::Io(e) => {
            eprintln!("E010 I/O: {e}");
        }
        other => {
            eprintln!("{} {other}", other.stable_code());
        }
    }
}
```

---

## Stable codes in logs

Use `err.stable_code()` to emit a stable, searchable error code in structured logs:

```rust,ignore
use edifact_rs::from_bytes;

match from_bytes(b"BAD").collect::<Result<Vec<_>, _>>() {
    Ok(_) => {}
    Err(e) => {
        tracing::error!(code = e.stable_code(), error = %e, "EDIFACT parse error");
    }
}
```

---

## Diagnostics (rich error output)

Enable the `diagnostics` feature to get `miette::Diagnostic` on all variants with
`offset` / `span` fields. See [Diagnostics](@/docs/diagnostics.md) for details.

---

## Next steps

- [Diagnostics](@/docs/diagnostics.md) — miette span-annotated rendering
- [Validation](@/docs/validation.md) — `ValidationReport` vs `EdifactError`
- [Performance](@/docs/performance.md) — error-free fast paths

---

### E031 — `UnrecognisedSyntaxIdentifier`

```text
unrecognised syntax identifier 'XXXX': expected UNOA-UNOK, UNOX, UNOY, or KECA
```

**When**: `UNB` S001 DE 0001 names no defined character repertoire. The value is
`UN` plus a two-character repertoire code, so the defined set is `UNOA`–`UNOK`,
`UNOX`, `UNOY`, and `KECA`.

**Fields**: `0: String` — the offending identifier.

**Fix**: Declare the repertoire the payload is actually written in. A value that
*is* defined but that this crate cannot decode — `UNOX`, `KECA` — raises
[E039](#e039-unsupportedcharset) instead; the two make different claims about
whose problem it is.

---

### E032 — `DuplicateReference`

```text
duplicate {tag} reference '{reference}' at bytes {span}
```

**When**: Two messages in one interchange share a `UNH` reference (DE 0062), or
two functional groups share a `UNG` reference (DE 0048).  ISO 9735-1 requires
both to be unique within the interchange; duplicates make a message
unaddressable, because a receiver keying on the reference processes one
occurrence and silently drops the rest.

**Fields**: `tag: String` (`UNH` or `UNG`), `reference: String`, `span: Span`.

**Fix**: Assign a distinct control reference to every message and group.

---

### E033 — `UnknownDataElement`

```text
segment {tag} has no data element {data_element} in its definition
```

**When**: A code-addressed accessor — `Segment::value_by_code`,
`span_by_code`, `element_by_code`, or `SegmentLayout::resolve_code` — was given a
UN/EDIFACT data element identifier that the supplied `SegmentDefinition` /
`OwnedSegmentDef` does not declare.

This is the variant that makes code-addressed access safer than positional
access: a stale or mistyped reference fails here instead of reading whichever
element happens to sit at the wrong index. The derive form
(`#[edifact(element = "3055")]` under `#[edifact(layout = ...)]`) catches the
same mistake at compile time, during const evaluation.

**Fields**: `tag: String`, `data_element: String`.

**Fix**: Check the identifier against the directory definition for that segment.

---

### E034 — `AmbiguousDataElement`

```text
segment {tag} defines data element {data_element} at more than one position
```

**When**: The identifier resolves to more than one position in the definition,
so code-addressed access cannot pick one.

**Fields**: `tag: String`, `data_element: String`.

**Fix**: Address the element positionally (`element_str` / `component_str`), or
split the definition so the identifier is unique.

---

### E035 — `SegmentLayoutMismatch`

```text
segment layout is for {expected}, but the segment is {actual}
```

**When**: A `SegmentLayout` was applied to a segment with a different tag — for
example passing the `NAD` definition to a `DTM` segment. Resolving against the
wrong table is exactly the class of mistake code-addressed access exists to
prevent, so it is rejected before any lookup.

**Fields**: `expected: String` (the layout's tag), `actual: String`.

**Fix**: Look the definition up by the segment's own tag.

---

### E036 — `LimitExceeded`

```text
input exceeded the configured {limit} limit of {max}
```

**When**: The input carries more than a `ReaderConfig` whole-input budget allows —
`max_segments`, `max_messages`, or `max_input_bytes`. The per-segment size guard
has its own code (`E020 SegmentTooLong`).

**Why an error and not a quiet stop**: a budget that merely ended the iterator is
indistinguishable from a clean end of input, so
`collect::<Result<Vec<_>, _>>()` would succeed on a **truncated** interchange and
everything downstream would treat a fragment as the whole message. Input that ends
*exactly* at a limit is not a violation.

**Fields**: `limit: &'static str` (`"max_segments"`, `"max_messages"`, or
`"max_input_bytes"`), `max: u64`.

**Fix**: Raise the corresponding `ReaderConfig` budget if the input is legitimate,
or reject it as oversized. Segments parsed before the violation are still
delivered, so a manual iteration can keep the partial result.

---

### E037 — `RepetitionSeparatorNotDeclared`

```text
cannot write a repeating data element: the active service string advice
declares no repetition separator
```

**When**: A `Segment` carrying an `Element` with repetitions (ISO 9735-1 §8.6) was
handed to a `Writer` whose `ServiceStringAdvice` holds the space "not used"
sentinel at UNA position 7.

**Why**: There is no byte to write between the occurrences. Emitting the space
anyway produces output that reads back as a *single* occurrence whose value
contains a space — silent data corruption from a call that reported success.

**Fix**: Build the writer with `Writer::with_una` and a `ServiceStringAdvice`
whose `repetition_sep` is set, or flatten the repetitions before writing.

**Recovery**: The check runs before any byte is written, so the sink is untouched
and the writer can be reused for the next segment.

### E038 — `CharacterNotInRepertoire`

```text
character 'ü' at offset 1 is not in the UNOA character repertoire
```

**When**: A writer bound with [`Writer::with_charset`](@/docs/writing.md#character-repertoires)
was handed a value containing a character the declared repertoire cannot carry —
or [`Charset::encode`] was called directly with one.

**Why**: `UNB` S001 DE 0001 tells the receiver which table to decode the payload
with. A character outside that table has no byte to be written as; emitting one
anyway produces a value the receiver reads as a different character, or as an
undefined slot.

**Fix**: Transliterate the value (`ü` → `ue`), or declare a wider repertoire —
`UNOC` for Latin-1, `UNOY` for full UTF-8.

**Recovery**: The check happens during value encoding, so a partially written
segment is possible. Prefer validating with
[`Charset::first_violation`] before writing when you need to skip bad records
and continue.

---

### E039 — `UnsupportedCharset`

```text
character repertoire 'UNOX' is not supported
```

**When**: An interchange declares `UNOX` (ISO 2022 code extension) or `KECA`
(Korean) in `UNB` S001 DE 0001.

**Why**: Every repertoire this crate supports is single-byte and
ASCII-transparent, which is what makes byte-level delimiter scanning sound.
`UNOX` is stateful and `KECA` is multi-byte, so a `+` byte inside a multi-byte
sequence would be mistaken for an element separator. Mis-decoding silently is
worse than refusing.

**Fix**: Ask the partner for `UNOC` or `UNOY`, or transcode the interchange with
a dedicated codec before handing it to `edifact-rs`.

---

### E040 — `NonFiniteNumber`

```text
non-finite number NaN has no EDIFACT representation
```

**When**: `DecimalFloat(f64::NAN)` or `DecimalFloat(f32::INFINITY)` was
serialized.

**Why**: An EDIFACT numeric data element is digits with an optional sign and
decimal mark (ISO 9735-1 §10). Rust's `Display` renders these values as `NaN`,
`inf`, and `-inf` — text no receiver can parse, and which this crate's own reader
returns as an ordinary string rather than a number. Writing it turns an
arithmetic bug into a wire-format bug found days later.

**Fix**: Check the calculation, or omit the element rather than emitting a
placeholder.

---

### E041 — `CharacterRepertoireMismatch`

```text
UNB declares repertoire UNOA, but the writer encodes UNOC
```

**When**: `Writer::begin_interchange` was called with a syntax identifier that
differs from the repertoire the writer was bound to with `Writer::with_charset`.

**Why**: The header would tell the receiver to decode the body with the wrong
table. Every non-ASCII value then arrives as mojibake, and nothing in the
interchange reveals why.

**Fix**: Pass the writer's own identifier — `writer.charset().unwrap().syntax_identifier()`
— or bind the writer to the repertoire the header declares.

---

### E042 — `EmptyInterchange`

```text
interchange IC4711 contains no message or group
```

**When**: A `UNB`/`UNZ` pair encloses nothing. ISO 9735-1 §7.1 requires an
interchange to "contain at least one group, or one message or one package".

**Why**: `UNZ+0` makes the control count agree with the (absent) content, so no
count check can see this. An empty interchange is usually a producer that
serialised an empty result set instead of skipping the send — the receiver files
a delivery, acknowledges it, and nothing arrives.

**Fields**: `control_ref: String` — `UNB` DE 0020.

**Fix**: Send nothing rather than an empty envelope.

---

### E043 — `EmptyMessage`

```text
message MSG1 has no segments between UNH and UNT
```

**When**: A `UNH`/`UNT` pair encloses nothing. ISO 9735-1 §7.3 requires a message
to "contain at least one additional segment".

**Why**: As with E042, `UNT+2` is internally consistent, so the segment-count
check confirms the message rather than rejecting it.

**Fields**: `message_ref: String` (`UNH` DE 0062), `span: Span` of the `UNH`.

**Fix**: Omit the message entirely if it has no content.

---

### E044 — `PackageNotSupported`

```text
segment UNO opens or closes a package, which this crate does not parse
```

**When**: The interchange carries a package — `UNO`…`UNP` (ISO 9735-1 §7.9,
elaborated by ISO 9735-8).

**Why**: The object inside a package is arbitrary binary data whose length is
declared in `UNO` S022 DE 0810. It is not EDIFACT-encoded, and feeding it to a
tokenizer that scans for delimiters produces nonsense. A package is a *legal*
member of an interchange, so reporting it as a stray segment would send you
looking for a corruption that is not there.

**Fields**: `tag: String` (`UNO` or `UNP`), `span: Span`.

**Fix**: Split the object out of the byte stream using the declared length, then
parse the remaining segments. `service::UNO` and `service::UNP` ship as layouts
so the header and trailer themselves can be read by data element identifier.

---

### E045 — `BlankDataElementValue`

```text
segment FTX element 1 component 0: value is only spaces
```

**When**: A data element value consists of nothing but spaces. ISO 9735-1 §9.3:
"A data element value containing only space(s) shall not be allowed."

**Why**: Trailing spaces are insignificant and must be suppressed (§9.1), so a
value made only of spaces is an element that should have been omitted. It is a
classic artefact of a fixed-width source record copied into a variable-length
field — and a receiver comparing it against a code list will not treat it as
absent.

Raised as a **warning**, not an error: the value is still readable.

**Fields**: `tag: String`, `element_index: usize`, `component_index: usize`,
`span: Span`.

**Fix**: Omit the element instead of padding it. Emitted by
[`SyntaxValidator`](@/docs/validation.md#syntax-validation).

---

### E046 — `SegmentWithoutDataElements`

```text
segment DTM contains no data element
```

**When**: A segment carries nothing but its tag. ISO 9735-1 §7.5: "A segment
shall contain at least one data element in addition to the segment tag."

**Why**: §8.5 adds that a conditional segment whose only content is the tag
"shall be omitted in its entirety" — so `DTM'` is either a mandatory segment that
lost its data or a conditional one that should not have been sent. Note that
`DTM+'` is *not* this error: an empty data element is present, which is how
EDIFACT spells a mandatory segment with no data to carry (§8.4).

**Fields**: `tag: String`, `span: Span`.

**Fix**: Supply the data, or drop the segment. Emitted by
[`SyntaxValidator`](@/docs/validation.md#syntax-validation).

---

### E047 — `TooManyRepetitions`

```text
segment RFF element 0 occurs 3 times, at most 1 allowed
```

**When**: A data element occurred more times than its definition's `max_repeat`
allows. ISO 9735-1 §7.5 requires a segment specification to state each element's
maximum number of occurrences.

**Why**: `max_repeat` had been carried on every `ElementRef` and read by nothing,
so a definition that said "this element occurs once" constrained nothing — and a
caller who wrote it believed otherwise. It is enforced now.

**Fields**: `tag`, `element_index`, `max: u8`, `actual: usize`, `span: Span`.

**Fix**: Reduce the occurrences, or correct the definition if the directory
allows more. Reported by `CONTRL` as code 35.

---

### E048 — `InvalidCharacterType`

```text
segment UNZ element 0 component 0: "abc" is not n..6
```

**When**: A value's characters do not match its declared representation class —
a letter in an `n` field, for instance.

**Why**: ISO 9735-1 §10 fixes what "numeric" admits: digits, an optional leading
minus, a decimal mark (`.` or `,`), and an exponent. It excludes the space
character and the plus sign explicitly, and requires at least one digit after a
decimal mark — so `1.` and `.` are rejected while `.5` and `2.00` are not.

**Fields**: `tag`, `element_index`, `component_index`, `repr: String`,
`value: String`, `span: Span`.

**Fix**: Send a value of the declared class. Reported by `CONTRL` as code 37.

---

### E049 — `DataElementTooLong`

```text
segment UNZ element 1 component 0: 20 characters exceeds an..14
```

**When**: A value is longer than its declared representation allows.

**Why**: Length is counted in **characters**, not bytes — ISO 9735-1 §6: "one
graphic character shall be counted as one character, irrespective of the number
of bytes/octets required to encode it", so `ü` counts once. §5 excludes the
release character, which is automatic here because release sequences are resolved
before validation. For a numeric value §10 excludes more still: the sign, the
decimal mark, and the exponent — `-123.45` is five characters, not seven.

**Fields**: `tag`, `element_index`, `component_index`, `repr: String`,
`actual: usize`, `span: Span`.

**Fix**: Shorten the value. Reported by `CONTRL` as code 39.

---

### E050 — `DataElementTooShort`

```text
segment UNB element 0 component 0: 3 characters is short of a4
```

**When**: A value is shorter than its declared **fixed-length** representation.

**Why**: Only a fixed representation (`n8`, `a1`, `a4`) has a minimum above one;
a variable one (`an..35`) is satisfied by any non-empty value, because an empty
value means the element is absent (§8.1) rather than too short.

**Fields**: `tag`, `element_index`, `component_index`, `repr: String`,
`actual: usize`, `span: Span`.

**Fix**: Pad the value, or correct the definition if the directory declares it
variable. Reported by `CONTRL` as code 40.

---

### E051 — `TrailingSeparator`

```text
segment BGM ends in a separator that carries no value
```

**When**: A segment ends in an empty data element, or a composite ends in an
empty component — `BGM+220+'` and `DTM+137:20260101:'`.

**Why**: ISO 9735-1 §8.7.1: "If one or more non-repeating composite data elements
or stand-alone data elements at the end of a segment are omitted, the data
element separators which would normally follow them shall also be omitted."
§8.7.2 says the same for components at the end of a composite.

Note the contrast with an **interior** omission, which must keep its separator
(§8.7.1 Figure 1): `BGM+220++9'` is correct and is not reported. So is `BGM+'` —
§8.4 spells a mandatory segment with no data to carry exactly that way.

Raised as a **warning**: a correct parser reads the value anyway.

**Fields**: `tag: String`, `element_index: Option<usize>` (`Some` when the
trailing separators close a composite, `None` when they close the segment),
`span: Span`.

**Fix**: Stop emitting separators once the last value is written. Reported by
`CONTRL` as code 45.

---

### E052 — `GroupsAndMessagesMixed`

```text
interchange mixes groups with ungrouped messages
```

**When**: A `UNH` appears outside every `UNG`…`UNE` in an interchange that uses
groups.

**Why**: ISO 9735-1 §7.1 lists what an interchange may contain, and the entries
are exclusive — groups containing messages, *or* bare messages, never both. A
message outside every group has no group to be counted in, so `UNZ` DE 0036
cannot describe the interchange at all.

**Fields**: `span: Span`.

**Fix**: Put every message inside a group, or none of them. Reported by `CONTRL`
as code 30 — the code that exists for precisely this, rather than the general
"not supported in this position".

---

### E053 — `InsignificantCharacters`

```text
segment ZZZ element 0 component 0: leading zeroes are not suppressed
```

**When**: A **variable-length** value carries characters ISO 9735-1 §9.1 requires
the sender to suppress: leading zeroes in a numeric value, trailing spaces in an
alphabetic or alphanumeric one.

**Why**: These are the fingerprints of a fixed-width source record copied into a
variable-length field. The value is readable, so this is a warning — but a
receiver comparing `007` against the code `7`, or `"ACME "` against `"ACME"`,
will not match them.

Two deliberate exemptions. §9.1 allows "a single zero before a decimal mark", so
`0.5` is correct while `00.5` is not. And it governs *variable* length elements
only: a **fixed**-length numeric field is zero-padded by design and a fixed text
one space-padded, so neither is reported.

Requires a declared [representation](@/docs/validation.md#data-element-representations)
— without one there is no way to know whether the length is fixed.

**Fields**: `tag`, `element_index`, `component_index`, `kind: Insignificant`,
`span: Span`.

**Fix**: Suppress the characters before sending. Reported by `CONTRL` as code 12.

---

## Retired codes

These codes were used by variants that no longer exist.  They are never reissued,
so a stored code always identifies the same condition:

| Code | Former variant | Retired because |
|---|---|---|
| E018 | `ValidationFailed` | Superseded by `ValidationErrors` (E030). |
| E022 | `UnexpectedMessageType` | Message dispatch is a `match` on `MessageWindow::message_type`, not a registry. |
| E029 | `FunctionalGroupNotSupported` | Functional groups (`UNG`/`UNE`) are parsed and validated natively. |
