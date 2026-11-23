//! The automation lease and run record: one lease per domain and loop, so only
//! one runner works on a loop at a time, and the newest runs of each loop.
//!
//! Times are fixed-width ISO strings, compared lexically like every temporal
//! column, so a test moves the clock by naming a later string.

use crystalline_index::{
    AutomationRun, DomainId, DomainKind, DueKey, LeaseClaim, LeaseHolder, PathChecksum, Store,
    TursoStore,
};

/// Run a body against Turso (always) and Postgres (when configured).
macro_rules! parity {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                $body(&store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(scratch) = crate::pg::scratch("au") {
                    let store = crystalline_index::PostgresStore::open_in_schema(
                        scratch.url(),
                        scratch.name(),
                    )
                    .await
                    .expect("open the postgres test schema");
                    $body(&store).await;
                }
            }
        }
    };
}

async fn domain(store: &dyn Store, name: &str) -> DomainId {
    store
        .upsert_domain(name, Some(&format!("/k/{name}")), DomainKind::File)
        .await
        .unwrap()
}

fn holder(instance: &str, runner: &str) -> LeaseHolder {
    LeaseHolder {
        instance_id: instance.to_string(),
        account: Some("ada".to_string()),
        runner: runner.to_string(),
        unshared_at_start: vec![PathChecksum {
            path: "notes/a.md".to_string(),
            checksum: "sha-a".to_string(),
        }],
        due_at_start: vec![DueKey {
            kind: "thread".to_string(),
            key: "thread:42".to_string(),
            version: "v1".to_string(),
        }],
    }
}

fn token_of(claim: LeaseClaim) -> String {
    match claim {
        LeaseClaim::Acquired { token, .. } => token,
        LeaseClaim::Held(lease) => panic!("expected to acquire, held by {lease:?}"),
    }
}

const T0: &str = "2026-10-21T10:00:00+00:00";
const BEFORE_T0: &str = "2026-10-21T09:55:00+00:00";

async fn exclusive_until_stale(store: &dyn Store) {
    let d = domain(store, "eng").await;
    assert!(
        store
            .automation_lease(d, "reflect")
            .await
            .unwrap()
            .is_none()
    );

    let first = holder("inst-a", "cli");
    let token = token_of(
        store
            .take_automation_lease(d, "reflect", &first, T0, BEFORE_T0)
            .await
            .unwrap(),
    );
    let lease = store.automation_lease(d, "reflect").await.unwrap().unwrap();
    assert_eq!(lease.holder, first, "the holder reads back as stored");
    assert_eq!(lease.token, token);
    assert_eq!(lease.taken_at, T0);
    assert_eq!(lease.renewed_at, T0);

    // A second take while the lease is fresh is refused, the same instance
    // included: two runs of one loop never overlap.
    for second in [holder("inst-b", "n8n"), holder("inst-a", "cli")] {
        match store
            .take_automation_lease(
                d,
                "reflect",
                &second,
                "2026-10-21T10:01:00+00:00",
                "2026-10-21T09:56:00+00:00",
            )
            .await
            .unwrap()
        {
            LeaseClaim::Held(held) => assert_eq!(held, lease, "the holder is named, unchanged"),
            LeaseClaim::Acquired { .. } => panic!("a fresh lease must not be taken"),
        }
    }

    // Once `stale_before` passes the last renewal, a take succeeds and the
    // new holder gets a token of its own.
    let next = holder("inst-b", "k8s");
    let later = "2026-10-21T10:06:00+00:00";
    let new_token = token_of(
        store
            .take_automation_lease(d, "reflect", &next, later, "2026-10-21T10:01:00+00:00")
            .await
            .unwrap(),
    );
    assert_ne!(new_token, token);
    let lease = store.automation_lease(d, "reflect").await.unwrap().unwrap();
    assert_eq!(lease.holder, next);
    assert_eq!(lease.token, new_token);
    assert_eq!(lease.taken_at, later);
}
parity!(
    a_lease_is_exclusive_until_it_goes_stale,
    exclusive_until_stale
);

