//! `automation_status` as a poll asks it (issue #239): it reads through the
//! store's read connections, never the writer lock, folds the caller's
//! visibility once per call, and with `counts: false` skips the due work.

use std::sync::Arc;
use std::time::Duration;

use crystalline_core::automation::Loop;
use crystalline_core::time;
use crystalline_index::{Store, TursoStore};
use crystalline_service::params::{DiscussParams, WriteReflectionParams};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, ReflectionCaller, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::fixture::{Fx, engram, fixture_on, manifest};

/// How long a status may take while the writer lock is held; far past
/// what it needs, far short of forever.
const PATIENCE: Duration = Duration::from_secs(10);

const PAGE: &str = "# D\n\n## Night deploys\n<!-- r:aaaa1111 -->\n\nDeploys run at night.\n\n## Day deploys\n<!-- r:bbbb2222 -->\n\nSome run by day.\n";

fn files() -> Vec<(&'static str, String)> {
    vec![
        ("MANIFEST.md", manifest("reflection: on\n")),
        (
            "notes/deploys.md",
            engram("Deploys", "notes/deploys", "We deploy at night."),
        ),
    ]
}

/// `eng` on `store` with Reflection on, a page the agent wrote and a thread
/// a person opened on it, so reflect has one item due.
async fn reflecting_on(store: Arc<Mutex<dyn Store>>) -> Fx {
    let files = files();
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
    let fx = fixture_on(store, &borrowed, |_, _| {}, None, None, None).await;
    prepare(&fx).await;
    fx
}

async fn prepare(fx: &Fx) {
    fx.engine.set_automation_now(Some(
        time::parse_rfc3339("2026-10-21T10:00:00+00:00").unwrap(),
    ));
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "d.md".into(),
                markdown: Some(PAGE.into()),
                ..Default::default()
            },
            &ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false),
        )
        .await
        .unwrap();
    fx.engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                action: Some("open".into()),
                path: Some("d.md".into()),
                section: Some("aaaa1111".into()),
                text: Some("Still true?".into()),
                ..Default::default()
            },
            &ReflectionCaller::person(Scope::Unrestricted, "jordi"),
        )
        .await
        .unwrap();
}

async fn status_with(engine: &Engine, lp: Loop, counts: bool) -> Value {
    engine
        .automation_status_with(
            lp,
            &["eng".to_string()],
            &Scope::Unrestricted,
            false,
            counts,
        )
        .await
        .unwrap()
}

/// Both loops answer while a test holds the writer lock, and answer what
/// they answer without it.
async fn answers_under_the_held_lock(fx: &Fx) {
    let reflect = status_with(&fx.engine, Loop::Reflect, true).await;
    assert_eq!(reflect["domains"][0]["due"], true, "{reflect}");
    assert_eq!(
        reflect["domains"][0]["counts"]["waiting_for_the_agent"], 1,
        "{reflect}"
    );
    let evolve = status_with(&fx.engine, Loop::Evolve, true).await;
    let store = fx.engine.store();
    let held = store.lock().await;
    for (lp, before) in [(Loop::Reflect, &reflect), (Loop::Evolve, &evolve)] {
        let during = tokio::time::timeout(PATIENCE, status_with(&fx.engine, lp, true))
            .await
            .unwrap_or_else(|_| panic!("{lp:?} status waited for the writer lock"));
        assert_eq!(&during, before, "{lp:?}");
        let cheap = tokio::time::timeout(PATIENCE, status_with(&fx.engine, lp, false))
            .await
            .unwrap_or_else(|_| panic!("{lp:?} cheap status waited for the writer lock"));
        assert_eq!(cheap["domains"][0]["runs_on"], Value::Null, "{cheap}");
    }
    drop(held);
}

/// On a turso file the store has read connections: a status never waits
/// for the writer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_status_answers_while_the_writer_lock_is_held_on_turso() {
    let dir = tempfile::tempdir().unwrap();
    let store = TursoStore::open(&dir.path().join("index.db"))
        .await
        .unwrap();
    assert!(
        store.read_pool().is_some(),
        "a turso file has read connections"
    );
    let fx = reflecting_on(Arc::new(Mutex::new(store))).await;
    answers_under_the_held_lock(&fx).await;
}

