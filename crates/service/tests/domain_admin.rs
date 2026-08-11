//! Engine-level tests for the Group D domain-administration verbs:
//! unregister, GitHub status/disconnect, archive file reads and imports.

mod support;

use std::sync::Arc;

use crystalline_core::config::{
    DomainEntry, GitHubConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::params::{ListDomainsParams, ReadParams, SearchParams};
use tokio::sync::Mutex;

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";
const BETA: &str = "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Beta\n\nThe beta rule.\n";
const MANIFEST: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n";

/// A file domain "eng" (MANIFEST + alpha), domains_root pinned inside the
/// temp dir so nothing this suite creates can land in a real home folder.
async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    // Pin the domains root (Option<PathBuf>, config.rs:52) inside the temp
    // dir so nothing this suite creates can land in a real home folder.
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), MANIFEST).unwrap();
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// Unregister removes the registration and the index rows and nothing else:
/// the files stay exactly where they were, and the name is free again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregister_keeps_files_and_clears_the_index() {
    let (tmp, engine) = engine().await;
    // Indexed before: the engram is findable.
    let hits = engine
        .search_engrams(&SearchParams {
            query: Some("alpha".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(hits.to_string().contains("alpha"), "{hits}");

    let report = engine.domain_remove("eng").await.unwrap();
    assert_eq!(report["unregistered"], true);
    assert_eq!(report["files_kept"], true);

    // The files survive; the registration and the index rows do not.
    assert!(tmp.path().join("eng/alpha.md").exists());
    let listing = engine
        .list_domains(&ListDomainsParams::default())
        .await
        .unwrap();
    assert!(!listing.to_string().contains("\"eng\""), "{listing}");
    let hits = engine
        .search_engrams(&SearchParams {
            query: Some("alpha".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !hits.to_string().contains("alpha.md"),
        "index rows cleared: {hits}"
    );

    // Removing again reports the honest miss.
    assert!(engine.domain_remove("eng").await.is_err());
}

/// The archive source: every file of the domain, MANIFEST included, path
/// plus exact content - the portable view both storage kinds share.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domain_files_serves_manifest_and_engrams_verbatim() {
    let (_tmp, engine) = engine().await;
    let files = engine.domain_files("eng").await.unwrap();
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert!(paths.contains(&"MANIFEST.md"), "{paths:?}");
    assert!(paths.contains(&"alpha.md"));
    let alpha = files.iter().find(|(p, _)| p == "alpha.md").unwrap();
    assert_eq!(alpha.1, ALPHA, "exact bytes, not a re-serialization");
    assert!(engine.domain_files("ghost").await.is_err());
}

/// The same contract for a virtual domain, whose files exist nowhere but the
/// database: the archive of a domain with no folder is just as faithful.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domain_files_reads_a_virtual_domain_from_the_database() {
    let (_tmp, engine) = engine().await;
    engine.domain_add_virtual("scratch").await.unwrap();
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("alpha.md"), ALPHA).unwrap();
    engine
        .import_domain("scratch", src.path(), false, false)
        .await
        .unwrap();

    let files = engine.domain_files("scratch").await.unwrap();
    let alpha = files.iter().find(|(p, _)| p == "alpha.md").unwrap();
    assert_eq!(alpha.1, ALPHA, "exact bytes for the database-backed kind");
}

// --- archive import: one verb for preview and commit ------------------------

/// The import classification and both policies, on a file domain: create,
/// same-path collision skipped then overwritten, cross-path permalink
/// collision refused under BOTH policies, unparseable marked invalid,
/// MANIFEST ignored, dry_run writes nothing - and what lands is indexed
/// through the same targeted sync the watcher uses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_files_walks_the_classification_on_a_file_domain() {
    let (tmp, engine) = engine().await;
    let beta_edit = BETA.replace("The beta rule.", "The sharper beta rule.");

    // Dry run first: full classification, zero writes.
    let preview = engine
        .import_domain_files(
            "eng",
            &[
                ("beta.md".to_string(), BETA.to_string()),
                ("alpha.md".to_string(), ALPHA.to_string()),
                (
                    "MANIFEST.md".to_string(),
                    "---\nanything\n---\n".to_string(),
                ),
                ("broken.md".to_string(), "no frontmatter at all".to_string()),
            ],
            false,
            true,
        )
        .await
        .unwrap();
    assert_eq!(preview["created"], 1, "{preview}");
    assert_eq!(preview["skipped"], 1);
    assert_eq!(preview["ignored"], 1);
    assert_eq!(preview["invalid"], 1);
    assert!(
        !tmp.path().join("eng/beta.md").exists(),
        "dry_run writes nothing"
    );

    // Commit with skip: beta lands and is findable (the index synced).
    engine
        .import_domain_files(
            "eng",
            &[("beta.md".to_string(), BETA.to_string())],
            false,
            false,
        )
        .await
        .unwrap();
    assert!(tmp.path().join("eng/beta.md").exists());
    let hits = engine
        .search_engrams(&SearchParams {
            query: Some("beta".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(hits.to_string().contains("beta"), "{hits}");
    // Findable by identifier too: the targeted sync really indexed the file,
    // rather than the query text merely echoing in the response.
    engine
        .read_engram(&ReadParams {
            identifier: "beta".to_string(),
            domain: Some("eng".to_string()),
        })
        .await
        .unwrap();

    // Same path again: skip under skip, overwrite under overwrite.
    let skipped = engine
        .import_domain_files(
            "eng",
            &[("beta.md".to_string(), beta_edit.clone())],
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(skipped["skipped"], 1);
    let replaced = engine
        .import_domain_files(
            "eng",
            &[("beta.md".to_string(), beta_edit.clone())],
            true,
            false,
        )
        .await
        .unwrap();
    assert_eq!(replaced["overwritten"], 1);
    assert!(
        std::fs::read_to_string(tmp.path().join("eng/beta.md"))
            .unwrap()
            .contains("sharper")
    );

    // A permalink collision at a DIFFERENT path never overwrites: two files
    // must not claim one permalink.
    let cross = engine
        .import_domain_files(
            "eng",
            &[("elsewhere/beta.md".to_string(), BETA.to_string())],
            true,
            false,
        )
        .await
        .unwrap();
    assert_eq!(cross["skipped"], 1, "{cross}");
    assert!(!tmp.path().join("eng/elsewhere/beta.md").exists());

    // A traversal entry never escapes the domain root: an archive is untrusted
    // input, and `..` is refused rather than joined.
    let escape = engine
        .import_domain_files(
            "eng",
            &[("../outside.md".to_string(), BETA.to_string())],
            true,
            false,
        )
        .await
        .unwrap();
    assert_eq!(escape["invalid"], 1, "{escape}");
    assert!(!tmp.path().join("outside.md").exists());
}

/// Two entries of ONE batch claiming one permalink: the first lands, the
/// second is refused, so a batch can never seat two files on one permalink
/// however the snapshot was taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_files_refuses_an_in_batch_permalink_collision() {
    let (tmp, engine) = engine().await;
    let report = engine
        .import_domain_files(
            "eng",
            &[
                ("beta.md".to_string(), BETA.to_string()),
                ("also/beta.md".to_string(), BETA.to_string()),
            ],
            true,
            false,
        )
        .await
        .unwrap();
    assert_eq!(report["created"], 1, "{report}");
    assert_eq!(report["skipped"], 1, "{report}");
    assert!(tmp.path().join("eng/beta.md").exists());
    assert!(!tmp.path().join("eng/also/beta.md").exists());
}

/// The same verb serves virtual domains through index_markdown; the folder
/// entry auto-creates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_files_lands_in_a_virtual_domain_too() {
    let (_tmp, engine) = engine().await;
    engine.domain_add_virtual("pad").await.unwrap();
    let preview = engine
        .import_domain_files(
            "pad",
            &[("notes/beta.md".to_string(), BETA.to_string())],
            false,
            true,
        )
        .await
        .unwrap();
    assert_eq!(preview["created"], 1, "{preview}");
    assert!(
        engine
            .read_engram(&ReadParams {
                identifier: "beta".to_string(),
                domain: Some("pad".to_string()),
            })
            .await
            .is_err(),
        "dry_run writes nothing on the virtual arm either"
    );

    let done = engine
        .import_domain_files(
            "pad",
            &[("notes/beta.md".to_string(), BETA.to_string())],
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(done["created"], 1, "{done}");
    let read = engine
        .read_engram(&ReadParams {
            identifier: "beta".to_string(),
            domain: Some("pad".to_string()),
        })
        .await
        .unwrap();
    assert!(read.to_string().contains("The beta rule."), "{read}");
}

// --- GitHub connection: status, readiness, disconnect -----------------------

/// The base fixture plus GitHub: enabled in config, tokens redirected to a
/// file inside the temp dir, network replaced by the stub validator that
/// accepts any token as user "octo".
async fn engine_with_github() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    // Pin the domains root (Option<PathBuf>, config.rs:52) inside the temp
    // dir so nothing this suite creates can land in a real home folder.
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), MANIFEST).unwrap();
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_token_store_dir(root.join("tokens"))
            .with_connect_auth(Arc::new(support::StubConnectAuth::accepting("octo"))),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// The status verb never leaks token material and walks the connect
/// lifecycle: disconnected -> PAT connect -> connected(user, store kind) ->
/// disconnect -> disconnected. The PAT itself appears in no output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn github_connection_walks_the_lifecycle_without_leaking_tokens() {
    let (_tmp, engine) = engine_with_github().await;

    let before = engine.github_connection().await.unwrap();
    assert!(!before.connected);
    assert!(before.user.is_none());
    assert!(before.pending.is_none());

    engine
        .connect_with_token("pat-secret-123", None)
        .await
        .unwrap();
    let after = engine.github_connection().await.unwrap();
    assert!(after.connected);
    assert_eq!(after.user.as_deref(), Some("octo"));
    assert_eq!(after.token_store.as_deref(), Some("file"));
    let dump = serde_json::to_string(&after).unwrap();
    assert!(
        !dump.contains("pat-secret-123"),
        "no token material ever: {dump}"
    );

    let gone = engine.github_disconnect().await.unwrap();
    assert_eq!(gone["connected"], false);
    let end = engine.github_connection().await.unwrap();
    assert!(!end.connected, "the stored credential is really gone");
}

/// Readiness is what team registration gates on: enabled alone is not
/// enough, a loadable credential is required too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn github_ready_requires_enabled_and_a_credential() {
    let (_tmp, engine) = engine_with_github().await; // github.enabled=true in fixture
    assert!(!engine.github_ready().await, "enabled but no credential");
    engine.connect_with_token("pat", None).await.unwrap();
    assert!(engine.github_ready().await);
    engine.github_disconnect().await.unwrap();
    assert!(!engine.github_ready().await);
}
