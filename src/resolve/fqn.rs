// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Fully-qualified name (FQN) generation (ADD §7.1).
//!
//! Generates dot-separated FQNs in the format
//! `project.dir.subdir.file_full.entity_name[#disambiguator]`, with special
//! handling for Python `__init__.py`, C header files, and Fortran modules.
//! The file segment retains its full name (including extension) and the
//! optional disambiguator disambiguates same-name entities in a file.

use crate::model::Language;

/// Generates fully-qualified names following ADD §7.1.
///
/// Format: `project.dir.file_full.entity_name[#disambiguator]` (dot-separated).
pub struct FqnGenerator;

impl FqnGenerator {
    /// Generates an FQN for a top-level entity in a file.
    ///
    /// # Format
    ///
    /// `project.dir.file_full.entity_name[#disambiguator]`
    ///
    /// The file segment retains its full name (including extension). The
    /// optional `disambiguator` (ADR-003) disambiguates same-name entities in
    /// the same file (e.g. `impl Foo { fn new() }` vs `impl Bar { fn new() }`)
    /// by appending `#parent_type` to the FQN.
    ///
    /// # Special cases
    ///
    /// - Python `__init__.py`: the `__init__.py` segment is removed, so the
    ///   FQN uses the package path directly (e.g. `proj.src.pkg.MyClass`).
    /// - C header files: treated like any other file
    ///   (`proj.include.header.h.DEFINE`).
    /// - Path normalization: leading `./` is stripped and backslashes are
    ///   converted to forward slashes.
    /// - Directory segment dot replacement: dots in directory names are
    ///   replaced with underscores (ADR-002).
    #[must_use]
    pub fn generate(
        project: &str,
        file_path: &str,
        entity_name: &str,
        language: Language,
        disambiguator: Option<&str>,
    ) -> String {
        let segments = Self::path_segments(file_path, language);
        let mut parts: Vec<String> = vec![project.to_string()];
        parts.extend(segments);
        parts.push(entity_name.to_string());
        let mut fqn = parts.join(".");
        Self::append_disambiguator(&mut fqn, disambiguator);
        fqn
    }

    /// Generates an FQN for an entity nested inside a module (Fortran).
    ///
    /// Format: `project.file.module.entity[#disambiguator]`.
    ///
    /// # Reserved
    ///
    /// The `disambiguator` parameter is currently unused by any production
    /// caller (Fortran module extraction does not yet require disambiguation).
    /// It is retained for API symmetry with [`generate`] and to avoid a
    /// future breaking change if Fortran module-level disambiguation becomes
    /// necessary.
    #[must_use]
    pub fn generate_for_module(
        project: &str,
        file_path: &str,
        module_name: &str,
        entity_name: &str,
        disambiguator: Option<&str>,
    ) -> String {
        let segments = Self::path_segments(file_path, Language::Fortran);
        let mut parts: Vec<String> = vec![project.to_string()];
        parts.extend(segments);
        parts.push(module_name.to_string());
        parts.push(entity_name.to_string());
        let mut fqn = parts.join(".");
        Self::append_disambiguator(&mut fqn, disambiguator);
        fqn
    }

