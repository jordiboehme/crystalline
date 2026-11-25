//! Marking a Reflection page not fresh (0.37.0): who may mark, which pages
//! a mark reaches, what it writes and what it leaves alone, and how the
//! next reflect run reconciles the page and takes the mark off.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crystalline_core::reflection as r;
use crystalline_service::params::{MarkPages, MarkStale, ReflectParams, WriteReflectionParams};
use crystalline_service::read_limit::within;
use crystalline_service::rest::{AuthStore, ReflectionLevel, Role};
use crystalline_service::{DomainAccess, EngineError, ReflectionCaller, Scope};
use serde_json::Value;

use crate::docs::{agent, person, reflecting, write};
use crate::fixture::{Fx, fixture, manifest};

const PAGE: &str = "# A\n\n## One\n\nFirst.\n";

fn file(fx: &Fx, rel: &str) -> String {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).unwrap()
}

fn record(fx: &Fx, rel: &str) -> r::PageRecord {
    r::read_provenance(&file(fx, rel)).page
}

async fn put(fx: &Fx, path: &str, text: &str) {
    fx.engine
        .write_reflection(&write(path, text), &person())
        .await
        .unwrap();
}

/// An agent's create of `path` owned by `owner`.
async fn owned(fx: &Fx, path: &str, owner: &str) {
    let mut p = write(path, PAGE);
    p.owner = Some(owner.into());
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
}

fn act(action: &str, path: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        domain: "eng".into(),
        action: Some(action.into()),
        path: path.into(),
        ..Default::default()
    }
}

fn one(path: &str) -> MarkStale {
    MarkStale {
        pages: MarkPages::One(path.into()),
        only_page: false,
    }
}

async fn mark(fx: &Fx, m: &MarkStale, who: &ReflectionCaller) -> Result<Value, EngineError> {
    fx.engine.mark_reflection_stale("eng", m, who).await
}

async fn page_of(fx: &Fx, path: &str) -> Value {
    fx.engine
        .read_reflection("eng", path, &person())
        .await
        .unwrap()["page"]
        .clone()
}

/// The queue's items without their ages, which move with the clock.
fn ageless(q: &Value) -> Vec<Value> {
    q["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            let mut i = i.clone();
            i.as_object_mut().unwrap().remove("age_secs");
            i
        })
        .collect()
}

fn pages(answer: &Value) -> Vec<String> {
    answer["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect()
}

/// A mark writes `refresh_requested` (milliseconds, `Z`) and
/// `refresh_requested_by` into the page record and answers the page; the
/// page view and the mark beside the clock show it, even on a page never
/// reconciled and with no freshness; the file keeps its modified time.
#[tokio::test]
async fn a_mark_is_written_into_the_page_record_and_shown() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    let abs = fx.root.join("reflections/guides/a.md");
    let old = SystemTime::now() - Duration::from_secs(3 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&abs)
        .unwrap()
        .set_modified(old)
        .unwrap();

    let answer = mark(&fx, &one("guides/a.md"), &person()).await.unwrap();
    assert_eq!(answer["domain"], "eng");
    assert_eq!(answer["by"], "jordi");
    assert_eq!(answer["only_page"], false);
    assert_eq!(pages(&answer), ["guides/a.md"]);
    let at = answer["at"].as_str().unwrap();
    assert_eq!(at.len(), "2026-11-25T14:03:07.412Z".len(), "{at}");
    assert!(at.ends_with('Z'), "{at}");

    let rec = record(&fx, "guides/a.md");
    assert_eq!(rec.refresh_requested.as_deref(), Some(at));
    assert_eq!(rec.refresh_requested_by.as_deref(), Some("jordi"));
    assert!(file(&fx, "guides/a.md").contains("## One\n"), "text stays");
    assert_eq!(
        std::fs::metadata(&abs).unwrap().modified().unwrap(),
        old,
        "the file keeps its modified time"
    );

    let view = page_of(&fx, "guides/a.md").await;
    assert_eq!(view["refresh_requested"]["at"], at, "{view}");
    assert_eq!(view["refresh_requested"]["by"], "jordi");
    let beside_clock = fx
        .engine
        .reflection_page_mark("eng", "guides/a.md", &person())
        .await
        .unwrap();
    assert_eq!(beside_clock, view["refresh_requested"]);
    // A page with no mark shows null.
    put(&fx, "guides/b.md", PAGE).await;
    assert!(page_of(&fx, "guides/b.md").await["refresh_requested"].is_null());
}

