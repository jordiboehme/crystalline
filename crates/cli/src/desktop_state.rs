//! The private state an old Claude Desktop extension left inside Claude
//! Desktop's package on Windows (`%LOCALAPPDATA%\Packages\Claude_*\
//! LocalCache\Roaming\crystalline`): found, described and, with
//! `doctor --fix --merge-desktop-state`, merged into the real one.
//!
//! The scan reads `LOCALAPPDATA` on every platform: Windows always sets it,
//! nothing else does unless a person or a test sets it.

use std::path::{Path, PathBuf};

use crystalline_core::config::registration::canonical_root;
use crystalline_core::config::{DomainEntry, GitHubConfig, GlobalConfig};
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
    /// Private registrations this machine's own registration stands in
    /// for, one sentence each: a name whose private folder cannot be used
    /// here while this machine registers the name, or a folder this machine
    /// registers under another name.
    pub kept_this_machine: Vec<String>,
    /// Virtual domains whose private engrams were moved, with how many.
    pub imported: Vec<(String, u64)>,
    /// Virtual domains on both sides, with how many private engrams differ
    /// from the copy this machine already had, which was kept. Engrams that
    /// are the same on both sides are not counted.
    pub kept_both: Vec<(String, u64)>,
    /// Names registered on both sides with two different meanings, and
    /// private names this machine cannot take. Never overwritten; while any
    /// remains the folder is not renamed.
    pub conflicts: Vec<String>,
    /// Private engrams the import could not move, one sentence per domain
    /// naming the files and why. While any remains the folder is not
    /// renamed.
    pub not_moved: Vec<String>,
    /// Team domains newly registered here whose team state
    /// (`origins/<name>`) was copied from the private folder. The private
    /// copy stays where it was.
    pub origins_copied: Vec<String>,
    /// Whether this machine has team domains turned on (`github.enabled`).
    /// A team domain that came over updates and shares only once it is.
    pub github_enabled: bool,
    /// The `github` settings copied from the private config, by name. Never
    /// a value.
    pub github_carried: Vec<String>,
    /// Private files kept as they are and named for the person.
    pub private_kept: Vec<String>,
    /// Where the private folder went.
    pub renamed_to: Option<PathBuf>,
}

impl MergeReport {
    /// Whether the merge wrote anything on the real side or moved the folder.
    pub fn changed_something(&self) -> bool {
        !self.registered.is_empty()
            || !self.origins_copied.is_empty()
            || !self.github_carried.is_empty()
            || self.imported.iter().any(|(_, n)| *n > 0)
            || self.renamed_to.is_some()
    }
}

/// `folder` with symlinks resolved, or as it is when it cannot be.
fn canonical(folder: &Path) -> PathBuf {
    std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf())
}

fn same_registration(real: &DomainEntry, private: &DomainEntry) -> bool {
    real.is_virtual() == private.is_virtual() && canonical_root(real) == canonical_root(private)
}

/// Copy each `github` key `private` sets and `real` does not into `real`,
/// never over a key `real` sets. Answers the dotted names of the keys it
/// copied, never a value.
pub(crate) fn carry_github(real: &mut GlobalConfig, private: &GlobalConfig) -> Vec<String> {
    let Some(from) = private.github.clone() else {
        return Vec::new();
    };
    // Every field by name, so a new github key cannot be left out.
    let GitHubConfig {
        enabled,
        poll_secs,
        stacks,
        share_identity,
        agent_identity,
        api_url,
        oauth_client_id,
    } = from;
    fn take<T>(to: &mut Option<T>, from: Option<T>, key: &str, carried: &mut Vec<String>) {
        if to.is_none()
            && let Some(value) = from
        {
            *to = Some(value);
            carried.push(key.to_string());
        }
    }
    let had_block = real.github.is_some();
    let to = real.github.get_or_insert_with(GitHubConfig::default);
    let mut carried = Vec::new();
    take(&mut to.enabled, enabled, "github.enabled", &mut carried);
    take(
        &mut to.poll_secs,
        poll_secs,
        "github.poll_secs",
        &mut carried,
    );
    take(&mut to.stacks, stacks, "github.stacks", &mut carried);
    take(
        &mut to.share_identity,
        share_identity,
        "github.share_identity",
        &mut carried,
    );
    take(
        &mut to.agent_identity,
        agent_identity,
        "github.agent_identity",
        &mut carried,
    );
    take(&mut to.api_url, api_url, "github.api_url", &mut carried);
    take(
        &mut to.oauth_client_id,
        oauth_client_id,
        "github.oauth_client_id",
        &mut carried,
    );
    if !had_block && carried.is_empty() {
        real.github = None;
    }
    carried
}

