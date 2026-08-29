//! Profile rule packs: `ProfileRule`, `ProfileRulePack`, and supporting types.

use super::ValidationRuleContext;
use super::Validator;
use crate::group::SegmentGroupIndexed;
use crate::{EdifactError, Segment, ValidationIssue, ValidationReport, ValidationSeverity};
use std::sync::Arc;

/// A profile rule that can be added to a [`ProfileRulePack`].
///
/// Implement this trait to create reusable, composable profile rules for
/// EDIFACT message validation.  Rules receive a [`ValidationRuleContext`] that
/// provides optional typed metadata injected at validation call time via
/// [`super::context::ValidationContext::validate_with`].
///
/// # Multiple issues per invocation
///
/// [`evaluate`](ProfileRule::evaluate) appends issues into a caller-supplied
/// `Vec` rather than returning a single `Option`.  This lets one rule iterate
/// every matching segment and report *all* violations — not just the first.
///
/// # `with_bail_on_first_error` interaction
///
/// When [`ProfileRulePack::with_bail_on_first_error`] is set, the pack stops calling
/// further rules as soon as this method pushes at least one error-severity issue.
/// Issues already pushed remain in the report; subsequent rules in the same pack
/// are skipped.
pub trait ProfileRule: Send + Sync {
    /// Evaluate the rule against the given segments.
    ///
    /// Push any violations into `issues`.  Push nothing if the segments pass.
    fn evaluate(
        &self,
        segments: &[Segment<'_>],
        context: &ValidationRuleContext<'_>,
        issues: &mut Vec<ValidationIssue>,
    );
}

/// Wraps a context-aware closure as a [`ProfileRule`].
struct ClosureProfileRule<F>(F);

impl<F> ProfileRule for ClosureProfileRule<F>
where
    F: for<'a> Fn(&[Segment<'a>], &ValidationRuleContext<'_>, &mut Vec<ValidationIssue>)
        + Send
        + Sync,
{
    fn evaluate(
        &self,
        segments: &[Segment<'_>],
        context: &ValidationRuleContext<'_>,
        issues: &mut Vec<ValidationIssue>,
    ) {
        (self.0)(segments, context, issues);
    }
}

/// Wraps a context-free closure as a [`ProfileRule`] (ignores the context parameter).
struct StatelessClosureProfileRule<F>(F);

impl<F> ProfileRule for StatelessClosureProfileRule<F>
where
    F: for<'a> Fn(&[Segment<'a>], &mut Vec<ValidationIssue>) + Send + Sync,
{
    fn evaluate(
        &self,
        segments: &[Segment<'_>],
        _context: &ValidationRuleContext<'_>,
        issues: &mut Vec<ValidationIssue>,
    ) {
        (self.0)(segments, issues);
    }
}

/// A rule entry inside a [`ProfileRulePack`], optionally carrying a stable identifier.
///
/// The `id` is used by [`ProfileRulePack::merge_with_override`] to de-duplicate rules:
/// when two packs contain a rule with the same id, the rule from the *other* (override)
/// pack replaces the one in `self`.
pub(super) struct NamedRule {
    /// Stable identifier for this rule, e.g. `"PROFILE-4711-BGM-M"`.
    ///
    /// `None` for anonymous rules that can never be overridden by id.
    pub(super) id: Option<Arc<str>>,
    pub(super) rule: Arc<dyn ProfileRule + Send + Sync>,
}

impl Clone for NamedRule {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            rule: Arc::clone(&self.rule),
        }
    }
}

/// A group-scoped rule entry inside a [`ProfileRulePack`].
///
/// Group rules are evaluated by [`ProfileRulePack`] during a segment-group tree
/// traversal (see [`ValidationContext::validate_grouped`]).  Each rule
/// receives the current [`SegmentGroupIndexed`] node, the full message segment
/// slice, and the validation context.
///
/// The `group_scope` field restricts evaluation to groups whose `definition` field
/// matches: `Some("SG5")` fires only inside `SG5` groups; `None` fires for every
/// group in the traversal.
pub(super) struct NamedGroupRule {
    /// Stable identifier, used for override deduplication.
    pub(super) id: Option<Arc<str>>,
    /// If `Some(name)`, this rule fires only when `group.definition == name`.
    ///
    /// Accepts any `Into<Arc<str>>` at construction time, so both `&'static str`
    /// literals and owned `String`s are valid group scopes.
    pub(super) group_scope: Option<Arc<str>>,
    /// The rule closure.
    #[allow(clippy::type_complexity)]
    pub(super) rule: Arc<
        dyn Fn(
                &SegmentGroupIndexed<'_>,
                &[Segment<'_>],
                &ValidationRuleContext<'_>,
                &mut Vec<ValidationIssue>,
            ) + Send
            + Sync,
    >,
}

impl Clone for NamedGroupRule {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            group_scope: self.group_scope.clone(),
            rule: Arc::clone(&self.rule),
        }
    }
}

