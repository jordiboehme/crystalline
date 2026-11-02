//! The automation engine module: status, lease, run record and sharing for
//! the reflect and evolve loops.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset};
use crystalline_core::automation::{Loop, RunOutcome, RunReport};
use crystalline_core::config::{GitHubConfig, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::engine::ShareActor;
use crystalline_service::params::{DiscussParams, ReflectParams, WriteReflectionParams};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, EngineError, ReflectionCaller, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::fixture::{Fx, engram, fixture, manifest};
use crate::support::MockProvider;

/// When every test starts.
const START: &str = "2026-10-21T10:00:00+00:00";

fn at(rfc3339: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(rfc3339).unwrap()
}

fn start() -> DateTime<FixedOffset> {
    at(START)
}

const PAGE: &str = "# D\n\n## Night deploys\n<!-- r:aaaa1111 -->\n\nDeploys run at night.\n\n## Day deploys\n<!-- r:bbbb2222 -->\n\nSome run by day.\n";

fn person() -> ReflectionCaller {
    ReflectionCaller::person(Scope::Unrestricted, "jordi")
}

fn agent() -> ReflectionCaller {
    ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false)
}

/// `eng` with Reflection on, the page `d.md` written by the agent (so it is
/// reflected and its baseline is set), and the clock at [`START`].
async fn reflecting() -> Fx {
    let fx = fixture(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    fx.engine.set_automation_now(Some(start()));
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "d.md".into(),
                markdown: Some(PAGE.into()),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    fx
}

/// A person opens a thread on `section` of `d.md`; it waits for the agent.
async fn open_thread(fx: &Fx, section: &str, text: &str) -> String {
    let opened = fx
        .engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                action: Some("open".into()),
                path: Some("d.md".into()),
                section: Some(section.into()),
                text: Some(text.into()),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(opened["waiting_on"], "agent", "{opened}");
    opened["id"].as_str().unwrap().to_string()
}

async fn step(fx: &Fx, caller: &ReflectionCaller, action: &str, id: &str, text: &str) {
    fx.engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                action: Some(action.into()),
                path: Some("d.md".into()),
                thread: Some(id.into()),
                text: Some(text.into()),
                ..Default::default()
            },
            caller,
        )
        .await
        .unwrap();
}

/// The status of `lp` on `eng` as the machine owner.
async fn status(engine: &Engine, lp: Loop) -> Value {
    status_of(engine, lp, "eng").await
}

