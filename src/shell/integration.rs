//! Functions that emit data for the wrapping shell function.

use std::path::Path;

/// Print the selected directory to stdout so the shell wrapper can `cd` to it.
pub fn print_selected_dir(path: &Path) {
    // We intentionally use `print!` (not `println!`) to avoid a trailing
    // newline that might confuse some shell wrappers.  The bash function
    // below handles both forms.
    println!("{}", path.display());
}

/// Returns the bash function that users should add to their `.bashrc`.
///
/// The function name is `dt` and it invokes the binary by its package name
/// (read from `Cargo.toml` at compile time).
pub fn bash_function() -> String {
    let bin = env!("CARGO_PKG_NAME");
    format!(
        r#"
# ── {bin}: tree-based directory navigator ──────────────────
# Toggle with `dt`.  When you select a directory and press Enter,
# your shell cd's into it automatically.
dt() {{
    local dest
    dest="$(command {bin} "$@")"
    local exit_code=$?
    if [ $exit_code -eq 0 ] && [ -n "$dest" ] && [ -d "$dest" ]; then
        cd "$dest" || return
    fi
}}
"#
    )
}

/// Returns the zsh function that users should add to their `.zshrc`.
pub fn zsh_function() -> String {
    let bin = env!("CARGO_PKG_NAME");
    format!(
        r#"
# ── {bin}: tree-based directory navigator ──────────────────
# Toggle with `dt`.  When you select a directory and press Enter,
# your shell cd's into it automatically.
dt() {{
    local dest
    dest="$(command {bin} "$@")"
    local exit_code=$?
    if [[ $exit_code -eq 0 ]] && [[ -n "$dest" ]] && [[ -d "$dest" ]]; then
        cd "$dest"
    fi
}}
"#
    )
}

