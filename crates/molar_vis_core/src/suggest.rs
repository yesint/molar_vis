//! Selection-input assistance for the rep selection field.
//!
//! molar now returns a **structured** parse error ([`molar::prelude::SyntaxError`])
//! carrying the failure offset, the byte `span` of the offending word, and a curated
//! `expected` token list. The UI highlights the word from `span` directly; this module
//! turns the structured error into a concise one-line message.
//!
//! Pure logic — no IO, WASM-safe.

use molar::prelude::SyntaxError;

/// At most this many expected tokens are listed before we fall back to pointing at the
/// offending word (a leading-token failure can "expect" dozens of keywords, which is
/// noise — the useful information is then *which* word is wrong).
const EXPECTED_CAP: usize = 6;

/// A concise one-line message from molar's structured selection parse error:
/// - no expected tokens → `unexpected end of input`;
/// - a short expected set → `expected and, or, end of input`;
/// - a long expected set → `unexpected "<word>"` (listing them all would be noise).
pub fn concise_message(info: &SyntaxError) -> String {
    if info.expected.is_empty() {
        return "unexpected end of input".to_string();
    }
    if info.expected.len() > EXPECTED_CAP {
        return match info.input.get(info.span.clone()) {
            Some(word) if !word.is_empty() => format!("unexpected \"{word}\""),
            _ => "unexpected input".to_string(),
        };
    }
    let pretty: Vec<&str> = info
        .expected
        .iter()
        .map(|t| if t == "EOF" { "end of input" } else { t.as_str() })
        .collect();
    format!("expected {}", pretty.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(input: &str, span: std::ops::Range<usize>, expected: &[&str]) -> SyntaxError {
        SyntaxError {
            input: input.to_string(),
            offset: span.start,
            span,
            expected: expected.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn short_expected_set_is_listed_with_eof_humanized() {
        let e = err("chain A an resid 5", 8..10, &["EOF", "and", "or"]);
        assert_eq!(concise_message(&e), "expected end of input, and, or");
    }

    #[test]
    fn long_expected_set_points_at_the_offending_word() {
        // A leading typo "expects" every top-level keyword — list the word instead.
        let many: Vec<&str> = "a b c d e f g h".split(' ').collect();
        let e = err("protien", 0..7, &many);
        assert_eq!(concise_message(&e), "unexpected \"protien\"");
    }

    #[test]
    fn empty_expected_set_is_end_of_input() {
        let e = err("resid 5 and", 11..11, &[]);
        assert_eq!(concise_message(&e), "unexpected end of input");
    }
}