async fn status_of(engine: &Engine, lp: Loop, domain: &str) -> Value {
    let answer = engine
        .automation_status(lp, &[domain.to_string()], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(answer["domains"].as_array().unwrap().len(), 1, "{answer}");
    answer["domains"][0].clone()
}

async fn take(engine: &Engine, lp: Loop, domain: &str, runner: &str) -> String {
    let taken = engine
        .automation_lease_take(lp, domain, runner, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(taken["renew_secs"], 60, "{taken}");
    taken["token"].as_str().unwrap().to_string()
}

fn report(outcome: RunOutcome, done: u64, summary: &str) -> RunReport {
    RunReport {
        started_at: START.to_string(),
        outcome,
        done,
        summary: summary.to_string(),
    }
}

async fn record(engine: &Engine, lp: Loop, domain: &str, token: &str, r: RunReport) -> Value {
    engine
        .automation_run_record(lp, domain, token, &r, &Scope::Unrestricted)
        .await
        .unwrap()
}

// --- what is due ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_due_on_a_quiet_domain() {
    let fx = reflecting().await;
    for lp in [Loop::Reflect, Loop::Evolve] {
        let s = status(&fx.engine, lp).await;
        assert_eq!(s["domain"], "eng", "{s}");
        assert_eq!(s["loop"], lp.as_str(), "{s}");
        assert_eq!(s["due"], false, "{s}");
        assert_eq!(s["left_for_person"], 0, "{s}");
        assert_eq!(s["running"], Value::Null, "{s}");
        assert_eq!(s["runs_on"], Value::Null, "{s}");
        assert_eq!(s["last_run"], Value::Null, "{s}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waiting_thread_makes_reflect_due() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], true, "{s}");
    assert_eq!(s["counts"]["waiting_for_the_agent"], 1, "{s}");
    assert_eq!(s["counts"]["feedback"], 0, "{s}");
    // Evolve is another loop: a thread is nothing to it.
    assert_eq!(status(&fx.engine, Loop::Evolve).await["due"], false);
}

/// A judgment-only domain beside one with a reciprocal relation wired from
/// one end, which is mechanical.
const STUB: &str = "A title and almost nothing else.\n\n- [context] meant to be filled in later";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mechanical_finding_makes_evolve_due_and_judgment_does_not() {
    let fx = fixture(&[("stub-note.md", &engram("Stub note", "stub-note", STUB))]).await;
    fx.engine.set_automation_now(Some(start()));
    let s = status(&fx.engine, Loop::Evolve).await;
    assert!(s["counts"]["judgment"].as_u64().unwrap() > 0, "{s}");
    assert_eq!(s["counts"]["mechanical"], 0, "{s}");
    assert_eq!(s["due"], false, "judgment findings are never due: {s}");

    write_summary_pair(&fx.root);
    fx.engine.sync(None).await.unwrap();
    let s = status(&fx.engine, Loop::Evolve).await;
    assert_eq!(s["counts"]["mechanical"], 1, "{s}");
    assert_eq!(s["due"], true, "{s}");
}

/// `summary-doc` summarizes `full-text-doc`, which never says it is
/// summarized: V103, a mechanical finding.
fn write_summary_pair(root: &Path) {
    std::fs::write(
        root.join("summary-doc.md"),
        engram(
            "Summary doc",
            "summary-doc",
            "The distilled version of the migration write-up.\n\n- summarizes [[Full text doc]]\n- [context] written after the migration closed",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("full-text-doc.md"),
        engram(
            "Full text doc",
            "full-text-doc",
            "The full migration write-up with the shard tables.\n\n- [context] the source the summary was cut from\n- [lesson] keep the tables out of the summary",
        ),
    )
    .unwrap();
}

// --- the lease --------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_lease_reports_running_and_is_not_due() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    take(&fx.engine, Loop::Reflect, "eng", "k8s").await;

    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], false, "{s}");
    assert_eq!(s["running"]["runner"], "k8s", "{s}");
    assert_eq!(s["running"]["taken_at"], START, "{s}");
    assert_eq!(s["running"]["renewed_at"], START, "{s}");
    assert!(s["running"].get("token").is_none(), "{s}");

    let refused = fx
        .engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("already running"), "{refused}");
    // The other loop's lease is its own.
    take(&fx.engine, Loop::Evolve, "eng", "cli").await;
}

/// Review Focus 4.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crashed_runner_expires_after_five_minutes() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    let token = take(&fx.engine, Loop::Reflect, "eng", "k8s").await;
    fx.engine
        .set_automation_now(Some(start() + Duration::seconds(120)));
    fx.engine
        .automation_lease_renew(Loop::Reflect, "eng", &token, &Scope::Unrestricted)
        .await
        .unwrap();

    // Then the runner dies. Five minutes after the last renewal it still runs.
    fx.engine
        .set_automation_now(Some(start() + Duration::seconds(120 + 299)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["running"]["runner"], "k8s", "{s}");
    assert_eq!(s["due"], false, "{s}");

    fx.engine
        .set_automation_now(Some(start() + Duration::seconds(120 + 301)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["running"], Value::Null, "{s}");
    assert_eq!(s["due"], true, "{s}");

    let fresh = take(&fx.engine, Loop::Reflect, "eng", "cli").await;
    assert_ne!(fresh, token);
    let renewed = fx
        .engine
        .automation_lease_renew(Loop::Reflect, "eng", &token, &Scope::Unrestricted)
        .await;
    assert!(renewed.is_err(), "the dead runner's token is gone");
    let runs = fx
        .engine
        .automation_runs(Loop::Reflect, "eng", 10, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(runs["runs"][0]["outcome"], "expired", "{runs}");
    assert_eq!(runs["runs"][0]["runner"], "k8s", "{runs}");
}

// --- what a run leaves ------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_item_left_by_a_run_is_not_due_until_it_changes() {
    let fx = reflecting().await;
    let id = open_thread(&fx, "aaaa1111", "Should we move them?").await;
    let token = take(&fx.engine, Loop::Reflect, "eng", "cli").await;
    let recorded = record(
        &fx.engine,
        Loop::Reflect,
        "eng",
        &token,
        report(RunOutcome::Done, 0, "Left the thread for a person."),
    )
    .await;
    assert_eq!(recorded["run"]["left"], 1, "{recorded}");

    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], false, "{s}");
    assert_eq!(s["left_for_person"], 1, "{s}");
    assert_eq!(
        s["running"],
        Value::Null,
        "the record released the lease: {s}"
    );
    assert_eq!(s["last_run"]["outcome"], "done", "{s}");
    assert_eq!(
        s["last_run"]["summary"], "Left the thread for a person.",
        "{s}"
    );

    // A new message on the thread is a change: due again.
    step(&fx, &person(), "answer", &id, "Yes, by Friday please.").await;
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], true, "{s}");
    assert_eq!(s["left_for_person"], 0, "{s}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capped_run_leaves_the_rest_due() {
    let fx = reflecting().await;
    let first = open_thread(&fx, "aaaa1111", "Still true?").await;
    open_thread(&fx, "bbbb2222", "Which days?").await;
    let token = take(&fx.engine, Loop::Reflect, "eng", "cli").await;
    step(&fx, &agent(), "answer", &first, "Yes, still true.").await;
    let recorded = record(
        &fx.engine,
        Loop::Reflect,
        "eng",
        &token,
        report(RunOutcome::StoppedAtCap, 1, "Answered one thread."),
    )
    .await;
    assert_eq!(recorded["run"]["left"], 0, "{recorded}");
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], true, "{s}");
    assert_eq!(s["counts"]["waiting_for_the_agent"], 1, "{s}");
    assert_eq!(s["left_for_person"], 0, "{s}");
    assert_eq!(s["last_run"]["outcome"], "stopped_at_cap", "{s}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_run_parks_nothing() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    let token = take(&fx.engine, Loop::Reflect, "eng", "cli").await;
    record(
        &fx.engine,
        Loop::Reflect,
        "eng",
        &token,
        report(RunOutcome::Failed, 0, &"x".repeat(5000)),
    )
    .await;
    // The loop backs off for 5 minutes after the failure; then the item
    // that run did not settle is due again.
    fx.engine
        .set_automation_now(Some(start() + Duration::seconds(301)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], true, "{s}");
    assert_eq!(s["left_for_person"], 0, "{s}");
    assert_eq!(s["last_run"]["outcome"], "failed", "{s}");
    assert_eq!(
        s["last_run"]["summary"].as_str().unwrap().len(),
        2048,
        "the summary is capped"
    );
}

