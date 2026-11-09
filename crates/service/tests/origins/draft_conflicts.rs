//! A draft's conflict, read from what stands: her row, her record and the base
//! snapshot. Nothing is stored, so a conflict cannot go stale and a pass that
//! failed cannot hide one.
//!
//! The team's changes come in two ways. A pull through the mock forge is the
//! ordinary road. Writing the base snapshot straight is the state a direct
//! commit or a failed pass leaves: the snapshot moved and no pass looked at the
//! drafts since.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::origin::{commit_files, draft, engram, manifest, mirror, reviewing_domain};
use crate::support::{MockProvider, sha256_hex};
use crystalline_core::config::{DomainEntry, GlobalConfig, ReviewMode};
use crystalline_index::{Store, TursoStore};
use crystalline_remote::state::{BaseStamp, OriginState, remove_base_file, write_base_file};
use crystalline_service::draft_start::{self, DraftStart};
use crystalline_service::engine::{DraftConflict, DraftConflictKind, EngineError};
use crystalline_service::overlay_journal::{self, ConvergenceRecord};
use crystalline_service::params::{EditParams, MoveParams, WriteParams};
use crystalline_service::{Engine, Scope};
use tokio::sync::Mutex;

use DraftConflictKind::{AddAdd, AddressTaken, DeleteEdit, EditDelete, EditEdit, UnknownStart};

fn plan() -> Vec<u8> {
    engram(
        "Plan",
        "plan",
        "the first line of the plan\n\nthe middle of the plan\n\nthe last line of the plan",
    )
}

/// The plan after bob changed its first line.
fn plan_later() -> Vec<u8> {
    engram(
        "Plan",
        "plan",
        "bob's first line\n\nthe middle of the plan\n\nthe last line of the plan",
    )
}

fn plan_latest() -> Vec<u8> {
    engram(
        "Plan",
        "plan",
        "carol's first line\n\nthe middle of the plan\n\nthe last line of the plan",
    )
}

fn gone() -> Vec<u8> {
    engram("Gone", "gone", "a page she deletes")
}

