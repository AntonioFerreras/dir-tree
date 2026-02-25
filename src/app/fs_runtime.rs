//! Background filesystem/search jobs to keep the UI thread responsive.

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::core::{
    fs::{self, WalkConfig},
    search::SearchEntry,
    tree::{DirTree, EntryMeta},
};

pub enum FsUpdate {
    TreeRebuilt {
        generation: u64,
        root: PathBuf,
        result: anyhow::Result<DirTree>,
    },
    DirExpanded {
        path: PathBuf,
        result: anyhow::Result<Vec<EntryMeta>>,
    },
    SearchIndexed {
        generation: u64,
        root: PathBuf,
        entries: Vec<SearchEntry>,
    },
}

pub fn spawn_tree_rebuild(
    tx: mpsc::UnboundedSender<FsUpdate>,
    generation: u64,
    root: PathBuf,
    walk_config: WalkConfig,
    one_file_system: bool,
) {
    std::thread::spawn(move || {
        let result = fs::build_tree(&root, &walk_config, one_file_system);
        let _ = tx.send(FsUpdate::TreeRebuilt {
            generation,
            root,
            result,
        });
    });
}

pub fn spawn_dir_expand(
    tx: mpsc::UnboundedSender<FsUpdate>,
    path: PathBuf,
    walk_config: WalkConfig,
    one_file_system: bool,
) {
    std::thread::spawn(move || {
        let children = fs::scan_immediate_children(&path, &walk_config, one_file_system);
        let _ = tx.send(FsUpdate::DirExpanded {
            path,
            result: Ok(children),
        });
    });
}

pub fn spawn_search_index(
    tx: mpsc::UnboundedSender<FsUpdate>,
    generation: u64,
    root: PathBuf,
    walk_config: WalkConfig,
    one_file_system: bool,
) {
    std::thread::spawn(move || {
        let entries = crate::core::search::build_index(
            &root,
            walk_config.show_hidden,
            walk_config.respect_gitignore,
            one_file_system,
        );
        let _ = tx.send(FsUpdate::SearchIndexed {
            generation,
            root,
            entries,
        });
    });
}

