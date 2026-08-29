+++
title = "CONTRL Acknowledgements"
description = "Generate the ISO 9735-4 syntax and service report message from a validation report — receipt, acknowledgement, and rejection with per-segment error placement."
weight = 85
+++

`CONTRL` is how you tell a partner what happened to their interchange: that it
arrived, that it was syntactically accepted, or that it was rejected and exactly
where. It is defined by **ISO 9735-4**, part of the syntax standard rather than
of a directory release — so, like the envelope segments, there is one correct
answer and no version to choose.

`edifact-rs` builds one from a [`ValidationReport`](@/docs/validation.md), which
means the acknowledgement you send and the findings you logged cannot disagree.

---

## What CONTRL does and does not say

ISO 9735-4 §5.2 is explicit: acknowledging an interchange with `CONTRL` reports
on **syntax**, not on content. A `CONTRL` saying "acknowledged" means the
interchange parsed and its envelope was consistent. It does not mean anyone
agreed to the order.

Application-level replies — `APERAK`, `ORDRSP`, and the rest — are separate
messages and are not this crate's business.

---

## The two messages

§5.3.1 allows at most two `CONTRL` messages per subject interchange:

| | Action code | When | Built with |
|---|---|---|---|
| **Receipt** | `8` | Immediately, before checking | `Contrl::receipt` |
| **Acknowledgement** | `7` | After a clean syntax check | `Contrl::acknowledgement` |
| **Rejection** | `4` | After a failed syntax check | `Contrl::from_report` |

The receipt is optional. The second message is mandatory whenever the subject
interchange asked for an acknowledgement in `UNB` DE 0031 — check
`interchange.ack_requested()` before deciding whether you owe one.

```rust
use edifact_rs::{Contrl, from_bytes, validate_envelope};

let raw = b"UNB+UNOC:3+SENDER+RECEIVER+260101:0900+IC4711'\
            UNH+MSG1+ORDERS:D:96A:UN'BGM+220+PO-1+9'UNT+3+MSG1'\
            UNZ+1+IC4711'";
let segments: Vec<_> = from_bytes(raw).collect::<Result<Vec<_>, _>>()?;
let validated = validate_envelope(&segments)?;

let wire = Contrl::acknowledgement(&validated)
    .with_message_reference("ACK1")
    .to_edifact_string()?;

assert_eq!(
    wire,
    "UNH+ACK1+CONTRL:4:1:UN'UCI+IC4711+SENDER+RECEIVER+7'UNT+3+ACK1'",
);
# Ok::<(), edifact_rs::EdifactError>(())
```

A clean interchange needs no `UCM` per message: §5.3.4 makes the `UCI`'s
acknowledgement cover every message inside it, and a redundant `UCM` adds
nothing.

---

## Reporting a failed check

`Contrl::from_report` takes the subject interchange, its segments, and the
report. The segments are what turn a finding's byte span into the segment
position `UCS` DE 0096 needs, so errors land where they happened instead of
piling up on the interchange.

Use the **lenient** path: it is the only one that yields both an interchange and
its faults, which is precisely what a `CONTRL` reports.

```rust
use edifact_rs::{Contrl, ValidationContext, from_bytes, validate_envelope_lenient};

// `FTX+   ` carries a value of nothing but spaces — ISO 9735-1 §9.3.
let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
            UNH+M1+ORDERS:D:96A:UN'FTX+   'UNT+3+M1'\
            UNZ+1+IC1'";
let segments: Vec<_> = from_bytes(raw).collect::<Result<Vec<_>, _>>()?;

let validated = validate_envelope_lenient(&segments).interchange.unwrap();
let report = ValidationContext::builder()
    .with_envelope_validation()
    .with_syntax_validation()
    .build()
    .validate(&segments);

let wire = Contrl::from_report(&validated, &segments, &report).to_edifact_string()?;

// FTX is segment 2 of the message (UNH is 1); the value is data element
// position 2 (the tag is position 1), component 1.
assert!(wire.contains("UCS+2'"));
assert!(wire.contains("UCD+12+2:1'"));
# Ok::<(), edifact_rs::EdifactError>(())
```

