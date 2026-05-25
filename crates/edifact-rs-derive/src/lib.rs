//! Derive macros for `EdifactSerialize` and `EdifactDeserialize`.
//!
//! # Segment struct (single segment)
//!
//! ```ignore
//! #[derive(EdifactSerialize, EdifactDeserialize)]
//! #[edifact(segment = "BGM")]
//! pub struct BgmSegment {
//!     #[edifact(element = 0)]
//!     pub doc_name_code: String,
//!     #[edifact(element = 1)]
//!     pub doc_id: String,
//!     #[edifact(element = 2)]
//!     pub msg_function: Option<String>,
//! }
//! ```
//!
//! # Segment struct with qualifier
//!
//! ```ignore
//! #[derive(EdifactSerialize, EdifactDeserialize)]
//! #[edifact(segment = "NAD", qualifier = "MS")]
//! pub struct NadMs {
//!     #[edifact(element = 1)]
//!     pub party_id: String,
//! }
//! ```
//!
//! # Message struct (multiple segments)
//!
//! ```ignore
//! #[derive(EdifactSerialize, EdifactDeserialize)]
//! pub struct OrdersMessage {
//!     pub bgm: BgmSegment,
//!     pub buyer: NadMs,
//!     #[edifact(group)]
//!     pub lines: Vec<LinSegment>,
//! }
//! ```
//!
//! # `#[edifact(group)]` and `Vec<T>` fields
//!
//! The `#[edifact(group)]` attribute marks a `Vec<T>` field as a contiguous group of
//! repeated segments.  Without the attribute, `Vec<T>` on a segment struct collects
//! all matching segments from the window into the `Vec`.
//!
//! **Note**: `#[edifact(group)]` is a documentation and diagnostic attribute only — the
//! generated deserialization code for `Vec<T>` is identical whether the attribute is
//! present or absent.  Its value is in self-documenting intent and in enabling future
//! compile-time group-boundary enforcement.
//!
//! # Non-`String` fields and `Display` / `FromStr`
//!
//! Non-`String` field types (e.g. `u32`, `bool`, your own newtype) are serialized via
//! `Display` and deserialized via `FromStr`.  The derive macro does **not** add a
//! compile-time bound; if the type does not implement both traits the generated code
//! will fail to compile with a standard "trait not satisfied" error.
//!
//! To avoid surprises, ensure any non-`String` field type implements both:
//! ```ignore
//! impl std::fmt::Display for MyCode { ... }
//! impl std::str::FromStr for MyCode { ... }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, Type, parse_macro_input, spanned::Spanned};

// ── entry points ───────────────────────────────────────────────────────────────

