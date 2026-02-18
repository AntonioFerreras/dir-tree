//! UI / rendering layer — everything that touches Ratatui widgets.
//!
//! This layer takes the *core* data structures and turns them into pixels on
//! the terminal.  No filesystem I/O happens here.

pub mod inspector;
pub mod layout;
pub mod popup;
pub mod smooth_scroll;
pub mod spinner;
pub mod theme;
pub mod tree_widget;

