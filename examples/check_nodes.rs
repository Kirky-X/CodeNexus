fn main() {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_c::LANGUAGE.into()).unwrap();
    let src = "namespace alpha { class Token {}; }\nnamespace beta { class Token {}; }\n";
    let tree = parser.parse(src, None).unwrap();
    fn walk(node: tree_sitter::Node, depth: usize, src: &str) {
        let text = node_text(node, src);
        let mut field_info = String::new();
        for i in 0..node.child_count() {
            let child = node.child(i as u32).unwrap();
            if let Some(fname) = child.field_name_for_child(i as u32) {
                field_info.push_str(&format!(" [{}={:?}]", fname, node_text(child, src)));
            }
        }
        if !text.is_empty() && text.len() < 60 {
            println!("{}{} = {:?}{}", "  ".repeat(depth), node.kind(), text, field_info);
        } else {
            println!("{}{}{}", "  ".repeat(depth), node.kind(), field_info);
        }
        for i in 0..node.named_child_count() {
            walk(node.named_child(i as u32).unwrap(), depth + 1, src);
        }
    }
    walk(tree.root_node(), 0, src);
}
fn node_text(node: tree_sitter::Node, src: &str) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    src[start..end].trim().to_string()
}
