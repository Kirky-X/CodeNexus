//! Fortran language extractor (Adapter pattern, ADR-003, ADR-011).
//!
//! Adapts tree-sitter-fortran's syntax tree into CodeNexus nodes, edges, and
//! intermediate extraction records ([`ExtractResult`]).
//!
//! # Extracted node types
//!
//! - `module` → [`NodeLabel::Module`]
//! - `subroutine` → [`NodeLabel::Function`]
//! - `function` → [`NodeLabel::Function`]
//! - `program` → [`NodeLabel::Function`] (treated as a function)
//!
//! # Extracted records
//!
//! - `use_statement` → [`ImportInfo`]
//! - `subroutine_call` / `call_statement` → [`CallInfo`]
//! - `use iso_c_binding` → [`ExternInfo`] (FFI detection)

use tree_sitter::Node;

use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::resolve::FqnGenerator;

use super::error::{ParseError, Result};
use super::extractor::{CallInfo, ExternInfo, ExtractResult, Extractor, ImportInfo};
use super::parser_factory::ParserFactory;

/// Fortran language tree-sitter extractor (Adapter pattern).
pub struct FortranExtractor {
    _priv: (),
}

impl FortranExtractor {
    /// Creates a new `FortranExtractor`.
    #[must_use]
    pub const fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for FortranExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for FortranExtractor {
    fn language(&self) -> Language {
        Language::Fortran
    }

    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
        let mut result = ExtractResult::new(file_path, Language::Fortran);
        // TODO: implement reads/writes extraction for Fortran (BR-TRACE-005/006).
        // `result.reads` and `result.writes` are left empty for now; downstream
        // resolution gracefully produces no Reads/Writes edges when absent.
        let mut parser = ParserFactory::create_parser(Language::Fortran)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| ParseError::ParseFailed {
                file_path: file_path.to_string(),
            })?;
        let root = tree.root_node();
        for i in 0..root.named_child_count() as u32 {
            if let Some(child) = root.named_child(i) {
                visit_node(child, source, file_path, project, &mut result, None, None);
            }
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Tree-walking helpers
// ---------------------------------------------------------------------------

fn visit_node(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
    current_func: Option<&str>,
    current_parent: Option<&str>,
) {
    match node.kind() {
        "module" => {
            extract_module(node, source, file_path, project, result);
            // Extract the module name and pass it as current_parent so
            // entities inside the module can disambiguate their FQN (ADR-005).
            let module_name = statement_name(node, "module_statement", source);
            visit_children(
                node,
                source,
                file_path,
                project,
                result,
                current_func,
                module_name.as_deref(),
            );
        }
        "subroutine" => {
            extract_subroutine_or_function(
                node,
                source,
                file_path,
                project,
                result,
                "subroutine_statement",
                current_parent,
            );
            // Pass the subroutine's name as the enclosing function for body
            // traversal, so calls inside it can be attributed to it.
            let func_name = statement_name(node, "subroutine_statement", source);
            visit_children(
                node,
                source,
                file_path,
                project,
                result,
                func_name.as_deref(),
                current_parent,
            );
        }
        "function" => {
            extract_subroutine_or_function(
                node,
                source,
                file_path,
                project,
                result,
                "function_statement",
                current_parent,
            );
            let func_name = statement_name(node, "function_statement", source);
            visit_children(
                node,
                source,
                file_path,
                project,
                result,
                func_name.as_deref(),
                current_parent,
            );
        }
        "program" => {
            extract_program(node, source, file_path, project, result);
            let func_name = statement_name(node, "program_statement", source);
            visit_children(
                node,
                source,
                file_path,
                project,
                result,
                func_name.as_deref(),
                current_parent,
            );
        }
        "use_statement" => {
            extract_use(node, source, result);
        }
        "subroutine_call" | "call_statement" => {
            extract_call(node, source, file_path, project, current_func, current_parent, result);
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
        _ => {
            visit_children(node, source, file_path, project, result, current_func, current_parent);
        }
    }
}

fn visit_children(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
    current_func: Option<&str>,
    current_parent: Option<&str>,
) {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            visit_node(child, source, file_path, project, result, current_func, current_parent);
        }
    }
}

// ---------------------------------------------------------------------------
// Definition extractors
// ---------------------------------------------------------------------------

fn extract_module(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
) {
    let Some(name) = statement_name(node, "module_statement", source) else {
        return;
    };
    let qn = make_qn(file_path, &name, project, None);
    let model_node = ModelNode::builder(NodeLabel::Module, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Fortran)
        .project(project)
        .is_global(true)
        .build();
    add_definition_edges(file_path, project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_subroutine_or_function(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
    statement_kind: &str,
    parent: Option<&str>,
) {
    let Some(name) = statement_name(node, statement_kind, source) else {
        return;
    };
    let qn = make_qn(file_path, &name, project, parent);
    let signature = node_text(node, source).map(String::from);
    let mut builder = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Fortran)
        .project(project)
        .is_global(true);
    if let Some(sig) = signature {
        builder = builder.signature(sig);
    }
    let model_node = builder.build();
    add_definition_edges(file_path, project, &model_node, result);
    result.nodes.push(model_node);
}

fn extract_program(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    result: &mut ExtractResult,
) {
    let Some(name) = statement_name(node, "program_statement", source) else {
        return;
    };
    let qn = make_qn(file_path, &name, project, None);
    let model_node = ModelNode::builder(NodeLabel::Function, name, qn)
        .file_path(file_path)
        .start_line(node.start_position().row as u32 + 1)
        .end_line(node.end_position().row as u32 + 1)
        .language(Language::Fortran)
        .project(project)
        .is_global(true)
        .build();
    add_definition_edges(file_path, project, &model_node, result);
    result.nodes.push(model_node);
}

// ---------------------------------------------------------------------------
// Record extractors
// ---------------------------------------------------------------------------

fn extract_use(node: Node, source: &str, result: &mut ExtractResult) {
    // use_statement has a module_name child.
    let mut module_name = None;
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "module_name" {
                module_name = node_text(child, source).map(String::from);
                break;
            }
        }
    }
    let Some(name) = module_name else {
        return;
    };
    let line = node.start_position().row as u32 + 1;
    // Detect iso_c_binding for FFI.
    if name.eq_ignore_ascii_case("iso_c_binding") {
        result.externs.push(ExternInfo {
            language: Language::C,
            names: Vec::new(),
            line,
            signature: Some(name.clone()),
        });
    }
    result.imports.push(ImportInfo {
        source_file: name,
        imported_names: Vec::new(),
        line,
    });
}

