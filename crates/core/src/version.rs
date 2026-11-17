//! Comparing Crystalline versions.

/// Whether a `major.minor.patch` version is at least `min`. A pre-release or
/// build suffix after the patch is ignored, and a version that does not parse
/// is never enough: a guard is not handed to a server on a guess.
pub fn version_at_least(version: &str, min: &str) -> bool {
    fn parts(v: &str) -> Option<(u64, u64, u64)> {
        let core = v.trim().trim_start_matches('v');
        let core = core.split(['-', '+']).next()?;
        let mut it = core.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next()?.parse().ok()?;
        it.next().is_none().then_some((major, minor, patch))
    }
    match (parts(version), parts(min)) {
        (Some(have), Some(need)) => have >= need,
        _ => false,
    }
}

/// A version's place in semantic-versioning precedence: the numeric
/// `major.minor.patch` triple first, then the pre-release identifiers, with
/// build metadata ignored.
///
/// The pre-release part matters for any pre-release: two builds share a
/// triple (`0.21.0-dev.4817` and `0.21.0-dev.4820`), and comparing the triple
/// alone would read them as equal, so the newer one would attach to the older
/// daemon instead of taking over after an upgrade. A release outranks every
/// pre-release of its own triple, as semantic versioning orders them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Precedence {
    triple: (u64, u64, u64),
    /// Empty for a release. Compared identifier by identifier.
    pre: Vec<PreIdent>,
}

/// One dot-separated pre-release identifier. A numeric one sorts below an
/// alphanumeric one, numerics compare as numbers and the rest in ASCII order;
/// the derived order gets all three from the variant order and the payloads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PreIdent {
    Numeric(u64),
    Alpha(String),
}

impl Precedence {
    /// Parse a version string. Tolerates a two-part `major.minor`; `None`
    /// when the triple does not parse or a pre-release identifier is empty.
    pub fn parse(version: &str) -> Option<Self> {
        let version = version.trim();
        let without_build = version.split('+').next().unwrap_or(version);
        let (core, pre) = match without_build.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (without_build, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.trim().parse().ok()?;
        let minor = parts.next()?.trim().parse().ok()?;
        let patch = parts.next().unwrap_or("0").trim().parse().ok()?;
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|ident| {
                    if ident.is_empty() {
                        None
                    } else if ident.bytes().all(|b| b.is_ascii_digit()) {
                        ident.parse().ok().map(PreIdent::Numeric)
                    } else {
                        Some(PreIdent::Alpha(ident.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Self {
            triple: (major, minor, patch),
            pre,
        })
    }

    /// The numeric `major.minor.patch` triple.
    pub fn triple(&self) -> (u64, u64, u64) {
        self.triple
    }

    /// Whether this is a release: no pre-release identifiers.
    pub fn is_stable(&self) -> bool {
        self.pre.is_empty()
    }

    /// `major.minor.patch`, plus `-pre` when there are pre-release
    /// identifiers, built from the parsed parts only. Build metadata and
    /// any text that did not parse are never echoed.
    pub fn render(&self) -> String {
        let (major, minor, patch) = self.triple;
        let mut out = format!("{major}.{minor}.{patch}");
        for (i, ident) in self.pre.iter().enumerate() {
            out.push(if i == 0 { '-' } else { '.' });
            match ident {
                PreIdent::Numeric(n) => out.push_str(&n.to_string()),
                PreIdent::Alpha(s) => out.push_str(s),
            }
        }
        out
    }
}

impl Ord for Precedence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.triple.cmp(&other.triple).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Slice order goes identifier by identifier, and a shorter
                // list that is a prefix of the longer one sorts first, which
                // is the semantic-versioning rule for pre-release fields.
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Precedence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_built_from_parsed_parts() {
        let p = Precedence::parse(" v1.2.3+build ");
        assert!(p.is_none(), "a leading v does not parse");
        let p = Precedence::parse(" 1.2.3+build ").unwrap();
        assert_eq!(p.render(), "1.2.3");
        assert!(p.is_stable());
        let rc = Precedence::parse("1.2.3-rc.1").unwrap();
        assert_eq!(rc.render(), "1.2.3-rc.1");
        assert!(!rc.is_stable());
        assert_eq!(rc.triple(), (1, 2, 3));
    }

    #[test]
    fn render_never_echoes_extra_text() {
        // Alphanumeric identifiers are kept as written, so the text is only
        // ever what the parse accepted; callers that put it in front of a
        // person check the shape themselves (S30).
        for input in ["1.2.3-<script>", "1.2.3-rc.1 extra", "1.2.3.4", "1.2.3+<b>"] {
            if let Some(p) = Precedence::parse(input) {
                let r = p.render();
                assert!(!r.contains('+'), "{input} -> {r}");
                assert!(r.len() <= input.len(), "{input} -> {r}");
            }
        }
        assert_eq!(Precedence::parse("1.2.3+<b>").unwrap().render(), "1.2.3");
        assert_eq!(Precedence::parse("1.2.3.4").unwrap().render(), "1.2.3");
    }
}
