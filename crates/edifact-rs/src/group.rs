//! Segment group tree model for structured EDIFACT message navigation.
//!
//! Provides a group schema ([`GroupDef`], [`SegmentPlace`], [`GroupSchema`]) and
//! a resolver that partitions a flat segment slice into a
//! [`SegmentGroupIndexed`] tree according to it — [`group_segments_indexed`]
//! for a plain list of groups, [`GroupSchema::resolve`] when the placement of
//! every segment and the findings matter as well.
//!
//! # Model overview
//!
//! Every UN/EDIFACT message type has a fixed set of **segment groups**: named,
//! optionally-repeating sets of segments delimited by a specific *trigger*
//! segment tag.  For example, ORDERS D.11A has an `SG1` group starting with
//! `RFF`, an `SG2` group starting with `NAD`, and so on.
//!
//! A group is one of two kinds:
//!
//! - **Open** — it declares only its trigger and its child groups
//!   ([`GroupDef::new`], [`GroupDef::with_children`], or a builder that never
//!   calls [`segment`](GroupDefBuilder::segment)). Every segment that triggers
//!   nothing is a direct segment of whichever open group is current.
//! - **Ordered** — it declares its member segments as an implementation guide
//!   (MIG) lists them: trigger first, then member [`SegmentPlace`]s and child
//!   groups in guide order, each with `min`/`max` occurrences and optional
//!   [`Discriminator`]s ([`GroupDef::builder`] with at least one
//!   [`segment`](GroupDefBuilder::segment), or marked
//!   [`ordered`](GroupDefBuilder::ordered)). Its entries are walked front to
//!   back, and a segment that fits no entry is **unplaced** — recorded with an
//!   [`UnplacedReason`], never attached to a wrong group.
//!
//! Both kinds nest inside each other.
//!
//! # How a tag is resolved
//!
//! For each segment, innermost open group first:
//!
//! 1. **The innermost group's own candidates.** Open: its child-group triggers,
//!    in declaration order. Ordered: its current entry, then later entries;
//!    the first that matches wins, except that a place already at its `max`
//!    yields to a later entry that also matches.
//! 2. **The reachable places of each enclosing group, innermost first.** Open:
//!    all its child-group triggers. Ordered: its current and later entries — so
//!    a repeated group trigger opens the next occurrence. A match closes the
//!    groups inside it.
//! 3. **Otherwise**, in an open group the segment is a direct segment of it; in
//!    an ordered group it is unplaced.
//!
//! Step 1 comes first because the same trigger tag can appear at more
//! than one level — `UTILMD` triggers both SG2 (message-level parties) and SG12
//! (a Vorgang's parties, inside SG4) on `NAD`. A group ends at the first segment
//! the current branch cannot consume, never at one an outer branch could also
//! have consumed; the other order would make SG12 unreachable from any input.
//!
//! A group's own trigger is not among its candidates, so a repeated trigger
//! reaches step 2 and opens the **next occurrence** rather than nesting.
//!
//! ## Shared tags: why member declarations matter
//!
//! In `SG4(IDE){SG6(RFF), SG8(SEQ)}` with every group open, an `RFF` after a
//! `SEQ` opens a second SG6. An open schema builds that tree. Declare SG8's
//! members and step 1 places the `RFF` (and the `CCI` after it) in SG8:
//!
//! ```
//! use edifact_rs::group::{GroupDef, SegmentPlace, group_segments_indexed};
//!
//! let sg8 = GroupDef::builder("SG8", SegmentPlace::new("SEQ"))
//!     .segment(SegmentPlace::new("RFF"))
//!     .segment(SegmentPlace::new("CCI"))
//!     .segment(SegmentPlace::new("QTY"))
//!     .build()?;
//! let sg4 = GroupDef::builder("SG4", SegmentPlace::new("IDE"))
//!     .group(GroupDef::new("SG6", "RFF"))
//!     .group(sg8)
//!     .build()?;
//! let schema = [sg4];
//!
//! let segments: Vec<_> = edifact_rs::from_bytes(
//!     b"IDE+24+X'RFF+Z13:1'SEQ+Z01'RFF+Z19:ABC'CCI+Z30'SEQ+Z02'QTY+1:1'",
//! )
//! .collect::<Result<_, _>>()?;
//! let tree = group_segments_indexed(&segments, &schema, "ROOT");
//!
//! let sg4 = &tree.children[0];
//! let shape: Vec<(&str, usize, usize)> = sg4
//!     .children
//!     .iter()
//!     .map(|g| (g.definition, g.occurrence_index, g.total_span.len()))
//!     .collect();
//! assert_eq!(shape, [("SG6", 0, 1), ("SG8", 0, 3), ("SG8", 1, 2)]);
//! # Ok::<(), edifact_rs::EdifactError>(())
//! ```
//!
//! # What a group spans
//!
//! In an open group a tag that triggers nothing — `UNT`, for one — is a direct
//! segment of whichever group is open when it arrives. Pass the message **body**
//! ([`MessageWindow::body`][crate::MessageWindow::body]) rather than the full
//! window when the trailing boundary matters, or declare `UNT` as the last entry
//! of an ordered root ([`GroupSchema::ordered`]).
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

use crate::report::{GroupPath, GroupStep, ValidationIssue};
use crate::validator::ValidationLayer;
use crate::{EdifactError, Segment};
use smallvec::SmallVec;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// Deepest group nesting [`GroupDefBuilder::build`] and [`GroupSchema`] accept.
const MAX_DEPTH: usize = 64;

// ── storage ───────────────────────────────────────────────────────────────────

/// A string that is either `&'static` (const constructors) or shared.
#[derive(Clone)]
enum Text {
    Static(&'static str),
    Shared(Arc<str>),
}

impl Text {
    fn as_str(&self) -> &str {
        match self {
            Self::Static(s) => s,
            Self::Shared(s) => s,
        }
    }
}

impl std::fmt::Debug for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}

// ── Discriminator ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    OneOf {
        codes: Arc<[Arc<str>]>,
        optional: bool,
        accept_unclaimed: bool,
    },
    NotUsed,
}

/// A coded position that tells a [`SegmentPlace`] apart from other places of
/// the same tag, e.g. "`LOC` element 0 is `Z16`".
///
/// Positions are zero-based: `element` is the data element after the tag,
/// `component` the component inside it (`0` for a simple element). The value is
/// read from the element's **first repetition**; an empty or missing value is
/// *absent*.
///
/// | Rule | value absent | value admitted | other value |
/// |---|---|---|---|
/// | [`one_of`](Self::one_of) | no match, unless [`optional`](Self::optional) | match | no match, unless [`accept_unclaimed`](Self::accept_unclaimed) and no place of the tag admits it here |
/// | [`not_used`](Self::not_used) | match | — | no match |
///
/// # Example
///
/// ```
/// use edifact_rs::group::{Discriminator, SegmentPlace};
///
/// // `LOC+Z16` — the Marktlokation place.
/// let malo = SegmentPlace::new("LOC").when(Discriminator::one_of(0, 0, ["Z16"]));
/// assert_eq!(malo.discriminators()[0].codes()[0].as_ref(), "Z16");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discriminator {
    element: u16,
    component: u16,
    rule: Rule,
}

impl Discriminator {
    /// The value at (`element`, `component`) must be one of `codes`.
    ///
    /// Strict by default: an absent value does not match (see
    /// [`optional`](Self::optional)), nor does a code outside the list (see
    /// [`accept_unclaimed`](Self::accept_unclaimed)). An empty `codes` list is
    /// refused when the schema is built.
    #[must_use]
    pub fn one_of<I, S>(element: u16, component: u16, codes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Arc<str>>,
    {
        Self {
            element,
            component,
            rule: Rule::OneOf {
                codes: codes.into_iter().map(Into::into).collect(),
                optional: false,
                accept_unclaimed: false,
            },
        }
    }

    /// The position must be empty: a value there rules the place out, because
    /// it belongs to a place that uses it.
    #[must_use]
    pub const fn not_used(element: u16, component: u16) -> Self {
        Self {
            element,
            component,
            rule: Rule::NotUsed,
        }
    }

    /// An absent value cannot rule the place out (an optional qualifier).
    ///
    /// No effect on [`not_used`](Self::not_used).
    #[must_use]
    pub fn optional(mut self) -> Self {
        if let Rule::OneOf { optional, .. } = &mut self.rule {
            *optional = true;
        }
        self
    }

    /// A value that **no place of the same tag** admits at this position still
    /// matches — it is a code fault at this place, not a misplaced segment.
    ///
    /// A value that another place admits is never accepted here. No effect on
    /// [`not_used`](Self::not_used).
    #[must_use]
    pub fn accept_unclaimed(mut self) -> Self {
        if let Rule::OneOf {
            accept_unclaimed, ..
        } = &mut self.rule
        {
            *accept_unclaimed = true;
        }
        self
    }

    /// Zero-based data element index.
    #[must_use]
    pub fn element(&self) -> u16 {
        self.element
    }

    /// Zero-based component index (`0` for a simple element).
    #[must_use]
    pub fn component(&self) -> u16 {
        self.component
    }

    /// The admitted codes; empty for [`not_used`](Self::not_used).
    #[must_use]
    pub fn codes(&self) -> &[Arc<str>] {
        match &self.rule {
            Rule::OneOf { codes, .. } => codes,
            Rule::NotUsed => &[],
        }
    }

    /// Whether this is a [`not_used`](Self::not_used) position.
    #[must_use]
    pub fn is_not_used(&self) -> bool {
        matches!(self.rule, Rule::NotUsed)
    }

    /// Whether an absent value matches ([`optional`](Self::optional)).
    #[must_use]
    pub fn is_optional(&self) -> bool {
        matches!(self.rule, Rule::OneOf { optional: true, .. })
    }

    /// Whether unclaimed values match ([`accept_unclaimed`](Self::accept_unclaimed)).
    #[must_use]
    pub fn accepts_unclaimed(&self) -> bool {
        matches!(
            self.rule,
            Rule::OneOf {
                accept_unclaimed: true,
                ..
            }
        )
    }

    fn admits(&self, tag: &str, segment: &Segment<'_>, tables: &Tables) -> bool {
        let value = segment
            .component_str(usize::from(self.element), usize::from(self.component))
            .filter(|v| !v.is_empty());
        match (&self.rule, value) {
            (Rule::NotUsed, value) => value.is_none(),
            (Rule::OneOf { optional, .. }, None) => *optional,
            (
                Rule::OneOf {
                    codes,
                    accept_unclaimed,
                    ..
                },
                Some(value),
            ) => {
                codes.iter().any(|c| &**c == value)
                    || (*accept_unclaimed
                        && !tables.claimed(tag, self.element, self.component, value))
            }
        }
    }
}

// ── SegmentPlace ──────────────────────────────────────────────────────────────

/// One place a segment can occupy: a tag, optional [`Discriminator`]s that must
/// all match, `min`/`max` occurrences per group instance, and an optional
/// caller-defined id (e.g. a MIG segment number) returned with placements and
/// findings.
///
/// Used as a group's trigger and as a member of an ordered group.
///
/// # Example
///
/// ```
/// use edifact_rs::group::{Discriminator, SegmentPlace};
///
/// let place = SegmentPlace::owned(String::from("DTM"))
///     .with_id("00020")
///     .when(Discriminator::one_of(0, 0, ["92"]))
///     .with_max(1);
/// assert_eq!((place.tag(), place.id(), place.max()), ("DTM", Some("00020"), Some(1)));
/// ```
#[derive(Debug, Clone)]
pub struct SegmentPlace {
    tag: Text,
    id: Option<Text>,
    discriminators: Option<Arc<[Discriminator]>>,
    min: u32,
    max: Option<u32>,
}

impl SegmentPlace {
    /// A place for `tag` with no discriminators, `min` 0 and no `max`.
    #[must_use]
    pub const fn new(tag: &'static str) -> Self {
        Self {
            tag: Text::Static(tag),
            id: None,
            discriminators: None,
            min: 0,
            max: None,
        }
    }

