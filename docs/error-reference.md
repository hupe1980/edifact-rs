# Error Reference 🔴

All errors returned by `edifact-rs` are variants of `EdifactError`. Every variant
carries a stable, semver-protected code (`E001`–`E021`) accessible via
`err.stable_code()`. The enum is marked `#[non_exhaustive]` so future variants can
be added without breaking existing match arms.

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
| E011 | `InvalidSegmentForMessage` | Directory validator | `offset` |
| E012 | `InvalidElementCount` | Directory validator | `offset` |
| E013 | `InvalidComponentCount` | Directory validator | `offset` |
| E014 | `InvalidCodeValue` | Directory validator | `offset` |
| E015 | `MissingSegment` | Directory validator | — |
| E016 | `QualifierMismatch` | Typed deserializer | `offset` |
| E017 | `ConditionalRequirementNotMet` | Profile validator | `offset` |
| E018 | `ValidationFailed` | Strict validation | — |
| E019 | `InvalidReleaseSequence` | Parser | `offset` |
| E020 | `SegmentTooLong` | Reader parser | `offset` |
| E021 | `MissingRequiredComponent` | Deserializer | — |

---

## Variant details

### E001 — `UnexpectedEof`

```
unexpected end of input at byte offset {offset}
```

**When**: Parser exhausted input before finding a mandatory delimiter (segment
terminator, element separator, etc.).

**Fields**: `offset: usize` — byte position where input ended.

**Fix**: Ensure every segment ends with the configured segment terminator. Check that
the payload was not truncated.

---

### E002 — `InvalidDelimiter`

```
invalid delimiter byte 0x{byte:02X} at offset {offset}
```

**When**: A byte in a delimiter position (e.g. after parsing the UNA) is not a valid
ASCII delimiter.

**Fields**: `byte: u8`, `offset: usize`.

**Fix**: Check the UNA service string advice and the delimiter bytes in the payload.

---

### E003 — `InvalidText`

```
invalid EDIFACT text at byte offset {offset}
```

**When**: A segment or element contains a non-UTF-8 byte sequence.

**Fields**: `offset: usize`.

**Fix**: Re-encode the input as UTF-8. EDIFACT character set `UNOA` (Latin-1 subset)
must be transcoded before passing to `edifact-rs`.

---

### E004 — `MessageCountMismatch`

```
interchange message count mismatch: UNZ declared {expected}, found {actual}
```

**When**: The `UNZ` segment declares a message count that does not match the number
of `UNH`/`UNT` pairs observed.

**Fields**: `expected: u32`, `actual: u32`.

**Fix**: Regenerate the `UNZ` segment with the correct count, or check for missing /
extra `UNH..UNT` pairs.

---

### E005 — `SegmentCountMismatch`

```
segment count mismatch in message {message_ref}: UNT declared {expected}, found {actual}
```

**When**: The `UNT` segment's element 1 (segment count, inclusive of `UNH`/`UNT`)
does not match the actual count.

**Fields**: `expected: u32`, `actual: u32`, `message_ref: String`.

**Fix**: Recount segments and update `UNT` element 1.

---

### E006 — `InvalidSegmentTag`

```
invalid segment tag {0:?}
```

**When**: A segment tag that is not exactly 3 ASCII uppercase letters is encountered.

**Fields**: `String` — the bad tag text.

**Fix**: Segment tags must match `[A-Z]{3}`. Check for leading/trailing whitespace or
lowercase letters.

---

### E007 — `InvalidUna`

```
invalid UNA service string advice: must be exactly 9 bytes
```

**When**: A `UNA` segment is present but is not exactly 9 bytes (`UNA` + 6 service
characters).

**Fix**: The UNA must be exactly: `UNA:+.? '` (9 bytes with the default delimiters).

---

### E008 — `MissingRequiredElement`

```
missing required element {element_index} in segment {tag}
```

**When**: The typed deserializer (`#[derive(EdifactDeserialize)]`) expected a
mandatory element that was absent.

**Fields**: `tag: String`, `element_index: usize`.

**Fix**: Add the missing element to the segment, or mark the field `Option<T>` if it
is truly optional.

---

### E009 — `InvalidUtf8`

```
serialized output contains invalid UTF-8
```

**When**: An internal consistency check in the serializer detected non-UTF-8 output.
This should never occur in correct usage — file a bug if you see it.

