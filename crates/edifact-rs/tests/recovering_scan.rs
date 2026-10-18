//! The recovering scan: segments and faults from input the strict parser
//! refuses, the `UNB` addressing, message windows, and the `CONTRL` locations
//! of the acceptance cases.
//!
//! The acceptance cases re-create, as synthetic data, the inputs a CONTRL
//! sender built its own tokenizer for: a two-message interchange whose second
//! message does not tokenize, a trailer count mismatch, undecodable text, a
//! dangling release character, missing elements and components, and input
//! with no readable `UNB` at all.

use edifact_rs::contrl::{ReportingLevel, SyntaxError};
use edifact_rs::recover::Scanned;
use edifact_rs::{
    EdifactError, ReaderConfig, RecoveredInterchange, RecoveryConfig, from_bytes,
    from_bytes_with_config, scan_recovering,
};

/// Two `UTILMD` messages; the second message's `BGM` segment is `second_bgm`
/// (without its terminator).
fn utilmd_interchange(second_bgm: &[u8]) -> Vec<u8> {
    let mut out = b"UNB+UNOC:3+4012345000023:500+9900357000004:500+261001:0700+IC9'\
UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E03+X'UNT+3+1'\
UNH+2+UTILMD:D:11A:UN:S2.1'"
        .to_vec();
    out.extend_from_slice(second_bgm);
    out.extend_from_slice(b"'UNT+3+2'UNZ+2+IC9'");
    out
}

fn scan(input: &[u8]) -> RecoveredInterchange<'_> {
    RecoveredInterchange::scan(input, &RecoveryConfig::default())
}