#[proc_macro_derive(EdifactSerialize, attributes(edifact))]
/// Derive `edifact_rs::EdifactSerialize` for segment or message structs.
///
/// # Limitations
///
/// - **No generics**: the struct must not have generic type parameters.
/// - **No lifetime parameters**: the struct must own all its data (`String`,
///   not `&str`).  Borrow-based structs such as `Segment<'a>` cannot use this
///   derive macro.
pub fn derive_edifact_serialize(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    impl_serialize(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

#[proc_macro_derive(EdifactDeserialize, attributes(edifact))]
/// Derive `edifact_rs::EdifactDeserialize` for segment or message structs.
///
/// # Limitations
///
/// - **No generics**: the struct must not have generic type parameters.
/// - **No lifetime parameters**: the struct must own all its data (`String`,
///   not `&str`).  Add owned wrapper types or clone components at the
///   deserialization site if lifetime flexibility is required.
pub fn derive_edifact_deserialize(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    impl_deserialize(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

// ── attribute containers ───────────────────────────────────────────────────────

#[derive(Default)]
struct StructAttrs {
    /// `#[edifact(segment = "TAG")]`
    segment: Option<String>,
    /// `#[edifact(qualifier = "Q")]` — element 0 value for segment matching
    qualifier: Option<String>,
    qualifier_span: Option<proc_macro2::Span>,
    /// `#[edifact(qualifier_from = N)]` — zero-based element index; qualifier is dynamic at runtime.
    qualifier_from: Option<u32>,
    qualifier_from_span: Option<proc_macro2::Span>,
}

#[derive(Default)]
struct FieldAttrs {
    /// `#[edifact(element = N)]` — zero-based element index
    element: Option<u32>,
    element_span: Option<proc_macro2::Span>,
    /// `#[edifact(component = N)]` — component index within the element (for composite data elements)
    component: Option<u32>,
    component_span: Option<proc_macro2::Span>,
    /// `#[edifact(composite)]` — map the field as a full composite element via composite serde traits.
    composite: bool,
    composite_span: Option<proc_macro2::Span>,
    /// `#[edifact(group)]` — `Vec<T>`: each item is a separate segment
    group: bool,
    group_span: Option<proc_macro2::Span>,
    /// `#[edifact(qualifier = "Q")]` — message field constrained to qualifier.
    qualifier: Option<String>,
    qualifier_span: Option<proc_macro2::Span>,
}

// ── attribute parsing ──────────────────────────────────────────────────────────

fn parse_struct_attrs(input: &DeriveInput) -> syn::Result<StructAttrs> {
    let mut out = StructAttrs::default();
    for attr in &input.attrs {
        if !attr.path().is_ident("edifact") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("segment") {
                let lit = meta.value()?.parse::<syn::LitStr>()?;
                let tag = lit.value();
                if tag.len() != 3 || !tag.bytes().all(|b| b.is_ascii_uppercase()) {
                    return Err(syn::Error::new(
                        lit.span(),
                        format!(
                            "segment tag must be exactly 3 ASCII uppercase letters; got {tag:?}"
                        ),
                    ));
                }
                out.segment = Some(tag);
            } else if meta.path.is_ident("qualifier") {
                out.qualifier = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                out.qualifier_span = Some(meta.path.span());
            } else if meta.path.is_ident("qualifier_from") {
                let idx: u32 = meta.value()?.parse::<syn::LitInt>()?.base10_parse()?;
                out.qualifier_from = Some(idx);
                out.qualifier_from_span = Some(meta.path.span());
            } else {
                return Err(meta.error("unknown struct-level `edifact` key; expected `segment`, `qualifier`, or `qualifier_from`"));
            }
            Ok(())
        })?;
    }
    if (out.qualifier.is_some() || out.qualifier_from.is_some()) && out.segment.is_none() {
        return Err(syn::Error::new(
            out.qualifier_span
                .or(out.qualifier_from_span)
                .unwrap_or_else(|| input.span()),
            "#[edifact(qualifier = ...)] / #[edifact(qualifier_from = ...)] require #[edifact(segment = ...)]",
        ));
    }
    if out.qualifier.is_some() && out.qualifier_from.is_some() {
        return Err(syn::Error::new(
            out.qualifier_from_span
                .or(out.qualifier_span)
                .unwrap_or_else(|| input.span()),
            "use either #[edifact(qualifier = ...)] or #[edifact(qualifier_from = ...)], not both",
        ));
    }
    Ok(out)
}

fn parse_field_attrs(field: &Field) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    for attr in &field.attrs {
        if !attr.path().is_ident("edifact") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("element") {
                out.element = Some(meta.value()?.parse::<syn::LitInt>()?.base10_parse()?);
                out.element_span = Some(meta.path.span());
            } else if meta.path.is_ident("component") {
                out.component = Some(meta.value()?.parse::<syn::LitInt>()?.base10_parse()?);
                out.component_span = Some(meta.path.span());
            } else if meta.path.is_ident("composite") {
                out.composite = true;
                out.composite_span = Some(meta.path.span());
            } else if meta.path.is_ident("group") {
                out.group = true;
                out.group_span = Some(meta.path.span());
            } else if meta.path.is_ident("qualifier") {
                out.qualifier = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                out.qualifier_span = Some(meta.path.span());
            } else {
                return Err(meta.error("unknown field-level `edifact` key; expected `element`, `component`, `composite`, `group`, or `qualifier`"));
            }
            Ok(())
        })?;
    }
    Ok(out)
}

// ── type helpers ───────────────────────────────────────────────────────────────

fn is_option_type(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Option"))
}

fn is_vec_type(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Vec"))
}

/// Returns `true` for the `String` path type.
fn is_string_type(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.is_ident("String")
        || p.path.segments.last().is_some_and(|s| s.ident == "String"))
}

/// Returns `true` for `&str` or `&'_ str` reference types.
fn is_str_ref_type(ty: &Type) -> bool {
    let Type::Reference(r) = ty else { return false };
    matches!(r.elem.as_ref(), Type::Path(p) if p.path.is_ident("str"))
}

/// Returns `true` when `ty` is a type that can yield `&str` without allocating
/// (i.e. `String` or `&str`).
fn is_str_like(ty: &Type) -> bool {
    is_string_type(ty) || is_str_ref_type(ty)
}

fn option_inner_type(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let seg = path.path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(inner) = args.args.first()? else {
        return None;
    };
    Some(inner)
}

fn vec_inner_type(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let seg = path.path.segments.last()?;
    if seg.ident != "Vec" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(inner) = args.args.first()? else {
        return None;
    };
    Some(inner)
}

// ── named field extraction ─────────────────────────────────────────────────────

fn get_named_fields(input: &DeriveInput) -> syn::Result<&syn::FieldsNamed> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.params.span(),
            "EdifactSerialize/EdifactDeserialize do not support generic structs",
        ));
    }
    match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => Ok(f),
            _ => Err(syn::Error::new(
                input.span(),
                "EdifactSerialize/EdifactDeserialize only support structs with named fields",
            )),
        },
        _ => Err(syn::Error::new(
            input.span(),
            "EdifactSerialize/EdifactDeserialize only support structs",
        )),
    }
}

