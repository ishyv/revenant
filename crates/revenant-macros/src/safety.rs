//! Contract representation audit and recursive capability visitation. Supported Serde attributes must agree across input, output, TypeScript, and schema; recursive fields also rely on full schema validation.
use crate::Tokens;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, GenericParam, parse_quote};
pub(crate) fn fields(data: &Data) -> syn::Result<Vec<&syn::Field>> {
    match data {
        Data::Struct(s) => Ok(s.fields.iter().collect()),
        Data::Enum(e) => Ok(e.variants.iter().flat_map(|v| v.fields.iter()).collect()),
        Data::Union(u) => Err(syn::Error::new_spanned(
            u.union_token,
            "unions cannot be contracts",
        )),
    }
}
pub(crate) fn validate(input: &DeriveInput) -> syn::Result<()> {
    if input
        .generics
        .params
        .iter()
        .any(|p| !matches!(p, GenericParam::Type(_)))
    {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "contracts support owned type generics; lifetimes and const generics are unsupported",
        ));
    }
    let mut attributes: Vec<_> = input.attrs.iter().collect();
    for field in fields(&input.data)? {
        attributes.extend(&field.attrs);
    }
    if let Data::Enum(e) = &input.data {
        for variant in &e.variants {
            attributes.extend(&variant.attrs);
        }
    }
    for attribute in attributes {
        if attribute.path().is_ident("ts") || attribute.path().is_ident("schemars") {
            return Err(syn::Error::new_spanned(
                attribute,
                "independent TS/schema overrides are unsupported in contracts",
            ));
        }
        if attribute.path().is_ident("serde") {
            attribute.parse_nested_meta(|meta| {
                let allowed = [
                    "rename",
                    "rename_all",
                    "rename_all_fields",
                    "tag",
                    "content",
                    "untagged",
                    "transparent",
                    "deny_unknown_fields",
                ];
                if meta.path.is_ident("skip_serializing_if") {
                    let value: syn::LitStr = meta.value()?.parse()?;
                    if value.value() != "Option::is_none"
                        && value.value() != "std::option::Option::is_none"
                    {
                        return Err(meta.error("only Option::is_none omission is supported"));
                    }
                    return Ok(());
                }
                if meta.path.is_ident("default") {
                    if meta.input.peek(syn::Token![=]) {
                        return Err(meta.error("custom default functions are unsupported"));
                    }
                    return Ok(());
                }
                if !allowed.iter().any(|name| meta.path.is_ident(name)) {
                    return Err(meta.error("unsupported serialization attribute in contract"));
                }
                if meta.input.peek(syn::token::Paren) {
                    return Err(meta.error("asymmetric serialization names are unsupported"));
                }
                if meta.input.peek(syn::Token![=]) {
                    let _: syn::LitStr = meta.value()?.parse()?;
                }
                Ok(())
            })?;
        }
    }
    Ok(())
}
pub(crate) fn safe_impl(input: &DeriveInput, root: &Tokens) -> syn::Result<Tokens> {
    let name = &input.ident;
    let mut generics = input.generics.clone();
    for param in generics.type_params_mut() {
        param.bounds.push(parse_quote!(#root::Contract));
    }
    for field in fields(&input.data)? {
        let ty = &field.ty;
        let text = quote!(#ty).to_string();
        // Recursive fields are additionally audited by the complete generated schema.
        if !text
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|part| part == name.to_string() || part == "Self")
        {
            generics
                .make_where_clause()
                .predicates
                .push(parse_quote!(#ty: #root::WireSafe));
        }
    }
    let (ig, tg, wc) = generics.split_for_impl();
    let visit = handle_visitor(&input.data, name, root);
    Ok(quote!(impl #ig #root::WireSafe for #name #tg #wc {
        fn visit_handles(&self, visitor: &mut dyn FnMut(&#root::Handle) -> #root::Result<()>) -> #root::Result<()> {
            #visit
            Ok(())
        }
    }))
}
fn handle_visitor(data: &Data, name: &syn::Ident, root: &Tokens) -> Tokens {
    match data {
        Data::Struct(s) => {
            let members = s.fields.iter().enumerate().map(|(i, f)| {
                f.ident
                    .clone()
                    .map(syn::Member::Named)
                    .unwrap_or_else(|| syn::Member::Unnamed(i.into()))
            });
            quote!(#(#root::WireSafe::visit_handles(&self.#members, visitor)?;)*)
        }
        Data::Enum(e) => {
            let arms = e.variants.iter().map(|variant| {
                let id = &variant.ident;
                let bindings: Vec<_> = variant
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format_ident!("field_{i}"))
                    .collect();
                let pattern = match &variant.fields {
                    Fields::Named(fields) => {
                        let names = fields
                            .named
                            .iter()
                            .map(|field| field.ident.as_ref().unwrap());
                        quote!(#name::#id { #(#names: #bindings),* })
                    }
                    Fields::Unnamed(_) => quote!(#name::#id(#(#bindings),*)),
                    Fields::Unit => quote!(#name::#id),
                };
                quote!(#pattern => { #(#root::WireSafe::visit_handles(#bindings, visitor)?;)* })
            });
            quote!(match self { #(#arms),* })
        }
        Data::Union(_) => unreachable!(),
    }
}
pub(crate) fn optional_fields(data: &mut Data, add: bool) {
    let mark = |fields: &mut Fields| {
        for field in fields {
            if !add {
                field
                    .attrs
                    .retain(|attribute| !attribute.path().is_ident("ts"));
                continue;
            }
            let omitted = field
                .attrs
                .iter()
                .filter(|attribute| attribute.path().is_ident("serde"))
                .any(|attribute| {
                    let mut omitted = false;
                    let _ = attribute.parse_nested_meta(|meta| {
                        if meta.path.is_ident("skip_serializing_if") {
                            omitted = true;
                        }
                        if meta.input.peek(syn::Token![=]) {
                            let _: syn::Expr = meta.value()?.parse()?;
                        }
                        Ok(())
                    });
                    omitted
                });
            if omitted {
                field.attrs.push(parse_quote!(#[ts(optional)]));
            }
        }
    };
    match data {
        Data::Struct(s) => mark(&mut s.fields),
        Data::Enum(e) => {
            for variant in &mut e.variants {
                mark(&mut variant.fields);
            }
        }
        Data::Union(_) => {}
    }
}
