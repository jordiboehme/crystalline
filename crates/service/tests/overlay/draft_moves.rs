//! Every draft write keeps the record of where the draft started.
//!
//! A row with no record reads as "we cannot tell", so a write path that forgets
//! the record turns a new draft into an old one, and a path that writes the
//! wrong one reads a stale draft as in step, which is the silent overwrite the
//! record exists to stop. So every way a row gets written or taken away is here,
//! asserted on `draft_start::load`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GitHubConfig, GlobalConfig, OriginConfig, ReviewMode};
use crystalline_core::parse_engram;
use crystalline_index::{DomainKind, EngramRecord, FileStamp, Store, TursoStore};
use crystalline_remote::ops::DiscardTarget;
use crystalline_remote::state::{BaseStamp, OriginState, remove_base_file, write_base_file};
use crystalline_service::draft_start::{self, DraftStart};
use crystalline_service::engine::{DraftConflict, DraftConflictKind, FoldHook, FoldStep};
use crystalline_service::overlay_journal;
use crystalline_service::params::{
    DeleteParams, EditParams, MoveParams, ReadParams, SaveParams, SplitParams, WriteParams,
};
use crystalline_service::{Engine, FoldChoice, ReviewModeConfirm, Scope, ShareActor};
use tokio::sync::Mutex;

const MANIFEST: &str = "---\ntype: manifest\ntitle: team\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# team\n\n## Scope\n\n- The shared domain\n\n## When to Use\n\n- Route here for team work\n";
const PLAN: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Plan\n\n- [decision] the plan as the team has it #team\n";
/// What the team's plan says after a colleague's change.
const PLAN_LATER: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Plan\n\n- [decision] the plan after bob's change #team\n";
/// A team file with no `permalink:` line, whose address is its path's slug.
/// The team's MANIFEST when a share commits straight to the branch.
const MANIFEST_DIRECT: &str = "---\ntype: manifest\ntitle: team\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\nsharing: direct\n---\n\n# team\n\n## Scope\n\n- The shared domain\n\n## When to Use\n\n- Route here for team work\n";
/// A page with three lines far apart, so a change at each end merges.
const LONG: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Plan\n\n- [decision] the first line #team\n\n- [decision] the middle line, see [other](other.md) #team\n\n- [decision] the last line #team\n";
const BOB_LINE: &str = "bob's first line";
const HER_LINE: &str = "- [idea] alice's own line #team";
const LOOSE: &str = "---\ntype: engram\ntitle: Loose\ntags:\n  - team\nstatus: stable\n---\n\n# Loose\n\n- [fact] no permalink line here #team\n";
const NOTE: &str =
    "---\ntitle: Note\ntype: note\nstatus: draft\npermalink: note\n---\n\nteam body\n";
const RICH: &str = "---\ntype: engram\ntitle: Rich\npermalink: rich\ntags:\n  - team\nstatus: stable\n---\n\n# Rich\n\n- [decision] the first thing #team\n- [decision] the second thing #team\n- [idea] the third thing #team\n- [idea] the fourth thing #team\n";

/// A draft nobody else holds, answering to an address no file has.
fn own_text(address: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {address}\npermalink: {address}\nstatus: draft\n---\n\n{body}\n"
    )
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    engine: Arc<Engine>,
    store: Arc<Mutex<dyn Store>>,
    state: PathBuf,
    origins: PathBuf,
    mock: Option<Arc<crate::support::MockProvider>>,
}

/// A reviewing team domain with an origin, whose base snapshot says what the
/// folder says: the MANIFEST and `files`.
async fn fixture(files: &[(&str, &str)]) -> Fixture {
    build(files, true, MANIFEST).await
}

/// [`fixture`] with `sharing: direct`, so a share is a commit.
async fn fixture_direct(files: &[(&str, &str)]) -> Fixture {
    build(files, true, MANIFEST_DIRECT).await
}

/// The same domain with no origin and so no base snapshot at all.
async fn fixture_with_no_snapshot(files: &[(&str, &str)]) -> Fixture {
    build(files, false, MANIFEST).await
}

async fn build(files: &[(&str, &str)], origin: bool, manifest: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("team");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), manifest).unwrap();
    for (name, text) in files {
        let file = dir.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    let mut entry = DomainEntry::file(dir);
    entry.review = Some(ReviewMode::Overlay);
    if origin {
        entry.origin = Some(OriginConfig {
            repo: "acme/team".to_string(),
            path: None,
            branch: Some("main".to_string()),
            poll_secs: None,
        });
        cfg.github = Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        });
    }
    cfg.domains.insert("team".to_string(), entry);
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let state = root.join("state");
    let origins = root.join("origins");
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let mut engine = Engine::new(store.clone(), cfg, None, Some(config_path))
        .with_state_dir(state.clone())
        .with_origins_dir(origins.clone());
    let mut mock = None;
    if origin {
        let provider = Arc::new(crate::support::MockProvider::new());
        let mut held = BTreeMap::from([("MANIFEST.md".to_string(), manifest.as_bytes().to_vec())]);
        for (name, text) in files {
            held.insert((*name).to_string(), text.as_bytes().to_vec());
        }
        let commit = provider.add_commit(held);
        provider.set_branch("main", &commit);
        engine = engine.with_origin_provider(provider.clone());
        mock = Some(provider);
        let mut snapshot = OriginState::new("acme/team", "main");
        snapshot.base_commit = commit;
        let team_dir = origins.join("team");
        std::fs::create_dir_all(&team_dir).unwrap();
        snapshot.save(&team_dir).unwrap();
    }
    let engine = Arc::new(engine);
    engine.sync(None).await.unwrap();
    let f = Fixture {
        _tmp: tmp,
        root,
        engine,
        store,
        state,
        origins,
        mock,
    };
    if origin {
        f.team_has("MANIFEST.md", manifest.as_bytes());
        for (name, text) in files {
            f.team_has(name, text.as_bytes());
        }
    }
    f
}

fn alice() -> Scope {
    Scope::User {
        account: "alice".to_string(),
        admin: false,
    }
}

impl Fixture {
    fn snapshot_dir(&self) -> PathBuf {
        self.origins.join("team")
    }

    /// The team's file at `path` now holds `bytes`, in the folder and in the base
    /// snapshot, as after a pull. The index is not told, as a sync would.
    fn team_has(&self, path: &str, bytes: &[u8]) {
        std::fs::write(self.root.join("team").join(path), bytes).unwrap();
        self.snapshot_ahead(path, bytes);
    }

    /// The base snapshot holds `bytes` at `path` and the folder does not follow:
    /// the moment in a direct commit before the folder is written, and the state a
    /// failed folder write leaves for good.
    fn snapshot_ahead(&self, path: &str, bytes: &[u8]) {
        let dir = self.snapshot_dir();
        let mut state = OriginState::load(&dir).unwrap().unwrap();
        state.files.insert(
            path.to_string(),
            BaseStamp {
                sha256: crate::support::sha256_hex(bytes),
                size: bytes.len() as u64,
            },
        );
        write_base_file(&dir, path, bytes).unwrap();
        state.save(&dir).unwrap();
    }

    fn start(&self, path: &str) -> Option<DraftStart> {
        draft_start::load(&self.state, "team", "alice", path).unwrap()
    }

    async fn domain_id(&self, store: &dyn Store) -> crystalline_index::DomainId {
        store
            .upsert_domain(
                "team",
                Some(&self.root.join("team").to_string_lossy()),
                DomainKind::File,
            )
            .await
            .unwrap()
    }

    /// A draft from before the record existed: a row and its mirror, no record.
    async fn old_draft(&self, path: &str, text: &str) {
        let mut record = EngramRecord::from_engram(
            &parse_engram(text).unwrap(),
            path,
            FileStamp {
                mtime: 0,
                size: text.len() as u64,
                sha256: "0".repeat(64),
            },
        );
        record.content = text.to_string();
        let store = self.store.lock().await;
        let id = self.domain_id(&*store).await;
        store.upsert_overlay(id, "alice", &record).await.unwrap();
        overlay_journal::journal_write(&self.state, "team", "alice", path, text).unwrap();
    }

    /// The exact paths alice holds a row at, with whether each is a tombstone.
    async fn rows(&self) -> Vec<(String, bool)> {
        let store = self.store.lock().await;
        let id = self.domain_id(&*store).await;
        let mut rows: Vec<(String, bool)> = store
            .overlay_entries(id, "alice")
            .await
            .unwrap()
            .into_iter()
            .map(|entry| (entry.path, entry.tombstone))
            .collect();
        rows.sort();
        rows
    }

    /// Alice's first draft of the team's plan.
    async fn draft_plan(&self) {
        let mut over = write_params("Plan", "her body");
        over.overwrite = true;
        self.engine
            .write_engram_as(&over, None, &alice())
            .await
            .unwrap();
    }

    async fn delete(&self, identifier: &str) {
        self.engine
            .delete_engram_as(
                &DeleteParams {
                    identifier: identifier.to_string(),
                    domain: "team".to_string(),
                    expected_checksum: None,
                },
                None,
                &alice(),
            )
            .await
            .unwrap();
    }

    async fn moved(&self, identifier: &str, destination: &str) {
        self.engine
            .move_engram(&draft_move(identifier, destination), &alice())
            .await
            .unwrap();
    }

    async fn reads(&self, identifier: &str) -> String {
        self.engine
            .read_engram(&read(identifier), &alice())
            .await
            .unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Stand a folder where the record of `path` has to go, so writing it fails
    /// however often it is tried. Portable, as a mode bit is not.
    fn block_the_record(&self, path: &str) -> PathBuf {
        let file = draft_start::record_file(&self.state, "team", "alice", path).unwrap();
        std::fs::create_dir_all(&file).unwrap();
        file
    }

    /// The team moves the plan to `to` in one commit, and this machine pulls.
    async fn team_renames_plan_and_pull(&self, to: &str) {
        let mock = self.mock.as_ref().unwrap();
        let commit = mock.add_commit(BTreeMap::from([
            ("MANIFEST.md".to_string(), MANIFEST.as_bytes().to_vec()),
            (to.to_string(), PLAN.as_bytes().to_vec()),
        ]));
        mock.set_branch("main", &commit);
        self.engine
            .origin_update(Some("team"), &Scope::Unrestricted)
            .await
            .unwrap();
    }

    /// The forge's `main` as it stands.
    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        let mock = self.mock.as_ref().unwrap();
        let head = mock.branch_commit("main").unwrap();
        mock.commit_paths(&head)
            .into_iter()
            .map(|path| {
                let bytes = mock.commit_file(&head, &path).unwrap();
                (path, bytes)
            })
            .collect()
    }

    /// What the forge's `main` holds at `path`.
    fn forge_file(&self, path: &str) -> Option<String> {
        self.tree()
            .remove(path)
            .map(|bytes| String::from_utf8(bytes).unwrap())
    }

    /// A colleague commits these changes and this machine pulls them.
    async fn push(&self, changes: &[(&str, Option<String>)]) {
        let mut files = self.tree();
        for (path, text) in changes {
            match text {
                Some(text) => files.insert((*path).to_string(), text.clone().into_bytes()),
                None => files.remove(*path),
            };
        }
        let mock = self.mock.as_ref().unwrap();
        let commit = mock.add_commit(files);
        mock.set_branch("main", &commit);
        self.engine
            .origin_update(Some("team"), &Scope::Unrestricted)
            .await
            .unwrap();
    }

    /// Alice shares everything she holds.
    async fn share(&self) -> serde_json::Value {
        self.engine
            .origin_share(
                "team",
                None,
                None,
                None,
                None,
                ShareActor::Account("alice".to_string()),
            )
            .await
            .unwrap()
    }

    async fn conflicts(&self) -> Vec<DraftConflict> {
        self.engine.draft_conflicts("team", "alice").await.unwrap()
    }

    /// The text of alice's row at `path`.
    async fn row(&self, path: &str) -> String {
        let store = self.store.lock().await;
        let id = self.domain_id(&*store).await;
        store
            .overlay_entries(id, "alice")
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.path == path)
            .unwrap_or_else(|| panic!("alice holds no row at {path}"))
            .content
    }

    async fn append(&self, identifier: &str, line: &str) {
        self.engine
            .edit_engram_as(&edit_append(identifier, line), None, &alice())
            .await
            .unwrap();
    }

    async fn replace(&self, identifier: &str, find: &str, with: &str) {
        let mut p = edit_append(identifier, with);
        p.operation = "find_replace".to_string();
        p.find_text = Some(find.to_string());
        self.engine
            .edit_engram_as(&p, None, &alice())
            .await
            .unwrap();
    }
}