/// A profile/MIG rule pack that can be plugged into `ValidationContext`.
pub struct ProfileRulePack {
    name: String,
    /// Set of EDIFACT message types this pack is scoped to (e.g. `"ORDERS"`, `"INVOIC"`).
    ///
    /// Most packs target one or two message types, so a `SmallVec<[String; 2]>` avoids
    /// any heap allocation for the common case.
    message_types: smallvec::SmallVec<[String; 2]>,
    /// Association-assigned code (DE 0057) this pack is bound to, e.g. `"5.5.3a"`.
    release: Option<String>,
    pub(super) rules: Vec<NamedRule>,
    pub(super) group_rules: Vec<NamedGroupRule>,
    pub(super) bail_on_first_error: bool,
    /// Maximum number of issues that a single rule may contribute per evaluation.
    ///
    /// `None` means unlimited.  Useful for noisy rules that can fire once per
    /// segment occurrence in a large message (e.g. a missing-qualifier check
    /// over thousands of DTM segments).
    pub(super) max_issues_per_rule: Option<usize>,
}

impl ProfileRulePack {
    /// Create an empty rule pack.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            message_types: smallvec::SmallVec::new(),
            release: None,
            rules: Vec::new(),
            group_rules: Vec::new(),
            bail_on_first_error: false,
            max_issues_per_rule: None,
        }
    }

    /// Return the pack name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the message types this pack is scoped to.
    pub fn message_types(&self) -> impl Iterator<Item = &str> {
        self.message_types.iter().map(|s| s.as_str())
    }

    /// Return the number of rules in this pack.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Return the number of named rules (those with a stable identifier).
    pub fn named_rule_count(&self) -> usize {
        self.rules.iter().filter(|r| r.id.is_some()).count()
    }

    /// Return the number of anonymous rules (those without a stable identifier).
    pub fn anonymous_rule_count(&self) -> usize {
        self.rules.iter().filter(|r| r.id.is_none()).count()
    }

    /// Iterate over the stable identifiers of all **named** rules in this pack.
    pub fn rule_ids(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().filter_map(|r| r.id.as_deref())
    }

    /// Return the association-assigned release code this pack is bound to, if any.
    pub fn release(&self) -> Option<&str> {
        self.release.as_deref()
    }

    /// Restrict this pack to one or more EDIFACT message types from the `UNH` segment.
    pub fn for_message_type(mut self, message_type: impl Into<String>) -> Self {
        let s = message_type.into();
        if !self.message_types.iter().any(|x| x == &s) {
            self.message_types.push(s);
        }
        self
    }

    /// Bind this pack to a specific association-assigned code (DE 0057).
    pub fn for_release(mut self, release: impl Into<String>) -> Self {
        self.release = Some(release.into());
        self
    }

    /// Stop evaluating rules in this pack after the first `Error`- or `Critical`-severity
    /// finding.
    pub fn with_bail_on_first_error(mut self, bail: bool) -> Self {
        self.bail_on_first_error = bail;
        self
    }

    /// Cap the number of issues any single rule may emit per evaluation pass.
    ///
    /// Excess issues are discarded, keeping one noisy rule from flooding the
    /// report.
    ///
    /// The cap is *per rule per call* — not globally, and **not per group**: a
    /// group rule's budget is spent across the whole tree walk rather than reset
    /// at each occurrence, and a rule that has spent it is not called again on
    /// that pass.
    ///
    /// `None` removes the cap.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity, from_bytes};
    ///
    /// let pack = ProfileRulePack::new("NOISY")
    ///     .with_max_issues_per_rule(2)
    ///     .with_rule_fn(|segments, issues| {
    ///         for segment in segments {
    ///             issues.push(ValidationIssue::new(
    ///                 ValidationSeverity::Warning,
    ///                 format!("saw {}", segment.tag()),
    ///             ));
    ///         }
    ///     });
    ///
    /// let segments: Vec<_> = from_bytes(b"BGM+1'DTM+2'RFF+3'NAD+4'")
    ///     .collect::<Result<Vec<_>, _>>()?;
    /// let report = edifact_rs::ValidationContext::builder()
    ///     .with_profile_pack(pack)
    ///     .build()
    ///     .validate(&segments);
    ///
    /// assert_eq!(report.total_issues(), 2); // four segments, capped at two
    /// # Ok::<(), edifact_rs::EdifactError>(())
    /// ```
    pub fn with_max_issues_per_rule(mut self, limit: impl Into<Option<usize>>) -> Self {
        self.max_issues_per_rule = limit.into();
        self
    }

    /// Add a rule closure.
    ///
    /// The closure receives the segments and a `Vec` to push issues into.  This
    /// is the everyday form; reach for
    /// [`with_contextual_rule_fn`][Self::with_contextual_rule_fn] only when the
    /// rule needs the per-call [`ValidationRuleContext`].
    ///
    /// The rule is anonymous, so [`merge_with_override`][Self::merge_with_override]
    /// cannot replace it — use [`with_named_rule_fn`][Self::with_named_rule_fn]
    /// for anything a downstream pack may need to override.
    ///
    /// # Example
    ///
    /// ```
    /// use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity};
    ///
    /// let pack = ProfileRulePack::new("ORDERS").with_rule_fn(|segments, issues| {
    ///     if !segments.iter().any(|segment| segment.tag == "BGM") {
    ///         issues.push(ValidationIssue::new(
    ///             ValidationSeverity::Error,
    ///             "ORDERS requires a BGM",
    ///         ));
    ///     }
    /// });
    /// assert_eq!(pack.rule_count(), 1);
    /// ```
    pub fn with_rule_fn<F>(mut self, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>], &mut Vec<ValidationIssue>) + Send + Sync + 'static,
    {
        self.rules.push(NamedRule {
            id: None,
            rule: Arc::new(StatelessClosureProfileRule(rule)),
        });
        self
    }

    /// Add a rule closure with a stable identifier.
    ///
    /// The identifier is what [`merge_with_override`][Self::merge_with_override]
    /// matches on, so name every rule a downstream pack may need to replace.
    pub fn with_named_rule_fn<F>(mut self, id: impl Into<Arc<str>>, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>], &mut Vec<ValidationIssue>) + Send + Sync + 'static,
    {
        self.rules.push(NamedRule {
            id: Some(id.into()),
            rule: Arc::new(StatelessClosureProfileRule(rule)),
        });
        self
    }

    /// Add a rule closure that also receives the [`ValidationRuleContext`].
    ///
    /// The context carries the message reference, the message type, and any
    /// typed metadata passed to
    /// [`validate_with`][super::context::ValidationContext::validate_with].
    /// When the rule does not need it, [`with_rule_fn`][Self::with_rule_fn] is
    /// the shorter form.
    pub fn with_contextual_rule_fn<F>(mut self, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>], &ValidationRuleContext<'_>, &mut Vec<ValidationIssue>)
            + Send
            + Sync
            + 'static,
    {
        self.rules.push(NamedRule {
            id: None,
            rule: Arc::new(ClosureProfileRule(rule)),
        });
        self
    }

    /// Add a context-aware rule closure with a stable identifier.
    ///
    /// [`with_contextual_rule_fn`][Self::with_contextual_rule_fn] plus the
    /// override identifier described on
    /// [`with_named_rule_fn`][Self::with_named_rule_fn].
    pub fn with_named_contextual_rule_fn<F>(mut self, id: impl Into<Arc<str>>, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>], &ValidationRuleContext<'_>, &mut Vec<ValidationIssue>)
            + Send
            + Sync
            + 'static,
    {
        self.rules.push(NamedRule {
            id: Some(id.into()),
            rule: Arc::new(ClosureProfileRule(rule)),
        });
        self
    }

    /// Add a rule that asserts segment `tag` is present at least once.
    ///
    /// Emits an `Error`-severity issue when no segment with `tag` is found.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let pack = ProfileRulePack::new("MY-PROFILE")
    ///     .require_segment("BGM", "MY-BGM-M")
    ///     .require_segment("DTM", "MY-DTM-M");
    /// ```
    pub fn require_segment(self, tag: &'static str, rule_id: impl Into<Arc<str>>) -> Self {
        let id: Arc<str> = rule_id.into();
        self.with_named_rule_fn(id.clone(), move |segments, issues| {
            if !segments.iter().any(|s| s.tag == tag) {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!("mandatory segment {tag} is missing"),
                    )
                    .with_segment(tag)
                    .with_rule_id(id.as_ref()),
                );
            }
        })
    }

    /// Add a rule that asserts segment `tag` does **not** appear.
    ///
    /// Emits an `Error`-severity issue for each occurrence found.
    pub fn forbid_segment(self, tag: &'static str, rule_id: impl Into<Arc<str>>) -> Self {
        let id: Arc<str> = rule_id.into();
        self.with_named_rule_fn(id.clone(), move |segments, issues| {
            for (occ, s) in segments.iter().filter(|s| s.tag == tag).enumerate() {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!("segment {tag} must not appear"),
                    )
                    .with_span(s.span)
                    .with_segment(tag)
                    .with_segment_occurrence(u16::try_from(occ).unwrap_or(u16::MAX))
                    .with_rule_id(id.as_ref()),
                );
            }
        })
    }

    /// Add a rule that asserts data element `de_qualifier` at `(element, component)` equals
    /// `qualifier` for every occurrence of `tag`.
    pub fn require_qualifier(
        self,
        tag: &'static str,
        element: u8,
        component: u8,
        qualifier: &'static str,
        rule_id: impl Into<Arc<str>>,
    ) -> Self {
        let id: Arc<str> = rule_id.into();
        self.with_named_rule_fn(id.clone(), move |segments, issues| {
            for (occ, s) in segments.iter().filter(|s| s.tag == tag).enumerate() {
                let actual = s
                    .get_element(element as usize)
                    .and_then(|e| e.get_component(component as usize));
                if actual != Some(qualifier) {
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            format!(
                                "segment {tag} element {element} component {component} must be \
                                 {qualifier:?} but found {:?}",
                                actual.unwrap_or("<absent>")
                            ),
                        )
                        .with_segment(tag)
                        .with_element_index(element)
                        .with_component_index(component)
                        .with_segment_occurrence(u16::try_from(occ).unwrap_or(u16::MAX))
                        .with_rule_id(id.as_ref()),
                    );
                }
            }
        })
    }

    // ── Group-scoped rule builders ──────────────────────────────────────────

    /// Add a group-aware rule closure that fires for **every** group node in the
    /// DFS traversal of the segment-group tree.
    ///
    /// The closure receives:
    /// - `group: &SegmentGroupIndexed` — the current tree node (with `definition`,
    ///   `total_span`, `children`).
    /// - `group_segments: &[Segment<'_>]` — all segments in this group's subtree
    ///   (`all_segments[group.total_span.clone()]`).
    /// - `context: &ValidationRuleContext<'_>` — per-call metadata and message info.
    /// - `issues: &mut Vec<ValidationIssue>` — push violations here.
    ///
    /// # Group-name scoping
    ///
    /// Use [`with_scoped_group_rule_fn`](Self::with_scoped_group_rule_fn) when you
    /// only want the rule to fire for a specific group definition (e.g. `"SG5"`).
    pub fn with_group_rule_fn<F>(mut self, rule: F) -> Self
    where
        F: Fn(
                &SegmentGroupIndexed<'_>,
                &[Segment<'_>],
                &ValidationRuleContext<'_>,
                &mut Vec<ValidationIssue>,
            ) + Send
            + Sync
            + 'static,
    {
        self.group_rules.push(NamedGroupRule {
            id: None,
            group_scope: None,
            rule: Arc::new(rule),
        });
        self
    }

    /// Add a **named** group-aware rule closure that fires for every group node.
    pub fn with_named_group_rule_fn<F>(mut self, id: impl Into<Arc<str>>, rule: F) -> Self
    where
        F: Fn(
                &SegmentGroupIndexed<'_>,
                &[Segment<'_>],
                &ValidationRuleContext<'_>,
                &mut Vec<ValidationIssue>,
            ) + Send
            + Sync
            + 'static,
    {
        self.group_rules.push(NamedGroupRule {
            id: Some(id.into()),
            group_scope: None,
            rule: Arc::new(rule),
        });
        self
    }

    /// Add a named group-aware rule closure scoped to a specific group definition.
    ///
    /// The closure is called only when the DFS traversal enters a group whose
    /// [`SegmentGroupIndexed::definition`] equals `group_scope` (e.g. `"SG5"`).
    ///
    /// Accepts any `impl Into<Arc<str>>` as `group_scope`, so both `&'static str`
    /// literals and owned `String`s are valid.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let pack = ProfileRulePack::new("PROFILE-ORDERS")
    ///     .with_scoped_group_rule_fn("SG5", "SG5-CAV-M", |_group, segs, _ctx, issues| {
    ///         if !segs.iter().any(|s| s.tag == "CAV") {
    ///             issues.push(
    ///                 ValidationIssue::new(ValidationSeverity::Error, "CAV missing in SG5")
    ///                     .with_segment("CAV")
    ///                     .with_rule_id("SG5-CAV-M"),
    ///             );
    ///         }
    ///     });
    /// ```
    pub fn with_scoped_group_rule_fn<F>(
        mut self,
        group_scope: impl Into<Arc<str>>,
        id: impl Into<Arc<str>>,
        rule: F,
    ) -> Self
    where
        F: Fn(
                &SegmentGroupIndexed<'_>,
                &[Segment<'_>],
                &ValidationRuleContext<'_>,
                &mut Vec<ValidationIssue>,
            ) + Send
            + Sync
            + 'static,
    {
        self.group_rules.push(NamedGroupRule {
            id: Some(id.into()),
            group_scope: Some(group_scope.into()),
            rule: Arc::new(rule),
        });
        self
    }

    /// Assert segment `tag` is present in every occurrence of group `group_scope`.
    ///
    /// For example, `require_segment_in_group("SG5", "LOC", "SG5-LOC-M")` fires
    /// once per `SG5` instance that contains no `LOC` segment.
    ///
    /// Issues are automatically annotated with the group name.
    ///
    /// Accepts any `impl Into<Arc<str>>` as `group_scope`.
    pub fn require_segment_in_group(
        self,
        group_scope: impl Into<Arc<str>>,
        tag: &'static str,
        rule_id: impl Into<Arc<str>>,
    ) -> Self {
        let scope: Arc<str> = group_scope.into();
        let id: Arc<str> = rule_id.into();
        let scope_msg = Arc::clone(&scope);
        self.with_scoped_group_rule_fn(scope, id.clone(), move |_group, segs, _ctx, issues| {
            if !segs.iter().any(|s| s.tag == tag) {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!("mandatory segment {tag} is missing from group {scope_msg}"),
                    )
                    .with_segment(tag)
                    .with_rule_id(id.as_ref()),
                );
            }
        })
    }

    /// Assert segment `tag` does **not** appear in any occurrence of group `group_scope`.
    ///
    /// Emits an `Error`-severity issue for each occurrence found.
    ///
    /// Accepts any `impl Into<Arc<str>>` as `group_scope`.
    pub fn forbid_segment_in_group(
        self,
        group_scope: impl Into<Arc<str>>,
        tag: &'static str,
        rule_id: impl Into<Arc<str>>,
    ) -> Self {
        let scope: Arc<str> = group_scope.into();
        let id: Arc<str> = rule_id.into();
        let scope_msg = Arc::clone(&scope);
        self.with_scoped_group_rule_fn(scope, id.clone(), move |_group, segs, _ctx, issues| {
            for (occ, s) in segs.iter().filter(|s| s.tag == tag).enumerate() {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!("segment {tag} must not appear in group {scope_msg}"),
                    )
                    .with_span(s.span)
                    .with_segment(tag)
                    .with_segment_occurrence(u16::try_from(occ).unwrap_or(u16::MAX))
                    .with_rule_id(id.as_ref()),
                );
            }
        })
    }

    /// Assert qualifier `qualifier` at `(element, component)` in segment `tag` is
    /// present in every occurrence of group `group_scope`.
    ///
    /// Accepts any `impl Into<Arc<str>>` as `group_scope`.
    pub fn require_qualifier_in_group(
        self,
        group_scope: impl Into<Arc<str>>,
        tag: &'static str,
        element: u8,
        component: u8,
        qualifier: &'static str,
        rule_id: impl Into<Arc<str>>,
    ) -> Self {
        let scope: Arc<str> = group_scope.into();
        let id: Arc<str> = rule_id.into();
        let scope_msg = Arc::clone(&scope);
        self.with_scoped_group_rule_fn(scope, id.clone(), move |_group, segs, _ctx, issues| {
            for (occ, s) in segs.iter().filter(|s| s.tag == tag).enumerate() {
                let actual = s
                    .get_element(element as usize)
                    .and_then(|e| e.get_component(component as usize));
                if actual != Some(qualifier) {
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            format!(
                                "segment {tag} element {element} component {component} must be \
                                 {qualifier:?} in group {scope_msg}, found {:?}",
                                actual.unwrap_or("<absent>")
                            ),
                        )
                        .with_segment(tag)
                        .with_element_index(element)
                        .with_component_index(component)
                        .with_segment_occurrence(u16::try_from(occ).unwrap_or(u16::MAX))
                        .with_rule_id(id.as_ref()),
                    );
                }
            }
        })
    }

    /// Return the number of group-scoped rules in this pack.
    pub fn group_rule_count(&self) -> usize {
        self.group_rules.len()
    }

    // ── Scope filtering ────────────────────────────────────────────────────

    /// Whether this pack's message-type and release scopes admit `segments`.
    ///
    /// Both scopes read `UNH` S009 — the message type is component 0, the
    /// association assigned code (DE 0057) is component 4 — so the flat and the
    /// group pass share one implementation instead of two copies that could
    /// drift.  The `UNH` is looked up only for the scopes that are actually
    /// configured, and the message type is taken from the pre-extracted
    /// [`ValidationRuleContext`] when the surrounding
    /// [`ValidationContext`][super::context::ValidationContext] already found it.
    fn scope_admits(&self, segments: &[Segment<'_>], context: &ValidationRuleContext<'_>) -> bool {
        if !self.message_types.is_empty() {
            let message_type = context
                .message_type
                .or_else(|| unh_s009(segments).and_then(|e| e.get_component(0)));
            if !message_type.is_some_and(|mt| self.message_types.iter().any(|x| x == mt)) {
                return false;
            }
        }
        if let Some(bound_release) = &self.release {
            if unh_s009(segments).and_then(|e| e.get_component(4)) != Some(bound_release.as_str()) {
                return false;
            }
        }
        true
    }

    // ── Private group validation engine ────────────────────────────────────

    /// Recursively walk the segment-group tree and evaluate group-scoped rules.
    ///
    /// Called internally by [`Validator::validate_group_batch`].
    ///
    /// `budget[i]` is how many further issues group rule `i` may still emit on
    /// this call.  It is threaded through the recursion rather than reset per
    /// group: [`with_max_issues_per_rule`][Self::with_max_issues_per_rule] caps
    /// *per rule per call*, and a group rule fires once per group occurrence.
    fn walk_group_tree(
        &self,
        group: &SegmentGroupIndexed<'_>,
        all_segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
        budget: &mut [usize],
    ) {
        let group_segs = all_segments.get(group.total_span.clone()).unwrap_or(&[]);
        let mut rule_issues: Vec<ValidationIssue> = Vec::new();

        for (index, named) in self.group_rules.iter().enumerate() {
            // Skip if this rule is scoped to a different group name.
            if let Some(scope) = &named.group_scope {
                if group.definition != scope.as_ref() {
                    continue;
                }
            }
            // Skip the call rather than discard its output: a rule that has
            // spent its budget should not cost time it cannot report.
            if self.max_issues_per_rule.is_some() && budget[index] == 0 {
                continue;
            }
            let errors_before = report.errors.len();
            (named.rule)(group, group_segs, context, &mut rule_issues);
            if self.max_issues_per_rule.is_some() {
                rule_issues.truncate(budget[index]);
                budget[index] -= rule_issues.len();
            }
            for mut issue in rule_issues.drain(..) {
                // Auto-stamp the group name if the rule didn't set it explicitly.
                if issue.segment_group.is_none() {
                    issue = issue.with_segment_group(group.definition);
                }
                match issue.severity {
                    ValidationSeverity::Critical | ValidationSeverity::Error => {
                        report.add_error(issue);
                    }
                    ValidationSeverity::Warning => {
                        report.add_warning(issue);
                    }
                    ValidationSeverity::Info => {
                        report.add_info(issue);
                    }
                }
            }
            if self.bail_on_first_error && report.errors.len() > errors_before {
                return;
            }
        }

        for child in &group.children {
            let errors_before_child = report.errors.len();
            self.walk_group_tree(child, all_segments, report, context, budget);
            if self.bail_on_first_error && report.errors.len() > errors_before_child {
                return;
            }
        }
    }

    /// Add a rule that implements [`ProfileRule`].
    pub fn with_rule(mut self, rule: impl ProfileRule + 'static) -> Self {
        self.rules.push(NamedRule {
            id: None,
            rule: Arc::new(rule),
        });
        self
    }

    /// Add a named rule that implements [`ProfileRule`].
    pub fn with_named_rule(
        mut self,
        id: impl Into<Arc<str>>,
        rule: impl ProfileRule + 'static,
    ) -> Self {
        self.rules.push(NamedRule {
            id: Some(id.into()),
            rule: Arc::new(rule),
        });
        self
    }

    /// Prepend all rules from `base` to this pack.
    ///
    /// Rules from `base` are shared (via [`Arc`] cloning) and run first.
    /// Message-type restrictions from `base` are also merged.  The resulting
    /// release scope must be compatible with both packs.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::IncompatibleReleaseScopes`] if both packs specify
    /// different release scopes.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let base = ProfileRulePack::new("MIG-BASE")
    ///     .with_rule_fn(/* mandatory segment rules */);
    ///
    /// let profile_4711 = ProfileRulePack::new("PROFILE-4711")
    ///     .extend_from(&base)?
    ///     .with_rule_fn(/* 4711-specific rules */);
    /// ```
    ///
    /// When your base pack is wrapped in an [`Arc`] you can dereference it:
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    ///
    /// let base: Arc<ProfileRulePack> = Arc::new(
    ///     ProfileRulePack::new("BASE").with_rule_fn(/* … */),
    /// );
    ///
    /// let derived = ProfileRulePack::new("DERIVED")
    ///     .extend_from(&*base)?          // deref Arc<T> to &T
    ///     .with_rule_fn(/* … */);
    /// ```
    pub fn extend_from(mut self, base: &ProfileRulePack) -> Result<Self, EdifactError> {
        let mut combined = base.rules.clone();
        combined.append(&mut self.rules);
        self.rules = combined;
        // Prepend group rules from base too.
        let mut combined_group = base.group_rules.clone();
        combined_group.append(&mut self.group_rules);
        self.group_rules = combined_group;
        for mt in &base.message_types {
            if !self.message_types.iter().any(|x| x == mt) {
                self.message_types.push(mt.clone());
            }
        }
        self.release = merge_release_scopes(self.release.take(), base.release.clone())?;
        Ok(self)
    }

    /// Merge `other` into `self`, with `other` taking precedence for any rule
    /// whose id already exists in `self`.
    ///
    /// - Rules in `other` that have a stable id matching a rule in `self` **replace**
    ///   the rule at the same position in `self`.
    /// - Rules in `other` with no id, or with an id not present in `self`, are
    ///   **appended** to `self`.
    /// - Rules present only in `self` (no matching override in `other`) are
    ///   **retained unchanged**.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::IncompatibleReleaseScopes`] if both packs specify
    /// different release scopes.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let base = ProfileRulePack::new("MIG-5.4")
    ///     .with_named_rule_fn("PROFILE-4711-BGM-M", |segs, _issues| { /* old */ });
    ///
    /// let delta = ProfileRulePack::new("MIG-5.5-delta")
    ///     .with_named_rule_fn("PROFILE-4711-BGM-M", |segs, _issues| { /* updated */ });
    ///
    /// // `result` runs the updated BGM-M rule only once:
    /// let result = base.merge_with_override(delta)?;
    /// assert_eq!(result.rule_count(), 1);
    /// ```
    pub fn merge_with_override(mut self, mut other: Self) -> Result<Self, EdifactError> {
        let mut id_to_index: std::collections::HashMap<Arc<str>, usize> = Default::default();
        for (idx, rule) in self.rules.iter().enumerate() {
            if let Some(id) = &rule.id {
                id_to_index.insert(id.clone(), idx);
            }
        }

        let mut replacements: Vec<(usize, NamedRule)> = Vec::new();
        let mut to_append = Vec::new();

        for other_rule in other.rules.drain(..) {
            if let Some(id) = &other_rule.id {
                if let Some(&idx) = id_to_index.get(id) {
                    replacements.push((idx, other_rule));
                } else {
                    to_append.push(other_rule);
                }
            } else {
                to_append.push(other_rule);
            }
        }

        for (idx, rule) in replacements {
            if idx < self.rules.len() {
                self.rules[idx] = rule;
            }
        }

        self.rules.append(&mut to_append);
        for mt in other.message_types.drain(..) {
            if !self.message_types.contains(&mt) {
                self.message_types.push(mt);
            }
        }
        // Merge group rules: named overrides replace matching entries; others are appended.
        let mut group_id_to_index: std::collections::HashMap<Arc<str>, usize> = Default::default();
        for (idx, rule) in self.group_rules.iter().enumerate() {
            if let Some(id) = &rule.id {
                group_id_to_index.insert(id.clone(), idx);
            }
        }
        let mut group_replacements: Vec<(usize, NamedGroupRule)> = Vec::new();
        let mut group_to_append = Vec::new();
        for other_rule in other.group_rules.drain(..) {
            if let Some(id) = &other_rule.id {
                if let Some(&idx) = group_id_to_index.get(id) {
                    group_replacements.push((idx, other_rule));
                } else {
                    group_to_append.push(other_rule);
                }
            } else {
                group_to_append.push(other_rule);
            }
        }
        for (idx, rule) in group_replacements {
            if idx < self.group_rules.len() {
                self.group_rules[idx] = rule;
            }
        }
        self.group_rules.append(&mut group_to_append);
        self.release = merge_release_scopes(self.release.take(), other.release.take())?;
        Ok(self)
    }
}

