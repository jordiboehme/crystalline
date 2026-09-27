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

use super::{DomainEntry, NameOrigin};

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
    // A slug carries no `.`, so the stepping below trims exactly what this
    // function always trimmed.
    step_domain_name(base.trim_end_matches('-'), taken)
}

/// The default name for a domain rooted at GitHub repository `repo`
/// (`owner/name`): the repository's own name segment, run through
/// [`derive_domain_name`] against no existing names. Callers that must
/// avoid a collision pass the result on to [`step_domain_name`] or
/// [`choose_domain_name`], which take their own `taken` predicate.
pub fn default_repo_domain_name(repo: &str) -> String {
    let segment = repo.rsplit('/').next().unwrap_or(repo);
    derive_domain_name(segment, |_| false)
}

/// The default name for a local file domain rooted at `root`: the folder's
/// basename, slugified the same way [`default_repo_domain_name`] slugifies a
/// repository segment.
pub fn default_folder_domain_name(root: &Path) -> String {
    let base = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    derive_domain_name(&base, |_| false)
}

/// `name` itself when it is a valid, free domain name, else `name-2`,
/// `name-3`... cut to fit the length cap. Unlike [`derive_domain_name`] the
/// spelling is kept exactly as given (no slugify): a MANIFEST's
/// `domain_name` is already a valid name, and stepping must not silently
/// rewrite it. `name` must already pass [`validate_domain_name`] itself, or
/// no `-N` candidate built from it ever will either, and the search never
/// terminates.
pub fn step_domain_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let usable = |candidate: &str| validate_domain_name(candidate).is_ok() && !taken(candidate);
    if usable(name) {
        return name.to_string();
    }
    let mut n = 2;
    loop {
        // Shorten the stem by the suffix's length so every candidate stays
        // inside the cap however high the counter climbs.
        let suffix = format!("-{n}");
        let stem: String = name
            .chars()
            .take(MAX_DOMAIN_NAME_CHARS.saturating_sub(suffix.chars().count()))
            .collect();
        let candidate = format!("{}{suffix}", stem.trim_end_matches(['-', '.']));
        if usable(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Whether `name` looks like it was worked out for `entry` rather than
/// chosen on purpose, for a domain registered before `name_origin` existed.
/// A name set by an environment variable (`env_defined`) is always
/// [`NameOrigin::Explicit`], and so is a virtual domain's: it has no
/// repository or folder to derive a default from. Otherwise `name` counts
/// as [`NameOrigin::Derived`] when it equals the repository default (an
/// entry with `origin` set) or the folder default (a plain file entry),
/// matching either the folder's raw basename or its slugified form.
pub fn infer_name_origin(name: &str, entry: &DomainEntry, env_defined: bool) -> NameOrigin {
    if env_defined || entry.is_virtual() {
        return NameOrigin::Explicit;
    }
    if let Some(origin) = &entry.origin
        && name == default_repo_domain_name(&origin.repo)
    {
        return NameOrigin::Derived;
    }
    if let Some(root) = entry.file_path() {
        let raw = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == raw || name == default_folder_domain_name(&root) {
            return NameOrigin::Derived;
        }
    }
    NameOrigin::Explicit
}

/// The result of [`choose_domain_name`]: the name to register, how it was
/// arrived at, and, when a MANIFEST-declared name had to step because it was
/// already taken, the canonical name that stepping leaves shadowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameChoice {
    /// The name to register.
    pub name: String,
    /// How `name` was arrived at.
    pub origin: NameOrigin,
    /// The MANIFEST-declared canonical name, when stepping moved away from
    /// it because it was already taken.
    pub shadowed_canonical: Option<String>,
}

/// Picks the name a new registration gets, in order: `explicit` when given
/// (always [`NameOrigin::Explicit`]); else `manifest_name` when it is a
/// valid domain name, stepped by [`step_domain_name`] if `taken` and
/// reporting the original as `shadowed_canonical` when stepping changed it
/// (always [`NameOrigin::Derived`]); else `default_name()`
/// ([`NameOrigin::Derived`]). `validate_domain_name` is deliberately not
/// applied to `explicit` here: callers keep today's order of running
/// [`decide_registration`] first and [`validate_domain_name`] only after.
pub fn choose_domain_name(
    explicit: Option<&str>,
    manifest_name: Option<&str>,
    default_name: impl FnOnce() -> String,
    taken: impl Fn(&str) -> bool,
) -> NameChoice {
    if let Some(name) = explicit {
        return NameChoice {
            name: name.to_string(),
            origin: NameOrigin::Explicit,
            shadowed_canonical: None,
        };
    }
    if let Some(declared) = manifest_name.filter(|n| validate_domain_name(n).is_ok()) {
        let name = step_domain_name(declared, &taken);
        let shadowed_canonical = (name != declared).then(|| declared.to_string());
        return NameChoice {
            name,
            origin: NameOrigin::Derived,
            shadowed_canonical,
        };
    }
    NameChoice {
        name: default_name(),
        origin: NameOrigin::Derived,
        shadowed_canonical: None,
    }
}

/// Whether a registration should write `domain_name` into the MANIFEST.
/// Never when the MANIFEST already declares one (`manifest_declares`), and
/// never for a team domain (`entry.origin` set) whatever its name's origin -
/// explicit or derived. The owner adds the name upstream by hand; an
/// automatic write would plant a pending local change that blocks review
/// mode and conflicts once the owner adds the name upstream. Otherwise
/// written for every plain local file or virtual domain (no `origin`),
/// whatever its name's origin.
pub fn needs_manifest_write_back(entry: &DomainEntry, manifest_declares: bool) -> bool {
    !manifest_declares && entry.origin.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OriginConfig;

    #[test]
    fn the_repo_default_is_the_last_segment_slugified() {
        assert_eq!(
            default_repo_domain_name("acme/Eng-Knowledge"),
            "eng-knowledge"
        );
        assert_eq!(default_repo_domain_name("acme/CON"), "con-2");
    }

    #[test]
    fn the_folder_default_is_the_basename_slugified() {
        assert_eq!(
            default_folder_domain_name(Path::new("/x/My Notes")),
            "my-notes"
        );
    }

    #[test]
    fn stepping_keeps_the_spelling_and_appends_a_counter() {
        let taken = |n: &str| matches!(n, "Eng.Docs" | "Eng.Docs-2");
        assert_eq!(step_domain_name("Eng.Docs", taken), "Eng.Docs-3");
        assert_eq!(step_domain_name("free", |_| false), "free");
        let long = "a".repeat(64);
        let stepped = step_domain_name(&long, |n| n == long);
        assert!(
            stepped.chars().count() <= MAX_DOMAIN_NAME_CHARS,
            "{stepped}"
        );
        assert!(stepped.ends_with("-2"));
        assert!(validate_domain_name(&stepped).is_ok());
    }

    #[test]
    fn an_explicit_name_wins_and_is_explicit() {
        let c = choose_domain_name(Some("ops"), Some("platform"), || "repo".into(), |_| false);
        assert_eq!(
            c,
            NameChoice {
                name: "ops".into(),
                origin: NameOrigin::Explicit,
                shadowed_canonical: None
            }
        );
    }

    #[test]
    fn the_manifest_name_comes_next_and_is_derived() {
        let c = choose_domain_name(None, Some("platform"), || "repo".into(), |_| false);
        assert_eq!(
            c,
            NameChoice {
                name: "platform".into(),
                origin: NameOrigin::Derived,
                shadowed_canonical: None
            }
        );
    }

    #[test]
    fn a_taken_manifest_name_steps_and_reports_the_shadowed_canonical() {
        let c = choose_domain_name(
            None,
            Some("platform"),
            || "repo".into(),
            |n| n == "platform",
        );
        assert_eq!(c.name, "platform-2");
        assert_eq!(c.origin, NameOrigin::Derived);
        assert_eq!(c.shadowed_canonical.as_deref(), Some("platform"));
    }

    #[test]
    fn an_invalid_manifest_name_is_ignored_for_the_default() {
        let c = choose_domain_name(None, Some("../up"), || "repo".into(), |_| false);
        assert_eq!(c.name, "repo");
        assert_eq!(c.origin, NameOrigin::Derived);
    }

    #[test]
    fn inference_counts_the_two_defaults_as_derived() {
        let team = DomainEntry {
            origin: Some(OriginConfig {
                repo: "acme/eng-knowledge".into(),
                path: None,
                branch: None,
                poll_secs: None,
            }),
            ..DomainEntry::file("/x/y")
        };
        assert_eq!(
            infer_name_origin("eng-knowledge", &team, false),
            NameOrigin::Derived
        );
        assert_eq!(infer_name_origin("eng", &team, false), NameOrigin::Explicit);
        let local = DomainEntry::file("/x/My Notes");
        assert_eq!(
            infer_name_origin("my-notes", &local, false),
            NameOrigin::Derived
        );
        assert_eq!(
            infer_name_origin("My Notes", &local, false),
            NameOrigin::Derived
        );
        assert_eq!(
            infer_name_origin("journal", &local, false),
            NameOrigin::Explicit
        );
        assert_eq!(
            infer_name_origin("my-notes", &local, true),
            NameOrigin::Explicit
        );
        assert_eq!(
            infer_name_origin("scratch", &DomainEntry::virtual_domain(), false),
            NameOrigin::Explicit
        );
    }

    #[test]
    fn write_back_skips_every_team_domain_explicit_or_derived() {
        let team = DomainEntry {
            origin: Some(OriginConfig {
                repo: "acme/eng".into(),
                path: None,
                branch: None,
                poll_secs: None,
            }),
            ..DomainEntry::file("/x")
        };
        assert!(!needs_manifest_write_back(&team, false));
        let team_explicit = DomainEntry {
            name_origin: Some(NameOrigin::Explicit),
            ..team
        };
        assert!(!needs_manifest_write_back(&team_explicit, false));
        assert!(needs_manifest_write_back(&DomainEntry::file("/x"), false));
        assert!(needs_manifest_write_back(
            &DomainEntry::virtual_domain(),
            false
        ));
        assert!(!needs_manifest_write_back(&DomainEntry::file("/x"), true));
    }

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
