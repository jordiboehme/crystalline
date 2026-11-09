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

use super::origin::{draft, engram, manifest, manifest_sharing_direct, mirror, reviewing_domain};
use crate::support::{MockProvider, sha256_hex};
use crystalline_core::config::{DomainEntry, GlobalConfig, ReviewMode};
use crystalline_index::{Store, TursoStore};
use crystalline_remote::ops::DiscardTarget;
use crystalline_remote::provider::ProposalState;
use crystalline_remote::state::{
    BaseStamp, OriginState, ProposalStatus, remove_base_file, write_base_file,
};
use crystalline_service::draft_start::{self, DraftStart, SharedText};
use crystalline_service::engine::{
    DraftConflict, DraftConflictKind, EngineError, PreviewCredential, ShareActor,
};
use crystalline_service::overlay_journal::{self, ConvergenceRecord};
use crystalline_service::params::{EditParams, MoveParams, ReadParams, WriteParams};
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
}

async fn team(files: &[(&str, Vec<u8>)]) -> Team {
    team_with(manifest(), files).await
}

/// [`team`] whose MANIFEST says `sharing: direct`, so a share is a commit.
async fn team_direct(files: &[(&str, Vec<u8>)]) -> Team {
    team_with(manifest_sharing_direct(), files).await
}

async fn team_with(manifest: Vec<u8>, files: &[(&str, Vec<u8>)]) -> Team {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let mut all = vec![("MANIFEST.md", manifest)];
    all.extend(files.iter().cloned());
    let eng = reviewing_domain(tmp.path(), mock.clone(), &all).await;
    Team { tmp, mock, eng }
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

    /// The repository as it stands on `main`, so each push is the whole tree.
    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        let head = self.mock.branch_commit("main").unwrap();
        self.mock
            .commit_paths(&head)
            .into_iter()
            .map(|path| {
                let bytes = self.mock.commit_file(&head, &path).unwrap();
                (path, bytes)
            })
            .collect()
    }

    /// What the forge's `main` holds at `path`.
    fn forge_file(&self, path: &str) -> Option<String> {
        let head = self.mock.branch_commit("main").unwrap();
        self.mock.commit_file(&head, path).map(|bytes| text(&bytes))
    }

    /// The team commits these changes, and this machine has not pulled them.
    fn commit(&self, changes: &[(&str, Option<Vec<u8>>)]) {
        let mut files = self.tree();
        for (path, bytes) in changes {
            match bytes {
                Some(bytes) => files.insert((*path).to_string(), bytes.clone()),
                None => files.remove(*path),
            };
        }
        let commit = self.mock.add_commit(files);
        self.mock.set_branch("main", &commit);
    }

    /// What the forge holds at `path` on `branch`, as bytes.
    fn branch_file(&self, branch: &str, path: &str) -> Option<Vec<u8>> {
        let head = self.mock.branch_commit(branch).unwrap();
        self.mock.commit_file(&head, path)
    }

    /// The team commits these changes and this machine pulls them, but the
    /// convergence pass of that pull fails: the base moved and no pass looked at
    /// the drafts.
    async fn push_with_no_pass(&self, changes: &[(&str, Option<Vec<u8>>)]) {
        self.commit(changes);
        self.eng.fail_next_convergence();
        self.eng
            .origin_update(Some("team"), &Scope::Unrestricted)
            .await
            .unwrap();
    }

    /// `actor` shares `files`, or everything they hold.
    async fn share(&self, actor: &str, files: Option<&[&str]>) -> serde_json::Value {
        let files: Option<Vec<String>> =
            files.map(|files| files.iter().map(|path| path.to_string()).collect());
        self.eng
            .origin_share(
                "team",
                None,
                None,
                None,
                files.as_deref(),
                ShareActor::Account(actor.to_string()),
            )
            .await
            .unwrap()
    }

    /// What a share of `files` by `actor` would do.
    async fn preview(&self, actor: &str, files: Option<&[&str]>) -> serde_json::Value {
        let files: Option<Vec<String>> =
            files.map(|files| files.iter().map(|path| path.to_string()).collect());
        self.eng
            .origin_share_preview(
                "team",
                None,
                None,
                files.as_deref(),
                ShareActor::Account(actor.to_string()),
                PreviewCredential::ActingIdentity,
            )
            .await
            .unwrap()
    }

    /// `actor` discards their draft at `path`.
    async fn discard(&self, actor: &str, path: &str) {
        self.eng
            .discard_local_changes(
                "team",
                &[DiscardTarget {
                    path: path.to_string(),
                    sha256: None,
                }],
                &ShareActor::Account(actor.to_string()),
            )
            .await
            .unwrap();
    }

    /// The team commits these changes and this machine pulls them.
    async fn push(&mut self, changes: &[(&str, Option<Vec<u8>>)]) {
        self.push_as(changes, &Scope::Unrestricted).await;
    }

    /// [`Team::push`], with the pull run by `scope`.
    async fn push_as(&mut self, changes: &[(&str, Option<Vec<u8>>)], scope: &Scope) {
        let mut files = self.tree();
        for (path, bytes) in changes {
            match bytes {
                Some(bytes) => files.insert((*path).to_string(), bytes.clone()),
                None => files.remove(*path),
            };
        }
        let commit = self.mock.add_commit(files);
        self.mock.set_branch("main", &commit);
        self.eng.origin_update(Some("team"), scope).await.unwrap();
    }

    /// What `actor` reads at `identifier`.
    async fn reads(&self, actor: &str, identifier: &str) -> String {
        self.eng
            .read_engram(
                &ReadParams {
                    identifier: identifier.to_string(),
                    domain: Some("team".to_string()),
                    share_link: None,
                    include_images: None,
                },
                &scope(actor),
            )
            .await
            .unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The exact paths `actor` holds a row at, with whether each is a tombstone.
    async fn rows(&self, actor: &str) -> Vec<(String, bool)> {
        let store = self.eng.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        let mut rows: Vec<(String, bool)> = store
            .overlay_entries(id, actor)
            .await
            .unwrap()
            .into_iter()
            .map(|row| (row.path, row.tombstone))
            .collect();
        rows.sort();
        rows
    }

    /// `actor` replaces `find` with `with` in the page `identifier`.
    async fn replace(&self, actor: &str, identifier: &str, find: &str, with: &str) {
        let mut p = edit_append(identifier, with);
        p.operation = "find_replace".to_string();
        p.find_text = Some(find.to_string());
        self.eng
            .edit_engram_as(&p, None, &scope(actor))
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
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", "alice's own last word").await;
    // Written straight, as a direct commit whose pass failed: a pull's pass would
    // merge it.
    t.snapshot_has("plan.md", Some(&plan_later()));
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
                merging: None,
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
            merging: None,
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
            merging: None,
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
            merging: None,
        },
    )
    .unwrap();
    assert_eq!(t.conflicts("alice").await, vec![]);
}

// --- The pass merges -----------------------------------------------------------
//
// A pull asks every row of every person: a stale draft that merges cleanly takes
// the team's change and its start moves on; what does not merge stands, untouched,
// as a conflict.

const BOB_LINE: &str = "bob's first line";
const HER_WORD: &str = "alice's own last word";

/// The plan after bob's first line and then carol's middle.
fn plan_carol() -> Vec<u8> {
    engram(
        "Plan",
        "plan",
        "bob's first line\n\ncarol's middle\n\nthe last line of the plan",
    )
}

#[tokio::test]
async fn a_clean_merge_rewrites_her_draft_and_she_reads_both_lines() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    let read = t.reads("alice", "plan").await;
    assert!(read.contains(BOB_LINE) && read.contains(HER_WORD), "{read}");
    assert_eq!(
        std::fs::read(t.root().join("plan.md")).unwrap(),
        plan_later(),
        "the team's file is not touched by her merge"
    );
    assert_eq!(t.conflicts("alice").await, vec![]);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.start.as_deref(), Some(text(&plan_later()).as_str()));
    assert!(record.merged_at.is_some(), "she is told: {record:?}");
}

#[tokio::test]
async fn a_same_line_edit_keeps_her_text_and_stands_as_a_conflict() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.replace(
        "alice",
        "plan",
        "the first line of the plan",
        "alice's first line",
    )
    .await;
    let hers = t.row("alice", "plan.md").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    assert_eq!(t.row("alice", "plan.md").await, hers, "her text stays");
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.start.as_deref(), Some(text(&plan()).as_str()));
    assert_eq!(record.merged_at, None);
}

#[tokio::test]
async fn two_peer_edits_in_a_row_both_merge_and_survive_a_rebuild() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    t.push(&[("plan.md", Some(plan_carol()))]).await;

    let read = t.reads("alice", "plan").await;
    for line in [BOB_LINE, "carol's middle", HER_WORD] {
        assert!(read.contains(line), "{line}: {read}");
    }
    assert_eq!(t.conflicts("alice").await, vec![]);

    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let rebuilt = t.engine_over(store, |_| {});
    rebuilt.sync(None).await.unwrap();
    assert_eq!(
        rebuilt.draft_conflicts("team", "alice").await.unwrap(),
        vec![]
    );
    let row = {
        let store = rebuilt.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        store.overlay_entries(id, "alice").await.unwrap()
    };
    assert_eq!(row.len(), 1, "{row:?}");
    for line in [BOB_LINE, "carol's middle", HER_WORD] {
        assert!(row[0].content.contains(line), "{line}: {}", row[0].content);
    }
}

#[tokio::test]
async fn a_draft_that_became_the_teams_text_ends() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.replace("alice", "plan", "the first line of the plan", BOB_LINE)
        .await;
    // Bob made the same change. Her row also carries her `generated` stamp, which
    // is nobody's words.
    t.push(&[("plan.md", Some(plan_later()))]).await;

    assert_eq!(t.rows("alice").await, vec![]);
    assert_eq!(t.record("alice", "plan.md"), None);
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn a_peers_merge_in_her_draft_is_done_by_a_pull_she_did_not_run() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.push_as(&[("plan.md", Some(plan_later()))], &scope("bob"))
        .await;

    let row = t.row("alice", "plan.md").await;
    assert!(row.contains(BOB_LINE) && row.contains(HER_WORD), "{row}");
    assert!(t.record("alice", "plan.md").unwrap().merged_at.is_some());
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn a_failed_merge_write_leaves_the_row() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let hers = t.row("alice", "plan.md").await;
    // Bob's change breaks the frontmatter. The merge comes out as text no page
    // can be read from, so the pass cannot write it.
    let broken = text(&plan_later()).replace("title: Plan", "title: [Plan");
    t.push(&[("plan.md", Some(broken.into_bytes()))]).await;

    assert_eq!(t.row("alice", "plan.md").await, hers);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.start.as_deref(), Some(text(&plan()).as_str()));
    assert_eq!(record.merged_at, None);
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)],
        "it stands until something merges it"
    );
}

#[tokio::test]
async fn a_peer_rename_takes_a_plain_draft_along_and_merges_there() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.push(&[("plan.md", None), ("renamed.md", Some(plan_later()))])
        .await;

    assert_eq!(
        t.rows("alice").await,
        vec![("renamed.md".to_string(), false)]
    );
    let row = t.row("alice", "renamed.md").await;
    assert!(row.contains(BOB_LINE) && row.contains(HER_WORD), "{row}");
    let record = t.record("alice", "renamed.md").unwrap();
    assert_eq!(record.from.as_deref(), Some("renamed.md"));
    assert_eq!(record.start.as_deref(), Some(text(&plan_later()).as_str()));
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn a_peer_rename_of_a_file_she_moved_is_address_taken() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "mine.md").await;
    // Bob renames it and changes a line on the way, so the index reads the new
    // file whole and it keeps its address.
    t.push(&[("plan.md", None), ("bobs.md", Some(plan_later()))])
        .await;

    assert_eq!(
        t.rows("alice").await,
        vec![
            ("mine.md".to_string(), false),
            ("plan.md".to_string(), true)
        ],
        "neither half of her move is dropped"
    );
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            other: Some("bobs.md".to_string()),
            ..conflict("alice", "mine.md", AddressTaken)
        }]
    );
}

#[tokio::test]
async fn a_peer_file_at_the_path_she_moved_to_is_a_conflict() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    let hers = t.row("alice", "renamed.md").await;
    let his = engram("Renamed", "renamed", "bob's own page at that path");
    t.push(&[("renamed.md", Some(his.clone()))]).await;

    assert_eq!(t.row("alice", "renamed.md").await, hers);
    assert_eq!(
        t.rows("alice").await,
        vec![
            ("plan.md".to_string(), true),
            ("renamed.md".to_string(), false)
        ]
    );
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", AddAdd)
        }]
    );
    assert_eq!(t.forge_file("renamed.md"), Some(text(&his)));
}

#[tokio::test]
async fn a_peer_delete_of_a_file_she_edited_stands_as_a_conflict() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let hers = t.row("alice", "plan.md").await;
    t.push(&[("plan.md", None)]).await;

    assert_eq!(t.row("alice", "plan.md").await, hers);
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditDelete)]
    );
}

#[tokio::test]
async fn her_delete_against_a_peer_edit_stands_as_a_conflict() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.delete("alice", "plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    assert_eq!(t.rows("alice").await, vec![("plan.md".to_string(), true)]);
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", DeleteEdit)]
    );
}

#[tokio::test]
async fn her_delete_against_a_peer_delete_ends() {
    let mut t = team(&[("plan.md", plan()), ("other.md", gone())]).await;
    t.delete("alice", "plan").await;
    t.push(&[("plan.md", None)]).await;

    assert_eq!(t.rows("alice").await, vec![]);
    assert_eq!(t.record("alice", "plan.md"), None);
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn the_pass_counts_what_it_merged() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.append("bob", "plan", "bob's own last word").await;
    t.snapshot_has("plan.md", Some(&plan_later()));
    let report = t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(report.merged, 2, "{report:?}");
    assert_eq!(report.diverged, 0, "{report:?}");
}

#[tokio::test]
async fn an_armed_failure_stops_the_pass_before_it_changes_anything() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let hers = t.row("alice", "plan.md").await;
    t.snapshot_has("plan.md", Some(&plan_later()));
    t.eng.fail_next_convergence();
    t.eng.converge_overlays("team").await.unwrap_err();
    assert_eq!(t.row("alice", "plan.md").await, hers);
    // Armed once: the next pass runs.
    t.eng.converge_overlays("team").await.unwrap();
    assert!(t.row("alice", "plan.md").await.contains(BOB_LINE));
}

/// M1 through a pull: the team deleted the old pair's source, and the pass does
/// not take the tombstone alone, which would leave the draft reading as a new
/// file whose share brings the page back.
#[tokio::test]
async fn an_old_move_whose_source_the_team_deleted_stands_through_a_pull() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.old_tombstone("alice", "plan.md", &plan()).await;
    t.old_draft(
        "alice",
        "renamed.md",
        &text(&plan()).replace("the last line", "alice's last line"),
    )
    .await;
    t.push(&[("plan.md", None)]).await;

    assert_eq!(
        t.rows("alice").await,
        vec![
            ("plan.md".to_string(), true),
            ("renamed.md".to_string(), false)
        ]
    );
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", EditDelete)
        }]
    );
}

/// An old pair is paired by a guess, so its draft landing says nothing about the
/// path it may have left: while the team holds that path, the pair stands as the
/// one conflict the list names at the draft, with the same id before and after
/// the pass.
#[tokio::test]
async fn an_old_move_whose_draft_landed_keeps_her_deletion_while_the_team_holds_it() {
    let t = team(&[("plan.md", plan())]).await;
    let hers = text(&plan()).replace("the last line", "alice's last line");
    t.old_tombstone("alice", "plan.md", &plan()).await;
    t.old_draft("alice", "renamed.md", &hers).await;
    t.snapshot_has("renamed.md", Some(hers.as_bytes()));
    let before = t.conflicts("alice").await;
    assert_eq!(
        before,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", UnknownStart)
        }]
    );
    t.eng.converge_overlays("team").await.unwrap();

    assert_eq!(
        t.rows("alice").await,
        vec![
            ("plan.md".to_string(), true),
            ("renamed.md".to_string(), false)
        ]
    );
    assert_eq!(t.conflicts("alice").await, before);

    // Once the team lets go of it too, the next pull ends it.
    let mut t = team(&[("plan.md", plan())]).await;
    t.old_tombstone("alice", "plan.md", &plan()).await;
    t.old_draft("alice", "renamed.md", &hers).await;
    t.push(&[
        ("plan.md", None),
        ("renamed.md", Some(hers.clone().into_bytes())),
    ])
    .await;
    assert_eq!(t.rows("alice").await, vec![]);
}

