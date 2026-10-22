//! Operation signature checking, source metadata, and linker discovery expansion.
use crate::{Tokens, expand_result, sdk};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Pat, ReturnType, Type, parse_macro_input};

pub(crate) fn attribute(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    expand_result(expand_operation(attr.into(), function))
}

pub(crate) fn app(input: TokenStream) -> TokenStream {
    let root = sdk();
    let base = if input.is_empty() {
        quote!(#root::Application::new())
    } else {
        let expression = parse_macro_input!(input as syn::Expr);
        quote!(#expression)
    };
    quote! {
        /// Compiles this crate's annotated operations and configured native capabilities.
        pub fn app() -> #root::Application {
            (#base).discover(env!("CARGO_PKG_NAME"), module_path!())
        }
    }
    .into()
}

pub(crate) fn list(input: TokenStream) -> TokenStream {
    let paths = parse_macro_input!(input with syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated);
    let root = sdk();
    let mut factories = Vec::new();
    for mut path in paths {
        let Some(last) = path.segments.last_mut() else {
            continue;
        };
        if !matches!(last.arguments, syn::PathArguments::None) {
            return syn::Error::new_spanned(path, "operation paths cannot have generic arguments")
                .to_compile_error()
                .into();
        }
        last.ident = format_ident!("{}_operation", last.ident);
        factories.push(quote!(#path as #root::OperationFactory));
    }
    quote!(::std::vec![#(#factories),*]).into()
}

fn camel_case(name: &str) -> String {
    let mut output = String::new();
    let mut upper = false;
    for character in name.trim_start_matches("r#").chars() {
        if character == '_' {
            upper = !output.is_empty();
        } else if output.is_empty() {
            output.push(character.to_ascii_lowercase());
        } else if upper {
            output.push(character.to_ascii_uppercase());
            upper = false;
        } else {
            output.push(character);
        }
    }
    output
}

fn expand_operation(attr: Tokens, function: ItemFn) -> syn::Result<Tokens> {
    let mut id: Option<syn::LitStr> = None;
    let mut description: Option<syn::LitStr> = None;
    let mut effect: Option<syn::LitStr> = None;
    let mut independent = false;
    let mut agent = false;
    let mut legacy_input = false;
    let mut updates: Option<Type> = None;
    let mut work: Option<syn::LitStr> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("id") {
            if id.is_some() {
                return Err(meta.error("duplicate operation id"));
            }
            id = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("description") {
            if description.is_some() {
                return Err(meta.error("duplicate description"));
            }
            description = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("read_only") {
            if effect.is_some() {
                return Err(meta.error("conflicting effect declarations"));
            }
            effect = Some(syn::LitStr::new(
                "readOnly",
                meta.path.segments[0].ident.span(),
            ));
        } else if meta.path.is_ident("effect") {
            if effect.is_some() {
                return Err(meta.error("conflicting effect declarations"));
            }
            let value: syn::LitStr = meta.value()?.parse()?;
            if !matches!(value.value().as_str(), "unknown" | "readOnly" | "write") {
                return Err(syn::Error::new_spanned(
                    value,
                    "effect must be unknown, readOnly, or write",
                ));
            }
            effect = Some(value);
        } else if meta.path.is_ident("independent") {
            if independent {
                return Err(meta.error("duplicate independent declaration"));
            }
            independent = true;
        } else if meta.path.is_ident("agent") {
            if agent {
                return Err(meta.error("duplicate agent declaration"));
            }
            agent = true;
        } else if meta.path.is_ident("legacy_input") {
            if legacy_input {
                return Err(meta.error("duplicate legacy_input declaration"));
            }
            legacy_input = true;
        } else if meta.path.is_ident("updates") {
            if updates.is_some() {
                return Err(meta.error("duplicate updates declaration"));
            }
            updates = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("work") {
            if work.is_some() {
                return Err(meta.error("duplicate work declaration"));
            }
            let value: syn::LitStr = meta.value()?.parse()?;
            if !matches!(value.value().as_str(), "cpu" | "io" | "interactive") {
                return Err(syn::Error::new_spanned(
                    value,
                    "work must be cpu, io, or interactive",
                ));
            }
            work = Some(value);
        } else {
            return Err(meta.error(
                "expected id, description, read_only, effect, independent, agent, legacy_input, updates, or work",
            ));
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, attr)?;
    if work
        .as_ref()
        .is_some_and(|value| value.value() == "interactive")
        && !effect
            .as_ref()
            .is_some_and(|value| value.value() == "readOnly")
    {
        return Err(syn::Error::new_spanned(
            work.as_ref().unwrap(),
            "interactive work requires read_only",
        ));
    }
    if let Some(id) = &id
        && (id.value().is_empty() || id.value().chars().any(char::is_whitespace))
    {
        return Err(syn::Error::new_spanned(
            id,
            "operation id cannot be empty or contain whitespace",
        ));
    }
    let sig = &function.sig;
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &sig.generics,
            "operation functions cannot be generic",
        ));
    }
    if sig.unsafety.is_some() || sig.abi.is_some() || sig.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            sig,
            "operation must be a safe Rust function",
        ));
    }
    let mut arguments = Vec::new();
    let mut has_context = false;
    for (index, argument) in sig.inputs.iter().enumerate() {
        let FnArg::Typed(argument) = argument else {
            return Err(syn::Error::new_spanned(
                argument,
                "operation cannot have a receiver",
            ));
        };
        let Pat::Ident(pattern) = &*argument.pat else {
            return Err(syn::Error::new_spanned(
                &argument.pat,
                "operation arguments must be named identifiers",
            ));
        };
        if pattern.by_ref.is_some() || pattern.subpat.is_some() {
            return Err(syn::Error::new_spanned(
                pattern,
                "operation arguments must be owned named values",
            ));
        }
        let is_context = matches!(&*argument.ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "TaskContext"));
        if is_context {
            if index + 1 != sig.inputs.len() {
                return Err(syn::Error::new_spanned(
                    argument,
                    "TaskContext must be the final argument",
                ));
            }
            has_context = true;
        } else {
            if matches!(
                &*argument.ty,
                Type::Reference(_) | Type::Ptr(_) | Type::ImplTrait(_) | Type::TraitObject(_)
            ) {
                return Err(syn::Error::new_spanned(
                    &argument.ty,
                    "operation arguments must be owned contract types",
                ));
            }
            arguments.push((pattern.ident.clone(), argument.ty.clone()));
        }
    }
    if legacy_input && arguments.len() != 1 {
        return Err(syn::Error::new_spanned(
            sig,
            "legacy_input requires exactly one input argument",
        ));
    }
    let (output, fallible) = match &sig.output {
        ReturnType::Default => (syn::parse_quote!(()), false),
        ReturnType::Type(_, ty) => {
            if let Type::Path(path) = &**ty {
                if let Some(segment) = path
                    .path
                    .segments
                    .last()
                    .filter(|segment| segment.ident == "Result")
                {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(output)) = args.args.first() {
                            (output.clone(), true)
                        } else {
                            return Err(syn::Error::new_spanned(ty, "expected Result<Output>"));
                        }
                    } else {
                        return Err(syn::Error::new_spanned(ty, "expected Result<Output>"));
                    }
                } else {
                    ((**ty).clone(), false)
                }
            } else {
                ((**ty).clone(), false)
            }
        }
    };
    let doc = function
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| {
            if let syn::Meta::NameValue(value) = &attr.meta
                && let syn::Expr::Lit(lit) = &value.value
                && let syn::Lit::Str(text) = &lit.lit
            {
                return Some(text.value().trim().to_owned());
            }
            None
        })
        .collect::<Vec<_>>()
        .join("\n");
    let description =
        description.unwrap_or_else(|| syn::LitStr::new(&doc, proc_macro2::Span::call_site()));
    let root = sdk();
    let name = &sig.ident;
    let descriptor = format_ident!("{}_operation", name);
    let input_name = format_ident!(
        "__Revenant{}Input",
        name.to_string().trim_start_matches("r#")
    );
    let discovery = format_ident!(
        "__REVENANT_DISCOVER_{}",
        name.to_string().trim_start_matches("r#")
    );
    let visibility = &function.vis;
    let cfg: Vec<_> = function
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"))
        .collect();
    let fields: Vec<_> = arguments.iter().map(|(name, _)| name).collect();
    let types: Vec<_> = arguments.iter().map(|(_, ty)| ty).collect();
    let wire_names: Vec<_> = if legacy_input {
        Vec::new()
    } else {
        fields
            .iter()
            .map(|field| camel_case(&field.to_string()))
            .collect()
    };
    let mut unique_names = std::collections::BTreeSet::new();
    for (name, field) in wire_names.iter().zip(&fields) {
        if !unique_names.insert(name) {
            return Err(syn::Error::new_spanned(
                field,
                "colliding wire argument names after camelCase conversion",
            ));
        }
    }
    let input_fields = fields.iter().zip(&types).map(|(field, ty)| {
        // One spelling feeds descriptor metadata and all contract derivations.
        // Explicit names also avoid rename_all's byte slicing of Unicode identifiers.
        let wire_name = camel_case(&field.to_string());
        let optional = matches!(&***ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Option"));
        if optional {
            quote!(#[serde(rename = #wire_name, default, skip_serializing_if = "Option::is_none")] #field: #ty)
        } else {
            quote!(#[serde(rename = #wire_name)] #field: #ty)
        }
    });
    let (input_type, input_definition, mut call_args) = if legacy_input {
        let ty = &types[0];
        (quote!(#ty), quote!(), vec![quote!(__revenant_input)])
    } else {
        (
            quote!(#input_name),
            quote! {
                #(#cfg)*
                #[doc(hidden)]
                #[allow(non_camel_case_types)]
                #[derive(#root::Contract)]
                #[serde(deny_unknown_fields)]
                struct #input_name { #(#input_fields),* }
            },
            fields
                .iter()
                .map(|field| quote!(__revenant_input.#field))
                .collect(),
        )
    };
    if has_context {
        call_args.push(quote!(__revenant_context));
    }
    let call = if sig.asyncness.is_some() {
        quote!(#name(#(#call_args),*).await)
    } else {
        quote!(#name(#(#call_args),*))
    };
    let result = if fallible {
        quote!(#call)
    } else {
        quote!(Ok(#call))
    };
    let id = match id {
        Some(id) => quote!(#id),
        None => quote!(#root::inferred_operation_id(module_path!(), stringify!(#name))),
    };
    let effect = match effect.as_ref().map(|effect| effect.value()).as_deref() {
        Some("readOnly") => quote!(#root::OperationEffect::ReadOnly),
        Some("write") => quote!(#root::OperationEffect::Write),
        _ => quote!(#root::OperationEffect::Unknown),
    };
    let update_registration =
        updates.map(|ty| quote!(.and_then(|registration| registration.with_updates::<#ty>())));
    let work_class = match work.as_ref().map(|value| value.value()).as_deref() {
        Some("io") => quote!(#root::WorkClass::Io),
        Some("interactive") => quote!(#root::WorkClass::Interactive),
        _ => quote!(#root::WorkClass::Cpu),
    };
    Ok(quote! {
        #function
        #input_definition
        #(#cfg)*
        #[doc(hidden)]
        #visibility fn #descriptor() -> #root::Result<#root::OperationRegistration> {
            #root::OperationRegistration::new::<#input_type, #output, _, _>(
                #id, #description,
                |__revenant_input: #input_type, __revenant_context| async move { #result },
            )#update_registration.map(|mut registration| {
                registration.descriptor.arguments = vec![#(::std::string::String::from(#wire_names)),*];
                registration.descriptor.effect = #effect;
                registration.descriptor.work_class = #work_class;
                registration.descriptor.independent = #independent;
                registration.descriptor.agent = #agent;
                registration.descriptor.source = Some(#root::OperationSource {
                    file: file!().into(), line: line!(), module: module_path!().into(),
                });
                registration
            })
        }
        #(#cfg)*
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        #[#root::linkme::distributed_slice(#root::OPERATION_DISCOVERY)]
        #[linkme(crate = #root::linkme)]
        static #discovery: #root::OperationDiscovery = #root::OperationDiscovery {
            package: env!("CARGO_PKG_NAME"),
            module: module_path!(),
            source: concat!(module_path!(), "::", stringify!(#name)),
            factory: #descriptor,
        };
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_work_classes_reject_interactive_mutations_and_unknown_values() {
        for attributes in [
            quote!(work = "interactive"),
            quote!(effect = "write", work = "interactive"),
            quote!(work = "unbounded"),
            quote!(work = "io", work = "cpu"),
        ] {
            assert!(
                expand_operation(
                    attributes,
                    syn::parse_quote!(
                        fn bad() {}
                    )
                )
                .is_err()
            );
        }
        let result = expand_operation(
            quote!(read_only, work = "interactive"),
            syn::parse_quote!(
                fn read() -> u32 {
                    1
                }
            ),
        )
        .unwrap()
        .to_string();
        assert!(result.contains("WorkClass :: Interactive"));
    }

    #[test]
    fn update_contract_is_compiled_into_the_registration() {
        let expanded = expand_operation(
            quote!(updates = ScanUpdate),
            syn::parse_quote!(
                fn scan(context: TaskContext) -> u32 {
                    0
                }
            ),
        )
        .expect("typed update metadata is supported");
        assert!(
            expanded
                .to_string()
                .contains("with_updates :: < ScanUpdate >")
        );
    }

    #[test]
    fn wire_argument_names_match_camel_case_for_uppercase_identifiers() {
        assert_eq!(camel_case("URL"), "uRL");
        assert_eq!(camel_case("display_name"), "displayName");
        assert_eq!(camel_case("r#type"), "type");
    }

    #[test]
    fn named_arguments_reject_colliding_wire_names() {
        let function = syn::parse_quote! {
            fn collide(item_name: String, itemName: String) -> String { item_name + &itemName }
        };
        let error = expand_operation(quote!(), function).unwrap_err();
        assert!(error.to_string().contains("colliding wire argument"));
    }

    #[test]
    fn operation_rejects_unsafe_borrowed_generic_and_unnamed_signatures() {
        for function in [
            syn::parse_quote!(
                unsafe fn bad(input: u32) -> u32 {
                    input
                }
            ),
            syn::parse_quote!(
                fn bad(input: &str) -> String {
                    input.into()
                }
            ),
            syn::parse_quote!(
                fn bad<T>(input: T) -> T {
                    input
                }
            ),
            syn::parse_quote!(
                fn bad((left, right): (u32, u32)) -> u32 {
                    left + right
                }
            ),
            syn::parse_quote!(
                fn bad(context: TaskContext, input: u32) -> u32 {
                    input
                }
            ),
        ] {
            assert!(expand_operation(quote!(), function).is_err());
        }
    }

    #[test]
    fn legacy_input_and_effect_declarations_reject_ambiguous_configuration() {
        for (attributes, function) in [
            (
                quote!(legacy_input),
                syn::parse_quote!(
                    fn bad() -> u32 {
                        0
                    }
                ),
            ),
            (
                quote!(legacy_input),
                syn::parse_quote!(
                    fn bad(left: u32, right: u32) -> u32 {
                        left + right
                    }
                ),
            ),
            (
                quote!(read_only, effect = "write"),
                syn::parse_quote!(
                    fn bad() {}
                ),
            ),
            (
                quote!(effect = "parallel"),
                syn::parse_quote!(
                    fn bad() {}
                ),
            ),
            (
                quote!(id = " "),
                syn::parse_quote!(
                    fn bad() {}
                ),
            ),
        ] {
            assert!(expand_operation(attributes, function).is_err());
        }
    }
}
