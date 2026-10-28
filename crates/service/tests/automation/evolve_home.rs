//! One evolve runner per team domain: `evolve_home` in the MANIFEST names
//! the instance whose scheduled evolve runs work the domain. The first
//! scheduled run at an instance with `service.public_url` fills it in, the
//! run's own pull request carries it, and every other clone refuses a
//! scheduled run and names the home.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use crystalline_core::automation::Loop;
use crystalline_core::config::{GitHubConfig, GlobalConfig, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::engine::{PolicyEdit, ShareActor};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, EngineError, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::fixture::{engram, fixture, manifest};
use crate::support::MockProvider;

/// The instance under test, when it has an address.
const HERE: &str = "https://kb.example.com";

/// Another instance.
const THERE: &str = "https://home.example";

fn start() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-21T10:00:00+00:00").unwrap()
}

/// A team MANIFEST that shares directly and carries the team marker, with
/// extra frontmatter lines (each ending in `\n`).
fn team_manifest(extra: &str) -> Vec<u8> {
    format!(
        "---\ntype: manifest\ntitle: Team\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\nsharing: direct\n{extra}---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n"
    )
    .into_bytes()
}

/// The team domain `brand`, registered from the forge over `manifest`, on
/// an instance whose `service.public_url` is `public_url`.
async fn team_at(
    tmp: &tempfile::TempDir,
    mock: Arc<MockProvider>,
    manifest: Vec<u8>,
    public_url: Option<&str>,
) -> (Engine, std::path::PathBuf) {
    let commit = mock.add_commit(BTreeMap::from([("MANIFEST.md".to_string(), manifest)]));
    mock.set_branch("main", &commit);
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        service: public_url.map(|url| ServiceConfig {
            public_url: Some(url.to_string()),
            ..ServiceConfig::default()
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
    )
    .await
    .unwrap();
    eng.set_automation_now(Some(start()));
    std::fs::create_dir_all(root.join("notes")).unwrap();
    (eng, root)
}

async fn status_of(engine: &Engine, domain: &str) -> Value {
    let answer = engine
        .automation_status(Loop::Evolve, &[domain.to_string()], &Scope::Unrestricted)
        .await
        .unwrap();
    answer["domains"][0].clone()
}

async fn take_as(engine: &Engine, domain: &str, scope: &Scope) -> Value {
    engine
        .automation_lease_take(Loop::Evolve, domain, "k8s", scope)
        .await
        .unwrap()
}

fn manifest_text(root: &std::path::Path) -> String {
    std::fs::read_to_string(root.join("MANIFEST.md")).unwrap()
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

fn write_note(root: &std::path::Path, rel: &str, body: &str) {
    let permalink = rel.trim_end_matches(".md");
    std::fs::write(root.join(rel), engram(body, permalink, body)).unwrap();
}

/// The first scheduled run at an instance with an address claims the home:
/// the take says so, the MANIFEST names this instance, the change waits in
/// the share preview, and the run's own pull request carries it beside the
/// run's fix.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_scheduled_run_claims_the_home_and_its_pull_request_carries_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = team_at(&tmp, mock.clone(), team_manifest(""), Some(HERE)).await;
    assert_eq!(status_of(&eng, "brand").await["runs_on"], Value::Null);

    let taken = take_as(&eng, "brand", &Scope::Unrestricted).await;
    assert_eq!(taken["evolve_home"], HERE, "{taken}");
    assert_eq!(
        taken["note"],
        "This instance is now the evolve home of 'brand' (evolve_home: https://kb.example.com). The run's pull request carries the MANIFEST change, or the next run's when this run shares nothing; once it is merged, other clones of the domain refuse a scheduled evolve run.",
        "{taken}"
    );
    assert!(
        manifest_text(&root).contains("evolve_home: https://kb.example.com\n"),
        "{}",
        manifest_text(&root)
    );
    let waiting = eng
        .local_changes("brand", &ShareActor::Owner)
        .await
        .unwrap();
    assert!(
        waiting["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["path"] == "MANIFEST.md"),
        "the claim waits as a change to share: {waiting}"
    );

    let token = taken["token"].as_str().unwrap();
    write_note(&root, "notes/fix.md", "the run's fix");
    let shared = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            token,
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
    // A second run while the pull request is open claims nothing new; it
    // carries the claim on until the team's copy has it.
    eng.automation_lease_release(Loop::Evolve, "brand", token, &Scope::Unrestricted)
        .await
        .unwrap();
    let again = take_as(&eng, "brand", &Scope::Unrestricted).await;
    assert!(again.get("evolve_home").is_none(), "{again}");
    assert!(
        again["note"]
            .as_str()
            .unwrap()
            .contains("is not in the team's copy yet"),
        "{again}"
    );
}

/// A claim the first run never shared (it failed, was stopped or ran with
/// `--no-share`) is not stranded: the next run says so, and its pull request
/// carries the MANIFEST change, which a person's edit would otherwise hold
/// back as unshared work from before the take.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claim_the_first_run_did_not_share_rides_with_the_next_run() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = team_at(&tmp, mock.clone(), team_manifest(""), Some(HERE)).await;
    let first = take_as(&eng, "brand", &Scope::Unrestricted).await;
    assert_eq!(first["evolve_home"], HERE, "{first}");
    eng.automation_lease_release(
        Loop::Evolve,
        "brand",
        first["token"].as_str().unwrap(),
        &Scope::Unrestricted,
    )
    .await
    .unwrap();

    let second = take_as(&eng, "brand", &Scope::Unrestricted).await;
    assert!(second.get("evolve_home").is_none(), "{second}");
    assert_eq!(
        second["note"],
        "The claim of the evolve home of 'brand' for this instance (evolve_home: https://kb.example.com) is not in the team's copy yet, so this run's pull request carries it too.",
        "{second}"
    );
    write_note(&root, "notes/fix.md", "the run's fix");
    let shared = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            second["token"].as_str().unwrap(),
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
    assert_eq!(shared["held_back"], serde_json::json!([]), "{shared}");
}

/// A person's own edit of the MANIFEST beside an unshared claim is theirs to
/// share: the run holds the MANIFEST back, as it holds back any file a
/// person had unshared work on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claim_beside_a_persons_manifest_edit_is_held_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = team_at(&tmp, mock.clone(), team_manifest(""), Some(HERE)).await;
    let first = take_as(&eng, "brand", &Scope::Unrestricted).await;
    eng.automation_lease_release(
        Loop::Evolve,
        "brand",
        first["token"].as_str().unwrap(),
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    let edited = manifest_text(&root).replace("- always\n", "- always, and on Mondays\n");
    std::fs::write(root.join("MANIFEST.md"), edited).unwrap();
    eng.sync(Some("brand")).await.unwrap();

    let second = take_as(&eng, "brand", &Scope::Unrestricted).await;
    assert!(second.get("note").is_none(), "{second}");
    write_note(&root, "notes/fix.md", "the run's fix");
    std::fs::write(
        root.join("MANIFEST.md"),
        manifest_text(&root).replace("on Mondays", "on Mondays and Fridays"),
    )
    .unwrap();
    let shared = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            second["token"].as_str().unwrap(),
            "Fixed one finding.",
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(shared_files(&shared), vec!["notes/fix.md"], "{shared}");
    assert_eq!(
        shared["held_back"],
        serde_json::json!(["MANIFEST.md"]),
        "{shared}"
    );
}

/// On a domain that reviews changes the claim is the runner's draft, and
/// the run's proposal carries it through the review.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_a_reviewing_domain_the_claim_is_the_runners_draft_and_rides_the_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = team_at(&tmp, mock.clone(), team_manifest(""), Some(HERE)).await;
    eng.set_review_mode(
        "brand",
        Some(crystalline_core::config::ReviewMode::Overlay),
        crystalline_service::ReviewModeConfirm::Confirmed { folds: Vec::new() },
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("runner", "runner", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(DomainAccess::new(auth)));
    let runner = Scope::User {
        account: "runner".into(),
        admin: false,
    };
    let taken = take_as(&eng, "brand", &runner).await;
    assert_eq!(taken["evolve_home"], HERE, "{taken}");
    assert!(
        !manifest_text(&root).contains("evolve_home"),
        "the folder is the reviewed truth; the claim is a draft"
    );
    let shared = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            taken["token"].as_str().unwrap(),
            "Claimed the home.",
            &runner,
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "shared", "{shared}");
    assert_eq!(shared_files(&shared), vec!["MANIFEST.md"], "{shared}");
}