/// Status of `lp` on `domain` with counts and without: the answer before
/// the gate's entries expire, and the same answers while a test holds the
/// writer lock after they expired.
#[cfg(feature = "postgres")]
async fn same_under_the_held_lock(engine: &Engine, lp: Loop, domain: &str) -> Value {
    let ask = |counts: bool| async move {
        engine
            .automation_status_with(
                lp,
                &[domain.to_string()],
                &Scope::Unrestricted,
                false,
                counts,
            )
            .await
            .unwrap()
    };
    let full = ask(true).await;
    let cheap = ask(false).await;
    engine.forget_remembered_gates();
    let store = engine.store();
    let held = store.lock().await;
    for (counts, before) in [(true, &full), (false, &cheap)] {
        let during = tokio::time::timeout(PATIENCE, ask(counts))
            .await
            .unwrap_or_else(|_| {
                panic!("{lp:?} status of '{domain}' (counts {counts}) waited for the writer lock")
            });
        assert_eq!(&during, before, "{lp:?} '{domain}' counts {counts}");
    }
    drop(held);
    full
}

/// On Postgres the read pool answers: on the instance that holds the
/// domain's host lock once the gate's remembered entry expired (it lasts
/// `VISIBILITY_TTL` on a shared database), on an instance another one hosts
/// the domain for, and for the evolve status of a virtual domain, whose
/// MANIFEST is a stored row.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_status_answers_while_the_writer_lock_is_held_on_postgres() {
    use crate::fixture::{instance_on, scratch_schema};

    let Some(scratch) = scratch_schema() else {
        return;
    };
    let open = || async {
        let store = crystalline_index::PostgresStore::open_in_schema(scratch.url(), scratch.name())
            .await
            .expect("open the postgres test schema");
        Arc::new(Mutex::new(store)) as Arc<Mutex<dyn Store>>
    };
    let files = files();
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
    let alpha = instance_on(open().await, "alpha", &borrowed).await;
    prepare(&alpha).await;
    let beta = instance_on(open().await, "beta", &borrowed).await;
    beta.engine.set_automation_now(Some(
        time::parse_rfc3339("2026-10-21T10:00:00+00:00").unwrap(),
    ));
    alpha.engine.domain_add_virtual("vnotes").await.unwrap();

    answers_under_the_held_lock(&alpha).await;
    let reflect = same_under_the_held_lock(&alpha.engine, Loop::Reflect, "eng").await;
    assert_eq!(reflect["domains"][0]["due"], true, "{reflect}");
    same_under_the_held_lock(&alpha.engine, Loop::Evolve, "eng").await;
    same_under_the_held_lock(&alpha.engine, Loop::Evolve, "vnotes").await;
    let elsewhere = same_under_the_held_lock(&beta.engine, Loop::Reflect, "eng").await;
    assert_eq!(elsewhere["domains"][0]["runs_on"], "alpha", "{elsewhere}");
}

/// `counts: false` answers the lease, the last run and where the loop runs,
/// with `due`, `counts` and `left_for_person` null, and works out nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_status_without_counts_runs_no_detection() {
    let store = TursoStore::open_in_memory().await.unwrap();
    let fx = reflecting_on(Arc::new(Mutex::new(store))).await;
    for lp in [Loop::Reflect, Loop::Evolve] {
        let before = fx.engine.due_detections();
        let cheap = status_with(&fx.engine, lp, false).await;
        assert_eq!(fx.engine.due_detections(), before, "{lp:?}: {cheap}");
        let entry = &cheap["domains"][0];
        for key in ["due", "counts", "left_for_person"] {
            assert_eq!(entry[key], Value::Null, "{lp:?} {key}: {cheap}");
        }
        for key in ["running", "runs_on", "reason", "last_run", "delivery"] {
            assert!(entry.get(key).is_some(), "{lp:?} keeps {key}: {cheap}");
        }
        let full = status_with(&fx.engine, lp, true).await;
        assert_eq!(fx.engine.due_detections(), before + 1, "{lp:?}: {full}");
        let entry = &full["domains"][0];
        assert!(entry["due"].is_boolean(), "{full}");
        assert!(entry["counts"].is_object(), "{full}");
    }
}