/// Without `only_page` the pages a named page owns are marked too,
/// recursively, all with one time; a retired owned page is left out. With
/// it only the named page is marked.
#[tokio::test]
async fn a_mark_reaches_the_owned_pages_unless_only_the_page_is_asked() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    owned(&fx, "arch/db.md", "arch.md").await;
    owned(&fx, "arch/db/schema.md", "arch/db.md").await;
    owned(&fx, "arch/old.md", "arch.md").await;
    fx.engine
        .write_reflection(&act("retire", "arch/old.md"), &agent())
        .await
        .unwrap();

    let answer = mark(&fx, &one("arch.md"), &person()).await.unwrap();
    let mut marked = pages(&answer);
    assert_eq!(marked[0], "arch.md", "the named page first");
    marked.sort();
    assert_eq!(marked, ["arch.md", "arch/db.md", "arch/db/schema.md"]);
    let at = answer["at"].as_str().unwrap();
    for page in ["arch.md", "arch/db.md", "arch/db/schema.md"] {
        assert_eq!(record(&fx, page).refresh_requested.as_deref(), Some(at));
    }
    assert_eq!(record(&fx, "arch/old.md").refresh_requested, None);

    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    owned(&fx, "arch/db.md", "arch.md").await;
    let answer = mark(
        &fx,
        &MarkStale {
            pages: MarkPages::One("arch.md".into()),
            only_page: true,
        },
        &person(),
    )
    .await
    .unwrap();
    assert_eq!(pages(&answer), ["arch.md"]);
    assert_eq!(answer["only_page"], true);
    assert_eq!(record(&fx, "arch/db.md").refresh_requested, None);
}