/// The pass decides without the draft lock and writes under it. A writer that
/// changed the row in between wins: the pass leaves the row for its next run
/// rather than write a merge of the text it read before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pass_racing_an_edit_never_loses_the_edit() {
    let t = Arc::new(team(&[("plan.md", plan())]).await);
    t.append("alice", "plan", HER_WORD).await;
    t.snapshot_has("plan.md", Some(&plan_later()));

    // An edit holds her draft's lock between its read and its write.
    t.eng.hold_next_draft_edit(1500);
    let edit = {
        let t = t.clone();
        tokio::spawn(async move { t.append("alice", "plan", "a word while the pass ran").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    t.eng.converge_overlays("team").await.unwrap();
    edit.await.unwrap();

    let row = t.row("alice", "plan.md").await;
    assert!(row.contains("a word while the pass ran"), "{row}");
    assert!(row.contains(HER_WORD), "{row}");
    // The next pass merges what stands now.
    t.eng.converge_overlays("team").await.unwrap();
    let row = t.row("alice", "plan.md").await;
    for line in [BOB_LINE, HER_WORD, "a word while the pass ran"] {
        assert!(row.contains(line), "{line}: {row}");
    }
}

/// A page that takes no part in a share holds no address, so a team rename onto
/// a dotfile never takes her draft along (C12).
#[tokio::test]
async fn a_rename_onto_a_dotfile_does_not_take_her_draft_along() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.push(&[("plan.md", None), (".plan.md", Some(plan()))])
        .await;

    assert_eq!(t.rows("alice").await, vec![("plan.md".to_string(), false)]);
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditDelete)]
    );
}

/// C1 in the pass: a record that cannot be read leaves that person's rows as
/// they are, and the pass goes on for everybody else.
#[tokio::test]
async fn a_broken_record_leaves_her_rows_and_the_pass_goes_on_for_others() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.append("bob", "plan", "bob's own last word").await;
    let file = draft_start::record_file(&t.state(), "team", "alice", "plan.md").unwrap();
    std::fs::write(&file, b"{ not a record").unwrap();
    let hers = t.row("alice", "plan.md").await;
    t.snapshot_has("plan.md", Some(&plan_later()));

    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(t.row("alice", "plan.md").await, hers);
    assert_eq!(std::fs::read(&file).unwrap(), b"{ not a record");
    assert!(t.row("bob", "plan.md").await.contains(BOB_LINE));
}

/// C1 in the status: a person whose conflicts cannot be read makes the domain's
/// count unknown, and an unknown count is left out rather than shown too low.
#[tokio::test]
async fn a_status_with_an_unreadable_record_leaves_the_count_out() {
    let mut t = team(&[("plan.md", plan()), ("gone.md", gone())]).await;
    t.write("alice", "Plan", "alice rewrote the plan").await;
    t.write("bob", "Gone", "bob rewrote it").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(converged(&t.eng).await["diverged"], 1);

    let file = draft_start::record_file(&t.state(), "team", "bob", "gone.md").unwrap();
    std::fs::write(&file, b"{ not a record").unwrap();
    assert!(converged(&t.eng).await.is_null());
}

/// The convergence record's old row conflicts are a list a gate reads, so a
/// pass that cannot read the file leaves it as it is rather than save an empty
/// one over it.
#[tokio::test]
async fn a_pass_leaves_a_convergence_record_it_cannot_read() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let file = t
        .state()
        .join("overlays")
        .join("team")
        .join(".convergence.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"{ not a record").unwrap();
    t.snapshot_has("plan.md", Some(&plan_later()));

    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"{ not a record");
    assert!(t.row("alice", "plan.md").await.contains(BOB_LINE));
}

/// What the domain's owner reads under `converged`.
async fn converged(eng: &Engine) -> serde_json::Value {
    eng.origin_status(Some("team"), false, false, &Scope::Unrestricted)
        .await
        .unwrap()["domains"][0]["converged"]
        .clone()
}

/// A team rename onto a path she drafts at herself never takes her other draft
/// there: no follow writes over a second draft of hers.
#[tokio::test]
async fn a_team_rename_onto_a_path_she_drafts_leaves_both_drafts() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.eng
        .write_engram_as(
            &write_params("Plan v2", "her other page"),
            None,
            &scope("alice"),
        )
        .await
        .unwrap();
    let other = t.row("alice", "plan-v2.md").await;
    t.push(&[("plan.md", None), ("plan-v2.md", Some(plan_later()))])
        .await;

    assert_eq!(t.row("alice", "plan-v2.md").await, other);
    assert!(t.row("alice", "plan.md").await.contains(HER_WORD));
}

