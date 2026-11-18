//! `crystalline update check`: the newest stable release, how this binary
//! was installed and what `update now` would do for it; `crystalline update
//! now`: the same plan, then the action for the install method.
//!
//! Nothing from the network is shown as it came (S23): the newest version
//! is read through [`release_version`] and rendered from the parsed
//! [`Precedence`], the notes URL is built from it, the method and the
//! action come from enums, and a failed check is said in a fixed sentence.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use crystalline_core::time::{self, Stamp};
use crystalline_core::update::{
    CheckOutcome, Checks, Detection, FAILED_REASON, Gate, InstallMethod, LatestRelease, Lookup,
    Refusal, UPDATE_CACHE_FILE, UpdateCache, age_gate, archive_member, asset_name, display_path,
};
use crystalline_core::version::{Precedence, release_tag, release_version};
use crystalline_service::update::swap::finish_after_swap;
use crystalline_service::update::{
    Handover, Spawner, SystemSpawner, UpdateError, UpdateLock, download_dir, extract_zip_member,
    fetch_verified, hand_over, install_tarball, latest_release, remove_older_tags,
    root_owned_install_dir, take_lock, under_sudo, writable_install_dir,
};
use serde::{Serialize, Serializer};

use crate::ExitWith;
use crate::update_windows::{self, DaemonStart};

/// The exit code of `update check` (and of a plan-only `update now`) when a
/// newer stable release is available and old enough to be offered.
pub(crate) const EXIT_UPDATE_AVAILABLE: u8 = 10;

/// The command `update now` runs, with consent, on a Homebrew install.
pub(crate) const BREW_UPGRADE: &str = "brew upgrade jordiboehme/tap/crystalline";

/// The page every release's notes are on, without the tag.
const RELEASES_URL: &str = "https://github.com/jordiboehme/crystalline/releases";

/// What `update check` found and what `update now` would do.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Plan {
    /// This binary's version.
    #[serde(serialize_with = "rendered")]
    pub(crate) current: Precedence,
    /// The newest stable release; `None` when the check named none.
    #[serde(serialize_with = "rendered_option")]
    pub(crate) newest: Option<Precedence>,
    /// When the newest release was published; `None` when only the
    /// fallback answered.
    #[serde(with = "time::serde::stamp_option")]
    pub(crate) published_at: Option<Stamp>,
    /// The release-age gate of the newest release.
    #[serde(serialize_with = "gate")]
    pub(crate) gate: Gate,
    /// How this binary was installed, as its stable key.
    pub(crate) method: InstallMethod,
    /// What `update now` would do.
    pub(crate) action: Action,
    /// The release notes of the newest release.
    pub(crate) notes_url: String,
    /// When the plan was made: the publish age is counted to it.
    #[serde(skip)]
    pub(crate) made_at: Stamp,
}

/// What `update now` would do, by install method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    /// Run the package manager's upgrade (Homebrew).
    Upgrade { command: String },
    /// Download, verify and replace the binary in place (tar.gz on Unix).
    Swap,
    /// Download and verify the .deb, then print the apt command.
    DownloadDeb,
    /// Download and verify the .msi, then print the steps.
    DownloadMsi,
    /// Download and verify the zip, then print the steps.
    DownloadZip,
    /// Nothing, for this reason.
    Refuse(Refusal),
    /// Nothing: no newer stable release.
    UpToDate,
}

/// The plan for `current` given the newest stable release the check found,
/// its publish time, its gate (pass [`Gate::Open`] for `--ignore-age`),
/// this binary's detection and the time the plan is made at (`now`). Only a
/// strictly newer version is an update; an older or equal one is
/// [`Action::UpToDate`]. Pure.
pub(crate) fn plan(
    current: Precedence,
    newest: Option<Precedence>,
    published_at: Option<Stamp>,
    gate: Gate,
    detection: Detection,
    now: Stamp,
) -> Plan {
    let newer = newest.as_ref().is_some_and(|found| *found > current);
    let action = if !newer {
        Action::UpToDate
    } else if let Some(refusal) = detection.refusal {
        Action::Refuse(refusal)
    } else {
        match detection.method {
            InstallMethod::Homebrew => Action::Upgrade {
                command: BREW_UPGRADE.to_string(),
            },
            InstallMethod::Tarball => Action::Swap,
            InstallMethod::Deb => Action::DownloadDeb,
            InstallMethod::Msi => Action::DownloadMsi,
            InstallMethod::Zip => Action::DownloadZip,
            InstallMethod::Docker => Action::Refuse(Refusal::Docker),
            InstallMethod::Mcpb => Action::Refuse(Refusal::Mcpb),
            InstallMethod::Unknown => Action::Refuse(Refusal::Unknown),
        }
    };
    let notes_url = match newest.as_ref().or(Some(&current)).and_then(release_tag) {
        Some(tag) => format!("{RELEASES_URL}/tag/{tag}"),
        None => RELEASES_URL.to_string(),
    };
    Plan {
        current,
        newest,
        published_at,
        gate,
        method: detection.method,
        action,
        notes_url,
        made_at: now,
    }
}

impl Plan {
    /// Whether a newer stable release is there and old enough to offer.
    pub(crate) fn update_available(&self) -> bool {
        self.newer() && self.gate == Gate::Open
    }

    /// `update check`'s exit code: [`EXIT_UPDATE_AVAILABLE`] when an update
    /// is available, else 0 (up to date, or too new to offer yet).
    pub(crate) fn exit_code(&self) -> u8 {
        if self.update_available() {
            EXIT_UPDATE_AVAILABLE
        } else {
            0
        }
    }

    fn newer(&self) -> bool {
        self.action != Action::UpToDate
    }
}

/// Why `update now` will not act, as one fixed sentence.
pub(crate) fn refusal_text(refusal: &Refusal) -> String {
    match refusal {
        Refusal::NotOfficial => "not an official release build".to_string(),
        Refusal::AppPackage => "inside an app package; run it in a normal terminal".to_string(),
        Refusal::MarkerPathMismatch { marker, path } => format!(
            "the install marker says {} but the path reads as {}",
            marker.label(),
            path.label()
        ),
        Refusal::Docker => "a Docker install; pull the new image".to_string(),
        Refusal::Mcpb => "the Claude Desktop extension; Claude Desktop updates it".to_string(),
        Refusal::Unknown => "the install method could not be told".to_string(),
    }
}

/// What `update now` would do, as the person reads it.
fn action_text(action: &Action) -> String {
    match action {
        Action::Upgrade { command } => format!("run {command} (asks first; --yes skips the question)"),
        Action::Swap => {
            "download and verify the release, replace this binary and hand over the daemon (asks first; --yes skips the question)".to_string()
        }
        Action::DownloadDeb => {
            "download and verify the .deb, then print the apt command that installs it".to_string()
        }
        Action::DownloadMsi => {
            "download and verify the .msi, then print the steps that install it".to_string()
        }
        Action::DownloadZip => {
            "download and verify the zip, then print the steps that replace crystalline.exe"
                .to_string()
        }
        Action::Refuse(refusal) => format!("does nothing ({})", refusal_text(refusal)),
        Action::UpToDate => "does nothing (this is the newest stable release)".to_string(),
    }
}

/// The plan as `update check` prints it.
pub(crate) fn render_plan(plan: &Plan) -> String {
    let current = plan.current.render();
    let mut out = String::new();
    match (&plan.newest, plan.newer(), plan.gate) {
        (Some(newest), true, Gate::Open) => out.push_str(&format!(
            "Crystalline {} is available (this is {current}).\n",
            newest.render()
        )),
        (Some(newest), true, Gate::OpensAt(at)) => out.push_str(&format!(
            "Crystalline {} is out (this is {current}), but too new to offer: it is offered from {}. --ignore-age offers it now.\n",
            newest.render(),
            at.rfc3339_secs_z()
        )),
        (Some(newest), true, Gate::Never) => out.push_str(&format!(
            "Crystalline {} is out (this is {current}), but the release-age gate never opens for it. --ignore-age offers it now.\n",
            newest.render()
        )),
        (Some(newest), _, _) => out.push_str(&format!(
            "Crystalline {current} is up to date (newest stable release: {}).\n",
            newest.render()
        )),
        (None, _, _) => out.push_str(&format!("Crystalline {current} is up to date.\n")),
    }
    if let Some(at) = plan.published_at {
        out.push_str(&format!(
            "Published: {} ({})\n",
            at.rfc3339_secs_z(),
            age_text(plan.made_at.since(at))
        ));
    }
    out.push_str(&format!("Install method: {}\n", plan.method.label()));
    out.push_str(&format!("update now: {}\n", action_text(&plan.action)));
    out.push_str(&format!("Release notes: {}\n", plan.notes_url));
    out
}

/// How long ago a release was published, as a person reads it: whole days
/// from one day on, else whole hours, else "less than an hour ago". A
/// publish time later than now (a wrong clock) is "in the future".
fn age_text(age: time::Delta) -> String {
    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    if age < time::Delta::ZERO {
        "in the future".to_string()
    } else if age.num_days() >= 1 {
        plural(age.num_days(), "day")
    } else if age.num_hours() >= 1 {
        plural(age.num_hours(), "hour")
    } else {
        "less than an hour ago".to_string()
    }
}

/// The line `update check` prints on stderr when another process holds the
/// check lock.
pub(crate) const WAITING_FOR_LOCK: &str = "Another update check is running; waiting for it. A lock left by a stopped process is taken over after two minutes.";

