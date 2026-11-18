//! `crystalline update check`: the newest stable release, how this binary
//! was installed and what `update now` would do for it.
//!
//! Nothing from the network is shown as it came (S23): the newest version
//! is read through [`release_version`] and rendered from the parsed
//! [`Precedence`], the notes URL is built from it, the method and the
//! action come from enums, and a failed check is said in a fixed sentence.

use anyhow::Context;
use crystalline_core::time::{self, Stamp};
use crystalline_core::update::{
    CheckOutcome, Detection, FAILED_REASON, Gate, InstallMethod, Refusal, UPDATE_CACHE_FILE,
    UpdateCache,
};
use crystalline_core::version::{Precedence, release_tag, release_version};
use serde::{Serialize, Serializer};

use crate::ExitWith;

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
/// its publish time, its gate (pass [`Gate::Open`] for `--ignore-age`) and
/// this binary's detection. Only a strictly newer version is an update; an
/// older or equal one is [`Action::UpToDate`]. Pure.
pub(crate) fn plan(
    current: Precedence,
    newest: Option<Precedence>,
    published_at: Option<Stamp>,
    gate: Gate,
    detection: Detection,
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
        Action::Refuse(refusal) => format!("nothing: {}", refusal_text(refusal)),
        Action::UpToDate => "nothing: this is the newest stable release".to_string(),
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
        out.push_str(&format!("Published: {}\n", at.rfc3339_secs_z()));
    }
    out.push_str(&format!("Install method: {}\n", plan.method.label()));
    out.push_str(&format!("update now: {}\n", action_text(&plan.action)));
    out.push_str(&format!("Release notes: {}\n", plan.notes_url));
    out
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
    let min_age_hours = crystalline_service::overlay::load(None)?
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
    let plan = plan(current, Some(found.0), found.1, found.2, detection);
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
    let (version, published_at) = match &cache.outcome {
        CheckOutcome::Release {
            version,
            published_at,
            ..
        } => (version, *published_at),
        CheckOutcome::Failed { reason } => {
            anyhow::bail!("update check failed: {}", plain_reason(reason))
        }
    };
    let Some(found) = release_version(version) else {
        anyhow::bail!("update check failed: {FAILED_REASON}");
    };
    let gate = if ignore_age {
        Gate::Open
    } else {
        cache.gate(now, min_age_hours).unwrap_or(Gate::Open)
    };
    Ok((found, published_at, gate))
}

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
            let plan = plan(v("0.33.0"), Some(v("0.34.0")), None, Gate::Open, detection);
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
        );
        assert_eq!(plan.exit_code(), 0);
        assert_eq!(
            render_plan(&plan),
            "Crystalline 0.34.0 is out (this is 0.33.0), but too new to offer: it is offered from 2026-11-18T10:00:00Z. --ignore-age offers it now.\n\
             Published: 2026-11-17T10:00:00Z\n\
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
        plan(v("0.33.0"), Some(v("0.34.0")), None, gate, detection)
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
}
