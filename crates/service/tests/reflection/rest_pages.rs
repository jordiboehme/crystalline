//! The REST routes of a page's life beside its text: its settings, its
//! retirement and its clock, and the pages a reflect run missed (design
//! 2.2 to 2.4, 3.2 and 4).

use reqwest::Method;
use serde_json::{Value, json};

use crate::rest::{Session, serve};

const PAGE: &str = "guides/deploying.md";

/// A curator changes a page's settings, retires and restores it, and every
/// reader sees the settings on the page and its clock; an editor who does
/// not curate is refused, and so is a freshness that does not read.
#[tokio::test]
async fn a_curator_sets_retires_and_restores_and_a_reader_sees_the_clock() {
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    let ann = Session::login(s.addr, "ann").await;

    let (status, receipt) = ed
        .send(
            Method::PATCH,
            "/api/v1/domains/eng/reflection/settings",
            Some(json!({ "path": PAGE, "freshness": "1d" })),
        )
        .await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["page"]["freshness"], "1d", "{receipt}");
    assert_eq!(receipt["path"], PAGE, "{receipt}");

    let (status, err) = ann
        .send(
            Method::PATCH,
            "/api/v1/domains/eng/reflection/settings",
            Some(json!({ "path": PAGE, "freshness": "6h" })),
        )
        .await;
    assert_eq!(status, 403, "{err}");
    assert!(err["detail"].as_str().unwrap().contains("curate"), "{err}");

    for (body, says) in [
        (json!({ "path": PAGE, "freshness": "0h" }), "shorter"),
        (json!({ "path": PAGE }), "at least one"),
        (json!({ "path": PAGE, "status": "archived" }), "retire"),
    ] {
        let (status, err) = ed
            .send(
                Method::PATCH,
                "/api/v1/domains/eng/reflection/settings",
                Some(body.clone()),
            )
            .await;
        assert_eq!(status, 422, "{body}: {err}");
        assert!(
            err["detail"].as_str().unwrap().contains(says),
            "{body}: {err}"
        );
    }

    // A reader without engram rights reads the settings and the clock.
    let (status, doc) = ann
        .get(&format!("/api/v1/domains/eng/reflections/{PAGE}"))
        .await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["page"]["freshness"], "1d", "{doc}");
    assert_eq!(doc["page"]["pact"]["path"], "guides/deploying.pact.md");
    let (status, clock) = ann
        .get(&format!("/api/v1/domains/eng/reflection/clock?path={PAGE}"))
        .await;
    assert_eq!(status, 200, "{clock}");
    assert_eq!(clock["clock"]["freshness"], "1d", "{clock}");
    assert_eq!(clock["clock"]["clock_of"], PAGE, "{clock}");
    assert_eq!(clock["clock"]["stale"], false, "{clock}");
    let (_, none) = ann
        .get("/api/v1/domains/eng/reflection/clock?path=gone.md")
        .await;
    assert_eq!(none, json!({ "clock": null }));

    // Retire and restore: a curator's, and the tree marks the page.
    let (status, err) = ann
        .send(
            Method::POST,
            "/api/v1/domains/eng/reflection/retire",
            Some(json!({ "path": PAGE })),
        )
        .await;
    assert_eq!(status, 403, "{err}");
    let (status, retired) = ed
        .send(
            Method::POST,
            "/api/v1/domains/eng/reflection/retire",
            Some(json!({ "path": PAGE })),
        )
        .await;
    assert_eq!(status, 200, "{retired}");
    assert_eq!(
        retired,
        json!({ "domain": "eng", "path": PAGE, "retired": true, "was_owned_by": null })
    );
    let (_, tree) = ann.get("/api/v1/domains/eng/reflection/tree").await;
    let leaf = &tree["nodes"][0]["children"][0];
    assert_eq!(leaf["path"], PAGE, "{tree}");
    assert_eq!(leaf["retired"], true, "{tree}");
    let (status, again) = ed
        .send(
            Method::POST,
            "/api/v1/domains/eng/reflection/retire",
            Some(json!({ "path": PAGE })),
        )
        .await;
    assert_eq!(status, 422, "a retired page is not retired twice: {again}");

    let (status, restored) = ed
        .send(
            Method::POST,
            "/api/v1/domains/eng/reflection/restore",
            Some(json!({ "path": PAGE })),
        )
        .await;
    assert_eq!(status, 200, "{restored}");
    assert_eq!(
        restored,
        json!({ "domain": "eng", "path": PAGE, "restored": true, "owner": null })
    );
    let (_, doc) = ann
        .get(&format!("/api/v1/domains/eng/reflections/{PAGE}"))
        .await;
    assert_eq!(doc["page"]["retired"], Value::Null, "{doc}");
}

/// The missed list is instance wide and names a page a reflect run left,
/// with its domain and since when, to the callers who may read it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_missed_list_names_a_page_a_run_left_to_its_readers() {
    use crystalline_core::automation::{Loop, RunOutcome, RunReport};
    use crystalline_service::Scope;
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    let nora = Session::login(s.addr, "nora").await;
    let (status, missed) = ed.get("/api/v1/reflection/missed").await;
    assert_eq!(status, 200, "{missed}");
    assert_eq!(missed, json!({ "pages": [] }), "no run, nothing missed");

    let engine = &s.fx.engine;
    let at = |t: &str| crystalline_core::time::parse_rfc3339(t).unwrap();
    engine
        .set_reflection_page_settings(
            "eng",
            &crystalline_service::params::PageSettingsChange {
                path: PAGE.into(),
                freshness: Some("1h".into()),
                ..Default::default()
            },
            &crystalline_service::ReflectionCaller::owner(),
        )
        .await
        .unwrap();
    {
        let store = engine.store();
        let store = store.lock().await;
        let id = store.domain_id("eng").await.unwrap().unwrap();
        store
            .set_reflection_reconcile(
                id,
                &crystalline_index::ReflectionReconcile {
                    path: PAGE.to_string(),
                    last_reconciled: "2026-10-21T10:00:00+00:00".to_string(),
                    outcome: "unchanged".to_string(),
                    pact_hash: String::new(),
                    version: String::new(),
                },
            )
            .await
            .unwrap();
    }
    // A run that started after the page fell due ended without it.
    engine.set_automation_now(Some(at("2026-10-21T13:00:00+00:00")));
    let taken = engine
        .automation_lease_take(Loop::Reflect, "eng", "cron", &Scope::Unrestricted)
        .await
        .unwrap();
    engine.set_automation_now(Some(at("2026-10-21T13:05:00+00:00")));
    engine
        .automation_run_record(
            Loop::Reflect,
            "eng",
            taken["token"].as_str().unwrap(),
            &RunReport {
                started_at: String::new(),
                outcome: RunOutcome::StoppedAtCap,
                done: 20,
                summary: "capped".into(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    engine.set_automation_now(Some(at("2026-10-21T16:05:00+00:00")));

    let (status, missed) = ed.get("/api/v1/reflection/missed").await;
    assert_eq!(status, 200, "{missed}");
    assert_eq!(
        missed,
        json!({ "pages": [ { "domain": "eng", "path": PAGE, "freshness": "1h",
            "last_checked_secs": 6 * 3600 + 300, "missed_secs": 3 * 3600,
            "waiting": false } ] }),
    );
    let (status, missed) = nora.get("/api/v1/reflection/missed").await;
    assert_eq!(status, 200, "{missed}");
    assert_eq!(missed, json!({ "pages": [] }), "nora may not read eng");
}