/// What each item is: a tag, or `!` plus the fault's stable code.
fn shape(items: &[Scanned<'_>]) -> Vec<String> {
    items
        .iter()
        .map(|item| match item {
            Scanned::Segment(segment) => segment.tag().to_owned(),
            Scanned::Fault(fault) => format!("!{}", fault.error().stable_code()),
            _ => unreachable!("only segments and faults exist"),
        })
        .collect()
}

/// Strict parity (contract C1/C2): the segments before the first fault are the
/// strict segments, and the first fault is the strict error.
#[track_caller]
fn assert_strict_parity(input: &[u8]) {
    let strict: Vec<_> = from_bytes(input).collect();
    let items: Vec<_> = scan_recovering(input, &RecoveryConfig::default()).collect();
    for (index, result) in strict.iter().enumerate() {
        match result {
            Ok(segment) => assert_eq!(items[index].as_segment(), Some(segment), "item {index}"),
            Err(error) => {
                let fault = items[index]
                    .as_fault()
                    .expect("the strict error is a fault");
                assert_eq!(fault.error(), error, "item {index}");
            }
        }
    }
    if strict.iter().all(Result::is_ok) {
        assert_eq!(items.len(), strict.len(), "no fault for accepted input");
    }
}

// ── scan behaviour ───────────────────────────────────────────────────────────

#[test]
fn accepted_input_scans_to_the_strict_segments_and_no_fault() {
    let input = utilmd_interchange(b"BGM+E03+X");
    assert_strict_parity(&input);
    let recovered = scan(&input);
    assert!(recovered.is_complete());
    assert_eq!(recovered.faults().count(), 0);
    assert_eq!(recovered.items().len(), 8);
}

#[test]
fn the_scan_resynchronises_after_a_bad_tag() {
    let input = utilmd_interchange(b"BGME03+X");
    assert_strict_parity(&input);
    let recovered = scan(&input);
    assert_eq!(
        shape(recovered.items()),
        ["UNB", "UNH", "BGM", "UNT", "UNH", "!E006", "UNT", "UNZ"]
    );
    let fault = recovered.faults().next().expect("one fault");
    assert_eq!(fault.ordinal(), 5);
    assert!(!fault.is_terminal());
    assert_eq!(&input[fault.span().start..fault.span().end], b"BGME03+X'");
    assert!(recovered.is_complete());
}

#[test]
fn the_scan_resynchronises_after_an_empty_segment() {
    let input = b"UNH+1+ORDERS:D:96A:UN''BGM+220'UNT+3+1'";
    assert_strict_parity(input);
    assert_eq!(shape(scan(input).items()), ["UNH", "!E002", "BGM", "UNT"]);
}

#[test]
fn the_scan_resynchronises_after_invalid_text() {
    let input = b"UNH+1+ORDERS:D:96A:UN'BGM+\xFF\xFE'DTM+137'UNT+4+1'";
    assert_strict_parity(input);
    assert_eq!(shape(scan(input).items()), ["UNH", "!E003", "DTM", "UNT"]);
}

#[test]
fn a_released_terminator_is_not_a_boundary() {
    // `?'` is data: the faulted segment runs on to the next unreleased `'`.
    let input = b"UNH+1+ORDERS:D:96A:UN'BGM+\xFF?'still BGM'UNT+3+1'";
    assert_strict_parity(input);
    let recovered = scan(input);
    assert_eq!(shape(recovered.items()), ["UNH", "!E003", "UNT"]);
    let fault = recovered.faults().next().expect("fault");
    assert_eq!(
        &input[fault.span().start..fault.span().end],
        b"BGM+\xFF?'still BGM'"
    );
}

#[test]
fn a_terminal_fault_is_the_last_item() {
    let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220";
    assert_strict_parity(input);
    let recovered = scan(input);
    assert_eq!(shape(recovered.items()), ["UNH", "!E001"]);
    let fault = recovered.faults().next().expect("fault");
    assert!(fault.is_terminal());
    assert!(!recovered.is_complete());
}

#[test]
fn version_4_repetitions_resolve_as_in_the_strict_parser() {
    let input = b"UNB+UNOC:4+S+R+20260101:0900+IC1'UNH+1+ORDERS:D:96A:UN'\
RFF+ON:1*VN:2'\xFF'UNT+4+1'UNZ+1+IC1'";
    assert_strict_parity(input);
    let recovered = scan(input);
    let rff = recovered.items()[2].as_segment().expect("RFF parses");
    assert_eq!(rff.elements[0].repetitions().count(), 2);
    assert_eq!(
        shape(recovered.items()),
        ["UNB", "UNH", "RFF", "!E006", "UNT", "UNZ"]
    );
}

#[test]
fn a_bom_and_leading_whitespace_are_skipped_as_in_the_strict_parser() {
    let mut input = vec![0xEF, 0xBB, 0xBF, b'\r', b'\n'];
    input.extend_from_slice(&utilmd_interchange(b"BGME03+X"));
    assert_strict_parity(&input);
    let recovered = scan(&input);
    assert_eq!(
        recovered.items()[0].as_segment().map(|s| s.span.start),
        Some(5)
    );
    assert!(recovered.address().is_some());
}

#[test]
fn a_una_is_honoured() {
    let input =
        b"UNA:+.? 'UNB+UNOC:3+S+R+260101:0900+IC1'UNH+1+ORDERS:D:96A:UN'BGME03'UNT+3+1'UNZ+1+IC1'";
    assert_strict_parity(input);
    assert_eq!(
        shape(scan(input).items()),
        ["UNB", "UNH", "!E006", "UNT", "UNZ"]
    );
}

// ── addressing and identity ──────────────────────────────────────────────────

#[test]
fn the_unb_parties_are_read_from_a_broken_interchange() {
    let input = utilmd_interchange(b"BGME03+X");
    let recovered = scan(&input);
    let address = recovered.address().expect("addressable");
    assert_eq!(address.sender_id(), "4012345000023");
    assert_eq!(address.sender_qualifier(), "500");
    assert_eq!(address.recipient_id(), "9900357000004");
    assert_eq!(address.recipient_qualifier(), "500");
    assert_eq!(address.control_ref(), "IC9");
    assert_eq!(address.syntax_identifier(), "UNOC");
    assert_eq!(address.syntax_version(), "3");
}

#[test]
fn garbage_has_no_address_and_at_least_one_fault() {
    let recovered = scan(b"\0\x01not edifact");
    assert!(recovered.address().is_none());
    assert!(recovered.faults().count() >= 1);
    assert!(recovered.messages().is_empty());
}

#[test]
fn a_faulted_unb_still_yields_its_address_with_releases_resolved() {
    let input = b"UNB+UNOC:3+SEND?+ER:14+RECV:14+26\xFF101:0900+IC?'1'\
UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'UNZ+1+IC?'1'";
    let recovered = scan(input);
    assert_eq!(
        recovered.items()[0].as_fault().map(|f| f.raw_tag()),
        Some(Some("UNB"))
    );
    let address = recovered.address().expect("addressable");
    assert_eq!(address.sender_id(), "SEND+ER");
    assert_eq!(address.sender_qualifier(), "14");
    assert_eq!(address.recipient_id(), "RECV");
    assert_eq!(address.control_ref(), "IC'1");
}

#[test]
fn an_unreadable_control_reference_means_no_address() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+\xFF'UNZ+0+X'";
    assert!(scan(input).address().is_none());
}