/// A page with no `permalink:` line: its address is its path's slug.
fn loose() -> Vec<u8> {
    b"---\ntype: engram\ntitle: Loose\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nno permalink line here\n".to_vec()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn scope(actor: &str) -> Scope {
    Scope::User {
        account: actor.to_string(),
        admin: false,
    }
}

/// The id a conflict of `actor` at `path` has, by the rule the brief gives.
fn id(actor: &str, path: &str) -> String {
    format!(
        "d{}",
        &sha256_hex(format!("{actor}\0{path}").as_bytes())[..7]
    )
}

fn conflict(actor: &str, path: &str, kind: DraftConflictKind) -> DraftConflict {
    DraftConflict {
        id: id(actor, path),
        path: path.to_string(),
        kind,
        moved_from: None,
        file: false,
        other: None,
    }
}

/// Whether `dir`'s filesystem folds letter case.
fn folds_case(dir: &Path) -> bool {
    let probe = dir.join("Probe");
    std::fs::create_dir(&probe).unwrap();
    let folds = dir.join("probe").exists();
    std::fs::remove_dir(&probe).unwrap();
    folds
}

struct Team {
    tmp: tempfile::TempDir,
    mock: Arc<MockProvider>,
    eng: Engine,
    /// The repository as it stands, so each push is the whole tree.
    files: BTreeMap<String, Vec<u8>>,
}

async fn team(files: &[(&str, Vec<u8>)]) -> Team {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let mut all = vec![("MANIFEST.md", manifest())];
    all.extend(files.iter().cloned());
    let eng = reviewing_domain(tmp.path(), mock.clone(), &all).await;
    let files = commit_files(&all);
    Team {
        tmp,
        mock,
        eng,
        files,
    }
}

impl Team {
    fn root(&self) -> PathBuf {
        self.tmp.path().join("team-knowledge")
    }

    /// The state directory the fixture names, where the journal and the records
    /// live.
    fn state(&self) -> PathBuf {
        self.tmp.path().to_path_buf()
    }

    fn snapshot_dir(&self) -> PathBuf {
        self.tmp.path().join("origins").join("team")
    }

    /// The team commits these changes and this machine pulls them.
    async fn push(&mut self, changes: &[(&str, Option<Vec<u8>>)]) {
        for (path, bytes) in changes {
            match bytes {
                Some(bytes) => self.files.insert((*path).to_string(), bytes.clone()),
                None => self.files.remove(*path),
            };
        }
        let commit = self.mock.add_commit(self.files.clone());
        self.mock.set_branch("main", &commit);
        self.eng
            .origin_update(Some("team"), &Scope::Unrestricted)
            .await
            .unwrap();
    }

    /// The base snapshot and the folder hold `bytes` at `path` (or nothing), and
    /// no pass ran: a direct commit whose pass failed.
    fn snapshot_has(&self, path: &str, bytes: Option<&[u8]>) {
        let dir = self.snapshot_dir();
        let mut state = OriginState::load(&dir).unwrap().unwrap();
        let file = self.root().join(path);
        match bytes {
            Some(bytes) => {
                state.files.insert(
                    path.to_string(),
                    BaseStamp {
                        sha256: sha256_hex(bytes),
                        size: bytes.len() as u64,
                    },
                );
                write_base_file(&dir, path, bytes).unwrap();
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, bytes).unwrap();
            }
            None => {
                state.files.remove(path);
                remove_base_file(&dir, path).unwrap();
                let _ = std::fs::remove_file(&file);
            }
        }
        state.save(&dir).unwrap();
    }

    /// The snapshot keeps `path` as a key and loses its copy: a damaged state
    /// directory, which says the team has a file there and not what it holds.
    fn copy_lost(&self, path: &str) {
        remove_base_file(&self.snapshot_dir(), path).unwrap();
    }

    /// The base snapshot holds `bytes` at `path` and the folder is not touched:
    /// what a pull or commit leaves when its folder write failed.
    fn snapshot_only(&self, path: &str, bytes: &[u8]) {
        let dir = self.snapshot_dir();
        let mut state = OriginState::load(&dir).unwrap().unwrap();
        state.files.insert(
            path.to_string(),
            BaseStamp {
                sha256: sha256_hex(bytes),
                size: bytes.len() as u64,
            },
        );
        write_base_file(&dir, path, bytes).unwrap();
        state.save(&dir).unwrap();
    }

    /// The pass took `actor`'s row at `path`, and its record with it.
    async fn row_taken(&self, actor: &str, path: &str) {
        {
            let store = self.eng.store();
            let store = store.lock().await;
            let id = store.domain_id("team").await.unwrap().unwrap();
            assert!(store.clear_overlay_entry(id, actor, path).await.unwrap());
        }
        overlay_journal::journal_clear_tombstone(&self.state(), "team", actor, path).unwrap();
        draft_start::clear(&self.state(), "team", actor, path).unwrap();
    }

    async fn sides(&self, actor: &str, path: &str) -> serde_json::Value {
        self.eng
            .draft_conflict_sides("team", actor, &id(actor, path))
            .await
            .unwrap()
    }

    /// `actor` rewrites the page titled `title` through the engine's write verb,
    /// so the draft holds a record.
    async fn write(&self, actor: &str, title: &str, body: &str) {
        self.eng
            .write_engram_as(&write_params(title, body), None, &scope(actor))
            .await
            .unwrap();
    }

    async fn append(&self, actor: &str, identifier: &str, line: &str) {
        self.eng
            .edit_engram_as(&edit_append(identifier, line), None, &scope(actor))
            .await
            .unwrap();
    }

    async fn delete(&self, actor: &str, identifier: &str) {
        self.eng
            .delete_engram_as(
                &crystalline_service::params::DeleteParams {
                    identifier: identifier.to_string(),
                    domain: "team".to_string(),
                    expected_checksum: None,
                },
                None,
                &scope(actor),
            )
            .await
            .unwrap();
    }

    async fn moved(&self, actor: &str, identifier: &str, destination: &str) {
        self.eng
            .move_engram(
                &MoveParams {
                    identifier: identifier.to_string(),
                    domain: "team".to_string(),
                    destination: destination.to_string(),
                    destination_domain: None,
                    permalink: None,
                    update_links: None,
                },
                &scope(actor),
            )
            .await
            .unwrap();
    }

    async fn conflicts(&self, actor: &str) -> Vec<DraftConflict> {
        self.eng.draft_conflicts("team", actor).await.unwrap()
    }

    fn record(&self, actor: &str, path: &str) -> Option<DraftStart> {
        draft_start::load(&self.state(), "team", actor, path).unwrap()
    }

    /// The text of `actor`'s row at `path`.
    async fn row(&self, actor: &str, path: &str) -> String {
        let store = self.eng.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        store
            .overlay_entries(id, actor)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.path == path)
            .unwrap_or_else(|| panic!("{actor} holds no row at {path}"))
            .content
    }

    /// A draft from before the record existed (0.31): a row and its mirror, no
    /// record.
    async fn old_draft(&self, actor: &str, path: &str, text: &str) {
        draft(&self.eng, actor, path, text).await;
        mirror(&self.state(), actor, path, text);
    }

    /// An old deletion: a tombstone row, holding the text it deleted as
    /// `tombstone_record` fills one, and its mirror, no record.
    async fn old_tombstone(&self, actor: &str, path: &str, deleted: &[u8]) {
        let deleted = text(deleted);
        let parsed = crystalline_core::parse_engram(&deleted).unwrap();
        let mut record = crystalline_index::EngramRecord::from_engram(
            &parsed,
            path,
            crystalline_index::FileStamp {
                mtime: 0,
                size: deleted.len() as u64,
                sha256: "0".repeat(64),
            },
        );
        record.content = deleted;
        record.tombstone = true;
        record.permalink = path.to_string();
        {
            let store = self.eng.store();
            let store = store.lock().await;
            let id = store.domain_id("team").await.unwrap().unwrap();
            store.upsert_overlay(id, actor, &record).await.unwrap();
        }
        overlay_journal::journal_tombstone(&self.state(), "team", actor, path).unwrap();
    }

    /// A second engine over the same folders: the config file as it stands, with
    /// `edit` applied, and `store`.
    fn engine_over(
        &self,
        store: Arc<Mutex<dyn Store>>,
        edit: impl FnOnce(&mut GlobalConfig),
    ) -> Engine {
        let config_path = self.tmp.path().join("config.yaml");
        let mut config: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
        config.github = Some(crystalline_core::config::GitHubConfig {
            enabled: Some(true),
            ..Default::default()
        });
        edit(&mut config);
        Engine::new(store, config, None, Some(config_path))
            .with_origin_provider(self.mock.clone())
            .with_origins_dir(self.tmp.path().join("origins"))
            .with_state_dir(self.state())
    }
}

