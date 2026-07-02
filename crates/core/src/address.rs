//! Permalinks, the `crystalline://` address scheme and link resolution.
//!
//! A permalink is a domain-relative slug path; the domain name lives in the
//! registry, never in the file, so re-homing a domain never rewrites permalinks.
//! Link resolution runs over a caller-supplied lookup table so this crate never
//! needs a database.

use std::collections::HashMap;

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
}

/// Resolve a link target relative to the current domain.
///
/// Resolution order for a bare target: permalink match in the current domain,
/// then title match in the current domain. Bare titles never resolve
/// cross-domain. An explicit `[[domain:Target]]` resolves only within the named
/// domain (permalink first, then title).
pub fn resolve<R: LinkResolver + ?Sized>(
    target: &LinkTarget,
    current_domain: &str,
    lookup: &R,
) -> Resolution {
    match &target.domain {
        Some(domain) => lookup
            .by_permalink(domain, &target.target)
            .or_else(|| lookup.by_title(domain, &target.target))
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
}

impl LookupTable {
    /// Create an empty table.
    pub fn new() -> LookupTable {
        LookupTable::default()
    }

    /// Register an Engram's domain, permalink and title.
    pub fn insert(&mut self, domain: &str, permalink: &str, title: &str) {
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
}