---

### E010 — `Io`

```
(transparent — wraps std::io::Error)
```

**When**: Any I/O error from `std::io::Read` / `std::io::Write`.

**Fields**: wraps `IoError(std::io::Error)`.

**Fix**: Check file permissions, disk space, or network connectivity depending on
the underlying I/O source.

---

### E011 — `InvalidSegmentForMessage`

```
segment {tag} is not valid for message type {message_type}
```

**When**: Directory validation found a segment that is not permitted in the current
message type.

**Fields**: `tag: String`, `message_type: String`, `offset: usize`.

**Fix**: Remove the unsupported segment or switch to the correct message type.

---

### E012 — `InvalidElementCount`

```
segment {tag} has {actual} elements, expected between {min} and {max}
```

**When**: Directory validation found an element count outside the allowed `[min, max]`
range.

**Fields**: `tag`, `min`, `max`, `actual`, `offset`.

**Fix**: Adjust the element count to within the directory-defined bounds.

---

### E013 — `InvalidComponentCount`

```
segment {tag} element {element_index} has {actual} components, expected {expected}
```

**When**: A composite element has a different number of components than expected.

**Fields**: `tag`, `element_index`, `expected: u8`, `actual: u8`, `offset`.

**Fix**: Fix the composite element arity to match the directory definition.

---

### E014 — `InvalidCodeValue`

```
segment {tag} element {element_index}: '{value}' is not a valid code (code list {code_list})
```

**When**: A field that should hold a code-list value contains an unrecognised code.

**Fields**: `tag`, `element_index`, `value`, `code_list`, `offset`, `suggestion: Option<&'static str>`.

**Fix**: Use a valid code from the referenced code list. If `suggestion` is present,
it will contain a remediation hint.

---

### E015 — `MissingSegment`

```
required segment {tag} is missing from message (position {expected_position})
```

**When**: Structural validation determined a mandatory segment is absent.

**Fields**: `tag: String`, `expected_position: String` (human-readable position hint).

**Fix**: Add the missing segment at the indicated position.

---

### E016 — `QualifierMismatch`

```
segment {tag} has qualifier '{actual}', expected '{expected}'
```

**When**: A qualified-segment mapping (e.g. `NAD+BY`) found a qualifier that does
not match the expected value.

**Fields**: `tag`, `actual`, `expected`, `offset`.

**Fix**: Correct the qualifier or use the right qualified segment type.

---

### E017 — `ConditionalRequirementNotMet`

```
segment {tag} element {element_index}: conditional requirement not met ({condition})
```

**When**: An element that is required by a conditional rule (e.g. "required when
element 0 is `BY`") is absent.

**Fields**: `tag`, `element_index`, `condition: String`, `offset`.

**Fix**: Provide the conditionally required element, or remove the element that
triggered the condition.

---

### E018 — `ValidationFailed`

```
validation failed with {error_count} issue(s); first issue: {first_message}
```

**When**: `validate_strict` was called and the `ValidationReport` contained at least
one error. This wraps the entire report as a single `EdifactError`.

**Fields**: `error_count: usize`, `first_message: String`.

**Fix**: Switch to `validate_lenient` and inspect the full `ValidationReport`, or
fix all issues before calling `validate_strict`.

---

### E019 — `InvalidReleaseSequence`

```
invalid release sequence at byte offset {offset}: dangling release character
```

**When**: A release character (`?` by default) appears at the end of input with no
following byte to escape.

**Fields**: `offset: usize`.

**Fix**: Remove the trailing release character, or add the character that should be
escaped.

---

### E020 — `SegmentTooLong`

```
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

```
missing required component {component_index} of element {element_index} in segment {tag}
```

**When**: The typed deserializer expected a mandatory component within a composite
element that was present but did not contain the required component.

**Fields**: `tag: String`, `element_index: usize`, `component_index: usize`.

**Fix**: Provide the missing component inside the composite element, or mark the
field `Option<T>` if it is truly optional.

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

```rust
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
`offset` fields. See [Diagnostics](diagnostics.md) for details.

---

## Next steps

- [Diagnostics](diagnostics.md) — miette span-annotated rendering
- [Validation](validation.md) — `ValidationReport` vs `EdifactError`
- [Performance](performance.md) — error-free fast paths
