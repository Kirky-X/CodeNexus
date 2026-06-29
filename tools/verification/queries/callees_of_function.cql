// Query: callees_of_function
// Returns: (symbol_name, file_path) tuples for functions that are called by another function
// Tests: CALLS edge resolution in the reverse direction
// Comparison: order-insensitive set equality on (symbol_name, file_path)

// === CODENEXUS ===
MATCH (callee:Function), (r:CodeRelation)
WHERE r.target = callee.id AND r.type = 'CALLS'
RETURN callee.name AS symbol_name, callee.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (caller:Function)-[r:CodeRelation]->(callee:Function)
WHERE r.type = 'CALLS'
RETURN callee.name AS symbol_name, callee.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
