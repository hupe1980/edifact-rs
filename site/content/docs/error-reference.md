+++
title = "Error Reference"
description = "Every EdifactError variant with its stable code E001-E037, the fields it carries, when it fires, and how to fix it."
weight = 110
+++

All errors returned by `edifact-rs` are variants of `EdifactError`. Every variant
carries a stable, semver-protected code (`E001`–`E037`) accessible via
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
| E022 | `UnexpectedMessageType` | Message dispatch | — |
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

**Fix**: Re-encode the input as UTF-8. EDIFACT character set `UNOA` (Latin-1 subset)
must be transcoded before passing to `edifact-rs`.

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
   are stripped first per ISO 9735-1 §3.3.

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

### E022 — `UnexpectedMessageType`

```text
no handler registered for message type {message_type}
```

**When**: `MessageDispatch::dispatch` was called with a message whose `UNH` segment
specifies a type that has no registered handler and no fallback was configured.

**Fields**: `message_type: String`.

**Fix**: Register a handler with `MessageDispatch::on("TYPE", ...)`, or add a
catch-all fallback handler.

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
bare report.  Note that `validate_strict` itself returns
`Result<ValidationReport, ValidationReport>` (the `Err` arm carries the full report)
and does **not** produce this variant automatically; callers must wrap it themselves
when needed.

**Fields**: `error_count: usize`, `report: Box<ValidationReport>`.

**Fix**: Inspect `report` for the full list of issues with locations, rule IDs, and
suggested fixes. Call `validate_lenient` if you want validation to always return a
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
unrecognised syntax identifier '{0}': expected UNOA/UNOB/UNOC/UNOD/UNOE/UNOF (or KECA)
```

**When**: `UNB` DE 0001 holds a value that is not one of the syntax identifiers
defined in ISO 9735-1 §3.1.

**Fields**: `0: String` — the offending identifier.

**Fix**: Use one of `UNOA`, `UNOB`, `UNOC`, `UNOD`, `UNOE`, `UNOF`, or `KECA`.

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

**When**: A `Segment` carrying an `Element` with repetitions (ISO 9735-4 §3.1) was
handed to a `Writer` whose `ServiceStringAdvice` holds the space "not used"
sentinel at UNA position 7.

**Why**: There is no byte to write between the occurrences. Emitting the space
anyway produces output that reads back as a *single* occurrence whose value
contains a space — silent data corruption from a call that reported success.

**Fix**: Build the writer with `Writer::with_una` and a `ServiceStringAdvice`
whose `repetition_sep` is set, or flatten the repetitions before writing.

---

## Retired codes

These codes were used by variants that no longer exist.  They are never reissued,
so a stored code always identifies the same condition:

| Code | Former variant | Retired because |
|---|---|---|
| E018 | `ValidationFailed` | Superseded by `ValidationErrors` (E030). |
| E029 | `FunctionalGroupNotSupported` | Functional groups (`UNG`/`UNE`) are now parsed and validated natively. |
