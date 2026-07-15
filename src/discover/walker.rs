// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! File discovery walker (ADR-012, BR-INDEX-006).
//!
//! Wraps the [`ignore`] crate to walk a repository tree while honoring
//! `.gitignore`/`.codenexusignore` rules and the [`ALWAYS_SKIP_DIRS`]
//! allowlist. Only files whose extension maps to a supported [`Language`] are
//! returned.

use std::path::{Path, PathBuf};

use ignore::{DirEntry, WalkBuilder};

use crate::model::Language;

use super::error::DiscoverError;

/// Directory names that are always pruned during discovery (BR-INDEX-006).
///
/// These directories typically hold build artifacts, dependencies, or
/// editor/IDE state that should never be indexed.
pub const ALWAYS_SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vscode",
];

/// Metadata for a single discovered source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// Absolute (or as-given) path to the file on disk.
    pub path: PathBuf,
    /// Path relative to the discovery root, using the platform separator.
    pub relative_path: String,
    /// Detected language for the file, if its extension is recognized.
    pub language: Option<Language>,
    /// File size in bytes.
    pub size: u64,
}

/// Walks a directory tree discovering source code files.
///
/// Honors `.gitignore`, `.codenexusignore`, `.git/info/exclude`, the global
/// gitignore, and `.ignore` files via the [`ignore`] crate. Directories listed
/// in [`ALWAYS_SKIP_DIRS`] (or a custom skip list) are pruned.
#[derive(Debug, Clone)]
pub struct Walker {
    root: PathBuf,
    skip_dirs: Vec<String>,
}

impl Walker {
    /// Creates a new walker rooted at `root` using the default
    /// [`ALWAYS_SKIP_DIRS`] skip list.
    #[must_use]
    pub fn new(root: impl AsRef<Path>) -> Self {
        Walker {
            root: root.as_ref().to_path_buf(),
            skip_dirs: ALWAYS_SKIP_DIRS
                .iter()
                .copied()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Creates a new walker rooted at `root` with a custom skip list.
    #[must_use]
    pub fn with_skip_dirs(root: impl AsRef<Path>, skip_dirs: Vec<String>) -> Self {
        Walker {
            root: root.as_ref().to_path_buf(),
            skip_dirs,
        }
    }

    /// Walks the tree and returns all recognized source files.
    ///
    /// Files without a recognized code extension are excluded. The returned
    /// vector is in traversal order (not sorted).
    pub fn discover(&self) -> Result<Vec<FileInfo>, DiscoverError> {
        let mut builder = WalkBuilder::new(&self.root);
        builder
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .ignore(true)
            .require_git(false)
            .add_custom_ignore_filename(".codenexusignore");

        let skip_dirs = self.skip_dirs.clone();
        builder.filter_entry(move |entry: &DirEntry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    if skip_dirs.iter().any(|d| d.as_str() == name) {
                        return false;
                    }
                }
            }
            true
        });
        let walker = builder.build();

        let mut files = Vec::new();
        for result in walker {
            let entry = result?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let language = match is_code_file(path) {
                Some(lang) => lang,
                None => continue,
            };
            let metadata = entry.metadata()?;
            let relative_path =
                path.strip_prefix(&self.root)
                    .map_err(|_| DiscoverError::RelativePath {
                        path: path.to_path_buf(),
                        root: self.root.clone(),
                    })?;
            files.push(FileInfo {
                path: path.to_path_buf(),
                relative_path: relative_path.to_string_lossy().into_owned(),
                language: Some(language),
                size: metadata.len(),
            });
        }
        Ok(files)
    }
}

/// Returns the [`Language`] for a file with a recognized code extension, or
/// `None` for unrecognized/non-code files.
///
/// Only returns languages whose tree-sitter parser is compiled in (i.e.,
/// whose `lang-*` Cargo feature is enabled). A `.json` file returns `None`
/// when `lang-json` is not enabled, preventing downstream dispatcher panics.
#[must_use]
pub fn is_code_file(path: &Path) -> Option<Language> {
    let lang = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(Language::from_extension)?;
    if Language::compiled().contains(&lang) {
        Some(lang)
    } else {
        None
    }
}

/// Returns `true` if `dir_name` is in the hardcoded [`ALWAYS_SKIP_DIRS`] list
/// (BR-INDEX-006).
#[must_use]
pub fn should_skip_dir(dir_name: &str) -> bool {
    ALWAYS_SKIP_DIRS.contains(&dir_name)
}

