//! Integration tests for group-scoped validation (F-011 / F-029).
//!
//! These tests verify that [`ProfileRulePack`] group rules:
//! - Only fire for the group they are scoped to.
//! - Do **not** cross-fire when the same segment tag appears in a different group.
//! - Correctly auto-stamp `ValidationIssue::segment_group`.
//! - Work through [`ValidationContext::validate_grouped`], for segments from
//!   either parsing path.
//! - Honour `max_issues_per_rule` across the whole tree walk, not per group.

use edifact_rs::{
    ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity,
    group::{GroupDef, group_segments_indexed},
};

// ── Schema shared across tests ────────────────────────────────────────────────

/// Minimal multi-level schema:
///
/// ```text
/// ROOT
///   SG1 (trigger: RFF) — reference group
///     (no children)
///   SG5 (trigger: LOC) — location group
///     SG6 (trigger: QTY) — quantity sub-group
/// ```
static SCHEMA: &[GroupDef] = &[
    GroupDef {
        name: "SG1",
        trigger: "RFF",
        children: &[],
    },
    GroupDef {
        name: "SG5",
        trigger: "LOC",
        children: &[GroupDef {
            name: "SG6",
            trigger: "QTY",
            children: &[],
        }],
    },
];

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse ASCII EDIFACT snippet into owned segments.
fn owned_segs(input: &[u8]) -> Vec<edifact_rs::OwnedSegment> {
    edifact_rs::from_reader(std::io::Cursor::new(input))
        .collect::<Result<_, _>>()
        .expect("fixture should parse")
}

/// Parse ASCII EDIFACT snippet into borrowed segments (from a String so we own the buffer).
fn parse_segs(input: &str) -> Vec<edifact_rs::Segment<'static>> {
    // Leak the string so we get 'static lifetime — acceptable in tests only.
    let s: &'static str = Box::leak(input.to_owned().into_boxed_str());
    edifact_rs::from_bytes(s.as_bytes())
        .collect::<Result<_, _>>()
        .expect("fixture should parse")
}

// ── F-029 Test 1: group rule fires only for the scoped group ──────────────────

/// A rule scoped to "SG5" DOES fire when DTM is absent from SG5, even though it
/// appears in SG1. Cross-group contamination must not suppress the missing-segment
/// error for SG5.
#[test]
fn group_rule_fires_when_required_segment_absent_from_scoped_group() {
    // DTM is in SG1 (after RFF), not in SG5 (LOC).  Rule: DTM must be in SG5.
    let input = "UNH+1+ORDERS:D:04B:UN'RFF+Z13:REF1'DTM+137:20230101:102'LOC+172+LOC1'UNT+5+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST").require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    // DTM is present in SG1 but NOT in SG5, so the rule should fire.
    assert!(
        report.has_errors(),
        "expected error: DTM missing from SG5 — got: {report}"
    );
    let issue = report.errors().first().expect("at least one error");
    assert_eq!(issue.segment_group.as_deref(), Some("SG5"));
    assert_eq!(issue.rule_id.as_deref(), Some("SG5-DTM-M"));
}

/// A rule scoped to "SG5" must NOT fire when DTM is present in SG5.
#[test]
fn group_rule_does_not_fire_when_segment_present_in_scoped_group() {
    // DTM inside SG5 (after LOC).
    let input = "UNH+1+ORDERS:D:04B:UN'LOC+172+LOC1'DTM+137:20230101:102'UNT+3+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST").require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    assert!(
        report.is_valid(),
        "expected no errors (DTM is present in SG5): {report}"
    );
}

// ── F-029 Test 2: forbid_segment_in_group ─────────────────────────────────────

#[test]
fn forbid_segment_in_group_fires_when_segment_present() {
    // UNS must not appear in SG5.
    let input = "UNH+1+ORDERS:D:04B:UN'LOC+172+LOC1'UNS+D'UNT+3+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST").forbid_segment_in_group("SG5", "UNS", "SG5-UNS-F");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    assert!(report.has_errors(), "expected error: UNS in SG5 — {report}");
    let issue = report.errors().first().unwrap();
    assert_eq!(issue.segment_group.as_deref(), Some("SG5"));
}

