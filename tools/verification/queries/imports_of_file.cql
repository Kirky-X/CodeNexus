// Query: imports_of_file
// Returns: (symbol_name, file_path) tuples for files that import something
// Tests: IMPORTS edge resolution + File node extraction
// Comparison: order-insensitive set equality on (symbol_name, file_path)

// === CODENEXUS ===
MATCH (r:CodeRelation)
WHERE r.type = 'IMPORTS' AND r.project = '__PID__'
WITH r
MATCH (f:File)
WHERE f.id = r.source
RETURN f.filePath AS symbol_name, f.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (f:File)-[r:CodeRelation]->(target)
WHERE r.type = 'IMPORTS'
RETURN f.filePath AS symbol_name, f.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