fn write_params(title: &str, content: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: "team".to_string(),
        title: title.to_string(),
        content: Some(content.to_string()),
        folder: None,
        engram_type: None,
        tags: vec!["team".to_string()],
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
        expected_checksum: None,
    }
}

fn read(identifier: &str) -> ReadParams {
    ReadParams {
        identifier: identifier.to_string(),
        domain: Some("team".to_string()),
        share_link: None,
        include_images: None,
    }
}

fn draft_move(identifier: &str, destination: &str) -> MoveParams {
    MoveParams {
        identifier: identifier.to_string(),
        domain: "team".to_string(),
        destination: destination.to_string(),
        destination_domain: None,
        permalink: None,
        update_links: None,
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

/// Whether `dir`'s filesystem folds letter case.
fn folds_case(dir: &Path) -> bool {
    let probe = dir.join("Probe");
    std::fs::create_dir(&probe).unwrap();
    let folds = dir.join("probe").exists();
    std::fs::remove_dir(&probe).unwrap();
    folds
}

/// The record a path has when it stands where it started, on the team's text.
fn in_step(path: &str, text: &str) -> DraftStart {
    DraftStart {
        path: path.to_string(),
        from: Some(path.to_string()),
        start: Some(text.to_string()),
        start_sha256: None,
        shared: None,
        merged_at: None,
        merging: None,
    }
}

#[tokio::test]
async fn a_new_draft_records_the_teams_text_and_a_second_write_does_not_move_it() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));

    // The team moves on, and she writes again: where the draft started did not
    // change, and reading it from the snapshot again would hide bob's change.
    f.team_has("plan.md", PLAN_LATER.as_bytes());
    let mut again = write_params("Plan", "her second body");
    again.overwrite = true;
    f.engine
        .write_engram_as(&again, None, &alice())
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));
}

/// The start is what the verb read, as LF, whatever line ending the team's file
/// has.
#[tokio::test]
async fn a_new_draft_records_what_she_read_as_lf() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.team_has("plan.md", PLAN_LATER.replace('\n', "\r\n").as_bytes());
    f.draft_plan().await;
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN_LATER)));

    // A writer with no read of its own takes the snapshot's text, also as LF.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.team_has("plan.md", PLAN_LATER.replace('\n', "\r\n").as_bytes());
    f.engine
        .restore_engram(
            "team",
            "plan.md",
            &PLAN.replace("as the team", "restored"),
            &alice(),
        )
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN_LATER)));
}

/// A pull lands while a first edit stands between its read and its write. Her row
/// does not hold bob's line, so a start of bob's text would read as in step and
/// her share would overwrite him. If the pull deleted the file, a start of nothing
/// would read as a new file and undo his delete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pull_during_a_first_edit_leaves_the_start_on_what_she_read() {
    for deletes in [false, true] {
        let f = fixture(&[("plan.md", PLAN)]).await;
        f.engine.hold_next_draft_edit(800);
        let edit = {
            let engine = f.engine.clone();
            tokio::spawn(async move {
                engine
                    .edit_engram_as(&edit_append("plan", "- [fact] hers #team"), None, &alice())
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let mut files = BTreeMap::from([("MANIFEST.md".to_string(), MANIFEST.as_bytes().to_vec())]);
        if !deletes {
            files.insert("plan.md".to_string(), PLAN_LATER.as_bytes().to_vec());
        }
        let mock = f.mock.as_ref().unwrap();
        let commit = mock.add_commit(files);
        mock.set_branch("main", &commit);
        f.engine
            .origin_update(Some("team"), &Scope::Unrestricted)
            .await
            .unwrap();
        let held = OriginState::load(&f.snapshot_dir()).unwrap().unwrap();
        assert_eq!(
            held.files.contains_key("plan.md"),
            !deletes,
            "the pull landed"
        );

        edit.await.unwrap().unwrap();

        assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
        assert_eq!(
            f.start("plan.md"),
            Some(in_step("plan.md", PLAN)),
            "deletes: {deletes}"
        );
    }
}

/// The snapshot stands ahead of the folder, so what she sees is older than what
/// the snapshot says. Each verb records what it read, and never bob's text, which
/// her row does not hold.
#[tokio::test]
async fn a_snapshot_ahead_of_the_folder_does_not_become_her_start() {
    let ahead = async || {
        let f = fixture(&[("plan.md", PLAN)]).await;
        f.snapshot_ahead("plan.md", PLAN_LATER.as_bytes());
        f
    };

    let f = ahead().await;
    f.engine
        .edit_engram_as(&edit_append("plan", "- [fact] hers #team"), None, &alice())
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)), "edit");

    let f = ahead().await;
    f.draft_plan().await;
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)), "write");

    let f = ahead().await;
    let mut guarded = write_params("Plan", "her body");
    guarded.overwrite = true;
    guarded.expected_checksum = Some(crate::support::sha256_hex(PLAN.as_bytes()));
    f.engine
        .write_engram_as(&guarded, None, &alice())
        .await
        .unwrap();
    assert_eq!(
        f.start("plan.md"),
        Some(in_step("plan.md", PLAN)),
        "write with a checksum"
    );

    let f = ahead().await;
    f.engine
        .save_engram(
            &SaveParams {
                domain: "team".to_string(),
                identifier: "plan".to_string(),
                content: PLAN.replace("as the team", "saved by alice"),
                expected_checksum: crate::support::sha256_hex(PLAN.as_bytes()),
            },
            &alice(),
        )
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)), "save");

    let f = ahead().await;
    f.delete("plan").await;
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)), "delete");

    let f = ahead().await;
    f.moved("plan", "renamed.md").await;
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)), "move");
    let moved = f.start("renamed.md").unwrap();
    assert_eq!(moved.from.as_deref(), Some("plan.md"));
    assert_eq!(moved.start.as_deref(), Some(PLAN), "move");

    let f = ahead().await;
    let mut asked = draft_move("plan", "plan.md");
    asked.permalink = Some("a-new-address".to_string());
    f.engine.move_engram(&asked, &alice()).await.unwrap();
    assert_eq!(
        f.start("plan.md"),
        Some(in_step("plan.md", PLAN)),
        "permalink-only move"
    );
}

/// The snapshot holds a file the folder does not have yet, and she creates one at
/// that path. She read nothing there, so she did not edit his file.
#[tokio::test]
async fn a_new_file_at_a_path_only_the_snapshot_holds_has_no_start() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.snapshot_ahead("fresh.md", PLAN_LATER.as_bytes());
    f.engine
        .write_engram_as(&write_params("Fresh", "only hers"), None, &alice())
        .await
        .unwrap();
    let record = f.start("fresh.md").unwrap();
    assert_eq!((record.from, record.start), (None, None));
}

/// A team file that cannot be read can still be replaced: she read nothing there,
/// and the write is not refused for it.
#[tokio::test]
async fn an_overwrite_of_an_unreadable_team_file_still_lands() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    std::fs::write(f.root.join("team/plan.md"), b"\xff\xfe not text").unwrap();
    f.draft_plan().await;
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    let record = f.start("plan.md").unwrap();
    assert_eq!((record.from, record.start), (None, None));
}

/// Her draft moved to `b.md`, then the team added its own `b.md`, and she moves on.
/// The deletion of the team's `b.md` is of a file she never saw. A record that
/// still said `from: plan.md` would read as in step, and her share would delete it
/// with no conflict.
#[tokio::test]
async fn a_tombstone_over_a_draft_from_elsewhere_does_not_read_as_in_step() {
    let team_b = own_text("team-b", "the team's own b");
    let moved_onto_by_the_team = async || {
        let f = fixture(&[("plan.md", PLAN)]).await;
        f.draft_plan().await;
        f.moved("plan", "b.md").await;
        f.team_has("b.md", team_b.as_bytes());
        f.engine.sync(None).await.unwrap();
        f
    };
    let no_known_start = DraftStart {
        path: "b.md".to_string(),
        from: Some("b.md".to_string()),
        start: None,
        start_sha256: None,
        shared: None,
        merged_at: None,
        merging: None,
    };

    let f = moved_onto_by_the_team().await;
    f.moved("plan", "c.md").await;
    assert_eq!(f.start("b.md"), Some(no_known_start.clone()), "move");
    let draft = f.start("c.md").unwrap();
    assert_eq!(
        draft.from.as_deref(),
        Some("plan.md"),
        "the draft keeps its own"
    );
    assert_eq!(draft.start.as_deref(), Some(PLAN));
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));

    let f = moved_onto_by_the_team().await;
    f.delete("plan").await;
    assert_eq!(f.start("b.md"), Some(no_known_start.clone()), "delete");

    // The same for a new file of hers, whose `from` is nothing.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .write_engram_as(&write_params("B", "only hers"), None, &alice())
        .await
        .unwrap();
    f.team_has("b.md", team_b.as_bytes());
    f.engine.sync(None).await.unwrap();
    f.delete("b").await;
    assert_eq!(
        f.start("b.md"),
        Some(no_known_start.clone()),
        "her new file"
    );

    // A move that fails puts the record back as it was.
    let f = moved_onto_by_the_team().await;
    let before = f.start("b.md").unwrap();
    assert_eq!(before.from.as_deref(), Some("plan.md"));
    f.block_the_record("c.md");
    f.engine
        .move_engram(&draft_move("plan", "c.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(f.start("b.md"), Some(before), "rolled back");

    // A deletion whose row the store refuses leaves the record as it was: her
    // other draft holds the column the tombstone's row needs.
    let f = moved_onto_by_the_team().await;
    let before = f.start("b.md").unwrap();
    f.engine
        .restore_engram(
            "team",
            "other.md",
            &own_text("b.md", "an address that is this path"),
            &alice(),
        )
        .await
        .unwrap();
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &alice(),
        )
        .await
        .unwrap_err();
    assert_eq!(f.start("b.md"), Some(before), "the row did not land");
}

/// C1: asking a held row's record whether it names this path can meet a broken
/// one, and the deletion refuses with nothing changed. A discard is the way out.
#[tokio::test]
async fn a_delete_of_a_draft_with_a_broken_record_refuses_and_changes_nothing() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    let file = draft_start::record_file(&f.state, "team", "alice", "plan.md").unwrap();
    std::fs::write(&file, b"{ not json").unwrap();
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &alice(),
        )
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(std::fs::read(&file).unwrap(), b"{ not json");

    f.engine
        .discard_local_changes(
            "team",
            &[DiscardTarget {
                path: "plan.md".to_string(),
                sha256: None,
            }],
            &ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("plan.md"), None);
}

/// A restore writes the row like every other writer of this draft, under its lock,
/// or a first edit and a restore could both find no row and the last record win.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_waits_for_the_draft_lock() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    let mirror = f
        .state
        .join("overlays")
        .join("team")
        .join("alice")
        .join("fresh.md");
    let lock = f.engine.file_write_lock(&mirror);
    let held = lock.lock().await;
    let mut restoring = {
        let engine = f.engine.clone();
        tokio::spawn(async move {
            engine
                .restore_engram("team", "fresh.md", &own_text("fresh", "put back"), &alice())
                .await
                .map(|_| ())
        })
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), &mut restoring)
            .await
            .is_err(),
        "the restore waits for the lock"
    );
    assert_eq!(f.rows().await, vec![]);
    drop(held);
    restoring.await.unwrap().unwrap();
    assert_eq!(f.rows().await, vec![("fresh.md".to_string(), false)]);
}

#[tokio::test]
async fn a_new_file_records_no_start() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .write_engram_as(&write_params("Fresh", "only hers"), None, &alice())
        .await
        .unwrap();
    assert_eq!(
        f.start("fresh.md"),
        Some(DraftStart {
            path: "fresh.md".to_string(),
            from: None,
            start: None,
            start_sha256: None,
            shared: None,
            merged_at: None,
            merging: None,
        }),
        "a record that says new, which no record at all could not"
    );
}

/// `read_base_file("Note.md")` answers with `note.md`'s bytes on a disk that
/// folds case, so only asking for a real key gives one answer on both disks.
#[tokio::test]
async fn a_new_spelling_of_a_team_file_is_a_new_file_on_either_disk() {
    let f = fixture(&[("note.md", NOTE)]).await;
    eprintln!(
        "the temp folder folds case: {}",
        folds_case(f.root.as_path())
    );
    f.engine
        .restore_engram(
            "team",
            "Note.md",
            &own_text("another-address", "another file"),
            &alice(),
        )
        .await
        .unwrap();
    let record = f.start("Note.md").unwrap();
    assert_eq!(record.from, None);
    assert_eq!(record.start, None);
}

