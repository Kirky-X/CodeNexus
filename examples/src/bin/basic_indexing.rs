use std::fs;

use codenexus::index::IndexFacade;
use codenexus::query::QueryFacade;
use tempfile::TempDir;

const SAMPLE_CODE: &str = r#"
pub struct Parser {
    depth: usize,
}

impl Parser {
    pub fn new() -> Self {
        Self { depth: 0 }
    }

    pub fn parse(&mut self, input: &str) -> Vec<String> {
        self.depth += 1;
        let tokens = tokenize(input);
        tokens
    }
}

fn tokenize(input: &str) -> Vec<String> {
    input.split_whitespace().map(String::from).collect()
}

pub fn validate(tokens: &[String]) -> bool {
    !tokens.is_empty()
}
"#;

fn main() {
    println!("=== CodeNexus Example: Basic Indexing ===\n");

    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let source_dir = temp_dir.path().join("src");
    fs::create_dir_all(&source_dir).expect("failed to create source dir");
    fs::write(source_dir.join("lib.rs"), SAMPLE_CODE).expect("failed to write sample code");

    let db_path = temp_dir.path().join("codenexus.lbug");

    let facade = IndexFacade::new(&db_path).expect("IndexFacade::new");
    let result = facade.index(&source_dir, "demo-project", true).expect("index");

    println!("=== Indexing Complete ===");
    println!("  Files indexed:  {}", result.files_indexed);
    println!("  Nodes created:  {}", result.nodes_created);
    println!("  Edges created:  {}", result.edges_created);
    println!("  Project ID:     {}", result.project_id);
    println!();

    let query = QueryFacade::new(&db_path).expect("QueryFacade::new");

    println!("=== Functions (via Cypher) ===");
    let qr = query
        .cypher("MATCH (f:Function) RETURN f.name, f.qualifiedName LIMIT 10")
        .expect("cypher failed");
    for row in &qr.rows {
        let name = row[0].as_str().unwrap_or("?");
        let qn = row[1].as_str().unwrap_or("?");
        println!("  {name} @ {qn}");
    }
    println!("\nTotal: {} functions", qr.rows.len());
}
