//! Marking a Reflection page not fresh (0.37.0): who may mark, which pages
//! a mark reaches, what it writes and what it leaves alone, and how the
//! next reflect run reconciles the page and takes the mark off.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crystalline_core::reflection as r;
use crystalline_service::params::{MarkPages, MarkStale, ReflectParams, WriteReflectionParams};
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
    assert_eq!(after["queue"], before["queue"], "no new item: {after}");
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
