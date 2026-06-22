//! Fast, smart local filesystem search for the File and Folder modes.
//!
//! Uses ripgrep's [`ignore`] walker to index `$HOME` once (skipping hidden,
//! `.gitignore`d, and a denylist of heavy/system/cache directories), then
//! fuzzy-ranks the in-memory index with [`SkimMatcherV2`] on every keystroke —
//! so after the initial walk, filtering is effectively instant.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ignore::overrides::OverrideBuilder;
use ignore::{WalkBuilder, WalkState};

/// Cap on indexed entries (keeps memory/time bounded on huge trees).
const MAX_INDEX: usize = 200_000;
/// Cap on directory recursion depth.
const MAX_DEPTH: usize = 14;
/// Heavy / build / cache directories to prune (on top of hidden + .gitignore).
const DENY_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "venv",
    ".venv",
    "__pycache__",
    "vendor",
    ".git",
    ".cache",
    "site-packages",
];

/// Which kind of entry a search is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Files,
    Dirs,
}

/// A single local filesystem result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEntry {
    /// Absolute path (used to open the entry).
    pub path: String,
    /// File/dir name (display title).
    pub name: String,
    /// Home-relative parent directory (display subtitle, e.g. `~/projects`).
    pub parent: String,
}

/// In-memory index of files and directories under `$HOME`.
#[derive(Debug, Default)]
pub struct Index {
    pub files: Vec<LocalEntry>,
    pub dirs: Vec<LocalEntry>,
}

/// The user's home directory (falls back to `.`).
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Build the index by walking `root` with smart ignores.
pub fn build_index(root: &Path) -> Index {
    let (tx, rx) = std::sync::mpsc::channel::<(bool, LocalEntry)>();
    let count = Arc::new(AtomicUsize::new(0));
    let root_owned = root.to_path_buf();

    let walker = WalkBuilder::new(root)
        .hidden(true) // skip dotfiles/dirs (covers most system/cache dirs)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        .max_depth(Some(MAX_DEPTH))
        .overrides(build_overrides(root))
        .build_parallel();

    walker.run(|| {
        let tx = tx.clone();
        let count = Arc::clone(&count);
        let root = root_owned.clone();
        Box::new(move |result| {
            let entry = match result {
                Ok(e) => e,
                Err(_) => return WalkState::Continue,
            };
            let path = entry.path();
            if path == root {
                return WalkState::Continue;
            }
            if count.fetch_add(1, Ordering::Relaxed) >= MAX_INDEX {
                return WalkState::Quit;
            }
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if tx.send((is_dir, make_entry(path, &root))).is_err() {
                return WalkState::Quit;
            }
            WalkState::Continue
        })
    });
    drop(tx);

    let mut index = Index::default();
    for (is_dir, entry) in rx {
        if is_dir {
            index.dirs.push(entry);
        } else {
            index.files.push(entry);
        }
    }
    index
}

/// Build the override set that prunes the heavy/system directories.
fn build_overrides(root: &Path) -> ignore::overrides::Override {
    let mut ob = OverrideBuilder::new(root);
    for dir in DENY_DIRS {
        // A leading `!` makes the glob an *exclude* (ignore) rule.
        let _ = ob.add(&format!("!**/{dir}/**"));
        let _ = ob.add(&format!("!**/{dir}"));
    }
    ob.build()
        .unwrap_or_else(|_| OverrideBuilder::new(root).build().expect("empty override"))
}

/// Turn a path into a display-friendly [`LocalEntry`] relative to `home`.
fn make_entry(path: &Path, home: &Path) -> LocalEntry {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = path
        .parent()
        .map(|p| display_path(p, home))
        .unwrap_or_default();
    LocalEntry {
        path: path.to_string_lossy().into_owned(),
        name,
        parent,
    }
}

/// Render `p` relative to `home`, using a leading `~`.
fn display_path(p: &Path, home: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => p.display().to_string(),
    }
}

