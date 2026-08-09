//! Segment group tree model for structured EDIFACT message navigation.
//!
//! Provides a recursive group schema ([`GroupDef`]) and a segment-slice-to-tree
//! function ([`group_segments_indexed`]) that partitions a flat segment slice into a
//! [`SegmentGroupIndexed`] tree according to the schema.
//!
//! # Model overview
//!
//! Every UN/EDIFACT message type has a fixed set of **segment groups**: named,
//! optionally-repeating sets of segments delimited by a specific *trigger*
//! segment tag.  For example, ORDERS D.11A has an `SG1` group starting with
//! `RFF`, an `SG2` group starting with `NAD`, and so on.
//!
//! This module provides lightweight, allocation-efficient types for defining
//! and working with these groups without requiring message-type-specific
//! generated code.
//!
//! # Example
//!
//! ```rust,ignore
//! use edifact_rs::group::{GroupDef, group_segments_indexed};
//!
//! static SG32: &[GroupDef] = &[GroupDef::new("SG32", "PRI")];
//! static ORDERS_GROUPS: &[GroupDef] = &[
//!     GroupDef::new("SG2", "NAD"),
//!     GroupDef::with_children("SG7", "LIN", SG32),
//! ];
//!
//! let root = group_segments_indexed(&segments, ORDERS_GROUPS, "ROOT");
//! for child in &root.children {
//!     let child_segs = &segments[child.total_span.clone()];
//!     println!("{} #{}: {} segments", child.definition, child.occurrence_index, child_segs.len());
//! }
//! ```

use crate::{OwnedSegment, Segment};
use smallvec::SmallVec;
use std::ops::Range;

// ── GroupDef ──────────────────────────────────────────────────────────────────

/// Schema describing one segment group within an EDIFACT message.
///
/// The lifetime `'a` is what the schema's strings and nested slices borrow
/// from.  A `const`/`static` table is `GroupDef<'static>` and costs no
/// allocation; a schema deserialized from a MIG at startup borrows from an
/// arena the caller owns.  Earlier releases hard-coded `&'static str`, which
/// made runtime-loaded schemas impossible even though the directory side of the
/// crate ([`OwnedSegmentDef`][crate::OwnedSegmentDef],
/// [`DirectoryValidatorBuilder`][crate::DirectoryValidatorBuilder]) has always
/// supported them.
///
/// # Returning a schema from a trait method
///
/// `static SCHEMA: &[GroupDef] = …` still compiles unchanged: in a `static`, the
/// elided lifetime resolves to `'static`. In **return position on a method**, it
/// does not — it binds to `&self`, so a trait method declared
/// `fn schema(&self) -> &'static [GroupDef]` fails to compile. Name the inner
/// lifetime explicitly there:
///
/// ```
/// use edifact_rs::group::GroupDef;
///
/// static SCHEMA: &[GroupDef] = &[GroupDef::new("SG1", "RFF")]; // unchanged
///
/// trait MessageSchema {
///     //                          ↓ both lifetimes named
///     fn groups(&self) -> &'static [GroupDef<'static>];
/// }
///
/// struct Orders;
/// impl MessageSchema for Orders {
///     fn groups(&self) -> &'static [GroupDef<'static>] {
///         SCHEMA
///     }
/// }
/// assert_eq!(Orders.groups()[0].name, "SG1");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct GroupDef<'a> {
    /// Human-readable group name, e.g. `"SG2"`.
    pub name: &'a str,
    /// The segment tag whose appearance starts a new instance of this group.
    pub trigger: &'a str,
    /// Nested child groups within this group.
    ///
    /// The first trigger encountered among `children` ends the current child
    /// and starts a new one; a trigger that matches a sibling or ancestor group
    /// ends this group entirely.
    pub children: &'a [GroupDef<'a>],
}

impl<'a> GroupDef<'a> {
    /// A leaf group: `name` is opened by `trigger` and has no nested groups.
    #[must_use]
    pub const fn new(name: &'a str, trigger: &'a str) -> Self {
        Self {
            name,
            trigger,
            children: &[],
        }
    }

    /// A group with nested child groups.
    #[must_use]
    pub const fn with_children(
        name: &'a str,
        trigger: &'a str,
        children: &'a [GroupDef<'a>],
    ) -> Self {
        Self {
            name,
            trigger,
            children,
        }
    }
}

