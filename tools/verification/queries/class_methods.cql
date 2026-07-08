// Query: class_methods
// Returns: (symbol_name, file_path) tuples for methods owned by a class/struct/trait
// Tests: class method extraction (HAS_METHOD edge in gitnexus; parentQn in CodeNexus)
// Note: For Rust repos, classes are absent — Struct/Trait/Impl are the owners.
//       CodeNexus does not emit HAS_METHOD edges; class methods are identified
//       by the Method node's parentQn field (set to the owning class/struct/
//       trait/impl qualified name by the extractors). gitnexus uses HAS_METHOD.

// === CODENEXUS ===
// B8 fix: use parentQn instead of HAS_METHOD edge (CodeNexus doesn't emit it)
MATCH (m:Method)
WHERE m.parentQn <> '' AND m.project = '__PID__'
RETURN m.name AS symbol_name, m.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200

// === GITNEXUS ===
MATCH (owner)-[r:CodeRelation]->(m:Method)
WHERE r.type = 'HAS_METHOD'
RETURN m.name AS symbol_name, m.filePath AS file_path
ORDER BY symbol_name, file_path
LIMIT 200