fn validate_field_attrs(
    ident: &syn::Ident,
    ty: &Type,
    attrs: &FieldAttrs,
    is_segment_struct: bool,
) -> syn::Result<()> {
    if attrs.group && !is_vec_type(ty) {
        return Err(syn::Error::new(
            attrs.group_span.unwrap_or_else(|| ident.span()),
            format!("field `{ident}`: #[edifact(group)] requires Vec<T>"),
        ));
    }
    if attrs.group && (attrs.element.is_some() || attrs.component.is_some()) {
        return Err(syn::Error::new(
            attrs.group_span.unwrap_or_else(|| ident.span()),
            format!(
                "field `{ident}`: #[edifact(group)] cannot be combined with element/component positioning"
            ),
        ));
    }
    if attrs.composite && attrs.component.is_some() {
        return Err(syn::Error::new(
            attrs.component_span.unwrap_or_else(|| ident.span()),
            format!(
                "field `{ident}`: #[edifact(component = ...)] cannot be combined with #[edifact(composite)]"
            ),
        ));
    }
    if attrs.composite && attrs.group {
        return Err(syn::Error::new(
            attrs.composite_span.unwrap_or_else(|| ident.span()),
            format!(
                "field `{ident}`: #[edifact(composite)] cannot be combined with #[edifact(group)]"
            ),
        ));
    }
    if is_segment_struct && attrs.group {
        return Err(syn::Error::new(
            attrs.group_span.unwrap_or_else(|| ident.span()),
            format!("field `{ident}`: #[edifact(group)] is only valid on message structs"),
        ));
    }
    if !is_segment_struct && (attrs.element.is_some() || attrs.component.is_some()) {
        return Err(syn::Error::new(
            attrs
                .element_span
                .or(attrs.component_span)
                .unwrap_or_else(|| ident.span()),
            format!(
                "field `{ident}`: element/component positioning is only valid on segment structs"
            ),
        ));
    }
    if !is_segment_struct && attrs.composite {
        return Err(syn::Error::new(
            attrs.composite_span.unwrap_or_else(|| ident.span()),
            format!("field `{ident}`: #[edifact(composite)] is only valid on segment structs"),
        ));
    }
    if is_segment_struct && attrs.qualifier.is_some() {
        return Err(syn::Error::new(
            attrs.qualifier_span.unwrap_or_else(|| ident.span()),
            format!(
                "field `{ident}`: #[edifact(qualifier = ...)] is only valid on message struct fields"
            ),
        ));
    }
    if attrs.qualifier.is_some() && attrs.group && !is_vec_type(ty) {
        return Err(syn::Error::new(
            attrs.qualifier_span.unwrap_or_else(|| ident.span()),
            format!("field `{ident}`: qualifier-constrained groups must be Vec<T>"),
        ));
    }
    Ok(())
}

// ── EdifactSerialize ───────────────────────────────────────────────────────────

