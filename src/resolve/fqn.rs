//! Fully-qualified name (FQN) generation (ADD §7.1).
//!
//! Generates dot-separated FQNs in the format
//! `project.dir.subdir.filename.entity_name`, with special handling for
//! Python `__init__.py`, C header files, and Fortran modules.

use crate::model::Language;

/// Generates fully-qualified names following ADD §7.1.
///
/// Format: `project.dir.subdir.filename.entity_name` (dot-separated).
pub struct FqnGenerator;

impl FqnGenerator {
    /// Generates an FQN for a top-level entity in a file.
    ///
    /// # Special cases
    ///
    /// - Python `__init__.py`: the `__init__` segment is removed, so the FQN
    ///   uses the package path directly (e.g. `proj.src.pkg.MyClass`).
    /// - C header files: treated like any other file
    ///   (`proj.include.header.DEFINE`).
    /// - Path normalization: leading `./` is stripped and backslashes are
    ///   converted to forward slashes.
    #[must_use]
    pub fn generate(
        project: &str,
        file_path: &str,
        entity_name: &str,
        language: Language,
    ) -> String {
        let segments = Self::path_segments(file_path, language);
        let mut parts: Vec<String> = vec![project.to_string()];
        parts.extend(segments);
        parts.push(entity_name.to_string());
        parts.join(".")
    }

    /// Generates an FQN for an entity nested inside a module (Fortran).
    ///
    /// Format: `project.file.module.entity`.
    #[must_use]
    pub fn generate_for_module(
        project: &str,
        file_path: &str,
        module_name: &str,
        entity_name: &str,
    ) -> String {
        let segments = Self::path_segments(file_path, Language::Fortran);
        let mut parts: Vec<String> = vec![project.to_string()];
        parts.extend(segments);
        parts.push(module_name.to_string());
        parts.push(entity_name.to_string());
        parts.join(".")
    }

    /// Returns the parent qualified name (FQN without the last segment).
    ///
    /// Returns `None` if the FQN has no dot (single segment).
    ///
    /// # Examples
    ///
    /// - `"proj.src.main.Parser.parse"` -> `Some("proj.src.main.Parser")`
    /// - `"proj.src.main"` -> `Some("proj.src")`
    /// - `"proj"` -> `None`
    #[must_use]
    pub fn parent_qn(fqn: &str) -> Option<String> {
        fqn.rfind('.').map(|pos| fqn[..pos].to_string())
    }

    /// Splits a file path into FQN path segments.
    ///
    /// Normalizes the path (forward slashes, no leading `./`), removes the
    /// file extension from the last segment, and—for Python `__init__.py`—
    /// removes the `__init__` segment entirely.
    fn path_segments(file_path: &str, language: Language) -> Vec<String> {
        // Normalize: backslashes -> forward slashes, strip leading "./"
        let normalized = file_path.replace('\\', "/");
        let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);

        let mut segments: Vec<String> = normalized
            .split('/')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        // Remove the file extension from the last segment.
        if let Some(last) = segments.last_mut() {
            if let Some(dot_pos) = last.rfind('.') {
                last.truncate(dot_pos);
            }
        }

        // Python __init__.py: drop the "__init__" segment so the FQN uses
        // the package path directly.
        if language == Language::Python && segments.last().is_some_and(|s| s == "__init__") {
            segments.pop();
        }

        segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Basic generation ---

    #[test]
    fn basic_rust_file() {
        let fqn = FqnGenerator::generate("myproject", "src/main.rs", "parse", Language::Rust);
        assert_eq!(fqn, "myproject.src.main.parse");
    }

    #[test]
    fn nested_path() {
        let fqn = FqnGenerator::generate(
            "myproject",
            "src/deep/nested/file.rs",
            "entity",
            Language::Rust,
        );
        assert_eq!(fqn, "myproject.src.deep.nested.file.entity");
    }

    // --- Python __init__.py special case ---