#[test]
fn forbid_segment_in_group_does_not_fire_when_segment_absent() {
    let input = "UNH+1+ORDERS:D:04B:UN'LOC+172+LOC1'DTM+137:20230101:102'UNT+3+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST").forbid_segment_in_group("SG5", "UNS", "SG5-UNS-F");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    assert!(ctx.validate_grouped(&tree, &segs).is_valid());
}

// ── F-029 Test 3: cross-group non-contamination ───────────────────────────────

/// The same segment tag in two different groups must produce independent issues:
/// a rule on SG5/DTM must not mark the SG1/DTM occurrence and vice-versa.
#[test]
fn group_rules_for_different_groups_do_not_cross_contaminate() {
    // DTM in SG1 (after RFF) but not in SG5 (after LOC), and QTY in SG6 (after QTY trigger).
    let input =
        "UNH+1+ORDERS:D:04B:UN'RFF+Z13:R1'DTM+137:20230101:102'LOC+172+L1'QTY+220:100'UNT+5+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    // SG1 rule: require DTM (present → no error)
    // SG5 rule: require DTM (absent in SG5 → error)
    let pack = ProfileRulePack::new("TEST")
        .require_segment_in_group("SG1", "DTM", "SG1-DTM-M")
        .require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    // Exactly one error (SG5 missing DTM), not two.
    assert_eq!(report.errors().len(), 1, "expected 1 error; got {report}");
    assert_eq!(
        report.errors()[0].rule_id.as_deref(),
        Some("SG5-DTM-M"),
        "the error must be for SG5, not SG1"
    );
}

// ── F-029 Test 4: segment_group auto-stamped ──────────────────────────────────

#[test]
fn group_rule_issues_are_auto_stamped_with_group_name() {
    let input = "UNH+1+ORDERS:D:04B:UN'LOC+172+L1'UNT+2+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST").require_segment_in_group("SG5", "QTY", "SG5-QTY-M");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    assert!(report.has_errors());
    // Auto-stamp: segment_group must equal the group definition name.
    for issue in report.errors() {
        assert_eq!(
            issue.segment_group.as_deref(),
            Some("SG5"),
            "issue must be auto-stamped with SG5"
        );
    }
}

// ── F-029 Test 5: validate_lenient_grouped_owned ──────────────────────────────

#[test]
fn validate_lenient_grouped_owned_works_with_owned_segments() {
    let input = b"UNH+1+ORDERS:D:04B:UN'LOC+172+L1'DTM+137:20230101:102'UNT+3+1'";
    let owned = owned_segs(input);
    let tree = group_segments_indexed(&owned, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST").require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &owned);
    assert!(
        report.is_valid(),
        "DTM present in SG5 — no errors expected: {report}"
    );
}

// ── F-029 Test 6: multiple group occurrences (repetition) ────────────────────

/// When SG5 repeats, the rule fires per-occurrence.
#[test]
fn group_rule_fires_per_occurrence_when_group_repeats() {
    // Two SG5 groups: first has DTM, second does not.
    let input = "UNH+1+ORDERS:D:04B:UN'LOC+172+L1'DTM+137:20230101:102'LOC+172+L2'UNT+4+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST").require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    // One SG5 is missing DTM → exactly one error.
    assert_eq!(
        report.errors().len(),
        1,
        "exactly one SG5 missing DTM: {report}"
    );
}

// ── F-029 Test 7: with_scoped_group_rule_fn custom closure ───────────────────

#[test]
fn custom_scoped_group_rule_fn_fires_and_sets_segment_group() {
    let input = "UNH+1+ORDERS:D:04B:UN'LOC+172+L1'QTY+220:0'UNT+3+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST").with_scoped_group_rule_fn(
        "SG6",
        "SG6-QTY-NONZERO",
        |_group, group_segs, _ctx, issues| {
            for s in group_segs.iter().filter(|s| s.tag == "QTY") {
                let qty_val = s
                    .get_element(0)
                    .and_then(|e| e.get_component(1))
                    .unwrap_or("0");
                if qty_val == "0" {
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Warning,
                            "QTY value is zero in SG6",
                        )
                        .with_segment("QTY")
                        .with_rule_id("SG6-QTY-NONZERO"),
                    );
                }
            }
        },
    );
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    assert!(!report.warnings().is_empty(), "expected zero-qty warning");
    assert_eq!(
        report.warnings()[0].segment_group.as_deref(),
        Some("SG6"),
        "warning must be auto-stamped with SG6"
    );
}

