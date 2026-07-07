// Query: implements_list
// Returns: (symbol_name, file_path) tuples for types that implement an interface/trait
// Tests: IMPLEMENTS edge resolution
// Comparison: order-insensitive set equality on (symbol_name, file_path)

// === CODENEXUS ===
// IMPLEMENTS edge sources are Struct/Enum (Rust), Class (Python/Java/TS).
// The old query matched only Impl nodes (inherent impls), missing all
// trait impls whose source is the type definition node.
MATCH (r:CodeRelation), (n:Struct)
WHERE r.source = n.id AND r.type = 'IMPLEMENTS'
RETURN n.name AS symbol_name, n.filePath AS file_path
UNION
MATCH (r:CodeRelation), (n:Enum)
WHERE r.source = n.id AND r.type = 'IMPLEMENTS'
RETURN n.name AS symbol_name, n.filePath AS file_path
UNION
MATCH (r:CodeRelation), (n:Class)
WHERE r.source = n.id AND r.type = 'IMPLEMENTS'
RETURN n.name AS symbol_name, n.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (impl)-[r:CodeRelation]->(iface)
WHERE r.type = 'IMPLEMENTS'
RETURN impl.name AS symbol_name, impl.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