    #[test]
    fn python_init_py_removes_init_segment() {
        let fqn = FqnGenerator::generate(
            "myproject",
            "src/pkg/__init__.py",
            "MyClass",
            Language::Python,
        );
        assert_eq!(fqn, "myproject.src.pkg.MyClass");
    }

    #[test]
    fn python_regular_file_keeps_filename_segment() {
        let fqn = FqnGenerator::generate(
            "myproject",
            "src/pkg/module.py",
            "MyClass",
            Language::Python,
        );
        assert_eq!(fqn, "myproject.src.pkg.module.MyClass");
    }

    #[test]
    fn python_init_py_at_root() {
        let fqn = FqnGenerator::generate("proj", "__init__.py", "X", Language::Python);
        assert_eq!(fqn, "proj.X");
    }

    #[test]
    fn python_init_py_in_nested_dir() {
        let fqn = FqnGenerator::generate("proj", "src/a/b/__init__.py", "Y", Language::Python);
        assert_eq!(fqn, "proj.src.a.b.Y");
    }

    // --- C header files ---

    #[test]
    fn c_header_file() {
        let fqn = FqnGenerator::generate("myproject", "include/header.h", "MY_DEFINE", Language::C);
        assert_eq!(fqn, "myproject.include.header.MY_DEFINE");
    }

    #[test]
    fn c_source_file() {
        let fqn = FqnGenerator::generate("proj", "src/main.c", "main", Language::C);
        assert_eq!(fqn, "proj.src.main.main");
    }

    // --- Fortran module ---

    #[test]
    fn fortran_module_entity() {
        let fqn = FqnGenerator::generate_for_module("proj", "src/mod.f90", "mymod", "my_func");
        assert_eq!(fqn, "proj.src.mod.mymod.my_func");
    }

    #[test]
    fn fortran_module_nested_path() {
        let fqn = FqnGenerator::generate_for_module(
            "proj",
            "src/physics/solver.f90",
            "solver_mod",
            "solve",
        );
        assert_eq!(fqn, "proj.src.physics.solver.solver_mod.solve");
    }

    #[test]
    fn fortran_module_with_backslash_path() {
        let fqn = FqnGenerator::generate_for_module("proj", "src\\physics\\solver.f90", "m", "e");
        assert_eq!(fqn, "proj.src.physics.solver.m.e");
    }

    // --- parent_qn ---

    #[test]
    fn parent_qn_returns_parent() {
        let parent = FqnGenerator::parent_qn("proj.src.main.Parser.parse");
        assert_eq!(parent.as_deref(), Some("proj.src.main.Parser"));
    }

    #[test]
    fn parent_qn_for_top_level() {
        let parent = FqnGenerator::parent_qn("proj.src.main");
        assert_eq!(parent.as_deref(), Some("proj.src"));
    }

    #[test]
    fn parent_qn_for_single_segment_returns_none() {
        let parent = FqnGenerator::parent_qn("proj");
        assert!(parent.is_none());
    }

    #[test]
    fn parent_qn_for_two_segments() {
        let parent = FqnGenerator::parent_qn("proj.entity");
        assert_eq!(parent.as_deref(), Some("proj"));
    }

    #[test]
    fn parent_qn_for_empty_string_returns_none() {
        let parent = FqnGenerator::parent_qn("");
        assert!(parent.is_none());
    }

    // --- Path normalization ---

    #[test]
    fn strips_leading_dot_slash() {
        let fqn = FqnGenerator::generate("proj", "./src/main.rs", "foo", Language::Rust);
        assert_eq!(fqn, "proj.src.main.foo");
    }

    #[test]
    fn normalizes_backslashes_to_forward_slashes() {
        let fqn = FqnGenerator::generate("proj", "src\\main.rs", "foo", Language::Rust);
        assert_eq!(fqn, "proj.src.main.foo");
    }