#[tokio::test]
async fn a_delete_records_the_teams_text_even_after_a_draft_of_it() {
    // With no draft of it.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.delete("plan").await;
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), true)]);
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));

    // After a draft of it, with the team moved on since: the deletion stands on
    // what the draft started from, not on what the team says today.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    f.team_has("plan.md", PLAN_LATER.as_bytes());
    f.delete("plan").await;
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), true)]);
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));
}

/// A file in the folder the snapshot does not hold, which nobody has shared: its
/// deletion still names its own path, and says the team has no text there. A move
/// of it carries that, so the draft knows where it came from.
#[tokio::test]
async fn a_tombstone_names_its_own_path_when_the_snapshot_does_not_hold_it() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    std::fs::write(
        f.root.join("team/stray.md"),
        own_text("stray", "not shared"),
    )
    .unwrap();
    f.engine.sync(None).await.unwrap();
    let held_here = |path: &str, from: &str| DraftStart {
        path: path.to_string(),
        from: Some(from.to_string()),
        start: None,
        start_sha256: None,
        shared: None,
        merged_at: None,
        merging: None,
    };

    f.delete("stray").await;
    assert_eq!(f.start("stray.md"), Some(held_here("stray.md", "stray.md")));

    let f = fixture(&[("plan.md", PLAN)]).await;
    std::fs::write(
        f.root.join("team/stray.md"),
        own_text("stray", "not shared"),
    )
    .unwrap();
    f.engine.sync(None).await.unwrap();
    f.moved("stray", "kept.md").await;
    assert_eq!(f.start("stray.md"), Some(held_here("stray.md", "stray.md")));
    assert_eq!(f.start("kept.md"), Some(held_here("kept.md", "stray.md")));
}

/// Recording today's text over an old draft's deletion would make a stale
/// delete read as in step.
#[tokio::test]
async fn a_delete_of_an_old_draft_writes_no_record() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.old_draft("plan.md", &PLAN.replace("as the team", "as alice"))
        .await;
    f.delete("plan").await;
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), true)]);
    assert_eq!(f.start("plan.md"), None);
}

/// A draft of a path no file holds ends with its delete, and so does its record.
#[tokio::test]
async fn a_delete_of_her_own_new_file_takes_the_record() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .write_engram_as(&write_params("Fresh", "only hers"), None, &alice())
        .await
        .unwrap();
    assert!(f.start("fresh.md").is_some());
    f.delete("fresh").await;
    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("fresh.md"), None);
}

#[tokio::test]
async fn a_move_carries_the_start_at_any_permalink() {
    // (source, its address, its text, the destination, a new named permalink)
    let shapes: [(&str, &str, &str, &str, Option<&str>); 3] = [
        ("plan.md", "plan", PLAN, "renamed.md", None),
        ("plan.md", "plan", PLAN, "renamed.md", Some("a-new-address")),
        ("loose.md", "loose", LOOSE, "elsewhere/tidy.md", None),
    ];
    for (src, address, team_text, dest, permalink) in shapes {
        for with_a_draft in [false, true] {
            let f = fixture(&[(src, team_text)]).await;
            if with_a_draft {
                f.engine
                    .edit_engram_as(&edit_append(address, "- [fact] hers #team"), None, &alice())
                    .await
                    .unwrap();
            }
            let mut asked = draft_move(address, dest);
            asked.permalink = permalink.map(str::to_string);
            f.engine.move_engram(&asked, &alice()).await.unwrap();

            let at_dest = f.start(dest).expect("the draft carries a record");
            assert_eq!(at_dest.path, dest, "{dest}, draft {with_a_draft}");
            assert_eq!(
                at_dest.from.as_deref(),
                Some(src),
                "{dest}, draft {with_a_draft}"
            );
            assert_eq!(
                at_dest.start.as_deref(),
                Some(team_text),
                "{dest}, draft {with_a_draft}"
            );
            let tombstone = f.start(src).expect("the tombstone carries its own");
            assert_eq!(tombstone, in_step(src, team_text), "draft {with_a_draft}");
        }
    }
}

/// The team moved on before she moved: the draft she moved is still based on
/// what it started from, and a fresh look at the snapshot would hide bob's edit.
#[tokio::test]
async fn a_move_of_a_draft_keeps_the_start_it_had_and_not_todays_text() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    f.team_has("plan.md", PLAN_LATER.as_bytes());
    f.moved("plan", "renamed.md").await;
    assert_eq!(f.start("renamed.md").unwrap().start.as_deref(), Some(PLAN));
    assert_eq!(f.start("plan.md").unwrap().start.as_deref(), Some(PLAN));
}

/// A new file stays a new file wherever it goes: its `from` is not its old path,
/// which never was a team path, and no tombstone is left behind.
#[tokio::test]
async fn a_move_of_her_own_new_file_stays_new() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .write_engram_as(&write_params("Fresh", "only hers"), None, &alice())
        .await
        .unwrap();
    f.moved("fresh", "later.md").await;
    assert_eq!(f.rows().await, vec![("later.md".to_string(), false)]);
    assert_eq!(f.start("fresh.md"), None);
    let record = f.start("later.md").unwrap();
    assert_eq!(record.from, None);
    assert_eq!(record.start, None);
}

/// The permalink-only rename: one write at the path it has.
#[tokio::test]
async fn a_permalink_only_move_starts_or_keeps_the_record_at_its_own_path() {
    // First touch of the team's file.
    let f = fixture(&[("plan.md", PLAN)]).await;
    let mut asked = draft_move("plan", "plan.md");
    asked.permalink = Some("a-new-address".to_string());
    f.engine.move_engram(&asked, &alice()).await.unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));

    // Over a draft, with the team moved on: left alone.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    f.team_has("plan.md", PLAN_LATER.as_bytes());
    let mut asked = draft_move("plan", "plan.md");
    asked.permalink = Some("a-new-address".to_string());
    f.engine.move_engram(&asked, &alice()).await.unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));
}

#[tokio::test]
async fn a_second_move_hands_the_record_on_and_leaves_none_behind() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.moved("plan", "first.md").await;
    f.moved("plan", "second.md").await;

    assert_eq!(
        f.start("first.md"),
        None,
        "nothing stays at the middle path"
    );
    let record = f.start("second.md").unwrap();
    assert_eq!(record.path, "second.md");
    assert_eq!(record.from.as_deref(), Some("plan.md"));
    assert_eq!(record.start.as_deref(), Some(PLAN));
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));
}

/// A move back by case keeps the start; the plain move back is
/// `a_plain_move_back_lands_on_her_own_deletion_and_survives_a_rebuild`.
#[tokio::test]
async fn a_move_back_keeps_the_start() {
    let f = fixture(&[("note.md", NOTE)]).await;
    f.moved("note", "Note.md").await;
    f.moved("note", "note.md").await;

    assert_eq!(f.rows().await, vec![("note.md".to_string(), false)]);
    assert_eq!(f.start("note.md"), Some(in_step("note.md", NOTE)));
    assert_eq!(f.start("Note.md"), None, "nothing stays at the other end");
}

/// A move onto a team path that is not where the draft started is refused, even
/// over a deletion of hers there, and the refusal costs no record. Only the way
/// back is free (`a_plain_move_back_lands_on_her_own_deletion_and_survives_a_rebuild`).
#[tokio::test]
async fn a_refused_move_onto_another_deleted_team_file_changes_no_record() {
    let f = fixture(&[("plan.md", PLAN), ("loose.md", LOOSE)]).await;
    f.moved("plan", "renamed.md").await;
    f.delete("loose").await;
    let before = (
        f.start("plan.md"),
        f.start("renamed.md"),
        f.start("loose.md"),
    );
    f.engine
        .move_engram(&draft_move("plan", "loose.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(
        (
            f.start("plan.md"),
            f.start("renamed.md"),
            f.start("loose.md")
        ),
        before
    );
}

#[tokio::test]
async fn a_case_move_keeps_two_records_on_either_disk() {
    let f = fixture(&[("note.md", NOTE)]).await;
    eprintln!("the temp folder folds case: {}", folds_case(&f.root));
    f.moved("note", "Note.md").await;

    assert_eq!(f.start("note.md"), Some(in_step("note.md", NOTE)));
    let draft = f.start("Note.md").unwrap();
    assert_eq!(draft.path, "Note.md");
    assert_eq!(draft.from.as_deref(), Some("note.md"));
    assert_eq!(draft.start.as_deref(), Some(NOTE));
}

#[tokio::test]
async fn a_move_of_an_old_draft_writes_no_record() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.old_draft("plan.md", &PLAN.replace("as the team", "as alice"))
        .await;
    // A record left at the destination by an earlier failure is not hers to
    // inherit: it would read a draft of unknown start as known.
    draft_start::save(
        &f.state,
        "team",
        "alice",
        &in_step("renamed.md", "a text nobody started from\n"),
    )
    .unwrap();

    f.moved("plan", "renamed.md").await;

    assert_eq!(
        f.rows().await,
        vec![
            ("plan.md".to_string(), true),
            ("renamed.md".to_string(), false)
        ]
    );
    assert_eq!(f.start("renamed.md"), None);
    assert_eq!(f.start("plan.md"), None, "the tombstone half is as unknown");
}

/// A record whose row went away is nobody's. It is written over, never read.
#[tokio::test]
async fn a_new_row_does_not_inherit_a_record_left_behind() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    let left = in_step("plan.md", "a text nobody started from\n");
    draft_start::save(&f.state, "team", "alice", &left).unwrap();
    f.draft_plan().await;
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));

    draft_start::save(&f.state, "team", "alice", &left).unwrap();
    f.engine
        .discard_local_changes(
            "team",
            &[DiscardTarget {
                path: "plan.md".to_string(),
                sha256: None,
            }],
            &ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    draft_start::save(&f.state, "team", "alice", &left).unwrap();
    f.delete("plan").await;
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));
}

#[tokio::test]
async fn the_record_survives_a_rebuild() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    f.moved("plan", "renamed.md").await;
    let before = (f.start("plan.md").unwrap(), f.start("renamed.md").unwrap());
    let config_path = f.root.join("config.yaml");
    let config: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let rebuilt = Arc::new(
        Engine::new(store, config, None, Some(config_path))
            .with_state_dir(f.state.clone())
            .with_origins_dir(f.origins.clone()),
    );
    rebuilt.sync(None).await.unwrap();

    let back = rebuilt
        .read_engram(&read("plan"), &alice())
        .await
        .expect("the row came back from the journal");
    assert_eq!(back["path"], "renamed.md", "{back}");
    assert_eq!(
        (f.start("plan.md").unwrap(), f.start("renamed.md").unwrap()),
        before
    );
}

#[tokio::test]
async fn a_discard_takes_the_record() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    assert!(f.start("plan.md").is_some());
    f.engine
        .discard_local_changes(
            "team",
            &[DiscardTarget {
                path: "plan.md".to_string(),
                sha256: None,
            }],
            &ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("plan.md"), None);
}

/// Taking the team's side ends the draft through the same drop. A merged text
/// keeps the record and moves its start to the team's text it was merged with.
#[tokio::test]
async fn a_resolve_for_theirs_takes_the_record_and_a_merged_one_moves_it() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    f.team_has("plan.md", PLAN_LATER.as_bytes());
    f.engine
        .origin_resolve(
            "team",
            "plan.md",
            None,
            Some(PLAN.replace("as the team", "merged").as_bytes()),
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN_LATER)));

    f.engine
        .origin_resolve(
            "team",
            "plan.md",
            Some("theirs"),
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("plan.md"), None);
}

