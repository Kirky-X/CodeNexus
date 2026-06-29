// Query: extends_chain
// Returns: (symbol_name, file_path) tuples for types that extend another type
// Tests: EXTENDS edge resolution
// Note: Rust repos have no EXTENDS edges; expect empty set on both sides.

// === CODENEXUS ===
MATCH (child:Class), (r:CodeRelation)
WHERE r.source = child.id AND r.type = 'EXTENDS'
RETURN child.name AS symbol_name, child.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (child)-[r:CodeRelation]->(parent)
WHERE r.type = 'EXTENDS'
RETURN child.name AS symbol_name, child.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