/// A merge the view refuses to write leaves the row and its start: here the
/// team gave the page an address another team file already answers to, so her
/// merged draft could not take it.
#[tokio::test]
async fn a_merge_onto_an_address_held_elsewhere_leaves_the_row_and_its_start() {
    let mut t = team(&[("plan.md", plan()), ("gone.md", gone())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let hers = t.row("alice", "plan.md").await;
    let taken = text(&plan_later()).replace("permalink: plan", "permalink: gone");
    t.push(&[("plan.md", Some(taken.into_bytes()))]).await;

    assert_eq!(t.row("alice", "plan.md").await, hers);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.start.as_deref(), Some(text(&plan()).as_str()));
    assert_eq!(record.merged_at, None);
}

/// The pass decides a landed row on the rows it read first. An edit that comes
/// in after a merge was written and before the landed row is dropped is hers:
/// the drop re-reads the row under its lock and leaves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_landing_after_a_merge_leaves_an_edit_that_came_in_meanwhile() {
    let t = Arc::new(team(&[("plan.md", plan())]).await);
    // Her draft holds what she started from: unedited, so the merge is the
    // team's text and the row has then landed.
    draft(&t.eng, "alice", "plan.md", &text(&plan())).await;
    mirror(&t.state(), "alice", "plan.md", &text(&plan()));
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
            merging: None,
        },
    )
    .unwrap();
    t.snapshot_has("plan.md", Some(&plan_later()));

    t.eng.hold_convergence_drop(0, 1500);
    let pass = {
        let t = t.clone();
        tokio::spawn(async move { t.eng.converge_overlays("team").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    t.append("alice", "plan", "her line while the pass ran")
        .await;
    pass.await.unwrap().unwrap();

    assert_eq!(t.rows("alice").await, vec![("plan.md".to_string(), false)]);
    let row = t.row("alice", "plan.md").await;
    assert!(row.contains("her line while the pass ran"), "{row}");
    assert!(row.contains(BOB_LINE), "{row}");
}

/// While the pass waits at one draft's lock, she edits another draft the pass
/// read as landed. The drop re-reads that row and leaves her edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_landed_draft_edited_while_the_pass_waits_at_another_lock_keeps_the_edit() {
    let zz = engram("Zz", "zz", "the team's last page");
    let t = Arc::new(team(&[("plan.md", plan()), ("zz.md", zz.clone())]).await);
    t.append("alice", "plan", HER_WORD).await;
    t.append("alice", "zz", "her word on zz").await;
    // The team took her zz as it stands, and changed the plan under her.
    let landed = t.row("alice", "zz.md").await;
    t.snapshot_has("zz.md", Some(landed.as_bytes()));
    t.snapshot_has("plan.md", Some(&plan_later()));

    // An edit holds the plan's lock, so the pass waits there.
    t.eng.hold_next_draft_edit(1500);
    let held = {
        let t = t.clone();
        tokio::spawn(async move { t.append("alice", "plan", "a word on the plan").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let pass = {
        let t = t.clone();
        tokio::spawn(async move { t.eng.converge_overlays("team").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    t.append("alice", "zz", "her second word on zz").await;
    held.await.unwrap();
    pass.await.unwrap().unwrap();

    assert!(
        t.rows("alice")
            .await
            .contains(&("zz.md".to_string(), false))
    );
    let row = t.row("alice", "zz.md").await;
    assert!(row.contains("her second word on zz"), "{row}");
}

/// The start of a written merge could not be saved. The row holds the merge
/// over the old start, under the merge mark: the gate still lists it, the next
/// pass merges again to the same text, writes nothing, moves the start and
/// tells her.
#[tokio::test]
async fn a_merge_whose_start_could_not_move_merges_again_and_tells_her() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.snapshot_has("plan.md", Some(&plan_later()));

    t.eng.fail_next_merge_start();
    let first = t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(first.merged, 1, "{first:?}");
    let merged = t.row("alice", "plan.md").await;
    assert!(
        merged.contains(BOB_LINE) && merged.contains(HER_WORD),
        "{merged}"
    );
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.start.as_deref(), Some(text(&plan()).as_str()));
    assert_eq!(record.merging, Some(true));
    assert_eq!(record.merged_at, None);
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );

    let second = t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(second.merged, 0, "the row already held it: {second:?}");
    assert_eq!(t.row("alice", "plan.md").await, merged);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.start.as_deref(), Some(text(&plan_later()).as_str()));
    assert_eq!(record.merging, None);
    assert!(record.merged_at.is_some(), "she is told: {record:?}");
    assert_eq!(t.conflicts("alice").await, vec![]);
}

// --- Share and preview refuse --------------------------------------------------
//
// A share or a preview asks the conflict list after its own pull and pass. A
// conflict among what would travel refuses the share, and the forge keeps what
// the team has. Each test asserts the forge, not only the answer.

const ALICE_LINE: &str = "alice's first line";

/// Alice changes the plan's first line, the one bob's change takes too.
async fn same_line(t: &Team) {
    t.replace("alice", "plan", "the first line of the plan", ALICE_LINE)
        .await;
}

fn crlf(bytes: &[u8]) -> Vec<u8> {
    text(bytes).replace('\n', "\r\n").into_bytes()
}

/// Every line of `bytes` ends in CRLF, and there is at least one.
fn wholly_crlf(bytes: &str) -> bool {
    bytes.contains('\n') && bytes.matches('\n').count() == bytes.matches("\r\n").count()
}

fn kinds(answer: &serde_json::Value) -> Vec<(String, String)> {
    answer["conflicts"]
        .as_array()
        .unwrap_or_else(|| panic!("no conflicts in {answer}"))
        .iter()
        .map(|c| {
            (
                c["path"].as_str().unwrap().to_string(),
                c["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_direct_share_of_a_stale_edit_refuses_and_the_peers_line_stays() {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    same_line(&t).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(answer["count"], 1, "{answer}");
    assert_eq!(answer["conflicts"][0]["kind"], "EditEdit", "{answer}");
    let forge = t.forge_file("plan.md").unwrap();
    assert!(
        forge.contains(BOB_LINE) && !forge.contains(ALICE_LINE),
        "{forge}"
    );
}

#[tokio::test]
async fn a_direct_share_that_merges_lands_both_lines() {
    let t = team_direct(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.push_with_no_pass(&[("plan.md", Some(plan_later()))])
        .await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("plan.md").unwrap();
    assert!(
        forge.contains(BOB_LINE) && forge.contains(HER_WORD),
        "{forge}"
    );
}

#[tokio::test]
async fn a_proposal_of_a_stale_edit_holds_the_peers_line_on_the_branch_after_a_clean_merge() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.push_with_no_pass(&[("plan.md", Some(plan_later()))])
        .await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "proposed", "{answer}");
    let branch = answer["branch"].as_str().unwrap();
    let held = text(&t.branch_file(branch, "plan.md").unwrap());
    assert!(held.contains(BOB_LINE) && held.contains(HER_WORD), "{held}");
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

#[tokio::test]
async fn a_proposal_of_a_same_line_edit_refuses() {
    let mut t = team(&[("plan.md", plan())]).await;
    same_line(&t).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(
        kinds(&answer),
        vec![("plan.md".to_string(), "EditEdit".to_string())]
    );
    let state = OriginState::load(&t.snapshot_dir()).unwrap().unwrap();
    assert!(state.proposals.is_empty(), "{:?}", state.proposals);
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

#[tokio::test]
async fn a_stale_delete_refuses_and_the_file_stays() {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    t.delete("alice", "plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(
        kinds(&answer),
        vec![("plan.md".to_string(), "DeleteEdit".to_string())]
    );
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

#[tokio::test]
async fn a_share_with_no_update_first_brings_the_base_up_and_refuses() {
    let t = team_direct(&[("plan.md", plan())]).await;
    same_line(&t).await;
    t.commit(&[("plan.md", Some(plan_later()))]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
    let state = OriginState::load(&t.snapshot_dir()).unwrap().unwrap();
    assert_eq!(
        state.base_commit,
        t.mock.branch_commit("main").unwrap(),
        "the share's pull brought the base up"
    );
}

#[tokio::test]
async fn a_share_refuses_a_mergeable_stale_draft_when_the_pass_failed() {
    let t = team_direct(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.commit(&[("plan.md", Some(plan_later()))]);
    t.eng.fail_next_convergence();

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(
        kinds(&answer),
        vec![("plan.md".to_string(), "EditEdit".to_string())]
    );
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

/// A merge written into her row whose start could not move stands under its
/// mark, and the share refuses until a pass has settled it.
#[tokio::test]
async fn a_share_refuses_while_a_merge_has_not_settled_its_start() {
    let t = team_direct(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.commit(&[("plan.md", Some(plan_later()))]);
    t.eng.fail_next_merge_start();

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(t.record("alice", "plan.md").unwrap().merging, Some(true));
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));

    // The next share's pass settles it, and the merge goes through.
    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("plan.md").unwrap();
    assert!(
        forge.contains(BOB_LINE) && forge.contains(HER_WORD),
        "{forge}"
    );
}

/// Two drafts: the plan in conflict, and a new page that is not.
async fn a_conflict_and_another_file() -> Team {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    same_line(&t).await;
    t.write("alice", "Other", "a page only alice has").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    t
}

#[tokio::test]
async fn a_preview_names_the_conflicts_beside_the_plan_and_leaves_the_forge_and_her_draft() {
    let t = a_conflict_and_another_file().await;
    let hers = t.row("alice", "plan.md").await;
    let before = t.tree();

    let plan = t.preview("alice", None).await;
    assert_eq!(plan["action"], "commit", "{plan}");
    let changed: Vec<&str> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert!(
        changed.contains(&"plan.md") && changed.contains(&"other.md"),
        "{plan}"
    );
    assert_eq!(plan["draft_conflicts"][0]["path"], "plan.md", "{plan}");
    assert_eq!(plan["draft_conflicts"][0]["kind"], "EditEdit", "{plan}");
    assert_eq!(t.tree(), before);
    assert_eq!(t.row("alice", "plan.md").await, hers);
}

#[tokio::test]
async fn a_preview_naming_a_conflicted_file_answers_conflicts_pending() {
    let t = a_conflict_and_another_file().await;
    let before = t.tree();

    let plan = t.preview("alice", Some(&["plan.md", "other.md"])).await;
    assert_eq!(plan["action"], "conflicts_pending", "{plan}");
    assert_eq!(plan["count"], 1, "{plan}");
    assert_eq!(
        kinds(&plan),
        vec![("plan.md".to_string(), "EditEdit".to_string())]
    );
    assert_eq!(plan["changes"].as_array().unwrap().len(), 2, "{plan}");
    assert!(
        plan.get("branch").is_none(),
        "no word of the commit: {plan}"
    );
    assert_eq!(t.tree(), before);

    // Another file alone plans as usual.
    let plan = t.preview("alice", Some(&["other.md"])).await;
    assert_eq!(plan["action"], "commit", "{plan}");
}

#[tokio::test]
async fn sharing_one_other_file_goes_through() {
    let t = a_conflict_and_another_file().await;
    let answer = t.share("alice", Some(&["other.md"])).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert!(t.forge_file("other.md").is_some());
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

#[tokio::test]
async fn a_conflicted_file_named_in_another_spelling_still_refuses() {
    let t = a_conflict_and_another_file().await;
    let answer = t.share("alice", Some(&["./plan.md"])).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

#[tokio::test]
async fn sharing_everything_refuses() {
    let t = a_conflict_and_another_file().await;
    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(t.forge_file("other.md"), None);
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

#[tokio::test]
async fn a_colleagues_conflict_does_not_block_her_share() {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    t.replace(
        "bob",
        "plan",
        "the first line of the plan",
        "bob's own first line",
    )
    .await;
    t.write("alice", "Other", "a page only alice has").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(t.conflicts("bob").await.len(), 1);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert!(t.forge_file("other.md").is_some());
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

/// Alice moved the plan and changed its first line; bob changed that line too.
async fn a_move_in_conflict() -> Team {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    same_line(&t).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", EditEdit)
        }]
    );
    t
}

#[tokio::test]
async fn sharing_the_conflicted_files_move_partner_refuses() {
    let t = a_move_in_conflict().await;
    let answer = t.share("alice", Some(&["plan.md"])).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

// One half of a move.

#[tokio::test]
async fn sharing_only_the_deletion_of_a_move_frees_the_draft() {
    let t = team_direct(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;

    let answer = t.share("alice", Some(&["plan.md"])).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert_eq!(t.forge_file("plan.md"), None);
    assert_eq!(t.forge_file("renamed.md"), None);
    assert_eq!(t.conflicts("alice").await, vec![], "no EditDelete");
    assert_eq!(
        t.rows("alice").await,
        vec![("renamed.md".to_string(), false)]
    );
    let record = t.record("alice", "renamed.md").unwrap();
    assert_eq!((record.from, record.start), (None, None));

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("renamed.md").unwrap();
    assert!(forge.contains("the first line of the plan"), "{forge}");
    assert_eq!(t.rows("alice").await, vec![]);
}

/// Her draft lands and the team still holds where she started, so her deletion
/// stands, in step and with no conflict, for the next share to carry.
#[tokio::test]
async fn sharing_only_the_draft_of_a_move_leaves_no_conflict() {
    let t = team_direct(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;

    let answer = t.share("alice", Some(&["renamed.md"])).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert!(t.forge_file("renamed.md").is_some());
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan())));
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(
        t.rows("alice").await,
        vec![
            ("plan.md".to_string(), true),
            ("renamed.md".to_string(), false)
        ],
        "both halves stand, in step"
    );

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert_eq!(t.forge_file("plan.md"), None);
    assert_eq!(t.rows("alice").await, vec![]);
    assert_eq!(t.conflicts("alice").await, vec![]);
}

// A second move, and a discard of one half, while the move is in conflict.

#[tokio::test]
async fn a_second_move_while_in_conflict_is_refused_at_the_share() {
    let t = a_move_in_conflict().await;
    t.moved("alice", "plan", "again.md").await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(answer["conflicts"][0]["path"], "again.md", "{answer}");
    assert_eq!(answer["conflicts"][0]["moved_from"], "plan.md", "{answer}");
    assert_eq!(t.forge_file("again.md"), None);
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

#[tokio::test]
async fn discarding_only_the_deletion_of_a_move_in_conflict_is_refused_at_the_share() {
    let t = a_move_in_conflict().await;
    t.discard("alice", "plan.md").await;

    // The team's page shows through at the old path again and answers to the
    // address her draft carries, so the draft stands as that clash.
    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(
        kinds(&answer),
        vec![("renamed.md".to_string(), "AddressTaken".to_string())]
    );
    assert_eq!(answer["conflicts"][0]["other"], "plan.md", "{answer}");
    assert_eq!(t.forge_file("renamed.md"), None);
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

#[tokio::test]
async fn discarding_only_the_draft_of_a_move_in_conflict_leaves_a_refused_deletion() {
    let t = a_move_in_conflict().await;
    t.discard("alice", "renamed.md").await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(
        kinds(&answer),
        vec![("plan.md".to_string(), "DeleteEdit".to_string())]
    );
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

// Line endings: a review share keeps the team's.

#[tokio::test]
async fn a_merged_move_of_a_crlf_file_lands_crlf() {
    let mut t = team_direct(&[("plan.md", crlf(&plan()))]).await;
    t.moved("alice", "plan", "renamed.md").await;
    t.append("alice", "plan", HER_WORD).await;
    t.push(&[("plan.md", Some(crlf(&plan_later())))]).await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("renamed.md").unwrap();
    assert!(wholly_crlf(&forge), "{forge:?}");
    assert!(
        forge.contains(BOB_LINE) && forge.contains(HER_WORD),
        "{forge}"
    );
    assert_eq!(t.forge_file("plan.md"), None);
    assert_eq!(
        t.rows("alice").await,
        vec![],
        "the move ended with the commit"
    );
}

#[tokio::test]
async fn an_edit_of_a_crlf_file_lands_crlf() {
    let t = team_direct(&[("plan.md", crlf(&plan()))]).await;
    t.append("alice", "plan", HER_WORD).await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("plan.md").unwrap();
    assert!(wholly_crlf(&forge), "{forge:?}");
    assert!(forge.contains(HER_WORD), "{forge}");
    assert_eq!(
        t.rows("alice").await,
        vec![],
        "the draft ended with the commit"
    );
}

#[tokio::test]
async fn an_unedited_move_of_a_crlf_file_lands_the_teams_bytes() {
    let t = team_direct(&[("plan.md", crlf(&plan()))]).await;
    t.moved("alice", "plan", "renamed.md").await;

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert_eq!(t.forge_file("renamed.md"), Some(text(&crlf(&plan()))));
    assert_eq!(t.forge_file("plan.md"), None);
}

#[tokio::test]
async fn withdrawing_an_unchanged_crlf_draft_restores_it() {
    let t = team(&[("plan.md", crlf(&plan()))]).await;
    t.append("alice", "plan", HER_WORD).await;
    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "proposed", "{answer}");
    let branch = answer["branch"].as_str().unwrap();
    let proposed = text(&t.branch_file(branch, "plan.md").unwrap());
    assert!(wholly_crlf(&proposed), "{proposed:?}");

    let report = t
        .eng
        .origin_withdraw("team", None, true, ShareActor::Account("alice".to_string()))
        .await
        .unwrap();
    assert_eq!(
        report["restored"],
        serde_json::json!(["plan.md"]),
        "{report}"
    );
    assert_eq!(
        report["skipped_diverged"],
        serde_json::json!([]),
        "{report}"
    );
    assert_eq!(t.rows("alice").await, vec![]);
}

/// A direct commit that deletes another file leaves her unshared move where it
/// started, so a later team change still merges into it.
#[tokio::test]
async fn sharing_another_deletion_keeps_a_move_where_it_started() {
    let mut t = team_direct(&[("plan.md", plan()), ("gone.md", gone())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    t.delete("alice", "gone").await;
    let answer = t.share("alice", Some(&["gone.md"])).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert_eq!(t.forge_file("gone.md"), None);
    let record = t.record("alice", "renamed.md").unwrap();
    assert_eq!(record.from.as_deref(), Some("plan.md"));

    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert!(t.row("alice", "renamed.md").await.contains(BOB_LINE));
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// A record the share cannot read refuses the share. Her deletion's record is
/// read by the gate alone, since nothing is staged from a deletion.
#[tokio::test]
async fn a_share_refuses_when_her_records_cannot_be_read() {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    t.delete("alice", "plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    let file = draft_start::record_file(&t.state(), "team", "alice", "plan.md").unwrap();
    std::fs::write(&file, b"{ not a record").unwrap();

    t.eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap_err();
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan_later())));
}

/// The share dialog's discard sends back the digest the preview showed. For a
/// draft of a CRLF team file that is the digest of the staged CRLF bytes, and
/// the discard takes it.
#[tokio::test]
async fn a_discard_from_the_share_dialog_of_a_crlf_draft_goes_through() {
    let t = team(&[("plan.md", crlf(&plan()))]).await;
    t.append("alice", "plan", HER_WORD).await;
    let plan = t.preview("alice", None).await;
    let sha = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == "plan.md")
        .unwrap_or_else(|| panic!("{plan}"))["sha"]
        .as_str()
        .unwrap()
        .to_string();

    let report = t
        .eng
        .discard_local_changes(
            "team",
            &[DiscardTarget {
                path: "plan.md".to_string(),
                sha256: Some(sha),
            }],
            &ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(report["refused"], serde_json::json!([]), "{report}");
    assert_eq!(t.rows("alice").await, vec![]);
}

/// A direct commit whose deletion she shared, where her draft could not be told
/// that it starts nowhere now: the receipt says so in a sentence.
#[tokio::test]
async fn a_direct_commit_says_when_her_moved_draft_could_not_be_freed() {
    let t = team_direct(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    // Another record in her folder cannot be read, so the list of her records
    // fails while each draft's own record still reads.
    let folder = draft_start::record_file(&t.state(), "team", "alice", "plan.md")
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::write(
        folder.join(format!("{}.json", "0".repeat(64))),
        b"{ not a record",
    )
    .unwrap();

    let answer = t.share("alice", Some(&["plan.md"])).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let said = answer["free_error"]
        .as_str()
        .unwrap_or_else(|| panic!("{answer}"));
    assert!(said.contains("plan.md"), "{said}");
    assert!(!said.contains('\n'), "{said}");
    assert_eq!(t.forge_file("plan.md"), None);
}

/// A preview runs the pass for the caller's own drafts only: a colleague's stale
/// draft stays as it is until a pull or a share of theirs.
#[tokio::test]
async fn a_preview_merges_only_her_own_drafts() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.append("bob", "plan", "bob's own last word").await;
    let his = t.row("bob", "plan.md").await;
    t.push_with_no_pass(&[("plan.md", Some(plan_later()))])
        .await;

    t.preview("alice", None).await;
    assert!(t.row("alice", "plan.md").await.contains(BOB_LINE));
    assert_eq!(t.row("bob", "plan.md").await, his);
}

/// A preview made on the instance's credential, for a caller who connected no
/// GitHub identity of their own, writes no draft at all.
#[tokio::test]
async fn a_preview_on_read_scope_writes_no_draft() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let hers = t.row("alice", "plan.md").await;
    t.push_with_no_pass(&[("plan.md", Some(plan_later()))])
        .await;
    t.eng.refuse_next_personal_credential();

    let plan = t
        .eng
        .origin_share_preview(
            "team",
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
            PreviewCredential::ReadScopeFallback,
        )
        .await
        .unwrap();
    assert_eq!(
        plan["draft_conflicts"][0]["kind"], "EditEdit",
        "the stale draft is still named: {plan}"
    );
    assert_eq!(t.row("alice", "plan.md").await, hers);
}

/// Each conflict in a refusal says it is a draft's, as the status detail does,
/// so a reader can tell it from a conflict of a domain without review.
#[tokio::test]
async fn a_refused_share_marks_each_conflict_as_a_draft() {
    let t = a_conflict_and_another_file().await;
    let answer = t.share("alice", None).await;
    assert_eq!(answer["conflicts"][0]["draft"], true, "{answer}");
    let plan = t.preview("alice", None).await;
    assert_eq!(plan["draft_conflicts"][0]["draft"], true, "{plan}");
    let plan = t.preview("alice", Some(&["plan.md"])).await;
    assert_eq!(plan["conflicts"][0]["draft"], true, "{plan}");
}

fn old_page() -> Vec<u8> {
    engram("Old", "old", "the old page")
}

/// A preview whose own pull brings in a team rename converges every person's
/// drafts, as any pull does: the rename is applied once, and a pass that missed
/// it could never follow it later.
#[tokio::test]
async fn a_preview_whose_pull_applied_a_rename_takes_a_colleagues_draft_along() {
    let t = team(&[("old.md", old_page())]).await;
    t.append("bob", "old", "bob's word").await;
    t.commit(&[("old.md", None), ("new.md", Some(old_page()))]);

    t.preview("alice", None).await;
    assert_eq!(t.rows("bob").await, vec![("new.md".to_string(), false)]);
    assert_eq!(t.conflicts("bob").await, vec![]);
}

/// The same on read scope: the pass belongs to the pull, so her own draft
/// follows the rename too.
#[tokio::test]
async fn a_read_scope_preview_whose_pull_applied_a_rename_takes_her_draft_along() {
    let t = team(&[("old.md", old_page())]).await;
    t.append("alice", "old", "her word").await;
    t.commit(&[("old.md", None), ("new.md", Some(old_page()))]);
    t.eng.refuse_next_personal_credential();

    t.eng
        .origin_share_preview(
            "team",
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
            PreviewCredential::ReadScopeFallback,
        )
        .await
        .unwrap();
    assert_eq!(t.rows("alice").await, vec![("new.md".to_string(), false)]);
    assert_eq!(t.conflicts("alice").await, vec![]);
}

// --- A proposal knows what it holds ------------------------------------------

/// A word of the reviewer's in place of hers, made on the proposal's branch.
const REVIEWED_WORD: &str = "the reviewer's own last word";

/// A line she adds in the middle, which a reviewer may take out again.
const HER_EXTRA: &str = "alice's extra line in the middle";

impl Team {
    /// This machine pulls what the forge holds.
    async fn pull(&self) {
        self.eng
            .origin_update(Some("team"), &Scope::Unrestricted)
            .await
            .unwrap();
    }

    /// The team merges the proposal `answer` opened: its branch with `changes`
    /// made on it first (a reviewer's edits, or the merge the forge makes with a
    /// `main` that moved), as `main`, and the forge says it merged.
    fn merge(&self, answer: &serde_json::Value, changes: &[(&str, Option<Vec<u8>>)]) {
        let number = answer["number"].as_u64().unwrap();
        let commit = self.merged_commit(answer, changes);
        self.mock.set_branch("main", &commit);
        self.mock.set_proposal_state(number, ProposalState::Merged);
    }

    /// The commit the forge makes when it merges the proposal `answer` opened,
    /// with `changes` made on its branch first. `main` is not moved.
    fn merged_commit(
        &self,
        answer: &serde_json::Value,
        changes: &[(&str, Option<Vec<u8>>)],
    ) -> String {
        let branch = answer["branch"].as_str().unwrap();
        let head = self.mock.branch_commit(branch).unwrap();
        let mut files: BTreeMap<String, Vec<u8>> = self
            .mock
            .commit_paths(&head)
            .into_iter()
            .map(|path| {
                let bytes = self.mock.commit_file(&head, &path).unwrap();
                (path, bytes)
            })
            .collect();
        for (path, bytes) in changes {
            match bytes {
                Some(bytes) => files.insert((*path).to_string(), bytes.clone()),
                None => files.remove(*path),
            };
        }
        self.mock.add_commit(files)
    }

    /// What `actor`'s record at `path` says the open proposal holds.
    fn shared(&self, actor: &str, path: &str) -> Option<SharedText> {
        self.record(actor, path).and_then(|record| record.shared)
    }

    /// Change the origin state as `change` says, the way a step this test does
    /// not run would leave it.
    fn state_edit(&self, change: impl FnOnce(&mut OriginState)) {
        let dir = self.snapshot_dir();
        let mut state = OriginState::load(&dir).unwrap().unwrap();
        change(&mut state);
        state.save(&dir).unwrap();
    }
}

/// `actor`'s share of everything opens a proposal: its number and branch.
async fn proposed(t: &Team, actor: &str) -> (serde_json::Value, u64, String) {
    let answer = t.share(actor, None).await;
    assert_eq!(answer["outcome"], "proposed", "{answer}");
    let number = answer["number"].as_u64().unwrap();
    let branch = answer["branch"].as_str().unwrap().to_string();
    (answer, number, branch)
}

fn holds(proposal: u64, text: Option<String>) -> Option<SharedText> {
    Some(SharedText { proposal, text })
}

/// N1 by the proposal road: her edited move merges, both rows end, and a later
/// peer edit of the moved page is what she reads.
#[tokio::test]
async fn a_merged_proposal_of_an_edited_move_ends_both_rows() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.moved("alice", "plan", "renamed.md").await;
    let (answer, number, branch) = proposed(&t, "alice").await;
    let hers = t.row("alice", "renamed.md").await;
    assert_eq!(t.shared("alice", "renamed.md"), holds(number, Some(hers)));
    assert_eq!(t.shared("alice", "plan.md"), holds(number, None));
    let held = text(&t.branch_file(&branch, "renamed.md").unwrap());

    t.merge(&answer, &[]);
    t.pull().await;
    assert_eq!(t.rows("alice").await, vec![]);
    assert_eq!(t.conflicts("alice").await, vec![]);

    let his = held.replace("the first line of the plan", BOB_LINE);
    t.push(&[("renamed.md", Some(his.into_bytes()))]).await;
    let read = t.reads("alice", "plan").await;
    assert!(read.contains(BOB_LINE) && read.contains(HER_WORD), "{read}");
    assert_eq!(t.share("alice", None).await["outcome"], "nothing_to_share");
}

/// A reviewer rewrote her line on the branch before it merged: her draft ends
/// and she reads the reviewer's words, with no conflict against her own share.
#[tokio::test]
async fn a_merged_proposal_a_reviewer_changed_ends_her_draft_with_no_conflict() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, branch) = proposed(&t, "alice").await;
    assert_eq!(
        t.shared("alice", "plan.md"),
        holds(number, Some(t.row("alice", "plan.md").await))
    );
    let reviewed =
        text(&t.branch_file(&branch, "plan.md").unwrap()).replace(HER_WORD, REVIEWED_WORD);

    t.merge(&answer, &[("plan.md", Some(reviewed.into_bytes()))]);
    t.pull().await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(t.rows("alice").await, vec![]);
    let read = t.reads("alice", "plan").await;
    assert!(
        read.contains(REVIEWED_WORD) && !read.contains(HER_WORD),
        "{read}"
    );
}

/// A line she added that a reviewer took out before merging stays out: the
/// merge reads her row against what the branch held, not against her old start.
#[tokio::test]
async fn a_line_a_reviewer_removed_stays_removed_after_the_merge() {
    let t = team(&[("plan.md", plan())]).await;
    let middle = "the middle of the plan";
    t.replace("alice", "plan", middle, &format!("{middle}\n\n{HER_EXTRA}"))
        .await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, branch) = proposed(&t, "alice").await;
    assert_eq!(
        t.shared("alice", "plan.md"),
        holds(number, Some(t.row("alice", "plan.md").await))
    );
    let reviewed =
        text(&t.branch_file(&branch, "plan.md").unwrap()).replace(&format!("\n\n{HER_EXTRA}"), "");
    assert!(!reviewed.contains(HER_EXTRA), "{reviewed}");

    t.merge(&answer, &[("plan.md", Some(reviewed.into_bytes()))]);
    t.pull().await;
    assert_eq!(t.rows("alice").await, vec![]);
    let read = t.reads("alice", "plan").await;
    assert!(
        read.contains(HER_WORD) && !read.contains(HER_EXTRA),
        "{read}"
    );
}

/// She edits again after sharing, and a reviewer rewrites her shared line: her
/// later edit merges against what the branch held, so the two fit.
#[tokio::test]
async fn an_edit_after_sharing_merges_against_what_the_branch_held() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, branch) = proposed(&t, "alice").await;
    let shared = t.row("alice", "plan.md").await;
    same_line(&t).await;
    assert_eq!(
        t.shared("alice", "plan.md"),
        holds(number, Some(shared)),
        "an edit keeps what the proposal holds"
    );
    let reviewed =
        text(&t.branch_file(&branch, "plan.md").unwrap()).replace(HER_WORD, REVIEWED_WORD);

    t.merge(&answer, &[("plan.md", Some(reviewed.clone().into_bytes()))]);
    t.pull().await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    let row = t.row("alice", "plan.md").await;
    assert!(
        row.contains(ALICE_LINE) && row.contains(REVIEWED_WORD) && !row.contains(HER_WORD),
        "{row}"
    );
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.shared, None);
    assert_eq!(record.start.as_deref(), Some(reviewed.as_str()));
}

/// A peer's change lands while her proposal is open: it merges into her row,
/// the record still says what the branch holds, and the proposal's merge ends
/// her draft.
#[tokio::test]
async fn a_peer_change_while_a_proposal_is_open_merges_into_her_row_and_the_merge_ends_it() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, _) = proposed(&t, "alice").await;
    let shared = t.row("alice", "plan.md").await;

    t.push(&[("plan.md", Some(plan_later()))]).await;
    let row = t.row("alice", "plan.md").await;
    assert!(row.contains(BOB_LINE) && row.contains(HER_WORD), "{row}");
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(t.shared("alice", "plan.md"), holds(number, Some(shared)));

    // The forge merges her branch into the `main` bob moved: both lines.
    t.merge(&answer, &[("plan.md", Some(row.into_bytes()))]);
    t.pull().await;
    assert_eq!(t.rows("alice").await, vec![]);
    let read = t.reads("alice", "plan").await;
    assert!(read.contains(BOB_LINE) && read.contains(HER_WORD), "{read}");
}

/// A peer's change to her very line while her proposal is open: her row stands
/// as a conflict, and sharing again refuses rather than amend the branch.
#[tokio::test]
async fn a_peer_change_on_the_same_line_while_a_proposal_is_open_refuses_the_amend() {
    let mut t = team(&[("plan.md", plan())]).await;
    same_line(&t).await;
    let (_, number, branch) = proposed(&t, "alice").await;
    let shared = t.row("alice", "plan.md").await;

    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );
    let again = t.share("alice", None).await;
    assert_eq!(again["outcome"], "conflicts_pending", "{again}");
    let held = text(&t.branch_file(&branch, "plan.md").unwrap());
    assert!(
        held.contains(ALICE_LINE) && !held.contains(BOB_LINE),
        "{held}"
    );
    assert_eq!(t.shared("alice", "plan.md"), holds(number, Some(shared)));
}

/// A status probe marks the proposal merged before any pull brought its text
/// in. A pass then leaves her draft and what it holds alone; the pull that
/// takes the merge ends it.
#[tokio::test]
async fn a_proposal_a_probe_marked_merged_keeps_its_text_until_the_pull() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, _) = proposed(&t, "alice").await;
    let hers = t.row("alice", "plan.md").await;
    t.merge(&answer, &[]);
    // What a status probe leaves: the record marked, still in the chain.
    t.state_edit(|state| {
        let open = state
            .proposals
            .iter_mut()
            .find(|p| p.number == number)
            .unwrap();
        open.status = ProposalStatus::Merged;
    });

    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(t.row("alice", "plan.md").await, hers);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.shared, holds(number, Some(hers.clone())));
    assert_eq!(record.start, Some(text(&plan())));

    t.pull().await;
    assert_eq!(t.rows("alice").await, vec![]);
    assert!(t.reads("alice", "plan").await.contains(HER_WORD));
}

/// She withdraws a proposal after a peer's change merged into her row: the row
/// is left and named, and the next pass forgets what the proposal held while
/// her start stays the team's text the merge read.
#[tokio::test]
async fn a_withdrawn_proposal_forgets_what_it_held_and_a_merged_row_is_left() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (_, number, _) = proposed(&t, "alice").await;
    let shared = t.row("alice", "plan.md").await;
    assert_eq!(t.shared("alice", "plan.md"), holds(number, Some(shared)));
    t.push(&[("plan.md", Some(plan_later()))]).await;
    let merged = t.row("alice", "plan.md").await;

    let report = t
        .eng
        .origin_withdraw("team", None, true, ShareActor::Account("alice".to_string()))
        .await
        .unwrap();
    assert_eq!(
        report["skipped_diverged"],
        serde_json::json!(["plan.md"]),
        "{report}"
    );

    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(t.row("alice", "plan.md").await, merged);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.shared, None);
    assert_eq!(record.start, Some(text(&plan_later())));
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// A reviewer declines her proposal: her draft stands as it was, and once the
/// record leaves the chain the next pass forgets what it held, start untouched.
#[tokio::test]
async fn a_declined_proposal_leaves_the_draft_as_it_stands() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (_, number, _) = proposed(&t, "alice").await;
    let hers = t.row("alice", "plan.md").await;
    assert_eq!(
        t.shared("alice", "plan.md"),
        holds(number, Some(hers.clone()))
    );

    t.mock.set_proposal_state(number, ProposalState::Declined);
    t.pull().await;
    assert_eq!(t.row("alice", "plan.md").await, hers);
    assert_eq!(t.conflicts("alice").await, vec![]);
    // The next share moves a declined record to history; this test does it.
    t.state_edit(|state| {
        let at = state
            .proposals
            .iter()
            .position(|p| p.number == number)
            .unwrap();
        let declined = state.proposals.remove(at);
        assert_eq!(declined.status, ProposalStatus::Declined);
        state.push_history(declined);
    });

    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(t.row("alice", "plan.md").await, hers);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.shared, None);
    assert_eq!(record.from.as_deref(), Some("plan.md"));
    assert_eq!(record.start, Some(text(&plan())));
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// She amends her open proposal with another page only, after editing the
/// shared one again. An amend proposes exactly what it carries, so the branch
/// no longer holds that page: the record forgets it, and her draft stays with
/// both her edits once the proposal merges.
#[tokio::test]
async fn an_amend_that_leaves_a_page_out_forgets_what_the_branch_held_of_it() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, branch) = proposed(&t, "alice").await;
    let shared = t.row("alice", "plan.md").await;
    assert_eq!(t.shared("alice", "plan.md"), holds(number, Some(shared)));
    same_line(&t).await;
    t.write("alice", "Other", "a page only alice has").await;

    let again = t.share("alice", Some(&["other.md"])).await;
    assert_eq!(again["outcome"], "updated", "{again}");
    assert_eq!(
        again["proposal"]["number"].as_u64(),
        Some(number),
        "{again}"
    );
    assert_eq!(t.branch_file(&branch, "plan.md"), Some(plan()));
    assert_eq!(t.shared("alice", "plan.md"), None);
    let other = t.row("alice", "other.md").await;
    assert_eq!(t.shared("alice", "other.md"), holds(number, Some(other)));

    t.merge(&answer, &[]);
    t.pull().await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(t.rows("alice").await, vec![("plan.md".to_string(), false)]);
    let row = t.row("alice", "plan.md").await;
    assert!(row.contains(ALICE_LINE) && row.contains(HER_WORD), "{row}");
}