    /// [`new`](Self::new) for a tag only known at run time.
    #[must_use]
    pub fn owned(tag: impl Into<Arc<str>>) -> Self {
        Self {
            tag: Text::Shared(tag.into()),
            ..Self::new("")
        }
    }

    /// Set the caller-defined id (e.g. the MIG segment number).
    #[must_use]
    pub fn with_id(mut self, id: impl Into<Arc<str>>) -> Self {
        self.id = Some(Text::Shared(id.into()));
        self
    }

    /// Add a discriminator; all of a place's discriminators must match.
    #[must_use]
    pub fn when(mut self, discriminator: Discriminator) -> Self {
        let mut all: Vec<Discriminator> = self.discriminators().to_vec();
        all.push(discriminator);
        self.discriminators = Some(all.into());
        self
    }

    /// Minimum occurrences per group instance (default 0).
    #[must_use]
    pub fn with_min(mut self, min: u32) -> Self {
        self.min = min;
        self
    }

    /// Maximum occurrences per group instance (default unbounded).
    #[must_use]
    pub fn with_max(mut self, max: u32) -> Self {
        self.max = Some(max);
        self
    }

    /// The segment tag.
    #[must_use]
    pub fn tag(&self) -> &str {
        self.tag.as_str()
    }

    /// The caller-defined id, if set.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_ref().map(Text::as_str)
    }

    /// The discriminators, in the order they were added.
    #[must_use]
    pub fn discriminators(&self) -> &[Discriminator] {
        self.discriminators.as_deref().unwrap_or(&[])
    }

    /// Minimum occurrences per group instance.
    #[must_use]
    pub fn min(&self) -> u32 {
        self.min
    }

    /// Maximum occurrences per group instance; `None` is unbounded.
    #[must_use]
    pub fn max(&self) -> Option<u32> {
        self.max
    }

    fn matches(&self, segment: &Segment<'_>, tables: &Tables) -> bool {
        let tag = self.tag();
        segment.tag() == tag
            && self
                .discriminators()
                .iter()
                .all(|d| d.admits(tag, segment, tables))
    }
}

// ── Entry ─────────────────────────────────────────────────────────────────────

/// One entry of an ordered group or an ordered [`GroupSchema`] root.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum Entry {
    /// A segment place.
    Segment(SegmentPlace),
    /// A nested group.
    Group(GroupDef),
}

// ── GroupDef ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Body {
    /// Open, built by the const constructors.
    Static(&'static [GroupDef]),
    /// Open, built by the builder: every entry is a group.
    Open(Arc<[Entry]>),
    /// Ordered: entry 0 is the trigger.
    Ordered(Arc<[Entry]>),
}

/// Schema describing one segment group within an EDIFACT message.
///
/// A group is **open** or **ordered** — see [the module
/// documentation][self#model-overview]. The const constructors build open
/// groups over `'static` data, so a schema table costs no allocation:
///
/// ```
/// use edifact_rs::group::GroupDef;
///
/// static SG32: &[GroupDef] = &[GroupDef::new("SG32", "PRI")];
/// static SCHEMA: &[GroupDef] = &[
///     GroupDef::new("SG2", "NAD"),
///     GroupDef::with_children("SG7", "LIN", SG32),
/// ];
///
/// trait MessageSchema {
///     fn groups(&self) -> &'static [GroupDef];
/// }
/// struct Orders;
/// impl MessageSchema for Orders {
///     fn groups(&self) -> &'static [GroupDef] {
///         SCHEMA
///     }
/// }
/// assert_eq!(Orders.groups()[1].children().next().unwrap().name(), "SG32");
/// ```
///
/// A child slice must be a named `static` (or `const`) of its own, as `SG32`
/// is above. Written inline inside another constant expression —
/// `GroupDef::with_children("SG7", "LIN", &[GroupDef::new("SG32", "PRI")])` —
/// it fails to compile with `E0493` ("destructor cannot be evaluated at
/// compile-time"): `GroupDef` can own [`Arc`]-shared data, so a temporary
/// array of it is only lifetime-extended at the top of the initializer.
///
/// [`GroupDef::builder`] builds groups from run-time strings (shared behind
/// [`Arc`], no leaks) and is the only way to declare member segments, an id or
/// bounds. Cloning is O(1).
#[derive(Debug, Clone)]
pub struct GroupDef {
    name: Text,
    id: Option<Text>,
    trigger: SegmentPlace,
    body: Body,
    min: u32,
    max: Option<u32>,
}

impl GroupDef {
    /// An open leaf group: `name` is opened by `trigger` and has no nested groups.
    #[must_use]
    pub const fn new(name: &'static str, trigger: &'static str) -> Self {
        Self::with_children(name, trigger, &[])
    }

    /// An open group with nested child groups.
    ///
    /// See [the resolution rule][self#how-a-tag-is-resolved] for what happens
    /// when a tag could open a child here *and* a group further out — the
    /// nested definition wins, which is what makes a child group whose trigger
    /// is shared with an outer group reachable at all.
    #[must_use]
    pub const fn with_children(
        name: &'static str,
        trigger: &'static str,
        children: &'static [GroupDef],
    ) -> Self {
        Self {
            name: Text::Static(name),
            id: None,
            trigger: SegmentPlace::new(trigger),
            body: Body::Static(children),
            min: 0,
            max: None,
        }
    }

    /// Start building a group named `name`, opened by `trigger`.
    ///
    /// The group is **ordered** as soon as one member segment is declared with
    /// [`segment`](GroupDefBuilder::segment) or [`ordered`](GroupDefBuilder::ordered)
    /// is called, and open otherwise.
    pub fn builder(name: impl Into<Arc<str>>, trigger: SegmentPlace) -> GroupDefBuilder {
        GroupDefBuilder {
            name: name.into(),
            id: None,
            trigger,
            entries: Vec::new(),
            ordered: false,
            min: 0,
            max: None,
        }
    }

    /// The group name, e.g. `"SG2"`.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The caller-defined id, if set.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_ref().map(Text::as_str)
    }

    /// The place whose segment opens an instance of this group.
    #[must_use]
    pub fn trigger(&self) -> &SegmentPlace {
        &self.trigger
    }

    /// The tag of the [`trigger`](Self::trigger) segment.
    #[must_use]
    pub fn trigger_tag(&self) -> &str {
        self.trigger.tag()
    }

    /// Whether the group declares member segments (see [the module
    /// documentation][self#model-overview]).
    #[must_use]
    pub fn is_ordered(&self) -> bool {
        matches!(self.body, Body::Ordered(_))
    }

    /// The entries of an ordered group in guide order, the trigger first.
    ///
    /// Empty for an open group, whose child groups are [`children`](Self::children).
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        match &self.body {
            Body::Ordered(entries) => entries,
            Body::Static(_) | Body::Open(_) => &[],
        }
    }

    /// The child groups, in declaration order.
    pub fn children(&self) -> impl Iterator<Item = &GroupDef> + '_ {
        let (groups, entries): (&[GroupDef], &[Entry]) = match &self.body {
            Body::Static(groups) => (groups, &[]),
            Body::Open(entries) | Body::Ordered(entries) => (&[], entries),
        };
        groups.iter().chain(entries.iter().filter_map(|e| match e {
            Entry::Group(g) => Some(g),
            Entry::Segment(_) => None,
        }))
    }

    /// Minimum occurrences per parent instance (default 0).
    #[must_use]
    pub fn min(&self) -> u32 {
        self.min
    }

    /// Maximum occurrences per parent instance; `None` (the default) is unbounded.
    #[must_use]
    pub fn max(&self) -> Option<u32> {
        self.max
    }

    /// The members the resolver walks for an instance of this group.
    fn members(&self) -> (Members<'_>, bool) {
        match &self.body {
            Body::Static(groups) => (Members::Groups(groups), false),
            Body::Open(entries) => (Members::Entries(entries), false),
            Body::Ordered(entries) => (Members::Entries(entries), true),
        }
    }
}

/// Builder for an owned [`GroupDef`], returned by [`GroupDef::builder`].
#[derive(Debug, Clone)]
#[must_use]
pub struct GroupDefBuilder {
    name: Arc<str>,
    id: Option<Arc<str>>,
    trigger: SegmentPlace,
    entries: Vec<Entry>,
    ordered: bool,
    min: u32,
    max: Option<u32>,
}