// ── F-029 Test 8: message-type scoping works with group rules ─────────────────

#[test]
fn group_rules_respect_message_type_scoping() {
    // Pack scoped to ORDERS; message is INVOIC → no group rules should fire.
    let input = "UNH+1+INVOIC:D:96A:UN'LOC+172+L1'UNT+2+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("ORDERS-ONLY")
        .for_message_type("ORDERS")
        .require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    assert!(
        report.is_valid(),
        "INVOIC message: ORDERS-scoped group rules must not fire: {report}"
    );
}

// ── F-029 Test 9: flat + group phases both run ─────────────────────────────────

#[test]
fn flat_and_group_validation_both_run_in_grouped_mode() {
    // Flat rule: BGM must be present (it's absent).
    // Group rule: DTM must be in SG5 (also absent).
    let input = "UNH+1+ORDERS:D:04B:UN'LOC+172+L1'UNT+2+1'";
    let segs = parse_segs(input);
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("TEST")
        .require_segment("BGM", "BGM-M")
        .require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);
    // Both a flat error and a group error should appear.
    let rule_ids: Vec<Option<&str>> = report
        .errors()
        .iter()
        .map(|i| i.rule_id.as_deref())
        .collect();
    assert!(
        rule_ids.contains(&Some("BGM-M")),
        "flat rule BGM-M must fire: {report}"
    );
    assert!(
        rule_ids.contains(&Some("SG5-DTM-M")),
        "group rule SG5-DTM-M must fire: {report}"
    );
}

// ── segment_occurrence semantics: occurrence among matching segments, not absolute ─

#[test]
fn forbid_segment_segment_occurrence_is_relative_not_absolute() {
    // Three QTY segments at absolute positions 1, 2, 3 in the message.
    // forbid_segment fires for each; occurrences must be 0, 1, 2 (relative).
    let segs = parse_segs("UNH+1+ORDERS:D:96A:UN'QTY+21:10'QTY+21:20'QTY+21:30'UNT+4+1'");
    let pack = ProfileRulePack::new("TEST").forbid_segment("QTY", "TEST-FORBID-QTY");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();
    let report = ctx.validate(&segs);
    let mut occurrences: Vec<u16> = report
        .errors()
        .iter()
        .filter_map(|i| i.segment_occurrence)
        .collect();
    occurrences.sort_unstable();
    assert_eq!(
        occurrences,
        vec![0, 1, 2],
        "segment_occurrence must be 0-based relative to matching segments, not absolute positions"
    );
}

#[test]
fn forbid_segment_in_group_occurrence_is_relative_not_absolute() {
    // SG5 group (trigger: LOC) with two QTY segments at positions 1 and 2 within the group.
    // LOC is at absolute 0 within the group slice; occurrences for QTY must be 0 and 1.
    let segs = parse_segs("UNH+1+ORDERS:D:04B:UN'LOC+172+L1'QTY+21:10'QTY+21:20'UNT+4+1'");
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");
    let pack = ProfileRulePack::new("TEST").forbid_segment_in_group("SG5", "QTY", "TEST-SG5-QTY");
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();
    let report = ctx.validate_grouped(&tree, &segs);
    let mut occurrences: Vec<u16> = report
        .errors()
        .iter()
        .filter_map(|i| i.segment_occurrence)
        .collect();
    occurrences.sort_unstable();
    assert_eq!(
        occurrences,
        vec![0, 1],
        "segment_occurrence in group must count only matching segments, not absolute group slice position"
    );
}

// ── bail_on_first_error: child traversal must not stop on pre-existing errors ─

