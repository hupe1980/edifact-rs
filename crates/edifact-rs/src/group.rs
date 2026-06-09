//! Segment group tree model for structured EDIFACT message navigation.
//!
//! Provides a recursive group schema ([`GroupDef`]) and a segment-slice-to-tree
//! function ([`group_segments`]) that partitions a flat segment slice into a
//! [`SegmentGroup`] tree according to the schema.
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
//! use edifact_rs::group::{GroupDef, group_segments};
//!
//! static ORDERS_GROUPS: &[GroupDef] = &[
//!     GroupDef { name: "SG2", trigger: "NAD", children: &[] },
//!     GroupDef {
//!         name: "SG7",
//!         trigger: "LIN",
//!         children: &[
//!             GroupDef { name: "SG32", trigger: "PRI", children: &[] },
//!         ],
//!     },
//! ];
//!
//! let root = group_segments(&segments, ORDERS_GROUPS, "ROOT");
//! for child in &root.children {
//!     println!("{}: {} segments", child.definition, child.segments.len());
//! }
//! ```

use crate::{OwnedSegment, Segment};
use smallvec::SmallVec;
use std::ops::Range;

// ── GroupDef ──────────────────────────────────────────────────────────────────

/// Static schema describing one segment group within an EDIFACT message.
///
/// `GroupDef` is designed to be declared as a `static` or `const` value, so
/// both the struct itself and all nested `children` references are
/// `'static`-lifetime slices with no heap allocation.
#[derive(Debug, Clone, Copy)]
pub struct GroupDef {
    /// Human-readable group name, e.g. `"SG2"`.
    pub name: &'static str,
    /// The segment tag whose appearance starts a new instance of this group.
    pub trigger: &'static str,
    /// Nested child groups within this group.
    ///
    /// The first trigger encountered among `children` ends the current child
    /// and starts a new one; a trigger that matches a sibling or ancestor group
    /// ends this group entirely.
    pub children: &'static [GroupDef],
}

// ── SegmentGroup ──────────────────────────────────────────────────────────────

/// A populated segment group produced by [`group_segments`].
///
/// Each segment is cloned (shallow copy) from the input slice: the `Vec` of
/// elements is heap-allocated per segment, but the string data inside each
/// element still borrows from the original input buffer via the `'a` lifetime.
///
/// # Memory note
///
/// `group_segments` clones each [`Segment`] into the tree.  For large messages
/// or deeply nested schemas where minimal allocation is critical, consider
/// working with the original flat segment slice and deriving group boundaries
/// yourself based on the schema's trigger tags.
#[derive(Debug)]
pub struct SegmentGroup<'a> {
    /// Group name from the schema, e.g. `"SG2"`, or `"ROOT"` for the envelope.
    pub definition: &'static str,
    /// Segments that belong directly to this group instance.
    ///
    /// Segment values are cloned from the input slice, but the string data
    /// inside each segment borrows from the original input for `'a`.
    pub segments: Vec<Segment<'a>>,
    /// Child group instances, in the order they appear in the message.
    pub children: Vec<SegmentGroup<'a>>,
}

