//! The engine keeps one name table over every registered domain and pushes its
//! spellings into the index, so a domain answers to its MANIFEST `domain_name`
//! as well as to its local name: in the tool parameters (through
//! `Engine::local_domain_name`) and in the references content carries (through
//! the index's `domain_spelling` table).
//!
//! The table follows the MANIFEST: a sync, an edit of a file domain's MANIFEST
//! and an edit of a virtual domain's MANIFEST engram each rebuild it and push
//! what changed. The push compares before it writes, so an unchanged table
//! costs no write and two instances sharing one index do not take a contested
//! spelling from each other on every refresh.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, NameOrigin};
use crystalline_index::{Store, TursoStore};
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::{
    BrowseParams, EditParams, ReadParams, SearchParams, WriteParams,
};
use serde_json::Value;
use tokio::sync::Mutex;

const TODAY: &str = "2026-01-01";

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: {TODAY}\n---\n\n# {title}\n\n{body}\n"
    )
}

/// A file domain folder under `dir` holding a MANIFEST that declares
/// `domain_name` (the template writes it) and the given engrams.
fn file_domain(
    dir: &Path,
    folder: &str,
    declared: &str,
    engrams: &[(&str, String)],
) -> DomainEntry {
    let root = dir.join(folder);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("MANIFEST.md"),
        crystalline_core::manifest_template(declared, TODAY),
    )
    .unwrap();
    for (file, text) in engrams {
        std::fs::write(root.join(file), text).unwrap();
    }
    DomainEntry::file(root)
}

type SharedStore = Arc<Mutex<dyn Store>>;

async fn memory_store() -> SharedStore {
    Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()))
}

/// An engine over `domains`, reading its configuration file from `dir` (which
/// holds none), so no test ever reads the developer's real configuration.
fn engine(store: SharedStore, dir: &Path, domains: Vec<(&str, DomainEntry)>) -> Engine {
    let mut cfg = GlobalConfig::default();
    for (name, entry) in domains {
        cfg.domains.insert(name.to_string(), entry);
    }
    Engine::new(store, cfg, None, Some(dir.join("config.yaml"))).with_state_dir(dir.join("state"))
}

async fn read(engine: &Engine, domain: &str, identifier: &str) -> Value {
    engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some(domain.to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

/// Whether the one prose wikilink of an engram resolved, as the read
/// receipt reports it.
async fn link_resolved(engine: &Engine, domain: &str, identifier: &str) -> bool {
    let read = read(engine, domain, identifier).await;
    let links = read["links"].as_array().expect("a links array");
    assert_eq!(links.len(), 1, "one link in {identifier}: {links:?}");
    links[0]["resolved"].as_bool().unwrap()
}

fn replace_manifest_name(domain: &str, from: &str, to: &str) -> EditParams {
    EditParams {
        identifier: "manifest".to_string(),
        domain: domain.to_string(),
        operation: "find_replace".to_string(),
        find_text: Some(format!("domain_name: {from}")),
        content: Some(format!("domain_name: {to}")),
        expected_replacements: Some(1),
        ..EditParams::default()
    }
}

/// `eng-knowledge` declares `domain_name: eng`; after one sync the name
/// resolves to it and a link spelled with it binds.
#[tokio::test]
async fn a_manifest_domain_name_resolves_after_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = file_domain(
        tmp.path(),
        "eng-knowledge",
        "eng",
        &[("runbook.md", engram("Runbook", "runbook", "the steps"))],
    );
    let ops = file_domain(
        tmp.path(),
        "ops",
        "ops",
        &[(
            "oncall.md",
            engram("Oncall", "oncall", "Follow [[eng:runbook]] first."),
        )],
    );
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![("eng-knowledge", eng), ("ops", ops)],
    );

    engine.sync(None).await.unwrap();

    assert_eq!(
        engine.local_domain_name("eng").await.as_deref(),
        Some("eng-knowledge")
    );
    assert_eq!(
        engine.local_domain_name("eng-knowledge").await.as_deref(),
        Some("eng-knowledge")
    );
    assert_eq!(engine.local_domain_name("nowhere").await, None);
    assert_eq!(
        engine.localize_visible("eng", &HashSet::new()).await,
        "eng-knowledge"
    );
    assert_eq!(
        engine.localize_visible("nowhere", &HashSet::new()).await,
        "nowhere"
    );
    assert!(
        link_resolved(&engine, "ops", "oncall").await,
        "a link spelled with the canonical name binds"
    );
}

