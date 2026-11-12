//! The engine's side of the generated `index.md` files: the per-domain memo
//! and lock behind `refresh_index_files`.

use super::*;
use crate::index_files::{Touched, refresh};
use crystalline_core::config::DomainEntry;
use crystalline_index::TursoStore;

const MANIFEST: &str = "---\ntype: manifest\ntitle: notes\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# notes\n\n## Scope\n\n- Everything about notes\n\n## When to Use\n\n- Route here for notes questions\n";

fn engram(title: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\ntags:\n  - t\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\nBody.\n"
    )
}

/// A file domain `notes` with a MANIFEST and `count` engrams over three
/// folders, synced once.
async fn domain(tmp: &Path, count: usize) -> (Arc<Engine>, PathBuf) {
    let root = tmp.join("notes");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), MANIFEST).unwrap();
    for i in 0..count {
        let path = root.join(format!("f{}/note-{i}.md", i % 3));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, engram(&format!("Note {i}"))).unwrap();
    }
    let mut config = GlobalConfig::default();
    config
        .domains
        .insert("notes".to_string(), DomainEntry::file(root.clone()));
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(Arc::new(Mutex::new(store)), config, None, None));
    engine.sync(None).await.unwrap();
    (engine, root)
}

fn params(title: &str, folder: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: "notes".to_string(),
        title: title.to_string(),
        content: Some("Written.".to_string()),
        folder: Some(folder.to_string()),
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
        expected_checksum: None,
    }
}

async fn last_refresh(engine: &Engine, domain: &str) -> crate::index_files::IndexRefresh {
    engine.index_memo(domain).lock().await.last()
}

/// The sync that found the domain read every file once; a write afterwards
/// reads only its own.
#[tokio::test]
async fn a_write_into_a_warm_domain_reads_only_its_own_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, root) = domain(tmp.path(), 30).await;
    let seeded = last_refresh(&engine, "notes").await;
    assert_eq!(
        seeded.read, 31,
        "the first refresh reads every concept file"
    );
    assert_eq!(engine.index_memo("notes").lock().await.len(), 31);

    engine
        .write_engram(&params("Fresh note", "f1"))
        .await
        .unwrap();
    let after = last_refresh(&engine, "notes").await;
    assert_eq!((after.read, after.reused), (1, 31));
    assert!(
        std::fs::read_to_string(root.join("f1/index.md"))
            .unwrap()
            .contains("[Fresh note](")
    );
    let check = refresh(&root, ReflectionMode::Off);
    assert_eq!((check.written, check.removed), (0, 0));
}

/// Refreshes of one domain run one at a time, each reading the disk once it
/// holds the lock, so with a write landing between two of them the files
/// end up as a full pass renders them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refreshes_of_one_domain_run_one_at_a_time() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, root) = domain(tmp.path(), 60).await;
    for round in 0..20 {
        let first = {
            let engine = engine.clone();
            tokio::spawn(async move {
                engine
                    .refresh_index_files("notes", Touched::Paths(Vec::new()))
                    .await;
            })
        };
        let rel = format!("f{}/note-{round}.md", round % 3);
        let path = root.join(&rel);
        let tmp_file = path.with_file_name(".round.tmp");
        std::fs::write(&tmp_file, engram(&format!("Round {round}"))).unwrap();
        std::fs::rename(&tmp_file, &path).unwrap();
        let second = {
            let engine = engine.clone();
            tokio::spawn(async move {
                engine
                    .refresh_index_files("notes", Touched::path(&rel))
                    .await;
            })
        };
        first.await.unwrap();
        second.await.unwrap();
        let check = refresh(&root, ReflectionMode::Off);
        assert_eq!(
            (check.written, check.removed),
            (0, 0),
            "round {round}: the last refresh left an older listing"
        );
    }
}

/// A renamed domain keeps what its refresh remembers; a forgotten one
/// drops it, and so does turning the listings off.
#[tokio::test]
async fn the_memo_follows_a_rename_and_goes_with_the_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, _root) = domain(tmp.path(), 6).await;
    assert_eq!(engine.index_memo("notes").lock().await.len(), 7);

    engine.rekey_index_memo("notes", "renamed");
    assert!(!engine.index_memos.lock().unwrap().contains_key("notes"));
    assert_eq!(engine.index_memo("renamed").lock().await.len(), 7);

    engine.forget_domain("renamed");
    assert!(!engine.index_memos.lock().unwrap().contains_key("renamed"));

    engine
        .refresh_index_files("notes", Touched::Paths(Vec::new()))
        .await;
    assert!(engine.index_memos.lock().unwrap().contains_key("notes"));
    engine.config.write().unwrap().index =
        Some(crystalline_core::config::IndexConfig { files: Some(false) });
    engine
        .refresh_index_files("notes", Touched::Paths(Vec::new()))
        .await;
    assert!(engine.index_memos.lock().unwrap().is_empty());
}
