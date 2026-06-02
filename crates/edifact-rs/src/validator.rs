//! Validation pipeline for structural and semantic EDIFACT checks.

use crate::{EdifactError, Segment, ValidationIssue, ValidationReport, ValidationSeverity};
use std::any::Any;
use std::sync::Arc;

/// Typed context injected into profile rule closures at validation time.
///
/// Rules access per-call metadata via [`ValidationRuleContext::metadata`].
/// If no metadata was injected, every `metadata()` call returns `None`.
///
/// # Example
///
/// ```rust,ignore
/// let pack = ProfileRulePack::new("AHB-11001")
///     .with_rule_fn(|segs, ctx| {
///         let pruefid: &Pruefid = ctx.metadata()?;
///         // use pruefid …
///         None
///     });
///
/// let report = ValidationContext::builder()
///     .with_profile_pack(pack)
///     .build()
///     .validate_lenient_with(&segments, &my_pruefid);
/// ```
#[derive(Clone, Copy)]
pub struct ValidationRuleContext<'a> {
    metadata: Option<&'a (dyn Any + Send + Sync)>,
}

impl<'a> ValidationRuleContext<'a> {
    /// Construct a context with no metadata.
    pub fn empty() -> Self {
        Self { metadata: None }
    }

    /// Construct a context holding a typed metadata reference.
    pub fn new<T: Any + Send + Sync>(value: &'a T) -> Self {
        Self {
            metadata: Some(value as &(dyn Any + Send + Sync)),
        }
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
            .finish()
    }
}

