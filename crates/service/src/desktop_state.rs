//! The second Crystalline state Claude Desktop's app package keeps on
//! Windows (`%LOCALAPPDATA%\Packages\Claude_*\LocalCache\Roaming\
//! crystalline`): found and described. A process inside the package has
//! every new file it creates under AppData redirected there, so an old
//! Claude Desktop extension and, before 0.30.0, every command run inside
//! Claude Desktop (Claude Code's shell and hooks among them) could build it.
//! `crystalline doctor --fix --merge-desktop-state` merges it into the real
//! one (the CLI's `desktop_state` module).
//!
//! The scan reads `LOCALAPPDATA` on every platform: Windows always sets it,
//! nothing else does unless a person or a test sets it.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// One private state folder and what it holds.
#[derive(Debug, Clone, Serialize)]
pub struct DesktopState {
    pub folder: PathBuf,
    /// How many domains its `config.yaml` registers.
    pub domains: usize,
    /// The names its `config.yaml` registers, in its order.
    pub domain_names: Vec<String>,
    /// `index.db` plus its write-ahead log, when there is one.
    pub index_bytes: Option<u64>,
    /// The newest change to a file directly in the folder.
    pub last_change: Option<String>,
    /// The pid of a Crystalline daemon its record names that still runs.
    pub daemon_alive: Option<u32>,
    /// The domains its config registers that the daemon does not serve,
    /// when the daemon was asked (see [`DesktopState::compare_with`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unserved: Option<Vec<String>>,
}

impl DesktopState {
    /// Note which of this state's domains `registered` (the daemon's
    /// `registered` list from `ctl status`) does not hold.
    pub fn compare_with(&mut self, registered: &[String]) {
        self.unserved = Some(
            self.domain_names
                .iter()
                .filter(|name| !registered.contains(name))
                .cloned()
                .collect(),
        );
    }
}

/// The state files a private folder and the real one can share: when one
/// of them is the same file on both sides, the merge leaves the folder
/// alone ([`shares_a_file`]).
pub const STATE_FILES: [&str; 3] = ["config.yaml", "index.db", "service.json"];

/// Every private state folder under `local_appdata` that holds state (a
/// `config.yaml` or an `index.db`), sorted by path. With `real` set, a
/// folder that is only a view of the real state folder ([`is_view_of`]) is
/// left out: nothing is split there.
pub fn scan(local_appdata: Option<&Path>, real: Option<&Path>) -> Vec<DesktopState> {
    let Some(root) = local_appdata else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root.join("Packages")) else {
        return Vec::new();
    };
    let mut found: Vec<DesktopState> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
        .map(|e| {
            e.path()
                .join("LocalCache")
                .join("Roaming")
                .join("crystalline")
        })
        .filter(|folder| holds_state(folder))
        .filter(|folder| real.is_none_or(|real| !is_view_of(folder, real)))
        .map(|folder| describe(&folder))
        .collect();
    found.sort_by(|a, b| a.folder.cmp(&b.folder));
    found
}

/// Whether the private `folder` is only another name for the real state
/// folder `real`: the folder itself is the real one, or its `config.yaml`
/// is the real `config.yaml` (by volume and file index on Windows, device
/// and inode elsewhere). The config decides, because it is what a reader
/// of the routing block sees.
pub fn is_view_of(folder: &Path, real: &Path) -> bool {
    if same_file::is_same_file(folder, real).unwrap_or(false) {
        return true;
    }
    let (private, own) = (folder.join("config.yaml"), real.join("config.yaml"));
    private.is_file() && own.is_file() && same_file::is_same_file(&private, &own).unwrap_or(false)
}

/// Whether the private `folder` and the real state folder `real` share any
/// state file (one of [`STATE_FILES`] is the same file on both sides). The
/// guard of the merge: a folder that shares a file with the real one is
/// never merged or renamed, even when its config is its own.
pub fn shares_a_file(folder: &Path, real: &Path) -> bool {
    is_view_of(folder, real)
        || STATE_FILES.iter().any(|name| {
            let (private, own) = (folder.join(name), real.join(name));
            private.is_file()
                && own.is_file()
                && same_file::is_same_file(&private, &own).unwrap_or(false)
        })
}

/// Whether `folder` holds Crystalline state: a `config.yaml` or an
/// `index.db`. An empty folder an old extension left behind holds none.
fn holds_state(folder: &Path) -> bool {
    ["config.yaml", "index.db"]
        .iter()
        .any(|name| folder.join(name).is_file())
}

/// [`scan`] of this machine's `LOCALAPPDATA`. Outside an app package the
/// file identity check runs against this machine's state folder. Inside
/// one it cannot: there both names open the package's private copy first,
/// so they would look like one file exactly when the state is split.
pub fn scan_here() -> Vec<DesktopState> {
    let real = (!crate::runs_in::PackageContext::here().is_packaged())
        .then(|| crystalline_core::config::state_dir().ok())
        .flatten();
    scan(
        crystalline_core::config::local_appdata().as_deref(),
        real.as_deref(),
    )
}