/// `path` as a person reads it: without the verbatim prefix a canonical
/// Windows path carries.
fn shown(path: &Path) -> std::path::Display<'_> {
    dunce::simplified(path).display()
}

fn meaning(entry: &DomainEntry) -> String {
    match entry.file_path() {
        Some(root) if !entry.is_virtual() => format!("the folder {}", shown(&root)),
        _ => "a virtual domain".to_string(),
    }
}

/// Why a private file domain's folder cannot be registered here as it
/// stands, `None` when it can.
enum Unusable {
    /// Inside the private state folder, at this path relative to it: it
    /// moves away with the rename.
    Inside(PathBuf),
    /// Not there at all (a path the package redirected, a drive that is
    /// gone), or no folder named: a broken registration.
    Missing(String),
}

impl Unusable {
    fn why(&self, folder: &Path) -> String {
        match self {
            Unusable::Inside(rel) => format!(
                "its folder {} is inside Claude Desktop's private folder",
                shown(&folder.join(rel))
            ),
            Unusable::Missing(why) => why.clone(),
        }
    }
}

fn unusable_root(entry: &DomainEntry, canonical_folder: &Path) -> Option<Unusable> {
    let Some(root) = canonical_root(entry) else {
        return Some(Unusable::Missing("it names no folder".to_string()));
    };
    if let Ok(rel) = root.strip_prefix(canonical_folder) {
        return Some(Unusable::Inside(rel.to_path_buf()));
    }
    (!root.is_dir())
        .then(|| Unusable::Missing(format!("its folder {} does not exist here", shown(&root))))
}

/// Why this machine cannot take a private name it does not register, the
/// checks `domain add` makes: a name an environment variable defines here,
/// or a spelling (the name, or the canonical name its MANIFEST declares)
/// that already means another domain here.
fn name_taken_here(
    name: &str,
    entry: &DomainEntry,
    loaded: &crystalline_service::LoadedConfig,
    table: &crystalline_core::names::NameTable,
) -> Option<String> {
    if let Some(env) = loaded.overlay.env_domain(name) {
        return Some(format!(
            "'{name}' is defined here by the environment variable {}; unset it or rename one of them, then merge again",
            env.var
        ));
    }
    let declared = entry
        .file_path()
        .filter(|_| !entry.is_virtual())
        .and_then(|root| crystalline_core::domain_name_at(&root));
    for spelling in std::iter::once(name.to_string()).chain(declared) {
        if let Some(other) = table.resolve(&spelling) {
            return Some(format!(
                "'{name}' in Claude Desktop's state clashes with this machine's domain '{other}', which already answers to '{spelling}'; rename one of them with crystalline domain rename, then merge again"
            ));
        }
    }
    None
}

/// How many of `collisions` (paths the import skipped) hold text that
/// differs between the private export and this machine's copy. Exported from
/// this machine only when there is a collision to compare.
async fn differing(
    name: &str,
    collisions: &[String],
    private_dir: &Path,
    scratch: &Path,
    real_config: Option<&Path>,
) -> anyhow::Result<u64> {
    if collisions.is_empty() {
        return Ok(0);
    }
    let real_dir = scratch.join("real").join(name);
    std::fs::create_dir_all(&real_dir)?;
    crystalline_service::domain_export(name, &real_dir, true, false, None, real_config).await?;
    Ok(collisions
        .iter()
        .filter(|rel| {
            std::fs::read(private_dir.join(rel)).ok() != std::fs::read(real_dir.join(rel)).ok()
        })
        .count() as u64)
}