/// A profile rule that can be added to a [`ProfileRulePack`].
///
/// Implement this trait to create reusable, composable profile rules for
/// EDIFACT message validation.  Rules receive a [`ValidationRuleContext`] that
/// provides optional typed metadata injected at validation call time via
/// [`ValidationContext::validate_lenient_with`].
pub trait ProfileRule: Send + Sync {
    /// Evaluate the rule against the given segments.
    ///
    /// Return `Some(issue)` if the rule is violated, or `None` if the segments pass.
    fn evaluate(
        &self,
        segments: &[Segment<'_>],
        context: &ValidationRuleContext<'_>,
    ) -> Option<ValidationIssue>;
}

/// Wraps a context-aware closure as a [`ProfileRule`].
struct ClosureProfileRule<F>(F);

impl<F> ProfileRule for ClosureProfileRule<F>
where
    F: for<'a> Fn(&[Segment<'a>], &ValidationRuleContext<'_>) -> Option<ValidationIssue>
        + Send
        + Sync,
{
    fn evaluate(
        &self,
        segments: &[Segment<'_>],
        context: &ValidationRuleContext<'_>,
    ) -> Option<ValidationIssue> {
        (self.0)(segments, context)
    }
}

/// Wraps a context-free closure as a [`ProfileRule`] (ignores the context parameter).
struct StatelessClosureProfileRule<F>(F);

impl<F> ProfileRule for StatelessClosureProfileRule<F>
where
    F: for<'a> Fn(&[Segment<'a>]) -> Option<ValidationIssue> + Send + Sync,
{
    fn evaluate(
        &self,
        segments: &[Segment<'_>],
        _context: &ValidationRuleContext<'_>,
    ) -> Option<ValidationIssue> {
        (self.0)(segments)
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
    message_types: Vec<String>,
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
            message_types: Vec::new(),
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
    pub fn message_types(&self) -> &[String] {
        &self.message_types
    }

    /// Return the number of rules in this pack.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
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
        let message_type = message_type.into();
        if !self.message_types.contains(&message_type) {
            self.message_types.push(message_type);
        }
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
    /// The closure receives both the segment slice and a [`ValidationRuleContext`]
    /// that may carry typed metadata injected at validation call time via
    /// [`ValidationContext::validate_lenient_with`].
    ///
    /// For rules that do not need context, use [`with_stateless_rule_fn`][Self::with_stateless_rule_fn].
    pub fn with_rule_fn<F>(mut self, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>], &ValidationRuleContext<'_>) -> Option<ValidationIssue>
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
        F: for<'a> Fn(&[Segment<'a>], &ValidationRuleContext<'_>) -> Option<ValidationIssue>
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
    /// Convenience wrapper for rules that do not inspect the
    /// [`ValidationRuleContext`].
    pub fn with_stateless_rule_fn<F>(mut self, rule: F) -> Self
    where
        F: for<'a> Fn(&[Segment<'a>]) -> Option<ValidationIssue> + Send + Sync + 'static,
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
        F: for<'a> Fn(&[Segment<'a>]) -> Option<ValidationIssue> + Send + Sync + 'static,
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
    /// # Example
    ///
    /// ```rust,ignore
    /// let base = ProfileRulePack::new("MIG-UTILMD-BASE")
    ///     .with_stateless_rule_fn(/* mandatory segment rules */);
    ///
    /// let ahb_11001 = ProfileRulePack::new("AHB-11001")
    ///     .extend_from(&base)
    ///     .with_stateless_rule_fn(/* 11001-specific rules */);
    /// ```
    pub fn extend_from(mut self, base: &ProfileRulePack) -> Result<Self, EdifactError> {
        let mut combined = base.rules.clone();
        combined.append(&mut self.rules);
        self.rules = combined;
        for mt in &base.message_types {
            if !self.message_types.contains(mt) {
                self.message_types.push(mt.clone());
            }
        }
        self.release = merge_release_scopes(self.release.take(), base.release.clone())?;
        Ok(self)
    }

    /// Merge two packs into one combined pack.
    ///
    /// Rules from `self` run before rules from `other`.  If both packs contain
    /// named rules with the same id, **both run** — use
    /// [`merge_with_override`][Self::merge_with_override] to de-duplicate by id instead.
    /// Release scoping follows the same compatibility rule as
    /// [`extend_from`][Self::extend_from].
    pub fn merge(mut self, mut other: Self) -> Result<Self, EdifactError> {
        for message_type in other.message_types.drain(..) {
            if !self.message_types.contains(&message_type) {
                self.message_types.push(message_type);
            }
        }
        self.release = merge_release_scopes(self.release.take(), other.release.take())?;
        self.rules.append(&mut other.rules);
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
    /// Message-type restrictions from `other` are merged into `self`.
    /// Release scoping follows the same compatibility rule as
    /// [`extend_from`][Self::extend_from].
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let base = ProfileRulePack::new("UTILMD-5.4")
    ///     .with_named_stateless_rule_fn("AHB-11001-BGM-M", |segs| { /* old rule */ None });
    ///
    /// let delta = ProfileRulePack::new("UTILMD-5.5-delta")
    ///     .with_named_stateless_rule_fn("AHB-11001-BGM-M", |segs| { /* updated rule */ None });
    ///
    /// // `result` runs the updated BGM-M rule only once:
    /// let result = base.merge_with_override(delta);
    /// assert_eq!(result.rule_count(), 1);
    /// ```
    pub fn merge_with_override(mut self, mut other: Self) -> Result<Self, EdifactError> {
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

        for message_type in other.message_types.drain(..) {
            if !self.message_types.contains(&message_type) {
                self.message_types.push(message_type);
            }
        }
        self.release = merge_release_scopes(self.release.take(), other.release.take())?;
        Ok(self)
    }
}