fn write_params(title: &str, body: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: "team".to_string(),
        title: title.to_string(),
        content: Some(body.to_string()),
        folder: None,
        engram_type: None,
        tags: vec!["test".to_string()],
        status: None,
        metadata: None,
        overwrite: true,
        share_link: None,
        model: None,
        expected_checksum: None,
    }
}

fn edit_append(identifier: &str, line: &str) -> EditParams {
    EditParams {
        identifier: identifier.to_string(),
        domain: "team".to_string(),
        operation: "append".to_string(),
        content: Some(line.to_string()),
        key: None,
        value: None,
        values: None,
        find_text: None,
        expected_replacements: None,
        section: None,
        include_subsections: false,
        expected_checksum: None,
        ack_scope: None,
        share_link: None,
        model: None,
    }
}

// --- Step 1: the table, for drafts with a record ----------------------------

#[tokio::test]
async fn a_stale_draft_is_its_authors_conflict() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![],
        "in step until the team moves"
    );

    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );
    assert_eq!(t.conflicts("bob").await, vec![], "bob holds no draft");

    // Bob's draft starts from bob's line, so only a later change is his conflict,
    // and it has an id of his own.
    t.write("bob", "Plan", "bob rewrote the plan").await;
    assert_eq!(t.conflicts("bob").await, vec![]);
    t.push(&[("plan.md", Some(plan_latest()))]).await;
    let bobs = t.conflicts("bob").await;
    assert_eq!(bobs, vec![conflict("bob", "plan.md", EditEdit)]);
    assert_ne!(bobs[0].id, id("alice", "plan.md"));
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );
}

#[tokio::test]
async fn two_people_each_hold_a_conflict_on_one_file() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    t.write("bob", "Plan", "bob rewrote the plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    for actor in ["alice", "bob"] {
        let found = t.conflicts(actor).await;
        assert_eq!(found, vec![conflict(actor, "plan.md", EditEdit)], "{actor}");
        let sides = t
            .eng
            .draft_conflict_sides("team", actor, &found[0].id)
            .await
            .unwrap();
        assert_eq!(
            sides["local"],
            t.row(actor, "plan.md").await,
            "{actor}: {sides}"
        );
        assert!(
            sides["local"]
                .as_str()
                .unwrap()
                .contains(&format!("{actor} rewrote the plan")),
            "{sides}"
        );
        assert_eq!(sides["base"], text(&plan()), "{actor}: her own start");
        assert_eq!(sides["upstream"], text(&plan_later()));
    }
}