/// Index a file domain the merge just registered: over the daemon when one
/// runs, else directly, the two routes `domain add` takes.
async fn sync_registered(
    name: &str,
    root: &Path,
    real_config: Option<&Path>,
) -> anyhow::Result<()> {
    if crystalline_service::use_daemon(None, real_config)
        && crystalline_service::ctl_if_running(
            serde_json::json!({ "v": 1, "cmd": "sync", "domain": name, "embed": false }),
        )
        .await?
        .is_some()
    {
        return Ok(());
    }
    crate::cmd::sync_domain_direct(name, root, real_config, None).await?;
    Ok(())
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

/// Copy the folder `from` to `to`, which must not exist yet. Only files
/// and folders: anything else fails the copy rather than being left out.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        } else {
            return Err(std::io::Error::other(format!(
                "{} is neither a file nor a folder",
                shown(&entry.path())
            )));
        }
    }
    Ok(())
}

/// Bring the team state of the private team domain `name` to this machine:
/// copy `<folder>/origins/<name>` into `origins`, never over a state this
/// machine already has. The copy goes to a staging folder first and is
/// renamed into place, so a failed copy leaves nothing half written. `Ok`
/// carries the new folder, `None` when the private side has no team state
/// for the name. `Err` is the sentence that says why the domain stays out.
fn carry_team_state(
    name: &str,
    folder: &Path,
    origins: Result<PathBuf, String>,
) -> Result<Option<PathBuf>, String> {
    let from = folder.join("origins").join(name);
    if !from.is_dir() {
        return Ok(None);
    }
    let origins = origins.map_err(|e| {
        format!(
            "'{name}' in Claude Desktop's state is a team domain, and its team state could not be copied here ({e}), so it was not registered. Fix that, then merge again"
        )
    })?;
    let to = origins.join(name);
    let taken = || {
        format!(
            "'{name}' in Claude Desktop's state is a team domain, but this machine already has team state under that name in {}, and the merge never overwrites it. Move that folder away if no domain here uses it, then merge again",
            shown(&to)
        )
    };
    if to.exists() {
        return Err(taken());
    }
    let staging = origins.join(format!(".{name}.merging-{}", std::process::id()));
    let copied = std::fs::create_dir_all(&origins)
        .and_then(|()| copy_tree(&from, &staging))
        .and_then(|()| {
            if to.exists() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    taken(),
                ));
            }
            std::fs::rename(&staging, &to)
        });
    match copied {
        Ok(()) => Ok(Some(to)),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            if e.kind() == std::io::ErrorKind::AlreadyExists && to.exists() {
                return Err(taken());
            }
            Err(format!(
                "'{name}' in Claude Desktop's state is a team domain, and its team state could not be copied to {} ({e}), so it was not registered. Fix that, then merge again",
                shown(&to)
            ))
        }
    }
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

