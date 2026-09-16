#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Only test valid UTF-8 input; invalid UTF-8 is silently skipped.
    let input = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return,
    };

    let escaped = codenexus::storage::schema::escape_cypher_string(input);

    // Invariant 1: every '\' in the output must be the start of a recognized
    // escape sequence (\\, \', \n, \r, \t, \0). A bare backslash followed by
    // an unrecognized character indicates an incomplete escape.
    let bytes = escaped.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            assert!(
                i + 1 < bytes.len(),
                "trailing backslash in escaped output for input: {input:?}"
            );
            match bytes[i + 1] {
                b'\\' | b'\'' | b'n' | b'r' | b't' | b'0' => {
                    i += 2; // valid escape sequence
                }
                other => {
                    panic!(
                        "unrecognized escape sequence '\\{}' in output for input: {input:?}",
                        other as char
                    );
                }
            }
        } else {
            i += 1;
        }
    }

    // Invariant 2: no unescaped single quote may appear in the output.
    // An unescaped quote would allow Cypher injection via string literal breakout.
    let mut i = 0;
    while i < escaped.len() {
        if escaped.as_bytes()[i] == b'\\' {
            i += 2; // skip escape sequence
            continue;
        }
        assert_ne!(
            escaped.as_bytes()[i],
            b'\'',
            "unescaped single quote in output for input: {input:?}"
        );
        i += 1;
    }
});