/// Leaving review ends every draft through the same drop.
#[tokio::test]
async fn a_fold_takes_the_records() {
    let f = fixture(&[("plan.md", PLAN), ("note.md", NOTE)]).await;
    f.draft_plan().await;
    f.delete("note").await;
    assert!(f.start("plan.md").is_some() && f.start("note.md").is_some());
    f.engine
        .set_review_mode(
            "team",
            None,
            ReviewModeConfirm::Confirmed {
                folds: [("alice".to_string(), FoldChoice::Fold)]
                    .into_iter()
                    .collect(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), None);
    assert_eq!(f.start("note.md"), None);
}

/// Every verb that writes a row, on its first write of a team's file: each lands
/// the record, and none of them does it by a way of its own.
#[tokio::test]
async fn every_writer_of_a_new_draft_keeps_the_record() {
    let checksum = |text: &str| crate::support::sha256_hex(text.as_bytes());

    // MCP and REST `write_engram`.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)), "write");

    // MCP and REST `edit_engram`, and a room's source edit.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .edit_engram_as(&edit_append("plan", "- [fact] hers #team"), None, &alice())
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)), "edit");

    // Fluid's save, and through `save_into_overlay` a room's save.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .save_engram(
            &SaveParams {
                domain: "team".to_string(),
                identifier: "plan".to_string(),
                content: PLAN.replace("as the team", "saved by alice"),
                expected_checksum: checksum(PLAN),
            },
            &alice(),
        )
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)), "save");

    // A restore of an earlier version, a room's undo of its own save among them.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .restore_engram(
            "team",
            "plan.md",
            &PLAN.replace("as the team", "restored"),
            &alice(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.start("plan.md"),
        Some(in_step("plan.md", PLAN)),
        "restore"
    );

    // A split: the source is a first edit, the new engram is a new file.
    let f = fixture(&[("rich.md", RICH)]).await;
    let source = f.engine.read_engram(&read("rich"), &alice()).await.unwrap();
    let lines: Vec<usize> = source["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["line"].as_u64().unwrap() as usize)
        .take(1)
        .collect();
    f.engine
        .split_engram_as(
            &SplitParams {
                domain: "team".to_string(),
                identifier: "rich".to_string(),
                title: "The Split Out Part".to_string(),
                folder: None,
                observations: lines,
                sections: Vec::new(),
                expected_checksum: Some(source["checksum"].as_str().unwrap().to_string()),
            },
            None,
            &alice(),
        )
        .await
        .unwrap();
    assert_eq!(f.start("rich.md"), Some(in_step("rich.md", RICH)), "split");
    let split = f.start("the-split-out-part.md").expect("split out");
    assert_eq!((split.from, split.start), (None, None));
}

/// Nothing is known about a domain with no base snapshot, so no record claims
/// anything: `from: None` would say "a new file" about a file the folder holds.
#[tokio::test]
async fn a_domain_with_no_snapshot_records_nothing() {
    let f = fixture_with_no_snapshot(&[("plan.md", PLAN), ("note.md", NOTE)]).await;
    f.draft_plan().await;
    assert_eq!(f.start("plan.md"), None);
    f.moved("note", "moved.md").await;
    assert_eq!(f.start("moved.md"), None);
    assert_eq!(f.start("note.md"), None);
    f.delete("plan").await;
    assert_eq!(f.start("plan.md"), None);
    assert!(f.reads("note").await.contains("team body"));
}

/// A key with no readable copy cannot say what the team has. The write lands
/// with no record, which reads as "we cannot tell". Recording `start: None` would
/// read a file the team has as a new file, and a record left there is not hers.
#[tokio::test]
async fn a_key_with_no_readable_copy_writes_no_record_and_the_row_lands() {
    let left = in_step("plan.md", "a text nobody started from\n");
    for no_copy in [true, false] {
        let damaged = async || {
            let f = fixture(&[("plan.md", PLAN)]).await;
            if no_copy {
                remove_base_file(&f.snapshot_dir(), "plan.md").unwrap();
            } else {
                write_base_file(&f.snapshot_dir(), "plan.md", b"\xff\xfe not text").unwrap();
            }
            draft_start::save(&f.state, "team", "alice", &left).unwrap();
            f
        };

        let f = damaged().await;
        f.draft_plan().await;
        assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
        assert_eq!(f.start("plan.md"), None, "no copy: {no_copy}");

        let f = damaged().await;
        f.delete("plan").await;
        assert_eq!(f.rows().await, vec![("plan.md".to_string(), true)]);
        assert_eq!(f.start("plan.md"), None, "no copy: {no_copy}");
    }
}

/// A state file that does not parse is an error, as it is to a pull and a share,
/// so nothing can be told from it and the write refuses, row and all.
#[tokio::test]
async fn a_state_file_that_does_not_parse_refuses_a_new_draft_and_leaves_nothing() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    std::fs::write(f.snapshot_dir().join("state.json"), b"{ not json").unwrap();
    let mut over = write_params("Plan", "her body");
    over.overwrite = true;
    f.engine
        .write_engram_as(&over, None, &alice())
        .await
        .unwrap_err();
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &alice(),
        )
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("plan.md"), None);
}

/// C1: a broken record is an error and never a missing one. The move refuses
/// while nothing has changed, so no destination can read as an old draft.
#[tokio::test]
async fn a_move_of_a_draft_with_a_broken_record_refuses_and_changes_nothing() {
    // Her draft of a team file, and her own new file, where no tombstone is
    // written that would ask the record again.
    for own_new_file in [false, true] {
        let f = fixture(&[("plan.md", PLAN)]).await;
        let (path, address, words) = if own_new_file {
            f.engine
                .write_engram_as(&write_params("Fresh", "only hers"), None, &alice())
                .await
                .unwrap();
            ("fresh.md", "fresh", "only hers")
        } else {
            f.draft_plan().await;
            ("plan.md", "plan", "her body")
        };
        let file = draft_start::record_file(&f.state, "team", "alice", path).unwrap();
        std::fs::write(&file, b"{ not json").unwrap();

        f.engine
            .move_engram(&draft_move(address, "renamed.md"), &alice())
            .await
            .unwrap_err();

        assert_eq!(f.rows().await, vec![(path.to_string(), false)]);
        assert!(f.reads(address).await.contains(words));
        assert_eq!(std::fs::read(&file).unwrap(), b"{ not json");
        assert_eq!(f.start("renamed.md"), None);
    }
}

/// The row after the record failed: the row's address is taken by the column a
/// tombstone holds its path in, so the store refuses what the check let through.
/// The record saved for a row that never landed goes with it.
#[tokio::test]
async fn a_failed_row_write_leaves_no_record_behind() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.delete("plan").await;
    f.engine
        .restore_engram(
            "team",
            "other.md",
            &own_text("plan.md", "an address a tombstone's row holds"),
            &alice(),
        )
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), true)]);
    assert_eq!(f.start("other.md"), None);
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", PLAN)));
}

/// The same for a deletion: her draft elsewhere holds the column the tombstone's
/// row needs, so the store refuses it, and the record it saved goes with it.
#[tokio::test]
async fn a_failed_tombstone_row_leaves_no_record_behind() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .restore_engram(
            "team",
            "other.md",
            &own_text("plan.md", "an address that is the plan's path"),
            &alice(),
        )
        .await
        .unwrap();
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &alice(),
        )
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("other.md".to_string(), false)]);
    assert_eq!(f.start("plan.md"), None);
}