/// One status call over several domains folds the caller's visibility
/// once, however many rights checks it makes; the next call folds again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_status_call_folds_the_callers_visibility_once() {
    let store = TursoStore::open_in_memory().await.unwrap();
    let fx = reflecting_on(Arc::new(Mutex::new(store))).await;
    for name in ["ops", "web"] {
        let root = fx.tmp.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("MANIFEST.md"),
            manifest("reflection: on\n").replace("title: eng", &format!("title: {name}")),
        )
        .unwrap();
        fx.engine
            .domain_add_local(Some(name), Some(root.to_str().unwrap()))
            .await
            .unwrap();
    }
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("runner", "runner", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let runner = Scope::User {
        account: "runner".into(),
        admin: false,
    };
    for lp in [Loop::Reflect, Loop::Evolve] {
        let before = fx.engine.visibility_folds();
        let answer = fx
            .engine
            .automation_status_with(lp, &[], &runner, false, true)
            .await
            .unwrap();
        let domains = answer["domains"].as_array().unwrap();
        assert!(domains.len() >= 2, "{lp:?}: {answer}");
        assert_eq!(
            fx.engine.visibility_folds(),
            before + 1,
            "{lp:?} folds once: {answer}"
        );
    }
}

/// `eng` (nothing else due) with the page `w.md` the agent wrote from the
/// one engram, with a freshness of one day and its reconcile recorded at `at`.
async fn rolling_page(at: &str) -> Fx {
    let store = TursoStore::open_in_memory().await.unwrap();
    rolling_page_on(Arc::new(Mutex::new(store)), at).await
}

/// [`rolling_page`] on `store`.
async fn rolling_page_on(store: Arc<Mutex<dyn Store>>, at: &str) -> Fx {
    rolling_page_every(store, "1d", at).await
}

/// [`rolling_page_on`] with the freshness `every`.
async fn rolling_page_every(store: Arc<Mutex<dyn Store>>, every: &str, at: &str) -> Fx {
    let files = files();
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
    let fx = fixture_on(store, &borrowed, |_, _| {}, None, None, None).await;
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "w.md".into(),
                markdown: Some(format!(
                    "---\nfreshness: {every}\n---\n# Week\n\n## Now\n\nText.\n"
                )),
                sources: std::collections::BTreeMap::from([
                    ("_top".to_string(), Vec::new()),
                    ("Now".to_string(), vec!["notes/deploys".to_string()]),
                ]),
                ..Default::default()
            },
            &ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false),
        )
        .await
        .unwrap();
    reconciled_at(&fx, "w.md", at).await;
    fx
}

/// Record a reconcile of `path` at `at` exactly.
async fn reconciled_at(fx: &Fx, path: &str, at: &str) {
    let store = fx.engine.store();
    let store = store.lock().await;
    let id = store.domain_id("eng").await.unwrap().unwrap();
    store
        .set_reflection_reconcile(
            id,
            &crystalline_index::ReflectionReconcile {
                path: path.to_string(),
                last_reconciled: at.to_string(),
                outcome: "unchanged".to_string(),
                pact_hash: String::new(),
                version: String::new(),
            },
        )
        .await
        .unwrap();
}

fn now_at(engine: &Engine, at: &str) {
    engine.set_automation_now(Some(time::parse_rfc3339(at).unwrap()));
}

/// A page whose freshness ran out is due work for a run, counted on its own
/// (`stale`, not among the `updates`), and the status names when the next
/// page falls due, with counts and without (the cheap poll a runner makes
/// to know when to come back), from the automation clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_page_is_due_work_and_the_status_says_when_the_next_is() {
    let fx = rolling_page("2026-10-21T10:00:00+00:00").await;

    now_at(&fx.engine, "2026-10-21T12:00:00+00:00");
    for counts in [true, false] {
        let fresh = status_with(&fx.engine, Loop::Reflect, counts).await;
        let entry = &fresh["domains"][0];
        assert_eq!(entry["next_stale_at"], "2026-10-22T10:00:00Z", "{fresh}");
        assert_eq!(entry["next_stale_in_secs"], 22 * 3600, "{fresh}");
        if counts {
            assert_eq!(entry["due"], false, "{fresh}");
            assert_eq!(entry["counts"]["stale"], 0, "{fresh}");
        }
    }

    now_at(&fx.engine, "2026-10-22T11:00:00+00:00");
    let before = fx.engine.due_detections();
    let cheap = status_with(&fx.engine, Loop::Reflect, false).await;
    assert_eq!(fx.engine.due_detections(), before, "no detection: {cheap}");
    assert_eq!(
        cheap["domains"][0]["next_stale_in_secs"],
        23 * 3600,
        "a due page names the start of its next period: {cheap}"
    );
    let due = status_with(&fx.engine, Loop::Reflect, true).await;
    let entry = &due["domains"][0];
    assert_eq!(entry["due"], true, "{due}");
    assert_eq!(entry["counts"]["stale"], 1, "{due}");
    assert_eq!(
        entry["counts"]["updates"], 0,
        "stale is its own count: {due}"
    );
    assert_eq!(entry["counts"]["pages_to_update"], 0, "{due}");
    assert_eq!(entry["next_stale_at"], "2026-10-23T10:00:00Z", "{due}");
}