    /// Appends `#sanitized_disambiguator` to `fqn` when `disambiguator` is
    /// `Some` (ADR-003). Only alphanumeric and underscore are retained; all
    /// other characters (including generic-lifetime syntax like `<'a, C>`,
    /// spaces, commas) become underscores so they cannot corrupt the FQN's
    /// dot/hash structure or downstream CSV serialization.
    fn append_disambiguator(fqn: &mut String, disambiguator: Option<&str>) {
        if let Some(d) = disambiguator {
            fqn.push('#');
            for c in d.chars() {
                fqn.push(if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' });
            }
        }
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
    /// Normalizes the path (forward slashes, no leading `./`). The last
    /// segment (file name) retains its full name **including extension** so
    /// that files sharing a stem but differing in extension (e.g. `foo.ts`
    /// vs `foo.tsx`) produce distinct FQNs (ADR-001). Directory segments have
    /// their dots replaced with underscores so a directory name like
    /// `kpp-2.1` is not misread as FQN dot separators (ADR-002). For Python
    /// `__init__.py`, the `__init__.py` segment is removed entirely so the
    /// FQN uses the package path directly.
    fn path_segments(file_path: &str, language: Language) -> Vec<String> {
        // Normalize: backslashes -> forward slashes, strip leading "./"
        let normalized = file_path.replace('\\', "/");
        let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);

        let mut segments: Vec<String> = normalized
            .split('/')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        // ADR-002: Directory segments (everything except the last/file
        // segment) have dots replaced with underscores so directory names
        // like `kpp-2.1` do not collide with FQN dot separators.
        if segments.len() > 1 {
            let last_idx = segments.len() - 1;
            for seg in &mut segments[..last_idx] {
                *seg = seg.replace('.', "_");
            }
        }

        // Python __init__.py: drop the "__init__.py" segment so the FQN uses
        // the package path directly. (ADR-001: full filename is retained, so
        // the comparison is against the full "__init__.py" not "__init__".)
        if language == Language::Python
            && segments.last().is_some_and(|s| s == "__init__.py")
        {
            segments.pop();
        }

        segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Basic generation (ADR-001: full filename retained) ---

    #[test]
    fn basic_rust_file() {
        let fqn = FqnGenerator::generate("myproject", "src/main.rs", "parse", Language::Rust, None);
        assert_eq!(fqn, "myproject.src.main.rs.parse");
    }

    #[test]
    fn nested_path() {
        let fqn = FqnGenerator::generate(
            "myproject",
            "src/deep/nested/file.rs",
            "entity",
            Language::Rust,
            None,
        );
        assert_eq!(fqn, "myproject.src.deep.nested.file.rs.entity");
    }

    // --- Python __init__.py special case ---

    #[test]
    fn python_init_py_removes_init_segment() {
        let fqn = FqnGenerator::generate(
            "myproject",
            "src/pkg/__init__.py",
            "MyClass",
            Language::Python,
            None,
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
            None,
        );
        assert_eq!(fqn, "myproject.src.pkg.module.py.MyClass");
    }

    #[test]
    fn python_init_py_at_root() {
        let fqn = FqnGenerator::generate("proj", "__init__.py", "X", Language::Python, None);
        assert_eq!(fqn, "proj.X");
    }

    #[test]
    fn python_init_py_in_nested_dir() {
        let fqn = FqnGenerator::generate(
            "proj",
            "src/a/b/__init__.py",
            "Y",
            Language::Python,
            None,
        );
        assert_eq!(fqn, "proj.src.a.b.Y");
    }

    // --- C header files ---

    #[test]
    fn c_header_file() {
        let fqn = FqnGenerator::generate(
            "myproject",
            "include/header.h",
            "MY_DEFINE",
            Language::C,
            None,
        );
        assert_eq!(fqn, "myproject.include.header.h.MY_DEFINE");
    }

    #[test]
    fn c_source_file() {
        let fqn = FqnGenerator::generate("proj", "src/main.c", "main", Language::C, None);
        assert_eq!(fqn, "proj.src.main.c.main");
    }

    // --- Fortran module ---

    #[test]
    fn fortran_module_entity() {
        let fqn = FqnGenerator::generate_for_module(
            "proj",
            "src/mod.f90",
            "mymod",
            "my_func",
            None,
        );
        assert_eq!(fqn, "proj.src.mod.f90.mymod.my_func");
    }

    #[test]
    fn fortran_module_nested_path() {
        let fqn = FqnGenerator::generate_for_module(
            "proj",
            "src/physics/solver.f90",
            "solver_mod",
            "solve",
            None,
        );
        assert_eq!(fqn, "proj.src.physics.solver.f90.solver_mod.solve");
    }

    #[test]
    fn fortran_module_with_backslash_path() {
        let fqn = FqnGenerator::generate_for_module(
            "proj",
            "src\\physics\\solver.f90",
            "m",
            "e",
            None,
        );
        assert_eq!(fqn, "proj.src.physics.solver.f90.m.e");
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
        let fqn = FqnGenerator::generate("proj", "./src/main.rs", "foo", Language::Rust, None);
        assert_eq!(fqn, "proj.src.main.rs.foo");
    }

    #[test]
    fn normalizes_backslashes_to_forward_slashes() {
        let fqn = FqnGenerator::generate("proj", "src\\main.rs", "foo", Language::Rust, None);
        assert_eq!(fqn, "proj.src.main.rs.foo");
    }

    #[test]
    fn normalizes_mixed_separators() {
        let fqn = FqnGenerator::generate(
            "proj",
            "src\\deep/nested\\file.rs",
            "foo",
            Language::Rust,
            None,
        );
        assert_eq!(fqn, "proj.src.deep.nested.file.rs.foo");
    }

    #[test]
    fn strips_leading_dot_slash_only_once() {
        // Per spec: "remove leading ./" — only one occurrence is stripped.
        // "././src/main.rs" -> "./src/main.rs" -> segments [".", "src", "main.rs"]
        // ADR-002: directory segments have dots replaced with underscores,
        // so the leading "." segment becomes "_".
        let fqn = FqnGenerator::generate("proj", "././src/main.rs", "foo", Language::Rust, None);
        assert_eq!(fqn, "proj._.src.main.rs.foo");
    }

    // --- ADR-001: Extension retention (formerly "removes_*_extension") ---

    #[test]
    fn retains_c_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.c", "e", Language::C, None);
        assert_eq!(fqn, "p.src.f.c.e");
    }

    #[test]
    fn retains_h_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.h", "e", Language::C, None);
        assert_eq!(fqn, "p.src.f.h.e");
    }

