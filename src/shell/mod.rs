//! Shell integration helpers.
//!
//! The binary communicates with the calling shell through **stdout**.
//! All TUI rendering goes to the alternate screen (stderr-backed), so stdout
//! is reserved for the "result" — typically the selected directory path that
//! the shell wrapper will `cd` into.

pub mod integration;