/// She edits the draft while her share is between the forge and recording
/// what the proposal holds: the branch holds the text from before her edit, so
/// the record is left without it, and her edit survives the merge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_while_a_share_records_what_it_holds_is_not_taken_for_it() {
    let t = Arc::new(team(&[("plan.md", plan())]).await);
    t.append("alice", "plan", HER_WORD).await;
    let (entered, release) = t.eng.hold_next_proposal_record_until();
    let share = {
        let t = t.clone();
        tokio::spawn(async move { t.share("alice", None).await })
    };
    entered.notified().await;
    same_line(&t).await;
    release.notify_one();
    let answer = share.await.unwrap();
    assert_eq!(answer["outcome"], "proposed", "{answer}");
    let branch = answer["branch"].as_str().unwrap();
    let held = text(&t.branch_file(branch, "plan.md").unwrap());
    assert!(
        held.contains(HER_WORD) && !held.contains(ALICE_LINE),
        "{held}"
    );
    assert_eq!(t.shared("alice", "plan.md"), None);

    t.merge(&answer, &[]);
    t.pull().await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    let row = t.row("alice", "plan.md").await;
    assert!(row.contains(ALICE_LINE) && row.contains(HER_WORD), "{row}");
}

/// A reviewer took her whole change back out before merging, and the pass of
/// the pull that brought the merge failed. Her record still starts from before
/// the merge, where the team's text reads as in step, so the draft stands
/// refused until a pass settles it, and no share carries her line again.
#[tokio::test]
async fn a_merged_proposal_whose_pass_failed_is_refused_until_settled() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, _, _) = proposed(&t, "alice").await;

    t.merge(&answer, &[("plan.md", Some(plan()))]);
    t.eng.fail_next_convergence();
    t.pull().await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );
    t.eng.fail_next_convergence();
    let refused = t.share("alice", None).await;
    assert_eq!(refused["outcome"], "conflicts_pending", "{refused}");
    assert_eq!(t.forge_file("plan.md"), Some(text(&plan())));

    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(t.rows("alice").await, vec![]);
    assert!(!t.reads("alice", "plan").await.contains(HER_WORD));
}

/// A proposal of only the deletion half of her move merges: the deletion was
/// hers, so the draft she moved starts nowhere and is a new page, as after a
/// direct commit of that half.
#[tokio::test]
async fn a_merged_proposal_of_only_the_deletion_of_a_move_frees_the_draft() {
    let t = team(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    let answer = t.share("alice", Some(&["plan.md"])).await;
    assert_eq!(answer["outcome"], "proposed", "{answer}");
    let number = answer["number"].as_u64().unwrap();
    assert_eq!(t.shared("alice", "plan.md"), holds(number, None));
    assert_eq!(t.shared("alice", "renamed.md"), None);

    t.merge(&answer, &[]);
    t.pull().await;
    assert_eq!(t.forge_file("plan.md"), None);
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(
        t.rows("alice").await,
        vec![("renamed.md".to_string(), false)]
    );
    let record = t.record("alice", "renamed.md").unwrap();
    assert_eq!((record.from, record.start), (None, None));
}

/// How her case move's proposal merged.
#[derive(Clone, Copy, PartialEq)]
enum CaseMerge {
    /// As she shared it.
    AsShared,
    /// With the reviewer's word in place of hers.
    Reviewed,
    /// As she shared it, and she edited the draft again after sharing.
    EditedSince,
}

/// Her edited case move `plan.md` to `Plan.md`, proposed and merged. A share
/// does not carry the spelling, so the team's file stays `plan.md`.
///
/// Asserted per disk. Where the folder folds case the team's `plan.md` is her
/// file, and both halves end, unless she edited the draft since. Where it keeps
/// the two apart, her case move stays, in step with what merged: the start is
/// the snapshot's real key, never `Plan.md`, which the snapshot does not hold
/// and would read as the team deleting her page.
async fn an_edited_case_move_merged(how: CaseMerge) {
    let t = team(&[("plan.md", plan())]).await;
    let folds = folds_case(&t.root());
    eprintln!("the temp folder folds case: {folds}");
    t.append("alice", "plan", HER_WORD).await;
    t.moved("alice", "plan", "Plan.md").await;
    let (answer, number, branch) = proposed(&t, "alice").await;
    let held = text(&t.branch_file(&branch, "plan.md").unwrap());
    assert!(held.contains(HER_WORD), "{held}");
    assert_eq!(
        t.shared("alice", "Plan.md"),
        holds(number, Some(t.row("alice", "Plan.md").await))
    );
    assert_eq!(
        t.shared("alice", "plan.md"),
        None,
        "the proposal does not delete the team's file"
    );
    let merged = match how {
        CaseMerge::Reviewed => held.replace(HER_WORD, REVIEWED_WORD),
        CaseMerge::AsShared | CaseMerge::EditedSince => held,
    };
    if how == CaseMerge::EditedSince {
        same_line(&t).await;
    }

    t.merge(&answer, &[("plan.md", Some(merged.clone().into_bytes()))]);
    t.pull().await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    if how == CaseMerge::EditedSince {
        assert_eq!(
            t.rows("alice").await,
            vec![
                ("Plan.md".to_string(), false),
                ("plan.md".to_string(), true)
            ]
        );
        let row = t.row("alice", "Plan.md").await;
        assert!(row.contains(ALICE_LINE) && row.contains(HER_WORD), "{row}");
        let record = t.record("alice", "Plan.md").unwrap();
        assert_eq!(record.from.as_deref(), Some("plan.md"));
        assert_eq!(record.start, t.forge_file("plan.md"));
        return;
    }
    let reviewed = how == CaseMerge::Reviewed;
    if folds {
        assert_eq!(t.rows("alice").await, vec![]);
        let read = t.reads("alice", "plan").await;
        assert!(
            read.contains(if reviewed { REVIEWED_WORD } else { HER_WORD }),
            "{read}"
        );
    } else {
        assert_eq!(
            t.rows("alice").await,
            vec![
                ("Plan.md".to_string(), false),
                ("plan.md".to_string(), true)
            ]
        );
        let record = t.record("alice", "Plan.md").unwrap();
        assert_eq!(record.from.as_deref(), Some("plan.md"));
        assert_eq!(record.start, t.forge_file("plan.md"));
        assert_eq!(record.shared, None);
        assert_eq!(t.row("alice", "Plan.md").await, merged);
    }
    assert_eq!(t.share("alice", None).await["outcome"], "nothing_to_share");
}

#[tokio::test]
async fn a_merged_proposal_of_an_edited_case_move_ends_on_a_folding_disk() {
    an_edited_case_move_merged(CaseMerge::AsShared).await;
}

#[tokio::test]
async fn a_merged_proposal_of_an_edited_case_move_a_reviewer_changed_ends_on_a_folding_disk() {
    an_edited_case_move_merged(CaseMerge::Reviewed).await;
}

/// On either disk her case move stays when she wrote more since sharing: the
/// merge carried only what she shared.
#[tokio::test]
async fn a_merged_proposal_of_a_case_move_she_edited_since_stays_on_either_disk() {
    an_edited_case_move_merged(CaseMerge::EditedSince).await;
}

// --- Fix round 1: a merged proposal settles only what it held ----------------

/// Something other than her share rewrote her open proposal, an older
/// instance or a member on another machine, so its record lists none of her
/// pages and its branch holds the team's text of hers. Her own record still
/// names it. Once it merges her draft keeps her edit: only a proposal whose
/// own record still holds her text moves her start.
#[tokio::test]
async fn a_merged_proposal_whose_record_no_longer_holds_her_page_leaves_her_draft() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, _) = proposed(&t, "alice").await;
    let hers = t.row("alice", "plan.md").await;
    assert_eq!(
        t.shared("alice", "plan.md"),
        holds(number, Some(hers.clone()))
    );
    t.state_edit(|state| {
        let open = state
            .proposals
            .iter_mut()
            .find(|p| p.number == number)
            .unwrap();
        open.files.clear();
    });

    t.merge(&answer, &[("plan.md", Some(plan()))]);
    t.pull().await;
    assert_eq!(t.rows("alice").await, vec![("plan.md".to_string(), false)]);
    assert_eq!(t.row("alice", "plan.md").await, hers);
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// An amend leaves her page out, and recording what it holds fails at another
/// page. Her page still forgets what the branch held of it, so the merge leaves
/// her edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_step_that_fails_part_way_still_forgets_a_page_left_out() {
    let t = Arc::new(team(&[("plan.md", plan())]).await);
    t.append("alice", "plan", HER_WORD).await;
    t.write("alice", "Other", "a page only alice has").await;
    let (answer, number, _) = proposed(&t, "alice").await;
    let hers = t.row("alice", "plan.md").await;
    assert_eq!(
        t.shared("alice", "plan.md"),
        holds(number, Some(hers.clone()))
    );
    t.append("alice", "other", "a second word on the other page")
        .await;

    // The record of the other page cannot be read while the amend records.
    let file = draft_start::record_file(&t.state(), "team", "alice", "other.md").unwrap();
    let kept = std::fs::read(&file).unwrap();
    let (entered, release) = t.eng.hold_next_proposal_record_until();
    let share = {
        let t = t.clone();
        tokio::spawn(async move { t.share("alice", Some(&["other.md"])).await })
    };
    entered.notified().await;
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    release.notify_one();
    let again = share.await.unwrap();
    assert_eq!(again["outcome"], "updated", "{again}");
    assert_eq!(t.shared("alice", "plan.md"), None);
    std::fs::remove_dir(&file).unwrap();
    std::fs::write(&file, kept).unwrap();

    t.merge(&answer, &[]);
    t.pull().await;
    assert_eq!(t.row("alice", "plan.md").await, hers);
    assert!(
        !t.conflicts("alice")
            .await
            .iter()
            .any(|c| c.path == "plan.md"),
        "{:?}",
        t.conflicts("alice").await
    );
}