impl GroupDefBuilder {
    /// Set the caller-defined id (e.g. a MIG group number).
    pub fn id(mut self, id: impl Into<Arc<str>>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Declare the next member segment place; makes the group **ordered**.
    pub fn segment(mut self, place: SegmentPlace) -> Self {
        self.ordered = true;
        self.entries.push(Entry::Segment(place));
        self
    }

    /// Make the group **ordered** even though it declares no member segment.
    ///
    /// A MIG group often holds only its trigger and child groups (`SG5` is just
    /// `LOC`); declared this way it refuses everything else — an unexpected
    /// segment is unplaced rather than a direct segment of the group. Without
    /// it, and without [`segment`](Self::segment), the group is open.
    pub fn ordered(mut self) -> Self {
        self.ordered = true;
        self
    }

    /// Declare the next child group.
    pub fn group(mut self, group: GroupDef) -> Self {
        self.entries.push(Entry::Group(group));
        self
    }

    /// Minimum occurrences per parent instance (default 0).
    pub fn min(mut self, min: u32) -> Self {
        self.min = min;
        self
    }

    /// Maximum occurrences per parent instance (default unbounded).
    pub fn max(mut self, max: u32) -> Self {
        self.max = Some(max);
        self
    }

    /// Build the group, checking its whole subtree.
    ///
    /// # Errors
    ///
    /// [`EdifactError::InvalidGroupSchema`] when a group name is empty, a tag
    /// is not three upper-case ASCII letters, `min` exceeds `max`, `max` is 0,
    /// a [`Discriminator::one_of`] admits no code, or the subtree nests deeper
    /// than 64 groups.
    pub fn build(self) -> Result<GroupDef, EdifactError> {
        let body = if self.ordered {
            let mut entries = Vec::with_capacity(self.entries.len() + 1);
            entries.push(Entry::Segment(self.trigger.clone()));
            entries.extend(self.entries);
            Body::Ordered(entries.into())
        } else {
            Body::Open(self.entries.into())
        };
        let group = GroupDef {
            name: Text::Shared(self.name),
            id: self.id.map(Text::Shared),
            trigger: self.trigger,
            body,
            min: self.min,
            max: self.max,
        };
        check_group(&group, 1)?;
        Ok(group)
    }
}

// ── schema checks ─────────────────────────────────────────────────────────────

fn invalid(group: &str, reason: &'static str) -> EdifactError {
    EdifactError::InvalidGroupSchema {
        group: group.to_owned(),
        reason,
    }
}

fn check_bounds(group: &str, min: u32, max: Option<u32>) -> Result<(), EdifactError> {
    match max {
        Some(0) => Err(invalid(group, "max is 0")),
        Some(max) if min > max => Err(invalid(group, "min exceeds max")),
        _ => Ok(()),
    }
}

fn check_place(group: &str, place: &SegmentPlace) -> Result<(), EdifactError> {
    if !crate::tokenizer::is_valid_segment_tag(place.tag()) {
        return Err(invalid(
            group,
            "segment tag is not three upper-case ASCII letters",
        ));
    }
    if place.discriminators().iter().any(|d| match &d.rule {
        Rule::OneOf { codes, .. } => codes.is_empty(),
        Rule::NotUsed => false,
    }) {
        return Err(invalid(group, "one_of discriminator admits no code"));
    }
    check_bounds(group, place.min, place.max)
}

/// Check `group` and its subtree; `depth` is the group's own nesting level.
///
/// Recursion stops at [`MAX_DEPTH`], so it is bounded even for a static table.
fn check_group(group: &GroupDef, depth: usize) -> Result<(), EdifactError> {
    let name = group.name();
    if depth > MAX_DEPTH {
        return Err(invalid(name, "groups nest deeper than 64 levels"));
    }
    if name.is_empty() {
        return Err(invalid(name, "group name is empty"));
    }
    check_bounds(name, group.min, group.max)?;
    check_place(name, &group.trigger)?;
    let (members, _) = group.members();
    for j in 0..members.len() {
        match members.get(j) {
            Cand::Seg(place) => check_place(name, place)?,
            Cand::Group(child) => check_group(child, depth + 1)?,
        }
    }
    Ok(())
}

// ── SegmentGroupIndexed ───────────────────────────────────────────────────────

/// Zero-copy segment group tree.  Stores index ranges into the original flat
/// segment slice rather than cloning each segment.
///
/// Produced by [`group_segments_indexed`] and [`GroupSchema::resolve`].  To
/// access the actual segments use the original `&[Segment<'a>]` together with
/// [`total_span`]:
///
/// ```rust,ignore
/// let indexed = group_segments_indexed(&segments, MY_SCHEMA, "ROOT");
/// for child in &indexed.children {
///     let child_segs = &segments[child.total_span.clone()];
/// }
/// ```
///
/// Marked `#[non_exhaustive]`: only the resolver builds trees.
///
/// [`total_span`]: SegmentGroupIndexed::total_span
#[derive(Debug)]
#[non_exhaustive]
pub struct SegmentGroupIndexed<'a> {
    /// Group name from the schema, e.g. `"SG2"`, or the root name.
    ///
    /// Borrows from the schema, so it lives exactly as long as the schema does.
    pub definition: &'a str,
    /// The schema group this is an instance of; `None` for the root.
    pub def: Option<&'a GroupDef>,
    /// Contiguous span `[start, end)` of absolute indices into the original flat
    /// segment slice covering **all** segments in this group instance — trigger
    /// segment, direct segments, unplaced segments and all descendant groups
    /// combined.
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
    /// Absolute indices, ascending, of the segments skipped in this instance
    /// because they fit no place (ordered groups only; always empty in an open
    /// group). They lie inside [`total_span`](Self::total_span) but are not
    /// [direct segments](Self::direct_segment_indices).
    pub unplaced: Vec<usize>,
}

impl<'a> SegmentGroupIndexed<'a> {
    /// Iterate over the absolute indices of segments that belong *directly* to
    /// this group — those within [`total_span`] that are covered by no child
    /// group's [`total_span`] and are not [`unplaced`].
    ///
    /// Complexity: `O(total_span.len() + children.len() + unplaced.len())` —
    /// one merge walk over the (sorted, disjoint) child spans and the unplaced
    /// indices.
    ///
    /// [`total_span`]: SegmentGroupIndexed::total_span
    /// [`unplaced`]: SegmentGroupIndexed::unplaced
    pub fn direct_segment_indices(&self) -> impl Iterator<Item = usize> + '_ {
        DirectIndices {
            pos: self.total_span.start,
            end: self.total_span.end,
            children: &self.children,
            unplaced: &self.unplaced,
        }
    }

    /// The segments this group spans, resolved against the slice it was built
    /// from.
    ///
    /// The tree stores index ranges rather than copies, so reading a group means
    /// pairing it back with the slice it was built from.
    ///
    /// Returns an empty slice if `all` is shorter than that.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::group::{GroupDef, group_segments_indexed};
    ///
    /// static SCHEMA: &[GroupDef] = &[GroupDef::new("SG2", "NAD")];
    ///
    /// let segments: Vec<_> = edifact_rs::from_bytes(b"BGM+220'NAD+BY+1'DTM+137:1:102'")
    ///     .collect::<Result<Vec<_>, _>>()?;
    /// let tree = group_segments_indexed(&segments, SCHEMA, "ROOT");
    ///
    /// let sg2 = tree.children[0].segments(&segments);
    /// assert_eq!(sg2.iter().map(edifact_rs::Segment::tag).collect::<Vec<_>>(), ["NAD", "DTM"]);
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    #[must_use]
    pub fn segments<'s, 'd>(&self, all: &'s [Segment<'d>]) -> &'s [Segment<'d>] {
        all.get(self.total_span.clone()).unwrap_or(&[])
    }

    /// Every group in this subtree, **including this one**, in document order.
    ///
    /// Depth-first pre-order, so a parent is always yielded before its children
    /// and siblings in the order they appear on the wire.
    pub fn descendants(&self) -> Descendants<'_, 'a> {
        Descendants { stack: vec![self] }
    }

    /// Every group in this subtree named `name`, in document order.
    ///
    /// Searches the whole subtree, so it finds groups that `children` alone
    /// does not reach — a nested group can share a trigger with one further out
    /// and sit at any depth.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::group::{GroupDef, group_segments_indexed};
    ///
    /// static SG12: &[GroupDef] = &[GroupDef::new("SG12", "NAD")];
    /// static SCHEMA: &[GroupDef] = &[
    ///     GroupDef::new("SG2", "NAD"),
    ///     GroupDef::with_children("SG4", "IDE", SG12),
    /// ];
    ///
    /// let segments: Vec<_> = edifact_rs::from_bytes(
    ///     b"NAD+MS+SENDER'IDE+24+V1'NAD+Z09+KUNDE'IDE+24+V2'NAD+VY+PARTY'",
    /// )
    /// .collect::<Result<Vec<_>, _>>()?;
    /// let tree = group_segments_indexed(&segments, SCHEMA, "ROOT");
    ///
    /// // Both Vorgänge's parties, however deep they sit.
    /// let parties: Vec<&str> = tree
    ///     .find("SG12")
    ///     .filter_map(|group| group.segments(&segments).first())
    ///     .filter_map(|nad| nad.element_str(0))
    ///     .collect();
    /// assert_eq!(parties, ["Z09", "VY"]);
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    pub fn find<'q>(
        &'q self,
        name: &'q str,
    ) -> impl Iterator<Item = &'q SegmentGroupIndexed<'a>> + 'q {
        self.descendants().filter(move |g| g.definition == name)
    }
}

/// Merge walk behind [`SegmentGroupIndexed::direct_segment_indices`].
struct DirectIndices<'t, 'a> {
    pos: usize,
    end: usize,
    /// Child groups not yet passed; their spans are sorted and disjoint.
    children: &'t [SegmentGroupIndexed<'a>],
    /// Unplaced indices not yet passed, ascending.
    unplaced: &'t [usize],
}

impl Iterator for DirectIndices<'_, '_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        while self.pos < self.end {
            while let Some((first, rest)) = self.children.split_first() {
                if first.total_span.end <= self.pos {
                    self.children = rest;
                } else {
                    break;
                }
            }
            if let Some(child) = self.children.first() {
                if child.total_span.start <= self.pos {
                    self.pos = child.total_span.end;
                    continue;
                }
            }
            while let Some((&first, rest)) = self.unplaced.split_first() {
                if first < self.pos {
                    self.unplaced = rest;
                } else {
                    break;
                }
            }
            let pos = self.pos;
            self.pos += 1;
            if self.unplaced.first() == Some(&pos) {
                continue;
            }
            return Some(pos);
        }
        None
    }
}

/// Depth-first pre-order iterator over a [`SegmentGroupIndexed`] subtree.
///
/// Returned by [`SegmentGroupIndexed::descendants`].
pub struct Descendants<'t, 'a> {
    stack: Vec<&'t SegmentGroupIndexed<'a>>,
}

impl<'t, 'a> Iterator for Descendants<'t, 'a> {
    type Item = &'t SegmentGroupIndexed<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        // Pushed in reverse so siblings come back out in document order.
        self.stack.extend(node.children.iter().rev());
        Some(node)
    }
}