/// `crystalline update check`: look up the newest stable release now (the
/// passive check need not be on), through the same lock and cache the
/// daemon uses, and print the plan. Exit 10 when an update is available,
/// 0 when up to date or the release is too new to offer, 1 on an error.
pub(crate) async fn check(ignore_age: bool, json: bool) -> anyhow::Result<()> {
    let current = Precedence::parse(crystalline_core::VERSION)
        .context("this binary's version could not be read")?;
    // The effective settings, a Postgres configuration document included,
    // so the gate is the one the daemon, the session block and Fluid use.
    let min_age_hours = crystalline_service::config_for_report(None)
        .await?
        .effective
        .update_min_age_hours();
    let state = crystalline_core::config::state_dir()
        .context("no state folder to keep the update check in")?;
    let now = time::now();
    let cache = crystalline_service::update::check_now(&state.join(UPDATE_CACHE_FILE), now, || {
        eprintln!("{WAITING_FOR_LOCK}")
    })
    .await;
    let found = found_release(&cache, now, min_age_hours, ignore_age)?;
    let detection = crystalline_service::update::detect_here().detection;
    let plan = plan(current, Some(found.0), found.1, found.2, detection, now);
    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        print!("{}", render_plan(&plan));
    }
    match plan.exit_code() {
        0 => Ok(()),
        code => Err(ExitWith(code).into()),
    }
}

/// The newest release in `cache`, its publish time and its gate (open with
/// `ignore_age`), or the error a failed check is said with.
fn found_release(
    cache: &UpdateCache,
    now: Stamp,
    min_age_hours: u32,
    ignore_age: bool,
) -> anyhow::Result<(Precedence, Option<Stamp>, Gate)> {
    let (version, published_at, first_seen) = match &cache.outcome {
        CheckOutcome::Release {
            version,
            published_at,
            first_seen,
        } => (version, *published_at, *first_seen),
        CheckOutcome::Failed { reason } => anyhow::bail!(failure_line(reason)),
    };
    let Some(found) = release_version(version) else {
        anyhow::bail!("update check failed: {FAILED_REASON}");
    };
    // Counted from the publish time, else from when this machine first saw
    // the release, as `UpdateCache::gate` does.
    let gate = if ignore_age {
        Gate::Open
    } else {
        age_gate(published_at.unwrap_or(first_seen), now, min_age_hours)
    };
    Ok((found, published_at, gate))
}

/// The error line of a failed check. A connection failure's cause (proxy,
/// name lookup, TLS, timeout) is logged at info level and never cached
/// (S23), so the line says how to see it.
fn failure_line(reason: &str) -> String {
    let reason = plain_reason(reason);
    if reason == CANNOT_REACH {
        format!("update check failed: {reason} (RUST_LOG=info shows why)")
    } else {
        format!("update check failed: {reason}")
    }
}

/// The sentence the check caches when GitHub could not be reached.
const CANNOT_REACH: &str = "cannot reach GitHub";

/// A failure sentence as the cache stores it, when it is one of the plain
/// sentences the check writes (letters, digits, spaces and `'`); anything
/// else, such as a hand-edited file, is said as [`FAILED_REASON`].
fn plain_reason(reason: &str) -> &str {
    let plain = !reason.is_empty()
        && reason.len() <= 80
        && reason
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '\'');
    if plain { reason } else { FAILED_REASON }
}

/// Whether `update now` needs a yes before it acts on `method` (S20): only
/// the methods that change installed files, the tar.gz swap and `brew
/// upgrade`. The .deb, the MSI and the zip are only downloaded and
/// verified, which changes nothing installed, so an agent's shell, which is
/// no terminal, may do it without `--yes`.
pub(crate) fn needs_consent(method: InstallMethod) -> bool {
    matches!(method, InstallMethod::Tarball | InstallMethod::Homebrew)
}

/// How `update now` goes on once an update is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Consent {
    /// Act: `--yes`, or a method that needs no yes.
    Go,
    /// Ask at the terminal.
    Ask,
    /// No terminal to ask at: print the plan and exit 10 (U10).
    PlanOnly,
}

/// The consent rule (S20) for `method`, given `--yes` and whether a person
/// at a terminal can be asked.
pub(crate) fn consent(method: InstallMethod, yes: bool, terminal: bool) -> Consent {
    if yes || !needs_consent(method) {
        Consent::Go
    } else if terminal {
        Consent::Ask
    } else {
        Consent::PlanOnly
    }
}

/// Why `update now` will not act on `detection`, or `None` when it will. A
/// method without an action counts as unknown even without a refusal.
fn refusal_of(detection: &Detection) -> Option<Refusal> {
    match detection.refusal {
        Some(refusal) => Some(refusal),
        None if !detection.method.acts() => Some(Refusal::Unknown),
        None => None,
    }
}

/// The error line of a refused `update now`.
fn refused_line(refusal: &Refusal) -> String {
    format!("update now does nothing: {}", refusal_text(refusal))
}

/// The line `update now` prints when it stops before acting.
const NOTHING_CHANGED: &str = "Nothing was changed.";

/// The line a gated `update now` adds when the release has no publish time
/// and the check never saw it.
pub(crate) const AGE_FROM_NOW: &str = "GitHub's API did not answer, so the release age counts from now. crystalline update check starts the count; --ignore-age skips the wait.";

/// The note under a gated plan: [`AGE_FROM_NOW`] when the gate was counted
/// from now. `update now` keeps no first sighting of its own, so without
/// one the wait would restart at every run.
pub(crate) fn gate_note(since: Option<Stamp>, gate: Gate) -> Option<&'static str> {
    (since.is_none() && matches!(gate, Gate::OpensAt(_))).then_some(AGE_FROM_NOW)
}

/// The line a plan-only `update now` ends with (U10).
const RUN_WITH_YES: &str = "Nothing was changed. Run crystalline update now --yes to update.";

/// What `brew upgrade` is said with when it left the version where it was
/// (S13).
pub(crate) const BREW_DID_NOT_UPGRADE: &str = "brew did not upgrade (tap not refreshed?)";

/// What one run of `update now` takes from outside, so a test can hand in
/// every part of it: the detection, the spawner and the euid are seams,
/// never read from the environment inside.
pub(crate) struct NowContext<'a> {
    /// This binary's version.
    pub(crate) current: Precedence,
    /// How this binary was installed.
    pub(crate) detection: Detection,
    /// The cache folder: the update lock and the download folders.
    pub(crate) cache_dir: PathBuf,
    /// The update check's cache, read for when this machine first saw a
    /// release the lookup found without a publish time.
    pub(crate) check_cache: Option<UpdateCache>,
    /// `update.min_age_hours` from the effective settings.
    pub(crate) min_age_hours: u32,
    /// Runs `brew` and the installed binary.
    pub(crate) spawner: &'a dyn Spawner,
    /// The `brew` to run ([`brew_program`]).
    pub(crate) brew: String,
    /// Whether a person at a terminal can be asked.
    pub(crate) terminal: bool,
    /// Asks the question and says whether the answer was yes.
    pub(crate) ask: &'a (dyn Fn(&str) -> bool + Sync),
    /// The effective user id on Unix, `None` elsewhere.
    pub(crate) euid: Option<u32>,
    /// Whether this run is under `sudo` ([`under_sudo`]): then `cache_dir`
    /// is a fresh folder of root's, and the daemon is never touched (S5).
    pub(crate) sudo: bool,
    /// The canonical exe the detection spoke for: the one file a tar.gz
    /// update replaces.
    pub(crate) exe: Option<PathBuf>,
    /// How the running daemon was started, for the MSI steps; `None` when
    /// no daemon runs.
    pub(crate) daemon: Option<DaemonStart>,
    /// `%ProgramFiles%`, which names the MSI's default folder.
    pub(crate) program_files: Option<String>,
    /// When the run started.
    pub(crate) now: Stamp,
}

/// `crystalline update now`: refuse before anything else when this install
/// cannot be updated, then take the update lock, look up the newest stable
/// release and act on it for the install method. Exit 0 when done or when
/// there is nothing to do (up to date, or too new to offer), 10 when an
/// update is available but nothing was done (no `--yes` and no terminal,
/// or a no at the question), 1 on a refusal or an error.
pub(crate) async fn now(yes: bool, ignore_age: bool) -> anyhow::Result<()> {
    let current = Precedence::parse(crystalline_core::VERSION)
        .context("this binary's version could not be read")?;
    let detected = crystalline_service::update::detect_here();
    let detection = detected.detection;
    // Every refusal comes before any request or file (S6).
    if let Some(refusal) = refusal_of(&detection) {
        anyhow::bail!(refused_line(&refusal));
    }
    let euid = effective_uid();
    let sudo = euid.is_some_and(|euid| under_sudo(euid, std::env::var("SUDO_UID").ok().as_deref()));
    let min_age_hours = crystalline_service::config_for_report(None)
        .await?
        .effective
        .update_min_age_hours();
    // Under sudo the person's cache folder is never written: the lock and
    // the download go to a fresh folder of root's (S5).
    let cache_dir = if sudo {
        tempfile::Builder::new()
            .prefix("crystalline-update-")
            .tempdir()
            .context("no temporary folder to download into")?
            .keep()
    } else {
        crystalline_core::config::cache_dir().context("no cache folder to download into")?
    };
    let check_cache = crystalline_core::config::state_dir()
        .ok()
        .and_then(|dir| crystalline_service::update::read_cache(&dir.join(UPDATE_CACHE_FILE)));
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let context = NowContext {
        current,
        detection,
        cache_dir,
        check_cache,
        min_age_hours,
        spawner: &SystemSpawner,
        brew: brew_program(detected.exe.as_deref()),
        terminal,
        ask: &ask_at_terminal,
        euid,
        sudo,
        exe: detected.exe,
        daemon: running_daemon(),
        program_files: std::env::var("ProgramFiles").ok(),
        now: time::now(),
    };
    let result = run_now(&context, yes, ignore_age).await;
    // The folder of a sudo run goes, but for a .deb whose apt command was
    // printed: that command points into it.
    if sudo && !keeps_sudo_folder(detection.method, &result, &context.cache_dir) {
        let _ = std::fs::remove_dir_all(&context.cache_dir);
    }
    match result? {
        0 => Ok(()),
        code => Err(ExitWith(code).into()),
    }
}

