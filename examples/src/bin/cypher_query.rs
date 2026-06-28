use codenexus_examples::{index_sample_code, open_query, setup};

const SAMPLE_CODE: &str = r#"
pub fn parse(input: &str) -> Vec<String> {
    input.split_whitespace().map(String::from).collect()
}

pub fn validate(tokens: &[String]) -> bool {
    !tokens.is_empty()
}

pub fn transform(data: &[String]) -> String {
    data.join(" ")
}

struct Config {
    debug: bool,
}

impl Config {
    fn new() -> Self {
        Self { debug: false }
    }
}
"#;

fn main() {
    println!("=== CodeNexus Example: Cypher Query ===\n");

    let ctx = setup(SAMPLE_CODE);
    let _result = index_sample_code(&ctx, "cypher-demo");

    let query = open_query(&ctx);

    println!("--- Query 1: All Functions ---");
    let qr = query
        .cypher("MATCH (f:Function) RETURN f.name, f.qualifiedName")
        .expect("cypher failed");
    print_query_result(&qr);

    println!("--- Query 2: Name contains 'parse' ---");
    let qr = query
        .cypher("MATCH (n) WHERE n.name CONTAINS 'parse' RETURN n.name, n.qualifiedName")
        .expect("cypher failed");
    print_query_result(&qr);

    println!("--- Query 3: All Structs ---");
    let qr = query
        .cypher("MATCH (s:Struct) RETURN s.name")
        .expect("cypher failed");
    print_query_result(&qr);
}

fn print_query_result(result: &codenexus::query::QueryResult) {
    println!("  Columns: {:?}", result.columns);
    println!("  Rows ({}):", result.rows.len());
    for (i, row) in result.rows.iter().enumerate() {
        let formatted: Vec<String> = row.iter().map(|v| format!("{v}")).collect();
        println!("    [{}] {}", i + 1, formatted.join(" | "));
    }
    println!("  Duration: {}ms\n", result.duration_ms);
}
