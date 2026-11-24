//! The one way accounts are found by typed text: the Users screen's search box
//! and the people picker both go through [`AccountMatcher`], so a name matches
//! the same way everywhere.
//!
//! Matching is done in Rust over the account list rather than in SQL: the
//! folding below (case and accents) is not something both backends do alike,
//! and an instance holds hundreds of accounts, not millions.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// `text` with its case and accents folded away, for comparing what a person
/// typed with what is stored: "Zoë" and "ZOE" fold to "zoe". Letters with no
/// base-plus-accent form (such as the sharp s, folded to "ss") are handled
/// one by one; everything else stays as typed.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.nfd().filter(|c| !is_combining_mark(*c)) {
        match c {
            'ß' | 'ẞ' => out.push_str("ss"),
            'ø' | 'Ø' => out.push('o'),
            'æ' | 'Æ' => out.push_str("ae"),
            'œ' | 'Œ' => out.push_str("oe"),
            'đ' | 'Đ' => out.push('d'),
            'ł' | 'Ł' => out.push('l'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// Whether `folded` (already folded) starts with `query`, or any word of it
/// does. A word starts after any character that is not a letter or digit, so
/// "ada.lovelace", "Ada Lovelace" and "ada-lovelace" all match "lov".
fn prefix_or_word_prefix(folded: &str, query: &str) -> bool {
    if folded.starts_with(query) {
        return true;
    }
    let mut previous_is_word = false;
    for (index, c) in folded.char_indices() {
        let is_word = c.is_alphanumeric();
        if is_word && !previous_is_word && folded[index..].starts_with(query) {
            return true;
        }
        previous_is_word = is_word;
    }
    false
}

/// A typed search, folded once and then tried against any number of accounts.
///
/// An account matches when the query is a prefix of its login name or of its
/// display name, or of any word in either (case and accents folded). When
/// built with `match_email`, an account also matches when the query is its
/// whole email address; an address is never matched by prefix, so the search
/// cannot be used to walk the list of addresses a letter at a time. An empty
/// query matches nothing: callers that want "no filter" skip the matcher.
#[derive(Clone, Debug)]
pub struct AccountMatcher {
    query: String,
    email: Option<String>,
}

impl AccountMatcher {
    /// A matcher for `query` (trimmed and folded). `match_email` switches on
    /// the exact full-address match; leave it off for any caller who may not
    /// learn which address belongs to whom.
    pub fn new(query: &str, match_email: bool) -> Self {
        let query = fold(query.trim());
        let email = (match_email && !query.is_empty()).then(|| query.clone());
        AccountMatcher { query, email }
    }

    /// Whether the query is empty, so nothing can match.
    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    /// Whether the account with this login, display name and email matches.
    pub fn matches(&self, login: &str, display: &str, email: Option<&str>) -> bool {
        if self.query.is_empty() {
            return false;
        }
        if prefix_or_word_prefix(&fold(login), &self.query)
            || prefix_or_word_prefix(&fold(display), &self.query)
        {
            return true;
        }
        match (&self.email, email) {
            (Some(wanted), Some(email)) => fold(email.trim()) == *wanted,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(query: &str, login: &str, display: &str, email: Option<&str>, emails: bool) -> bool {
        AccountMatcher::new(query, emails).matches(login, display, email)
    }

    #[test]
    fn case_and_accents_are_folded_on_both_sides() {
        assert!(hit("zoe", "zoe", "Zoë Müller", None, false));
        assert!(hit("ZOË", "zoe", "Zoe", None, false));
        assert!(hit("muller", "m", "Zoë Müller", None, false));
        assert!(hit("MÜLL", "m", "Zoe Muller", None, false));
        assert!(hit("strasse", "s", "Straße", None, false));
        assert_eq!(fold("Łódź"), "lodz");
    }

    #[test]
    fn a_prefix_of_the_login_or_display_name_matches() {
        assert!(hit("ad", "ada", "Someone", None, false));
        assert!(hit("some", "ada", "Someone", None, false));
        assert!(hit("ada lo", "x", "Ada Lovelace", None, false));
    }

    #[test]
    fn a_word_prefix_matches_but_a_middle_of_a_word_does_not() {
        assert!(hit("love", "ada", "Ada Lovelace", None, false));
        assert!(hit("lov", "ada.lovelace", "A", None, false));
        assert!(hit("lov", "ada-lovelace", "A", None, false));
        assert!(!hit("velace", "ada.lovelace", "Ada Lovelace", None, false));
        assert!(!hit("da", "ada", "Ada", None, false));
    }

    #[test]
    fn an_email_matches_only_whole_and_only_when_asked() {
        let email = Some("Ada@Example.com");
        assert!(hit("ada@example.com", "x", "X", email, true));
        assert!(hit("  ADA@example.COM ", "x", "X", email, true));
        assert!(!hit("ada@example.com", "x", "X", email, false));
        assert!(!hit("ada@example", "x", "X", email, true));
        assert!(!hit("example.com", "x", "X", email, true));
        assert!(!hit("ada@example.com", "x", "X", None, true));
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        let matcher = AccountMatcher::new("   ", true);
        assert!(matcher.is_empty());
        assert!(!matcher.matches("ada", "Ada", Some("")));
    }
}