/// A page a person marked not fresh is due work at once, counted under
/// `stale` (not among the `updates`), whatever its freshness says; the next
/// stale time stays the freshness's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_marked_page_is_stale_work_and_the_next_time_stays() {
    let fx = rolling_page("2026-10-21T10:00:00+00:00").await;
    now_at(&fx.engine, "2026-10-21T12:00:00+00:00");
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "plain.md".into(),
                markdown: Some("# Plain\n\n## Now\n\nText.\n".into()),
                ..Default::default()
            },
            &ReflectionCaller::person(Scope::Unrestricted, "jordi"),
        )
        .await
        .unwrap();
    let before = status_with(&fx.engine, Loop::Reflect, true).await;
    let updates = before["domains"][0]["counts"]["updates"].clone();
    for path in ["w.md", "plain.md"] {
        fx.engine
            .write_reflection(
                &WriteReflectionParams {
                    domain: "eng".into(),
                    path: path.into(),
                    action: Some("mark_stale".into()),
                    ..Default::default()
                },
                &ReflectionCaller::person(Scope::Unrestricted, "jordi"),
            )
            .await
            .unwrap();
    }
    let due = status_with(&fx.engine, Loop::Reflect, true).await;
    let entry = &due["domains"][0];
    assert_eq!(entry["due"], true, "{due}");
    assert_eq!(entry["counts"]["stale"], 2, "{due}");
    assert_eq!(entry["counts"]["updates"], updates, "{due}");
    assert_eq!(entry["counts"]["pages_to_update"], 0, "{due}");
    assert_eq!(entry["next_stale_at"], "2026-10-22T10:00:00Z", "{due}");
}

/// A retired page and a page without a freshness are never due and name
/// no next time; with no page that keeps itself current both are null.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_rolling_page_there_is_no_next_stale_time() {
    let fx = rolling_page("2026-10-21T10:00:00+00:00").await;
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "w.md".into(),
                action: Some("retire".into()),
                ..Default::default()
            },
            &ReflectionCaller::person(Scope::Unrestricted, "jordi"),
        )
        .await
        .unwrap();
    now_at(&fx.engine, "2026-10-25T10:00:00+00:00");
    for counts in [true, false] {
        let answer = status_with(&fx.engine, Loop::Reflect, counts).await;
        let entry = &answer["domains"][0];
        assert_eq!(entry["next_stale_at"], Value::Null, "{answer}");
        assert_eq!(entry["next_stale_in_secs"], Value::Null, "{answer}");
        if counts {
            // The engram the retired page cited is unreflected now; that
            // is an update, never a stale page.
            assert_eq!(entry["counts"]["stale"], 0, "{answer}");
        }
    }
}

fn jordi() -> ReflectionCaller {
    ReflectionCaller::person(Scope::Unrestricted, "jordi")
}

