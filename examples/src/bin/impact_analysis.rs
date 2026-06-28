use codenexus::model::{Edge, EdgeType, Graph, Node, NodeLabel};
use codenexus::trace::{ImpactAnalyzer, TraceFacade, TraceType};
use codenexus_examples::{index_sample_code, open_query, setup};

const SAMPLE_CODE: &str = r#"
pub fn main() {
    let data = parse("hello world");
    validate(&data);
    transform(&data);
}

pub fn parse(input: &str) -> Vec<String> {
    input.split_whitespace().map(String::from).collect()
}

pub fn validate(tokens: &[String]) -> bool {
    tokens.len() < 100
}

pub fn transform(data: &[String]) -> String {
    let formatted = format_tokens(data);
    post_process(&formatted)
}

fn format_tokens(data: &[String]) -> String {
    data.join(" ")
}

fn post_process(s: &str) -> String {
    s.to_uppercase()
}
"#;

fn build_graph_from_db(query: &codenexus::query::QueryFacade) -> Graph {
    let mut graph = Graph::new();

    let node_tables = [
        ("Function", NodeLabel::Function),
        ("Struct", NodeLabel::Struct),
        ("Impl", NodeLabel::Impl),
        ("File", NodeLabel::File),
        ("Project", NodeLabel::Project),
        ("Variable", NodeLabel::Variable),
        ("Parameter", NodeLabel::Parameter),
    ];

    for (table, label) in &node_tables {
        let cypher = format!("MATCH (n:{table}) RETURN n.id, n.name");
        if let Ok(qr) = query.cypher(&cypher) {
            for row in &qr.rows {
                let id = row[0].as_str().unwrap_or("").to_string();
                let name = row[1].as_str().unwrap_or("").to_string();

                if id.is_empty() || name.is_empty() {
                    continue;
                }

                let builder = Node::builder(*label, &name, &name).id(id);
                graph.add_node(builder.build());
            }
        }
    }

    let cypher = "MATCH (e:CodeRelation) RETURN e.source, e.target, e.type, e.confidence, e.project";
    if let Ok(qr) = query.cypher(cypher) {
        for row in &qr.rows {
            let source = row[0].as_str().unwrap_or("").to_string();
            let target = row[1].as_str().unwrap_or("").to_string();
            let edge_type_str = row[2].as_str().unwrap_or("CALLS");
            let confidence = row[3].as_f64().unwrap_or(0.5) as f32;
            let project = row[4].as_str().unwrap_or("").to_string();

            let edge_type = match edge_type_str {
                "CALLS" => EdgeType::Calls,
                "READS" => EdgeType::Reads,
                "WRITES" => EdgeType::Writes,
                "IMPORTS" => EdgeType::Imports,
                "CONTAINS" => EdgeType::Contains,
                "DEFINES" => EdgeType::Defines,
                _ => EdgeType::Calls,
            };

            graph.add_edge(
                Edge::builder(source, target, edge_type, &project)
                    .confidence(confidence)
                    .build(),
            );
        }
    }

    graph
}

fn main() {
    println!("=== CodeNexus Example: Impact Analysis ===\n");

    let ctx = setup(SAMPLE_CODE);
    let _result = index_sample_code(&ctx, "impact-demo");

    let query = open_query(&ctx);
    let graph = build_graph_from_db(&query);

    println!("Graph: {} nodes, {} edges\n", graph.node_count(), graph.edge_count());

    println!("--- Impact Analysis: parse (depth=5) ---");
    if let Some(parse_node) = graph.nodes.values().find(|n| n.name == "parse") {
        let analyzer = ImpactAnalyzer::new(&graph);
        let impacts = analyzer.analyze(&parse_node.id, 5);
        if impacts.is_empty() {
            println!("  No impact found");
        } else {
            println!("  Affected symbols ({}):", impacts.len());
            for node in &impacts {
                println!("    {} [{}]", node.name, node.label);
            }
        }
    }
    println!();

    println!("--- Impact Analysis: validate (depth=3) ---");
    if let Some(validate_node) = graph.nodes.values().find(|n| n.name == "validate") {
        let analyzer = ImpactAnalyzer::new(&graph);
        let impacts = analyzer.analyze(&validate_node.id, 3);
        if impacts.is_empty() {
            println!("  No impact found");
        } else {
            println!("  Affected symbols ({}):", impacts.len());
            for node in &impacts {
                println!("    {} [{}]", node.name, node.label);
            }
        }
    }
    println!();

    println!("--- Trace: parse forward (depth=3) ---");
    let facade = TraceFacade::new(&graph);
    match facade.trace("parse", TraceType::Calls, 3) {
        Ok(result) => {
            println!("  Paths: {}", result.paths.len());
            for (i, path) in result.paths.iter().enumerate() {
                let names: Vec<&str> = path.nodes.iter().map(|n| n.name.as_str()).collect();
                println!("    Path {}: {} (depth={})", i + 1, names.join(" -> "), path.depth);
            }
        }
        Err(e) => println!("  Error: {e}"),
    }
}