/// Editing the MANIFEST to `domain_name: platform` moves the name: the old
/// one resolves nowhere, a link spelled with it unbinds, and a link written
/// with the new one binds.
#[tokio::test]
async fn a_manifest_edit_changes_the_table() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = file_domain(
        tmp.path(),
        "eng-knowledge",
        "eng",
        &[("runbook.md", engram("Runbook", "runbook", "the steps"))],
    );
    let ops = file_domain(
        tmp.path(),
        "ops",
        "ops",
        &[(
            "oncall.md",
            engram("Oncall", "oncall", "Follow [[eng:runbook]] first."),
        )],
    );
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![("eng-knowledge", eng), ("ops", ops)],
    );
    engine.sync(None).await.unwrap();
    assert!(link_resolved(&engine, "ops", "oncall").await);

    engine
        .edit_engram(&replace_manifest_name("eng-knowledge", "eng", "platform"))
        .await
        .unwrap();

    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("eng-knowledge")
    );
    assert_eq!(engine.local_domain_name("eng").await, None);
    assert!(
        !link_resolved(&engine, "ops", "oncall").await,
        "a link spelled with the former canonical name no longer binds"
    );

    engine
        .write_engram(&WriteParams {
            domain: "ops".to_string(),
            title: "Escalation".to_string(),
            content: "Then [[platform:runbook]].".to_string(),
            folder: None,
            engram_type: None,
            tags: Vec::new(),
            status: None,
            metadata: None,
            overwrite: false,
            share_link: None,
            model: None,
        })
        .await
        .unwrap();
    assert!(
        link_resolved(&engine, "ops", "escalation").await,
        "a link spelled with the new canonical name binds"
    );
}

/// A virtual domain's MANIFEST engram declares its name the same way.
#[tokio::test]
async fn a_virtual_domain_declares_its_name_too() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let engine = engine(
        store.clone(),
        tmp.path(),
        vec![("scratch", DomainEntry::virtual_domain())],
    );
    engine.domain_add_virtual("scratch").await.unwrap();
    assert_eq!(engine.local_domain_name("notes").await, None);

    engine
        .edit_engram(&replace_manifest_name("scratch", "scratch", "notes"))
        .await
        .unwrap();

    assert_eq!(
        engine.local_domain_name("notes").await.as_deref(),
        Some("scratch")
    );
    let id = store
        .lock()
        .await
        .domain_id("scratch")
        .await
        .unwrap()
        .expect("the virtual domain has a row");
    let spellings = store.lock().await.domain_spellings().await.unwrap();
    assert!(
        spellings.contains(&("notes".to_string(), id)),
        "the index resolves the declared name too: {spellings:?}"
    );
}

/// A refresh whose table matches what the index already holds writes
/// nothing: the comparison runs before the replace, not inside it.
#[tokio::test]
async fn an_unchanged_table_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = file_domain(tmp.path(), "eng-knowledge", "eng", &[]);
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![("eng-knowledge", eng)],
    );
    engine.sync(None).await.unwrap();
    let after_sync = engine.spelling_replaces_issued();
    assert_eq!(after_sync, 1, "the first push records the canonical name");

    engine.refresh_names().await;
    engine.sync(None).await.unwrap();
    engine.refresh_names().await;
    assert_eq!(engine.spelling_replaces_issued(), after_sync);
}

/// Two instances over one index, each registering its own domain that
/// declares the same canonical name. The first to push holds it; neither
/// takes it from the other on later refreshes.
#[tokio::test]
async fn instances_sharing_an_index_do_not_flip_a_contested_name() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let a_dir = tmp.path().join("a-home");
    let b_dir = tmp.path().join("b-home");
    let a = engine(
        store.clone(),
        &a_dir,
        vec![("alpha", file_domain(tmp.path(), "alpha", "shared", &[]))],
    );
    let b = engine(
        store.clone(),
        &b_dir,
        vec![("beta", file_domain(tmp.path(), "beta", "shared", &[]))],
    );
    a.sync(None).await.unwrap();
    b.sync(None).await.unwrap();
    let settled = store.lock().await.domain_spellings().await.unwrap();
    let (a_pushes, b_pushes) = (a.spelling_replaces_issued(), b.spelling_replaces_issued());

    for _ in 0..2 {
        a.refresh_names().await;
        b.refresh_names().await;
    }

    assert_eq!(
        store.lock().await.domain_spellings().await.unwrap(),
        settled,
        "the contested spelling stays where it is"
    );
    assert_eq!(a.spelling_replaces_issued(), a_pushes);
    assert_eq!(b.spelling_replaces_issued(), b_pushes);
}

/// A canonical name equal to the own name of a domain row this instance no
/// longer registers can never land, because the replace re-claims every
/// row's own name last. The comparison knows it and does not retry on every
/// refresh.
#[tokio::test]
async fn a_name_held_by_an_unregistered_row_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let before = engine(
        store.clone(),
        &tmp.path().join("before"),
        vec![("eng", file_domain(tmp.path(), "old-eng", "eng", &[]))],
    );
    before.sync(None).await.unwrap();

    let after = engine(
        store.clone(),
        &tmp.path().join("after"),
        vec![(
            "eng-knowledge",
            file_domain(tmp.path(), "eng-knowledge", "eng", &[]),
        )],
    );
    after.sync(None).await.unwrap();
    let pushes = after.spelling_replaces_issued();
    after.refresh_names().await;
    after.refresh_names().await;
    assert_eq!(after.spelling_replaces_issued(), pushes);
}