/// `All` marks every page that is not retired, in path order, and no pact
/// file; a Reflection with no page answers no page. `Many` drops repeats.
#[tokio::test]
async fn all_marks_every_live_page_and_many_drops_repeats() {
    let fx = reflecting().await;
    let answer = fx
        .engine
        .mark_reflection_stale(
            "eng",
            &MarkStale {
                pages: MarkPages::All,
                only_page: false,
            },
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(pages(&answer), Vec::<String>::new());

    put(&fx, "PACT.md", "# Pact\n\n## Tone\n\nPlain.\n").await;
    put(&fx, "b.md", PAGE).await;
    put(&fx, "a.md", PAGE).await;
    put(&fx, "a.pact.md", "# Pact of a\n\n## Tone\n\nShort.\n").await;
    put(&fx, "gone.md", PAGE).await;
    fx.engine
        .write_reflection(&act("retire", "gone.md"), &agent())
        .await
        .unwrap();
    let answer = mark(
        &fx,
        &MarkStale {
            pages: MarkPages::All,
            only_page: true,
        },
        &person(),
    )
    .await
    .unwrap();
    assert_eq!(pages(&answer), ["a.md", "b.md"]);
    assert!(!file(&fx, "PACT.md").contains("refresh_requested"));
    assert!(!file(&fx, "a.pact.md").contains("refresh_requested"));

    let answer = mark(
        &fx,
        &MarkStale {
            pages: MarkPages::Many(vec!["b.md".into(), "a.md".into(), "b.md".into()]),
            only_page: false,
        },
        &person(),
    )
    .await
    .unwrap();
    assert_eq!(pages(&answer), ["b.md", "a.md"]);
}

/// A re-mark moves the time forward, also within one millisecond: the new
/// time is later than every mark it replaces.
#[tokio::test]
async fn a_re_mark_always_moves_the_time_forward() {
    let fx = reflecting().await;
    put(&fx, "a.md", PAGE).await;
    // A mark from the future (another instance's clock): the re-mark still
    // comes after it.
    let ahead = "2099-01-01T00:00:00.000Z";
    let text = file(&fx, "a.md");
    let mut prov = r::read_provenance(&text);
    prov.page.set_mark(ahead.into(), "ada".into());
    std::fs::write(
        fx.root.join("reflections/a.md"),
        r::write_provenance(&text, &prov).unwrap(),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let answer = mark(&fx, &one("a.md"), &person()).await.unwrap();
    assert_eq!(answer["at"], "2099-01-01T00:00:00.001Z");
    let again = mark(&fx, &one("a.md"), &person()).await.unwrap();
    assert_eq!(again["at"], "2099-01-01T00:00:00.002Z");
    assert_eq!(
        record(&fx, "a.md").refresh_requested_by.as_deref(),
        Some("jordi")
    );
}

/// A pact file, a missing page, a retired page and an empty name are
/// refused before anything is written, with their own sentences.
#[tokio::test]
async fn what_is_no_live_page_is_refused_and_nothing_is_written() {
    let fx = reflecting().await;
    put(&fx, "PACT.md", "# Pact\n\n## Tone\n\nPlain.\n").await;
    put(&fx, "a.md", PAGE).await;
    put(&fx, "old.md", PAGE).await;
    fx.engine
        .write_reflection(&act("retire", "old.md"), &agent())
        .await
        .unwrap();
    let many = |paths: &[&str]| MarkStale {
        pages: MarkPages::Many(paths.iter().map(|p| p.to_string()).collect()),
        only_page: false,
    };

    let e = mark(&fx, &many(&["a.md", "PACT.md"]), &person())
        .await
        .unwrap_err();
    assert!(matches!(e, EngineError::Invalid(_)), "{e}");
    assert_eq!(
        e.to_string(),
        "'PACT.md' is a pact file, no page: only a page is marked not fresh"
    );
    let e = mark(&fx, &many(&["a.md", "gone.md"]), &person())
        .await
        .unwrap_err();
    assert!(matches!(e, EngineError::NotFound(_)), "{e}");
    let e = mark(&fx, &many(&["a.md", "old.md"]), &person())
        .await
        .unwrap_err();
    assert!(matches!(e, EngineError::Invalid(_)), "{e}");
    assert_eq!(
        e.to_string(),
        "'old.md' is retired and is never reconciled; restore it first (write_reflection action=restore) to mark it not fresh"
    );
    for nothing in [many(&[]), one("  ")] {
        let e = mark(&fx, &nothing, &person()).await.unwrap_err();
        assert_eq!(
            e.to_string(),
            "name the pages to mark: path, pages or all: true"
        );
    }
    assert!(
        !file(&fx, "a.md").contains("refresh_requested"),
        "nothing written"
    );
}

/// A contributor marks (edit is enough); a reader does not, nor an editor
/// off the write list, nor anyone in a read-only domain.
#[tokio::test]
async fn edit_is_needed_and_the_write_list_and_read_only_hold() {
    let (fx, ann, _) = crate::page_rights::editor_and_curator().await;
    put(&fx, "a.md", PAGE).await;
    mark(&fx, &one("a.md"), &ann).await.unwrap();
    assert_eq!(
        record(&fx, "a.md").refresh_requested_by.as_deref(),
        Some("ann")
    );

    let fx = reflecting().await;
    put(&fx, "a.md", PAGE).await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["rita", "lisa"] {
        auth.add_user(name, name, None, Role::None, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_reflection_level("rita", ReflectionLevel::Reader)
        .await
        .unwrap();
    auth.set_reflection_level("lisa", ReflectionLevel::Contributor)
        .await
        .unwrap();
    auth.set_write_list("lisa", &["agents-*".to_string()])
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let user = |name: &str| {
        ReflectionCaller::person(
            Scope::User {
                account: name.into(),
                admin: false,
            },
            name,
        )
    };
    for who in ["rita", "lisa"] {
        let e = mark(&fx, &one("a.md"), &user(who)).await.unwrap_err();
        assert!(matches!(e, EngineError::Forbidden(_)), "{who}: {e}");
    }
    assert!(!file(&fx, "a.md").contains("refresh_requested"));

    let fx = fixture(&[("MANIFEST.md", &manifest("reflection: on\nread_only: on\n"))]).await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(fx.root.join("reflections/a.md"), PAGE).unwrap();
    fx.engine.sync(None).await.unwrap();
    let e = mark(&fx, &one("a.md"), &person()).await.unwrap_err();
    assert!(matches!(e, EngineError::DomainReadOnly(_)), "{e}");
}

/// The mark changes no section and no pact hash: a reconciled page raises
/// no `page_pact_changed` and no feedback item once it is marked, a
/// whole-page write keeps the mark, and so does a move.
#[tokio::test]
async fn a_mark_raises_nothing_and_survives_a_write_and_a_move() {
    let fx = reflecting().await;
    put(&fx, "PACT.md", "# Pact\n\n## Tone\n\nPlain.\n").await;
    put(&fx, "a.md", PAGE).await;
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                outcome: Some("updated".into()),
                ..act("reconciled", "a.md")
            },
            &agent(),
        )
        .await
        .unwrap();
    let before = fx
        .engine
        .reflect(
            &ReflectParams {
                domains: vec!["eng".into()],
                limit: Some(100),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let stored_hash = fx
        .engine
        .reflection_reconcile("eng", "a.md")
        .await
        .unwrap()
        .unwrap()
        .pact_hash;
    let answer = mark(&fx, &one("a.md"), &person()).await.unwrap();
    let at = answer["at"].as_str().unwrap().to_string();
    let after = fx
        .engine
        .reflect(
            &ReflectParams {
                domains: vec!["eng".into()],
                limit: Some(100),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(ageless(&after), ageless(&before), "no new item: {after}");
    let state = fx
        .engine
        .reflection_reconcile("eng", "a.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.pact_hash, stored_hash);
    let fresh = r::page_pact_hash(
        "a.md",
        [
            ("PACT.md", file(&fx, "PACT.md").as_str()),
            ("a.md", file(&fx, "a.md").as_str()),
        ],
    );
    assert_eq!(state.pact_hash, fresh, "the mark is no input of the hash");

    // A whole-page write by an agent keeps the stored page record.
    fx.engine
        .write_reflection(&write("a.md", "# A\n\n## One\n\nSecond.\n"), &agent())
        .await
        .unwrap();
    assert_eq!(
        record(&fx, "a.md").refresh_requested.as_deref(),
        Some(at.as_str())
    );

    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                to: Some("b.md".into()),
                ..act("move", "a.md")
            },
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(
        record(&fx, "b.md").refresh_requested.as_deref(),
        Some(at.as_str())
    );
}

/// The mark over MCP: one page per call, `only_page` honoured, and an
/// unknown action names `mark_stale` among the actions.
#[tokio::test]
async fn write_reflection_marks_one_page() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    owned(&fx, "arch/db.md", "arch.md").await;
    let answer = fx
        .engine
        .write_reflection(
            &WriteReflectionParams {
                only_page: true,
                ..act("mark_stale", "arch.md")
            },
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(pages(&answer), ["arch.md"]);
    assert_eq!(record(&fx, "arch/db.md").refresh_requested, None);
    let e = fx
        .engine
        .write_reflection(&act("mark", "arch.md"), &person())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("mark_stale"), "{e}");
}

// ---- The queue and the reconcile (M2b) ----

fn eng() -> ReflectParams {
    ReflectParams {
        domains: vec!["eng".into()],
        limit: Some(100),
        ..Default::default()
    }
}

fn with_stale() -> ReflectParams {
    ReflectParams {
        stale: Some(true),
        ..eng()
    }
}

async fn queue(fx: &Fx) -> Value {
    fx.engine.reflect(&with_stale(), &agent()).await.unwrap()
}

fn stale_items(q: &Value) -> Vec<Value> {
    q["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == "stale")
        .cloned()
        .collect()
}

fn stale_of(q: &Value, path: &str) -> Option<Value> {
    stale_items(q).into_iter().find(|i| i["path"] == path)
}

fn reconciled(path: &str, outcome: &str, version: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        outcome: Some(outcome.into()),
        version: Some(version.into()),
        ..act("reconciled", path)
    }
}

async fn reconcile(fx: &Fx, path: &str, version: &str) -> Result<Value, EngineError> {
    fx.engine
        .write_reflection(&reconciled(path, "updated", version), &agent())
        .await
}

const DAY: i64 = 86_400;

/// Move the recorded reconcile of `path` `ago` seconds into the past.
async fn backdate(fx: &Fx, path: &str, ago: i64) {
    let store = fx.engine.store();
    let store = store.lock().await;
    let id = store.domain_id("eng").await.unwrap().unwrap();
    let mut state = store
        .reflection_reconcile(id, path)
        .await
        .unwrap()
        .expect("a recorded reconcile");
    let at = crystalline_core::time::now()
        .saturating_sub(crystalline_core::time::saturating_seconds(ago));
    state.last_reconciled = at.rfc3339();
    store.set_reflection_reconcile(id, &state).await.unwrap();
}

/// A marked page is a `stale` item for a run and on request, never by
/// default for an interactive session; a page never reconciled is due as
/// soon as it is marked. Its item names the mark, has no freshness, and a
/// version with the mark part. The reconcile that answers it takes the
/// mark off the file (keeping the modified time) and the page is no longer
/// due.
#[tokio::test]
async fn a_marked_page_is_stale_work_until_the_reconcile_takes_the_mark_off() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    assert!(stale_of(&queue(&fx).await, "guides/a.md").is_none());
    let answer = mark(&fx, &one("guides/a.md"), &person()).await.unwrap();
    let at = answer["at"].as_str().unwrap().to_string();

    let interactive = fx.engine.reflect(&eng(), &agent()).await.unwrap();
    assert!(stale_items(&interactive).is_empty(), "{interactive}");
    let run = within(
        Some(crate::page_lifecycle::unattended()),
        fx.engine.reflect(&eng(), &agent()),
    )
    .await
    .unwrap();
    assert!(stale_of(&run, "guides/a.md").is_some(), "{run}");

    let q = queue(&fx).await;
    let item = stale_of(&q, "guides/a.md").expect("a stale item");
    assert_eq!(item["refresh_requested"]["at"], at.as_str(), "{item}");
    assert_eq!(item["refresh_requested"]["by"], "jordi");
    assert!(item["freshness"].is_null(), "{item}");
    assert_eq!(item["owned_marked"], serde_json::json!([]));
    let version = item["version"].as_str().unwrap().to_string();
    let parts: Vec<&str> = version.split(':').collect();
    assert_eq!(parts.len(), 4, "{version}");
    let at_ms = crystalline_core::time::parse_rfc3339(&at)
        .unwrap()
        .stamp()
        .unix_millis();
    assert_eq!(parts[2], at_ms.to_string());

    let abs = fx.root.join("reflections/guides/a.md");
    let modified = std::fs::metadata(&abs).unwrap().modified().unwrap();
    let receipt = reconcile(&fx, "guides/a.md", &version).await.unwrap();
    assert_eq!(receipt["still_marked"], serde_json::json!([]), "{receipt}");
    assert!(receipt["refresh_requested"].is_null(), "{receipt}");
    assert!(!file(&fx, "guides/a.md").contains("refresh_requested"));
    assert_eq!(
        std::fs::metadata(&abs).unwrap().modified().unwrap(),
        modified
    );
    assert!(stale_of(&queue(&fx).await, "guides/a.md").is_none());
    assert!(page_of(&fx, "guides/a.md").await["refresh_requested"].is_null());
    assert!(
        fx.engine
            .reflection_page_mark("eng", "guides/a.md", &person())
            .await
            .unwrap()
            .is_null()
    );
}

/// A mark written after the item was handed out survives that item's
/// reconcile, also by an unattended run: the receipt names it in
/// `still_marked`, and the page is due again with a new version.
#[tokio::test]
async fn an_unattended_run_cannot_cover_a_later_mark() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    mark(&fx, &one("guides/a.md"), &person()).await.unwrap();
    let version = stale_of(&queue(&fx).await, "guides/a.md").unwrap()["version"]
        .as_str()
        .unwrap()
        .to_string();
    let again = mark(&fx, &one("guides/a.md"), &person()).await.unwrap();
    let receipt = within(
        Some(crate::page_lifecycle::unattended()),
        fx.engine
            .write_reflection(&reconciled("guides/a.md", "unchanged", &version), &agent()),
    )
    .await
    .unwrap();
    assert_eq!(
        receipt["still_marked"],
        serde_json::json!([{ "path": "guides/a.md", "at": again["at"], "by": "jordi" }]),
        "{receipt}"
    );
    assert_eq!(receipt["refresh_requested"]["at"], again["at"]);
    assert_eq!(
        record(&fx, "guides/a.md").refresh_requested.as_deref(),
        again["at"].as_str()
    );
    let item = stale_of(&queue(&fx).await, "guides/a.md").expect("due again");
    assert_ne!(item["version"].as_str().unwrap(), version);
}

/// A version whose mark part is newer than every mark of the set answers
/// no item the queue handed out: refused, nothing recorded, nothing
/// cleared. A 3-part version clears nothing.
#[tokio::test]
async fn a_made_up_mark_part_is_refused() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    mark(&fx, &one("guides/a.md"), &person()).await.unwrap();
    let version = stale_of(&queue(&fx).await, "guides/a.md").unwrap()["version"]
        .as_str()
        .unwrap()
        .to_string();
    let parts: Vec<&str> = version.split(':').collect();
    let later = parts[2].parse::<i64>().unwrap() + 1;
    let made_up = format!("{}:{}:{later}:{}", parts[0], parts[1], parts[3]);
    let e = reconcile(&fx, "guides/a.md", &made_up).await.unwrap_err();
    assert!(matches!(e, EngineError::Invalid(_)), "{e}");
    assert!(e.to_string().contains("no version the reflect queue handed out"), "{e}");
    assert!(
        fx.engine
            .reflection_reconcile("eng", "guides/a.md")
            .await
            .unwrap()
            .unwrap()
            .outcome
            .is_none(),
        "nothing recorded"
    );
    assert!(file(&fx, "guides/a.md").contains("refresh_requested"));

    // A page with no mark at all refuses any mark part.
    put(&fx, "guides/b.md", PAGE).await;
    let e = reconcile(&fx, "guides/b.md", &made_up).await.unwrap_err();
    assert!(e.to_string().contains("no version the reflect queue handed out"), "{e}");

    // A version without a mark part records and clears nothing.
    let three = format!("{}:{}:{}", parts[0], parts[1], parts[3]);
    let receipt = reconcile(&fx, "guides/a.md", &three).await.unwrap();
    assert_eq!(receipt["still_marked"].as_array().unwrap().len(), 1);
    assert!(file(&fx, "guides/a.md").contains("refresh_requested"));
}

/// A version built from what the page view shows (no recorded reconcile,
/// period 0, the mark as its mark part) with a made-up hash is no version
/// the queue handed out: refused, nothing recorded, the mark stays.
#[tokio::test]
async fn a_version_with_a_made_up_hash_clears_nothing() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    mark(&fx, &one("guides/a.md"), &person()).await.unwrap();
    let at = page_of(&fx, "guides/a.md").await["refresh_requested"]["at"]
        .as_str()
        .unwrap()
        .to_string();
    let at_ms = crystalline_core::time::parse_rfc3339(&at)
        .unwrap()
        .stamp()
        .unix_millis();
    let made_up = format!("0:0:{at_ms}:{}", "0".repeat(64));
    let e = reconcile(&fx, "guides/a.md", &made_up).await.unwrap_err();
    assert!(matches!(e, EngineError::Invalid(_)), "{e}");
    assert!(e.to_string().contains("read the reflect queue again"), "{e}");
    assert!(
        fx.engine
            .reflection_reconcile("eng", "guides/a.md")
            .await
            .unwrap()
            .unwrap()
            .outcome
            .is_none(),
        "nothing recorded"
    );
    assert!(file(&fx, "guides/a.md").contains("refresh_requested"));
    // The version the queue hands out is taken.
    let version = stale_of(&queue(&fx).await, "guides/a.md").unwrap()["version"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(version, made_up);
    reconcile(&fx, "guides/a.md", &version).await.unwrap();
    assert!(!file(&fx, "guides/a.md").contains("refresh_requested"));
}

/// While a marked page has an item of its own, `reconciled` needs its
/// version; an owned page under a due owner records without one, and the
/// owner's record takes the owned pages' marks off.
#[tokio::test]
async fn the_owner_record_takes_the_owned_marks_off() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    owned(&fx, "arch/db.md", "arch.md").await;
    owned(&fx, "arch/api.md", "arch.md").await;
    mark(&fx, &one("arch.md"), &person()).await.unwrap();

    let q = queue(&fx).await;
    assert_eq!(stale_items(&q).len(), 1, "one item for the set: {q}");
    let item = stale_of(&q, "arch.md").unwrap();
    assert_eq!(item["owned_marked"], item["owned"], "{item}");
    assert_eq!(item["owned"].as_array().unwrap().len(), 2, "{item}");
    let version = item["version"].as_str().unwrap().to_string();

    let e = reconcile(&fx, "arch.md", "").await.unwrap_err();
    assert!(e.to_string().contains("marked not fresh"), "{e}");
    let e = reconcile(&fx, "arch.md", &version).await.unwrap_err();
    assert!(
        e.to_string().contains("arch/db.md"),
        "owned pages first: {e}"
    );
    for child in ["arch/db.md", "arch/api.md"] {
        let receipt = reconcile(&fx, child, "").await.unwrap();
        assert_eq!(receipt["still_marked"], serde_json::json!([]));
        assert!(
            file(&fx, child).contains("refresh_requested"),
            "kept for the owner"
        );
    }
    let receipt = reconcile(&fx, "arch.md", &version).await.unwrap();
    assert_eq!(receipt["still_marked"], serde_json::json!([]), "{receipt}");
    for page in ["arch.md", "arch/db.md", "arch/api.md"] {
        assert!(!file(&fx, page).contains("refresh_requested"), "{page}");
    }
    assert!(stale_items(&queue(&fx).await).is_empty());
}

/// An honest run that retired the only marked page of the set while it
/// worked: the owner's version still carries that mark, and the set holds
/// no readable mark any more. The record lands and clears nothing; the
/// retired page keeps its mark in its file.
#[tokio::test]
async fn a_run_that_retired_the_marked_owned_page_still_records() {
    let fx = reflecting().await;
    put(&fx, "week.md", "---\nfreshness: 1d\n---\n# Week\n").await;
    owned(&fx, "week/mon.md", "week.md").await;
    reconcile(&fx, "week/mon.md", "").await.unwrap();
    reconcile(&fx, "week.md", "").await.unwrap();
    mark(&fx, &one("week/mon.md"), &person()).await.unwrap();
    backdate(&fx, "week.md", 2 * DAY).await;
    let owner = stale_of(&queue(&fx).await, "week.md").expect("the owner's item");
    let version = owner["version"].as_str().unwrap().to_string();
    assert_eq!(version.split(':').count(), 4, "{version}");

    fx.engine
        .write_reflection(&act("retire", "week/mon.md"), &agent())
        .await
        .unwrap();
    let receipt = reconcile(&fx, "week.md", &version).await.unwrap();
    assert_eq!(receipt["still_marked"], serde_json::json!([]), "{receipt}");
    assert!(
        fx.engine
            .reflection_reconcile("eng", "week.md")
            .await
            .unwrap()
            .unwrap()
            .outcome
            .is_some(),
        "recorded"
    );
    assert!(file(&fx, "week/mon.md").contains("refresh_requested"));
    assert!(stale_items(&queue(&fx).await).is_empty());
}

/// The owner marked first, its owned page later, then the owned page
/// retired by the run: the version's mark part (the owned page's) is newer
/// than every mark left in the set. The record lands and clears the
/// owner's mark, never more than the newest mark it still reads.
#[tokio::test]
async fn a_run_that_retired_the_newest_marked_page_clears_up_to_what_is_left() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    owned(&fx, "arch/db.md", "arch.md").await;
    mark(
        &fx,
        &MarkStale {
            pages: MarkPages::One("arch.md".into()),
            only_page: true,
        },
        &person(),
    )
    .await
    .unwrap();
    mark(&fx, &one("arch/db.md"), &person()).await.unwrap();
    let item = stale_of(&queue(&fx).await, "arch.md").unwrap();
    assert_eq!(item["owned_marked"], serde_json::json!(["arch/db.md"]));
    let version = item["version"].as_str().unwrap().to_string();

    fx.engine
        .write_reflection(&act("retire", "arch/db.md"), &agent())
        .await
        .unwrap();
    let receipt = reconcile(&fx, "arch.md", &version).await.unwrap();
    assert_eq!(receipt["still_marked"], serde_json::json!([]), "{receipt}");
    assert!(receipt["refresh_requested"].is_null(), "{receipt}");
    assert!(!file(&fx, "arch.md").contains("refresh_requested"));
    assert!(stale_items(&queue(&fx).await).is_empty());
}

