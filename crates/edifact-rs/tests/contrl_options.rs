//! A configurable `CONTRL`: its own message identifier, the action codes each
//! level may carry, how far a rejection reaches, warnings, the service segment
//! and S011 on `UCI`/`UCF`/`UCM`, and the `CONTRL` for an interchange that does
//! not parse.
//!
//! The BDEW values used here (`CONTRL:D:3:UN:2.0b`, `UCI` {7, 4}, `UCM` {4},
//! whole-file rejection, no warnings) come from the BDEW CONTRL MIG 2.0b and the
//! expected bytes of a hand renderer for the same subjects. They are caller
//! configuration and appear only in tests and documentation, never in the
//! library. The interchanges are synthetic.

use edifact_rs::contrl::{
    Action, ActionPolicy, Contrl, ContrlIdentifier, ContrlOptions, RejectionScope, ReportingLevel,
};
use edifact_rs::{
    EdifactError, OwnedSegment, RecoveredInterchange, RecoveryConfig, ValidatedInterchange,
    ValidationContext, ValidationReport, from_bytes, validate_envelope, validate_envelope_lenient,
};

// ── fixtures ─────────────────────────────────────────────────────────────────

/// The BDEW subject's `UNB`, syntax version 3.
const UNB_3: &str = "UNB+UNOC:3+4012345000023:500+9900357000004:500+261001:0700+IC9'";
/// The same subject as syntax version 4 (`CCYYMMDD` date).
const UNB_4: &str = "UNB+UNOC:4+4012345000023:500+9900357000004:500+20261001:0700+IC9'";
/// A clean message with an association assigned code (DE 0057 `S2.1`).
const MSG_1: &str = "UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E03+X'UNT+3+1'";
/// The `UCI` every BDEW case starts with, up to (not including) DE 0083.
const UCI: &str = "UCI+IC9+4012345000023:500+9900357000004:500";

/// `unb`, `body`, and a `UNZ` counting `count` messages with reference `IC9`.
fn interchange(unb: &str, body: &str, count: usize) -> String {
    format!("{unb}{body}UNZ+{count}+IC9'")
}

fn parse(raw: &str) -> Vec<OwnedSegment> {
    from_bytes(raw.as_bytes())
        .map(|r| r.map(|s| s.into_owned()))
        .collect::<Result<Vec<_>, _>>()
        .expect("subject parses")
}

/// The subject, its lenient envelope and the envelope + syntax report.
fn subject(raw: &str) -> (Vec<OwnedSegment>, ValidatedInterchange, ValidationReport) {
    let segments = parse(raw);
    let validated = validate_envelope_lenient(&segments)
        .interchange
        .expect("subject is structurally interpretable");
    let report = ValidationContext::builder()
        .with_envelope_validation()
        .with_syntax_validation()
        .build()
        .validate(&segments);
    (segments, validated, report)
}

fn wire(contrl: Contrl) -> String {
    contrl
        .with_message_reference("ACK1")
        .to_edifact_string()
        .expect("render")
}

fn report_with(raw: &str, options: &ContrlOptions) -> Result<Contrl, EdifactError> {
    let (segments, validated, report) = subject(raw);
    Contrl::from_report_with(&validated, &segments, &report, options)
}

fn report_default(raw: &str) -> String {
    let (segments, validated, report) = subject(raw);
    wire(Contrl::from_report(&validated, &segments, &report))
}

/// The BDEW configuration — caller code, as a market participant writes it.
fn bdew() -> ContrlOptions {
    ContrlOptions::new()
        .identifier(
            ContrlIdentifier::syntax_3()
                .with_association("2.0b")
                .expect("an..6"),
        )
        .actions(
            ActionPolicy::new()
                .permit(
                    ReportingLevel::Interchange,
                    &[Action::Acknowledged, Action::Rejected],
                )
                .permit(ReportingLevel::Message, &[Action::Rejected]),
        )
        .rejection(RejectionScope::Interchange)
        .report_warnings(false)
}

