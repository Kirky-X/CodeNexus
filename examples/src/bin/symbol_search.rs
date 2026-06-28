use codenexus::model::NodeLabel;
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

fn internal_helper() {}

struct Config {
    debug: bool,
}
"#;

fn main() {
    println!("=== CodeNexus Example: Symbol Search ===\n");

    let ctx = setup(SAMPLE_CODE);
    let result = index_sample_code(&ctx, "search-demo");

    let query = open_query(&ctx);

    println!("--- Search by name: 'parse' ---");
    let results = query
        .search("parse", Some(&result.project_id), 10)
        .expect("search failed");
    print_search_results(&results);

    println!("--- Search by type: Function ---");
    let results = query
        .search_by_type(NodeLabel::Function, Some(&result.project_id), 10)
        .expect("search_by_type failed");
    print_search_results(&results);

    println!("--- Search by type: Struct ---");
    let results = query
        .search_by_type(NodeLabel::Struct, Some(&result.project_id), 10)
        .expect("search_by_type failed");
    print_search_results(&results);

    println!("--- Search by name: 'nonexistent' (empty result) ---");
    let results = query
        .search("nonexistent", Some(&result.project_id), 10)
        .expect("search failed");
    if results.is_empty() {
        println!("  No symbols found (expected)\n");
    } else {
        print_search_results(&results);
    }
}

fn print_search_results(results: &[codenexus::query::SearchResult]) {
    if results.is_empty() {
        println!("  No results\n");
        return;
    }
    for r in results {
        println!(
            "  {} [{}] @ {:?} score={:.2}",
            r.name,
            r.label,
            r.file_path.as_deref().unwrap_or("N/A"),
            r.score
        );
    }
    println!();
}