/// A child marked on its own while its owner is not due gets an item of
/// its own; once the owner is due by its freshness, the child folds into
/// the owner's item, `owned_marked` names it and the owner's version
/// carries its mark, so the owner's record takes it off.
#[tokio::test]
async fn a_marked_child_has_its_own_item_until_its_owner_is_due() {
    let fx = reflecting().await;
    put(&fx, "week.md", "---\nfreshness: 1d\n---\n# Week\n").await;
    owned(&fx, "week/mon.md", "week.md").await;
    fx.engine
        .write_reflection(&reconciled("week/mon.md", "updated", ""), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&reconciled("week.md", "updated", ""), &agent())
        .await
        .unwrap();
    mark(&fx, &one("week/mon.md"), &person()).await.unwrap();
    let q = queue(&fx).await;
    let child = stale_of(&q, "week/mon.md").expect("the child's own item");
    assert!(stale_of(&q, "week.md").is_none(), "{q}");
    let e = reconcile(&fx, "week/mon.md", "").await.unwrap_err();
    assert!(e.to_string().contains("version"), "{e}");
    assert!(child["version"].as_str().unwrap().split(':').count() == 4);

    backdate(&fx, "week.md", 2 * DAY).await;
    let q = queue(&fx).await;
    assert!(stale_of(&q, "week/mon.md").is_none(), "folded: {q}");
    let owner = stale_of(&q, "week.md").expect("the owner's item");
    assert_eq!(owner["freshness"], "1d");
    assert_eq!(owner["owned_marked"], serde_json::json!(["week/mon.md"]));
    assert!(owner["refresh_requested"].is_null());
    let version = owner["version"].as_str().unwrap().to_string();
    assert_eq!(version.split(':').count(), 4, "{version}");
    // The child was reconciled before its mark: it is not done.
    assert_eq!(owner["owned_reconciled"], serde_json::json!([]));
    let e = reconcile(&fx, "week.md", &version).await.unwrap_err();
    assert!(e.to_string().contains("week/mon.md"), "{e}");
    reconcile(&fx, "week/mon.md", "").await.unwrap();
    reconcile(&fx, "week.md", &version).await.unwrap();
    assert!(!file(&fx, "week/mon.md").contains("refresh_requested"));
}