/// Whether the temp folder of a sudo run stays: only for a .deb that was
/// downloaded (the run ended with 0 and a release folder is there), whose
/// printed apt command points into it. A refused, failed or up-to-date run
/// leaves nothing behind.
pub(crate) fn keeps_sudo_folder(
    method: InstallMethod,
    result: &anyhow::Result<u8>,
    folder: &Path,
) -> bool {
    method == InstallMethod::Deb
        && matches!(result, Ok(0))
        && std::fs::read_dir(folder.join("updates")).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        })
}

/// Why a tar.gz install cannot be updated in place, before anything is
/// downloaded: its folder is not writable for this process. The text names
/// the folder and the manual steps, and, for a folder of root's, `sudo
/// crystalline update now`. `None` when the swap can go ahead.
pub(crate) fn tarball_refusal(exe: Option<&Path>, euid: Option<u32>) -> Option<String> {
    // No canonical exe: nothing the swap could act on.
    let Some(exe) = exe else {
        return Some(refused_line(&Refusal::Unknown));
    };
    if writable_install_dir(exe) {
        return None;
    }
    let dir = exe.parent().unwrap_or(exe);
    let by_hand = format!(
        "take crystalline from the tar.gz of the newest release ({RELEASES_URL}/latest) and move it over {}.",
        display_path(exe)
    );
    let next = if root_owned_install_dir(exe) && euid != Some(0) {
        format!("Run sudo crystalline update now, or update by hand: {by_hand}")
    } else {
        format!("Update by hand: {by_hand}")
    };
    Some(format!(
        "update now does nothing: you cannot write to {}. {next}",
        display_path(dir)
    ))
}

/// [`now`] once its parts are known: the lock, the lookup and the action.
/// Hands back the exit code.
pub(crate) async fn run_now(
    context: &NowContext<'_>,
    yes: bool,
    ignore_age: bool,
) -> anyhow::Result<u8> {
    if let Some(refusal) = refusal_of(&context.detection) {
        anyhow::bail!(refused_line(&refusal));
    }
    // On Unix a tar.gz folder that is not writable is refused before any
    // request or file too.
    if context.detection.method == InstallMethod::Tarball
        && let Some(refusal) = tarball_refusal(context.exe.as_deref(), context.euid)
    {
        anyhow::bail!(refusal);
    }
    let mut lock = Some(take_lock(&context.cache_dir)?);
    // A fresh lookup without the cached ETag: a "not modified" carries no
    // digests, and this one release names the tag of every download and
    // of the signature.
    let release = match latest_release(None).await? {
        Lookup::Fresh(release, _) => release,
        Lookup::NotModified => anyhow::bail!("GitHub's answer could not be read"),
    };
    remove_older_tags(&context.cache_dir, &release.tag);
    act(context, &release, yes, ignore_age, &mut lock).await
}

/// The plan for `release`, printed, then the action for the install
/// method. Hands back the exit code. `lock` is the update lock the run
/// holds; a tar.gz update lets go of it once the swap is done, before
/// `update finish` runs, which needs no lock.
pub(crate) async fn act(
    context: &NowContext<'_>,
    release: &LatestRelease,
    yes: bool,
    ignore_age: bool,
    lock: &mut Option<UpdateLock>,
) -> anyhow::Result<u8> {
    // Counted from the publish time, else from when this machine first saw
    // this very release (the check's cache), else from now.
    let first_seen = context
        .check_cache
        .as_ref()
        .and_then(UpdateCache::known_release)
        .filter(|known| known.version == release.tag)
        .map(|known| known.published_at.unwrap_or(known.first_seen));
    let since = release.published_at.or(first_seen);
    let gate = if ignore_age {
        Gate::Open
    } else {
        age_gate(
            since.unwrap_or(context.now),
            context.now,
            context.min_age_hours,
        )
    };
    let plan = plan(
        context.current.clone(),
        Some(release.version.clone()),
        release.published_at,
        gate,
        context.detection,
        context.now,
    );
    print!("{}", render_plan(&plan));
    if !plan.update_available() {
        if let Some(note) = gate_note(since, gate) {
            println!("{note}");
        }
        return Ok(0);
    }
    let method = context.detection.method;
    match consent(method, yes, context.terminal) {
        Consent::Go => {}
        Consent::Ask => {
            let question = format!("Update to {} now? [y/N] ", release.version.render());
            if !(context.ask)(&question) {
                println!("{NOTHING_CHANGED}");
                return Ok(EXIT_UPDATE_AVAILABLE);
            }
        }
        Consent::PlanOnly => {
            println!("{RUN_WITH_YES}");
            return Ok(EXIT_UPDATE_AVAILABLE);
        }
    }
    match method {
        InstallMethod::Homebrew => {
            println!(
                "{}",
                brew_upgrade(context.spawner, &context.brew, &context.current)?
            );
            Ok(0)
        }
        InstallMethod::Deb => {
            let asset = asset_name(method, &release.version).ok_or(UpdateError::NoAsset)?;
            let dir = download_dir(&context.cache_dir, &release.tag);
            let (path, checks) = fetch_verified(release, &asset, &dir).await?;
            println!("{}", checks_line(&checks, &release.tag));
            println!("Downloaded: {}", display_path(&path));
            print!("{}", apt_steps(&dir, &asset, context.euid, context.sudo));
            Ok(0)
        }
        InstallMethod::Msi | InstallMethod::Zip => {
            let asset = asset_name(method, &release.version).ok_or(UpdateError::NoAsset)?;
            let member = archive_member(&release.version).ok_or(UpdateError::NoAsset)?;
            let steps = windows_steps(context, release, &asset, &member).await?;
            print!("{steps}");
            Ok(0)
        }
        InstallMethod::Tarball => {
            let exe = context
                .exe
                .as_deref()
                .with_context(|| refused_line(&Refusal::Unknown))?;
            let asset = asset_name(method, &release.version).ok_or(UpdateError::NoAsset)?;
            let member = archive_member(&release.version).ok_or(UpdateError::NoAsset)?;
            let dir = download_dir(&context.cache_dir, &release.tag);
            let checks = install_tarball(
                context.spawner,
                release,
                &asset,
                &member,
                &dir,
                exe,
                cfg!(target_os = "macos"),
            )
            .await?;
            println!("{}", checks_line(&checks, &release.tag));
            println!(
                "Replaced {} with {}.",
                display_path(exe),
                release.version.render()
            );
            after_swap_unlocked(lock, context.spawner, exe, context.sudo)
        }
        InstallMethod::Docker | InstallMethod::Mcpb | InstallMethod::Unknown => {
            anyhow::bail!(refused_line(&Refusal::Unknown))
        }
    }
}

/// The MSI and zip branch of [`act`] (S22): remove a `crystalline.old.exe`
/// an earlier update left, download and verify the asset (and for the zip
/// extract `crystalline.exe` into the download folder), then build the
/// steps that install it, write them to `steps.txt` in the download folder
/// and hand them back for printing, followed by the line that names that
/// file. Nothing is run and the install folder is never written but for
/// the leftover.
pub(crate) async fn windows_steps(
    context: &NowContext<'_>,
    release: &LatestRelease,
    asset: &str,
    member: &str,
) -> anyhow::Result<String> {
    let exe = context
        .exe
        .as_deref()
        .with_context(|| refused_line(&Refusal::Unknown))?;
    let leftover = update_windows::remove_old_exe(exe);
    let dir = download_dir(&context.cache_dir, &release.tag);
    let (path, checks) = fetch_verified(release, asset, &dir).await?;
    println!("{}", checks_line(&checks, &release.tag));
    println!("Downloaded: {}", display_path(&path));
    let steps = if context.detection.method == InstallMethod::Zip {
        extract_zip_member(&path, member, &dir)?;
        let mut steps = String::new();
        if let Some(old) = &leftover {
            steps.push_str(&format!(
                "{} is still there and in use; delete it once nothing runs it, before step 2.\n",
                display_path(old)
            ));
        }
        steps.push_str(&update_windows::zip_steps(&dir, exe, &release.version));
        steps
    } else {
        // The install root is the parent of bin\ (S3).
        let root = exe
            .parent()
            .and_then(Path::parent)
            .with_context(|| refused_line(&Refusal::Unknown))?;
        update_windows::msi_steps(
            &dir,
            asset,
            root,
            context.daemon.as_ref(),
            update_windows::is_default_folder(root, context.program_files.as_deref()),
        )
    };
    let file = dir.join(update_windows::STEPS_FILE);
    std::fs::write(&file, update_windows::steps_file_bytes(&steps))
        .with_context(|| format!("cannot write {}", display_path(&file)))?;
    Ok(format!(
        "{steps}These steps are saved in {}.\n",
        display_path(&file)
    ))
}

/// How the daemon that runs now was started, from its record; `None` when
/// none runs.
fn running_daemon() -> Option<DaemonStart> {
    let info = crystalline_service::instance::read_lock_info()?;
    if !crystalline_service::instance::process_alive(info.pid) {
        return None;
    }
    update_windows::daemon_start(&info)
}