/// Merge one private state into this machine's real one, adding what it did
/// to `report`. Takes the union of both domain lists, moves the engrams of
/// the private virtual domains, never overwrites what the real side has and
/// renames the private folder when no conflict is left. The private folder
/// is only read. On an error, `report` still holds what was done before it,
/// so a doctor run never hides a registration it already saved.
pub async fn merge_into(
    state: &DesktopState,
    real_config: Option<&Path>,
    today: chrono::NaiveDate,
    report: &mut MergeReport,
) -> anyhow::Result<()> {
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
    let table = crate::cmd::build_name_table(&loaded);
    report.github_enabled = loaded.effective.github_enabled();
    let mut file = loaded.file.clone();
    report.folders.push(state.folder.clone());
    let problems_before = report.conflicts.len() + report.not_moved.len();
    let canonical_folder = canonical(&state.folder);
    let mut registered = Vec::new();
    // Team state copied for `registered`, removed again if the config
    // cannot be saved, so a later merge does not find it in the way.
    let mut carried: Vec<(String, PathBuf)> = Vec::new();
    let mut virtuals: Vec<(String, bool)> = Vec::new();
    // Names whose files sit inside the private folder while this machine
    // registers the name: said once the rename has decided where they are.
    let mut inside: Vec<(String, String, PathBuf)> = Vec::new();

    for (name, entry) in &private.domains {
        let unusable = (!entry.is_virtual())
            .then(|| unusable_root(entry, &canonical_folder))
            .flatten();
        let real = file.domains.get(name);
        // The folder first: one this machine registers under another name is
        // already here, and its MANIFEST usually declares that other name,
        // which the name check below would read as a clash.
        if real.is_none()
            && !entry.is_virtual()
            && let Some(other) = canonical_root(entry)
                .and_then(|root| crate::cmd::existing_file_domain_at(&root, &file))
        {
            report.kept_this_machine.push(format!(
                "'{name}' in Claude Desktop's state is {}, which this machine registers as '{other}': kept '{other}' and skipped '{name}'",
                meaning(entry)
            ));
            continue;
        }
        if real.is_none()
            && let Some(problem) = name_taken_here(name, entry, &loaded, &table)
        {
            report.conflicts.push(problem);
            continue;
        }
        match (real, unusable) {
            (None, _) if entry.is_virtual() => virtuals.push((name.clone(), false)),
            (None, Some(unusable)) => report.conflicts.push(format!(
                "'{name}' in Claude Desktop's state cannot be registered here: {}. Move its files to a folder of your own, register it with crystalline domain add, then merge again",
                unusable.why(&state.folder)
            )),
            (None, None) => {
                // A team domain is of no use here without its team state:
                // copied first, and the domain stays out when it cannot be.
                if entry.origin.is_some() {
                    let origins =
                        crystalline_core::config::origins_state_dir().map_err(|e| e.to_string());
                    match carry_team_state(name, &state.folder, origins) {
                        Ok(Some(to)) => carried.push((name.clone(), to)),
                        Ok(None) => {}
                        Err(why) => {
                            report.conflicts.push(why);
                            continue;
                        }
                    }
                }
                file.domains.insert(name.clone(), entry.clone());
                registered.push(name.clone());
            }
            (Some(real), _) if same_registration(real, entry) => {
                if entry.is_virtual() {
                    virtuals.push((name.clone(), true));
                } else {
                    report.already_registered.push(name.clone());
                }
            }
            (Some(real), Some(Unusable::Inside(rel))) => {
                inside.push((name.clone(), meaning(real), rel));
            }
            (Some(real), Some(missing)) => report.kept_this_machine.push(format!(
                "'{name}' in Claude Desktop's state cannot be used here ({}): kept {} as this machine registers it",
                missing.why(&state.folder),
                meaning(real)
            )),
            (Some(real), None) => report.conflicts.push(format!(
                "'{name}' is {} here and {} in Claude Desktop's state; rename one of them with crystalline domain rename, then merge again",
                meaning(real),
                meaning(entry)
            )),
        }
    }
    // The github settings only the old extension's config had: copied key
    // by key, never over a key this machine sets. Saved with the
    // registrations, before any virtual domain is registered below.
    let github_carried = carry_github(&mut file, &private);
    if !registered.is_empty() || !github_carried.is_empty() {
        if let Err(e) = crystalline_core::config::save_yaml(&loaded.path, &file) {
            for (_, to) in &carried {
                let _ = std::fs::remove_dir_all(to);
            }
            return Err(e.into());
        }
        report.registered.extend(registered.iter().cloned());
        report
            .origins_copied
            .extend(carried.into_iter().map(|(name, _)| name));
        report.github_carried.extend(github_carried);
        // A carried github.enabled turns team domains on here, unless the
        // environment says otherwise: asked again, so the report does not
        // tell the person to turn on what is now on.
        report.github_enabled = loaded.overlay.apply(&file).github_enabled();
        // Indexed as `domain add` indexes a new folder, so the merged
        // domains answer at once.
        for name in &registered {
            if let Some(root) = file.domains[name].file_path() {
                sync_registered(name, &root, real_config).await?;
            }
        }
    }

    if !virtuals.is_empty() {
        let scratch = tempfile::tempdir()?;
        let (db_copy, config_copy) = copy_private_state(&state.folder, scratch.path())?;
        for (name, on_both_sides) in virtuals {
            let exported = scratch.path().join("export").join(&name);
            // An export of a domain with no rows writes no folder, and the
            // import needs one.
            std::fs::create_dir_all(&exported)?;
            crystalline_service::domain_export_from_copy(&name, &exported, &db_copy, &config_copy)
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
            let strings = |key: &str| -> Vec<String> {
                imported[key]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let written = imported["files_written"].as_u64().unwrap_or(0);
            if written > 0 || !on_both_sides {
                report.imported.push((name.clone(), written));
            }
            let warnings = strings("warnings");
            if !warnings.is_empty() {
                let engrams = if warnings.len() == 1 {
                    "1 engram".to_string()
                } else {
                    format!("{} engrams", warnings.len())
                };
                report.not_moved.push(format!(
                    "'{name}': {engrams} could not be moved: {}. The folder {} stays, so nothing is lost. Open the engrams named here, fix or remove them in Claude Desktop's state, then run the merge again",
                    warnings.join("; "),
                    shown(&state.folder)
                ));
            }
            let kept = differing(
                &name,
                &strings("collisions"),
                &exported,
                scratch.path(),
                real_config,
            )
            .await?;
            if kept > 0 {
                report.kept_both.push((name, kept));
            }
        }
    }

    for kept in ["web-auth.db", "origins", "instance-id"] {
        if state.folder.join(kept).exists() && !report.private_kept.iter().any(|k| k == kept) {
            report.private_kept.push(kept.to_string());
        }
    }
    let now_at = if report.conflicts.len() + report.not_moved.len() == problems_before {
        let target = merged_name(&state.folder, today);
        std::fs::rename(&state.folder, &target)?;
        report.renamed_to = Some(target.clone());
        format!("are now in {}", shown(&target))
    } else {
        format!("stay in {}", shown(&state.folder))
    };
    for (name, real, rel) in inside {
        let place = if rel.as_os_str().is_empty() {
            now_at.clone()
        } else {
            format!("{now_at} under {}", rel.display())
        };
        report.kept_this_machine.push(format!(
            "'{name}' in Claude Desktop's state kept its files inside Claude Desktop's private folder; they {place}. Kept {real} as this machine registers it"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each key the old extension's config set and this machine's does not
    /// comes over; a key set here never changes. The report gets names that
    /// are settings keys, never a value.
    #[test]
    fn the_github_block_is_carried_key_by_key_and_never_over_a_key_set_here() {
        use crystalline_core::config::GitHubConfig;
        let private = GlobalConfig {
            github: Some(GitHubConfig {
                enabled: Some(true),
                poll_secs: Some(120),
                stacks: Some(false),
                share_identity: Some("personal".to_string()),
                agent_identity: Some("share-bot".to_string()),
                api_url: Some("https://ghe.example.com/api/v3".to_string()),
                oauth_client_id: Some("Iv1.0123456789abcdef".to_string()),
            }),
            ..GlobalConfig::default()
        };
        let mut real = GlobalConfig {
            github: Some(GitHubConfig {
                enabled: Some(false),
                poll_secs: Some(600),
                ..GitHubConfig::default()
            }),
            ..GlobalConfig::default()
        };
        let carried = carry_github(&mut real, &private);
        assert_eq!(
            carried,
            [
                "github.stacks",
                "github.share_identity",
                "github.agent_identity",
                "github.api_url",
                "github.oauth_client_id"
            ]
        );
        let github = real.github.clone().unwrap();
        assert_eq!(
            github.enabled,
            Some(false),
            "a key set here is never overwritten"
        );
        assert_eq!(github.poll_secs, Some(600));
        assert_eq!(
            github.api_url.as_deref(),
            Some("https://ghe.example.com/api/v3")
        );
        assert_eq!(
            github.oauth_client_id.as_deref(),
            Some("Iv1.0123456789abcdef")
        );

        let mut fresh = GlobalConfig::default();
        let all = carry_github(&mut fresh, &private);
        assert_eq!(all.len(), 7, "every key of the block: {all:?}");
        for key in &all {
            assert!(
                crystalline_service::settings::registry()
                    .iter()
                    .any(|s| s.key == key),
                "{key} is a settings key"
            );
        }
        assert_eq!(fresh.github, private.github);

        let mut untouched = GlobalConfig::default();
        assert!(carry_github(&mut untouched, &GlobalConfig::default()).is_empty());
        assert!(
            untouched.github.is_none(),
            "no empty block is written for nothing"
        );
    }

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
