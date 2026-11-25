//! The one-time pin of the old default domains root.
//!
//! Until 0.36 an unset `domains_root` meant `~/Documents/Crystalline`; from
//! 0.37.0 it means the hidden `~/.crystalline`. An install whose domains live
//! under the old default gets `domains_root: ~/Documents/Crystalline` written
//! into its configuration once, so its new domains keep landing beside the
//! old ones. Nothing is moved.
//!
//! This module holds the pure decision ([`is_due`], [`due`]), the change
//! itself ([`pin`]) and the record that makes it happen once
//! ([`PinRecord`]). The engine and the registration functions call them
//! under their own locks.
//!
//! The decision never opens or stats anything inside `~/Documents`: on macOS
//! that can be refused, or raise a privacy prompt, and a fresh install must
//! never touch the folder at all. Paths are compared by their text
//! ([`crate::path::lies_within`]). The one exception is the best effort
//! second root for a symlinked Documents folder ([`legacy_roots`]), which
//! resolves `~/Documents` itself, a name in the home, and only when nothing
//! matched by text.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{ConfigError, GlobalConfig, LEGACY_DOMAINS_ROOT, expand_tilde_in, save_bytes};

/// The record's file name in the state folder.
pub const RECORD_FILE: &str = "domains-root-pin.json";

/// The record key of a configuration kept as a database document (document
/// mode). A configuration file is keyed by its absolute path.
pub const DOCUMENT_KEY: &str = "document";

/// The log line a pin writes, the same from every caller.
pub const PINNED_NOTE: &str = "new domains keep landing under ~/Documents/Crystalline beside the existing ones: domains_root was set to ~/Documents/Crystalline";

/// Whether paths compare without regard to case here: the default
/// filesystems of macOS and Windows ignore it.
pub fn fold_case() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

/// The record key of a configuration: the file's path, or
/// [`DOCUMENT_KEY`] when there is no file (document mode).
///
/// One file has one key however it is spelled: the path is resolved
/// (symbolic links followed, `.` and `..` applied) through its nearest part
/// that exists, so a file that is not there yet gets the key it will have,
/// and on macOS and Windows the case is folded ([`fold_case`]).
pub fn config_key(config_path: Option<&Path>) -> String {
    let Some(path) = config_path else {
        return DOCUMENT_KEY.to_string();
    };
    let key = resolved(path).display().to_string();
    if fold_case() {
        crate::path::fold_path_case(&key)
    } else {
        key
    }
}

/// `path` made absolute and resolved through its nearest existing
/// ancestor, with the parts below it joined back as they are spelled. The
/// absolute path when nothing resolves.
fn resolved(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut tail = Vec::new();
    let mut current = absolute.as_path();
    loop {
        if let Ok(real) = std::fs::canonicalize(current) {
            return tail.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                current = parent;
            }
            _ => return absolute,
        }
    }
}

/// The persisted file domains' paths, tilde-expanded against `home`. Virtual
/// domains have no folder and never count.
fn file_domain_paths<'a>(
    persisted: &'a GlobalConfig,
    home: &'a Path,
) -> impl Iterator<Item = PathBuf> + 'a {
    persisted
        .domains
        .values()
        .filter(|entry| !entry.is_virtual())
        .filter_map(|entry| entry.path.as_ref())
        .map(move |path| expand_tilde_in(&path.to_string_lossy(), home))
}

/// Whether the persisted configuration registers at least one file domain.
pub fn has_file_domains(persisted: &GlobalConfig) -> bool {
    persisted
        .domains
        .values()
        .any(|entry| !entry.is_virtual() && entry.path.is_some())
}

/// The old default under `home`, by text: `<home>/Documents/Crystalline`.
pub fn lexical_root(home: &Path) -> PathBuf {
    expand_tilde_in(LEGACY_DOMAINS_ROOT, home)
}

/// The roots a domain must lie under for the pin: always the
/// [`lexical_root`], and, only when the configuration has file domains and
/// none of them matched by text, the same root under the resolved
/// `~/Documents` (a symlinked Documents folder). The resolve reads the
/// `Documents` name in the home, never anything inside it; a failure is
/// ignored. With a text match, or no file domains, nothing is touched.
pub fn legacy_roots(home: &Path, lexical_hit: bool, has_file_domains: bool) -> Vec<PathBuf> {
    let lexical = lexical_root(home);
    let mut roots = vec![lexical.clone()];
    if has_file_domains
        && !lexical_hit
        && let Ok(documents) = std::fs::canonicalize(home.join("Documents"))
    {
        let canonical = documents.join("Crystalline");
        if canonical != lexical {
            roots.push(canonical);
        }
    }
    roots
}