/// Run `brew upgrade` (S13), then read the version of the binary Homebrew
/// links now, `<brew --prefix>/bin/crystalline --version`, and say whether
/// it moved past `current`. A version that did not move is an error, since
/// `brew upgrade` can exit 0 without upgrading when the tap is stale. Only
/// the parsed version is shown, never what the binary printed (S23).
///
/// `brew` is the program [`brew_program`] found for this install.
pub(crate) fn brew_upgrade(
    spawner: &dyn Spawner,
    brew: &str,
    current: &Precedence,
) -> anyhow::Result<String> {
    println!("Running: {BREW_UPGRADE}");
    let args: Vec<&str> = BREW_UPGRADE.split(' ').skip(1).collect();
    let code = spawner.run(brew, &args).with_context(|| {
        if brew == "brew" {
            "brew could not be started (is it on PATH?)".to_string()
        } else {
            format!("{brew} could not be started")
        }
    })?;
    if code != 0 {
        anyhow::bail!("brew upgrade failed (exit code {code})");
    }
    let unreadable = "brew upgrade ran, but the installed version could not be read";
    let prefix = spawner.output(brew, &["--prefix"]).context(unreadable)?;
    let binary = Path::new(prefix.trim()).join("bin").join("crystalline");
    let printed = spawner
        .output(&binary.to_string_lossy(), &["--version"])
        .context(unreadable)?;
    let installed = printed
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("crystalline "))
        .and_then(|version| Precedence::parse(version.trim()))
        .filter(Precedence::is_stable)
        .context(unreadable)?;
    if installed > *current {
        Ok(format!("upgraded to {}", installed.render()))
    } else {
        anyhow::bail!(BREW_DID_NOT_UPGRADE)
    }
}

/// What a tar.gz update says under `sudo` instead of running `update
/// finish` (S5).
pub(crate) const SUDO_LEAVES_THE_DAEMON: &str =
    "Under sudo the daemon is not touched: run `crystalline status` as yourself.";

/// [`after_swap`] once the update lock is let go: the swap is done, so the
/// next run may start while `update finish` hands the daemon over.
pub(crate) fn after_swap_unlocked(
    lock: &mut Option<UpdateLock>,
    spawner: &dyn Spawner,
    exe: &Path,
    sudo: bool,
) -> anyhow::Result<u8> {
    drop(lock.take());
    after_swap(spawner, exe, sudo)
}

/// After the swap: under `sudo` only [`SUDO_LEAVES_THE_DAEMON`]; else run
/// `<exe> update finish`, which hands the daemon over (S4). Hands back the
/// exit code; a failed `update finish` is an error that says the binary is
/// in place.
pub(crate) fn after_swap(spawner: &dyn Spawner, exe: &Path, sudo: bool) -> anyhow::Result<u8> {
    if sudo {
        println!("{SUDO_LEAVES_THE_DAEMON}");
        return Ok(0);
    }
    println!("Running: {} update finish", display_path(exe));
    let code = finish_after_swap(spawner, exe)
        .context("the binary is updated, but update finish could not be started")?;
    if code != 0 {
        anyhow::bail!(
            "the binary is updated, but update finish failed (exit code {code}): run crystalline status to hand the daemon over"
        );
    }
    Ok(0)
}

/// `crystalline update finish`, run by the old binary as `<new> update
/// finish` right after a tar.gz swap (S4): hand the daemon over to this
/// binary through the existing displacement path, then refresh the
/// installed hooks and skills ([`crate::install::auto_reconcile`]). Under
/// `sudo` it does nothing but say so.
pub(crate) async fn finish() -> anyhow::Result<()> {
    let sudo = effective_uid()
        .is_some_and(|euid| under_sudo(euid, std::env::var("SUDO_UID").ok().as_deref()));
    if sudo {
        println!("{SUDO_LEAVES_THE_DAEMON}");
        return Ok(());
    }
    let handover = hand_over().await;
    let own = env!("CARGO_PKG_VERSION");
    let line = handover_line(&handover, own);
    let failed = matches!(
        handover,
        Handover::StillRunning { .. } | Handover::NoSuccessor { .. }
    );
    if !failed {
        println!("{line}");
    }
    if handover != Handover::Packaged {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        for notice in crate::install::auto_reconcile(own, &cwd) {
            println!("{notice}");
        }
        println!("{RECONCILE_SCOPE}");
    }
    if failed {
        anyhow::bail!(line);
    }
    Ok(())
}

/// What `update finish` says about the hooks and skills it refreshed.
pub(crate) const RECONCILE_SCOPE: &str = "Refreshed the installed hooks and skills for user scope and for this folder's project only; other projects refresh when a session starts in them.";

/// The line `update finish` prints for `handover`, this binary at `own`.
pub(crate) fn handover_line(handover: &Handover, own: &str) -> String {
    match handover {
        Handover::Packaged => "Inside an app package the daemon is never handed over: run `crystalline status` in a normal terminal.".to_string(),
        Handover::NoDaemon => format!("No daemon is running; the next one starts with {own}."),
        Handover::NotOlder { version } => format!("The daemon already runs {version}."),
        Handover::Kept { version, why } => format!(
            "The daemon ({version}) was left running, because it cannot be started again from here the way it ran: {why}."
        ),
        Handover::StillRunning { version } => format!(
            "the daemon ({version}) did not stop when asked and still runs: run crystalline status to try again"
        ),
        Handover::HandedOver { from } => format!("Handed the daemon over from {from} to {own}."),
        Handover::NoSuccessor { from } => format!(
            "the daemon ({from}) stopped, but the new one did not start: run crystalline status to start it"
        ),
    }
}

/// The `brew` of the Homebrew this binary was installed by: `<prefix>/bin/brew`
/// when the canonical exe lies under `<prefix>/Cellar/` and that file is
/// there, so an agent's shell or a service without Homebrew on its `PATH`
/// still finds it; else plain `brew` from `PATH`.
pub(crate) fn brew_program(exe: Option<&Path>) -> String {
    exe.into_iter()
        .flat_map(Path::ancestors)
        .find(|dir| dir.file_name().is_some_and(|name| name == "Cellar"))
        .and_then(Path::parent)
        .map(|prefix| prefix.join("bin").join("brew"))
        .filter(|brew| brew.is_file())
        .map_or_else(
            || "brew".to_string(),
            |brew| brew.to_string_lossy().into_owned(),
        )
}

/// The checks a download passed, in the order they ran, as one line (spec
/// "Verification", item 6).
pub(crate) fn checks_line(checks: &Checks, tag: &str) -> String {
    let mut ran = Vec::new();
    if checks.digest {
        ran.push("the sha256 digest GitHub lists".to_string());
    }
    if checks.signature {
        ran.push(format!("the minisign signature over SHA256SUMS for {tag}"));
    }
    if checks.sums {
        ran.push("the line in SHA256SUMS".to_string());
    }
    if checks.codesign {
        ran.push("the macOS code signature".to_string());
    }
    if checks.version {
        ran.push("the new binary's version".to_string());
    }
    let mut line = format!("Verified: {}.", ran.join(", "));
    if !checks.digest {
        line.push_str(" GitHub's API did not answer, so there was no digest to check.");
    }
    line
}

/// The apt steps for a verified .deb in `dir`: change into the folder, then
/// `sudo apt install ./<deb>`, without `sudo` when already root (`euid` 0).
/// Under `sudo` (`sudo_run`) the person's shell is not root and `dir` is a
/// folder of root's they cannot change into, so the line keeps `sudo` and
/// names the package by its full path. `update now` never runs them.
pub(crate) fn apt_steps(dir: &Path, deb: &str, euid: Option<u32>, sudo_run: bool) -> String {
    if sudo_run {
        return format!(
            "Install it with:\n  sudo apt install {}\n",
            shell_quoted(&display_path(&dir.join(deb)))
        );
    }
    let sudo = if euid == Some(0) { "" } else { "sudo " };
    format!(
        "Install it with:\n  cd {}\n  {sudo}apt install ./{deb}\n",
        shell_quoted(&display_path(dir))
    )
}

