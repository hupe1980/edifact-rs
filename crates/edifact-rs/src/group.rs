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
/// For read-heavy workloads consider keeping the original segment slice and
/// using group indices rather than cloned values.
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
    stack: Vec<(&'g SegmentGroup<'a>, usize, usize)>,
}

impl<'g, 'a> AllSegmentsIter<'g, 'a> {
    fn new(root: &'g SegmentGroup<'a>) -> Self {
        Self {
            stack: vec![(root, 0, 0)],
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
    let mut i = 0;
    while i < segments.len() {
        let tag = segments[i].tag;

        // If this tag is a trigger for an ancestor group, stop and return
        // so the ancestor can create a new group instance.
        if stop_triggers.contains(&tag) {
            break;
        }

        // Does this tag trigger a group in the schema?
        if let Some(def) = schema.iter().find(|d| d.trigger == tag) {
            // Start a new child group instance
            let mut child = SegmentGroup::new(def.name);
            // The trigger segment belongs to the new child
            child.segments.push(segments[i].clone());
            i += 1;

            // Build the combined stop triggers: parent stops + current schema triggers.
            // Use SmallVec to avoid heap allocation for typical schema sizes.
            let mut combined_stop: SmallVec<[&'static str; 16]> =
                SmallVec::from_slice(stop_triggers);
            for d in schema {
                if !combined_stop.contains(&d.trigger) {
                    combined_stop.push(d.trigger);
                }
            }

            // Recurse into children of this group — pass rest of segments
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
