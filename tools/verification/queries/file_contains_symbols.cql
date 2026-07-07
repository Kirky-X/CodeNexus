// Query: file_contains_symbols
// Returns: (symbol_name, file_path) tuples for symbols defined within files
// Tests: DEFINES edge resolution + cross-type symbol extraction
// Note: We pick Function symbols (most numerous, easiest to compare).

// === CODENEXUS ===
MATCH (r:CodeRelation)
WHERE r.type = 'DEFINES'
WITH r
MATCH (fn:Function)
WHERE fn.id = r.target
RETURN fn.name AS symbol_name, fn.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (f:File)-[r:CodeRelation]->(fn:Function)
WHERE r.type = 'DEFINES'
RETURN fn.name AS symbol_name, fn.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