/// Whether `persisted` is due for the pin against `legacy_roots`: no
/// `domains_root` set, and at least one file domain at or under one of the
/// roots. Pure: it compares text only. Case is folded as [`fold_case`] says.
pub fn due(persisted: &GlobalConfig, home: &Path, legacy_roots: &[PathBuf]) -> bool {
    due_with(persisted, home, legacy_roots, fold_case())
}

/// [`due`] with the case rule named, so a test of Windows paths reads the
/// same on any OS.
pub fn due_with(
    persisted: &GlobalConfig,
    home: &Path,
    legacy_roots: &[PathBuf],
    fold_case: bool,
) -> bool {
    if persisted.domains_root.is_some() {
        return false;
    }
    file_domain_paths(persisted, home).any(|path| {
        legacy_roots
            .iter()
            .any(|root| crate::path::lies_within(&path, root, fold_case))
    })
}

/// The whole decision for `persisted` under `home`: [`due`] against the
/// text root first, then, only when that found nothing and there are file
/// domains, against the resolved root as well ([`legacy_roots`]). The record
/// is the caller's part.
pub fn is_due(persisted: &GlobalConfig, home: &Path) -> bool {
    if persisted.domains_root.is_some() || !has_file_domains(persisted) {
        return false;
    }
    if due(persisted, home, &[lexical_root(home)]) {
        return true;
    }
    let roots = legacy_roots(home, false, true);
    roots.len() > 1 && due(persisted, home, &roots)
}

/// [`is_due`] under the user's own home. False when there is no home.
pub fn is_due_here(persisted: &GlobalConfig) -> bool {
    super::home_dir().is_ok_and(|home| is_due(persisted, &home))
}

/// Set the pin: `domains_root` becomes the literal `~/Documents/Crystalline`.
pub fn pin(cfg: &mut GlobalConfig) {
    cfg.domains_root = Some(PathBuf::from(LEGACY_DOMAINS_ROOT));
}

/// The record of the configurations that were checked, pinned or not, in
/// `<state_dir>/domains-root-pin.json`. A key in it is never checked again,
/// so `crystalline config unset domains_root` after the pin is not undone at
/// the next start. A lost state folder loses the record, and then a check can
/// pin again: accepted, the pin only keeps the old place.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PinRecord {
    /// The record's format, 1.
    pub v: u32,
    /// Every configuration checked so far.
    #[serde(default)]
    pub checked: Vec<CheckedConfig>,
}

/// One checked configuration in a [`PinRecord`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedConfig {
    /// The configuration's key ([`config_key`]).
    pub config: String,
    /// Whether the check wrote the pin.
    pub pinned: bool,
    /// When it was checked, RFC 3339.
    pub at: String,
    /// The Crystalline version that checked it.
    pub version: String,
}