/// A page's clock on the automation clock, in relative seconds (design
/// 2.2): fresh with the time until it falls due, due, then overdue on the
/// page one whole period after it fell due. With no run known nothing
/// missed it: it is in no list of missed pages, and there is no hint about
/// the loop. A reconcile time ahead of the clock (a host takeover) reads as
/// just checked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_is_overdue_one_whole_period_after_it_fell_due() {
    let fx = rolling_page("2026-10-21T10:00:00+00:00").await;

    now_at(&fx.engine, "2026-10-21T12:00:00+00:00");
    let fresh = clock_of(&fx).await;
    assert_eq!(fresh["freshness"], "1d", "{fresh}");
    assert_eq!(fresh["last_checked_secs"], 2 * 3600, "{fresh}");
    assert_eq!(fresh["due_in_secs"], 22 * 3600, "{fresh}");
    assert_eq!(fresh["stale"], false, "{fresh}");
    assert_eq!(fresh["overdue_secs"], Value::Null, "{fresh}");
    assert_eq!(fresh["waiting"], false, "{fresh}");

    now_at(&fx.engine, "2026-10-22T11:00:00+00:00");
    let due = clock_of(&fx).await;
    assert_eq!(due["stale"], true, "{due}");
    assert_eq!(due["due_in_secs"], Value::Null, "{due}");
    assert_eq!(due["overdue_secs"], Value::Null, "due, not overdue: {due}");

    now_at(&fx.engine, "2026-10-23T13:00:00+00:00");
    let late = clock_of(&fx).await;
    assert_eq!(late["last_checked_secs"], 51 * 3600, "{late}");
    assert_eq!(late["overdue_secs"], 3 * 3600, "{late}");
    assert_eq!(late["missed_secs"], Value::Null, "no run: {late}");
    assert_eq!(late["loop_slower_than_freshness"], false, "{late}");
    assert_eq!(missed(&fx).await, serde_json::json!([]));

    now_at(&fx.engine, "2026-10-21T09:00:00+00:00");
    let behind = clock_of(&fx).await;
    assert_eq!(behind["last_checked_secs"], 0, "{behind}");
    assert_eq!(behind["due_in_secs"], 25 * 3600, "{behind}");
}

async fn clock_of(fx: &Fx) -> Value {
    fx.engine
        .reflection_page_clock("eng", "w.md", &jordi())
        .await
        .unwrap()
}

/// The pages a reflect run missed, as the admin banner and doctor read them.
async fn missed(fx: &Fx) -> Value {
    fx.engine.overdue_reflection_pages(&jordi()).await.unwrap()["pages"].clone()
}

/// A reflect run of `eng` from `from` to `to` on the automation clock that
/// stopped at its cap.
async fn reflect_run(fx: &Fx, from: &str, to: &str) {
    reflect_run_said(fx, from, to, "").await;
}