/// Record one run of `outcome` on `eng` at `now`.
async fn run_at(fx: &Fx, now: DateTime<FixedOffset>, outcome: RunOutcome) {
    fx.engine.set_automation_now(Some(now));
    let token = take(&fx.engine, Loop::Reflect, "eng", "cli").await;
    record(
        &fx.engine,
        Loop::Reflect,
        "eng",
        &token,
        report(outcome, 0, ""),
    )
    .await;
}

/// A run that fails the same way every time costs one run per wait, not one
/// per poll: after N failed runs in a row the loop is not due, and says why,
/// until 5 minutes after the last failure, doubled for each further one. A
/// person can still start a run with force, and a run that did not fail
/// ends the wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_runs_back_off_until_a_run_does_not_fail() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    run_at(&fx, start(), RunOutcome::Failed).await;
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], false, "{s}");
    assert_eq!(s["reason"], "backing off after 1 failed run", "{s}");
    assert_eq!(
        s["counts"]["waiting_for_the_agent"], 1,
        "the work is still counted: {s}"
    );

    // A person who starts a run by hand skips the wait, and only the wait.
    let forced = fx
        .engine
        .automation_status_forced(
            Loop::Reflect,
            &["eng".to_string()],
            &Scope::Unrestricted,
            true,
        )
        .await
        .unwrap();
    assert_eq!(forced["domains"][0]["due"], true, "{forced}");

    // A second failure 6 minutes in: the wait doubles to 10 minutes from it.
    let second = start() + Duration::seconds(360);
    run_at(&fx, second, RunOutcome::Failed).await;
    fx.engine
        .set_automation_now(Some(second + Duration::seconds(599)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["reason"], "backing off after 2 failed runs", "{s}");
    assert_eq!(s["due"], false, "{s}");
    fx.engine
        .set_automation_now(Some(second + Duration::seconds(601)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], true, "{s}");
    assert_eq!(s["reason"], Value::Null, "{s}");

    // A run that stops at its cap ends the row; the next failure waits 5
    // minutes again.
    let capped = second + Duration::seconds(700);
    run_at(&fx, capped, RunOutcome::StoppedAtCap).await;
    run_at(&fx, capped + Duration::seconds(10), RunOutcome::Failed).await;
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["reason"], "backing off after 1 failed run", "{s}");
    fx.engine
        .set_automation_now(Some(capped + Duration::seconds(311)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], true, "{s}");
}

