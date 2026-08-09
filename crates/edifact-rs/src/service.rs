//! Segment definitions for the ISO 9735 **service segments**.
//!
//! `UNB`, `UNZ`, `UNG`, `UNE`, `UNH`, `UNT`, and `UNS` are specified by the
//! syntax standard itself, not by a UN/EDIFACT directory release. There is one
//! correct answer for their structure, it does not change between D.96A and
//! D.24B, and it is not part of the licensed directory data this crate
//! deliberately does not ship — so it ships here.
//!
//! # Why this exists
//!
//! Code-addressed access ([`Segment::value_by_code`][crate::Segment::value_by_code],
//! `#[edifact(element = "0020")]`) needs a [`SegmentDefinition`] to resolve
//! against. Requiring every consumer to hand-author `UNB` before they can read
//! an interchange control reference by name made the crate's headline safety
//! feature unreachable for exactly the segments every EDIFACT program touches.
//!
//! ```
//! use edifact_rs::{from_bytes, service};
//!
//! let segments: Vec<_> =
//!     from_bytes(b"UNB+UNOC:3+SENDER+RECEIVER+260101:0900+IC4711'UNZ+0+IC4711'")
//!         .collect::<Result<Vec<_>, _>>()?;
//!
//! // DE 0020 — interchange control reference — by name, not by counting to 4.
//! assert_eq!(segments[0].value_by_code(&service::UNB, "0020")?, Some("IC4711"));
//! // Reaching into a composite works the same way: S004 component 1.
//! assert_eq!(segments[0].value_by_code(&service::UNB, "0017")?, Some("260101"));
//! # Ok::<(), edifact_rs::EdifactError>(())
//! ```
//!
//! # Scope
//!
//! Service segments only. Message-level segments (`BGM`, `DTM`, `NAD`, `LIN`, …)
//! and their composites (`C002`, `C507`, `C082`, …) belong to a **directory
//! release**: their content differs between releases, and the directories are
//! large and separately licensed. Supply those yourself as `static` tables, or
//! load them at run time with
//! [`DirectoryValidatorBuilder`][crate::DirectoryValidatorBuilder].
//!
//! `UCI`/`UCM`/`UCS`/`UCD` are not here either: they belong to the `CONTRL`
//! *message*, not to the interchange envelope.
//!
//! # Codes that genuinely repeat
//!
//! Some identifiers appear at more than one position and are therefore not
//! code-addressable — by design, because "which one" has no answer:
//!
//! | Segment | Code | Appears in |
//! |---|---|---|
//! | `UNB` | `0007` | S002 (sender) and S003 (recipient) |
//! | `UNH` | `0051` | S009, S016, S017, and S018 |
//!
//! Resolving one of those returns
//! [`EdifactError::AmbiguousDataElement`][crate::EdifactError::AmbiguousDataElement].
//! Address the unambiguous neighbour instead — `0004` for the sender, `0010` for
//! the recipient — or read the slot positionally.
//!
//! # `const`, not `static`
//!
//! Every definition here is a `const`, because `#[edifact(layout = …)]` expands
//! into a `const` initialiser and a `const` cannot read a `static`. Pointing the
//! derive straight at `service::UNB` therefore just works, with no alias in
//! between.
//!
//! # Syntax versions
//!
//! The definitions follow **syntax version 4**, which is a superset of version 3
//! for every service segment: the extra components are all conditional. A
//! version 3 interchange therefore validates cleanly against them, and a version
//! 4 one is not rejected for carrying components version 3 lacked.

use crate::directory_validator::{ComponentRef, ElementRef, SegmentDefinition, Status};

use Status::{Conditional as C, Mandatory as M};

// ── composite data elements ───────────────────────────────────────────────────

/// S001 — syntax identifier (`UNB`).
pub const S001: &[ComponentRef] = &[
    ComponentRef::new(1, "0001", M), // Syntax identifier
    ComponentRef::new(2, "0002", M), // Syntax version number
    ComponentRef::new(3, "0080", C), // Service code list directory version number
    ComponentRef::new(4, "0133", C), // Character encoding, coded
];

/// S002 — interchange sender (`UNB`).
pub const S002: &[ComponentRef] = &[
    ComponentRef::new(1, "0004", M), // Interchange sender identification
    ComponentRef::new(2, "0007", C), // Identification code qualifier
    ComponentRef::new(3, "0008", C), // Interchange sender internal identification
    ComponentRef::new(4, "0042", C), // Interchange sender internal sub-identification
];

