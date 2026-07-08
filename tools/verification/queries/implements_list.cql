// Query: implements_list
// Returns: (symbol_name, file_path) tuples for types that implement an interface/trait
// Tests: IMPLEMENTS edge resolution
// Comparison: order-insensitive set equality on (symbol_name, file_path)

// === CODENEXUS ===
// IMPLEMENTS edge sources are Struct/Enum (Rust), Class (Python/Java/TS).
// CodeRelation is a NODE table in CodeNexus, so we use WITH clause chaining
// instead of Cartesian product (LadybugDB does not support `MATCH (a), (b)`).
MATCH (r:CodeRelation)
WHERE r.type = 'IMPLEMENTS' AND r.project = '__PID__'
WITH r
MATCH (n:Struct)
WHERE n.id = r.source
RETURN n.name AS symbol_name, n.filePath AS file_path
UNION
MATCH (r:CodeRelation)
WHERE r.type = 'IMPLEMENTS' AND r.project = '__PID__'
WITH r
MATCH (n:Enum)
WHERE n.id = r.source
RETURN n.name AS symbol_name, n.filePath AS file_path
UNION
MATCH (r:CodeRelation)
WHERE r.type = 'IMPLEMENTS' AND r.project = '__PID__'
WITH r
MATCH (n:Class)
WHERE n.id = r.source
RETURN n.name AS symbol_name, n.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (impl)-[r:CodeRelation]->(iface)
WHERE r.type = 'IMPLEMENTS'
RETURN impl.name AS symbol_name, impl.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
