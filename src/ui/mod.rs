//! UI / rendering layer — everything that touches Ratatui widgets.
//!
//! This layer takes the *core* data structures and turns them into pixels on
//! the terminal.  No filesystem I/O happens here.

pub mod layout;
pub mod theme;
pub mod tree_widget;