#[cfg(all(
    test,
    feature = "lang-c",
    feature = "lang-fortran",
    feature = "lang-python",
    feature = "lang-rust",
    feature = "lang-typescript"
))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    /// Writes a file at `dir/rel` (creating parent directories as needed).
    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Collects the relative paths of a slice of [`FileInfo`].
    fn rel_paths(files: &[FileInfo]) -> Vec<&str> {
        files.iter().map(|f| f.relative_path.as_str()).collect()
    }

    #[test]
    fn discovers_basic_code_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "fn main() {}");
        write_file(root, "foo.c", "int main(void) { return 0; }");
        write_file(root, "bar.py", "print('hi')");
        write_file(root, "baz.ts", "console.log('hi');");
        write_file(root, "qux.f90", "program qux\nend program qux\n");

        let walker = Walker::new(root);
        let files = walker.discover().unwrap();

        assert_eq!(files.len(), 5);
        let paths = rel_paths(&files);
        assert!(paths.contains(&"main.rs"));
        assert!(paths.contains(&"foo.c"));
        assert!(paths.contains(&"bar.py"));
        assert!(paths.contains(&"baz.ts"));
        assert!(paths.contains(&"qux.f90"));
    }

    #[test]
    fn respects_gitignore_ac_index_004() {
        // AC-INDEX-004: .gitignore with "target/" skips target/ files.
        // Use an empty skip list so ALWAYS_SKIP_DIRS does not mask the
        // .gitignore behavior.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, ".gitignore", "target/\n");
        write_file(root, "target/skip.rs", "fn skip() {}");
        write_file(root, "src/main.rs", "fn main() {}");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        let paths = rel_paths(&files);
        assert!(paths.contains(&"src/main.rs"));
        assert!(!paths.iter().any(|p| p.contains("target")));
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn respects_gitignore_custom_directory() {
        // Proves .gitignore itself is honored (not just ALWAYS_SKIP_DIRS) by
        // ignoring a directory that is NOT in the hardcoded list.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, ".gitignore", "ignored/\n");
        write_file(root, "ignored/secret.rs", "fn secret() {}");
        write_file(root, "kept.rs", "fn kept() {}");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        let paths = rel_paths(&files);
        assert!(paths.contains(&"kept.rs"));
        assert!(!paths.iter().any(|p| p.contains("ignored")));
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn respects_codenexusignore() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, ".codenexusignore", "secret.rs\n");
        write_file(root, "secret.rs", "fn secret() {}");
        write_file(root, "main.rs", "fn main() {}");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        let paths = rel_paths(&files);
        assert!(paths.contains(&"main.rs"));
        assert!(!paths.contains(&"secret.rs"));
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn skips_all_always_skip_dirs_br_index_006() {
        // BR-INDEX-006: ALWAYS_SKIP_DIRS are pruned even without a .gitignore.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        for dir in ALWAYS_SKIP_DIRS {
            write_file(root, &format!("{dir}/inside.rs"), "fn inside() {}");
        }
        write_file(root, "src/main.rs", "fn main() {}");

        let walker = Walker::new(root);
        let files = walker.discover().unwrap();

        let paths = rel_paths(&files);
        assert!(paths.contains(&"src/main.rs"));
        for dir in ALWAYS_SKIP_DIRS {
            assert!(
                !paths.iter().any(|p| p.starts_with(dir)),
                "found a file under always-skip dir {dir}"
            );
        }
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn excludes_non_code_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "fn main() {}");
        write_file(root, "notes.txt", "hello");
        write_file(root, "README.md", "# readme");
        write_file(root, "config.ini", "[settings]");
        write_file(root, "data.yaml", "key: value");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "main.rs");
    }

    #[test]
    fn detects_language_from_extension() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "a.rs", "");
        write_file(root, "b.c", "");
        write_file(root, "c.h", "");
        write_file(root, "d.py", "");
        write_file(root, "e.ts", "");
        write_file(root, "f.tsx", "");
        write_file(root, "g.f90", "");
        write_file(root, "h.f", "");
        write_file(root, "i.f95", "");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        let lang_of = |name: &str| -> Language {
            files
                .iter()
                .find(|f| f.relative_path == name)
                .and_then(|f| f.language)
                .unwrap_or_else(|| panic!("file {name} not found"))
        };

        assert_eq!(lang_of("a.rs"), Language::Rust);
        assert_eq!(lang_of("b.c"), Language::C);
        assert_eq!(lang_of("c.h"), Language::C);
        assert_eq!(lang_of("d.py"), Language::Python);
        assert_eq!(lang_of("e.ts"), Language::TypeScript);
        assert_eq!(lang_of("f.tsx"), Language::TypeScript);
        assert_eq!(lang_of("g.f90"), Language::Fortran);
        assert_eq!(lang_of("h.f"), Language::Fortran);
        assert_eq!(lang_of("i.f95"), Language::Fortran);
    }

    #[test]
    fn empty_directory_returns_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let walker = Walker::new(tmp.path());
        let files = walker.discover().unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn traverses_nested_directories() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "a.rs", "");
        write_file(root, "src/b.rs", "");
        write_file(root, "src/sub/c.rs", "");
        write_file(root, "src/sub/deep/d.rs", "");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();
        assert_eq!(files.len(), 4);
    }

    #[test]
    fn computes_relative_paths_correctly() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "");
        write_file(root, "src/lib.rs", "");
        write_file(root, "src/sub/mod.rs", "");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        let paths: Vec<String> = files.iter().map(|f| f.relative_path.clone()).collect();
        assert!(paths.contains(&"main.rs".to_string()));
        assert!(paths.contains(&"src/lib.rs".to_string()));
        assert!(paths.contains(&"src/sub/mod.rs".to_string()));
    }

    #[test]
    fn includes_hidden_code_files() {
        // hidden(false) means hidden files are not skipped by the walker; a
        // hidden file with a recognized extension is therefore discovered.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, ".secret.rs", "fn secret() {}");
        write_file(root, "main.rs", "fn main() {}");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        let paths = rel_paths(&files);
        assert!(paths.contains(&".secret.rs"));
        assert!(paths.contains(&"main.rs"));
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn records_file_size() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let content = "fn main() { println!(\"hello world\"); }";
        write_file(root, "main.rs", content);

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].size, content.len() as u64);
    }

    #[test]
    fn file_info_carries_absolute_path() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", "");

        let walker = Walker::with_skip_dirs(root, vec![]);
        let files = walker.discover().unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, root.join("main.rs"));
        assert_eq!(files[0].language, Some(Language::Rust));
    }

    #[test]
    fn with_skip_dirs_uses_custom_list() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // "custom_skip" is NOT in ALWAYS_SKIP_DIRS; "target" IS.
        write_file(root, "custom_skip/a.rs", "");
        write_file(root, "target/b.rs", "");
        write_file(root, "main.rs", "");

        let walker = Walker::with_skip_dirs(root, vec!["custom_skip".to_string()]);
        let files = walker.discover().unwrap();

        let paths = rel_paths(&files);
        assert!(paths.contains(&"main.rs"));
        // target/ is NOT skipped because the custom list omits it.
        assert!(paths.contains(&"target/b.rs"));
        // custom_skip/ IS skipped because the custom list includes it.
        assert!(!paths.iter().any(|p| p.starts_with("custom_skip")));
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn is_code_file_detects_all_supported_extensions() {
        assert_eq!(is_code_file(Path::new("foo.rs")), Some(Language::Rust));
        assert_eq!(is_code_file(Path::new("foo.c")), Some(Language::C));
        assert_eq!(is_code_file(Path::new("foo.h")), Some(Language::C));
        assert_eq!(is_code_file(Path::new("foo.py")), Some(Language::Python));
        assert_eq!(
            is_code_file(Path::new("foo.ts")),
            Some(Language::TypeScript)
        );
        assert_eq!(
            is_code_file(Path::new("foo.tsx")),
            Some(Language::TypeScript)
        );
        assert_eq!(is_code_file(Path::new("foo.f90")), Some(Language::Fortran));
        assert_eq!(is_code_file(Path::new("foo.f")), Some(Language::Fortran));
        assert_eq!(is_code_file(Path::new("foo.f95")), Some(Language::Fortran));
    }

    #[test]
    fn is_code_file_returns_none_for_non_code() {
        assert_eq!(is_code_file(Path::new("foo.txt")), None);
        assert_eq!(is_code_file(Path::new("foo.md")), None);
        assert_eq!(is_code_file(Path::new("foo.yaml")), None);
        assert_eq!(is_code_file(Path::new("foo")), None);
        assert_eq!(is_code_file(Path::new(".gitignore")), None);
        assert_eq!(is_code_file(Path::new("Makefile")), None);
    }

    #[test]
    fn is_code_file_handles_paths_with_directories() {
        assert_eq!(
            is_code_file(Path::new("src/sub/mod.rs")),
            Some(Language::Rust)
        );
        assert_eq!(
            is_code_file(Path::new("/abs/path/main.c")),
            Some(Language::C)
        );
    }

    #[test]
    fn should_skip_dir_matches_all_always_skip_dirs() {
        for dir in ALWAYS_SKIP_DIRS {
            assert!(should_skip_dir(dir), "expected {dir} to be skipped");
        }
    }

    #[test]
    fn should_skip_dir_returns_false_for_normal_dirs() {
        assert!(!should_skip_dir("src"));
        assert!(!should_skip_dir("lib"));
        assert!(!should_skip_dir("tests"));
        assert!(!should_skip_dir("examples"));
        assert!(!should_skip_dir(""));
    }

    #[test]
    fn always_skip_dirs_contains_expected_entries() {
        assert_eq!(
            ALWAYS_SKIP_DIRS,
            &[
                "target",
                "node_modules",
                ".git",
                "dist",
                "build",
                "__pycache__",
                ".venv",
                "venv",
                ".idea",
                ".vscode",
            ]
        );
    }
}