/// A runner with its own account: the claim is noted as the run's write,
/// so the attributed share carries it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attributed_run_shares_its_claim() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = team_at(&tmp, mock.clone(), team_manifest(""), Some(HERE)).await;
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("runner", "runner", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(DomainAccess::new(auth)));
    let runner = Scope::User {
        account: "runner".into(),
        admin: false,
    };
    let taken = take_as(&eng, "brand", &runner).await;
    assert_eq!(taken["evolve_home"], HERE, "{taken}");
    assert!(manifest_text(&root).contains("evolve_home: https://kb.example.com\n"));
    let shared = eng
        .automation_share(
            Loop::Evolve,
            "brand",
            taken["token"].as_str().unwrap(),
            "Claimed the home.",
            &runner,
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "shared", "{shared}");
    assert_eq!(shared["attributed"], true, "{shared}");
    assert_eq!(shared_files(&shared), vec!["MANIFEST.md"], "{shared}");
}

/// A clone that is not the home is refused and names the home: in the
/// status, which the webhook and `evolve run` read, and in the take.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clone_that_is_not_the_home_is_refused_naming_it() {
    for public_url in [Some(HERE), None] {
        let tmp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockProvider::new());
        let manifest = team_manifest("evolve_home: https://home.example\n");
        let (eng, root) = team_at(&tmp, mock, manifest, public_url).await;
        let s = status_of(&eng, "brand").await;
        assert_eq!(s["runs_on"], THERE, "{public_url:?}: {s}");
        assert_eq!(s["due"], false, "{public_url:?}: {s}");
        let refused = eng
            .automation_lease_take(Loop::Evolve, "brand", "k8s", &Scope::Unrestricted)
            .await
            .unwrap_err();
        assert_eq!(refused.to_string(), "runs on https://home.example");
        assert!(matches!(refused, EngineError::Refused(_)), "{refused:?}");
        assert!(manifest_text(&root).contains("evolve_home: https://home.example\n"));
    }
}