/// A failure recorded "later" than now, after the clock went back, is no
/// reason to wait: the backoff never outlasts its own length.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_from_the_future_backs_nothing_off() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    run_at(&fx, start(), RunOutcome::Failed).await;
    fx.engine
        .set_automation_now(Some(start() - Duration::hours(2)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["reason"], Value::Null, "{s}");
    assert_eq!(s["due"], true, "{s}");
}

/// The wait never grows past 6 hours, however many runs failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_backoff_stops_at_six_hours() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    let mut now = start();
    for _ in 0..12 {
        run_at(&fx, now, RunOutcome::Failed).await;
        now += Duration::seconds(1);
    }
    let last = now - Duration::seconds(1);
    fx.engine
        .set_automation_now(Some(last + Duration::seconds(6 * 3600 - 1)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["reason"], "backing off after 12 failed runs", "{s}");
    fx.engine
        .set_automation_now(Some(last + Duration::seconds(6 * 3600 + 1)));
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], true, "{s}");
}

/// An instance started read-only writes nothing, so no loop runs there:
/// status says it is not hosted here and a take is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_runs_no_loop() {
    let fx = fixture(&[("stub-note.md", &engram("Stub note", "stub-note", STUB))]).await;
    write_summary_pair(&fx.root);
    fx.engine.sync(None).await.unwrap();
    let cfg = fx.engine.config();
    let store = fx.engine.store();
    let read_only = Engine::new(store, cfg, None, None).with_read_only(true);
    read_only.set_automation_now(Some(start()));
    for lp in [Loop::Evolve, Loop::Reflect] {
        let answer = read_only
            .automation_status(lp, &["eng".to_string()], &Scope::Unrestricted)
            .await
            .unwrap();
        let s = &answer["domains"][0];
        assert_eq!(s["due"], false, "{s}");
        assert_eq!(s["reason"], "not hosted here", "{s}");
        let refused = read_only
            .automation_lease_take(lp, "eng", "cli", &Scope::Unrestricted)
            .await
            .unwrap_err();
        assert!(matches!(refused, EngineError::ReadOnly), "{refused}");
    }
}

/// The age of the one `source_changed` item in `eng`'s queue.
async fn source_change_age(fx: &Fx) -> i64 {
    let queue = fx
        .engine
        .reflect(
            &ReflectParams {
                domains: vec!["eng".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let item = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["what"] == "source_changed")
        .unwrap_or_else(|| panic!("a source_changed item: {queue}"));
    item["age_secs"].as_i64().unwrap()
}

/// A section's source changed a day ago and a run left the update for a
/// person. Time goes on: the item's age moves (it is read from the real
/// clock against the engram's mtime), the test clock moves a day, and the
/// version does not move, so the item stays left. The index records an
/// mtime only with a content change, so the source's new text is dated a
/// day back before it is synced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unchanged_item_keeps_its_version_a_day_later() {
    let fx = reflecting().await;
    let source = fx.root.join("runbooks/deploy.md");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(
        &source,
        engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "d.md".into(),
                sources: BTreeMap::from([(
                    "aaaa1111".to_string(),
                    vec!["runbooks/deploy".to_string()],
                )]),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    std::fs::write(
        &source,
        engram(
            "Deploy",
            "runbooks/deploy",
            "- [fact] deploys run in the morning",
        ),
    )
    .unwrap();
    let file = std::fs::File::options().write(true).open(&source).unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(86_400))
        .unwrap();
    drop(file);
    fx.engine.sync(None).await.unwrap();
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], true, "{s}");
    assert_eq!(s["counts"]["updates"], 1, "{s}");

    let token = take(&fx.engine, Loop::Reflect, "eng", "cli").await;
    record(
        &fx.engine,
        Loop::Reflect,
        "eng",
        &token,
        report(RunOutcome::Done, 0, "Left the update."),
    )
    .await;
    let before = source_change_age(&fx).await;
    assert!(before >= 86_000, "a day old: {before}");

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    fx.engine
        .set_automation_now(Some(start() + Duration::days(1)));
    let after = source_change_age(&fx).await;
    assert!(after > before, "the age moved: {before} then {after}");
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], false, "{s}");
    assert_eq!(s["left_for_person"], 1, "{s}");
}