/// `text` as one word for a POSIX shell: as it is when it holds only
/// characters a shell takes literally, else in single quotes.
fn shell_quoted(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,~=".contains(c));
    if plain {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// The question at the terminal, on stderr; yes only for `y` or `yes`.
fn ask_at_terminal(question: &str) -> bool {
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// This process's effective user id on Unix.
#[cfg(unix)]
fn effective_uid() -> Option<u32> {
    // SAFETY: geteuid takes no arguments, cannot fail and touches no memory.
    Some(unsafe { libc::geteuid() })
}

/// No user id outside Unix.
#[cfg(not(unix))]
fn effective_uid() -> Option<u32> {
    None
}

fn rendered<S: Serializer>(version: &Precedence, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&version.render())
}

fn rendered_option<S: Serializer>(
    version: &Option<Precedence>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match version {
        Some(version) => serializer.serialize_some(&version.render()),
        None => serializer.serialize_none(),
    }
}

/// The gate as `"open"`, `"never"` or `{"opens_at": "<rfc3339>"}`.
fn gate<S: Serializer>(gate: &Gate, serializer: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "snake_case")]
    enum Wire {
        Open,
        OpensAt(#[serde(with = "time::serde::stamp")] Stamp),
        Never,
    }
    match gate {
        Gate::Open => Wire::Open,
        Gate::OpensAt(at) => Wire::OpensAt(*at),
        Gate::Never => Wire::Never,
    }
    .serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Precedence {
        Precedence::parse(text).unwrap()
    }

    fn acting(method: InstallMethod) -> Detection {
        Detection {
            method,
            refusal: None,
        }
    }

    fn at(day: u32) -> Stamp {
        Stamp::from_ymd_hms(2026, 11, day, 10, 0, 0).unwrap()
    }

    /// When the plans here are made: three hours after `at(17)`.
    fn now() -> Stamp {
        Stamp::from_ymd_hms(2026, 11, 17, 13, 0, 0).unwrap()
    }

    #[test]
    fn each_method_has_its_action() {
        for (method, action) in [
            (
                InstallMethod::Homebrew,
                Action::Upgrade {
                    command: "brew upgrade jordiboehme/tap/crystalline".to_string(),
                },
            ),
            (InstallMethod::Tarball, Action::Swap),
            (InstallMethod::Deb, Action::DownloadDeb),
            (InstallMethod::Msi, Action::DownloadMsi),
            (InstallMethod::Zip, Action::DownloadZip),
            (InstallMethod::Docker, Action::Refuse(Refusal::Docker)),
            (InstallMethod::Mcpb, Action::Refuse(Refusal::Mcpb)),
            (InstallMethod::Unknown, Action::Refuse(Refusal::Unknown)),
        ] {
            let plan = plan(
                v("0.33.0"),
                Some(v("0.34.0")),
                None,
                Gate::Open,
                acting(method),
                now(),
            );
            assert_eq!(plan.action, action, "{method:?}");
            assert_eq!(plan.method, method);
        }
    }

    #[test]
    fn a_refusal_wins_and_keeps_the_method() {
        for refusal in [
            Refusal::NotOfficial,
            Refusal::AppPackage,
            Refusal::MarkerPathMismatch {
                marker: InstallMethod::Deb,
                path: InstallMethod::Tarball,
            },
        ] {
            let detection = Detection {
                method: InstallMethod::Tarball,
                refusal: Some(refusal),
            };
            let plan = plan(
                v("0.33.0"),
                Some(v("0.34.0")),
                None,
                Gate::Open,
                detection,
                now(),
            );
            assert_eq!(plan.action, Action::Refuse(refusal));
            assert_eq!(plan.method, InstallMethod::Tarball);
            assert_eq!(plan.exit_code(), 10, "an update is still available");
        }
    }

    #[test]
    fn only_a_strictly_newer_release_is_an_update() {
        for newest in ["0.33.0", "0.32.9", "0.0.1"] {
            let plan = plan(
                v("0.33.0"),
                Some(v(newest)),
                None,
                Gate::Open,
                acting(InstallMethod::Tarball),
                now(),
            );
            assert_eq!(plan.action, Action::UpToDate, "{newest}");
            assert_eq!(plan.exit_code(), 0);
            assert!(render_plan(&plan).starts_with(&format!(
                "Crystalline 0.33.0 is up to date (newest stable release: {newest}).\n"
            )));
        }
    }

    #[test]
    fn a_gated_release_exits_0_and_says_when() {
        let plan = plan(
            v("0.33.0"),
            Some(v("0.34.0")),
            Some(at(17)),
            Gate::OpensAt(at(18)),
            acting(InstallMethod::Deb),
            now(),
        );
        assert_eq!(plan.exit_code(), 0);
        assert_eq!(
            render_plan(&plan),
            "Crystalline 0.34.0 is out (this is 0.33.0), but too new to offer: it is offered from 2026-11-18T10:00:00Z. --ignore-age offers it now.\n\
             Published: 2026-11-17T10:00:00Z (3 hours ago)\n\
             Install method: deb\n\
             update now: download and verify the .deb, then print the apt command that installs it\n\
             Release notes: https://github.com/jordiboehme/crystalline/releases/tag/v0.34.0\n"
        );
        let never = Plan {
            gate: Gate::Never,
            ..plan
        };
        assert_eq!(never.exit_code(), 0);
        assert!(
            render_plan(&never).contains("the release-age gate never opens for it"),
            "{}",
            render_plan(&never)
        );
    }

    #[test]
    fn an_available_update_renders_every_line() {
        let plan = plan(
            v("0.33.0"),
            Some(v("0.34.0")),
            None,
            Gate::Open,
            acting(InstallMethod::Homebrew),
            now(),
        );
        assert_eq!(plan.exit_code(), 10);
        assert_eq!(
            render_plan(&plan),
            "Crystalline 0.34.0 is available (this is 0.33.0).\n\
             Install method: homebrew\n\
             update now: run brew upgrade jordiboehme/tap/crystalline (asks first; --yes skips the question)\n\
             Release notes: https://github.com/jordiboehme/crystalline/releases/tag/v0.34.0\n"
        );
    }

    #[test]
    fn the_publish_age_reads_at_a_glance() {
        let hours = |n: i64| time::hours(n);
        assert_eq!(age_text(time::minutes(59)), "less than an hour ago");
        assert_eq!(age_text(hours(1)), "1 hour ago");
        assert_eq!(age_text(hours(23)), "23 hours ago");
        assert_eq!(age_text(hours(24)), "1 day ago");
        assert_eq!(age_text(hours(24 * 3 + 5)), "3 days ago");
        assert_eq!(age_text(hours(-2)), "in the future");
    }

    #[test]
    fn a_failure_line_says_how_to_see_a_connection_cause() {
        assert_eq!(
            failure_line("cannot reach GitHub"),
            "update check failed: cannot reach GitHub (RUST_LOG=info shows why)"
        );
        assert_eq!(
            failure_line("GitHub answered with status 502"),
            "update check failed: GitHub answered with status 502"
        );
        assert_eq!(failure_line("<b>"), "update check failed: the check failed");
    }

    #[test]
    fn refusals_have_fixed_sentences() {
        assert_eq!(
            refusal_text(&Refusal::NotOfficial),
            "not an official release build"
        );
        assert_eq!(
            refusal_text(&Refusal::MarkerPathMismatch {
                marker: InstallMethod::Deb,
                path: InstallMethod::Tarball
            }),
            "the install marker says deb but the path reads as tar.gz"
        );
        assert!(refusal_text(&Refusal::AppPackage).contains("run it in a normal terminal"));
    }

    #[test]
    fn the_json_shape_uses_stable_keys() {
        let plan = plan(
            v("0.33.0"),
            Some(v("0.34.0")),
            Some(at(17)),
            Gate::OpensAt(at(18)),
            Detection {
                method: InstallMethod::Tarball,
                refusal: Some(Refusal::NotOfficial),
            },
            now(),
        );
        assert_eq!(
            serde_json::to_value(&plan).unwrap(),
            serde_json::json!({
                "current": "0.33.0",
                "newest": "0.34.0",
                "published_at": "2026-11-17T10:00:00Z",
                "gate": {"opens_at": "2026-11-18T10:00:00Z"},
                "method": "tarball",
                "action": {"refuse": "not_official"},
                "notes_url": "https://github.com/jordiboehme/crystalline/releases/tag/v0.34.0"
            })
        );
        let open = plan_with(Gate::Open, acting(InstallMethod::Homebrew));
        let value = serde_json::to_value(&open).unwrap();
        assert_eq!(value["gate"], "open");
        assert_eq!(value["published_at"], serde_json::Value::Null);
        assert_eq!(
            value["action"],
            serde_json::json!({"upgrade": {"command": "brew upgrade jordiboehme/tap/crystalline"}})
        );
        let never = plan_with(Gate::Never, acting(InstallMethod::Deb));
        let value = serde_json::to_value(&never).unwrap();
        assert_eq!(value["gate"], "never");
        assert_eq!(value["action"], "download_deb");
    }

    fn plan_with(gate: Gate, detection: Detection) -> Plan {
        plan(v("0.33.0"), Some(v("0.34.0")), None, gate, detection, now())
    }

    #[test]
    fn a_failure_sentence_is_shown_only_when_plain() {
        assert_eq!(plain_reason("cannot reach GitHub"), "cannot reach GitHub");
        assert_eq!(
            plain_reason("GitHub's answer could not be read"),
            "GitHub's answer could not be read"
        );
        for odd in ["", "see https://evil.example", "<b>", "a\nb"] {
            assert_eq!(plain_reason(odd), FAILED_REASON, "{odd:?}");
        }
    }

    /// A spawner that runs nothing: it records each command and answers
    /// `run` with `code` and `output` from `outputs`, by program.
    struct Recording {
        code: i32,
        outputs: Vec<(&'static str, String)>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl Recording {
        fn new(code: i32, outputs: Vec<(&'static str, String)>) -> Recording {
            Recording {
                code,
                outputs,
                calls: std::sync::Mutex::default(),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, program: &str, args: &[&str]) {
            let mut call = vec![program];
            call.extend_from_slice(args);
            self.calls.lock().unwrap().push(call.join(" "));
        }
    }

    impl Spawner for Recording {
        fn run(&self, program: &str, args: &[&str]) -> std::io::Result<i32> {
            self.record(program, args);
            Ok(self.code)
        }

        fn output(&self, program: &str, args: &[&str]) -> std::io::Result<String> {
            self.record(program, args);
            self.outputs
                .iter()
                .find(|(name, _)| *name == program)
                .map(|(_, out)| out.clone())
                .ok_or_else(|| std::io::Error::other("no such program"))
        }
    }

    /// What `brew --prefix` and the linked binary print, the binary at
    /// `version`.
    fn brew_outputs(version: &str) -> Vec<(&'static str, String)> {
        vec![
            ("brew", "/opt/homebrew\n".to_string()),
            (
                "/opt/homebrew/bin/crystalline",
                format!("crystalline {version}\nCopyright (C) 2026 Jordi Böhme - MIT\n"),
            ),
        ]
    }

    const BREW_CALLS: [&str; 3] = [
        "brew upgrade jordiboehme/tap/crystalline",
        "brew --prefix",
        "/opt/homebrew/bin/crystalline --version",
    ];

    fn ctx<'a>(
        method: InstallMethod,
        spawner: &'a dyn Spawner,
        terminal: bool,
        ask: &'a (dyn Fn(&str) -> bool + Sync),
    ) -> NowContext<'a> {
        NowContext {
            current: v("0.33.0"),
            detection: acting(method),
            cache_dir: PathBuf::from("/nonexistent/cache"),
            check_cache: None,
            min_age_hours: 24,
            spawner,
            brew: "brew".to_string(),
            terminal,
            ask,
            euid: Some(1000),
            sudo: false,
            exe: None,
            daemon: None,
            program_files: None,
            now: now(),
        }
    }

    fn newer_release() -> LatestRelease {
        LatestRelease {
            tag: "v0.34.0".to_string(),
            version: v("0.34.0"),
            published_at: Some(at(10)),
            digests: Default::default(),
            from_api: true,
        }
    }

    /// A fake release download on 127.0.0.1 serving `files` under
    /// `/jordiboehme/crystalline/releases/download/<tag>/`; anything else
    /// is a 404. Plain HTTP/1.1, one request per connection.
    #[cfg(debug_assertions)]
    fn serve_release(tag: &str, files: Vec<(String, Vec<u8>)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let prefix = format!("/jordiboehme/crystalline/releases/download/{tag}/");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line == "\r\n" => break,
                        Ok(_) => {}
                    }
                }
                let path = request.split(' ').nth(1).unwrap_or_default();
                let found = path
                    .strip_prefix(&prefix)
                    .and_then(|name| files.iter().find(|(file, _)| file == name));
                let (status, body): (u16, &[u8]) = match found {
                    Some((_, body)) => (200, body),
                    None => (404, b""),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        base
    }

    /// Every file under `dir` with its bytes, by path.
    #[cfg(debug_assertions)]
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(at) = todo.pop() {
            for entry in std::fs::read_dir(&at).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    todo.push(path);
                } else {
                    files.push((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        files.sort();
        files
    }

    /// The download seam and the test key, set for one test at a time.
    #[cfg(debug_assertions)]
    static WINDOWS_SEAMS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// `update now`'s MSI or zip branch, [`windows_steps`], against a fake
    /// release of `asset` (signed with the test key) for an install of
    /// `method` at `<tmp>/Program Files/Crystalline/bin/crystalline.exe`.
    /// Hands back the install folder before and after, what was printed,
    /// and the download folder.
    #[cfg(debug_assertions)]
    async fn run_windows_branch(
        method: InstallMethod,
        asset: &str,
        bytes: Vec<u8>,
    ) -> (
        Vec<(PathBuf, Vec<u8>)>,
        Vec<(PathBuf, Vec<u8>)>,
        String,
        PathBuf,
        tempfile::TempDir,
    ) {
        use sha2::Digest;
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/update");
        let tag = "v0.34.0";
        let hex: String = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let sums = format!("{hex}  {asset}\n");
        let secret = std::fs::read_to_string(fixtures.join("test.key")).unwrap();
        let sk = minisign::SecretKeyBox::from_string(&secret)
            .unwrap()
            .into_unencrypted_secret_key()
            .unwrap();
        let sig = minisign::sign(
            None,
            &sk,
            sums.as_bytes(),
            Some(&format!("tag:{tag}")),
            None,
        )
        .unwrap()
        .into_string();
        let base = serve_release(
            tag,
            vec![
                (asset.to_string(), bytes),
                ("SHA256SUMS".to_string(), sums.into_bytes()),
                ("SHA256SUMS.minisig".to_string(), sig.into_bytes()),
            ],
        );

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Program Files").join("Crystalline");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("crystalline.exe");
        std::fs::write(&exe, b"the installed binary").unwrap();
        std::fs::write(root.join("LICENSE"), b"license").unwrap();
        if method == InstallMethod::Msi {
            std::fs::write(bin.join("install-source"), b"msi\n").unwrap();
        }
        let before = snapshot(&root);

        let spawner = Recording::new(0, Vec::new());
        let mut context = ctx(method, &spawner, false, &never_asked);
        context.cache_dir = tmp.path().join("cache");
        context.exe = Some(exe);
        context.daemon = Some(DaemonStart {
            by_task: true,
            serve_args: Vec::new(),
            env: Vec::new(),
        });
        let release = LatestRelease {
            from_api: false,
            ..newer_release()
        };
        let printed = {
            let _seams = WINDOWS_SEAMS.lock().await;
            // SAFETY: the seams are read only by the download and the key
            // lookup, which no other test of this binary runs, and the lock
            // keeps the tests that set them apart.
            unsafe {
                std::env::set_var("CRYSTALLINE_TEST_UPDATE_DOWNLOAD", &base);
                std::env::set_var("CRYSTALLINE_TEST_UPDATE_KEY", fixtures.join("test.pub"));
            }
            let printed = windows_steps(&context, &release, asset, "crystalline.exe").await;
            // SAFETY: as above.
            unsafe {
                std::env::remove_var("CRYSTALLINE_TEST_UPDATE_DOWNLOAD");
                std::env::remove_var("CRYSTALLINE_TEST_UPDATE_KEY");
            }
            printed.unwrap()
        };
        assert_eq!(spawner.calls(), Vec::<String>::new(), "nothing is run");
        let after = snapshot(&root);
        let dir = download_dir(&context.cache_dir, tag);
        (before, after, printed, dir, tmp)
    }

    /// A zip holding `crystalline.exe` with `body`.
    #[cfg(debug_assertions)]
    fn zip_with_exe(body: &[u8]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("crystalline.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(body).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn msi_and_zip_leave_the_install_folder_byte_identical() {
        let msi = "crystalline-v0.34.0-windows-amd64.msi";
        let (before, after, printed, dir, _tmp) =
            run_windows_branch(InstallMethod::Msi, msi, b"the msi".to_vec()).await;
        assert_eq!(
            before, after,
            "the MSI branch writes nothing in the install folder"
        );
        assert!(
            printed.contains("Start-Process msiexec.exe -Wait -PassThru"),
            "{printed}"
        );
        assert!(
            printed.contains("schtasks.exe /Run /TN '\\Crystalline\\Daemon'"),
            "{printed}"
        );
        assert_eq!(std::fs::read(dir.join(msi)).unwrap(), b"the msi");

        let zip = "crystalline-v0.34.0-windows-amd64.zip";
        let (before, after, printed, dir, _tmp) =
            run_windows_branch(InstallMethod::Zip, zip, zip_with_exe(b"the new exe")).await;
        assert_eq!(
            before, after,
            "the zip branch writes nothing in the install folder"
        );
        assert!(printed.contains("Rename-Item -LiteralPath"), "{printed}");
        assert_eq!(
            std::fs::read(dir.join("crystalline.exe")).unwrap(),
            b"the new exe",
            "the zip's exe is extracted into the download folder"
        );
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn steps_txt_matches_stdout() {
        for (method, asset, bytes) in [
            (
                InstallMethod::Msi,
                "crystalline-v0.34.0-windows-amd64.msi",
                b"the msi".to_vec(),
            ),
            (
                InstallMethod::Zip,
                "crystalline-v0.34.0-windows-amd64.zip",
                zip_with_exe(b"the new exe"),
            ),
        ] {
            let (_, _, printed, dir, _tmp) = run_windows_branch(method, asset, bytes).await;
            let file = dir.join("steps.txt");
            let saved = std::fs::read(&file).unwrap();
            let saved = saved
                .strip_prefix("\u{feff}".as_bytes())
                .expect("a byte order mark");
            let saved = std::str::from_utf8(saved).unwrap();
            assert_eq!(
                printed,
                format!("{saved}These steps are saved in {}.\n", display_path(&file)),
            );
            assert!(
                saved.starts_with("Install it with") || saved.starts_with("Replace"),
                "{saved}"
            );
        }
    }

    fn never_asked(_: &str) -> bool {
        panic!("nobody may be asked")
    }

    #[test]
    fn consent_rule_per_method() {
        // Download-only methods need no yes, with or without a terminal.
        for method in [InstallMethod::Deb, InstallMethod::Msi, InstallMethod::Zip] {
            assert!(!needs_consent(method), "{method:?}");
            for terminal in [false, true] {
                assert_eq!(consent(method, false, terminal), Consent::Go, "{method:?}");
            }
        }
        // Methods that change installed files: --yes, else a question at a
        // terminal, else only the plan.
        for method in [InstallMethod::Tarball, InstallMethod::Homebrew] {
            assert!(needs_consent(method), "{method:?}");
            assert_eq!(consent(method, true, false), Consent::Go);
            assert_eq!(consent(method, false, true), Consent::Ask);
            assert_eq!(consent(method, false, false), Consent::PlanOnly);
        }
    }

    #[tokio::test]
    async fn without_a_terminal_tarball_and_homebrew_print_the_plan_and_exit_10() {
        for method in [InstallMethod::Tarball, InstallMethod::Homebrew] {
            let spawner = Recording::new(0, Vec::new());
            let context = ctx(method, &spawner, false, &never_asked);
            let code = act(&context, &newer_release(), false, false, &mut None)
                .await
                .unwrap();
            assert_eq!(code, EXIT_UPDATE_AVAILABLE, "{method:?}");
            assert!(spawner.calls().is_empty(), "{method:?}: nothing ran");
        }
    }

    /// Download-only methods go on to the download without a terminal and
    /// without `--yes`. The cache folder is a file, so the download fails
    /// before any request (and on a host without that asset, `NoAsset`).
    #[tokio::test]
    async fn without_a_terminal_download_only_methods_go_on_to_the_download() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("not-a-folder");
        std::fs::write(&file, b"").unwrap();
        for method in [InstallMethod::Deb, InstallMethod::Msi, InstallMethod::Zip] {
            let spawner = Recording::new(0, Vec::new());
            let context = NowContext {
                cache_dir: file.clone(),
                ..ctx(method, &spawner, false, &never_asked)
            };
            let result = act(&context, &newer_release(), false, false, &mut None).await;
            assert!(
                result.is_err(),
                "{method:?}: went past the plan: {result:?}"
            );
            assert!(spawner.calls().is_empty(), "{method:?}");
        }
    }

    #[tokio::test]
    async fn a_no_at_the_terminal_does_nothing_and_exits_10() {
        let spawner = Recording::new(0, brew_outputs("0.34.0"));
        let asked = std::sync::Mutex::new(Vec::new());
        let ask = |question: &str| {
            asked.lock().unwrap().push(question.to_string());
            false
        };
        let context = ctx(InstallMethod::Homebrew, &spawner, true, &ask);
        let code = act(&context, &newer_release(), false, false, &mut None)
            .await
            .unwrap();
        assert_eq!(code, EXIT_UPDATE_AVAILABLE);
        assert_eq!(*asked.lock().unwrap(), ["Update to 0.34.0 now? [y/N] "]);
        assert!(spawner.calls().is_empty());

        // A yes runs brew.
        let ask = |_: &str| true;
        let context = NowContext {
            ask: &ask,
            ..context
        };
        let code = act(&context, &newer_release(), false, false, &mut None)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(spawner.calls(), BREW_CALLS);
    }

    #[tokio::test]
    async fn nothing_is_done_when_up_to_date_or_too_new() {
        let spawner = Recording::new(0, brew_outputs("0.34.0"));
        // Up to date.
        let context = NowContext {
            current: v("0.34.0"),
            ..ctx(InstallMethod::Homebrew, &spawner, false, &never_asked)
        };
        assert_eq!(
            act(&context, &newer_release(), true, false, &mut None)
                .await
                .unwrap(),
            0
        );
        // Published an hour before now: the 24 h gate is closed, and
        // --ignore-age opens it.
        let young = LatestRelease {
            published_at: now().checked_sub(time::hours(1)),
            ..newer_release()
        };
        let context = ctx(InstallMethod::Homebrew, &spawner, false, &never_asked);
        assert_eq!(
            act(&context, &young, true, false, &mut None).await.unwrap(),
            0
        );
        assert!(spawner.calls().is_empty(), "nothing ran");
        assert_eq!(
            act(&context, &young, true, true, &mut None).await.unwrap(),
            0
        );
        assert_eq!(spawner.calls(), BREW_CALLS, "--ignore-age acted");
    }

    #[tokio::test]
    async fn without_a_publish_time_the_gate_counts_from_first_seen() {
        let spawner = Recording::new(0, brew_outputs("0.34.0"));
        let fallback = LatestRelease {
            published_at: None,
            from_api: false,
            ..newer_release()
        };
        // Never seen before: counted from now, so the gate is closed.
        let context = ctx(InstallMethod::Homebrew, &spawner, false, &never_asked);
        assert_eq!(
            act(&context, &fallback, true, false, &mut None)
                .await
                .unwrap(),
            0
        );
        assert!(spawner.calls().is_empty());
        // Seen two days ago by the check: open.
        let seen = UpdateCache {
            v: crystalline_core::update::UPDATE_CACHE_FORMAT,
            checked_at: at(15),
            outcome: CheckOutcome::Release {
                version: "v0.34.0".to_string(),
                published_at: None,
                first_seen: at(15),
            },
            etag: None,
            last_release: None,
        };
        let context = NowContext {
            check_cache: Some(seen),
            ..context
        };
        assert_eq!(
            act(&context, &fallback, true, false, &mut None)
                .await
                .unwrap(),
            0
        );
        assert_eq!(spawner.calls(), BREW_CALLS);
    }

    #[test]
    fn brew_branch_records_the_command_and_reports_no_move() {
        let spawner = Recording::new(0, brew_outputs("0.33.0"));
        let err = brew_upgrade(&spawner, "brew", &v("0.33.0")).unwrap_err();
        assert_eq!(err.to_string(), BREW_DID_NOT_UPGRADE);
        assert_eq!(err.to_string(), "brew did not upgrade (tap not refreshed?)");
        assert_eq!(spawner.calls(), BREW_CALLS);

        let spawner = Recording::new(0, brew_outputs("0.34.0"));
        assert_eq!(
            brew_upgrade(&spawner, "brew", &v("0.33.0")).unwrap(),
            "upgraded to 0.34.0"
        );

        // A failed brew stops there.
        let spawner = Recording::new(1, brew_outputs("0.34.0"));
        let err = brew_upgrade(&spawner, "brew", &v("0.33.0")).unwrap_err();
        assert_eq!(err.to_string(), "brew upgrade failed (exit code 1)");
        assert_eq!(spawner.calls(), BREW_CALLS[..1]);

        // A pre-release is no upgrade, even when it reads as newer.
        let spawner = Recording::new(0, brew_outputs("0.35.0-rc.1"));
        let err = brew_upgrade(&spawner, "brew", &v("0.33.0")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "brew upgrade ran, but the installed version could not be read"
        );

        // The brew found for the install is the one run.
        let spawner = Recording::new(0, Vec::new());
        let err = brew_upgrade(&spawner, "/opt/homebrew/bin/brew", &v("0.33.0")).unwrap_err();
        assert_eq!(
            spawner.calls()[..2],
            [
                "/opt/homebrew/bin/brew upgrade jordiboehme/tap/crystalline",
                "/opt/homebrew/bin/brew --prefix"
            ]
        );
        assert!(err.to_string().contains("could not be read"), "{err}");

        // What the binary prints is never shown as it came.
        let spawner = Recording::new(0, brew_outputs("0.34.0-rc.1 <b>"));
        let err = brew_upgrade(&spawner, "brew", &v("0.33.0")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "brew upgrade ran, but the installed version could not be read"
        );
    }

    #[test]
    fn brew_is_found_beside_the_cellar_or_on_path() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("homebrew");
        let exe = prefix.join("Cellar/crystalline/0.33.0/bin/crystalline");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(brew_program(Some(&exe)), "brew", "no brew in the prefix");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::write(prefix.join("bin/brew"), b"").unwrap();
        assert_eq!(
            brew_program(Some(&exe)),
            prefix.join("bin/brew").to_string_lossy()
        );
        assert_eq!(
            brew_program(Some(Path::new("/usr/local/bin/crystalline"))),
            "brew"
        );
        assert_eq!(brew_program(None), "brew");
    }

    #[test]
    fn a_gate_counted_from_now_says_how_to_start_the_count() {
        let closed = Gate::OpensAt(at(18));
        assert_eq!(gate_note(None, closed), Some(AGE_FROM_NOW));
        assert_eq!(
            AGE_FROM_NOW,
            "GitHub's API did not answer, so the release age counts from now. crystalline update check starts the count; --ignore-age skips the wait."
        );
        assert_eq!(gate_note(Some(at(17)), closed), None, "a known start");
        assert_eq!(gate_note(None, Gate::Open), None);
        assert_eq!(gate_note(None, Gate::Never), None);
    }

    #[test]
    fn deb_branch_prints_apt_with_and_without_sudo() {
        let dir = Path::new("/home/ada/.cache/crystalline/updates/v0.34.0");
        let deb = "crystalline_0.34.0_amd64.deb";
        assert_eq!(
            apt_steps(dir, deb, Some(1000), false),
            "Install it with:\n  cd /home/ada/.cache/crystalline/updates/v0.34.0\n  sudo apt install ./crystalline_0.34.0_amd64.deb\n"
        );
        assert_eq!(
            apt_steps(dir, deb, Some(0), false),
            "Install it with:\n  cd /home/ada/.cache/crystalline/updates/v0.34.0\n  apt install ./crystalline_0.34.0_amd64.deb\n"
        );
        // A folder with a space or a quote stays one word.
        assert_eq!(
            apt_steps(Path::new("/home/a b/it's"), deb, Some(1000), false),
            "Install it with:\n  cd '/home/a b/it'\\''s'\n  sudo apt install ./crystalline_0.34.0_amd64.deb\n"
        );
        // Under sudo: euid 0, but the person's shell is not root, and the
        // folder is root's, so the full path and `sudo` stay.
        assert_eq!(
            apt_steps(
                Path::new("/tmp/crystalline-update-x/v0.34.0"),
                deb,
                Some(0),
                true
            ),
            "Install it with:\n  sudo apt install /tmp/crystalline-update-x/v0.34.0/crystalline_0.34.0_amd64.deb\n"
        );
    }

    #[test]
    fn checks_ran_are_listed() {
        let all = Checks {
            digest: true,
            sums: true,
            signature: true,
            codesign: false,
            version: false,
        };
        assert_eq!(
            checks_line(&all, "v0.34.0"),
            "Verified: the sha256 digest GitHub lists, the minisign signature over SHA256SUMS for v0.34.0, the line in SHA256SUMS."
        );
        let fallback = Checks {
            digest: false,
            ..all
        };
        assert_eq!(
            checks_line(&fallback, "v0.34.0"),
            "Verified: the minisign signature over SHA256SUMS for v0.34.0, the line in SHA256SUMS. GitHub's API did not answer, so there was no digest to check."
        );
        let tarball = Checks {
            codesign: true,
            version: true,
            ..all
        };
        assert!(
            checks_line(&tarball, "v0.34.0")
                .ends_with(", the macOS code signature, the new binary's version."),
            "{}",
            checks_line(&tarball, "v0.34.0")
        );
    }

    #[test]
    fn refusals_name_the_reason() {
        let refused = |method: InstallMethod, refusal: Option<Refusal>| {
            refusal_of(&Detection { method, refusal }).map(|r| refused_line(&r))
        };
        for (method, refusal, line) in [
            (
                InstallMethod::Tarball,
                Some(Refusal::NotOfficial),
                "update now does nothing: not an official release build",
            ),
            (
                InstallMethod::Homebrew,
                Some(Refusal::AppPackage),
                "update now does nothing: inside an app package; run it in a normal terminal",
            ),
            (
                InstallMethod::Docker,
                Some(Refusal::Docker),
                "update now does nothing: a Docker install; pull the new image",
            ),
            (
                InstallMethod::Mcpb,
                Some(Refusal::Mcpb),
                "update now does nothing: the Claude Desktop extension; Claude Desktop updates it",
            ),
            (
                InstallMethod::Unknown,
                Some(Refusal::MarkerPathMismatch {
                    marker: InstallMethod::Deb,
                    path: InstallMethod::Homebrew,
                }),
                "update now does nothing: the install marker says deb but the path reads as homebrew",
            ),
            (
                InstallMethod::Unknown,
                Some(Refusal::Unknown),
                "update now does nothing: the install method could not be told",
            ),
            // A method without an action is refused even without a reason.
            (
                InstallMethod::Docker,
                None,
                "update now does nothing: the install method could not be told",
            ),
        ] {
            assert_eq!(
                refused(method, refusal).as_deref(),
                Some(line),
                "{method:?}"
            );
        }
        for method in [
            InstallMethod::Homebrew,
            InstallMethod::Deb,
            InstallMethod::Msi,
            InstallMethod::Zip,
            InstallMethod::Tarball,
        ] {
            assert_eq!(refused(method, None), None, "{method:?}");
        }
    }

    #[tokio::test]
    async fn a_refusal_comes_before_the_lock_and_the_lookup() {
        let cache = tempfile::tempdir().unwrap();
        let spawner = Recording::new(0, Vec::new());
        let context = NowContext {
            detection: Detection {
                method: InstallMethod::Tarball,
                refusal: Some(Refusal::NotOfficial),
            },
            cache_dir: cache.path().to_path_buf(),
            ..ctx(InstallMethod::Tarball, &spawner, false, &never_asked)
        };
        let err = run_now(&context, true, true).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "update now does nothing: not an official release build"
        );
        assert!(
            !cache.path().join("updates").exists(),
            "no lock and no download folder"
        );
    }

    #[test]
    fn sudo_skips_finish_and_says_so() {
        let exe = Path::new("/opt/crystalline/crystalline");
        let spawner = Recording::new(0, Vec::new());
        assert_eq!(after_swap(&spawner, exe, true).unwrap(), 0);
        assert!(spawner.calls().is_empty(), "{:?}", spawner.calls());
        assert_eq!(
            SUDO_LEAVES_THE_DAEMON,
            "Under sudo the daemon is not touched: run `crystalline status` as yourself."
        );

        let spawner = Recording::new(0, Vec::new());
        assert_eq!(after_swap(&spawner, exe, false).unwrap(), 0);
        assert_eq!(
            spawner.calls(),
            vec!["/opt/crystalline/crystalline update finish"]
        );

        let failing = Recording::new(3, Vec::new());
        let err = after_swap(&failing, exe, false).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the binary is updated, but update finish failed (exit code 3): run crystalline status to hand the daemon over"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_is_not_writable_is_refused_with_the_manual_steps() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid takes no arguments and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let install = tempfile::tempdir().unwrap();
        let exe = install.path().join("crystalline");
        std::fs::write(&exe, b"bin").unwrap();
        assert_eq!(tarball_refusal(Some(&exe), Some(1000)), None);
        std::fs::set_permissions(install.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let refusal = tarball_refusal(Some(&exe), Some(1000));
        std::fs::set_permissions(install.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            refusal.unwrap(),
            format!(
                "update now does nothing: you cannot write to {}. Update by hand: take crystalline from the tar.gz of the newest release (https://github.com/jordiboehme/crystalline/releases/latest) and move it over {}.",
                display_path(install.path()),
                display_path(&exe)
            )
        );

        // A folder of root's gets the sudo hint, but not for root itself.
        let root_owned = Path::new("/usr/bin/true");
        let refusal = tarball_refusal(Some(root_owned), Some(1000)).unwrap();
        assert!(
            refusal.starts_with(
                "update now does nothing: you cannot write to /usr/bin. Run sudo crystalline update now, or update by hand: "
            ),
            "{refusal}"
        );
        assert_eq!(
            tarball_refusal(None, Some(1000)),
            Some(refused_line(&Refusal::Unknown))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unwritable_tarball_folder_is_refused_before_the_lock() {
        // SAFETY: geteuid takes no arguments and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let cache = tempfile::tempdir().unwrap();
        let spawner = Recording::new(0, Vec::new());
        let context = NowContext {
            cache_dir: cache.path().to_path_buf(),
            exe: Some(PathBuf::from("/usr/bin/true")),
            ..ctx(InstallMethod::Tarball, &spawner, false, &never_asked)
        };
        let err = run_now(&context, true, true).await.unwrap_err();
        assert!(
            err.to_string()
                .starts_with("update now does nothing: you cannot write to /usr/bin."),
            "{err}"
        );
        assert!(!cache.path().join("updates").exists(), "no lock, no folder");
        assert!(spawner.calls().is_empty());
    }

    #[test]
    fn update_finish_names_what_it_did_and_why_not() {
        let own = "0.34.0";
        let kept = Handover::Kept {
            version: "0.33.0".to_string(),
            why: "it read CRYSTALLINE_DATABASE_URL from its environment and that variable is not set here".to_string(),
        };
        assert_eq!(
            handover_line(&kept, own),
            "The daemon (0.33.0) was left running, because it cannot be started again from here the way it ran: it read CRYSTALLINE_DATABASE_URL from its environment and that variable is not set here."
        );
        assert_eq!(
            handover_line(
                &Handover::HandedOver {
                    from: "0.33.0".to_string()
                },
                own
            ),
            "Handed the daemon over from 0.33.0 to 0.34.0."
        );
        assert_eq!(
            handover_line(&Handover::NoDaemon, own),
            "No daemon is running; the next one starts with 0.34.0."
        );
        assert!(handover_line(&Handover::Packaged, own).contains("never handed over"));
        assert!(
            handover_line(
                &Handover::NoSuccessor {
                    from: "0.33.0".to_string()
                },
                own
            )
            .contains("did not start")
        );
    }

    #[test]
    fn a_sudo_folder_stays_only_for_a_downloaded_deb() {
        let folder = tempfile::tempdir().unwrap();
        let ok: anyhow::Result<u8> = Ok(0);
        // Nothing downloaded yet (up to date, gated): it goes.
        std::fs::create_dir_all(folder.path().join("updates")).unwrap();
        assert!(!keeps_sudo_folder(InstallMethod::Deb, &ok, folder.path()));
        std::fs::create_dir_all(folder.path().join("updates").join("v0.34.0")).unwrap();
        assert!(keeps_sudo_folder(InstallMethod::Deb, &ok, folder.path()));
        assert!(!keeps_sudo_folder(
            InstallMethod::Tarball,
            &ok,
            folder.path()
        ));
        let failed: anyhow::Result<u8> = Err(anyhow::anyhow!("refused"));
        assert!(!keeps_sudo_folder(
            InstallMethod::Deb,
            &failed,
            folder.path()
        ));
        let plan_only: anyhow::Result<u8> = Ok(EXIT_UPDATE_AVAILABLE);
        assert!(!keeps_sudo_folder(
            InstallMethod::Deb,
            &plan_only,
            folder.path()
        ));
    }

    #[tokio::test]
    async fn a_tarball_without_its_exe_is_refused_before_the_lock() {
        let cache = tempfile::tempdir().unwrap();
        let spawner = Recording::new(0, Vec::new());
        let context = NowContext {
            cache_dir: cache.path().to_path_buf(),
            exe: None,
            ..ctx(InstallMethod::Tarball, &spawner, false, &never_asked)
        };
        let err = run_now(&context, true, true).await.unwrap_err();
        assert_eq!(err.to_string(), refused_line(&Refusal::Unknown));
        assert!(!cache.path().join("updates").exists(), "no lock, no folder");
    }

    /// `update finish` runs with the update lock free: the spawner tries to
    /// take it while it "runs" the new binary.
    #[test]
    fn update_finish_runs_after_the_lock_is_let_go() {
        struct TakesTheLock {
            cache: PathBuf,
            took: std::sync::Mutex<Option<bool>>,
        }
        impl Spawner for TakesTheLock {
            fn run(&self, _: &str, _: &[&str]) -> std::io::Result<i32> {
                *self.took.lock().unwrap() = Some(take_lock(&self.cache).is_ok());
                Ok(0)
            }
            fn output(&self, _: &str, _: &[&str]) -> std::io::Result<String> {
                unreachable!()
            }
        }
        let cache = tempfile::tempdir().unwrap();
        let mut lock = Some(take_lock(cache.path()).unwrap());
        let spawner = TakesTheLock {
            cache: cache.path().to_path_buf(),
            took: std::sync::Mutex::default(),
        };
        assert!(take_lock(cache.path()).is_err(), "held before the finish");
        let code = after_swap_unlocked(&mut lock, &spawner, Path::new("/opt/c/crystalline"), false)
            .unwrap();
        assert_eq!(code, 0);
        assert!(lock.is_none());
        assert_eq!(
            *spawner.took.lock().unwrap(),
            Some(true),
            "the lock was free"
        );
    }
}