fn merge_release_scopes(
    current: Option<String>,
    incoming: Option<String>,
) -> Result<Option<String>, EdifactError> {
    match (current, incoming) {
        (Some(current), Some(incoming)) => {
            // Both packs specify a release; they must match to compose safely.
            if current != incoming {
                return Err(EdifactError::IncompatibleReleaseScopes { current, incoming });
            }
            Ok(Some(current))
        }
        (current @ Some(_), None) => Ok(current),
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

        // Message-type filter: skip if no registered type matches.
        let message_type = unh
            .and_then(|s| s.get_element(1))
            .and_then(|e| e.get_component(0));
        if !self.message_types.is_empty()
            && !message_type.is_some_and(|mt| self.message_types.iter().any(|t| t == mt))
        {
            return;
        }

        // Release filter: skip if pack is bound to a specific association code that
        // does not match the message's UNH DE 0057 (element 1, component 4).
        if let Some(bound_release) = &self.release {
            let msg_association = unh
                .and_then(|s| s.get_element(1))
                .and_then(|e| e.get_component(4));
            if msg_association != Some(bound_release.as_str()) {
                return;
            }
        }

        for named in &self.rules {
            if let Some(issue) = named.rule.evaluate(segments, context) {
                let was_error = match issue.severity {
                    ValidationSeverity::Critical | ValidationSeverity::Error => {
                        report.add_error(issue);
                        true
                    }
                    ValidationSeverity::Warning => {
                        report.add_warning(issue);
                        false
                    }
                    ValidationSeverity::Info => {
                        report.add_info(issue);
                        false
                    }
                };
                if self.bail_on_first_error && was_error {
                    return;
                }
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
    structure_enabled: bool,
    code_list_enabled: bool,
    profile_enabled: bool,
    message_type: Option<String>,
    metadata: Option<Arc<dyn Any + Send + Sync>>,
}

/// Builder for [`ValidationContext`].
#[must_use = "call `.build()` to produce a `ValidationContext`"]
pub struct ValidationContextBuilder {
    inner: ValidationContext,
}

impl Default for ValidationContextBuilder {
    /// Default context builder has all layers enabled, same as [`ValidationContextBuilder::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationContextBuilder {
    /// Create a new context builder with all layers enabled.
    pub fn new() -> Self {
        Self {
            inner: ValidationContext {
                validators: Vec::new(),
                structure_enabled: true,
                code_list_enabled: true,
                profile_enabled: true,
                message_type: None,
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
        let ctx = self
            .metadata
            .as_ref()
            .map(|arc| ValidationRuleContext {
                metadata: Some(arc.as_ref() as &(dyn Any + Send + Sync)),
            })
            .unwrap_or_else(ValidationRuleContext::empty);
        self.validate_with_context(segments, &ctx)
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
        let ctx = ValidationRuleContext::new(value);
        self.validate_with_context(segments, &ctx)
    }

    /// Execute validators in strict mode for enabled layers.
    ///
    /// Returns `Ok(report)` when validation produces no errors.  The returned
    /// report may still contain warnings and infos — warnings do **not** cause
    /// this method to return `Err`.  Call [`validate_lenient`][Self::validate_lenient]
    /// if you want to inspect warnings without failing on errors.
    pub fn validate_strict(
        &self,
        segments: &[Segment<'_>],
    ) -> Result<ValidationReport, EdifactError> {
        let report = self.validate_lenient(segments);
        Self::strict_check(report)
    }

    /// Execute validators in strict mode with per-call typed metadata.
    ///
    /// See [`validate_lenient_with`][Self::validate_lenient_with] for context usage and
    /// [`validate_strict`][Self::validate_strict] for strict-mode semantics.
    pub fn validate_strict_with<T: Any + Send + Sync>(
        &self,
        segments: &[Segment<'_>],
        value: &T,
    ) -> Result<ValidationReport, EdifactError> {
        let report = self.validate_lenient_with(segments, value);
        Self::strict_check(report)
    }

    fn validate_with_context(
        &self,
        segments: &[Segment<'_>],
        context: &ValidationRuleContext<'_>,
    ) -> ValidationReport {
        let mut report = ValidationReport::default();
        for lv in &self.validators {
            if self.layer_enabled(lv.layer) {
                lv.validator.validate_batch(segments, &mut report, context);
            }
        }
        report
    }

    fn strict_check(report: ValidationReport) -> Result<ValidationReport, EdifactError> {
        if report.has_errors() {
            let first_message = report
                .errors()
                .first()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "unknown validation failure".to_owned());
            return Err(EdifactError::ValidationFailed {
                error_count: report.errors().len(),
                first_message,
            });
        }
        Ok(report)
    }

    /// Message type metadata associated with this context, if provided.
    pub fn message_type(&self) -> Option<&str> {
        self.message_type.as_deref()
    }

    fn layer_enabled(&self, layer: ValidationLayer) -> bool {
        match layer {
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
        | EdifactError::UnexpectedEof { offset } => {
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
            .with_stateless_rule_fn(|segments| {
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
            })
            .with_stateless_rule_fn(|segments| {
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
        assert!(matches!(result, Err(EdifactError::ValidationFailed { .. })));
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
        assert!(matches!(result, Err(EdifactError::ValidationFailed { .. })));
    }
}
