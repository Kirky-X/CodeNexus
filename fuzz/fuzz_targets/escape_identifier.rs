#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Only test valid UTF-8 input.
    let input = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return,
    };

    let result = codenexus::storage::schema::escape_identifier(input);

    // Invariant: if the input is a reserved keyword, the output must be
    // wrapped in backticks. Otherwise, the output must equal the input.
    if codenexus::storage::schema::is_reserved_keyword(input) {
        assert!(
            result.starts_with('`') && result.ends_with('`'),
            "reserved keyword {input:?} not wrapped in backticks: {result}"
        );
        // The inner content (between backticks) must be the original input.
        let inner = &result[1..result.len() - 1];
        assert_eq!(inner, input);
    } else {
        assert_eq!(
            result.as_ref(),
            input,
            "non-keyword {input:?} was modified: {result}"
        );
    }
});