/// [`reflect_run`] whose runner reports `said` as its start, by its own
/// clock (empty: it reports none).
async fn reflect_run_said(fx: &Fx, from: &str, to: &str, said: &str) {
    now_at(&fx.engine, from);
    let taken = fx
        .engine
        .automation_lease_take(Loop::Reflect, "eng", "cron", &Scope::Unrestricted)
        .await
        .unwrap();
    now_at(&fx.engine, to);
    fx.engine
        .automation_run_record(
            Loop::Reflect,
            "eng",
            taken["token"].as_str().unwrap(),
            &crystalline_core::automation::RunReport {
                started_at: said.to_string(),
                outcome: crystalline_core::automation::RunOutcome::StoppedAtCap,
                done: 20,
                summary: "capped".into(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
}

async fn hourly_page(at: &str) -> Fx {
    let store = TursoStore::open_in_memory().await.unwrap();
    rolling_page_every(Arc::new(Mutex::new(store)), "1h", at).await
}

/// A 1h page under a daily run that reconciles it is overdue on its page,
/// with the hint that the loop runs less often than its freshness, and a
/// run never missed it: it is in no list of missed pages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daily_run_that_reconciles_an_hourly_page_only_hints_at_the_loop() {
    let fx = hourly_page("2026-10-21T10:00:30+00:00").await;
    reflect_run(
        &fx,
        "2026-10-21T10:00:00+00:00",
        "2026-10-21T10:01:00+00:00",
    )
    .await;
    now_at(&fx.engine, "2026-10-21T23:00:00+00:00");
    let clock = clock_of(&fx).await;
    assert_eq!(clock["overdue_secs"], 11 * 3600 - 30, "{clock}");
    assert_eq!(clock["missed_secs"], Value::Null, "{clock}");
    assert_eq!(clock["loop_slower_than_freshness"], true, "{clock}");
    assert_eq!(missed(&fx).await, serde_json::json!([]));

    // The next day's run reconciles it again.
    reflect_run(
        &fx,
        "2026-10-22T10:00:00+00:00",
        "2026-10-22T10:01:00+00:00",
    )
    .await;
    reconciled_at(&fx, "w.md", "2026-10-22T10:00:30+00:00").await;
    now_at(&fx.engine, "2026-10-22T20:00:00+00:00");
    let clock = clock_of(&fx).await;
    assert_eq!(clock["loop_slower_than_freshness"], true, "{clock}");
    assert_eq!(clock["missed_secs"], Value::Null, "{clock}");
    assert_eq!(missed(&fx).await, serde_json::json!([]));
}

/// A run that started after the page fell due and ended without
/// reconciling it (it stopped at its cap) missed the page from that run's
/// end: in the list with its domain and since when, without the hint. A
/// run still going does not count.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_that_skipped_a_due_page_missed_it_from_its_end() {
    let fx = hourly_page("2026-10-21T10:00:00+00:00").await;
    // A run going now, past the time the page fell due: not missed yet.
    now_at(&fx.engine, "2026-10-21T12:00:00+00:00");
    let taken = fx
        .engine
        .automation_lease_take(Loop::Reflect, "eng", "cron", &Scope::Unrestricted)
        .await
        .unwrap();
    now_at(&fx.engine, "2026-10-21T12:00:30+00:00");
    assert_eq!(missed(&fx).await, serde_json::json!([]), "a running run");
    fx.engine
        .automation_lease_release(
            Loop::Reflect,
            "eng",
            taken["token"].as_str().unwrap(),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();

    reflect_run(
        &fx,
        "2026-10-21T13:00:00+00:00",
        "2026-10-21T13:05:00+00:00",
    )
    .await;
    now_at(&fx.engine, "2026-10-21T16:05:00+00:00");
    let clock = clock_of(&fx).await;
    assert_eq!(clock["missed_secs"], 3 * 3600, "{clock}");
    assert_eq!(clock["overdue_secs"], 4 * 3600 + 300, "{clock}");
    assert_eq!(clock["loop_slower_than_freshness"], false, "{clock}");
    assert_eq!(
        missed(&fx).await,
        serde_json::json!([{
            "domain": "eng",
            "path": "w.md",
            "freshness": "1h",
            "last_checked_secs": 6 * 3600 + 300,
            "missed_secs": 3 * 3600,
            "waiting": false,
        }])
    );
}

/// Whether a run missed a page is decided on the server's clock: a runner
/// whose clock is behind reports a start before the page fell due, and the
/// run still missed the page, because the lease was taken after.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_missed_a_page_by_the_servers_clock_not_the_runners() {
    let fx = hourly_page("2026-10-21T10:00:00+00:00").await;
    reflect_run_said(
        &fx,
        "2026-10-21T13:00:00+00:00",
        "2026-10-21T13:05:00+00:00",
        "2026-10-21T10:30:00+00:00",
    )
    .await;
    now_at(&fx.engine, "2026-10-21T16:05:00+00:00");
    let clock = clock_of(&fx).await;
    assert_eq!(clock["missed_secs"], 3 * 3600, "{clock}");
    assert_eq!(missed(&fx).await.as_array().map(Vec::len), Some(1));
    let runs = fx
        .engine
        .automation_status(Loop::Reflect, &["eng".to_string()], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        runs["domains"][0]["last_run"]["started_at"], "2026-10-21T13:00:00+00:00",
        "the run started when its lease was taken: {runs}"
    );
}

/// A retired page has no clock and is never in the list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retired_page_has_no_clock() {
    let fx = hourly_page("2026-10-21T10:00:00+00:00").await;
    reflect_run(
        &fx,
        "2026-10-21T13:00:00+00:00",
        "2026-10-21T13:05:00+00:00",
    )
    .await;
    now_at(&fx.engine, "2026-10-25T10:00:00+00:00");
    assert!(missed(&fx).await.as_array().unwrap().len() == 1);
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "w.md".into(),
                action: Some("retire".into()),
                ..Default::default()
            },
            &jordi(),
        )
        .await
        .unwrap();
    assert_eq!(clock_of(&fx).await, Value::Null);
    assert_eq!(missed(&fx).await, serde_json::json!([]));
}