/// S003 — interchange recipient (`UNB`).
pub const S003: &[ComponentRef] = &[
    ComponentRef::new(1, "0010", M), // Interchange recipient identification
    ComponentRef::new(2, "0007", C), // Identification code qualifier
    ComponentRef::new(3, "0014", C), // Interchange recipient internal identification
    ComponentRef::new(4, "0046", C), // Interchange recipient internal sub-identification
];

/// S004 — date and time of preparation (`UNB`, `UNG`).
pub const S004: &[ComponentRef] = &[
    ComponentRef::new(1, "0017", M), // Date
    ComponentRef::new(2, "0019", M), // Time
];

/// S005 — recipient reference / password details (`UNB`).
pub const S005: &[ComponentRef] = &[
    ComponentRef::new(1, "0022", M), // Recipient reference/password
    ComponentRef::new(2, "0025", C), // Recipient reference/password qualifier
];

/// S006 — application sender identification (`UNG`).
pub const S006: &[ComponentRef] = &[
    ComponentRef::new(1, "0040", M), // Application sender identification
    ComponentRef::new(2, "0007", C), // Identification code qualifier
];

/// S007 — application recipient identification (`UNG`).
pub const S007: &[ComponentRef] = &[
    ComponentRef::new(1, "0044", M), // Application recipient identification
    ComponentRef::new(2, "0007", C), // Identification code qualifier
];

/// S008 — message version (`UNG`).
pub const S008: &[ComponentRef] = &[
    ComponentRef::new(1, "0052", M), // Message version number
    ComponentRef::new(2, "0054", M), // Message release number
    ComponentRef::new(3, "0057", C), // Association assigned code
];

/// S009 — message identifier (`UNH`).
pub const S009: &[ComponentRef] = &[
    ComponentRef::new(1, "0065", M), // Message type
    ComponentRef::new(2, "0052", M), // Message version number
    ComponentRef::new(3, "0054", M), // Message release number
    ComponentRef::new(4, "0051", M), // Controlling agency, coded
    ComponentRef::new(5, "0057", C), // Association assigned code
    ComponentRef::new(6, "0110", C), // Code list directory version number
    ComponentRef::new(7, "0113", C), // Message type sub-function identification
];

/// S010 — status of the transfer (`UNH`).
pub const S010: &[ComponentRef] = &[
    ComponentRef::new(1, "0070", M), // Sequence of transfers
    ComponentRef::new(2, "0073", C), // First and last transfer
];

/// S016 — message subset identification (`UNH`).
pub const S016: &[ComponentRef] = &[
    ComponentRef::new(1, "0115", M), // Message subset identification
    ComponentRef::new(2, "0116", C), // Message subset version number
    ComponentRef::new(3, "0118", C), // Message subset release number
    ComponentRef::new(4, "0051", C), // Controlling agency, coded
];

/// S017 — message implementation guideline identification (`UNH`).
pub const S017: &[ComponentRef] = &[
    ComponentRef::new(1, "0121", M), // Message implementation guideline identification
    ComponentRef::new(2, "0122", C), // Message implementation guideline version number
    ComponentRef::new(3, "0124", C), // Message implementation guideline release number
    ComponentRef::new(4, "0051", C), // Controlling agency, coded
];

/// S018 — scenario identification (`UNH`).
pub const S018: &[ComponentRef] = &[
    ComponentRef::new(1, "0127", M), // Scenario identification
    ComponentRef::new(2, "0128", C), // Scenario version number
    ComponentRef::new(3, "0130", C), // Scenario release number
    ComponentRef::new(4, "0051", C), // Controlling agency, coded
];

// ── segment definitions ───────────────────────────────────────────────────────

const UNB_ELEMENTS: &[ElementRef] = &[
    ElementRef::composite(1, "S001", M, 1, S001),
    ElementRef::composite(2, "S002", M, 1, S002),
    ElementRef::composite(3, "S003", M, 1, S003),
    ElementRef::composite(4, "S004", M, 1, S004),
    ElementRef::new(5, "0020", M, 1), // Interchange control reference
    ElementRef::composite(6, "S005", C, 1, S005),
    ElementRef::new(7, "0026", C, 1),  // Application reference
    ElementRef::new(8, "0029", C, 1),  // Processing priority code
    ElementRef::new(9, "0031", C, 1),  // Acknowledgement request
    ElementRef::new(10, "0032", C, 1), // Interchange agreement identifier
    ElementRef::new(11, "0035", C, 1), // Test indicator
];

