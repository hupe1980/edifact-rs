//! Validation pipeline for structural and semantic EDIFACT checks.

use crate::{
    EdifactError, OwnedSegment, Segment, ValidationIssue, ValidationReport, ValidationSeverity,
};
use std::any::Any;
use std::sync::Arc;

/// Typed context injected into profile rule closures at validation time.
///
/// Rules access per-call metadata via [`ValidationRuleContext::metadata`] and
/// the message reference (UNH element 0) via [`ValidationRuleContext::message_ref`].
///
/// # Example
///
/// ```rust,ignore
/// let pack = ProfileRulePack::new("AHB-11001")
///     .with_rule_fn(|segs, ctx, issues| {
///         let pruefid: &Pruefid = ctx.metadata()?;
///         let msg_ref = ctx.message_ref().unwrap_or("<unknown>");
///         // use pruefid and msg_ref …
///     });
///
/// let report = ValidationContext::builder()
///     .with_profile_pack(pack)
///     .with_message_ref("0001")
///     .build()
///     .validate_lenient_with(&segments, &my_pruefid);
/// ```
#[derive(Clone, Copy)]
pub struct ValidationRuleContext<'a> {
    metadata: Option<&'a (dyn Any + Send + Sync)>,
    /// Message reference (`UNH` element 0) for this validation call.
    ///
    /// Set via [`ValidationContextBuilder::with_message_ref`] or via
    /// [`ValidationContext::validate_lenient_with`].  `None` when no reference was configured.
    pub message_ref: Option<&'a str>,
}

impl<'a> ValidationRuleContext<'a> {
    /// Construct a context with no metadata and no message reference.
    pub fn empty() -> Self {
        Self {
            metadata: None,
            message_ref: None,
        }
    }

    /// Construct a context holding a typed metadata reference.
    pub fn new<T: Any + Send + Sync>(value: &'a T) -> Self {
        Self {
            metadata: Some(value as &(dyn Any + Send + Sync)),
            message_ref: None,
        }
    }

    /// Attach a message reference to this context (builder-style).
    pub fn with_message_ref(mut self, msg_ref: &'a str) -> Self {
        self.message_ref = Some(msg_ref);
        self
    }

    /// Downcast the metadata to `T`.  Returns `None` if no metadata was
    /// injected or if the concrete type does not match `T`.
    pub fn metadata<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.metadata?.downcast_ref::<T>()
    }

    /// Return `true` if metadata was provided.
    pub fn has_metadata(&self) -> bool {
        self.metadata.is_some()
    }
}

impl std::fmt::Debug for ValidationRuleContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationRuleContext")
            .field("has_metadata", &self.metadata.is_some())
            .field("message_ref", &self.message_ref)
            .finish()
    }
}

/// A profile rule that can be added to a [`ProfileRulePack`].
///
/// Implement this trait to create reusable, composable profile rules for
/// EDIFACT message validation.  Rules receive a [`ValidationRuleContext`] that
/// provides optional typed metadata injected at validation call time via
/// [`ValidationContext::validate_lenient_with`].
///
/// # Multiple issues per invocation
///
/// [`evaluate`](ProfileRule::evaluate) appends issues into a caller-supplied
/// `Vec` rather than returning a single `Option`.  This lets one rule iterate
/// every matching segment and report *all* violations — not just the first.
///
/// Rules that only ever emit a single issue can still return early:
///
/// ```rust,ignore
/// fn evaluate(&self, segments: &[Segment<'_>], _ctx: &ValidationRuleContext<'_>,
///             issues: &mut Vec<ValidationIssue>) {
///     if let Some(problem) = check_something(segments) {
///         issues.push(problem);
///     }
/// }
/// ```
///
/// # `bail_on_first_error` interaction
///
/// When [`ProfileRulePack::bail_on_first_error`] is set, the pack stops calling
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
struct NamedRule {
    /// Stable identifier for this rule, e.g. `"AHB-11001-BGM-M"`.
    ///
    /// `None` for anonymous rules that can never be overridden by id.
    id: Option<Arc<str>>,
    rule: Arc<dyn ProfileRule + Send + Sync>,
}

impl Clone for NamedRule {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            rule: Arc::clone(&self.rule),
        }
    }
}

/// A profile/MIG rule pack that can be plugged into `ValidationContext`.
pub struct ProfileRulePack {
    name: String,
    /// Set of EDIFACT message types this pack is scoped to (e.g. `"ORDERS"`, `"INVOIC"`).
    ///
    /// `BTreeSet` provides O(log n) membership tests and deterministic iteration order
    /// without requiring the `hashbrown` dependency.  Profile packs rarely contain more
    /// than a handful of types, so the difference over a `Vec` is negligible in practice,
    /// but the semantics (no duplicates, sorted iteration) are more correct.
    message_types: std::collections::BTreeSet<String>,
    /// Association-assigned code (DE 0057) this pack is bound to, e.g. `"5.5.3a"`.
    ///
    /// `None` means the pack applies universally regardless of association code.
    release: Option<String>,
    rules: Vec<NamedRule>,
    bail_on_first_error: bool,
}

impl ProfileRulePack {
    /// Create an empty rule pack.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            message_types: std::collections::BTreeSet::new(),
            release: None,
            rules: Vec::new(),
            bail_on_first_error: false,
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

    /// Iterate over the stable identifiers of all **named** rules in this pack.
    ///
    /// Anonymous rules (added without an id) are skipped.
    pub fn rule_ids(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().filter_map(|r| r.id.as_deref())
    }

    /// Return the association-assigned release code this pack is bound to, if any.
    ///
    /// `None` means the pack applies to messages of any association code.
    pub fn release(&self) -> Option<&str> {
        self.release.as_deref()
    }