/// Partition `segments` into a [`SegmentGroupIndexed`] tree without cloning.
///
/// Stores `Range<usize>` indices into the original flat slice rather than
/// copying each [`Segment`] into the tree.  Use the original slice together
/// with [`SegmentGroupIndexed::total_span`] to access segments.
///
/// The root is open: segments that trigger none of `schema` are direct root
/// segments. Ordered groups inside `schema` are resolved as [the module
/// documentation][self#how-a-tag-is-resolved] describes, with their unplaced
/// segments recorded on the node they were skipped in. For per-segment
/// placements, findings or an ordered root, compile a [`GroupSchema`] once and
/// use [`GroupSchema::resolve`]. For a schema in which no group declares members,
/// the tree is the open-group resolution described above.
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
/// assert_eq!(&segments[sg1.total_span.clone()].iter().map(|s| s.tag()).collect::<Vec<_>>(),
///            &["RFF"]);
///
/// // SG5 spans LOC + DTM + QTY (all three segments, including the SG6 child).
/// let sg5_tags: Vec<_> = segments[sg5.total_span.clone()].iter().map(|s| s.tag()).collect();
/// assert_eq!(sg5_tags, &["LOC", "DTM", "QTY"]);
///
/// // SG5's direct segments (LOC + DTM) exclude the SG6 child (QTY).
/// let sg5_direct: Vec<_> = sg5.direct_segment_indices()
///     .map(|i| segments[i].tag())
///     .collect();
/// assert_eq!(sg5_direct, &["LOC", "DTM"]);
///
/// // SG6 contains only QTY.
/// let sg6 = sg5.children.iter().find(|g| g.definition == "SG6").unwrap();
/// assert_eq!(segments[sg6.total_span.clone()].iter().map(|s| s.tag()).collect::<Vec<_>>(),
///            &["QTY"]);
/// ```
///
/// # Group validation
///
/// `group_segments_indexed` pairs naturally with
/// [`ValidationContext::validate_grouped`][crate::ValidationContext::validate_grouped] to enforce group-presence rules:
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
/// let report = ctx.validate_grouped(&tree, &segments);
/// ```
///
/// # What a group spans
///
/// In open groups grouping is driven purely by trigger tags: a group runs from
/// its trigger to the next trigger belonging to a sibling or ancestor, or to the
/// end of the slice.  Nothing stops the final group at `UNT`, because the
/// trailer is not a trigger of anything — pass the message *body* when the group
/// boundaries matter, or accept that the trailer lands inside the last group.
///
/// # Complexity
///
/// `O(n × reachable places)` time — the places of the open groups on the
/// current path, which for an open schema is `O(n × schema_depth × fan-out)`
/// as before — and `O(tree_nodes + unplaced)` space, plus an `O(schema)` lookup
/// table when the schema declares members or discriminators. No `Segment`
/// clones, no recursion over the input.
pub fn group_segments_indexed<'g>(
    segments: &[Segment<'_>],
    schema: &'g [GroupDef],
    root_name: &'g str,
) -> SegmentGroupIndexed<'g> {
    let root = Members::Groups(schema);
    let tables = Tables::build(root);
    let resolved = resolve(segments, root, false, root_name, &tables, false);
    resolved.into_tree()
}

// ── GroupSchema ───────────────────────────────────────────────────────────────

#[derive(Debug)]
struct SchemaInner {
    root_name: Arc<str>,
    ordered: bool,
    entries: Vec<Entry>,
    tables: Tables,
}

/// A compiled, validated group schema: an open root (a list of groups) or an
/// ordered root (a list of entries, walked like an ordered group's members),
/// with its lookup tables built once.
///
/// [`resolve`](Self::resolve) assigns every segment of one message to its
/// place and yields a [`GroupResolution`]. Cloning is O(1); the schema is
/// `Send + Sync`.
///
/// # Example
///
/// ```
/// use edifact_rs::group::{Discriminator, Entry, GroupDef, GroupSchema, SegmentPlace};
///
/// let loc = |code: &str| {
///     GroupDef::builder("SG5", SegmentPlace::new("LOC").when(Discriminator::one_of(0, 0, [code])))
///         .ordered() // only LOC: anything else in an SG5 is unplaced
///         .build()
/// };
/// let sg4 = GroupDef::builder("SG4", SegmentPlace::new("IDE"))
///     .segment(SegmentPlace::new("DTM"))
///     .group(loc("Z16")?)
///     .group(loc("Z17")?)
///     .build()?;
/// let schema = GroupSchema::ordered(
///     "UTILMD",
///     [Entry::Segment(SegmentPlace::new("UNH")), Entry::Group(sg4), Entry::Segment(SegmentPlace::new("UNT"))],
/// )?;
///
/// let segments: Vec<_> = edifact_rs::from_bytes(b"UNH+1+UTILMD:D:11A:UN'IDE+24+V1'LOC+Z17+2'LOC+Z16+1'UNT+5+1'")
///     .collect::<Result<_, _>>()?;
/// let resolution = schema.resolve(&segments);
///
/// // LOC+Z16 after LOC+Z17 is out of guide order: skipped and reported.
/// assert_eq!(resolution.unplaced().len(), 1);
/// assert_eq!(resolution.unplaced()[0].index, 3);
/// assert_eq!(resolution.group_path(2).unwrap().to_string(), "SG4[0]/SG5[0]");
/// assert!(resolution.placement(4).is_some(), "UNT still finds its place");
/// # Ok::<(), edifact_rs::EdifactError>(())
/// ```
#[derive(Debug, Clone)]
pub struct GroupSchema {
    inner: Arc<SchemaInner>,
}

impl GroupSchema {
    /// An open root holding `groups`, like [`group_segments_indexed`]'s schema.
    ///
    /// # Errors
    ///
    /// [`EdifactError::InvalidGroupSchema`] as for [`GroupDefBuilder::build`],
    /// applied to every group (static groups included) and to the whole depth.
    pub fn new(
        root_name: impl Into<Arc<str>>,
        groups: impl IntoIterator<Item = GroupDef>,
    ) -> Result<Self, EdifactError> {
        Self::compile(
            root_name.into(),
            false,
            groups.into_iter().map(Entry::Group),
        )
    }

    /// An ordered root: `entries` are walked front to back like an ordered
    /// group's members (the root has no trigger).
    ///
    /// # Errors
    ///
    /// [`EdifactError::InvalidGroupSchema`] as for [`new`](Self::new).
    pub fn ordered(
        root_name: impl Into<Arc<str>>,
        entries: impl IntoIterator<Item = Entry>,
    ) -> Result<Self, EdifactError> {
        Self::compile(root_name.into(), true, entries)
    }

    fn compile(
        root_name: Arc<str>,
        ordered: bool,
        entries: impl IntoIterator<Item = Entry>,
    ) -> Result<Self, EdifactError> {
        let entries: Vec<Entry> = entries.into_iter().collect();
        if root_name.is_empty() {
            return Err(invalid("", "root name is empty"));
        }
        for entry in &entries {
            match entry {
                Entry::Segment(place) => check_place(&root_name, place)?,
                Entry::Group(group) => check_group(group, 1)?,
            }
        }
        let tables = Tables::build(Members::Entries(&entries));
        Ok(Self {
            inner: Arc::new(SchemaInner {
                root_name,
                ordered,
                entries,
                tables,
            }),
        })
    }

    /// The root name, used as the tree root's [`definition`](SegmentGroupIndexed::definition).
    #[must_use]
    pub fn root_name(&self) -> &str {
        &self.inner.root_name
    }

    /// Whether the root is ordered ([`ordered`](Self::ordered)).
    #[must_use]
    pub fn is_ordered(&self) -> bool {
        self.inner.ordered
    }

    /// The root entries: groups for an open root, entries for an ordered one.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.inner.entries
    }

    /// Resolve one message's segments (for example `UNH` to `UNT`).
    ///
    /// `O(n × reachable places)` time, `O(n)` extra memory, no recursion over
    /// the input.
    #[must_use]
    pub fn resolve<'s>(&'s self, segments: &[Segment<'_>]) -> GroupResolution<'s> {
        let inner = &*self.inner;
        let resolved = resolve(
            segments,
            Members::Entries(&inner.entries),
            inner.ordered,
            &inner.root_name,
            &inner.tables,
            true,
        );
        GroupResolution::new(resolved)
    }
}

// ── GroupResolution ───────────────────────────────────────────────────────────

/// Why a segment was left unplaced.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnplacedReason {
    /// It fits a place the walk has already passed — out of guide order.
    OutOfOrder,
    /// It fits a place elsewhere in the schema, not reachable from here.
    NotHere,
    /// It fits no place in the schema.
    Unknown,
}

impl UnplacedReason {
    /// A stable lowercase identifier: `out_of_order`, `not_here`, `unknown`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutOfOrder => "out_of_order",
            Self::NotHere => "not_here",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for UnplacedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OutOfOrder => "out of order: it fits a place already passed",
            Self::NotHere => "it fits a place elsewhere in the message, not here",
            Self::Unknown => "it fits no place in the schema",
        })
    }
}

/// A segment the resolver skipped.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unplaced {
    /// Index into the resolved segment slice.
    pub index: usize,
    /// Why it fits nowhere.
    pub reason: UnplacedReason,
}

/// The place a segment fills.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct Placement<'s> {
    /// The segment place (for a group trigger, the group's [`trigger`](GroupDef::trigger)).
    pub place: &'s SegmentPlace,
    /// The group whose instance holds the segment; `None` at the root.
    pub group: Option<&'s GroupDef>,
    /// Zero-based repetition of this place inside its instance.
    pub occurrence: u32,
}

/// The result of [`GroupSchema::resolve`]: the instance tree, every segment's
/// placement, the unplaced segments, and the findings they imply.
#[derive(Debug)]
pub struct GroupResolution<'s> {
    tree: SegmentGroupIndexed<'s>,
    nodes: Vec<NodeInfo<'s>>,
    slots: Vec<Slot<'s>>,
    unplaced: Vec<Unplaced>,
    overflows: Vec<Overflow<'s>>,
}

/// What a resolution keeps about each instance after the tree is built.
#[derive(Debug)]
struct NodeInfo<'s> {
    def: Option<&'s GroupDef>,
    name: &'s str,
    parent: Option<usize>,
    occurrence: usize,
    members: Members<'s>,
    first: usize,
    counts: SmallVec<[u32; 8]>,
}

impl<'s> GroupResolution<'s> {
    fn new(resolved: Resolved<'s>) -> Self {
        let Resolved {
            nodes,
            slots,
            unplaced,
            overflows,
        } = resolved;
        let mut infos = Vec::with_capacity(nodes.len());
        let mut children = Vec::with_capacity(nodes.len());
        let mut trees: Vec<Option<SegmentGroupIndexed<'s>>> = Vec::with_capacity(nodes.len());
        for mut node in nodes {
            children.push(std::mem::take(&mut node.children));
            infos.push(NodeInfo {
                def: node.def,
                name: node.name,
                parent: node.parent,
                occurrence: node.occurrence,
                members: node.members,
                first: node.first,
                counts: node.counts.clone(),
            });
            trees.push(Some(node.into_indexed()));
        }
        let tree = assemble(trees, &children);
        Self {
            tree,
            nodes: infos,
            slots,
            unplaced,
            overflows,
        }
    }

