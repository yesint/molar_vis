//! Selection-input assistance for the rep selection field:
//!
//! - [`SelHints`] — the distinct values a molecule offers for the value-taking
//!   selection keywords (`chain`/`resname`/`name`) and the numeric ranges
//!   (`resid`/`resindex`/`index`), computed once from the static topology and
//!   cached. [`SelHints::hint_for`] turns "the keyword the user is currently
//!   typing" into a one-line hint shown under the field (e.g. `chains: A B C R`,
//!   `resid: 2..120`).
//! - [`parse_sel_error`] — molar formats a selection **parse** error as
//!   `"syntax error: \n<text>\n----^\nExpected <…>"`; this extracts a concise
//!   message and the **caret offset** (which character the `^` points at) so the
//!   UI can highlight exactly where the error is, inside the field.
//!
//! Pure logic over a molar `System` + strings — no IO, WASM-safe.

use std::collections::BTreeSet;

use molar::prelude::*;

/// Value-taking selection keywords we offer hints for. Plain value keywords
/// (`chain`/`resname`/`name`) list their distinct values; numeric ones
/// (`resid`/`resindex`/`index`) show a range.
const KEYWORDS: &[&str] = &["chain", "resid", "resindex", "index", "resname", "name"];

/// At most this many distinct string values are listed before eliding the rest.
const LIST_CAP: usize = 24;

/// Distinct per-molecule selection values, cached for the suggestion hints.
/// Derived from the topology (which never changes after load), so it is computed
/// once per molecule and reused.
#[derive(Default)]
pub struct SelHints {
    /// Distinct chain ids, sorted.
    pub chains: Vec<char>,
    /// Distinct residue names, sorted.
    pub resnames: Vec<String>,
    /// Distinct atom names, sorted.
    pub names: Vec<String>,
    /// `(min, max)` residue id (can be negative); `None` if no atoms.
    pub resid: Option<(i32, i32)>,
    /// `(min, max)` residue index; `None` if no atoms.
    pub resindex: Option<(usize, usize)>,
    /// Total atom count (for the `index` range `0..n-1`).
    pub n_atoms: usize,
}

impl SelHints {
    /// Scan the system's atoms once to collect the distinct values / ranges.
    pub fn compute(system: &System) -> Self {
        let bound = system.select_all_bound();
        let mut chains = BTreeSet::new();
        let mut resnames = BTreeSet::new();
        let mut names = BTreeSet::new();
        let (mut rid_lo, mut rid_hi) = (i32::MAX, i32::MIN);
        let (mut rix_lo, mut rix_hi) = (usize::MAX, usize::MIN);
        let mut n = 0usize;
        for a in bound.iter_atoms() {
            chains.insert(a.chain);
            resnames.insert(a.resname.as_str().to_string());
            names.insert(a.name.as_str().to_string());
            rid_lo = rid_lo.min(a.resid);
            rid_hi = rid_hi.max(a.resid);
            rix_lo = rix_lo.min(a.resindex);
            rix_hi = rix_hi.max(a.resindex);
            n += 1;
        }
        SelHints {
            chains: chains.into_iter().collect(),
            resnames: resnames.into_iter().collect(),
            names: names.into_iter().collect(),
            resid: (n > 0).then_some((rid_lo, rid_hi)),
            resindex: (n > 0).then_some((rix_lo, rix_hi)),
            n_atoms: n,
        }
    }

    /// A one-line hint for the **last grammar keyword** appearing in `text`
    /// (where the user is presumably typing a value), or `None` if the trailing
    /// context isn't a value-taking keyword.
    pub fn hint_for(&self, text: &str) -> Option<String> {
        match last_keyword(text)? {
            "chain" if !self.chains.is_empty() => {
                let list = self
                    .chains
                    .iter()
                    .map(|&c| if c.is_whitespace() { '·' } else { c }.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                Some(format!("chains: {list}"))
            }
            "resid" => self.resid.map(|(lo, hi)| format!("resid: {lo}..{hi}")),
            "resindex" => self.resindex.map(|(lo, hi)| format!("resindex: {lo}..{hi}")),
            "index" if self.n_atoms > 0 => Some(format!("index: 0..{}", self.n_atoms - 1)),
            "resname" => fmt_list("resnames", &self.resnames),
            "name" => fmt_list("names", &self.names),
            _ => None,
        }
    }
}

/// `"label: a b c … (+N)"`, eliding past [`LIST_CAP`]; `None` if empty.
fn fmt_list(label: &str, items: &[String]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let shown = items
        .iter()
        .take(LIST_CAP)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = format!("{label}: {shown}");
    if items.len() > LIST_CAP {
        out.push_str(&format!(" … (+{})", items.len() - LIST_CAP));
    }
    Some(out)
}

/// The last [`KEYWORDS`] entry appearing as a whole word in `text` (case-
/// insensitive). Whole-word tokenization avoids `resname`/`name` and
/// `resid`/`resindex` confusing each other.
fn last_keyword(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    let mut last = None;
    for tok in lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if let Some(k) = KEYWORDS.iter().copied().find(|&k| k == tok) {
            last = Some(k);
        }
    }
    last
}

/// Parse molar's selection-error string into a concise message and the 0-based
/// **caret offset** (character index into the *trimmed* selection) the error
/// points at. molar's parse errors look like:
///
/// ```text
/// syntax error:
/// chain A an resid 5
/// ----------^
/// Expected one of ...
/// ```
///
/// Returns `(message, Some(caret))` for that shape, else `(message, None)` for
/// non-positional errors (evaluation failures, etc.).
pub fn parse_sel_error(raw: &str) -> (String, Option<usize>) {
    let lines: Vec<&str> = raw.split('\n').collect();
    if lines.len() >= 4 {
        if let Some(pos) = lines[2].find('^') {
            // The prefix before '^' must be all dashes for this to be molar's
            // caret line (not some '^' inside a message).
            if lines[2][..pos].bytes().all(|b| b == b'-') {
                let expected = lines[3].trim();
                let msg = if expected.is_empty() {
                    "syntax error".to_string()
                } else {
                    expected.to_string()
                };
                return (msg, Some(pos));
            }
        }
    }
    let cleaned = raw.trim().trim_start_matches("syntax error:").trim();
    let msg = if cleaned.is_empty() { raw.trim() } else { cleaned };
    (msg.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_keyword_picks_trailing_value_keyword() {
        assert_eq!(last_keyword("chain"), Some("chain"));
        assert_eq!(last_keyword("chain A"), Some("chain"));
        assert_eq!(last_keyword("protein and resid 5"), Some("resid"));
        assert_eq!(last_keyword("resindex 0:10"), Some("resindex"));
        assert_eq!(last_keyword("name CA and resname ALA"), Some("resname"));
        assert_eq!(last_keyword("protein"), None);
        assert_eq!(last_keyword(""), None);
    }

    #[test]
    fn parse_error_extracts_caret() {
        // Mirrors molar's SelectionExpr::new formatting.
        let raw = "syntax error: \nchain A an resid\n----------^\nExpected operator";
        let (msg, caret) = parse_sel_error(raw);
        assert_eq!(caret, Some(10));
        assert_eq!(msg, "Expected operator");
    }

    #[test]
    fn parse_error_without_caret_is_passed_through() {
        let (msg, caret) = parse_sel_error("division by zero");
        assert_eq!(caret, None);
        assert_eq!(msg, "division by zero");
    }
}