#[tokio::test]
async fn a_conflict_survives_a_rebuild() {
    let mut t = team(&[("plan.md", plan()), ("gone.md", gone())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    t.delete("alice", "gone").await;
    t.push(&[
        ("plan.md", Some(plan_later())),
        (
            "gone.md",
            Some(engram("Gone", "gone", "bob kept and changed it")),
        ),
    ])
    .await;
    let before = t.conflicts("alice").await;
    assert_eq!(
        before,
        vec![
            conflict("alice", "gone.md", DeleteEdit),
            conflict("alice", "plan.md", EditEdit),
        ]
    );

    // A fresh index, restored from the journal: the tombstone's row is filled from
    // the base as it is now, which is bob's text, and still reads as hers to
    // settle, because the record says what she deleted.
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let rebuilt = t.engine_over(store, |_| {});
    rebuilt.sync(None).await.unwrap();
    assert_eq!(
        rebuilt.draft_conflicts("team", "alice").await.unwrap(),
        before
    );
}

#[tokio::test]
async fn the_conflict_sides_are_start_mine_and_the_teams() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    let sides = t
        .eng
        .draft_conflict_sides("team", "alice", &id("alice", "plan.md"))
        .await
        .unwrap();
    assert_eq!(
        sides,
        serde_json::json!({
            "id": id("alice", "plan.md"),
            "path": "plan.md",
            "kind": "EditEdit",
            "draft": true,
            "detected_at": null,
            "base": text(&plan()),
            "local": t.row("alice", "plan.md").await,
            "upstream": text(&plan_later()),
            "moved_from": null,
            "other": null,
            "note": null,
        })
    );
}

#[tokio::test]
async fn asking_for_another_persons_conflict_is_not_found() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    let hers = id("alice", "plan.md");
    assert!(
        t.eng
            .draft_conflict_sides("team", "alice", &hers)
            .await
            .is_ok()
    );

    let err = t
        .eng
        .draft_conflict_sides("team", "bob", &hers)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound(_)), "{err:?}");
    // Not by path either: a path is no id, also for her own.
    let err = t
        .eng
        .draft_conflict_sides("team", "alice", "plan.md")
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound(_)), "{err:?}");
    let err = t
        .eng
        .draft_conflict_sides("team", "bob", "plan.md")
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn a_mergeable_stale_draft_is_listed_until_a_pass_has_merged_it() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", "alice's own last word").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    // Bob's first line and her last word fit together, and nothing has merged
    // them yet: what she would share still holds the old first line.
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );

    // What a pass writes: the merged row first, the start second.
    let merged = t
        .row("alice", "plan.md")
        .await
        .replace("the first line of the plan", "bob's first line");
    draft(&t.eng, "alice", "plan.md", &merged).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)],
        "a merged row on the old start is still listed: a crash here replays the merge"
    );
    let mut record = t.record("alice", "plan.md").unwrap();
    record.start = Some(text(&plan_later()));
    draft_start::save(&t.state(), "team", "alice", &record).unwrap();
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn a_landed_move_with_a_failed_pass_is_not_a_conflict() {
    let t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    t.append("alice", "plan", "alice's own last word").await;
    let hers = t.row("alice", "renamed.md").await;
    assert_eq!(
        t.record("alice", "renamed.md").unwrap().from.as_deref(),
        Some("plan.md")
    );

    // Her share committed straight: the team holds her text at the new path and
    // let go of the old one. No pass ran.
    t.snapshot_has("renamed.md", Some(hers.as_bytes()));
    t.snapshot_has("plan.md", None);
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn a_landed_draft_at_its_own_path_with_a_failed_pass_is_not_a_conflict() {
    let t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    let hers = t.row("alice", "plan.md").await;
    t.snapshot_has("plan.md", Some(hers.as_bytes()));
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn a_generated_listing_row_is_not_a_conflict() {
    let t = team(&[("plan.md", plan())]).await;
    // Each has a start the team no longer holds, which would read as a deletion
    // she did not see, were it a page that takes part in a share.
    for path in ["index.md", "notes/index.md", "log.md", ".hidden.md"] {
        let body = engram("Listing", path.trim_end_matches(".md"), "a listing");
        draft(&t.eng, "alice", path, &text(&body)).await;
        draft_start::save(
            &t.state(),
            "team",
            "alice",
            &DraftStart {
                path: path.to_string(),
                from: Some(path.to_string()),
                start: Some("an older listing\n".to_string()),
                start_sha256: None,
                shared: None,
                merged_at: None,
            },
        )
        .unwrap();
    }
    // The control: the same row at a page that takes part.
    draft(
        &t.eng,
        "alice",
        "page.md",
        &text(&engram("Page", "page", "a page")),
    )
    .await;
    draft_start::save(
        &t.state(),
        "team",
        "alice",
        &DraftStart {
            path: "page.md".to_string(),
            from: Some("page.md".to_string()),
            start: Some("an older page\n".to_string()),
            start_sha256: None,
            shared: None,
            merged_at: None,
        },
    )
    .unwrap();
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "page.md", EditDelete)]
    );
}

#[tokio::test]
async fn a_permalink_another_team_file_took_is_address_taken() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Idea", "alice's idea").await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    t.push(&[("other/thought.md", Some(engram("Thought", "idea", "bob's")))])
        .await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            other: Some("other/thought.md".to_string()),
            ..conflict("alice", "idea.md", AddressTaken)
        }]
    );
}

#[tokio::test]
async fn address_taken_wins_over_edit_delete() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    t.push(&[("plan.md", None)]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditDelete)]
    );
    // A later, separate commit gives her address to another page. Keep mine cannot
    // clear that, so it is what she is told.
    t.push(&[("elsewhere.md", Some(engram("Elsewhere", "plan", "bob's")))])
        .await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            other: Some("elsewhere.md".to_string()),
            ..conflict("alice", "plan.md", AddressTaken)
        }]
    );
}

#[tokio::test]
async fn conflicts_answer_with_the_review_key_off() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    let off = t.engine_over(t.eng.store(), |config| {
        config.domains.get_mut("team").unwrap().review = None;
    });
    assert!(!off.reviews_changes("team"));
    assert_eq!(
        off.draft_conflicts("team", "alice").await.unwrap(),
        vec![conflict("alice", "plan.md", EditEdit)]
    );
}

#[tokio::test]
async fn a_domain_with_no_origin_has_no_conflicts_and_no_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("solo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), manifest()).unwrap();
    let mut config = GlobalConfig::default();
    let mut entry = DomainEntry::file(dir);
    entry.review = Some(ReviewMode::Overlay);
    config.domains.insert("solo".to_string(), entry);
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let eng = Engine::new(store, config, None, None)
        .with_state_dir(tmp.path().join("state"))
        .with_origins_dir(tmp.path().join("origins"));
    eng.sync(None).await.unwrap();
    eng.write_engram_as(
        &WriteParams {
            domain: "solo".to_string(),
            ..write_params("Idea", "an idea")
        },
        None,
        &scope("alice"),
    )
    .await
    .unwrap();
    assert_eq!(eng.draft_conflicts("solo", "alice").await.unwrap(), vec![]);
    // A name nobody registered is still an error, not a quiet empty.
    assert!(eng.draft_conflicts("nowhere", "alice").await.is_err());
}

