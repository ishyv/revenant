//! Source-owned projections with explicit decimal and identity policies.
use crate::{Tokens, expand_result, sdk};
use proc_macro::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::{Data, DeriveInput, Fields, Type, parse_macro_input, parse_quote, spanned::Spanned};

pub(crate) fn derive(item: TokenStream) -> TokenStream {
    expand_result(expand(parse_macro_input!(item as DeriveInput)))
}

fn container(ty: &Type) -> Option<(&syn::Ident, &Type)> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if !matches!(segment.ident.to_string().as_str(), "Option" | "Vec" | "Box") {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let Some(syn::GenericArgument::Type(inner)) = arguments.args.first() else {
        return None;
    };
    Some((&segment.ident, inner))
}
fn wide(ty: &Type) -> Option<bool> {
    let Type::Path(path) = ty else {
        return None;
    };
    match path.path.segments.last()?.ident.to_string().as_str() {
        "u64" => Some(false),
        "i64" => Some(true),
        _ => None,
    }
}
fn wire_type(ty: &Type, decimal: bool, root: &Tokens) -> syn::Result<Type> {
    if matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|part| matches!(part.ident.to_string().as_str(), "usize" | "isize" | "u128" | "i128" | "PathBuf" | "SystemTime")))
    {
        return Err(syn::Error::new_spanned(
            ty,
            "unsupported domain field; use an explicit lossless adapter",
        ));
    }
    if let Some((name, inner)) = container(ty) {
        let inner = wire_type(inner, decimal, root)?;
        return Ok(parse_quote!(#name<#inner>));
    }
    if let Some(signed) = wide(ty) {
        if !decimal {
            return Err(syn::Error::new_spanned(
                ty,
                "wide integers require #[wire(decimal)] for lossless decimal strings",
            ));
        }
        return Ok(if signed {
            parse_quote!(#root::DecimalI64)
        } else {
            parse_quote!(#root::DecimalU64)
        });
    }
    if decimal {
        return Err(syn::Error::new_spanned(
            ty,
            "#[wire(decimal)] requires an i64/u64 field or container of them",
        ));
    }
    if !matches!(ty, Type::Path(_)) && !matches!(ty, Type::Tuple(tuple) if tuple.elems.is_empty()) {
        return Err(syn::Error::new_spanned(
            ty,
            "wire projections require owned models; borrowed, pointer, function and tuple fields need an explicit adapter",
        ));
    }
    syn::parse2(quote_spanned!(ty.span()=> <#ty as #root::WireProjection>::Wire))
}
fn conversion(ty: &Type, decimal: bool, root: &Tokens, value: Tokens, reverse: bool) -> Tokens {
    if let Some((name, inner)) = container(ty) {
        let converted = conversion(inner, decimal, root, quote!(value), reverse);
        return match name.to_string().as_str() {
            "Option" => quote!((#value).map(|value| #converted)),
            "Vec" => quote!((#value).into_iter().map(|value| #converted).collect()),
            _ => quote!(Box::new({ let value = *(#value); #converted })),
        };
    }
    if decimal {
        if reverse {
            quote!((#value).get())
        } else {
            quote!((#value).into())
        }
    } else if reverse {
        quote_spanned!(ty.span()=> <#ty as #root::WireProjection>::from_wire(#value))
    } else {
        quote_spanned!(ty.span()=> <#ty as #root::WireProjection>::into_wire(#value))
    }
}
fn expand(input: DeriveInput) -> syn::Result<Tokens> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            input.generics,
            "projection generics need an explicit adapter",
        ));
    }
    let root = sdk();
    let name = &input.ident;
    let mut wire_name = format_ident!("{}Wire", name);
    let mut nominal = false;
    for attr in input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("wire"))
    {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("id") {
                nominal = true;
                Ok(())
            } else if meta.path.is_ident("name") {
                let value: syn::LitStr = meta.value()?.parse()?;
                wire_name = syn::parse_str(&value.value())
                    .map_err(|_| meta.error("name must be a Rust identifier"))?;
                Ok(())
            } else {
                Err(meta.error("expected id or name"))
            }
        })?;
    }
    let Data::Struct(source) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "WireProjection currently supports named models and single-field nominal IDs; enums require explicit adapters",
        ));
    };
    let mut mirror = input.clone();
    mirror.ident = wire_name.clone();
    mirror.attrs.retain(|attr| attr.path().is_ident("doc"));
    mirror.attrs.push(parse_quote!(#[derive(#root::Contract)]));
    let Data::Struct(target) = &mut mirror.data else {
        unreachable!()
    };
    if nominal {
        let Fields::Unnamed(fields) = &source.fields else {
            return Err(syn::Error::new_spanned(
                &source.fields,
                "#[wire(id)] requires a single-field String newtype",
            ));
        };
        if fields.unnamed.len() != 1
            || !matches!(&fields.unnamed[0].ty, Type::Path(path) if path.path.segments.last().is_some_and(|part| part.ident == "String"))
        {
            return Err(syn::Error::new_spanned(
                fields,
                "#[wire(id)] requires a single-field String newtype",
            ));
        }
        mirror.attrs.push(parse_quote!(#[serde(transparent)]));
        return Ok(quote! {
            #mirror
            impl #root::WireProjection for #name {
                type Wire = #wire_name;
                fn into_wire(self) -> Self::Wire { #wire_name(self.0) }
                fn from_wire(wire: Self::Wire) -> Self { Self(wire.0) }
            }
            impl From<#name> for #wire_name { fn from(value: #name) -> Self { #root::WireProjection::into_wire(value) } }
            impl From<#wire_name> for #name { fn from(value: #wire_name) -> Self { #root::WireProjection::from_wire(value) } }
        });
    }
    let Fields::Named(_) = source.fields else {
        return Err(syn::Error::new_spanned(
            &source.fields,
            "WireProjection requires named fields or #[wire(id)]",
        ));
    };
    mirror
        .attrs
        .push(parse_quote!(#[serde(rename_all = "camelCase", deny_unknown_fields)]));
    let mut forward = Vec::new();
    let mut reverse = Vec::new();
    for (field, target) in source.fields.iter().zip(target.fields.iter_mut()) {
        let member = field.ident.as_ref().unwrap();
        let mut decimal = false;
        let mut identity = false;
        let mut rename: Option<syn::LitStr> = None;
        for attr in field
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("wire"))
        {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("decimal") {
                    decimal = true;
                    Ok(())
                } else if meta.path.is_ident("contract") {
                    identity = true;
                    Ok(())
                } else if meta.path.is_ident("rename") {
                    rename = Some(meta.value()?.parse()?);
                    Ok(())
                } else {
                    Err(meta.error("expected decimal, contract, or rename"))
                }
            })?;
        }
        if identity && decimal {
            return Err(syn::Error::new_spanned(
                field,
                "contract and decimal projection policies conflict",
            ));
        }
        target.attrs.retain(|attr| attr.path().is_ident("doc"));
        target.vis = parse_quote!(pub);
        if let Some(rename) = rename {
            target.attrs.push(parse_quote!(#[serde(rename = #rename)]));
        }
        target.ty = if identity {
            field.ty.clone()
        } else {
            wire_type(&field.ty, decimal, &root)?
        };
        let to = if identity {
            quote!(self.#member)
        } else {
            conversion(&field.ty, decimal, &root, quote!(self.#member), false)
        };
        let from = if identity {
            quote!(wire.#member)
        } else {
            conversion(&field.ty, decimal, &root, quote!(wire.#member), true)
        };
        forward.push(quote!(#member: #to));
        reverse.push(quote!(#member: #from));
    }
    Ok(quote! {
        #mirror
        impl #root::WireProjection for #name {
            type Wire = #wire_name;
            fn into_wire(self) -> Self::Wire { #wire_name { #(#forward),* } }
            fn from_wire(wire: Self::Wire) -> Self { Self { #(#reverse),* } }
        }
        impl From<#name> for #wire_name { fn from(value: #name) -> Self { #root::WireProjection::into_wire(value) } }
        impl From<#wire_name> for #name { fn from(value: #wire_name) -> Self { #root::WireProjection::from_wire(value) } }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsafe_fields_and_implicit_wide_integer_conversions_report_source_errors() {
        for (source, expected) in [
            ("struct Bad { count: u64 }", "wide integers require"),
            (
                "struct Bad { count: Option<Vec<i64>> }",
                "wide integers require",
            ),
            ("struct Bad { pointer: *const u8 }", "owned models"),
            ("struct Bad { count: usize }", "explicit lossless adapter"),
            ("struct Bad { value: &'static str }", "owned models"),
            (
                "struct Bad { #[wire(decimal)] count: u32 }",
                "requires an i64/u64",
            ),
            ("#[wire(id)] struct Bad(u64);", "single-field String"),
        ] {
            let error = expand(syn::parse_str(source).unwrap()).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
