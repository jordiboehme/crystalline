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
use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
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
