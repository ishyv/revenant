//! Compiled native authoring macros. Public entry points remain at crate root as
//! required by Rust; signature expansion and wire-safety auditing are private modules.
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
mod contract;
mod operation;
mod safety;

fn sdk() -> Tokens {
    match proc_macro_crate::crate_name("revenant-sdk") {
        Ok(proc_macro_crate::FoundCrate::Name(name)) => {
            let name = format_ident!("{}", name);
            quote!(::#name)
        }
        _ => quote!(::revenant_sdk),
    }
}
fn expand_result(result: syn::Result<Tokens>) -> TokenStream {
    result.unwrap_or_else(|e| e.to_compile_error()).into()
}

/// Exposes an ordinary async Rust function as a typed application operation.
///
/// `id = "group.name"` selects its generated capability path. Rustdoc supplies
/// hover documentation unless `description` overrides it. Input and output must
/// satisfy the compiled contract; an optional second argument is TaskContext.
/// The function remains callable as Rust, and operations! consumes a hidden factory.
/// Invalid signatures or attributes produce a compile error; registration can fail
/// at application validation if IDs or wire contracts conflict.
#[proc_macro_attribute]
pub fn operation(attr: TokenStream, item: TokenStream) -> TokenStream {
    operation::attribute(attr, item)
}

/// Composes documented operations using their original source function paths.
///
/// Supply this list to Application::operations, for example
/// `revenant_sdk::operations![records::normalize]`. Paths must name operation
/// functions without generic arguments. Hidden registration factories are resolved
/// at compile time; factories are called later by application validation.
#[proc_macro]
pub fn operations(input: TokenStream) -> TokenStream {
    operation::list(input)
}

/// Adds Serde, JSON Schema, TypeScript, and recursive handle admission to a type.
///
/// Use on an owned struct or enum. The attribute takes no arguments and supports
/// a restricted symmetric set of Serde attributes. Wide integers require decimal
/// wrappers; independent TypeScript/schema overrides, unions, borrowed generics,
/// and custom serialization hooks are rejected. Source documentation is retained.
#[proc_macro_attribute]
pub fn contract(attr: TokenStream, item: TokenStream) -> TokenStream {
    contract::attribute(attr, item)
}

/// Derives an audited wire contract without requiring the source type to be Clone.
///
/// Emits owned/deserialization and borrowed/serialization mirrors, schema and
/// TypeScript implementations, and nested-handle visitation. Mirrors are hidden
/// from ordinary Rustdoc. Use the same owned struct/enum and symmetric Serde
/// restrictions as the contract attribute; invalid representations fail compilation.
#[proc_macro_derive(Contract, attributes(serde))]
pub fn derive_contract(item: TokenStream) -> TokenStream {
    contract::derive(item)
}