/// The home itself runs and claims nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_home_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let manifest = team_manifest("evolve_home: https://kb.example.com\n");
    let (eng, root) = team_at(&tmp, mock, manifest, Some(HERE)).await;
    let before = manifest_text(&root);
    assert_eq!(status_of(&eng, "brand").await["runs_on"], Value::Null);
    let taken = take_as(&eng, "brand", &Scope::Unrestricted).await;
    assert!(taken["token"].is_string(), "{taken}");
    assert!(taken.get("note").is_none(), "{taken}");
    assert_eq!(manifest_text(&root), before);
}

/// No home yet and no address to claim it with: the run goes on as before
/// and says that every clone may still run evolve.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_public_url_a_team_domain_runs_and_says_there_is_no_home() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root) = team_at(&tmp, mock, team_manifest(""), None).await;
    let before = manifest_text(&root);
    let taken = take_as(&eng, "brand", &Scope::Unrestricted).await;
    assert!(taken["token"].is_string(), "{taken}");
    assert!(taken.get("evolve_home").is_none(), "{taken}");
    assert_eq!(
        taken["note"],
        "'brand' has no evolve_home yet, so every clone of it may run evolve. This instance has no service.public_url, so it cannot become the home: the first scheduled evolve run at an instance with one claims it.",
        "{taken}"
    );
    assert_eq!(manifest_text(&root), before);
}

/// A local domain is not a team domain: the key is ignored, nothing is
/// refused and nothing is claimed. A plain clone that carries the team
/// marker is a team domain and is refused like any other clone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_domain_ignores_evolve_home_and_a_marked_clone_is_refused() {
    let local = fixture(&[(
        "MANIFEST.md",
        &manifest("evolve_home: https://home.example\n"),
    )])
    .await;
    let s = status_of(&local.engine, "eng").await;
    assert_eq!(s["runs_on"], Value::Null, "{s}");
    let taken = take_as(&local.engine, "eng", &Scope::Unrestricted).await;
    assert!(taken.get("note").is_none(), "{taken}");
    assert!(taken.get("evolve_home").is_none(), "{taken}");

    let clone = fixture(&[(
        "MANIFEST.md",
        &manifest("team: github\nevolve_home: https://home.example\n"),
    )])
    .await;
    let s = status_of(&clone.engine, "eng").await;
    assert_eq!(s["runs_on"], THERE, "{s}");
    let refused = clone
        .engine
        .automation_lease_take(Loop::Evolve, "eng", "k8s", &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert_eq!(refused.to_string(), "runs on https://home.example");
}