#[test]
fn message_identity_comes_from_a_parsed_or_a_faulted_unh() {
    let input = utilmd_interchange(b"BGME03+X");
    let recovered = scan(&input);
    let identity = recovered.messages()[1].identity().expect("readable");
    assert_eq!(
        (
            identity.message_ref(),
            identity.message_type(),
            identity.version(),
            identity.release(),
            identity.controlling_agency(),
            identity.association_code()
        ),
        ("2", "UTILMD", "D", "11A", "UN", "S2.1")
    );

    // A faulted UNH still opens its window, and its identity is read raw.
    let input = b"UNH+7+ORDERS:D:96A:UN:A\xFF'BGM+220'UNT+3+7'";
    let recovered = scan(input);
    assert_eq!(recovered.messages().len(), 1);
    let identity = recovered.messages()[0]
        .identity()
        .expect("0062 and 0065 readable");
    assert_eq!(
        (identity.message_ref(), identity.message_type()),
        ("7", "ORDERS")
    );
    assert_eq!(identity.association_code(), "");
    assert_eq!(recovered.messages()[0].last(), Some(2));

    // Without a readable DE 0062 there is no identity — never a guess.
    let input = b"UNH+\xFF+ORDERS:D:96A:UN'BGM+220'UNT+3+1'";
    assert!(scan(input).messages()[0].identity().is_none());
}

#[test]
fn groups_are_windowed_and_messages_carry_their_group() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'\
UNG+ORDERS+SND+RCV+260101:0900+G1+UN+D:96A'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNE+1+G1'\
UNG+ORDERS+SND+RCV+260101:0900+G2+UN+D:96A'UNH+M2+ORDERS:D:96A:UN'BGM+220'UNT+3+M2'UNE+1+G2'\
UNZ+2+IC1'";
    let recovered = scan(input);
    let refs: Vec<_> = recovered.groups().iter().map(|g| g.group_ref()).collect();
    assert_eq!(refs, [Some("G1"), Some("G2")]);
    let groups: Vec<_> = recovered.messages().iter().map(|m| m.group()).collect();
    assert_eq!(groups, [Some(0), Some(1)]);
}

// ── acceptance cases: CONTRL locations ───────────────────────────────────────

#[test]
fn utilmd_interchange_bad_tag_is_message_2_segment_2_code_12_on_the_tag() {
    let input = utilmd_interchange(b"BGME03+X");
    let recovered = scan(&input);
    let locations = recovered.fault_locations();
    assert_eq!(locations.len(), 1);
    let l = &locations[0];
    assert_eq!(l.level(), ReportingLevel::DataElement);
    // Message index 1 is the message with DE 0062 `2`.
    assert_eq!(l.message(), Some(1));
    assert_eq!(
        recovered.messages()[1].identity().map(|i| i.message_ref()),
        Some("2")
    );
    assert_eq!(l.segment_position(), Some(2));
    assert_eq!(l.element_position(), Some(1));
    assert_eq!(l.component_position(), None);
    assert_eq!(l.error().code(), "12");
    assert!(l.is_exact());
}

#[test]
fn utilmd_interchange_count_mismatch_is_29_on_the_ucm_naming_unt() {
    let input = utilmd_interchange(b"BGM+E03+X");
    let recovered = scan(&input);
    // The scan itself is clean: control counts are the envelope layer's.
    assert_eq!(recovered.faults().count(), 0);
    let unt = recovered.items()[6].span();
    let error = EdifactError::SegmentCountMismatch {
        expected: 9,
        actual: 3,
        message_ref: "2".into(),
        span: unt,
    };
    let l = recovered.locate(&error, None).expect("located");
    assert_eq!(l.level(), ReportingLevel::Message);
    assert_eq!(l.message(), Some(1));
    assert_eq!(l.service_segment(), Some("UNT"));
    assert_eq!(l.segment_position(), None);
    assert_eq!(l.error(), SyntaxError::ControlCountMismatch);
}

#[test]
fn utilmd_interchange_invalid_text_is_21_at_element_3_component_1() {
    let input = utilmd_interchange(b"BGM+E03+X\xFF:A");
    let recovered = scan(&input);
    let l = &recovered.fault_locations()[0];
    assert_eq!(l.message(), Some(1));
    assert_eq!(l.segment_position(), Some(2));
    assert_eq!(l.element_position(), Some(3));
    assert_eq!(l.component_position(), Some(1));
    assert_eq!(l.error().code(), "21");
}