    /// Restrict this pack to one or more EDIFACT message types from the `UNH` segment.
    ///
    /// When a pack has one or more message-type restrictions, its rules are only evaluated
    /// against messages whose `UNH` element 1, component 0 matches one of the registered
    /// types (e.g. `"ORDERS"`, `"INVOIC"`).
    ///
    /// # Silent-skip behaviour
    ///
    /// If the input segments do not contain a `UNH` segment, or if the `UNH` message-type
    /// element is absent, the pack will **silently skip all rules** rather than returning an
    /// error.  This is intentional: without a readable message type the pack cannot
    /// determine whether its rules apply, so it errs on the side of no false positives.
    ///
    /// If you need a hard failure on a missing `UNH`, add a dedicated [`ProfileRule`] that
    /// checks for the segment's presence before other rules run.
    pub fn for_message_type(mut self, message_type: impl Into<String>) -> Self {
        self.message_types.insert(message_type.into());
        self
    }

    /// Bind this pack to a specific association-assigned code (DE 0057).
    ///
    /// When a release is set, rules are only evaluated against messages whose
    /// `UNH` element 1, component 4 matches `release` exactly (e.g. `"5.5.3a"`).
    /// Packs with no bound release are universal — they run for every message
    /// regardless of its association code.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let pack = ProfileRulePack::new("UTILMD-5.5.3a")
    ///     .for_message_type("UTILMD")
    ///     .for_release("5.5.3a");
    /// ```
    pub fn for_release(mut self, release: impl Into<String>) -> Self {
        self.release = Some(release.into());
        self
    }

    /// Stop evaluating rules in this pack after the first `Error`- or `Critical`-severity
    /// finding.
    ///
    /// Bail applies *per pack*, not globally — other packs in the
    /// [`ValidationContext`] still run even when this pack bails early.  This
    /// avoids flooding validation reports with cascading false positives when a
    /// mandatory segment is missing and all subsequent rules reference its content.
    pub fn bail_on_first_error(mut self, bail: bool) -> Self {
        self.bail_on_first_error = bail;
        self
    }

    /// Add a context-aware rule closure.
    ///
    /// The closure receives the segment slice, a [`ValidationRuleContext`], and a
    /// `&mut Vec<ValidationIssue>` to push any violations into.  Push nothing if
    /// the segments pass.  Multiple issues may be pushed per invocation.
    ///
    /// For rules that do not need context, use [`with_stateless_rule_fn`][Self::with_stateless_rule_fn].
    pub fn with_rule_fn<F>(mut self, rule: F) -> Self
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
    /// The `id` is used by [`merge_with_override`][Self::merge_with_override] to de-duplicate
    /// rules across packs: if `other` has a rule with the same `id`, it replaces the
    /// corresponding rule in `self`.
    pub fn with_named_rule_fn<F>(mut self, id: impl Into<Arc<str>>, rule: F) -> Self
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