/// C1: a record that cannot be read refuses, and is never read as "no record".
#[tokio::test]
async fn a_broken_record_refuses() {
    let t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    let file = draft_start::record_file(&t.state(), "team", "alice", "plan.md").unwrap();
    std::fs::write(&file, b"{ not a record").unwrap();
    assert!(t.eng.draft_conflicts("team", "alice").await.is_err());
    assert!(
        t.eng
            .draft_conflict_sides("team", "alice", &id("alice", "plan.md"))
            .await
            .is_err()
    );
}

/// C5: a key with no readable copy is a file of the team's, never absent.
#[tokio::test]
async fn a_key_with_no_copy_is_unknown_start() {
    let t = team(&[
        ("plan.md", plan()),
        ("other.md", engram("Other", "other", "x")),
    ])
    .await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    t.copy_lost("plan.md");
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", UnknownStart)]
    );
    let sides = t
        .eng
        .draft_conflict_sides("team", "alice", &id("alice", "plan.md"))
        .await
        .unwrap();
    assert_eq!(sides["upstream"], serde_json::Value::Null, "{sides}");
    assert!(sides["note"].is_string(), "{sides}");

    // An old draft over such a key, and an old tombstone: neither reads as "the
    // team has nothing here".
    t.old_draft(
        "alice",
        "other.md",
        &text(&engram("Other", "other", "hers")),
    )
    .await;
    t.copy_lost("other.md");
    let found = t.conflicts("alice").await;
    assert!(
        found.contains(&conflict("alice", "other.md", UnknownStart)),
        "{found:?}"
    );
}

#[tokio::test]
async fn an_edited_move_in_conflict_lists_once_at_the_draft() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    t.append("alice", "plan", "alice's own last word").await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    t.snapshot_has("plan.md", Some(&plan_later()));
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", EditEdit)
        }],
        "the tombstone half is not a conflict of its own"
    );
    let sides = t
        .eng
        .draft_conflict_sides("team", "alice", &id("alice", "renamed.md"))
        .await
        .unwrap();
    assert_eq!(
        sides["upstream"],
        text(&plan_later()),
        "the team's text where she started"
    );
    assert_eq!(sides["base"], text(&plan()), "{sides}");
    assert_eq!(sides["moved_from"], "plan.md", "{sides}");
    // The team deleting the page she moved is told at the draft too. Today's pass
    // takes the tombstone on that pull; the draft still names where it started.
    t.push(&[("plan.md", None)]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", EditDelete)
        }]
    );
}

/// Her edited case move, shared straight and with no pass after it. On a disk
/// that folds case the team's key keeps the old spelling, on one that keeps the
/// two apart it takes hers. Neither half is a conflict: the lone tombstone would
/// read "you deleted it, the team changed it" on the first.
#[tokio::test]
async fn an_edited_case_move_landed_with_a_failed_pass_is_not_a_conflict_on_either_disk() {
    let t = team(&[("plan.md", plan())]).await;
    let folds = folds_case(t.tmp.path());
    eprintln!("the temp folder folds case: {folds}");
    t.moved("alice", "plan", "Plan.md").await;
    t.append("alice", "plan", "alice's own last word").await;
    let hers = t.row("alice", "Plan.md").await;
    if folds {
        t.snapshot_has("plan.md", Some(hers.as_bytes()));
    } else {
        t.snapshot_has("plan.md", None);
        t.snapshot_has("Plan.md", Some(hers.as_bytes()));
    }
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// A snapshot that holds two spellings of one path has one copy for both on a
/// disk that folds case, so neither can be read. On a disk that keeps them apart
/// each has its own copy.
#[tokio::test]
async fn two_spellings_of_one_path_cannot_be_read_on_a_disk_that_folds_case() {
    let t = team(&[("plan.md", plan())]).await;
    let folds = folds_case(t.tmp.path());
    t.write("alice", "Plan", "alice rewrote the plan").await;
    let theirs = engram("Plan", "plan-two", "bob's other plan");
    let dir = t.snapshot_dir();
    let mut state = OriginState::load(&dir).unwrap().unwrap();
    state.files.insert(
        "Plan.md".to_string(),
        BaseStamp {
            sha256: sha256_hex(&theirs),
            size: theirs.len() as u64,
        },
    );
    write_base_file(&dir, "Plan.md", &theirs).unwrap();
    state.save(&dir).unwrap();

    let found = t.conflicts("alice").await;
    if folds {
        // One copy, now bob's bytes, for two keys: her start cannot be told from
        // it, so it is not read as in step, nor as a change of bob's.
        assert_eq!(found, vec![conflict("alice", "plan.md", UnknownStart)]);
    } else {
        assert_eq!(found, vec![], "plan.md is still her start");
    }
}

// --- Step 4: drafts with no record (option A) --------------------------------

#[tokio::test]
async fn an_old_draft_with_no_record_is_unknown_start() {
    let t = team(&[("plan.md", plan())]).await;
    t.old_draft("alice", "plan.md", &text(&plan_later())).await;
    assert_eq!(t.record("alice", "plan.md"), None);
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", UnknownStart)],
        "not AddAdd: we cannot tell she did not start from the team's file"
    );
    let sides = t
        .eng
        .draft_conflict_sides("team", "alice", &id("alice", "plan.md"))
        .await
        .unwrap();
    assert_eq!(sides["base"], serde_json::Value::Null, "{sides}");
    assert_eq!(sides["local"], text(&plan_later()));
    assert_eq!(sides["upstream"], text(&plan()));
}