#[test]
fn utilmd_interchange_dangling_release_is_22_at_its_element() {
    // The input ends with a release character inside the second message.
    let mut input = utilmd_interchange(b"BGM+E03+X");
    let cut = input
        .windows(9)
        .rposition(|w| w == b"BGM+E03+X")
        .expect("BGM")
        + 9;
    input.truncate(cut);
    input.push(b'?');
    let recovered = scan(&input);
    let fault = recovered.faults().next().expect("fault");
    assert_eq!(fault.error().stable_code(), "E019");
    assert!(fault.is_terminal());
    let l = &recovered.fault_locations()[0];
    assert_eq!(
        (
            l.message(),
            l.segment_position(),
            l.element_position(),
            l.component_position()
        ),
        (Some(1), Some(2), Some(3), None)
    );
    assert_eq!(l.error().code(), "22");
    // The message never closed; no trailer was invented.
    assert_eq!(recovered.messages()[1].last(), None);
}

#[test]
fn utilmd_interchange_missing_element_and_component_land_on_the_first_segment_with_the_tag() {
    let input = utilmd_interchange(b"BGM+E03+X");
    let recovered = scan(&input);
    let element = EdifactError::MissingRequiredElement {
        tag: "BGM".into(),
        element_index: 1,
    };
    let l = recovered.locate(&element, Some(1)).expect("located");
    assert_eq!(
        (
            l.level(),
            l.message(),
            l.segment_position(),
            l.element_position(),
            l.component_position()
        ),
        (ReportingLevel::DataElement, Some(1), Some(2), Some(3), None)
    );
    assert_eq!(l.error().code(), "13");
    assert!(!l.is_exact());

    let component = EdifactError::MissingRequiredComponent {
        tag: "BGM".into(),
        element_index: 1,
        component_index: 0,
    };
    let l = recovered.locate(&component, Some(1)).expect("located");
    assert_eq!(
        (l.element_position(), l.component_position()),
        (Some(3), Some(1))
    );
    assert_eq!(l.error().code(), "13");

    // In UNH or UNT: the UCM, naming the service segment, with S011.
    for tag in ["UNH", "UNT"] {
        let error = EdifactError::MissingRequiredElement {
            tag: tag.into(),
            element_index: 1,
        };
        let l = recovered.locate(&error, Some(1)).expect("located");
        assert_eq!(l.level(), ReportingLevel::Message);
        assert_eq!(l.service_segment(), Some(tag));
        assert_eq!(l.element_position(), Some(3));
        assert_eq!(l.error().code(), "13");
    }
}

// ── bounded, never inventing content ─────────────────────────────────────────

#[test]
fn a_message_without_unt_gets_no_invented_trailer() {
    let input = b"UNH+1+ORDERS:D:96A:UN'BGM+1'UNH+2+ORDERS:D:96A:UN'BGM+2'UNT+3+2'";
    let recovered = scan(input);
    assert_eq!(
        shape(recovered.items()),
        ["UNH", "BGM", "UNH", "BGM", "UNT"]
    );
    assert_eq!(recovered.messages()[0].last(), None);
    assert_eq!(recovered.messages()[1].last(), Some(4));
    assert!(recovered.is_complete());
}

#[test]
fn a_terminal_fault_leaves_the_interchange_incomplete() {
    let input = b"UNB+UNOC:3+S+R+260101:0900+IC1'UNH+1+ORDERS:D:96A:UN'BGM+1";
    let recovered = scan(input);
    assert!(!recovered.is_complete());
    assert_eq!(
        recovered
            .items()
            .last()
            .and_then(|i| i.as_fault())
            .map(|f| f.is_terminal()),
        Some(true)
    );
}

#[test]
fn a_segment_over_the_limit_without_a_terminator_ends_the_scan() {
    // The faulted segment's terminator lies beyond `max_segment_bytes`: the
    // strict error is reported first, then the scan ends as an overlong segment.
    let mut input = b"UNH+1+ORDERS:D:96A:UN'BGM+\xFF+".to_vec();
    input.extend(std::iter::repeat_n(b'X', 200));
    input.extend_from_slice(b"'UNT+3+1'");
    let config = RecoveryConfig::new().reader(ReaderConfig::default().max_segment_bytes(64));
    let strict = from_bytes_with_config(&input, *config.reader_config())
        .find_map(Result::err)
        .expect("strict error");
    let items: Vec<_> = scan_recovering(&input, &config).collect();
    assert_eq!(
        shape(&items),
        ["UNH", &format!("!{}", strict.stable_code()), "!E020"]
    );
    assert!(items[2].as_fault().is_some_and(|f| f.is_terminal()));
}