#[test]
fn bail_on_first_error_does_not_skip_sibling_groups_due_to_earlier_flat_errors() {
    // Two SG5 groups (LOC+L1 and LOC+L2), each missing DTM.
    // The flat pass also fires an error (BGM missing).
    // With bail_on_first_error the group pass should stop after the FIRST group
    // error it introduces — not skip all group rules because the flat pass
    // already put errors in the report before the group pass started.
    let segs = parse_segs(
        "UNH+1+ORDERS:D:04B:UN'\
         LOC+172+L1'LOC+172+L2'UNT+3+1'",
    );
    let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");

    // A pack with bail_on_first_error that requires DTM in every SG5.
    // Also require BGM (flat) — this fires first and puts an error in the report.
    let pack = ProfileRulePack::new("TEST")
        .require_segment("BGM", "BGM-M")
        .require_segment_in_group("SG5", "DTM", "SG5-DTM-M")
        .with_bail_on_first_error(true);
    let ctx = ValidationContext::builder().with_profile_pack(pack).build();

    let report = ctx.validate_grouped(&tree, &segs);

    // We must have at least the flat BGM error AND at least one group error.
    // (bail_on_first_error stops after the first group error from THIS pass,
    // not because the flat pass already populated errors.)
    assert!(report.has_errors(), "expected errors in report: {report}");
    let rule_ids: Vec<&str> = report
        .errors()
        .iter()
        .filter_map(|i| i.rule_id.as_deref())
        .collect();
    assert!(
        rule_ids.contains(&"BGM-M"),
        "flat error BGM-M must be present: {report}"
    );
    assert!(
        rule_ids.contains(&"SG5-DTM-M"),
        "group error SG5-DTM-M must not be skipped by pre-existing flat errors: {report}"
    );
}

// ── Runtime-loaded schemas ────────────────────────────────────────────────────