Warnings are reported but do not reject: the interchange above stays
acknowledged (`7`) while still naming the offending value. §5.3.3 allows exactly
that — "errors may be reported even if the referenced-level is acknowledged".

---

## Where a finding lands

The message nests five reporting levels, each naming a part of the subject
interchange:

```text
UCI   the interchange          → UNB / UNZ
 UCF  a group                  → UNG / UNE
  UCM a message or package     → UNH / UNT
   UCS a segment               → by position, UNH = 1
    UCD a data element         → by position within that segment
```

Which levels appear follows the subject interchange. §5.3.1 makes segment groups
1 and 3 mutually exclusive: an interchange that uses `UNG`/`UNE` is reported
through `UCF`, and one that does not gets `UCM` directly under the `UCI`. There
is nothing to configure — `from_report` reads the shape off the subject.

§5.3.3 asks for the **lowest** level that can express a fault, and forbids
repeating the code further up. But Annex A decides which level may carry which
code, and the two rules can pull in opposite directions: "duplicate detected"
(`26`) is meaningless below the message, so pushing it down would emit a `UCS`
that says a segment is wrong without saying how.

`edifact-rs` resolves that the way the standard requires — the lowest level that
is *both* able to locate the fault and permitted to carry the code:

```rust
use edifact_rs::contrl::{ReportingLevel, SyntaxError};

// Legal everywhere, so it descends as far as the finding can locate it.
assert!(SyntaxError::InvalidValue.permitted_at(ReportingLevel::DataElement));

// Message level and above only — a UCS could not carry it.
assert!(SyntaxError::DuplicateDetected.permitted_at(ReportingLevel::Message));
assert!(!SyntaxError::DuplicateDetected.permitted_at(ReportingLevel::Segment));

// Segment level only.
assert!(SyntaxError::TooManyGroupRepetitions.permitted_at(ReportingLevel::Segment));
assert!(!SyntaxError::TooManyGroupRepetitions.permitted_at(ReportingLevel::Interchange));
# Ok::<(), edifact_rs::EdifactError>(())
```

`with_interchange_error` applies the same rule: a code Annex A forbids at the
interchange level is dropped rather than written, because a `CONTRL` carrying an
illegal code is one the partner's translator rejects.

### Groups

A fault in a group's own `UNG`/`UNE` rejects that group and, implicitly,
everything in it — while the interchange around it stays acknowledged:

