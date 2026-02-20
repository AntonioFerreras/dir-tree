//! Search index + ranking for filename/dirname lookup.
//!
//! Matching is name-substring based (with optional case sensitivity).

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

#[derive(Debug, Clone)]
pub struct SearchEntry {
    pub path: PathBuf,
    pub name: String,
    pub name_lower: String,
    pub is_dir: bool,
    pub rel_depth: usize,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RankKey {
    exact: bool,
    prefix: bool,
    match_pos: usize,
    name_len: usize,
    rel_depth: usize,
    is_dir: bool,
}

impl RankKey {
    fn cmp_better(self, other: Self) -> Ordering {
        // "Better" should come first in ascending sort.
        other
            .exact
            .cmp(&self.exact)
            .then_with(|| other.prefix.cmp(&self.prefix))
            .then_with(|| self.match_pos.cmp(&other.match_pos))
            .then_with(|| self.name_len.cmp(&other.name_len))
            .then_with(|| self.rel_depth.cmp(&other.rel_depth))
            .then_with(|| other.is_dir.cmp(&self.is_dir))
    }
}

/// Build a flat index of every entry under `root` (including `root`).
pub fn build_index(
    root: &Path,
    show_hidden: bool,
    respect_gitignore: bool,
    one_file_system: bool,
) -> Vec<SearchEntry> {
    let mut out = Vec::new();

    if let Some(root_name) = root.file_name().and_then(|n| n.to_str()) {
        out.push(SearchEntry {
            path: root.to_path_buf(),
            name: root_name.to_string(),
            name_lower: root_name.to_lowercase(),
            is_dir: true,
            rel_depth: 0,
        });
    }

    let walker = WalkBuilder::new(root)
        .hidden(!show_hidden)
        .git_ignore(respect_gitignore)
        .same_file_system(one_file_system)
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if path == root {
            continue;
        }
        let Some(name_os) = path.file_name() else {
            continue;
        };
        let name = name_os.to_string_lossy().into_owned();
        if name.is_empty() {
            continue;
        }
        let rel_depth = path
            .strip_prefix(root)
            .ok()
            .map(|p| p.components().count())
            .unwrap_or(0);
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        out.push(SearchEntry {
            path: path.to_path_buf(),
            name_lower: name.to_lowercase(),
            name,
            is_dir,
            rel_depth,
        });
    }

    out
}

/// Search pre-indexed entries using name substring matching.
pub fn search_entries(
    entries: &[SearchEntry],
    query: &str,
    case_sensitive: bool,
    limit: usize,
) -> Vec<SearchResult> {
    let q = query.trim();
    if q.is_empty() || limit == 0 {
        return Vec::new();
    }

    let q_lower = if case_sensitive {
        String::new()
    } else {
        q.to_lowercase()
    };

    let mut ranked: Vec<(RankKey, &SearchEntry)> = Vec::new();
    for entry in entries {
        let (haystack, needle) = if case_sensitive {
            (entry.name.as_str(), q)
        } else {
            (entry.name_lower.as_str(), q_lower.as_str())
        };
        let Some(pos) = haystack.find(needle) else {
            continue;
        };
        ranked.push((
            RankKey {
                exact: haystack == needle,
                prefix: haystack.starts_with(needle),
                match_pos: pos,
                name_len: entry.name.chars().count(),
                rel_depth: entry.rel_depth,
                is_dir: entry.is_dir,
            },
            entry,
        ));
    }

    ranked.sort_by(|(a_rank, a_entry), (b_rank, b_entry)| {
        a_rank
            .cmp_better(*b_rank)
            .then_with(|| a_entry.path.cmp(&b_entry.path))
    });
    ranked.truncate(limit);

    ranked
        .into_iter()
        .map(|(_, e)| SearchResult {
            path: e.path.clone(),
            name: e.name.clone(),
            is_dir: e.is_dir,
        })
        .collect()
}

