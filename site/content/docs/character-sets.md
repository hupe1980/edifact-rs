+++
title = "Character Sets"
description = "Decode UNOA-UNOK and UNOY interchanges, encode them back, and validate a payload against the repertoire its own UNB declares."
weight = 25
+++

# Character Sets

An EDIFACT interchange says which character repertoire it is written in:
`UNB` S001 component 1, the **syntax identifier**. The service characters,
segment tags, and that identifier itself are ASCII in every repertoire, so the
header can always be read before the encoding is known.

## The problem this solves

**UTF-8 is not a superset of `UNOC`.** `UNOC` is ISO 8859-1, where `ü` is the
single byte `0xFC`. That byte is not valid UTF-8. A German `ORDERS` carrying
`Müller` is a perfectly conformant `UNOC` interchange that a UTF-8-only reader
rejects outright:

```rust
use edifact_rs::from_bytes;

let mut raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'NAD+BY+M".to_vec();
raw.push(0xFC);                              // `ü` in ISO 8859-1
raw.extend_from_slice(b"ller'UNZ+0+IC1'");

// Straight through the parser: E003, invalid text.
assert!(from_bytes(&raw).collect::<Result<Vec<_>, _>>().is_err());
```

The same holds for every `UNOD`…`UNOK` interchange. The rest of `edifact-rs` is
UTF-8 throughout, so the fix is to decode at the boundary.

## Decoding a whole interchange

`decode_interchange` reads the repertoire out of the interchange's own `UNB`
and converts the payload to UTF-8:

```rust
use edifact_rs::{decode_interchange, from_bytes};

let mut raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'NAD+BY+M".to_vec();
raw.push(0xFC);
raw.extend_from_slice(b"ller'UNZ+0+IC1'");

let utf8 = decode_interchange(&raw)?;
let segments: Vec<_> = from_bytes(&utf8).collect::<Result<Vec<_>, _>>()?;

assert_eq!(segments[1].element_str(1), Some("Müller"));
# Ok::<(), edifact_rs::EdifactError>(())
```

It is safe to call unconditionally. An ASCII payload, a `UNOY` (UTF-8) payload,
and an input with no `UNB` at all are returned **borrowed** — nothing is copied,
and the zero-copy path through `from_bytes` is preserved:

```rust
use edifact_rs::decode_interchange;
use std::borrow::Cow;

let ascii = b"UNB+UNOC:3+S+R+260101:0900+IC1'BGM+220'UNZ+0+IC1'";
assert!(matches!(decode_interchange(ascii)?, Cow::Borrowed(_)));
# Ok::<(), edifact_rs::EdifactError>(())
```

> **Spans index the decoded buffer.** When transcoding actually happens, one
> `0xFC` becomes two bytes, so offsets shift. Every `Span` produced downstream
> points into the buffer `decode_interchange` returned — the one you are holding,
> and the one diagnostics render against.

## Decoding a stream

Buffering a multi-gigabyte interchange just to transcode it would undo the
crate's constant-memory guarantee. `decode_reader` buffers only the first few
kilobytes — enough to find the `UNB` — reads the repertoire out of it, and
streams the rest:

```rust
use edifact_rs::{decode_reader, from_reader_collect};

let mut raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'NAD+BY+M".to_vec();
raw.push(0xFC);
raw.extend_from_slice(b"ller'UNZ+0+IC1'");

let segments = from_reader_collect(decode_reader(std::io::Cursor::new(raw))?)?;

assert_eq!(segments[1].element_str(1), Some("Müller"));
# Ok::<(), edifact_rs::EdifactError>(())
```

Like `decode_interchange`, it is safe to wrap around any stream: a UTF-8
interchange and one with no `UNB` at all pass through unchanged.

When the repertoire is already known — from the trading-partner agreement, say —
`Charset::decoding_reader` skips the probe:

```rust
use edifact_rs::{Charset, from_reader_collect};

# let mut raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'NAD+BY+M".to_vec();
# raw.push(0xFC);
# raw.extend_from_slice(b"ller'UNZ+0+IC1'");
let reader = Charset::UnoC.decoding_reader(std::io::Cursor::new(raw));
let segments = from_reader_collect(reader)?;
# assert_eq!(segments[1].element_str(1), Some("Müller"));
# Ok::<(), edifact_rs::EdifactError>(())
```

## Writing

The write side is symmetric, and getting it wrong is the failure that is hardest
to diagnose: a `UNOC` header with a UTF-8 body arrives as mojibake with nothing
in the interchange to explain why. Bind the writer to the repertoire:

```rust
use edifact_rs::{Charset, Writer};

let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoC);
writer.write_composites("NAD", &[&["BY"], &["Müller"]])?;

// `ü` goes out as the single Latin-1 byte 0xFC.
assert_eq!(writer.finish()?, b"NAD+BY+M\xFCller'".to_vec());
# Ok::<(), edifact_rs::EdifactError>(())
```