/// `eng-knowledge` declaring `domain_name: eng` with the machine-local alias
/// `old-eng`, beside `ops`, over one store, synced.
async fn eng_with_alias(dir: &Path) -> Engine {
    let mut eng = file_domain(
        dir,
        "eng-knowledge",
        "eng",
        &[(
            "runbook.md",
            engram("Runbook", "runbook", "the rollback steps"),
        )],
    );
    eng.aliases = vec!["old-eng".to_string()];
    let ops = file_domain(dir, "ops", "ops", &[]);
    let engine = engine(
        memory_store().await,
        dir,
        vec![("eng-knowledge", eng), ("ops", ops)],
    );
    engine.sync(None).await.unwrap();
    engine
}

fn write_params(domain: &str, title: &str, content: &str) -> WriteParams {
    WriteParams {
        domain: domain.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
    }
}

/// Every verb takes the canonical name or an alias wherever it takes a
/// domain, and works on the local name behind it.
#[tokio::test]
async fn every_verb_accepts_a_canonical_name_or_an_alias() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = eng_with_alias(tmp.path()).await;

    for (identifier, domain) in [
        ("crystalline://eng/runbook", None),
        ("crystalline://old-eng/runbook", None),
        ("runbook", Some("old-eng")),
        ("runbook", Some("eng")),
    ] {
        let read = engine
            .read_engram(
                &ReadParams {
                    identifier: identifier.to_string(),
                    domain: domain.map(str::to_string),
                    share_link: None,
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap_or_else(|e| panic!("read {identifier} in {domain:?}: {e}"));
        assert_eq!(read["domain"], "eng-knowledge", "{identifier} {domain:?}");
    }

    let hits = |value: &Value| value["hits"].as_array().map_or(0, Vec::len);
    let search = |domains: Vec<&str>| SearchParams {
        query: Some("rollback".to_string()),
        domains: domains.into_iter().map(str::to_string).collect(),
        search_type: Some("text".to_string()),
        ..SearchParams::default()
    };
    let found = engine
        .search_engrams(&search(vec!["eng"]), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(hits(&found) > 0, "the canonical name filters: {found}");
    let nobody = engine
        .search_engrams(&search(vec!["nobody"]), &Scope::Unrestricted)
        .await
        .expect("an unmatched filter entry is no error");
    assert_eq!(hits(&nobody), 0, "an unmatched filter matches nothing");

    let written = engine
        .write_engram(&write_params("old-eng", "Canary", "Roll out slowly."))
        .await
        .unwrap();
    assert!(
        tmp.path().join("eng-knowledge/canary.md").is_file(),
        "the write landed in the local domain: {written}"
    );

    let browsed = engine
        .browse_domain(
            &BrowseParams {
                domain: "eng".to_string(),
                path: None,
                depth: None,
                glob: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(browsed["domain"], "eng-knowledge", "{browsed}");
}

/// A spelling that lands on a domain the caller may not see stays exactly as
/// typed, so the ordinary unknown-domain path answers it in the caller's own
/// words.
#[tokio::test]
async fn a_hidden_domain_keeps_the_spelling_the_caller_typed() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = eng_with_alias(tmp.path()).await;
    let hidden: HashSet<String> = ["eng-knowledge".to_string()].into();
    let none = HashSet::new();

    assert_eq!(engine.localize_visible("eng", &none).await, "eng-knowledge");
    assert_eq!(
        engine.localize_visible("old-eng", &none).await,
        "eng-knowledge"
    );
    assert_eq!(engine.localize_visible("eng", &hidden).await, "eng");
    assert_eq!(engine.localize_visible("old-eng", &hidden).await, "old-eng");
    assert_eq!(engine.localize_visible("nobody", &none).await, "nobody");

    let p = ReadParams {
        identifier: "crystalline://old-eng/runbook".to_string(),
        domain: Some("eng".to_string()),
        share_link: None,
    };
    let seen = engine.localized(&p, &none).await;
    assert_eq!(seen.identifier, "crystalline://eng-knowledge/runbook");
    assert_eq!(seen.domain.as_deref(), Some("eng-knowledge"));
    let unseen = engine.localized(&p, &hidden).await;
    assert_eq!(unseen.identifier, p.identifier);
    assert_eq!(unseen.domain, p.domain);
}

/// A read judges a link by what its domain spelling means, not by its bytes:
/// the file on disk spelling the target with the alias while the index still
/// holds the row it recorded under the canonical name is the same link.
#[tokio::test]
async fn a_link_respelled_with_an_alias_still_reads_as_resolved() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = eng_with_alias(tmp.path()).await;
    let oncall = tmp.path().join("ops/oncall.md");
    std::fs::write(
        &oncall,
        engram("Oncall", "oncall", "Follow [[eng:runbook]] first."),
    )
    .unwrap();
    engine.sync(Some("ops")).await.unwrap();
    assert!(link_resolved(&engine, "ops", "oncall").await);

    // Same line, same target, another spelling of the same domain; no sync.
    std::fs::write(
        &oncall,
        engram("Oncall", "oncall", "Follow [[old-eng:runbook]] first."),
    )
    .unwrap();
    assert!(
        link_resolved(&engine, "ops", "oncall").await,
        "the alias names the same domain the indexed row points at"
    );
}

/// The watcher hands a root `MANIFEST.md` edit to the targeted sync, and that
/// pass moves the name: the new one resolves and the old one no longer does.
/// Deleting the MANIFEST releases the canonical name.
#[tokio::test]
async fn a_targeted_sync_of_the_manifest_moves_the_name() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = eng_with_alias(tmp.path()).await;
    let manifest = tmp.path().join("eng-knowledge/MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replace("domain_name: eng", "domain_name: platform"),
    )
    .unwrap();

    engine
        .sync_paths("eng-knowledge", vec!["MANIFEST.md".to_string()])
        .await
        .unwrap();
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("eng-knowledge")
    );
    assert_eq!(engine.local_domain_name("eng").await, None);

    std::fs::remove_file(&manifest).unwrap();
    engine
        .sync_paths("eng-knowledge", vec!["MANIFEST.md".to_string()])
        .await
        .unwrap();
    assert_eq!(engine.local_domain_name("platform").await, None);
    assert_eq!(
        engine.local_domain_name("old-eng").await.as_deref(),
        Some("eng-knowledge"),
        "the alias outlives the MANIFEST"
    );
}