// --- who runs ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_domain_runs_only_on_its_home() {
    let fx = fixture(&[(
        "MANIFEST.md",
        &manifest("reflection: on\nreflection_home: https://home.example\n"),
    )])
    .await;
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["runs_on"], "https://home.example", "{s}");
    assert_eq!(s["due"], false, "{s}");
    let refused = fx
        .engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert_eq!(refused.to_string(), "runs on https://home.example");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rights_are_checked() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("vera", "Vera", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let vera = Scope::User {
        account: "vera".into(),
        admin: false,
    };
    for lp in [Loop::Reflect, Loop::Evolve] {
        let all = fx.engine.automation_status(lp, &[], &vera).await.unwrap();
        assert_eq!(all["domains"], serde_json::json!([]), "{lp}: {all}");
        let named = fx
            .engine
            .automation_status(lp, &["eng".to_string()], &vera)
            .await
            .unwrap_err();
        assert!(matches!(named, EngineError::Forbidden(_)), "{lp}: {named}");
        let take = fx
            .engine
            .automation_lease_take(lp, "eng", "cli", &vera)
            .await
            .unwrap_err();
        assert!(matches!(take, EngineError::Forbidden(_)), "{lp}: {take}");
        let runs = fx
            .engine
            .automation_runs(lp, "eng", 10, &vera)
            .await
            .unwrap_err();
        assert!(matches!(runs, EngineError::Forbidden(_)), "{lp}: {runs}");
    }
    // The owner sees the domain.
    let all = fx
        .engine
        .automation_status(Loop::Reflect, &[], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(all["domains"][0]["domain"], "eng", "{all}");
}

// --- sharing ----------------------------------------------------------------

/// Carries the team marker, as every team repository does after its first
/// share from 0.30.0 on, so a run's share carries only the run's files.
fn team_manifest_direct() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\nsharing: direct\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

/// A team domain `brand` registered from the forge, whose MANIFEST shares
/// directly, with the clock at [`START`].
async fn direct_team(
    tmp: &tempfile::TempDir,
    mock: Arc<MockProvider>,
) -> (Engine, std::path::PathBuf) {
    direct_team_with(tmp, mock, team_manifest_direct()).await
}

/// [`direct_team`] over a MANIFEST of the caller's own.
async fn direct_team_with(
    tmp: &tempfile::TempDir,
    mock: Arc<MockProvider>,
    manifest: Vec<u8>,
) -> (Engine, std::path::PathBuf) {
    let commit = mock.add_commit(BTreeMap::from([("MANIFEST.md".to_string(), manifest)]));
    mock.set_branch("main", &commit);
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let store = TursoStore::open_in_memory().await.unwrap();
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(tmp.path().join("config.yaml")),
    )
    .with_origin_provider(mock)
    .with_origins_dir(tmp.path().join("origins"))
    .with_state_dir(tmp.path().to_path_buf());
    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    eng.set_automation_now(Some(start()));
    std::fs::create_dir_all(root.join("notes")).unwrap();
    (eng, root)
}