/// A page without a freshness has no clock and is never in the list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_without_a_freshness_has_no_clock() {
    let fx = hourly_page("2026-10-21T10:00:00+00:00").await;
    let text = "# Plain\n\n## Now\n\nText.\n";
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "plain.md".into(),
                markdown: Some(text.into()),
                sources: std::collections::BTreeMap::from([("_top".to_string(), Vec::new())]),
                ..Default::default()
            },
            &ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false),
        )
        .await
        .unwrap();
    reconciled_at(&fx, "plain.md", "2026-10-21T10:00:00+00:00").await;
    reflect_run(
        &fx,
        "2026-10-21T13:00:00+00:00",
        "2026-10-21T13:05:00+00:00",
    )
    .await;
    now_at(&fx.engine, "2026-10-25T10:00:00+00:00");
    let clock = fx
        .engine
        .reflection_page_clock("eng", "plain.md", &jordi())
        .await
        .unwrap();
    assert_eq!(clock, Value::Null);
    let pages = missed(&fx).await;
    assert!(
        pages
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["path"] != "plain.md"),
        "{pages}"
    );
}

/// `crystalline doctor` reads the page clocks from an index it opened
/// itself (no daemon): the same clock the engine answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_page_clocks_read_from_a_store_match_the_engine() {
    let fx = rolling_page("2026-10-21T10:00:00+00:00").await;
    let now = time::parse_rfc3339("2026-10-23T13:00:00+00:00").unwrap();
    now_at(&fx.engine, "2026-10-23T13:00:00+00:00");
    let store = fx.engine.store();
    let store = store.lock().await;
    let id = store.domain_id("eng").await.unwrap().unwrap();
    let clocks =
        crystalline_service::engine::page_clocks_in(&*store, id, now.stamp().unix_seconds())
            .await
            .unwrap();
    assert_eq!(clocks.len(), 1, "{clocks:?}");
    assert_eq!(clocks[0].path, "w.md");
    assert_eq!(clocks[0].freshness.written, "1d");
    assert_eq!(clocks[0].timing.overdue_secs, Some(3 * 3600));
    assert_eq!(clocks[0].timing.missed_secs, None, "no run");
    assert_eq!(clocks[0].timing.last_checked_secs, 51 * 3600);
}

/// The control socket answers the pages a reflect run missed to
/// `crystalline doctor` on the machine owner's behalf.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_control_socket_names_the_overdue_pages() {
    let fx = hourly_page("2026-10-21T10:00:00+00:00").await;
    reflect_run(
        &fx,
        "2026-10-21T13:00:00+00:00",
        "2026-10-21T13:05:00+00:00",
    )
    .await;
    now_at(&fx.engine, "2026-10-21T16:05:00+00:00");
    let answer = crystalline_service::control::answer_for_test(
        fx.engine.clone(),
        &serde_json::json!({ "v": 1, "cmd": "reflection_overdue" }),
    )
    .await;
    assert_eq!(answer["ok"], true, "{answer}");
    let pages = &answer["data"]["pages"];
    assert_eq!(pages[0]["domain"], "eng", "{answer}");
    assert_eq!(pages[0]["path"], "w.md", "{answer}");
    assert_eq!(pages[0]["missed_secs"], 3 * 3600, "{answer}");
}

/// The `reconciled` action reads the same clock as the queue: a page due on
/// the automation clock needs the stale item's version.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconcile_reads_the_automation_clock() {
    let now = time::now();
    let fx = rolling_page(&now.rfc3339()).await;
    fx.engine.set_automation_now(Some(
        now.saturating_add(time::saturating_seconds(2 * 86_400))
            .to_offset_utc(),
    ));
    let refused = fx
        .engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "w.md".into(),
                action: Some("reconciled".into()),
                outcome: Some("unchanged".into()),
                ..Default::default()
            },
            &ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false),
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("stale item"), "{refused}");
}