/// `UNB` — interchange header.
pub const UNB: SegmentDefinition =
    SegmentDefinition::new("UNB", "Interchange header", UNB_ELEMENTS);

const UNZ_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "0036", M, 1), // Interchange control count
    ElementRef::new(2, "0020", M, 1), // Interchange control reference
];

/// `UNZ` — interchange trailer.
pub const UNZ: SegmentDefinition =
    SegmentDefinition::new("UNZ", "Interchange trailer", UNZ_ELEMENTS);

const UNG_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "0038", M, 1), // Functional group identification
    ElementRef::composite(2, "S006", M, 1, S006),
    ElementRef::composite(3, "S007", M, 1, S007),
    ElementRef::composite(4, "S004", M, 1, S004),
    ElementRef::new(5, "0048", M, 1), // Functional group reference number
    ElementRef::new(6, "0051", M, 1), // Controlling agency, coded
    ElementRef::composite(7, "S008", M, 1, S008),
];

/// `UNG` — functional group header.
pub const UNG: SegmentDefinition =
    SegmentDefinition::new("UNG", "Functional group header", UNG_ELEMENTS);

const UNE_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "0060", M, 1), // Number of messages
    ElementRef::new(2, "0048", M, 1), // Functional group reference number
];

/// `UNE` — functional group trailer.
pub const UNE: SegmentDefinition =
    SegmentDefinition::new("UNE", "Functional group trailer", UNE_ELEMENTS);

const UNH_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "0062", M, 1), // Message reference number
    ElementRef::composite(2, "S009", M, 1, S009),
    ElementRef::new(3, "0068", C, 1), // Common access reference
    ElementRef::composite(4, "S010", C, 1, S010),
    ElementRef::composite(5, "S016", C, 1, S016),
    ElementRef::composite(6, "S017", C, 1, S017),
    ElementRef::composite(7, "S018", C, 1, S018),
];

/// `UNH` — message header.
pub const UNH: SegmentDefinition = SegmentDefinition::new("UNH", "Message header", UNH_ELEMENTS);

const UNT_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "0074", M, 1), // Number of segments in the message
    ElementRef::new(2, "0062", M, 1), // Message reference number
];

/// `UNT` — message trailer.
pub const UNT: SegmentDefinition = SegmentDefinition::new("UNT", "Message trailer", UNT_ELEMENTS);

const UNS_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "0081", M, 1), // Section identification
];

/// `UNS` — section control.
pub const UNS: SegmentDefinition = SegmentDefinition::new("UNS", "Section control", UNS_ELEMENTS);

// ── lookup ────────────────────────────────────────────────────────────────────

/// The service-segment definition for `tag`, if there is one.
///
/// Plugs straight into
/// [`DirectoryValidator::new`][crate::DirectoryValidator::new] as the
/// `segment_lookup` hook, or chains ahead of a directory-specific lookup so the
/// envelope is validated without duplicating it in every directory table.
///
/// # Example
///
/// ```
/// use edifact_rs::service;
///
/// assert_eq!(service::lookup("UNB").map(|d| d.tag), Some("UNB"));
/// assert!(service::lookup("BGM").is_none()); // a directory segment, not a service one
/// ```
#[must_use]
pub fn lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    Some(match tag {
        "UNB" => &UNB,
        "UNZ" => &UNZ,
        "UNG" => &UNG,
        "UNE" => &UNE,
        "UNH" => &UNH,
        "UNT" => &UNT,
        "UNS" => &UNS,
        _ => return None,
    })
}