/// A destination that cannot be written, in each shape a source can have: the
/// source goes back to exactly what it held, the record included.
#[tokio::test]
async fn a_failed_move_destination_restores_the_source_record() {
    let edited = PLAN.replace("as the team", "as alice");

    // Her draft of a team file: the tombstone kept the record, and it stays.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    f.team_has("plan.md", PLAN_LATER.as_bytes());
    let before = f.start("plan.md").unwrap();
    f.block_the_record("renamed.md");
    f.engine
        .move_engram(&draft_move("plan", "renamed.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert!(f.reads("plan").await.contains("her body"));
    assert_eq!(f.start("plan.md"), Some(before), "her draft's start");

    // Her own new file: its row and its record were dropped, and both come back
    // as they were, even though the team has since added a file at that path.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .write_engram_as(&write_params("Fresh", "only hers"), None, &alice())
        .await
        .unwrap();
    let before = f.start("fresh.md").unwrap();
    f.team_has("fresh.md", PLAN_LATER.as_bytes());
    f.block_the_record("later.md");
    f.engine
        .move_engram(&draft_move("fresh", "later.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("fresh.md".to_string(), false)]);
    assert_eq!(f.start("fresh.md"), Some(before), "still a new file");

    // An old draft of a file only she has: back with no record, not a fresh one.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.old_draft("fresh.md", &own_text("fresh", "only hers"))
        .await;
    f.block_the_record("later.md");
    f.engine
        .move_engram(&draft_move("fresh", "later.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("fresh.md".to_string(), false)]);
    assert_eq!(f.start("fresh.md"), None, "still unknown");

    // An old draft of a team file: the same, over the tombstone.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.old_draft("plan.md", &edited).await;
    f.block_the_record("renamed.md");
    f.engine
        .move_engram(&draft_move("plan", "renamed.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(f.start("plan.md"), None, "still unknown");

    // The team's file with no draft: nothing of hers stands at all afterwards,
    // and so no record either.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.block_the_record("renamed.md");
    f.engine
        .move_engram(&draft_move("plan", "renamed.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("plan.md"), None, "the base shows through again");
}

/// The record cannot be put back on the way out of a failed move: her row and her
/// text come back all the same, and the record goes, which reads as "we cannot
/// tell". A wrong record would read as a known start.
#[tokio::test]
async fn a_record_that_cannot_be_put_back_costs_the_start_and_never_the_text() {
    // Her draft of a team file.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    f.block_the_record("renamed.md");
    f.engine.fail_next_start_restore();
    f.engine
        .move_engram(&draft_move("plan", "renamed.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert!(f.reads("plan").await.contains("her body"));
    assert_eq!(f.start("plan.md"), None);

    // Her own new file.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.engine
        .write_engram_as(&write_params("Fresh", "only hers"), None, &alice())
        .await
        .unwrap();
    f.block_the_record("later.md");
    f.engine.fail_next_start_restore();
    f.engine
        .move_engram(&draft_move("fresh", "later.md"), &alice())
        .await
        .unwrap_err();
    assert_eq!(f.rows().await, vec![("fresh.md".to_string(), false)]);
    assert!(f.reads("fresh").await.contains("only hers"));
    assert_eq!(f.start("fresh.md"), None);
}

/// A folder of records that refuses writes fails the save and the clear both, in
/// the rollback too. Her row and text still come back, and the record stands. Unix
/// only for the mode bit, and passed over where the mode does not bind, as for root.
#[cfg(unix)]
#[tokio::test]
async fn a_folder_of_records_that_refuses_writes_never_costs_the_text() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |dir: &Path, bits: u32| {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(bits)).unwrap();
    };
    for own_new_file in [false, true] {
        let f = fixture(&[("plan.md", PLAN)]).await;
        let (path, address, words) = if own_new_file {
            f.engine
                .write_engram_as(&write_params("Fresh", "only hers"), None, &alice())
                .await
                .unwrap();
            ("fresh.md", "fresh", "only hers")
        } else {
            f.draft_plan().await;
            ("plan.md", "plan", "her body")
        };
        let before = f.start(path);
        assert!(before.is_some());
        let file = draft_start::record_file(&f.state, "team", "alice", path).unwrap();
        let starts = file.parent().unwrap().to_path_buf();
        mode(&starts, 0o555);
        if std::fs::write(starts.join("probe"), b"").is_ok() {
            std::fs::remove_file(starts.join("probe")).unwrap();
            mode(&starts, 0o755);
            eprintln!("skipped: this user writes through the mode");
            return;
        }
        let moved = f
            .engine
            .move_engram(&draft_move(address, "later.md"), &alice())
            .await;
        let (rows, reads, record) = (f.rows().await, f.reads(address).await, f.start(path));
        mode(&starts, 0o755);
        moved.unwrap_err();
        assert_eq!(rows, vec![(path.to_string(), false)]);
        assert!(reads.contains(words));
        assert_eq!(record, before);
    }
}

#[tokio::test]
async fn a_failed_follow_of_a_team_rename_restores_the_source_record() {
    // Her draft, with a record.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    let before = f.start("plan.md").unwrap();
    f.block_the_record("plan-v2.md");
    f.team_renames_plan_and_pull("plan-v2.md").await;
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(f.start("plan.md"), Some(before));

    // An old draft: the snapshot no longer holds the path, and it comes back
    // with no record rather than one calling it a new file.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.old_draft("plan.md", &PLAN.replace("as the team", "as alice"))
        .await;
    f.block_the_record("plan-v2.md");
    f.team_renames_plan_and_pull("plan-v2.md").await;
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(f.start("plan.md"), None);
}

/// C1 on the pass's side: a broken record stops the follow before anything is
/// dropped, so the draft stands where it was and never reads as an old draft.
#[tokio::test]
async fn a_follow_of_a_draft_with_a_broken_record_leaves_it_where_it_was() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    let file = draft_start::record_file(&f.state, "team", "alice", "plan.md").unwrap();
    std::fs::write(&file, b"{ not json").unwrap();

    f.team_renames_plan_and_pull("plan-v2.md").await;

    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(std::fs::read(&file).unwrap(), b"{ not json");
    assert_eq!(f.start("plan-v2.md"), None);
}

/// The team moved the file, so the draft's team path moved with it, and what it
/// started from did not: a colleague's edit in the same pull is still a change.
#[tokio::test]
async fn a_followed_team_rename_points_the_record_at_the_new_path() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.draft_plan().await;
    f.team_renames_plan_and_pull("plan-v2.md").await;

    assert_eq!(f.rows().await, vec![("plan-v2.md".to_string(), false)]);
    assert_eq!(f.start("plan.md"), None);
    assert_eq!(
        f.start("plan-v2.md"),
        Some(DraftStart {
            path: "plan-v2.md".to_string(),
            from: Some("plan-v2.md".to_string()),
            start: Some(PLAN.to_string()),
            start_sha256: None,
            shared: None,
            merged_at: None,
            merging: None,
        })
    );

    // An old draft follows and stays unknown.
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.old_draft("plan.md", &PLAN.replace("as the team", "as alice"))
        .await;
    f.team_renames_plan_and_pull("plan-v2.md").await;
    assert_eq!(f.rows().await, vec![("plan-v2.md".to_string(), false)]);
    assert_eq!(f.start("plan-v2.md"), None);
}

// --- The pass merges a moved draft ---------------------------------------------

fn bobs(text: &str) -> String {
    text.replace("the first line", BOB_LINE)
}

/// What a moved draft's conflict is, at its new path.
fn moved_conflict(path: &str, from: &str, kind: DraftConflictKind) -> DraftConflict {
    DraftConflict {
        id: format!(
            "d{}",
            &crate::support::sha256_hex(format!("alice\0{path}").as_bytes())[..7]
        ),
        path: path.to_string(),
        kind,
        moved_from: Some(from.to_string()),
        file: false,
        other: None,
    }
}

#[tokio::test]
async fn an_unedited_rename_carries_only_the_move_and_the_peers_line() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.moved("plan", "renamed.md").await;
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;

    assert_eq!(
        f.rows().await,
        vec![
            ("plan.md".to_string(), true),
            ("renamed.md".to_string(), false)
        ]
    );
    assert_eq!(
        f.row("renamed.md").await,
        bobs(LONG),
        "his line and her move"
    );
    assert!(f.reads("plan").await.contains(BOB_LINE));
    let record = f.start("renamed.md").unwrap();
    assert_eq!(record.from.as_deref(), Some("plan.md"));
    assert_eq!(record.start.as_deref(), Some(bobs(LONG).as_str()));
    assert_eq!(record.merged_at, None, "none of her words changed");
    assert_eq!(f.start("plan.md"), Some(in_step("plan.md", &bobs(LONG))));
    assert_eq!(f.conflicts().await, vec![]);
}

#[tokio::test]
async fn an_unedited_folder_move_carries_the_peers_line_with_links_rebased() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.moved("plan", "sub/plan.md").await;
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;

    let row = f.row("sub/plan.md").await;
    assert!(row.contains(BOB_LINE), "{row}");
    assert!(row.contains("[other](../other.md)"), "{row}");
    assert!(!row.contains("(other.md)"), "{row}");
    assert_eq!(f.conflicts().await, vec![]);
}

#[tokio::test]
async fn an_unedited_case_move_keeps_the_peers_line_on_either_disk() {
    let f = fixture(&[("plan.md", LONG)]).await;
    eprintln!("the temp folder folds case: {}", folds_case(&f.root));
    f.moved("plan", "Plan.md").await;
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;

    assert_eq!(
        f.rows().await,
        vec![
            ("Plan.md".to_string(), false),
            ("plan.md".to_string(), true)
        ]
    );
    assert!(f.row("Plan.md").await.contains(BOB_LINE));
    assert_eq!(f.conflicts().await, vec![]);
}

#[tokio::test]
async fn an_unedited_case_move_shares_no_delete_on_either_disk() {
    let f = fixture_direct(&[("plan.md", LONG)]).await;
    eprintln!("the temp folder folds case: {}", folds_case(&f.root));
    f.moved("plan", "Plan.md").await;
    let answer = f.share().await;
    assert!(
        answer["deleted"].as_array().is_none_or(Vec::is_empty),
        "{answer}"
    );
    assert_eq!(f.forge_file("plan.md").as_deref(), Some(LONG), "{answer}");
    assert_eq!(f.share().await["outcome"], "nothing_to_share");

    // A colleague's edit first, then her share: his line is what the forge holds.
    let f = fixture_direct(&[("plan.md", LONG)]).await;
    f.moved("plan", "Plan.md").await;
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;
    let answer = f.share().await;
    assert!(
        answer["deleted"].as_array().is_none_or(Vec::is_empty),
        "{answer}"
    );
    let held = f.tree();
    assert!(
        held.values()
            .any(|bytes| String::from_utf8_lossy(bytes).contains(BOB_LINE)),
        "{answer}"
    );
}

/// Asserted per disk: on a disk that folds case her share lands her move, and on
/// one that does not, only her text travels and the spelling stays hers.
#[tokio::test]
async fn an_edited_case_move_ends_when_it_lands_on_either_disk() {
    let f = fixture_direct(&[("plan.md", LONG)]).await;
    let folds = folds_case(&f.root);
    eprintln!("the temp folder folds case: {folds}");
    f.append("plan", HER_LINE).await;
    f.moved("plan", "Plan.md").await;
    let answer = f.share().await;
    assert_eq!(answer["outcome"], "committed", "{answer}");

    if folds {
        // Her text stands at the spelling the team has, which on this disk is
        // her file: both halves end.
        assert_eq!(f.rows().await, vec![], "{answer}");
    } else {
        // A share does not carry the spelling, and on this disk the team's
        // `plan.md` is not her `Plan.md`: what stays is an unedited case move,
        // in step with the text she shared.
        assert_eq!(
            f.rows().await,
            vec![
                ("Plan.md".to_string(), false),
                ("plan.md".to_string(), true)
            ],
            "{answer}"
        );
        assert_eq!(
            f.start("Plan.md").unwrap().start,
            f.forge_file("plan.md"),
            "{answer}"
        );
    }
    assert_eq!(f.conflicts().await, vec![]);
    let held = f.tree();
    assert!(
        held.values()
            .any(|bytes| String::from_utf8_lossy(bytes).contains("alice's own line")),
        "{held:?}"
    );
    assert_eq!(f.share().await["outcome"], "nothing_to_share");
}

#[tokio::test]
async fn an_edited_move_merges_both_lines() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.append("plan", HER_LINE).await;
    f.moved("plan", "renamed.md").await;
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;

    let row = f.row("renamed.md").await;
    assert!(row.contains(BOB_LINE) && row.contains(HER_LINE), "{row}");
    let record = f.start("renamed.md").unwrap();
    assert_eq!(record.start.as_deref(), Some(bobs(LONG).as_str()));
    assert!(record.merged_at.is_some());
    assert_eq!(
        f.start("plan.md").unwrap().start.as_deref(),
        Some(bobs(LONG).as_str()),
        "the deletion half starts there too"
    );
    assert_eq!(f.conflicts().await, vec![]);
}

#[tokio::test]
async fn an_edited_move_on_the_same_line_is_a_conflict_at_the_new_path() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.replace("plan", "the first line", "alice's first line")
        .await;
    f.moved("plan", "renamed.md").await;
    let hers = f.row("renamed.md").await;
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;

    assert_eq!(f.row("renamed.md").await, hers);
    assert_eq!(
        f.conflicts().await,
        vec![moved_conflict(
            "renamed.md",
            "plan.md",
            DraftConflictKind::EditEdit
        )]
    );
}

/// N1, direct: her own share of an edited move ends both halves, and a later
/// colleague's edit of the new path is his, with nothing of hers to share.
#[tokio::test]
async fn an_edited_move_ends_when_it_lands_and_a_later_peer_edit_stays() {
    let f = fixture_direct(&[("plan.md", LONG)]).await;
    f.append("plan", HER_LINE).await;
    f.moved("plan", "renamed.md").await;
    let answer = f.share().await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.conflicts().await, vec![]);
    assert_eq!(answer["drafts_folded"], 2, "{answer}");

    let landed = f.forge_file("renamed.md").unwrap();
    f.push(&[("renamed.md", Some(bobs(&landed)))]).await;
    assert!(f.reads("plan").await.contains(BOB_LINE));
    assert_eq!(f.share().await["outcome"], "nothing_to_share");
    assert!(f.forge_file("renamed.md").unwrap().contains(BOB_LINE));
}

#[tokio::test]
async fn a_move_with_a_new_permalink_keeps_the_peers_edit() {
    let f = fixture(&[("plan.md", LONG)]).await;
    let mut asked = draft_move("plan", "renamed.md");
    asked.permalink = Some("renamed".to_string());
    f.engine.move_engram(&asked, &alice()).await.unwrap();
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;

    let row = f.row("renamed.md").await;
    assert!(row.contains(BOB_LINE), "{row}");
    assert!(row.contains("permalink: renamed"), "{row}");
    assert_eq!(f.conflicts().await, vec![]);
}

#[tokio::test]
async fn a_move_of_a_file_with_no_permalink_line_keeps_the_peers_edit() {
    let f = fixture(&[("loose.md", LOOSE)]).await;
    f.moved("loose", "elsewhere.md").await;
    let later = LOOSE.replace("no permalink line here", "bob's line about it");
    f.push(&[("loose.md", Some(later))]).await;

    let row = f.row("elsewhere.md").await;
    assert!(row.contains("bob's line about it"), "{row}");
    assert_eq!(f.conflicts().await, vec![]);
}

#[tokio::test]
async fn a_move_back_with_no_edit_after_a_peer_edit_ends_and_shows_the_peer() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.moved("plan", "renamed.md").await;
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;
    f.moved("plan", "plan.md").await;

    assert_eq!(f.conflicts().await, vec![]);
    assert!(f.reads("plan").await.contains(BOB_LINE));
    f.engine.converge_overlays("team").await.unwrap();
    assert_eq!(f.rows().await, vec![]);
    assert!(f.reads("plan").await.contains(BOB_LINE));
}

#[tokio::test]
async fn a_peer_delete_of_a_file_she_moved_stands_as_a_conflict() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.moved("plan", "renamed.md").await;
    f.push(&[("plan.md", None)]).await;

    assert_eq!(
        f.rows().await,
        vec![
            ("plan.md".to_string(), true),
            ("renamed.md".to_string(), false)
        ],
        "neither half is dropped"
    );
    assert_eq!(
        f.conflicts().await,
        vec![moved_conflict(
            "renamed.md",
            "plan.md",
            DraftConflictKind::EditDelete
        )]
    );
}

#[tokio::test]
async fn a_crlf_team_file_clears_her_landed_draft() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.append("plan", HER_LINE).await;
    let hers = f.row("plan.md").await;
    f.push(&[("plan.md", Some(hers.replace('\n', "\r\n")))])
        .await;

    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("plan.md"), None);
}

/// A plain move back onto the path she moved from: her own deletion there is no
/// obstacle (C3).
#[tokio::test]
async fn a_plain_move_back_lands_on_her_own_deletion_and_survives_a_rebuild() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.append("plan", HER_LINE).await;
    f.moved("plan", "renamed.md").await;
    f.moved("plan", "plan.md").await;

    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(f.start("renamed.md"), None);
    let record = f.start("plan.md").unwrap();
    assert_eq!(record.from.as_deref(), Some("plan.md"));
    assert_eq!(record.start.as_deref(), Some(LONG));

    // The mirror holds the draft and no tombstone, so a rebuild brings the draft.
    let journal = overlay_journal::journal_entries(&f.state, "team");
    assert!(!journal.unreadable);
    let mine: Vec<_> = journal
        .entries
        .iter()
        .filter(|entry| entry.actor == "alice")
        .map(|entry| (entry.path.clone(), entry.content.is_some()))
        .collect();
    assert_eq!(mine, vec![("plan.md".to_string(), true)]);
}