impl<'a> SegmentGroup<'a> {
    fn new(definition: &'static str) -> Self {
        Self {
            definition,
            segments: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Iterate over all segments in this group and all descendant groups,
    /// depth-first.
    pub fn all_segments(&self) -> impl Iterator<Item = &Segment<'a>> + '_ {
        AllSegmentsIter::new(self)
    }

    /// Find the first segment with the given `tag` in this group (not children).
    ///
    /// # Shallow search
    ///
    /// This method searches only the segments directly owned by **this** group
    /// instance — it does **not** recurse into child groups.  To search the
    /// entire subtree use [`SegmentGroup::all_segments`] with [`Iterator::find`]:
    ///
    /// ```ignore
    /// group.all_segments().find(|s| s.tag == "LIN")
    /// ```
    pub fn find_segment(&self, tag: &str) -> Option<&Segment<'a>> {
        self.segments.iter().find(|s| s.tag == tag)
    }
}

// ── AllSegmentsIter ───────────────────────────────────────────────────────────

struct AllSegmentsIter<'g, 'a> {
    // Stack of (current_group, current_seg_idx, current_child_idx)
    stack: SmallVec<[(&'g SegmentGroup<'a>, usize, usize); 8]>,
}

impl<'g, 'a> AllSegmentsIter<'g, 'a> {
    fn new(root: &'g SegmentGroup<'a>) -> Self {
        Self {
            stack: smallvec::smallvec![(root, 0, 0)],
        }
    }
}

impl<'g, 'a> Iterator for AllSegmentsIter<'g, 'a> {
    type Item = &'g Segment<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (group, seg_idx, child_idx) = self.stack.last_mut()?;
            // Yield segments first.
            if *seg_idx < group.segments.len() {
                let seg = &group.segments[*seg_idx];
                *seg_idx += 1;
                return Some(seg);
            }
            // Then recurse into children
            if *child_idx < group.children.len() {
                let child = &group.children[*child_idx];
                *child_idx += 1;
                self.stack.push((child, 0, 0));
                continue;
            }
            // Done with this group
            self.stack.pop();
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // Conservative lower bound: count remaining direct segments in all
        // frames on the stack.  Children not yet pushed are not counted, so
        // the true total may be higher, but this is still a valid lower bound.
        let lower: usize = self
            .stack
            .iter()
            .map(|(g, seg_idx, _)| g.segments.len().saturating_sub(*seg_idx))
            .sum();
        (lower, None)
    }
}

// ── group_segments ────────────────────────────────────────────────────────────

/// Partition `segments` into a [`SegmentGroup`] tree according to `schema`.
///
/// # Algorithm
///
/// The algorithm is a single-pass linear scan:
///
/// 1. Segments that do not match any group trigger in `schema` are added to
///    the current group's `segments`.
/// 2. When a trigger matching a group in `schema` is encountered:
///    - If an open child with the same trigger already exists it is closed and
///      a new instance is started (repetition).
///    - If the trigger belongs to a *sibling* or *ancestor* group the current
///      group is closed first (the caller handles restart).
///    - Nested schemas recurse: child group triggers follow the same rules
///      within their parent.
///
/// # Root group
///
/// The returned root group has `definition` set to `root_name` (typically
/// `"ROOT"` or the message type string).  Segments before the first matching
/// trigger land in the root's own `segments` vec.
///
/// # Example
///
/// ```rust,ignore
/// let tree = group_segments(&segments, MY_SCHEMA, "ORDERS");
/// for sg2 in tree.children.iter().filter(|g| g.definition == "SG2") {
///     println!("NAD group: {:?}", sg2.segments.iter().map(|s| s.tag).collect::<Vec<_>>());
/// }
/// ```
pub fn group_segments<'a>(
    segments: &[Segment<'a>],
    schema: &'static [GroupDef],
    root_name: &'static str,
) -> SegmentGroup<'a> {
    let mut root = SegmentGroup::new(root_name);
    group_recursive(segments, &mut root, schema);
    root
}

/// Internal recursive grouping.  Returns the number of segments consumed.
fn group_recursive<'a>(
    segments: &[Segment<'a>],
    parent: &mut SegmentGroup<'a>,
    schema: &'static [GroupDef],
) -> usize {
    group_recursive_inner(segments, parent, schema, &[])
}

