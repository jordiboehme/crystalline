//! Text similarity primitives: a normalizing fold and a bigram Dice
//! coefficient.
//!
//! Pure text math with no state, no I/O and no dependency beyond the standard
//! library, which is why it belongs in the format crate: the verify `Q004`
//! rule and the index crate's near-duplicate sweep need the exact same
//! numbers, and a second copy of the arithmetic would drift.
//!
//! Dice over character bigrams was chosen over a longest-common-subsequence
//! ratio for its O(n) simplicity and its stability under paragraph
//! reordering: moving a block of text changes only the handful of bigrams at
//! the seams.

use std::collections::HashMap;

/// Fold text into the comparable form every similarity consumer uses: ASCII
/// lowercase, every character that is neither alphanumeric nor whitespace
/// replaced by a space, then whitespace runs collapsed to single spaces with
/// both ends trimmed.
///
/// The fold is deliberately lossy. Punctuation, markdown syntax and line
/// breaks carry no meaning for a duplicate check, so removing them lets two
/// spellings of the same paragraph compare equal.
///
/// ```
/// use crystalline_core::similarity::normalize;
///
/// assert_eq!(normalize("  Hello, World!\nAgain.  "), "hello world again");
/// assert_eq!(normalize("---"), "");
/// ```
pub fn normalize(s: &str) -> String {
    let filtered: String = s
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    filtered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bigram Dice coefficient: `2 * |intersection| / (|bigrams(a)| +
/// |bigrams(b)|)`, using multiset (count-aware) bigram intersection.
///
/// The result is in `0.0..=1.0`: `1.0` for identical input, `0.0` when either
/// side has fewer than two characters or the two share no bigram. Callers
/// normally pass [`normalize`]d text so casing and punctuation do not shift
/// the score.
///
/// ```
/// use crystalline_core::similarity::dice_coefficient;
///
/// assert_eq!(dice_coefficient("night", "night"), 1.0);
/// assert_eq!(dice_coefficient("abcd", "wxyz"), 0.0);
/// assert!(dice_coefficient("night", "nacht") < 0.5);
/// ```
pub fn dice_coefficient(a: &str, b: &str) -> f64 {
    let ba = bigrams(a);
    let bb = bigrams(b);
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<(char, char), i32> = HashMap::new();
    for bg in &ba {
        *counts.entry(*bg).or_insert(0) += 1;
    }
    let mut intersection = 0usize;
    for bg in &bb {
        if let Some(c) = counts.get_mut(bg)
            && *c > 0
        {
            *c -= 1;
            intersection += 1;
        }
    }
    2.0 * intersection as f64 / (ba.len() + bb.len()) as f64
}

/// Every adjacent character pair in a string, in order. Empty when the string
/// holds fewer than two characters.
fn bigrams(s: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 2 {
        return Vec::new();
    }
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lowercases_and_collapses() {
        assert_eq!(normalize("Foo   BAR"), "foo bar");
        assert_eq!(normalize("a-b_c"), "a b c");
        assert_eq!(normalize("\n\tspaced\r\n"), "spaced");
    }

    #[test]
    fn normalize_keeps_alphanumerics_only() {
        assert_eq!(normalize("v0.11.2 (rc1)!"), "v0 11 2 rc1");
        assert_eq!(normalize("###"), "");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn dice_is_one_for_identical_text() {
        assert_eq!(dice_coefficient("the same words", "the same words"), 1.0);
    }

    #[test]
    fn dice_is_zero_for_disjoint_or_too_short_text() {
        assert_eq!(dice_coefficient("abcd", "wxyz"), 0.0);
        assert_eq!(dice_coefficient("a", "a"), 0.0);
        assert_eq!(dice_coefficient("", "anything"), 0.0);
    }

    #[test]
    fn dice_is_symmetric_and_bounded() {
        let a = "the quick brown fox jumps over the lazy dog";
        let b = "the quick brown fox leaps over the lazy dog";
        let ab = dice_coefficient(a, b);
        let ba = dice_coefficient(b, a);
        assert!((ab - ba).abs() < f64::EPSILON, "{ab} vs {ba}");
        assert!((0.0..=1.0).contains(&ab), "{ab} out of range");
        assert!(ab > 0.8, "one word swapped should stay very similar: {ab}");
    }

    #[test]
    fn dice_counts_bigrams_as_a_multiset() {
        // `aa` twice versus once: the multiset intersection is 1, not 2.
        let sim = dice_coefficient("aaa", "aa");
        assert!((sim - 2.0 * 1.0 / 3.0).abs() < 1e-12, "{sim}");
    }

    #[test]
    fn dice_is_stable_under_paragraph_reordering() {
        let one = normalize("First paragraph here. Second paragraph there.");
        let two = normalize("Second paragraph there. First paragraph here.");
        assert!(dice_coefficient(&one, &two) > 0.9);
    }
}