/// `only_page` on an owner marks only the owner; its owned pages are still
/// reconciled before it (a normal reconcile, not in `owned_marked`), also
/// when they were reconciled after the owner's last record but before the
/// mark.
#[tokio::test]
async fn only_the_page_on_an_owner_still_reconciles_its_owned_pages_first() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    owned(&fx, "arch/db.md", "arch.md").await;
    reconcile(&fx, "arch.md", "").await.unwrap();
    reconcile(&fx, "arch/db.md", "").await.unwrap();
    mark(
        &fx,
        &MarkStale {
            pages: MarkPages::One("arch.md".into()),
            only_page: true,
        },
        &person(),
    )
    .await
    .unwrap();
    let item = stale_of(&queue(&fx).await, "arch.md").unwrap();
    assert_eq!(item["owned"], serde_json::json!(["arch/db.md"]));
    assert_eq!(item["owned_marked"], serde_json::json!([]));
    assert_eq!(item["owned_reconciled"], serde_json::json!([]), "{item}");
    let version = item["version"].as_str().unwrap().to_string();
    let e = reconcile(&fx, "arch.md", &version).await.unwrap_err();
    assert!(e.to_string().contains("arch/db.md"), "{e}");
    reconcile(&fx, "arch/db.md", "").await.unwrap();
    let item = stale_of(&queue(&fx).await, "arch.md").unwrap();
    assert_eq!(item["owned_reconciled"], serde_json::json!(["arch/db.md"]));
    reconcile(&fx, "arch.md", &version).await.unwrap();
    assert!(stale_items(&queue(&fx).await).is_empty());
}