/// A merge whose text is the team's own ends the draft: the team took her line
/// and added one of his.
#[tokio::test]
async fn a_merge_equal_to_the_teams_text_ends_the_draft() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.append("plan", HER_LINE).await;
    // Her text with his line in it, her `generated` stamp and all.
    let theirs = bobs(&f.row("plan.md").await);
    f.push(&[("plan.md", Some(theirs))]).await;

    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("plan.md"), None);
    assert!(f.reads("plan").await.contains(BOB_LINE));
}

/// An edited case move whose edit the team took, with a line of his own: her
/// draft takes his text and is an unedited case move from there, which stays in
/// step on either disk, as C10 has it. Nobody's change ends her case move.
#[tokio::test]
async fn an_edited_case_move_whose_edit_the_team_took_stays_on_either_disk() {
    let f = fixture(&[("plan.md", LONG)]).await;
    eprintln!("the temp folder folds case: {}", folds_case(&f.root));
    f.append("plan", HER_LINE).await;
    let theirs = bobs(&f.row("plan.md").await);
    f.moved("plan", "Plan.md").await;
    f.push(&[("plan.md", Some(theirs.clone()))]).await;

    assert_eq!(
        f.rows().await,
        vec![
            ("Plan.md".to_string(), false),
            ("plan.md".to_string(), true)
        ]
    );
    let row = f.row("Plan.md").await;
    assert!(row.contains(BOB_LINE) && row.contains(HER_LINE), "{row}");
    assert_eq!(
        f.start("Plan.md").unwrap().start.as_deref(),
        Some(theirs.as_str())
    );
    assert_eq!(f.conflicts().await, vec![]);
}

/// A merge into an unedited case move whose start could not be saved leaves the
/// team's text over the old start. On a disk that folds case that reads like her
/// case move landed at the team's spelling; the merge mark keeps it in step, and
/// the next pass moves the start. Both rows stand on either disk.
#[tokio::test]
async fn a_case_move_whose_start_could_not_move_stays_on_either_disk() {
    let f = fixture(&[("plan.md", LONG)]).await;
    eprintln!("the temp folder folds case: {}", folds_case(&f.root));
    f.moved("plan", "Plan.md").await;
    f.engine.fail_next_merge_start();
    f.push(&[("plan.md", Some(bobs(LONG)))]).await;
    let both = vec![
        ("Plan.md".to_string(), false),
        ("plan.md".to_string(), true),
    ];
    assert_eq!(f.rows().await, both);
    assert_eq!(f.start("Plan.md").unwrap().merging, Some(false));
    assert_eq!(
        f.start("plan.md").unwrap().start.as_deref(),
        Some(LONG),
        "the deletion half keeps its start until the draft's has moved"
    );

    f.engine.converge_overlays("team").await.unwrap();
    assert_eq!(f.rows().await, both);
    let record = f.start("Plan.md").unwrap();
    assert_eq!(record.merging, None);
    assert_eq!(record.start.as_deref(), Some(bobs(LONG).as_str()));
    assert!(f.row("Plan.md").await.contains(BOB_LINE));
    assert_eq!(f.conflicts().await, vec![]);
}

/// Her share of an edited case move lands, and the pass ends the move: the
/// tombstone first, then the draft. The draft changes between the two, so it
/// stays; the tombstone is put back, so the move stands whole and her own old
/// path is never read as a team file holding her address.
///
/// The edit verbs cannot write the draft in that window: with the tombstone gone
/// the team's `plan.md` holds her address, and the address check refuses them.
/// So the row is written straight, as any writer that skips that check would.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_between_the_two_drops_of_a_landed_move_keeps_the_move_whole() {
    let f = Arc::new(fixture_direct(&[("plan.md", LONG)]).await);
    let folds = folds_case(&f.root);
    eprintln!("the temp folder folds case: {folds}");
    f.append("plan", HER_LINE).await;
    f.moved("plan", "Plan.md").await;

    // Hold the second drop, the draft's, and edit the draft in that window.
    f.engine.hold_convergence_drop(1, 1500);
    let share = {
        let f = f.clone();
        tokio::spawn(async move { f.share().await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let edited = format!(
        "{}- [idea] a word after her share #team\n",
        f.row("Plan.md").await
    );
    f.old_draft("Plan.md", &edited).await;
    let answer = share.await.unwrap();
    assert_eq!(answer["outcome"], "committed", "{answer}");

    assert_eq!(
        f.rows().await,
        vec![
            ("Plan.md".to_string(), false),
            ("plan.md".to_string(), true)
        ],
        "{answer}"
    );
    assert!(f.row("Plan.md").await.contains("a word after her share"));
    assert!(
        f.start("plan.md").is_some(),
        "the half's record came back too"
    );
    let kinds: Vec<DraftConflictKind> = f.conflicts().await.into_iter().map(|c| c.kind).collect();
    assert!(
        !kinds.contains(&DraftConflictKind::AddressTaken),
        "{kinds:?}"
    );
}

// --- Keep mine and Take theirs on a move ------------------------------------

impl Fixture {
    async fn resolve(&self, path: &str, keep: &str) -> serde_json::Value {
        self.engine
            .origin_resolve(
                "team",
                path,
                Some(keep),
                None,
                ShareActor::Account("alice".to_string()),
            )
            .await
            .unwrap()
    }

    /// Alice moves the plan to `to` with her own first line, and bob changes the
    /// same line: one conflict, at her new path.
    async fn a_move_in_conflict(&self, to: &str) {
        self.replace("plan", "the first line", "alice's first line")
            .await;
        self.moved("plan", to).await;
        self.push(&[("plan.md", Some(bobs(LONG)))]).await;
        assert_eq!(
            self.conflicts().await,
            vec![moved_conflict(to, "plan.md", DraftConflictKind::EditEdit)]
        );
    }
}

/// N4: Take theirs at the path she moved from settles the whole move, for a
/// rename and for a case move, on either disk.
#[tokio::test]
async fn resolving_a_move_at_the_old_path_settles_the_whole_move() {
    for to in ["renamed.md", "Plan.md"] {
        let f = fixture(&[("plan.md", LONG)]).await;
        eprintln!("{to}: the temp folder folds case: {}", folds_case(&f.root));
        f.a_move_in_conflict(to).await;

        let receipt = f.resolve("plan.md", "theirs").await;
        assert_eq!(receipt["resolved"], to, "{receipt}");
        assert_eq!(receipt["kept"], "theirs", "{receipt}");
        assert_eq!(receipt["remaining"], 0, "{receipt}");
        assert_eq!(f.rows().await, vec![], "{to}");
        assert_eq!((f.start("plan.md"), f.start(to)), (None, None), "{to}");
        assert_eq!(f.conflicts().await, vec![], "{to}");
        assert!(f.reads("plan").await.contains(BOB_LINE), "{to}");
        assert_eq!(f.share().await["outcome"], "nothing_to_share", "{to}");
    }
}

/// N1 after a resolve: Keep mine on an edited move in conflict, then her direct
/// share, ends both halves with no row left.
#[tokio::test]
async fn keep_mine_on_an_edited_move_ends_when_shared_with_no_row_left() {
    let f = fixture_direct(&[("plan.md", LONG)]).await;
    f.a_move_in_conflict("renamed.md").await;

    let receipt = f.resolve("renamed.md", "mine").await;
    assert_eq!(receipt["resolved"], "renamed.md", "{receipt}");
    assert_eq!(receipt["kept"], "mine", "{receipt}");
    for path in ["renamed.md", "plan.md"] {
        let record = f.start(path).unwrap();
        assert_eq!(record.from.as_deref(), Some("plan.md"), "{path}");
        assert_eq!(record.start.as_deref(), Some(bobs(LONG).as_str()), "{path}");
    }
    assert_eq!(f.conflicts().await, vec![]);

    let answer = f.share().await;
    assert_eq!(answer["outcome"], "committed", "{answer}");
    assert_eq!(f.rows().await, vec![], "{answer}");
    assert!(
        f.forge_file("renamed.md")
            .unwrap()
            .contains("alice's first line")
    );
    assert_eq!(f.forge_file("plan.md"), None);
    assert_eq!(f.share().await["outcome"], "nothing_to_share");
}

/// Keep mine on a case move starts it at the spelling the snapshot holds, never
/// at her own spelling, which the team does not hold: a later change of the
/// team's merges, on either disk, rather than reading as the page deleted.
#[tokio::test]
async fn keep_mine_on_a_case_move_starts_at_the_teams_spelling() {
    let f = fixture(&[("plan.md", LONG)]).await;
    eprintln!("the temp folder folds case: {}", folds_case(&f.root));
    f.a_move_in_conflict("Plan.md").await;

    f.resolve("Plan.md", "mine").await;
    for path in ["Plan.md", "plan.md"] {
        let record = f.start(path).unwrap();
        assert_eq!(record.from.as_deref(), Some("plan.md"), "{path}");
        assert_eq!(record.start.as_deref(), Some(bobs(LONG).as_str()), "{path}");
    }
    assert_eq!(f.conflicts().await, vec![]);

    let later = bobs(LONG).replace("the last line", "carol's last line");
    f.push(&[("plan.md", Some(later.clone()))]).await;
    let row = f.row("Plan.md").await;
    assert!(
        row.contains("alice's first line") && row.contains("carol's last line"),
        "{row}"
    );
    assert_eq!(f.start("Plan.md").unwrap().start, Some(later));
    assert_eq!(f.conflicts().await, vec![]);
}

/// m-1: a merged resolve takes the pass's three steps. Its start could not move
/// after the row was written, so the record still marks the merge, and the
/// next pass merges again rather than reading her case move as landed: both
/// halves stand, on either disk.
#[tokio::test]
async fn a_merged_resolve_whose_start_could_not_move_keeps_the_case_move() {
    let f = fixture(&[("plan.md", LONG)]).await;
    eprintln!("the temp folder folds case: {}", folds_case(&f.root));
    f.a_move_in_conflict("Plan.md").await;
    let before = f.start("Plan.md").unwrap();

    f.engine.fail_next_merge_start();
    f.engine
        .origin_resolve(
            "team",
            "Plan.md",
            None,
            Some(bobs(LONG).as_bytes()),
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap_err();
    assert_eq!(f.row("Plan.md").await, bobs(LONG));
    assert_eq!(
        f.start("Plan.md"),
        Some(DraftStart {
            merging: Some(false),
            ..before
        }),
        "the mark stands over the old start"
    );

    f.engine.converge_overlays("team").await.unwrap();
    assert_eq!(
        f.rows().await,
        vec![
            ("Plan.md".to_string(), false),
            ("plan.md".to_string(), true)
        ]
    );
    let record = f.start("Plan.md").unwrap();
    assert_eq!(record.merging, None);
    assert_eq!(record.start.as_deref(), Some(bobs(LONG).as_str()));
    assert_eq!(f.conflicts().await, vec![]);
}

/// m-3: Keep mine on a move whose deletion half changed meanwhile settles the
/// draft and says the half is not settled, so she settles it again.
#[tokio::test]
async fn a_move_settled_half_way_says_so_and_a_second_press_finishes_it() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.a_move_in_conflict("renamed.md").await;

    f.engine.fail_next_half_settle();
    let err = f
        .engine
        .origin_resolve(
            "team",
            "renamed.md",
            Some("mine"),
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Your draft of renamed.md is settled, but plan.md, where you moved it from, changed \
         while it was being settled and is not. Settle plan.md again."
    );
    assert_eq!(
        f.start("renamed.md").unwrap().start.as_deref(),
        Some(bobs(LONG).as_str())
    );

    let receipt = f.resolve("plan.md", "mine").await;
    assert_eq!(receipt["resolved"], "renamed.md", "{receipt}");
    assert_eq!(
        f.start("plan.md").unwrap().start.as_deref(),
        Some(bobs(LONG).as_str())
    );
    assert_eq!(f.conflicts().await, vec![]);
}

/// m-7: a merged text whose mirror could not be written still lands, and the
/// receipt says so, as every draft write does.
#[tokio::test]
async fn a_merged_resolve_tells_the_caller_when_its_mirror_failed() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.a_move_in_conflict("renamed.md").await;
    // A folder where the mirror's file goes, so the copy cannot be written.
    let mirror = f.state.join("overlays/team/alice/renamed.md");
    std::fs::remove_file(&mirror).unwrap();
    std::fs::create_dir_all(mirror.join("in-the-way")).unwrap();

    let merged = bobs(LONG).replace("the last line", "alice's last line");
    let receipt = f
        .engine
        .origin_resolve(
            "team",
            "renamed.md",
            None,
            Some(merged.as_bytes()),
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(receipt["kept"], "merged", "{receipt}");
    assert!(receipt["draft_warning"].is_string(), "{receipt}");
    assert_eq!(f.row("renamed.md").await, merged);
}

// --- leaving review refuses a draft that cannot be merged --------------------

/// A fold step that runs `change` and waits for nothing.
fn sync_hook(change: impl FnOnce() + Send + 'static) -> FoldHook {
    Box::new(move || {
        change();
        Box::pin(async {})
    })
}

/// The fold's sentence for a person whose conflicts stand.
fn unsettled_of(actor: &str, paths: &str) -> String {
    format!(
        "'{actor}' has conflicts in {paths}. They can settle them, or you can choose discard, \
         which ends their text."
    )
}

/// The fold's sentence, to an admin, for a name with no account any more.
fn unsettled_of_removed(actor: &str, paths: &str) -> String {
    format!(
        "'{actor}' has conflicts in {paths} and no account any more, so nobody can settle \
         them. Choose discard, or end their drafts on the Users screen."
    )
}

/// The plan's sentence on a colleague's row in conflict.
fn row_of(actor: &str) -> String {
    format!("'{actor}' has a conflict here. Ask them to settle it, or choose discard.")
}

fn bob() -> Scope {
    Scope::User {
        account: "bob".to_string(),
        admin: false,
    }
}

impl Fixture {
    /// Leave review with these answers, as the domain's owner.
    async fn fold(
        &self,
        answers: &[(&str, FoldChoice)],
    ) -> Result<serde_json::Value, crystalline_service::engine::EngineError> {
        self.engine
            .set_review_mode(
                "team",
                None,
                ReviewModeConfirm::Confirmed {
                    folds: answers
                        .iter()
                        .map(|(actor, choice)| ((*actor).to_string(), *choice))
                        .collect(),
                },
                &Scope::Unrestricted,
            )
            .await
    }

    /// The plan of leaving review, as `scope` asks for it.
    async fn plan(&self, scope: &Scope) -> serde_json::Value {
        self.engine
            .set_review_mode("team", None, ReviewModeConfirm::Preview, scope)
            .await
            .unwrap()
    }

    /// Whether the config file still says the domain reviews changes.
    fn reviews(&self) -> bool {
        let text = std::fs::read_to_string(self.root.join("config.yaml")).unwrap();
        let cfg: GlobalConfig = serde_yaml_ng::from_str(&text).unwrap();
        cfg.domains["team"].review.is_some()
    }

    /// What the folder the team shares holds at `path`.
    fn folder(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join("team").join(path)).ok()
    }

    /// The names the folder lists, spelled as the disk keeps them.
    fn listed(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root.join("team"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| {
                name.ends_with(".md") && !["MANIFEST.md", "index.md"].contains(&name.as_str())
            })
            .collect();
        names.sort();
        names
    }

    /// Alice changes the plan's first line, and the team changes the same line
    /// with no pull and no pass: a conflict nothing has looked at yet.
    async fn a_stale_conflict(&self) {
        self.replace("plan", "the first line", "alice's first line")
            .await;
        self.team_has("plan.md", bobs(LONG).as_bytes());
    }

    /// An accounts store with no account in it, so every name but the
    /// machine owner's is a removed one.
    async fn no_accounts(&self) -> Arc<crystalline_service::rest::AuthStore> {
        let auth = Arc::new(
            crystalline_service::rest::AuthStore::open(&self.root.join("web-auth.db"))
                .await
                .unwrap(),
        );
        self.engine
            .set_domain_access(Arc::new(crystalline_service::DomainAccess::new(
                auth.clone(),
            )));
        auth
    }
}

/// The fold runs the pass first, so a stale edit that merges lands with the
/// team's line beside hers.
#[tokio::test]
async fn a_fold_of_a_stale_edit_that_merges_lands_both_lines() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.append("plan", HER_LINE).await;
    f.team_has("plan.md", bobs(LONG).as_bytes());

    let plan = f.plan(&Scope::Unrestricted).await;
    assert_eq!(
        plan["actors"][0]["drafts"][0]["conflict"],
        serde_json::Value::Null,
        "{plan}"
    );
    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap();
    let landed = f.folder("plan.md").unwrap();
    assert!(
        landed.contains(BOB_LINE) && landed.contains(HER_LINE),
        "{landed}"
    );
    assert!(!f.reviews());
}

