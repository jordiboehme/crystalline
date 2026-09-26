//! What `crystalline doctor` asks the engine about domain names: the name
//! report (shadowed and contested names, dropped aliases, team domains that
//! declare no name, adoptions still waiting and links spelled with a name only
//! this machine uses), the fix that respells those links, and the empty index
//! row a removed domain used to leave behind under its name.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, NameOrigin, OriginConfig};
use crystalline_index::{DomainKind, Store, TursoStore};
use crystalline_service::{Engine, EngineError, Scope};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const TODAY: &str = "2026-01-01";

type SharedStore = Arc<Mutex<dyn Store>>;

async fn memory_store() -> SharedStore {
    Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()))
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: {TODAY}\n---\n\n# {title}\n\n{body}\n"
    )
}

/// A folder whose MANIFEST declares `domain_name: declared`.
fn declared_folder(dir: &Path, folder: &str, declared: &str) -> PathBuf {
    let root = dir.join(folder);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("MANIFEST.md"),
        crystalline_core::manifest_template(declared, TODAY),
    )
    .unwrap();
    root
}

/// A folder whose MANIFEST declares no `domain_name`.
fn bare_folder(dir: &Path, folder: &str) -> PathBuf {
    let root = dir.join(folder);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("MANIFEST.md"),
        format!(
            "---\ntype: manifest\ntitle: {folder}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: {TODAY}\n---\n\n# {folder}\n\n## Scope\n\n- x\n\n## When to Use\n\n- x\n"
        ),
    )
    .unwrap();
    root
}

fn explicit(root: PathBuf) -> DomainEntry {
    DomainEntry::file(root).with_name_origin(NameOrigin::Explicit)
}

fn team(root: PathBuf, repo: &str) -> DomainEntry {
    DomainEntry {
        origin: Some(OriginConfig {
            repo: repo.to_string(),
            path: None,
            branch: None,
            poll_secs: None,
        }),
        ..explicit(root)
    }
}

/// A config file on disk holding `domains`, and an engine over it with a
/// state directory, the way a daemon reads both at startup.
fn engine(store: SharedStore, dir: &Path, domains: Vec<(&str, DomainEntry)>) -> Engine {
    let mut cfg = GlobalConfig::default();
    for (name, entry) in domains {
        cfg.domains.insert(name.to_string(), entry);
    }
    let config_path = dir.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    Engine::new(store, cfg, None, Some(config_path)).with_state_dir(dir.join("state"))
}

fn saved(dir: &Path) -> GlobalConfig {
    crystalline_core::config::load_yaml(&dir.join("config.yaml")).unwrap()
}

/// `eng` declares `ops`, which is the local name of `ops`: shadowed. `a` and
/// `b` both declare `shared`, which nobody registers: contested. `b` lists
/// the alias `ops`, which `ops` owns: dropped.
#[tokio::test]
async fn the_report_names_shadowed_contested_and_dropped_names() {
    let tmp = tempfile::tempdir().unwrap();
    let mut b = explicit(declared_folder(tmp.path(), "b", "shared"));
    b.aliases.push("ops".to_string());
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![
            ("ops", explicit(bare_folder(tmp.path(), "ops"))),
            ("eng", explicit(declared_folder(tmp.path(), "eng", "ops"))),
            ("a", explicit(declared_folder(tmp.path(), "a", "shared"))),
            ("b", b),
        ],
    );
    engine.sync(None).await.unwrap();

    let report = engine.name_report().await.unwrap();
    assert_eq!(
        report["shadowed"],
        json!([{ "domain": "eng", "canonical": "ops", "held_by": "ops" }]),
        "{report}"
    );
    assert_eq!(
        report["conflicts"],
        json!([{ "name": "shared", "claimants": ["a", "b"] }]),
        "{report}"
    );
    assert_eq!(
        report["dropped_aliases"],
        json!([{ "domain": "b", "alias": "ops", "held_by": "ops" }]),
        "{report}"
    );
    assert_eq!(report["local_spellings"], json!([]), "{report}");
    assert_eq!(report["adoption_pending"], json!([]), "{report}");
}

/// Every team domain whose MANIFEST declares no `domain_name` is named, with
/// its repository, whether its local name came from the repository or was
/// typed. A team domain that declares a name, and a local domain that does
/// not, are not.
#[tokio::test]
async fn a_team_domain_without_a_declared_name_is_named() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![
            ("kb", team(bare_folder(tmp.path(), "kb"), "acme/kb")),
            (
                "handbook",
                team(
                    declared_folder(tmp.path(), "hb", "handbook"),
                    "acme/eng-handbook",
                ),
            ),
            ("notes", explicit(bare_folder(tmp.path(), "notes"))),
        ],
    );

    let report = engine.name_report().await.unwrap();
    assert_eq!(
        report["team_without_domain_name"],
        json!([{ "domain": "kb", "repo": "acme/kb" }]),
        "{report}"
    );
}