// ── SegmentGroupIndexed ───────────────────────────────────────────────────────

/// Zero-copy segment group tree.  Stores index ranges into the original flat
/// segment slice rather than cloning each segment.
///
/// Produced by [`group_segments_indexed`].  To access the actual segments use
/// the original `&[Segment<'a>]` together with [`total_span`]:
///
/// ```rust,ignore
/// let indexed = group_segments_indexed(&segments, MY_SCHEMA, "ROOT");
/// for child in &indexed.children {
///     let child_segs = &segments[child.total_span.clone()];
/// }
/// ```
///
/// [`total_span`]: SegmentGroupIndexed::total_span
#[derive(Debug)]
pub struct SegmentGroupIndexed<'a> {
    /// Group name from the schema, e.g. `"SG2"`, or the root name.
    ///
    /// Borrows from the schema, so it lives exactly as long as the schema does.
    pub definition: &'a str,
    /// Contiguous span `[start, end)` of absolute indices into the original flat
    /// segment slice covering **all** segments in this group instance — trigger
    /// segment, direct segments, and all descendant groups combined.
    ///
    /// Use this to slice the original `&[Segment<'_>]` to get every segment
    /// belonging to this group:
    ///
    /// ```rust,ignore
    /// let all_sg2_segs = &segments[sg2.total_span.clone()];
    /// ```
    ///
    /// To iterate over only the segments that belong *directly* to this group
    /// (excluding descendants), use [`direct_segment_indices`].
    ///
    /// [`direct_segment_indices`]: SegmentGroupIndexed::direct_segment_indices
    pub total_span: Range<usize>,
    /// Child group instances, in message order.
    pub children: Vec<SegmentGroupIndexed<'a>>,
    /// Zero-based occurrence index of this group instance among all siblings
    /// with the same `definition` at this level.
    ///
    /// For example, the first `SG5` child at a given level has `occurrence_index = 0`,
    /// the second `SG5` has `occurrence_index = 1`, etc.  Siblings with a
    /// *different* definition have independent counters.
    ///
    /// This field is essential for producing unambiguous rule-violation IDs
    /// (e.g. `"SG5[2]/DTM"`) when the same group type repeats.
    pub occurrence_index: usize,
}

impl SegmentGroupIndexed<'_> {
    /// Iterate over the absolute indices of segments that belong *directly* to
    /// this group — i.e. those within [`total_span`] that are **not** covered
    /// by any child group's [`total_span`].
    ///
    /// Complexity: `O(total_span.len() × children.len())`.  For typical EDIFACT
    /// message structures (≤ 8 children per group) this is negligible.
    ///
    /// [`total_span`]: SegmentGroupIndexed::total_span
    pub fn direct_segment_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.total_span.clone().filter(|i| {
            !self
                .children
                .iter()
                .any(|child| child.total_span.contains(i))
        })
    }
}

