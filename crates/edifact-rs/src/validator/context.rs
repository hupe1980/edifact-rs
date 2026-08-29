//! Validation context: `ValidationContext`, `ValidationContextBuilder`, `LayeredValidator`.

use super::pack::ProfileRulePack;
use super::{
    CharsetValidator, EnvelopeValidator, ValidationLayer, ValidationRuleContext, Validator,
};
use crate::{Segment, ValidationReport, ValidationSeverity};
use std::any::Any;
use std::sync::Arc;

pub(super) struct LayeredValidator {
    pub(super) layer: ValidationLayer,
    pub(super) validator: Box<dyn Validator + Send + Sync>,
}

/// Runs the four validation layers over one segment slice, collecting every
/// issue into a single [`ValidationReport`].
///
/// | Layer | [`ValidationLayer`] | Default | Checks | Provided by |
/// |---|---|---|---|---|
/// | Envelope | `Envelope` | off | `UNB`/`UNH`/`UNT`/`UNZ` structure and counts | [`EnvelopeValidator`] |
/// | Structure | `Structure` | on | segment presence, order, arity | `DirectoryValidator` |
/// | Code-list | `CodeList` | on | DE values against the directory's code lists | `DirectoryValidator` |
/// | Profile | `Profile` | on | partner and industry business rules | [`ProfileRulePack`] |
///
/// Validators run in registration order within a layer; layers have no ordering
/// beyond that. With the envelope layer on, `UNB`/`UNZ`/`UNG`/`UNE` are excluded
/// from the slice later layers see.
///
/// A pack can be scoped by message type (`for_message_type`) and
/// association-assigned code (`for_release`).
///
/// # Group-aware validation
///
/// [`validate_grouped`][Self::validate_grouped] runs the flat pass and then a
/// group pass over a [`SegmentGroupIndexed`][crate::SegmentGroupIndexed] tree,
/// so rules scoped to a segment group — "DTM must appear in every SG5" — can
/// fire.
///
/// # Example — building a context
///
/// ```rust,ignore
/// use std::sync::{Arc, LazyLock};
/// use edifact_rs::{ProfileRulePack, ValidationContext, ValidationLayer};
///
/// static ORDERS_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
///     Arc::new(
///         ProfileRulePack::new("ORDERS-MIG-5.5")
///             .for_message_type("ORDERS")
///             .require_segment("BGM", "MIG-BGM-M")
///             .require_segment_in_group("SG2", "NAD", "SG2-NAD-M"),
///     )
/// });
///
/// let ctx = ValidationContext::builder()
///     .with_envelope_validation()
///     .with_profile_pack_arc(Arc::clone(&*ORDERS_PACK))
///     .build();
///
/// let report = ctx.validate(&segments);
/// ```
pub struct ValidationContext {
    pub(super) validators: Vec<LayeredValidator>,
    pub(super) envelope_enabled: bool,
    pub(super) structure_enabled: bool,
    pub(super) code_list_enabled: bool,
    pub(super) profile_enabled: bool,
    /// Stop evaluating all remaining validators as soon as a `Critical`-severity
    /// issue appears in the report.
    pub(super) bail_on_first_critical: bool,
    pub(super) message_type: Option<String>,
    /// Injected into every emitted `ValidationIssue` when set.
    pub(super) message_ref: Option<String>,
    pub(super) metadata: Option<Arc<dyn Any + Send + Sync>>,
    /// Advisory issues unconditionally appended to every report produced by
    /// this context — regardless of what segments are validated.
    ///
    /// Use [`ValidationContextBuilder::with_static_issue`] to populate.
    pub(super) static_issues: Vec<crate::ValidationIssue>,
}

/// Builder for [`ValidationContext`].
#[must_use = "call `.build()` to produce a `ValidationContext`"]
pub struct ValidationContextBuilder {
    pub(super) inner: ValidationContext,
}