    #[test]
    fn retains_rs_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.rs", "e", Language::Rust, None);
        assert_eq!(fqn, "p.src.f.rs.e");
    }

    #[test]
    fn retains_py_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.py", "e", Language::Python, None);
        assert_eq!(fqn, "p.src.f.py.e");
    }

    #[test]
    fn retains_ts_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.ts", "e", Language::TypeScript, None);
        assert_eq!(fqn, "p.src.f.ts.e");
    }

    #[test]
    fn retains_f90_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.f90", "e", Language::Fortran, None);
        assert_eq!(fqn, "p.src.f.f90.e");
    }

    #[test]
    fn retains_tsx_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.tsx", "e", Language::TypeScript, None);
        assert_eq!(fqn, "p.src.f.tsx.e");
    }

    #[test]
    fn retains_f95_extension() {
        let fqn = FqnGenerator::generate("p", "src/f.f95", "e", Language::Fortran, None);
        assert_eq!(fqn, "p.src.f.f95.e");
    }

    // --- Edge cases ---

    #[test]
    fn empty_file_path() {
        let fqn = FqnGenerator::generate("proj", "", "entity", Language::Rust, None);
        assert_eq!(fqn, "proj.entity");
    }

    #[test]
    fn filename_only_no_directory() {
        let fqn = FqnGenerator::generate("proj", "main.rs", "foo", Language::Rust, None);
        assert_eq!(fqn, "proj.main.rs.foo");
    }

    #[test]
    fn multiple_dots_in_filename() {
        // ADR-001: full filename is retained (no extension stripping).
        let fqn = FqnGenerator::generate("p", "src/foo.test.rs", "e", Language::Rust, None);
        assert_eq!(fqn, "p.src.foo.test.rs.e");
    }

    #[test]
    fn no_extension() {
        let fqn = FqnGenerator::generate("p", "src/Makefile", "e", Language::Rust, None);
        assert_eq!(fqn, "p.src.Makefile.e");
    }

    #[test]
    fn trailing_slash_in_path() {
        let fqn = FqnGenerator::generate("p", "src/dir/", "e", Language::Rust, None);
        // Trailing slash produces an empty segment which is filtered out.
        assert_eq!(fqn, "p.src.dir.e");
    }

    #[test]
    fn fortran_generate_for_module_does_not_apply_init_rule() {
        // generate_for_module uses Fortran language; __init__ would not be
        // special-cased (only Python triggers that). ADR-001: full filename
        // retained, so the file segment is "__init__.f90".
        let fqn = FqnGenerator::generate_for_module("p", "src/__init__.f90", "m", "e", None);
        assert_eq!(fqn, "p.src.__init__.f90.m.e");
    }

    #[test]
    fn typescript_file() {
        let fqn = FqnGenerator::generate(
            "proj",
            "src/components/Button.tsx",
            "Button",
            Language::TypeScript,
            None,
        );
        assert_eq!(fqn, "proj.src.components.Button.tsx.Button");
    }

    #[test]
    fn empty_project_name() {
        let fqn = FqnGenerator::generate("", "src/main.rs", "foo", Language::Rust, None);
        assert_eq!(fqn, ".src.main.rs.foo");
    }

    #[test]
    fn empty_entity_name() {
        let fqn = FqnGenerator::generate("proj", "src/main.rs", "", Language::Rust, None);
        assert_eq!(fqn, "proj.src.main.rs.");
    }

    // --- ADR-001/002/003: New disambiguation tests ---

    #[test]
    fn file_full_name_preserved() {
        // ADR-001: file segment retains full name including extension.
        let fqn = FqnGenerator::generate("p", "src/f.rs", "e", Language::Rust, None);
        assert_eq!(fqn, "p.src.f.rs.e");
    }

    #[test]
    fn multi_extension_file_preserved() {
        // ADR-001: multi-extension file names like go.test.ts are retained
        // whole so that go.test.ts and go.test.tsx produce distinct FQNs.
        let fqn = FqnGenerator::generate("p", "src/go.test.ts", "e", Language::TypeScript, None);
        assert_eq!(fqn, "p.src.go.test.ts.e");
    }

    #[test]
    fn dir_segment_dot_replaced() {
        // ADR-002: dots in directory segments are replaced with underscores
        // so directory names like `kpp-2.1` do not collide with FQN dot
        // separators.
        let fqn = FqnGenerator::generate("p", "kpp-2.1/SDIRK.f90", "e", Language::Fortran, None);
        assert_eq!(fqn, "p.kpp-2_1.SDIRK.f90.e");
    }

    #[test]
    fn disambiguator_with_parent_type() {
        // ADR-003: parent-type disambiguator appends #parent to the FQN.
        let fqn = FqnGenerator::generate(
            "p",
            "src/main.rs",
            "new",
            Language::Rust,
            Some("SymbolTable"),
        );
        assert_eq!(fqn, "p.src.main.rs.new#SymbolTable");
    }

    #[test]
    fn disambiguator_none_no_suffix() {
        // ADR-003: when disambiguator is None, no # suffix is appended.
        let fqn = FqnGenerator::generate("p", "src/main.rs", "add", Language::Rust, None);
        assert_eq!(fqn, "p.src.main.rs.add");
    }

    #[test]
    fn disambiguator_sanitizes_special_chars() {
        // ADR-003: special characters in the disambiguator are replaced with
        // underscores so they cannot corrupt the FQN's dot/hash structure or
        // downstream CSV serialization. Only alphanumeric + underscore are
        // retained; all other chars (including generic-lifetime syntax like
        // `<'a, C>`) become underscores.
        let fqn = FqnGenerator::generate(
            "p",
            "src/main.rs",
            "new",
            Language::Rust,
            Some("a.b/c#d<'a, C>"),
        );
        assert_eq!(fqn, "p.src.main.rs.new#a_b_c_d__a__C_");
    }
}