/// `eng` is called `engineering` by its MANIFEST and keeps its explicit local
/// name here. A link in `ops` spelled `eng` names the domain by a name only
/// this machine uses: the report counts it per file, the fix respells it on
/// disk through the write path, and the report is clean afterwards.
#[tokio::test]
async fn local_only_spellings_are_counted_and_fixed() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = declared_folder(tmp.path(), "eng", "engineering");
    std::fs::write(
        eng.join("runbook.md"),
        engram("Runbook", "runbook", "The steps."),
    )
    .unwrap();
    let ops = bare_folder(tmp.path(), "ops");
    std::fs::write(
        ops.join("note.md"),
        engram(
            "Note",
            "note",
            "See [[eng:runbook]] and crystalline://eng/runbook for the steps.",
        ),
    )
    .unwrap();
    std::fs::write(
        ops.join("clean.md"),
        engram("Clean", "clean", "See [[engineering:runbook]]."),
    )
    .unwrap();
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![("eng", explicit(eng)), ("ops", explicit(ops.clone()))],
    );
    engine.sync(None).await.unwrap();

    let report = engine.name_report().await.unwrap();
    assert_eq!(
        report["local_spellings"],
        json!([{
            "domain": "ops", "path": "note.md", "spelling": "eng",
            "canonical": "engineering", "count": 2
        }]),
        "{report}"
    );

    let fixed = engine.fix_local_spellings().await.unwrap();
    assert_eq!(fixed, 2);
    let text = std::fs::read_to_string(ops.join("note.md")).unwrap();
    assert!(
        text.contains("[[engineering:runbook]]")
            && text.contains("crystalline://engineering/runbook"),
        "{text}"
    );
    assert!(!text.contains("[[eng:"), "{text}");

    let report = engine.name_report().await.unwrap();
    assert_eq!(report["local_spellings"], json!([]), "{report}");
    assert_eq!(engine.fix_local_spellings().await.unwrap(), 0);
}

/// A read-only instance writes nothing, so the fix refuses there.
#[tokio::test]
async fn the_fix_refuses_on_a_read_only_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![("ops", explicit(bare_folder(tmp.path(), "ops")))],
    )
    .with_read_only(true);
    assert!(matches!(
        engine.fix_local_spellings().await,
        Err(EngineError::ReadOnly)
    ));
}

/// Removing a domain drops its index row, so the name is free at once for a
/// rename onto it.
#[tokio::test]
async fn removing_a_domain_frees_its_name_in_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let engine = engine(
        store.clone(),
        tmp.path(),
        vec![
            ("eng", explicit(bare_folder(tmp.path(), "eng"))),
            ("ops", explicit(bare_folder(tmp.path(), "ops"))),
        ],
    );
    engine.sync(None).await.unwrap();
    assert!(store.lock().await.domain_id("ops").await.unwrap().is_some());

    engine
        .unregister_domain("ops", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    assert_eq!(store.lock().await.domain_id("ops").await.unwrap(), None);

    engine
        .rename_domain_local("eng", "ops", NameOrigin::Explicit, &Scope::Unrestricted)
        .await
        .expect("the removed domain's name is free");
}

/// A row an older version left under a name refuses a rename onto it, and
/// the refusal names the doctor fix that drops it.
#[tokio::test]
async fn a_leftover_row_refusal_names_the_doctor_fix() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    store
        .lock()
        .await
        .upsert_domain("taken", Some("/gone/taken"), DomainKind::File)
        .await
        .unwrap();
    let engine = engine(
        store,
        tmp.path(),
        vec![("eng", explicit(bare_folder(tmp.path(), "eng")))],
    );
    engine.sync(None).await.unwrap();
    let err = engine
        .rename_domain_local("eng", "taken", NameOrigin::Explicit, &Scope::Unrestricted)
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("crystalline doctor --fix") && !text.contains("reindex --wipe"),
        "{text}"
    );
}

