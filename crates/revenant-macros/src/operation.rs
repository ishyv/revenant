//! Operation signature checking, source metadata, and composition expansion. Expansion retains the source async function and emits a hidden registration factory consumed by operations!.
use crate::{Tokens, expand_result, sdk};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, ReturnType, Type, parse_macro_input};
/// Exposes an ordinary async Rust function as a typed application operation.
///
/// `id = "group.name"` determines the generated Svelte capability path. Rustdoc
/// becomes the operation's hover documentation. Input and output must implement
/// `Contract`; an optional second argument receives the owned `TaskContext`.
pub(crate) fn attribute(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    expand_result(expand_operation(attr.into(), function))
}

/// Composes operations using their source function paths.
///
/// Pass this to `Application::operations`, for example
/// `revenant::operations![records::normalize]`. Registration helpers remain
/// implementation details; invalid function paths fail during Rust compilation.
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
fn expand_operation(attr: Tokens, function: ItemFn) -> syn::Result<Tokens> {
    let mut id: Option<syn::LitStr> = None;
    let mut description: Option<syn::LitStr> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("id") {
            id = Some(meta.value()?.parse()?);
            Ok(())
        } else if meta.path.is_ident("description") {
            description = Some(meta.value()?.parse()?);
            Ok(())
        } else {
            Err(meta.error("expected id or description"))
        }
    });
    syn::parse::Parser::parse2(parser, attr)?;
    let id = id
        .ok_or_else(|| syn::Error::new_spanned(&function.sig, "operation requires id = \"...\""))?;
    if id.value().is_empty() {
        return Err(syn::Error::new_spanned(id, "operation id cannot be empty"));
    }
    let sig = &function.sig;
    if sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(sig, "operation must be async"));
    }
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &sig.generics,
            "operation functions cannot be generic",
        ));
    }
    if sig.unsafety.is_some() || sig.abi.is_some() || sig.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            sig,
            "operation must be a safe Rust async function",
        ));
    }
    if !matches!(sig.inputs.len(), 1 | 2) {
        return Err(syn::Error::new_spanned(
            sig,
            "operation expects input: Input, optionally followed by ctx: TaskContext",
        ));
    }
    let input = match &sig.inputs[0] {
        FnArg::Typed(arg) => &arg.ty,
        _ => {
            return Err(syn::Error::new_spanned(
                sig,
                "operation cannot have a receiver",
            ));
        }
    };
    if let Some(argument) = sig.inputs.get(1) {
        let ctx = match argument {
            FnArg::Typed(arg) => &arg.ty,
            _ => {
                return Err(syn::Error::new_spanned(
                    argument,
                    "operation cannot have a receiver",
                ));
            }
        };
        if !matches!(&**ctx, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "TaskContext"))
        {
            return Err(syn::Error::new_spanned(
                ctx,
                "second argument must be TaskContext",
            ));
        }
    }
    let output = match &sig.output {
        ReturnType::Type(_, ty) => match &**ty {
            Type::Path(path)
                if path
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "Result") =>
            {
                match &path.path.segments.last().unwrap().arguments {
                    syn::PathArguments::AngleBracketed(args) => match args.args.first() {
                        Some(syn::GenericArgument::Type(ty)) => ty.clone(),
                        _ => return Err(syn::Error::new_spanned(ty, "expected Result<Output>")),
                    },
                    _ => return Err(syn::Error::new_spanned(ty, "expected Result<Output>")),
                }
            }
            _ => return Err(syn::Error::new_spanned(ty, "expected Result<Output>")),
        },
        _ => {
            return Err(syn::Error::new_spanned(
                sig,
                "operation requires Result<Output>",
            ));
        }
    };
    let doc = function
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| {
            if let syn::Meta::NameValue(value) = &a.meta {
                if let syn::Expr::Lit(lit) = &value.value {
                    if let syn::Lit::Str(s) = &lit.lit {
                        return Some(s.value().trim().to_owned());
                    }
                }
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
    let visibility = &function.vis;
    let callable = if sig.inputs.len() == 2 {
        quote!(#name)
    } else {
        quote!(|input, _context| #name(input))
    };
    Ok(quote! {
        #function
        #[doc(hidden)]
        #visibility fn #descriptor() -> #root::Result<#root::OperationRegistration> {
            #root::OperationRegistration::new::<#input, #output, _, _>(#id, #description, #callable)
                .map(|mut registration| {
                    registration.descriptor.source = Some(#root::OperationSource {
                        file: file!().into(), line: line!(), module: module_path!().into(),
                    });
                    registration
                })
        }
    })
}