fn shared_files(shared: &Value) -> Vec<String> {
    let mut files: Vec<String> = shared["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_string())
        .collect();
    files.sort();
    files
}

/// `brand` with an accounts store in which `runner` (the runner's account)
/// and `pia` (a person) may write, and the runner's scope.
async fn with_accounts(eng: &Engine, tmp: &tempfile::TempDir) -> Scope {
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["runner", "pia"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    eng.set_domain_access(Arc::new(DomainAccess::new(auth)));
    Scope::User {
        account: "runner".into(),
        admin: false,
    }
}

fn write_note(root: &Path, rel: &str, body: &str) {
    let permalink = rel.trim_end_matches(".md");
    std::fs::write(root.join(rel), engram(body, permalink, body)).unwrap();
}

fn names(list: &Value) -> Vec<String> {
    let mut out: Vec<String> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

/// A run's share on a repository with no team marker yet carries it once,
/// and says so: the receipt names the MANIFEST with the reason, and the pull
/// request body ends with the note after the run's own summary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_share_carries_the_team_marker_and_says_why() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let unmarked = String::from_utf8(team_manifest_direct())
        .unwrap()
        .replace("team: github\n", "")
        .into_bytes();
    let (eng, root) = direct_team_with(&tmp, mock.clone(), unmarked).await;
    let token = take(&eng, Loop::Evolve, "brand", "k8s").await;
    write_note(&root, "notes/fix.md", "the run's fix");
    let shared = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            &token,
            "Fixed one finding.",
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "shared", "{shared}");
    assert_eq!(
        shared_files(&shared),
        vec!["MANIFEST.md", "notes/fix.md"],
        "{shared}"
    );
    assert_eq!(
        shared["team_marker"],
        serde_json::json!({
            "path": "MANIFEST.md",
            "reason": "marks this repository as a team domain; added once"
        }),
        "{shared}"
    );
    let body = mock
        .proposal_body(1)
        .expect("the run opened a pull request");
    assert!(body.starts_with("Fixed one finding."), "{body}");
    assert!(
        body.trim_end().ends_with(
            "MANIFEST.md gains `team: github`. Crystalline added it: it marks this repository as a team domain, and it is added once."
        ),
        "{body}"
    );
}

/// The no-account fallback: a lease taken as the machine owner cannot
/// attribute a write, so it shares what is new or changed since the take,
/// as a pull request even under `sharing: direct`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_opens_a_pull_request_under_direct_sharing() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = direct_team(&tmp, mock.clone()).await;
    let token = take(&eng, Loop::Evolve, "brand", "k8s").await;
    write_note(&root, "notes/fix.md", "the run's fix");
    let shared = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            &token,
            "Fixed one finding.",
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "shared", "{shared}");
    assert_eq!(shared["attributed"], false, "{shared}");
    assert_eq!(shared_files(&shared), vec!["notes/fix.md"], "{shared}");
    let url = shared["url"].as_str().unwrap().to_string();
    assert!(
        mock.calls()
            .iter()
            .any(|c| c.starts_with("create_proposal")),
        "a pull request, not a direct commit: {:?}",
        mock.calls()
    );
    assert!(
        !mock
            .calls()
            .iter()
            .any(|c| c.starts_with("update_branch:main:")),
        "{:?}",
        mock.calls()
    );
    let recorded = record(
        &eng,
        Loop::Evolve,
        "brand",
        &token,
        report(RunOutcome::Done, 1, "Fixed one finding."),
    )
    .await;
    assert_eq!(recorded["run"]["pr_url"], url.as_str(), "{recorded}");
    assert!(recorded["run"].get("left_items").is_none(), "{recorded}");
    let s = status_of(&eng, Loop::Evolve, "brand").await;
    assert_eq!(s["last_run"]["pr_url"], url.as_str(), "{s}");
    assert!(s["last_run"].get("left_items").is_none(), "{s}");
}

/// A second run while the first run's pull request is still open: the
/// share adds to that pull request (`updated`) and reports what it carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_run_adds_to_the_open_pull_request() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = direct_team(&tmp, mock.clone()).await;
    let first = take(&eng, Loop::Evolve, "brand", "k8s").await;
    write_note(&root, "notes/fix.md", "the first fix");
    let one = eng
        .automation_share(Loop::Evolve, "brand", &first, "One.", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(one["outcome"], "shared", "{one}");
    record(
        &eng,
        Loop::Evolve,
        "brand",
        &first,
        report(RunOutcome::Done, 1, "One."),
    )
    .await;

    let second = take(&eng, Loop::Evolve, "brand", "k8s").await;
    write_note(&root, "notes/fix-two.md", "the second fix");
    let two = eng
        .automation_share(Loop::Evolve, "brand", &second, "Two.", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(two["outcome"], "shared", "{two}");
    assert!(
        shared_files(&two).contains(&"notes/fix-two.md".to_string()),
        "{two}"
    );
    assert!(two["url"].is_string(), "{two}");
    // The share's own outcome was `updated`: one pull request, amended.
    let calls = mock.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("create_proposal"))
            .count(),
        1,
        "{calls:?}"
    );
    assert!(calls.iter().any(|c| c == "update_proposal:1"), "{calls:?}");
    assert_eq!(two["url"], one["url"], "the same pull request");
}

