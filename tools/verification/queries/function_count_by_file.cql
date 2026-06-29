// Query: function_count_by_file
// Returns: (symbol_name, file_path) tuples where symbol_name is the file path
//         and the count of functions in that file. We return one row per
//         (file, function_count) so set equality compares the distribution.
// Tests: Function node extraction per file
// Note: Counts are returned as strings for markdown-table uniformity.

// === CODENEXUS ===
MATCH (fn:Function)
RETURN fn.filePath AS symbol_name, fn.filePath AS file_path, count(fn) AS fn_count
ORDER BY fn_count DESC
LIMIT 200

// === GITNEXUS ===
MATCH (fn:Function)
RETURN fn.filePath AS symbol_name, fn.filePath AS file_path, count(fn) AS fn_count
ORDER BY fn_count DESC
LIMIT 200
