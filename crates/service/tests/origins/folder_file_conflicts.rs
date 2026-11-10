//! A conflict the pull records in the domain folder at a file beside the pages
//! (`assets/...`): the folder held its own bytes, and the team changed the same
//! file. A file never merges, so it settles one of two ways: the folder keeps
//! its file, or it takes the team's and a copy of its own is kept. Each surface
//! that resolves reaches it, in a domain that takes changes directly and in one
//! that reviews them, and the domain shares again once it is settled.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::origin::{commit_files, engine_with, engram, manifest, reviewing_domain};
use crate::support::MockProvider;
use crystalline_remote::RemoteError;
use crystalline_remote::state::OriginState;
use crystalline_service::engine::{EngineError, ShareActor};
use crystalline_service::{Engine, Scope};
use serde_json::{Value, json};

const DECK: &str = "assets/deck.png";
const TEAM_DECK: &[u8] = b"\x89PNG\r\n\x1a\n\x00the team's deck";
const FOLDER_DECK: &[u8] = b"\x89PNG\r\n\x1a\n\x00the deck edited in the folder";
const NEWER_DECK: &[u8] = b"\x89PNG\r\n\x1a\n\x00the team's newer deck";

/// How the domain takes changes.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Direct,
    Reviewing,
}

/// The way a resolve reaches the engine.
#[derive(Clone, Copy, Debug)]
enum Surface {
    /// By path, the verb MCP's `resolve_conflict` calls.
    Path,
    /// By path from the CLI, over the control socket.
    Ctl,
    /// The folder's conflict only, the verb the JSON API calls by id.
    Folder,
}

const SURFACES: [Surface; 3] = [Surface::Path, Surface::Ctl, Surface::Folder];

struct Fx {
    tmp: tempfile::TempDir,
    mock: Arc<MockProvider>,
    eng: Arc<Engine>,
}

async fn fx(kind: Kind) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let files = [
        ("MANIFEST.md", manifest()),
        ("plan.md", engram("Plan", "plan", "the plan")),
        (DECK, TEAM_DECK.to_vec()),
    ];
    let eng = match kind {
        Kind::Reviewing => reviewing_domain(tmp.path(), mock.clone(), &files).await,
        Kind::Direct => {
            let commit = mock.add_commit(commit_files(&files));
            mock.set_branch("main", &commit);
            let eng = engine_with(
                &tmp.path().join("config.yaml"),
                &tmp.path().join("origins"),
                mock.clone(),
                true,
                false,
            )
            .await;
            eng.origin_add(
                "acme/team",
                Some("team"),
                None,
                None,
                Some(tmp.path().join("team-knowledge").to_str().unwrap()),
                None,
            )
            .await
            .unwrap();
            eng
        }
    };
    Fx {
        tmp,
        mock,
        eng: Arc::new(eng),
    }
}

impl Fx {
    fn root(&self) -> PathBuf {
        self.tmp.path().join("team-knowledge")
    }

    fn deck(&self) -> Option<Vec<u8>> {
        std::fs::read(self.root().join(DECK)).ok()
    }

    /// The paths of the conflicts the folder has recorded.
    fn conflicts(&self) -> Vec<String> {
        OriginState::load(&self.tmp.path().join("origins").join("team"))
            .unwrap()
            .unwrap()
            .conflicts
            .into_iter()
            .map(|c| c.path)
            .collect()
    }

    /// The kept copies of the domain itself, not of anybody's draft, with
    /// their file and bytes.
    fn kept(&self) -> Vec<(String, PathBuf, Vec<u8>)> {
        crystalline_core::kept::scan(&self.tmp.path().join(crystalline_core::kept::KEPT_DIR))
            .into_iter()
            .filter(|kept| kept.owner.is_none())
            .map(|kept| {
                let bytes = std::fs::read(&kept.file).unwrap();
                (kept.path, kept.file, bytes)
            })
            .collect()
    }

    /// The folder's file is edited out of band, then the team changes it
    /// (`Some`) or deletes it (`None`), and this machine pulls.
    async fn folder_conflict(&self, team: Option<&[u8]>) {
        std::fs::create_dir_all(self.root().join("assets")).unwrap();
        std::fs::write(self.root().join(DECK), FOLDER_DECK).unwrap();
        let head = self.mock.branch_commit("main").unwrap();
        let mut files: std::collections::BTreeMap<String, Vec<u8>> = self
            .mock
            .commit_paths(&head)
            .into_iter()
            .map(|path| {
                let bytes = self.mock.commit_file(&head, &path).unwrap();
                (path, bytes)
            })
            .collect();
        match team {
            Some(bytes) => files.insert(DECK.to_string(), bytes.to_vec()),
            None => files.remove(DECK),
        };
        let commit = self.mock.add_commit(files);
        self.mock.set_branch("main", &commit);
        self.eng
            .origin_update(Some("team"), &Scope::Unrestricted)
            .await
            .unwrap();
        assert_eq!(
            self.conflicts(),
            vec![DECK.to_string()],
            "the pull recorded it"
        );
    }