/// A never-reconciled page with a freshness of its own keeps its period and
/// age through a mark (the mark keeps the file's modified time): its item
/// is due by both, with the freshness and the freshness period.
#[tokio::test]
async fn a_never_reconciled_page_keeps_its_freshness_clock_through_a_mark() {
    let fx = reflecting().await;
    put(&fx, "week.md", "---\nfreshness: 1d\n---\n# Week\n").await;
    let abs = fx.root.join("reflections/week.md");
    std::fs::File::options()
        .write(true)
        .open(&abs)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3 * DAY as u64))
        .unwrap();
    fx.engine.sync(None).await.unwrap();
    let before = stale_of(&queue(&fx).await, "week.md").expect("due by freshness");
    mark(&fx, &one("week.md"), &person()).await.unwrap();
    let after = stale_of(&queue(&fx).await, "week.md").expect("due by both");
    assert_eq!(after["freshness"], "1d");
    assert_eq!(after["last_reconciled"], before["last_reconciled"]);
    let period = |v: &Value| {
        v["version"]
            .as_str()
            .unwrap()
            .split(':')
            .nth(1)
            .unwrap()
            .to_string()
    };
    assert_eq!(period(&after), period(&before));
    assert!(
        after["age_secs"].as_i64().unwrap() >= before["age_secs"].as_i64().unwrap(),
        "{before} {after}"
    );
}