fn impl_serialize(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let struct_attrs = parse_struct_attrs(input)?;
    let fields = get_named_fields(input)?;
    let is_segment_struct = struct_attrs.segment.is_some();

    // Collect (field_ident, field_type, FieldAttrs).
    let field_data: Vec<(&syn::Ident, &Type, FieldAttrs)> = fields
        .named
        .iter()
        .map(|f| {
            let attrs = parse_field_attrs(f)?;
            let ident = f
                .ident
                .as_ref()
                .ok_or_else(|| syn::Error::new_spanned(f, "only named fields are supported"))?;
            validate_field_attrs(ident, &f.ty, &attrs, is_segment_struct)?;
            Ok((ident, &f.ty, attrs))
        })
        .collect::<syn::Result<_>>()?;

    let body = if let Some(seg_tag) = &struct_attrs.segment {
        // ── Segment struct: emit one EDIFACT segment ──────────────────────────
        // When a struct-level qualifier is declared, inject it at slot 0.
        // Fields at (element=0, component>=1) extend it as composite components.
        // Fields at element >= 1 are emitted as regular elements.
        let (qualifier_emit, start_slot, elem0_comp_stmts) = if let Some(qual) =
            &struct_attrs.qualifier
        {
            // Error only if a field claims element=0 with no component or component=0.
            for (i, (ident, _, attrs)) in field_data.iter().enumerate() {
                let elem = attrs.element.unwrap_or(i as u32);
                let comp = attrs.component.unwrap_or(0);
                if elem == 0 && comp == 0 {
                    return Err(syn::Error::new(
                        attrs
                            .element_span
                            .or(attrs.component_span)
                            .unwrap_or_else(|| ident.span()),
                        format!(
                            "field `{}`: cannot use #[edifact(qualifier = ...)] with a field at element = 0 without component >= 1; the qualifier occupies component 0",
                            ident
                        ),
                    ));
                }
            }
            // Collect fields at element=0, component>0, sorted by component.
            let mut comp_fields: Vec<(u32, usize)> = field_data
                .iter()
                .enumerate()
                .filter_map(|(i, (_, _, attrs))| {
                    let elem = attrs.element.unwrap_or(i as u32);
                    let comp = attrs.component.unwrap_or(0);
                    if elem == 0 && comp > 0 {
                        Some((comp, i))
                    } else {
                        None
                    }
                })
                .collect();
            comp_fields.sort_by_key(|(c, _)| *c);
            let comp_stmts: Vec<TokenStream2> = comp_fields
                .iter()
                .map(|(_, fi)| {
                    let (ident, ty, _) = &field_data[*fi];
                    emit_component_element(ident, ty)
                })
                .collect();
            let q = quote! {
                emitter.emit(::edifact_rs::EdifactEvent::Element { value: #qual })?;
            };
            (q, 1u32, quote! { #(#comp_stmts)* })
        } else {
            (quote! {}, 0u32, quote! {})
        };

        // Rebuild indexed/field_map excluding element=0 fields (handled above).
        let regular_field_data: Vec<(u32, usize)> = field_data
            .iter()
            .enumerate()
            .filter_map(|(i, (_, _, attrs))| {
                let elem = attrs.element.unwrap_or(i as u32);
                if elem < start_slot {
                    None
                } else {
                    Some((elem, i))
                }
            })
            .collect();
        let reg_max_idx = regular_field_data
            .iter()
            .map(|(e, _)| *e)
            .max()
            .unwrap_or(start_slot.saturating_sub(1));
        let reg_field_map: std::collections::HashMap<u32, usize> =
            regular_field_data.iter().copied().collect();

        let mut elem_stmts: Vec<TokenStream2> = Vec::new();
        for slot in start_slot..=reg_max_idx {
            if let Some(&fi) = reg_field_map.get(&slot) {
                let (ident, ty, attrs) = &field_data[fi];
                if attrs.composite {
                    elem_stmts.push(emit_composite_field(ident, ty));
                } else {
                    elem_stmts.push(emit_element(ident, ty));
                }
            } else {
                // Gap: emit an empty element separator.
                elem_stmts.push(quote! {
                    emitter.emit(::edifact_rs::EdifactEvent::Element { value: "" })?;
                });
            }
        }

        quote! {
            emitter.emit(::edifact_rs::EdifactEvent::StartSegment { tag: #seg_tag })?;
            #qualifier_emit
            #elem0_comp_stmts
            #(#elem_stmts)*
            emitter.emit(::edifact_rs::EdifactEvent::EndSegment)?;
        }
    } else {
        // ── Message struct: delegate to each field ────────────────────────────
        let stmts: Vec<TokenStream2> = field_data
            .iter()
            .map(|(ident, ty, attrs)| {
                if attrs.group || is_vec_type(ty) {
                    quote! {
                        for __item in &self.#ident {
                            ::edifact_rs::EdifactSerialize::edifact_serialize(__item, emitter)?;
                        }
                    }
                } else {
                    quote! {
                        ::edifact_rs::EdifactSerialize::edifact_serialize(&self.#ident, emitter)?;
                    }
                }
            })
            .collect();
        quote! { #(#stmts)* }
    };

    Ok(quote! {
        impl ::edifact_rs::EdifactSerialize for #name {
            fn edifact_serialize<__E: ::edifact_rs::EventEmitter>(
                &self,
                emitter: &mut __E,
            ) -> ::core::result::Result<(), ::edifact_rs::EdifactError> {
                #body
                ::core::result::Result::Ok(())
            }
        }
    })
}

/// Generate the token stream that emits field `ident` (of type `ty`) as one element.
///
/// For `String` and `&str` fields the value is emitted zero-copy via `.as_str()`
/// (or directly).  All other types fall back to `ToString::to_string`.
fn emit_element(ident: &syn::Ident, ty: &Type) -> TokenStream2 {
    if is_option_type(ty) {
        let inner_is_str = option_inner_type(ty).is_some_and(is_str_like);
        if inner_is_str {
            quote! {
                match &self.#ident {
                    ::core::option::Option::Some(__v) => {
                        emitter.emit(::edifact_rs::EdifactEvent::Element { value: __v.as_str() })?;
                    }
                    ::core::option::Option::None => {
                        emitter.emit(::edifact_rs::EdifactEvent::Element { value: "" })?;
                    }
                }
            }
        } else {
            quote! {
                match &self.#ident {
                    ::core::option::Option::Some(__v) => {
                        let __s = ::std::string::ToString::to_string(__v);
                        emitter.emit(::edifact_rs::EdifactEvent::Element { value: &__s })?;
                    }
                    ::core::option::Option::None => {
                        emitter.emit(::edifact_rs::EdifactEvent::Element { value: "" })?;
                    }
                }
            }
        }
    } else if is_string_type(ty) {
        quote! {
            emitter.emit(::edifact_rs::EdifactEvent::Element { value: self.#ident.as_str() })?;
        }
    } else if is_str_ref_type(ty) {
        quote! {
            emitter.emit(::edifact_rs::EdifactEvent::Element { value: self.#ident })?;
        }
    } else {
        quote! {
            {
                let __s = ::std::string::ToString::to_string(&self.#ident);
                emitter.emit(::edifact_rs::EdifactEvent::Element { value: &__s })?;
            }
        }
    }
}

/// Generate the token stream that emits field `ident` as a composite component (`ComponentElement`).
///
/// For `String` and `&str` fields the value is emitted zero-copy.
fn emit_component_element(ident: &syn::Ident, ty: &Type) -> TokenStream2 {
    if is_option_type(ty) {
        let inner_is_str = option_inner_type(ty).is_some_and(is_str_like);
        if inner_is_str {
            quote! {
                match &self.#ident {
                    ::core::option::Option::Some(__v) => {
                        emitter.emit(::edifact_rs::EdifactEvent::ComponentElement { value: __v.as_str() })?;
                    }
                    ::core::option::Option::None => {
                        emitter.emit(::edifact_rs::EdifactEvent::ComponentElement { value: "" })?;
                    }
                }
            }
        } else {
            quote! {
                match &self.#ident {
                    ::core::option::Option::Some(__v) => {
                        let __s = ::std::string::ToString::to_string(__v);
                        emitter.emit(::edifact_rs::EdifactEvent::ComponentElement { value: &__s })?;
                    }
                    ::core::option::Option::None => {
                        emitter.emit(::edifact_rs::EdifactEvent::ComponentElement { value: "" })?;
                    }
                }
            }
        }
    } else if is_string_type(ty) {
        quote! {
            emitter.emit(::edifact_rs::EdifactEvent::ComponentElement { value: self.#ident.as_str() })?;
        }
    } else if is_str_ref_type(ty) {
        quote! {
            emitter.emit(::edifact_rs::EdifactEvent::ComponentElement { value: self.#ident })?;
        }
    } else {
        quote! {
            {
                let __s = ::std::string::ToString::to_string(&self.#ident);
                emitter.emit(::edifact_rs::EdifactEvent::ComponentElement { value: &__s })?;
            }
        }
    }
}

/// Generate the token stream that emits a full composite field via `EdifactCompositeSerialize`.
fn emit_composite_field(ident: &syn::Ident, ty: &Type) -> TokenStream2 {
    if is_option_type(ty) {
        quote! {
            match &self.#ident {
                ::core::option::Option::Some(__v) => {
                    ::edifact_rs::EdifactCompositeSerialize::edifact_serialize_composite(__v, emitter)?;
                }
                ::core::option::Option::None => {
                    emitter.emit(::edifact_rs::EdifactEvent::Element { value: "" })?;
                }
            }
        }
    } else {
        quote! {
            ::edifact_rs::EdifactCompositeSerialize::edifact_serialize_composite(&self.#ident, emitter)?;
        }
    }
}

// ── EdifactDeserialize ─────────────────────────────────────────────────────────

fn impl_deserialize(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let struct_attrs = parse_struct_attrs(input)?;
    let fields = get_named_fields(input)?;
    let is_segment_struct = struct_attrs.segment.is_some();

    let field_data: Vec<(&syn::Ident, &Type, FieldAttrs)> = fields
        .named
        .iter()
        .map(|f| {
            let attrs = parse_field_attrs(f)?;
            let ident = f
                .ident
                .as_ref()
                .ok_or_else(|| syn::Error::new_spanned(f, "only named fields are supported"))?;
            validate_field_attrs(ident, &f.ty, &attrs, is_segment_struct)?;
            Ok((ident, &f.ty, attrs))
        })
        .collect::<syn::Result<_>>()?;

    let field_names: Vec<&syn::Ident> = field_data.iter().map(|(id, _, _)| *id).collect();

    let (body, owned_body, segment_tag_impl) = if let Some(seg_tag) = &struct_attrs.segment {
        // ── Segment struct ────────────────────────────────────────────────────
        let qualifier_guard = if let Some(qual) = &struct_attrs.qualifier {
            quote! {
                if __seg.element_str(0).unwrap_or("") != #qual {
                    return ::core::result::Result::Err(
                        ::edifact_rs::EdifactError::MissingRequiredElement {
                            tag: #seg_tag.to_owned(),
                            element_index: 0,
                        }
                    );
                }
            }
        } else if let Some(idx) = struct_attrs.qualifier_from {
            quote! {
                if __seg.element_str(#idx as usize).unwrap_or("").is_empty() {
                    return ::core::result::Result::Err(
                        ::edifact_rs::EdifactError::MissingRequiredElement {
                            tag: #seg_tag.to_owned(),
                            element_index: #idx as usize,
                        }
                    );
                }
            }
        } else {
            quote! {}
        };

        let find_seg = if let Some(qual) = &struct_attrs.qualifier {
            quote! {
                ::edifact_rs::find_qualified_segment(segments, #seg_tag, #qual)
            }
        } else {
            quote! {
                ::edifact_rs::find_segment(segments, #seg_tag)
            }
        };

        let field_inits: Vec<TokenStream2> = field_data
            .iter()
            .enumerate()
            .map(|(decl_i, (ident, ty, attrs))| -> syn::Result<TokenStream2> {
                let idx = attrs.element.unwrap_or(decl_i as u32) as usize;
                if attrs.composite {
                    if is_option_type(ty) {
                        let inner_ty = option_inner_type(ty)
                            .ok_or_else(|| syn::Error::new(ident.span(), "expected Option<T>"))?;
                        return Ok(quote! {
                            let #ident = match ::edifact_rs::composite_element(__seg, #idx) {
                                ::core::option::Option::Some(__composite) => {
                                    ::core::option::Option::Some(
                                        <#inner_ty as ::edifact_rs::EdifactCompositeDeserialize>::edifact_deserialize_composite(__composite)?
                                    )
                                }
                                ::core::option::Option::None => ::core::option::Option::None,
                            };
                        });
                    }
                    return Ok(quote! {
                        let #ident = <#ty as ::edifact_rs::EdifactCompositeDeserialize>::edifact_deserialize_composite(
                            ::edifact_rs::composite_element(__seg, #idx).ok_or_else(|| ::edifact_rs::EdifactError::MissingRequiredElement {
                                tag: #seg_tag.to_owned(),
                                element_index: #idx as usize,
                            })?
                        )?;
                    });
                }
                let value_expr = if let Some(comp) = attrs.component {
                    let comp = comp as usize;
                    quote! {
                        __seg.get_element(#idx).and_then(|__e| __e.get_component(#comp))
                    }
                } else {
                    quote! { __seg.element_str(#idx) }
                };
                Ok(if is_option_type(ty) {
                    quote! {
                        let #ident = #value_expr
                            .filter(|__s| !__s.is_empty())
                            .map(::std::string::String::from);
                    }
                } else {
                    quote! {
                        let #ident = #value_expr
                            .filter(|__s| !__s.is_empty())
                            .ok_or_else(|| ::edifact_rs::EdifactError::MissingRequiredElement {
                                tag: #seg_tag.to_owned(),
                                element_index: #idx as usize,
                            })?
                            .to_owned();
                    }
                })
            })
            .collect::<syn::Result<_>>()?;

        let body = quote! {
            let __seg = #find_seg
                .ok_or_else(|| ::edifact_rs::EdifactError::MissingSegment {
                    tag: #seg_tag.to_owned(),
                    expected_position: "message body".to_owned(),
                })?;
            #qualifier_guard
            #(#field_inits)*
            ::core::result::Result::Ok(Self { #(#field_names),* })
        };

        // Also generate EdifactSegmentTag impl.
        let qualifier_match = if let Some(qual) = &struct_attrs.qualifier {
            quote! {
                fn matches_segment(seg: &::edifact_rs::Segment<'_>) -> bool {
                    seg.tag == Self::SEGMENT_TAG
                        && seg.element_str(0).unwrap_or("") == #qual
                }
            }
        } else if let Some(idx) = struct_attrs.qualifier_from {
            quote! {
                fn matches_segment(seg: &::edifact_rs::Segment<'_>) -> bool {
                    seg.tag == Self::SEGMENT_TAG
                        && !seg.element_str(#idx as usize).unwrap_or("").is_empty()
                }
            }
        } else {
            quote! {}
        };

        let seg_tag_impl = quote! {
            impl ::edifact_rs::EdifactSegmentTag for #name {
                const SEGMENT_TAG: &'static str = #seg_tag;
                #qualifier_match
            }
        };

        // ── Owned-segment deserialization path ────────────────────────────────
        // Works directly on `&[OwnedSegment]` without allocating a `Vec<Segment>`.
        let find_seg_owned = if let Some(qual) = &struct_attrs.qualifier {
            quote! {
                ::edifact_rs::find_qualified_segment_owned(segments, #seg_tag, #qual)
            }
        } else {
            quote! {
                ::edifact_rs::find_segment_owned(segments, #seg_tag)
            }
        };

        let field_inits_owned: Vec<TokenStream2> = field_data
            .iter()
            .enumerate()
            .map(|(decl_i, (ident, ty, attrs))| -> syn::Result<TokenStream2> {
                let idx = attrs.element.unwrap_or(decl_i as u32) as usize;
                if attrs.composite {
                    if is_option_type(ty) {
                        let inner_ty = option_inner_type(ty)
                            .ok_or_else(|| syn::Error::new(ident.span(), "expected Option<T>"))?;
                        return Ok(quote! {
                            let #ident = match __seg.elements.get(#idx) {
                                ::core::option::Option::Some(__e) => {
                                    let __cows = __e.components.iter()
                                        .map(|s| ::std::borrow::Cow::Borrowed(s.as_str()))
                                        .collect::<::std::vec::Vec<::std::borrow::Cow<'_, str>>>();
                                    ::core::option::Option::Some(
                                        <#inner_ty as ::edifact_rs::EdifactCompositeDeserialize>::edifact_deserialize_composite(
                                            ::edifact_rs::CompositeElement::from_slice(&__cows)
                                        )?
                                    )
                                }
                                ::core::option::Option::None => ::core::option::Option::None,
                            };
                        });
                    }
                    return Ok(quote! {
                        let #ident = {
                            let __cows = __seg.elements.get(#idx)
                                .ok_or_else(|| ::edifact_rs::EdifactError::MissingRequiredElement {
                                    tag: #seg_tag.to_owned(),
                                    element_index: #idx as usize,
                                })?
                                .components.iter()
                                .map(|s| ::std::borrow::Cow::Borrowed(s.as_str()))
                                .collect::<::std::vec::Vec<::std::borrow::Cow<'_, str>>>();
                            <#ty as ::edifact_rs::EdifactCompositeDeserialize>::edifact_deserialize_composite(
                                ::edifact_rs::CompositeElement::from_slice(&__cows)
                            )?
                        };
                    });
                }
                let value_expr_owned = if let Some(comp) = attrs.component {
                    let comp = comp as usize;
                    quote! { __seg.component_str(#idx, #comp) }
                } else {
                    quote! { __seg.element_str(#idx) }
                };
                Ok(if is_option_type(ty) {
                    quote! {
                        let #ident = #value_expr_owned
                            .filter(|__s| !__s.is_empty())
                            .map(::std::string::String::from);
                    }
                } else {
                    quote! {
                        let #ident = #value_expr_owned
                            .filter(|__s| !__s.is_empty())
                            .ok_or_else(|| ::edifact_rs::EdifactError::MissingRequiredElement {
                                tag: #seg_tag.to_owned(),
                                element_index: #idx as usize,
                            })?
                            .to_owned();
                    }
                })
            })
            .collect::<syn::Result<_>>()?;

        let owned_body = quote! {
            let __seg = #find_seg_owned
                .ok_or_else(|| ::edifact_rs::EdifactError::MissingSegment {
                    tag: #seg_tag.to_owned(),
                    expected_position: "message body".to_owned(),
                })?;
            #qualifier_guard
            #(#field_inits_owned)*
            ::core::result::Result::Ok(Self { #(#field_names),* })
        };

        (body, owned_body, seg_tag_impl)
    } else {
        // ── Message struct: delegate to each field ────────────────────────────
        let field_inits: Vec<TokenStream2> = field_data
            .iter()
            .map(|(ident, ty, attrs)| -> syn::Result<TokenStream2> {
                Ok(if let Some(qual) = &attrs.qualifier {
                    if attrs.group || is_vec_type(ty) {
                        let inner_ty = vec_inner_type(ty)
                            .ok_or_else(|| syn::Error::new(ident.span(), "expected Vec<T>"))?;
                        quote! {
                            let #ident = segments
                                .iter()
                                .filter(|__seg| {
                                    __seg.tag == <#inner_ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG
                                        && __seg.element_str(0).unwrap_or("") == #qual
                                })
                                .map(|__seg| {
                                    ::edifact_rs::EdifactDeserialize::edifact_deserialize(
                                        ::core::slice::from_ref(__seg),
                                    )
                                })
                                .collect::<::core::result::Result<::std::vec::Vec<#inner_ty>, ::edifact_rs::EdifactError>>()?;
                        }
                    } else if is_option_type(ty) {
                        let inner_ty = option_inner_type(ty)
                            .ok_or_else(|| syn::Error::new(ident.span(), "expected Option<T>"))?;
                        quote! {
                            let #ident = match ::edifact_rs::find_qualified_segment(
                                segments,
                                <#inner_ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG,
                                #qual,
                            ) {
                                ::core::option::Option::Some(__seg) => {
                                    ::core::option::Option::Some(
                                        ::edifact_rs::EdifactDeserialize::edifact_deserialize(
                                            ::core::slice::from_ref(__seg),
                                        )?
                                    )
                                }
                                ::core::option::Option::None => ::core::option::Option::None,
                            };
                        }
                    } else {
                        quote! {
                            let __seg = ::edifact_rs::find_qualified_segment(
                                segments,
                                <#ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG,
                                #qual,
                            )
                            .ok_or_else(|| ::edifact_rs::EdifactError::MissingSegment {
                                tag: <#ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG.to_owned(),
                                expected_position: "message body".to_owned(),
                            })?;
                            let #ident = ::edifact_rs::EdifactDeserialize::edifact_deserialize(
                                ::core::slice::from_ref(__seg),
                            )?;
                        }
                    }
                } else if attrs.group || is_vec_type(ty) {
                    let inner_ty = vec_inner_type(ty)
                        .ok_or_else(|| syn::Error::new(ident.span(), "expected Vec<T>"))?;
                    quote! {
                        let #ident = ::edifact_rs::find_segments_typed::<#inner_ty>(segments)
                            .map(|__seg| {
                                <#inner_ty as ::edifact_rs::EdifactDeserialize>::edifact_deserialize(
                                    ::core::slice::from_ref(__seg),
                                )
                            })
                            .collect::<::core::result::Result<::std::vec::Vec<#inner_ty>, _>>()?;
                    }
                } else if is_option_type(ty) {
                    let inner_ty = option_inner_type(ty)
                        .ok_or_else(|| syn::Error::new(ident.span(), "expected Option<T>"))?;
                    quote! {
                        let #ident = if segments
                            .iter()
                            .any(|__seg| __seg.tag == <#inner_ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG)
                        {
                            ::core::option::Option::Some(
                                <#inner_ty as ::edifact_rs::EdifactDeserialize>::edifact_deserialize(segments)?
                            )
                        } else {
                            ::core::option::Option::None
                        };
                    }
                } else {
                    quote! {
                        let #ident = ::edifact_rs::EdifactDeserialize::edifact_deserialize(segments)?;
                    }
                })
            })
            .collect::<syn::Result<_>>()?;

        let body = quote! {
            #(#field_inits)*
            ::core::result::Result::Ok(Self { #(#field_names),* })
        };

        // ── Owned-segment message deserialization path ────────────────────────
        // Works directly on `&[OwnedSegment]` without converting to `Vec<Segment>`.
        let field_inits_owned: Vec<TokenStream2> = field_data
            .iter()
            .map(|(ident, ty, attrs)| -> syn::Result<TokenStream2> {
                Ok(if let Some(qual) = &attrs.qualifier {
                    if attrs.group || is_vec_type(ty) {
                        let inner_ty = vec_inner_type(ty)
                            .ok_or_else(|| syn::Error::new(ident.span(), "expected Vec<T>"))?;
                        quote! {
                            let #ident = segments
                                .iter()
                                .filter(|__seg| {
                                    __seg.tag == <#inner_ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG
                                        && __seg.element_str(0).unwrap_or("") == #qual
                                })
                                .map(|__seg| {
                                    <#inner_ty as ::edifact_rs::EdifactDeserialize>::edifact_deserialize_owned(
                                        ::core::slice::from_ref(__seg),
                                    )
                                })
                                .collect::<::core::result::Result<::std::vec::Vec<#inner_ty>, ::edifact_rs::EdifactError>>()?;
                        }
                    } else if is_option_type(ty) {
                        let inner_ty = option_inner_type(ty)
                            .ok_or_else(|| syn::Error::new(ident.span(), "expected Option<T>"))?;
                        quote! {
                            let #ident = match ::edifact_rs::find_qualified_segment_owned(
                                segments,
                                <#inner_ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG,
                                #qual,
                            ) {
                                ::core::option::Option::Some(__seg) => {
                                    ::core::option::Option::Some(
                                        <#inner_ty as ::edifact_rs::EdifactDeserialize>::edifact_deserialize_owned(
                                            ::core::slice::from_ref(__seg),
                                        )?
                                    )
                                }
                                ::core::option::Option::None => ::core::option::Option::None,
                            };
                        }
                    } else {
                        quote! {
                            let __seg = ::edifact_rs::find_qualified_segment_owned(
                                segments,
                                <#ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG,
                                #qual,
                            )
                            .ok_or_else(|| ::edifact_rs::EdifactError::MissingSegment {
                                tag: <#ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG.to_owned(),
                                expected_position: "message body".to_owned(),
                            })?;
                            let #ident = <#ty as ::edifact_rs::EdifactDeserialize>::edifact_deserialize_owned(
                                ::core::slice::from_ref(__seg),
                            )?;
                        }
                    }
                } else if attrs.group || is_vec_type(ty) {
                    let inner_ty = vec_inner_type(ty)
                        .ok_or_else(|| syn::Error::new(ident.span(), "expected Vec<T>"))?;
                    quote! {
                        let #ident = segments
                            .iter()
                            .filter(|__seg| <#inner_ty as ::edifact_rs::EdifactSegmentTag>::matches_owned_segment(__seg))
                            .map(|__seg| {
                                <#inner_ty as ::edifact_rs::EdifactDeserialize>::edifact_deserialize_owned(
                                    ::core::slice::from_ref(__seg),
                                )
                            })
                            .collect::<::core::result::Result<::std::vec::Vec<#inner_ty>, _>>()?;
                    }
                } else if is_option_type(ty) {
                    let inner_ty = option_inner_type(ty)
                        .ok_or_else(|| syn::Error::new(ident.span(), "expected Option<T>"))?;
                    quote! {
                        let #ident = if segments
                            .iter()
                            .any(|__seg| __seg.tag == <#inner_ty as ::edifact_rs::EdifactSegmentTag>::SEGMENT_TAG)
                        {
                            ::core::option::Option::Some(
                                <#inner_ty as ::edifact_rs::EdifactDeserialize>::edifact_deserialize_owned(segments)?
                            )
                        } else {
                            ::core::option::Option::None
                        };
                    }
                } else {
                    quote! {
                        let #ident = <#ty as ::edifact_rs::EdifactDeserialize>::edifact_deserialize_owned(segments)?;
                    }
                })
            })
            .collect::<syn::Result<_>>()?;

        let owned_body = quote! {
            #(#field_inits_owned)*
            ::core::result::Result::Ok(Self { #(#field_names),* })
        };

        (body, owned_body, quote! {})
    };

    Ok(quote! {
        impl ::edifact_rs::EdifactDeserialize for #name {
            fn edifact_deserialize(
                segments: &[::edifact_rs::Segment<'_>],
            ) -> ::core::result::Result<Self, ::edifact_rs::EdifactError> {
                #body
            }

            fn edifact_deserialize_owned(
                segments: &[::edifact_rs::OwnedSegment],
            ) -> ::core::result::Result<Self, ::edifact_rs::EdifactError> {
                #owned_body
            }
        }
        #segment_tag_impl
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn trybuild_ui() {
        let t = trybuild::TestCases::new();
        t.pass("tests/ui/pass_*.rs");
        t.compile_fail("tests/ui/fail_*.rs");
    }
}
