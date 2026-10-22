//! Compiled native authoring macros. Public entry points remain at crate root as
//! required by Rust; signature expansion and wire-safety auditing are private modules.
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
mod contract;
mod operation;
mod projection;
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

/// Exposes an ordinary sync or async Rust function as a typed application operation.
///
/// `id = "group.name"` overrides the inferred module/function path. Rustdoc supplies
/// hover documentation unless `description` overrides it. Owned named arguments
/// generate the input record; an optional final argument is TaskContext. Return a
/// plain contract value or Revenant Result. `read_only`, `independent`, and `agent`
/// declare effects, batch scheduling, and application-agent exposure separately.
/// `work = "io"` selects blocking I/O; `work = "interactive"` reserves read
/// responsiveness and requires `read_only`. Undeclared work defaults to CPU.
/// `legacy_input` preserves a single argument's original wire shape for migration.
/// The function remains callable as Rust; app! discovers its hidden registration.
/// Invalid signatures or attributes produce a compile error; registration can fail
/// at application validation if IDs or wire contracts conflict.
#[proc_macro_attribute]
pub fn operation(attr: TokenStream, item: TokenStream) -> TokenStream {
    operation::attribute(attr, item)
}

/// Defines `pub fn app() -> Application` using this crate's annotated operations.
///
/// With no arguments, starts from Application::new(). Pass an Application builder
/// expression to configure capabilities or explicitly include dependency operations.
/// Linker discovery excludes other packages and other crates in the same package;
/// the compiled manifest checks duplicate operation IDs before native startup.
#[proc_macro]
pub fn app(input: TokenStream) -> TokenStream {
    operation::app(input)
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

/// Generates a lossless boundary model and reversible owned conversions.
/// Named source models generate `<Model>Wire` with ordinary contract metadata.
/// Wide integer fields require `#[wire(decimal)]`; `#[wire(id)]` preserves
/// nominal String newtypes. `#[wire(contract)]` retains explicit boundary types.
/// Unsupported shapes produce diagnostics on the source field.
#[proc_macro_derive(WireProjection, attributes(wire))]
pub fn derive_wire_projection(item: TokenStream) -> TokenStream {
    projection::derive(item)
}