    /// Settles the folder's conflict at [`DECK`] through `surface`.
    async fn resolve(
        &self,
        surface: Surface,
        keep: Option<&str>,
        content: Option<&[u8]>,
    ) -> Result<Value, String> {
        match surface {
            Surface::Path => self
                .eng
                .origin_resolve_expecting("team", DECK, keep, content, None, ShareActor::Owner)
                .await
                .map_err(|e| e.to_string()),
            Surface::Folder => self
                .eng
                .origin_resolve_folder("team", DECK, keep, content, ShareActor::Owner)
                .await
                .map_err(|e| e.to_string()),
            Surface::Ctl => {
                let mut req =
                    json!({ "v": 1, "cmd": "origin_resolve", "domain": "team", "path": DECK });
                if let Some(keep) = keep {
                    req["keep"] = json!(keep);
                }
                if let Some(bytes) = content {
                    use base64::Engine as _;
                    req["content_b64"] =
                        json!(base64::engine::general_purpose::STANDARD.encode(bytes));
                }
                let reply =
                    crystalline_service::control::answer_for_test(self.eng.clone(), &req).await;
                match reply["ok"].as_bool() {
                    Some(true) => Ok(reply["data"].clone()),
                    _ => Err(reply["error"].as_str().unwrap_or_default().to_string()),
                }
            }
        }
    }

    /// A share of everything the owner holds.
    async fn share(&self) -> Result<Value, EngineError> {
        self.eng
            .origin_share("team", None, None, None, None, ShareActor::Owner)
            .await
    }

    /// What the proposal a share opened holds at `path`.
    fn proposed(&self, receipt: &Value, path: &str) -> Option<Vec<u8>> {
        let branch = receipt["branch"].as_str().expect("a proposal branch");
        let commit = self.mock.branch_commit(branch).unwrap();
        self.mock.commit_file(&commit, path)
    }

    /// A change of a page that is ready to share: in the folder for a domain
    /// that takes changes directly, as the owner's draft in a reviewing one.
    async fn a_page_to_share(&self, kind: Kind) {
        let changed = engram("Plan", "plan", "the plan, changed");
        match kind {
            Kind::Direct => {
                std::fs::write(self.root().join("plan.md"), &changed).unwrap();
                self.eng.sync(Some("team")).await.unwrap();
            }
            Kind::Reviewing => {
                super::origin::drafted(
                    &self.eng,
                    self.tmp.path(),
                    crystalline_service::scope::OWNER_IDENTITY_NAME,
                    "plan.md",
                    &String::from_utf8(changed).unwrap(),
                )
                .await
            }
        }
    }
}

/// What was stuck: a folder conflict at a file refuses every share of the
/// domain, and a resolve is what clears it. Before the fix every surface
/// refused the path, so the domain could never share again.
#[tokio::test]
async fn a_folder_conflict_at_a_file_blocks_shares_until_a_resolve_settles_it() {
    for kind in [Kind::Direct, Kind::Reviewing] {
        let f = fx(kind).await;
        f.folder_conflict(Some(NEWER_DECK)).await;
        f.a_page_to_share(kind).await;
        let pending = f.share().await.unwrap();
        assert_eq!(
            pending["outcome"], "conflicts_pending",
            "{kind:?}: {pending}"
        );

        f.resolve(Surface::Path, Some("theirs"), None)
            .await
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        assert_eq!(f.conflicts(), Vec::<String>::new(), "{kind:?}");
        let shared = f.share().await.unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        assert_eq!(shared["outcome"], "proposed", "{kind:?}: {shared}");
    }
}

