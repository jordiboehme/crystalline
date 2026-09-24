//! One decision for every surface that registers a domain: the CLI's
//! `domain add`, the engine's local, virtual and team paths (and through
//! them MCP's `add_domain` and the JSON API) and `domain init --name`.
//!
//! Two rules, in a load-bearing order. [`decide_registration`] runs first:
//! a name that is already registered to the same folder and kind is adopted
//! as it is, and one registered differently is a conflict naming what holds
//! it. Only a genuinely new name reaches [`validate_domain_name`]. So a name
//! registered before these rules existed (by hand, by an older CLI, by an
//! environment variable) can still be re-added idempotently and keeps
//! serving, while no surface can register a new name that breaks a
//! `crystalline://` address or a Windows checkout.

use std::path::{Path, PathBuf};

use super::DomainEntry;

/// The longest domain name, in characters (not bytes). The cap is about a
/// readable folder segment and a readable piece of a `crystalline://`
/// address; the operating system's own segment limit is larger and separate.
pub const MAX_DOMAIN_NAME_CHARS: usize = 64;

/// Whether `name` may name a NEW domain: every character a Unicode
/// alphanumeric or one of `-`, `_`, `.`; at most [`MAX_DOMAIN_NAME_CHARS`]
/// characters; no leading or trailing dot; and not a Windows reserved device
/// name. An allowlist, so every path separator, all whitespace, the
/// Windows-illegal punctuation and the invisible format characters fall out
/// without a line of their own. A decomposed (NFD) accent is refused, since a
/// combining mark is not alphanumeric; normalizing first would need a
/// dependency. The name is checked exactly as given: a caller that accepts
/// padded input (the JSON API) trims it before handing it on.
///
/// The error is the one sentence every surface shows.
pub fn validate_domain_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("the domain name is empty".to_string());
    }
    let stem = name.split('.').next().unwrap_or(name);
    let allowed = name.chars().count() <= MAX_DOMAIN_NAME_CHARS
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !is_windows_device_name(stem);
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "'{name}' cannot name a domain: use letters, digits, hyphens, \
             underscores and dots (no leading or trailing dot), 64 \
             characters or fewer, and not a Windows device name (CON, PRN, \
             AUX, NUL, COM1-COM9, LPT1-LPT9)"
        ))
    }
}