/// Every `CONTRL` must reparse and pass the crate's own envelope rules.
fn assert_wellformed(contrl: &Contrl) {
    let bytes = contrl
        .to_interchange_bytes("UNOC", "3", "261001", "0800", "R1")
        .expect("render");
    let segments: Vec<_> = from_bytes(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .expect("CONTRL reparses");
    validate_envelope(&segments).expect("CONTRL passes validate_envelope");
}

// ── the header follows the subject, or the caller ────────────────────────────

#[test]
fn syntax_versions_1_to_3_are_answered_with_d_3_un() {
    for version in ["1", "2", "3"] {
        let raw = interchange(
            &format!("UNB+UNOA:{version}+S+R+261001:0700+IC9'"),
            "UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'",
            1,
        );
        let recovered = RecoveredInterchange::scan(raw.as_bytes(), &RecoveryConfig::default());
        let contrl =
            Contrl::from_recovered(&recovered, &[], &ContrlOptions::default()).expect("addressed");
        assert_eq!(
            contrl.identifier(),
            &ContrlIdentifier::syntax_3(),
            "{version}"
        );
        assert!(
            wire(contrl).starts_with("UNH+ACK1+CONTRL:D:3:UN'"),
            "{version}"
        );
    }
}

#[test]
fn syntax_version_3_derives_d_3_un_from_a_validation_run() {
    let raw = interchange(UNB_3, MSG_1, 1);
    let segments = parse(&raw);
    let validated = validate_envelope(&segments).expect("valid");
    assert_eq!(
        wire(Contrl::acknowledgement(&validated)),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+7'UNT+3+ACK1'")
    );
}

#[test]
fn syntax_version_4_is_answered_with_4_1_un() {
    let raw = interchange(UNB_4, MSG_1, 1);
    let segments = parse(&raw);
    let validated = validate_envelope(&segments).expect("valid");
    assert_eq!(
        wire(Contrl::acknowledgement(&validated)),
        format!("UNH+ACK1+CONTRL:4:1:UN'{UCI}+7'UNT+3+ACK1'")
    );
}

#[test]
fn an_unknown_or_empty_syntax_version_is_answered_with_4_1_un() {
    for unb in [
        "UNB+UNOC:9+S+R+261001:0700+IC9'",
        "UNB+UNOC+S+R+261001:0700+IC9'",
    ] {
        let raw = interchange(unb, "UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'", 1);
        let recovered = RecoveredInterchange::scan(raw.as_bytes(), &RecoveryConfig::default());
        let contrl =
            Contrl::from_recovered(&recovered, &[], &ContrlOptions::default()).expect("addressed");
        assert!(wire(contrl).starts_with("UNH+ACK1+CONTRL:4:1:UN'"), "{unb}");
    }
    assert_eq!(
        ContrlIdentifier::for_syntax_version(""),
        ContrlIdentifier::syntax_4()
    );
    assert_eq!(
        ContrlIdentifier::for_syntax_version("9"),
        ContrlIdentifier::syntax_4()
    );
}

#[test]
fn bdew_acknowledgement_is_byte_exact() {
    let raw = interchange(UNB_3, MSG_1, 1);
    let segments = parse(&raw);
    let validated = validate_envelope(&segments).expect("valid");
    let contrl = Contrl::acknowledgement_with(&validated, &bdew()).expect("7 is permitted");
    assert_wellformed(&contrl);
    assert_eq!(
        wire(contrl),
        "UNH+ACK1+CONTRL:D:3:UN:2.0b'UCI+IC9+4012345000023:500+9900357000004:500+7'UNT+3+ACK1'"
    );
}

#[test]
fn an_invalid_identifier_names_its_data_element() {
    let too_long = ContrlIdentifier::syntax_3().with_association("2.0b-xx");
    assert!(matches!(
        too_long,
        Err(EdifactError::InvalidContrlIdentifier { data_element: "0057", ref value }) if value == "2.0b-xx"
    ));
    assert!(matches!(
        ContrlIdentifier::syntax_3().with_association(""),
        Err(EdifactError::InvalidContrlIdentifier {
            data_element: "0057",
            ..
        })
    ));
    assert!(matches!(
        ContrlIdentifier::new("", "3", "UN"),
        Err(EdifactError::InvalidContrlIdentifier {
            data_element: "0052",
            ..
        })
    ));
    assert!(matches!(
        ContrlIdentifier::new("D", "3333", "UN"),
        Err(EdifactError::InvalidContrlIdentifier {
            data_element: "0054",
            ..
        })
    ));
    assert!(matches!(
        ContrlIdentifier::new("D", "3", ""),
        Err(EdifactError::InvalidContrlIdentifier {
            data_element: "0051",
            ..
        })
    ));
    let id = ContrlIdentifier::new("D", "3", "UN")
        .and_then(|id| id.with_association("EAN005"))
        .expect("valid");
    assert_eq!(
        (id.version(), id.release(), id.agency(), id.association()),
        ("D", "3", "UN", Some("EAN005"))
    );
}

#[test]
fn an_explicit_syntax_4_identifier_overrides_the_header_of_a_v3_subject() {
    // An explicit identifier wins over the subject's syntax version, through
    // the options and on a built CONTRL.
    let raw = interchange(UNB_3, MSG_1, 1);
    let segments = parse(&raw);
    let validated = validate_envelope(&segments).expect("valid");
    let options = ContrlOptions::new().identifier(ContrlIdentifier::syntax_4());
    let via_options = Contrl::acknowledgement_with(&validated, &options).expect("permitted");
    assert!(wire(via_options).starts_with("UNH+ACK1+CONTRL:4:1:UN'"));
    let via_builder =
        Contrl::acknowledgement(&validated).with_identifier(ContrlIdentifier::syntax_4());
    assert_eq!(via_builder.identifier(), &ContrlIdentifier::syntax_4());
    assert!(wire(via_builder).starts_with("UNH+ACK1+CONTRL:4:1:UN'"));
}

// ── action policy and rejection scope ────────────────────────────────────────

/// Two messages; the second's `UNT` count is wrong.
fn second_message_faulty(unb: &str) -> String {
    interchange(
        unb,
        &format!("{MSG_1}UNH+2+UTILMD:D:11A:UN:S2.1'BGM+E03+Y'UNT+9+2'"),
        2,
    )
}

#[test]
fn a_receipt_under_a_7_4_policy_is_refused() {
    let raw = interchange(UNB_3, MSG_1, 1);
    let segments = parse(&raw);
    let validated = validate_envelope(&segments).expect("valid");
    let result = Contrl::receipt_with(&validated.interchange, &bdew());
    assert!(matches!(
        result,
        Err(EdifactError::ActionNotPermitted {
            segment_tag: "UCI",
            action: "8"
        })
    ));
}

#[test]
fn a_warning_only_ucm_is_refused_under_a_4_only_policy_unless_warnings_are_off() {
    // `FTX+   ` is a warning (ISO 9735-1 §9.3): the UCM it yields carries `7`.
    let raw = interchange(UNB_3, "UNH+1+ORDERS:D:96A:UN'FTX+   'UNT+3+1'", 1);
    let policy = ActionPolicy::new().permit(ReportingLevel::Message, &[Action::Rejected]);
    let warnings_on = ContrlOptions::new().actions(policy);
    assert!(matches!(
        report_with(&raw, &warnings_on),
        Err(EdifactError::ActionNotPermitted {
            segment_tag: "UCM",
            action: "7"
        })
    ));
    let warnings_off = warnings_on.report_warnings(false);
    let contrl = report_with(&raw, &warnings_off).expect("nothing left to report");
    assert_eq!(
        wire(contrl),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+7'UNT+3+ACK1'")
    );
}

#[test]
fn a_policy_that_forbids_rejecting_the_interchange_is_loud() {
    let raw = format!("{UNB_3}{MSG_1}UNZ+1+IC-OTHER'");
    let policy = ActionPolicy::new().permit(ReportingLevel::Interchange, &[Action::Acknowledged]);
    assert!(matches!(
        report_with(&raw, &ContrlOptions::new().actions(policy)),
        Err(EdifactError::ActionNotPermitted {
            segment_tag: "UCI",
            action: "4"
        })
    ));
}

#[test]
fn whole_interchange_scope_rejects_the_uci_and_names_only_the_faulty_message() {
    let raw = second_message_faulty(UNB_3);
    let options = ContrlOptions::new().rejection(RejectionScope::Interchange);
    let contrl = report_with(&raw, &options).expect("default policy");
    assert_eq!(contrl.action(), Action::Rejected);
    assert_wellformed(&contrl);
    assert_eq!(
        wire(contrl),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+4'UCM+2+UTILMD:D:11A:UN:S2.1+4+29+UNT'UNT+4+ACK1'")
    );
}

#[test]
fn per_level_scope_keeps_the_uci_acknowledged() {
    let raw = second_message_faulty(UNB_3);
    let contrl = report_with(&raw, &ContrlOptions::default()).expect("default policy");
    assert_eq!(contrl.action(), Action::Acknowledged);
    assert_eq!(
        wire(contrl),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+7'UCM+2+UTILMD:D:11A:UN:S2.1+4+29+UNT'UNT+4+ACK1'")
    );
}

#[test]
fn an_interchange_level_fault_reports_nothing_below_under_either_scope() {
    // The UNZ reference is wrong *and* a message count is wrong: the UCI's own
    // rejection covers everything, so no UCM follows.
    let raw = format!("{UNB_3}{MSG_1}UNH+2+UTILMD:D:11A:UN:S2.1'BGM+E03+Y'UNT+9+2'UNZ+2+IC-OTHER'");
    for scope in [RejectionScope::PerLevel, RejectionScope::Interchange] {
        let contrl = report_with(&raw, &ContrlOptions::new().rejection(scope)).expect("permitted");
        assert_eq!(
            wire(contrl),
            format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+4+28+UNZ'UNT+3+ACK1'"),
            "{scope:?}"
        );
    }
}

#[test]
fn a_grouped_subject_under_whole_interchange_scope_rejects_ucf_and_uci_without_codes() {
    let raw = format!(
        "{UNB_3}UNG+UTILMD+SND:14+RCV:14+261001:0700+GRP1+UN+D:11A'\
         {MSG_1}UNH+2+UTILMD:D:11A:UN:S2.1'BGM+E03+Y'UNT+9+2'UNE+2+GRP1'UNZ+1+IC9'"
    );
    let options = ContrlOptions::new().rejection(RejectionScope::Interchange);
    let contrl = report_with(&raw, &options).expect("default policy");
    assert_wellformed(&contrl);
    assert_eq!(
        wire(contrl),
        format!(
            "UNH+ACK1+CONTRL:D:3:UN'{UCI}+4'UCF+GRP1+SND:14+RCV:14+4'\
             UCM+2+UTILMD:D:11A:UN:S2.1+4+29+UNT'UNT+5+ACK1'"
        )
    );
}

#[test]
fn default_options_equal_the_option_less_constructors() {
    // K5: `*_with(…, &ContrlOptions::default())` never fails and changes nothing.
    let defaults = ContrlOptions::default();
    for raw in [
        interchange(UNB_3, MSG_1, 1),
        second_message_faulty(UNB_3),
        second_message_faulty(UNB_4),
        interchange(UNB_3, "UNH+1+ORDERS:D:96A:UN'FTX+   'UNT+3+1'", 1),
        format!("{UNB_3}{MSG_1}UNZ+1+IC-OTHER'"),
    ] {
        let (segments, validated, report) = subject(&raw);
        let plain = Contrl::from_report(&validated, &segments, &report);
        let with = Contrl::from_report_with(&validated, &segments, &report, &defaults)
            .expect("default policy permits everything");
        assert_eq!(plain.segments(), with.segments(), "{raw}");

        let plain = Contrl::receipt(&validated.interchange);
        let with = Contrl::receipt_with(&validated.interchange, &defaults).expect("permitted");
        assert_eq!(plain.segments(), with.segments(), "{raw}");

        let plain = Contrl::acknowledgement(&validated);
        let with = Contrl::acknowledgement_with(&validated, &defaults).expect("permitted");
        assert_eq!(plain.segments(), with.segments(), "{raw}");
    }
}

// ── complete reporting levels ────────────────────────────────────────────────

#[test]
fn bdew_unt_count_rejection_is_byte_exact() {
    // A message-level rejection: the UCM names the failing UNT.
    let raw = interchange(UNB_3, "UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E03+X'UNT+9+1'", 1);
    let contrl = report_with(&raw, &bdew()).expect("only 4s are emitted");
    assert_wellformed(&contrl);
    assert_eq!(
        wire(contrl),
        "UNH+ACK1+CONTRL:D:3:UN:2.0b'UCI+IC9+4012345000023:500+9900357000004:500+4'\
         UCM+1+UTILMD:D:11A:UN:S2.1+4+29+UNT'UNT+4+ACK1'"
    );
}

#[test]
fn a_unz_reference_mismatch_names_unz_on_the_uci() {
    let raw = format!("{UNB_3}{MSG_1}UNZ+1+IC-OTHER'");
    assert_eq!(
        report_default(&raw),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+4+28+UNZ'UNT+3+ACK1'")
    );
}

#[test]
fn a_une_reference_mismatch_names_une_on_the_ucf() {
    let raw = format!(
        "{UNB_3}UNG+UTILMD+SND+RCV+261001:0700+GRP1+UN+D:11A'{MSG_1}UNE+1+GRP-OTHER'UNZ+1+IC9'"
    );
    assert_eq!(
        report_default(&raw),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+7'UCF+GRP1+SND+RCV+4+28+UNE'UNT+4+ACK1'")
    );
}

#[test]
fn a_duplicate_unh_reference_names_unh_on_the_ucm() {
    let raw = interchange(
        UNB_3,
        &format!("{MSG_1}UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E03+Y'UNT+3+1'"),
        2,
    );
    assert_eq!(
        report_default(&raw),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+7'UCM+1+UTILMD:D:11A:UN:S2.1+4+26+UNH'UNT+4+ACK1'")
    );
}

#[test]
fn an_unsupported_charset_names_unb_and_its_s011() {
    let raw = interchange(
        "UNB+UNOX:3+4012345000023:500+9900357000004:500+261001:0700+IC9'",
        MSG_1,
        1,
    );
    let recovered = RecoveredInterchange::scan(raw.as_bytes(), &RecoveryConfig::default());
    // The envelope layer refuses the repertoire; locate it on the scan.
    let error = EdifactError::UnsupportedCharset {
        syntax_identifier: "UNOX".into(),
    };
    let location = recovered.locate(&error, None).expect("a subject fault");
    let contrl = Contrl::from_recovered(&recovered, &[location], &ContrlOptions::default())
        .expect("addressed");
    assert_eq!(
        wire(contrl),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+4+46+UNB+2:1'UNT+3+ACK1'")
    );
}

#[test]
fn ucm_copies_the_subject_association_code() {
    // UCM S009 carries the subject's association code (`S2.1`) verbatim.
    let raw = second_message_faulty(UNB_3);
    assert!(report_default(&raw).contains("UCM+2+UTILMD:D:11A:UN:S2.1+"));
    // …and a subject without DE 0057 gets none.
    let raw = interchange(UNB_3, "UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+9+1'", 1);
    assert!(report_default(&raw).contains("UCM+1+ORDERS:D:96A:UN+4+29+UNT'"));
}

/// Every `contrl.rs` wire test re-run on a syntax 4 subject: `CONTRL:4:1:UN`,
/// each error code is reported on the service segment it concerns.
#[test]
fn contrl_wire_cases_on_syntax_4_subjects() {
    const V4: &str = "UNB+UNOC:4+S+R+20260101:0900+IC1'";
    let cases: [(String, &str); 6] = [
        (
            format!(
                "{V4}UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNH+M1+ORDERS:D:96A:UN'BGM+221'UNT+3+M1'UNZ+2+IC1'"
            ),
            // Duplicate message reference: code 26 on UNH.
            "UNH+ACK1+CONTRL:4:1:UN'UCI+IC1+S+R+7'UCM+M1+ORDERS:D:96A:UN+4+26+UNH'UNT+4+ACK1'",
        ),
        (
            format!("{V4}UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNZ+1+IC-OTHER'"),
            // Interchange reference mismatch: code 28 on UNZ.
            "UNH+ACK1+CONTRL:4:1:UN'UCI+IC1+S+R+4+28+UNZ'UNT+3+ACK1'",
        ),
        (
            format!("{V4}UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+9+M1'UNZ+1+IC1'"),
            // Segment count mismatch: code 29 on UNT.
            "UNH+ACK1+CONTRL:4:1:UN'UCI+IC1+S+R+7'UCM+M1+ORDERS:D:96A:UN+4+29+UNT'UNT+4+ACK1'",
        ),
        (
            format!(
                "{V4}UNG+ORDERS+SND:14+RCV:14+20260101:0900+GRP1+UN+D:96A'UNH+M1+ORDERS:D:96A:UN'FTX+   'UNT+3+M1'UNE+1+GRP1'UNZ+1+IC1'"
            ),
            "UNH+ACK1+CONTRL:4:1:UN'UCI+IC1+S+R+7'UCF+GRP1+SND:14+RCV:14+7'UCM+M1+ORDERS:D:96A:UN+7'UCS+2'UCD+12+2:1'UNT+7+ACK1'",
        ),
        (
            format!(
                "{V4}UNG+ORDERS+SND+RCV+20260101:0900+GRP1+UN+D:96A'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNE+1+GRP-OTHER'UNZ+1+IC1'"
            ),
            // Group reference mismatch: code 28 on UNE.
            "UNH+ACK1+CONTRL:4:1:UN'UCI+IC1+S+R+7'UCF+GRP1+SND+RCV+4+28+UNE'UNT+4+ACK1'",
        ),
        (
            format!(
                "{V4}UNG+ORDERS+SND+RCV+20260101:0900+GRP1+UN+D:96A'UNH+M1+ORDERS:D:96A:UN'BGM+220'UNT+3+M1'UNE+1+GRP1'UNZ+1+IC1'"
            ),
            "UNH+ACK1+CONTRL:4:1:UN'UCI+IC1+S+R+7'UNT+3+ACK1'",
        ),
    ];
    for (raw, expected) in cases {
        assert_eq!(report_default(&raw), expected, "{raw}");
    }
    // The data-element case of the module tests, ungrouped.
    let raw = format!("{V4}UNH+M1+ORDERS:D:96A:UN'FTX+   'UNT+3+M1'UNZ+1+IC1'");
    let out = report_default(&raw);
    assert!(
        out.starts_with("UNH+ACK1+CONTRL:4:1:UN'UCI+IC1+S+R+7'"),
        "{out}"
    );
    assert!(
        out.contains("UCM+M1+ORDERS:D:96A:UN+7'UCS+2'UCD+12+2:1'"),
        "{out}"
    );
}

// ── CONTRL for an interchange that does not parse ────────────────────────────

/// Two messages; the second's `BGM` tag is broken (`BGME03`): the
/// per-message rejection case.
fn broken_second_tag() -> String {
    interchange(
        UNB_3,
        "UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E03+X'UNT+3+1'\
         UNH+2+UTILMD:D:11A:UN:S2.1'BGME03+X'UNT+3+2'",
        2,
    )
}

#[test]
fn bdew_per_message_from_recovered_is_byte_exact() {
    let raw = broken_second_tag();
    let recovered = RecoveredInterchange::scan(raw.as_bytes(), &RecoveryConfig::default());
    let contrl = Contrl::from_recovered(&recovered, &[], &bdew()).expect("only 4s");
    assert_wellformed(&contrl);
    let wire = wire(contrl);
    assert_eq!(
        wire,
        "UNH+ACK1+CONTRL:D:3:UN:2.0b'UCI+IC9+4012345000023:500+9900357000004:500+4'\
         UCM+2+UTILMD:D:11A:UN:S2.1+4'UCS+2'UCD+12+1'UNT+6+ACK1'"
    );

    // The same wire, field by field.
    assert!(wire.contains("+CONTRL:D:3:UN:2.0b'"));
    let uci = wire
        .split('\'')
        .find(|s| s.starts_with("UCI+"))
        .expect("UCI");
    assert!(uci.ends_with("+4"), "UCI DE 0083 = 4 and no DE 0085: {uci}");
    assert!(uci.starts_with("UCI+IC9+4012345000023:"));
    assert!(wire.contains("'UCM+2+UTILMD:D:11A:UN:"));
    let ucm = wire
        .split('\'')
        .find(|s| s.starts_with("UCM+"))
        .expect("UCM");
    assert!(ucm.ends_with("+4"), "UCM DE 0083 = 4: {ucm}");
    assert!(
        !wire.contains("UCM+1+"),
        "the readable message is not named"
    );
    assert!(wire.contains("'UCS+2'"));
    assert!(wire.contains("'UCD+12+1'"));
}

#[test]
fn per_level_from_recovered_keeps_the_uci_acknowledged() {
    let raw = broken_second_tag();
    let recovered = RecoveredInterchange::scan(raw.as_bytes(), &RecoveryConfig::default());
    let contrl =
        Contrl::from_recovered(&recovered, &[], &ContrlOptions::default()).expect("addressed");
    assert_eq!(
        wire(contrl),
        format!(
            "UNH+ACK1+CONTRL:D:3:UN'{UCI}+7'UCM+2+UTILMD:D:11A:UN:S2.1+4'UCS+2'UCD+12+1'UNT+6+ACK1'"
        )
    );
}

#[test]
fn no_readable_addressing_is_no_contrl() {
    let recovered = RecoveredInterchange::scan(b"\x00\x01not edifact", &RecoveryConfig::default());
    assert!(matches!(
        Contrl::from_recovered(&recovered, &[], &ContrlOptions::default()),
        Err(EdifactError::MissingSegment { ref tag, .. }) if tag == "UNB"
    ));
}

#[test]
fn a_terminal_fault_never_acknowledges() {
    // 10 MB of terminators after a valid UNB: the fault budget ends the scan.
    let mut raw = UNB_3.as_bytes().to_vec();
    raw.extend(std::iter::repeat_n(b'\'', 10 * 1024 * 1024));
    let recovered = RecoveredInterchange::scan(&raw, &RecoveryConfig::default());
    assert!(!recovered.is_complete());
    for scope in [RejectionScope::PerLevel, RejectionScope::Interchange] {
        let contrl =
            Contrl::from_recovered(&recovered, &[], &ContrlOptions::new().rejection(scope))
                .expect("addressed");
        assert_eq!(contrl.action(), Action::Rejected, "{scope:?}");
        assert_wellformed(&contrl);
    }

    // An unterminated final segment inside a message: the message is named,
    // the UCI rejects without a code of its own, never `7`.
    let raw = format!("{UNB_3}UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E03+X");
    let recovered = RecoveredInterchange::scan(raw.as_bytes(), &RecoveryConfig::default());
    assert!(!recovered.is_complete());
    let contrl =
        Contrl::from_recovered(&recovered, &[], &ContrlOptions::default()).expect("addressed");
    assert_eq!(contrl.action(), Action::Rejected);
    let out = wire(contrl);
    assert!(out.contains(&format!("{UCI}+4'")), "{out}");
    assert!(out.contains("UCM+1+UTILMD:D:11A:UN:S2.1+4"), "{out}");
}

#[test]
fn extra_locations_are_reported_like_faults() {
    // A clean interchange, and a typed layer that found BGM element 1 missing.
    let raw = interchange(UNB_3, "UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'", 1);
    let recovered = RecoveredInterchange::scan(raw.as_bytes(), &RecoveryConfig::default());
    let error = EdifactError::MissingRequiredElement {
        tag: "BGM".into(),
        element_index: 1,
    };
    let location = recovered.locate(&error, Some(0)).expect("a subject fault");
    let contrl = Contrl::from_recovered(&recovered, &[location], &ContrlOptions::default())
        .expect("addressed");
    assert_eq!(
        wire(contrl),
        format!(
            "UNH+ACK1+CONTRL:D:3:UN'{UCI}+7'UCM+1+ORDERS:D:96A:UN+4'UCS+2'UCD+13+3'UNT+6+ACK1'"
        )
    );

    // And with nothing to report, the recovered path acknowledges.
    let contrl =
        Contrl::from_recovered(&recovered, &[], &ContrlOptions::default()).expect("addressed");
    assert_eq!(
        wire(contrl),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+7'UNT+3+ACK1'")
    );
}

#[test]
fn a_fault_in_an_unnamed_message_rejects_the_uci() {
    // The second UNH has no readable DE 0062, so no UCM can name it.
    let raw = interchange(
        UNB_3,
        "UNH+1+ORDERS:D:96A:UN'BGM+220'UNT+3+1'UNH++ORDERS:D:96A:UN'BGME03+X'UNT+3+2'",
        2,
    );
    let recovered = RecoveredInterchange::scan(raw.as_bytes(), &RecoveryConfig::default());
    let contrl =
        Contrl::from_recovered(&recovered, &[], &ContrlOptions::default()).expect("addressed");
    assert_eq!(
        wire(contrl),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+4'UNT+3+ACK1'")
    );
}

#[test]
fn an_unsupported_charset_from_a_validation_run_names_unb_and_its_s011() {
    // Through `from_report`: the span-less E039 is anchored on UNB S001.
    let raw = interchange(
        "UNB+UNOX:3+4012345000023:500+9900357000004:500+261001:0700+IC9'",
        MSG_1,
        1,
    );
    assert_eq!(
        report_default(&raw),
        format!("UNH+ACK1+CONTRL:D:3:UN'{UCI}+4+46+UNB+2:1'UNT+3+ACK1'")
    );
}

// ── reply interchange addressing ─────────────────────────────────────────────

/// The reply `UNB` swaps the parties and keeps each one's DE 0007 qualifier,
/// so callers need not wrap the `CONTRL` themselves.
#[test]
fn reply_unb_keeps_party_qualifiers() {
    let raw = interchange(UNB_3, MSG_1, 1);
    let (_, validated, _) = subject(&raw);
    let wire = Contrl::acknowledgement(&validated)
        .to_interchange_string("UNOC", "3", "261001", "0800", "R1")
        .expect("render");
    assert!(
        wire.starts_with("UNB+UNOC:3+9900357000004:500+4012345000023:500+261001:0800+R1'"),
        "{wire}"
    );
    let segments: Vec<_> = from_bytes(wire.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("reply reparses");
    validate_envelope(&segments).expect("reply passes validate_envelope");
}

/// A party without a qualifier is written without one, as before.
#[test]
fn reply_unb_without_qualifiers_is_unchanged() {
    let raw = interchange("UNB+UNOC:3+SENDER+RECEIVER+261001:0700+IC9'", MSG_1, 1);
    let (_, validated, _) = subject(&raw);
    let wire = Contrl::acknowledgement(&validated)
        .to_interchange_string("UNOC", "3", "261001", "0800", "R1")
        .expect("render");
    assert!(
        wire.starts_with("UNB+UNOC:3+RECEIVER+SENDER+261001:0800+R1'"),
        "{wire}"
    );
}
