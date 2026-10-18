//! The legacy open-group resolver — a frozen copy of the earlier
//! `group_recursive_indexed` from `src/group.rs`, kept as a test oracle.
//!
//! The current engine must produce the same tree as this reference for every
//! schema in which no group declares member segments ("open" groups). Do not
//! change the algorithm below: it is the definition of "unchanged".
//!
//! Only the types differ from the original — `LegacyDef` mirrors the legacy
//! `GroupDef<'a>` fields and `LegacyNode` the legacy `SegmentGroupIndexed`
//! fields.

use std::collections::HashMap;
use std::ops::Range;

/// The legacy `GroupDef<'a>`: name, trigger tag, child groups.
#[derive(Debug, Clone)]
pub struct LegacyDef {
    pub name: String,
    pub trigger: String,
    pub children: Vec<LegacyDef>,
}

/// The legacy `SegmentGroupIndexed` fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyNode {
    pub definition: String,
    pub total_span: Range<usize>,
    pub children: Vec<LegacyNode>,
    pub occurrence_index: usize,
}

/// Legacy `group_segments_indexed` over a tag sequence.
pub fn legacy_group(tags: &[&str], schema: &[LegacyDef], root_name: &str) -> LegacyNode {
    let mut root = LegacyNode {
        definition: root_name.to_owned(),
        total_span: 0..0,
        children: Vec::new(),
        occurrence_index: 0,
    };
    group_recursive_indexed(tags, &mut root, schema, &[], 0);
    root
}

fn group_recursive_indexed(
    tags: &[&str],
    parent: &mut LegacyNode,
    schema: &[LegacyDef],
    stop_triggers: &[String],
    offset: usize,
) -> usize {
    let combined_stop: Vec<String> = {
        let mut v: Vec<String> = stop_triggers.to_vec();
        for d in schema {
            if !v.contains(&d.trigger) {
                v.push(d.trigger.clone());
            }
        }
        v
    };

    let span_start = if !parent.total_span.is_empty() {
        parent.total_span.start
    } else {
        offset
    };

    let mut i = 0;
    let mut occ_counts: HashMap<String, usize> = HashMap::new();
    while i < tags.len() {
        let tag = tags[i];
        let matched = schema.iter().find(|d| d.trigger == tag);

        if matched.is_none() && stop_triggers.iter().any(|t| t == tag) {
            break;
        }

        if let Some(def) = matched {
            let child_offset = offset + i;
            let occ_idx = {
                let c = occ_counts.entry(def.name.clone()).or_insert(0);
                let idx = *c;
                *c += 1;
                idx
            };
            let mut child = LegacyNode {
                definition: def.name.clone(),
                total_span: child_offset..child_offset + 1,
                children: Vec::new(),
                occurrence_index: occ_idx,
            };
            i += 1;

            let consumed = group_recursive_indexed(
                &tags[i..],
                &mut child,
                &def.children,
                &combined_stop,
                offset + i,
            );
            i += consumed;

            parent.children.push(child);
        } else {
            i += 1;
        }
    }

    parent.total_span = span_start..(offset + i);

    i
}

/// Legacy `direct_segment_indices`: the span minus every child span.
pub fn legacy_direct(node: &LegacyNode) -> Vec<usize> {
    node.total_span
        .clone()
        .filter(|i| !node.children.iter().any(|c| c.total_span.contains(i)))
        .collect()
}