/// A MIG loaded at startup cannot produce `&'static str`.  `GroupDef` used to
/// hard-code `'static`, which made every runtime-loaded group schema impossible
/// even though the directory side of the crate (`OwnedSegmentDef`,
/// `DirectoryValidatorBuilder`) has always supported them.
#[test]
fn a_schema_built_at_runtime_groups_identically_to_a_static_one() {
    // Stand-in for names parsed out of a MIG file at startup.
    let names: Vec<String> = vec![
        "SG1".to_owned(),
        "RFF".to_owned(),
        "SG5".to_owned(),
        "LOC".to_owned(),
        "SG6".to_owned(),
        "QTY".to_owned(),
    ];
    let sg6 = vec![GroupDef::new(&names[4], &names[5])];
    let runtime_schema = vec![
        GroupDef::new(&names[0], &names[1]),
        GroupDef::with_children(&names[2], &names[3], &sg6),
    ];

    let input =
        b"UNH+1+ORDERS:D:96A:UN'RFF+ON:1'LOC+172+DE1'DTM+163:20260101:102'QTY+220:5'UNT+6+1'";
    let segs: Vec<_> = edifact_rs::from_bytes(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("parse");

    let from_static = group_segments_indexed(&segs, SCHEMA, "ROOT");
    let from_runtime = group_segments_indexed(&segs, &runtime_schema, "ROOT");

    fn shape(g: &edifact_rs::SegmentGroupIndexed<'_>) -> Vec<(String, std::ops::Range<usize>)> {
        let mut out = vec![(g.definition.to_owned(), g.total_span.clone())];
        for child in &g.children {
            out.extend(shape(child));
        }
        out
    }
    assert_eq!(shape(&from_static), shape(&from_runtime));
    assert!(shape(&from_runtime).iter().any(|(n, _)| n == "SG6"));
}

/// `max_issues_per_rule` documents itself as applying "per rule per call".
///
/// The group walk applied it per rule per *group occurrence*, so a rule firing
/// in twenty groups emitted twenty times the cap — and a group rule is the most
/// likely to flood a report, which is the case the cap exists for.
#[test]
fn max_issues_per_rule_caps_a_group_rule_across_the_whole_tree() {
    // Twenty SG1 occurrences, each triggering the rule once.
    let mut input = String::from("UNH+1+ORDERS:D:96A:UN'");
    for i in 0..20 {
        input.push_str(&format!("RFF+ON:{i}'"));
    }
    input.push_str("UNT+22+1'");
    let segments = parse_segs(&input);
    let tree = group_segments_indexed(&segments, SCHEMA, "ROOT");

    let pack = ProfileRulePack::new("CAP")
        .with_max_issues_per_rule(3)
        .with_scoped_group_rule_fn("SG1", "CAP-SG1", |_group, _segs, _ctx, issues| {
            issues.push(ValidationIssue::new(
                ValidationSeverity::Error,
                "one issue per SG1 occurrence",
            ));
        });

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_grouped(&tree, &segments);

    assert_eq!(
        report.total_issues(),
        3,
        "the cap is per rule per call, so twenty group occurrences must still \
         yield at most three issues, not twenty",
    );
}

/// The `UTILMD` shape: SG2 (message-level parties) and SG12 (a Vorgang's
/// parties, inside SG4) both trigger on `NAD`.
static UTILMD_LIKE: &[GroupDef] = &[
    GroupDef {
        name: "SG2",
        trigger: "NAD",
        children: &[],
    },
    GroupDef {
        name: "SG4",
        trigger: "IDE",
        children: &[GroupDef {
            name: "SG12",
            trigger: "NAD",
            children: &[],
        }],
    },
];

/// A group-scoped rule must see the group the message structure assigns, not the
/// one the traversal happened to reopen.
///
/// Before the traversal preferred a nested definition over an ancestor's
/// sibling, every `NAD` after the first `IDE` reopened a top-level SG2 — so an
/// SG2-scoped rule fired on a Vorgang's parties, and an SG12-scoped rule never
/// fired at all. That made a group-scoped `NAD` rule unsound on any message
/// carrying an SG12, and forced downstream profiles to express the constraint on
/// the flat `NAD` rule instead.
#[test]
fn a_group_scoped_nad_rule_fires_on_the_group_the_structure_assigns() {
    let input = "UNH+1+UTILMD:D:11A:UN'BGM+E01+1+9'                 NAD+MS+SENDER::293'NAD+MR+RECEIVER::293'                 IDE+24+VORGANG1'NAD+Z09+KUNDE::293'DTM+92:20260101:102'NAD+VY+PARTY::293'";
    let segments = parse_segs(input);
    let tree = group_segments_indexed(&segments, UTILMD_LIKE, "ROOT");

    // One rule per group, each recording the qualifiers it was shown.
    let pack = ProfileRulePack::new("UTILMD-LIKE")
        .with_scoped_group_rule_fn("SG2", "SG2-NAD", |_group, segs, _ctx, issues| {
            for qualifier in segs
                .iter()
                .filter(|s| s.tag == "NAD")
                .filter_map(|s| s.element_str(0))
            {
                issues.push(
                    ValidationIssue::new(ValidationSeverity::Info, qualifier).with_rule_id("SG2"),
                );
            }
        })
        .with_scoped_group_rule_fn("SG12", "SG12-NAD", |_group, segs, _ctx, issues| {
            for qualifier in segs
                .iter()
                .filter(|s| s.tag == "NAD")
                .filter_map(|s| s.element_str(0))
            {
                issues.push(
                    ValidationIssue::new(ValidationSeverity::Info, qualifier).with_rule_id("SG12"),
                );
            }
        });

    let report = ValidationContext::builder()
        .with_profile_pack(pack)
        .build()
        .validate_grouped(&tree, &segments);

    let seen = |rule: &str| -> Vec<String> {
        report
            .infos()
            .iter()
            .filter(|i| i.rule_id.as_deref() == Some(rule))
            .map(|i| i.message.clone())
            .collect()
    };

    assert_eq!(
        seen("SG2"),
        ["MS", "MR"],
        "SG2 sees only the message-level parties",
    );
    assert_eq!(
        seen("SG12"),
        ["Z09", "VY"],
        "SG12 sees the Vorgang's parties — and is reachable at all",
    );
}

/// The reader side of the same shape: enumerating a Vorgang's parties.
#[test]
fn every_nested_party_group_is_enumerable_from_the_tree() {
    let input = "NAD+MS+SENDER::293'                 IDE+24+V1'NAD+Z09+KUNDE::293'                 IDE+24+V2'NAD+VY+PARTY::293'NAD+DP+DELIVERY::293'";
    let segments = parse_segs(input);
    let tree = group_segments_indexed(&segments, UTILMD_LIKE, "ROOT");

    // Per Vorgang, in document order.
    let per_vorgang: Vec<Vec<&str>> = tree
        .find("SG4")
        .map(|sg4| {
            sg4.find("SG12")
                .filter_map(|sg12| sg12.segments(&segments).first())
                .filter_map(|nad| nad.element_str(0))
                .collect()
        })
        .collect();

    assert_eq!(per_vorgang, [vec!["Z09"], vec!["VY", "DP"]]);
}
