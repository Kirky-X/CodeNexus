// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Integration tests for non-ASCII file path support (Task 12, 🔴-05).
//!
//! Verifies that the indexing pipeline correctly handles file paths
//! containing non-ASCII characters (Chinese, Japanese, Korean, emoji, etc.)
//! on all platforms.

use codenexus::index::IndexFacade;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Writes a Rust file at `dir/rel` (creating parent directories as needed).
fn write_file(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, content).unwrap();
}

#[test]
fn test_chinese_directory_name() {
    let tmp = TempDir::new().unwrap();
    // Create a directory with Chinese characters
    let chinese_dir = tmp.path().join("中文目录");
    fs::create_dir_all(&chinese_dir).unwrap();
    write_file(&chinese_dir, "main.rs", "fn main() {}\n");

    let db = tmp.path().join("测试.db");
    let facade = IndexFacade::new(&db).expect("facade");
    let result = facade.index(&chinese_dir, "中文项目", false);
    assert!(
        result.is_ok(),
        "indexing Chinese path should succeed: {:?}",
        result.err()
    );
    let result = result.unwrap();
    assert_eq!(result.files_indexed, 1);
}

#[test]
fn test_chinese_file_name() {
    let tmp = TempDir::new().unwrap();
    // Rust allows Chinese identifiers but we use Chinese filename with ASCII content
    write_file(tmp.path(), "函数.rs", "fn main() {}\n");

    let db = tmp.path().join("test.db");
    let facade = IndexFacade::new(&db).expect("facade");
    let result = facade.index(tmp.path(), "test", false);
    assert!(
        result.is_ok(),
        "indexing Chinese filename should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_japanese_directory_name() {
    let tmp = TempDir::new().unwrap();
    let japanese_dir = tmp.path().join("日本語ディレクトリ");
    fs::create_dir_all(&japanese_dir).unwrap();
    write_file(&japanese_dir, "main.rs", "fn main() {}\n");

    let db = tmp.path().join("test.db");
    let facade = IndexFacade::new(&db).expect("facade");
    let result = facade.index(&japanese_dir, "プロジェクト", false);
    assert!(
        result.is_ok(),
        "indexing Japanese path should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_korean_directory_name() {
    let tmp = TempDir::new().unwrap();
    let korean_dir = tmp.path().join("한국어_디렉토리");
    fs::create_dir_all(&korean_dir).unwrap();
    write_file(&korean_dir, "main.rs", "fn main() {}\n");

    let db = tmp.path().join("test.db");
    let facade = IndexFacade::new(&db).expect("facade");
    let result = facade.index(&korean_dir, "프로젝트", false);
    assert!(
        result.is_ok(),
        "indexing Korean path should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_mixed_ascii_and_non_ascii_paths() {
    let tmp = TempDir::new().unwrap();
    // Mix of ASCII and non-ASCII paths
    write_file(tmp.path(), "main.rs", "fn main() {}\n");
    write_file(tmp.path(), "中文/工具.rs", "fn tool() {}\n");
    write_file(tmp.path(), "src/日本語.rs", "fn func() {}\n");

    let db = tmp.path().join("test.db");
    let facade = IndexFacade::new(&db).expect("facade");
    let result = facade.index(tmp.path(), "mixed", false);
    assert!(
        result.is_ok(),
        "indexing mixed paths should succeed: {:?}",
        result.err()
    );
    let result = result.unwrap();
    assert!(
        result.files_indexed >= 3,
        "should index at least 3 files, got {}",
        result.files_indexed
    );
}

#[test]
fn test_unicode_project_name() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\n");

    let db = tmp.path().join("test.db");
    let facade = IndexFacade::new(&db).expect("facade");
    let result = facade.index(tmp.path(), "プロジェクト_中文_한국어", false);
    assert!(
        result.is_ok(),
        "Unicode project name should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_non_ascii_db_path() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn main() {}\n");

    // Database path with non-ASCII characters
    let db = tmp.path().join("数据库.lbug");
    let facade = IndexFacade::new(&db).expect("facade");
    let result = facade.index(tmp.path(), "test", false);
    assert!(
        result.is_ok(),
        "non-ASCII db path should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_walker_discovers_non_ascii_paths() {
    use codenexus::discover::Walker;
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "中文.rs", "fn main() {}\n");
    write_file(tmp.path(), "日本語/main.rs", "fn main() {}\n");

    let walker = Walker::new(tmp.path());
    let files = walker.discover().expect("discover");
    assert_eq!(files.len(), 2, "should discover both files");
    // Verify paths contain the non-ASCII characters
    let paths: Vec<String> = files.iter().map(|f| f.relative_path.clone()).collect();
    assert!(
        paths.iter().any(|p| p.contains("中文")),
        "should find Chinese path: {:?}",
        paths
    );
    assert!(
        paths.iter().any(|p| p.contains("日本語")),
        "should find Japanese path: {:?}",
        paths
    );
}

#[test]
fn test_incremental_index_non_ascii_path() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "函数.rs", "fn foo() {}\n");

    let db = tmp.path().join("test.db");
    let facade = IndexFacade::new(&db).expect("facade");

    // First index
    let result1 = facade.index(tmp.path(), "test", false);
    assert!(result1.is_ok(), "first index should succeed");

    // Modify the file and re-index (incremental)
    write_file(tmp.path(), "函数.rs", "fn foo() { /* modified */ }\n");
    let result2 = facade.index_incremental(tmp.path(), "test", false);
    assert!(
        result2.is_ok(),
        "incremental index should succeed: {:?}",
        result2.err()
    );
}