    /// The instance tree.
    #[must_use]
    pub fn tree(&self) -> &SegmentGroupIndexed<'s> {
        &self.tree
    }

    /// The instance tree, by value.
    #[must_use]
    pub fn into_tree(self) -> SegmentGroupIndexed<'s> {
        self.tree
    }

    /// The place segment `index` fills.
    ///
    /// `None` when it is unplaced, out of range, or a direct segment of an open
    /// group (which declares no place for it).
    #[must_use]
    pub fn placement(&self, index: usize) -> Option<Placement<'s>> {
        let slot = self.slots.get(index)?;
        let place = slot.place?;
        Some(Placement {
            place,
            group: self.nodes[slot.instance].def,
            occurrence: slot.occurrence,
        })
    }

    /// Every unplaced segment, ascending by index.
    #[must_use]
    pub fn unplaced(&self) -> &[Unplaced] {
        &self.unplaced
    }

    /// The [`GroupPath`] of the instance holding segment `index` — for an
    /// unplaced segment, the instance it was skipped in.
    ///
    /// `None` at the root or when `index` is out of range.
    #[must_use]
    pub fn group_path(&self, index: usize) -> Option<GroupPath> {
        self.instance_path(self.slots.get(index)?.instance)
    }

    fn instance_path(&self, mut instance: usize) -> Option<GroupPath> {
        let mut steps = Vec::new();
        while let Some(parent) = self.nodes[instance].parent {
            let node = &self.nodes[instance];
            steps.push(GroupStep::new(node.name, node.occurrence)?);
            instance = parent;
        }
        steps.reverse();
        GroupPath::new(steps)
    }

    /// The validation issues this resolution implies, in segment order, then
    /// the missing places:
    ///
    /// | Finding | Error | Kind |
    /// |---|---|---|
    /// | an unplaced segment | [`SegmentNotPlaced`][EdifactError::SegmentNotPlaced] | `Placement` |
    /// | a place occurring beyond its `max` (one per extra occurrence) | [`TooManySegmentOccurrences`][EdifactError::TooManySegmentOccurrences] | `Count` |
    /// | a group occurring beyond its `max` (one per extra instance) | [`TooManyGroupOccurrences`][EdifactError::TooManyGroupOccurrences] | `Count` |
    /// | a place or group below its `min` in an instance | [`MissingSegment`][EdifactError::MissingSegment] (`E015`, naming the trigger tag for a group) | `Missing` |
    ///
    /// Each issue carries layer [`Structure`][ValidationLayer::Structure], the
    /// [`group_path`](ValidationIssue::group_path) of the instance concerned
    /// (none at the root), the segment's span and, when `segments` starts at
    /// `UNH`, its [`segment_position`](ValidationIssue::segment_position); the
    /// place's or group's `id`, when set, is in the context under `place`.
    ///
    /// `segments` must be the slice that was resolved.
    #[must_use]
    pub fn findings(&self, segments: &[Segment<'_>]) -> Vec<ValidationIssue> {
        let mut keyed: Vec<(usize, ValidationIssue)> = Vec::new();
        let finish = |issue: ValidationIssue,
                      index: Option<usize>,
                      path: Option<GroupPath>,
                      id: Option<&str>| {
            let mut issue = issue.with_layer(ValidationLayer::Structure);
            if let Some(path) = path {
                issue = issue.with_group_path(path);
            }
            if let Some(position) =
                index.and_then(|i| crate::validator::message_position(segments, i))
            {
                issue = issue.with_segment_position(position);
            }
            if let Some(id) = id {
                issue = issue.with_context_entry("place", id);
            }
            issue
        };

        for unplaced in &self.unplaced {
            let Some(segment) = segments.get(unplaced.index) else {
                continue;
            };
            let err = EdifactError::SegmentNotPlaced {
                tag: segment.tag().to_owned(),
                reason: unplaced.reason,
                span: segment.span,
            };
            let path = self.group_path(unplaced.index);
            keyed.push((
                unplaced.index,
                finish(
                    crate::validator::issue_from_error(err),
                    Some(unplaced.index),
                    path,
                    None,
                ),
            ));
        }

        for overflow in &self.overflows {
            let Some(segment) = segments.get(overflow.index) else {
                continue;
            };
            let (err, id) = match overflow.cand {
                Cand::Seg(place) => (
                    EdifactError::TooManySegmentOccurrences {
                        tag: segment.tag().to_owned(),
                        max: place.max.unwrap_or(u32::MAX),
                        span: segment.span,
                    },
                    place.id(),
                ),
                Cand::Group(group) => (
                    EdifactError::TooManyGroupOccurrences {
                        group: group.name().to_owned(),
                        max: group.max.unwrap_or(u32::MAX),
                        span: segment.span,
                    },
                    group.id(),
                ),
            };
            let path = self.instance_path(overflow.instance);
            keyed.push((
                overflow.index,
                finish(
                    crate::validator::issue_from_error(err),
                    Some(overflow.index),
                    path,
                    id,
                ),
            ));
        }

        for (instance, node) in self.nodes.iter().enumerate() {
            for j in node.first..node.members.len() {
                let cand = node.members.get(j);
                let (min, tag, id) = match cand {
                    Cand::Seg(place) => (place.min, place.tag(), place.id()),
                    Cand::Group(group) => (group.min, group.trigger_tag(), group.id()),
                };
                if node.counts.get(j).copied().unwrap_or(0) >= min {
                    continue;
                }
                let path = self.instance_path(instance);
                let err = EdifactError::MissingSegment {
                    tag: tag.to_owned(),
                    expected_position: path
                        .as_ref()
                        .map_or_else(|| node.name.to_owned(), ToString::to_string),
                };
                keyed.push((
                    usize::MAX,
                    finish(crate::validator::issue_from_error(err), None, path, id),
                ));
            }
        }

        keyed.sort_by_key(|(key, _)| *key);
        keyed.into_iter().map(|(_, issue)| issue).collect()
    }
}

// ── resolver ──────────────────────────────────────────────────────────────────

/// The members of one level: a static group slice or an entry slice.
#[derive(Debug, Clone, Copy)]
enum Members<'g> {
    Groups(&'g [GroupDef]),
    Entries(&'g [Entry]),
}

impl<'g> Members<'g> {
    fn len(self) -> usize {
        match self {
            Self::Groups(g) => g.len(),
            Self::Entries(e) => e.len(),
        }
    }

    fn get(self, j: usize) -> Cand<'g> {
        match self {
            Self::Groups(g) => Cand::Group(&g[j]),
            Self::Entries(e) => match &e[j] {
                Entry::Segment(place) => Cand::Seg(place),
                Entry::Group(group) => Cand::Group(group),
            },
        }
    }
}

/// One candidate place of a level.
#[derive(Debug, Clone, Copy)]
enum Cand<'g> {
    Seg(&'g SegmentPlace),
    Group(&'g GroupDef),
}

impl Cand<'_> {
    fn matches(self, segment: &Segment<'_>, tables: &Tables) -> bool {
        match self {
            Self::Seg(place) => place.matches(segment, tables),
            Self::Group(group) => group.trigger.matches(segment, tables),
        }
    }

    fn max(self) -> Option<u32> {
        match self {
            Self::Seg(place) => place.max,
            Self::Group(group) => group.max,
        }
    }
}

/// Schema-wide lookups, by tag: every place (to tell `NotHere` from
/// `Unknown`) and every code admitted at each position (for
/// [`Discriminator::accept_unclaimed`]).
#[derive(Debug, Default)]
struct Tables {
    by_tag: HashMap<Box<str>, TagInfo>,
}

#[derive(Debug, Default)]
struct TagInfo {
    places: Vec<SegmentPlace>,
    claimed: Vec<(u16, u16, Arc<str>)>,
}

impl Tables {
    /// Walk the schema below `root`, at most [`MAX_DEPTH`] groups deep.
    ///
    /// The depth cap keeps the walk finite for any static table; schemas
    /// compiled by [`GroupSchema`] are checked to be within it.
    fn build(root: Members<'_>) -> Self {
        let mut tables = Self::default();
        let mut stack: Vec<(Members<'_>, usize, usize)> = vec![(root, 0, 0)];
        while let Some((members, from, depth)) = stack.pop() {
            for j in from..members.len() {
                match members.get(j) {
                    Cand::Seg(place) => tables.add(place),
                    Cand::Group(group) => {
                        tables.add(&group.trigger);
                        if depth < MAX_DEPTH {
                            let (inner, ordered) = group.members();
                            // An ordered group's entry 0 is its trigger, added above.
                            stack.push((inner, usize::from(ordered), depth + 1));
                        }
                    }
                }
            }
        }
        tables
    }

    fn add(&mut self, place: &SegmentPlace) {
        let info = self.by_tag.entry(place.tag().into()).or_default();
        for d in place.discriminators() {
            if let Rule::OneOf { codes, .. } = &d.rule {
                for code in codes.iter() {
                    info.claimed
                        .push((d.element, d.component, Arc::clone(code)));
                }
            }
        }
        info.places.push(place.clone());
    }

    fn claimed(&self, tag: &str, element: u16, component: u16, value: &str) -> bool {
        self.by_tag.get(tag).is_some_and(|info| {
            info.claimed
                .iter()
                .any(|(e, c, code)| *e == element && *c == component && &**code == value)
        })
    }

    fn any_place_matches(&self, segment: &Segment<'_>) -> bool {
        self.by_tag
            .get(segment.tag())
            .is_some_and(|info| info.places.iter().any(|p| p.matches(segment, self)))
    }
}

/// An instance under construction.
#[derive(Debug)]
struct Node<'g> {
    def: Option<&'g GroupDef>,
    name: &'g str,
    parent: Option<usize>,
    start: usize,
    end: usize,
    occurrence: usize,
    children: Vec<usize>,
    unplaced: Vec<usize>,
    /// Instances opened so far per child group name, for `occurrence_index`.
    names: SmallVec<[(&'g str, usize); 4]>,
    members: Members<'g>,
    ordered: bool,
    /// First member that is a candidate: 1 for an ordered group (entry 0 is
    /// its trigger), 0 otherwise.
    first: usize,
    /// Ordered: the current entry.
    cursor: usize,
    /// Occurrences per member in this instance.
    counts: SmallVec<[u32; 8]>,
}

impl<'g> Node<'g> {
    fn new(
        def: Option<&'g GroupDef>,
        name: &'g str,
        parent: Option<usize>,
        start: usize,
        occurrence: usize,
        (members, ordered): (Members<'g>, bool),
    ) -> Self {
        let first = usize::from(ordered && def.is_some());
        let mut counts: SmallVec<[u32; 8]> = SmallVec::from_elem(0, members.len());
        if first == 1 {
            // The trigger that opened this instance.
            counts[0] = 1;
        }
        Self {
            def,
            name,
            parent,
            start,
            end: start,
            occurrence,
            children: Vec::new(),
            unplaced: Vec::new(),
            names: SmallVec::new(),
            members,
            ordered,
            first,
            cursor: 0,
            counts,
        }
    }

    /// Step-1 candidates (module docs, "How a tag is resolved") of this level
    /// for `segment`: open — the first child
    /// group whose trigger matches; ordered — the first matching entry from the
    /// current one on, where an entry at its `max` yields to a later match.
    fn candidate(&self, segment: &Segment<'_>, tables: &Tables) -> Option<usize> {
        if !self.ordered {
            return (0..self.members.len()).find(|&j| {
                matches!(self.members.get(j), Cand::Group(_))
                    && self.members.get(j).matches(segment, tables)
            });
        }
        let mut full = None;
        for j in self.cursor.max(self.first)..self.members.len() {
            let cand = self.members.get(j);
            if !cand.matches(segment, tables) {
                continue;
            }
            if cand.max().is_some_and(|max| self.counts[j] >= max) {
                full = full.or(Some(j));
                continue;
            }
            return Some(j);
        }
        full
    }

    /// Whether `segment` fits an entry this ordered level has already passed.
    fn passed(&self, segment: &Segment<'_>, tables: &Tables) -> bool {
        self.ordered
            && (0..self.cursor.max(self.first))
                .any(|j| self.members.get(j).matches(segment, tables))
    }

    fn into_indexed(self) -> SegmentGroupIndexed<'g> {
        SegmentGroupIndexed {
            definition: self.name,
            def: self.def,
            total_span: self.start..self.end,
            children: Vec::new(),
            occurrence_index: self.occurrence,
            unplaced: self.unplaced,
        }
    }
}

/// Per-segment record kept by [`GroupSchema::resolve`].
#[derive(Debug, Clone, Copy)]
struct Slot<'g> {
    instance: usize,
    place: Option<&'g SegmentPlace>,
    occurrence: u32,
}

/// An occurrence beyond `max`: segment `index` filled `cand` once too often
/// (for a group, `instance` is the extra instance).
#[derive(Debug, Clone, Copy)]
struct Overflow<'g> {
    index: usize,
    instance: usize,
    cand: Cand<'g>,
}