#[tokio::test]
async fn a_fold_of_a_conflict_refuses_and_changes_nothing() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.a_stale_conflict().await;
    let hers = f.row("plan.md").await;
    let start = f.start("plan.md");

    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    assert_eq!(
        refused.to_string(),
        format!(
            "Nothing in 'team' was folded. {}",
            unsettled_of("alice", "plan.md")
        ),
        "refused before the key came off"
    );
    assert!(f.reviews(), "the key is still on");
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(f.row("plan.md").await, hers);
    assert_eq!(f.start("plan.md"), start);
    assert_eq!(
        f.folder("plan.md"),
        Some(bobs(LONG)),
        "the folder is unchanged"
    );
}

/// The plan names the conflict on her row: to the owner as hers, and to her as
/// her own.
#[tokio::test]
async fn a_fold_plan_names_the_conflict_on_the_row() {
    let f = fixture(&[("plan.md", LONG), ("note.md", NOTE)]).await;
    f.a_stale_conflict().await;
    f.delete("note").await;
    f.team_has(
        "note.md",
        NOTE.replace("team body", "bob's body").as_bytes(),
    );

    let plan = f.plan(&Scope::Unrestricted).await;
    let rows = &plan["actors"][0]["drafts"];
    assert_eq!(rows[0]["path"], "note.md", "{plan}");
    assert_eq!(
        rows[0]["conflict"],
        row_of("alice"),
        "a deletion stands too: {plan}"
    );
    assert_eq!(rows[1]["path"], "plan.md", "{plan}");
    assert_eq!(rows[1]["conflict"], row_of("alice"), "{plan}");

    let own = Scope::User {
        account: "alice".to_string(),
        admin: true,
    };
    let plan = f.plan(&own).await;
    assert_eq!(
        plan["actors"][0]["drafts"][1]["conflict"], "You have a conflict here. Settle it first.",
        "{plan}"
    );

    // A move lists once, at its draft, and both its rows carry the sentence.
    let f = fixture(&[("plan.md", LONG)]).await;
    f.a_move_in_conflict("renamed.md").await;
    let plan = f.plan(&Scope::Unrestricted).await;
    let rows = &plan["actors"][0]["drafts"];
    for row in [0, 1] {
        assert_eq!(rows[row]["conflict"], row_of("alice"), "{plan}");
    }
}

#[tokio::test]
async fn a_fold_of_an_unedited_move_lands_the_peers_line_on_either_disk() {
    let f = fixture(&[("plan.md", LONG)]).await;
    eprintln!("the temp folder folds case: {}", folds_case(&f.root));
    f.moved("plan", "renamed.md").await;
    f.team_has("plan.md", bobs(LONG).as_bytes());

    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap();
    assert_eq!(f.listed(), vec!["renamed.md".to_string()]);
    assert_eq!(f.folder("renamed.md"), Some(bobs(LONG)));
}

/// Asserted per disk: a disk that does not fold case lands her spelling with
/// the team's line, and one that folds keeps one file with the team's line.
#[tokio::test]
async fn a_fold_of_an_unedited_case_move_lands_her_spelling_on_a_case_sensitive_disk() {
    let f = fixture(&[("plan.md", LONG)]).await;
    let folds = folds_case(&f.root);
    eprintln!("the temp folder folds case: {folds}");
    f.moved("plan", "Plan.md").await;
    f.team_has("plan.md", bobs(LONG).as_bytes());

    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap();
    let listed = f.listed();
    if folds {
        assert_eq!(listed.len(), 1, "{listed:?}");
    } else {
        assert_eq!(listed, vec!["Plan.md".to_string()]);
    }
    assert_eq!(f.folder(&listed[0]), Some(bobs(LONG)));
}

#[tokio::test]
async fn a_fold_of_a_stale_delete_refuses_and_the_file_stays() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.delete("plan").await;
    f.team_has("plan.md", PLAN_LATER.as_bytes());

    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains(&unsettled_of("alice", "plan.md")),
        "{refused}"
    );
    assert!(f.reviews());
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), true)]);
    assert_eq!(f.folder("plan.md").as_deref(), Some(PLAN_LATER));
}

#[tokio::test]
async fn discard_still_works_on_a_conflict() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.a_stale_conflict().await;

    f.fold(&[("alice", FoldChoice::Discard)]).await.unwrap();
    assert!(!f.reviews());
    assert_eq!(f.rows().await, vec![]);
    assert_eq!(f.start("plan.md"), None);
    assert_eq!(f.folder("plan.md"), Some(bobs(LONG)));
}

/// The second check, after the rooms have closed with the key off. A room's own
/// save lands in the folder there and is no draft, so what makes her draft
/// stale in that window is the team's change arriving with it, which the seam
/// stands in for. The refusal puts the key back, as the address check does.
#[tokio::test]
async fn a_pull_during_the_fold_puts_the_key_back() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.replace("plan", "the first line", "alice's first line")
        .await;
    let hers = f.row("plan.md").await;
    let (folder, snapshot) = (f.root.join("team"), f.snapshot_dir());
    f.engine.at_next_fold_step(
        FoldStep::RoomsClosed,
        sync_hook(move || {
            let bytes = bobs(LONG).into_bytes();
            std::fs::write(folder.join("plan.md"), &bytes).unwrap();
            let mut state = OriginState::load(&snapshot).unwrap().unwrap();
            state.files.insert(
                "plan.md".to_string(),
                BaseStamp {
                    sha256: crate::support::sha256_hex(&bytes),
                    size: bytes.len() as u64,
                },
            );
            write_base_file(&snapshot, "plan.md", &bytes).unwrap();
            state.save(&snapshot).unwrap();
        }),
    );

    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    let words = refused.to_string();
    assert!(words.contains(&unsettled_of("alice", "plan.md")), "{words}");
    assert!(
        words.ends_with(
            "This changed while the fold was under way, and the domain reviews changes again."
        ),
        "{words}"
    );
    assert!(!words.contains("out-of-band"), "no room closed: {words}");
    assert!(f.reviews(), "the key is back on");
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(f.row("plan.md").await, hers);
    assert_eq!(f.folder("plan.md"), Some(bobs(LONG)));
}

/// A fold that failed part way leaves the key off and her rows standing. The
/// repeat still asks for conflicts, and says what the earlier fold left.
#[tokio::test]
async fn a_mid_fold_recovery_still_refuses_a_conflict() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.replace("plan", "the first line", "alice's first line")
        .await;
    f.engine
        .write_engram_as(
            &WriteParams {
                folder: Some("sub".to_string()),
                ..write_params("Fresh", "her new page")
            },
            None,
            &alice(),
        )
        .await
        .unwrap();
    // A file where the folder `sub` has to go, so the fold fails after the plan.
    std::fs::write(f.root.join("team").join("sub"), "not a folder").unwrap();
    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    assert!(!f.reviews(), "the half-state: the key is off");
    assert_eq!(f.rows().await.len(), 2);

    f.team_has("plan.md", bobs(LONG).as_bytes());
    std::fs::remove_file(f.root.join("team").join("sub")).unwrap();
    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    assert_eq!(
        refused.to_string(),
        format!(
            "Nothing more in 'team' was folded. {} An earlier fold stopped part way, and what it \
             wrote stays in the folder whatever you choose.",
            unsettled_of("alice", "plan.md")
        )
    );
    assert!(!f.reviews(), "the key stays off");
    assert_eq!(f.rows().await.len(), 2);
    assert_eq!(f.folder("plan.md"), Some(bobs(LONG)));
}

#[tokio::test]
async fn a_removed_names_conflict_refuses_the_fold_with_its_two_ways_out() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.a_stale_conflict().await;
    let _auth = f.no_accounts().await;

    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    let words = refused.to_string();
    assert!(
        words.contains(&unsettled_of_removed("alice", "plan.md")),
        "{words}"
    );
    assert!(f.reviews());
    let plan = f.plan(&Scope::Unrestricted).await;
    assert_eq!(
        plan["actors"][0]["drafts"][0]["conflict"],
        "'alice' has no account any more, so nobody can settle this. Choose discard, or end \
         their drafts on the Users screen.",
        "nobody is told to ask a removed name: {plan}"
    );
    assert_eq!(f.folder("plan.md"), Some(bobs(LONG)));

    f.fold(&[("alice", FoldChoice::Discard)]).await.unwrap();
    assert_eq!(f.folder("plan.md"), Some(bobs(LONG)));
    assert_eq!(f.rows().await, vec![]);
}

