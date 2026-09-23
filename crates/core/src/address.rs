//! Permalinks, the `crystalline://` address scheme and link resolution.
//!
//! A permalink is a domain-relative slug path; the domain name lives in the
//! registry, never in the file, so re-homing a domain never rewrites permalinks.
//! Link resolution runs over a caller-supplied lookup table so this crate never
//! needs a database.

use std::collections::{HashMap, HashSet};

use crate::attachment::ASSETS_PREFIX;
use crate::engram::LinkTarget;

/// The address scheme prefix.
pub const SCHEME: &str = "crystalline://";

/// Slugify a domain-relative path into a permalink.
///
/// The input is lowercased, the `.md` extension is dropped, and every run of
/// characters outside `[a-z0-9/-]` becomes a single hyphen. Path separators are
/// preserved; each segment has leading and trailing hyphens trimmed and empty
/// segments are dropped.
pub fn slugify(path: &str) -> String {
    let path = path
        .strip_suffix(".md")
        .or_else(|| path.strip_suffix(".MD"))
        .unwrap_or(path);
    let lowered = path.to_lowercase();

    // Collapse disallowed runs to a single hyphen, keeping `/` and `-`.
    let mut collapsed = String::with_capacity(lowered.len());
    let mut pending_hyphen = false;
    for ch in lowered.chars() {
        let keep = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '/' || ch == '-';
        if keep {
            if pending_hyphen {
                collapsed.push('-');
                pending_hyphen = false;
            }
            collapsed.push(ch);
        } else {
            pending_hyphen = true;
        }
    }

    collapsed
        .split('/')
        .map(|seg| seg.trim_matches('-'))
        .filter(|seg| !seg.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// The permalink a file answers to when its frontmatter names none: the
/// domain-relative path, slugified. The one derivation every surface shares -
/// verify's link rules, the move's `permalink: "path"` and the evolve rule that
/// compares a permalink against its folder - so the three can never disagree
/// about what "in step with the path" means.
pub fn path_permalink(rel_path: &str) -> String {
    slugify(rel_path)
}

/// The folder part of a permalink or slugified path: everything before the
/// last `/`, empty at the root.
pub fn permalink_folder(permalink: &str) -> &str {
    permalink
        .rsplit_once('/')
        .map(|(folder, _)| folder)
        .unwrap_or("")
}

/// Check a permalink a caller asked for by name, answering why it cannot be
/// one.
///
/// A permalink is what [`slugify`] produces, so the test is that slugifying it
/// changes nothing: lowercase ASCII letters, digits, `-` and `/`, no empty or
/// hyphen-trimmed segment. Two misreadings get a message of their own because
/// they are the likely ones - an absolute `crystalline://` address and a
/// `domain:` prefix, both of which name the domain the permalink never
/// carries. The reserved `assets/` folder is refused too: an address under it
/// is an attachment's, never an engram's.
pub fn validate_permalink(permalink: &str) -> Result<(), String> {
    if permalink.trim().is_empty() {
        return Err("the permalink is empty".to_string());
    }
    if permalink.starts_with(SCHEME) {
        return Err(format!(
            "'{permalink}' is an absolute address; a permalink is domain-relative, so drop \
             the {SCHEME}<domain>/ part"
        ));
    }
    if permalink.contains(':') {
        return Err(format!(
            "'{permalink}' carries a domain prefix; a permalink is domain-relative and never \
             names its domain"
        ));
    }
    let slug = slugify(permalink);
    if slug != permalink {
        return Err(if slug.is_empty() {
            format!("'{permalink}' does not slugify to a permalink; use letters or digits")
        } else {
            format!(
                "'{permalink}' is not a permalink: use lowercase letters, digits, '-' and '/' \
                 only, for example '{slug}'"
            )
        });
    }
    if permalink.starts_with(ASSETS_PREFIX) {
        return Err(format!(
            "'{permalink}' cannot be a permalink: it sits under the reserved {ASSETS_PREFIX} folder, \
             which holds attachments and never an engram"
        ));
    }
    Ok(())
}

/// A parsed `crystalline://<domain>/<permalink>` address, including the `/*`
/// glob form used by context anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrystallineUrl {
    /// The domain segment.
    pub domain: String,
    /// The permalink path. For a glob this is the prefix before `*`.
    pub permalink: String,
    /// Whether the address is a `/*` prefix glob.
    pub glob: bool,
}

impl CrystallineUrl {
    /// Parse a `crystalline://` address. Returns `None` when the scheme is
    /// missing or the domain segment is empty.
    pub fn parse(input: &str) -> Option<CrystallineUrl> {
        let rest = input.strip_prefix(SCHEME)?;
        let (domain, tail) = match rest.split_once('/') {
            Some((d, t)) => (d, t),
            None => (rest, ""),
        };
        if domain.is_empty() {
            return None;
        }
        let (permalink, glob) = if tail == "*" {
            (String::new(), true)
        } else if let Some(prefix) = tail.strip_suffix("/*") {
            (format!("{prefix}/"), true)
        } else {
            (tail.to_string(), false)
        };
        Some(CrystallineUrl {
            domain: domain.to_string(),
            permalink,
            glob,
        })
    }

    /// Format back into a `crystalline://` address, round-tripping the glob
    /// form.
    pub fn to_url(&self) -> String {
        if self.glob {
            if self.permalink.is_empty() {
                format!("{SCHEME}{}/*", self.domain)
            } else {
                format!("{SCHEME}{}/{}*", self.domain, self.permalink)
            }
        } else if self.permalink.is_empty() {
            format!("{SCHEME}{}", self.domain)
        } else {
            format!("{SCHEME}{}/{}", self.domain, self.permalink)
        }
    }

    /// The attachment path this address names, when it names one: a non-glob
    /// address whose permalink sits under the reserved `assets/` prefix.
    ///
    /// Consumer sites branch on this instead of testing the prefix by hand, so
    /// the reserved prefix has one spelling.
    pub fn asset_path(&self) -> Option<&str> {
        if self.glob {
            return None;
        }
        self.permalink
            .starts_with(ASSETS_PREFIX)
            .then_some(self.permalink.as_str())
    }

    /// Whether a candidate `(domain, permalink)` is matched by this address.
    pub fn matches(&self, domain: &str, permalink: &str) -> bool {
        if self.domain != domain {
            return false;
        }
        if self.glob {
            permalink.starts_with(&self.permalink)
        } else {
            self.permalink == permalink
        }
    }
}

impl std::fmt::Display for CrystallineUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_url())
    }
}

