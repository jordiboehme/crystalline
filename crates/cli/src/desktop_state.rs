//! The private state an old Claude Desktop extension left inside Claude
//! Desktop's package on Windows (`%LOCALAPPDATA%\Packages\Claude_*\
//! LocalCache\Roaming\crystalline`): found, described and, with
//! `doctor --fix --merge-desktop-state`, merged into the real one.
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
    /// `index.db` plus its write-ahead log, when there is one.
    pub index_bytes: Option<u64>,
    /// The newest change to a file directly in the folder.
    pub last_change: Option<String>,
    /// The pid of a Crystalline daemon its record names that still runs.
    pub daemon_alive: Option<u32>,
}

/// Every private state folder under `local_appdata`, sorted by path.
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
        .filter(|folder| folder.is_dir())
        .map(|folder| describe(&folder))
        .collect();
    found.sort_by(|a, b| a.folder.cmp(&b.folder));
    found
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

    /// Only a `Claude_*` package with a `crystalline` folder in its roaming
    /// cache counts, and the states come back sorted by folder.
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
        std::fs::create_dir_all(state("Claude_b")).unwrap();
        std::fs::create_dir_all(state("Claude_a")).unwrap();
        std::fs::write(state("Claude_a").join("index.db-wal"), [0u8; 10]).unwrap();
        std::fs::create_dir_all(state("Other_x")).unwrap();
        std::fs::create_dir_all(local.path().join("Packages").join("Claude_c")).unwrap();
        let found = scan(Some(local.path()));
        let folders: Vec<&Path> = found.iter().map(|s| s.folder.as_path()).collect();
        assert_eq!(
            folders,
            [state("Claude_a").as_path(), state("Claude_b").as_path()]
        );
        assert_eq!(found[0].domains, 0, "no config.yaml");
        assert_eq!(found[0].index_bytes, Some(10), "the log alone counts");
        assert!(found[0].last_change.is_some());
        assert_eq!(found[1].index_bytes, None);
        assert_eq!(found[1].daemon_alive, None);
    }
}