async fn renew_and_release_by_token(store: &dyn Store) {
    let d = domain(store, "eng").await;
    let token = token_of(
        store
            .take_automation_lease(d, "evolve", &holder("inst-a", "cli"), T0, BEFORE_T0)
            .await
            .unwrap(),
    );
    let beat = "2026-10-21T10:01:00+00:00";

    assert!(
        !store
            .renew_automation_lease(d, "evolve", "not-the-token", beat)
            .await
            .unwrap(),
        "a renew with another token updates nothing"
    );
    assert_eq!(
        store
            .automation_lease(d, "evolve")
            .await
            .unwrap()
            .unwrap()
            .renewed_at,
        T0
    );
    assert!(
        store
            .renew_automation_lease(d, "evolve", &token, beat)
            .await
            .unwrap()
    );
    let lease = store.automation_lease(d, "evolve").await.unwrap().unwrap();
    assert_eq!(lease.renewed_at, beat);
    assert_eq!(lease.taken_at, T0, "a renew keeps the take time");

    assert!(
        store
            .release_automation_lease(d, "evolve", "not-the-token")
            .await
            .unwrap()
            .is_none(),
        "a release with another token deletes nothing"
    );
    assert!(store.automation_lease(d, "evolve").await.unwrap().is_some());

    let released = store
        .release_automation_lease(d, "evolve", &token)
        .await
        .unwrap();
    assert_eq!(released, Some(lease), "the release hands back what it held");
    assert!(store.automation_lease(d, "evolve").await.unwrap().is_none());
    assert!(
        !store
            .renew_automation_lease(d, "evolve", &token, beat)
            .await
            .unwrap(),
        "a released lease cannot be renewed"
    );
}
parity!(renew_and_release_need_the_token, renew_and_release_by_token);

async fn loops_are_independent(store: &dyn Store) {
    let d = domain(store, "eng").await;
    let other = domain(store, "ops").await;
    let reflect = token_of(
        store
            .take_automation_lease(d, "reflect", &holder("inst-a", "cli"), T0, BEFORE_T0)
            .await
            .unwrap(),
    );
    let evolve = token_of(
        store
            .take_automation_lease(d, "evolve", &holder("inst-a", "cli"), T0, BEFORE_T0)
            .await
            .unwrap(),
    );
    token_of(
        store
            .take_automation_lease(other, "reflect", &holder("inst-a", "cli"), T0, BEFORE_T0)
            .await
            .unwrap(),
    );

    assert!(
        store
            .release_automation_lease(d, "reflect", &reflect)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .release_automation_lease(d, "reflect", &evolve)
            .await
            .unwrap()
            .is_none(),
        "one loop's token never releases the other loop"
    );
    assert_eq!(
        store
            .automation_lease(d, "evolve")
            .await
            .unwrap()
            .unwrap()
            .token,
        evolve,
        "releasing reflect leaves evolve held"
    );
    assert!(
        store
            .automation_lease(other, "reflect")
            .await
            .unwrap()
            .is_some(),
        "and another domain's reflect lease too"
    );
}
parity!(
    reflect_and_evolve_leases_never_block_each_other,
    loops_are_independent
);

fn run(n: usize) -> AutomationRun {
    AutomationRun {
        started_at: format!("2026-10-21T10:{n:02}:00+00:00"),
        ended_at: format!("2026-10-21T10:{n:02}:30+00:00"),
        runner: "cli".to_string(),
        account: n.is_multiple_of(2).then(|| "ada".to_string()),
        outcome: "done".to_string(),
        done: n as u64,
        left: 1,
        judgment_waiting: 2,
        summary: format!("run {n}"),
        pr_url: n
            .is_multiple_of(3)
            .then(|| format!("https://example.test/pull/{n}")),
        left_items: vec![DueKey {
            kind: "finding".to_string(),
            key: format!("V102:eng:page-{n}"),
            version: "ev1".to_string(),
        }],
    }
}

async fn runs_newest_first_pruned(store: &dyn Store) {
    let d = domain(store, "eng").await;
    for n in 0..55 {
        store
            .record_automation_run(d, "reflect", &run(n), 50)
            .await
            .unwrap();
    }
    store
        .record_automation_run(d, "evolve", &run(0), 50)
        .await
        .unwrap();

    let runs = store.automation_runs(d, "reflect", 100).await.unwrap();
    assert_eq!(runs.len(), 50, "pruned to the newest fifty");
    let expected: Vec<AutomationRun> = (5..55).rev().map(run).collect();
    assert_eq!(runs, expected, "newest first, every field as recorded");

    let top = store.automation_runs(d, "reflect", 3).await.unwrap();
    assert_eq!(top, vec![run(54), run(53), run(52)]);

    assert_eq!(
        store.automation_runs(d, "evolve", 100).await.unwrap(),
        vec![run(0)],
        "another loop keeps its own record"
    );
}
parity!(
    runs_are_kept_newest_first_and_pruned_to_fifty,
    runs_newest_first_pruned
);