/// She moves her page after sharing it, and the proposal merges with her text
/// at the old path: the move stands, in step, with no word of the team
/// deleting her page.
async fn a_move_after_sharing_merged(reviewed: bool) {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, _, branch) = proposed(&t, "alice").await;
    t.moved("alice", "plan", "renamed.md").await;
    assert_eq!(t.shared("alice", "renamed.md"), None);
    let held = text(&t.branch_file(&branch, "plan.md").unwrap());
    let merged = if reviewed {
        held.replace(
            "the middle of the plan",
            "the middle, as the reviewer put it",
        )
    } else {
        held
    };

    t.merge(&answer, &[("plan.md", Some(merged.into_bytes()))]);
    t.pull().await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(
        t.rows("alice").await,
        vec![
            ("plan.md".to_string(), true),
            ("renamed.md".to_string(), false)
        ]
    );
    let row = t.row("alice", "renamed.md").await;
    assert!(row.contains(HER_WORD), "{row}");
    if reviewed {
        assert!(row.contains("as the reviewer put it"), "{row}");
    }
}

#[tokio::test]
async fn a_move_after_sharing_stays_in_step_after_the_merge() {
    a_move_after_sharing_merged(false).await;
}

#[tokio::test]
async fn a_move_after_sharing_a_reviewer_changed_stays_in_step_after_the_merge() {
    a_move_after_sharing_merged(true).await;
}

/// The forge merges her proposal after the pull read `main` and before it read
/// the proposal's state: that pull's base does not hold the merge, so no pass
/// settles her draft against it, and the next pull brings the merge.
#[tokio::test]
async fn a_merge_between_the_head_probe_and_the_status_probe_waits_for_the_next_pull() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, _) = proposed(&t, "alice").await;
    let hers = t.row("alice", "plan.md").await;
    let merged = t.merged_commit(&answer, &[]);
    t.mock.set_proposal_state(number, ProposalState::Merged);
    t.mock
        .move_branch_after_head_probes(t.mock.branch_head_calls() + 1, "main", &merged);

    t.pull().await;
    // A pass before the next pull, as her share or preview runs one.
    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(t.row("alice", "plan.md").await, hers);
    assert_eq!(t.shared("alice", "plan.md"), holds(number, Some(hers)));

    t.pull().await;
    assert_eq!(t.rows("alice").await, vec![]);
    assert!(t.reads("alice", "plan").await.contains(HER_WORD));
}

/// Her proposal deletes a page, and a reviewer puts the page back before it
/// merges. Her deletion did not land, and she is asked rather than the
/// deletion being proposed again.
#[tokio::test]
async fn a_merged_deletion_a_reviewer_put_back_is_asked() {
    let t = team(&[("plan.md", plan()), ("gone.md", gone())]).await;
    t.delete("alice", "gone").await;
    let (answer, number, _) = proposed(&t, "alice").await;
    assert_eq!(t.shared("alice", "gone.md"), holds(number, None));

    t.merge(&answer, &[("gone.md", Some(gone()))]);
    t.pull().await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "gone.md", DeleteEdit)]
    );
}

/// She shares, adds a line and amends, and while the amend records what it
/// holds she takes the line out again. Her row is neither what this amend
/// holds nor what the first share held, and the record keeps nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_back_while_an_amend_records_keeps_no_earlier_text() {
    let t = Arc::new(team(&[("plan.md", plan())]).await);
    t.append("alice", "plan", HER_WORD).await;
    let (_, number, _) = proposed(&t, "alice").await;
    let first = t.row("alice", "plan.md").await;
    assert_eq!(
        t.shared("alice", "plan.md"),
        holds(number, Some(first.clone()))
    );
    t.append("alice", "plan", "a line she takes back").await;

    let (entered, release) = t.eng.hold_next_proposal_record_until();
    let share = {
        let t = t.clone();
        tokio::spawn(async move { t.share("alice", None).await })
    };
    entered.notified().await;
    t.replace("alice", "plan", "\n\na line she takes back", "")
        .await;
    release.notify_one();
    let again = share.await.unwrap();
    assert_eq!(again["outcome"], "updated", "{again}");
    assert!(!t.row("alice", "plan.md").await.contains("takes back"));
    assert_eq!(t.shared("alice", "plan.md"), None);
}

// --- One proposal at a time, whoever's it is ---------------------------------

/// Bob shares while alice's proposal is open, on a forge with no stacks: the
/// share answers whose proposal is open and what to do, and the branch of
/// alice's proposal is not touched. Her own next share still updates it.
#[tokio::test]
async fn a_share_while_another_members_proposal_is_open_is_refused_without_stacks() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (_, number, branch) = proposed(&t, "alice").await;
    let held = t.mock.branch_commit(&branch).unwrap();
    t.write("bob", "Other", "a page only bob has").await;

    let his = t.share("bob", None).await;
    assert_eq!(his["outcome"], "proposal_open", "{his}");
    assert_eq!(his["own"], false, "{his}");
    assert_eq!(his["proposal"]["number"].as_u64(), Some(number), "{his}");
    assert_eq!(
        his["guidance"],
        format!(
            "alice's proposal #{number} is still open. Share again when it is merged or closed."
        ),
        "{his}"
    );
    assert_eq!(t.mock.branch_commit(&branch), Some(held));
    assert_eq!(t.branch_file(&branch, "other.md"), None);
    let plan = t.preview("bob", None).await;
    assert_eq!(plan["action"], "proposal_open", "{plan}");
    assert_eq!(plan["own"], false, "{plan}");
    assert_eq!(plan["guidance"], his["guidance"], "{plan}");

    t.append("alice", "plan", "a second word of hers").await;
    let again = t.share("alice", None).await;
    assert_eq!(again["outcome"], "updated", "{again}");
    assert_eq!(
        again["proposal"]["number"].as_u64(),
        Some(number),
        "{again}"
    );
    let now = text(&t.branch_file(&branch, "plan.md").unwrap());
    assert!(now.contains("a second word of hers"), "{now}");
}

/// Once alice's proposal is merged, bob's share goes through as his own.
#[tokio::test]
async fn a_share_goes_through_once_another_members_proposal_is_merged() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, _) = proposed(&t, "alice").await;
    t.write("bob", "Other", "a page only bob has").await;
    assert_eq!(t.share("bob", None).await["outcome"], "proposal_open");

    t.merge(&answer, &[]);
    let his = t.share("bob", None).await;
    assert_eq!(his["outcome"], "proposed", "{his}");
    assert_ne!(his["number"].as_u64(), Some(number), "{his}");
}

/// Once alice's proposal is closed, bob's share goes through as his own.
#[tokio::test]
async fn a_share_goes_through_once_another_members_proposal_is_closed() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (_, number, _) = proposed(&t, "alice").await;
    t.write("bob", "Other", "a page only bob has").await;
    assert_eq!(t.share("bob", None).await["outcome"], "proposal_open");

    t.mock.set_proposal_state(number, ProposalState::Declined);
    let his = t.share("bob", None).await;
    assert_eq!(his["outcome"], "proposed", "{his}");
    assert_ne!(his["number"].as_u64(), Some(number), "{his}");
}

// --- A share that races the merge --------------------------------------------

/// The team merges her proposal while her share of a newer edit is between its
/// pull and its push. The forge keeps the merged branch, so the push lands on a
/// proposal that already merged without it: her newer text never reached the
/// team, the receipt says so, and her draft keeps it to share again.
#[tokio::test]
async fn a_share_that_updates_a_proposal_merged_meanwhile_keeps_her_newer_text() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, number, _) = proposed(&t, "alice").await;
    let merged = t.merged_commit(&answer, &[]);
    same_line(&t).await;
    let newer = t.row("alice", "plan.md").await;

    // The share reads the head twice: in the pull it opens with, and in the
    // pinned pull inside the proposal step. The merge lands after the second.
    t.mock
        .merge_after_head_probes(t.mock.branch_head_calls() + 2, number, "main", &merged);
    let again = t.share("alice", None).await;
    assert_eq!(again["outcome"], "updated", "{again}");
    assert_eq!(
        again["note"],
        format!(
            "The team merged proposal #{number} while this change was on its way. If your \
             change is not in it, it stays in your drafts and you can share it again."
        ),
        "{again}"
    );
    assert_eq!(t.shared("alice", "plan.md"), None);

    t.pull().await;
    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(t.row("alice", "plan.md").await, newer);
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert!(t.forge_file("plan.md").unwrap().contains(HER_WORD));
}

// --- Keep mine and Take theirs ----------------------------------------------

impl Team {
    /// `actor` settles their draft at `path`: `keep` is mine or theirs, or
    /// `merged` is the text to write.
    async fn resolve(
        &self,
        actor: &str,
        path: &str,
        keep: Option<&str>,
        merged: Option<&str>,
    ) -> Result<serde_json::Value, EngineError> {
        self.eng
            .origin_resolve(
                "team",
                path,
                keep,
                merged.map(str::as_bytes),
                ShareActor::Account(actor.to_string()),
            )
            .await
    }

    /// The kept texts of `actor`'s drafts, with their text.
    fn kept_of(&self, actor: &str) -> Vec<(String, String)> {
        let root = self.state().join(crystalline_core::kept::KEPT_DIR);
        crystalline_core::kept::held_by(&root, actor)
            .unwrap()
            .into_iter()
            .map(|kept| {
                (
                    kept.path.clone(),
                    std::fs::read_to_string(&kept.file).unwrap(),
                )
            })
            .collect()
    }
}

/// The receipt of a settled draft at `path`.
fn settled(receipt: &serde_json::Value, path: &str, kept: &str, kept_copy: bool) {
    assert_eq!(receipt["resolved"], path, "{receipt}");
    assert_eq!(receipt["remaining"], 0, "{receipt}");
    assert_eq!(receipt["draft"], true, "{receipt}");
    assert_eq!(receipt["kept"], kept, "{receipt}");
    assert_eq!(receipt["kept_copy"], kept_copy, "{receipt}");
}

/// A same-line conflict at plan.md: alice's first line against bob's.
async fn a_same_line_conflict(t: &mut Team) -> String {
    same_line(t).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );
    t.row("alice", "plan.md").await
}

