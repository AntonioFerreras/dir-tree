//! File/dir metadata inspection used by the inspector pane.
//!
//! This module performs filesystem reads and returns plain data structures.
//! No UI or Ratatui types are used here.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct InspectorInfo {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    pub detected_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub readonly: bool,
    pub perms_symbolic: Option<String>,
    pub perms_octal: Option<String>,
    pub modified_unix: Option<u64>,
    pub created_unix: Option<u64>,
    pub symlink_target: Option<String>,
    pub subdirs: Option<u64>,
    pub subfiles: Option<u64>,
    pub others: Option<u64>,
    pub error: Option<String>,
}

pub fn inspect_path(path: &Path) -> InspectorInfo {
    let mut info = InspectorInfo {
        path: path.to_path_buf(),
        name: path
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| path.display().to_string()),
        kind: "Unknown".to_string(),
        detected_type: None,
        size_bytes: None,
        readonly: false,
        perms_symbolic: None,
        perms_octal: None,
        modified_unix: None,
        created_unix: None,
        symlink_target: None,
        subdirs: None,
        subfiles: None,
        others: None,
        error: None,
    };

    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            info.error = Some(format!("stat error: {e}"));
            return info;
        }
    };

    let ft = meta.file_type();
    info.readonly = meta.permissions().readonly();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = meta.mode();
        info.perms_symbolic = Some(mode_to_symbolic(mode));
        info.perms_octal = Some(format!("{:04o}", mode & 0o7777));
    }
    info.modified_unix = to_unix_secs(meta.modified().ok());
    info.created_unix = to_unix_secs(meta.created().ok());

    if ft.is_dir() {
        info.kind = "Directory".to_string();
        let (subdirs, subfiles, others, err) = count_immediate_children(path);
        info.subdirs = Some(subdirs);
        info.subfiles = Some(subfiles);
        info.others = Some(others);
        if let Some(e) = err {
            info.error = Some(e);
        }
    } else if ft.is_symlink() {
        info.kind = "Symlink".to_string();
        info.size_bytes = Some(meta.len());
        if let Ok(target) = std::fs::read_link(path) {
            info.symlink_target = Some(target.display().to_string());
        }
    } else if ft.is_file() {
        info.kind = "File".to_string();
        info.size_bytes = Some(meta.len());
        info.detected_type = detect_file_type(path);
    } else {
        info.kind = "Other".to_string();
        info.size_bytes = Some(0);
    }

    info
}

fn count_immediate_children(path: &Path) -> (u64, u64, u64, Option<String>) {
    let mut subdirs = 0u64;
    let mut subfiles = 0u64;
    let mut others = 0u64;

    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => return (0, 0, 0, Some(format!("read_dir error: {e}"))),
    };

    for entry in entries {
        match entry {
            Ok(ent) => match ent.file_type() {
                Ok(ft) => {
                    if ft.is_dir() {
                        subdirs += 1;
                    } else if ft.is_file() || ft.is_symlink() {
                        subfiles += 1;
                    } else {
                        others += 1;
                    }
                }
                Err(_) => {
                    others += 1;
                }
            },
            Err(_) => {
                others += 1;
            }
        }
    }

    (subdirs, subfiles, others, None)
}

fn to_unix_secs(t: Option<SystemTime>) -> Option<u64> {
    t.and_then(|v| v.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
}

fn detect_file_type(path: &Path) -> Option<String> {
    // Uses shared-mime-info signatures (magic) for robust content-based
    // detection, not just extension matching.
    tree_magic_mini::from_filepath(path).map(str::to_string)
}

#[cfg(unix)]
fn mode_to_symbolic(mode: u32) -> String {
    let mut s = String::new();
    let file_kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o100000 => '-',
        0o060000 => 'b',
        0o020000 => 'c',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '?',
    };
    s.push(file_kind);

    let bits = [
        0o400, 0o200, 0o100, // user
        0o040, 0o020, 0o010, // group
        0o004, 0o002, 0o001, // other
    ];
    for (i, bit) in bits.iter().enumerate() {
        let ch = match i % 3 {
            0 => {
                if mode & bit != 0 {
                    'r'
                } else {
                    '-'
                }
            }
            1 => {
                if mode & bit != 0 {
                    'w'
                } else {
                    '-'
                }
            }
            _ => {
                if mode & bit != 0 {
                    'x'
                } else {
                    '-'
                }
            }
        };
        s.push(ch);
    }
    s
}