/// A virtual domain registered over a MANIFEST engram that is already in the
/// index (another instance wrote it) answers to the name that engram
/// declares as soon as the registration returns.
#[tokio::test]
async fn registering_a_virtual_domain_over_an_existing_manifest_reads_its_name() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let first = engine(
        store.clone(),
        &tmp.path().join("first"),
        vec![("scratch", DomainEntry::virtual_domain())],
    );
    first.domain_add_virtual("scratch").await.unwrap();
    first
        .edit_engram(&replace_manifest_name("scratch", "scratch", "notes"))
        .await
        .unwrap();

    let second_home = tmp.path().join("second");
    std::fs::create_dir_all(&second_home).unwrap();
    let second = engine(store.clone(), &second_home, vec![]);
    // The second instance has built its table, and read every virtual
    // MANIFEST it registers (none), before the registration.
    second.refresh_names().await;
    assert_eq!(second.local_domain_name("notes").await, None);

    second.domain_add_virtual("scratch").await.unwrap();
    assert_eq!(
        second.local_domain_name("notes").await.as_deref(),
        Some("scratch")
    );
}

/// A freshly opened engine that never refreshed its names (a one-shot
/// command, or a REST request before any MCP connection) still resolves a
/// virtual domain's declared name on its first verb.
#[tokio::test]
async fn a_fresh_engine_resolves_a_virtual_name_on_its_first_verb() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let first = engine(
        store.clone(),
        &tmp.path().join("first"),
        vec![("scratch", DomainEntry::virtual_domain())],
    );
    first.domain_add_virtual("scratch").await.unwrap();
    first
        .edit_engram(&replace_manifest_name("scratch", "scratch", "notes"))
        .await
        .unwrap();

    let fresh = engine(
        store.clone(),
        &tmp.path().join("fresh"),
        vec![("scratch", DomainEntry::virtual_domain())],
    );
    let read = fresh
        .read_engram(
            &ReadParams {
                identifier: "manifest".to_string(),
                domain: Some("notes".to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(read["domain"], "scratch");
}

/// Of two table builds racing each other, the one that read its inputs last
/// stands, whichever finishes last: a slower build that started earlier
/// never stores its older table over the newer one.
#[tokio::test]
async fn a_slower_older_build_never_replaces_a_newer_table() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = eng_with_alias(tmp.path()).await;
    let input = |canonical: &str| {
        vec![crystalline_core::NameInput {
            local: "eng-knowledge".to_string(),
            canonical: Some(canonical.to_string()),
            aliases: Vec::new(),
        }]
    };
    let older = engine.names_ticket_for_test();
    let newer = engine.names_ticket_for_test();

    engine.install_names_for_test(newer, &input("platform"));
    let standing = engine.install_names_for_test(older, &input("legacy"));

    assert_eq!(standing.resolve("platform"), Some("eng-knowledge"));
    assert_eq!(standing.resolve("legacy"), None);
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("eng-knowledge")
    );
}

/// Another process (the CLI's `domain add` beside a running daemon) registers
/// `eng` while `eng-knowledge` declares `domain_name: eng`. Nothing marks this
/// engine's table stale, yet the new local name must win at once: a call
/// naming `eng` reaches the new domain, never the one whose canonical name it
/// shadows.
#[tokio::test]
async fn a_local_name_registered_by_another_process_wins_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = eng_with_alias(tmp.path()).await;
    assert_eq!(
        engine.local_domain_name("eng").await.as_deref(),
        Some("eng-knowledge"),
        "the table is built and maps the canonical name"
    );

    let mut on_disk = GlobalConfig::default();
    on_disk.domains.insert(
        "eng".to_string(),
        file_domain(tmp.path(), "eng-new", "eng-new", &[]),
    );
    crystalline_core::config::save_yaml(&tmp.path().join("config.yaml"), &on_disk).unwrap();

    let none = HashSet::new();
    assert_eq!(engine.localize_visible("eng", &none).await, "eng");
    let p = ReadParams {
        identifier: "crystalline://eng/x".to_string(),
        domain: Some("eng".to_string()),
        share_link: None,
    };
    let seen = engine.localized(&p, &none).await;
    assert_eq!(seen.domain.as_deref(), Some("eng"));
    assert_eq!(seen.identifier, "crystalline://eng/x");
    // The alias is untouched by the new registration.
    assert_eq!(
        engine.localize_visible("old-eng", &none).await,
        "eng-knowledge"
    );
}