/// The key is never set by hand. Removing it is how the home moves: at the
/// instance that should run evolve, whose next scheduled run then claims it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_home_moves_by_removing_it_where_evolve_should_run() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let manifest = team_manifest("evolve_home: https://home.example\n");
    let (eng, root) = team_at(&tmp, mock, manifest, Some(HERE)).await;
    let set = eng
        .edit_manifest_policies(
            "brand",
            &[PolicyEdit::Set {
                key: "evolve_home".into(),
                value: HERE.into(),
            }],
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert_eq!(
        set.to_string(),
        "`evolve_home` is filled in by the first scheduled evolve run at an instance with service.public_url; it is not set by hand. To move the home, remove it at the instance that should run evolve (unset evolve_home), then run evolve there"
    );
    // Through configure, as an agent or a person removes it.
    eng.change_domain_settings(
        "brand",
        &std::collections::BTreeMap::new(),
        &["evolve_home".to_string()],
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    assert!(!manifest_text(&root).contains("evolve_home"));
    let taken = take_as(&eng, "brand", &Scope::Unrestricted).await;
    assert_eq!(taken["evolve_home"], HERE, "{taken}");
    assert!(manifest_text(&root).contains("evolve_home: https://kb.example.com\n"));
}

/// The policies view draws the key read-only, with how it is filled in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_policies_view_says_how_the_home_is_filled_in() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let manifest = team_manifest("evolve_home: https://home.example\n");
    let (eng, _root) = team_at(&tmp, mock, manifest, Some(HERE)).await;
    let view = eng
        .domain_settings("brand", &Scope::Unrestricted)
        .await
        .unwrap();
    let row = view["policies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "evolve_home")
        .cloned()
        .expect("an evolve_home row");
    assert_eq!(row["effective"], THERE, "{row}");
    assert_eq!(row["can_change"], false, "{row}");
    assert_eq!(
        row["how"],
        "filled in by the first scheduled evolve run at an instance with service.public_url; to move the home, unset it with configure at the instance that should run evolve",
        "{row}"
    );
}

/// On a shared database the home is asked first, then the host lock among
/// the instances that are the home. Instances of one database that carry
/// one address are all the home, and the host lock picks one; when the
/// home names another address, nobody in the group runs.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_postgres_the_home_comes_first_and_the_host_lock_decides_among_the_home() {
    use crate::fixture::{instance_at, scratch_schema};
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
    let marked = manifest("team: github\nevolve_home: https://a.example\n");
    let files = [("MANIFEST.md", marked.as_str())];
    let a = instance_at(open().await, "alpha", "https://a.example", &files).await;
    let b = instance_at(open().await, "beta", "https://b.example", &files).await;
    a.engine.set_automation_now(Some(start()));
    b.engine.set_automation_now(Some(start()));

    let s = status_of(&b.engine, "eng").await;
    assert_eq!(s["runs_on"], "https://a.example", "{s}");
    let taken = take_as(&a.engine, "eng", &Scope::Unrestricted).await;
    a.engine
        .automation_lease_release(
            Loop::Evolve,
            "eng",
            taken["token"].as_str().unwrap(),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();

    // The home names beta, and alpha holds the host lock: each names the
    // other, and nobody in the group runs.
    let other = manifest("team: github\nevolve_home: https://b.example\n");
    for fx in [&a, &b] {
        std::fs::write(fx.root.join("MANIFEST.md"), &other).unwrap();
    }
    let s = status_of(&a.engine, "eng").await;
    assert_eq!(s["runs_on"], "https://b.example", "{s}");
    let s = status_of(&b.engine, "eng").await;
    assert!(s["runs_on"].is_string(), "the host lock holder: {s}");
    assert_ne!(s["runs_on"], "https://b.example", "{s}");
}
