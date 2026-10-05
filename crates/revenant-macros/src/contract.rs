//! Contract attribute and derive expansion. Owned and borrowed wire mirrors preserve documentation and serialization without requiring application types to implement Clone.
use crate::safety::{fields, optional_fields, safe_impl, validate};
use crate::{Tokens, expand_result, sdk};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Type, parse_macro_input, parse_quote};
pub(crate) fn attribute(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "contract takes no arguments",
        )
        .to_compile_error()
        .into();
    }
    let mut input = parse_macro_input!(item as DeriveInput);
    expand_result((|| {
        validate(&input)?;
        let root = sdk();
        let safe = safe_impl(&input, &root)?;
        let root_string = quote!(#root).to_string().replace(' ', "");
        let serde_path = format!("{root_string}::serde");
        let schema_path = format!("{root_string}::schemars");
        let ts_path = format!("{root_string}::ts_rs");
        input.attrs.insert(0, parse_quote!(#[derive(#root::serde::Serialize, #root::serde::Deserialize, #root::schemars::JsonSchema, #root::ts_rs::TS)]));
        input
            .attrs
            .push(parse_quote!(#[serde(crate = #serde_path)]));
        input
            .attrs
            .push(parse_quote!(#[schemars(crate = #schema_path)]));
        input.attrs.push(parse_quote!(#[ts(crate = #ts_path)]));
        optional_fields(&mut input.data, true);
        Ok(quote!(#input #safe))
    })())
}

pub(crate) fn derive(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    expand_result(expand_contract(input))
}
fn expand_contract(input: DeriveInput) -> syn::Result<Tokens> {
    validate(&input)?;
    let root = sdk();
    let name = &input.ident;
    let wire_name = name.to_string();
    let owned_name = format_ident!("__Revenant{}Owned", name);
    let borrowed_name = format_ident!("__Revenant{}Borrowed", name);
    let root_string = quote!(#root).to_string().replace(' ', "");
    let serde_path = format!("{root_string}::serde");
    let schema_path = format!("{root_string}::schemars");
    let ts_path = format!("{root_string}::ts_rs");
    let mut owned = input.clone();
    owned.vis = input.vis.clone();
    owned.ident = owned_name.clone();
    owned
        .attrs
        .retain(|a| a.path().is_ident("serde") || a.path().is_ident("doc"));
    owned.attrs.insert(0, parse_quote!(#[derive(#root::serde::Deserialize, #root::schemars::JsonSchema, #root::ts_rs::TS)]));
    owned
        .attrs
        .push(parse_quote!(#[serde(crate = #serde_path)]));
    owned
        .attrs
        .push(parse_quote!(#[schemars(crate = #schema_path, rename = #wire_name)]));
    owned
        .attrs
        .push(parse_quote!(#[ts(crate = #ts_path, rename = #wire_name)]));
    optional_fields(&mut owned.data, true);
    struct ReplaceSelf(Tokens);
    impl syn::visit_mut::VisitMut for ReplaceSelf {
        fn visit_type_mut(&mut self, ty: &mut Type) {
            if matches!(ty, Type::Path(p) if p.path.is_ident("Self")) {
                *ty = syn::parse2(self.0.clone()).unwrap();
            } else {
                syn::visit_mut::visit_type_mut(self, ty);
            }
        }
    }
    let (_, original_tg, _) = input.generics.split_for_impl();
    syn::visit_mut::VisitMut::visit_derive_input_mut(
        &mut ReplaceSelf(quote!(#name #original_tg)),
        &mut owned,
    );
    let mut borrowed = owned.clone();
    borrowed.vis = syn::Visibility::Inherited;
    borrowed.ident = borrowed_name.clone();
    borrowed.attrs.retain(|a| a.path().is_ident("serde"));
    borrowed
        .attrs
        .insert(0, parse_quote!(#[derive(#root::serde::Serialize)]));
    optional_fields(&mut borrowed.data, false);
    borrowed
        .generics
        .params
        .insert(0, parse_quote!('__revenant_wire));
    fn borrow_fields(fields: &mut Fields) {
        for field in fields {
            let ty = field.ty.clone();
            field.ty = parse_quote!(&'__revenant_wire #ty);
        }
    }
    match &mut borrowed.data {
        Data::Struct(s) => borrow_fields(&mut s.fields),
        Data::Enum(e) => {
            for variant in &mut e.variants {
                borrow_fields(&mut variant.fields);
            }
        }
        Data::Union(_) => unreachable!(),
    }
    if fields(&input.data)?.is_empty() {
        borrowed.generics.params = borrowed.generics.params.into_iter().skip(1).collect();
    }
    let (serialize_value, deserialize_value) =
        conversions(&input.data, name, &owned_name, &borrowed_name);
    let mut generics = input.generics.clone();
    for param in generics.type_params_mut() {
        param.bounds.push(parse_quote!(#root::Contract));
    }
    let (ig, tg, wc) = generics.split_for_impl();
    let mut deser_generics = generics.clone();
    deser_generics
        .params
        .insert(0, parse_quote!('__revenant_de));
    let (dig, _, dwc) = deser_generics.split_for_impl();
    let safe = safe_impl(&input, &root)?;
    Ok(quote! {
        #[doc(hidden)] #owned
        #[doc(hidden)] #borrowed
        impl #ig #root::serde::Serialize for #name #tg #wc {
            fn serialize<S: #root::serde::Serializer>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error> {
                #root::serde::Serialize::serialize(&(#serialize_value), serializer)
            }
        }
        impl #dig #root::serde::Deserialize<'__revenant_de> for #name #tg #dwc {
            fn deserialize<D: #root::serde::Deserializer<'__revenant_de>>(deserializer: D) -> ::std::result::Result<Self, D::Error> {
                let wire: #owned_name #tg = #root::serde::Deserialize::deserialize(deserializer)?;
                Ok(#deserialize_value)
            }
        }
        impl #ig #root::schemars::JsonSchema for #name #tg #wc {
            fn schema_name() -> ::std::borrow::Cow<'static, str> { <#owned_name #tg as #root::schemars::JsonSchema>::schema_name() }
            fn schema_id() -> ::std::borrow::Cow<'static, str> { <#owned_name #tg as #root::schemars::JsonSchema>::schema_id() }
            fn json_schema(generator: &mut #root::schemars::SchemaGenerator) -> #root::schemars::Schema {
                <#owned_name #tg as #root::schemars::JsonSchema>::json_schema(generator)
            }
        }
        impl #ig #root::ts_rs::TS for #name #tg #wc {
            type WithoutGenerics = <#owned_name #tg as #root::ts_rs::TS>::WithoutGenerics;
            type OptionInnerType = Self;
            fn name(cfg: &#root::ts_rs::Config) -> String { <#owned_name #tg as #root::ts_rs::TS>::name(cfg) }
            fn ident(cfg: &#root::ts_rs::Config) -> String { <#owned_name #tg as #root::ts_rs::TS>::ident(cfg) }
            fn inline(cfg: &#root::ts_rs::Config) -> String { <#owned_name #tg as #root::ts_rs::TS>::inline(cfg) }
            fn inline_flattened(cfg: &#root::ts_rs::Config) -> String { <#owned_name #tg as #root::ts_rs::TS>::inline_flattened(cfg) }
            fn decl(cfg: &#root::ts_rs::Config) -> String { <#owned_name #tg as #root::ts_rs::TS>::decl(cfg) }
            fn decl_concrete(cfg: &#root::ts_rs::Config) -> String { <#owned_name #tg as #root::ts_rs::TS>::decl_concrete(cfg) }
            fn output_path() -> Option<::std::path::PathBuf> { <#owned_name #tg as #root::ts_rs::TS>::output_path() }
            fn visit_dependencies(visitor: &mut impl #root::ts_rs::TypeVisitor) where Self: 'static { <#owned_name #tg as #root::ts_rs::TS>::visit_dependencies(visitor) }
            fn visit_generics(visitor: &mut impl #root::ts_rs::TypeVisitor) where Self: 'static { <#owned_name #tg as #root::ts_rs::TS>::visit_generics(visitor) }
        }
        #safe
    })
}
fn conversions(
    data: &Data,
    name: &syn::Ident,
    owned: &syn::Ident,
    borrowed: &syn::Ident,
) -> (Tokens, Tokens) {
    fn construct(fields: &Fields, target: Tokens, source: Tokens, borrowing: bool) -> Tokens {
        let values: Vec<_> = fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let member = field
                    .ident
                    .clone()
                    .map(syn::Member::Named)
                    .unwrap_or_else(|| syn::Member::Unnamed(index.into()));
                if borrowing {
                    quote!(&#source.#member)
                } else {
                    quote!(#source.#member)
                }
            })
            .collect();
        match fields {
            Fields::Named(f) => {
                let names = f.named.iter().map(|f| f.ident.as_ref().unwrap());
                quote!(#target { #(#names: #values),* })
            }
            Fields::Unnamed(_) => quote!(#target (#(#values),*)),
            Fields::Unit => quote!(#target),
        }
    }
    match data {
        Data::Struct(s) => (
            construct(&s.fields, quote!(#borrowed), quote!(self), true),
            construct(&s.fields, quote!(#name), quote!(wire), false),
        ),
        Data::Enum(e) => {
            let arms = |source: &syn::Ident, target: &syn::Ident| {
                e.variants.iter().map(|v| {
                let variant = &v.ident;
                let vars: Vec<_> = v.fields.iter().enumerate().map(|(i, _)| format_ident!("field_{i}")).collect();
                match &v.fields {
                    Fields::Named(f) => {
                        let names: Vec<_> = f.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
                        quote!(#source::#variant { #(#names: #vars),* } => #target::#variant { #(#names: #vars),* })
                    }
                    Fields::Unnamed(_) => quote!(#source::#variant(#(#vars),*) => #target::#variant(#(#vars),*)),
                    Fields::Unit => quote!(#source::#variant => #target::#variant),
                }
            }).collect::<Vec<_>>()
            };
            let serialize = arms(name, borrowed);
            let deserialize = arms(owned, name);
            (
                quote!(match self { #(#serialize),* }),
                quote!(match wire { #(#deserialize),* }),
            )
        }
        Data::Union(_) => unreachable!(),
    }
}
