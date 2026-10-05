//! Desktop commands and their author-owned/generated file boundaries.
//!
//! `new` writes a minimal app, `dev` owns Vite and native process trees, `build`
//! publishes compiled contracts before packaging, and `setup` only diagnoses.
pub mod build;
pub mod dev;
pub mod new;
pub mod setup;