#[derive(Debug)]
struct Resolved<'g> {
    nodes: Vec<Node<'g>>,
    slots: Vec<Slot<'g>>,
    unplaced: Vec<Unplaced>,
    overflows: Vec<Overflow<'g>>,
}

impl<'g> Resolved<'g> {
    fn into_tree(self) -> SegmentGroupIndexed<'g> {
        let mut children = Vec::with_capacity(self.nodes.len());
        let mut trees = Vec::with_capacity(self.nodes.len());
        for mut node in self.nodes {
            children.push(std::mem::take(&mut node.children));
            trees.push(Some(node.into_indexed()));
        }
        assemble(trees, &children)
    }
}

/// Nest the flat instance list into a tree, without recursion: every child is
/// created after its parent, so walking backwards finds children complete.
fn assemble<'g>(
    mut trees: Vec<Option<SegmentGroupIndexed<'g>>>,
    children: &[Vec<usize>],
) -> SegmentGroupIndexed<'g> {
    for i in (1..trees.len()).rev() {
        let kids: Vec<SegmentGroupIndexed<'g>> = children[i]
            .iter()
            .filter_map(|&c| trees[c].take())
            .collect();
        if let Some(node) = trees[i].as_mut() {
            node.children = kids;
        }
    }
    let kids: Vec<SegmentGroupIndexed<'g>> = children
        .first()
        .map(|c| c.iter().filter_map(|&c| trees[c].take()).collect())
        .unwrap_or_default();
    let mut root = trees
        .first_mut()
        .and_then(Option::take)
        .expect("the root instance always exists");
    root.children = kids;
    root
}