#[tokio::test]
async fn an_old_draft_equal_to_the_base_is_no_conflict() {
    let t = team(&[("plan.md", plan())]).await;
    t.old_draft("alice", "plan.md", &text(&plan())).await;
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn an_old_tombstone_over_a_file_the_team_has_is_unknown_start() {
    let t = team(&[("plan.md", plan()), ("gone.md", gone())]).await;
    t.old_tombstone("alice", "plan.md", &plan()).await;
    // The control: a tombstone over a file the team also deleted has landed.
    t.old_tombstone("alice", "gone.md", &gone()).await;
    t.snapshot_has("gone.md", None);
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", UnknownStart)]
    );
}

#[tokio::test]
async fn an_old_new_file_needs_nothing() {
    let t = team(&[("plan.md", plan())]).await;
    t.old_draft("alice", "fresh.md", &text(&engram("Fresh", "fresh", "new")))
        .await;
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn an_old_draft_the_record_file_lists_with_the_file_gone_is_edit_delete() {
    let t = team(&[("plan.md", plan())]).await;
    t.old_draft(
        "alice",
        "fresh.md",
        &text(&engram("Fresh", "fresh", "hers")),
    )
    .await;
    t.old_draft(
        "alice",
        "other.md",
        &text(&engram("Other", "other", "hers")),
    )
    .await;
    // A pull from before 0.32 found the team deleted it under her draft. Bob's
    // entry says nothing about hers.
    let mut record = ConvergenceRecord::default();
    record
        .conflicts
        .insert("alice".to_string(), vec!["fresh.md".to_string()]);
    record
        .conflicts
        .insert("bob".to_string(), vec!["other.md".to_string()]);
    overlay_journal::journal_save_record(&t.state(), "team", &record).unwrap();
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "fresh.md", EditDelete)],
        "not InStep"
    );

    // A report file that cannot be read is no list of nothing: the share would
    // add back what the team deleted.
    std::fs::write(
        t.state()
            .join("overlays")
            .join("team")
            .join(".convergence.json"),
        b"{ broken",
    )
    .unwrap();
    assert!(t.eng.draft_conflicts("team", "alice").await.is_err());
}

#[tokio::test]
async fn an_old_move_lists_once_at_the_draft() {
    // By address: the permalink travelled with the page.
    let t = team(&[("plan.md", plan_later())]).await;
    t.old_tombstone("alice", "plan.md", &plan()).await;
    t.old_draft(
        "alice",
        "renamed.md",
        &text(&plan()).replace("the last line", "alice's last line"),
    )
    .await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", UnknownStart)
        }]
    );

    // By text: a page with no permalink line answers to its path, so the moved
    // copy has another address and only the text is the same.
    let t = team(&[(
        "loose.md",
        text(&loose()).replace("no permalink", "bob's").into_bytes(),
    )])
    .await;
    t.old_tombstone("alice", "loose.md", &loose()).await;
    t.old_draft("alice", "moved/loose.md", &text(&loose()))
        .await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("loose.md".to_string()),
            ..conflict("alice", "moved/loose.md", UnknownStart)
        }]
    );

    // A landed old move is nothing at either end.
    let t = team(&[("plan.md", plan())]).await;
    t.old_tombstone("alice", "plan.md", &plan()).await;
    t.old_draft("alice", "renamed.md", &text(&plan())).await;
    t.snapshot_has("plan.md", None);
    t.snapshot_has("renamed.md", Some(&plan()));
    assert_eq!(t.conflicts("alice").await, vec![]);
}

// --- Review round ------------------------------------------------------------

/// C11: a landing folder that cannot say whether it folds case is not read as
/// folding. Her edited case move whose text the team holds at the old spelling
/// is then not landed, on either disk: from Task 6 on a landed move drops both
/// halves, and on a disk that keeps the two apart that drops her rename.
#[tokio::test]
async fn a_case_move_into_a_folder_that_cannot_say_has_not_landed() {
    let t = team(&[(
        "notes/plan.md",
        engram("Plan", "plan", "the plan in its folder"),
    )])
    .await;
    eprintln!("the temp folder folds case: {}", folds_case(t.tmp.path()));
    t.moved("alice", "plan", "notes/Plan.md").await;
    t.append("alice", "plan", "alice's own last word").await;
    let hers = t.row("alice", "notes/Plan.md").await;
    // The team holds her text at the old spelling, and the landing folder holds
    // no name to ask: the folder file is gone, the snapshot keeps it.
    t.snapshot_only("notes/plan.md", hers.as_bytes());
    // The folder's generated listing goes too, as a folder of one page has none
    // before its first sync.
    for name in ["plan.md", "index.md"] {
        let _ = std::fs::remove_file(t.root().join("notes").join(name));
    }
    let left: Vec<_> = std::fs::read_dir(t.root().join("notes"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
    let listed = vec![DraftConflict {
        moved_from: Some("notes/plan.md".to_string()),
        ..conflict("alice", "notes/Plan.md", EditEdit)
    }];
    assert_eq!(t.conflicts("alice").await, listed, "an empty folder");

    // A folder that cannot be listed, with the file back in it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let notes = t.root().join("notes");
        std::fs::write(notes.join("plan.md"), &hers).unwrap();
        std::fs::set_permissions(&notes, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unlistable = std::fs::read_dir(&notes).is_err();
        let found = t.conflicts("alice").await;
        std::fs::set_permissions(&notes, std::fs::Permissions::from_mode(0o755)).unwrap();
        if unlistable {
            assert_eq!(found, listed, "a folder that cannot be listed");
        } else {
            eprintln!("skipped the unreadable half: this user lists any folder");
        }
    }
}

/// I2: a file a direct commit deleted, whose sync failed, holds no address,
/// though the index still has its row.
#[tokio::test]
async fn a_deleted_team_file_the_index_still_lists_holds_no_address() {
    let t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    t.append("alice", "plan", "alice's own last word").await;
    // The commit deleted plan.md and no sync ran; the pass took her tombstone.
    t.snapshot_has("plan.md", None);
    t.row_taken("alice", "plan.md").await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", EditDelete)
        }],
        "not AddressTaken by a file that is gone"
    );
}

