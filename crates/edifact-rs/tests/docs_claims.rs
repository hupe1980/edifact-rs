//! Keeps the Performance guide's figures tied to the tests that assert them.
//!
//! The guide quotes the committed allocation constants and names, in its
//! "Where these numbers come from" table, the test functions behind each claim.
//! A re-committed constant or a renamed test fails here until the guide follows.

#[allow(dead_code)]
#[path = "../benches/bench_data.rs"]
mod bench_data;

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn guide() -> String {
    read(&root().join("../../site/content/docs/performance.md"))
}

/// `value` grouped in threes with spaces, as the guides print numbers.
fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The value of `const <name>: usize = …;` in `tests/allocation_budget.rs`.
fn committed(name: &str) -> usize {
    let source = read(&root().join("tests/allocation_budget.rs"));
    let prefix = format!("const {name}: usize = ");
    let start = source
        .find(&prefix)
        .unwrap_or_else(|| panic!("allocation_budget.rs has no `{prefix}`"))
        + prefix.len();
    let end = start + source[start..].find(';').expect("constant ends with ';'");
    source[start..end]
        .replace('_', "")
        .parse()
        .expect("integer constant")
}

#[test]
fn quoted_figures_match_committed_constants() {
    let guide = guide();
    let fixture = bench_data::one_mb().len();
    let collect = committed("ONE_MB_SLICE_COLLECT_PEAK");
    let expected = [
        grouped(fixture),
        grouped(collect),
        format!("{:.1} × input", collect as f64 / fixture as f64),
        grouped(committed("ONE_MB_SLICE_STREAM_PEAK")),
        grouped(committed("ONE_MB_READER_STREAM_PEAK")),
    ];
    for figure in expected {
        assert!(
            guide.contains(figure.as_str()),
            "performance.md does not quote `{figure}`"
        );
    }
}

#[test]
fn evidence_table_names_existing_tests() {
    let guide = guide();
    let section = guide
        .split("## Where these numbers come from")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .expect("performance.md has a \"Where these numbers come from\" section");
    let mut checked = 0;
    for row in section.lines().filter(|l| l.starts_with('|')) {
        let Some(cell) = row.trim_end_matches('|').rsplit('|').next() else {
            continue;
        };
        let Some((file, functions)) = cell.trim().split_once(':') else {
            continue;
        };
        let file = file.trim().trim_matches('`');
        let source = read(&root().join(format!("tests/{file}.rs")));
        for function in functions.split(',') {
            let function = function.trim().trim_matches('`');
            assert!(
                source.contains(&format!("fn {function}(")),
                "performance.md cites `{file}::{function}`, which does not exist"
            );
            checked += 1;
        }
    }
    assert!(checked > 10, "only {checked} test references found");
}