/// Fuzzy-search the index for `term`, returning up to `limit` ranked entries.
/// An empty term returns the first `limit` entries (recent-ish ordering).
pub fn search(index: &Index, kind: LocalKind, term: &str, limit: usize) -> Vec<LocalEntry> {
    let entries = match kind {
        LocalKind::Files => &index.files,
        LocalKind::Dirs => &index.dirs,
    };

    let term = term.trim();
    if term.is_empty() {
        return entries.iter().take(limit).cloned().collect();
    }

    // `ignore_case` makes matching case-insensitive regardless of query case
    // (the default is smart-case, where an uppercase letter forces a match).
    let matcher = SkimMatcherV2::default().ignore_case();
    let mut scored: Vec<(i64, &LocalEntry)> = entries
        .iter()
        .filter_map(|e| matcher.fuzzy_match(&e.name, term).map(|score| (score, e)))
        .collect();

    // Highest score first; then prefer shallower paths; then shorter names.
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| path_depth(&a.1.path).cmp(&path_depth(&b.1.path)))
            .then_with(|| a.1.name.len().cmp(&b.1.name.len()))
    });
    scored.into_iter().take(limit).map(|(_, e)| e.clone()).collect()
}

/// Number of path separators — used to prefer shallower (closer-to-home) hits.
fn path_depth(path: &str) -> usize {
    path.bytes().filter(|&b| b == b'/').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, parent: &str) -> LocalEntry {
        LocalEntry {
            path: format!("{}/{}", parent.replace('~', "/home/u"), name),
            name: name.to_string(),
            parent: parent.to_string(),
        }
    }

    fn sample_index() -> Index {
        Index {
            files: vec![
                entry("report.pdf", "~/docs"),
                entry("readme.md", "~/projects/app"),
                entry("main.rs", "~/projects/app/src"),
                entry("notes.txt", "~/docs"),
            ],
            dirs: vec![
                entry("projects", "~"),
                entry("downloads", "~"),
                entry("documents", "~"),
            ],
        }
    }

    #[test]
    fn empty_term_returns_first_n() {
        let idx = sample_index();
        let out = search(&idx, LocalKind::Files, "", 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "report.pdf");
    }

    #[test]
    fn fuzzy_matches_files_by_name() {
        let idx = sample_index();
        let out = search(&idx, LocalKind::Files, "readme", 10);
        assert!(!out.is_empty());
        assert_eq!(out[0].name, "readme.md");
    }

    #[test]
    fn fuzzy_is_subsequence() {
        let idx = sample_index();
        // "mrs" is a subsequence of "main.rs".
        let out = search(&idx, LocalKind::Files, "mrs", 10);
        assert!(out.iter().any(|e| e.name == "main.rs"));
    }

    #[test]
    fn searches_dirs_separately() {
        let idx = sample_index();
        let out = search(&idx, LocalKind::Dirs, "down", 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "downloads");
        // A file name shouldn't appear in a dir search.
        let none = search(&idx, LocalKind::Dirs, "report", 10);
        assert!(none.is_empty());
    }

    #[test]
    fn search_is_case_insensitive() {
        let idx = sample_index();
        // An uppercase query still matches lowercase names (files and dirs).
        let files = search(&idx, LocalKind::Files, "REPORT", 10);
        assert!(files.iter().any(|e| e.name == "report.pdf"));
        let dirs = search(&idx, LocalKind::Dirs, "DOWN", 10);
        assert!(dirs.iter().any(|e| e.name == "downloads"));
    }

    #[test]
    fn non_match_returns_empty() {
        let idx = sample_index();
        let out = search(&idx, LocalKind::Files, "zzzzzz", 10);
        assert!(out.is_empty());
    }

    #[test]
    fn prefers_shallower_paths() {
        let idx = Index {
            files: vec![],
            dirs: vec![
                LocalEntry {
                    path: "/home/u/a/b/c/projects".into(),
                    name: "projects".into(),
                    parent: "~/a/b/c".into(),
                },
                LocalEntry {
                    path: "/home/u/projects".into(),
                    name: "projects".into(),
                    parent: "~".into(),
                },
            ],
        };
        let out = search(&idx, LocalKind::Dirs, "projects", 10);
        assert_eq!(out[0].path, "/home/u/projects");
    }

    #[test]
    fn display_path_uses_tilde() {
        let home = Path::new("/home/u");
        assert_eq!(display_path(Path::new("/home/u"), home), "~");
        assert_eq!(display_path(Path::new("/home/u/docs"), home), "~/docs");
        assert_eq!(display_path(Path::new("/etc"), home), "/etc");
    }

    #[test]
    fn make_entry_splits_name_and_parent() {
        let home = Path::new("/home/u");
        let e = make_entry(Path::new("/home/u/docs/report.pdf"), home);
        assert_eq!(e.name, "report.pdf");
        assert_eq!(e.parent, "~/docs");
        assert_eq!(e.path, "/home/u/docs/report.pdf");
    }
}