/// Ruling B: with an account on the lease, only the paths its runner wrote
/// over MCP travel. A person's edit on another path during the run stays
/// out; a person's unshared edit on a path the run fixed too is held back
/// and named.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_takes_only_the_runs_files_when_people_had_unshared_work() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = direct_team(&tmp, mock.clone()).await;
    let runner = with_accounts(&eng, &tmp).await;
    // A person's unshared edit, before the run takes its lease.
    write_note(&root, "notes/shared-edit.md", "a person's draft");
    // The lease is the runner's: taken as its account.
    let taken = eng
        .automation_lease_take(Loop::Evolve, "brand", "k8s", &runner)
        .await
        .unwrap();
    let token = taken["token"].as_str().unwrap().to_string();
    // The run writes over MCP (what the tool dispatch notes): its own fix,
    // and a fix of the file the person had unshared work on.
    write_note(&root, "notes/fix.md", "the run's fix");
    write_note(
        &root,
        "notes/shared-edit.md",
        "a person's draft, fixed by the run",
    );
    eng.note_written(
        "brand",
        "runner",
        &[
            "notes/fix.md".to_string(),
            "notes/shared-edit.md".to_string(),
        ],
    )
    .await
    .unwrap();
    // A person edits another path during the run, through Fluid or the
    // files; nothing notes it. Another account's MCP write is not noted
    // for this lease either.
    write_note(&root, "notes/person-during.md", "typed while the run ran");
    write_note(&root, "notes/pia.md", "pia's agent wrote this");
    eng.note_written("brand", "pia", &["notes/pia.md".to_string()])
        .await
        .unwrap();

    let shared = eng
        .automation_share(Loop::Evolve, "brand", &token, "Fixed two.", &runner)
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "shared", "{shared}");
    assert_eq!(shared["attributed"], true, "{shared}");
    assert_eq!(shared_files(&shared), vec!["notes/fix.md"], "{shared}");
    assert_eq!(
        names(&shared["held_back"]),
        vec!["notes/shared-edit.md"],
        "{shared}"
    );
    // The people's work is still theirs to share.
    let left = eng
        .local_changes("brand", &ShareActor::Owner)
        .await
        .unwrap();
    let paths: Vec<&str> = left["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    for person in [
        "notes/shared-edit.md",
        "notes/person-during.md",
        "notes/pia.md",
    ] {
        assert!(paths.contains(&person), "{person}: {left}");
    }
}

/// The fallback holds back a path a person had unshared work on too, even
/// when the run changed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fallback_holds_back_a_persons_unshared_path() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = direct_team(&tmp, mock.clone()).await;
    write_note(&root, "notes/x.md", "a person's draft");
    let token = take(&eng, Loop::Evolve, "brand", "k8s").await;
    write_note(&root, "notes/x.md", "a person's draft, changed by the run");
    let shared = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            &token,
            "Nothing of mine.",
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "nothing_to_share", "{shared}");
    assert_eq!(shared["attributed"], false, "{shared}");
    assert_eq!(names(&shared["held_back"]), vec!["notes/x.md"], "{shared}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_share_and_a_record_with_a_wrong_token_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = direct_team(&tmp, mock.clone()).await;
    take(&eng, Loop::Evolve, "brand", "k8s").await;
    write_note(&root, "notes/fix.md", "the run's fix");
    let share = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            "not-the-token",
            "x",
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(share.to_string().contains("lease is not held"), "{share}");
    assert!(
        !mock
            .calls()
            .iter()
            .any(|c| c.starts_with("create_proposal")),
        "{:?}",
        mock.calls()
    );
    let recorded = eng
        .automation_run_record(
            Loop::Evolve,
            "brand",
            "not-the-token",
            &report(RunOutcome::Done, 0, "x"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(
        recorded.to_string().contains("lease is not held"),
        "{recorded}"
    );
    let runs = eng
        .automation_runs(Loop::Evolve, "brand", 10, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(runs["runs"], serde_json::json!([]), "{runs}");
}

// --- more of the lease ------------------------------------------------------

/// A run whose lease expired and was taken over records nothing: the
/// takeover recorded it as expired, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_after_a_takeover_is_refused() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    let old = take(&fx.engine, Loop::Reflect, "eng", "k8s").await;
    fx.engine
        .set_automation_now(Some(start() + Duration::seconds(301)));
    take(&fx.engine, Loop::Reflect, "eng", "cli").await;
    let err = fx
        .engine
        .automation_run_record(
            Loop::Reflect,
            "eng",
            &old,
            &report(RunOutcome::Done, 0, "late"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("lease is not held"), "{err}");
    let runs = fx
        .engine
        .automation_runs(Loop::Reflect, "eng", 10, &Scope::Unrestricted)
        .await
        .unwrap();
    let outcomes: Vec<&str> = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, vec!["expired"], "{runs}");
}

/// Two runners racing for one lease: exactly one gets it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_runners_racing_get_one_lease() {
    let fx = reflecting().await;
    open_thread(&fx, "aaaa1111", "Still true?").await;
    let engine = fx.engine.clone();
    let race = |runner: &'static str| {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .automation_lease_take(Loop::Reflect, "eng", runner, &Scope::Unrestricted)
                .await
        })
    };
    let (a, b) = tokio::join!(race("a"), race("b"));
    let results = [a.unwrap(), b.unwrap()];
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    let refused = results.iter().find_map(|r| r.as_ref().err()).unwrap();
    assert_eq!(refused.to_string(), "already running");
}