/// Keep mine: the folder's bytes stay, nothing is kept, and the next share
/// carries them in place of the team's.
#[tokio::test]
async fn keep_mine_settles_a_folder_file_on_every_surface() {
    for kind in [Kind::Direct, Kind::Reviewing] {
        for surface in SURFACES {
            let f = fx(kind).await;
            f.folder_conflict(Some(NEWER_DECK)).await;
            let receipt = f
                .resolve(surface, Some("mine"), None)
                .await
                .unwrap_or_else(|e| panic!("{kind:?} {surface:?}: {e}"));
            let at = format!("{kind:?} {surface:?}: {receipt}");
            assert_eq!(receipt["resolved"], DECK, "{at}");
            assert_eq!(receipt["remaining"], 0, "{at}");
            assert_eq!(receipt["file"], true, "{at}");
            assert_eq!(receipt["kept"], "mine", "{at}");
            assert_eq!(receipt["kept_copy"], false, "{at}");
            assert!(receipt["draft"].is_null(), "the folder's: {at}");
            assert_eq!(receipt["note"], "The folder keeps its own file.", "{at}");
            assert_eq!(f.deck().as_deref(), Some(FOLDER_DECK), "{at}");
            assert_eq!(f.conflicts(), Vec::<String>::new(), "{at}");
            assert_eq!(f.kept(), vec![], "{at}");
        }
    }
    // In a domain that takes changes directly, the folder's file is what the
    // next share proposes.
    let f = fx(Kind::Direct).await;
    f.folder_conflict(Some(NEWER_DECK)).await;
    f.resolve(Surface::Path, Some("mine"), None).await.unwrap();
    let shared = f.share().await.unwrap();
    assert_eq!(f.proposed(&shared, DECK).as_deref(), Some(FOLDER_DECK));
}

/// Take theirs: the folder takes the team's bytes, and its own are kept first,
/// as bytes under the file's own extension.
#[tokio::test]
async fn take_theirs_settles_a_folder_file_and_keeps_a_copy_on_every_surface() {
    for kind in [Kind::Direct, Kind::Reviewing] {
        for surface in SURFACES {
            let f = fx(kind).await;
            f.folder_conflict(Some(NEWER_DECK)).await;
            let receipt = f
                .resolve(surface, Some("theirs"), None)
                .await
                .unwrap_or_else(|e| panic!("{kind:?} {surface:?}: {e}"));
            let at = format!("{kind:?} {surface:?}: {receipt}");
            assert_eq!(receipt["resolved"], DECK, "{at}");
            assert_eq!(receipt["remaining"], 0, "{at}");
            assert_eq!(receipt["file"], true, "{at}");
            assert_eq!(receipt["kept"], "theirs", "{at}");
            assert_eq!(receipt["kept_copy"], true, "{at}");
            assert!(receipt["draft"].is_null(), "the folder's: {at}");
            assert_eq!(
                receipt["note"], "The folder now holds the team's file.",
                "{at}"
            );
            assert_eq!(
                receipt["kept_note"],
                "A copy of the folder's file is kept. Find it in your kept list.",
                "{at}"
            );
            assert_eq!(f.deck().as_deref(), Some(NEWER_DECK), "{at}");
            assert_eq!(f.conflicts(), Vec::<String>::new(), "{at}");
            let kept = f.kept();
            assert_eq!(kept.len(), 1, "{at}");
            assert_eq!(kept[0].0, DECK, "{at}");
            assert_eq!(kept[0].2, FOLDER_DECK, "{at}");
            assert_eq!(
                kept[0].1.extension().and_then(|e| e.to_str()),
                Some("png"),
                "{at}"
            );
            // The machine owner is on this disk, so it is told where.
            assert_eq!(
                receipt["kept_file"].as_str().map(Path::new),
                Some(kept[0].1.as_path()),
                "{at}"
            );
        }
    }
}

/// The team deleted the file the folder had edited: taking theirs removes it,
/// and the folder's bytes are kept.
#[tokio::test]
async fn take_theirs_on_a_team_deletion_removes_the_file_and_keeps_a_copy() {
    for kind in [Kind::Direct, Kind::Reviewing] {
        let f = fx(kind).await;
        f.folder_conflict(None).await;
        let receipt = f
            .resolve(Surface::Path, Some("theirs"), None)
            .await
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        assert_eq!(receipt["kept_copy"], true, "{kind:?}: {receipt}");
        assert_eq!(f.deck(), None, "{kind:?}");
        assert_eq!(f.conflicts(), Vec::<String>::new(), "{kind:?}");
        let kept = f.kept();
        assert_eq!(kept.len(), 1, "{kind:?}");
        assert_eq!(kept[0].2, FOLDER_DECK, "{kind:?}");
    }
}

