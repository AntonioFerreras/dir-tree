//! Core algorithms – filesystem traversal, tree construction, and grouping.
//!
//! Nothing in this module depends on any TUI or rendering crate.
//! Every type is `Send + Sync` so it can be shared across async tasks.

pub mod fs;
pub mod grouping;
pub mod inspector;
pub mod size;
pub mod tree;

