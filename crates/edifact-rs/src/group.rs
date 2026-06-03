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

use crate::Segment;
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
/// the original `&[Segment<'a>]` together with the stored ranges:
///
/// ```rust,ignore
/// let indexed = group_segments_indexed(&segments, MY_SCHEMA, "ROOT");
/// for child in &indexed.children {
///     let child_segs = &segments[child.segment_range.clone()];
/// }
/// ```
#[derive(Debug)]
pub struct SegmentGroupIndexed {
    /// Group name from the schema, e.g. `"SG2"`, or the root name.
    pub definition: &'static str,
    /// Range of absolute indices into the original flat segment slice that
    /// belong **directly** to this group instance (not to child groups).
    ///
    /// An empty range means this group instance has no direct segments (all
    /// content lives in child groups).
    pub segment_range: Range<usize>,
    /// Child group instances, in message order.
    pub children: Vec<SegmentGroupIndexed>,
}

/// Partition `segments` into a [`SegmentGroupIndexed`] tree without cloning.
///
/// This is the zero-allocation counterpart to [`group_segments`]: instead of
/// copying each [`Segment`] into the tree, it records `Range<usize>` indices
/// into the original flat slice.  Use the original slice together with
/// [`SegmentGroupIndexed::segment_range`] to access segments:
///
/// ```rust,ignore
/// let tree = group_segments_indexed(&segments, MY_SCHEMA, "ORDERS");
/// for sg2 in tree.children.iter().filter(|g| g.definition == "SG2") {
///     let segs = &segments[sg2.segment_range.clone()];
///     println!("first segment: {:?}", segs.first().map(|s| s.tag));
/// }
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
        segment_range: 0..0,
        children: Vec::new(),
    };
    // We track the extent of direct segments belonging to `parent`.  Seed
    // from any pre-existing non-empty range so trigger segments that were
    // placed into `parent.segment_range` before calling this function are
    // not lost when the parent has no additional direct segments.
    //
    // For simplicity `segment_range` is a single contiguous span from the
    // first direct segment to the last + 1.  Non-contiguous direct segments
    // (uncommon in practice) are covered by the span without gaps.
    group_recursive_indexed(segments, &mut root, schema, &[], 0);
    root
}

/// Internal recursive indexed grouping.  Returns the number of segments consumed.
fn group_recursive_indexed<'a>(
    segments: &[Segment<'a>],
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

    // Seed from any trigger segment already placed in the parent's range.
    let mut direct_start: Option<usize> = if !parent.segment_range.is_empty() {
        Some(parent.segment_range.start)
    } else {
        None
    };
    let mut direct_end: usize = direct_start.map_or(offset, |s| s + 1);

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
                // Trigger segment starts the child's segment range; filled in below.
                segment_range: child_offset..child_offset,
                children: Vec::new(),
            };
            // Trigger segment itself is the first direct segment of the child.
            child.segment_range = child_offset..child_offset + 1;
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
            let abs = offset + i;
            if direct_start.is_none() {
                direct_start = Some(abs);
            }
            direct_end = abs + 1;
            i += 1;
        }
    }

    parent.segment_range = match direct_start {
        Some(start) => start..direct_end,
        None => offset..offset, // no direct segments
    };

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
