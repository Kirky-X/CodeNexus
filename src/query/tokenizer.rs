// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `codenexus_tokenizer` — identifier-aware tokenization for BM25 search (H11).
//!
//! Source-code identifiers use `camelCase`, `PascalCase`, `snake_case`,
//! `SCREAMING_SNAKE_CASE`, and mixed conventions. A naive whitespace tokenizer
//! treats `parseFile` as a single token, making it unsearchable by `parse`.
//!
//! [`codenexus_tokenize`] splits identifiers into lowercase sub-token tokens
//! using boundary rules derived from the naming conventions above:
//!
//! | Input           | Tokens                     |
//! |-----------------|----------------------------|
//! | `parseFile`     | `parse`, `file`            |
//! | `parse_file`    | `parse`, `file`            |
//! | `ParseFile`     | `parse`, `file`            |
//! | `HTTPClient`    | `http`, `client`           |
//! | `parseFile_v2`  | `parse`, `file`, `v`, `2`  |
//! | `SCREAMING_SNAKE` | `screaming`, `snake`    |
//!
//! # Boundary rules
//!
//! 1. **Non-alphanumeric** characters (underscore, hyphen, dot, etc.) are
//!    separators — they flush the current token.
//! 2. **lowercase → Uppercase** (`parseFile`): split before the uppercase.
//! 3. **Uppercase-run → Uppercase+lowercase** (`HTTPClient`): split before the
//!    last uppercase so `HTTP` and `Client` are separate tokens.
//! 4. **letter → digit** (`file2`): split before the digit.
//! 5. **digit → letter** (`2nd`): split before the letter.
//!
//! All output tokens are lowercased for case-insensitive matching.

/// Splits an identifier into lowercase sub-token tokens.
///
/// See the [module docs](self) for the full boundary rules and examples.
#[must_use]
pub fn codenexus_tokenize(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in name.chars() {
        if ch.is_uppercase() {
            // camelCase boundary: lowercase→Uppercase (parseFile → parse | File)
            if current.ends_with(|c: char| c.is_lowercase()) {
                flush(&mut current, &mut tokens);
            }
            current.push(ch);
        } else if ch.is_lowercase() {
            // Uppercase-run → Uppercase+lowercase (HTTPClient → HTTP | Client):
            // when a lowercase follows an uppercase run of length > 1, split
            // off the last uppercase to start the new CamelCase word.
            if current.len() > 1 && current.chars().all(|c| c.is_uppercase()) {
                let last = current.pop().expect("current has len > 1");
                flush(&mut current, &mut tokens);
                current.push(last);
            } else if current.ends_with(|c: char| c.is_ascii_digit()) {
                // digit→lowercase boundary (2nd → 2 | nd)
                flush(&mut current, &mut tokens);
            }
            current.push(ch);
        } else if ch.is_ascii_digit() {
            // letter→digit boundary (file2 → file | 2)
            if current.ends_with(|c: char| c.is_alphabetic()) {
                flush(&mut current, &mut tokens);
            }
            current.push(ch);
        } else {
            // Separator (underscore, hyphen, dot, space, etc.)
            flush(&mut current, &mut tokens);
        }
    }
    flush(&mut current, &mut tokens);
    tokens
}

/// Pushes `current` (lowercased) into `tokens` and clears `current`.
fn flush(current: &mut String, tokens: &mut Vec<String>) {
    if !current.is_empty() {
        tokens.push(current.to_ascii_lowercase());
        current.clear();
    }
}

/// Joins the tokens of `name` with single spaces, producing a string suitable
/// for FTS indexing or `CONTAINS` matching.
///
/// Example: `parseFile` → `"parse file"`.
#[must_use]
pub fn codenexus_tokenize_join(name: &str) -> String {
    codenexus_tokenize(name).join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_camel_case() {
        assert_eq!(codenexus_tokenize("parseFile"), vec!["parse", "file"]);
    }

    #[test]
    fn tokenize_pascal_case() {
        assert_eq!(codenexus_tokenize("ParseFile"), vec!["parse", "file"]);
    }

    #[test]
    fn tokenize_snake_case() {
        assert_eq!(codenexus_tokenize("parse_file"), vec!["parse", "file"]);
    }

    #[test]
    fn tokenize_screaming_snake() {
        assert_eq!(
            codenexus_tokenize("SCREAMING_SNAKE"),
            vec!["screaming", "snake"]
        );
    }

    #[test]
    fn tokenize_acronym_prefix() {
        // HTTPClient → HTTP | Client
        assert_eq!(
            codenexus_tokenize("HTTPClient"),
            vec!["http", "client"]
        );
    }

    #[test]
    fn tokenize_single_word() {
        assert_eq!(codenexus_tokenize("parse"), vec!["parse"]);
    }

    #[test]
    fn tokenize_single_uppercase_word() {
        assert_eq!(codenexus_tokenize("PARSE"), vec!["parse"]);
    }

    #[test]
    fn tokenize_empty_string() {
        let result = codenexus_tokenize("");
        assert!(result.is_empty());
    }

    #[test]
    fn tokenize_with_digits_letter_to_digit() {
        // file2 → file | 2
        assert_eq!(codenexus_tokenize("file2"), vec!["file", "2"]);
    }

    #[test]
    fn tokenize_with_digits_digit_to_letter() {
        // 2nd → 2 | nd
        assert_eq!(codenexus_tokenize("2nd"), vec!["2", "nd"]);
    }

    #[test]
    fn tokenize_mixed_case_with_digits_and_underscore() {
        // parseFile_v2 → parse | file | v | 2
        assert_eq!(
            codenexus_tokenize("parseFile_v2"),
            vec!["parse", "file", "v", "2"]
        );
    }

    #[test]
    fn tokenize_multiple_separators() {
        assert_eq!(
            codenexus_tokenize("parse__file"),
            vec!["parse", "file"]
        );
    }

    #[test]
    fn tokenize_hyphen_separator() {
        assert_eq!(
            codenexus_tokenize("parse-file"),
            vec!["parse", "file"]
        );
    }

    #[test]
    fn tokenize_dot_separator() {
        assert_eq!(
            codenexus_tokenize("parse.file"),
            vec!["parse", "file"]
        );
    }

    #[test]
    fn tokenize_join_produces_space_separated() {
        assert_eq!(codenexus_tokenize_join("parseFile"), "parse file");
    }

    #[test]
    fn tokenize_join_empty_string() {
        assert_eq!(codenexus_tokenize_join(""), "");
    }

    #[test]
    fn tokenize_all_uppercase_acronym() {
        // Single acronym: HTTP → http
        assert_eq!(codenexus_tokenize("HTTP"), vec!["http"]);
    }

    #[test]
    fn tokenize_mixed_acronyms() {
        // XMLParser → XML | Parser
        assert_eq!(
            codenexus_tokenize("XMLParser"),
            vec!["xml", "parser"]
        );
    }

    #[test]
    fn tokenize_preserves_no_case_in_output() {
        let tokens = codenexus_tokenize("ParseFile");
        assert!(tokens.iter().all(|t| t == &t.to_ascii_lowercase()));
    }
}
