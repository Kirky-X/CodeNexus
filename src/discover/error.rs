// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Error types for the file discovery subsystem.

use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur while walking a repository with [`super::Walker`].
#[derive(Debug, Error)]
pub enum DiscoverError {
    /// An IO error occurred while reading a file or directory entry.
    #[error("IO error while discovering files: {0}")]
    Io(#[from] std::io::Error),

    /// An error reported by the underlying `ignore` walker.
    #[error("ignore walker error: {0}")]
    Walk(#[from] ignore::Error),

    /// A discovered file path could not be expressed relative to the root.
    #[error("failed to compute relative path for {path} relative to root {root}")]
    RelativePath {
        /// The file path that could not be made relative.
        path: PathBuf,
        /// The discovery root.
        root: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_variant_displays_message() {
        let err = DiscoverError::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing file",
        ));
        let msg = err.to_string();
        assert!(msg.contains("IO error"), "got: {msg}");
    }

    #[test]
    fn relative_path_variant_displays_paths() {
        let err = DiscoverError::RelativePath {
            path: PathBuf::from("/a/b/c.rs"),
            root: PathBuf::from("/x"),
        };
        let msg = err.to_string();
        assert!(msg.contains("/a/b/c.rs"), "got: {msg}");
        assert!(msg.contains("/x"), "got: {msg}");
        assert!(msg.contains("relative path"), "got: {msg}");
    }
}