/// I2: a team file in the snapshot that no sync has indexed yet holds its
/// address. Nothing on the share path checks addresses.
#[tokio::test]
async fn a_team_file_the_index_does_not_list_yet_holds_its_address() {
    let t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Idea", "alice's idea").await;
    t.snapshot_has(
        "other/thought.md",
        Some(&engram("Thought", "idea", "bob's")),
    );
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            other: Some("other/thought.md".to_string()),
            ..conflict("alice", "idea.md", AddressTaken)
        }]
    );
    // A page the index lists with older bytes answers to what the snapshot
    // holds now.
    let t = team(&[
        ("plan.md", plan()),
        ("other.md", engram("Other", "other", "x")),
    ])
    .await;
    t.write("alice", "Idea", "alice's idea").await;
    t.snapshot_has("other.md", Some(&engram("Other", "idea", "bob took it")));
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            other: Some("other.md".to_string()),
            ..conflict("alice", "idea.md", AddressTaken)
        }]
    );
    // A page with no permalink line answers to its path's slug.
    let t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Loose", "alice's loose page").await;
    t.snapshot_has("other/loose.md", Some(&loose()));
    let found = t.conflicts("alice").await;
    assert!(
        found.iter().all(|c| c.kind != AddressTaken),
        "other/loose answers to other/loose, not loose: {found:?}"
    );
}

/// I3: an old move whose draft lands on a team file shows that file, which Keep
/// mine would overwrite, and not the source.
#[tokio::test]
async fn an_old_move_onto_a_team_file_shows_that_file() {
    let bobs = engram("Renamed", "renamed", "bob's own page");
    let t = team(&[("plan.md", plan()), ("renamed.md", bobs.clone())]).await;
    t.old_tombstone("alice", "plan.md", &plan()).await;
    t.old_draft(
        "alice",
        "renamed.md",
        &text(&plan()).replace("the last line", "alice's last line"),
    )
    .await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", UnknownStart)
        }]
    );
    assert_eq!(
        t.sides("alice", "renamed.md").await["upstream"],
        text(&bobs)
    );
}

/// I3, with a record: the start cannot be read, the team holds a file where she
/// moved to, and that file is what the conflict shows.
#[tokio::test]
async fn a_move_with_an_unreadable_start_onto_a_team_file_shows_that_file() {
    let t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    let bobs = engram("Renamed", "renamed", "bob's own page");
    t.snapshot_has("renamed.md", Some(&bobs));
    t.copy_lost("plan.md");
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", UnknownStart)
        }]
    );
    assert_eq!(
        t.sides("alice", "renamed.md").await["upstream"],
        text(&bobs)
    );
}

/// M1: an old move whose source the team deleted is a deletion she has to see,
/// not a page that comes back under her new name.
#[tokio::test]
async fn an_old_move_whose_source_the_team_deleted_is_edit_delete() {
    let t = team(&[("plan.md", plan())]).await;
    t.old_tombstone("alice", "plan.md", &plan()).await;
    t.old_draft(
        "alice",
        "renamed.md",
        &text(&plan()).replace("the last line", "alice's last line"),
    )
    .await;
    t.snapshot_has("plan.md", None);
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", EditDelete)
        }]
    );
}

/// M5: two sides that need a note keep both.
#[tokio::test]
async fn two_sides_with_no_text_keep_both_notes() {
    let t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    let mut record = t.record("alice", "plan.md").unwrap();
    record.start = None;
    record.start_sha256 = Some(sha256_hex(b"bytes that were not text"));
    draft_start::save(&t.state(), "team", "alice", &record).unwrap();
    t.snapshot_has("plan.md", Some(b"\xff\xfe not text"));
    let sides = t.sides("alice", "plan.md").await;
    let note = sides["note"].as_str().unwrap();
    assert!(note.contains("base"), "{note}");
    assert!(note.contains("upstream"), "{note}");
}

