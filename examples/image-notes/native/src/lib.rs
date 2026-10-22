//! Luma: a persistent image catalog expressed through Revenant operations.
mod cache;
mod catalog;
mod contracts;
mod error;
mod filesystem;
mod operations;
mod rendition;
mod scanning;
revenant::app!();