/// Sources are engrams, never pages, so no page cites a marked page: a page
/// that links to it gets no `source_changed` from the mark or from the
/// reconcile that takes the mark off, and naming a page as a source is
/// refused.
#[tokio::test]
async fn a_mark_and_its_clearing_raise_nothing_on_other_pages() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    let mut p = write(
        "guides/b.md",
        "# B\n\n## Deploys\n\nSee [A](a.md). Deploys run at night.\n",
    );
    p.sources = [("Deploys".to_string(), vec!["runbooks/deploy".to_string()])].into();
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let mut cite = write("guides/c.md", "# C\n\n## Pages\n\nText.\n");
    cite.sources = [("Pages".to_string(), vec!["guides/a".to_string()])].into();
    let e = fx
        .engine
        .write_reflection(&cite, &agent())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no engram"), "{e}");

    let without_stale = |q: &Value| -> Vec<Value> {
        q["queue"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["what"] != "stale")
            .map(|i| {
                // An age moves with the clock, which a slow run crosses.
                let mut i = i.clone();
                i.as_object_mut().unwrap().remove("age_secs");
                i
            })
            .collect()
    };
    let before = without_stale(&queue(&fx).await);
    mark(&fx, &one("guides/a.md"), &person()).await.unwrap();
    assert_eq!(without_stale(&queue(&fx).await), before);
    let version = stale_of(&queue(&fx).await, "guides/a.md").unwrap()["version"]
        .as_str()
        .unwrap()
        .to_string();
    reconcile(&fx, "guides/a.md", &version).await.unwrap();
    let after = queue(&fx).await;
    assert_eq!(without_stale(&after), before, "{after}");
    assert!(
        !after["queue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["what"] == "source_changed"),
        "{after}"
    );
}

