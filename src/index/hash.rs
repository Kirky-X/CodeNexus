// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! SHA-256 file content hashing (ADR-009).
//!
//! Provides deterministic SHA-256 digests used by the incremental indexer to
//! detect file changes (BR-INDEX-001~003). Hashes are returned as lowercase
//! hexadecimal strings (64 characters), matching the format stored in the
//! `File` node's `hash` property.

use std::path::Path;

use sha2::{Digest, Sha256};

/// Computes the SHA-256 hash of the file at `path`, returning the digest as
/// a lowercase 64-character hex string.
///
/// # Errors
///
/// Returns [`std::io::Error`] if the file cannot be read.
pub fn compute_file_hash(path: &Path) -> Result<String, std::io::Error> {
    let content = std::fs::read(path)?;
    Ok(compute_content_hash(&content))
}

/// Computes the SHA-256 hash of `content`, returning the digest as a
/// lowercase 64-character hex string.
#[must_use]
pub fn compute_content_hash(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let digest = hasher.finalize();
    // 32 bytes → 64 hex chars, lowercase.
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    // --- compute_content_hash ---

    #[test]
    fn compute_content_hash_returns_64_char_hex_string() {
        let hash = compute_content_hash(b"hello world");
        assert_eq!(hash.len(), 64, "SHA-256 hex digest must be 64 chars");
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hash must be lowercase hex: {hash}"
        );
    }

    #[test]
    fn compute_content_hash_known_value() {
        // SHA-256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        let hash = compute_content_hash(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn compute_content_hash_empty_input() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hash = compute_content_hash(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn compute_content_hash_same_content_same_hash() {
        let a = compute_content_hash(b"fn main() {}");
        let b = compute_content_hash(b"fn main() {}");
        assert_eq!(a, b, "identical content must produce identical hashes");
    }

    #[test]
    fn compute_content_hash_different_content_different_hash() {
        let a = compute_content_hash(b"fn main() {}");
        let b = compute_content_hash(b"fn main() { }");
        assert_ne!(a, b, "different content must produce different hashes");
    }

    #[test]
    fn compute_content_hash_handles_binary_content() {
        let binary: Vec<u8> = (0..=255u8).collect();
        let hash = compute_content_hash(&binary);
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_content_hash_handles_unicode() {
        let hash = compute_content_hash("fn 你好() {}".as_bytes());
        assert_eq!(hash.len(), 64);
    }

    // --- compute_file_hash ---

    #[test]
    fn compute_file_hash_returns_64_char_hex_string() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"fn main() {}").unwrap();
        let hash = compute_file_hash(tmp.path()).expect("hash should succeed");
        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hash must be lowercase hex: {hash}"
        );
    }

    #[test]
    fn compute_file_hash_matches_content_hash() {
        let tmp = NamedTempFile::new().unwrap();
        let content = b"int main(void) { return 0; }";
        fs::write(tmp.path(), content).unwrap();
        let file_hash = compute_file_hash(tmp.path()).unwrap();
        let content_hash = compute_content_hash(content);
        assert_eq!(file_hash, content_hash);
    }

    #[test]
    fn compute_file_hash_same_file_same_hash() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"fn foo() {}").unwrap();
        let a = compute_file_hash(tmp.path()).unwrap();
        let b = compute_file_hash(tmp.path()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn compute_file_hash_changes_when_content_changes() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"fn foo() {}").unwrap();
        let before = compute_file_hash(tmp.path()).unwrap();
        fs::write(tmp.path(), b"fn bar() {}").unwrap();
        let after = compute_file_hash(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn compute_file_hash_nonexistent_file_returns_error() {
        let result = compute_file_hash(Path::new("/nonexistent/path/does_not_exist.rs"));
        assert!(result.is_err(), "nonexistent file should error");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn compute_file_hash_empty_file() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"").unwrap();
        let hash = compute_file_hash(tmp.path()).unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