// --- registration write-back (Task 11) ---------------------------------------

/// A folder holding a MANIFEST that declares `domain_name: declared`, not yet
/// registered in any engine's config.
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

/// A folder holding a MANIFEST with no `domain_name` line at all.
fn bare_manifest_folder(dir: &Path, folder: &str, title: &str) -> PathBuf {
    let root = dir.join(folder);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("MANIFEST.md"),
        format!(
            "---\ntype: manifest\ntitle: {title}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: {TODAY}\n---\n\n# {title}\n\n## Scope\n\n- x\n\n## When to Use\n\n- x\n"
        ),
    )
    .unwrap();
    root
}

/// A nameless add of a folder whose MANIFEST declares `domain_name: platform`
/// adopts that name, records it `derived` and writes it into the saved
/// config file (the MANIFEST itself already declared it, so no MANIFEST
/// write happens).
#[tokio::test]
async fn nameless_add_adopts_the_manifest_name_as_derived() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = declared_folder(tmp.path(), "some-folder", "platform");
    let engine = engine(memory_store().await, tmp.path(), vec![]);

    let report = engine
        .domain_add_local(None, Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    assert_eq!(report["domain"], "platform");
    assert_eq!(report["name_origin"], "derived");
    assert_eq!(report["shadowed"], false);
    assert!(report.get("note").is_none(), "{report}");

    let cfg: GlobalConfig =
        crystalline_core::config::load_yaml(&tmp.path().join("config.yaml")).unwrap();
    assert_eq!(
        cfg.domains["platform"].name_origin,
        Some(NameOrigin::Derived)
    );
}

/// The same MANIFEST-declared name, but `platform` is already registered
/// elsewhere: the newcomer steps to `platform-2`, still `derived`, and the
/// result reports the shadow with the one-sentence note. `platform` itself
/// keeps resolving to the first domain.
#[tokio::test]
async fn nameless_add_steps_around_a_taken_manifest_name_and_reports_the_shadow() {
    let tmp = tempfile::tempdir().unwrap();
    let first = file_domain(tmp.path(), "platform-folder", "platform", &[]);
    let engine = engine(memory_store().await, tmp.path(), vec![("platform", first)]);
    engine.sync(None).await.unwrap();

    let folder = declared_folder(tmp.path(), "second-folder", "platform");
    let report = engine
        .domain_add_local(None, Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    assert_eq!(report["domain"], "platform-2");
    assert_eq!(report["name_origin"], "derived");
    assert_eq!(report["shadowed"], true);
    let note = report["note"].as_str().expect("a note is given");
    assert!(note.contains("platform"), "{note}");
    assert!(note.contains("platform-2"), "{note}");
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("platform"),
        "the local name always wins over a shadowed canonical claim"
    );

    // The second folder's own MANIFEST is untouched: it still declares
    // `platform`, not the stepped `platform-2`.
    let manifest = std::fs::read_to_string(folder.join("MANIFEST.md")).unwrap();
    assert!(manifest.contains("domain_name: platform\n"), "{manifest}");
}

/// An explicit name for a folder whose MANIFEST already declares a different
/// name registers under the explicit name, `explicit`, and never rewrites
/// the MANIFEST's own declared name.
#[tokio::test]
async fn an_explicit_name_does_not_rewrite_a_declared_manifest_name() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = declared_folder(tmp.path(), "ops-folder", "platform");
    let engine = engine(memory_store().await, tmp.path(), vec![]);

    let report = engine
        .domain_add_local(Some("ops"), Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    assert_eq!(report["domain"], "ops");
    assert_eq!(report["name_origin"], "explicit");
    let manifest = std::fs::read_to_string(folder.join("MANIFEST.md")).unwrap();
    assert!(manifest.contains("domain_name: platform"), "{manifest}");
    assert!(!manifest.contains("domain_name: ops"), "{manifest}");
}

/// A nameless add of a folder whose MANIFEST declares no name at all falls
/// back to the folder's basename, `derived`, and writes that name back into
/// the MANIFEST that had none.
#[tokio::test]
async fn nameless_add_writes_the_basename_default_back_into_a_bare_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = bare_manifest_folder(tmp.path(), "My Notes", "My Notes");
    let engine = engine(memory_store().await, tmp.path(), vec![]);

    let report = engine
        .domain_add_local(None, Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    assert_eq!(report["domain"], "my-notes");
    assert_eq!(report["name_origin"], "derived");
    let manifest = std::fs::read_to_string(folder.join("MANIFEST.md")).unwrap();
    assert!(manifest.contains("domain_name: my-notes"), "{manifest}");
}

