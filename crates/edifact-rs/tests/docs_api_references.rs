//! Verifies that every `edifact_rs::` item named in the guides actually exists.
//!
//! Guides carry a mix of runnable examples (compiled as doctests) and
//! illustrative fragments marked `rust,ignore`.  Only the former are checked by
//! the compiler, so this covers the ignored snippets too: it extracts every
//! item imported from `edifact_rs` across the guides and asserts each is public.
//!
//! The public API is read out of `src/lib.rs` rather than mirrored in a list
//! here, so it cannot go stale in either direction.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn docs_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/edifact-rs; the guides live at the repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../site/content/docs")
        .canonicalize()
        .expect("site/content/docs must exist under the workspace root")
}

/// Every name reachable as `edifact_rs::<name>`, parsed out of `src/lib.rs`.
///
/// Covers the three ways an item reaches the crate root: a `pub use` re-export
/// (braced or single), a `pub mod` declaration, and an item defined at the root
/// itself.  `#[macro_export]` macros land at the root too and are picked up from
/// their definition.
fn public_api() -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let lib = std::fs::read_to_string(root.join("lib.rs")).expect("src/lib.rs must be readable");
    let mut api = BTreeSet::new();

    // `pub use path::{A, B as C, d};` and `pub use path::Item;`
    let mut rest = lib.as_str();
    while let Some(pos) = rest.find("pub use ") {
        rest = &rest[pos + "pub use ".len()..];
        let Some(end) = rest.find(';') else { break };
        let stmt = &rest[..end];
        rest = &rest[end + 1..];
        match stmt.find('{') {
            Some(brace) => {
                let body = stmt[brace + 1..].trim_end().trim_end_matches('}');
                for part in body.split(',') {
                    // `X as Y` re-exports under `Y`.
                    let name = part.split_whitespace().last().unwrap_or("");
                    if !name.is_empty() {
                        api.insert(name.trim_end_matches('}').to_owned());
                    }
                }
            }
            None => {
                if let Some(name) = stmt.rsplit("::").next() {
                    let name = name.split_whitespace().last().unwrap_or("").trim();
                    if !name.is_empty() {
                        api.insert(name.to_owned());
                    }
                }
            }
        }
    }

    // `pub mod name;` and root-level `pub fn` / `pub struct` / `pub enum` / `pub trait`.
    for line in lib.lines() {
        let line = line.trim_start();
        for (prefix, terminators) in [
            ("pub mod ", &[';'][..]),
            ("pub fn ", &['(', '<'][..]),
            ("pub struct ", &['<', '{', '(', ';'][..]),
            ("pub enum ", &['<', '{'][..]),
            ("pub trait ", &['<', '{', ':'][..]),
        ] {
            if let Some(tail) = line.strip_prefix(prefix) {
                let name: String = tail
                    .chars()
                    .take_while(|c| !terminators.contains(c) && !c.is_whitespace())
                    .collect();
                if !name.is_empty() {
                    api.insert(name);
                }
            }
        }
    }

    // `#[macro_export]` macros are addressable at the crate root wherever they
    // are defined, so scan the whole module tree for them.
    for entry in walk(&root) {
        let text = std::fs::read_to_string(&entry).expect("module must be readable");
        let mut rest = text.as_str();
        while let Some(pos) = rest.find("#[macro_export]") {
            rest = &rest[pos + "#[macro_export]".len()..];
            if let Some(decl) = rest.find("macro_rules! ") {
                let tail = &rest[decl + "macro_rules! ".len()..];
                let name: String = tail.chars().take_while(|c| !c.is_whitespace()).collect();
                if !name.is_empty() {
                    api.insert(name);
                }
            }
        }
    }

    assert!(
        api.len() > 80,
        "parsed only {} public items from lib.rs — the parser is out of step \
         with how the crate declares its exports",
        api.len(),
    );
    api
}

/// Every `.rs` file under `dir`, recursively.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

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
    let api = public_api();
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