async fn wipe_clears_lease_and_runs(store: &dyn Store) {
    let d = domain(store, "eng").await;
    token_of(
        store
            .take_automation_lease(d, "reflect", &holder("inst-a", "cli"), T0, BEFORE_T0)
            .await
            .unwrap(),
    );
    store
        .record_automation_run(d, "reflect", &run(1), 50)
        .await
        .unwrap();

    store.wipe().await.expect("the wipe clears both tables");

    assert!(
        store
            .automation_lease(d, "reflect")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .automation_runs(d, "reflect", 50)
            .await
            .unwrap()
            .is_empty()
    );
}
parity!(wipe_drops_both_tables, wipe_clears_lease_and_runs);

async fn empty_domain_row_takes_its_automation(store: &dyn Store) {
    let d = domain(store, "eng").await;
    token_of(
        store
            .take_automation_lease(d, "reflect", &holder("inst-a", "cli"), T0, BEFORE_T0)
            .await
            .unwrap(),
    );
    store
        .record_automation_run(d, "evolve", &run(1), 50)
        .await
        .unwrap();

    assert!(store.drop_empty_domain_row("eng").await.unwrap());

    assert!(
        store
            .automation_lease(d, "reflect")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .automation_runs(d, "evolve", 50)
            .await
            .unwrap()
            .is_empty()
    );
}
parity!(
    an_empty_domain_row_drops_with_its_lease_and_runs,
    empty_domain_row_takes_its_automation
);

/// A run's written paths and its pull request ride on the lease: a note
/// lands only on a lease of the noting account (whatever its age), each
/// path once, and a take over a stale lease starts clean and names the
/// lease it replaced.
async fn lease_carries_written_and_pr_url(store: &dyn Store) {
    let d = domain(store, "eng").await;
    let first = holder("inst-a", "cli");
    let token = token_of(
        store
            .take_automation_lease(d, "evolve", &first, T0, BEFORE_T0)
            .await
            .unwrap(),
    );
    let paths = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        store
            .note_automation_written(d, "ada", None, &paths(&["b.md", "a.md"]))
            .await
            .unwrap(),
        1
    );
    store
        .note_automation_written(d, "ada", None, &paths(&["a.md", "c.md"]))
        .await
        .unwrap();
    // Another account notes nothing.
    assert_eq!(
        store
            .note_automation_written(d, "bob", None, &paths(&["x.md"]))
            .await
            .unwrap(),
        0
    );
    // A note for the other loop lands on no evolve lease.
    assert_eq!(
        store
            .note_automation_written(d, "ada", Some("reflect"), &paths(&["r.md"]))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .note_automation_written(d, "ada", Some("evolve"), &paths(&["c.md"]))
            .await
            .unwrap(),
        1
    );
    assert!(
        store
            .set_automation_pr_url(d, "evolve", &token, "https://forge.test/pull/1")
            .await
            .unwrap()
    );
    assert!(
        !store
            .set_automation_pr_url(d, "evolve", "not-the-token", "https://x")
            .await
            .unwrap()
    );
    let lease = store.automation_lease(d, "evolve").await.unwrap().unwrap();
    let mut written = lease.written.clone();
    written.sort();
    assert_eq!(written, paths(&["a.md", "b.md", "c.md"]));
    assert_eq!(lease.pr_url.as_deref(), Some("https://forge.test/pull/1"));
    // The token, the written paths and the unshared files never serialize.
    let shown = serde_json::to_value(&lease).unwrap();
    assert!(shown.get("token").is_none(), "{shown}");
    assert!(shown.get("written").is_none(), "{shown}");
    assert!(
        shown["holder"].get("unshared_at_start").is_none(),
        "{shown}"
    );

    let later = "2026-10-21T10:06:00+00:00";
    match store
        .take_automation_lease(
            d,
            "evolve",
            &holder("inst-b", "k8s"),
            later,
            "2026-10-21T10:01:00+00:00",
        )
        .await
        .unwrap()
    {
        LeaseClaim::Acquired { replaced, .. } => {
            assert_eq!(replaced, Some(lease), "the stale lease, as it was");
        }
        LeaseClaim::Held(held) => panic!("a stale lease is taken over: {held:?}"),
    }
    let fresh = store.automation_lease(d, "evolve").await.unwrap().unwrap();
    assert!(
        fresh.written.is_empty() && fresh.pr_url.is_none(),
        "{fresh:?}"
    );
    // The first take of an unheld lease replaced nothing.
    match store
        .take_automation_lease(d, "reflect", &first, T0, BEFORE_T0)
        .await
        .unwrap()
    {
        LeaseClaim::Acquired { replaced, .. } => assert_eq!(replaced, None),
        LeaseClaim::Held(held) => panic!("{held:?}"),
    }
}
parity!(
    written_paths_and_the_pull_request_ride_on_the_lease,
    lease_carries_written_and_pr_url
);