/// A brand-new folder with no MANIFEST at all gets one scaffolded that
/// already carries the derived basename as its `domain_name` (Task 3a); the
/// write-back path itself has nothing left to do.
#[tokio::test]
async fn a_fresh_folder_scaffold_already_carries_the_derived_name() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Brand New");
    let engine = engine(memory_store().await, tmp.path(), vec![]);

    let report = engine
        .domain_add_local(None, Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    assert_eq!(report["manifest_created"], true);
    assert_eq!(report["name_origin"], "derived");
    let manifest = std::fs::read_to_string(folder.join("MANIFEST.md")).unwrap();
    assert!(manifest.contains("domain_name: brand-new"), "{manifest}");
}

/// A brand-new folder given an explicit name gets a scaffold that already
/// carries it, recorded `explicit`.
#[tokio::test]
async fn a_fresh_folder_named_explicitly_scaffolds_with_that_name() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("fresh");
    let engine = engine(memory_store().await, tmp.path(), vec![]);

    let report = engine
        .domain_add_local(Some("fresh"), Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    assert_eq!(report["manifest_created"], true);
    assert_eq!(report["name_origin"], "explicit");
    let manifest = std::fs::read_to_string(folder.join("MANIFEST.md")).unwrap();
    assert!(manifest.contains("domain_name: fresh"), "{manifest}");
}

/// `domain_add_virtual` records `explicit` and the MANIFEST engram declares
/// the name - here entirely from the scaffold template, since a brand-new
/// virtual domain's MANIFEST always carries it already.
#[tokio::test]
async fn virtual_add_records_explicit_and_declares_its_name() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = engine(memory_store().await, tmp.path(), vec![]);

    let report = engine.domain_add_virtual("scratch").await.unwrap();

    assert_eq!(report["name_origin"], "explicit");
    assert_eq!(report["canonical_name"], "scratch");
    assert_eq!(report["shadowed"], false);
    let markdown = engine.manifest_markdown("scratch").await.unwrap();
    assert!(markdown.contains("domain_name: scratch"), "{markdown}");

    let cfg: GlobalConfig =
        crystalline_core::config::load_yaml(&tmp.path().join("config.yaml")).unwrap();
    assert_eq!(
        cfg.domains["scratch"].name_origin,
        Some(NameOrigin::Explicit)
    );
}

/// Re-adding an already-adopted registration is a no-op on the MANIFEST: the
/// first add writes `domain_name` back once, and the second, identical add
/// changes not one byte, with `name_origin` unchanged.
#[tokio::test]
async fn readding_an_adopted_registration_writes_nothing_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = bare_manifest_folder(tmp.path(), "adopt-me", "Adopt Me");
    let engine = engine(memory_store().await, tmp.path(), vec![]);

    let first = engine
        .domain_add_local(None, Some(folder.to_str().unwrap()))
        .await
        .unwrap();
    let name = first["domain"].as_str().unwrap().to_string();
    let after_first = std::fs::read_to_string(folder.join("MANIFEST.md")).unwrap();
    assert!(
        after_first.contains(&format!("domain_name: {name}")),
        "{after_first}"
    );

    let second = engine
        .domain_add_local(Some(&name), Some(folder.to_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(second["adopted"], true);
    assert_eq!(
        second["name_origin"], first["name_origin"],
        "name_origin kept"
    );
    let after_second = std::fs::read_to_string(folder.join("MANIFEST.md")).unwrap();
    assert_eq!(after_first, after_second, "no second write");
}

/// Review Focus 5: an explicit name that looks like a bare YAML number
/// (`1.0`) still round-trips as the string it is: the write-back writes it
/// quoted, so the declared name reads back exactly as given.
#[tokio::test]
async fn a_numeric_looking_explicit_name_round_trips_as_a_string() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = bare_manifest_folder(tmp.path(), "versioned", "Versioned");
    let engine = engine(memory_store().await, tmp.path(), vec![]);

    engine
        .domain_add_local(Some("1.0"), Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    let source = std::fs::read_to_string(folder.join("MANIFEST.md")).unwrap();
    assert_eq!(
        crystalline_core::domain_name_of_source(&source).as_deref(),
        Some("1.0"),
        "{source}"
    );
}

// --- adoption after a sync (Task 17) ------------------------------------------

/// A config file on disk holding `domains`, and an engine over it with a
/// state directory, the way a daemon reads both at startup.
fn adopting_engine(store: SharedStore, dir: &Path, domains: Vec<(&str, DomainEntry)>) -> Engine {
    let mut cfg = GlobalConfig::default();
    for (name, entry) in domains {
        cfg.domains.insert(name.to_string(), entry);
    }
    let config_path = dir.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    Engine::new(store, cfg, None, Some(config_path)).with_state_dir(dir.join("state"))
}

/// The caller sequence every surface runs: a sync, then the adoption.
async fn sync_and_adopt(engine: &Engine) -> Value {
    engine.sync(None).await.unwrap();
    engine.adopt_domain_names().await.unwrap()
}

fn saved(dir: &Path) -> GlobalConfig {
    crystalline_core::config::load_yaml(&dir.join("config.yaml")).unwrap()
}

fn declare(root: &Path, name: &str) {
    std::fs::write(
        root.join("MANIFEST.md"),
        crystalline_core::manifest_template(name, TODAY),
    )
    .unwrap();
}

/// A derived local name follows a `domain_name` the MANIFEST gains: the
/// domain is renamed on this machine, stays derived, and keeps its old
/// local name as an alias.
#[tokio::test]
async fn a_derived_name_follows_a_new_manifest_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = bare_manifest_folder(tmp.path(), "eng", "Eng");
    let engine = adopting_engine(
        memory_store().await,
        tmp.path(),
        vec![(
            "eng",
            DomainEntry::file(root.clone()).with_name_origin(NameOrigin::Derived),
        )],
    );
    sync_and_adopt(&engine).await;

    declare(&root, "platform");
    let report = sync_and_adopt(&engine).await;

    let cfg = saved(tmp.path());
    assert!(!cfg.domains.contains_key("eng"), "{report}");
    let entry = &cfg.domains["platform"];
    assert_eq!(entry.name_origin, Some(NameOrigin::Derived));
    assert!(entry.aliases.contains(&"eng".to_string()), "{entry:?}");
    assert_eq!(entry.canonical_seen.as_deref(), Some("platform"));
    let renamed = report
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["action"] == "renamed")
        .unwrap_or_else(|| panic!("a rename is reported: {report}"));
    assert_eq!(renamed["domain"], "platform", "{report}");
    assert_eq!(renamed["previous"], "eng", "{report}");
    assert_eq!(
        engine.local_domain_name("eng").await.as_deref(),
        Some("platform")
    );
}