/// The page clocks read through the read connections: on a turso file the
/// first status (nothing remembered of the pages yet, so their text is
/// read) answers `next_stale_in_secs` while a test holds the writer lock,
/// with counts and without.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_next_stale_time_never_waits_for_the_writer_lock() {
    let dir = tempfile::tempdir().unwrap();
    let store = TursoStore::open(&dir.path().join("index.db"))
        .await
        .unwrap();
    let fx = rolling_page_on(Arc::new(Mutex::new(store)), "2026-10-21T10:00:00+00:00").await;
    now_at(&fx.engine, "2026-10-21T12:00:00+00:00");
    let store = fx.engine.store();
    let held = store.lock().await;
    for counts in [false, true] {
        let answer = tokio::time::timeout(PATIENCE, status_with(&fx.engine, Loop::Reflect, counts))
            .await
            .unwrap_or_else(|_| panic!("counts {counts}: the status waited for the writer lock"));
        assert_eq!(
            answer["domains"][0]["next_stale_in_secs"],
            22 * 3600,
            "{answer}"
        );
    }
    drop(held);
}

/// The leases an MCP call's agent gate reads (`run_leases`, asked for every
/// tool call, resource read and prompt) come through the read connections:
/// on a turso file they answer while a test holds the writer lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_run_leases_never_wait_for_the_writer_lock() {
    let dir = tempfile::tempdir().unwrap();
    let store = TursoStore::open(&dir.path().join("index.db"))
        .await
        .unwrap();
    let fx = rolling_page_on(Arc::new(Mutex::new(store)), "2026-10-21T10:00:00+00:00").await;
    now_at(&fx.engine, "2026-10-21T12:00:00+00:00");
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cron", &Scope::Unrestricted)
        .await
        .unwrap();
    let store = fx.engine.store();
    let held = store.lock().await;
    let leases = tokio::time::timeout(PATIENCE, fx.engine.run_leases(&Scope::Unrestricted))
        .await
        .expect("the run leases waited for the writer lock")
        .unwrap();
    assert_eq!(leases, [(Loop::Reflect, "eng".to_string())]);
    drop(held);
}

/// The page clocks read the page texts in one query when nothing is
/// remembered (the first status after a start), and then only the text of
/// a page whose checksum moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cold_page_memo_reads_the_texts_in_one_query() {
    let fx = rolling_page("2026-10-21T10:00:00+00:00").await;
    for n in 0..12 {
        fx.engine
            .write_reflection(
                &WriteReflectionParams {
                    domain: "eng".into(),
                    path: format!("p{n}.md"),
                    markdown: Some("---\nfreshness: 1d\n---\n# P\n".into()),
                    sources: std::collections::BTreeMap::from([("_top".to_string(), Vec::new())]),
                    ..Default::default()
                },
                &ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false),
            )
            .await
            .unwrap();
    }
    now_at(&fx.engine, "2026-10-21T12:00:00+00:00");
    let before = fx.engine.page_text_reads();
    status_with(&fx.engine, Loop::Reflect, false).await;
    assert_eq!(fx.engine.page_text_reads(), before + 1, "one bulk read");
    status_with(&fx.engine, Loop::Reflect, false).await;
    assert_eq!(fx.engine.page_text_reads(), before + 1, "all remembered");
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "p3.md".into(),
                markdown: Some("---\nfreshness: 2d\n---\n# P\n".into()),
                sources: std::collections::BTreeMap::from([("_top".to_string(), Vec::new())]),
                ..Default::default()
            },
            &ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false),
        )
        .await
        .unwrap();
    status_with(&fx.engine, Loop::Reflect, false).await;
    assert_eq!(fx.engine.page_text_reads(), before + 2, "one page read");
}

/// A caller who may not read the Reflection gets no page clock (refused,
/// as a read of the page is) and no missed page of that domain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_who_may_not_read_gets_no_clock() {
    let fx = hourly_page("2026-10-21T10:00:00+00:00").await;
    reflect_run(
        &fx,
        "2026-10-21T13:00:00+00:00",
        "2026-10-21T13:05:00+00:00",
    )
    .await;
    now_at(&fx.engine, "2026-10-21T16:05:00+00:00");
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["keeper", "out"] {
        auth.add_user(name, name, None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("eng", true, "keeper")
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let out = ReflectionCaller::person(
        Scope::User {
            account: "out".into(),
            admin: false,
        },
        "out",
    );
    assert!(
        fx.engine
            .reflection_page_clock("eng", "w.md", &out)
            .await
            .is_err()
    );
    let pages = fx.engine.overdue_reflection_pages(&out).await.unwrap();
    assert_eq!(pages["pages"], serde_json::json!([]), "{pages}");
    // The machine owner still sees it.
    assert_eq!(missed(&fx).await.as_array().unwrap().len(), 1);
}