impl Default for ValidationContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationContextBuilder {
    /// Create a new context builder.
    ///
    /// Structure, code-list, and profile layers are enabled by default.
    /// The envelope layer is **disabled** by default.
    pub fn new() -> Self {
        Self {
            inner: ValidationContext {
                validators: Vec::new(),
                envelope_enabled: false,
                structure_enabled: true,
                code_list_enabled: true,
                profile_enabled: true,
                bail_on_first_critical: false,
                message_type: None,
                message_ref: None,
                metadata: None,
                static_issues: Vec::new(),
            },
        }
    }

    /// Attach typed metadata accessible to context-aware profile rules.
    pub fn with_metadata<T: Any + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.inner.metadata = Some(Arc::new(value));
        self
    }

    /// Stamp every issue produced by this context with the given message reference.
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

    /// Stop validating once a `Critical`-severity issue has appeared.
    ///
    /// The check happens **between validators**, not between issues: the
    /// validator that raised the `Critical` still finishes and contributes
    /// everything it found, and every validator after it — in any layer — is
    /// skipped. Bailing mid-validator would mean a report whose contents depend
    /// on the order rules happen to run in.
    ///
    /// Default: `false` (run every enabled layer and collect all issues).
    pub fn bail_on_first_critical(mut self, bail: bool) -> Self {
        self.inner.bail_on_first_critical = bail;
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
    pub fn with_envelope_validation(mut self) -> Self {
        self.inner.envelope_enabled = true;
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Envelope,
            validator: Box::new(EnvelopeValidator),
        });
        self
    }

    /// Check every value against the repertoire the interchange declares, and
    /// enable the envelope layer.
    ///
    /// Adds a [`CharsetValidator`] reading `UNB` S001 DE 0001. See
    /// [`with_charset_validation_for`][Self::with_charset_validation_for] to pin
    /// a repertoire instead of reading it from the envelope.
    pub fn with_charset_validation(mut self) -> Self {
        self.inner.envelope_enabled = true;
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Envelope,
            validator: Box::new(CharsetValidator::from_envelope()),
        });
        self
    }

    /// Check every value against a fixed repertoire, and enable the envelope layer.
    ///
    /// Use for message-level slices that carry no `UNB`, or to hold a partner to
    /// a stricter repertoire than the one they declare.
    pub fn with_charset_validation_for(mut self, charset: crate::Charset) -> Self {
        self.inner.envelope_enabled = true;
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Envelope,
            validator: Box::new(CharsetValidator::with_charset(charset)),
        });
        self
    }

    /// Check the directory-independent ISO 9735-1 syntax rules, and enable the
    /// envelope layer.
    ///
    /// See [`SyntaxValidator`][crate::SyntaxValidator] for the exact rules. They
    /// apply to any interchange from any partner in any directory, so this needs
    /// no configuration and is worth enabling wherever the envelope layer is on.
    pub fn with_syntax_validation(mut self) -> Self {
        self.inner.envelope_enabled = true;
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Envelope,
            validator: Box::new(crate::validator::SyntaxValidator),
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
    ///
    /// The pack's own message-type scoping — set with
    /// [`ProfileRulePack::for_message_type`] — is what decides whether its rules
    /// run.  The context's message type does not narrow an unscoped pack.
    pub fn with_profile_pack(self, pack: ProfileRulePack) -> Self {
        self.with_profile_pack_inner(pack)
    }

    fn with_profile_pack_inner(mut self, pack: ProfileRulePack) -> Self {
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Profile,
            validator: Box::new(pack),
        });
        self
    }

    /// Add a reference-counted profile rule pack to the profile layer.
    ///
    /// Unlike [`with_profile_pack`](Self::with_profile_pack), this method stores the pack
    /// behind an [`Arc`] so context forking (via
    /// [`ValidationContext::fork_with_message_ref`]) only increments the reference count
    /// instead of deep-cloning the rule vec.
    ///
    /// This is the preferred API for downstream code that caches packs in static
    /// storage (`LazyLock`, `OnceLock`) and reuses them across many validation calls.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::sync::{Arc, LazyLock};
    /// use edifact_rs::{ProfileRulePack, ValidationContext};
    ///
    /// static PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    ///     Arc::new(ProfileRulePack::new("MIG").require_segment("BGM", "MIG-BGM-M"))
    /// });
    ///
    /// let ctx = ValidationContext::builder()
    ///     .with_profile_pack_arc(Arc::clone(&*PACK))
    ///     .build();
    /// ```
    pub fn with_profile_pack_arc(mut self, pack: std::sync::Arc<ProfileRulePack>) -> Self {
        self.inner.validators.push(LayeredValidator {
            layer: ValidationLayer::Profile,
            validator: Box::new(pack),
        });
        self
    }

    /// Unconditionally append `issue` to every report produced by this context.
    ///
    /// Static issues are emitted on every `validate_*` call — they are not
    /// evaluated against segments.  This is useful for advisory notices that
    /// should always be present regardless of message content (e.g. "the profile layer
    /// is inactive for this message type").
    pub fn with_static_issue(mut self, issue: crate::ValidationIssue) -> Self {
        self.inner.static_issues.push(issue);
        self
    }

    /// Finalize builder and create context.
    #[must_use = "call `.validate()` on the resulting context"]
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
    pub fn validate(&self, segments: &[Segment<'_>]) -> ValidationReport {
        self.validate_with_context(segments, &self.build_rule_context())
    }

    /// Execute flat + group-aware validators in lenient mode.
    ///
    /// This method runs the full flat validation pass (same as
    /// [`validate_lenient`](Self::validate)) **and** then runs the
    /// group-aware pass by calling [`Validator::validate_group_batch`] on every
    /// validator.  Validators without group rules treat `validate_group_batch`
    /// as a no-op, so this is safe to call for any context.
    ///
    /// # When to use
    ///
    /// Use this method when you have already grouped your segments with
    /// [`group_segments_indexed`][crate::group_segments_indexed] or
    /// [`group_segments_indexed`][crate::group_segments_indexed] and
    /// want group-presence or cross-group rules (via
    /// [`ProfileRulePack::with_scoped_group_rule_fn`][crate::ProfileRulePack::with_scoped_group_rule_fn])
    /// to fire.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use edifact_rs::{group_segments_indexed, ValidationContext};
    /// use edifact_rs::group::GroupDef;
    ///
    /// static SCHEMA: &[GroupDef] = &[GroupDef::new("SG5", "LOC")];
    ///
    /// let tree = group_segments_indexed(&segments, SCHEMA, "ROOT");
    /// let pack = ProfileRulePack::new("PROFILE")
    ///     .require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    /// let ctx = ValidationContext::builder().with_profile_pack(pack).build();
    ///
    /// let report = ctx.validate_grouped(&tree, &segments);
    /// ```
    pub fn validate_grouped(
        &self,
        root: &crate::group::SegmentGroupIndexed<'_>,
        segments: &[Segment<'_>],
    ) -> ValidationReport {
        let base_ctx = self.build_rule_context();
        // Phase 1: flat validation.
        let mut report = self.validate_with_context(segments, &base_ctx);
        // Phase 2: group-aware validation with pre-extracted UNH message type.
        let unh_mt = segments
            .iter()
            .find(|s| s.tag == "UNH")
            .and_then(|s| s.get_element(1))
            .and_then(|e| e.get_component(0));
        let ctx_with_type;
        let group_ctx: &ValidationRuleContext<'_> = if let Some(mt) = unh_mt {
            ctx_with_type = ValidationRuleContext {
                metadata: base_ctx.metadata,
                message_ref: base_ctx.message_ref,
                message_type: Some(mt),
            };
            &ctx_with_type
        } else {
            &base_ctx
        };
        self.run_group_pass(root, segments, &mut report, group_ctx);
        report
    }

    /// Phase-2 group pass: call `validate_group_batch` on each enabled validator.
    fn run_group_pass(
        &self,
        root: &crate::group::SegmentGroupIndexed<'_>,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
    ) {
        // Short-circuit: skip the entire DFS tree walk when no enabled validator
        // has group rules.  This avoids the O(n) borrowed-segment allocation in
        // `validate_lenient_grouped_owned` for the common case where the context
        // only has flat (envelope/structure/code-list) validators.
        if !self
            .validators
            .iter()
            .any(|lv| self.layer_enabled(lv.layer) && lv.validator.has_group_rules())
        {
            return;
        }
        for lv in &self.validators {
            if !self.layer_enabled(lv.layer) {
                continue;
            }
            lv.validator
                .validate_group_batch(root, segments, report, context);
            if self.bail_on_first_critical && report.has_critical_errors() {
                break;
            }
        }
    }

    /// Execute validators with per-call typed metadata.
    ///
    /// `message_type` is set to `None` here; the concrete validation method
    /// (`validate_with_context`) re-extracts the message type from the `UNH`
    /// segment, so there is no information loss.
    pub fn validate_with<T: Any + Send + Sync>(
        &self,
        segments: &[Segment<'_>],
        value: &T,
    ) -> ValidationReport {
        let ctx = ValidationRuleContext {
            metadata: Some(value as &(dyn Any + Send + Sync)),
            message_ref: self.message_ref.as_deref(),
            message_type: None,
        };
        self.validate_with_context(segments, &ctx)
    }

    fn build_rule_context(&self) -> ValidationRuleContext<'_> {
        self.metadata
            .as_ref()
            .map(|arc| ValidationRuleContext {
                metadata: Some(arc.as_ref() as &(dyn Any + Send + Sync)),
                message_ref: self.message_ref.as_deref(),
                message_type: None,
            })
            .unwrap_or_else(|| ValidationRuleContext {
                metadata: None,
                message_ref: self.message_ref.as_deref(),
                message_type: None,
            })
    }

    fn validate_with_context(
        &self,
        segments: &[Segment<'_>],
        context: &ValidationRuleContext<'_>,
    ) -> ValidationReport {
        let mut report = ValidationReport::default();
        // Pre-extract UNH message type once (F-017).
        let unh_message_type = segments
            .iter()
            .find(|s| s.tag == "UNH")
            .and_then(|s| s.get_element(1))
            .and_then(|e| e.get_component(0));
        let ctx_with_type;
        let effective_ctx: &ValidationRuleContext<'_> = if let Some(mt) = unh_message_type {
            ctx_with_type = ValidationRuleContext {
                metadata: context.metadata,
                message_ref: context.message_ref,
                message_type: Some(mt),
            };
            &ctx_with_type
        } else {
            context
        };
        let mut filtered: Option<Vec<Segment<'_>>> = None;
        // See the owned path: computed once so filtering is independent of the
        // order in which validators were registered.
        let envelope_active = self.envelope_layer_active();

        for lv in &self.validators {
            if !self.layer_enabled(lv.layer) {
                continue;
            }
            if lv.layer == ValidationLayer::Envelope {
                lv.validator
                    .validate_batch(segments, &mut report, effective_ctx);
            } else {
                let active: &[Segment<'_>] = if envelope_active {
                    match envelope_interior(segments) {
                        // Common case: UNB/UNZ bracket the message and no
                        // UNG/UNE appear inside, so a sub-slice suffices and no
                        // segment has to be deep-cloned.
                        Some(interior) => interior,
                        None => filtered.get_or_insert_with(|| {
                            segments
                                .iter()
                                .filter(|s| !matches!(s.tag(), "UNB" | "UNZ" | "UNG" | "UNE"))
                                .cloned()
                                .collect()
                        }),
                    }
                } else {
                    segments
                };
                lv.validator
                    .validate_batch(active, &mut report, effective_ctx);
            }
            if self.bail_on_first_critical && report.has_critical_errors() {
                break;
            }
        }

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
        // Append static advisory issues unconditionally.
        for issue in &self.static_issues {
            match issue.severity {
                ValidationSeverity::Critical | ValidationSeverity::Error => {
                    report.add_error(issue.clone());
                }
                ValidationSeverity::Warning => {
                    report.warnings.push(issue.clone());
                }
                ValidationSeverity::Info => {
                    report.infos.push(issue.clone());
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

    /// Create a child context that inherits all rules and configuration from `self`
    /// but is scoped to a specific message reference (UNH DE 0062).
    ///
    /// Issues produced by the child context are automatically stamped with
    /// `message_ref`, making it easy to correlate findings in a multi-message
    /// interchange back to the originating `UNH`/`UNT` envelope.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let base_ctx = ValidationContext::builder()
    ///     .with_profile_pack(mig_pack)
    ///     .build();
    ///
    /// for (ref_no, message_segments) in messages {
    ///     let child = base_ctx.fork_with_message_ref(&ref_no);
    ///     let report = child.validate(&message_segments);
    /// }
    /// ```
    pub fn fork_with_message_ref(&self, message_ref: impl Into<String>) -> Self {
        let validators: Vec<LayeredValidator> = self
            .validators
            .iter()
            .filter_map(|lv| {
                lv.validator.fork().map(|forked| LayeredValidator {
                    layer: lv.layer,
                    validator: forked,
                })
            })
            .collect();
        // Count how many validators were excluded (non-forkable).
        let excluded_count = self.validators.len() - validators.len();

        let mut static_issues = self.static_issues.clone();
        if excluded_count > 0 {
            static_issues.push(
                crate::ValidationIssue::new(
                    crate::ValidationSeverity::Info,
                    format!(
                        "{excluded_count} validator(s) excluded from forked context \
                         because fork() returned None; all their rules (flat and \
                         group-pass) will not run for this message",
                    ),
                )
                .with_rule_id("edifact-rs::fork::excluded-validator"),
            );
        }

        Self {
            validators,
            envelope_enabled: self.envelope_enabled,
            structure_enabled: self.structure_enabled,
            code_list_enabled: self.code_list_enabled,
            profile_enabled: self.profile_enabled,
            bail_on_first_critical: self.bail_on_first_critical,
            message_type: self.message_type.clone(),
            message_ref: Some(message_ref.into()),
            metadata: self.metadata.clone(),
            static_issues,
        }
    }

    fn layer_enabled(&self, layer: ValidationLayer) -> bool {
        match layer {
            ValidationLayer::Envelope => self.envelope_enabled,
            ValidationLayer::Structure => self.structure_enabled,
            ValidationLayer::CodeList => self.code_list_enabled,
            ValidationLayer::Profile => self.profile_enabled,
        }
    }

    /// Whether an enabled envelope-layer validator is registered.
    ///
    /// Determines whether envelope segments are hidden from later layers.  It is
    /// a property of the context as a whole, not of how far the validator loop
    /// has progressed.
    fn envelope_layer_active(&self) -> bool {
        self.envelope_enabled
            && self
                .validators
                .iter()
                .any(|lv| lv.layer == ValidationLayer::Envelope)
    }
}

/// Return the message body as a sub-slice when the envelope segments form a
/// clean `UNB` … `UNZ` bracket with no functional groups inside.
///
/// Returns `None` when the caller must fall back to filter-and-clone (functional
/// groups present, or the interchange is not bracketed as expected).
fn envelope_interior<'s, 'a>(segments: &'s [Segment<'a>]) -> Option<&'s [Segment<'a>]> {
    let (first, last) = (segments.first()?, segments.last()?);
    if segments.len() < 2 || first.tag != "UNB" || last.tag != "UNZ" {
        return None;
    }
    let interior = &segments[1..segments.len() - 1];
    if interior
        .iter()
        .any(|s| matches!(s.tag(), "UNB" | "UNZ" | "UNG" | "UNE"))
    {
        return None;
    }
    Some(interior)
}