/// Partition `segments` into a [`SegmentGroupIndexed`] tree without cloning.
///
/// Stores `Range<usize>` indices into the original flat slice rather than
/// copying each [`Segment`] into the tree.  Use the original slice together
/// with [`SegmentGroupIndexed::total_span`] to access segments.
///
/// # Worked Example
///
/// Consider a simplified 3-level multi-level schema:
///
/// ```rust
/// use edifact_rs::group::{GroupDef, group_segments_indexed};
/// use edifact_rs::from_bytes;
///
/// // Schema: ROOT → SG1 (trigger: RFF) → SG5 (trigger: LOC) → SG6 (trigger: QTY)
/// static SG6: &[GroupDef] = &[GroupDef::new("SG6", "QTY")];
/// static SCHEMA: &[GroupDef] = &[
///     GroupDef::new("SG1", "RFF"),
///     GroupDef::with_children("SG5", "LOC", SG6),
/// ];
///
/// // A small multi-level message fragment (no envelope for clarity).
/// let input = b"RFF+Z13:REF1'LOC+172+DE123'DTM+163:20230101:102'QTY+220:100:KWH'";
/// let segments: Vec<_> = from_bytes(input)
///     .collect::<Result<_, _>>()
///     .unwrap();
///
/// let tree = group_segments_indexed(&segments, SCHEMA, "ROOT");
///
/// // The root contains no direct segments (all consumed by SG1 / SG5).
/// assert!(tree.direct_segment_indices().next().is_none());
///
/// // One SG1 group and one SG5 group at root level.
/// let sg1 = tree.children.iter().find(|g| g.definition == "SG1").unwrap();
/// let sg5 = tree.children.iter().find(|g| g.definition == "SG5").unwrap();
///
/// // SG1 spans the RFF segment only.
/// assert_eq!(&segments[sg1.total_span.clone()].iter().map(|s| s.tag).collect::<Vec<_>>(),
///            &["RFF"]);
///
/// // SG5 spans LOC + DTM + QTY (all three segments, including the SG6 child).
/// let sg5_tags: Vec<_> = segments[sg5.total_span.clone()].iter().map(|s| s.tag).collect();
/// assert_eq!(sg5_tags, &["LOC", "DTM", "QTY"]);
///
/// // SG5's direct segments (LOC + DTM) exclude the SG6 child (QTY).
/// let sg5_direct: Vec<_> = sg5.direct_segment_indices()
///     .map(|i| segments[i].tag)
///     .collect();
/// assert_eq!(sg5_direct, &["LOC", "DTM"]);
///
/// // SG6 contains only QTY.
/// let sg6 = sg5.children.iter().find(|g| g.definition == "SG6").unwrap();
/// assert_eq!(segments[sg6.total_span.clone()].iter().map(|s| s.tag).collect::<Vec<_>>(),
///            &["QTY"]);
/// ```
///
/// # Group validation
///
/// `group_segments_indexed` pairs naturally with
/// [`crate::validator::ValidationContext::validate_lenient_grouped`] to enforce group-presence rules:
///
/// ```rust,ignore
/// use edifact_rs::{ProfileRulePack, ValidationContext};
///
/// let pack = ProfileRulePack::new("MY-PROFILE")
///     .require_segment_in_group("SG5", "DTM", "SG5-DTM-M")
///     .forbid_segment_in_group("SG1", "LOC", "SG1-LOC-F");
/// let ctx = ValidationContext::builder().with_profile_pack(pack).build();
///
/// let tree = group_segments_indexed(&segments, SCHEMA, "ORDERS");
/// let report = ctx.validate_lenient_grouped(&tree, &segments);
/// ```
///
/// # What a group spans
///
/// Grouping is driven purely by trigger tags: a group runs from its trigger to
/// the next trigger belonging to a sibling or ancestor, or to the end of the
/// slice.  Nothing stops the final group at `UNT`, because the trailer is not a
/// trigger of anything — pass the message *body* when the group boundaries
/// matter, or accept that the trailer lands inside the last group.
///
/// # Complexity
///
/// `O(n × schema_depth)` time, `O(tree_nodes)` space.  No `Segment` clones.
pub fn group_segments_indexed<'g>(
    segments: &[Segment<'_>],
    schema: &'g [GroupDef<'g>],
    root_name: &'g str,
) -> SegmentGroupIndexed<'g> {
    let mut root = SegmentGroupIndexed {
        definition: root_name,
        total_span: 0..0,
        children: Vec::new(),
        occurrence_index: 0,
    };
    group_recursive_indexed(segments, &mut root, schema, &[], 0);
    root
}

/// Partition an owned-segment slice into a [`SegmentGroupIndexed`] tree according to `schema`.
///
/// Equivalent to [`group_segments_indexed`] but accepts `&[OwnedSegment]`.
pub fn group_owned_segments_indexed<'g>(
    segments: &[OwnedSegment],
    schema: &'g [GroupDef<'g>],
    root_name: &'g str,
) -> SegmentGroupIndexed<'g> {
    let borrowed: Vec<Segment<'_>> = segments.iter().map(|s| s.as_borrowed()).collect();
    group_segments_indexed(&borrowed, schema, root_name)
}