/// An explicit local name is kept; the declared name resolves to it.
#[tokio::test]
async fn an_explicit_name_is_kept_and_the_canonical_resolves_to_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = declared_folder(tmp.path(), "eng", "platform");
    let engine = adopting_engine(
        memory_store().await,
        tmp.path(),
        vec![(
            "eng",
            DomainEntry::file(root).with_name_origin(NameOrigin::Explicit),
        )],
    );

    sync_and_adopt(&engine).await;

    let cfg = saved(tmp.path());
    assert!(!cfg.domains.contains_key("platform"));
    let entry = &cfg.domains["eng"];
    assert_eq!(entry.name_origin, Some(NameOrigin::Explicit));
    assert_eq!(entry.canonical_seen.as_deref(), Some("platform"));
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("eng")
    );
}

/// A declared name another domain registers here is left shadowed: the
/// derived domain keeps its name.
#[tokio::test]
async fn a_taken_name_is_left_shadowed() {
    let tmp = tempfile::tempdir().unwrap();
    let ops = declared_folder(tmp.path(), "ops", "ops");
    let eng = declared_folder(tmp.path(), "eng", "ops");
    let engine = adopting_engine(
        memory_store().await,
        tmp.path(),
        vec![
            (
                "ops",
                DomainEntry::file(ops).with_name_origin(NameOrigin::Explicit),
            ),
            (
                "eng",
                DomainEntry::file(eng).with_name_origin(NameOrigin::Derived),
            ),
        ],
    );

    sync_and_adopt(&engine).await;

    let cfg = saved(tmp.path());
    assert!(cfg.domains.contains_key("eng") && cfg.domains.contains_key("ops"));
    assert_eq!(cfg.domains["eng"].canonical_seen.as_deref(), Some("ops"));
    assert!(engine.name_table_now().await.is_shadowed("eng"));
    assert_eq!(
        engine.local_domain_name("ops").await.as_deref(),
        Some("ops")
    );
}

/// A changed canonical name: the previous one becomes an alias and the
/// derived domain follows the new one.
#[tokio::test]
async fn a_changed_canonical_keeps_the_previous_one_as_an_alias() {
    let tmp = tempfile::tempdir().unwrap();
    let root = declared_folder(tmp.path(), "eng", "platform");
    let engine = adopting_engine(
        memory_store().await,
        tmp.path(),
        vec![(
            "eng",
            DomainEntry::file(root.clone()).with_name_origin(NameOrigin::Derived),
        )],
    );
    sync_and_adopt(&engine).await;
    assert!(saved(tmp.path()).domains.contains_key("platform"));

    declare(&root, "core");
    sync_and_adopt(&engine).await;

    let cfg = saved(tmp.path());
    assert!(!cfg.domains.contains_key("platform"));
    let entry = &cfg.domains["core"];
    assert_eq!(entry.name_origin, Some(NameOrigin::Derived));
    assert!(entry.aliases.contains(&"platform".to_string()), "{entry:?}");
    assert!(entry.aliases.contains(&"eng".to_string()), "{entry:?}");
    assert_eq!(entry.canonical_seen.as_deref(), Some("core"));
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("core")
    );
}

