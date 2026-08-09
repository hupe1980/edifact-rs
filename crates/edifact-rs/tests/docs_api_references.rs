//! Verifies that every `edifact_rs::` item named in the guides actually exists.
//!
//! Guides carry a mix of runnable examples (compiled as doctests) and
//! illustrative fragments marked `rust,ignore`.  Only the former are checked by
//! the compiler, so a rename could still leave a stale name in an ignored
//! snippet — which is exactly how several guides came to reference private
//! module paths and methods that had never existed.
//!
//! This test extracts the item names imported from `edifact_rs` across all
//! guides and asserts each one is in the crate's public API, covering ignored
//! snippets too.
//!
//! When this fails, either the guide is stale or the export was removed on
//! purpose — update whichever is wrong; do not add the name to the allow-list
//! unless it is genuinely re-exported.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn docs_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/edifact-rs; the guides live at the repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../site/content/docs")
        .canonicalize()
        .expect("site/content/docs must exist under the workspace root")
}

/// Every name re-exported from the crate root, plus the public modules.
///
/// Kept as an explicit list rather than derived at runtime because Rust has no
/// stable reflection over a crate's exports.  `cargo doc` is the source of
/// truth; this mirrors it.
const PUBLIC_API: &[&str] = &[
    // modules
    "contrl",
    "de",
    "directory_validator",
    "group",
    "report",
    "ser",
    // core model
    "Components",
    "OwnedComponents",
    "BorrowedElement",
    "BorrowedSegment",
    "Element",
    "OwnedElement",
    "OwnedSegment",
    "Segment",
    "Span",
    // errors
    "EdifactError",
    "IoError",
    // character repertoires
    "Charset",
    "CharsetValidator",
    "SyntaxValidator",
    "severity_for_error",
    // layout auditing
    "LayoutAudit",
    "LayoutFinding",
    "LayoutSlot",
    "audit_directory",
    "Insignificant",
    // data element representations
    "Repr",
    "ReprKind",
    "DecodingReader",
    "charset",
    "decode_interchange",
    "decode_reader",
    "sniff_charset",
    "DecodingSegmentStream",
    "from_bytes_decoded",
    "from_bytes_decoded_with_config",
    "from_reader_decoded",
    "from_reader_decoded_with_config",
    // CONTRL acknowledgements (ISO 9735-4)
    "Action",
    "Contrl",
    "ReportingLevel",
    "SyntaxError",
    // ISO 9735 service-segment layouts
    "service",
    // envelope
    "FunctionalGroupEnvelope",
    "GroupIdentifier",
    "InterchangeEnvelope",
    "LenientResult",
    "MessageEnvelope",
    "MessageIdentifier",
    "ValidatedInterchange",
    "parse_ung",
    "parse_unh",
    "validate_envelope",
    "validate_envelope_from_owned",
    "validate_envelope_lenient",
    "validate_envelope_lenient_from_owned",
    "validate_envelope_owned",
    "validate_envelope_lenient_owned",
    // grouping
    "GroupDef",
    "SegmentGroupIndexed",
    "group_owned_segments_indexed",
    "group_segments_indexed",
    // parser / tokenizer
    "OwnedSegmentStream",
    "Parser",
    "ReaderConfig",
    "ServiceStringAdvice",
    "Token",
    "Tokenizer",
    "from_bufread",
    "from_bufread_stream",
    "from_bufread_stream_with_config",
    "from_bytes",
    "from_bytes_owned",
    "from_bytes_owned_with_config",
    "from_bytes_windows",
    "from_bytes_with_config",
    "from_reader",
    "from_reader_collect",
    "from_reader_with_config",
    // writer / events
    "AsDataElement",
    "DataElement",
    "EdifactEvent",
    "EventEmitter",
    "MessageWriter",
    "OwnedEdifactEvent",
    "VecEmitter",
    "Writer",
    "WriterEmitter",
    "elements",
    "emit_sparse_segment",
    "segments_to_bytes",
    "segments_to_bytes_owned",
    "to_writer",
    // validation
    "EnvelopeValidator",
    "ProfileRule",
    "ProfileRulePack",
    "ValidationContext",
    "ValidationContextBuilder",
    "ValidationIssue",
    "ValidationLayer",
    "ValidationReport",
    "ValidationRuleContext",
    "ValidationSeverity",
    "Validator",
    "validate_each",
    // directory validation
    "ComponentRef",
    "DirectoryValidator",
    "DirectoryValidatorBuilder",
    "ElementPath",
    "ElementRef",
    "OwnedComponentRef",
    "OwnedElementRef",
    "OwnedSegmentDef",
    "SegmentDefinition",
    "SegmentLayout",
    "Status",
    // serde layer
    "CompositeElement",
    "DecimalFloat",
    "DecimalFloatDisplay",
    "DispatchedMessage",
    "EdifactCompositeDeserialize",
    "EdifactCompositeSerialize",
    "EdifactDeserialize",
    "EdifactSegmentTag",
    "EdifactSerialize",
    "MessageDispatch",
    "MessageWindow",
    "MessageWindowsIter",
    "MessageWindowsSliceIter",
    "OwnedMessageWindow",
    "SegmentAccessor",
    "composite_element",
    "contiguous_groups_by_qualifier",
    "contiguous_groups_iter",
    "deserialize",
    "deserialize_all_from_reader",
    "deserialize_all_streaming",
    "deserialize_first_from_reader",
    "deserialize_first_streaming",
    "deserialize_messages_bytes",
    "deserialize_messages_from_reader",
    "deserialize_str",
    "element_str",
    "find_qualified_segment",
    "find_qualified_segment_owned",
    "find_segment",
    "find_segment_owned",
    "find_segment_typed",
    "find_segments_iter",
    "find_segments_typed",
    "get_components_iter",
    "groups_are_contiguous_by_qualifier",
    "message_windows_from_reader",
    "optional_component",
    "optional_element",
    "qualifier_matches_pattern",
    "required_component",
    "required_element",
    "to_bytes",
    "to_edifact_string",
];

