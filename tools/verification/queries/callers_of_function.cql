// Query: callers_of_function
// Returns: (symbol_name, file_path) tuples for functions that call another function
// Tests: CALLS edge resolution + Function node extraction
// Comparison: order-insensitive set equality on (symbol_name, file_path)

// === CODENEXUS ===
MATCH (r:CodeRelation)
WHERE r.type = 'CALLS'
WITH r
MATCH (caller:Function)
WHERE caller.id = r.source
RETURN caller.name AS symbol_name, caller.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (caller:Function)-[r:CodeRelation]->(callee:Function)
WHERE r.type = 'CALLS'
RETURN caller.name AS symbol_name, caller.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