    #[test]
    fn normalizes_mixed_separators() {
        let fqn =
            FqnGenerator::generate("proj", "src\\deep/nested\\file.rs", "foo", Language::Rust);
        assert_eq!(fqn, "proj.src.deep.nested.file.foo");
    }

    #[test]
    fn strips_leading_dot_slash_only_once() {
        // Per spec: "remove leading ./" — only one occurrence is stripped.
        // "././src/main.rs" -> "./src/main.rs" -> segments [".", "src", "main"]
        let fqn = FqnGenerator::generate("proj", "././src/main.rs", "foo", Language::Rust);
        assert_eq!(fqn, "proj...src.main.foo");
    }

    // --- Different extensions ---

    #[test]
    fn removes_c_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.c", "e", Language::C);
        assert_eq!(fqn, "p.src.f.e");
    }

    #[test]
    fn removes_h_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.h", "e", Language::C);
        assert_eq!(fqn, "p.src.f.e");
    }

    #[test]
    fn removes_rs_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.rs", "e", Language::Rust);
        assert_eq!(fqn, "p.src.f.e");
    }

    #[test]
    fn removes_py_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.py", "e", Language::Python);
        assert_eq!(fqn, "p.src.f.e");
    }

    #[test]
    fn removes_ts_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.ts", "e", Language::TypeScript);
        assert_eq!(fqn, "p.src.f.e");
    }

    #[test]
    fn removes_f90_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.f90", "e", Language::Fortran);
        assert_eq!(fqn, "p.src.f.e");
    }

    #[test]
    fn removes_tsx_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.tsx", "e", Language::TypeScript);
        assert_eq!(fqn, "p.src.f.e");
    }

    #[test]
    fn removes_f95_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.f95", "e", Language::Fortran);
        assert_eq!(fqn, "p.src.f.e");
    }

    // --- Edge cases ---

    #[test]
    fn empty_file_path() {
        let fqn = FqnGenerator::generate("proj", "", "entity", Language::Rust);
        assert_eq!(fqn, "proj.entity");
    }

    #[test]
    fn filename_only_no_directory() {
        let fqn = FqnGenerator::generate("proj", "main.rs", "foo", Language::Rust);
        assert_eq!(fqn, "proj.main.foo");
    }

    #[test]
    fn multiple_dots_in_filename() {
        // Only the last extension should be removed.
        let fqn = FqnGenerator::generate("p", "src/foo.test.rs", "e", Language::Rust);
        assert_eq!(fqn, "p.src.foo.test.e");
    }

    #[test]
    fn no_extension() {
        let fqn = FqnGenerator::generate("p", "src/Makefile", "e", Language::Rust);
        assert_eq!(fqn, "p.src.Makefile.e");
    }

    #[test]
    fn trailing_slash_in_path() {
        let fqn = FqnGenerator::generate("p", "src/dir/", "e", Language::Rust);
        // Trailing slash produces an empty segment which is filtered out.
        assert_eq!(fqn, "p.src.dir.e");
    }

    #[test]
    fn fortran_generate_for_module_does_not_apply_init_rule() {
        // generate_for_module uses Fortran language; __init__ would not be
        // special-cased (only Python triggers that).
        let fqn = FqnGenerator::generate_for_module("p", "src/__init__.f90", "m", "e");
        assert_eq!(fqn, "p.src.__init__.m.e");
    }

    #[test]
    fn typescript_file() {
        let fqn = FqnGenerator::generate(
            "proj",
            "src/components/Button.tsx",
            "Button",
            Language::TypeScript,
        );
        assert_eq!(fqn, "proj.src.components.Button.Button");
    }

    #[test]
    fn empty_project_name() {
        let fqn = FqnGenerator::generate("", "src/main.rs", "foo", Language::Rust);
        assert_eq!(fqn, ".src.main.foo");
    }

    #[test]
    fn empty_entity_name() {
        let fqn = FqnGenerator::generate("proj", "src/main.rs", "", Language::Rust);
        assert_eq!(fqn, "proj.src.main.");
    }
}