#[tokio::test]
async fn ending_a_removed_names_drafts_takes_the_records_and_a_new_account_inherits_nothing() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.a_stale_conflict().await;
    let auth = f.no_accounts().await;
    assert!(f.start("plan.md").is_some());

    f.engine
        .discard_account_drafts("alice", "root")
        .await
        .unwrap();
    assert_eq!(f.start("plan.md"), None);
    assert!(
        draft_start::all(&f.state, "team", "alice")
            .unwrap()
            .is_empty()
    );

    auth.add_user(
        "alice",
        "alice",
        None,
        crystalline_service::rest::Role::Editor,
        "pw12345678",
    )
    .await
    .unwrap();
    assert_eq!(f.conflicts().await, vec![]);
    let plan = f.plan(&Scope::Unrestricted).await;
    assert_eq!(plan["actors"], serde_json::json!([]), "{plan}");
    f.fold(&[]).await.unwrap();
    assert_eq!(f.folder("plan.md"), Some(bobs(LONG)));
}

/// A text kept from a removed name's draft is no draft: it never blocks a fold.
#[tokio::test]
async fn a_kept_text_of_a_removed_name_takes_no_part_in_a_conflict() {
    let f = fixture(&[("plan.md", PLAN)]).await;
    f.old_draft(
        "plan.md",
        &PLAN.replace("as the team has it", "her old words"),
    )
    .await;
    f.team_has("plan.md", PLAN_LATER.as_bytes());
    let receipt = f.resolve("plan.md", "theirs").await;
    assert_eq!(receipt["kept_copy"], true, "{receipt}");
    let _auth = f.no_accounts().await;
    f.engine
        .write_engram_as(&write_params("Bobs", "bob's page"), None, &bob())
        .await
        .unwrap();

    assert_eq!(f.conflicts().await, vec![]);
    f.fold(&[("bob", FoldChoice::Fold)]).await.unwrap();
    assert_eq!(f.folder("plan.md").as_deref(), Some(PLAN_LATER));
    assert!(f.folder("bobs.md").unwrap().contains("bob's page"));
}

/// Only the people who fold are asked: a colleague's conflict refuses the fold
/// while he folds, and with discard chosen for him the others land.
#[tokio::test]
async fn a_colleagues_conflict_with_her_choice_discard_lets_the_others_fold() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.a_stale_conflict().await;
    f.engine
        .write_engram_as(&write_params("Bobs", "bob's page"), None, &bob())
        .await
        .unwrap();

    let refused = f
        .fold(&[("alice", FoldChoice::Fold), ("bob", FoldChoice::Fold)])
        .await
        .unwrap_err();
    let words = refused.to_string();
    assert!(words.contains(&unsettled_of("alice", "plan.md")), "{words}");
    assert!(!words.contains("'bob'"), "{words}");
    assert_eq!(f.folder("bobs.md"), None);

    f.fold(&[("alice", FoldChoice::Discard), ("bob", FoldChoice::Fold)])
        .await
        .unwrap();
    assert!(f.folder("bobs.md").unwrap().contains("bob's page"));
    assert_eq!(f.folder("plan.md"), Some(bobs(LONG)));
}

/// C1: a record that cannot be read refuses the fold and changes nothing. The
/// plan still answers, and says her conflicts could not be read.
#[tokio::test]
async fn a_fold_with_a_broken_record_refuses_and_changes_nothing() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.append("plan", HER_LINE).await;
    let file = draft_start::record_file(&f.state, "team", "alice", "plan.md").unwrap();
    std::fs::write(&file, "not a record").unwrap();

    let plan = f.plan(&Scope::Unrestricted).await;
    assert_eq!(plan["actors"][0]["conflicts_unreadable"], true, "{plan}");
    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    assert!(
        matches!(
            refused,
            crystalline_service::engine::EngineError::Conflict(_)
        ),
        "a 409, not a 500: {refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "Nothing in 'team' was folded. The conflicts of 'alice' could not be read, so their \
         drafts cannot be folded safely. Choose discard for 'alice', which ends their text, or \
         fold again once the problem in the log is fixed."
    );
    assert!(f.reviews());
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(f.folder("plan.md").as_deref(), Some(LONG));

    f.fold(&[("alice", FoldChoice::Discard)]).await.unwrap();
    assert_eq!(f.folder("plan.md").as_deref(), Some(LONG));
}

/// The machine owner has no account and is never a removed name: their
/// conflict refuses with the way out a person has.
#[tokio::test]
async fn the_machine_owners_conflict_is_never_a_removed_names() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.engine
        .edit_engram_as(
            &EditParams {
                operation: "find_replace".to_string(),
                find_text: Some("the first line".to_string()),
                ..edit_append("plan", "the owner's first line")
            },
            None,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    f.team_has("plan.md", bobs(LONG).as_bytes());
    let _auth = f.no_accounts().await;

    let refused = f.fold(&[("owner", FoldChoice::Fold)]).await.unwrap_err();
    let words = refused.to_string();
    assert_eq!(
        words,
        "Nothing in 'team' was folded. You have conflicts in plan.md. Settle them first, or \
         choose discard, which ends your text.",
        "the owner reads their own drafts as theirs"
    );
}

/// C1 at the second check: a record that breaks while the rooms close refuses
/// the fold, and the key goes back on with nothing folded.
#[tokio::test]
async fn a_record_broken_during_the_fold_puts_the_key_back() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.append("plan", HER_LINE).await;
    let file = draft_start::record_file(&f.state, "team", "alice", "plan.md").unwrap();
    f.engine.at_next_fold_step(
        FoldStep::RoomsClosed,
        sync_hook(move || {
            std::fs::write(&file, "not a record").unwrap();
        }),
    );

    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    assert!(
        matches!(
            refused,
            crystalline_service::engine::EngineError::Conflict(_)
        ),
        "{refused:?}"
    );
    assert!(
        refused
            .to_string()
            .ends_with("The domain reviews changes again."),
        "{refused}"
    );
    assert!(f.reviews(), "the key is back on");
    assert_eq!(f.rows().await, vec![("plan.md".to_string(), false)]);
    assert_eq!(f.folder("plan.md").as_deref(), Some(LONG));
}

/// Window A: the team's change is pulled while the key is still on, and the
/// pull's pass merges it into her row. The fold lands that row, not the text it
/// read before the pull.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pull_before_the_key_comes_off_keeps_the_teams_line() {
    let f = Arc::new(fixture(&[("plan.md", LONG)]).await);
    f.append("plan", HER_LINE).await;
    let g = f.clone();
    f.engine.at_next_fold_step(
        FoldStep::KeyOff,
        Box::new(move || {
            Box::pin(async move {
                // The update's tail waits for the admin lock the fold holds, so
                // the pull runs on its own and the fold goes on once the pass
                // has merged the team's line into her row.
                let h = g.clone();
                tokio::spawn(async move {
                    h.push(&[("plan.md", Some(bobs(LONG)))]).await;
                });
                for _ in 0..200 {
                    if g.row("plan.md").await.contains(BOB_LINE) {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                panic!("the pull never merged into her row");
            })
        }),
    );

    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap();
    let landed = f.folder("plan.md").unwrap();
    assert!(
        landed.contains(BOB_LINE) && landed.contains(HER_LINE),
        "{landed}"
    );
}

/// Window B: a pull that starts after the last check waits for the fold, so
/// it merges into what the fold wrote and the team's line stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pull_after_the_last_check_waits_for_the_fold() {
    let f = Arc::new(fixture(&[("plan.md", LONG)]).await);
    f.append("plan", HER_LINE).await;
    let pull: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> = Arc::default();
    let (g, slot) = (f.clone(), pull.clone());
    f.engine.at_next_fold_step(
        FoldStep::Checked,
        Box::new(move || {
            Box::pin(async move {
                let task = tokio::spawn(async move {
                    g.push(&[("plan.md", Some(bobs(LONG)))]).await;
                });
                *slot.lock().unwrap() = Some(task);
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            })
        }),
    );

    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap();
    let task = pull.lock().unwrap().take().unwrap();
    task.await.unwrap();
    let landed = f.folder("plan.md").unwrap();
    assert!(
        landed.contains(BOB_LINE) && landed.contains(HER_LINE),
        "{landed}"
    );
}

/// From a stale row in a fold that stopped part way to a finished fold: the
/// pass still merges what merges, resolve still settles the rest, and the
/// repeat lands both.
#[tokio::test]
async fn a_mid_fold_recovery_merges_settles_and_finishes() {
    let f = fixture(&[("plan.md", LONG), ("rich.md", RICH)]).await;
    f.append("plan", HER_LINE).await;
    f.replace("rich", "the first thing", "alice's first thing")
        .await;
    f.engine
        .write_engram_as(
            &WriteParams {
                folder: Some("sub".to_string()),
                ..write_params("Fresh", "her new page")
            },
            None,
            &alice(),
        )
        .await
        .unwrap();
    std::fs::write(f.root.join("team").join("sub"), "not a folder").unwrap();
    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    assert!(!f.reviews());
    assert!(
        f.state.join("overlays/team/.fold-unfinished").is_file(),
        "the stopped fold leaves its mark, so a pull before the repeat still merges"
    );

    // The team changes both pages: one change merges with hers, one does not.
    f.team_has("plan.md", bobs(LONG).as_bytes());
    let bobs_rich = RICH.replace("the first thing", "bob's first thing");
    f.team_has("rich.md", bobs_rich.as_bytes());
    std::fs::remove_file(f.root.join("team").join("sub")).unwrap();

    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains(&unsettled_of("alice", "rich.md")),
        "only the page that does not merge stands: {refused}"
    );
    let merged = f.row("plan.md").await;
    assert!(
        merged.contains(BOB_LINE) && merged.contains(HER_LINE),
        "{merged}"
    );

    let receipt = f.resolve("rich.md", "mine").await;
    assert_eq!(receipt["kept"], "mine", "{receipt}");
    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap();
    assert!(!f.reviews());
    assert_eq!(f.rows().await, vec![]);
    let plan = f.folder("plan.md").unwrap();
    assert!(plan.contains(BOB_LINE) && plan.contains(HER_LINE), "{plan}");
    assert!(f.folder("rich.md").unwrap().contains("alice's first thing"));
    assert!(f.folder("sub/fresh.md").is_some());
    assert!(
        !f.state.join("overlays/team/.fold-unfinished").exists(),
        "the mark is gone once the fold finished"
    );
}

/// A repeated fold that meets a new conflict at its second check leaves the
/// domain as it found it: the key stays off and the mark stays.
#[tokio::test]
async fn a_pull_during_a_repeated_fold_leaves_the_key_off() {
    let f = fixture(&[("plan.md", LONG)]).await;
    f.replace("plan", "the first line", "alice's first line")
        .await;
    f.engine
        .write_engram_as(
            &WriteParams {
                folder: Some("sub".to_string()),
                ..write_params("Fresh", "her new page")
            },
            None,
            &alice(),
        )
        .await
        .unwrap();
    std::fs::write(f.root.join("team").join("sub"), "not a folder").unwrap();
    f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    std::fs::remove_file(f.root.join("team").join("sub")).unwrap();

    let (folder, snapshot) = (f.root.join("team"), f.snapshot_dir());
    f.engine.at_next_fold_step(
        FoldStep::RoomsClosed,
        sync_hook(move || {
            let bytes = bobs(LONG).into_bytes();
            std::fs::write(folder.join("plan.md"), &bytes).unwrap();
            let mut state = OriginState::load(&snapshot).unwrap().unwrap();
            state.files.insert(
                "plan.md".to_string(),
                BaseStamp {
                    sha256: crate::support::sha256_hex(&bytes),
                    size: bytes.len() as u64,
                },
            );
            write_base_file(&snapshot, "plan.md", &bytes).unwrap();
            state.save(&snapshot).unwrap();
        }),
    );
    let refused = f.fold(&[("alice", FoldChoice::Fold)]).await.unwrap_err();
    let words = refused.to_string();
    assert!(
        words.ends_with("This changed while the fold was under way."),
        "{words}"
    );
    assert!(!f.reviews(), "the key stays off");
    assert!(f.state.join("overlays/team/.fold-unfinished").is_file());
    assert_eq!(f.rows().await.len(), 2);
}