/// A derived domain whose MANIFEST declares a name an older version's empty
/// row still holds: the adoption waits quietly (reported, never renamed and
/// never recorded as seen), the name report says why and names the fix,
/// `doctor --fix`'s collection drops the row, and the next sync adopts.
#[tokio::test]
async fn an_adoption_blocked_by_a_leftover_row_waits_and_is_explained() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    store
        .lock()
        .await
        .upsert_domain("platform", Some("/gone/platform"), DomainKind::File)
        .await
        .unwrap();
    let engine = engine(
        store.clone(),
        tmp.path(),
        vec![(
            "eng",
            DomainEntry::file(declared_folder(tmp.path(), "eng", "platform"))
                .with_name_origin(NameOrigin::Derived),
        )],
    );
    engine.sync(None).await.unwrap();
    let adopted = engine.adopt_domain_names().await.unwrap();
    assert_eq!(
        adopted,
        json!([{
            "domain": "eng", "canonical": "platform", "action": "waiting",
            "reason": "leftover_row"
        }]),
        "{adopted}"
    );
    let cfg = saved(tmp.path());
    assert!(cfg.domains.contains_key("eng"));
    assert_eq!(cfg.domains["eng"].canonical_seen, None);

    let report = engine.name_report().await.unwrap();
    let pending = report["adoption_pending"].as_array().unwrap();
    assert_eq!(pending.len(), 1, "{report}");
    assert_eq!(pending[0]["domain"], "eng");
    assert_eq!(pending[0]["canonical"], "platform");
    assert_eq!(pending[0]["canonical_seen"], Value::Null);
    let reason = pending[0]["reason"].as_str().unwrap_or_default();
    assert!(reason.contains("crystalline doctor --fix"), "{reason}");

    // What `doctor` shows before the fix: the row, empty, droppable.
    let preview = engine.collect_orphaned_domains(None, true).await.unwrap();
    let row = preview["considered"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["domain"] == "platform")
        .cloned()
        .unwrap_or_else(|| panic!("the row is considered: {preview}"));
    assert_eq!(row["kept"], "no_rows", "{row}");
    assert_eq!(row["row_droppable"], true, "{row}");
    assert!(
        store
            .lock()
            .await
            .domain_id("platform")
            .await
            .unwrap()
            .is_some()
    );

    // `doctor --fix`.
    let fixed = engine.collect_orphaned_domains(None, false).await.unwrap();
    let row = fixed["considered"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["domain"] == "platform")
        .cloned()
        .unwrap();
    assert_eq!(row["row_dropped"], true, "{row}");
    assert_eq!(
        store.lock().await.domain_id("platform").await.unwrap(),
        None
    );

    engine.sync(None).await.unwrap();
    let adopted = engine.adopt_domain_names().await.unwrap();
    assert!(
        adopted
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "renamed"),
        "{adopted}"
    );
    assert!(saved(tmp.path()).domains.contains_key("platform"));
    let report = engine.name_report().await.unwrap();
    assert_eq!(report["adoption_pending"], json!([]), "{report}");
}

/// The daemon's own timer keeps the empty rows it finds: it waits for a
/// person, and only a person's `doctor --fix` drops them.
#[tokio::test]
async fn the_timed_collection_keeps_an_empty_row() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    store
        .lock()
        .await
        .upsert_domain("gone", Some("/gone"), DomainKind::File)
        .await
        .unwrap();
    let engine = engine(
        store.clone(),
        tmp.path(),
        vec![("eng", explicit(bare_folder(tmp.path(), "eng")))],
    );
    let report = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();
    let row = report["considered"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["domain"] == "gone")
        .cloned()
        .unwrap();
    assert!(row.get("row_dropped").is_none(), "{row}");
    assert!(
        store
            .lock()
            .await
            .domain_id("gone")
            .await
            .unwrap()
            .is_some()
    );
}

/// An empty row a removed virtual domain left behind is dropped by a
/// person's `doctor --fix` too: with nothing in it, nothing is lost.
#[tokio::test]
async fn an_empty_virtual_row_left_behind_is_dropped_on_demand() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    store
        .lock()
        .await
        .upsert_domain("vault", None, DomainKind::Virtual)
        .await
        .unwrap();
    let engine = engine(
        store.clone(),
        tmp.path(),
        vec![("eng", explicit(bare_folder(tmp.path(), "eng")))],
    );

    let preview = engine.collect_orphaned_domains(None, true).await.unwrap();
    let row = preview["considered"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["domain"] == "vault")
        .cloned()
        .unwrap_or_else(|| panic!("{preview}"));
    assert_eq!(row["row_droppable"], true, "{row}");

    let fixed = engine.collect_orphaned_domains(None, false).await.unwrap();
    let row = fixed["considered"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["domain"] == "vault")
        .cloned()
        .unwrap();
    assert_eq!(row["row_dropped"], true, "{row}");
    assert_eq!(store.lock().await.domain_id("vault").await.unwrap(), None);
}

/// A rename after a sync that fails for a reason the state does not show (a
/// state folder a removed domain left under the name): the engine remembers
/// the error, and the name report gives it as the reason.
#[tokio::test]
async fn a_failed_adoption_reports_its_error_as_the_reason() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("state/origins/platform")).unwrap();
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![(
            "eng",
            DomainEntry::file(declared_folder(tmp.path(), "eng", "platform"))
                .with_name_origin(NameOrigin::Derived),
        )],
    );
    engine.sync(None).await.unwrap();
    let adopted = engine.adopt_domain_names().await.unwrap();
    assert!(
        adopted
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "failed"),
        "{adopted}"
    );

    let report = engine.name_report().await.unwrap();
    let pending = report["adoption_pending"].as_array().unwrap();
    assert_eq!(pending.len(), 1, "{report}");
    let reason = pending[0]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("the last rename to 'platform' failed") && reason.contains("left over"),
        "{reason}"
    );

    // Once the folder is out of the way the next sync adopts, and the
    // remembered error goes with it.
    std::fs::remove_dir_all(tmp.path().join("state/origins/platform")).unwrap();
    engine.sync(None).await.unwrap();
    engine.adopt_domain_names().await.unwrap();
    let report = engine.name_report().await.unwrap();
    assert_eq!(report["adoption_pending"], json!([]), "{report}");
}