```rust
# use edifact_rs::{Contrl, ValidationContext, from_bytes, validate_envelope_lenient};
// The UNE reference does not match its UNG.
let raw = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
            UNG+ORDERS+SND+RCV+260101:0900+GRP1+UN+D:96A'\
            UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'\
            UNE+1+GRP-OTHER'\
            UNZ+1+IC1'";
let segments: Vec<_> = from_bytes(raw).collect::<Result<Vec<_>, _>>()?;
let validated = validate_envelope_lenient(&segments).interchange.unwrap();
let report = ValidationContext::builder()
    .with_envelope_validation()
    .build()
    .validate(&segments);

let wire = Contrl::from_report(&validated, &segments, &report).to_edifact_string()?;

// UCI stays at 7; the UCF carries the rejection and code 28.
assert_eq!(
    wire,
    "UNH+IC1+CONTRL:4:1:UN'UCI+IC1+S+R+7'UCF+GRP1+SND+RCV+4+28'UNT+4+IC1'",
);
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Error-code mapping

Every `EdifactError` maps to the narrowest Annex A code that is true of it.
§5.3.3 asks for exactly that: "if a precise error code is defined, a more general
(and imprecise) error code should not be used."

| Finding | `CONTRL` code |
|---|---|
| `MessageCountMismatch`, `SegmentCountMismatch` | `29` control count does not match |
| `QualifierMismatch` (control references) | `28` references do not match |
| `DuplicateReference` | `26` duplicate detected |
| `MissingRequiredElement`, `MissingSegment` | `13` missing |
| `InvalidCodeValue`, `BlankDataElementValue` | `12` invalid value |
| `InvalidSegmentForMessage` | `15` not supported in this position |
| `InvalidElementCount`, `InvalidComponentCount` | `16` too many constituents |
| `UnrecognisedSyntaxIdentifier`, `UnsupportedCharset` | `46` character set not supported |
| `CharacterNotInRepertoire`, `InvalidText` | `21` invalid character(s) |
| `InvalidUna`, `InvalidDelimiter` | `22` invalid service character(s) |
| `EmptyInterchange`, `EmptyMessage` | `32` lower level empty |
| `PackageNotSupported` | `47` envelope functionality not supported |
| anything else | `18` unspecified error |

`SyntaxError::for_issue` does the same job from a `ValidationIssue`'s stable
code, so findings from **any** validator map correctly — including third-party
ones that know nothing about `CONTRL`. An issue with no stable code, such as a
profile-rule finding, becomes `18`, which is what the annex provides for a fault
it does not name.

---

## Sending it

A `CONTRL` must travel in an interchange of its own and never inside a group
(§5.3). `to_interchange_bytes` builds that envelope, swapping sender and
recipient so the reply goes back the way it came:

```rust
# use edifact_rs::{Contrl, from_bytes, validate_envelope};
# let raw = b"UNB+UNOC:3+SENDER+RECEIVER+260101:0900+IC4711'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNZ+1+IC4711'";
# let segments: Vec<_> = from_bytes(raw).collect::<Result<Vec<_>, _>>()?;
# let validated = validate_envelope(&segments)?;
let wire = Contrl::acknowledgement(&validated)
    .to_interchange_string("UNOC", "3", "260101", "0930", "ACK-1")?;

assert!(wire.starts_with("UNB+UNOC:3+RECEIVER+SENDER+260101:0930+ACK-1'"));
assert!(wire.ends_with("UNZ+1+ACK-1'"));
# Ok::<(), edifact_rs::EdifactError>(())
```

`UNT` DE 0074 and `UNZ` DE 0036 are computed from what was actually emitted, so
the counts are right by construction.

---

## Parsing an inbound CONTRL

The same layouts serve the read direction. A `CONTRL` is built entirely from
segments in `edifact_rs::service`, so `service::lookup` validates one with no
directory involved:

```rust
use edifact_rs::{DirectoryValidator, ValidationContext, ValidationLayer, from_bytes, service};

let raw = b"UNH+ACK1+CONTRL:4:1:UN'UCI+IC4711+SENDER+RECEIVER+7'UNT+3+ACK1'";
let segments: Vec<_> = from_bytes(raw).collect::<Result<Vec<_>, _>>()?;

// Read the verdict by data element identifier, not by counting to element 3.
assert_eq!(segments[1].value_by_code(&service::UCI, "0083")?, Some("7"));
assert_eq!(segments[1].value_by_code(&service::UCI, "0020")?, Some("IC4711"));

let validator = DirectoryValidator::new(
    "iso-9735-4", service::lookup, |_, _| true, |_, _| None, |_, _| None, None,
);
let report = ValidationContext::builder()
    .with_validator(ValidationLayer::Structure, validator)
    .build()
    .validate(&segments);
assert!(!report.has_errors());
# Ok::<(), edifact_rs::EdifactError>(())
```

---

## Scope

This module generates and reads the `CONTRL` **message**. It does not implement
the two other service messages of the ISO 9735 family — `AUTACK` (part 6) and
`KEYMAN` (part 9) — which belong to the security parts this crate does not
cover.

`UNO`…`UNP` **packages** are reported rather than acknowledged: their object is
arbitrary binary data this crate does not tokenize, so a subject interchange
carrying one raises `E044` and maps to code `47`, "envelope functionality not
supported" — which is the honest answer rather than a silent acknowledgement.

---

## Further reading

- [Validation](@/docs/validation.md) — producing the report a `CONTRL` reports on
- [Error Reference](@/docs/error-reference.md) — every stable code and what it means
- [Writing](@/docs/writing.md) — the `Writer` that renders the result
