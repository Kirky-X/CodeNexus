// Query: imports_of_file
// Returns: (symbol_name, file_path) tuples for files that import something
// Tests: IMPORTS edge resolution + File node extraction
// Comparison: order-insensitive set equality on (symbol_name, file_path)

// === CODENEXUS ===
MATCH (f:File), (r:CodeRelation)
WHERE r.source = f.id AND r.type = 'IMPORTS'
RETURN f.filePath AS symbol_name, f.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (f:File)-[r:CodeRelation]->(target)
WHERE r.type = 'IMPORTS'
RETURN f.filePath AS symbol_name, f.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