/// Every service-segment definition, in envelope order.
///
/// # Example
///
/// ```
/// use edifact_rs::service;
///
/// assert_eq!(
///     service::ALL.iter().map(|d| d.tag).collect::<Vec<_>>(),
///     ["UNB", "UNG", "UNH", "UNT", "UNE", "UNZ", "UNS"],
/// );
/// ```
pub const ALL: &[&SegmentDefinition] = &[&UNB, &UNG, &UNH, &UNT, &UNE, &UNZ, &UNS];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory_validator::SegmentLayout;

    #[test]
    fn every_definition_numbers_its_positions_consecutively_from_one() {
        for definition in ALL {
            for (index, element) in definition.elements.iter().enumerate() {
                assert_eq!(
                    element.position() as usize,
                    index + 1,
                    "{}: element positions must be consecutive and one-based",
                    definition.tag,
                );
                for (component_index, component) in element.components().iter().enumerate() {
                    assert_eq!(
                        component.position() as usize,
                        component_index + 1,
                        "{} {}: component positions must be consecutive and one-based",
                        definition.tag,
                        element.data_element(),
                    );
                }
            }
        }
    }

    #[test]
    fn the_lookup_and_the_list_agree() {
        for definition in ALL {
            assert_eq!(
                lookup(definition.tag).map(|d| d.tag),
                Some(definition.tag),
                "{} is in ALL but not in lookup()",
                definition.tag,
            );
        }
        assert_eq!(ALL.len(), 7);
    }

    #[test]
    fn envelope_positions_match_the_ones_the_envelope_parser_assumes() {
        // `envelope.rs` reads these slots by hand-written index.  If the two ever
        // disagree, one of them is wrong — so pin them against each other.
        let cases: [(&SegmentDefinition, &str, usize, Option<usize>); 9] = [
            (&UNB, "0001", 0, Some(0)), // S001 comp 1 — syntax identifier
            (&UNB, "0004", 1, Some(0)), // S002 comp 1 — sender id
            (&UNB, "0010", 2, Some(0)), // S003 comp 1 — recipient id
            (&UNB, "0017", 3, Some(0)), // S004 comp 1 — date
            (&UNB, "0020", 4, None),    // interchange control reference
            (&UNB, "0035", 10, None),   // test indicator
            (&UNZ, "0036", 0, None),    // interchange control count
            (&UNH, "0062", 0, None),    // message reference number
            (&UNH, "0065", 1, Some(0)), // S009 comp 1 — message type
        ];
        for (definition, code, element, component) in cases {
            let path = definition
                .resolve_code(code)
                .unwrap_or_else(|e| panic!("{} {code}: {e}", definition.tag));
            assert_eq!(
                (path.element, path.component),
                (element, component),
                "{} {code}",
                definition.tag,
            );
        }
    }

    #[test]
    fn codes_that_genuinely_repeat_are_reported_as_ambiguous() {
        // Documented in the module header: these have no single right answer.
        assert!(
            UNB.resolve_code("0007").is_err(),
            "UNB 0007 (S002 and S003)"
        );
        assert!(
            UNH.resolve_code("0051").is_err(),
            "UNH 0051 (S009/S016/S017/S018)"
        );
        // And the neighbours that disambiguate them do resolve.
        assert!(UNB.resolve_code("0004").is_ok());
        assert!(UNB.resolve_code("0010").is_ok());
    }

    #[test]
    fn a_real_interchange_resolves_through_the_shipped_layouts() {
        let input = b"UNB+UNOC:3+SENDER:14+RECEIVER:14+260101:0900+IC4711++ORDERS'\
                      UNH+MSG1+ORDERS:D:96A:UN:EAN008'\
                      UNT+2+MSG1'UNZ+1+IC4711'";
        let segments: Vec<_> = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(
            segments[0].value_by_code(&UNB, "0020").unwrap(),
            Some("IC4711")
        );
        assert_eq!(
            segments[0].value_by_code(&UNB, "0004").unwrap(),
            Some("SENDER")
        );
        assert_eq!(
            segments[0].value_by_code(&UNB, "0010").unwrap(),
            Some("RECEIVER")
        );
        assert_eq!(
            segments[0].value_by_code(&UNB, "0019").unwrap(),
            Some("0900")
        );
        assert_eq!(
            segments[0].value_by_code(&UNB, "0026").unwrap(),
            Some("ORDERS")
        );
        assert_eq!(
            segments[1].value_by_code(&UNH, "0062").unwrap(),
            Some("MSG1")
        );
        assert_eq!(
            segments[1].value_by_code(&UNH, "0057").unwrap(),
            Some("EAN008")
        );
        assert_eq!(segments[2].value_by_code(&UNT, "0074").unwrap(), Some("2"));
        assert_eq!(segments[3].value_by_code(&UNZ, "0036").unwrap(), Some("1"));

        // A layout applied to the wrong segment is refused, not silently misread.
        assert!(segments[0].value_by_code(&UNH, "0062").is_err());
    }

    #[test]
    fn the_shipped_layouts_drive_the_directory_validator() {
        use crate::{ValidationContext, ValidationLayer};

        let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'UNZ+1+IC1'";
        let segments: Vec<_> = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        let validator = crate::DirectoryValidator::new(
            "iso-9735-service",
            lookup,
            |_, _| true,
            |_, _| None,
            |_, _| None,
            None,
        );
        let report = ValidationContext::builder()
            .with_validator(ValidationLayer::Structure, validator)
            .build()
            .validate_lenient(&segments);

        assert!(!report.has_errors(), "{:#?}", report.errors());
    }
}