#[tokio::test]
async fn keep_mine_lands_her_text_and_the_second_share_has_nothing() {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    let hers = a_same_line_conflict(&mut t).await;
    // A merge mark a pass left behind (N-2) goes with the settled conflict.
    let marked = DraftStart {
        merging: Some(true),
        ..t.record("alice", "plan.md").unwrap()
    };
    draft_start::save(&t.state(), "team", "alice", &marked).unwrap();

    let receipt = t
        .resolve("alice", "plan.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "mine", false);
    assert_eq!(t.row("alice", "plan.md").await, hers, "her text stays");
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.from.as_deref(), Some("plan.md"));
    assert_eq!(record.start.as_deref(), Some(text(&plan_later()).as_str()));
    assert_eq!(record.merging, None);
    assert_eq!(t.conflicts("alice").await, vec![]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("plan.md").unwrap();
    assert!(
        forge.contains(ALICE_LINE) && !forge.contains(BOB_LINE),
        "{forge}"
    );
    assert_eq!(t.rows("alice").await, vec![]);
    assert_eq!(t.share("alice", None).await["outcome"], "nothing_to_share");
}

#[tokio::test]
async fn take_theirs_ends_the_draft_and_she_reads_the_team() {
    let mut t = team(&[("plan.md", plan())]).await;
    a_same_line_conflict(&mut t).await;

    let receipt = t
        .resolve("alice", "plan.md", Some("theirs"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "theirs", false);
    assert_eq!(t.rows("alice").await, vec![]);
    assert_eq!(t.record("alice", "plan.md"), None);
    let read = t.reads("alice", "plan").await;
    assert!(
        read.contains(BOB_LINE) && !read.contains(ALICE_LINE),
        "{read}"
    );
    assert_eq!(t.share("alice", None).await["outcome"], "nothing_to_share");
}

#[tokio::test]
async fn merged_lands_her_merged_text() {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    a_same_line_conflict(&mut t).await;
    let merged = text(&plan_later()).replace("the last line of the plan", HER_WORD);

    let receipt = t
        .resolve("alice", "plan.md", None, Some(&merged))
        .await
        .unwrap();
    settled(&receipt, "plan.md", "merged", false);
    assert_eq!(t.row("alice", "plan.md").await, merged);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.from.as_deref(), Some("plan.md"));
    assert_eq!(record.start.as_deref(), Some(text(&plan_later()).as_str()));
    assert_eq!(t.conflicts("alice").await, vec![]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("plan.md").unwrap();
    assert!(
        forge.contains(BOB_LINE) && forge.contains(HER_WORD),
        "{forge}"
    );
    assert_eq!(t.rows("alice").await, vec![]);
}

#[tokio::test]
async fn keep_mine_on_a_peer_deleted_file_lands_as_a_new_file() {
    let mut t = team_direct(&[("plan.md", plan()), ("gone.md", gone())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.push(&[("plan.md", None)]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditDelete)]
    );

    let receipt = t
        .resolve("alice", "plan.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "mine", false);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(
        (record.from, record.start, record.start_sha256),
        (None, None, None)
    );
    assert_eq!(t.conflicts("alice").await, vec![]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert!(t.forge_file("plan.md").unwrap().contains(HER_WORD));
    assert_eq!(t.rows("alice").await, vec![]);
}

#[tokio::test]
async fn keep_mine_on_her_delete_of_a_peer_edit_deletes_knowingly() {
    let mut t = team_direct(&[("plan.md", plan()), ("gone.md", gone())]).await;
    t.delete("alice", "plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", DeleteEdit)]
    );

    let receipt = t
        .resolve("alice", "plan.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "mine", false);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.from.as_deref(), Some("plan.md"));
    assert_eq!(record.start.as_deref(), Some(text(&plan_later()).as_str()));
    assert_eq!(t.conflicts("alice").await, vec![]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert_eq!(t.forge_file("plan.md"), None);
    assert_eq!(t.rows("alice").await, vec![]);
}

#[tokio::test]
async fn take_theirs_on_her_delete_brings_the_file_back_for_her() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.delete("alice", "plan").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    let receipt = t
        .resolve("alice", "plan.md", Some("theirs"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "theirs", false);
    assert_eq!(t.rows("alice").await, vec![]);
    assert!(t.reads("alice", "plan").await.contains(BOB_LINE));
}

#[tokio::test]
async fn keep_mine_on_a_peer_file_at_her_path_replaces_it_knowingly() {
    let mut t = team_direct(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    let his = engram("Renamed", "renamed", "bob's own page at that path");
    t.push(&[("renamed.md", Some(his.clone()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", AddAdd)
        }]
    );

    let receipt = t
        .resolve("alice", "renamed.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "renamed.md", "mine", false);
    let record = t.record("alice", "renamed.md").unwrap();
    assert_eq!(record.from.as_deref(), Some("renamed.md"));
    assert_eq!(record.start.as_deref(), Some(text(&his).as_str()));
    let half = t.record("alice", "plan.md").unwrap();
    assert_eq!(half.from.as_deref(), Some("plan.md"));
    assert_eq!(half.start.as_deref(), Some(text(&plan()).as_str()));
    assert_eq!(t.conflicts("alice").await, vec![]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("renamed.md").unwrap();
    assert!(forge.contains("the first line of the plan"), "{forge}");
    assert_eq!(t.forge_file("plan.md"), None);
    assert_eq!(t.rows("alice").await, vec![]);
}

#[tokio::test]
async fn keep_mine_on_an_old_draft_writes_the_first_record() {
    let t = team(&[("plan.md", plan_later())]).await;
    t.old_draft(
        "alice",
        "plan.md",
        &text(&plan()).replace("the last line", "alice's last line"),
    )
    .await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", UnknownStart)]
    );

    let receipt = t
        .resolve("alice", "plan.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "mine", false);
    assert_eq!(
        t.record("alice", "plan.md"),
        Some(DraftStart {
            path: "plan.md".to_string(),
            from: Some("plan.md".to_string()),
            start: Some(text(&plan_later())),
            start_sha256: None,
            shared: None,
            merged_at: None,
            merging: None,
        })
    );
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn keep_mine_on_an_old_move_settles_both_halves_with_one_press() {
    let t = team_direct(&[("plan.md", plan_later())]).await;
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

    let receipt = t
        .resolve("alice", "renamed.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "renamed.md", "mine", false);
    for path in ["renamed.md", "plan.md"] {
        let record = t.record("alice", path).unwrap();
        assert_eq!(record.from.as_deref(), Some("plan.md"), "{path}");
        assert_eq!(
            record.start.as_deref(),
            Some(text(&plan_later()).as_str()),
            "{path}"
        );
    }
    assert_eq!(t.conflicts("alice").await, vec![]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert!(
        t.forge_file("renamed.md")
            .unwrap()
            .contains("alice's last line")
    );
    assert_eq!(t.forge_file("plan.md"), None);
    assert_eq!(t.rows("alice").await, vec![]);
}

/// I-1: bob renames the page she moved and changes no byte. The index then
/// reads the renamed page's address from its path, so the gate sees the page
/// gone. Keep mine reads the team's copy, finds her address taken and refuses,
/// as it does when the gate says so.
#[tokio::test]
async fn keep_mine_is_refused_for_a_taken_address() {
    for changed in [false, true] {
        let mut t = team(&[("plan.md", plan())]).await;
        t.moved("alice", "plan", "mine.md").await;
        let his = if changed { plan_later() } else { plan() };
        t.push(&[("plan.md", None), ("bobs.md", Some(his))]).await;
        let rows = t.rows("alice").await;
        let record = t.record("alice", "mine.md");
        let conflicts = t.conflicts("alice").await;
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");

        let err = t
            .resolve("alice", "mine.md", Some("mine"), None)
            .await
            .unwrap_err();
        let said = err.to_string();
        // The list may say the page is gone (the index's reading of a pure
        // rename), so the sentence says why Keep mine is refused.
        assert_eq!(
            said,
            "The team's file bobs.md now uses the same address as your draft of mine.md, so \
             keeping yours would put two pages at one address. Give it another address in a \
             merged text (Edit merged in Fluid, --content-file on the command line), or take \
             theirs.",
            "changed {changed}"
        );
        // A merged text at that address is refused the same way.
        let same = t.row("alice", "mine.md").await;
        let err = t
            .resolve("alice", "mine.md", None, Some(&same))
            .await
            .unwrap_err();
        let said = err.to_string();
        assert!(
            said.contains("bobs.md")
                && said.contains("merged text")
                && said.contains("take theirs"),
            "changed {changed}: {said}"
        );
        assert_eq!(t.rows("alice").await, rows, "changed {changed}");
        assert_eq!(t.record("alice", "mine.md"), record, "changed {changed}");
        assert_eq!(t.conflicts("alice").await, conflicts, "changed {changed}");
    }
}

#[tokio::test]
async fn a_later_peer_edit_after_keep_mine_merges_again_against_the_new_start() {
    let mut t = team(&[("plan.md", plan())]).await;
    a_same_line_conflict(&mut t).await;
    t.resolve("alice", "plan.md", Some("mine"), None)
        .await
        .unwrap();

    t.push(&[("plan.md", Some(plan_carol()))]).await;
    let row = t.row("alice", "plan.md").await;
    assert!(
        row.contains(ALICE_LINE) && row.contains("carol's middle") && !row.contains(BOB_LINE),
        "{row}"
    );
    assert_eq!(
        t.record("alice", "plan.md").unwrap().start.as_deref(),
        Some(text(&plan_carol()).as_str())
    );
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn resolving_another_persons_path_finds_nothing_of_hers() {
    let mut t = team(&[("plan.md", plan())]).await;
    let hers = a_same_line_conflict(&mut t).await;
    let record = t.record("alice", "plan.md");

    for keep in ["mine", "theirs"] {
        let err = t
            .resolve("bob", "plan.md", Some(keep), None)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::NotFound(_)), "{keep}: {err}");
    }
    assert_eq!(t.row("alice", "plan.md").await, hers);
    assert_eq!(t.record("alice", "plan.md"), record);
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );
}

/// Settling a draft makes no call to the forge, so it works with GitHub off.
#[tokio::test]
async fn a_draft_settles_with_github_off() {
    let mut t = team(&[("plan.md", plan())]).await;
    a_same_line_conflict(&mut t).await;
    let off = t.engine_over(t.eng.store(), |config| {
        config.github = Some(crystalline_core::config::GitHubConfig {
            enabled: Some(false),
            ..Default::default()
        });
    });
    assert!(!off.github_enabled());

    let receipt = off
        .origin_resolve(
            "team",
            "plan.md",
            Some("mine"),
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    settled(&receipt, "plan.md", "mine", false);
    assert_eq!(off.draft_conflicts("team", "alice").await.unwrap(), vec![]);
}

/// M-2: a direct commit of only the deletion half of her move could not free
/// the draft. It reads `EditDelete`, and Keep mine keeps the page as a new file.
#[tokio::test]
async fn keep_mine_after_a_failed_freeing_keeps_the_page() {
    let t = team_direct(&[("plan.md", plan())]).await;
    t.moved("alice", "plan", "renamed.md").await;
    let folder = draft_start::record_file(&t.state(), "team", "alice", "plan.md")
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let broken = folder.join(format!("{}.json", "0".repeat(64)));
    std::fs::write(&broken, b"{ not a record").unwrap();
    let answer = t.share("alice", Some(&["plan.md"])).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert!(answer["free_error"].is_string(), "{answer}");
    std::fs::remove_file(&broken).unwrap();
    assert_eq!(
        t.conflicts("alice").await,
        vec![DraftConflict {
            moved_from: Some("plan.md".to_string()),
            ..conflict("alice", "renamed.md", EditDelete)
        }]
    );

    let receipt = t
        .resolve("alice", "renamed.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "renamed.md", "mine", false);
    let record = t.record("alice", "renamed.md").unwrap();
    assert_eq!((record.from, record.start), (None, None));
    assert_eq!(t.conflicts("alice").await, vec![]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let forge = t.forge_file("renamed.md").unwrap();
    assert!(forge.contains("the first line of the plan"), "{forge}");
    assert_eq!(t.rows("alice").await, vec![]);
}

/// M-1: Keep mine forgets the proposal, so a later pass does not settle a merged
/// proposal over the start she chose and take the reviewer's text into her row.
#[tokio::test]
async fn keep_mine_after_a_merged_proposal_is_not_undone_by_the_next_pass() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let (answer, _, branch) = proposed(&t, "alice").await;
    let hers = t.row("alice", "plan.md").await;
    let reviewed =
        text(&t.branch_file(&branch, "plan.md").unwrap()).replace(HER_WORD, REVIEWED_WORD);
    t.merge(&answer, &[("plan.md", Some(reviewed.clone().into_bytes()))]);
    t.eng.fail_next_convergence();
    t.pull().await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)],
        "a merged proposal no pass settled"
    );

    let receipt = t
        .resolve("alice", "plan.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "mine", false);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.shared, None);
    assert_eq!(record.merging, None);
    assert_eq!(record.start.as_deref(), Some(reviewed.as_str()));

    t.eng.converge_overlays("team").await.unwrap();
    assert_eq!(t.row("alice", "plan.md").await, hers, "her choice stands");
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// M18: Keep mine over a team file with no text writes a digest and no text.
/// The merge of her proposal then settles a text start alone, so the draft ends
/// rather than reading `UnknownStart` for good.
#[tokio::test]
async fn keep_mine_on_a_file_with_no_text_then_a_merged_proposal_reads_one_start() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    let not_text = vec![0xff, 0xfe, 0x00, 0x41];
    t.push(&[("plan.md", Some(not_text.clone()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", EditEdit)]
    );

    let receipt = t
        .resolve("alice", "plan.md", Some("mine"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "mine", false);
    let record = t.record("alice", "plan.md").unwrap();
    assert_eq!(record.start, None);
    assert_eq!(record.start_sha256, Some(sha256_hex(&not_text)));
    assert_eq!(t.conflicts("alice").await, vec![]);

    let (answer, _, _) = proposed(&t, "alice").await;
    t.merge(&answer, &[]);
    t.pull().await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(t.rows("alice").await, vec![]);
    assert!(t.reads("alice", "plan").await.contains(HER_WORD));
}

// Option A's kept copy (owner question 2). Reversible: these three tests and the
// keep in `resolve_in_overlay`.

#[tokio::test]
async fn take_theirs_on_an_old_draft_keeps_a_copy_for_its_owner() {
    let t = team(&[("plan.md", plan_later())]).await;
    let hers = text(&plan()).replace("the last line", "alice's last line");
    t.old_draft("alice", "plan.md", &hers).await;

    let receipt = t
        .resolve("alice", "plan.md", Some("theirs"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "theirs", true);
    assert_eq!(t.rows("alice").await, vec![]);
    assert_eq!(t.kept_of("alice"), vec![("plan.md".to_string(), hers)]);
    assert_eq!(t.kept_of("bob"), vec![]);
}

#[tokio::test]
async fn take_theirs_on_a_new_conflict_keeps_no_copy() {
    let mut t = team(&[("plan.md", plan())]).await;
    a_same_line_conflict(&mut t).await;

    let receipt = t
        .resolve("alice", "plan.md", Some("theirs"), None)
        .await
        .unwrap();
    settled(&receipt, "plan.md", "theirs", false);
    assert_eq!(t.kept_of("alice"), vec![]);
}

#[tokio::test]
async fn a_failed_keep_refuses_the_take() {
    let t = team(&[("plan.md", plan_later())]).await;
    let hers = text(&plan()).replace("the last line", "alice's last line");
    t.old_draft("alice", "plan.md", &hers).await;
    // A file where the kept folder goes, so no text can be kept.
    std::fs::write(
        t.state().join(crystalline_core::kept::KEPT_DIR),
        b"in the way",
    )
    .unwrap();

    t.resolve("alice", "plan.md", Some("theirs"), None)
        .await
        .unwrap_err();
    assert_eq!(t.row("alice", "plan.md").await, hers, "the last copy stays");
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", UnknownStart)]
    );
}

/// `remaining` counts her own conflicts still standing, not the old record's.
#[tokio::test]
async fn the_receipt_counts_her_conflicts_left() {
    let mut t = team(&[("plan.md", plan()), ("gone.md", gone())]).await;
    same_line(&t).await;
    t.append("alice", "gone", HER_WORD).await;
    t.push(&[("plan.md", Some(plan_later())), ("gone.md", None)])
        .await;
    assert_eq!(t.conflicts("alice").await.len(), 2);

    let receipt = t
        .resolve("alice", "plan.md", Some("theirs"), None)
        .await
        .unwrap();
    assert_eq!(receipt["remaining"], 1, "{receipt}");
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "gone.md", EditDelete)]
    );
}

/// m-2: a folder conflict in a reviewing domain is the folder's, and settling
/// it writes the team's folder, so it still needs GitHub on. Only a draft
/// settles with GitHub off.
#[tokio::test]
async fn a_folder_conflict_in_a_reviewing_domain_still_needs_github() {
    let t = team(&[("plan.md", plan())]).await;
    t.state_edit(|state| {
        state.conflicts.push(crystalline_remote::state::Conflict {
            id: "abcdef01".to_string(),
            path: "plan.md".to_string(),
            kind: crystalline_remote::merge::ConflictKind::EditEdit,
            base_commit: "base".to_string(),
            upstream_commit: "upstream".to_string(),
            detected_at: chrono::Utc::now(),
        });
    });
    let off = t.engine_over(t.eng.store(), |config| {
        config.github = Some(crystalline_core::config::GitHubConfig {
            enabled: Some(false),
            ..Default::default()
        });
    });

    for keep in ["mine", "theirs"] {
        let err = off
            .origin_resolve(
                "team",
                "plan.md",
                Some(keep),
                None,
                ShareActor::Account("alice".to_string()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::Remote(crystalline_remote::RemoteError::NotEnabled)
            ),
            "{keep}: {err}"
        );
    }
    assert_eq!(std::fs::read(t.root().join("plan.md")).unwrap(), plan());
}

// --- Files beside the pages -------------------------------------------------
//
// A file never merges. Its record says which team bytes it started from, and a
// team change since is a conflict that only a choice settles: an upload of other
// bytes does not.

const DECK: &str = "assets/deck.png";
const TEAM_DECK: &[u8] = b"\x89PNG\r\n\x1a\n\x00the team's deck";
const BOBS_DECK: &[u8] = b"\x89PNG\r\n\x1a\n\x00bob's newer deck";
const HER_DECK: &[u8] = b"\x89PNG\r\n\x1a\n\x00alice's deck";

fn file_conflict(actor: &str, path: &str, kind: DraftConflictKind) -> DraftConflict {
    DraftConflict {
        file: true,
        ..conflict(actor, path, kind)
    }
}

impl Team {
    /// `actor` uploads `bytes` at `path`, which lands as their draft file.
    async fn upload(&self, actor: &str, path: &str, bytes: &[u8]) {
        let written = self
            .eng
            .attachment_write_as("team", path, bytes.to_vec(), &scope(actor))
            .await
            .unwrap();
        assert!(written.draft, "an upload in review mode is a draft");
    }

    /// `actor` deletes the file at `path`, which lands as their draft deletion.
    async fn delete_file(&self, actor: &str, path: &str) {
        let draft = self
            .eng
            .attachment_delete_as("team", path, &scope(actor))
            .await
            .unwrap();
        assert!(draft, "a deletion in review mode is a draft");
    }

    /// The bytes `actor` holds at `path` in their files overlay, `Some(None)`
    /// for a deletion, `None` for nothing.
    fn held_file(&self, actor: &str, path: &str) -> Option<Option<Vec<u8>>> {
        let file = self
            .state()
            .join("overlays")
            .join("team")
            .join(actor)
            .join("files")
            .join(path);
        let mut sidecar = file.clone().into_os_string();
        sidecar.push(".tombstone");
        match std::fs::read(&file) {
            Ok(bytes) => Some(Some(bytes)),
            Err(_) if Path::new(&sidecar).exists() => Some(None),
            Err(_) => None,
        }
    }

    fn file_record(&self, actor: &str, path: &str) -> Option<draft_start::DraftFileStart> {
        draft_start::load_file(&self.state(), "team", actor, path).unwrap()
    }

    /// What `actor` reads at the file `path`.
    async fn reads_file(&self, actor: &str, path: &str) -> Option<Vec<u8>> {
        match self
            .eng
            .attachment_read_as("team", path, &scope(actor))
            .await
        {
            Ok((bytes, _)) => Some(bytes),
            Err(EngineError::NotFound(_)) => None,
            Err(e) => panic!("{e}"),
        }
    }
}

/// Alice uploads her deck over the team's, and bob's newer deck lands first.
async fn a_stale_upload(t: &mut Team) {
    t.upload("alice", DECK, HER_DECK).await;
    t.push(&[(DECK, Some(BOBS_DECK.to_vec()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, EditEdit)]
    );
}

#[tokio::test]
async fn a_stale_uploaded_file_refuses_the_share_and_the_peers_file_stays() {
    let mut t = team_direct(&[(DECK, TEAM_DECK.to_vec())]).await;
    a_stale_upload(&mut t).await;
    assert_eq!(t.conflicts("bob").await, vec![], "nobody else's");

    for files in [None, Some(&[DECK][..])] {
        let answer = t.share("alice", files).await;
        assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
        assert_eq!(answer["conflicts"][0]["path"], DECK, "{answer}");
        assert_eq!(answer["conflicts"][0]["file"], true, "{answer}");
    }
    assert_eq!(t.branch_file("main", DECK).unwrap(), BOBS_DECK);
    assert_eq!(t.held_file("alice", DECK), Some(Some(HER_DECK.to_vec())));

    // Another upload is not a choice: the start stays where she began.
    t.upload("alice", DECK, b"\x89PNG\r\n\x1a\n\x00her second try")
        .await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, EditEdit)]
    );
    assert_eq!(t.share("alice", None).await["outcome"], "conflicts_pending");
    assert_eq!(t.branch_file("main", DECK).unwrap(), BOBS_DECK);
}

#[tokio::test]
async fn a_stale_deleted_file_refuses_the_share() {
    let mut t = team_direct(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.delete_file("alice", DECK).await;
    t.push(&[(DECK, Some(BOBS_DECK.to_vec()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, DeleteEdit)]
    );

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(t.branch_file("main", DECK).unwrap(), BOBS_DECK);
    assert_eq!(t.held_file("alice", DECK), Some(None));
}

#[tokio::test]
async fn keep_mine_on_a_file_lands_her_bytes() {
    let mut t = team_direct(&[(DECK, TEAM_DECK.to_vec())]).await;
    a_stale_upload(&mut t).await;

    let receipt = t.resolve("alice", DECK, Some("mine"), None).await.unwrap();
    settled(&receipt, DECK, "mine", false);
    assert_eq!(t.held_file("alice", DECK), Some(Some(HER_DECK.to_vec())));
    assert_eq!(
        t.file_record("alice", DECK).unwrap().start_sha.as_deref(),
        Some(sha256_hex(BOBS_DECK).as_str())
    );
    assert_eq!(t.conflicts("alice").await, vec![]);

    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert_eq!(t.branch_file("main", DECK).unwrap(), HER_DECK);
    assert_eq!(t.held_file("alice", DECK), None, "her file has landed");
    assert_eq!(t.file_record("alice", DECK), None);
    assert_eq!(t.share("alice", None).await["outcome"], "nothing_to_share");
}

#[tokio::test]
async fn take_theirs_on_a_file_clears_hers() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    a_stale_upload(&mut t).await;

    let receipt = t
        .resolve("alice", DECK, Some("theirs"), None)
        .await
        .unwrap();
    settled(&receipt, DECK, "theirs", false);
    assert_eq!(t.held_file("alice", DECK), None);
    assert_eq!(t.file_record("alice", DECK), None);
    assert_eq!(t.reads_file("alice", DECK).await.unwrap(), BOBS_DECK);
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(t.share("alice", None).await["outcome"], "nothing_to_share");
}

#[tokio::test]
async fn merged_on_a_file_refuses() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    a_stale_upload(&mut t).await;

    let err = t
        .resolve("alice", DECK, None, Some("merged bytes"))
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "A file cannot be merged here. Keep one of the two."
    );
    assert_eq!(t.held_file("alice", DECK), Some(Some(HER_DECK.to_vec())));
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, EditEdit)]
    );
}

#[tokio::test]
async fn an_old_uploaded_file_is_unknown_start() {
    let t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    assert_eq!(
        t.file_record("alice", DECK).unwrap().start_sha.as_deref(),
        Some(sha256_hex(TEAM_DECK).as_str())
    );
    // A file uploaded before the record: nothing says where it started.
    draft_start::clear_file(&t.state(), "team", "alice", DECK).unwrap();
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, UnknownStart)]
    );
    assert_eq!(t.share("alice", None).await["outcome"], "conflicts_pending");

    // Keep mine gives it its first record.
    t.resolve("alice", DECK, Some("mine"), None).await.unwrap();
    assert_eq!(t.conflicts("alice").await, vec![]);
}

