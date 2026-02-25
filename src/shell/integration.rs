//! Functions that emit data for the wrapping shell function.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const CD_PREFIX: &str = "__DT_CD__=";
const CLIP_PREFIX: &str = "__DT_CLIP__=";

/// Emit machine-readable exit payload for shell wrappers.
pub fn print_exit_payload(cd_dir: Option<&Path>, copied_path: Option<&Path>) {
    if let Some(path) = cd_dir {
        println!("{CD_PREFIX}{}", path.display());
    }
    if let Some(path) = copied_path {
        println!("{CLIP_PREFIX}{}", path.display());
    }
}

/// Attempt to copy `path` into the system clipboard.
pub fn copy_path_to_clipboard(path: &Path) -> bool {
    let text = path.display().to_string();

    #[cfg(target_os = "macos")]
    {
        return run_clip_command("pbcopy", &[], &text);
    }

    #[cfg(target_os = "windows")]
    {
        return run_clip_command("cmd", &["/C", "clip"], &text);
    }

    #[cfg(target_os = "linux")]
    {
        if run_clip_command("wl-copy", &[], &text) {
            return true;
        }
        return run_clip_command("xclip", &["-selection", "clipboard"], &text);
    }

    #[allow(unreachable_code)]
    false
}

fn run_clip_command(cmd: &str, args: &[&str], input: &str) -> bool {
    let mut child = match Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(input.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    }

    child.wait().map(|s| s.success()).unwrap_or(false)
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
# Toggle with `dt`.  Enter on a directory changes cwd, and copy actions
# print a clipboard notice after the TUI exits.
dt() {{
    local output
    output="$(command {bin} "$@")"
    local exit_code=$?
    local dest=""
    local copied=""
    while IFS= read -r line; do
        case "$line" in
            {CD_PREFIX}*) dest="${{line#{CD_PREFIX}}}" ;;
            {CLIP_PREFIX}*) copied="${{line#{CLIP_PREFIX}}}" ;;
        esac
    done <<< "$output"
    if [ $exit_code -eq 0 ] && [ -n "$dest" ] && [ -d "$dest" ]; then
        cd "$dest" || return
    fi
    if [ $exit_code -eq 0 ] && [ -n "$copied" ]; then
        printf 'Copied to clipboard: %s\n' "$copied"
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
# Toggle with `dt`.  Enter on a directory changes cwd, and copy actions
# print a clipboard notice after the TUI exits.
dt() {{
    local output
    output="$(command {bin} "$@")"
    local exit_code=$?
    local dest=""
    local copied=""
    while IFS= read -r line; do
        case "$line" in
            {CD_PREFIX}*) dest="${{line#{CD_PREFIX}}}" ;;
            {CLIP_PREFIX}*) copied="${{line#{CLIP_PREFIX}}}" ;;
        esac
    done <<< "$output"
    if [[ $exit_code -eq 0 ]] && [[ -n "$dest" ]] && [[ -d "$dest" ]]; then
        cd "$dest"
    fi
    if [[ $exit_code -eq 0 ]] && [[ -n "$copied" ]]; then
        printf 'Copied to clipboard: %s\n' "$copied"
    fi
}}
"#
    )
}