/// M6: a row with a record reads `AddAdd` when the team adds a file where she
/// holds a new one, and the sides show the team's file.
#[tokio::test]
async fn a_team_file_at_her_new_files_path_is_add_add() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Idea", "alice's idea").await;
    let bobs = engram("Idea", "idea", "bob's idea");
    t.push(&[("idea.md", Some(bobs.clone()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "idea.md", AddAdd)]
    );
    let sides = t.sides("alice", "idea.md").await;
    assert_eq!(sides["upstream"], text(&bobs), "{sides}");
    assert_eq!(sides["base"], serde_json::Value::Null, "{sides}");
}

/// M6: the same where she moved to, which reads the team's file there, not the
/// text where she started.
#[tokio::test]
async fn a_team_file_at_the_path_she_moved_to_is_add_add_with_its_text() {
    let t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    let bobs = engram("Renamed", "renamed", "bob's own page");
    t.snapshot_has("renamed.md", Some(&bobs));
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", AddAdd)
        }]
    );
    assert_eq!(
        t.sides("alice", "renamed.md").await["upstream"],
        text(&bobs)
    );
}

// --- Fix round 2 ---------------------------------------------------------------

/// N1: her share of the new path landed and the old one stands, and bob changed
/// the old one. The conflict is about bob's text where she started, not about her
/// own text at her new path.
#[tokio::test]
async fn a_moved_draft_whose_new_path_landed_shows_the_teams_text_where_she_started() {
    let t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    t.append("alice", "plan", "alice's own last word").await;
    let hers = t.row("alice", "renamed.md").await;
    // A share that named only renamed.md landed, then bob edited plan.md.
    t.snapshot_has("renamed.md", Some(hers.as_bytes()));
    t.snapshot_has("plan.md", Some(&plan_later()));
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", EditEdit)
        }]
    );
    let sides = t.sides("alice", "renamed.md").await;
    assert_eq!(sides["upstream"], text(&plan_later()), "{sides}");
    assert_eq!(sides["local"], hers, "{sides}");
}

/// N1, where she started cannot be read: `UnknownStart`, and the side says so
/// rather than showing her own text as the team's.
#[tokio::test]
async fn a_moved_draft_whose_new_path_landed_with_an_unreadable_start_says_so() {
    let t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    t.append("alice", "plan", "alice's own last word").await;
    let hers = t.row("alice", "renamed.md").await;
    t.snapshot_has("renamed.md", Some(hers.as_bytes()));
    t.copy_lost("plan.md");
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", UnknownStart)
        }]
    );
    let sides = t.sides("alice", "renamed.md").await;
    assert_eq!(sides["upstream"], serde_json::Value::Null, "{sides}");
    assert!(
        sides["note"]
            .as_str()
            .is_some_and(|note| note.contains("upstream") && note.contains("readable")),
        "{sides}"
    );
}

/// I2: a file the index never holds as a page (a log, a listing, a dotfile) holds
/// no address, whatever its frontmatter says.
#[tokio::test]
async fn a_file_that_takes_no_part_holds_no_address() {
    let t = team(&[("plan.md", plan())]).await;
    t.write("alice", "Idea", "alice's idea").await;
    t.snapshot_has(
        "notes/log.md",
        Some(&engram("Log", "idea", "the team's log")),
    );
    t.snapshot_has(
        "notes/.idea.md",
        Some(&engram("Hidden", "idea", "a dotfile")),
    );
    assert_eq!(t.conflicts("alice").await, vec![]);

    // A generated listing answers to its path's slug, which an ordinary page of
    // another spelling can share: still no clash, as the listing is no page.
    let t = team(&[("plan.md", plan())]).await;
    t.snapshot_has(
        "notes/index.md",
        Some(b"# Contents\n\n- [Plan](../plan.md)\n"),
    );
    t.old_draft(
        "alice",
        "notes/Index.md",
        "---\ntype: engram\ntitle: Index\nstatus: current\n---\n\nher page\n",
    )
    .await;
    let found = t.conflicts("alice").await;
    assert!(found.iter().all(|c| c.kind != AddressTaken), "{found:?}");
}

/// Her own other drafts say what their paths answer to in her view, as the pass
/// reads them: a draft that gives plan.md a new address frees the old one for her
/// new page.
#[tokio::test]
async fn an_address_her_own_draft_gave_up_is_free_for_her_other_draft() {
    let t = team(&[("plan.md", plan())]).await;
    let renamed = text(&plan()).replace("permalink: plan\n", "permalink: plan-two\n");
    draft(&t.eng, "alice", "plan.md", &renamed).await;
    draft_start::save(
        &t.state(),
        "team",
        "alice",
        &DraftStart {
            path: "plan.md".to_string(),
            from: Some("plan.md".to_string()),
            start: Some(text(&plan())),
            start_sha256: None,
            shared: None,
            merged_at: None,
        },
    )
    .unwrap();
    draft(
        &t.eng,
        "alice",
        "idea.md",
        &text(&engram("Idea", "plan", "her new page")),
    )
    .await;
    draft_start::save(
        &t.state(),
        "team",
        "alice",
        &DraftStart {
            path: "idea.md".to_string(),
            from: None,
            start: None,
            start_sha256: None,
            shared: None,
            merged_at: None,
        },
    )
    .unwrap();
    assert_eq!(t.conflicts("alice").await, vec![]);
}