/// Whether `stem` (the segment before the first dot, so `CON.txt` is checked
/// as `CON` while `a.CON` is checked as `a`) names a Windows reserved device:
/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`, matched
/// ASCII-case-insensitively, plus the superscript-digit spellings
/// `COM\u{b9}`/`COM\u{b2}`/`COM\u{b3}` and their LPT twins, which Windows
/// resolves to the same device and which pass a bare alphanumeric allowlist.
/// `COM10` and up are not reserved. Only an EXACT stem matches, never a prefix.
pub fn is_windows_device_name(stem: &str) -> bool {
    let mut chars = stem.chars();
    let head: String = chars
        .by_ref()
        .take(3)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let rest: Vec<char> = chars.collect();
    match (head.as_str(), rest.as_slice()) {
        ("CON" | "PRN" | "AUX" | "NUL", []) => true,
        ("COM" | "LPT", [c]) => {
            c.is_ascii_digit() && *c != '0' || matches!(c, '\u{b9}' | '\u{b2}' | '\u{b3}')
        }
        _ => false,
    }
}

/// A file domain's root with `~` expanded and symlinks resolved, falling back
/// to the expanded path when it cannot be resolved (a folder that is gone).
/// `None` for a virtual domain. The one comparison every surface uses to say
/// "the same folder".
pub fn canonical_root(entry: &DomainEntry) -> Option<PathBuf> {
    let path = entry.file_path()?;
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

/// What a caller is asking to register under a name.
#[derive(Debug, Clone, Copy)]
pub enum RegistrationRequest<'a> {
    /// A file domain rooted at `root`, already canonicalized by the caller.
    File {
        /// The canonical root the caller resolved.
        root: &'a Path,
    },
    /// A database-backed virtual domain.
    Virtual,
}

/// The answer to a registration request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// The name is already registered to the same folder and kind: leave the
    /// entry untouched (its origin, provision and review included).
    Adopt,
    /// The name is free: register it, after [`validate_domain_name`].
    Register,
    /// The name is registered to something else; the message names it.
    Conflict(String),
}

/// Decide what registering `name` as `request` means, given the entry the
/// registry already holds for that name, if any. Same name, same canonical
/// folder, same kind is [`Registration::Adopt`]; same name with a different
/// folder or kind is [`Registration::Conflict`]; no entry is
/// [`Registration::Register`]. Pure apart from resolving the stored root
/// ([`canonical_root`]).
pub fn decide_registration(
    name: &str,
    existing: Option<&DomainEntry>,
    request: &RegistrationRequest<'_>,
) -> Registration {
    let Some(entry) = existing else {
        return Registration::Register;
    };
    match (request, entry.is_virtual()) {
        (RegistrationRequest::Virtual, true) => Registration::Adopt,
        (RegistrationRequest::Virtual, false) => Registration::Conflict(format!(
            "domain '{name}' is a file domain; pass a different name"
        )),
        (RegistrationRequest::File { .. }, true) => Registration::Conflict(format!(
            "domain '{name}' is a virtual domain; pass a different name"
        )),
        (RegistrationRequest::File { root }, false) => match canonical_root(entry) {
            Some(held) if held == *root => Registration::Adopt,
            Some(held) => Registration::Conflict(format!(
                "domain '{name}' is already registered at a different folder, {}; pass a \
                 different name, or pass that folder to keep the registration as it is",
                held.display()
            )),
            None => Registration::Conflict(format!(
                "domain '{name}' is already registered at a different folder; pass a different name"
            )),
        },
    }
}

/// A domain name derived from free text (a folder's basename, a repository's
/// name) that always passes [`validate_domain_name`]: slugified the way a
/// permalink is, `domain` when that leaves nothing, cut to fit the length cap
/// with room for a suffix, and suffixed `-2`, `-3`... while the candidate is
/// `taken` or is not a valid name (so a repository called `CON` becomes
/// `con-2`).
pub fn derive_domain_name(raw: &str, taken: impl Fn(&str) -> bool) -> String {
    let slug = crate::slugify(raw).replace('/', "-");
    let slug = if slug.is_empty() {
        "domain".to_string()
    } else {
        slug
    };
    // Room for a `-NN` suffix inside the cap; a slug is ASCII, so chars and
    // bytes agree here.
    let base: String = slug.chars().take(MAX_DOMAIN_NAME_CHARS - 4).collect();
    let base = base.trim_end_matches('-').to_string();
    let usable = |candidate: &str| validate_domain_name(candidate).is_ok() && !taken(candidate);
    if usable(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        // Shorten the base by the suffix's length, so a long base keeps every
        // candidate inside the cap however far the counter climbs (a 60
        // character base would otherwise overflow from `-1000` on and never
        // validate again).
        let suffix = format!("-{n}");
        let stem: String = base
            .chars()
            .take(MAX_DOMAIN_NAME_CHARS.saturating_sub(suffix.len()))
            .collect();
        let candidate = format!("{}{suffix}", stem.trim_end_matches('-'));
        if usable(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_name_is_one_plain_segment() {
        assert!(validate_domain_name("brand-knowledge_2").is_ok());
        for bad in [
            "", "   ", " notes ", "../up", "a/b", "a\\b", "a:b", ".hidden", "a b", "a\tb",
        ] {
            assert!(
                validate_domain_name(bad).is_err(),
                "{bad:?} names no domain"
            );
        }
        assert_eq!(
            validate_domain_name("").unwrap_err(),
            "the domain name is empty"
        );
    }

    #[test]
    fn the_allowlist_refuses_windows_hostile_punctuation_a_cap_and_bare_dots() {
        for bad in ["a*b", "a?b", "a<b", "a>b", "a|b", "a\"b", "notes."] {
            assert!(
                validate_domain_name(bad).is_err(),
                "{bad:?} names no domain"
            );
        }
        assert!(validate_domain_name(&"a".repeat(65)).is_err());
        assert!(validate_domain_name(&"a".repeat(64)).is_ok());
        for ok in ["notes.v2", "wissen", "知识库"] {
            assert!(validate_domain_name(ok).is_ok(), "{ok:?} is legal");
        }
    }

    #[test]
    fn the_allowlist_refuses_nfd_decomposed_names() {
        assert!(validate_domain_name("cafe\u{0301}").is_err());
    }

    #[test]
    fn the_allowlist_refuses_windows_reserved_device_names() {
        for bad in [
            "con",
            "CON",
            "PRN",
            "AUX",
            "NUL",
            "com1",
            "LPT9",
            "CON.txt",
            "nul.md",
            "COM\u{b9}",
            "LPT\u{b9}",
        ] {
            assert!(
                validate_domain_name(bad).is_err(),
                "{bad:?} is a device name"
            );
        }
        for ok in ["console", "com10", "a.CON"] {
            assert!(validate_domain_name(ok).is_ok(), "{ok:?} is legal");
        }
    }

    #[test]
    fn the_refusal_names_the_rules() {
        let err = validate_domain_name("a b").unwrap_err();
        assert!(
            err.starts_with("'a b' cannot name a domain: use letters"),
            "{err}"
        );
    }

    #[test]
    fn a_free_name_registers() {
        let dir = tempfile::tempdir().unwrap();
        let request = RegistrationRequest::File { root: dir.path() };
        assert_eq!(
            decide_registration("notes", None, &request),
            Registration::Register
        );
        assert_eq!(
            decide_registration("notes", None, &RegistrationRequest::Virtual),
            Registration::Register
        );
    }

    #[test]
    fn the_same_folder_is_adopted_and_a_team_entry_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut team = DomainEntry::file(root.clone());
        team.provision = Some(true);
        team.review = Some(crate::config::ReviewMode::Overlay);
        team.origin = Some(crate::config::OriginConfig {
            repo: "acme/kb".to_string(),
            path: None,
            branch: Some("trunk".to_string()),
            poll_secs: None,
        });
        let request = RegistrationRequest::File { root: &root };
        assert_eq!(
            decide_registration("kb", Some(&team), &request),
            Registration::Adopt
        );
    }

    #[test]
    fn a_different_folder_is_a_conflict_that_names_the_held_one() {
        let held = tempfile::tempdir().unwrap();
        let wanted = tempfile::tempdir().unwrap();
        let entry = DomainEntry::file(held.path());
        let wanted_root = std::fs::canonicalize(wanted.path()).unwrap();
        let request = RegistrationRequest::File { root: &wanted_root };
        let Registration::Conflict(msg) = decide_registration("kb", Some(&entry), &request) else {
            panic!("a different folder must conflict");
        };
        assert!(
            msg.contains("already registered at a different folder"),
            "{msg}"
        );
        let held_root = std::fs::canonicalize(held.path()).unwrap();
        assert!(msg.contains(&held_root.display().to_string()), "{msg}");
    }

    #[test]
    fn a_different_kind_is_a_conflict_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = DomainEntry::file(root.clone());
        let virt = DomainEntry::virtual_domain();
        assert_eq!(
            decide_registration("kb", Some(&file), &RegistrationRequest::Virtual),
            Registration::Conflict("domain 'kb' is a file domain; pass a different name".into())
        );
        assert_eq!(
            decide_registration(
                "kb",
                Some(&virt),
                &RegistrationRequest::File { root: &root }
            ),
            Registration::Conflict("domain 'kb' is a virtual domain; pass a different name".into())
        );
        assert_eq!(
            decide_registration("kb", Some(&virt), &RegistrationRequest::Virtual),
            Registration::Adopt
        );
    }

    #[test]
    fn a_grandfathered_bad_name_is_adopted_without_validation() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let entry = DomainEntry::file(root.clone());
        assert!(validate_domain_name("my notes").is_err());
        assert_eq!(
            decide_registration(
                "my notes",
                Some(&entry),
                &RegistrationRequest::File { root: &root }
            ),
            Registration::Adopt
        );
    }

    #[test]
    fn a_derived_name_always_validates() {
        let never = |_: &str| false;
        assert_eq!(derive_domain_name("Team Notes", never), "team-notes");
        assert_eq!(derive_domain_name("---", never), "domain");
        assert_eq!(derive_domain_name("CON", never), "con-2");
        assert_eq!(derive_domain_name("nul", never), "nul-2");
        let long = derive_domain_name(&"x".repeat(200), never);
        assert!(validate_domain_name(&long).is_ok(), "{long}");
        assert_eq!(derive_domain_name("notes", |n| n == "notes"), "notes-2");
    }

    #[test]
    fn a_derived_name_stays_inside_the_cap_however_high_the_suffix_climbs() {
        // The first thousand or so candidates are taken, so the suffix grows
        // to four digits; every candidate must still fit the cap, or the
        // search would never find a valid one.
        let taken = |n: &str| match n.rsplit_once('-') {
            Some((_, digits)) => digits.parse::<u32>().is_ok_and(|d| d < 1000),
            None => true,
        };
        let name = derive_domain_name(&"x".repeat(200), taken);
        assert!(validate_domain_name(&name).is_ok(), "{name}");
        assert!(name.ends_with("-1000"), "{name}");
        assert_eq!(name.chars().count(), MAX_DOMAIN_NAME_CHARS, "{name}");
    }
}