fn describe(folder: &Path) -> DesktopState {
    let domain_names: Vec<String> = crystalline_core::config::load_yaml::<
        crystalline_core::config::GlobalConfig,
    >(&folder.join("config.yaml"))
    .map(|cfg| cfg.domains.keys().cloned().collect())
    .unwrap_or_default();
    let index_bytes = ["index.db", "index.db-wal"]
        .iter()
        .filter_map(|name| std::fs::metadata(folder.join(name)).ok())
        .map(|meta| meta.len())
        .reduce(|a, b| a + b);
    let last_change = std::fs::read_dir(folder)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
        .map(|time| {
            chrono::DateTime::<chrono::Utc>::from(time)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        });
    let daemon_alive = std::fs::read_to_string(folder.join("service.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<crate::instance::LockInfo>(&text).ok())
        .map(|record| record.pid)
        .filter(|pid| crate::instance::process_is_crystalline(*pid));
    DesktopState {
        folder: folder.to_path_buf(),
        domains: domain_names.len(),
        domain_names,
        index_bytes,
        last_change,
        daemon_alive,
        unserved: None,
    }
}

/// The line the MCP bridge adds to a session's onboarding instructions when
/// Claude Desktop's package keeps a second state that registers domains:
/// the person is in the chat, not in a terminal, and that state is why the
/// routing block and the searches can disagree. With `registered` (the
/// daemon's `registered` list) the note names the private domains the
/// daemon does not serve, and there is none when it serves them all;
/// without it (the daemon could not be asked) it says what may be wrong.
/// `None` without a state that registers domains.
pub fn split_note(states: &[DesktopState], registered: Option<&[String]>) -> Option<String> {
    let state = states.iter().find(|s| s.domains > 0)?;
    let ask = "Ask the user to quit Claude Desktop and run `crystalline doctor --fix --merge-desktop-state` in a terminal, which merges it into the daemon's state.";
    match registered {
        Some(registered) => {
            let unserved: Vec<&str> = states
                .iter()
                .flat_map(|s| s.domain_names.iter())
                .filter(|name| !registered.contains(name))
                .map(String::as_str)
                .collect();
            if unserved.is_empty() {
                return None;
            }
            Some(format!(
                "Claude Desktop keeps a second Crystalline state in its app package ({}), and it registers {} domain{} the Crystalline daemon does not serve ({}), so a routing block read inside Claude Desktop can list {} while the searches do not find {}. {ask}",
                state.folder.display(),
                unserved.len(),
                if unserved.len() == 1 { "" } else { "s" },
                unserved.join(", "),
                if unserved.len() == 1 { "it" } else { "them" },
                if unserved.len() == 1 { "it" } else { "them" },
            ))
        }
        None => Some(format!(
            "Claude Desktop keeps a second Crystalline state in its app package ({}, {} domain{}). It may register domains the Crystalline daemon does not serve: if the searches miss a domain the routing block lists, that is why. {ask}",
            state.folder.display(),
            state.domains,
            if state.domains == 1 { "" } else { "s" }
        )),
    }
}

/// The private state folder of the package `full_name` (a package full
/// name such as `Claude_1.0.0.0_x64__pzs8sxrjxfjjc`: its family is the
/// first and the last part) under `local_appdata`.
pub fn private_folder_of(full_name: &str, local_appdata: &Path) -> PathBuf {
    let mut parts = full_name.split('_');
    let name = parts.next().unwrap_or(full_name);
    let family = match parts.next_back() {
        Some(publisher) if !publisher.is_empty() => format!("{name}_{publisher}"),
        _ => full_name.to_string(),
    };
    local_appdata
        .join("Packages")
        .join(family)
        .join("LocalCache")
        .join("Roaming")
        .join("crystalline")
}

/// What a `status` or `doctor` run inside an app package says about what
/// it reads: the state folder opens the package's private copy first.
/// `None` outside a package.
pub fn packaged_read_line() -> Option<String> {
    let crate::runs_in::PackageContext::Packaged { full_name } =
        crate::runs_in::PackageContext::here()
    else {
        return None;
    };
    let private = crystalline_core::config::local_appdata()
        .map(|local| private_folder_of(full_name, &local).display().to_string())
        .unwrap_or_else(|| "the package's LocalCache folder".to_string());
    Some(format!(
        "This command runs inside Claude Desktop's app package ({full_name}), so what it reads from the Crystalline state folder comes from the package's private copy first ({private}). Run it in a terminal outside Claude Desktop for this machine's own view."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plant(local: &Path, family: &str, config: &str) -> PathBuf {
        let folder = local
            .join("Packages")
            .join(family)
            .join("LocalCache")
            .join("Roaming")
            .join("crystalline");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("config.yaml"), config).unwrap();
        folder
    }

    #[test]
    fn a_separate_private_config_is_reported_with_its_domain_names() {
        let local = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        std::fs::write(real.path().join("config.yaml"), "domains: {}\n").unwrap();
        plant(
            local.path(),
            "Claude_pzs8sxrjxfjjc",
            "domains:\n  notes:\n    path: /n\n  work:\n    path: /w\n",
        );
        let found = scan(Some(local.path()), Some(real.path()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].domains, 2);
        assert_eq!(found[0].domain_names, vec!["notes", "work"]);
    }

    /// A private folder whose config is the real config under another name
    /// (a hard link here, the same file ID the 0.22.1 report showed) is a
    /// view, not a split: neither reported nor ever merged.
    #[test]
    fn a_private_folder_that_is_a_view_of_the_real_one_is_not_reported() {
        let local = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        let real_config = real.path().join("config.yaml");
        std::fs::write(&real_config, "domains:\n  notes:\n    path: /n\n").unwrap();
        let folder = local
            .path()
            .join("Packages")
            .join("Claude_pzs8sxrjxfjjc")
            .join("LocalCache")
            .join("Roaming")
            .join("crystalline");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::hard_link(&real_config, folder.join("config.yaml")).unwrap();
        assert!(is_view_of(&folder, real.path()));
        assert!(scan(Some(local.path()), Some(real.path())).is_empty());
        assert_eq!(
            scan(Some(local.path()), None).len(),
            1,
            "without the real folder to compare with, the folder is reported"
        );
    }

    #[test]
    fn the_comparison_names_the_domains_the_daemon_does_not_serve() {
        let local = tempfile::tempdir().unwrap();
        plant(
            local.path(),
            "Claude_x",
            "domains:\n  notes:\n    path: /n\n  work:\n    path: /w\n",
        );
        let mut state = scan(Some(local.path()), None).remove(0);
        state.compare_with(&["work".to_string()]);
        assert_eq!(state.unserved, Some(vec!["notes".to_string()]));
    }

    #[test]
    fn the_split_note_needs_a_state_with_domains() {
        let local = tempfile::tempdir().unwrap();
        plant(local.path(), "Claude_x", "domains: {}\n");
        assert!(split_note(&scan(Some(local.path()), None), None).is_none());
        plant(
            local.path(),
            "Claude_y",
            "domains:\n  notes:\n    path: /n\n",
        );
        let note = split_note(&scan(Some(local.path()), None), None).unwrap();
        assert!(note.contains("1 domain)"), "{note}");
        assert!(note.contains("It may register"), "{note}");
        assert!(note.contains("--merge-desktop-state"), "{note}");
    }

    #[test]
    fn the_split_note_names_only_what_the_daemon_does_not_serve() {
        let local = tempfile::tempdir().unwrap();
        plant(
            local.path(),
            "Claude_y",
            "domains:\n  notes:\n    path: /n\n  work:\n    path: /w\n",
        );
        let states = scan(Some(local.path()), None);
        let note = split_note(&states, Some(&["work".to_string()])).unwrap();
        assert!(
            note.contains("1 domain the Crystalline daemon does not serve (notes)"),
            "{note}"
        );
        assert!(
            split_note(&states, Some(&["notes".to_string(), "work".to_string()])).is_none(),
            "nothing to say when the daemon serves them all"
        );
    }

    /// A private folder whose config is its own but whose index is the real
    /// one is reported, and the merge guard still sees the shared file.
    #[test]
    fn a_shared_index_alone_is_no_view_but_guards_the_merge() {
        let local = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        std::fs::write(real.path().join("index.db"), [0u8; 8]).unwrap();
        std::fs::write(real.path().join("config.yaml"), "domains: {}\n").unwrap();
        let folder = plant(
            local.path(),
            "Claude_z",
            "domains:\n  notes:\n    path: /n\n",
        );
        std::fs::hard_link(real.path().join("index.db"), folder.join("index.db")).unwrap();
        assert!(!is_view_of(&folder, real.path()));
        assert!(shares_a_file(&folder, real.path()));
        assert_eq!(scan(Some(local.path()), Some(real.path())).len(), 1);
    }

    #[test]
    fn the_private_folder_is_named_by_the_package_family() {
        let folder = private_folder_of("Claude_1.0.0.0_x64__pzs8sxrjxfjjc", Path::new("/local"));
        assert_eq!(
            folder,
            Path::new("/local")
                .join("Packages")
                .join("Claude_pzs8sxrjxfjjc")
                .join("LocalCache")
                .join("Roaming")
                .join("crystalline")
        );
    }
}