/// A file cannot be merged: the resolve says so, and nothing changes.
#[tokio::test]
async fn a_merge_of_a_folder_file_is_refused_on_every_surface() {
    for kind in [Kind::Direct, Kind::Reviewing] {
        for surface in SURFACES {
            let f = fx(kind).await;
            f.folder_conflict(Some(NEWER_DECK)).await;
            let err = f
                .resolve(surface, None, Some(b"merged bytes"))
                .await
                .unwrap_err();
            assert_eq!(
                err, "A file cannot be merged here. Keep one of the two.",
                "{kind:?} {surface:?}"
            );
            assert_eq!(
                f.deck().as_deref(),
                Some(FOLDER_DECK),
                "{kind:?} {surface:?}"
            );
            assert_eq!(
                f.conflicts(),
                vec![DECK.to_string()],
                "{kind:?} {surface:?}"
            );
        }
    }
}

/// The folder's bytes exist nowhere else, so when no copy can be kept the
/// take is refused and the folder keeps them.
#[tokio::test]
async fn a_failed_copy_refuses_the_take() {
    let f = fx(Kind::Direct).await;
    f.folder_conflict(Some(NEWER_DECK)).await;
    // A file where the kept folder goes, so nothing can be kept.
    std::fs::write(
        f.tmp.path().join(crystalline_core::kept::KEPT_DIR),
        b"in the way",
    )
    .unwrap();
    f.resolve(Surface::Path, Some("theirs"), None)
        .await
        .unwrap_err();
    assert_eq!(
        f.deck().as_deref(),
        Some(FOLDER_DECK),
        "the last copy stays"
    );
    assert_eq!(f.conflicts(), vec![DECK.to_string()]);
}

/// With no conflict at a file, a resolve says so and names the ones that
/// stand, in a domain that takes changes directly.
#[tokio::test]
async fn a_resolve_of_a_file_with_no_conflict_says_so() {
    let f = fx(Kind::Direct).await;
    let err = f
        .eng
        .origin_resolve("team", DECK, Some("mine"), None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Remote(RemoteError::ConflictNotFound { .. })
        ),
        "{err}"
    );
    assert_eq!(
        err.to_string(),
        format!("no conflict recorded for {DECK}; open conflicts: none")
    );
}

/// In a reviewing domain, a resolve by path settles the caller's own draft of
/// the file first and leaves the folder's conflict; the folder verb settles the
/// folder and leaves her draft.
#[tokio::test]
async fn by_path_her_draft_comes_first_and_the_folder_verb_reaches_the_folder() {
    let f = fx(Kind::Reviewing).await;
    let alice = Scope::User {
        account: "alice".to_string(),
        admin: false,
    };
    let her_deck = b"\x89PNG\r\n\x1a\n\x00alice's deck".to_vec();
    let written = f
        .eng
        .attachment_write_as("team", DECK, her_deck.clone(), &alice)
        .await
        .unwrap();
    assert!(written.draft, "an upload in review mode is a draft");
    f.folder_conflict(Some(NEWER_DECK)).await;
    assert_eq!(
        f.eng.draft_conflicts("team", "alice").await.unwrap().len(),
        1
    );

    let mine = f
        .eng
        .origin_resolve(
            "team",
            DECK,
            Some("mine"),
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(mine["draft"], true, "her draft: {mine}");
    assert_eq!(f.conflicts(), vec![DECK.to_string()], "the folder's stands");
    assert_eq!(f.deck().as_deref(), Some(FOLDER_DECK));

    let folder = f
        .eng
        .origin_resolve_folder(
            "team",
            DECK,
            Some("theirs"),
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert!(folder["draft"].is_null(), "the folder's: {folder}");
    // Only the machine owner is told where on this disk a copy is.
    assert!(folder["kept_file"].is_null(), "{folder}");
    assert_eq!(f.conflicts(), Vec::<String>::new());
    assert_eq!(f.deck().as_deref(), Some(NEWER_DECK));
    assert_eq!(
        f.eng
            .attachment_read_as("team", DECK, &alice)
            .await
            .unwrap()
            .0,
        her_deck,
        "her draft stands"
    );
}

/// In a reviewing domain, a caller with no draft of the file and no folder
/// conflict there is told both.
#[tokio::test]
async fn by_path_with_no_draft_and_no_folder_conflict_says_both() {
    let f = fx(Kind::Reviewing).await;
    let err = f
        .eng
        .origin_resolve(
            "team",
            DECK,
            Some("mine"),
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound(_)), "{err}");
    assert_eq!(
        err.to_string(),
        format!(
            "you are not drafting the file '{DECK}' in domain 'team', and no conflict stands \
             there either. This domain reviews changes, so what a resolution settles is your \
             own draft"
        )
    );
}