/// A resolved reference to an Engram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRef {
    /// The domain the target lives in.
    pub domain: String,
    /// The target's permalink.
    pub permalink: String,
}

/// The outcome of resolving a link target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The target resolved to a specific Engram.
    Resolved(ResolvedRef),
    /// A bare target that did not match anything in the current domain.
    Unresolved,
    /// An explicit `[[domain:Target]]` that did not resolve in that domain.
    CrossDomainUnresolved {
        /// The named domain.
        domain: String,
    },
}

/// A lookup of titles and permalinks, provided by the caller. Core has no
/// database, so resolution runs against whatever table the caller supplies.
pub trait LinkResolver {
    /// Resolve a permalink within a domain to a reference.
    fn by_permalink(&self, domain: &str, permalink: &str) -> Option<ResolvedRef>;
    /// Resolve a title within a domain to a reference.
    fn by_title(&self, domain: &str, title: &str) -> Option<ResolvedRef>;
    /// Whether this name is one of the domains the caller knows about.
    ///
    /// The one question the parser cannot answer for itself. `[[Log: Weekly
    /// Garden Notes]]` and `[[gardening: Composting Basics]]` are the same
    /// shape, and only the registry says which of the two words before the
    /// colon is a domain. See [`resolve`] for what the answer decides.
    fn is_domain(&self, name: &str) -> bool;
}

