// Query: implements_list
// Returns: (symbol_name, file_path) tuples for types that implement an interface/trait
// Tests: IMPLEMENTS edge resolution
// Comparison: order-insensitive set equality on (symbol_name, file_path)

// === CODENEXUS ===
MATCH (impl:Impl), (r:CodeRelation)
WHERE r.source = impl.id AND r.type = 'IMPLEMENTS'
RETURN impl.name AS symbol_name, impl.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (impl)-[r:CodeRelation]->(iface)
WHERE r.type = 'IMPLEMENTS'
RETURN impl.name AS symbol_name, impl.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