fn group_recursive_inner<'a>(
    segments: &[Segment<'a>],
    parent: &mut SegmentGroup<'a>,
    schema: &'static [GroupDef],
    stop_triggers: &[&'static str],
) -> usize {
    // Pre-compute the combined stop set for all children of this schema level.
    // This is computed once per schema level, not once per segment, so the
    // SmallVec is allocated at most O(depth) times rather than O(depth × n).
    let combined_stop: SmallVec<[&'static str; 16]> = {
        let mut v: SmallVec<[&'static str; 16]> = SmallVec::from_slice(stop_triggers);
        for d in schema {
            if !v.contains(&d.trigger) {
                v.push(d.trigger);
            }
        }
        v
    };

    let mut i = 0;
    while i < segments.len() {
        let tag = segments[i].tag;

        // A trigger matching a stop tag means we must return control to the parent group.
        if stop_triggers.iter().copied().any(|t| t == tag) {
            break;
        }

        // Does this tag trigger a group in the schema?
        if let Some(def) = schema.iter().find(|d| d.trigger == tag) {
            // Start a new child group instance
            let mut child = SegmentGroup::new(def.name);
            // The trigger segment belongs to the new child
            child.segments.push(segments[i].clone());
            i += 1;

            // Recurse into children of this group — pass the pre-computed stop set.
            let consumed =
                group_recursive_inner(&segments[i..], &mut child, def.children, &combined_stop);
            i += consumed;

            parent.children.push(child);
        } else {
            // Segment doesn't match any group trigger in this schema — it
            // belongs to the parent group's own segments.  This also covers
            // leaf groups (empty schema): all non-stop-trigger segments after
            // the trigger are accumulated into the current group.
            parent.segments.push(segments[i].clone());
            i += 1;
        }
    }
    i
}

// ── SegmentGroupIndexed ───────────────────────────────────────────────────────

/// A zero-clone counterpart to [`SegmentGroup`] that stores index ranges into
/// the original flat segment slice rather than cloning each segment.
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
pub struct SegmentGroupIndexed {
    /// Group name from the schema, e.g. `"SG2"`, or the root name.
    pub definition: &'static str,
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
    pub children: Vec<SegmentGroupIndexed>,
}

impl SegmentGroupIndexed {
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
/// This is the zero-allocation counterpart to [`group_segments`]: instead of
/// copying each [`Segment`] into the tree, it records `Range<usize>` indices
/// into the original flat slice.  Use the original slice together with
/// [`SegmentGroupIndexed::total_span`] to access segments.
///
/// # Worked Example
///
/// Consider a simplified 3-level MSCONS-like schema:
///
/// ```rust
/// use edifact_rs::group::{GroupDef, group_segments_indexed};
/// use edifact_rs::from_bytes;
///
/// // Schema: ROOT → SG1 (trigger: RFF) → SG5 (trigger: LOC) → SG6 (trigger: QTY)
/// static SCHEMA: &[GroupDef] = &[
///     GroupDef { name: "SG1", trigger: "RFF", children: &[] },
///     GroupDef {
///         name: "SG5",
///         trigger: "LOC",
///         children: &[
///             GroupDef { name: "SG6", trigger: "QTY", children: &[] },
///         ],
///     },
/// ];
///
/// // A small MSCONS-like message fragment (no envelope for clarity).
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
/// [`ValidationContext::validate_lenient_grouped`] to enforce group-presence rules:
///
/// ```rust,ignore
/// use edifact_rs::{ProfileRulePack, ValidationContext};
///
/// let pack = ProfileRulePack::new("MY-AHB")
///     .require_segment_in_group("SG5", "DTM", "SG5-DTM-M")
///     .forbid_segment_in_group("SG1", "LOC", "SG1-LOC-F");
/// let ctx = ValidationContext::builder().with_profile_pack(pack).build();
///
/// let tree = group_segments_indexed(&segments, SCHEMA, "MSCONS");
/// let report = ctx.validate_lenient_grouped(&tree, &segments);
/// ```
///
/// # Complexity
///
/// `O(n × schema_depth)` time, `O(tree_nodes)` space.  No `Segment` clones.
pub fn group_segments_indexed<'a>(
    segments: &[Segment<'a>],
    schema: &'static [GroupDef],
    root_name: &'static str,
) -> SegmentGroupIndexed {
    let mut root = SegmentGroupIndexed {
        definition: root_name,
        total_span: 0..0,
        children: Vec::new(),
    };
    group_recursive_indexed(segments, &mut root, schema, &[], 0);
    root
}

/// Partition an owned-segment slice into a [`SegmentGroup`] tree according to `schema`.
///
/// Equivalent to [`group_segments`] but accepts `&[OwnedSegment]` for use with
/// the reader-based API ([`crate::from_reader`] → [`crate::OwnedSegmentStream`]).
///
/// Internally borrows each `OwnedSegment` as a `Segment<'_>` and delegates to
/// [`group_segments`], so all grouping logic is shared.
pub fn group_owned_segments<'a>(
    segments: &'a [OwnedSegment],
    schema: &'static [GroupDef],
    root_name: &'static str,
) -> SegmentGroup<'a> {
    let borrowed: Vec<Segment<'a>> = segments.iter().map(|s| s.as_borrowed()).collect();
    group_segments(&borrowed, schema, root_name)
}

/// Partition an owned-segment slice into a [`SegmentGroupIndexed`] tree according to `schema`.
///
/// Equivalent to [`group_segments_indexed`] but accepts `&[OwnedSegment]`.
pub fn group_owned_segments_indexed(
    segments: &[OwnedSegment],
    schema: &'static [GroupDef],
    root_name: &'static str,
) -> SegmentGroupIndexed {
    let borrowed: Vec<Segment<'_>> = segments.iter().map(|s| s.as_borrowed()).collect();
    group_segments_indexed(&borrowed, schema, root_name)
}

/// Internal recursive indexed grouping.  Returns the number of segments consumed.
fn group_recursive_indexed<'a>(    segments: &[Segment<'a>],
    parent: &mut SegmentGroupIndexed,
    schema: &'static [GroupDef],
    stop_triggers: &[&'static str],
    offset: usize,
) -> usize {
    let combined_stop: SmallVec<[&'static str; 16]> = {
        let mut v: SmallVec<[&'static str; 16]> = SmallVec::from_slice(stop_triggers);
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
    while i < segments.len() {
        let tag = segments[i].tag;

        if stop_triggers.iter().copied().any(|t| t == tag) {
            break;
        }

        if let Some(def) = schema.iter().find(|d| d.trigger == tag) {
            let child_offset = offset + i;
            let mut child = SegmentGroupIndexed {
                definition: def.name,
                // Pre-seed the trigger segment; the recursive call extends
                // total_span to cover the full child subtree.
                total_span: child_offset..child_offset + 1,
                children: Vec::new(),
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
        let tree = group_segments(&segs, SCHEMA, "ROOT");
        assert_eq!(tree.segments.len(), 2, "UNH + BGM should be in root");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].definition, "SG1");
    }

    #[test]
    fn repeated_trigger_creates_multiple_children() {
        let segs = vec![seg("UNH"), seg("NAD"), seg("NAD"), seg("UNT")];
        let tree = group_segments(&segs, SCHEMA, "ROOT");
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
    fn nested_child_groups() {
        let segs = vec![seg("NAD"), seg("CTA"), seg("CTA")];
        let tree = group_segments(&segs, SCHEMA, "ROOT");
        let sg1 = &tree.children[0];
        assert_eq!(sg1.definition, "SG1");
        // Two CTA triggers → two SG2 children inside SG1
        assert_eq!(sg1.children.len(), 2);
        assert!(sg1.children.iter().all(|c| c.definition == "SG2"));
    }

    #[test]
    fn all_segments_iterator_depth_first() {
        let segs = vec![seg("UNH"), seg("NAD"), seg("CTA")];
        let tree = group_segments(&segs, SCHEMA, "ROOT");
        let tags: Vec<_> = tree.all_segments().map(|s| s.tag).collect();
        assert!(tags.contains(&"UNH"));
        assert!(tags.contains(&"NAD"));
        assert!(tags.contains(&"CTA"));
    }
}