A character the repertoire cannot carry is refused
([`E038`](@/docs/error-reference.md#e038-characternotinrepertoire)) rather than
written as something the receiver reads differently:

```rust
use edifact_rs::{Charset, EdifactError, Writer};

let mut writer = Writer::new(Vec::new()).with_charset(Charset::UnoA);
// Level A is upper-case only.
let err = writer.write_composites("NAD", &[&["BY"], &["Müller"]]).unwrap_err();
assert!(matches!(err, EdifactError::CharacterNotInRepertoire { .. }));
```

And a `UNB` that declares a repertoire the writer does not encode is refused too
([`E041`](@/docs/error-reference.md#e041-characterrepertoiremismatch)), so the
header cannot lie about the body.

## Validating the payload against the header

Partners enforce the repertoire, and they do it *after* you have sent the file. A
`UNOA` interchange carrying a lower-case letter is conformant-looking and will
still be rejected. `CharsetValidator` checks it locally:

```rust
use edifact_rs::{ValidationContext, from_bytes};

// UNOA is upper-case only, but the party name is mixed case.
let segments: Vec<_> =
    from_bytes(b"UNB+UNOA:3+S+R+260101:0900+IC1'NAD+BY+Acme Ltd'UNZ+0+IC1'")
        .collect::<Result<_, _>>()?;

let report = ValidationContext::builder()
    .with_charset_validation()   // reads the repertoire from UNB S001
    .build()
    .validate_lenient(&segments);

let issue = report.errors().iter().find(|i| i.error_code() == Some("E038")).unwrap();
assert_eq!(issue.segment_tag.as_deref(), Some("NAD"));
# Ok::<(), edifact_rs::EdifactError>(())
```

Use `with_charset_validation_for(Charset::UnoA)` to pin a repertoire instead —
for message-level slices that carry no `UNB`, or to hold a partner to something
stricter than they declare.

**Decoding is permissive; validation is strict.** `decode_interchange` treats
`UNOA` and `UNOB` as ASCII rather than enforcing their restricted repertoires,
because real interchanges routinely carry a character or two outside level A and
refusing to *parse* them would hide every other finding behind an encoding error.
The repertoire check is a separate, suppressible validation issue.

## The repertoires

| Identifier | Encoding | Notes |
|---|---|---|
| `UNOA` | ISO 9735 level A | `A`–`Z`, `0`–`9`, space, `.,-()/='+:?!"%&*;<>` |
| `UNOB` | ISO 9735 level B | Level A plus `a`–`z` |
| `UNOC` | ISO 8859-1 | Latin-1 — by far the most common non-ASCII repertoire |
| `UNOD` | ISO 8859-2 | Latin-2, Central European |
| `UNOE` | ISO 8859-5 | Latin/Cyrillic |
| `UNOF` | ISO 8859-7 | Latin/Greek |
| `UNOG` | ISO 8859-3 | Latin-3, South European |
| `UNOH` | ISO 8859-4 | Latin-4, North European |
| `UNOI` | ISO 8859-6 | Latin/Arabic |
| `UNOJ` | ISO 8859-8 | Latin/Hebrew |
| `UNOK` | ISO 8859-9 | Latin-5, Turkish |
| `UNOY` | ISO 10646-1 | UTF-8 — the identity transform |

`UNOX` (ISO 2022 code extension) and `KECA` (Korean) are **not** supported. Both
are stateful or multi-byte in ways that break the byte-level delimiter scanning
every other repertoire permits — a `+` byte inside a multi-byte sequence would be
mistaken for an element separator. They report
[`E039`](@/docs/error-reference.md#e039-unsupportedcharset) rather than being
silently mis-decoded.

Bytes that a repertoire leaves undefined (ISO 8859-6 has 45 such slots) are an
error, not a substituted replacement character:

```rust
use edifact_rs::{Charset, EdifactError};

// 0xA1 is undefined in ISO 8859-6.
let err = Charset::UnoI.decode(b"AB\xA1CD").unwrap_err();
assert!(matches!(err, EdifactError::InvalidText { offset: 2 }));
```

## Checking a value before you build a message

`Charset::permits` and `Charset::first_violation` answer the question without
writing anything, which is what you want when the fix is to transliterate rather
than to fail:

```rust
use edifact_rs::Charset;

assert!(Charset::UnoA.permits('A'));
assert!(!Charset::UnoA.permits('a'));

// Byte offset of the first character that would be rejected.
assert_eq!(Charset::UnoA.first_violation("ORDER"), None);
assert_eq!(Charset::UnoA.first_violation("ORDer"), Some((3, 'e')));
```

---

## Further reading

- [Core Concepts](@/docs/core-concepts.md) — the wire format and the UNA service string
- [Writing](@/docs/writing.md) — the `Writer` and delimiter escaping
- [Validation](@/docs/validation.md) — the layered validation pipeline
- [Error Reference](@/docs/error-reference.md) — `E038`, `E039`, `E041`