fn extract_call(
    node: Node,
    source: &str,
    file_path: &str,
    project: &str,
    current_func: Option<&str>,
    current_parent: Option<&str>,
    result: &mut ExtractResult,
) {
    // subroutine_call has an identifier child (the callee) and an argument_list.
    let mut callee = None;
    let mut args = Vec::new();
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            match child.kind() {
                "identifier" => {
                    if callee.is_none() {
                        callee = node_text(child, source).map(String::from);
                    }
                }
                "argument_list" => {
                    for j in 0..child.named_child_count() as u32 {
                        if let Some(arg) = child.named_child(j) {
                            if let Ok(text) = arg.utf8_text(source.as_bytes()) {
                                args.push(text.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let Some(callee) = callee else {
        return;
    };
    let caller_qn = current_func.map(|name| make_qn(file_path, name, project, current_parent));
    result.calls.push(CallInfo {
        caller_qn,
        callee_name: callee,
        line: node.start_position().row as u32 + 1,
        args,
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the name from a statement child node
/// (e.g. `module_statement`, `subroutine_statement`).
/// Tries the `name` field first, then falls back to a child with kind `name`.
fn statement_name(node: Node, statement_kind: &str, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == statement_kind {
                // Try the `name` field first.
                if let Some(name_node) = child.child_by_field_name("name") {
                    if let Some(text) = node_text(name_node, source) {
                        return Some(text.to_string());
                    }
                }
                // Fall back to a named child with kind `name` or `identifier`.
                for j in 0..child.named_child_count() as u32 {
                    if let Some(grandchild) = child.named_child(j) {
                        if grandchild.kind() == "name" || grandchild.kind() == "identifier" {
                            if let Some(text) = node_text(grandchild, source) {
                                return Some(text.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

fn make_qn(file_path: &str, name: &str, project: &str, parent: Option<&str>) -> String {
    FqnGenerator::generate(project, file_path, name, Language::Fortran, parent)
}

fn add_definition_edges(
    file_path: &str,
    project: &str,
    node: &ModelNode,
    result: &mut ExtractResult,
) {
    result.edges.push(Edge::new(
        file_path.to_string(),
        node.id.clone(),
        EdgeType::Contains,
        project,
    ));
    result.edges.push(Edge::new(
        file_path.to_string(),
        node.id.clone(),
        EdgeType::Defines,
        project,
    ));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeLabel;

    const FORTRAN_SOURCE: &str = r#"module mymod
    use iso_c_binding
contains
    subroutine my_sub(a, b)
        integer, intent(in) :: a
        integer, intent(out) :: b
        b = a + 1
    end subroutine
    function my_func(x) result(y)
        integer, intent(in) :: x
        integer :: y
        y = x * 2
    end function
end module

program main
    use mymod
    integer :: a, b
    call my_sub(1, b)
end program
"#;

    fn extract(source: &str) -> ExtractResult {
        let ext = FortranExtractor::new();
        ext.extract(source, "test.f90", "proj").expect("extraction should succeed")
    }

    #[test]
    fn language_returns_fortran() {
        assert_eq!(FortranExtractor::new().language(), Language::Fortran);
    }

    #[test]
    fn default_creates_extractor() {
        let ext = FortranExtractor::default();
        assert_eq!(ext.language(), Language::Fortran);
    }

    #[test]
    fn extracts_module() {
        let result = extract(FORTRAN_SOURCE);
        let modules: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Module).collect();
        assert_eq!(modules.len(), 1, "should extract 1 module");
        assert_eq!(modules[0].name, "mymod");
        assert_eq!(modules[0].language, Some(Language::Fortran));
        assert_eq!(modules[0].project, "proj");
        assert_eq!(modules[0].file_path.as_deref(), Some("test.f90"));
    }

    #[test]
    fn extracts_subroutine() {
        let result = extract(FORTRAN_SOURCE);
        let funcs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Function).collect();
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"my_sub"),
            "should extract my_sub subroutine: {:?}",
            names
        );
    }

    #[test]
    fn extracts_function() {
        let result = extract(FORTRAN_SOURCE);
        let funcs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Function).collect();
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"my_func"),
            "should extract my_func function: {:?}",
            names
        );
    }

    #[test]
    fn extracts_program() {
        let result = extract(FORTRAN_SOURCE);
        let funcs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Function).collect();
        let names: Vec<_> = funcs.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"main"),
            "should extract main program: {:?}",
            names
        );
    }

    #[test]
    fn extracts_use_statements() {
        let result = extract(FORTRAN_SOURCE);
        // Two use statements: iso_c_binding and mymod.
        assert_eq!(result.imports.len(), 2, "should extract 2 use statements");
        let sources: Vec<_> = result.imports.iter().map(|i| i.source_file.as_str()).collect();
        assert!(sources.contains(&"iso_c_binding"));
        assert!(sources.contains(&"mymod"));
    }

    #[test]
    fn extracts_call_to_my_sub() {
        let result = extract(FORTRAN_SOURCE);
        let callees: Vec<_> = result.calls.iter().map(|c| c.callee_name.as_str()).collect();
        assert!(
            callees.contains(&"my_sub"),
            "should extract call to my_sub: {:?}",
            callees
        );
    }

    #[test]
    fn call_has_line_and_args() {
        let result = extract(FORTRAN_SOURCE);
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "my_sub")
            .expect("call to my_sub should exist");
        assert_eq!(call.line, 19);
        assert_eq!(call.args.len(), 2, "my_sub(1, b) should have 2 args");
    }

    #[test]
    fn detects_iso_c_binding_ffi() {
        let result = extract(FORTRAN_SOURCE);
        assert!(
            !result.externs.is_empty(),
            "should detect iso_c_binding as FFI"
        );
        let ext = &result.externs[0];
        assert_eq!(ext.language, Language::C, "iso_c_binding should map to C");
    }

    #[test]
    fn creates_contains_and_defines_edges() {
        let result = extract(FORTRAN_SOURCE);
        let contains_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Contains).count();
        let defines_count = result.edges.iter().filter(|e| e.edge_type == EdgeType::Defines).count();
        let node_count = result.nodes.len();
        assert_eq!(contains_count, node_count);
        assert_eq!(defines_count, node_count);
    }

    #[test]
    fn qualified_name_uses_file_path_and_name() {
        let result = extract(FORTRAN_SOURCE);
        let mymod = result.nodes.iter().find(|n| n.name == "mymod").unwrap();
        assert_eq!(mymod.qualified_name, "proj.test.f90.mymod");
    }

    #[test]
    fn module_entity_has_module_disambiguator() {
        // ADR-005: entities inside a Fortran module must carry the module
        // name as a disambiguator so same-name subroutines in different
        // modules do not collide.
        let result = extract(FORTRAN_SOURCE);
        let my_sub = result
            .nodes
            .iter()
            .find(|n| n.name == "my_sub")
            .expect("should find my_sub");
        assert_eq!(
            my_sub.qualified_name, "proj.test.f90.my_sub#mymod",
            "module-internal subroutine should carry #module disambiguator"
        );
        let my_func = result
            .nodes
            .iter()
            .find(|n| n.name == "my_func")
            .expect("should find my_func");
        assert_eq!(
            my_func.qualified_name, "proj.test.f90.my_func#mymod",
            "module-internal function should carry #module disambiguator"
        );
    }

    #[test]
    fn empty_source_returns_empty_result() {
        let result = extract("");
        assert!(result.is_empty());
    }

    #[test]
    fn subroutine_has_signature() {
        let result = extract(FORTRAN_SOURCE);
        let my_sub = result.nodes.iter().find(|n| n.name == "my_sub").unwrap();
        assert!(my_sub.signature.is_some(), "subroutine should have a signature");
        assert!(my_sub.signature.as_deref().unwrap().contains("my_sub"));
    }

    #[test]
    fn result_language_is_fortran() {
        let result = extract(FORTRAN_SOURCE);
        assert_eq!(result.language, Language::Fortran);
        assert_eq!(result.file_path, "test.f90");
    }

    #[test]
    fn handles_standalone_subroutine() {
        let src = "subroutine foo(a)\n    integer :: a\nend subroutine\n";
        let result = extract(src);
        let funcs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Function).collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "foo");
    }

    #[test]
    fn handles_standalone_function() {
        let src = "function bar(x) result(y)\n    integer :: x, y\n    y = x\nend function\n";
        let result = extract(src);
        let funcs: Vec<_> = result.nodes.iter().filter(|n| n.label == NodeLabel::Function).collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "bar");
    }

    #[test]
    fn use_without_iso_c_binding_no_extern() {
        let src = "program p\n    use other_mod\nend program\n";
        let result = extract(src);
        assert!(
            result.externs.is_empty(),
            "non-iso_c_binding use should not create extern"
        );
    }

    #[test]
    fn call_in_function_has_dotted_fqn_caller_qn() {
        // Spec: Fortran 函数内调用生成非 None caller_qn (点分 FQN 格式)。
        let src = "subroutine caller()\n    call callee()\nend subroutine\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "/tmp/demo/main.f90", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.tmp.demo.main.f90.caller"),
            "caller_qn should be the dotted FQN of the enclosing subroutine"
        );
        // The caller FQN must match the enclosing subroutine's node id.
        let caller_node = result
            .nodes
            .iter()
            .find(|n| n.name == "caller")
            .expect("should find caller subroutine node");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some(caller_node.qualified_name.as_str()),
            "caller_qn must match the caller subroutine node id"
        );
    }

    #[test]
    fn call_in_program_has_program_caller_qn() {
        // Spec intent: 顶层调用 caller_qn 为 None。Fortran 语义冲突 surfaced
        // (Rule 7/12): Fortran 要求每条可执行语句必须位于 program/subroutine/
        // function 内，且 program 被当作 NodeLabel::Function（见模块文档第 11
        // 行）。因此 Fortran 不存在 Python/TypeScript 意义上的"模块顶层调用"。
        // 这里验证等价语义：program 内的调用 caller_qn 应为 program 自身的
        // 点分 FQN，且与 program 节点的 qualified_name 一致。
        let src = "program main\n    call callee()\nend program\n";
        let ext = FortranExtractor::new();
        let result = ext
            .extract(src, "main.f90", "proj")
            .expect("extraction should succeed");
        let call = result
            .calls
            .iter()
            .find(|c| c.callee_name == "callee")
            .expect("should find call to callee inside program");
        let program_node = result
            .nodes
            .iter()
            .find(|n| n.name == "main")
            .expect("should find program main node");
        assert_eq!(
            call.caller_qn.as_deref(),
            Some(program_node.qualified_name.as_str()),
            "call inside program should have caller_qn matching the program's FQN"
        );
        assert_eq!(
            call.caller_qn.as_deref(),
            Some("proj.main.f90.main"),
            "caller_qn should be the dotted FQN of the enclosing program"
        );
    }
}