impl PinRecord {
    /// The record's path in `state_dir`.
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join(RECORD_FILE)
    }

    /// The record in `state_dir`. A missing file is an empty record. A file
    /// that cannot be read or does not parse is an empty record too, and the
    /// second value says why, for the caller to log; the next
    /// [`PinRecord::record`] rewrites it.
    pub fn load(state_dir: &Path) -> (PinRecord, Option<String>) {
        let path = Self::path(state_dir);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (PinRecord::default(), None);
            }
            Err(e) => {
                return (
                    PinRecord::default(),
                    Some(format!("cannot read {}: {e}", path.display())),
                );
            }
        };
        match serde_json::from_slice::<PinRecord>(&bytes) {
            Ok(record) => (record, None),
            Err(e) => (
                PinRecord::default(),
                Some(format!(
                    "{} does not parse, read as empty: {e}",
                    path.display()
                )),
            ),
        }
    }

    /// Whether the configuration `key` was checked before.
    pub fn checked(&self, key: &str) -> bool {
        self.checked.iter().any(|c| c.config == key)
    }

    /// Add `key` to the record in `state_dir`, pinned or not, unless it is
    /// there already. Re-reads the file first, so a key another process just
    /// added is kept. Written atomically.
    pub fn record(state_dir: &Path, key: &str, pinned: bool) -> Result<(), ConfigError> {
        let (mut record, _) = Self::load(state_dir);
        if record.checked(key) {
            return Ok(());
        }
        record.v = 1;
        record.checked.push(CheckedConfig {
            config: key.to_string(),
            pinned,
            at: crate::time::now().rfc3339_secs_z(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        });
        let mut bytes =
            serde_json::to_vec_pretty(&record).map_err(|e| ConfigError::Yaml(e.to_string()))?;
        bytes.push(b'\n');
        save_bytes(&Self::path(state_dir), &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DomainEntry;

    fn with(paths: &[&str]) -> GlobalConfig {
        let mut cfg = GlobalConfig::default();
        for (i, p) in paths.iter().enumerate() {
            cfg.domains
                .insert(format!("d{i}"), DomainEntry::file(PathBuf::from(p)));
        }
        cfg
    }

    fn home() -> PathBuf {
        PathBuf::from("/Users/ada")
    }

    fn due_here(cfg: &GlobalConfig, fold: bool) -> bool {
        due_with(cfg, &home(), &[lexical_root(&home())], fold)
    }

    #[test]
    fn a_domain_under_the_old_default_is_due_in_every_spelling() {
        assert!(due_here(&with(&["~/Documents/Crystalline/kb"]), false));
        assert!(due_here(
            &with(&["/Users/ada/Documents/Crystalline/kb"]),
            false
        ));
        assert!(due_here(
            &with(&["/Users/ada/Documents/Crystalline"]),
            false
        ));
        assert!(due_here(
            &with(&["/Users/ada/./Documents/x/../Crystalline/kb"]),
            false
        ));
        assert!(due_here(
            &with(&["/Users/ada/documents/crystalline/kb"]),
            true
        ));
        assert!(!due_here(
            &with(&["/Users/ada/documents/crystalline/kb"]),
            false
        ));
    }

    #[test]
    fn a_windows_verbatim_path_is_due_with_folding() {
        let home = PathBuf::from(r"C:\Users\ada");
        let root = expand_tilde_in(LEGACY_DOMAINS_ROOT, &home);
        let cfg = with(&[r"\\?\C:\Users\ada\Documents\Crystalline\kb"]);
        assert!(due_with(&cfg, &home, std::slice::from_ref(&root), true));
        let cfg = with(&[r"c:\users\ADA\documents\crystalline\kb"]);
        assert!(due_with(&cfg, &home, &[root], true));
    }

    #[test]
    fn nothing_else_is_due() {
        let mut rooted = with(&["~/Documents/Crystalline/kb"]);
        rooted.domains_root = Some(PathBuf::from("~/Documents/Crystalline"));
        assert!(!due_here(&rooted, true));
        assert!(!due_here(&GlobalConfig::default(), true));
        let mut virtual_only = GlobalConfig::default();
        virtual_only
            .domains
            .insert("v".into(), DomainEntry::virtual_domain());
        assert!(!due_here(&virtual_only, true));
        for elsewhere in [
            "~/.crystalline/kb",
            "~/Documents/notes",
            "~/Documents/Crystalline-old/kb",
            "/knowledge/team",
        ] {
            assert!(!due_here(&with(&[elsewhere]), true), "{elsewhere}");
        }
        // A domain from the environment overlay is not in the persisted
        // configuration the decision reads, so it never counts.
        assert!(!is_due(&GlobalConfig::default(), &home()));
    }

    #[test]
    fn is_due_needs_no_disk_for_a_text_match() {
        // The home does not exist: a text match decides without a resolve.
        let cfg = with(&["/Users/ada/Documents/Crystalline/kb"]);
        assert!(is_due(&cfg, &home()));
        assert!(!is_due(&with(&["/srv/kb"]), &home()));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_documents_folder_counts_through_the_resolved_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let real = tmp.path().join("cloud").join("Documents");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(real.join("Crystalline/kb")).unwrap();
        std::os::unix::fs::symlink(&real, home.join("Documents")).unwrap();
        let resolved = std::fs::canonicalize(&real).unwrap();
        let cfg = with(&[&resolved.join("Crystalline/kb").to_string_lossy()]);
        assert!(!due(&cfg, &home, &[lexical_root(&home)]));
        assert!(is_due(&cfg, &home));
        assert_eq!(legacy_roots(&home, false, true).len(), 2);
        // A text hit, or no file domains, resolves nothing.
        assert_eq!(legacy_roots(&home, true, true), vec![lexical_root(&home)]);
        assert_eq!(legacy_roots(&home, false, false), vec![lexical_root(&home)]);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_documents_folder_is_never_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let docs = home.join("Documents");
        std::fs::create_dir(&docs).unwrap();
        std::fs::set_permissions(&docs, std::fs::Permissions::from_mode(0o000)).unwrap();
        let text_hit = with(&[&docs.join("Crystalline/kb").to_string_lossy()]);
        assert!(is_due(&text_hit, &home));
        assert!(!is_due(&GlobalConfig::default(), &home));
        assert!(!is_due(&with(&["/srv/kb"]), &home));
        std::fs::set_permissions(&docs, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn pin_writes_the_literal_old_default() {
        let mut cfg = with(&["~/Documents/Crystalline/kb"]);
        pin(&mut cfg);
        assert_eq!(
            cfg.domains_root.as_deref(),
            Some(Path::new("~/Documents/Crystalline"))
        );
        assert!(!is_due(&cfg, &home()));
    }

    #[test]
    fn the_record_keeps_each_key_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (empty, problem) = PinRecord::load(tmp.path());
        assert_eq!(empty, PinRecord::default());
        assert!(problem.is_none());

        PinRecord::record(tmp.path(), "/a/config.yaml", true).unwrap();
        PinRecord::record(tmp.path(), "/a/config.yaml", false).unwrap();
        PinRecord::record(tmp.path(), DOCUMENT_KEY, false).unwrap();
        let (record, problem) = PinRecord::load(tmp.path());
        assert!(problem.is_none());
        assert_eq!(record.v, 1);
        assert_eq!(record.checked.len(), 2);
        assert!(record.checked("/a/config.yaml"));
        assert!(record.checked[0].pinned, "the first entry wins");
        assert!(record.checked(DOCUMENT_KEY));
        assert!(!record.checked("/b/config.yaml"));
    }

    #[test]
    fn a_record_that_does_not_parse_reads_as_empty_and_is_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(PinRecord::path(tmp.path()), b"{not json").unwrap();
        let (record, problem) = PinRecord::load(tmp.path());
        assert_eq!(record, PinRecord::default());
        assert!(problem.unwrap().contains("does not parse"));
        PinRecord::record(tmp.path(), "k", false).unwrap();
        let (record, problem) = PinRecord::load(tmp.path());
        assert!(problem.is_none());
        assert!(record.checked("k"));
    }

    #[test]
    fn a_config_file_is_keyed_by_its_absolute_path() {
        assert_eq!(config_key(None), DOCUMENT_KEY);
        let key = config_key(Some(Path::new("rel/config.yaml")));
        assert!(Path::new(&key).is_absolute(), "{key}");
    }

    #[test]
    fn a_config_file_has_one_key_however_it_is_spelled() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Config");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.yaml");
        // The same key before and after the file exists.
        let before = config_key(Some(&file));
        std::fs::write(&file, "domains: {}\n").unwrap();
        assert_eq!(config_key(Some(&file)), before);
        // A `.` and a `..` in the spelling.
        let dotted = dir.join(".").join("x").join("..").join("config.yaml");
        std::fs::create_dir_all(dir.join("x")).unwrap();
        assert_eq!(config_key(Some(&dotted)), before);
        // Through a linked folder, and through a linked file.
        #[cfg(unix)]
        {
            let linked_dir = tmp.path().join("linked");
            std::os::unix::fs::symlink(&dir, &linked_dir).unwrap();
            assert_eq!(config_key(Some(&linked_dir.join("config.yaml"))), before);
            let linked_file = tmp.path().join("config.yaml");
            std::os::unix::fs::symlink(&file, &linked_file).unwrap();
            assert_eq!(config_key(Some(&linked_file)), before);
        }
        // Another case, where the filesystem ignores it.
        if fold_case() {
            let upper = tmp.path().join("CONFIG").join("Config.YAML");
            assert_eq!(config_key(Some(&upper)), before);
        }
    }
}