/// A domain an environment variable defines is never renamed, and nothing
/// about it is written to the config file.
#[tokio::test]
async fn an_environment_domain_is_never_renamed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = declared_folder(tmp.path(), "envdom", "platform");
    let overlay = crystalline_service::EnvOverlay::from_vars(vec![(
        "CRYSTALLINE_DOMAIN_ENVDOM".to_string(),
        root.display().to_string(),
    )])
    .unwrap();
    let engine =
        adopting_engine(memory_store().await, tmp.path(), vec![]).with_env_overlay(overlay);

    let report = sync_and_adopt(&engine).await;

    assert_eq!(report, serde_json::json!([]), "{report}");
    let cfg = saved(tmp.path());
    assert!(cfg.domains.is_empty(), "{:?}", cfg.domains.keys());
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("envdom")
    );
}

/// The one-time catch-up for a config written before 0.20.0: every entry
/// gets its name origin inferred, local domains declare their name in their
/// MANIFEST, team domains never get one written (explicit or derived), and a
/// second pass writes nothing.
#[tokio::test]
async fn the_catch_up_infers_every_origin_once_and_writes_local_manifests_only() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = bare_manifest_folder(tmp.path(), "eng", "Eng");
    let notes = bare_manifest_folder(tmp.path(), "notes", "Notes");
    let kb = bare_manifest_folder(tmp.path(), "kb", "Kb");
    let handbook = bare_manifest_folder(tmp.path(), "hb", "Handbook");
    let team = |root: PathBuf, repo: &str| DomainEntry {
        origin: Some(crystalline_core::config::OriginConfig {
            repo: repo.to_string(),
            path: None,
            branch: None,
            poll_secs: None,
        }),
        ..DomainEntry::file(root)
    };
    let engine = adopting_engine(
        memory_store().await,
        tmp.path(),
        vec![
            ("eng", DomainEntry::file(eng.clone())),
            ("my-notes", DomainEntry::file(notes.clone())),
            ("kb", team(kb.clone(), "acme/kb")),
            ("handbook", team(handbook.clone(), "acme/eng-handbook")),
        ],
    );
    let kb_before = std::fs::read(kb.join("MANIFEST.md")).unwrap();
    let handbook_before = std::fs::read(handbook.join("MANIFEST.md")).unwrap();

    sync_and_adopt(&engine).await;

    let cfg = saved(tmp.path());
    let origin = |name: &str| cfg.domains[name].name_origin;
    assert_eq!(origin("eng"), Some(NameOrigin::Derived));
    assert_eq!(origin("my-notes"), Some(NameOrigin::Explicit));
    assert_eq!(origin("kb"), Some(NameOrigin::Derived));
    assert_eq!(origin("handbook"), Some(NameOrigin::Explicit));
    let declared = |root: &Path| {
        crystalline_core::domain_name_of_source(
            &std::fs::read_to_string(root.join("MANIFEST.md")).unwrap(),
        )
    };
    assert_eq!(declared(&eng).as_deref(), Some("eng"));
    assert_eq!(declared(&notes).as_deref(), Some("my-notes"));
    assert_eq!(std::fs::read(kb.join("MANIFEST.md")).unwrap(), kb_before);
    assert_eq!(
        std::fs::read(handbook.join("MANIFEST.md")).unwrap(),
        handbook_before,
        "an explicit team domain gets no automatic write either"
    );

    let config_bytes = std::fs::read(tmp.path().join("config.yaml")).unwrap();
    let roots = [&eng, &notes, &kb, &handbook];
    let manifests: Vec<Vec<u8>> = roots
        .iter()
        .map(|root| std::fs::read(root.join("MANIFEST.md")).unwrap())
        .collect();
    let again = sync_and_adopt(&engine).await;
    assert_eq!(again, serde_json::json!([]), "{again}");
    assert_eq!(
        std::fs::read(tmp.path().join("config.yaml")).unwrap(),
        config_bytes,
        "the second pass writes nothing"
    );
    for (root, before) in roots.iter().zip(manifests) {
        assert_eq!(std::fs::read(root.join("MANIFEST.md")).unwrap(), before);
    }
}

/// The rename an adoption runs syncs the renamed domain, and that sync never
/// runs an adoption of its own: only the surfaces that call
/// `adopt_domain_names` after their sync do.
#[tokio::test]
async fn a_rename_and_its_sync_never_run_an_adoption() {
    let tmp = tempfile::tempdir().unwrap();
    let root = declared_folder(tmp.path(), "eng", "eng");
    let engine = adopting_engine(
        memory_store().await,
        tmp.path(),
        vec![(
            "eng",
            DomainEntry::file(root.clone()).with_name_origin(NameOrigin::Derived),
        )],
    );
    engine.sync(None).await.unwrap();
    engine
        .rename_domain_local("eng", "moved", NameOrigin::Derived, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(engine.adoptions_run(), 0);

    declare(&root, "platform");
    engine.sync(None).await.unwrap();
    assert_eq!(engine.adoptions_run(), 0, "a sync alone adopts nothing");
    let report = engine.adopt_domain_names().await.unwrap();
    assert!(
        saved(tmp.path()).domains.contains_key("platform"),
        "{report}"
    );
    assert_eq!(
        engine.adoptions_run(),
        1,
        "the adoption's own rename and its sync run no second one"
    );
}
