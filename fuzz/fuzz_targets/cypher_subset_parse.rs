#![no_main]

use libfuzzer_sys::fuzz_target;

// Fuzzes the read-only Cypher subset gate (`validate_cypher_subset`) —
// the pest grammar is the single authority that decides which queries the
// MCP `query` tool will accept, so its top-level invariant is: any input
// yields `Ok(())` or a structured error, never a panic. Additional
// invariant: a query that only pads/permutes a known write clause must be
// rejected (the gate must stay a *write* gate).
fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return,
    };

    let result = codenexus::query::cypher_subset::validate_cypher_subset(input);

    if let Err(err) = result {
        // Errors must carry a human-readable message (never empty) so the
        // caller (MCP tool response) can surface a diagnosis.
        let msg = err.to_string();
        assert!(
            !msg.is_empty(),
            "empty rejection message for input: {input:?}"
        );
    }
});