/// Extract every item named in a `use edifact_rs::...;` statement.
///
/// Handles both single-line and brace-wrapped multi-line imports, and strips the
/// rustdoc hidden-line marker so `# use ...` setup lines are covered too.
fn imported_names(source: &str) -> BTreeSet<String> {
    // Normalise: drop hidden-line markers, then join into one buffer so a
    // multi-line `use edifact_rs::{ ... };` is a single match.
    let normalised: String = source
        .lines()
        .map(|raw| {
            let t = raw.trim_start();
            t.strip_prefix("# ")
                .unwrap_or_else(|| t.strip_prefix('#').unwrap_or(t))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut found = BTreeSet::new();
    let mut rest = normalised.as_str();
    while let Some(pos) = rest.find("use edifact_rs::") {
        rest = &rest[pos + "use edifact_rs::".len()..];
        // The statement runs to the terminating `;`.
        let Some(end) = rest.find(';') else { break };
        let stmt = rest[..end].trim();
        rest = &rest[end + 1..];

        let body = stmt
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(stmt);
        for part in body.split(',') {
            let name = part
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('}');
            if name.is_empty() || name == "self" || name == "*" {
                continue;
            }
            let head = name.split("::").next().unwrap_or(name);
            if head.chars().all(|c| c.is_alphanumeric() || c == '_') && !head.is_empty() {
                found.insert(head.to_owned());
            }
        }
    }
    found
}

#[test]
fn guides_only_reference_public_api_items() {
    let api: BTreeSet<&str> = PUBLIC_API.iter().copied().collect();
    let mut problems: Vec<String> = Vec::new();

    let mut sources: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(docs_dir()).expect("read the guides directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "md") {
            let text = std::fs::read_to_string(&path).expect("read guide");
            sources.push((path.display().to_string(), text));
        }
    }
    let readme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md");
    if let Ok(text) = std::fs::read_to_string(&readme) {
        sources.push((readme.display().to_string(), text));
    }

    assert!(!sources.is_empty(), "no guides found to check");

    for (name, text) in &sources {
        for item in imported_names(text) {
            if !api.contains(item.as_str()) {
                problems.push(format!("{name}: `edifact_rs::{item}` is not a public item"));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "guides reference items that are not part of the public API:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn public_api_list_is_accurate() {
    // Guard against the list above drifting into fiction: spot-check that a
    // representative sample really is importable at the crate root.
    #[allow(unused_imports)]
    use edifact_rs::{
        Action, Contrl, LayoutAudit, LayoutFinding, LayoutSlot, ReportingLevel, SyntaxError,
        SyntaxValidator, from_bytes_decoded, from_reader_decoded,
    };
    #[allow(unused_imports)]
    use edifact_rs::{
        AsDataElement, ComponentRef, DataElement, DirectoryValidator, EdifactError, Element,
        ElementPath, EventEmitter, LenientResult, OwnedComponentRef, OwnedSegment,
        OwnedSegmentStream, ProfileRulePack, ReaderConfig, Segment, SegmentLayout,
        ServiceStringAdvice, Span, Token, Tokenizer, ValidationContext, ValidationIssue,
        ValidationReport, Writer, WriterEmitter, emit_sparse_segment, from_bytes,
        to_edifact_string, validate_envelope, validate_envelope_lenient,
    };
    #[allow(unused_imports)]
    use edifact_rs::{Insignificant, Repr, ReprKind, audit_directory};
}

/// Every `EdifactError` variant must have an entry in the error reference guide.
///
/// The guide previously documented a variant that had been removed (and told
/// readers functional groups were unsupported, long after they were), while
/// omitting two live variants entirely. Nothing caught it because the guide was
/// prose only.
#[test]
fn error_reference_documents_every_stable_code() {
    // Driven by `error.rs` itself rather than a hand-maintained list of sample
    // values.  The previous shape built an array of one value per variant and
    // claimed a new variant "fails to compile" — it does not: an array is not a
    // match, and `EdifactError` is `#[non_exhaustive]`, so an integration test
    // cannot match it exhaustively anyway.  Two variants had already slipped
    // through.  `stable_code` *is* an exhaustive match inside the crate, so
    // reading the codes out of it covers every variant by construction.
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/error.rs"))
            .expect("src/error.rs must be readable");

    let body_start = source
        .find("pub const fn stable_code(")
        .expect("stable_code must exist");
    let body_end = source[body_start..]
        .find("\n    }")
        .map(|i| body_start + i)
        .expect("stable_code body must be delimited");
    let body = &source[body_start..body_end];

    let codes: BTreeSet<String> = body
        .match_indices("=> \"E")
        .map(|(i, _)| {
            // `i` points at `=`; the code literal starts four bytes later.
            let rest = &body[i + 4..];
            let end = rest.find('"').expect("code literal must be closed");
            rest[..end].to_owned()
        })
        .collect();

    assert!(
        codes.len() >= 35,
        "expected to find every stable code, only parsed {}: {codes:?}",
        codes.len()
    );

    let guide = std::fs::read_to_string(docs_dir().join("error-reference.md"))
        .expect("error-reference.md must exist");

    let missing: Vec<&String> = codes
        .iter()
        .filter(|code| !guide.contains(&format!("### {code} —")))
        .collect();
    assert!(
        missing.is_empty(),
        "error-reference.md is missing a `### <code> —` section for: {missing:?}"
    );

    // …and the summary table at the top must list them too.
    let table_missing: Vec<&String> = codes
        .iter()
        .filter(|code| !guide.contains(&format!("| {code} | `")))
        .collect();
    assert!(
        table_missing.is_empty(),
        "error-reference.md summary table is missing rows for: {table_missing:?}"
    );
}