#[tokio::test]
async fn the_same_bytes_as_the_team_clear() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    a_stale_upload(&mut t).await;

    // She uploads bob's bytes: her draft is the team's file.
    t.upload("alice", DECK, BOBS_DECK).await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    let answer = t.share("alice", None).await;
    assert_eq!(answer["outcome"], "nothing_to_share", "{answer}");
    assert_eq!(t.held_file("alice", DECK), None, "the share's pass ends it");
    assert_eq!(t.file_record("alice", DECK), None);

    // And a team change to exactly her bytes ends her draft as well.
    t.upload("alice", DECK, HER_DECK).await;
    t.push(&[(DECK, Some(HER_DECK.to_vec()))]).await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(t.held_file("alice", DECK), None);
    assert_eq!(t.file_record("alice", DECK), None);
}

#[tokio::test]
async fn a_fold_refuses_a_stale_file() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    a_stale_upload(&mut t).await;

    let err = t
        .eng
        .set_review_mode(
            "team",
            None,
            crystalline_service::ReviewModeConfirm::Confirmed {
                folds: [("alice".to_string(), crystalline_service::FoldChoice::Fold)]
                    .into_iter()
                    .collect(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(err.to_string().contains(DECK), "{err}");
    assert_eq!(std::fs::read(t.root().join(DECK)).unwrap(), BOBS_DECK);
    assert_eq!(t.held_file("alice", DECK), Some(Some(HER_DECK.to_vec())));
}

#[tokio::test]
async fn a_new_file_where_the_team_added_one_is_add_add() {
    let mut t = team_direct(&[]).await;
    t.upload("alice", DECK, HER_DECK).await;
    assert_eq!(
        t.file_record("alice", DECK),
        Some(draft_start::DraftFileStart {
            path: DECK.to_string(),
            start_sha: None,
            shared: None,
        })
    );
    t.push(&[(DECK, Some(BOBS_DECK.to_vec()))]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, AddAdd)]
    );
    assert_eq!(t.share("alice", None).await["outcome"], "conflicts_pending");
    assert_eq!(t.branch_file("main", DECK).unwrap(), BOBS_DECK);
}

#[tokio::test]
async fn an_upload_over_a_file_the_team_deleted_is_edit_delete() {
    let mut t = team_direct(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    t.push(&[(DECK, None)]).await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, EditDelete)]
    );
    assert_eq!(t.share("alice", None).await["outcome"], "conflicts_pending");
    assert_eq!(t.branch_file("main", DECK), None);

    // Keep mine lands it as a new file.
    t.resolve("alice", DECK, Some("mine"), None).await.unwrap();
    assert_eq!(t.file_record("alice", DECK).unwrap().start_sha, None);
    assert_eq!(t.share("alice", None).await["outcome"], "committed");
    assert_eq!(t.branch_file("main", DECK).unwrap(), HER_DECK);
}

#[tokio::test]
async fn resolving_a_file_she_does_not_hold_is_not_found() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    a_stale_upload(&mut t).await;
    let err = t
        .resolve("bob", DECK, Some("theirs"), None)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound(_)), "{err}");
    assert_eq!(t.held_file("alice", DECK), Some(Some(HER_DECK.to_vec())));
}

/// A pull moved the snapshot and its folder write failed, so her view still
/// shows the old file. She replaces what she saw, which is not the team's now.
#[tokio::test]
async fn an_upload_while_the_folder_lags_the_snapshot_is_a_conflict() {
    let t = team_direct(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.snapshot_only(DECK, BOBS_DECK);
    t.upload("alice", DECK, HER_DECK).await;
    assert_eq!(
        t.file_record("alice", DECK).unwrap().start_sha.as_deref(),
        Some(sha256_hex(TEAM_DECK).as_str())
    );
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, EditEdit)]
    );
}

/// A file from before the record that a pull from before saw the team delete:
/// the frozen list says so, and its share would bring the file back.
#[tokio::test]
async fn an_old_uploaded_file_the_team_deleted_is_edit_delete() {
    let t = team(&[]).await;
    for path in [DECK, "assets/other.png"] {
        t.upload("alice", path, HER_DECK).await;
        draft_start::clear_file(&t.state(), "team", "alice", path).unwrap();
    }
    let mut record = ConvergenceRecord::default();
    record
        .conflicts
        .insert("alice".to_string(), vec![DECK.to_string()]);
    overlay_journal::journal_save_record(&t.state(), "team", &record).unwrap();
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, EditDelete)],
        "the file the list does not name is a new file"
    );

    // Take theirs takes it off the list with the file.
    t.resolve("alice", DECK, Some("theirs"), None)
        .await
        .unwrap();
    assert_eq!(t.conflicts("alice").await, vec![]);
    let record = overlay_journal::journal_record(&t.state(), "team");
    assert_eq!(record.conflicts.get("alice"), None);
}

/// A file's record that cannot be read refuses the list and the share, and is
/// never read as a file with no record (C1).
#[tokio::test]
async fn a_broken_file_record_refuses_the_list_and_the_share() {
    let t = team_direct(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    let file = draft_start::file_record_file(&t.state(), "team", "alice", DECK).unwrap();
    std::fs::write(&file, b"").unwrap();

    t.eng.draft_conflicts("team", "alice").await.unwrap_err();
    t.eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap_err();
    assert_eq!(t.branch_file("main", DECK).unwrap(), TEAM_DECK);
}

// --- Fix round 1: case twins, kept copies, merged proposals, renames ---------

const DECK_TWIN: &str = "assets/Deck.png";

/// I-1: a spelling that differs from a team file only by case is one file on
/// a disk that folds case, so it is refused there and names the team's
/// spelling. A case-sensitive disk keeps the two apart, and both land.
#[tokio::test]
async fn a_case_twin_of_a_team_file_is_refused_where_the_disk_folds_case() {
    let mut t = team_direct(&[(DECK, TEAM_DECK.to_vec())]).await;
    let attempt = t
        .eng
        .attachment_write_as("team", DECK_TWIN, HER_DECK.to_vec(), &scope("alice"))
        .await;
    if folds_case(&t.root()) {
        let err = attempt.unwrap_err();
        assert!(matches!(err, EngineError::Conflict(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "'assets/Deck.png' and the team's file 'assets/deck.png' differ only by case, and \
             this disk may keep only one of them. Use the name 'assets/deck.png'."
        );
        assert_eq!(t.held_file("alice", DECK_TWIN), None);
        let err = t
            .eng
            .attachment_delete_as("team", DECK_TWIN, &scope("alice"))
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Conflict(_)), "{err}");
        assert_eq!(t.held_file("alice", DECK_TWIN), None);
    } else {
        attempt.unwrap();
        t.push(&[(DECK, Some(BOBS_DECK.to_vec()))]).await;
        assert_eq!(t.conflicts("alice").await, vec![]);
        let answer = t.share("alice", None).await;
        assert_eq!(answer["outcome"], "committed", "{answer}");
        assert_eq!(t.branch_file("main", DECK).unwrap(), BOBS_DECK);
        assert_eq!(t.branch_file("main", DECK_TWIN).unwrap(), HER_DECK);
    }
}

/// I-1: the team renames her file by case and changes it. On a folding disk the
/// team's file is the one her share would overwrite, so it is EditEdit; on a
/// case-sensitive disk her path is gone, EditDelete.
#[tokio::test]
async fn a_team_case_rename_under_her_file_reads_per_disk() {
    let mut t = team_direct(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    t.push(&[(DECK, None), (DECK_TWIN, Some(BOBS_DECK.to_vec()))])
        .await;
    let kind = if folds_case(&t.root()) {
        EditEdit
    } else {
        EditDelete
    };
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, kind)]
    );
    assert_eq!(t.share("alice", None).await["outcome"], "conflicts_pending");
    assert_eq!(t.branch_file("main", DECK_TWIN).unwrap(), BOBS_DECK);

    // Keep mine reads the team's file where the gate reads it, so it settles.
    t.resolve("alice", DECK, Some("mine"), None).await.unwrap();
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// I-2: Take theirs on a file from before the record keeps her bytes as a
/// copy, since they exist nowhere else.
#[tokio::test]
async fn take_theirs_on_an_old_file_keeps_a_copy_of_her_bytes() {
    let t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    draft_start::clear_file(&t.state(), "team", "alice", DECK).unwrap();
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, UnknownStart)]
    );

    let receipt = t
        .resolve("alice", DECK, Some("theirs"), None)
        .await
        .unwrap();
    settled(&receipt, DECK, "theirs", true);
    // C24: a path on this server is not hers to use; the receipt says where
    // the copy is listed instead.
    assert_eq!(receipt["kept_file"], serde_json::Value::Null, "{receipt}");
    assert_eq!(
        receipt["kept_note"], "A copy of your file is kept. Find it in your kept list.",
        "{receipt}"
    );
    assert_eq!(t.held_file("alice", DECK), None);
    let root = t.state().join(crystalline_core::kept::KEPT_DIR);
    let listed = crystalline_core::kept::held_by(&root, "alice").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, DECK);
    assert_eq!(std::fs::read(&listed[0].file).unwrap(), HER_DECK);

    // Read as a kept text it is a plain sentence that says where it is.
    let err = t
        .eng
        .kept_text("team", &listed[0].id, &scope("alice"))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "This kept copy is a file, not text. Download it from the kept list."
    );
}

/// I-2: a new conflict is not the last copy, so Take theirs keeps none.
#[tokio::test]
async fn take_theirs_on_a_new_file_conflict_keeps_no_copy() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    a_stale_upload(&mut t).await;
    let receipt = t
        .resolve("alice", DECK, Some("theirs"), None)
        .await
        .unwrap();
    settled(&receipt, DECK, "theirs", false);
    let root = t.state().join(crystalline_core::kept::KEPT_DIR);
    assert!(
        crystalline_core::kept::held_by(&root, "alice")
            .unwrap()
            .is_empty()
    );
}

/// M-1: her proposal merged her bytes and the team changed the file after.
/// Her file has landed, so it ends and the team's newer file stands.
#[tokio::test]
async fn a_merged_proposal_that_held_her_file_ends_it_when_the_team_changes_it_after() {
    let t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    let (answer, _, _) = proposed(&t, "alice").await;
    t.merge(&answer, &[]);
    t.commit(&[(DECK, Some(BOBS_DECK.to_vec()))]);
    t.pull().await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(t.held_file("alice", DECK), None);
    assert_eq!(t.reads_file("alice", DECK).await.unwrap(), BOBS_DECK);
}

/// M-1: bytes she uploaded after sharing were not in the merged proposal, so
/// a team change after the merge is asked about, as a page's newer edit is.
#[tokio::test]
async fn a_newer_upload_than_her_merged_proposal_is_asked_about() {
    let t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    let (answer, _, _) = proposed(&t, "alice").await;
    let newer = b"\x89PNG\r\n\x1a\n\x00alice's newer deck";
    t.upload("alice", DECK, newer).await;
    t.merge(&answer, &[]);
    t.commit(&[(DECK, Some(BOBS_DECK.to_vec()))]);
    t.pull().await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, EditEdit)]
    );
    assert_eq!(t.held_file("alice", DECK), Some(Some(newer.to_vec())));
}

/// M-3: the team moved the file she deleted. Her deletion does not end in
/// silence: it is DeleteEdit, and her deletion stands.
#[tokio::test]
async fn a_deletion_of_a_file_the_team_moved_is_delete_edit() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.delete_file("alice", DECK).await;
    t.push(&[(DECK, None), ("assets/moved.png", Some(TEAM_DECK.to_vec()))])
        .await;
    assert_eq!(
        t.conflicts("alice").await,
        vec![file_conflict("alice", DECK, DeleteEdit)]
    );
    assert_eq!(t.held_file("alice", DECK), Some(None));

    // The team deleting it outright still ends her deletion.
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.delete_file("alice", DECK).await;
    t.push(&[(DECK, None)]).await;
    assert_eq!(t.conflicts("alice").await, vec![]);
    assert_eq!(t.held_file("alice", DECK), None);
}

