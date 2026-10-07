//! The private state an old Claude Desktop extension left inside Claude
//! Desktop's package on Windows (`%LOCALAPPDATA%\Packages\Claude_*\
//! LocalCache\Roaming\crystalline`): found, described and, with
//! `doctor --fix --merge-desktop-state`, merged into the real one.
//!
//! The scan reads `LOCALAPPDATA` on every platform: Windows always sets it,
//! nothing else does unless a person or a test sets it.

use std::path::{Path, PathBuf};

use crystalline_core::config::{DomainEntry, GlobalConfig};
use serde::Serialize;

/// One private state folder and what it holds.
#[derive(Debug, Clone, Serialize)]
pub struct DesktopState {
    pub folder: PathBuf,
    /// How many domains its `config.yaml` registers.
    pub domains: usize,
    /// `index.db` plus its write-ahead log, when there is one.
    pub index_bytes: Option<u64>,
    /// The newest change to a file directly in the folder.
    pub last_change: Option<String>,
    /// The pid of a Crystalline daemon its record names that still runs.
    pub daemon_alive: Option<u32>,
}

/// Every private state folder under `local_appdata` that holds state (see
/// [`holds_state`]), sorted by path.
pub fn scan(local_appdata: Option<&Path>) -> Vec<DesktopState> {
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
        .map(|folder| describe(&folder))
        .collect();
    found.sort_by(|a, b| a.folder.cmp(&b.folder));
    found
}

/// Whether `folder` holds Crystalline state: a `config.yaml` or an
/// `index.db`. An empty folder an old extension left behind holds none.
fn holds_state(folder: &Path) -> bool {
    ["config.yaml", "index.db"]
        .iter()
        .any(|name| folder.join(name).is_file())
}

/// [`scan`] of this machine's `LOCALAPPDATA`.
pub fn scan_here() -> Vec<DesktopState> {
    scan(
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .as_deref(),
    )
}

fn describe(folder: &Path) -> DesktopState {
    let domains = crystalline_core::config::load_yaml::<crystalline_core::config::GlobalConfig>(
        &folder.join("config.yaml"),
    )
    .map(|cfg| cfg.domains.len())
    .unwrap_or(0);
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
        .and_then(|text| {
            serde_json::from_str::<crystalline_service::instance::LockInfo>(&text).ok()
        })
        .map(|record| record.pid)
        .filter(|pid| crystalline_service::instance::process_is_crystalline(*pid));
    DesktopState {
        folder: folder.to_path_buf(),
        domains,
        index_bytes,
        last_change,
        daemon_alive,
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1024 * 1024))
}

/// What one state says, for doctor and status alike.
pub fn describe_line(state: &DesktopState) -> String {
    let mut line = format!(
        "Claude Desktop kept its own Crystalline state in {}: {} domain{} in its config",
        state.folder.display(),
        state.domains,
        if state.domains == 1 { "" } else { "s" }
    );
    if let Some(bytes) = state.index_bytes {
        line.push_str(&format!(", an index of {}", megabytes(bytes)));
    }
    if let Some(when) = &state.last_change {
        line.push_str(&format!(", last changed {when}"));
    }
    if let Some(pid) = state.daemon_alive {
        line.push_str(&format!(
            ". A daemon from there still runs (pid {pid}), so two engines write the same knowledge folders"
        ));
    }
    line
}

/// The one line `crystalline status` prints, `None` without a split state.
pub fn status_line(states: &[DesktopState]) -> Option<String> {
    let first = states.first()?;
    Some(format!(
        "Desktop state: {} ({} domain{}{}). Run crystalline doctor for the details.",
        first.folder.display(),
        first.domains,
        if first.domains == 1 { "" } else { "s" },
        first
            .index_bytes
            .map(|b| format!(", {}", megabytes(b)))
            .unwrap_or_default()
    ))
}

/// What a merge did, for the doctor report.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MergeReport {
    /// The private folders this merge looked at. Empty when there was none.
    pub folders: Vec<PathBuf>,
    /// Names newly registered on the real side.
    pub registered: Vec<String>,
    /// Names registered on both sides the same way: left as they are.
    pub already_registered: Vec<String>,
    /// Virtual domains whose private engrams were moved, with how many.
    pub imported: Vec<(String, u64)>,
    /// Virtual domains on both sides, with the private engrams the real side
    /// already had and kept.
    pub kept_both: Vec<(String, u64)>,
    /// Names registered on both sides with two different meanings. Never
    /// overwritten; while any remains the folder is not renamed.
    pub conflicts: Vec<String>,
    /// Private files kept as they are and named for the person.
    pub private_kept: Vec<String>,
    /// Where the private folder went.
    pub renamed_to: Option<PathBuf>,
}