/// Internal recursive indexed grouping.  Returns the number of segments consumed.
fn group_recursive_indexed<'g>(
    segments: &[Segment<'_>],
    parent: &mut SegmentGroupIndexed<'g>,
    schema: &'g [GroupDef<'g>],
    stop_triggers: &[&'g str],
    offset: usize,
) -> usize {
    let combined_stop: SmallVec<[&'g str; 16]> = {
        let mut v: SmallVec<[&'g str; 16]> = SmallVec::from_slice(stop_triggers);
        for d in schema {
            if !v.contains(&d.trigger) {
                v.push(d.trigger);
            }
        }
        v
    };

    // `span_start` is the absolute index of the first segment in this group.
    // For child groups the caller pre-seeds `parent.total_span.start` with the
    // trigger segment position; for the root (or any group with no pre-seeded
    // trigger) we start at `offset`.
    let span_start = if !parent.total_span.is_empty() {
        parent.total_span.start // pre-seeded trigger position
    } else {
        offset
    };

    let mut i = 0;
    // Track how many children of each definition have been pushed at this level,
    // so we can stamp `occurrence_index` on each new child.
    let mut occ_counts: std::collections::HashMap<&'g str, usize> =
        std::collections::HashMap::new();
    while i < segments.len() {
        let tag = segments[i].tag;

        if stop_triggers.iter().copied().any(|t| t == tag) {
            break;
        }

        if let Some(def) = schema.iter().find(|d| d.trigger == tag) {
            let child_offset = offset + i;
            let occ_idx = {
                let c = occ_counts.entry(def.name).or_insert(0);
                let idx = *c;
                *c += 1;
                idx
            };
            let mut child = SegmentGroupIndexed {
                definition: def.name,
                // Pre-seed the trigger segment; the recursive call extends
                // total_span to cover the full child subtree.
                total_span: child_offset..child_offset + 1,
                children: Vec::new(),
                occurrence_index: occ_idx,
            };
            i += 1;

            let consumed = group_recursive_indexed(
                &segments[i..],
                &mut child,
                def.children,
                &combined_stop,
                offset + i,
            );
            i += consumed;

            parent.children.push(child);
        } else {
            i += 1;
        }
    }

    // Total span covers everything from the first segment (trigger or first
    // direct segment) to the last segment consumed in this call.
    parent.total_span = span_start..(offset + i);

    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;
    use crate::model::Element;

    fn seg(tag: &'static str) -> Segment<'static> {
        Segment {
            tag,
            span: Span::new(0, 0),
            tag_span: Span::new(0, 0),
            elements: vec![Element::of(&["x"])],
        }
    }

    static SCHEMA: &[GroupDef] = &[
        GroupDef {
            name: "SG1",
            trigger: "NAD",
            children: &[GroupDef {
                name: "SG2",
                trigger: "CTA",
                children: &[],
            }],
        },
        GroupDef {
            name: "SG3",
            trigger: "LIN",
            children: &[],
        },
    ];

    #[test]
    fn root_segments_before_first_trigger() {
        let segs = vec![seg("UNH"), seg("BGM"), seg("NAD")];
        let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");
        // UNH (0) and BGM (1) are direct root segments; NAD (2) is in SG1.
        let direct: Vec<_> = tree.direct_segment_indices().collect();
        assert_eq!(direct, vec![0, 1], "UNH + BGM should be direct in root");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].definition, "SG1");
    }

    #[test]
    fn repeated_trigger_creates_multiple_children() {
        let segs = vec![seg("UNH"), seg("NAD"), seg("NAD"), seg("UNT")];
        let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");
        // Two NAD triggers → two SG1 children
        assert_eq!(
            tree.children
                .iter()
                .filter(|c| c.definition == "SG1")
                .count(),
            2
        );
    }

    #[test]
    fn repeated_trigger_occurrence_index_is_stamped() {
        let segs = vec![seg("NAD"), seg("NAD"), seg("NAD")];
        let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");
        let indices: Vec<_> = tree.children.iter().map(|c| c.occurrence_index).collect();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn nested_child_groups() {
        let segs = vec![seg("NAD"), seg("CTA"), seg("CTA")];
        let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");
        let sg1 = &tree.children[0];
        assert_eq!(sg1.definition, "SG1");
        // Two CTA triggers → two SG2 children inside SG1
        assert_eq!(sg1.children.len(), 2);
        assert!(sg1.children.iter().all(|c| c.definition == "SG2"));
    }

    #[test]
    fn total_span_covers_all_segments() {
        let segs = vec![seg("UNH"), seg("NAD"), seg("CTA")];
        let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");
        // Root span covers all 3 segments
        let all_tags: Vec<_> = segs[tree.total_span.clone()]
            .iter()
            .map(|s| s.tag)
            .collect();
        assert!(all_tags.contains(&"UNH"));
        assert!(all_tags.contains(&"NAD"));
        assert!(all_tags.contains(&"CTA"));
    }
}