/// The `UNH` S009 composite (message identifier), if the slice carries a `UNH`.
///
/// One lookup answers both scope questions: message type is component 0,
/// association assigned code (DE 0057) is component 4.
fn unh_s009<'s, 'd>(segments: &'s [Segment<'d>]) -> Option<&'s crate::model::Element<'d>> {
    segments
        .iter()
        .find(|s| s.tag == "UNH")
        .and_then(|s| s.get_element(1))
}

pub(super) fn merge_release_scopes(
    current: Option<String>,
    incoming: Option<String>,
) -> Result<Option<String>, EdifactError> {
    match (current, incoming) {
        (Some(x), Some(y)) if x != y => Err(EdifactError::IncompatibleReleaseScopes {
            current: x,
            incoming: y,
        }),
        (Some(x), Some(_)) => Ok(Some(x)),
        (Some(x), None) => Ok(Some(x)),
        (None, incoming) => Ok(incoming),
    }
}

impl Validator for ProfileRulePack {
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
    ) {
        if !self.scope_admits(segments, context) {
            return;
        }

        let mut rule_issues: Vec<ValidationIssue> = Vec::new();

        for named in &self.rules {
            let errors_before = report.errors.len();
            named.rule.evaluate(segments, context, &mut rule_issues);
            // Apply per-rule issue cap if configured.
            if let Some(limit) = self.max_issues_per_rule {
                rule_issues.truncate(limit);
            }
            for issue in rule_issues.drain(..) {
                match issue.severity {
                    ValidationSeverity::Critical | ValidationSeverity::Error => {
                        report.add_error(issue);
                    }
                    ValidationSeverity::Warning => {
                        report.add_warning(issue);
                    }
                    ValidationSeverity::Info => {
                        report.add_info(issue);
                    }
                }
            }
            if self.bail_on_first_error && report.errors.len() > errors_before {
                return;
            }
        }
    }

    fn validate_group_batch(
        &self,
        root: &SegmentGroupIndexed<'_>,
        all_segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
    ) {
        if self.group_rules.is_empty() {
            return;
        }

        if !self.scope_admits(all_segments, context) {
            return;
        }

        // One budget slot per group rule, spent across the whole tree walk.
        let mut budget =
            vec![self.max_issues_per_rule.unwrap_or(usize::MAX); self.group_rules.len()];
        self.walk_group_tree(root, all_segments, report, context, &mut budget);
    }

    fn has_group_rules(&self) -> bool {
        !self.group_rules.is_empty()
    }

    fn fork(&self) -> Option<Box<dyn Validator + Send + Sync>> {
        Some(Box::new(self.clone()))
    }
}