impl MergeReport {
    /// Whether the merge wrote anything on the real side or moved the folder.
    pub fn changed_something(&self) -> bool {
        !self.registered.is_empty()
            || self.imported.iter().any(|(_, n)| *n > 0)
            || self.renamed_to.is_some()
    }

    /// Fold the report of a further state into this one.
    pub fn absorb(&mut self, other: MergeReport) {
        self.folders.extend(other.folders);
        self.registered.extend(other.registered);
        self.already_registered.extend(other.already_registered);
        self.imported.extend(other.imported);
        self.kept_both.extend(other.kept_both);
        self.conflicts.extend(other.conflicts);
        for kept in other.private_kept {
            if !self.private_kept.contains(&kept) {
                self.private_kept.push(kept);
            }
        }
        if other.renamed_to.is_some() {
            self.renamed_to = other.renamed_to;
        }
    }
}

fn same_registration(real: &DomainEntry, private: &DomainEntry) -> bool {
    real.is_virtual() == private.is_virtual() && real.file_path() == private.file_path()
}

fn meaning(entry: &DomainEntry) -> String {
    match entry.file_path() {
        Some(root) if !entry.is_virtual() => format!("the folder {}", root.display()),
        _ => "a virtual domain".to_string(),
    }
}

/// `crystalline.merged-<day>` beside `folder`, with `-2`, `-3` and so on when
/// that name is taken.
pub(crate) fn merged_name(folder: &Path, today: chrono::NaiveDate) -> PathBuf {
    let base = format!("crystalline.merged-{}", today.format("%Y-%m-%d"));
    let parent = folder.parent().unwrap_or(folder);
    let mut candidate = parent.join(&base);
    let mut n = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{base}-{n}"));
        n += 1;
    }
    candidate
}

/// Copy the private index (and its write-ahead files) and its `config.yaml`
/// into `scratch`, so the export opens copies only: the original index is
/// never migrated and nothing in the private folder is opened for writing.
/// The folder stays what the old extension can still open.
fn copy_private_state(folder: &Path, scratch: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    for name in ["index.db", "index.db-wal", "index.db-shm", "config.yaml"] {
        let from = folder.join(name);
        if from.is_file() {
            std::fs::copy(&from, scratch.join(name))?;
        }
    }
    Ok((scratch.join("index.db"), scratch.join("config.yaml")))
}