/// The resolver: an explicit stack of open instances, applying the precedence
/// rule of the module docs ("How a tag is resolved").
fn resolve<'g>(
    segments: &[Segment<'_>],
    root: Members<'g>,
    root_ordered: bool,
    root_name: &'g str,
    tables: &Tables,
    track: bool,
) -> Resolved<'g> {
    let mut nodes: Vec<Node<'g>> =
        vec![Node::new(None, root_name, None, 0, 0, (root, root_ordered))];
    let mut stack: Vec<usize> = vec![0];
    let mut slots: Vec<Slot<'g>> = Vec::with_capacity(if track { segments.len() } else { 0 });
    let mut unplaced: Vec<Unplaced> = Vec::new();
    let mut overflows: Vec<Overflow<'g>> = Vec::new();

    for (i, segment) in segments.iter().enumerate() {
        // Steps 1 and 2: the innermost level's own candidates, then each
        // enclosing level's reachable places, innermost first.
        let found = stack.iter().enumerate().rev().find_map(|(depth, &node)| {
            nodes[node]
                .candidate(segment, tables)
                .map(|j| (depth, node, j))
        });

        let slot = if let Some((depth, node, j)) = found {
            // A match further out closes every instance inside it.
            while stack.len() > depth + 1 {
                if let Some(closed) = stack.pop() {
                    nodes[closed].end = i;
                }
            }
            let level = &mut nodes[node];
            level.cursor = j;
            level.counts[j] = level.counts[j].saturating_add(1);
            let count = level.counts[j];
            let cand = level.members.get(j);
            let over = cand.max().is_some_and(|max| count > max);
            match cand {
                Cand::Seg(place) => {
                    if over {
                        overflows.push(Overflow {
                            index: i,
                            instance: node,
                            cand,
                        });
                    }
                    Slot {
                        instance: node,
                        place: Some(place),
                        occurrence: count - 1,
                    }
                }
                Cand::Group(group) => {
                    let name = group.name();
                    let occurrence = match level.names.iter_mut().find(|(n, _)| *n == name) {
                        Some((_, c)) => {
                            *c += 1;
                            *c - 1
                        }
                        None => {
                            level.names.push((name, 1));
                            0
                        }
                    };
                    let child = nodes.len();
                    nodes[node].children.push(child);
                    nodes.push(Node::new(
                        Some(group),
                        name,
                        Some(node),
                        i,
                        occurrence,
                        group.members(),
                    ));
                    stack.push(child);
                    if over {
                        overflows.push(Overflow {
                            index: i,
                            instance: child,
                            cand,
                        });
                    }
                    Slot {
                        instance: child,
                        place: Some(&group.trigger),
                        occurrence: 0,
                    }
                }
            }
        } else {
            // Step 3: a direct segment of an open level, unplaced in an ordered one.
            let node = *stack.last().expect("the root is never popped");
            if nodes[node].ordered {
                let reason = if stack.iter().any(|&n| nodes[n].passed(segment, tables)) {
                    UnplacedReason::OutOfOrder
                } else if tables.any_place_matches(segment) {
                    UnplacedReason::NotHere
                } else {
                    UnplacedReason::Unknown
                };
                nodes[node].unplaced.push(i);
                unplaced.push(Unplaced { index: i, reason });
            }
            Slot {
                instance: node,
                place: None,
                occurrence: 0,
            }
        };
        if track {
            slots.push(slot);
        }
    }

    let end = segments.len();
    for node in stack {
        nodes[node].end = end;
    }
    Resolved {
        nodes,
        slots,
        unplaced,
        overflows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Element;

    fn seg(tag: &'static str) -> Segment<'static> {
        Segment::new(tag, vec![Element::of(&["x"])])
    }

    static SG2: &[GroupDef] = &[GroupDef::new("SG2", "CTA")];
    static SCHEMA: &[GroupDef] = &[
        GroupDef::with_children("SG1", "NAD", SG2),
        GroupDef::new("SG3", "LIN"),
    ];

    /// A child group whose trigger also triggers a group at an ancestor level.
    ///
    /// `UTILMD` is the canonical case: SG2 carries the message-level parties,
    /// SG12 the parties of one Vorgang inside SG4, and both trigger on `NAD`.
    static SG12: &[GroupDef] = &[GroupDef::new("SG12", "NAD")];
    static NESTED_SAME_TRIGGER: &[GroupDef] = &[
        GroupDef::new("SG2", "NAD"),
        GroupDef::with_children("SG4", "IDE", SG12),
    ];

    #[test]
    fn a_nested_group_wins_over_an_ancestors_sibling_with_the_same_trigger() {
        // UNH BGM NAD NAD IDE NAD DTM NAD — the message *body*, with no `UNT`.
        // `UNT` triggers nothing, so it would land in whichever group ran last;
        // see `MessageWindow::body`, which exists for exactly that reason.
        let segs = vec![
            seg("UNH"),
            seg("BGM"),
            seg("NAD"),
            seg("NAD"),
            seg("IDE"),
            seg("NAD"),
            seg("DTM"),
            seg("NAD"),
        ];
        let tree = group_segments_indexed(&segs, NESTED_SAME_TRIGGER, "ROOT");

        let top: Vec<&str> = tree.children.iter().map(|c| c.definition).collect();
        assert_eq!(
            top,
            ["SG2", "SG2", "SG4"],
            "the two message-level NADs are SG2"
        );

        let sg4 = tree
            .children
            .iter()
            .find(|c| c.definition == "SG4")
            .expect("SG4 opens on IDE");
        // SG4 spans IDE through the last segment of its last child, not just IDE.
        assert_eq!(sg4.total_span, 4..8);

        let nested: Vec<&str> = sg4.children.iter().map(|c| c.definition).collect();
        assert_eq!(
            nested,
            ["SG12", "SG12"],
            "NAD inside SG4 nests as SG12 rather than reopening SG2",
        );
        assert_eq!(sg4.children[0].total_span, 5..7); // NAD + DTM
        assert_eq!(sg4.children[1].total_span, 7..8); // NAD
        assert_eq!(sg4.children[0].occurrence_index, 0);
        assert_eq!(sg4.children[1].occurrence_index, 1);
    }

    #[test]
    fn descendants_walk_the_whole_subtree_in_document_order() {
        let segs = vec![
            seg("NAD"),
            seg("IDE"),
            seg("NAD"),
            seg("DTM"),
            seg("IDE"),
            seg("NAD"),
        ];
        let tree = group_segments_indexed(&segs, NESTED_SAME_TRIGGER, "ROOT");

        let walked: Vec<&str> = tree.descendants().map(|g| g.definition).collect();
        assert_eq!(
            walked,
            ["ROOT", "SG2", "SG4", "SG12", "SG4", "SG12"],
            "pre-order: a parent before its children, siblings in wire order",
        );

        // `find` is the question a reader has: every SG12 anywhere, not just the
        // ones one level down.
        let sg12: Vec<usize> = tree.find("SG12").map(|g| g.total_span.start).collect();
        assert_eq!(sg12, [2, 5]);

        // …and resolving one back to its segments needs no manual slicing.
        let first = tree.find("SG12").next().unwrap();
        assert_eq!(
            first
                .segments(&segs)
                .iter()
                .map(Segment::tag)
                .collect::<Vec<_>>(),
            ["NAD", "DTM"],
        );
    }

    #[test]
    fn a_repeated_trigger_still_reopens_a_sibling_rather_than_nesting_forever() {
        // A group's own trigger is not among its children, so the second NAD
        // inside SG12 falls through to the stop set and SG4 opens a sibling —
        // it does not nest SG12 inside SG12.
        let segs = vec![seg("IDE"), seg("NAD"), seg("NAD"), seg("NAD")];
        let tree = group_segments_indexed(&segs, NESTED_SAME_TRIGGER, "ROOT");

        let sg4 = &tree.children[0];
        assert_eq!(sg4.definition, "SG4");
        assert_eq!(
            sg4.children.len(),
            3,
            "three sibling SG12s, not one nest of three"
        );
        assert!(
            sg4.children.iter().all(|c| c.children.is_empty()),
            "SG12 has no children, so nothing may nest inside it",
        );
    }

    #[test]
    fn a_tag_no_nested_definition_accepts_still_closes_the_group() {
        // `IDE` is not an SG4 child, so a second one closes SG4 and the root
        // opens the next occurrence — the behaviour the stop set exists for.
        let segs = vec![seg("IDE"), seg("NAD"), seg("IDE"), seg("NAD")];
        let tree = group_segments_indexed(&segs, NESTED_SAME_TRIGGER, "ROOT");

        let top: Vec<&str> = tree.children.iter().map(|c| c.definition).collect();
        assert_eq!(top, ["SG4", "SG4"]);
        assert_eq!(tree.children[0].total_span, 0..2);
        assert_eq!(tree.children[1].total_span, 2..4);
    }

    #[test]
    fn nesting_is_preferred_at_every_depth() {
        // Three levels, all triggered by NAD: the deepest definition that can
        // accept the tag is the one that gets it.
        static L3: &[GroupDef] = &[GroupDef::new("L3", "CTA")];
        static L2: &[GroupDef] = &[GroupDef::with_children("L2", "NAD", L3)];
        static DEEP: &[GroupDef] = &[
            GroupDef::new("L1", "NAD"),
            GroupDef::with_children("A", "IDE", L2),
        ];

        let segs = vec![seg("NAD"), seg("IDE"), seg("NAD"), seg("CTA")];
        let tree = group_segments_indexed(&segs, DEEP, "ROOT");

        assert_eq!(tree.children[0].definition, "L1");
        let a = &tree.children[1];
        assert_eq!(a.definition, "A");
        assert_eq!(a.children[0].definition, "L2");
        assert_eq!(a.children[0].children[0].definition, "L3");
    }

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
            .map(|s| s.tag())
            .collect();
        assert!(all_tags.contains(&"UNH"));
        assert!(all_tags.contains(&"NAD"));
        assert!(all_tags.contains(&"CTA"));
    }

    // ── 005: ordered groups, places, discriminators ──────────────────────────

    fn parse(input: &'static str) -> Vec<Segment<'static>> {
        crate::from_bytes(input.as_bytes())
            .collect::<Result<_, _>>()
            .expect("fixture parses")
    }

    /// `(name, occurrence, span)` of each child.
    fn shape<'g>(node: &SegmentGroupIndexed<'g>) -> Vec<(&'g str, usize, Range<usize>)> {
        node.children
            .iter()
            .map(|c| (c.definition, c.occurrence_index, c.total_span.clone()))
            .collect()
    }

    fn place(tag: &'static str, code: &'static str) -> SegmentPlace {
        SegmentPlace::new(tag).when(Discriminator::one_of(0, 0, [code]))
    }

    /// A trigger-only MIG group, ordered: nothing but its trigger fits.
    fn only(name: &str, trigger: SegmentPlace) -> GroupDef {
        GroupDef::builder(name, trigger).ordered().build().unwrap()
    }

    const SHARED_RFF_BODY: &str = "IDE+24+X'RFF+Z13:1'SEQ+Z01'RFF+Z19:ABC'CCI+Z30'SEQ+Z02'QTY+1:1'";

    #[test]
    fn shared_rff_with_members() {
        let sg8 = GroupDef::builder("SG8", SegmentPlace::new("SEQ"))
            .segment(SegmentPlace::new("RFF"))
            .segment(SegmentPlace::new("CCI"))
            .segment(SegmentPlace::new("QTY"))
            .build()
            .unwrap();
        let sg4 = GroupDef::builder("SG4", SegmentPlace::new("IDE"))
            .group(GroupDef::new("SG6", "RFF"))
            .group(sg8)
            .build()
            .unwrap();
        assert!(!sg4.is_ordered(), "SG4 declares no member segment: open");
        let schema = [sg4];
        let segs = parse(SHARED_RFF_BODY);
        let tree = group_segments_indexed(&segs, &schema, "ROOT");

        assert_eq!(shape(&tree), [("SG4", 0, 0..7)]);
        assert_eq!(
            shape(&tree.children[0]),
            [("SG6", 0, 1..2), ("SG8", 0, 2..5), ("SG8", 1, 5..7)],
            "RFF+Z19 and CCI+Z30 belong to SG8#0",
        );
        assert!(tree.descendants().all(|g| g.unplaced.is_empty()));
        assert!(
            tree.children[0].children[1]
                .def
                .is_some_and(GroupDef::is_ordered)
        );
    }

    #[test]
    fn shared_rff_open_schema() {
        static SG4: &[GroupDef] = &[GroupDef::new("SG6", "RFF"), GroupDef::new("SG8", "SEQ")];
        static SCHEMA: &[GroupDef] = &[GroupDef::with_children("SG4", "IDE", SG4)];
        let segs = parse(SHARED_RFF_BODY);
        let tree = group_segments_indexed(&segs, SCHEMA, "ROOT");
        // Open schema: the RFF that belongs to SG8 opens a second SG6.
        assert_eq!(
            shape(&tree.children[0]),
            [
                ("SG6", 0, 1..2),
                ("SG8", 0, 2..3),
                ("SG6", 1, 3..5),
                ("SG8", 1, 5..7)
            ],
        );
    }

    fn utilmd_sg4() -> GroupDef {
        let sg8 = GroupDef::builder("SG8", SegmentPlace::new("SEQ"))
            .segment(SegmentPlace::new("RFF"))
            .segment(SegmentPlace::new("CCI"))
            .build()
            .unwrap();
        GroupDef::builder("SG4", SegmentPlace::new("IDE"))
            .group(only("SG5", place("LOC", "Z16")))
            .group(only("SG5", place("LOC", "Z17")))
            .group(GroupDef::new("SG6", "RFF"))
            .group(sg8)
            .ordered()
            .build()
            .unwrap()
    }

    #[test]
    fn sg6_sg8_rff_ordered() {
        let schema = GroupSchema::new("ROOT", [utilmd_sg4()]).unwrap();
        let segs = parse(
            "IDE+24+X'LOC+Z16+1'LOC+Z17+2'RFF+Z13:1'SEQ+Z01'RFF+Z18:1'CCI+Z30'SEQ+Z02'RFF+Z19:2'",
        );
        let resolution = schema.resolve(&segs);
        let path = |i: usize| resolution.group_path(i).map(|p| p.to_string());

        assert!(resolution.unplaced().is_empty());
        assert_eq!(path(1).as_deref(), Some("SG4[0]/SG5[0]"));
        assert_eq!(path(2).as_deref(), Some("SG4[0]/SG5[1]"));
        assert_eq!(path(3).as_deref(), Some("SG4[0]/SG6[0]"), "RFF+Z13 is SG6");
        assert_eq!(
            path(5).as_deref(),
            Some("SG4[0]/SG8[0]"),
            "RFF after SEQ is SG8"
        );
        assert_eq!(path(6).as_deref(), Some("SG4[0]/SG8[0]"));
        assert_eq!(path(8).as_deref(), Some("SG4[0]/SG8[1]"));
        assert_eq!(path(0).as_deref(), Some("SG4[0]"));
    }

    fn sg5_places(with_dtm: bool) -> GroupSchema {
        let mut sg4 = GroupDef::builder("SG4", SegmentPlace::new("IDE"))
            .group(only("SG5", place("LOC", "Z16").with_id("00047")))
            .group(only("SG5", place("LOC", "Z17").with_id("00048")))
            .ordered();
        if with_dtm {
            sg4 = sg4.segment(SegmentPlace::new("DTM"));
        }
        GroupSchema::new("ROOT", [sg4.build().unwrap()]).unwrap()
    }

    #[test]
    fn sg5_places_by_code() {
        // Each LOC in the SG5 instance of its own place.
        let schema = sg5_places(false);
        let segs = parse("IDE+24+V1'LOC+Z16+1'LOC+Z17+2'");
        let resolution = schema.resolve(&segs);
        assert!(resolution.unplaced().is_empty());
        let first = resolution.placement(1).unwrap();
        let second = resolution.placement(2).unwrap();
        assert_eq!(first.place.id(), Some("00047"));
        assert_eq!(second.place.id(), Some("00048"));
        assert_eq!(first.group.map(GroupDef::name), Some("SG5"));
        assert_eq!(
            resolution.group_path(1).unwrap().to_string(),
            "SG4[0]/SG5[0]"
        );
        assert_eq!(
            resolution.group_path(2).unwrap().to_string(),
            "SG4[0]/SG5[1]"
        );

        // Z16 after Z17 is out of order; the DTM after it still fits.
        let schema = sg5_places(true);
        let segs = parse("IDE+24+V1'LOC+Z17+2'LOC+Z16+1'DTM+92:20261001:303'");
        let resolution = schema.resolve(&segs);
        assert_eq!(
            resolution.unplaced(),
            [Unplaced {
                index: 2,
                reason: UnplacedReason::OutOfOrder
            }]
        );
        assert!(resolution.placement(2).is_none());
        let dtm = resolution.placement(3).expect("DTM is placed");
        assert_eq!(dtm.place.tag(), "DTM");
        assert_eq!(resolution.group_path(3).unwrap().to_string(), "SG4[0]");
        // Recorded on the instance it was skipped in.
        let sg5 = &resolution.tree().children[0].children[0];
        assert_eq!(sg5.unplaced, [2]);
    }

    #[test]
    fn same_tag_places_told_apart_by_code() {
        let one = |p: SegmentPlace| Entry::Segment(p.with_max(1));
        let dtm = |code: &'static str| {
            SegmentPlace::new("DTM")
                .when(Discriminator::one_of(0, 0, [code]))
                .with_max(1)
        };
        let sg4 = GroupDef::builder("SG4", place("IDE", "24").with_id("00018"))
            .segment(dtm("92").with_id("00020"))
            .segment(dtm("93").with_id("00021"))
            .group(only("SG5", place("LOC", "Z16").with_id("00047")))
            .group(only("SG5", place("LOC", "Z22").with_id("00048")))
            .max(99_999)
            .build()
            .unwrap();
        let schema = GroupSchema::ordered(
            "UTILMD",
            [
                one(SegmentPlace::new("UNH").with_id("00003")),
                one(SegmentPlace::new("BGM").with_id("00004")),
                Entry::Group(only("SG2", place("NAD", "MS").with_id("00008"))),
                Entry::Group(only("SG2", place("NAD", "MR").with_id("00009"))),
                Entry::Group(sg4),
                one(SegmentPlace::new("UNT").with_id("00493")),
            ],
        )
        .unwrap();
        let segs = parse(
            "UNH+1+UTILMD:D:11A:UN:S2.2'BGM+E01+1'NAD+MS+1'NAD+MR+2'IDE+24+V1'DTM+93:20261001:303'LOC+Z22+1'LOC+Z16+2'UNT+8+1'",
        );
        let resolution = schema.resolve(&segs);
        let nr = |i: usize| resolution.placement(i).and_then(|p| p.place.id());
        assert_eq!(nr(3), Some("00009"), "NAD+MR is the second SG2");
        assert_eq!(nr(5), Some("00021"), "DTM+93 is Ende zum");
        assert_eq!(nr(6), Some("00048"), "LOC+Z22 is the ruhende Marktlokation");
        assert_eq!(nr(7), None);
        assert_eq!(
            resolution.unplaced(),
            [Unplaced {
                index: 7,
                reason: UnplacedReason::OutOfOrder
            }]
        );
        assert_eq!(
            nr(8),
            Some("00493"),
            "the UNT after it still finds its place"
        );
    }

    #[test]
    fn accept_unclaimed() {
        let schema = |accept: bool| {
            let z16 = Discriminator::one_of(0, 0, ["Z16"]);
            let z16 = if accept { z16.accept_unclaimed() } else { z16 };
            let sg4 = GroupDef::builder("SG4", SegmentPlace::new("IDE"))
                .group(only("SG5", SegmentPlace::new("LOC").when(z16)))
                .group(only("SG5", place("LOC", "Z17")))
                .ordered()
                .build()
                .unwrap();
            GroupSchema::new("ROOT", [sg4]).unwrap()
        };

        let segs = parse("IDE+24+V1'LOC+Z99+1'LOC+Z17+2'");
        let lenient = schema(true);
        let resolution = lenient.resolve(&segs);
        assert!(
            resolution.unplaced().is_empty(),
            "Z99 is claimed by no place"
        );
        assert_eq!(
            resolution.group_path(1).unwrap().to_string(),
            "SG4[0]/SG5[0]"
        );
        assert_eq!(
            resolution.group_path(2).unwrap().to_string(),
            "SG4[0]/SG5[1]"
        );

        // A code another place admits is never taken by accept_unclaimed.
        let segs_z17 = parse("IDE+24+V1'LOC+Z17+2'");
        let resolution = lenient.resolve(&segs_z17);
        assert_eq!(
            resolution.placement(1).unwrap().place.discriminators()[0].codes()[0].as_ref(),
            "Z17"
        );

        let strict = schema(false);
        let resolution = strict.resolve(&segs);
        assert_eq!(
            resolution.unplaced(),
            [Unplaced {
                index: 1,
                reason: UnplacedReason::Unknown
            }]
        );
    }

    #[test]
    fn optional_and_not_used_discriminators() {
        let sg4 = GroupDef::builder("SG4", SegmentPlace::new("IDE"))
            .segment(
                SegmentPlace::new("CCI")
                    .with_id("plain")
                    .when(Discriminator::not_used(0, 0)),
            )
            .segment(
                SegmentPlace::new("CCI")
                    .with_id("coded")
                    .when(Discriminator::one_of(0, 0, ["Z18"]).optional()),
            )
            .build()
            .unwrap();
        let schema = GroupSchema::new("ROOT", [sg4]).unwrap();
        let segs = parse("IDE+24'CCI+++ZB3'CCI+Z18'");
        let resolution = schema.resolve(&segs);
        let id = |i: usize| resolution.placement(i).and_then(|p| p.place.id());
        assert_eq!(
            id(1),
            Some("plain"),
            "nothing at element 0: not_used matches"
        );
        assert_eq!(
            id(2),
            Some("coded"),
            "a value at element 0 rules out not_used"
        );
        assert!(resolution.unplaced().is_empty());
    }

    #[test]
    fn a_place_at_max_yields_to_a_later_matching_entry() {
        let sg4 = GroupDef::builder("SG4", SegmentPlace::new("IDE"))
            .segment(SegmentPlace::new("DTM").with_id("first").with_max(1))
            .segment(SegmentPlace::new("DTM").with_id("second").with_max(1))
            .build()
            .unwrap();
        let schema = GroupSchema::new("ROOT", [sg4]).unwrap();
        let segs = parse("IDE+24'DTM+1'DTM+2'DTM+3'");
        let resolution = schema.resolve(&segs);
        let id = |i: usize| resolution.placement(i).and_then(|p| p.place.id());
        assert_eq!(
            (id(1), id(2), id(3)),
            (Some("first"), Some("second"), Some("second"))
        );
        // The third has nowhere better to go: it stays, as a count finding.
        let counts: Vec<_> = resolution
            .findings(&segs)
            .into_iter()
            .map(|f| f.error_code().map(str::to_owned))
            .collect();
        assert_eq!(counts, [Some("E069".to_owned())]);
    }

    #[test]
    fn own_trigger_opens_the_next_occurrence() {
        let sg8 = GroupDef::builder("SG8", SegmentPlace::new("SEQ"))
            .segment(SegmentPlace::new("QTY"))
            .build()
            .unwrap();
        let schema = [sg8];
        let segs = parse("SEQ+1'QTY+1'SEQ+2'SEQ+3'QTY+2'");
        let tree = group_segments_indexed(&segs, &schema, "ROOT");
        assert_eq!(
            shape(&tree),
            [("SG8", 0, 0..2), ("SG8", 1, 2..3), ("SG8", 2, 3..5)]
        );
    }

    #[test]
    fn unplaced_segments_are_not_direct_segments() {
        let sg1 = GroupDef::builder("SG1", SegmentPlace::new("AAA"))
            .segment(SegmentPlace::new("BBB"))
            .build()
            .unwrap();
        let schema = [sg1];
        let segs = parse("AAA+1'CCC+1'BBB+1'");
        let tree = group_segments_indexed(&segs, &schema, "ROOT");
        let sg1 = &tree.children[0];
        assert_eq!(sg1.total_span, 0..3);
        assert_eq!(sg1.unplaced, [1]);
        assert_eq!(sg1.direct_segment_indices().collect::<Vec<_>>(), [0, 2]);
    }

    #[test]
    fn empty_input_is_an_empty_root() {
        let tree = group_segments_indexed(&[], SCHEMA, "ROOT");
        assert_eq!(tree.total_span, 0..0);
        assert!(tree.children.is_empty() && tree.unplaced.is_empty());
        let schema =
            GroupSchema::ordered("ROOT", [Entry::Segment(SegmentPlace::new("UNH"))]).unwrap();
        assert!(schema.resolve(&[]).unplaced().is_empty());
    }

    #[test]
    fn owned_schema_from_strings() {
        struct Profile {
            groups: Vec<GroupDef>,
        }
        fn load(names: &[String]) -> Profile {
            let sg6 = GroupDef::builder(names[4].as_str(), SegmentPlace::owned(names[5].as_str()))
                .build()
                .unwrap();
            Profile {
                groups: vec![
                    GroupDef::builder(names[0].clone(), SegmentPlace::owned(names[1].clone()))
                        .build()
                        .unwrap(),
                    GroupDef::builder(names[2].clone(), SegmentPlace::owned(names[3].clone()))
                        .group(sg6)
                        .build()
                        .unwrap(),
                ],
            }
        }
        let names: Vec<String> = ["SG1", "RFF", "SG5", "LOC", "SG6", "QTY"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let profile = load(&names);
        drop(names);

        let segs = parse("RFF+ON:1'LOC+172+DE1'DTM+163:20260101:102'QTY+220:5'");
        let tree = group_segments_indexed(&segs, &profile.groups, "ROOT");
        assert_eq!(shape(&tree), [("SG1", 0, 0..1), ("SG5", 0, 1..4)]);
        assert_eq!(shape(&tree.children[1]), [("SG6", 0, 3..4)]);
    }

    fn reason(result: Result<GroupDef, EdifactError>) -> &'static str {
        match result {
            Err(EdifactError::InvalidGroupSchema { reason, .. }) => reason,
            other => panic!("expected InvalidGroupSchema, got {other:?}"),
        }
    }

    #[test]
    fn invalid_group_schemas_are_refused() {
        let b = || GroupDef::builder("SG1", SegmentPlace::new("NAD"));
        assert_eq!(reason(b().min(2).max(1).build()), "min exceeds max");
        assert_eq!(reason(b().max(0).build()), "max is 0");
        assert_eq!(
            reason(b().segment(SegmentPlace::new("CT")).build()),
            "segment tag is not three upper-case ASCII letters"
        );
        assert_eq!(
            reason(
                b().segment(SegmentPlace::new("CTA").with_min(3).with_max(2))
                    .build()
            ),
            "min exceeds max"
        );
        assert_eq!(
            reason(GroupDef::builder("SG1", SegmentPlace::new("nad")).build()),
            "segment tag is not three upper-case ASCII letters"
        );
        assert_eq!(
            reason(
                b().segment(SegmentPlace::new("CTA").when(Discriminator::one_of(
                    0,
                    0,
                    Vec::<&str>::new()
                )))
                .build()
            ),
            "one_of discriminator admits no code"
        );
        assert_eq!(
            reason(GroupDef::builder("", SegmentPlace::new("NAD")).build()),
            "group name is empty"
        );

        // 64 levels are fine, 65 are not.
        let mut group = GroupDef::builder("G", SegmentPlace::new("AAA"))
            .build()
            .unwrap();
        for _ in 1..64 {
            group = GroupDef::builder("G", SegmentPlace::new("AAA"))
                .group(group)
                .build()
                .unwrap();
        }
        assert_eq!(
            reason(
                GroupDef::builder("G", SegmentPlace::new("AAA"))
                    .group(group)
                    .build()
            ),
            "groups nest deeper than 64 levels"
        );

        // Static groups bypass the builder; GroupSchema checks them.
        static BAD: &[GroupDef] = &[GroupDef::new("SG1", "nad")];
        let err = GroupSchema::new("ROOT", BAD.iter().cloned()).unwrap_err();
        assert_eq!(err.stable_code(), "E071");
        assert!(matches!(
            GroupSchema::ordered("", []),
            Err(EdifactError::InvalidGroupSchema { .. })
        ));
    }
}
