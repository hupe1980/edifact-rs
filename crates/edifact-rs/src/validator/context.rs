//! Validation context: `ValidationContext`, `ValidationContextBuilder`, `LayeredValidator`.

use super::pack::ProfileRulePack;
use super::{EnvelopeValidator, ValidationLayer, ValidationRuleContext, Validator};
use crate::{OwnedSegment, Segment, ValidationReport, ValidationSeverity};
use std::any::Any;
use std::sync::Arc;

pub(super) struct LayeredValidator {
    pub(super) layer: ValidationLayer,
    pub(super) validator: Box<dyn Validator + Send + Sync>,
}

/// Runtime validation context for progressive layered validation.
///
/// # Architecture
///
/// `edifact-rs` validation is organized into **four independent layers**, each
/// responsible for a distinct class of checks.  All layers run against the same
/// segment slice; their issues are collected into a single [`ValidationReport`].
///
/// | Layer | [`ValidationLayer`] variant | Default | Type |
/// |---|---|---|---|
/// | **Envelope** | `Envelope` | disabled | [`EnvelopeValidator`] |
/// | **Structure** | `Structure` | enabled | external (e.g. `DirectoryValidator`) |
/// | **Code-list** | `CodeList` | enabled | external |
/// | **Profile** | `Profile` | enabled | [`ProfileRulePack`] / `Arc<ProfileRulePack>` |
///
/// Validators are run in registration order within each enabled layer.  Layers
/// themselves have no enforced ordering beyond the order in which they are added
/// via the builder.
///
/// ## Envelope layer
///
/// Checks `UNB`/`UNH`/`UNT`/`UNZ` structural invariants: presence, message
/// count, and segment count.  Enabled by calling
/// [`ValidationContextBuilder::with_envelope_validation`].  When enabled, the
/// envelope segments (`UNB`, `UNZ`, `UNG`, `UNE`) are *excluded* from the slice
/// passed to validators in subsequent layers.
///
/// ## Structure layer
///
/// Validates segment presence, order, and arity against an EDIFACT directory.
/// Implemented by `DirectoryValidator` (registered as a `Structure`-layer
/// validator via [`ValidationContextBuilder::with_validator`]).
///
/// ## Code-list layer
///
/// Validates DE values against EDIFACT code lists from the directory.  Also
/// implemented by `DirectoryValidator`.
///
/// ## Profile layer
///
/// Applies downstream business rules (BDEW AHB / MIG rules, custom constraints)
/// via [`ProfileRulePack`].  A pack can be scoped to specific EDIFACT message
/// types (`for_message_type`) and association-assigned codes (`for_release`).
///
/// ## Group-aware validation
///
/// Validators that implement [`Validator::validate_group_batch`] can additionally
/// enforce rules scoped to specific segment groups (e.g. "DTM must appear in every
/// SG5 occurrence").  Call [`validate_lenient_grouped`] with a pre-built
/// [`SegmentGroupIndexed`] tree to activate both the flat and group passes.
///
/// [`SegmentGroupIndexed`]: crate::SegmentGroupIndexed
/// [`validate_lenient_grouped`]: ValidationContext::validate_lenient_grouped
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
/// let report = ctx.validate_lenient(&segments);
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

    /// Stop all validation as soon as the first `Critical`-severity issue is produced.
    ///
    /// When set, [`ValidationContext::validate_lenient`] returns immediately after the
    /// first `Critical` issue from any validator, skipping all remaining packs and layers.
    ///
    /// Default: `false` (collect all issues across all layers).
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
    pub fn validate_lenient(&self, segments: &[Segment<'_>]) -> ValidationReport {
        self.validate_with_context(segments, &self.build_rule_context())
    }

    /// Execute flat + group-aware validators in lenient mode.
    ///
    /// This method runs the full flat validation pass (same as
    /// [`validate_lenient`](Self::validate_lenient)) **and** then runs the
    /// group-aware pass by calling [`Validator::validate_group_batch`] on every
    /// validator.  Validators without group rules treat `validate_group_batch`
    /// as a no-op, so this is safe to call for any context.
    ///
    /// # When to use
    ///
    /// Use this method when you have already grouped your segments with
    /// [`group_segments_indexed`][crate::group_segments_indexed] or
    /// [`group_owned_segments_indexed`][crate::group_owned_segments_indexed] and
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
    /// static SCHEMA: &[GroupDef] = &[
    ///     GroupDef { name: "SG5", trigger: "LOC", children: &[] },
    /// ];
    ///
    /// let tree = group_segments_indexed(&segments, SCHEMA, "ROOT");
    /// let pack = ProfileRulePack::new("AHB")
    ///     .require_segment_in_group("SG5", "DTM", "SG5-DTM-M");
    /// let ctx = ValidationContext::builder().with_profile_pack(pack).build();
    ///
    /// let report = ctx.validate_lenient_grouped(&tree, &segments);
    /// ```
    pub fn validate_lenient_grouped(
        &self,
        root: &crate::group::SegmentGroupIndexed,
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

    /// Execute flat + group-aware validators in strict mode.
    pub fn validate_strict_grouped(
        &self,
        root: &crate::group::SegmentGroupIndexed,
        segments: &[Segment<'_>],
    ) -> Result<ValidationReport, ValidationReport> {
        self.validate_lenient_grouped(root, segments).result()
    }

    /// Execute flat + group-aware validators against owned segments in lenient mode.
    pub fn validate_lenient_grouped_owned(
        &self,
        root: &crate::group::SegmentGroupIndexed,
        segments: &[crate::OwnedSegment],
    ) -> ValidationReport {
        let base_ctx = self.build_rule_context();
        // Phase 1: flat validation.
        let mut report = self.validate_with_context_owned(segments, &base_ctx);
        // Phase 2: group-aware validation — borrow owned segments.
        let borrowed: Vec<Segment<'_>> = segments.iter().map(|s| s.as_borrowed()).collect();
        let unh_mt = borrowed
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
        self.run_group_pass(root, &borrowed, &mut report, group_ctx);
        report
    }

    /// Execute flat + group-aware validators against owned segments in strict mode.
    pub fn validate_strict_grouped_owned(
        &self,
        root: &crate::group::SegmentGroupIndexed,
        segments: &[crate::OwnedSegment],
    ) -> Result<ValidationReport, ValidationReport> {
        self.validate_lenient_grouped_owned(root, segments).result()
    }

    /// Phase-2 group pass: call `validate_group_batch` on each enabled validator.
    fn run_group_pass(
        &self,
        root: &crate::group::SegmentGroupIndexed,
        segments: &[Segment<'_>],
        report: &mut ValidationReport,
        context: &ValidationRuleContext<'_>,
    ) {
        for lv in &self.validators {
            if !self.layer_enabled(lv.layer) {
                continue;
            }
            lv.validator
                .validate_group_batch(root, segments, report, context);
            if self.bail_on_first_critical
                && report
                    .errors
                    .iter()
                    .any(|i| i.severity == ValidationSeverity::Critical)
            {
                break;
            }
        }
    }

    /// Execute validators with per-call typed metadata.
    pub fn validate_lenient_with<T: Any + Send + Sync>(
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

    /// Execute validators in strict mode for enabled layers.
    pub fn validate_strict(
        &self,
        segments: &[Segment<'_>],
    ) -> Result<ValidationReport, ValidationReport> {
        self.validate_lenient(segments).result()
    }

    /// Execute validators in strict mode with per-call typed metadata.
    pub fn validate_strict_with<T: Any + Send + Sync>(
        &self,
        segments: &[Segment<'_>],
        value: &T,
    ) -> Result<ValidationReport, ValidationReport> {
        self.validate_lenient_with(segments, value).result()
    }

    /// Execute validators in lenient mode against an owned-segment slice.
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
                message_type: None,
            })
            .unwrap_or_else(|| ValidationRuleContext {
                metadata: None,
                message_ref: self.message_ref.as_deref(),
                message_type: None,
            })
    }

    fn validate_with_context_owned(
        &self,
        segments: &[OwnedSegment],
        context: &ValidationRuleContext<'_>,
    ) -> ValidationReport {
        let mut report = ValidationReport::default();
        // Pre-extract UNH message type once (F-017).
        let unh_message_type: Option<String> = segments
            .iter()
            .find(|s| s.tag == "UNH")
            .and_then(|s| s.component_str(1, 0))
            .map(str::to_owned);
        let ctx_with_type;
        let effective_ctx: &ValidationRuleContext<'_> = if let Some(ref mt) = unh_message_type {
            ctx_with_type = ValidationRuleContext {
                metadata: context.metadata,
                message_ref: context.message_ref,
                message_type: Some(mt.as_str()),
            };
            &ctx_with_type
        } else {
            context
        };
        let mut full_borrowed: Option<Vec<Segment<'_>>> = None;
        let mut filtered_borrowed: Option<Vec<Segment<'_>>> = None;
        let mut envelope_ran = false;

        for lv in &self.validators {
            if !self.layer_enabled(lv.layer) {
                continue;
            }
            if lv.layer == ValidationLayer::Envelope {
                let borrowed: Vec<Segment<'_>> = segments.iter().map(|s| s.as_borrowed()).collect();
                lv.validator.validate_batch(&borrowed, &mut report, effective_ctx);
                envelope_ran = true;
            } else if envelope_ran {
                let active = filtered_borrowed.get_or_insert_with(|| {
                    segments
                        .iter()
                        .filter(|s| !matches!(s.tag.as_str(), "UNB" | "UNZ" | "UNG" | "UNE"))
                        .map(|s| s.as_borrowed())
                        .collect()
                });
                lv.validator.validate_batch(active, &mut report, effective_ctx);
            } else {
                let active = full_borrowed
                    .get_or_insert_with(|| segments.iter().map(|s| s.as_borrowed()).collect());
                lv.validator.validate_batch(active, &mut report, effective_ctx);
            }
            if self.bail_on_first_critical
                && report
                    .errors
                    .iter()
                    .any(|i| i.severity == ValidationSeverity::Critical)
            {
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
        report
    }

    /// Execute validators in strict mode against an owned-segment slice.
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
        let mut envelope_ran = false;

        for lv in &self.validators {
            if !self.layer_enabled(lv.layer) {
                continue;
            }
            if lv.layer == ValidationLayer::Envelope {
                lv.validator.validate_batch(segments, &mut report, effective_ctx);
                envelope_ran = true;
            } else {
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
                lv.validator.validate_batch(active, &mut report, effective_ctx);
            }
            if self.bail_on_first_critical
                && report
                    .errors
                    .iter()
                    .any(|i| i.severity == ValidationSeverity::Critical)
            {
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
    ///     let report = child.validate_lenient(&message_segments);
    /// }
    /// ```
    pub fn fork_with_message_ref(&self, message_ref: impl Into<String>) -> Self {
        Self {
            validators: self
                .validators
                .iter()
                .map(|lv| LayeredValidator {
                    layer: lv.layer,
                    validator: lv.validator.fork(),
                })
                .collect(),
            envelope_enabled: self.envelope_enabled,
            structure_enabled: self.structure_enabled,
            code_list_enabled: self.code_list_enabled,
            profile_enabled: self.profile_enabled,
            bail_on_first_critical: self.bail_on_first_critical,
            message_type: self.message_type.clone(),
            message_ref: Some(message_ref.into()),
            metadata: self.metadata.clone(),
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
}