/// Merge one private state into this machine's real one. Takes the union of
/// both domain lists, moves the engrams of the private virtual domains,
/// never overwrites what the real side has and renames the private folder
/// when no conflict is left. The private folder is only read.
pub async fn merge(
    state: &DesktopState,
    real_config: Option<&Path>,
    today: chrono::NaiveDate,
) -> anyhow::Result<MergeReport> {
    if let Some(pid) = state.daemon_alive {
        anyhow::bail!(
            "a Crystalline daemon from {} still runs (pid {pid}); quit Claude Desktop and run the merge again",
            state.folder.display()
        );
    }
    let private_config = state.folder.join("config.yaml");
    let private: GlobalConfig = if private_config.is_file() {
        crystalline_core::config::load_yaml(&private_config)?
    } else {
        GlobalConfig::default()
    };
    let loaded = crate::cmd::load(real_config)?;
    let mut file = loaded.file.clone();
    let mut report = MergeReport {
        folders: vec![state.folder.clone()],
        ..MergeReport::default()
    };
    let mut virtuals: Vec<(String, bool)> = Vec::new();

    for (name, entry) in &private.domains {
        match file.domains.get(name) {
            None if entry.is_virtual() => virtuals.push((name.clone(), false)),
            None => {
                file.domains.insert(name.clone(), entry.clone());
                report.registered.push(name.clone());
            }
            Some(real) if same_registration(real, entry) => {
                if entry.is_virtual() {
                    virtuals.push((name.clone(), true));
                } else {
                    report.already_registered.push(name.clone());
                }
            }
            Some(real) => report.conflicts.push(format!(
                "'{name}' is {} here and {} in Claude Desktop's state; rename one of them with crystalline domain rename, then merge again",
                meaning(real),
                meaning(entry)
            )),
        }
    }
    if !report.registered.is_empty() {
        crystalline_core::config::save_yaml(&loaded.path, &file)?;
    }

    if !virtuals.is_empty() {
        let scratch = tempfile::tempdir()?;
        let (db_copy, config_copy) = copy_private_state(&state.folder, scratch.path())?;
        for (name, on_both_sides) in virtuals {
            let exported = scratch.path().join("export").join(&name);
            // An export of a domain with no rows writes no folder, and the
            // import needs one.
            std::fs::create_dir_all(&exported)?;
            crystalline_service::domain_export(
                &name,
                &exported,
                true,
                false,
                Some(&db_copy),
                Some(&config_copy),
            )
            .await?;
            if !on_both_sides {
                crate::cmd::domain_add_register_virtual(&name, real_config)?;
                report.registered.push(name.clone());
            }
            // Never overwrite: on both sides the real engrams win, and a new
            // domain has nothing of its own yet.
            let imported = crystalline_service::domain_import(
                &name,
                &exported,
                false,
                false,
                None,
                real_config,
            )
            .await?;
            let written = imported["files_written"].as_u64().unwrap_or(0);
            if on_both_sides {
                if written > 0 {
                    report.imported.push((name.clone(), written));
                }
                report
                    .kept_both
                    .push((name, imported["files_skipped"].as_u64().unwrap_or(0)));
            } else {
                report.imported.push((name, written));
            }
        }
    }

    for kept in ["web-auth.db", "origins", "instance-id"] {
        if state.folder.join(kept).exists() {
            report.private_kept.push(kept.to_string());
        }
    }
    if report.conflicts.is_empty() {
        let target = merged_name(&state.folder, today);
        std::fs::rename(&state.folder, &target)?;
        report.renamed_to = Some(target);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_names_the_folder_and_what_is_in_it() {
        let state = DesktopState {
            folder: PathBuf::from(
                r"C:\u\AppData\Local\Packages\Claude_x\LocalCache\Roaming\crystalline",
            ),
            domains: 3,
            index_bytes: Some(12 * 1024 * 1024),
            last_change: Some("2026-10-06T12:00:00+00:00".to_string()),
            daemon_alive: None,
        };
        let line = status_line(&[state]).unwrap();
        assert!(line.starts_with("Desktop state: "), "{line}");
        assert!(line.contains("3 domains"), "{line}");
        assert!(line.contains("12 MB"), "{line}");
        assert!(line.contains("crystalline doctor"), "{line}");
        assert_eq!(status_line(&[]), None);
    }

    #[test]
    fn the_merged_folder_is_named_by_the_day_and_never_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("crystalline");
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
        assert_eq!(
            merged_name(&folder, day),
            tmp.path().join("crystalline.merged-2026-10-07")
        );
        std::fs::create_dir_all(tmp.path().join("crystalline.merged-2026-10-07")).unwrap();
        assert_eq!(
            merged_name(&folder, day),
            tmp.path().join("crystalline.merged-2026-10-07-2")
        );
    }

    /// Only a `Claude_*` package whose roaming cache holds a `crystalline`
    /// folder with a `config.yaml` or an `index.db` counts. An empty folder
    /// left behind holds no knowledge. The states come back sorted by folder.
    #[test]
    fn the_scan_finds_only_crystalline_state_in_claude_packages() {
        assert!(scan(None).is_empty());
        let local = tempfile::tempdir().unwrap();
        assert!(scan(Some(local.path())).is_empty(), "no Packages folder");
        let state = |package: &str| {
            local
                .path()
                .join("Packages")
                .join(package)
                .join("LocalCache")
                .join("Roaming")
                .join("crystalline")
        };
        for package in [
            "Claude_a",
            "Claude_b",
            "Claude_empty",
            "Claude_log",
            "Other_x",
        ] {
            std::fs::create_dir_all(state(package)).unwrap();
        }
        std::fs::write(state("Claude_b").join("config.yaml"), "domains: {}\n").unwrap();
        std::fs::write(state("Claude_a").join("index.db"), [0u8; 10]).unwrap();
        std::fs::write(state("Claude_a").join("index.db-wal"), [0u8; 5]).unwrap();
        std::fs::write(state("Claude_log").join("index.db-wal"), [0u8; 5]).unwrap();
        std::fs::write(state("Other_x").join("index.db"), [0u8; 5]).unwrap();
        std::fs::create_dir_all(local.path().join("Packages").join("Claude_c")).unwrap();
        let found = scan(Some(local.path()));
        let folders: Vec<&Path> = found.iter().map(|s| s.folder.as_path()).collect();
        assert_eq!(
            folders,
            [state("Claude_a").as_path(), state("Claude_b").as_path()],
            "an empty folder, a log alone and another package are not state"
        );
        assert_eq!(found[0].domains, 0, "no config.yaml");
        assert_eq!(found[0].index_bytes, Some(15), "the index and its log");
        assert!(found[0].last_change.is_some());
        assert_eq!(found[1].index_bytes, None);
        assert_eq!(found[1].daemon_alive, None);
    }
}