    /// Add a context-free rule closure.
    ///
    /// The closure receives the segment slice and a `&mut Vec<ValidationIssue>` to
    /// push violations into.  Convenience wrapper for rules that do not inspect the
    /// [`ValidationRuleContext`].
    pub fn with_stateless_rule_fn<F>(mut self, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>], &mut Vec<ValidationIssue>) + Send + Sync + 'static,
    {
        self.rules.push(NamedRule {
            id: None,
            rule: Arc::new(StatelessClosureProfileRule(rule)),
        });
        self
    }

    /// Add a context-free rule closure with a stable identifier.
    ///
    /// See [`with_named_rule_fn`][Self::with_named_rule_fn] for override semantics.
    pub fn with_named_stateless_rule_fn<F>(mut self, id: impl Into<Arc<str>>, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>], &mut Vec<ValidationIssue>) + Send + Sync + 'static,
    {
        self.rules.push(NamedRule {
            id: Some(id.into()),
            rule: Arc::new(StatelessClosureProfileRule(rule)),
        });
        self
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
    ///
    /// See [`with_named_rule_fn`][Self::with_named_rule_fn] for override semantics.
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
    /// release scope must be compatible with both packs: if one pack is scoped
    /// to a release and the other is not, the scope is preserved; if both are
    /// scoped, they must match.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::IncompatibleReleaseScopes`] if both packs specify
    /// different release scopes.  Use
    /// [`merge_unchecked`][Self::merge_unchecked] in code-generated or
    /// build-verified contexts where compatibility is guaranteed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let base = ProfileRulePack::new("MIG-UTILMD-BASE")
    ///     .with_stateless_rule_fn(/* mandatory segment rules */);
    ///
    /// let ahb_11001 = ProfileRulePack::new("AHB-11001")
    ///     .extend_from(&base)?
    ///     .with_stateless_rule_fn(/* 11001-specific rules */);
    /// ```
    pub fn extend_from(
        mut self,
        base: &ProfileRulePack,
    ) -> Result<Self, crate::error::EdifactError> {
        let mut combined = base.rules.clone();
        combined.append(&mut self.rules);
        self.rules = combined;
        for mt in &base.message_types {
            self.message_types.insert(mt.clone());
        }
        self.release = merge_release_scopes(self.release.take(), base.release.clone())?;
        Ok(self)
    }

    /// Merge two packs into one combined pack.
    ///
    /// Rules from `self` run before rules from `other`.  If both packs contain
    /// named rules with the same id, **both run** — use
    /// [`merge_with_override`][Self::merge_with_override] to de-duplicate by id instead.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::IncompatibleReleaseScopes`] if both packs specify
    /// different release scopes.  Use
    /// [`merge_unchecked`][Self::merge_unchecked] in code-generated or
    /// build-verified contexts where compatibility is guaranteed.
    pub fn merge(mut self, mut other: Self) -> Result<Self, crate::error::EdifactError> {
        self.message_types.append(&mut other.message_types);
        self.release = merge_release_scopes(self.release.take(), other.release.take())?;
        self.rules.append(&mut other.rules);
        Ok(self)
    }

    /// Merge two packs without checking release-scope compatibility.
    ///
    /// Identical to [`merge`][Self::merge] except that incompatible release
    /// scopes do **not** panic — `other`'s release takes precedence when both
    /// packs specify different values.
    ///
    /// Use this in code-generated profiles where compatibility is guaranteed at
    /// build time and the `panic`-on-mismatch guard would only add noise.
    pub fn merge_unchecked(mut self, mut other: Self) -> Self {
        self.message_types.append(&mut other.message_types);
        // Let the incoming release win; `None` defers to whichever side has a value.
        self.release = match (self.release.take(), other.release.take()) {
            (_, Some(r)) => Some(r),
            (current, None) => current,
        };
        self.rules.append(&mut other.rules);
        self
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
    /// Message-type restrictions from `other` are merged into `self`.
    ///
    /// # Errors
    ///
    /// Returns [`EdifactError::IncompatibleReleaseScopes`] if both packs specify
    /// different release scopes.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let base = ProfileRulePack::new("UTILMD-5.4")
    ///     .with_named_stateless_rule_fn("AHB-11001-BGM-M", |segs, _issues| { /* old */ });
    ///
    /// let delta = ProfileRulePack::new("UTILMD-5.5-delta")
    ///     .with_named_stateless_rule_fn("AHB-11001-BGM-M", |segs, _issues| { /* updated */ });
    ///
    /// // `result` runs the updated BGM-M rule only once:
    /// let result = base.merge_with_override(delta)?;
    /// assert_eq!(result.rule_count(), 1);
    /// ```
    pub fn merge_with_override(
        mut self,
        mut other: Self,
    ) -> Result<Self, crate::error::EdifactError> {
        // Build an id→index map for self.rules to avoid O(n*m) behavior.
        let mut id_to_index: std::collections::HashMap<Arc<str>, usize> = Default::default();
        for (idx, rule) in self.rules.iter().enumerate() {
            if let Some(id) = &rule.id {
                id_to_index.insert(id.clone(), idx);
            }
        }

        // Process overrides in a single pass: collect replacements and appends.
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

        // Apply replacements in-place.
        for (idx, rule) in replacements {
            if idx < self.rules.len() {
                self.rules[idx] = rule;
            }
        }

        // Append new rules.
        self.rules.append(&mut to_append);

        self.message_types.append(&mut other.message_types);
        self.release = merge_release_scopes(self.release.take(), other.release.take())?;
        Ok(self)
    }
}

fn merge_release_scopes(
    current: Option<String>,
    incoming: Option<String>,
) -> Result<Option<String>, crate::error::EdifactError> {
    match (current, incoming) {
        (Some(x), Some(y)) if x != y => {
            Err(crate::error::EdifactError::IncompatibleReleaseScopes {
                current: x,
                incoming: y,
            })
        }
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
        let unh = segments.iter().find(|segment| segment.tag == "UNH");

        // Cache UNH element 1 to avoid two separate get_element(1) calls (F-019).
        let unh_e1 = unh.and_then(|s| s.get_element(1));

        // Message-type filter: skip if no registered type matches.
        let message_type = unh_e1.and_then(|e| e.get_component(0));
        if !self.message_types.is_empty()
            && !message_type.is_some_and(|mt| self.message_types.contains(mt))
        {
            return;
        }

        // Release filter: skip if pack is bound to a specific association code that
        // does not match the message's UNH DE 0057 (element 1, component 4).
        if let Some(bound_release) = &self.release {
            let msg_association = unh_e1.and_then(|e| e.get_component(4));
            if msg_association != Some(bound_release.as_str()) {
                return;
            }
        }

        // Reusable buffer: avoids a heap allocation per rule invocation on the
        // no-violation fast path.
        let mut rule_issues: Vec<ValidationIssue> = Vec::new();

        for named in &self.rules {
            let errors_before = report.errors.len();
            named.rule.evaluate(segments, context, &mut rule_issues);
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
            // bail_on_first_error fires at rule-invocation granularity: if this rule
            // pushed at least one error-severity issue, skip remaining rules.
            if self.bail_on_first_error && report.errors.len() > errors_before {
                return;
            }
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
            .field("bail_on_first_error", &self.bail_on_first_error)
            .finish()
    }
}

/// Validation layers used by [`ValidationContext`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationLayer {
    /// Interchange / message envelope checks (`UNB`/`UNH`/`UNT`/`UNZ` counts).
    Envelope,
    /// Directory structure checks (segment presence/order/arity).
    Structure,
    /// Directory code-list checks.
    CodeList,
    /// Downstream profile-pack checks.
    Profile,
}

struct LayeredValidator {
    layer: ValidationLayer,
    validator: Box<dyn Validator + Send + Sync>,
}

/// Runtime validation context for progressive layered validation.
pub struct ValidationContext {
    validators: Vec<LayeredValidator>,
    envelope_enabled: bool,
    structure_enabled: bool,
    code_list_enabled: bool,
    profile_enabled: bool,
    message_type: Option<String>,
    /// Injected into every emitted `ValidationIssue` when set.
    message_ref: Option<String>,
    metadata: Option<Arc<dyn Any + Send + Sync>>,
}

/// Builder for [`ValidationContext`].
#[must_use = "call `.build()` to produce a `ValidationContext`"]
pub struct ValidationContextBuilder {
    inner: ValidationContext,
}

impl Default for ValidationContextBuilder {
    /// Default context builder.
    ///
    /// Structure, code-list, and profile layers are enabled by default.
    /// The envelope layer is **disabled** by default; call
    /// [`ValidationContextBuilder::with_envelope_validation`] to enable it.
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationContextBuilder {
    /// Create a new context builder.
    ///
    /// Structure, code-list, and profile layers are enabled by default.
    /// The envelope layer is **disabled** by default; call
    /// [`with_envelope_validation`][Self::with_envelope_validation] to enable it
    /// and add the built-in [`EnvelopeValidator`] in one step.
    pub fn new() -> Self {
        Self {
            inner: ValidationContext {
                validators: Vec::new(),
                envelope_enabled: false,
                structure_enabled: true,
                code_list_enabled: true,
                profile_enabled: true,
                message_type: None,
                message_ref: None,
                metadata: None,
            },
        }
    }

    /// Attach typed metadata accessible to context-aware profile rules.
    ///
    /// Rules added with [`ProfileRulePack::with_rule_fn`] receive the metadata
    /// via [`ValidationRuleContext::metadata`] on every call to
    /// [`ValidationContext::validate_lenient`].
    ///
    /// For per-call metadata that varies between validation invocations, use
    /// [`ValidationContext::validate_lenient_with`] instead.
    pub fn with_metadata<T: Any + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.inner.metadata = Some(Arc::new(value));
        self
    }

    /// Stamp every issue produced by this context with the given message reference.
    ///
    /// The message reference corresponds to DE 0062 from the `UNH` segment.
    /// Use this when validating individual messages from a multi-message
    /// interchange so that issues in the resulting [`ValidationReport`] can be
    /// correlated back to the originating `UNH`/`UNT` envelope.
    pub fn with_message_ref(mut self, message_ref: impl Into<String>) -> Self {
        self.inner.message_ref = Some(message_ref.into());
        self
    }

    /// Set message type metadata for downstream validators.
    pub fn with_message_type(mut self, message_type: impl Into<String>) -> Self {
        self.inner.message_type = Some(message_type.into());
        let configured = self.inner.message_type.as_deref();
        for layered in &mut self.inner.validators {
            layered.validator.set_message_type(configured);
        }
        self
    }

    /// Enable/disable structure validators.
    pub fn structure(mut self, enabled: bool) -> Self {
        self.inner.structure_enabled = enabled;
        self
    }

    /// Enable/disable code-list validators.
    pub fn code_list(mut self, enabled: bool) -> Self {
        self.inner.code_list_enabled = enabled;
        self
    }

    /// Enable/disable profile validators.
    pub fn profile(mut self, enabled: bool) -> Self {
        self.inner.profile_enabled = enabled;
        self
    }

    /// Enable/disable envelope layer validators.
    ///
    /// Off by default.  Call [`with_envelope_validation`][Self::with_envelope_validation]
    /// to add the built-in [`EnvelopeValidator`] and enable the layer in one step.
    pub fn envelope(mut self, enabled: bool) -> Self {
        self.inner.envelope_enabled = enabled;
        self
    }

    /// Add the built-in [`EnvelopeValidator`] and enable the envelope layer.
    ///
    /// The built-in validator mirrors [`crate::validate_envelope`] but
    /// translates each structural error into a [`ValidationIssue`] so all
    /// issues land in the unified [`ValidationReport`] alongside profile and
    /// directory findings.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let report = ValidationContext::builder()
    ///     .with_envelope_validation()
    ///     .with_message_type("ORDERS")
    ///     .build()
    ///     .validate_lenient(&all_segments);
    /// ```
    pub fn with_envelope_validation(mut self) -> Self {
        self.inner.envelope_enabled = true;
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Envelope,
            validator: Box::new(EnvelopeValidator),
        });
        self
    }

    /// Add a validator assigned to `layer`.
    pub fn with_validator<V>(mut self, layer: ValidationLayer, mut validator: V) -> Self
    where
        V: Validator + 'static,
    {
        validator.set_message_type(self.inner.message_type.as_deref());
        self.inner.validators.push(LayeredValidator {
            layer,
            validator: Box::new(validator),
        });
        self
    }

    /// Add a profile rule pack to the profile layer.
    pub fn with_profile_pack(mut self, mut pack: ProfileRulePack) -> Self {
        pack.set_message_type(self.inner.message_type.as_deref());
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Profile,
            validator: Box::new(pack),
        });
        self
    }

    /// Finalize builder and create context.
    #[must_use = "call `.validate_lenient()` or `.validate_strict()` on the resulting context"]
    pub fn build(self) -> ValidationContext {
        self.inner
    }
}