/// What a runner sends is checked: a start time that is not RFC 3339 and a
/// runner label that is empty or too long are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runners_input_is_checked() {
    let fx = reflecting().await;
    for runner in ["", "   ", &"r".repeat(129)] {
        let err = fx
            .engine
            .automation_lease_take(Loop::Reflect, "eng", runner, &Scope::Unrestricted)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Invalid(_)), "{runner:?}: {err}");
    }
    let token = take(&fx.engine, Loop::Reflect, "eng", &"r".repeat(128)).await;
    let mut bad = report(RunOutcome::Done, 0, "x");
    bad.started_at = "yesterday".into();
    let err = fx
        .engine
        .automation_run_record(Loop::Reflect, "eng", &token, &bad, &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
}

/// A domain whose loop cannot run anywhere here says why beside `due`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_that_cannot_run_here_says_so() {
    let fx = fixture(&[]).await;
    let s = status(&fx.engine, Loop::Reflect).await;
    assert_eq!(s["due"], false, "{s}");
    assert_eq!(s["reason"], "not hosted here", "{s}");
    assert_eq!(s["runs_on"], Value::Null, "{s}");
    let quiet = status(&fx.engine, Loop::Evolve).await;
    assert_eq!(quiet["reason"], Value::Null, "{quiet}");
}

// --- on a shared database ---------------------------------------------------

/// Two instances on one Postgres schema: evolve runs on the host lock's
/// holder and the other names it; a take over a stale lease records one
/// expired run.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_postgres_evolve_runs_on_the_host_and_a_stale_lease_expires_once() {
    use crate::fixture::{instance_on, scratch_schema};
    use crystalline_index::Store;

    let Some(scratch) = scratch_schema() else {
        return;
    };
    let open = || async {
        let store = crystalline_index::PostgresStore::open_in_schema(scratch.url(), scratch.name())
            .await
            .expect("open the postgres test schema");
        Arc::new(Mutex::new(store)) as Arc<Mutex<dyn Store>>
    };
    let a = instance_on(open().await, "alpha", &[]).await;
    let b = instance_on(open().await, "beta", &[]).await;
    a.engine.set_automation_now(Some(start()));
    b.engine.set_automation_now(Some(start()));

    let s = status(&b.engine, Loop::Evolve).await;
    assert_eq!(s["runs_on"], "alpha", "{s}");
    let refused = b
        .engine
        .automation_lease_take(Loop::Evolve, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert_eq!(refused.to_string(), "runs on alpha");

    let old = take(&a.engine, Loop::Evolve, "eng", "k8s").await;
    a.engine
        .set_automation_now(Some(start() + Duration::seconds(301)));
    take(&a.engine, Loop::Evolve, "eng", "cli").await;
    let err = a
        .engine
        .automation_lease_renew(Loop::Evolve, "eng", &old, &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("lease is not held"), "{err}");
    let runs = a
        .engine
        .automation_runs(Loop::Evolve, "eng", 10, &Scope::Unrestricted)
        .await
        .unwrap();
    let outcomes: Vec<&str> = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, vec!["expired"], "{runs}");
}

/// A lease that went stale but that nobody took over is still its token's:
/// a write the runner made while it lapsed is noted and shared.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_while_the_lease_lapsed_is_still_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = direct_team(&tmp, mock.clone()).await;
    let runner = with_accounts(&eng, &tmp).await;
    let taken = eng
        .automation_lease_take(Loop::Evolve, "brand", "k8s", &runner)
        .await
        .unwrap();
    let token = taken["token"].as_str().unwrap().to_string();
    eng.set_automation_now(Some(start() + Duration::seconds(400)));
    write_note(&root, "notes/late.md", "written while the lease lapsed");
    eng.note_written("brand", "runner", &["notes/late.md".to_string()])
        .await
        .unwrap();
    eng.automation_lease_renew(Loop::Evolve, "brand", &token, &runner)
        .await
        .unwrap();
    let shared = eng
        .automation_share(Loop::Evolve, "brand", &token, "Late fix.", &runner)
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "shared", "{shared}");
    assert_eq!(shared_files(&shared), vec!["notes/late.md"], "{shared}");
    assert_eq!(shared["recorded"], true, "{shared}");
}