/// M-4: a name that cannot address a folder holds no files, so its list is
/// empty and not an error.
#[tokio::test]
async fn a_name_that_cannot_hold_files_has_no_conflicts() {
    let t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    assert_eq!(t.eng.draft_conflicts("team", "..").await.unwrap(), vec![]);
}

/// M-4: the pass decides a file has landed, and her new upload lands before it
/// takes the file's lock. The second look under the lock sees the new bytes,
/// so they stay.
#[tokio::test]
async fn an_upload_between_the_pass_deciding_and_clearing_stays() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    let file = t.state().join("overlays/team/alice/files").join(DECK);
    let later = b"\x89PNG\r\n\x1a\n\x00alice's later deck".to_vec();
    let written = later.clone();
    t.eng.before_next_file_settle(Box::new(move || {
        std::fs::write(&file, &written).unwrap();
    }));
    // The team takes her first bytes: her file would land.
    t.push(&[(DECK, Some(HER_DECK.to_vec()))]).await;
    assert_eq!(t.held_file("alice", DECK), Some(Some(later)));
}

/// N-2: a deletion she makes again after a merged proposal of hers deleted the
/// file once is a new draft. It stays, and its share deletes the team's file.
#[tokio::test]
async fn a_deletion_again_after_an_old_merged_deletion_stays() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec()), ("plan.md", plan())]).await;
    t.delete_file("alice", DECK).await;
    let (answer, _, _) = proposed(&t, "alice").await;
    t.merge(&answer, &[]);
    t.pull().await;
    assert_eq!(t.held_file("alice", DECK), None, "her deletion landed");

    t.push(&[(DECK, Some(BOBS_DECK.to_vec()))]).await;
    t.delete_file("alice", DECK).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(t.held_file("alice", DECK), Some(None), "never dropped");
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// N-2: putting back the bytes an old merged proposal held, over a newer team
/// file, is a new draft. It stays.
#[tokio::test]
async fn a_revert_to_an_old_merged_version_stays() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec()), ("plan.md", plan())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    let (answer, _, _) = proposed(&t, "alice").await;
    t.merge(&answer, &[]);
    t.pull().await;
    assert_eq!(t.held_file("alice", DECK), None, "her file landed");

    t.push(&[(DECK, Some(BOBS_DECK.to_vec()))]).await;
    t.upload("alice", DECK, HER_DECK).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(t.held_file("alice", DECK), Some(Some(HER_DECK.to_vec())));
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// Her own two spellings of one file name are one file where the disk may
/// fold case, so the second is refused and names the first.
#[tokio::test]
async fn her_own_case_twin_is_refused_where_the_disk_folds_case() {
    let t = team(&[]).await;
    t.upload("alice", "assets/new.png", HER_DECK).await;
    let attempt = t
        .eng
        .attachment_write_as(
            "team",
            "assets/New.png",
            BOBS_DECK.to_vec(),
            &scope("alice"),
        )
        .await;
    if folds_case(&t.root()) {
        let err = attempt.unwrap_err();
        assert!(matches!(err, EngineError::Conflict(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "You already draft 'assets/new.png', and on this disk the two names may be one \
             file. Use 'assets/new.png'."
        );
        assert_eq!(
            t.held_file("alice", "assets/new.png"),
            Some(Some(HER_DECK.to_vec()))
        );
    } else {
        attempt.unwrap();
        assert_eq!(
            t.held_file("alice", "assets/New.png"),
            Some(Some(BOBS_DECK.to_vec()))
        );
    }
}

/// N-2: a file her open proposal holds is not settled by that proposal until
/// it merges. A pull meanwhile leaves it as it is.
#[tokio::test]
async fn a_file_in_an_open_proposal_stays_until_it_merges() {
    let mut t = team(&[(DECK, TEAM_DECK.to_vec()), ("plan.md", plan())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    let (_, number, _) = proposed(&t, "alice").await;
    assert_eq!(
        t.file_record("alice", DECK).unwrap().shared,
        Some(draft_start::SharedFile {
            proposal: number,
            sha: Some(sha256_hex(HER_DECK)),
        })
    );
    t.push(&[("plan.md", Some(plan_later()))]).await;
    assert_eq!(t.held_file("alice", DECK), Some(Some(HER_DECK.to_vec())));
    assert_eq!(t.conflicts("alice").await, vec![]);
}

// --- Task 12: status and update name her own conflicts ------------------------

/// Her own conflict as status and update list it, with `draft: true`.
fn listed(actor: &str, path: &str, kind: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id(actor, path),
        "path": path,
        "kind": kind,
        "draft": true,
        "moved_from": null,
        "other": null,
        "file": false,
    })
}

/// What `scope`'s status of the team domain says, with `detail` as given.
async fn status_of(t: &Team, scope: &Scope, detail: bool) -> serde_json::Value {
    t.eng
        .origin_status(Some("team"), detail, false, scope)
        .await
        .unwrap()["domains"][0]
        .clone()
}

#[tokio::test]
async fn an_update_names_her_conflict_and_a_colleague_reads_none_of_it() {
    let t = team(&[("plan.md", plan())]).await;
    same_line(&t).await;
    t.commit(&[("plan.md", Some(plan_later()))]);

    let hers = t
        .eng
        .origin_update(Some("team"), &scope("alice"))
        .await
        .unwrap();
    let domain = &hers["domains"][0];
    assert_eq!(
        domain["conflicts"],
        serde_json::json!([listed("alice", "plan.md", "EditEdit")]),
        "{hers}"
    );
    assert_eq!(domain["drafts_merged"], serde_json::json!([]), "{hers}");

    let his = t
        .eng
        .origin_update(Some("team"), &scope("bob"))
        .await
        .unwrap();
    let domain = &his["domains"][0];
    assert_eq!(domain["conflicts"], serde_json::json!([]), "{his}");
    assert_eq!(domain["drafts_merged"], serde_json::json!([]), "{his}");
    assert!(!his.to_string().contains(&id("alice", "plan.md")), "{his}");
}

#[tokio::test]
async fn an_update_names_a_merged_draft() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.commit(&[("plan.md", Some(plan_later()))]);

    let hers = t
        .eng
        .origin_update(Some("team"), &scope("alice"))
        .await
        .unwrap();
    let domain = &hers["domains"][0];
    assert_eq!(
        domain["drafts_merged"],
        serde_json::json!(["plan.md"]),
        "{hers}"
    );
    assert_eq!(domain["conflicts"], serde_json::json!([]), "{hers}");

    // Up to date now, and she is still told until the draft ends.
    let again = t
        .eng
        .origin_update(Some("team"), &scope("alice"))
        .await
        .unwrap();
    assert_eq!(again["domains"][0]["up_to_date"], true, "{again}");
    assert_eq!(
        again["domains"][0]["drafts_merged"],
        serde_json::json!(["plan.md"]),
        "{again}"
    );
}

/// A merge the poller did reaches her through status: she ran nothing.
#[tokio::test]
async fn status_names_a_merge_a_poller_did() {
    let t = team(&[("plan.md", plan())]).await;
    t.append("alice", "plan", HER_WORD).await;
    t.commit(&[("plan.md", Some(plan_later()))]);
    assert_eq!(
        status_of(&t, &scope("alice"), false).await["drafts_merged"],
        serde_json::json!([])
    );

    let tokens = t.tmp.path().join("tokens");
    std::fs::create_dir_all(&tokens).unwrap();
    crystalline_remote::TokenStore::File {
        path: tokens.join("github-token.json"),
    }
    .save(&crystalline_remote::StoredToken {
        access_token: "test-token".to_string(),
        host: "github.com".to_string(),
        user: "mock-user".to_string(),
        created_at: chrono::Utc::now(),
        refresh_token: None,
        expires_at: None,
        refresh_expires_at: None,
    })
    .unwrap();
    let poller = t
        .engine_over(t.eng.store(), |_| {})
        .with_token_store_dir(tokens);
    poller
        .origin_poll_tick(std::time::Instant::now(), chrono::Utc::now())
        .await;
    assert!(t.row("alice", "plan.md").await.contains(BOB_LINE));

    let domain = status_of(&t, &scope("alice"), false).await;
    assert_eq!(
        domain["drafts_merged"],
        serde_json::json!(["plan.md"]),
        "{domain}"
    );
    assert_eq!(domain["conflicts"], serde_json::json!([]), "{domain}");
    let his = status_of(&t, &scope("bob"), false).await;
    assert_eq!(his["drafts_merged"], serde_json::json!([]), "{his}");
}

const BOB_DRAFT: &str = "bob's own draft of the first line";

#[tokio::test]
async fn status_detail_carries_both_texts_and_only_her_own() {
    let mut t = team(&[("plan.md", plan())]).await;
    same_line(&t).await;
    t.replace("bob", "plan", "the first line of the plan", BOB_DRAFT)
        .await;
    let hers = t.row("alice", "plan.md").await;
    t.push(&[("plan.md", Some(plan_later()))]).await;

    let domain = status_of(&t, &scope("alice"), true).await;
    let mut expected = listed("alice", "plan.md", "EditEdit");
    expected["base"] = serde_json::json!(text(&plan()));
    expected["local"] = serde_json::json!(hers);
    expected["upstream"] = serde_json::json!(text(&plan_later()));
    assert_eq!(
        domain["conflicts"],
        serde_json::json!([expected]),
        "{domain}"
    );
    assert!(!domain.to_string().contains(BOB_DRAFT), "{domain}");
    assert!(
        !domain.to_string().contains(&id("bob", "plan.md")),
        "{domain}"
    );

    // Without detail the entry names the conflict and carries no text.
    let plain = status_of(&t, &scope("alice"), false).await;
    assert_eq!(
        plain["conflicts"],
        serde_json::json!([listed("alice", "plan.md", "EditEdit")]),
        "{plain}"
    );
}

#[tokio::test]
async fn a_domain_owner_reads_counts_not_paths() {
    let other = |first: &str| {
        engram(
            "Other",
            "other",
            &format!("{first}\n\nthe middle of the other page\n\nthe end of the other page"),
        )
    };
    let mut t = team(&[("plan.md", plan()), ("other.md", other("the other page"))]).await;
    same_line(&t).await;
    t.append("alice", "other", HER_WORD).await;
    t.push(&[
        ("plan.md", Some(plan_later())),
        ("other.md", Some(other("bob changed the other page"))),
    ])
    .await;
    assert_eq!(
        status_of(&t, &scope("alice"), false).await["drafts_merged"],
        serde_json::json!(["other.md"])
    );

    let owners = status_of(&t, &Scope::Unrestricted, true).await;
    assert_eq!(owners["conflicts"], serde_json::json!([]), "{owners}");
    assert_eq!(owners["drafts_merged"], serde_json::json!([]), "{owners}");
    assert_eq!(
        owners["converged"]["actors"],
        serde_json::json!([{ "actor": "alice", "entries": 1 }]),
        "{owners}"
    );
    let said = owners.to_string();
    assert!(!said.contains(&id("alice", "plan.md")), "{said}");
    assert!(!said.contains(ALICE_LINE), "{said}");
    assert!(!said.contains(HER_WORD), "{said}");
}

/// The Task 5 review's M2: a record that cannot be read names the two ways
/// out, in status and in the share's refusal, and both ways work. An account
/// reaches this server from elsewhere, so its sentence names no place on the
/// server's disk (C24).
#[tokio::test]
async fn a_broken_record_names_its_ways_out() {
    let mut t = team(&[("plan.md", plan())]).await;
    same_line(&t).await;
    t.push(&[("plan.md", Some(plan_later()))]).await;
    let file = draft_start::record_file(&t.state(), "team", "alice", "plan.md").unwrap();
    let said = "The record of where your draft of plan.md started cannot be read. Discard that \
                draft, or ask whoever runs this server to remove its record, so it is read as \
                a draft from before 0.32.0.";

    std::fs::write(&file, b"{ not a record").unwrap();
    let domain = status_of(&t, &scope("alice"), false).await;
    assert_eq!(domain["conflicts_unreadable_reason"], said, "{domain}");
    assert!(domain.get("conflicts_unreadable").is_none(), "{domain}");
    assert_eq!(domain["conflicts"], serde_json::json!([]), "{domain}");
    let state = t.state().display().to_string();
    assert!(!domain.to_string().contains(&state), "{domain}");
    let err = t
        .eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err:?}");
    assert_eq!(err.to_string(), said);

    // A file removed reads as a draft from before 0.32.0.
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        t.conflicts("alice").await,
        vec![conflict("alice", "plan.md", UnknownStart)]
    );
    assert!(
        status_of(&t, &scope("alice"), false)
            .await
            .get("conflicts_unreadable_reason")
            .is_none()
    );

    // Discard ends the draft without reading the record.
    std::fs::write(&file, b"{ not a record").unwrap();
    t.discard("alice", "plan.md").await;
    assert_eq!(t.conflicts("alice").await, vec![]);
}

/// The machine owner is on this machine, so her sentence names the record's
/// file she can remove.
#[tokio::test]
async fn the_machine_owner_is_told_which_record_to_remove() {
    let mut t = team(&[("plan.md", plan())]).await;
    t.eng
        .edit_engram_as(
            &{
                let mut p = edit_append("plan", ALICE_LINE);
                p.operation = "find_replace".to_string();
                p.find_text = Some("the first line of the plan".to_string());
                p
            },
            None,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    t.push(&[("plan.md", Some(plan_later()))]).await;
    let file = draft_start::record_file(&t.state(), "team", "owner", "plan.md").unwrap();
    std::fs::write(&file, b"{ not a record").unwrap();

    let domain = status_of(&t, &Scope::Unrestricted, false).await;
    assert_eq!(
        domain["conflicts_unreadable_reason"],
        format!(
            "The record of where your draft of plan.md started cannot be read. Discard that \
             draft, or remove {} so it is read as a draft from before 0.32.0.",
            file.display()
        ),
        "{domain}"
    );
}

/// C24, I-3: a colleague's draft file whose record cannot be read is his, not
/// the sharer's. Her direct share lands, and its receipt names neither his
/// path nor "your draft".
#[tokio::test]
async fn a_colleagues_broken_file_record_stays_out_of_a_direct_share() {
    let t = team_direct(&[("plan.md", plan()), (DECK, TEAM_DECK.to_vec())]).await;
    t.upload("bob", DECK, HER_DECK).await;
    let file = draft_start::file_record_file(&t.state(), "team", "bob", DECK).unwrap();
    std::fs::write(&file, b"").unwrap();
    t.append("alice", "plan", HER_WORD).await;

    let answer = t.share("alice", Some(&["plan.md"])).await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    let said = answer.to_string();
    assert!(!said.contains(DECK), "{said}");
    assert!(!said.contains("your draft"), "{said}");
    assert!(answer.get("fold_error").is_none(), "{answer}");
    assert_eq!(t.held_file("bob", DECK), Some(Some(HER_DECK.to_vec())));
}

/// [`a_broken_record_names_its_ways_out`] for a file beside the pages.
#[tokio::test]
async fn a_broken_file_record_names_its_ways_out() {
    let t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.upload("alice", DECK, HER_DECK).await;
    let file = draft_start::file_record_file(&t.state(), "team", "alice", DECK).unwrap();
    std::fs::write(&file, b"").unwrap();

    let err = t.eng.draft_conflicts("team", "alice").await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "The record of where your draft of {DECK} started cannot be read. Discard that \
             draft, or ask whoever runs this server to remove its record, so it is read as a \
             draft from before 0.32.0."
        )
    );
}

/// C24, I-2: the machine owner is on this machine, so her receipt names where
/// her kept file is.
#[tokio::test]
async fn the_owners_kept_file_is_named_in_her_receipt() {
    let t = team(&[(DECK, TEAM_DECK.to_vec())]).await;
    t.eng
        .attachment_write_as("team", DECK, HER_DECK.to_vec(), &Scope::Unrestricted)
        .await
        .unwrap();
    draft_start::clear_file(&t.state(), "team", "owner", DECK).unwrap();

    let receipt = t
        .eng
        .origin_resolve("team", DECK, Some("theirs"), None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(receipt["kept_copy"], true, "{receipt}");
    let kept = receipt["kept_file"]
        .as_str()
        .unwrap_or_else(|| panic!("{receipt}"));
    assert_eq!(std::fs::read(kept).unwrap(), HER_DECK);
}