impl Clone for ProfileRulePack {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            message_types: self.message_types.clone(),
            release: self.release.clone(),
            rules: self.rules.clone(),
            group_rules: self.group_rules.clone(),
            bail_on_first_error: self.bail_on_first_error,
            max_issues_per_rule: self.max_issues_per_rule,
        }
    }
}

impl std::fmt::Debug for ProfileRulePack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileRulePack")
            .field("name", &self.name)
            .field("message_types", &self.message_types)
            .field("release", &self.release)
            .field("rule_count", &self.rules.len())
            .field("group_rule_count", &self.group_rules.len())
            .field("bail_on_first_error", &self.bail_on_first_error)
            .finish()
    }
}

/// `Arc<ProfileRulePack>` can be plugged directly into a [`super::context::ValidationContext`].
///
/// Forking (for `fork_with_message_ref`) only increments the reference count — no
/// deep copy of the rule vec is performed.  This is the zero-allocation path for
/// downstream code that caches packs in a `LazyLock` or `OnceLock`.
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::{Arc, LazyLock};
/// use edifact_rs::{ProfileRulePack, ValidationContext};
///
/// static ORDERS_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
///     Arc::new(
///         ProfileRulePack::new("ORDERS-MIG")
///             .for_message_type("ORDERS")
///             .require_segment("BGM", "MIG-BGM-M"),
///     )
/// });
///
/// let ctx = ValidationContext::builder()
///     .with_profile_pack_arc(Arc::clone(&ORDERS_PACK))
///     .build();
/// ```
impl Validator for Arc<ProfileRulePack> {
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
    ) {
        self.as_ref().validate_batch(segments, report, context);
    }

    fn validate_group_batch(
        &self,
        root: &SegmentGroupIndexed<'_>,
        all_segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
    ) {
        self.as_ref()
            .validate_group_batch(root, all_segments, report, context);
    }

    fn has_group_rules(&self) -> bool {
        self.as_ref().has_group_rules()
    }

    fn fork(&self) -> Option<Box<dyn Validator + Send + Sync>> {
        Some(Box::new(Arc::clone(self)))
    }
}