impl ValidationContext {
    /// Start building a validation context.
    pub fn builder() -> ValidationContextBuilder {
        ValidationContextBuilder::new()
    }

    /// Execute validators in lenient mode for enabled layers.
    ///
    /// Uses any metadata set via [`ValidationContextBuilder::with_metadata`].
    /// For per-call metadata, use [`validate_lenient_with`][Self::validate_lenient_with].
    pub fn validate_lenient(&self, segments: &[Segment<'_>]) -> ValidationReport {
        self.validate_with_context(segments, &self.build_rule_context())
    }

    /// Execute validators with per-call typed metadata.
    ///
    /// The metadata is accessible inside context-aware rule closures via
    /// [`ValidationRuleContext::metadata`].  This is the recommended path when
    /// a single [`ProfileRulePack`] serves multiple process-variant contexts
    /// (e.g., one pack per message type, injecting the Pruefidentifikator at
    /// call time).
    pub fn validate_lenient_with<T: Any + Send + Sync>(
        &self,
        segments: &[Segment<'_>],
        value: &T,
    ) -> ValidationReport {
        let ctx = ValidationRuleContext {
            metadata: Some(value as &(dyn Any + Send + Sync)),
            message_ref: self.message_ref.as_deref(),
        };
        self.validate_with_context(segments, &ctx)
    }

    /// Execute validators in strict mode for enabled layers.
    ///
    /// Returns `Ok(report)` when validation produces no errors.  The `Err` variant
    /// **also contains the full report** (errors, warnings, and infos) so that
    /// callers can inspect all issues even on failure.
    ///
    /// Warnings do **not** cause this method to return `Err`.  Call
    /// [`validate_lenient`][Self::validate_lenient] if you want to inspect warnings
    /// without failing on errors.
    pub fn validate_strict(
        &self,
        segments: &[Segment<'_>],
    ) -> Result<ValidationReport, ValidationReport> {
        self.validate_lenient(segments).result()
    }

    /// Execute validators in strict mode with per-call typed metadata.
    ///
    /// See [`validate_lenient_with`][Self::validate_lenient_with] for context usage and
    /// [`validate_strict`][Self::validate_strict] for strict-mode semantics.
    pub fn validate_strict_with<T: Any + Send + Sync>(
        &self,
        segments: &[Segment<'_>],
        value: &T,
    ) -> Result<ValidationReport, ValidationReport> {
        self.validate_lenient_with(segments, value).result()
    }

    /// Execute validators in lenient mode against an owned-segment slice.
    ///
    /// Avoids building a `Vec<Segment<'_>>` for the entire slice up front.
    /// Instead, segments are converted to `Segment<'_>` on demand, per validator
    /// layer:
    ///
    /// - **Envelope layer**: converts the full slice once (`O(n)` allocations).
    /// - **Non-envelope layers after envelope ran**: converts only the
    ///   non-service segments (UNB/UNZ/UNG/UNE filtered out) — also `O(n)` but
    ///   a smaller constant.
    /// - **Non-envelope layers when no envelope ran**: converts the full slice
    ///   once, shared across all remaining layers via a lazy `OnceCell`.
    ///
    /// When no layers are enabled this returns an empty [`ValidationReport`]
    /// without any allocation.
    pub fn validate_lenient_owned(&self, segments: &[OwnedSegment]) -> ValidationReport {
        if self.validators.is_empty()
            && !self.envelope_enabled
            && !self.structure_enabled
            && !self.code_list_enabled
            && !self.profile_enabled
        {
            return ValidationReport::default();
        }
        self.validate_with_context_owned(segments, &self.build_rule_context())
    }

    fn build_rule_context(&self) -> ValidationRuleContext<'_> {
        self.metadata
            .as_ref()
            .map(|arc| ValidationRuleContext {
                metadata: Some(arc.as_ref() as &(dyn Any + Send + Sync)),
                message_ref: self.message_ref.as_deref(),
            })
            .unwrap_or_else(|| ValidationRuleContext {
                metadata: None,
                message_ref: self.message_ref.as_deref(),
            })
    }

    /// Internal: validate owned segments without the upfront full-slice
    /// `as_borrowed()` conversion that `validate_lenient_owned` used to require.
    fn validate_with_context_owned(
        &self,
        segments: &[OwnedSegment],
        context: &ValidationRuleContext<'_>,
    ) -> ValidationReport {
        let mut report = ValidationReport::default();
        // Lazy full-slice borrow — built only when the first non-envelope
        // validator needs it (i.e. when no envelope validator ran).
        let mut full_borrowed: Option<Vec<Segment<'_>>> = None;
        // Lazy filtered borrow — built once when the first non-envelope
        // validator runs after an envelope pass.
        let mut filtered_borrowed: Option<Vec<Segment<'_>>> = None;
        let mut envelope_ran = false;

        for lv in &self.validators {
            if !self.layer_enabled(lv.layer) {
                continue;
            }
            if lv.layer == ValidationLayer::Envelope {
                // Convert full slice for envelope validation.
                let borrowed: Vec<Segment<'_>> = segments.iter().map(|s| s.as_borrowed()).collect();
                lv.validator.validate_batch(&borrowed, &mut report, context);
                envelope_ran = true;
            } else if envelope_ran {
                // Use filtered slice (no service segments).
                let active = filtered_borrowed.get_or_insert_with(|| {
                    segments
                        .iter()
                        .filter(|s| !matches!(s.tag.as_str(), "UNB" | "UNZ" | "UNG" | "UNE"))
                        .map(|s| s.as_borrowed())
                        .collect()
                });
                lv.validator.validate_batch(active, &mut report, context);
            } else {
                // No envelope pass yet — use the full slice, lazily converted.
                let active = full_borrowed
                    .get_or_insert_with(|| segments.iter().map(|s| s.as_borrowed()).collect());
                lv.validator.validate_batch(active, &mut report, context);
            }
        }

        // Stamp every issue with the message reference if one was configured.
        if let Some(ref msg_ref) = self.message_ref {
            for issue in report
                .errors
                .iter_mut()
                .chain(report.warnings.iter_mut())
                .chain(report.infos.iter_mut())
            {
                if issue.message_ref.is_none() {
                    issue.message_ref = Some(msg_ref.clone());
                }
            }
        }
        report
    }

    /// Execute validators in strict mode against an owned-segment slice.
    ///
    /// Equivalent to [`validate_strict`][Self::validate_strict] but accepts
    /// `&[OwnedSegment]` directly, avoiding a manual `.as_borrowed()` conversion
    /// at the call site.
    pub fn validate_strict_owned(
        &self,
        segments: &[OwnedSegment],
    ) -> Result<ValidationReport, ValidationReport> {
        self.validate_lenient_owned(segments).result()
    }

    fn validate_with_context(
        &self,
        segments: &[Segment<'_>],
        context: &ValidationRuleContext<'_>,
    ) -> ValidationReport {
        let mut report = ValidationReport::default();
        // After the envelope layer runs, strip the interchange / functional-group
        // service segments so they do not reach structure / profile validators that
        // only understand message-level segments.  The allocation is deferred until
        // the envelope layer is actually present and enabled.
        let mut filtered: Option<Vec<Segment<'_>>> = None;
        let mut envelope_ran = false;

        for lv in &self.validators {
            if !self.layer_enabled(lv.layer) {
                continue;
            }
            // For the envelope layer always use the full unmodified slice.
            if lv.layer == ValidationLayer::Envelope {
                lv.validator.validate_batch(segments, &mut report, context);
                envelope_ran = true;
            } else {
                // For every other layer: if the envelope ran, use the filtered
                // slice that has UNB/UNZ/UNG/UNE removed; otherwise use the
                // original slice unchanged.
                let active: &[Segment<'_>] = if envelope_ran {
                    filtered.get_or_insert_with(|| {
                        segments
                            .iter()
                            .filter(|s| !matches!(s.tag, "UNB" | "UNZ" | "UNG" | "UNE"))
                            .cloned()
                            .collect()
                    })
                } else {
                    segments
                };
                lv.validator.validate_batch(active, &mut report, context);
            }
        }

        // Stamp every issue with the message reference if one was configured.
        if let Some(ref msg_ref) = self.message_ref {
            for issue in report
                .errors
                .iter_mut()
                .chain(report.warnings.iter_mut())
                .chain(report.infos.iter_mut())
            {
                if issue.message_ref.is_none() {
                    issue.message_ref = Some(msg_ref.clone());
                }
            }
        }
        report
    }

    /// Message type metadata associated with this context, if provided.
    pub fn message_type(&self) -> Option<&str> {
        self.message_type.as_deref()
    }

    /// Message reference (`UNH` element 0) associated with this context, if provided.
    pub fn message_ref(&self) -> Option<&str> {
        self.message_ref.as_deref()
    }

    fn layer_enabled(&self, layer: ValidationLayer) -> bool {
        match layer {
            ValidationLayer::Envelope => self.envelope_enabled,
            ValidationLayer::Structure => self.structure_enabled,
            ValidationLayer::CodeList => self.code_list_enabled,
            ValidationLayer::Profile => self.profile_enabled,
        }
    }
}

/// Pluggable validator for parsed EDIFACT segments.
///
/// The primary contract is [`validate_batch`](Validator::validate_batch), which processes an
/// entire segment sequence and appends issues to a [`ValidationReport`].
///
/// Validators receive a [`ValidationRuleContext`] that may carry typed metadata
/// injected at validation call time.  Implementations that do not need the
/// context may ignore it.
///
/// For validators that work segment-by-segment, the convenience function
/// [`validate_each`] iterates over the slice and calls a per-segment closure,
/// so you only need to implement `validate_batch`:
///
/// ```rust,ignore
/// fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport, _ctx: &ValidationRuleContext<'_>) {
///     validate_each(segments, report, |seg| {
///         // return Ok(()) or Err(EdifactError::...)
///         Ok(())
///     });
/// }
/// ```
pub trait Validator: Send + Sync {
    /// Validate a full segment set and append issues to `report`.
    ///
    /// Implementations that do not need the context may ignore the `context` parameter.
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
    );

    /// Configure message-type metadata for validators that support explicit scoping.
    fn set_message_type(&mut self, _message_type: Option<&str>) {}
}

/// Helper for per-segment validators: iterates `segments`, calls `f` for each one,
/// and converts any `Err` into report entries.
///
/// Use this in `validate_batch` implementations that work segment-by-segment:
///
/// ```rust,ignore
/// fn validate_batch(&self, segments: &[Segment<'_>], report: &mut ValidationReport) {
///     validate_each(segments, report, |seg| { /* ... */ Ok(()) });
/// }
/// ```
pub fn validate_each<F>(segments: &[Segment<'_>], report: &mut ValidationReport, mut f: F)
where
    F: FnMut(&Segment<'_>) -> Result<(), EdifactError>,
{
    for segment in segments {
        if let Err(err) = f(segment) {
            report_error(report, err);
        }
    }
}

/// Convert a low-level validation error to a user-facing issue and append it.
pub(crate) fn report_error(report: &mut ValidationReport, err: EdifactError) {
    let issue = issue_from_error(err);
    match issue.severity {
        ValidationSeverity::Critical | ValidationSeverity::Error => report.add_error(issue),
        ValidationSeverity::Warning => report.add_warning(issue),
        ValidationSeverity::Info => report.add_info(issue),
    }
}

// ── EnvelopeValidator ─────────────────────────────────────────────────────────

/// Built-in validator for EDIFACT interchange envelope structure.
///
/// Checks `UNB`/`UNH`/`UNT`/`UNZ` segment presence, message counts, and
/// segment counts.  Registered by
/// [`ValidationContextBuilder::with_envelope_validation`].
///
/// The validator translates each [`EdifactError`] from
/// [`crate::validate_envelope`] into a [`ValidationIssue`] so that envelope
/// findings appear in the unified [`ValidationReport`] alongside structure and
/// profile results.
pub struct EnvelopeValidator;

impl Validator for EnvelopeValidator {
    fn validate_batch(
        &self,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        _ctx: &ValidationRuleContext<'_>,
    ) {
        if let Err(e) = crate::envelope::validate_envelope(segments) {
            report_error(report, e);
        }
    }
}

fn issue_from_error(err: EdifactError) -> ValidationIssue {
    let code = err.stable_code();
    let mut issue = ValidationIssue::new(severity_for(&err), err.to_string()).with_error_code(code);
    let default_hint = err.recovery_hint();

    match err {
        EdifactError::InvalidSegmentForMessage { tag, offset, .. } => {
            issue = issue.with_segment(tag).with_offset(offset);
        }
        EdifactError::InvalidElementCount { tag, offset, .. } => {
            issue = issue.with_segment(tag).with_offset(offset);
        }
        EdifactError::InvalidComponentCount {
            tag,
            element_index,
            offset,
            ..
        } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(u8::try_from(element_index).unwrap_or(u8::MAX))
                .with_offset(offset);
        }
        EdifactError::InvalidCodeValue {
            tag,
            element_index,
            offset,
            suggestion,
            ..
        } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(u8::try_from(element_index).unwrap_or(u8::MAX))
                .with_offset(offset);
            if let Some(s) = suggestion {
                issue = issue.with_suggestion(s);
            }
        }
        EdifactError::MissingSegment { tag, .. } => {
            issue = issue.with_segment(tag);
        }
        EdifactError::QualifierMismatch { tag, offset, .. } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(0)
                .with_offset(offset);
        }
        EdifactError::ConditionalRequirementNotMet {
            tag,
            element_index,
            offset,
            ..
        } => {
            issue = issue
                .with_segment(tag)
                .with_element_index(u8::try_from(element_index).unwrap_or(u8::MAX))
                .with_offset(offset);
        }
        EdifactError::MissingRequiredElement { tag, element_index } => {
            issue = issue.with_segment(tag);
            if let Ok(idx) = u8::try_from(element_index) {
                issue = issue.with_element_index(idx);
            }
        }
        EdifactError::MissingRequiredComponent {
            tag,
            element_index,
            component_index,
        } => {
            issue = issue.with_segment(tag);
            if let Ok(ei) = u8::try_from(element_index) {
                issue = issue.with_element_index(ei);
            }
            if let Ok(ci) = u8::try_from(component_index) {
                issue = issue.with_component_index(ci);
            }
        }
        EdifactError::InvalidReleaseSequence { offset }
        | EdifactError::InvalidDelimiter { offset, .. }
        | EdifactError::InvalidText { offset }
        | EdifactError::UnexpectedEof { offset }
        | EdifactError::UnexpectedDataToken { offset }
        | EdifactError::FunctionalGroupNotSupported { offset } => {
            issue = issue.with_offset(offset);
        }
        _ => {}
    }

    if issue.suggestion.is_none() {
        if let Some(hint) = default_hint {
            issue = issue.with_suggestion(hint);
        }
    }

    issue
}

fn severity_for(err: &EdifactError) -> ValidationSeverity {
    match err {
        EdifactError::InvalidCodeValue { .. } | EdifactError::QualifierMismatch { .. } => {
            ValidationSeverity::Warning
        }
        _ => ValidationSeverity::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Element;

    fn demo_orders_profile_pack() -> ProfileRulePack {
        ProfileRulePack::new("ORDERS-DEMO")
            .for_message_type("ORDERS")
            .with_stateless_rule_fn(|segments, issues| {
                issues.extend((|| -> Option<ValidationIssue> {
                    let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
                    let document_code = bgm.get_element(0)?.get_component(0)?;
                    (document_code == "220").then(|| {
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            "profile rule DEMO-P001 violated: BGM document code 220 is rejected in this demo pack",
                        )
                        .with_rule_id("DEMO-P001")
                        .with_segment("BGM")
                        .with_element_index(0)
                        .with_suggestion("Use a different BGM document code in this demo pack")
                    })
                })());
            })
            .with_stateless_rule_fn(|segments, issues| {
                issues.extend((|| -> Option<ValidationIssue> {
                    let bgm = segments.iter().find(|segment| segment.tag == "BGM")?;
                    let reference = bgm.get_element(1)?.get_component(0)?;
                    (reference == "PO123").then(|| {
                        ValidationIssue::new(
                            ValidationSeverity::Warning,
                            "profile rule DEMO-P002 warning: purchase-order reference PO123 is reserved in this demo pack",
                        )
                        .with_rule_id("DEMO-P002")
                        .with_segment("BGM")
                        .with_element_index(1)
                        .with_suggestion("Use a non-reserved reference in this demo pack")
                    })
                })());
            })
    }

    struct RejectBgm;

    struct WarnBgm;

    impl Validator for RejectBgm {
        fn validate_batch(
            &self,
            segments: &[Segment<'_>],
            report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
            validate_each(segments, report, |segment| {
                if segment.tag == "BGM" {
                    return Err(EdifactError::InvalidSegmentForMessage {
                        tag: "BGM".to_owned(),
                        message_type: "TEST".to_owned(),
                        offset: segment.tag_span.start,
                    });
                }
                Ok(())
            });
        }
    }

    impl Validator for WarnBgm {
        fn validate_batch(
            &self,
            segments: &[Segment<'_>],
            report: &mut ValidationReport,
            _context: &ValidationRuleContext<'_>,
        ) {
            validate_each(segments, report, |segment| {
                if segment.tag == "BGM" {
                    return Err(EdifactError::InvalidCodeValue {
                        tag: "BGM".to_owned(),
                        element_index: 0,
                        value: "XXX".to_owned(),
                        code_list: "1001".to_owned(),
                        offset: segment.span.start,
                        suggestion: None,
                    });
                }
                Ok(())
            });
        }
    }

    fn test_segment(tag: &'static str) -> Segment<'static> {
        Segment {
            tag,
            span: crate::Span::new(0, 0),
            tag_span: crate::Span::new(0, 0),
            elements: vec![Element::of(&["x"])],
        }
    }

    #[test]
    fn lenient_collects_issues() {
        let segments = vec![test_segment("UNH"), test_segment("BGM")];
        let mut report = ValidationReport::default();
        RejectBgm.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());
        assert!(report.has_errors());
        assert_eq!(report.errors().len(), 1);
    }

    #[test]
    fn strict_fails_on_errors() {
        let segments = vec![test_segment("BGM")];
        let mut report = ValidationReport::default();
        RejectBgm.validate_batch(&segments, &mut report, &ValidationRuleContext::empty());
        assert!(report.has_errors());
        assert_eq!(report.errors().len(), 1);
    }

    #[test]
    fn context_builder_respects_layer_toggles() {
        let segments = vec![test_segment("BGM")];
        let ctx = ValidationContext::builder()
            .structure(false)
            .with_validator(ValidationLayer::Structure, RejectBgm)
            .with_validator(ValidationLayer::CodeList, WarnBgm)
            .build();

        let report = ctx.validate_lenient(&segments);
        assert!(!report.has_errors());
        assert_eq!(report.warnings().len(), 1);
    }

    #[test]
    fn context_strict_fails_when_structure_enabled() {
        let segments = vec![test_segment("BGM")];
        let ctx = ValidationContext::builder()
            .with_message_type("ORDERS")
            .with_validator(ValidationLayer::Structure, RejectBgm)
            .build();

        assert_eq!(ctx.message_type(), Some("ORDERS"));
        let result = ctx.validate_strict(&segments);
        assert!(result.is_err());
        assert!(result.unwrap_err().has_errors());
    }

    #[test]
    fn report_error_applies_default_recovery_hint() {
        let mut report = ValidationReport::default();
        report_error(
            &mut report,
            EdifactError::InvalidReleaseSequence { offset: 9 },
        );

        let issue = report
            .errors()
            .first()
            .expect("expected one issue in the report");
        let hint = issue
            .suggestion
            .as_deref()
            .expect("expected default hint to be set");
        assert!(hint.contains("Release character"));
        assert_eq!(issue.error_code, Some("E019"));
    }

    #[test]
    fn missing_required_component_maps_metadata_to_issue() {
        let mut report = ValidationReport::default();
        report_error(
            &mut report,
            EdifactError::MissingRequiredComponent {
                tag: "BGM".to_owned(),
                element_index: 2,
                component_index: 1,
            },
        );

        let issue = report.errors().first().expect("expected one issue");
        assert_eq!(issue.error_code, Some("E021"));
        assert_eq!(issue.segment_tag.as_deref(), Some("BGM"));
        assert_eq!(issue.element_index, Some(2));
        assert_eq!(issue.component_index, Some(1));
    }

    #[test]
    fn profile_pack_lenient_collects_profile_rule_issues() {
        let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("expected parse success");

        let ctx = ValidationContext::builder()
            .with_profile_pack(demo_orders_profile_pack())
            .build();

        let report = ctx.validate_lenient(&segments);
        assert!(report.has_errors());
        assert!(
            report
                .errors()
                .iter()
                .any(|issue| issue.rule_id.as_deref() == Some("DEMO-P001"))
        );
        assert!(
            report
                .warnings()
                .iter()
                .any(|issue| issue.rule_id.as_deref() == Some("DEMO-P002"))
        );
    }

    #[test]
    fn profile_pack_strict_fails_when_profile_errors_exist() {
        let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+PO123+9'UNT+3+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("expected parse success");

        let ctx = ValidationContext::builder()
            .with_profile_pack(demo_orders_profile_pack())
            .build();
        let result = ctx.validate_strict(&segments);
        assert!(result.is_err());
        assert!(result.unwrap_err().has_errors());
    }

    // ── bail_on_first_error ──────────────────────────────────────────────────

    /// A rule that emits two error-severity issues (one per DTM segment).
    fn two_dtm_errors_rule() -> ProfileRulePack {
        ProfileRulePack::new("TEST-BAIL")
            .with_stateless_rule_fn(|segments, issues| {
                // Rule A: emits one error per DTM segment.
                for seg in segments.iter().filter(|s| s.tag == "DTM") {
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            format!("DTM error at offset {}", seg.span.start),
                        )
                        .with_rule_id("BAIL-R1")
                        .with_segment("DTM"),
                    );
                }
            })
            .with_stateless_rule_fn(|segments, issues| {
                // Rule B: never fires; used to verify bail skips this rule.
                for seg in segments.iter().filter(|s| s.tag == "BGM") {
                    issues.push(
                        ValidationIssue::new(ValidationSeverity::Error, "BGM error")
                            .with_rule_id("BAIL-R2")
                            .with_segment(seg.tag),
                    );
                }
            })
    }

    #[test]
    fn bail_on_first_error_fires_at_rule_invocation_granularity() {
        // Two DTM segments → Rule A emits 2 errors for them.
        // With bail, Rule B (BGM check) must NOT run.
        let input =
            b"UNH+1+ORDERS:D:96A:UN'BGM+220+9'DTM+137:20240101:102'DTM+163:20240201:102'UNT+5+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse failed");

        let pack_with_bail = two_dtm_errors_rule().bail_on_first_error(true);
        let ctx = ValidationContext::builder()
            .with_profile_pack(pack_with_bail)
            .build();
        let report = ctx.validate_lenient(&segments);

        // Rule A fires: both DTM errors are in the report (the whole rule invocation
        // runs to completion before bail is checked).
        assert_eq!(
            report
                .errors()
                .iter()
                .filter(|i| i.rule_id.as_deref() == Some("BAIL-R1"))
                .count(),
            2,
            "both DTM errors from Rule A should be present"
        );
        // Bail fired after Rule A: Rule B (BGM) must be skipped.
        assert_eq!(
            report
                .errors()
                .iter()
                .filter(|i| i.rule_id.as_deref() == Some("BAIL-R2"))
                .count(),
            0,
            "Rule B should have been skipped by bail"
        );
    }

    #[test]
    fn bail_disabled_runs_all_rules() {
        let input = b"UNH+1+ORDERS:D:96A:UN'BGM+220+9'DTM+137:20240101:102'UNT+4+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse failed");

        let pack_no_bail = two_dtm_errors_rule(); // bail_on_first_error defaults to false
        let ctx = ValidationContext::builder()
            .with_profile_pack(pack_no_bail)
            .build();
        let report = ctx.validate_lenient(&segments);

        // Both rules run: one DTM error from Rule A, one BGM error from Rule B.
        assert_eq!(
            report
                .errors()
                .iter()
                .filter(|i| i.rule_id.as_deref() == Some("BAIL-R1"))
                .count(),
            1
        );
        assert_eq!(
            report
                .errors()
                .iter()
                .filter(|i| i.rule_id.as_deref() == Some("BAIL-R2"))
                .count(),
            1
        );
    }

    // ── message_ref in ValidationRuleContext ─────────────────────────────────

    #[test]
    fn message_ref_is_visible_inside_rule_closure() {
        let input = b"UNH+MSG001+ORDERS:D:96A:UN'BGM+220+9'UNT+3+1'";
        let segments = crate::from_bytes(input)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse failed");

        let pack = ProfileRulePack::new("MSG-REF-TEST").with_rule_fn(|_segs, ctx, issues| {
            if let Some(mref) = ctx.message_ref {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Info,
                        format!("validating message {mref}"),
                    )
                    .with_rule_id("CTX-REF"),
                );
            }
        });

        let ctx = ValidationContext::builder()
            .with_profile_pack(pack)
            .with_message_ref("MSG001")
            .build();

        let report = ctx.validate_lenient(&segments);
        let info = report
            .infos()
            .iter()
            .find(|i| i.rule_id.as_deref() == Some("CTX-REF"))
            .expect("expected info issue from CTX-REF rule");
        assert!(info.message.contains("MSG001"));
        // The message_ref is also stamped onto the issue itself.
        assert_eq!(info.message_ref.as_deref(), Some("MSG001"));
    }
}
