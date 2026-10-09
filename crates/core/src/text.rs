//! Small helpers for the texts a person reads.

/// `n` and the noun that fits it: `plural(1, "file", "files")` is "1 file",
/// `plural(3, "file", "files")` is "3 files", and zero takes the plural,
/// "0 files". A verb that agrees with the noun goes into both forms:
/// `plural(n, "file differs", "files differ")`.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_takes_the_singular_and_every_other_count_the_plural() {
        assert_eq!(plural(1, "file", "files"), "1 file");
        assert_eq!(plural(3, "file", "files"), "3 files");
        assert_eq!(plural(0, "file", "files"), "0 files");
        assert_eq!(plural(1, "file differs", "files differ"), "1 file differs");
    }
}