/// Resolve a link target relative to the current domain.
///
/// Resolution order for a bare target: permalink match in the current domain,
/// then title match in the current domain. Bare titles never resolve
/// cross-domain. An explicit `[[domain:Target]]` resolves only within the named
/// domain (permalink first, then title).
///
/// **A prefix that names no domain is not a prefix.** The parse
/// ([`LinkTarget::parse`]) is domain-agnostic by design, so an engram titled
/// `Log: Weekly Garden Notes` produces a link that splits exactly like a
/// cross-domain one. When the named domain is not a domain this resolver knows
/// ([`LinkResolver::is_domain`]), the whole bracket text as written
/// ([`LinkTarget::raw`]) is tried as a permalink and then a title in the
/// current domain before the link is called broken. A prefix that does name a
/// domain keeps its meaning whatever the current domain happens to hold: the
/// registry decides, and a title that merely looks like a prefix never wins
/// against a real one.
pub fn resolve<R: LinkResolver + ?Sized>(
    target: &LinkTarget,
    current_domain: &str,
    lookup: &R,
) -> Resolution {
    match &target.domain {
        Some(domain) => lookup
            .by_permalink(domain, &target.target)
            .or_else(|| lookup.by_title(domain, &target.target))
            .or_else(|| {
                // Only for a prefix nobody registered: a known domain that
                // simply does not hold the target is a broken cross-domain
                // link, and reading it as a title would hide that.
                (!lookup.is_domain(domain))
                    .then(|| {
                        lookup
                            .by_permalink(current_domain, &target.raw)
                            .or_else(|| lookup.by_title(current_domain, &target.raw))
                    })
                    .flatten()
            })
            .map(Resolution::Resolved)
            .unwrap_or(Resolution::CrossDomainUnresolved {
                domain: domain.clone(),
            }),
        None => lookup
            .by_permalink(current_domain, &target.target)
            .or_else(|| lookup.by_title(current_domain, &target.target))
            .map(Resolution::Resolved)
            .unwrap_or(Resolution::Unresolved),
    }
}

/// A simple in-memory [`LinkResolver`] for tests and small callers.
#[derive(Debug, Default, Clone)]
pub struct LookupTable {
    // (domain, permalink) -> ref
    permalinks: HashMap<(String, String), ResolvedRef>,
    // (domain, lowercased title) -> ref
    titles: HashMap<(String, String), ResolvedRef>,
    // Every domain the caller registered, including one holding nothing.
    domains: HashSet<String>,
}

impl LookupTable {
    /// Create an empty table.
    pub fn new() -> LookupTable {
        LookupTable::default()
    }

    /// Declare a domain the table knows, whether or not anything in it was
    /// registered. [`LookupTable::insert`] declares one too; this is for a
    /// domain that holds nothing a link could land on and is still a domain.
    pub fn register_domain(&mut self, domain: &str) {
        self.domains.insert(domain.to_string());
    }

    /// Register an Engram's domain, permalink and title.
    pub fn insert(&mut self, domain: &str, permalink: &str, title: &str) {
        self.domains.insert(domain.to_string());
        let reference = ResolvedRef {
            domain: domain.to_string(),
            permalink: permalink.to_string(),
        };
        self.permalinks.insert(
            (domain.to_string(), permalink.to_string()),
            reference.clone(),
        );
        self.titles
            .insert((domain.to_string(), title.to_lowercase()), reference);
    }
}

impl LinkResolver for LookupTable {
    fn by_permalink(&self, domain: &str, permalink: &str) -> Option<ResolvedRef> {
        self.permalinks
            .get(&(domain.to_string(), permalink.to_string()))
            .cloned()
    }

    fn by_title(&self, domain: &str, title: &str) -> Option<ResolvedRef> {
        self.titles
            .get(&(domain.to_string(), title.to_lowercase()))
            .cloned()
    }

    fn is_domain(&self, name: &str) -> bool {
        self.domains.contains(name)
    }
}

#[cfg(test)]
mod permalink_tests {
    use super::*;

    #[test]
    fn a_slug_shaped_permalink_is_accepted() {
        assert_eq!(validate_permalink("projects/velog/alpha"), Ok(()));
        assert_eq!(validate_permalink("alpha-2"), Ok(()));
    }

    #[test]
    fn the_likely_misreadings_are_named() {
        let scheme = validate_permalink("crystalline://eng/alpha").unwrap_err();
        assert!(scheme.contains("domain-relative"), "{scheme}");
        let prefix = validate_permalink("eng:alpha").unwrap_err();
        assert!(prefix.contains("domain prefix"), "{prefix}");
        let shape = validate_permalink("Projects/Alpha Notes").unwrap_err();
        assert!(shape.contains("'projects/alpha-notes'"), "{shape}");
        assert!(validate_permalink("").is_err());
        assert!(validate_permalink("a//b").is_err());
        assert!(validate_permalink("/alpha").is_err());
        assert!(validate_permalink("assets/deck").is_err());
    }

    #[test]
    fn the_folder_of_a_permalink_is_everything_before_the_last_slash() {
        assert_eq!(permalink_folder("projects/velog/alpha"), "projects/velog");
        assert_eq!(permalink_folder("alpha"), "");
        assert_eq!(
            path_permalink("Projects/Velog/Alpha Notes.md"),
            "projects/velog/alpha-notes"
        );
    }
}
