//! Engine-level tests for GitHub origin collaboration: `origin_add`,
//! `origin_update`, `origin_status`, `origin_share`, `origin_share_preview`,
//! `origin_withdraw`, `origin_conflict_detail` and
//! `origin_resolve`, plus the gating matrix (the `github.enabled` refusal
//! and the read-only mode's asymmetric refusal). The embed-worker
//! scheduling checks also live here, beside the harness they share; they
//! include the non-origin `domain_add_local` one.
//!
//! Every test injects `crate::support::MockProvider` via `Engine::with_origin_provider`
//! and points origin state at a tempdir via `Engine::with_origins_dir`, so
//! nothing here reaches a network, a real GitHub repository or the real
//! machine's state directory.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::support::{CountingEmbedder, MockProvider, sha256_hex};
use crystalline_core::config::{GitHubConfig, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_remote::RemoteError;
use crystalline_remote::provider::{Feedback, ProposalState};
use crystalline_remote::state::{
    BranchKept, FeedbackItem, FeedbackKind, KeptKind, OriginState, Proposal, ProposalStatus,
    ProposedChange, ProposedFile, QueuedBranch, RetireWhy,
};
use crystalline_service::Scope;
use crystalline_service::engine::{EngineError, PreviewCredential, ShareActor};
use crystalline_service::params::{ReadParams, Readers, SearchParams};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::scope::DomainAccess;
use crystalline_service::{Engine, EnvOverlay};
use tokio::sync::Mutex;

fn config(github_enabled: bool) -> GlobalConfig {
    let mut cfg = GlobalConfig::default();
    if github_enabled {
        cfg.github = Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        });
    }
    cfg
}

pub(super) async fn engine_with(
    config_path: &Path,
    origins_dir: &Path,
    provider: Arc<MockProvider>,
    github_enabled: bool,
    read_only: bool,
) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        config(github_enabled),
        None,
        Some(config_path.to_path_buf()),
    )
    .with_read_only(read_only)
    .with_origin_provider(provider)
    .with_origins_dir(origins_dir.to_path_buf())
    // The overlay journal's own root, and it is named rather than defaulted:
    // an engine built without one refuses every draft write under the test
    // seam, on purpose, so a fixture that touches drafts has to say where the
    // mirror lives. It sits beside the origins directory, under the same
    // tempdir, so nothing here reaches the real machine's state directory.
    .with_state_dir(
        origins_dir
            .parent()
            .expect("the origins directory is inside the tempdir")
            .to_path_buf(),
    )
}

/// An engine whose only domains come from an environment overlay, wired to the
/// mock provider and a tempdir origins directory. GitHub is enabled so the
/// origin operations are not gated off.
async fn engine_with_env(
    config_path: &Path,
    origins_dir: &Path,
    provider: Arc<MockProvider>,
    env_vars: &[(&str, &str)],
) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    let overlay = EnvOverlay::from_vars(
        env_vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        config(true),
        None,
        Some(config_path.to_path_buf()),
    )
    .with_origin_provider(provider)
    .with_origins_dir(origins_dir.to_path_buf())
    .with_env_overlay(overlay)
}

/// The team's MANIFEST, carrying the team marker as every repository does
/// after its first share from 0.30.0 on, so a share here carries exactly the
/// changes a scenario makes. [`manifest_unmarked`] is the repository from
/// before.
pub(super) fn manifest() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

/// The team's MANIFEST before 0.30.0: no team marker yet.
fn manifest_unmarked() -> Vec<u8> {
    String::from_utf8(manifest())
        .unwrap()
        .replace("team: github\n", "")
        .into_bytes()
}

/// The same MANIFEST, declaring that this domain's generated folder listings
/// travel with it. The default is `local`, so a scenario whose subject needs a
/// listing in a share has to say so, the way a real team says it once in the
/// file all of its members hold.
fn manifest_sharing_indexes() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\ngenerated_indexes: shared\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

/// The same MANIFEST, declaring that a share of this domain commits straight
/// onto the connected branch instead of opening a proposal. The default is
/// `proposal`, so a scenario whose subject is the direct path has to say so,
/// the way a real team says it once in the file all of its members hold.
pub(super) fn manifest_sharing_direct() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\nsharing: direct\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

pub(super) fn engram(title: &str, permalink: &str, body: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
    )
    .into_bytes()
}

pub(super) fn commit_files(pairs: &[(&str, Vec<u8>)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(p, c)| (p.to_string(), c.clone()))
        .collect()
}

// --- gating matrix -----------------------------------------------------------

#[tokio::test]
async fn github_disabled_refuses_all_three_origin_operations() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        false,
        false,
    )
    .await;
    let root = tmp.path().join("root");

    let add_err = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(add_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{add_err}"
    );

    let update_err = eng
        .origin_update(None, &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(
        matches!(update_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{update_err}"
    );

    let status_err = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(
        matches!(status_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{status_err}"
    );
}

#[tokio::test]
async fn read_only_refuses_add_but_allows_update_and_status() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        true,
        true,
    )
    .await;
    let root = tmp.path().join("root");

    let add_err = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(add_err, EngineError::ReadOnly), "{add_err}");
    assert!(!root.exists(), "a refused add must not touch disk");

    // No origin domains are registered in this test, but the calls
    // themselves must not be refused for being read-only.
    let update = eng.origin_update(None, &Scope::Unrestricted).await.unwrap();
    assert_eq!(update["domains"].as_array().unwrap().len(), 0);
    assert_eq!(update["errors"].as_array().unwrap().len(), 0);

    let status = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["domains"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn github_disabled_refuses_share_withdraw_preview_and_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        false,
        false,
    )
    .await;

    let share_err = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(share_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{share_err}"
    );

    let withdraw_err = eng
        .origin_withdraw("brand", None, false, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(withdraw_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{withdraw_err}"
    );

    let preview_err = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(preview_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{preview_err}"
    );

    let resolve_err = eng
        .origin_resolve("brand", "notes/a.md", Some("mine"), None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(resolve_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{resolve_err}"
    );
}

#[tokio::test]
async fn read_only_refuses_share_withdraw_preview_and_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        true,
        true,
    )
    .await;

    // None of these need a registered domain: read-only refuses before the
    // domain is even resolved, exactly like `origin_add` above.
    let share_err = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(matches!(share_err, EngineError::ReadOnly), "{share_err}");

    let withdraw_err = eng
        .origin_withdraw("brand", None, false, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(withdraw_err, EngineError::ReadOnly),
        "{withdraw_err}"
    );

    let preview_err = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(preview_err, EngineError::ReadOnly),
        "{preview_err}"
    );

    let resolve_err = eng
        .origin_resolve("brand", "notes/a.md", Some("mine"), None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(resolve_err, EngineError::ReadOnly),
        "{resolve_err}"
    );
}

// --- origin_add ----------------------------------------------------------------

#[tokio::test]
async fn origin_add_creates_folder_registers_domain_and_indexes_engrams() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "shared knowledge about turbines"),
        ),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;

    let result = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap();

    assert_eq!(result["domain"], "brand-knowledge");
    assert_eq!(result["engrams"], 2);
    assert_eq!(result["base_commit"], commit);
    assert_eq!(result["root"], root.display().to_string());

    // Files landed on disk.
    assert!(root.join("MANIFEST.md").exists());
    assert!(root.join("notes/alpha.md").exists());

    // Registered in the in-memory config and persisted to the config file.
    assert!(eng.config().domains.contains_key("brand-knowledge"));
    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let entry = on_disk.domains.get("brand-knowledge").unwrap();
    let origin_cfg = entry.origin.as_ref().expect("origin config");
    assert_eq!(origin_cfg.repo, "acme/brand-knowledge");
    assert_eq!(origin_cfg.branch(), "main");
    assert_eq!(
        origin_cfg.branch.as_deref(),
        Some("main"),
        "the repository default the connect resolved is recorded, not left implicit"
    );
    assert_eq!(entry.file_path().as_deref(), Some(root.as_path()));

    // Indexed: readable through the engine's own read path.
    let read = eng
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("brand-knowledge".to_string()),
                share_link: None,
                include_images: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("shared knowledge about turbines")
    );
}

/// A fresh connect of the team repository at `acme/brand-knowledge`, whose
/// MANIFEST carries no team marker yet (a repository from before 0.30.0).
async fn connected_unmarked(tmp: &Path) -> (Engine, Arc<MockProvider>, std::path::PathBuf) {
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_unmarked())]));
    mock.set_branch("main", &commit);
    let root = tmp.join("brand-knowledge");
    let eng = engine_with(
        &tmp.join("config.yaml"),
        &tmp.join("origins"),
        mock.clone(),
        true,
        false,
    )
    .await;
    let result = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
    // A connect never writes into the team's MANIFEST: the folder is clean.
    assert_eq!(result["local_changes"], 0, "{result}");
    assert_eq!(
        std::fs::read(root.join("MANIFEST.md")).unwrap(),
        manifest_unmarked(),
        "the connect leaves the MANIFEST as the team wrote it"
    );
    (eng, mock, root)
}

fn shared_manifest(mock: &MockProvider, result: &serde_json::Value) -> String {
    let branch = result["branch"].as_str().expect("a proposal branch");
    let commit = mock.branch_commit(branch).expect("the share made a branch");
    String::from_utf8(mock.commit_file(&commit, "MANIFEST.md").unwrap()).unwrap()
}

/// The first share of everything from a connected instance carries the team
/// marker: the MANIFEST gains `team: github` right after its title, in the
/// folder and in the proposal, beside the change that was shared.
#[tokio::test]
async fn a_share_of_everything_carries_the_team_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, root) = connected_unmarked(tmp.path()).await;
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("notes/new.md"), engram("New", "new", "fresh")).unwrap();
    eng.sync(Some("brand-knowledge")).await.unwrap();
    let preview = eng
        .origin_share_preview(
            "brand-knowledge",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    assert_eq!(preview["team_marker"], team_marker(), "{preview}");
    assert_eq!(
        std::fs::read(root.join("MANIFEST.md")).unwrap(),
        manifest_unmarked(),
        "a preview writes nothing"
    );
    let result = eng
        .origin_share("brand-knowledge", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(result["added"], serde_json::json!(["notes/new.md"]));
    assert_eq!(result["updated"], serde_json::json!(["MANIFEST.md"]));
    assert_eq!(result["team_marker"], team_marker(), "{result}");
    let shared = shared_manifest(&mock, &result);
    assert!(shared.contains("title: Team\nteam: github\n"), "{shared}");
    let local = std::fs::read_to_string(root.join("MANIFEST.md")).unwrap();
    assert_eq!(local, shared, "the folder holds what was shared");
    let body = mock
        .proposal_body(result["number"].as_u64().unwrap())
        .unwrap();
    assert!(
        body.contains("Domain: brand-knowledge"),
        "the generated body stays: {body}"
    );
    assert!(
        body.trim_end().ends_with(MARKER_NOTE),
        "the body says why the MANIFEST changed: {body}"
    );
}

/// What the receipt and the preview say about the marker.
fn team_marker() -> serde_json::Value {
    serde_json::json!({
        "path": "MANIFEST.md",
        "reason": "marks this repository as a team domain; added once"
    })
}

/// The line a pull request body or a commit message ends with when the
/// share added the marker.
const MARKER_NOTE: &str = "MANIFEST.md gains `team: github`. Crystalline added it: it marks this repository as a team domain, and it is added once.";

/// Two members of a team whose repository has no marker yet: the first one's
/// share carries it and is merged, and the second one, who has not pulled
/// since, shares without a conflict. Both instances write the same bytes
/// for the marker, so the inline pull of the second share takes the team's
/// MANIFEST as it is.
#[tokio::test]
async fn a_second_member_shares_the_marker_without_a_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_unmarked())]));
    mock.set_branch("main", &commit);
    let mut members = Vec::new();
    for who in ["ann", "ben"] {
        let dir = tmp.path().join(who);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("brand-knowledge");
        let eng = engine_with(
            &dir.join("config.yaml"),
            &dir.join("origins"),
            mock.clone(),
            true,
            false,
        )
        .await;
        eng.origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(
            root.join(format!("notes/{who}.md")),
            engram(who, who, "a change of my own"),
        )
        .unwrap();
        eng.sync(Some("brand-knowledge")).await.unwrap();
        members.push((eng, root));
    }

    let (ann, _) = &members[0];
    let first = ann
        .origin_share("brand-knowledge", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(first["outcome"], "proposed", "{first}");
    let merged = mock
        .branch_commit(first["branch"].as_str().unwrap())
        .unwrap();
    mock.set_branch("main", &merged);
    mock.set_proposal_state(first["number"].as_u64().unwrap(), ProposalState::Merged);

    let (ben, ben_root) = &members[1];
    let second = ben
        .origin_share("brand-knowledge", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(second["outcome"], "proposed", "{second}");
    assert_eq!(
        second["added"],
        serde_json::json!(["notes/ben.md"]),
        "{second}"
    );
    assert_eq!(
        second["updated"],
        serde_json::json!([]),
        "the team's MANIFEST already says it: {second}"
    );
    let local = std::fs::read_to_string(ben_root.join("MANIFEST.md")).unwrap();
    assert_eq!(
        crystalline_core::team_of_source(&local),
        Some(crystalline_core::TeamForge::GitHub)
    );
}

/// Pressing share with nothing to share stays a no-op: the marker alone
/// never opens a proposal.
#[tokio::test]
async fn a_share_with_nothing_to_share_writes_no_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root) = connected_unmarked(tmp.path()).await;
    let result = eng
        .origin_share("brand-knowledge", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "nothing_to_share", "{result}");
    assert!(result.get("team_marker").is_none(), "{result}");
    assert_eq!(
        std::fs::read(root.join("MANIFEST.md")).unwrap(),
        manifest_unmarked()
    );
}

/// A narrowed share carries the marker as one more file when the MANIFEST
/// has no other unshared change, so exactly one line rides along. A MANIFEST
/// holding unshared edits nobody chose is left alone for a share that
/// includes it.
#[tokio::test]
async fn a_narrowed_share_adds_the_marker_only_to_an_unchanged_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, root) = connected_unmarked(tmp.path()).await;
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("notes/a.md"), engram("A", "a", "mine")).unwrap();
    std::fs::write(root.join("notes/b.md"), engram("B", "b", "theirs")).unwrap();
    eng.sync(Some("brand-knowledge")).await.unwrap();
    let result = eng
        .origin_share(
            "brand-knowledge",
            None,
            None,
            None,
            Some(&["notes/a.md".to_string()]),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(
        result["added"],
        serde_json::json!(["notes/a.md"]),
        "{result}"
    );
    assert_eq!(result["updated"], serde_json::json!(["MANIFEST.md"]));
    assert_eq!(result["team_marker"], team_marker(), "{result}");
    let shared = shared_manifest(&mock, &result);
    let before = String::from_utf8(manifest_unmarked()).unwrap();
    assert_eq!(
        shared,
        before.replace("title: Team\n", "title: Team\nteam: github\n"),
        "one line and nothing else"
    );

    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root) = connected_unmarked(tmp.path()).await;
    let edited = before.replace("- always\n", "- always\n- and on Fridays\n");
    std::fs::write(root.join("MANIFEST.md"), &edited).unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("notes/a.md"), engram("A", "a", "mine")).unwrap();
    eng.sync(Some("brand-knowledge")).await.unwrap();
    let result = eng
        .origin_share(
            "brand-knowledge",
            None,
            None,
            None,
            Some(&["notes/a.md".to_string()]),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(result["updated"], serde_json::json!([]), "{result}");
    assert!(result.get("team_marker").is_none(), "{result}");
    assert_eq!(
        std::fs::read_to_string(root.join("MANIFEST.md")).unwrap(),
        edited,
        "the unchosen MANIFEST edit is left as it was"
    );
}

/// A connect over a configuration file that does not parse is refused before
/// anything is downloaded: the registration at its end would refuse to save
/// over the broken file, after the files and the origin state were written.
/// Once the file parses again, the same connect lands.
#[tokio::test]
async fn origin_add_over_a_config_that_does_not_parse_downloads_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "shared knowledge about turbines"),
        ),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    let broken = "domains: [unclosed\n";
    std::fs::write(&config_path, broken).unwrap();

    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("brand"),
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("left as it is"), "{err}");
    assert_eq!(mock.tarball_calls(), 0, "nothing was downloaded");
    assert!(!root.exists(), "no files were written");
    assert!(
        !origins_dir.join("brand").exists(),
        "no origin state was written"
    );
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), broken);

    std::fs::remove_file(&config_path).unwrap();
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    assert!(root.join("MANIFEST.md").is_file());
    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    assert!(on_disk.domains.contains_key("brand"));
}

/// Connecting a repository as `brand` heals `[[brand:Alpha]]` in another
/// domain at once: the link waited pending while nothing answered to the
/// prefix, and the connect binds the rows spelled with the new name.
#[tokio::test]
async fn origin_add_heals_a_pending_link_that_names_the_new_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "brand alpha")),
    ]));
    mock.set_branch("main", &commit);
    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;

    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("src.md"),
        engram("Src", "src", "See [[brand:Alpha]]."),
    )
    .unwrap();
    eng.domain_add_local(Some("home"), Some(home.to_str().unwrap()))
        .await
        .unwrap();
    let src_resolved = || async {
        let read = eng
            .read_engram(
                &ReadParams {
                    identifier: "src".to_string(),
                    domain: Some("home".to_string()),
                    share_link: None,
                    include_images: None,
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap();
        read["links"][0]["resolved"].as_bool().unwrap()
    };
    assert!(!src_resolved().await, "`brand` names no domain yet");

    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(tmp.path().join("brand").to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    assert!(src_resolved().await, "healed by the connect");
}

#[tokio::test]
async fn origin_add_reports_stage_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "shared knowledge about turbines"),
        ),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;

    let seen: Arc<std::sync::Mutex<Vec<(u64, u64, String)>>> = Arc::default();
    let cb: crystalline_service::engine::OriginProgress = {
        let seen = seen.clone();
        Arc::new(move |step, total, msg: &str| {
            seen.lock().unwrap().push((step, total, msg.to_string()));
        })
    };
    eng.origin_add_with_progress(
        "acme/brand-knowledge",
        None,
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
        Some(cb),
    )
    .await
    .unwrap();
    let seen = seen.lock().unwrap();
    let steps: Vec<u64> = seen.iter().map(|(s, _, _)| *s).collect();
    assert_eq!(
        steps,
        vec![1, 2, 3, 4],
        "one strictly increasing step per stage"
    );
    assert!(seen.iter().all(|(_, total, _)| *total == 4));
    assert!(seen[0].2.contains("acme/brand-knowledge"));
}

#[tokio::test]
async fn origin_add_connects_a_registered_domain_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "the team version"),
        ),
        (
            "notes/beta.md",
            engram("Beta", "beta", "only upstream has this"),
        ),
    ]));
    mock.set_branch("main", &commit);

    // A plain file domain, already registered and on disk, whose alpha
    // differs from upstream and which has no beta at all.
    let root = tmp.path().join("brand-knowledge");
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        root.join("notes/alpha.md"),
        engram("Alpha", "alpha", "my local take"),
    )
    .unwrap();

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let store = TursoStore::open_in_memory().await.unwrap();
    let mut cfg = config(true);
    cfg.domains.insert(
        "brand".to_string(),
        crystalline_core::config::DomainEntry {
            kind: crystalline_core::config::DomainKind::File,
            path: Some(root.clone()),
            origin: None,
            provision: None,
            review: None,
            ..Default::default()
        },
    );
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    )
    .with_origin_provider(mock)
    .with_origins_dir(origins_dir);

    let result = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("brand"),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("a registered origin-less domain connects in place");

    assert_eq!(result["domain"], "brand");
    assert_eq!(result["root"], root.display().to_string());
    assert_eq!(result["adopted"], true);
    assert_eq!(result["local_changes"], 1, "the differing alpha");

    // Local knowledge kept, missing upstream knowledge arrived.
    let alpha = std::fs::read_to_string(root.join("notes/alpha.md")).unwrap();
    assert!(alpha.contains("my local take"), "{alpha}");
    assert!(root.join("notes/beta.md").exists());

    // The entry kept its root and gained the origin, persisted to disk.
    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let entry = on_disk.domains.get("brand").unwrap();
    assert_eq!(entry.origin.as_ref().unwrap().repo, "acme/brand-knowledge");
    assert_eq!(entry.file_path().as_deref(), Some(root.as_path()));
}

/// Connecting an origin to a registered, origin-less file domain attaches it
/// in place and keeps the decisions already made about that domain: whether
/// its artifacts are provisioned, and whether it reviews changes.
#[tokio::test]
async fn origin_add_adopting_a_registered_domain_keeps_its_provision_and_review() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let root = tmp.path().join("brand-knowledge");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let store = TursoStore::open_in_memory().await.unwrap();
    let mut cfg = config(true);
    cfg.domains.insert(
        "brand".to_string(),
        crystalline_core::config::DomainEntry {
            kind: crystalline_core::config::DomainKind::File,
            path: Some(root.clone()),
            origin: None,
            provision: Some(false),
            review: Some(crystalline_core::config::ReviewMode::Overlay),
            ..Default::default()
        },
    );
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    )
    .with_origin_provider(mock)
    .with_origins_dir(origins_dir);

    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        None,
        None,
    )
    .await
    .expect("a registered origin-less domain connects in place");

    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let entry = on_disk.domains.get("brand").unwrap();
    assert!(entry.origin.is_some());
    assert_eq!(
        entry.provision,
        Some(false),
        "the provisioning decision stays"
    );
    assert_eq!(
        entry.review,
        Some(crystalline_core::config::ReviewMode::Overlay),
        "review mode stays"
    );
}

/// A team domain's derived or explicit name is checked before the folder is
/// derived from it.
#[tokio::test]
async fn origin_add_refuses_a_bad_explicit_name() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);
    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;

    // A folder under the tempdir, so the red run (which accepts `a/b` and
    // downloads) never writes into the real home folder.
    let folder = tmp.path().join("team");
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("a/b"),
            None,
            None,
            Some(folder.to_str().unwrap()),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    assert!(err.to_string().contains("cannot name a domain"), "{err}");
}

#[tokio::test]
async fn origin_add_on_a_registered_domain_refuses_a_different_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let root = tmp.path().join("brand-knowledge");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let store = TursoStore::open_in_memory().await.unwrap();
    let mut cfg = config(true);
    cfg.domains.insert(
        "brand".to_string(),
        crystalline_core::config::DomainEntry {
            kind: crystalline_core::config::DomainKind::File,
            path: Some(root.clone()),
            origin: None,
            provision: None,
            review: None,
            ..Default::default()
        },
    );
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    )
    .with_origin_provider(mock)
    .with_origins_dir(origins_dir);

    let elsewhere = tmp.path().join("elsewhere");
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("brand"),
            None,
            None,
            Some(elsewhere.to_str().unwrap()),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(!elsewhere.exists(), "a refused add must not touch disk");
}

#[tokio::test]
async fn origin_add_refuses_a_domain_name_already_registered() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;

    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    let other_root = tmp.path().join("other");
    let err = eng
        .origin_add(
            "acme/other-repo",
            Some("brand"),
            None,
            None,
            Some(other_root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(!other_root.exists(), "a refused add must not touch disk");
}

#[tokio::test]
async fn origin_add_retry_of_the_same_connect_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "shared knowledge about turbines"),
        ),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;
    let root_str = root.to_str().unwrap();

    // connect once
    let first = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root_str),
            None,
        )
        .await
        .unwrap();
    // retry with identical arguments
    let second = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root_str),
            None,
        )
        .await
        .unwrap();
    assert_eq!(second["already_connected"], serde_json::json!(true));
    assert_eq!(second["base_commit"], first["base_commit"]);
    assert_eq!(second["domain"], first["domain"]);
    assert_eq!(second["engrams"], first["engrams"]);
}

#[tokio::test]
async fn origin_add_retry_treats_absent_branch_as_main() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;
    let root_str = root.to_str().unwrap();

    // connect with branch None, retry with Some("main"): both mean main
    eng.origin_add(
        "acme/brand-knowledge",
        None,
        None,
        None,
        Some(root_str),
        None,
    )
    .await
    .unwrap();
    let second = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            Some("main"),
            Some(root_str),
            None,
        )
        .await
        .unwrap();
    assert_eq!(second["already_connected"], serde_json::json!(true));
}

#[tokio::test]
async fn origin_add_without_a_branch_tracks_the_repository_default() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    mock.set_default_branch("trunk");
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("trunk", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;
    let root_str = root.to_str().unwrap();

    eng.origin_add(
        "acme/brand-knowledge",
        None,
        None,
        None,
        Some(root_str),
        None,
    )
    .await
    .unwrap();
    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let origin_cfg = on_disk.domains["brand-knowledge"].origin.clone().unwrap();
    assert_eq!(origin_cfg.branch.as_deref(), Some("trunk"));

    // A retry without a branch is the same connect, whatever was resolved.
    let again = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root_str),
            None,
        )
        .await
        .unwrap();
    assert_eq!(again["already_connected"], serde_json::json!(true));

    // Naming another branch is a different connect.
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            Some("main"),
            Some(root_str),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
}

#[tokio::test]
async fn origin_add_without_a_branch_refuses_when_the_default_cannot_be_read() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    mock.fail_default_branch();
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;

    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("--branch"), "{err}");
    assert!(
        !eng.config().domains.contains_key("brand-knowledge"),
        "never registered on a guessed branch"
    );
}

#[tokio::test]
async fn origin_add_retry_with_a_different_origin_still_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;
    let root_str = root.to_str().unwrap();

    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root_str),
        None,
    )
    .await
    .unwrap();
    // different repo under the same domain name
    let err = eng
        .origin_add(
            "acme/other-knowledge",
            Some("brand"),
            None,
            None,
            Some(root_str),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(err.to_string().contains("already connected"), "{err}");
    // same repo, different branch
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("brand"),
            None,
            Some("dev"),
            Some(root_str),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    // same repo, different subpath
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("brand"),
            Some("docs"),
            None,
            Some(root_str),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_add_racing_retry_waits_under_the_lock_and_never_redownloads() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "shared knowledge about turbines"),
        ),
    ]));
    mock.set_branch("main", &commit);

    // Park every tarball download on a gate: the first connect stalls
    // mid-download, holding the origin lock, while an identical retry races
    // in behind it.
    let gate = mock.block_tarball();

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = Arc::new(engine_with(&config_path, &origins_dir, mock.clone(), true, false).await);
    let root_str = root.to_str().unwrap().to_string();

    // Connect A: passes the pre-lock guard (no origin on file yet), takes the
    // origin lock and blocks in tarball with the config not yet persisted.
    let a = {
        let eng = eng.clone();
        let root_str = root_str.clone();
        tokio::spawn(async move {
            eng.origin_add(
                "acme/brand-knowledge",
                None,
                None,
                None,
                Some(&root_str),
                None,
            )
            .await
        })
    };
    // Give A a moment to reach the gate.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Connect B: identical arguments. It also passes the pre-lock guard
    // (still no origin persisted) and then queues on the origin lock A holds.
    let b = {
        let eng = eng.clone();
        let root_str = root_str.clone();
        tokio::spawn(async move {
            eng.origin_add(
                "acme/brand-knowledge",
                None,
                None,
                None,
                Some(&root_str),
                None,
            )
            .await
        })
    };
    // Give B a moment to reach and block on the origin lock.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Release the download; A finishes and persists its origin, then B wakes,
    // re-reads the config under the lock and must answer idempotently.
    gate.send(true).unwrap();

    let a = a.await.unwrap().unwrap();
    let b = b.await.unwrap().unwrap();

    // A is the fresh connect.
    assert!(
        a.get("already_connected").is_none(),
        "A is the fresh connect: {a}"
    );
    assert_eq!(a["domain"], "brand-knowledge");
    assert_eq!(a["engrams"], 2);

    // B saw A's just-persisted origin under the lock and answered
    // already-connected instead of re-running the whole connect.
    assert_eq!(b["already_connected"], serde_json::json!(true), "{b}");
    assert_eq!(b["domain"], a["domain"]);
    assert_eq!(b["base_commit"], a["base_commit"]);
    assert_eq!(b["engrams"], a["engrams"]);

    // The whole repo was downloaded exactly once: the racing retry never
    // re-downloaded it.
    assert_eq!(
        mock.tarball_calls(),
        1,
        "the racing retry must not re-download the repo"
    );
}

#[tokio::test]
async fn origin_add_schedules_embedding_on_the_worker_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "alpha body")),
    ]));
    mock.set_branch("main", &commit);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        true,
        false,
    )
    .await
    .with_embed_channel(tx);
    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        None,
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "origin_add must schedule a background embed instead of embedding inline"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embed_worker_runs_the_scheduled_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "alpha body")),
    ]));
    mock.set_branch("main", &commit);
    let root = tmp.path().join("brand-knowledge");

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = Arc::new(
        engine_with(
            &tmp.path().join("config.yaml"),
            &tmp.path().join("origins"),
            mock,
            true,
            false,
        )
        .await
        .with_embed_channel(tx),
    );
    let embedder = Arc::new(CountingEmbedder::new());
    eng.set_provider(embedder.clone());
    tokio::spawn(crystalline_service::engine::run_embed_worker(
        eng.clone(),
        rx,
    ));
    eng.origin_add(
        "acme/brand-knowledge",
        None,
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    // Poll up to 2 s for the worker to run the pass.
    for _ in 0..200 {
        if embedder.calls.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the embed worker never ran the scheduled pass");
}

// --- origin_update ---------------------------------------------------------

#[tokio::test]
async fn origin_update_applies_an_upstream_edit_and_the_index_reflects_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "version one")),
    ]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version two, revised upstream"),
        ),
    ]));
    mock.set_branch("main", &c2);

    let result = eng
        .origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    let domains = result["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "brand");
    assert_eq!(domains[0]["up_to_date"], false);
    assert_eq!(domains[0]["applied"][0], "notes/alpha.md");
    assert_eq!(result["errors"].as_array().unwrap().len(), 0);

    // The working tree carries the upstream edit.
    let content = std::fs::read_to_string(root.join("notes/alpha.md")).unwrap();
    assert!(content.contains("version two"));

    // The index reflects it too.
    let hits = eng
        .search_engrams(
            &SearchParams {
                query: Some("revised upstream".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(hits["total"], 1);
}

#[tokio::test]
async fn origin_update_schedules_embedding_on_the_worker_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "version one")),
    ]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false)
        .await
        .with_embed_channel(tx);
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    // Drain the connect's own scheduled embed so the assertion below sees
    // only the update's pass.
    while rx.try_recv().is_ok() {}

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version two, revised upstream"),
        ),
    ]));
    mock.set_branch("main", &c2);

    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "origin_update must schedule a background embed instead of embedding inline"
    );
}

#[tokio::test]
async fn origin_update_named_domain_with_no_origin_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        true,
        false,
    )
    .await;

    let err = eng
        .origin_update(Some("nope"), &Scope::Unrestricted)
        .await
        .unwrap_err();
    // Unregistered entirely, since none was ever added.
    assert!(matches!(err, EngineError::UnknownDomain { .. }), "{err}");
}

// --- env-defined domains -----------------------------------------------------

#[tokio::test]
async fn origin_update_bootstraps_an_env_domain_then_plain_pulls() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "shared knowledge about turbines"),
        ),
    ]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("team");
    let eng = engine_with_env(
        &config_path,
        &origins_dir,
        mock.clone(),
        &[
            ("CRYSTALLINE_DOMAIN_TEAM", root.to_str().unwrap()),
            ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/brand-knowledge"),
        ],
    )
    .await;

    // First update bootstraps: the missing-state env domain subscribes.
    let result = eng
        .origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    let domains = result["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "team");
    assert_eq!(domains[0]["bootstrapped"], true);
    assert_eq!(domains[0]["engrams"], 2);
    assert_eq!(domains[0]["base_commit"], c1);
    assert_eq!(result["errors"].as_array().unwrap().len(), 0);

    // Files landed on disk and origin state now exists.
    assert!(root.join("MANIFEST.md").exists());
    assert!(root.join("notes/alpha.md").exists());
    assert!(
        OriginState::load(&origins_dir.join("team"))
            .unwrap()
            .is_some(),
        "origin state written on bootstrap"
    );

    // Indexed and searchable through the engine's own read path.
    let hits = eng
        .search_engrams(
            &SearchParams {
                query: Some("turbines".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(hits["total"], 1);

    // Second update is a plain pull now that state is present: nothing new
    // upstream, so it is up to date and no longer marked bootstrapped.
    let result = eng
        .origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    let domains = result["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert!(
        domains[0]["bootstrapped"].is_null(),
        "the second pull does not bootstrap"
    );
    assert_eq!(domains[0]["up_to_date"], true);
}

#[tokio::test]
async fn origin_add_on_an_env_defined_name_names_the_variable() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let team_root = tmp.path().join("team");
    let eng = engine_with_env(
        &config_path,
        &origins_dir,
        mock,
        &[("CRYSTALLINE_DOMAIN_TEAM", team_root.to_str().unwrap())],
    )
    .await;

    let other_root = tmp.path().join("other");
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("team"),
            None,
            None,
            Some(other_root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap_err();
    match err {
        EngineError::Conflict(msg) => {
            assert!(msg.contains("CRYSTALLINE_DOMAIN_TEAM"), "{msg}")
        }
        other => panic!("expected Conflict naming the variable, got {other}"),
    }
    assert!(!other_root.exists(), "a refused add must not touch disk");
}

#[tokio::test]
async fn origin_update_one_domain_failing_does_not_abort_the_others() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let good_commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("good-branch", &good_commit);
    let bad_commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("bad-branch", &bad_commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let good_root = tmp.path().join("good");
    let bad_root = tmp.path().join("bad");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;

    eng.origin_add(
        "acme/good",
        Some("good"),
        None,
        Some("good-branch"),
        Some(good_root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    eng.origin_add(
        "acme/bad",
        Some("bad"),
        None,
        Some("bad-branch"),
        Some(bad_root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    // Corrupt "bad"'s origin state so its pull fails (simulating an
    // unavailable origin) without touching "good".
    std::fs::remove_file(origins_dir.join("bad").join("state.json")).unwrap();

    let good_commit_2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("good-branch", &good_commit_2);

    let result = eng.origin_update(None, &Scope::Unrestricted).await.unwrap();
    let domains = result["domains"].as_array().unwrap();
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{result}");
    assert_eq!(domains[0]["domain"], "good");
    assert_eq!(errors.len(), 1, "{result}");
    assert_eq!(errors[0]["domain"], "bad");
    assert!(
        errors[0]["error"]
            .as_str()
            .unwrap()
            .contains("origin state")
    );

    // The healthy domain still applied its upstream change.
    assert!(good_root.join("notes/new.md").exists());
}

#[tokio::test]
async fn origin_update_reports_a_proposal_transition_with_its_url_and_title() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    // Record an open share proposal directly in the domain's origin state, as
    // if it had been opened by a previous share (sharing itself is a later
    // task); `origin_update`'s pull refreshes it against the provider below.
    let state_dir = origins_dir.join("brand");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.proposals.push(Proposal {
        number: 7,
        url: "https://github.com/acme/brand-knowledge/pull/7".to_string(),
        branch: "share/glossary".to_string(),
        title: "Share glossary edits".to_string(),
        created_at: chrono::Utc::now(),
        status: ProposalStatus::Open,
        files: vec![],
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
        author_login: None,
    });
    state.save(&state_dir).unwrap();
    mock.set_proposal_state(7, ProposalState::Merged);

    // Move the branch so `pull` takes the "changed" path (which refreshes
    // proposals) rather than short-circuiting as up to date.
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("main", &c2);

    let result = eng
        .origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    let domains = result["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{result}");
    let proposals = domains[0]["proposals"].as_array().unwrap();
    assert_eq!(proposals.len(), 1, "{result}");
    assert_eq!(proposals[0]["number"], 7);
    assert_eq!(proposals[0]["status"], "Merged");
    assert_eq!(
        proposals[0]["url"],
        "https://github.com/acme/brand-knowledge/pull/7"
    );
    assert_eq!(proposals[0]["title"], "Share glossary edits");

    // The merged proposal moved from `proposals` to `history` on disk.
    let reloaded = OriginState::load(&state_dir).unwrap().unwrap();
    assert!(reloaded.proposals.iter().all(|p| p.number != 7));
    assert!(reloaded.history.iter().any(|p| p.number == 7));
}

// --- origin_status -----------------------------------------------------------

#[tokio::test]
async fn origin_status_reports_behind_and_connection() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    let status = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["connection"]["connected"], true);
    assert_eq!(status["connection"]["user"], "mock-user");
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "brand");
    assert_eq!(domains[0]["repo"], "acme/brand-knowledge");
    assert_eq!(domains[0]["behind"], false);
    assert_eq!(
        domains[0]["local_changes"], 0,
        "connecting as `brand` writes nothing into the team's MANIFEST"
    );

    // A local edit shows up as "ahead" (a local change against the base).
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/local.md"),
        engram("Local", "local", "not shared yet"),
    )
    .unwrap();
    let status_local = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status_local["domains"][0]["local_changes"], 1);

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("main", &c2);

    let status2 = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domains2 = status2["domains"].as_array().unwrap();
    assert_eq!(domains2[0]["behind"], true);
}

/// A kept entry reaches the status JSON with why its share was retired, what
/// kept it and a null reason when the forge gave none.
#[tokio::test]
async fn origin_status_names_why_and_what_kept_a_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &c1);
    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    let state_dir = origins_dir.join("brand");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.retire_queue.push(QueuedBranch {
        number: 3,
        branch: "crystalline/share-1".to_string(),
        onto: "main".to_string(),
        why: RetireWhy::Withdrawn,
        kept: Some(BranchKept {
            blocked_by: Some(70),
            reason: String::new(),
            kind: KeptKind::Head,
        }),
    });
    state.save(&state_dir).unwrap();

    let status = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["kept_branches"],
        serde_json::json!([{
            "branch": "crystalline/share-1",
            "number": 3,
            "onto": "main",
            "why": "withdrawn",
            "kind": "head",
            "blocked_by": 70,
            "reason": null,
            "message": "Branch crystalline/share-1 is kept: pull request #70 comes from it.",
        }])
    );
}

/// The keys one domain entry carries when nobody asked for detail. Pinned as a
/// list rather than spot-checked so an accidental `detail: null` - a key that
/// costs every reader something and says nothing - fails here.
const STATUS_KEYS_WITHOUT_DETAIL: [&str; 21] = [
    "base_commit",
    "behind",
    "branch",
    "conflicts",
    "declined_proposals",
    "direct_refused",
    "direct_shares",
    "domain",
    "kept_branches",
    "last_checked",
    "local_changes",
    "merged_unconsumed",
    "open_proposals",
    "probe_error",
    "repair_pending",
    "repo",
    "sharing",
    "skipped_large",
    "stack_link_pending",
    "stack_number",
    "stack_wedged",
];

/// A status asked for detail names the unshared work; a status not asked for it
/// is the payload it always was.
///
/// The delta is the one that misled a reader: something added, something
/// modified, something DELETED, and folder listings riding along. The deletion
/// is the assertion that earns the test, because it is the change kind no scan
/// of the filesystem can find - the file is not there to be found.
#[tokio::test]
async fn origin_status_detail_names_the_changes_and_the_default_still_only_counts_them() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest_sharing_indexes()),
        ("notes/edit.md", engram("Edit", "edit", "the team's copy")),
        ("notes/gone.md", engram("Gone", "gone", "the team's copy")),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    // One of each kind, plus two refreshed folder listings that ride along.
    std::fs::write(
        root.join("notes/added.md"),
        engram("Added", "added", "written here, never shared"),
    )
    .unwrap();
    std::fs::write(
        root.join("notes/edit.md"),
        engram("Edit", "edit", "edited here since the pull"),
    )
    .unwrap();
    std::fs::remove_file(root.join("notes/gone.md")).unwrap();
    std::fs::write(root.join("index.md"), b"# listing\n").unwrap();
    std::fs::write(root.join("notes/index.md"), b"# listing\n").unwrap();

    let counted = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &counted["domains"][0];
    assert_eq!(entry["local_changes"], 3, "{counted}");
    let mut keys: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys, STATUS_KEYS_WITHOUT_DETAIL,
        "a status nobody asked detail of is the payload it always was: {entry}"
    );

    let named = eng
        .origin_status(Some("brand"), true, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &named["domains"][0];
    assert_eq!(
        entry["local_changes"], 3,
        "the bare count is unchanged by asking: {entry}"
    );
    let detail = &entry["detail"];
    assert_eq!(detail["added"], serde_json::json!(["notes/added.md"]));
    assert_eq!(detail["modified"], serde_json::json!(["notes/edit.md"]));
    assert_eq!(
        detail["deleted"],
        serde_json::json!(["notes/gone.md"]),
        "the deleted file is on no filesystem: this is the only surface that can name it"
    );
    assert_eq!(
        detail["generated_indexes"],
        serde_json::json!(2),
        "the listings ride along as one number and are named nowhere: {detail}"
    );
    let named_count: usize = ["added", "modified", "deleted"]
        .iter()
        .map(|key| detail[*key].as_array().unwrap().len())
        .sum();
    assert_eq!(
        named_count,
        entry["local_changes"].as_u64().unwrap() as usize,
        "the three arrays sum to the count: {entry}"
    );
}

/// Offline is exactly when a caller cannot look the change list up anywhere
/// else, so the probe-free retry carries the detail too.
#[tokio::test]
async fn origin_status_detail_survives_an_offline_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/gone.md", engram("Gone", "gone", "the team's copy")),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    std::fs::remove_file(root.join("notes/gone.md")).unwrap();

    mock.fail_branch_head_offline("main");

    let status = eng
        .origin_status(Some("brand"), true, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &status["domains"][0];
    assert!(
        entry["probe_error"].is_string(),
        "this is the probe-free retry arm: {entry}"
    );
    assert_eq!(
        entry["detail"]["deleted"],
        serde_json::json!(["notes/gone.md"]),
        "a status that degraded to local state still names what the tree owes: {entry}"
    );
}

#[tokio::test]
async fn origin_status_with_no_domain_reports_every_origin_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    let status = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "brand");
}

#[tokio::test]
async fn origin_status_survives_a_live_offline_probe_for_a_connected_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    // A local edit so `local_changes` reports something real, not just a
    // default zero.
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/local.md"),
        engram("Local", "local", "not shared yet"),
    )
    .unwrap();

    // The GitHub connection (the mock provider override) is still present -
    // this is a live network outage, not a missing token - but the probe
    // itself cannot reach GitHub.
    mock.fail_branch_head_offline("main");

    let status = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["errors"].as_array().unwrap().len(),
        0,
        "an offline probe must never hard-fail origin_status: {status}"
    );
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{status}");
    assert_eq!(domains[0]["domain"], "brand");
    assert!(
        domains[0]["behind"].is_null(),
        "behind must degrade to unknown, not error: {status}"
    );
    assert_eq!(
        domains[0]["local_changes"], 1,
        "the local edit; connecting as `brand` writes nothing into the team's MANIFEST"
    );
    let probe_error = domains[0]["probe_error"]
        .as_str()
        .expect("probe_error must carry the offline message");
    assert!(probe_error.contains("offline"), "{probe_error}");
}

#[tokio::test]
async fn origin_status_offline_probe_on_one_domain_still_reports_both_domains() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let good_commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("good-branch", &good_commit);
    let bad_commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("bad-branch", &bad_commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let good_root = tmp.path().join("good");
    let bad_root = tmp.path().join("bad");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;

    eng.origin_add(
        "acme/good",
        Some("good"),
        None,
        Some("good-branch"),
        Some(good_root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    eng.origin_add(
        "acme/bad",
        Some("bad"),
        None,
        Some("bad-branch"),
        Some(bad_root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    mock.fail_branch_head_offline("bad-branch");

    let status = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["errors"].as_array().unwrap().len(), 0, "{status}");
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(
        domains.len(),
        2,
        "both domains must still be reported: {status}"
    );

    let good = domains
        .iter()
        .find(|d| d["domain"] == "good")
        .expect("good domain present");
    assert!(good["probe_error"].is_null());
    assert_eq!(good["behind"], false);

    let bad = domains
        .iter()
        .find(|d| d["domain"] == "bad")
        .expect("bad domain still present despite its offline probe");
    assert!(bad["probe_error"].as_str().is_some());
    assert!(bad["behind"].is_null());
}

#[tokio::test]
async fn origin_status_one_domain_genuinely_failing_does_not_abort_the_others() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let good_commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("good-branch", &good_commit);
    let bad_commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("bad-branch", &bad_commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let good_root = tmp.path().join("good");
    let bad_root = tmp.path().join("bad");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;

    eng.origin_add(
        "acme/good",
        Some("good"),
        None,
        Some("good-branch"),
        Some(good_root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    eng.origin_add(
        "acme/bad",
        Some("bad"),
        None,
        Some("bad-branch"),
        Some(bad_root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    // Corrupt "bad"'s origin state so its status genuinely fails, without
    // touching "good".
    std::fs::remove_file(origins_dir.join("bad").join("state.json")).unwrap();

    let status = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domains = status["domains"].as_array().unwrap();
    let errors = status["errors"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{status}");
    assert_eq!(domains[0]["domain"], "good");
    assert_eq!(errors.len(), 1, "{status}");
    assert_eq!(errors[0]["domain"], "bad");
    assert!(
        errors[0]["error"]
            .as_str()
            .unwrap()
            .contains("origin state")
    );
}

// --- origin_share --------------------------------------------------------------

#[tokio::test]
async fn origin_share_happy_path_opens_a_proposal_and_records_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();

    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed");
    // The engram, and only the engram. The domain declares nothing, so its
    // generated `index.md` - which the engine regenerated when the engram
    // landed - stays on this machine.
    assert_eq!(result["added"], serde_json::json!(["notes/new.md"]));
    assert!(
        result["url"].as_str().unwrap().starts_with("https://"),
        "{result}"
    );

    // The branch name is slugged from the registered domain name "brand",
    // never the working tree's own folder name "brand-knowledge".
    let branch = result["branch"].as_str().unwrap();
    assert!(branch.contains("share-brand-"), "{branch}");
    assert!(!branch.contains("brand-knowledge"), "{branch}");

    // Recorded in the domain's origin state, open.
    let state_dir = origins_dir.join("brand");
    let state = OriginState::load(&state_dir).unwrap().unwrap();
    assert_eq!(state.proposals.len(), 1);
    assert_eq!(state.proposals[0].status, ProposalStatus::Open);
    assert_eq!(state.proposals[0].branch, branch);

    // The generated PR title names the domain "brand", not the folder
    // "brand-knowledge" it happens to live in.
    let title = &state.proposals[0].title;
    assert!(title.contains("brand"), "{title}");
    assert!(!title.contains("brand-knowledge"), "{title}");

    // Nothing local changed: a share never touches the working tree.
    assert!(root.join("notes/new.md").exists());
}

/// The same share for a domain that declares `generated_indexes: shared`: the
/// listing the engine regenerated travels with the engram, which is what keeps
/// a repository browsable on the forge for a team that wants that.
#[tokio::test]
async fn a_domain_that_declares_shared_listings_carries_its_index_too() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_indexes())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();

    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed");
    assert_eq!(
        result["added"],
        serde_json::json!(["index.md", "notes/new.md"]),
        "the declared policy is what puts the listing in the share: {result}"
    );
}

/// The join the whole feature hangs on: the login `resolve_share_provider`
/// hands back is what the proposal record names, in the default identity mode.
///
/// It is worth a whole share rather than a shaper assertion because the engine
/// holds a second, same-typed `Option<String>` at that call site - the
/// personal-mode-only login write failures are enriched with - and swapping
/// the two would zero every instance-mode share's author while every other test
/// in this tree stayed green.
#[tokio::test]
async fn a_share_records_the_login_it_acted_as_on_the_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    // The default identity mode, and a credential that names somebody: the
    // shape resolution 3 locked, where instance mode records its own login
    // rather than nobody.
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false)
        .await
        .with_origin_provider_login("instance-gh");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();

    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");

    let state = OriginState::load(&origins_dir.join("brand"))
        .unwrap()
        .unwrap();
    assert_eq!(
        state.proposals[0].author_login.as_deref(),
        Some("instance-gh"),
        "the acting login reaches the record, not the personal-mode-only one"
    );
}

// --- the direct policy ---------------------------------------------------------

/// A direct domain registered from the forge: the MANIFEST already declares
/// `sharing: direct`, so the first share is a commit.
async fn direct_team(
    tmp: &tempfile::TempDir,
    mock: Arc<MockProvider>,
) -> (Engine, std::path::PathBuf, std::path::PathBuf) {
    direct_team_with(tmp, mock, manifest_sharing_direct()).await
}

/// [`direct_team`] over a MANIFEST of the caller's own.
async fn direct_team_with(
    tmp: &tempfile::TempDir,
    mock: Arc<MockProvider>,
    manifest: Vec<u8>,
) -> (Engine, std::path::PathBuf, std::path::PathBuf) {
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest)]));
    mock.set_branch("main", &commit);
    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false)
        .await
        .with_origin_provider_login("instance-gh");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();
    (eng, root, origins_dir)
}

#[tokio::test]
async fn a_direct_domain_shares_as_a_commit_and_the_files_stop_being_local_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root, origins_dir) = direct_team(&tmp, mock.clone()).await;

    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "committed", "{result}");
    let sha = result["sha"].as_str().unwrap().to_string();
    assert_eq!(
        result["url"],
        format!("https://forge.test/acme/brand-knowledge/commit/{sha}")
    );
    assert_eq!(result["branch"], "main");
    assert_eq!(result["added"], serde_json::json!(["notes/new.md"]));
    assert!(
        result.get("number").is_none() && result.get("drafts_folded").is_none(),
        "{result}"
    );
    assert_eq!(mock.branch_commit("main").unwrap(), sha);
    assert!(
        !mock
            .calls()
            .iter()
            .any(|c| c.starts_with("create_proposal")),
        "{:?}",
        mock.calls()
    );

    // The folder is untouched, and the share is no longer local work.
    assert_eq!(
        std::fs::read(root.join("notes/new.md")).unwrap(),
        engram("New", "new", "brand new content")
    );
    let status = eng
        .origin_status(
            Some("brand"),
            true,
            false,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let d = &status["domains"][0];
    assert_eq!(d["local_changes"], 0, "{d}");
    assert_eq!(d["detail"]["added"], serde_json::json!([]), "{d}");
    assert_eq!(d["sharing"], "direct", "{d}");
    assert_eq!(d["open_proposals"], serde_json::json!([]), "{d}");
    assert_eq!(d["direct_shares"][0]["sha"], sha, "{d}");
    assert_eq!(d["direct_shares"][0]["author_login"], "instance-gh", "{d}");
    assert!(
        d["direct_shares"][0].get("files").is_none(),
        "the status names the commit, not its files: {d}"
    );

    let state = OriginState::load(&origins_dir.join("brand"))
        .unwrap()
        .unwrap();
    assert_eq!(state.base_commit, sha);
    assert!(state.proposals.is_empty());
    assert_eq!(state.direct_shares.len(), 1);

    // The next pull finds head == base and settles up to date.
    let pulled = eng
        .origin_update(Some("brand"), &crystalline_service::Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(pulled["domains"][0]["up_to_date"], true, "{pulled}");
}

#[tokio::test]
async fn a_pulled_manifest_flips_the_policy_for_the_next_share_with_no_state_change_between() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);
    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    // Upstream flips the policy; the pull brings it down as an ordinary edit.
    let flipped = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_direct())]));
    mock.set_branch("main", &flipped);
    let before = std::fs::read_to_string(origins_dir.join("brand/state.json")).unwrap();
    let plan_before = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    // The policy is the folder's as this call found it, and this call's own
    // pull is what writes the new MANIFEST there - so the flip lands on the
    // NEXT share rather than on the preview that fetched it.
    assert_eq!(plan_before["sharing"], "proposal", "{plan_before}");
    assert_eq!(plan_before["action"], "nothing_to_share");
    let plan_after = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    assert_eq!(
        plan_after["sharing"], "direct",
        "the pulled MANIFEST decides, with nothing else having changed: {plan_after}"
    );
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("notes/new.md"), engram("New", "new", "x")).unwrap();
    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "committed", "{result}");
    assert!(
        !before.contains("\"sharing\""),
        "the policy never enters origin state: {before}"
    );
}

#[tokio::test]
async fn a_direct_preview_names_the_commit_and_an_open_proposal_refuses_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, _root, origins_dir) = direct_team(&tmp, mock.clone()).await;
    let plan = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    assert_eq!(plan["action"], "commit", "{plan}");
    assert_eq!(plan["branch"], "main");
    assert_eq!(plan["sharing"], "direct");
    assert_eq!(plan["repo"], "acme/brand-knowledge");
    // The new engram is this domain's only unshared work: connecting as
    // `brand` writes nothing into the team's MANIFEST.
    let mut paths: Vec<&str> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, vec!["notes/new.md"], "{plan}");

    // A proposal this machine recorded stands in the way, by number and url.
    let state_dir = origins_dir.join("brand");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.proposals.push(Proposal {
        number: 4,
        url: "https://github.test/pull/4".to_string(),
        branch: "crystalline/share-brand-x".to_string(),
        title: "Refine".to_string(),
        created_at: chrono::Utc::now(),
        status: ProposalStatus::Open,
        files: vec![],
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: vec![],
        updated_at: None,
        author_login: None,
    });
    state.save(&state_dir).unwrap();
    // The forge knows it too, so the share's own pull refreshes it rather than
    // asking after a proposal that exists on this machine alone.
    mock.set_proposal_state(4, ProposalState::Open);
    let plan = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    assert_eq!(plan["action"], "proposal_open", "{plan}");
    assert_eq!(plan["number"], 4);
    assert_eq!(plan["title"], "Refine");
    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposal_open", "{result}");
    assert_eq!(result["proposal"]["url"], "https://github.test/pull/4");
    assert!(
        result["guidance"]
            .as_str()
            .unwrap()
            .contains("merge or withdraw proposal #4 first")
    );

    // An own open proposal named on a direct domain goes the proposal path
    // rather than being refused: there is a proposal to amend. Proposal #4's
    // branch was never pushed to the forge, so that path settles it and
    // opens a fresh proposal with the change.
    let amended = eng
        .origin_share("brand", None, None, Some(4), None, ShareActor::Owner)
        .await;
    assert!(
        !format!("{amended:?}").contains("there is no proposal to amend"),
        "{amended:?}"
    );
    let amended = amended.unwrap();
    assert_eq!(amended["outcome"], "proposed", "{amended}");
    assert_eq!(
        amended["added"],
        serde_json::json!(["notes/new.md"]),
        "{amended}"
    );
}

/// A share that shares nothing names no team marker and leaves none
/// behind: a direct share the branch refuses answers `branch_protected`
/// without `team_marker`, and the folder's MANIFEST is as it was, so the
/// marker is not a local change nobody asked for.
#[tokio::test]
async fn a_refused_share_names_no_team_marker_and_leaves_none() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let unmarked = String::from_utf8(manifest_sharing_direct())
        .unwrap()
        .replace("team: github\n", "")
        .into_bytes();
    let (eng, root, _origins_dir) = direct_team_with(&tmp, mock.clone(), unmarked.clone()).await;
    mock.protect_branch("main", "Changes must be made through a pull request.");
    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "branch_protected", "{result}");
    assert!(result.get("team_marker").is_none(), "{result}");
    assert_eq!(std::fs::read(root.join("MANIFEST.md")).unwrap(), unmarked);
}

#[tokio::test]
async fn a_protected_branch_answers_branch_protected_through_the_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, _root, origins_dir) = direct_team(&tmp, mock.clone()).await;
    mock.protect_branch("main", "Changes must be made through a pull request.");
    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "branch_protected", "{result}");
    assert_eq!(result["branch"], "main");
    assert_eq!(
        result["message"],
        "Changes must be made through a pull request."
    );
    assert!(
        result["guidance"]
            .as_str()
            .unwrap()
            .contains("share_changes with as_proposal: true")
    );
    assert_eq!(result["fallback"], "proposal", "{result}");
    let state = OriginState::load(&origins_dir.join("brand"))
        .unwrap()
        .unwrap();
    let refusal = state
        .direct_refusal_for(Some("instance-gh"))
        .expect("recorded for the login the share went out on");
    assert_eq!(refusal.branch, "main");
}

/// A status shows the refusal of the caller's own login and nobody else's,
/// and none at all once the domain opens proposals.
#[tokio::test]
async fn the_status_shows_the_callers_own_refusal_and_not_on_a_proposal_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, root, origins_dir) = direct_team(&tmp, mock.clone()).await;
    let state_dir = origins_dir.join("brand");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.record_direct_refusal(crystalline_remote::state::DirectRefusal {
        login: Some("alice".to_string()),
        branch: "main".to_string(),
        message: "Changes must be made through a pull request.".to_string(),
        refused_at: chrono::Utc::now(),
    });
    state.save(&state_dir).unwrap();
    let refused = |status: &serde_json::Value| status["domains"][0]["direct_refused"].clone();
    let status = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        refused(&status),
        serde_json::Value::Null,
        "Alice's, not ours: {status}"
    );

    mock.protect_branch("main", "Changes must be made through a pull request.");
    eng.origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    let status = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        refused(&status)["line"],
        "Direct commits to main are refused for you, so your shares go as proposals.",
        "{status}"
    );
    assert_eq!(
        OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .direct_refused
            .len(),
        2
    );

    let manifest = String::from_utf8(manifest_sharing_direct())
        .unwrap()
        .replace("sharing: direct", "sharing: proposal");
    std::fs::write(root.join("MANIFEST.md"), manifest).unwrap();
    let status = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["domains"][0]["sharing"], "proposal", "{status}");
    assert_eq!(refused(&status), serde_json::Value::Null, "{status}");
}

/// The fallback through the engine: the first refusal offers a proposal, the
/// status line and the preview name it, and the next refused share opens a
/// proposal and says so.
#[tokio::test]
async fn a_refused_direct_share_falls_back_and_the_status_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, _root, _origins_dir) = direct_team(&tmp, mock.clone()).await;
    mock.protect_branch("main", "Changes must be made through a pull request.");
    let first = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(first["outcome"], "branch_protected", "{first}");
    assert_eq!(first["fallback"], "proposal");

    let status = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["direct_refused"]["line"],
        "Direct commits to main are refused for you, so your shares go as proposals.",
        "{status}"
    );
    let plan = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    assert_eq!(
        plan["note"],
        "This share tries a direct commit first. If the branch refuses it again, it opens a proposal.",
        "{plan}"
    );

    let second = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(second["outcome"], "proposed", "{second}");
    assert_eq!(second["fell_back"], true);
    assert_eq!(
        second["note"],
        "The branch main does not accept direct commits from you, so this share opened a proposal."
    );
}

/// A direct commit that lands after a refusal clears it and says so.
#[tokio::test]
async fn a_direct_commit_that_lands_after_a_refusal_says_direct_works_again() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, _root, _origins_dir) = direct_team(&tmp, mock.clone()).await;
    mock.protect_branch("main", "Changes must be made through a pull request.");
    eng.origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    mock.unprotect_branch("main");
    let landed = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(landed["outcome"], "committed", "{landed}");
    assert_eq!(landed["note"], "Direct commits to main work for you now.");
    let status = eng
        .origin_status(Some("brand"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["direct_refused"],
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn as_proposal_opens_a_proposal_through_the_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, _root, _origins_dir) = direct_team(&tmp, mock.clone()).await;
    let shared = eng
        .origin_share_with(
            "brand",
            None,
            None,
            None,
            None,
            Some(true),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "proposed", "{shared}");
    assert!(
        !mock
            .calls()
            .iter()
            .any(|c| c.starts_with("update_branch:main:"))
    );
}

#[tokio::test]
async fn the_daemon_status_block_names_each_domains_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (eng, _root, _origins_dir) = direct_team(&tmp, mock).await;
    let status = eng.status_report().await.unwrap();
    assert_eq!(
        status["origins"]["domains"][0]["sharing"], "direct",
        "{status}"
    );
}

#[tokio::test]
async fn origin_share_with_pending_conflicts_reports_them_without_erroring() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one")),
    ]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    // A genuine same-line conflict, from a real pull.
    std::fs::write(root.join("notes/a.md"), engram("A", "a", "line one LOCAL")).unwrap();
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one UPSTREAM")),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();

    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "conflicts_pending");
    assert_eq!(result["count"], 1);
    let conflicts = result["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"], "notes/a.md");
}

/// A preview and a share both pull first, and a pull writes files. Those files
/// have to reach the index in the same call, exactly as `origin_update` makes
/// them reach it: leaving them out means an engram is on disk and unsearchable
/// until the poller happens to run, which is the shape of a bug nobody
/// attributes to a share.
#[tokio::test]
async fn a_preview_and_a_share_index_what_their_pull_applied() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_indexes())]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    let found = |needle: &'static str| {
        let eng = &eng;
        async move {
            eng.search_engrams(
                &SearchParams {
                    query: Some(needle.to_string()),
                    ..SearchParams::default()
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap()["total"]
                .as_u64()
                .unwrap()
        }
    };

    // Upstream gains a file. The preview's pull applies it.
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest_sharing_indexes()),
        (
            "notes/upstream-one.md",
            engram("Upstream One", "upstream-one", "first upstream arrival"),
        ),
    ]));
    mock.set_branch("main", &c2);

    let plan = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    // Nothing this domain knows is unshared. What the plan does carry is the
    // folder listings the generator wrote when the domain was subscribed: an
    // origin that has never seen them is genuinely behind on them, and they
    // ride along with a share rather than being one - so the action is a
    // create, and every path in it is a listing.
    assert_eq!(plan["action"], "create", "{plan}");
    let changes = plan["changes"].as_array().unwrap();
    assert!(
        !changes.is_empty(),
        "the listings are what makes this a create: {plan}"
    );
    assert!(
        changes
            .iter()
            .all(|c| c["path"].as_str().unwrap_or_default().ends_with("index.md")),
        "{plan}"
    );
    assert!(root.join("notes/upstream-one.md").exists());
    assert_eq!(
        found("first upstream arrival").await,
        1,
        "the preview's pull reached the index"
    );

    // And again for a share, whose own pull applies a second one.
    let c3 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest_sharing_indexes()),
        (
            "notes/upstream-one.md",
            engram("Upstream One", "upstream-one", "first upstream arrival"),
        ),
        (
            "notes/upstream-two.md",
            engram("Upstream Two", "upstream-two", "second upstream arrival"),
        ),
    ]));
    mock.set_branch("main", &c3);

    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/local.md"),
        engram("Local", "local", "locally captured"),
    )
    .unwrap();

    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(
        found("second upstream arrival").await,
        1,
        "the share's pull reached the index too"
    );
}

// --- origin_withdraw, origin_share_preview, origin_conflict_detail -------------

/// A team engine over the mock: domain "kb" subscribed at a two-file commit,
/// one engram edited locally and already shared. Returns the engine, the
/// mock, the working-tree root and the open proposal's number.
async fn shared_team_engine(
    tmp: &tempfile::TempDir,
) -> (Engine, Arc<MockProvider>, std::path::PathBuf, u64) {
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("Alpha", "notes/a", "alpha")),
    ]));
    mock.set_branch("main", &commit);
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock.clone(),
        true,
        false,
    )
    .await;
    let root = tmp.path().join("kb");
    eng.origin_add(
        "acme/kb",
        Some("kb"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    std::fs::write(
        root.join("notes/a.md"),
        engram("Alpha", "notes/a", "alpha v2"),
    )
    .unwrap();
    let shared = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "proposed", "{shared}");
    let number = shared["number"].as_u64().unwrap();
    (eng, mock, root, number)
}

/// The injected test provider short-circuits BOTH share-identity modes, which
/// is what keeps every origin test in this file (and every other one that
/// injects a mock) free of a credential: an engine sharing personally, with no
/// token of any kind on disk, still shares through the mock and never reaches
/// the token store. Personal mode's refusals are engine unit tests, where a
/// real credential resolution actually runs.
#[tokio::test]
async fn an_injected_provider_short_circuits_personal_mode_too() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, _number) = shared_team_engine(&tmp).await;
    eng.configure(&crystalline_service::engine::ConfigureAction::Set {
        key: "github.share_identity".to_string(),
        value: "personal".to_string(),
    })
    .await
    .unwrap();

    std::fs::write(root.join("notes/c.md"), engram("Gamma", "notes/c", "gamma")).unwrap();
    let shared = eng
        .origin_share(
            "kb",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "updated", "{shared}");
}

/// A team engine over a stack-serving mock with a two-layer chain this machine
/// still owes the forge a link for. Both layers are real shares, and the saved
/// link is then broken by hand exactly as a failed `create_stack` leaves it -
/// the stack number cleared, the debt recorded - which is the state a status
/// probe would otherwise pay off. Returns the engine, the mock, the working
/// tree root and the origin state directory.
async fn engine_owing_a_stack_link(
    tmp: &tempfile::TempDir,
) -> (
    Engine,
    Arc<MockProvider>,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let mock = Arc::new(MockProvider::new());
    mock.enable_stacks();
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("Alpha", "notes/a", "alpha")),
    ]));
    mock.set_branch("main", &commit);
    let origins_dir = tmp.path().join("origins");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &origins_dir,
        mock.clone(),
        true,
        false,
    )
    .await;
    let root = tmp.path().join("kb");
    eng.origin_add(
        "acme/kb",
        Some("kb"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    std::fs::write(
        root.join("notes/a.md"),
        engram("Alpha", "notes/a", "alpha v2"),
    )
    .unwrap();
    let first = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(first["outcome"], "proposed", "{first}");
    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();
    let second = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(second["outcome"], "proposed", "{second}");
    assert!(
        second["stack_number"].is_number(),
        "the second layer is stacked on the first: {second}"
    );

    let state_dir = origins_dir.join("kb");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.stack_number = None;
    state.stack_link_pending = true;
    state.save(&state_dir).unwrap();
    (eng, mock, root, state_dir)
}

/// The stack calls a delta of the mock's call log carries, which is what
/// separates "the status wrote to the forge" from "the status read from it".
fn stack_calls(delta: &[String]) -> Vec<String> {
    delta
        .iter()
        .filter(|c| {
            c.starts_with("create_stack")
                || c.starts_with("extend_stack")
                || c.starts_with("dissolve_stack")
        })
        .cloned()
        .collect()
}

/// **A probed status in personal mode never spends the instance credential on
/// a forge write.** Settling an owed stack link is a `create_stack` /
/// `extend_stack`, and the provider a status probes with is the instance one,
/// since a read verb carries no actor. Instance mode is where that is the
/// credential every write goes out on anyway, so the settlement stays; personal
/// mode leaves the debt standing, and the next write on the acting identity's
/// own credential is what pays it off.
#[tokio::test]
async fn a_personal_mode_status_leaves_an_owed_stack_link_for_the_next_write() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, root, state_dir) = engine_owing_a_stack_link(&tmp).await;
    eng.configure(&crystalline_service::engine::ConfigureAction::Set {
        key: "github.share_identity".to_string(),
        value: "personal".to_string(),
    })
    .await
    .unwrap();

    let before = mock.calls().len();
    let status = eng
        .origin_status(Some("kb"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let delta = mock.calls().split_off(before);
    assert!(
        stack_calls(&delta).is_empty(),
        "a read verb made a forge write on the instance credential: {delta:?}"
    );
    assert_eq!(
        status["domains"][0]["stack_link_pending"], true,
        "the debt is reported rather than paid: {status}"
    );
    assert!(
        OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .stack_link_pending,
        "and it is still owed on disk"
    );

    // The next write-class call settles it, on the credential the write itself
    // goes out on - here the acting account's, through the injected provider.
    let before = mock.calls().len();
    std::fs::write(root.join("notes/c.md"), engram("Gamma", "notes/c", "gamma")).unwrap();
    let shared = eng
        .origin_share(
            "kb",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "proposed", "{shared}");
    let delta = mock.calls().split_off(before);
    assert!(
        delta.iter().any(|c| c.starts_with("create_stack:")),
        "the share paid the debt off: {delta:?}"
    );
    assert!(
        !OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .stack_link_pending,
        "the debt is settled"
    );
}

/// The instance-mode half of the same rule, pinned so the fix above cannot
/// quietly become a behavior change for the default install: with one
/// credential doing everything, a probed status settles the owed link exactly
/// as it always has.
#[tokio::test]
async fn an_instance_mode_status_still_settles_an_owed_stack_link() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, _root, state_dir) = engine_owing_a_stack_link(&tmp).await;

    let before = mock.calls().len();
    let status = eng
        .origin_status(Some("kb"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let delta = mock.calls().split_off(before);
    assert!(
        delta.iter().any(|c| c.starts_with("create_stack:")),
        "the probe settled the owed link: {delta:?}"
    );
    assert_eq!(
        status["domains"][0]["stack_link_pending"], false,
        "{status}"
    );
    assert!(
        !OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .stack_link_pending,
        "the debt is cleared on disk"
    );
}

/// A 403 from the forge on a personal write is unreadable in its raw form,
/// and the fix is not the caller's to guess: personal mode's stacks are
/// same-repo and forks are unsupported, so collaborator access is a hard
/// requirement rather than a suggestion.
///
/// This drives the failure through the SHARE VERB rather than through the
/// enrichment function alone, which is the half a unit test cannot pin: the
/// teaching text has to be wired into the verb, with the login the write
/// actually went out as and the repository it was refused by interpolated. The
/// instance half of the same rule rides along - in instance mode the raw text
/// is what a failure keeps (spec section 8).
#[tokio::test]
async fn a_403_on_a_personal_share_teaches_the_collaborator_requirement() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("Alpha", "notes/a", "alpha")),
    ]));
    mock.set_branch("main", &commit);
    // The injected provider has no credential behind it to read a login off,
    // so the login it acts as is supplied beside it.
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock.clone(),
        true,
        false,
    )
    .await
    .with_origin_provider_login("alice-gh");
    eng.configure(&crystalline_service::engine::ConfigureAction::Set {
        key: "github.share_identity".to_string(),
        value: "personal".to_string(),
    })
    .await
    .unwrap();
    let root = tmp.path().join("kb");
    eng.origin_add(
        "acme/kb",
        Some("kb"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    std::fs::write(
        root.join("notes/a.md"),
        engram("Alpha", "notes/a", "alpha v2"),
    )
    .unwrap();

    mock.forbid_writes();
    let err = eng
        .origin_share(
            "kb",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .expect_err("the forge refused the write");
    assert_eq!(
        err.to_string(),
        "your GitHub account @alice-gh needs write access to acme/kb - ask a maintainer to add you as a collaborator."
    );

    // The same failure on the instance credential keeps today's text: nobody
    // is being told to fix a personal connection that was never used.
    eng.configure(&crystalline_service::engine::ConfigureAction::Set {
        key: "github.share_identity".to_string(),
        value: "instance".to_string(),
    })
    .await
    .unwrap();
    let err = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("the forge refuses this one too");
    assert_eq!(
        err.to_string(),
        "Your GitHub account cannot write to acme/kb, so neither a direct commit nor a proposal can be made. Ask the repository's admins for write access.",
        "an instance-token failure says the account cannot write the repository"
    );
}

#[tokio::test]
async fn origin_withdraw_closes_the_pr_and_records_withdrawn() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, root, _number) = shared_team_engine(&tmp).await;
    let v = eng
        .origin_withdraw("kb", None, false, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(v["status"], "withdrawn");
    assert_eq!(v["closed"], true);
    assert!(v["restored"].as_array().unwrap().is_empty());
    assert!(
        mock.calls()
            .iter()
            .any(|c| c.starts_with("close_proposal:")),
        "{:?}",
        mock.calls()
    );
    // The local edit stays: no revert was asked for.
    let text = std::fs::read_to_string(root.join("notes/a.md")).unwrap();
    assert!(text.contains("alpha v2"), "{text}");
}

#[tokio::test]
async fn origin_withdraw_with_revert_restores_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, number) = shared_team_engine(&tmp).await;
    let v = eng
        .origin_withdraw("kb", Some(number), true, ShareActor::Owner)
        .await
        .unwrap();
    // The share carried only the engram edit: connecting as `kb` writes
    // nothing into the team's MANIFEST, so the revert puts back the one file.
    assert_eq!(v["restored"], serde_json::json!(["notes/a.md"]), "{v}");
    let text = std::fs::read_to_string(root.join("notes/a.md")).unwrap();
    assert!(!text.contains("alpha v2"), "restored to base: {text}");
}

#[tokio::test]
async fn origin_share_maps_updated_and_diverged() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, root, number) = shared_team_engine(&tmp).await;

    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();
    let second = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(second["outcome"], "updated");
    assert_eq!(second["proposal"]["number"], number);

    // A reviewer amends the proposal branch.
    let branch = second["proposal"]["branch"].as_str().unwrap().to_string();
    let amended = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch(&branch, &amended);
    std::fs::write(root.join("notes/c.md"), engram("Gamma", "notes/c", "gamma")).unwrap();
    let third = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(third["outcome"], "proposal_diverged");
    assert_eq!(third["proposal"]["number"], number);
    assert!(
        third["guidance"].as_str().unwrap().contains("withdraw"),
        "{third}"
    );
}

/// The `proposal` argument reaches `ops::propose` rather than being dropped
/// on the way: naming the one open layer amends exactly it, and the response
/// carries the stack fields (null here, since the mock forge serves no stacks
/// and the share takes the single-proposal fallback).
#[tokio::test]
async fn origin_share_amend_param_reaches_ops() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, number) = shared_team_engine(&tmp).await;
    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();

    let v = eng
        .origin_share("kb", None, None, Some(number), None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(v["outcome"], "updated", "{v}");
    assert_eq!(v["proposal"]["number"].as_u64(), Some(number), "{v}");
    assert!(v["proposal"]["stack_number"].is_null(), "{v}");
    assert!(v["proposal"]["stack_position"].is_null(), "{v}");
}

/// A share naming a proposal that is not an open layer earns `ops`'s teaching
/// refusal, and that text has to reach a control or MCP client word for word:
/// it names what was asked for and lists the layers that are actually open, so
/// the caller retries against a real number without a second round trip. The
/// engine boundary must not summarize it away - and nothing may be prepended
/// to it either: a framing clause in front of the guidance blames the machine
/// for what the request asked for, so the rendered error starts with the
/// teaching text itself.
#[tokio::test]
async fn origin_share_teaching_refusal_survives_the_engine_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, number) = shared_team_engine(&tmp).await;
    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();

    let err = eng
        .origin_share("kb", None, None, Some(9999), None, ShareActor::Owner)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        format!(
            "proposal #9999 is not an open layer of this domain; open layers: #{number} (layer 1)"
        ),
        "the refusal reaches a control or MCP client whole and unprefixed"
    );
}

/// `origin_status`'s per-domain entry names the stack and every debt around
/// it, even when there is nothing stacked: a caller reads the same four keys
/// whichever path the domain is on.
#[tokio::test]
async fn origin_status_json_names_wedge_and_pending_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, _root, _number) = shared_team_engine(&tmp).await;
    let v = eng
        .origin_status(Some("kb"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &v["domains"][0];
    assert!(domain["stack_number"].is_null(), "{v}");
    assert_eq!(
        domain["stack_wedged"].as_array().map(Vec::len),
        Some(0),
        "{v}"
    );
    assert_eq!(domain["repair_pending"], false, "{v}");
    assert_eq!(domain["stack_link_pending"], false, "{v}");
}

#[tokio::test]
async fn origin_share_preview_names_the_action_and_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, number) = shared_team_engine(&tmp).await;
    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();
    let v = eng
        .origin_share_preview(
            "kb",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    assert_eq!(v["action"], "update");
    assert_eq!(v["number"].as_u64(), Some(number));
    let changes = v["changes"].as_array().unwrap();
    assert!(!changes.is_empty());
    assert!(
        changes
            .iter()
            .all(|c| c["path"].is_string() && c["kind"].is_string()),
        "{changes:?}"
    );
    assert!(v["effective_title"].as_str().is_some());
}

#[tokio::test]
async fn origin_update_response_carries_open_proposal_feedback() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, _root, number) = shared_team_engine(&tmp).await;
    mock.set_feedback(
        number,
        Feedback {
            review_state: Some("changes_requested".to_string()),
            items: vec![FeedbackItem {
                author: "ana".to_string(),
                body: "tighten the wording".to_string(),
                path: None,
                line: None,
                submitted_at: "2026-08-21T10:00:00Z".to_string(),
                kind: FeedbackKind::Comment,
            }],
        },
    );
    let v = eng
        .origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();
    let prop = &v["domains"][0]["open_proposals"][0];
    assert_eq!(prop["feedback"][0]["body"], "tighten the wording", "{v}");
    assert_eq!(prop["review_state"], "changes_requested");
}

#[tokio::test]
async fn origin_status_flags_an_amended_open_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, _root, number) = shared_team_engine(&tmp).await;
    let branch = {
        let state_dir = tmp.path().join("origins").join("kb");
        let state = OriginState::load(&state_dir).unwrap().unwrap();
        state.proposals[0].branch.clone()
    };
    let amended = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch(&branch, &amended);

    let v = eng
        .origin_status(Some("kb"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let open = &v["domains"][0]["open_proposals"][0];
    assert_eq!(open["number"].as_u64(), Some(number), "{v}");
    assert_eq!(open["amended_upstream"], true, "{v}");
}

/// [`shared_team_engine`] carried one step further: the open proposal is
/// withdrawn, then a local and an upstream edit of the same engram are pulled
/// into a genuine EditEdit conflict. Returns the engine, the working-tree root
/// and the recorded conflict's id.
async fn conflicted_team_engine(tmp: &tempfile::TempDir) -> (Engine, std::path::PathBuf, String) {
    let (eng, mock, root, _number) = shared_team_engine(tmp).await;
    // Clear the open proposal so the conflict setup is the only moving part.
    eng.origin_withdraw("kb", None, false, ShareActor::Owner)
        .await
        .unwrap();
    // Local and upstream edit the same engram differently, then pull.
    std::fs::write(
        root.join("notes/a.md"),
        engram("Alpha", "notes/a", "mine mine"),
    )
    .unwrap();
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("Alpha", "notes/a", "theirs theirs")),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();

    let status = eng
        .origin_status(Some("kb"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let id = status["domains"][0]["conflicts"][0]["id"]
        .as_str()
        .expect("the pull recorded a conflict")
        .to_string();
    (eng, root, id)
}

#[tokio::test]
async fn origin_conflict_detail_reads_both_sides_by_id_or_path() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _root, id) = conflicted_team_engine(&tmp).await;
    let by_id = eng
        .origin_conflict_detail("kb", Some(&id), None)
        .await
        .unwrap();
    assert_eq!(by_id["id"], id.as_str());
    assert!(by_id["local"].as_str().unwrap().contains("mine mine"));
    assert!(
        by_id["upstream"]
            .as_str()
            .unwrap()
            .contains("theirs theirs")
    );
    let path = by_id["path"].as_str().unwrap().to_string();
    let by_path = eng
        .origin_conflict_detail("kb", None, Some(&path))
        .await
        .unwrap();
    assert_eq!(by_path["id"], id.as_str());
    assert!(by_id["note"].is_null(), "every side is UTF-8 here");
}

#[tokio::test]
async fn origin_conflict_detail_addressing_rules_and_a_missing_local_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, root, id) = conflicted_team_engine(&tmp).await;

    // Neither an id nor a path is a malformed request, not a missing one.
    let err = eng
        .origin_conflict_detail("kb", None, None)
        .await
        .unwrap_err();
    match err {
        EngineError::Invalid(msg) => assert!(msg.contains("an id or a path"), "{msg}"),
        other => panic!("expected Invalid, got {other}"),
    }

    // An id that matches nothing is not found, even though the path of the
    // one real conflict is passed alongside it: the id wins outright.
    let err = eng
        .origin_conflict_detail("kb", Some("deadbeef"), Some("notes/a.md"))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound(_)), "{err}");

    // A conflict whose local file is gone reports a null local side rather
    // than failing: the recorded base and upstream still answer.
    std::fs::remove_file(root.join("notes/a.md")).unwrap();
    let v = eng
        .origin_conflict_detail("kb", Some(&id), None)
        .await
        .unwrap();
    assert!(v["local"].is_null(), "{v}");
    assert!(v["upstream"].as_str().unwrap().contains("theirs theirs"));
    assert!(v["note"].is_null(), "a missing side is not a binary side");
}

#[tokio::test]
async fn origin_withdraw_restores_files_and_syncs_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/keep.md", engram("Keep", "keep", "base content")),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    // A previously opened, now declined proposal touching keep.md, without
    // going through a real `origin_share` call.
    let proposed = engram("Keep", "keep", "shared v2 content");
    std::fs::write(root.join("notes/keep.md"), &proposed).unwrap();
    let state_dir = origins_dir.join("brand");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.proposals.push(Proposal {
        number: 5,
        url: "https://github.test/pulls/5".to_string(),
        branch: "crystalline/share-brand-000101000000".to_string(),
        title: "Refine 1 engram in brand".to_string(),
        created_at: chrono::Utc::now(),
        status: ProposalStatus::Declined,
        files: vec![ProposedFile {
            path: "notes/keep.md".to_string(),
            change: ProposedChange::Modified,
            sha256: Some(sha256_hex(&proposed)),
            blob_sha: None,
            size: Some(proposed.len() as u64),
        }],
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
        author_login: None,
    });
    state.save(&state_dir).unwrap();

    let result = eng
        .origin_withdraw("brand", Some(5), true, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["restored"][0], "notes/keep.md");
    assert_eq!(
        result["closed"], false,
        "a declined proposal is already closed"
    );

    // The working tree is back to the base content.
    let content = std::fs::read_to_string(root.join("notes/keep.md")).unwrap();
    assert!(content.contains("base content"), "{content}");

    // The record moved to history, recorded as withdrawn.
    let reloaded = OriginState::load(&state_dir).unwrap().unwrap();
    assert!(reloaded.proposals.is_empty());
    assert_eq!(reloaded.history[0].status, ProposalStatus::Withdrawn);

    // The index reflects the restored content: sync ran after the withdraw.
    let hits = eng
        .search_engrams(
            &SearchParams {
                query: Some("base content".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(hits["total"], 1, "{hits}");
}

#[tokio::test]
async fn origin_withdraw_schedules_embedding_on_the_worker_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/keep.md", engram("Keep", "keep", "base content")),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false)
        .await
        .with_embed_channel(tx);
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    // Drain the connect's own scheduled embed so the assertion below sees
    // only the withdraw's pass.
    while rx.try_recv().is_ok() {}

    // A previously opened, now declined proposal touching keep.md, without
    // going through a real `origin_share` call.
    let proposed = engram("Keep", "keep", "shared v2 content");
    std::fs::write(root.join("notes/keep.md"), &proposed).unwrap();
    let state_dir = origins_dir.join("brand");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.proposals.push(Proposal {
        number: 5,
        url: "https://github.test/pulls/5".to_string(),
        branch: "crystalline/share-brand-000101000000".to_string(),
        title: "Refine 1 engram in brand".to_string(),
        created_at: chrono::Utc::now(),
        status: ProposalStatus::Declined,
        files: vec![ProposedFile {
            path: "notes/keep.md".to_string(),
            change: ProposedChange::Modified,
            sha256: Some(sha256_hex(&proposed)),
            blob_sha: None,
            size: Some(proposed.len() as u64),
        }],
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
        author_login: None,
    });
    state.save(&state_dir).unwrap();

    eng.origin_withdraw("brand", Some(5), true, ShareActor::Owner)
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "origin_withdraw must schedule a background embed instead of embedding inline"
    );
}

// --- origin_resolve --------------------------------------------------------------

#[tokio::test]
async fn origin_resolve_writes_the_resolution_and_syncs_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one")),
    ]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    // A local edit, then an upstream edit to the same line: a genuine
    // EditEdit conflict once pulled.
    std::fs::write(root.join("notes/a.md"), engram("A", "a", "line one LOCAL")).unwrap();
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one UPSTREAM")),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();

    let state_dir = origins_dir.join("brand");
    assert_eq!(
        OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .conflicts
            .len(),
        1
    );

    let result = eng
        .origin_resolve(
            "brand",
            "notes/a.md",
            Some("theirs"),
            None,
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(result["remaining"], 0);

    let content = std::fs::read_to_string(root.join("notes/a.md")).unwrap();
    assert!(content.contains("line one UPSTREAM"), "{content}");
    assert!(
        OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .conflicts
            .is_empty()
    );

    // The index reflects the resolved content: sync ran after resolve.
    let hits = eng
        .search_engrams(
            &SearchParams {
                query: Some("UPSTREAM".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(hits["total"], 1, "{hits}");
}

#[tokio::test]
async fn origin_resolve_unknown_path_errors_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    let err = eng
        .origin_resolve(
            "brand",
            "notes/missing.md",
            Some("mine"),
            None,
            ShareActor::Owner,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Remote(RemoteError::ConflictNotFound { .. })
        ),
        "{err}"
    );
}

// --- domain_add_local --------------------------------------------------------

#[tokio::test]
async fn domain_add_local_schedules_embedding_on_the_worker_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        false,
        false,
    )
    .await
    .with_embed_channel(tx);

    let folder = tmp.path().join("local-notes");
    std::fs::create_dir_all(folder.join("notes")).unwrap();
    std::fs::write(folder.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        folder.join("notes/alpha.md"),
        engram("Alpha", "alpha", "alpha body"),
    )
    .unwrap();

    eng.domain_add_local(Some("local-notes"), Some(folder.to_str().unwrap()))
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "domain_add_local must schedule a background embed instead of embedding inline"
    );
}

// --- hybrid search lock discipline (M2.1) ------------------------------------

/// Pins the three-phase `search_engrams` contract that the A2+A12 lock
/// restructure must preserve: with active embeddings and a stub provider a
/// hybrid search reports `mode: hybrid`, still returns hits and embeds the
/// query exactly once. It passes before and after the change (a regression pin,
/// not a red-first test); the win is that no store guard is held across the
/// provider embed call, which structural review guards, so this asserts
/// behavior only and never times a lock.
#[tokio::test]
async fn hybrid_search_returns_hits_and_embeds_the_query_once() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        false,
        false,
    )
    .await;
    let embedder = Arc::new(CountingEmbedder::new());
    eng.set_provider(embedder.clone());

    // A local domain with one engram, synced and embedded inline: no embed
    // channel is wired, so `domain_add_local` runs the embed pass itself and the
    // store ends up with active embeddings for the engine's model.
    let folder = tmp.path().join("local-notes");
    std::fs::create_dir_all(folder.join("notes")).unwrap();
    std::fs::write(folder.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        folder.join("notes/alpha.md"),
        engram("Alpha", "alpha", "alpha body"),
    )
    .unwrap();
    eng.domain_add_local(Some("local-notes"), Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    let before = embedder.calls.load(std::sync::atomic::Ordering::SeqCst);
    assert!(before >= 1, "the inline embed pass ran during domain add");

    let hits = eng
        .search_engrams(
            &SearchParams {
                query: Some("alpha".to_string()),
                search_type: Some("hybrid".to_string()),
                domains: vec!["local-notes".to_string()],
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();

    assert_eq!(
        hits["mode"], "hybrid",
        "active embeddings keep the mode hybrid"
    );
    assert!(
        hits["total"].as_u64().unwrap() >= 1,
        "the hybrid search still returns hits"
    );
    let after = embedder.calls.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(after, before + 1, "the query was embedded exactly once");
}

// --- a share of a domain that reviews changes --------------------------------
//
// In review mode nothing a member writes is on disk: every write joins that
// member's own draft overlay and the folder on disk goes on saying what the
// team reviewed. So a share of such a domain cannot be a walk of the folder -
// it would find nothing, or worse, somebody's stray direct edit. It is built
// from the acting actor's own overlay rows, drafts as content and tombstones
// as deletions, against the base snapshot.
//
// Every fixture here writes the overlay rows by hand, the way the write verbs
// do, and deliberately writes no journal mirror: the rows are the live truth
// and the plan is built from them, so a test that journals nothing and still
// sees its drafts shared is the test that says so.

/// The base engram the team already has, at `notes/plan.md`.
fn team_plan() -> Vec<u8> {
    engram("Plan", "plan", "the plan as the team has it")
}

/// One actor's draft of the same path: their private rewrite of it.
const DRAFT_PLAN: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\n---\n\nthe plan as this actor would have it\n";

/// A draft of a path no file holds: the sharp case, since nothing on disk
/// could have produced it.
const DRAFT_FRESH: &str = "---\ntype: engram\ntitle: Fresh\npermalink: fresh\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\n---\n\na page only this actor has\n";

/// Another actor's draft, so a scenario can prove whose work travels.
const DRAFT_ALICE: &str = "---\ntype: engram\ntitle: Alice\npermalink: alice\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\n---\n\na page only alice has\n";

/// A draft standing at a generated folder listing's own path.
const DRAFT_INDEX: &str = "---\ntype: index\ntitle: notes\npermalink: notes-index\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\n---\n\na listing this actor redrew\n";

/// An index row the way a write verb builds one for a draft that is on nobody's
/// disk: the markdown lives in the row's own `content` column, because the row
/// is the only place the draft exists.
fn overlay_record(text: &str, path: &str) -> crystalline_index::EngramRecord {
    let mut record = crystalline_index::EngramRecord::from_engram(
        &crystalline_core::parse_engram(text).unwrap(),
        path,
        crystalline_index::FileStamp {
            mtime: 0,
            size: text.len() as u64,
            sha256: "0".repeat(64),
        },
    );
    record.content = text.to_string();
    record
}

/// A reviewing domain `team` connected to `acme/team`, its first pull already
/// recorded, with `files` as the repository's own content.
///
/// Returns the engine, the working tree root and the origins directory. The
/// temp directory is the caller's to hold: everything here lives under it.
pub(super) async fn reviewing_domain(
    tmp: &Path,
    mock: Arc<MockProvider>,
    files: &[(&str, Vec<u8>)],
) -> Engine {
    let commit = mock.add_commit(commit_files(files));
    mock.set_branch("main", &commit);
    let root = tmp.join("team-knowledge");
    let eng = engine_with(
        &tmp.join("config.yaml"),
        &tmp.join("origins"),
        mock,
        true,
        false,
    )
    .await;
    eng.origin_add(
        "acme/team",
        Some("team"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    eng.set_review_mode(
        "team",
        Some(crystalline_core::config::ReviewMode::Overlay),
        crystalline_service::ReviewModeConfirm::Confirmed { folds: Vec::new() },
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    eng
}

/// Write one actor's draft of `path` straight into the index, the way a write
/// verb in review mode does: a row in that actor's dimension, and nothing on
/// disk.
pub(super) async fn draft(eng: &Engine, actor: &str, path: &str, text: &str) {
    let store = eng.store();
    let store = store.lock().await;
    let id = store
        .domain_id("team")
        .await
        .unwrap()
        .expect("team is indexed");
    store
        .upsert_overlay(id, actor, &overlay_record(text, path))
        .await
        .unwrap();
}

/// [`draft`] for a draft written since drafts keep a record of where they
/// started: the row, and the record a write verb leaves, read from the base
/// snapshot as it stands now. Call it before the team moves on. `tmp` is the
/// fixture's state directory, as [`mirror`] takes it.
pub(super) async fn drafted(eng: &Engine, tmp: &Path, actor: &str, path: &str, text: &str) {
    // A second write of a row keeps the record the first one left.
    if crystalline_service::draft_start::load(tmp, "team", actor, path)
        .unwrap()
        .is_some()
    {
        draft(eng, actor, path, text).await;
        return;
    }
    let snapshot = tmp.join("origins").join("team");
    let state = crystalline_remote::state::OriginState::load(&snapshot)
        .unwrap()
        .expect("the domain has a base snapshot");
    let start = if state.files.contains_key(path) {
        crystalline_remote::state::read_base_file(&snapshot, path)
            .unwrap()
            .map(|bytes| crystalline_core::to_lf(&String::from_utf8(bytes).unwrap()).into_owned())
    } else {
        None
    };
    let from = start.is_some().then(|| path.to_string());
    crystalline_service::draft_start::save(
        tmp,
        "team",
        actor,
        &crystalline_service::draft_start::DraftStart {
            path: path.to_string(),
            from,
            start,
            start_sha256: None,
            shared: None,
            merged_at: None,
            merging: None,
            settling: None,
        },
    )
    .unwrap();
    draft(eng, actor, path, text).await;
}

/// Write one actor's deletion of a base path: a tombstone row standing at that
/// path, whose permalink is the path, exactly as `write_overlay_tombstone`
/// records one.
async fn tombstone(eng: &Engine, actor: &str, path: &str) {
    let store = eng.store();
    let store = store.lock().await;
    let id = store
        .domain_id("team")
        .await
        .unwrap()
        .expect("team is indexed");
    let mut record = overlay_record(DRAFT_PLAN, path);
    record.tombstone = true;
    record.permalink = path.to_string();
    store.upsert_overlay(id, actor, &record).await.unwrap();
}

/// [`tombstone`] for a deletion made since drafts keep a record: the record the
/// delete verb leaves, starting from the team's text at that path.
async fn tombstoned(eng: &Engine, tmp: &Path, actor: &str, path: &str) {
    tombstone(eng, actor, path).await;
    let snapshot = tmp.join("origins").join("team");
    let start = crystalline_remote::state::read_base_file(&snapshot, path)
        .unwrap()
        .map(|bytes| crystalline_core::to_lf(&String::from_utf8(bytes).unwrap()).into_owned());
    crystalline_service::draft_start::save(
        tmp,
        "team",
        actor,
        &crystalline_service::draft_start::DraftStart {
            path: path.to_string(),
            from: Some(path.to_string()),
            start,
            start_sha256: None,
            shared: None,
            merged_at: None,
            merging: None,
            settling: None,
        },
    )
    .unwrap();
}

/// The scope one actor name means, as an authenticated surface resolves it.
/// The machine owner drafts as `owner`, which is [`Scope::Unrestricted`]'s own
/// overlay key, and anybody else drafts under their account.
fn scope_of(actor: &str) -> Scope {
    if actor == "owner" {
        Scope::Unrestricted
    } else {
        Scope::User {
            account: actor.to_string(),
            admin: false,
        }
    }
}

/// Write one actor's draft file: an attachment written in review mode, which
/// lands in that actor's files overlay and never in the folder.
///
/// It goes through `attachment_write_as` rather than through a `DomainView`,
/// because `domain_view` is `pub(crate)` and an integration test cannot build
/// one. The receipt's `draft` flag is asserted here so a fixture can never
/// quietly write the team's folder instead.
async fn file(eng: &Engine, actor: &str, path: &str, bytes: &[u8]) {
    let written = eng
        .attachment_write_as("team", path, bytes.to_vec(), &scope_of(actor))
        .await
        .unwrap();
    assert!(written.draft, "a write in review mode is a draft: {path}");
}

/// One actor's deletion of a reviewed file: a marker in their files overlay,
/// with the folder untouched.
async fn delete_file(eng: &Engine, actor: &str, path: &str) {
    let draft = eng
        .attachment_delete_as("team", path, &scope_of(actor))
        .await
        .unwrap();
    assert!(draft, "a deletion in review mode is a draft: {path}");
}

/// A file the team already has, and one only an actor's draft has.
const OLD_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00the team's old deck";
const DECK_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00a deck only alice has";

/// A domain that reviews changes gets the marker through its review: the
/// next proposal carries the MANIFEST line like any other change, the
/// preview and the receipt say so, and the folder moves only when the team
/// merges it.
#[tokio::test]
async fn a_review_mode_share_carries_the_marker_through_the_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[
            ("MANIFEST.md", manifest_unmarked()),
            ("notes/plan.md", team_plan()),
        ],
    )
    .await;
    let root = tmp.path().join("team-knowledge");
    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    let preview = eng
        .origin_share_preview(
            "team",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    assert_eq!(preview["team_marker"], team_marker(), "{preview}");
    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(
        result["updated"],
        serde_json::json!(["MANIFEST.md", "notes/plan.md"]),
        "{result}"
    );
    assert_eq!(result["team_marker"], team_marker(), "{result}");
    let shared = shared_manifest(&mock, &result);
    assert_eq!(
        shared,
        String::from_utf8(manifest_unmarked())
            .unwrap()
            .replace("title: Team\n", "title: Team\nteam: github\n"),
        "the proposal carries exactly the marker line"
    );
    assert_eq!(
        std::fs::read(root.join("MANIFEST.md")).unwrap(),
        manifest_unmarked(),
        "the folder waits for the review"
    );
}

/// A share carries the acting actor's overlay FILES beside their rows: the
/// bytes of a file they drafted travel as an addition, their deletion of a
/// reviewed file travels as a deletion, and the folder on disk is untouched by
/// any of it.
///
/// A share that dropped a draft file would be worse than one that carried
/// nothing: `ops::propose` detects against the staged tree, so a file left out
/// of the staging reads as a file the actor deleted and the proposal would ask
/// the team to delete their own copy.
#[tokio::test]
async fn an_overlay_share_stages_the_actors_files_and_the_proposal_carries_the_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", team_plan()),
            ("assets/old.png", OLD_PNG.to_vec()),
        ],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    file(&eng, "owner", "assets/deck.png", DECK_PNG).await;
    delete_file(&eng, "owner", "assets/old.png").await;

    // Scoped to the deck alone: a staged overlay file is a detected change
    // like any other, so `files` selects among them the way it always has.
    let scoped = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            Some(&["assets/deck.png".to_string()]),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(scoped["outcome"], "proposed", "{scoped}");
    assert_eq!(scoped["added"], serde_json::json!(["assets/deck.png"]));
    assert_eq!(scoped["updated"], serde_json::json!([]), "{scoped}");
    assert_eq!(scoped["deleted"], serde_json::json!([]), "{scoped}");
    let branch = scoped["branch"].as_str().unwrap().to_string();
    let commit = mock
        .branch_commit(&branch)
        .expect("the share made a branch");
    assert_eq!(
        mock.commit_file(&commit, "assets/deck.png").as_deref(),
        Some(DECK_PNG),
        "the proposal carries the draft file's own bytes"
    );

    // And the whole delta: the draft, the new file and the deletion together.
    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    // The second share amends the proposal the first one opened, so the delta
    // rides under `proposal` rather than at the top level.
    let amended = &result["proposal"];
    assert_eq!(
        amended["added"],
        serde_json::json!(["assets/deck.png"]),
        "{result}"
    );
    assert_eq!(amended["updated"], serde_json::json!(["notes/plan.md"]));
    assert_eq!(amended["deleted"], serde_json::json!(["assets/old.png"]));
    let branch = amended["branch"].as_str().unwrap().to_string();
    let commit = mock
        .branch_commit(&branch)
        .expect("the share made a branch");
    assert_eq!(
        mock.commit_file(&commit, "assets/deck.png").as_deref(),
        Some(DECK_PNG),
        "the deck is in the tree the proposal points at"
    );
    assert!(
        mock.commit_file(&commit, "assets/old.png").is_none(),
        "and the file the actor deleted is not"
    );

    // A share never touches the working tree, in review mode least of all.
    assert!(
        !root.join("assets/deck.png").exists(),
        "the draft file is nobody's folder file yet"
    );
    assert!(
        root.join("assets/old.png").exists(),
        "and the deletion is a draft too: the team's copy is where it was"
    );
}

/// The delta a share carries is the actor's overlay and nothing else: a draft
/// over a base file is an update, a draft of a path no file holds is an
/// addition, a tombstone is a deletion - and a file somebody dropped into the
/// reviewed folder by hand is not part of it at all, which is the whole point
/// of review mode.
#[tokio::test]
async fn an_overlay_share_proposes_exactly_the_actors_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", team_plan()),
            ("notes/old.md", engram("Old", "old", "on its way out")),
        ],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    tombstoned(&eng, tmp.path(), "owner", "notes/old.md").await;
    // A stray direct edit of the reviewed folder. Nobody reviewed it, so no
    // share carries it: a walk of the tree would, and this is how the test
    // says the plan is not a walk.
    std::fs::write(
        root.join("notes/stray.md"),
        engram("Stray", "stray", "written straight to disk"),
    )
    .unwrap();

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
    assert_eq!(result["updated"], serde_json::json!(["notes/plan.md"]));
    assert_eq!(result["deleted"], serde_json::json!(["notes/old.md"]));
    assert!(
        !result.to_string().contains("stray"),
        "the stray direct edit is not part of anybody's draft: {result}"
    );
    // A share never touches the working tree, and in review mode that includes
    // the draft: the folder still says what the team reviewed.
    assert!(!root.join("notes/fresh.md").exists());
    assert!(root.join("notes/old.md").exists());
}

/// Scoping a share selects within the acting actor's own overlay. A path
/// somebody else is drafting is not among this actor's changes, so naming it
/// refuses the share and says which path it was.
#[tokio::test]
async fn files_naming_another_actors_path_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    drafted(&eng, tmp.path(), "alice", "notes/alice.md", DRAFT_ALICE).await;

    let err = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            Some(&["notes/alice.md".to_string()]),
            ShareActor::Owner,
        )
        .await
        .expect_err("a path this actor is not drafting cannot be shared");
    let text = err.to_string();
    assert!(text.contains("notes/alice.md"), "{text}");

    // The actor's own path still shares, so the refusal is about whose draft
    // it is and not about scoping at all.
    let result = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            Some(&["notes/fresh.md".to_string()]),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
}

/// The CLI is the machine owner, and the machine owner drafts under one name.
/// A share it makes carries the owner's overlay and nobody else's.
#[tokio::test]
async fn the_owner_cli_shares_the_owner_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    drafted(&eng, tmp.path(), "alice", "notes/alice.md", DRAFT_ALICE).await;

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
    assert!(
        !result.to_string().contains("alice"),
        "alice's draft is hers to share: {result}"
    );

    // Alice waits while the owner's proposal is open, and once it is closed
    // the same engine, asked as alice, carries hers and not the owner's.
    let waits = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(waits["outcome"], "proposal_open", "{waits}");
    mock.set_proposal_state(
        result["number"].as_u64().unwrap(),
        crystalline_remote::provider::ProposalState::Declined,
    );
    let hers = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(hers["outcome"], "proposed", "{hers}");
    assert!(
        hers.to_string().contains("notes/alice.md") && !hers.to_string().contains("fresh"),
        "alice shares her own draft: {hers}"
    );
}

/// An agent over HTTP on an instance that makes no agent authenticate has no
/// identity, so there is no draft for a share to be of. It is refused with the
/// one message the write verbs refuse with, which teaches the way in.
#[tokio::test]
async fn an_http_agent_without_identity_cannot_share_a_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;

    let err = eng
        .origin_share("team", None, None, None, None, ShareActor::HttpAgent)
        .await
        .expect_err("nobody's draft is every draft");
    assert_eq!(
        err.to_string(),
        crystalline_service::OVERLAY_NEEDS_IDENTITY,
        "the refusal is the constant itself"
    );

    // The preview refuses the same way, so no client is ever asked to confirm
    // a share this instance would then refuse.
    let err = eng
        .origin_share_preview(
            "team",
            None,
            None,
            None,
            ShareActor::HttpAgent,
            PreviewCredential::ActingIdentity,
        )
        .await
        .expect_err("the preview carries the share's own gates");
    assert_eq!(err.to_string(), crystalline_service::OVERLAY_NEEDS_IDENTITY);
}

/// A generated directory index is not a draft, and the domain's own
/// `generated_indexes` policy decides whether it takes part in a share at all.
/// The rule binds BOTH sides: the actor's entries and the base snapshot. Filter
/// the entries alone and every index the repository already recorded turns into
/// a proposed deletion of a file that is sitting right there.
#[tokio::test]
async fn an_overlay_share_respects_the_domains_generated_index_policy_on_both_sides() {
    // Local listings: neither the draft of one nor the base's own key travels.
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest()),
            ("notes/index.md", engram("notes", "notes-index", "listing")),
        ],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    drafted(&eng, tmp.path(), "owner", "notes/index.md", DRAFT_INDEX).await;

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
    assert_eq!(result["updated"], serde_json::json!([]));
    assert_eq!(
        result["deleted"],
        serde_json::json!([]),
        "the base's own listing key is not a deletion: {result}"
    );

    // The same domain declaring `generated_indexes: shared`: both halves stay.
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest_sharing_indexes()),
            ("notes/index.md", engram("notes", "notes-index", "listing")),
        ],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    drafted(&eng, tmp.path(), "owner", "notes/index.md", DRAFT_INDEX).await;

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
    assert_eq!(
        result["updated"],
        serde_json::json!(["notes/index.md"]),
        "a domain that shares its listings carries the drafted one: {result}"
    );
}

/// A share of a reviewing domain still pulls the team's FOLDER.
///
/// Every share pulls first, because a proposal has to be mergeable when it is
/// opened. In review mode the tree the share is detected against is staged, and
/// a pull that landed there instead would advance the base snapshot while the
/// folder stayed where it was - leaving the team's own files behind their base,
/// where every later share reads them as deletions. So the pull runs against the
/// folder, and the share is staged afterwards.
#[tokio::test]
async fn a_review_mode_share_pulls_the_teams_folder_not_the_staged_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;

    // The team merged somebody else's work while this draft was being written.
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
        ("notes/merged.md", engram("Merged", "merged", "already in")),
    ]));
    mock.set_branch("main", &moved);

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));

    // The merged file is in the team's folder, where the whole instance reads
    // it, and not only in a staged tree that is gone by now.
    assert!(
        root.join("notes/merged.md").exists(),
        "the pull landed in the folder the team shares"
    );

    // And the next share still proposes the draft alone: nothing the pull
    // brought in reads as a deletion.
    let again = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(
        again["deleted"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "the folder is level with its own base: {again}"
    );
}

/// A draft carrying a `generated` block, so a preview of a domain that takes
/// changes directly has provenance to report and the contrast with a reviewing
/// domain's preview is a real difference rather than two empty answers.
const AUTHORED: &str = "---\ntype: engram\ntitle: Authored\npermalink: authored\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\ngenerated: { by: human:ada, at: 2026-08-29T09:00:00+00:00 }\n---\n\nsomebody is named for this one\n";

/// The team's copy moving while a share is being prepared is answered, not
/// merged.
///
/// A share pulls the folder first and then proposes against the staged tree. The
/// proposal step would pull for itself, and that pull would land the team's
/// merged work inside the staged tree - which is deleted when the share ends,
/// while the base snapshot advances past it, leaving the folder permanently
/// behind its own base with no pull that would ever bring it back. So the share
/// refuses instead, and says to run it again.
#[tokio::test]
async fn an_upstream_move_after_the_share_pulled_refuses_instead_of_merging_into_staging() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");
    let state_dir = tmp.path().join("origins").join("team");
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;

    let settled = crystalline_remote::state::OriginState::load(&state_dir)
        .unwrap()
        .unwrap()
        .base_commit;

    // The team merges something the instant after this share has pulled.
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
        ("notes/merged.md", engram("Merged", "merged", "already in")),
    ]));
    mock.move_branch_after_head_probes(mock.branch_head_calls() + 1, "main", &moved);

    let err = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("a share cannot merge the team's work into a tree it is about to delete");
    let text = err.to_string();
    assert!(text.contains("run it again"), "{text}");

    // Nothing moved: not the folder, not the base record.
    assert!(!root.join("notes/merged.md").exists());
    let after = crystalline_remote::state::OriginState::load(&state_dir)
        .unwrap()
        .unwrap();
    assert_eq!(after.base_commit, settled, "the base record stands still");
    assert!(!after.files.contains_key("notes/merged.md"));
    assert!(after.conflicts.is_empty(), "no conflict was recorded");

    // Nothing was lost either: the draft is still the actor's.
    {
        let store = eng.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        let entries = store.overlay_entries(id, "owner").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "notes/fresh.md");
    }

    // And running it again, after the pull has landed, shares as it should.
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(root.join("notes/merged.md").exists());
    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
}

/// An actor holding no drafts has nothing to share, and hears that rather than
/// a proposal of the folder they never wrote in.
#[tokio::test]
async fn an_actor_with_no_drafts_has_nothing_to_share() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    // Somebody else is drafting; this actor is not.
    drafted(&eng, tmp.path(), "alice", "notes/alice.md", DRAFT_ALICE).await;

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "nothing_to_share", "{result}");
}

/// A shared draft is still a draft.
///
/// Sharing proposes the work; it does not take it out of the overlay. The rows
/// stay exactly as they stood, which is the precondition convergence is defined
/// against: a merged proposal pulled back is what clears them, and nothing else.
/// So a second share of the same untouched drafts proposes the same paths again,
/// against the same base.
#[tokio::test]
async fn a_shared_draft_is_still_a_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;

    let before = {
        let store = eng.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        store.overlay_entries(id, "owner").await.unwrap()
    };
    assert_eq!(before.len(), 2);

    let first = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(first["outcome"], "proposed");
    assert_eq!(first["added"], serde_json::json!(["notes/fresh.md"]));
    assert_eq!(first["updated"], serde_json::json!(["notes/plan.md"]));

    let after = {
        let store = eng.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        store.overlay_entries(id, "owner").await.unwrap()
    };
    assert_eq!(after, before, "the share cleared nothing");

    // The assertion above is the design's own requirement: the rows survive.
    // What a second share then ANSWERS is a property of the forge shape - this
    // mock serves no stacks, so the share is detected against the trunk and
    // updates the one open proposal with the same paths. On a forge that serves
    // stacks the same untouched drafts would be nothing new to stack, because a
    // layer is detected against the chain tip; that is the question Task 11
    // owns, and either answer is the same surviving rows.
    let second = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(second["outcome"], "updated", "{second}");
    assert_eq!(
        second["proposal"]["added"],
        serde_json::json!(["notes/fresh.md"])
    );
    assert_eq!(
        second["proposal"]["updated"],
        serde_json::json!(["notes/plan.md"])
    );
}

/// The preview a person confirms a share on lists the actor's overlay paths, and
/// nothing is preselected for them.
///
/// Preselection is a guess at which files in a mixed delta are the caller's own,
/// read off each file's `generated` block. A reviewing domain's plan is built
/// from the caller's own drafts, so every path in it is already theirs and the
/// guess has nothing left to answer - which is why `last_author` is absent here
/// and present for the same file in a domain that takes changes directly.
#[tokio::test]
async fn the_share_preview_lists_the_overlay_paths_without_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/authored.md", AUTHORED).await;
    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    drafted(&eng, tmp.path(), "alice", "notes/alice.md", DRAFT_ALICE).await;

    let plan = eng
        .origin_share_preview(
            "team",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    let changes = plan["changes"].as_array().unwrap();
    let paths: Vec<&str> = changes
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["notes/authored.md", "notes/plan.md"]);
    for change in changes {
        assert_eq!(
            change["last_author"],
            serde_json::Value::Null,
            "a reviewing domain preselects nothing: {plan}"
        );
    }

    // The same file in a domain that takes changes directly DOES carry its
    // provenance, so the absence above is a decision and not an empty column.
    let direct_tmp = tempfile::tempdir().unwrap();
    let direct_mock = Arc::new(MockProvider::new());
    let commit = direct_mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    direct_mock.set_branch("main", &commit);
    let direct_root = direct_tmp.path().join("kb");
    let direct = engine_with(
        &direct_tmp.path().join("config.yaml"),
        &direct_tmp.path().join("origins"),
        direct_mock,
        true,
        false,
    )
    .await;
    direct
        .origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(direct_root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
    std::fs::create_dir_all(direct_root.join("notes")).unwrap();
    std::fs::write(direct_root.join("notes/authored.md"), AUTHORED).unwrap();
    let direct_plan = direct
        .origin_share_preview(
            "kb",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    // Looked up by path: the MANIFEST connecting as `kb` wrote the name into
    // travels in the same plan.
    let authored = direct_plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == "notes/authored.md")
        .unwrap_or_else(|| panic!("the authored engram is in the plan: {direct_plan}"));
    assert_eq!(authored["last_author"], "human:ada", "{direct_plan}");
}

/// A base path the state directory holds no copy of is an error naming the way
/// out, never a quiet read of the folder instead.
///
/// The base snapshot's copies are what makes "exactly this actor's draft" true:
/// read the folder for one of them and a stray direct edit of that path would
/// travel as this actor's work. Every path a pull records is written to both in
/// lockstep, so a missing copy is a state directory somebody damaged, and a
/// resync is the answer.
#[tokio::test]
async fn a_base_path_with_no_recorded_copy_asks_for_a_resync() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;

    // The recorded copy goes missing while the path stays in the snapshot.
    let copy = tmp
        .path()
        .join("origins")
        .join("team")
        .join("base")
        .join("notes")
        .join("plan.md");
    assert!(copy.exists(), "the pull recorded a base copy");
    std::fs::remove_file(&copy).unwrap();

    let err = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("a damaged state directory is not a share");
    let text = err.to_string();
    assert!(text.contains("notes/plan.md"), "{text}");
    assert!(text.contains("resync"), "{text}");
}

/// A draft path is held to the rule every write and move verb holds one to, and
/// to no stricter rule of the share's own.
///
/// `notes/plan: v2.md` is a filename a person can choose and this product
/// indexes like any other, so the staging screen is `is_within_domain` rather
/// than the archive-and-attachment rule beside it. What such a path then meets
/// is the repository path rule in `crystalline_remote`, which refuses a colon
/// segment because it is a drive or stream marker on Windows - and it refuses it
/// for a file in a folder exactly as it does for a draft. So the two domains
/// answer the same thing, which is the point: review mode adds no screen of its
/// own.
///
/// Unix only: a colon is not a legal filename character on Windows, so the
/// fixture cannot exist there. `write_staged_file` and the plain
/// `std::fs::write` below both go through NTFS's alternate-data-stream syntax
/// instead of failing - `plan: v2.md` lands as a file named `plan` carrying a
/// hidden `: v2.md` stream - so the share sees a clean, colon-free `notes/plan`
/// and proceeds instead of refusing, which is an OS quirk this fixture cannot
/// route around, not a difference in the screening rule itself.
#[cfg(unix)]
#[tokio::test]
async fn a_draft_path_is_screened_the_way_a_file_in_the_folder_is() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/plan: v2.md", DRAFT_FRESH).await;

    let drafted = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("the repository path rule refuses a colon segment");

    // The same filename, as a file in a domain that takes changes directly.
    let direct_tmp = tempfile::tempdir().unwrap();
    let direct_mock = Arc::new(MockProvider::new());
    let commit = direct_mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    direct_mock.set_branch("main", &commit);
    let direct_root = direct_tmp.path().join("kb");
    let direct = engine_with(
        &direct_tmp.path().join("config.yaml"),
        &direct_tmp.path().join("origins"),
        direct_mock,
        true,
        false,
    )
    .await;
    direct
        .origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(direct_root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
    std::fs::create_dir_all(direct_root.join("notes")).unwrap();
    std::fs::write(direct_root.join("notes/plan: v2.md"), DRAFT_FRESH).unwrap();
    let on_disk = direct
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("the same rule, for a file in the folder");

    assert_eq!(
        drafted.to_string(),
        on_disk.to_string(),
        "review mode adds no screen of its own"
    );
    assert!(
        !drafted.to_string().contains("not inside the domain"),
        "the engine does not invent a refusal for a legal domain path: {drafted}"
    );
}

// --- convergence, conflicts and withdraw into the overlay ---------------------
//
// A draft ends one of two ways: the team merges it and a later pull brings it
// back, which takes it out of the overlay, or it goes on standing against a
// folder that has moved under it, which is its author's conflict and nobody
// else's. Both are decided by the convergence pass that runs after a pull that
// advanced the base, and both are asserted here in the row AND in the mirror
// beside it: a row cleared while its mirror stays would put the draft back on
// the next `reindex --wipe`.

/// The file one actor's draft is mirrored in, under the state directory these
/// fixtures point at: `<tmp>/overlays/team/<actor>/<path>`.
fn journal_file(tmp: &Path, actor: &str, path: &str) -> std::path::PathBuf {
    let mut file = tmp.join("overlays").join("team").join(actor);
    for seg in path.split('/') {
        file.push(seg);
    }
    file
}

/// Mirror a draft the way `write_overlay_entry` does after the row lands. The
/// `draft` helper above writes the row alone on purpose; a scenario about
/// convergence needs both halves, because clearing exactly one of them is the
/// bug it exists to catch.
pub(super) fn mirror(tmp: &Path, actor: &str, path: &str, text: &str) {
    let file = journal_file(tmp, actor, path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// One actor's overlay rows in `team`, ordered as the store returns them.
async fn overlay_entries(eng: &Engine, actor: &str) -> Vec<crystalline_index::StoredEngram> {
    let store = eng.store();
    let store = store.lock().await;
    let id = store
        .domain_id("team")
        .await
        .unwrap()
        .expect("team is indexed");
    store.overlay_entries(id, actor).await.unwrap()
}

/// The paths one actor holds, for a readable assertion.
async fn overlay_paths(eng: &Engine, actor: &str) -> Vec<String> {
    overlay_entries(eng, actor)
        .await
        .into_iter()
        .map(|e| e.path)
        .collect()
}

/// The team merges a draft and the next pull brings it back: the draft is the
/// folder now, so it stops being a draft.
///
/// The row and the mirror go together. A draft at a path the pull never touched
/// is left exactly where it stands.
#[tokio::test]
async fn a_merged_and_pulled_draft_converges_out_of_the_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH);

    // The team reviewed the rewrite and merged it.
    let merged = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", DRAFT_PLAN.as_bytes().to_vec()),
    ]));
    mock.set_branch("main", &merged);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        overlay_paths(&eng, "owner").await,
        vec!["notes/fresh.md".to_string()],
        "the merged draft left the overlay and the unmerged one stayed"
    );
    assert!(
        !journal_file(tmp.path(), "owner", "notes/plan.md").exists(),
        "the mirror went with the row, or the next wipe puts the draft back"
    );
    assert!(
        journal_file(tmp.path(), "owner", "notes/fresh.md").exists(),
        "the draft that is still a draft still has its mirror"
    );

    let status = eng
        .origin_status(Some("team"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &status["domains"][0];
    assert_eq!(domain["converged"]["cleared"], 1, "{status}");
    assert_eq!(domain["converged"]["diverged"], 0, "{status}");
    assert_eq!(domain["my_drafts"], 1, "the count reads the rows: {status}");
}

/// What one actor's files overlay holds, as `(path, tombstone)` pairs ordered
/// by path.
fn files_held(tmp: &Path, actor: &str) -> Vec<(String, bool)> {
    let dir = tmp.join("overlays/team").join(actor).join("files");
    let mut out = Vec::new();
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, bool)>) {
        let Ok(listed) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in listed.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.path().is_dir() {
                walk(&entry.path(), &rel, out);
            } else if let Some(base) = rel.strip_suffix(".tombstone") {
                out.push((base.to_string(), true));
            } else {
                out.push((rel, false));
            }
        }
    }
    walk(&dir, "", &mut out);
    out.sort();
    out
}

/// A pull catches the folder up with a draft file, and the draft stops being
/// one - both ways round.
///
/// A file whose bytes are now the team's own is settled exactly as a merged
/// page is, and a deletion of a file the team has now deleted too is settled
/// with nothing left to mark: a marker over a base nobody holds says nothing
/// at all.
#[tokio::test]
async fn a_pull_converges_a_byte_equal_file_and_a_sidecar_of_a_gone_base_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", team_plan()),
            ("assets/old.png", OLD_PNG.to_vec()),
        ],
    )
    .await;

    file(&eng, "alice", "assets/deck.png", DECK_PNG).await;
    delete_file(&eng, "alice", "assets/old.png").await;
    assert_eq!(
        files_held(tmp.path(), "alice"),
        vec![
            ("assets/deck.png".to_string(), false),
            ("assets/old.png".to_string(), true),
        ],
        "both halves stand before the pull"
    );

    // The team merged her deck and deleted the old one.
    let merged = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
        ("assets/deck.png", DECK_PNG.to_vec()),
    ]));
    mock.set_branch("main", &merged);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        files_held(tmp.path(), "alice"),
        Vec::new(),
        "the folder says what she said, so she is drafting neither of them now"
    );
    let status = eng
        .origin_status(Some("team"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &status["domains"][0];
    assert_eq!(domain["converged"]["cleared"], 2, "{status}");
    assert_eq!(domain["converged"]["diverged"], 0, "{status}");
}

/// The team changes a file under the actor drafting it, and that is their
/// conflict and nobody else's. Writing the path again does not settle it; her
/// choice in a resolution does.
#[tokio::test]
async fn a_file_the_team_changed_under_the_actor_is_that_actors_divergence() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    file(&eng, "alice", "assets/deck.png", DECK_PNG).await;
    file(&eng, "bob", "assets/notes.png", DECK_PNG).await;

    // Somebody else's deck landed at the same path instead.
    let theirs = b"\x89PNG\r\n\x1a\n\x00the team's own deck".to_vec();
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
        ("assets/deck.png", theirs.clone()),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    // Nothing was taken away from her.
    assert_eq!(
        files_held(tmp.path(), "alice"),
        vec![("assets/deck.png".to_string(), false)],
        "a conflict is reported, never resolved behind the author's back"
    );

    let hers = eng
        .origin_status(Some("team"), false, false, &scope_of("alice"))
        .await
        .unwrap();
    assert_eq!(
        hers["domains"][0]["converged"]["mine"],
        serde_json::json!(["assets/deck.png"]),
        "her own conflict, named beside the pages: {hers}"
    );
    let his = eng
        .origin_status(Some("team"), false, false, &scope_of("bob"))
        .await
        .unwrap();
    assert_eq!(
        his["domains"][0]["converged"]["mine"],
        serde_json::json!([]),
        "and nobody else's: {his}"
    );

    // Somebody with no draft at the path has nothing to settle there.
    let refused = eng
        .origin_resolve(
            "team",
            "assets/deck.png",
            Some("mine"),
            None,
            ShareActor::Owner,
        )
        .await
        .expect_err("the owner holds no draft of this file");
    assert!(matches!(refused, EngineError::NotFound(_)), "{refused}");

    // Her next write of the same path does not settle it.
    file(&eng, "alice", "assets/deck.png", b"a third deck").await;
    let hers = eng
        .origin_status(Some("team"), false, false, &scope_of("alice"))
        .await
        .unwrap();
    assert_eq!(
        hers["domains"][0]["converged"]["mine"],
        serde_json::json!(["assets/deck.png"]),
        "an upload is not a choice: {hers}"
    );

    // Keep mine is.
    let settled = eng
        .origin_resolve(
            "team",
            "assets/deck.png",
            Some("mine"),
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(settled["kept"], "mine", "{settled}");
    let hers = eng
        .origin_status(Some("team"), false, false, &scope_of("alice"))
        .await
        .unwrap();
    assert!(
        !hers.to_string().contains("assets/deck.png"),
        "her conflict is named nowhere now: {hers}"
    );
}

/// The folder moves under a draft and the draft is its author's conflict.
///
/// Whoever owns the domain is told how many each actor is holding open, and
/// never what any of them says.
#[tokio::test]
async fn a_diverged_draft_is_its_authors_conflict_only() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    drafted(&eng, tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN);
    // A draft at a path this pull never touches: not merged, not in conflict.
    drafted(&eng, tmp.path(), "alice", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "alice", "notes/fresh.md", DRAFT_FRESH);

    // Somebody else's change to the same page landed instead.
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the plan as the team now has it"),
        ),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    // Nothing was taken away from her.
    assert_eq!(
        overlay_paths(&eng, "alice").await,
        vec!["notes/fresh.md".to_string(), "notes/plan.md".to_string()],
        "a conflict is reported, never resolved behind the author's back"
    );

    let hers = eng
        .origin_status(
            Some("team"),
            false,
            false,
            &Scope::User {
                account: "alice".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    let domain = &hers["domains"][0];
    assert_eq!(domain["converged"]["diverged"], 1, "{hers}");
    assert_eq!(
        domain["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "her own conflict, named: {hers}"
    );
    // Who may see the per-actor counts is Task 8's rule and not a second one
    // of this key's own: the same gate `drafts` already answers to, so the two
    // keys can never disagree about who is allowed to read them.
    assert_eq!(
        domain["converged"]["actors"].is_null(),
        domain["drafts"].is_null(),
        "the counts follow the draft counts beside them: {hers}"
    );

    // Somebody who is drafting nothing here has no conflict of their own.
    let theirs = eng
        .origin_status(
            Some("team"),
            false,
            false,
            &Scope::User {
                account: "bob".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        theirs["domains"][0]["converged"]["mine"],
        serde_json::json!([]),
        "{theirs}"
    );

    // Whoever owns the domain sees the counts, and no content at all.
    let owners = eng
        .origin_status(Some("team"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &owners["domains"][0];
    assert_eq!(
        domain["converged"]["actors"],
        serde_json::json!([{ "actor": "alice", "entries": 1 }]),
        "{owners}"
    );
    assert!(
        !domain["converged"]["actors"].to_string().contains("plan"),
        "an owner counts another actor's conflicts and reads none of them: {owners}"
    );
}

/// A pull that renames a base path takes the draft standing over it along.
///
/// The address is the signal: the base at the old path is gone and a base the
/// pull brought in answers to the address the draft holds, so the page moved
/// and the draft still applies to it. Row and mirror move as one.
#[tokio::test]
async fn a_renamed_base_path_takes_the_draft_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    drafted(&eng, tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN);

    // The team filed the same page under a new name.
    let renamed = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan-v2.md", team_plan()),
    ]));
    mock.set_branch("main", &renamed);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        overlay_paths(&eng, "alice").await,
        vec!["notes/plan-v2.md".to_string()],
        "the draft travelled with the page it is a draft of"
    );
    assert!(
        journal_file(tmp.path(), "alice", "notes/plan-v2.md").exists(),
        "the mirror moved with the row"
    );
    assert!(
        !journal_file(tmp.path(), "alice", "notes/plan.md").exists(),
        "and nothing was left at the old path for a wipe to resurrect"
    );
}

/// A pull that brings a base file answering to an address a live draft already
/// holds elsewhere is that author's conflict.
///
/// It is never a silent drop and never a rewrite of what the team reviewed:
/// search merges its hits by permalink, so two rows answering to one address
/// would lose one of them without a word.
#[tokio::test]
async fn a_pulled_address_collision_is_the_drafters_divergence() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(tmp.path(), mock.clone(), &[("MANIFEST.md", manifest())]).await;
    let root = tmp.path().join("team-knowledge");

    // A page only this actor has, answering to `plan`.
    drafted(&eng, tmp.path(), "owner", "notes/mine.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/mine.md", DRAFT_PLAN);

    // The team adds a page of its own at that address, under another name.
    let collided = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
    ]));
    mock.set_branch("main", &collided);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        overlay_paths(&eng, "owner").await,
        vec!["notes/mine.md".to_string()],
        "the draft stands where its author left it"
    );
    assert_eq!(
        std::fs::read(root.join("notes/plan.md")).unwrap(),
        team_plan(),
        "and the folder still says what the team reviewed"
    );

    let status = eng
        .origin_status(Some("team"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/mine.md"]),
        "{status}"
    );
}

/// Withdrawing a shared proposal with `revert` puts the actor's own view back,
/// and the folder the team reviewed is never touched.
#[tokio::test]
async fn withdraw_revert_lands_in_the_overlay_while_the_tree_stays() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH);

    let shared = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "proposed", "{shared}");

    let report = eng
        .origin_withdraw("team", None, true, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(report["closed"], true, "{report}");

    assert!(
        overlay_paths(&eng, "owner").await.is_empty(),
        "the revert put this actor's view back to the team's folder"
    );
    assert!(
        !journal_file(tmp.path(), "owner", "notes/plan.md").exists()
            && !journal_file(tmp.path(), "owner", "notes/fresh.md").exists(),
        "row and mirror went together"
    );

    // The folder is exactly what it was: a revert in a reviewing domain has
    // nothing to say about what the team reviewed.
    assert_eq!(
        std::fs::read(root.join("notes/plan.md")).unwrap(),
        team_plan()
    );
    assert!(!root.join("notes/fresh.md").exists());
}

/// A conflict resolved in a reviewing domain is a draft, not a write of the
/// folder: keeping the team's version ends the draft, and merged content
/// becomes the author's new draft.
#[tokio::test]
async fn a_resolution_in_a_reviewing_domain_is_a_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);

    let merged = eng
        .origin_resolve(
            "team",
            "notes/plan.md",
            None,
            Some(DRAFT_FRESH.as_bytes()),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(merged["draft"], true, "{merged}");
    let entries = overlay_entries(&eng, "owner").await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].content, DRAFT_FRESH, "{merged}");
    assert_eq!(
        std::fs::read(root.join("notes/plan.md")).unwrap(),
        team_plan(),
        "the folder the team reviewed is never what a resolution writes"
    );

    // Taking the team's version ends the draft entirely.
    let theirs = eng
        .origin_resolve(
            "team",
            "notes/plan.md",
            Some("theirs"),
            None,
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(theirs["draft"], true, "{theirs}");
    assert!(overlay_paths(&eng, "owner").await.is_empty());
    assert!(!journal_file(tmp.path(), "owner", "notes/plan.md").exists());
}

/// Conflicts and withdrawals are somebody's, so an agent with no identity is
/// refused in the words every draft verb refuses in.
#[tokio::test]
async fn an_http_agent_without_identity_cannot_resolve_or_withdraw_a_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;

    let err = eng
        .origin_resolve(
            "team",
            "notes/plan.md",
            Some("theirs"),
            None,
            ShareActor::HttpAgent,
        )
        .await
        .expect_err("nobody's conflict is nobody's to settle");
    assert_eq!(err.to_string(), crystalline_service::OVERLAY_NEEDS_IDENTITY);

    let err = eng
        .origin_withdraw("team", None, true, ShareActor::HttpAgent)
        .await
        .expect_err("nobody's proposal is nobody's to withdraw");
    assert_eq!(err.to_string(), crystalline_service::OVERLAY_NEEDS_IDENTITY);
}

/// A domain that takes changes directly holds no drafts, so the pass a pull
/// runs has nothing to do and touches nothing.
#[tokio::test]
async fn a_pull_into_a_domain_that_takes_changes_directly_converges_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);
    let root = tmp.path().join("kb");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock.clone(),
        true,
        false,
    )
    .await;
    eng.origin_add(
        "acme/kb",
        Some("kb"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();

    let status = eng
        .origin_status(Some("kb"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &status["domains"][0];
    assert!(
        domain["converged"].is_null(),
        "a domain nobody drafts in says nothing about drafts: {status}"
    );
    assert!(
        !tmp.path().join("overlays").exists(),
        "and no journal was made"
    );
}

/// The clear-only pass ends what has landed and calls nothing a conflict.
///
/// It is the whole of what can be said with no pull to attribute a divergence
/// to: a draft the folder has caught up with is over, and a draft that says
/// something else is unshared work, which is not a conflict and never was.
#[tokio::test]
async fn the_clear_only_pass_ends_what_has_landed_and_flags_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", DRAFT_PLAN.as_bytes().to_vec()),
            ("notes/old.md", engram("Old", "old", "on its way out")),
        ],
    )
    .await;

    // A draft the folder already says, a draft it does not, and a deletion of
    // a page the folder still has.
    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH);
    tombstoned(&eng, tmp.path(), "owner", "notes/old.md").await;

    let report = eng.converge_overlays("team").await.unwrap();
    assert_eq!(report.cleared, 1, "the draft the folder caught up with");
    assert_eq!(report.diverged, 0, "and nothing is anybody's conflict");
    assert_eq!(
        overlay_paths(&eng, "owner").await,
        vec!["notes/fresh.md".to_string(), "notes/old.md".to_string()]
    );

    // A domain that takes changes directly answers the same call with zeroes
    // and touches nothing at all.
    let quiet = eng.converge_overlays("no-such-domain").await.unwrap();
    assert_eq!(quiet, crystalline_service::ConvergenceReport::default());
}

/// A convergence pass that stops on an error still announces the drafts it
/// already ended: the batch guard is pushed each drop right after it lands, so
/// a later entry failing does not swallow the event for an earlier one. The
/// second draft's base copy is a directory, which no read turns into bytes, at
/// a key of the snapshot.
#[tokio::test]
async fn a_convergence_pass_that_stops_halfway_announces_what_it_cleared() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", DRAFT_PLAN.as_bytes().to_vec()),
        ],
    )
    .await;
    drafted(&eng, tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);
    drafted(&eng, tmp.path(), "owner", "notes/zz.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/zz.md", DRAFT_FRESH);
    let base = base_copy_of(&tmp.path().join("origins"), "notes/plan.md")
        .expect("the first pull wrote the base snapshot");
    std::fs::create_dir_all(base.join("notes/zz.md")).unwrap();
    // A key of the snapshot, so the pass reads its copy: a path that is no key
    // is absent without a read.
    let state_dir = base.parent().unwrap();
    let mut state = crystalline_remote::state::OriginState::load(state_dir)
        .unwrap()
        .unwrap();
    state.files.insert(
        "notes/zz.md".to_string(),
        crystalline_remote::state::BaseStamp {
            sha256: crate::support::sha256_hex(DRAFT_FRESH.as_bytes()),
            size: DRAFT_FRESH.len() as u64,
        },
    );
    state.save(state_dir).unwrap();

    let mut rx = eng.changes().subscribe();
    eng.converge_overlays("team")
        .await
        .expect_err("the second draft's base copy cannot be read");
    let mut dropped = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let crystalline_service::changes::Change::Engram(change) = envelope.change {
            dropped.push((change.path, change.draft_of));
        }
    }
    assert_eq!(
        dropped,
        vec![("notes/plan.md".to_string(), Some("owner".to_string()))],
        "the draft the pass ended before it stopped is announced"
    );
}

/// The `base` folder under `origins` that holds a snapshot copy of `rel`.
fn base_copy_of(origins: &Path, rel: &str) -> Option<std::path::PathBuf> {
    let mut dirs = vec![origins.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let base = dir.join("base");
        if base.join(rel).is_file() {
            return Some(base);
        }
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            if entry.path().is_dir() {
                dirs.push(entry.path());
            }
        }
    }
    None
}

/// A conflict the pull left in the reviewed folder is still settled against the
/// folder, even while the domain reviews changes.
///
/// It is nobody's draft: somebody edited the team's own files out of band and
/// upstream then changed the same file, so what settles it is the resolution
/// the verb has always performed. Review mode is not a reason to leave a
/// conflict standing in the team's files with no verb that can reach it - and
/// the drafts beside it are untouched by that resolution.
#[tokio::test]
async fn an_out_of_band_conflict_is_still_settled_against_the_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    // Somebody's draft, which takes no part in this at all.
    drafted(&eng, tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH);

    // A direct edit of the reviewed folder, and an upstream change to the same
    // file: the pull records a conflict nobody drafted.
    std::fs::write(
        root.join("notes/plan.md"),
        engram("Plan", "plan", "edited straight on disk"),
    )
    .unwrap();
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", engram("Plan", "plan", "changed upstream")),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    let state = OriginState::load(&tmp.path().join("origins").join("team"))
        .unwrap()
        .unwrap();
    assert_eq!(state.conflicts.len(), 1, "the pull recorded a conflict");

    let settled = eng
        .origin_resolve(
            "team",
            "notes/plan.md",
            Some("theirs"),
            None,
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(settled["remaining"], 0, "{settled}");
    assert!(
        settled["draft"].is_null(),
        "settling the folder's conflict is not a draft: {settled}"
    );
    assert_eq!(
        std::fs::read(root.join("notes/plan.md")).unwrap(),
        engram("Plan", "plan", "changed upstream"),
        "the folder took the team's version"
    );
    assert_eq!(
        overlay_paths(&eng, "owner").await,
        vec!["notes/fresh.md".to_string()],
        "and the draft beside it was never part of the question"
    );

    // A path that is neither a draft of this caller's nor a recorded conflict
    // teaches the way in rather than resolving something.
    let err = eng
        .origin_resolve(
            "team",
            "notes/nothing.md",
            Some("theirs"),
            None,
            ShareActor::Owner,
        )
        .await
        .expect_err("there is nothing there to settle");
    let text = err.to_string();
    assert!(
        text.contains("not drafting") && text.contains("draft the change first"),
        "{text}"
    );
}

// --- the convergence record survives the pulls that are about something else -
//
// A conflict is a fact about one actor's draft, not about the pull that
// happened to notice it. So the record is merged rather than rebuilt, and it
// lives beside the journal that mirrors the drafts it describes, which is what
// makes it survive a restart.

/// A pull about an unrelated path leaves a standing conflict standing.
#[tokio::test]
async fn an_unrelated_pull_leaves_a_standing_conflict_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", team_plan()),
            (
                "notes/other.md",
                engram("Other", "other", "untouched so far"),
            ),
        ],
    )
    .await;

    drafted(&eng, tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN);

    // Pull A moves the page ada is drafting: her draft is a conflict.
    let first = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the team moved it on"),
        ),
        (
            "notes/other.md",
            engram("Other", "other", "untouched so far"),
        ),
    ]));
    mock.set_branch("main", &first);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    let status = eng
        .origin_status(Some("team"), false, false, &ada)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "{status}"
    );

    // Pull B is about a page nobody is drafting.
    let second = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the team moved it on"),
        ),
        (
            "notes/other.md",
            engram("Other", "other", "changed upstream"),
        ),
    ]));
    mock.set_branch("main", &second);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    let status = eng
        .origin_status(Some("team"), false, false, &ada)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "a pull about another page does not settle ada's conflict: {status}"
    );
    assert_eq!(status["domains"][0]["converged"]["diverged"], 1, "{status}");
}

/// The clear-only pass ends what has landed and leaves every conflict standing.
#[tokio::test]
async fn the_clear_only_pass_leaves_the_conflicts_it_did_not_settle() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    drafted(&eng, tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN);

    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the team moved it on"),
        ),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    // Somebody else's draft has since become the folder, and a maintenance
    // sweep ends it. Ada's conflict is nobody's business of that sweep's.
    draft(
        &eng,
        "bo",
        "notes/plan.md",
        &String::from_utf8(engram("Plan", "plan", "the team moved it on")).unwrap(),
    )
    .await;
    mirror(
        tmp.path(),
        "bo",
        "notes/plan.md",
        &String::from_utf8(engram("Plan", "plan", "the team moved it on")).unwrap(),
    );
    let swept = eng.converge_overlays("team").await.unwrap();
    assert_eq!(swept.cleared, 1, "bo's draft is the folder now");

    let status = eng
        .origin_status(
            Some("team"),
            false,
            false,
            &Scope::User {
                account: "ada".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "the sweep settled nothing of ada's: {status}"
    );
}

/// The record lives beside the journal, so a restart still knows the conflicts.
#[tokio::test]
async fn a_restart_still_names_the_conflicts_the_last_pull_found() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    drafted(&eng, tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN);
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the team moved it on"),
        ),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    // A second engine over the same database, the same config file and the
    // same state directory: this process forgot everything it held in memory.
    let restarted = Engine::new(
        eng.store(),
        config(true),
        None,
        Some(tmp.path().join("config.yaml")),
    )
    .with_origin_provider(mock.clone())
    .with_origins_dir(tmp.path().join("origins"))
    .with_state_dir(tmp.path().to_path_buf());

    let status = restarted
        .origin_status(
            Some("team"),
            false,
            false,
            &Scope::User {
                account: "ada".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "a restart reads the conflicts off disk: {status}"
    );
}

/// While a domain reviews, an open proposal is somebody's, and the refusal says
/// whose.
///
/// Two different answers, because there are two different ways forward: the
/// proposal is yours and you may withdraw it, or it is somebody else's and it
/// is not yours to withdraw.
#[tokio::test]
async fn an_open_proposal_refuses_the_next_share_in_the_words_of_whose_it_is() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    mock.enable_stacks();
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    drafted(&eng, tmp.path(), "ada", "notes/ada.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "ada", "notes/ada.md", DRAFT_FRESH);
    let first = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("ada".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(first["outcome"], "proposed", "{first}");

    // Bo has never shared anything. The refusal must not tell him to do the
    // thing he is doing.
    drafted(&eng, tmp.path(), "bo", "notes/bo.md", DRAFT_ALICE).await;
    mirror(tmp.path(), "bo", "notes/bo.md", DRAFT_ALICE);
    let his = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("bo".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(his["outcome"], "proposal_open", "{his}");
    assert_eq!(his["own"], false, "{his}");
    let text = his["guidance"].as_str().unwrap().to_string();
    assert!(
        text.starts_with("ada's proposal #") && text.contains("merged or closed"),
        "the answer names whose it is and what to wait for: {text}"
    );
    assert!(
        !text.contains("share a fresh proposal instead"),
        "and never tells him to do what he was doing: {text}"
    );

    // Ada's own second share is the one that may withdraw and share again.
    drafted(&eng, tmp.path(), "ada", "notes/more.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "ada", "notes/more.md", DRAFT_PLAN);
    let err = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("ada".to_string()),
        )
        .await
        .expect_err("stacking on her own open layer is not served while the domain reviews");
    let text = err.to_string();
    assert!(
        text.contains("this domain reviews changes")
            && text.contains("withdraw it and share again"),
        "{text}"
    );

    // The record is written whole, the conflicts and the proposal owners beside
    // them, so a pass that only settles drafts must not forget whose the
    // proposal was.
    eng.converge_overlays("team").await.unwrap();
    let err = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("ada".to_string()),
        )
        .await
        .expect_err("her own proposal is still open");
    assert!(
        err.to_string().contains("withdraw it and share again"),
        "a convergence pass did not write over who owns the proposal: {err}"
    );
}

/// The accounts database wired into an engine, with one share-link on one
/// actor's draft already redeemed by `bob`.
///
/// Engine-level rather than over HTTP, because what is under test is the
/// ENGINE seam: a pull converging or renaming somebody's draft never passes a
/// route at all, and a test that drove one would be asserting about the wrong
/// layer.
async fn granted(eng: &Engine, tmp: &Path, actor: &str, path: &str) -> Arc<AuthStore> {
    let auth = Arc::new(AuthStore::open(&tmp.join("web-auth.db")).await.unwrap());
    for name in [actor, "bob"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    let minted = auth
        .mint_overlay_grant("team", path, actor, None)
        .await
        .unwrap();
    auth.redeem_overlay_grant(&minted.token, "bob")
        .await
        .unwrap()
        .expect("bob holds the link");
    auth
}

/// A pull that converges a granted draft out of the overlay ends its links.
///
/// The draft is the folder now, so there is nothing left to share: everybody
/// who can read the domain reads that text anyway. A link left live would be a
/// link to a draft that is not there, waiting to spring back onto whatever its
/// author drafts at that path next - and this path never touches the discard,
/// the fold or the move verb, which is exactly why the ending belongs at the
/// seam they all pass through rather than at each of them.
#[tokio::test]
async fn a_pulled_convergence_ends_the_links_on_the_draft_it_cleared() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    drafted(&eng, tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN);
    let auth = granted(&eng, tmp.path(), "alice", "notes/plan.md").await;
    assert_eq!(
        auth.overlay_grants_held("bob", "team").await.unwrap().len(),
        1,
        "bob holds the link before the pull"
    );

    // The team reviewed the rewrite and merged it.
    let merged = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", DRAFT_PLAN.as_bytes().to_vec()),
    ]));
    mock.set_branch("main", &merged);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert!(
        auth.overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "the link ended with the draft the pull cleared"
    );

    // And a later draft at the same path revives nothing.
    drafted(&eng, tmp.path(), "alice", "notes/plan.md", DRAFT_FRESH).await;
    assert!(
        auth.overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "a second draft at that path is alice's alone"
    );
}

/// And the rename a pull performs when the base carries the draft along ends
/// them too.
///
/// That rename is `move_draft_with_the_base`, which never goes through the move
/// verb - so it is the path a hook on the verb alone would miss, and the reason
/// the ending is at the drop rather than at the verbs.
#[tokio::test]
async fn convergences_rename_ends_the_links_on_the_path_it_left() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    drafted(&eng, tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN);
    let auth = granted(&eng, tmp.path(), "alice", "notes/plan.md").await;

    // The team filed the same page under a new name.
    let renamed = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan-v2.md", team_plan()),
    ]));
    mock.set_branch("main", &renamed);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        overlay_paths(&eng, "alice").await,
        vec!["notes/plan-v2.md".to_string()],
        "the draft travelled with the page it is a draft of"
    );
    assert!(
        auth.overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "and the link on the path it left ended with it: the author re-shares \
         the page under its new name"
    );
}

/// And the rename that makes the draft the folder ends them too, although it
/// writes no destination at all.
///
/// The other half of the same function, and the one a reader would skip. When
/// the team files the page under a new name and what it says there is word for
/// word what this actor was drafting, the draft has become the folder: there is
/// nothing left to write and nothing left to share, since everybody who can
/// read the domain reads that text anyway. The ending waits for the destination
/// on the path that has one, so this path has to end them where it returns -
/// which is the line a restructure loses.
#[tokio::test]
async fn a_convergence_that_makes_the_draft_the_folder_ends_its_links() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    // Her draft says exactly what the team's page says, which is what makes the
    // rename below land rather than move.
    let agreed = String::from_utf8(team_plan()).unwrap();
    drafted(&eng, tmp.path(), "alice", "notes/plan.md", &agreed).await;
    mirror(tmp.path(), "alice", "notes/plan.md", &agreed);
    let auth = granted(&eng, tmp.path(), "alice", "notes/plan.md").await;

    let renamed = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan-v2.md", team_plan()),
    ]));
    mock.set_branch("main", &renamed);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert!(
        overlay_paths(&eng, "alice").await.is_empty(),
        "her draft is the folder now, under its new name: {:?}",
        overlay_paths(&eng, "alice").await
    );
    assert!(
        auth.overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "and the link ended with the draft it was a link to"
    );
}

/// **A team domain owing work puts the sharing ask on the next write receipt,
/// ahead of the maintenance one** - and each ask lands once.
///
/// The order is the point. Both are armed here: the domain holds one
/// substantive change nobody has proposed, and a domain sits on the
/// maintenance backlog. The first receipt carries the sharing ask, because
/// unshared work is the team's problem until it is shared; the second carries
/// the maintenance one, because the first spent only the ask it emitted; the
/// third carries nothing, both being inside their cooldown.
#[tokio::test]
async fn unshared_team_work_puts_the_share_ask_on_a_write_receipt() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_indexes())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    // One substantive change the team has not seen.
    std::fs::write(
        root.join("added.md"),
        engram("Added", "added", "written here, never shared"),
    )
    .unwrap();

    // And a backlog, so the maintenance ask is armed beside it. The state
    // directory is the tempdir this engine was built with, never the machine's.
    let maintenance = tmp.path().join("hooks").join("maintenance.json");
    std::fs::create_dir_all(maintenance.parent().unwrap()).unwrap();
    std::fs::write(
        &maintenance,
        serde_json::json!({ "v": 1, "pending_domains": ["brand"] }).to_string(),
    )
    .unwrap();

    let first =
        crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &Scope::Unrestricted)
            .await
            .expect("a receipt with work to share carries the sharing ask");
    assert_eq!(
        first,
        crystalline_service::nudge::share_nudge_line(1, &["brand".to_string()]),
        "the sharing ask names the one domain and counts the one change"
    );

    let second =
        crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &Scope::Unrestricted)
            .await
            .expect("the maintenance ask was not spent by the sharing one");
    assert_eq!(second, crystalline_service::nudge::MCP_EVOLVE_NUDGE);

    assert!(
        crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &Scope::Unrestricted)
            .await
            .is_none(),
        "both asks are inside their cooldown now"
    );
}

/// **The sharing ask on a write receipt never names or counts a private team
/// domain the caller may not see.**
///
/// A direct-mode team domain's unshared count is a walk of its folder, which
/// holds everybody's work, so without a screen any account writing anywhere
/// would be told that `vault` exists and how much it owes. `brand` is the
/// control: it is visible and its one change is the whole of the stranger's
/// ask. The machine owner sees both, on its own cooldown key.
#[tokio::test]
async fn the_share_ask_names_no_team_domain_the_caller_may_not_see() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_indexes())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let brand_root = tmp.path().join("brand-knowledge");
    let vault_root = tmp.path().join("vault-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    for (repo, name, root) in [
        ("acme/brand-knowledge", "brand", &brand_root),
        ("acme/vault-knowledge", "vault", &vault_root),
    ] {
        eng.origin_add(
            repo,
            Some(name),
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
    }
    std::fs::write(
        brand_root.join("added.md"),
        engram("Added", "added", "written here, never shared"),
    )
    .unwrap();
    for slug in ["secret-one", "secret-two"] {
        std::fs::write(
            vault_root.join(format!("{slug}.md")),
            engram(slug, slug, "private work, never shared"),
        )
        .unwrap();
    }

    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("keeper", "keeper", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    auth.add_user("ada", "ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.set_domain_visibility("vault", true, "keeper")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(DomainAccess::new(auth)));
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    // Guard against a vacuous pass: ada really has `vault` hidden.
    assert_eq!(
        eng.hidden_domains(&ada).await.unwrap(),
        Some(std::collections::HashSet::from(["vault".to_string()])),
    );

    let hers = crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &ada)
        .await
        .expect("brand's change is hers to be asked about");
    assert_eq!(
        hers,
        crystalline_service::nudge::share_nudge_line(1, &["brand".to_string()]),
        "the ask names and counts only the domain ada may see"
    );

    let mine = crystalline_service::nudge::write_verb_trailer(&eng, None, &Scope::Unrestricted)
        .await
        .expect("the machine owner is asked about both");
    assert_eq!(
        mine,
        crystalline_service::nudge::share_nudge_line(
            3,
            &["brand".to_string(), "vault".to_string()]
        ),
        "the screen is about the caller, not the domain"
    );
}

/// **A read-only team domain gets neither ask on a write receipt**: its
/// unshared work is not announced (nothing local can change there) and its
/// pending mark does not arm the maintenance ask, while a writable domain
/// beside it keeps the install in scope. Switched off, both asks come back.
#[tokio::test]
async fn a_read_only_team_domain_gets_neither_ask_on_a_write_receipt() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_indexes())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let brand_root = tmp.path().join("brand-knowledge");
    let vault_root = tmp.path().join("vault-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    for (repo, name, root) in [
        ("acme/brand-knowledge", "brand", &brand_root),
        ("acme/vault-knowledge", "vault", &vault_root),
    ] {
        eng.origin_add(
            repo,
            Some(name),
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
    }
    std::fs::write(
        brand_root.join("added.md"),
        engram("Added", "added", "written here, never shared"),
    )
    .unwrap();
    let maintenance = tmp.path().join("hooks").join("maintenance.json");
    std::fs::create_dir_all(maintenance.parent().unwrap()).unwrap();
    std::fs::write(
        &maintenance,
        serde_json::json!({ "v": 1, "pending_domains": ["brand"] }).to_string(),
    )
    .unwrap();

    let manifest_path = brand_root.join("MANIFEST.md");
    let open = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(
        &manifest_path,
        open.replacen("\n---\n", "\nread_only: on\n---\n", 1),
    )
    .unwrap();
    assert!(eng.domain_is_read_only("brand").await, "the lock is real");

    assert_eq!(
        crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &Scope::Unrestricted)
            .await,
        None,
        "a locked domain owes no share and its pending mark arms no sweep"
    );

    std::fs::write(&manifest_path, &open).unwrap();
    assert_eq!(
        crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &Scope::Unrestricted)
            .await,
        Some(crystalline_service::nudge::share_nudge_line(
            1,
            &["brand".to_string()]
        )),
        "unlocked, the share ask is back"
    );
    assert_eq!(
        crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &Scope::Unrestricted)
            .await,
        Some(crystalline_service::nudge::MCP_EVOLVE_NUDGE.to_string()),
        "and so is the maintenance ask"
    );
}

/// **A team domain that owes nothing is walked at most once a minute**, however
/// many receipts go out in between.
///
/// Nothing is recorded when nothing is emitted - that rule is what keeps the
/// ask landing on the first write after there IS something to say - so the
/// sharing arm stays open on a machine whose team domains are fully shared, and
/// without a memo every single receipt would pay for a full tree read and hash.
/// The memo is what bounds that: the walk is paid once, and the receipts behind
/// it answer from what it found.
///
/// The file written between the two calls is the assertion that earns the test:
/// a second walk would have found it, so a second receipt that still says
/// nothing can only be the memo answering.
#[tokio::test]
async fn two_receipts_inside_the_memo_window_walk_the_tree_once() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_indexes())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    // Counted for this domain's own folder, which is this test's tempdir: a
    // sibling test in this binary walks its own team domain, and `cargo test`
    // runs the two as threads in one process.
    assert!(
        crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &Scope::Unrestricted)
            .await
            .is_none(),
        "a domain that owes nothing has nothing to ask about"
    );
    assert_eq!(
        crystalline_service::nudge::share_walks_for(&root, &origins_dir.join("brand")),
        1,
        "the first receipt pays for the walk"
    );

    std::fs::write(
        root.join("added.md"),
        engram("Added", "added", "written here, never shared"),
    )
    .unwrap();

    assert!(
        crystalline_service::nudge::write_verb_trailer(&eng, Some("ada"), &Scope::Unrestricted)
            .await
            .is_none(),
        "the second receipt answers from the memo, which has not seen the new file"
    );
    assert_eq!(
        crystalline_service::nudge::share_walks_for(&root, &origins_dir.join("brand")),
        1,
        "and it pays for no second walk inside the memo window"
    );
}

/// A full rename of a local domain respells the link a team domain holds to
/// it, and that rewrite is a local change of the team domain like any other
/// edit: it waits for the next share, and the rename opens no proposal.
#[tokio::test]
async fn a_full_rename_rewrites_a_team_domains_link_as_a_local_change() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/link.md",
            engram("Link", "link", "See [[eng:alpha]] first."),
        ),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    let eng_dir = tmp.path().join("eng");
    std::fs::create_dir_all(&eng_dir).unwrap();
    std::fs::write(eng_dir.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        eng_dir.join("alpha.md"),
        engram("Alpha", "alpha", "the runbook"),
    )
    .unwrap();
    eng.domain_add_local(Some("eng"), Some(eng_dir.to_str().unwrap()))
        .await
        .unwrap();
    let calls_before = mock.calls().len();

    let report = eng
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        report["rewritten"],
        serde_json::json!([{ "domain": "brand", "engrams": 1, "references": 1 }]),
        "{report}"
    );
    let text = std::fs::read_to_string(root.join("notes/link.md")).unwrap();
    assert!(text.contains("[[platform:alpha]]"), "{text}");
    let new_calls = mock.calls()[calls_before..].to_vec();
    assert!(
        !new_calls
            .iter()
            .any(|c| c.starts_with("create_") || c.contains("proposal")),
        "the rename opened nothing upstream: {new_calls:?}"
    );

    let status = eng
        .origin_status(Some("brand"), true, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &status["domains"][0];
    assert_eq!(
        entry["detail"]["modified"],
        serde_json::json!(["notes/link.md"]),
        "{entry}"
    );
    assert_eq!(entry["local_changes"], 1, "{entry}");
}

/// Finishing a MANIFEST rename respells the link a team domain holds to
/// the renamed domain, and that rewrite is a local change of the team
/// domain, waiting for the next share like the rewrite of a full rename.
#[tokio::test]
async fn the_respell_on_a_team_domain_becomes_local_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/link.md",
            engram("Link", "link", "See [[eng:alpha]] first."),
        ),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    let eng_dir = tmp.path().join("eng");
    std::fs::create_dir_all(&eng_dir).unwrap();
    std::fs::write(eng_dir.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        eng_dir.join("alpha.md"),
        engram("Alpha", "alpha", "the runbook"),
    )
    .unwrap();
    eng.domain_add_local(Some("eng"), Some(eng_dir.to_str().unwrap()))
        .await
        .unwrap();
    eng.adopt_domain_names().await.unwrap();
    let renamed = String::from_utf8(manifest()).unwrap().replacen(
        "status: current\n",
        "status: current\ndomain_name: platform\n",
        1,
    );
    std::fs::write(eng_dir.join("MANIFEST.md"), renamed).unwrap();
    eng.sync(Some("eng")).await.unwrap();
    eng.adopt_domain_names().await.unwrap();
    assert!(eng.rename_question("eng").is_some(), "the adoption asked");
    let calls_before = mock.calls().len();

    let report = eng
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        report["rewritten"],
        serde_json::json!([{ "domain": "brand", "engrams": 1, "references": 1 }]),
        "{report}"
    );
    let text = std::fs::read_to_string(root.join("notes/link.md")).unwrap();
    assert!(text.contains("[[platform:alpha]]"), "{text}");
    let new_calls = mock.calls()[calls_before..].to_vec();
    assert!(
        !new_calls
            .iter()
            .any(|c| c.starts_with("create_") || c.contains("proposal")),
        "the respell opened nothing upstream: {new_calls:?}"
    );

    let status = eng
        .origin_status(Some("brand"), true, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &status["domains"][0];
    assert_eq!(
        entry["detail"]["modified"],
        serde_json::json!(["notes/link.md"]),
        "{entry}"
    );
    assert_eq!(entry["local_changes"], 1, "{entry}");
}

fn manifest_read_only() -> Vec<u8> {
    String::from_utf8(manifest())
        .unwrap()
        .replacen("\n---\n", "\nread_only: on\n---\n", 1)
        .into_bytes()
}

/// The team repository stays the authority: a read-only copy keeps taking the
/// team's updates, the team switching the flag off takes effect at the pull,
/// and a share is refused while it holds.
#[tokio::test]
async fn a_pull_still_updates_a_read_only_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest_read_only()),
        ("notes/alpha.md", engram("Alpha", "alpha", "version one")),
    ]));
    mock.set_branch("main", &c1);
    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    assert!(eng.domain_is_read_only("brand").await);

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest_read_only()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version two, revised upstream"),
        ),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(root.join("notes/alpha.md"))
            .unwrap()
            .contains("version two")
    );

    assert!(
        eng.domain_is_read_only("brand").await,
        "the pull keeps the lock"
    );
    let calls_before = mock.calls().len();
    let share = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(matches!(share, EngineError::DomainReadOnly(_)), "{share}");
    // Every recorded call that is not one of the mock's reads counts, so a
    // write call added to the mock later is caught without a list change.
    let written: Vec<String> = mock.calls()[calls_before..]
        .iter()
        .filter(|c| {
            ![
                "branch_ref:",
                "proposal_feedback:",
                "list_open_proposals",
                "list_stacks",
            ]
            .iter()
            .any(|read| c.starts_with(read))
        })
        .cloned()
        .collect();
    assert!(
        written.is_empty(),
        "the refused share wrote nothing upstream: {written:?}"
    );

    let c3 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version two, revised upstream"),
        ),
    ]));
    mock.set_branch("main", &c3);
    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        !eng.domain_is_read_only("brand").await,
        "the team switched it off"
    );
}

/// A conflict on a read-only copy can take the team's side, which is the
/// direction a pull goes, and cannot keep this machine's side (decision D5).
#[tokio::test]
async fn a_conflict_takes_the_teams_side_on_a_read_only_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one")),
    ]));
    mock.set_branch("main", &c1);
    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    std::fs::write(root.join("notes/a.md"), engram("A", "a", "line one LOCAL")).unwrap();
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one UPSTREAM")),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    let manifest_path = root.join("MANIFEST.md");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(
        &manifest_path,
        text.replacen("\n---\n", "\nread_only: on\n---\n", 1),
    )
    .unwrap();

    let mine = eng
        .origin_resolve("brand", "notes/a.md", Some("mine"), None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(matches!(mine, EngineError::DomainReadOnly(_)), "{mine}");
    assert!(
        std::fs::read_to_string(root.join("notes/a.md"))
            .unwrap()
            .contains("line one LOCAL"),
        "a refused mine leaves the file as it was"
    );
    eng.origin_resolve(
        "brand",
        "notes/a.md",
        Some("theirs"),
        None,
        ShareActor::Owner,
    )
    .await
    .unwrap();
    assert!(
        std::fs::read_to_string(root.join("notes/a.md"))
            .unwrap()
            .contains("line one UPSTREAM")
    );
}

// --- standalone engines on a shared database ---------------------------------

/// A standalone engine (`open_standalone`, what every daemonless command
/// opens) over a Postgres schema of its own, as this machine, wired to the
/// mock forge. `None` with no Postgres URL.
#[cfg(feature = "postgres")]
async fn standalone_on_postgres(
    tmp: &Path,
    mock: Arc<MockProvider>,
) -> Option<(Engine, String, String)> {
    let url = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL").ok()?;
    if url.is_empty() {
        return None;
    }
    let schema = format!(
        "standalone_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    // Creates and migrates the schema the engine's URL points into.
    crystalline_index::PostgresStore::open_in_schema(&url, &schema)
        .await
        .unwrap();
    let sep = if url.contains('?') { '&' } else { '?' };
    let mut cfg = config(true);
    cfg.database = Some(crystalline_core::config::DatabaseConfig {
        backend: crystalline_core::config::DatabaseBackend::Postgres,
        url: Some(format!(
            "{url}{sep}options=-c%20search_path%3D{schema},public"
        )),
    });
    let config_path = tmp.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let loaded = crystalline_service::LoadedConfig {
        path: config_path,
        file: cfg.clone(),
        effective: cfg,
        overlay: EnvOverlay::default(),
    };
    let engine =
        crystalline_service::engine::open_standalone(loaded, &tmp.join("unused.db"), false)
            .await
            .unwrap()
            .with_origin_provider(mock)
            .with_origins_dir(tmp.join("origins"))
            .with_state_dir(tmp.join("state"));
    Some((engine, url, schema))
}

/// An origin update through a standalone engine on a shared database acts
/// as this machine: it claims the domain, indexes what it pulled and gives
/// the lock back. With another live instance holding the domain, the
/// update leaves the domain unindexed and says why in its answer.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_standalone_origin_update_on_postgres_indexes_what_it_pulled() {
    let _scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "version one")),
    ]));
    mock.set_branch("main", &c1);
    let Some((eng, url, schema)) = standalone_on_postgres(tmp.path(), mock.clone()).await else {
        return;
    };
    assert!(
        !eng.instance_id().is_empty(),
        "a standalone engine is this machine"
    );
    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version two, revised upstream"),
        ),
    ]));
    mock.set_branch("main", &c2);
    let result = eng
        .origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(result["errors"].as_array().unwrap().len(), 0, "{result}");
    let hits = eng
        .search_engrams(
            &SearchParams {
                query: Some("revised upstream".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(hits["total"], 1, "the pulled file is indexed: {hits}");
    let other = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
        .await
        .unwrap();
    let did = {
        use crystalline_index::ReadStore;
        let did = other.domain_id("brand").await.unwrap().unwrap();
        assert!(
            other.domain_host(did).await.unwrap().is_none(),
            "the one-shot gave its lock back"
        );
        did
    };

    // This machine's daemon holding the lock live (a command with --config
    // runs beside it): the command walks under it and leaves it held.
    let daemon_token = {
        use crystalline_index::Store;
        let crystalline_index::HostClaim::Acquired { token } = other
            .claim_domain_host(
                did,
                eng.instance_id(),
                eng.instance_id(),
                "2999-01-01T00:00:00+00:00",
                "2000-01-01T00:00:00+00:00",
                false,
            )
            .await
            .unwrap()
        else {
            panic!("this machine's daemon acquires the free lock");
        };
        token
    };
    let c_daemon = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version beside the daemon"),
        ),
    ]));
    mock.set_branch("main", &c_daemon);
    let result = eng
        .origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(result["errors"].as_array().unwrap().len(), 0, "{result}");
    let did = {
        use crystalline_index::ReadStore;
        use crystalline_index::Store;
        assert_eq!(
            other.domain_host(did).await.unwrap().unwrap().instance_id,
            eng.instance_id(),
            "the daemon's lock stays held"
        );
        assert!(
            other
                .release_domain_host(did, eng.instance_id(), &daemon_token)
                .await
                .unwrap(),
            "the daemon's own claim still names the lock"
        );
        // Another live instance hosts the domain now.
        other
            .claim_domain_host(
                did,
                "other-instance",
                "other-instance",
                "2999-01-01T00:00:00+00:00",
                "2000-01-01T00:00:00+00:00",
                false,
            )
            .await
            .unwrap();
        did
    };

    let c3 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version three from upstream"),
        ),
    ]));
    mock.set_branch("main", &c3);
    let result = eng
        .origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        result["errors"].to_string().contains("other-instance"),
        "the reason is in the answer: {result}"
    );
    {
        use crystalline_index::ReadStore;
        assert_eq!(
            other.domain_host(did).await.unwrap().unwrap().instance_id,
            "other-instance",
            "the other instance keeps its lock"
        );
    }
    other.drop_schema().await.unwrap();
}

/// The review pass reads and writes drafts through `Store`, so it is pinned on
/// Postgres too: a colleague's change on another line merges into her draft and
/// she reads both lines, and a change of the line she changed stays her text
/// and refuses her share as a conflict.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_stale_draft_merges_and_refuses_on_postgres() {
    let _scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let plan = |first: &str, last: &str| {
        engram(
            "Plan",
            "plan",
            &format!("{first}\n\nthe middle of the plan\n\n{last}"),
        )
    };
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "plan.md",
            plan("the first line of the plan", "the last line of the plan"),
        ),
        (
            "note.md",
            engram("Note", "note", "the note as the team has it"),
        ),
    ]));
    mock.set_branch("main", &c1);
    let Some((eng, _url, _schema)) = standalone_on_postgres(tmp.path(), mock.clone()).await else {
        return;
    };
    let root = tmp.path().join("team-knowledge");
    eng.origin_add(
        "acme/team",
        Some("team"),
        None,
        None,
        Some(root.to_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    eng.set_review_mode(
        "team",
        Some(crystalline_core::config::ReviewMode::Overlay),
        crystalline_service::ReviewModeConfirm::Confirmed { folds: Vec::new() },
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    let alice = Scope::User {
        account: "alice".to_string(),
        admin: false,
    };
    let edit = |identifier: &str, find: &str, with: &str| crystalline_service::params::EditParams {
        identifier: identifier.to_string(),
        domain: "team".to_string(),
        operation: "find_replace".to_string(),
        content: Some(with.to_string()),
        key: None,
        value: None,
        values: None,
        find_text: Some(find.to_string()),
        expected_replacements: None,
        section: None,
        include_subsections: false,
        expected_checksum: None,
        ack_scope: None,
        share_link: None,
        model: None,
    };
    eng.edit_engram_as(
        &edit("plan", "the last line of the plan", "alice's own last word"),
        None,
        &alice,
    )
    .await
    .unwrap();
    eng.edit_engram_as(
        &edit(
            "note",
            "the note as the team has it",
            "the note as alice has it",
        ),
        None,
        &alice,
    )
    .await
    .unwrap();

    // Bob changes the plan's first line, and the note's only line.
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "plan.md",
            plan("bob's first line", "the last line of the plan"),
        ),
        ("note.md", engram("Note", "note", "the note as bob has it")),
    ]));
    mock.set_branch("main", &c2);
    let result = eng
        .origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(result["errors"].as_array().unwrap().len(), 0, "{result}");

    let read = |identifier: &'static str| {
        let eng = &eng;
        let alice = &alice;
        async move {
            eng.read_engram(
                &ReadParams {
                    identifier: identifier.to_string(),
                    domain: Some("team".to_string()),
                    share_link: None,
                    include_images: None,
                },
                alice,
            )
            .await
            .unwrap()["content"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let plan_read = read("plan").await;
    assert!(
        plan_read.contains("bob's first line") && plan_read.contains("alice's own last word"),
        "a clean merge reads both lines: {plan_read}"
    );
    let note_read = read("note").await;
    assert!(
        note_read.contains("the note as alice has it"),
        "her text stays: {note_read}"
    );

    let answer = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert!(
        answer.to_string().contains("note.md") && !answer.to_string().contains("plan.md"),
        "only the same-line edit is a conflict: {answer}"
    );
}

// --- who may read, chosen at connect ----------------------------------------

/// An engine with a GitHub App pointed at a forge serving `acme/kb` (the App
/// installed on it unless `installed` is false) and the accounts store
/// installed before anything connects, so a private connect has somewhere to
/// write. `extra` is more environment beside the App's.
async fn readers_setup(
    tmp: &Path,
    installed: bool,
    extra: &[(&str, &str)],
) -> (Arc<super::github_app::Forge>, Arc<Engine>, Arc<AuthStore>) {
    use super::github_app::{app_env, engine, repo_with_manifest, spawn_forge};
    let installs: &[(&str, u64)] = if installed { &[("acme/kb", 1)] } else { &[] };
    let (forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        installs,
        &[(1, "ghs_test")],
    )
    .await;
    let mut env = app_env();
    env.extend_from_slice(extra);
    let eng = Arc::new(engine(tmp, &url, &env).await);
    let auth = Arc::new(AuthStore::open(&tmp.join("auth.db")).await.unwrap());
    auth.add_user("ada", "Ada", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth.clone()).with_governor(eng.knowledge_governor()),
    ));
    (forge, eng, auth)
}

async fn connect_kb(
    eng: &Engine,
    root: &Path,
    readers: Option<Readers>,
) -> Result<serde_json::Value, EngineError> {
    eng.origin_add(
        "acme/kb",
        Some("kb"),
        None,
        None,
        Some(root.to_str().unwrap()),
        readers,
    )
    .await
}

#[tokio::test]
async fn connect_private_writes_an_acl_row() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, eng, auth) = readers_setup(tmp.path(), true, &[]).await;
    let report = connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Private))
        .await
        .unwrap();
    assert_eq!(report["readers"], "private", "{report}");
    let acl = auth.domain_visibility("kb").await.unwrap();
    assert!(acl.is_some(), "a private domain has its acl row");
    assert_eq!(
        acl.unwrap().owner,
        "owner",
        "the machine owner chose it, and a governed domain never reads the owner"
    );
}

#[tokio::test]
async fn connect_shared_writes_none() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, eng, auth) = readers_setup(tmp.path(), true, &[]).await;
    let report = connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Shared))
        .await
        .unwrap();
    assert_eq!(report["readers"], "shared", "{report}");
    assert!(auth.domain_visibility("kb").await.unwrap().is_none());

    // A retry of the same connect answers the connected state and names who
    // reads it.
    let again = connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Private))
        .await
        .unwrap();
    assert_eq!(again["already_connected"], true, "{again}");
    assert_eq!(again["readers"], "shared", "{again}");
    assert_eq!(
        again["readers_note"],
        "this domain is already connected and its readers are shared; change who may read on its Members card",
        "{again}"
    );
    assert!(auth.domain_visibility("kb").await.unwrap().is_none());
}

#[tokio::test]
async fn an_already_connected_retry_names_private_readers() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, eng, _auth) = readers_setup(tmp.path(), true, &[]).await;
    connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Private))
        .await
        .unwrap();
    let again = connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Private))
        .await
        .unwrap();
    assert_eq!(again["already_connected"], true, "{again}");
    assert_eq!(again["readers"], "private", "{again}");
    assert!(again.get("readers_note").is_none(), "{again}");
}

#[tokio::test]
async fn connect_shared_on_a_private_domain_refuses_and_keeps_its_members() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = readers_setup(tmp.path(), true, &[]).await;
    auth.add_user("bea", "Bea", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    // A record an earlier registration of the name left behind, with a
    // member: the name is private, whether or not a domain holds it now.
    auth.set_domain_visibility("kb", true, "ada").await.unwrap();
    auth.upsert_domain_member(
        "kb",
        "bea",
        crystalline_service::rest::MemberLevel::Viewer,
        None,
        "ada",
    )
    .await
    .unwrap();

    let err = connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Shared))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "this domain is private; connect it as private, then change who may read on its Members card"
    );
    assert_eq!(
        auth.domain_visibility("kb").await.unwrap().unwrap().owner,
        "ada"
    );
    assert_eq!(auth.domain_members("kb").await.unwrap().len(), 1);
    assert!(
        !forge
            .hits()
            .iter()
            .any(super::github_app::Hit::is_repo_call),
        "nothing was asked of the repository"
    );
    assert!(
        eng.config().domains.get("kb").is_none(),
        "nothing registered"
    );
}

#[tokio::test]
async fn connect_without_readers_on_an_app_instance_refuses_before_anything_is_fetched() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = readers_setup(tmp.path(), true, &[]).await;
    let err = connect_kb(&eng, &tmp.path().join("kb"), None)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err:?}");
    assert!(
        err.to_string()
            .contains(crystalline_service::engine::READERS_REQUIRED),
        "{err}"
    );
    assert!(
        !forge
            .hits()
            .iter()
            .any(super::github_app::Hit::is_repo_call),
        "nothing was asked of the repository"
    );
    assert!(
        eng.config().domains.get("kb").is_none(),
        "nothing registered"
    );
    assert!(auth.domain_visibility("kb").await.unwrap().is_none());
}

#[tokio::test]
async fn a_failed_private_connect_leaves_no_acl_row() {
    let tmp = tempfile::tempdir().unwrap();
    // The App is not installed on the repository: the download is refused
    // after the readers were chosen.
    let (_forge, eng, auth) = readers_setup(tmp.path(), false, &[]).await;
    connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Private))
        .await
        .unwrap_err();
    assert!(eng.config().domains.get("kb").is_none());
    assert!(
        auth.domain_visibility("kb").await.unwrap().is_none(),
        "no acl row for a domain that never registered"
    );
}

#[tokio::test]
async fn a_private_connect_without_an_accounts_store_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    use super::github_app::{app_env, engine, repo_with_manifest, spawn_forge};
    let (forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let eng = engine(tmp.path(), &url, &app_env()).await;
    let err = connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Private))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("accounts"), "{err}");
    assert!(
        !forge
            .hits()
            .iter()
            .any(super::github_app::Hit::is_repo_call),
        "refused before anything was downloaded"
    );
    assert!(eng.config().domains.get("kb").is_none());
}

#[tokio::test]
async fn without_an_app_connect_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let head = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &head);
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        true,
        false,
    )
    .await;
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));

    // No readers: connected as before.
    let report = eng
        .origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(tmp.path().join("kb").to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
    assert!(report.get("readers").is_none(), "{report}");
    assert!(report.get("readers_note").is_none(), "{report}");

    // Readers given: ignored, with a note, and nothing is written.
    let report = eng
        .origin_add(
            "acme/other",
            Some("other"),
            None,
            None,
            Some(tmp.path().join("other").to_str().unwrap()),
            Some(Readers::Private),
        )
        .await
        .unwrap();
    assert!(report.get("readers").is_none(), "{report}");
    assert!(
        report["readers_note"]
            .as_str()
            .is_some_and(|n| n.contains("no GitHub App")),
        "{report}"
    );
    assert!(auth.domain_visibility("other").await.unwrap().is_none());
}

#[tokio::test]
async fn env_origin_without_readers_is_private_and_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kb");
    let root_str = root.to_str().unwrap().to_string();
    let (_forge, eng, auth) = readers_setup(
        tmp.path(),
        true,
        &[
            ("CRYSTALLINE_DOMAIN_KB", &root_str),
            ("CRYSTALLINE_DOMAIN_KB_ORIGIN", "acme/kb"),
        ],
    )
    .await;
    let (logs, _guard) = crate::support::capture_logs();
    eng.apply_env_readers().await.unwrap();
    assert!(
        auth.domain_visibility("kb").await.unwrap().is_some(),
        "absent readers is private"
    );
    let warnings: Vec<String> = logs
        .lines()
        .into_iter()
        .filter(|l| l.starts_with("WARN") && l.contains("CRYSTALLINE_DOMAIN_KB_READERS"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "one warning naming the variable: {warnings:?}"
    );
}

#[tokio::test]
async fn env_origin_with_readers_shared_writes_none_and_says_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kb");
    let root_str = root.to_str().unwrap().to_string();
    let (_forge, eng, auth) = readers_setup(
        tmp.path(),
        true,
        &[
            ("CRYSTALLINE_DOMAIN_KB", &root_str),
            ("CRYSTALLINE_DOMAIN_KB_ORIGIN", "acme/kb"),
            ("CRYSTALLINE_DOMAIN_KB_READERS", "shared"),
        ],
    )
    .await;
    let (logs, _guard) = crate::support::capture_logs();
    eng.apply_env_readers().await.unwrap();
    assert!(auth.domain_visibility("kb").await.unwrap().is_none());
    assert!(!logs.any_contains("CRYSTALLINE_DOMAIN_KB_READERS"));
}

#[tokio::test]
async fn env_readers_shared_never_opens_a_private_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kb");
    let root_str = root.to_str().unwrap().to_string();
    let (_forge, eng, auth) = readers_setup(
        tmp.path(),
        true,
        &[
            ("CRYSTALLINE_DOMAIN_KB", &root_str),
            ("CRYSTALLINE_DOMAIN_KB_ORIGIN", "acme/kb"),
            ("CRYSTALLINE_DOMAIN_KB_READERS", "shared"),
        ],
    )
    .await;
    auth.set_domain_visibility("kb", true, "ada").await.unwrap();
    let (logs, _guard) = crate::support::capture_logs();
    eng.apply_env_readers().await.unwrap();
    assert_eq!(
        auth.domain_visibility("kb").await.unwrap().unwrap().owner,
        "ada",
        "left private, exactly as it stood"
    );
    let warnings: Vec<String> = logs
        .lines()
        .into_iter()
        .filter(|l| {
            l.starts_with("WARN")
                && l.contains(
                    "CRYSTALLINE_DOMAIN_KB_READERS says shared, but domain 'kb' is private; set the variable to private, or remove the private record with: crystalline domain visibility kb default",
                )
        })
        .collect();
    assert_eq!(warnings.len(), 1, "{:?}", logs.lines());
}

#[tokio::test]
async fn env_readers_quietly_close_without_a_word() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kb");
    let root_str = root.to_str().unwrap().to_string();
    let (_forge, eng, auth) = readers_setup(
        tmp.path(),
        true,
        &[
            ("CRYSTALLINE_DOMAIN_KB", &root_str),
            ("CRYSTALLINE_DOMAIN_KB_ORIGIN", "acme/kb"),
        ],
    )
    .await;
    let (logs, _guard) = crate::support::capture_logs();
    eng.apply_env_readers_quietly().await.unwrap();
    assert!(auth.domain_visibility("kb").await.unwrap().is_some());
    assert!(
        !logs.any_contains("CRYSTALLINE_DOMAIN_KB_READERS"),
        "{:?}",
        logs.lines()
    );
}

#[tokio::test]
async fn env_readers_without_an_accounts_store_warn_once() {
    use super::github_app::{app_env, engine, repo_with_manifest, spawn_forge};
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let mut env = app_env();
    let a = tmp.path().join("a").to_str().unwrap().to_string();
    let b = tmp.path().join("b").to_str().unwrap().to_string();
    env.extend_from_slice(&[
        ("CRYSTALLINE_DOMAIN_A", a.as_str()),
        ("CRYSTALLINE_DOMAIN_A_ORIGIN", "acme/a"),
        ("CRYSTALLINE_DOMAIN_A_READERS", "private"),
        ("CRYSTALLINE_DOMAIN_B", b.as_str()),
        ("CRYSTALLINE_DOMAIN_B_ORIGIN", "acme/b"),
        ("CRYSTALLINE_DOMAIN_B_READERS", "private"),
    ]);
    let eng = engine(tmp.path(), &url, &env).await;
    let (logs, _guard) = crate::support::capture_logs();
    eng.apply_env_readers().await.unwrap();
    let warnings: Vec<String> = logs
        .lines()
        .into_iter()
        .filter(|l| l.starts_with("WARN") && l.contains("accounts"))
        .collect();
    assert_eq!(warnings.len(), 1, "{:?}", logs.lines());
}

#[tokio::test]
async fn a_failed_env_private_write_stops_the_start_naming_the_domain() {
    let tmp = tempfile::tempdir().unwrap();
    // The accounts database refuses every private record before the store
    // opens it: its acl table takes no row.
    {
        let path = tmp.path().join("auth.db");
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE domain_acl (domain TEXT PRIMARY KEY NOT NULL, visibility TEXT NOT NULL, owner TEXT NOT NULL, updated_at TEXT NOT NULL, CHECK (owner = 'nobody at all'))",
            (),
        )
        .await
        .unwrap();
    }
    let root = tmp.path().join("kb");
    let root_str = root.to_str().unwrap().to_string();
    let (_forge, eng, _auth) = readers_setup(
        tmp.path(),
        true,
        &[
            ("CRYSTALLINE_DOMAIN_KB", &root_str),
            ("CRYSTALLINE_DOMAIN_KB_ORIGIN", "acme/kb"),
            ("CRYSTALLINE_DOMAIN_KB_READERS", "private"),
        ],
    )
    .await;
    let result = eng.apply_env_readers().await;
    let err = result.unwrap_err();
    assert!(err.to_string().contains("'kb'"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_private_connect_whose_registration_fails_takes_the_row_back() {
    use super::github_app::{app_env, repo_with_manifest, spawn_forge};
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    // The configuration lives in a folder nothing can write into, so the
    // registration fails after the download and after the private record.
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(url),
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let overlay = EnvOverlay::from_vars(
        app_env()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let eng = Engine::new(
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
        cfg,
        None,
        Some(locked.join("config.yaml")),
    )
    .with_origins_dir(tmp.path().join("origins"))
    .with_state_dir(tmp.path().to_path_buf())
    .with_token_store_dir(tmp.path().join("tokens"))
    .with_env_overlay(overlay);
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));

    let err = connect_kb(&eng, &tmp.path().join("kb"), Some(Readers::Private))
        .await
        .unwrap_err();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(err.to_string().contains("config"), "{err}");
    assert!(eng.config().domains.get("kb").is_none());
    assert!(
        auth.domain_visibility("kb").await.unwrap().is_none(),
        "the private record went with the failed registration"
    );
}

/// Calls an MCP tool on a stdio session over `eng`, answering the JSON body
/// or the error text.
async fn mcp_call(
    eng: Arc<Engine>,
    tool: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crystalline_service::mcp::McpServer;
    use rmcp::model::CallToolRequestParams;
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_task =
        tokio::spawn(async move { rmcp::serve_server(McpServer::new(eng), server_io).await });
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let _server = server_task.await.unwrap().unwrap();
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let serde_json::Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    match client.peer().call_tool(params).await {
        Ok(result) => {
            let v = serde_json::to_value(&result).unwrap();
            if v["isError"].as_bool() == Some(true) {
                return Err(v.to_string());
            }
            let text = v
                .pointer("/content/0/text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tokio::test]
async fn mcp_add_domain_with_repo_without_readers_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = readers_setup(tmp.path(), true, &[]).await;
    let folder = tmp.path().join("kb").to_str().unwrap().to_string();
    let err = mcp_call(
        eng.clone(),
        "add_domain",
        serde_json::json!({"repo": "acme/kb", "domain": "kb", "folder": folder}),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains(
            "add_domain needs readers: shared (everyone signed in) or private (only people with access on GitHub)"
        ),
        "{err}"
    );
    assert!(
        !forge
            .hits()
            .iter()
            .any(super::github_app::Hit::is_repo_call)
    );

    let report = mcp_call(
        eng.clone(),
        "add_domain",
        serde_json::json!({"repo": "acme/kb", "domain": "kb", "folder": folder, "readers": "private"}),
    )
    .await
    .unwrap();
    assert_eq!(report["readers"], "private", "{report}");
    assert!(auth.domain_visibility("kb").await.unwrap().is_some());

    // Readers belong to a team domain only.
    let err = mcp_call(
        eng,
        "add_domain",
        serde_json::json!({"virtual": true, "domain": "v", "readers": "shared"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("readers"), "{err}");
}

#[tokio::test]
async fn env_readers_without_an_app_change_nothing_and_say_so() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kb");
    let root_str = root.to_str().unwrap().to_string();
    let eng = engine_with_env(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        Arc::new(MockProvider::new()),
        &[
            ("CRYSTALLINE_DOMAIN_KB", &root_str),
            ("CRYSTALLINE_DOMAIN_KB_ORIGIN", "acme/kb"),
            ("CRYSTALLINE_DOMAIN_KB_READERS", "private"),
        ],
    )
    .await;
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    let (logs, _guard) = crate::support::capture_logs();
    eng.apply_env_readers().await.unwrap();
    assert!(auth.domain_visibility("kb").await.unwrap().is_none());
    assert!(
        logs.lines()
            .iter()
            .any(|l| l.starts_with("INFO") && l.contains("CRYSTALLINE_DOMAIN_KB_READERS")),
        "{:?}",
        logs.lines()
    );
}

#[tokio::test]
async fn env_readers_quietly_take_no_write_on_a_domain_already_private() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kb");
    let root_str = root.to_str().unwrap().to_string();
    let (_forge, eng, auth) = readers_setup(
        tmp.path(),
        true,
        &[
            ("CRYSTALLINE_DOMAIN_KB", &root_str),
            ("CRYSTALLINE_DOMAIN_KB_ORIGIN", "acme/kb"),
        ],
    )
    .await;
    eng.apply_env_readers_quietly().await.unwrap();
    assert!(auth.domain_visibility("kb").await.unwrap().is_some());

    // Another connection holds the accounts database's write lock. A second
    // run that only reads finishes at once; one that took a write lock would
    // wait out the busy timeout and fail. The second connection needs
    // turso's multiprocess WAL, which its Windows IO backend does not offer;
    // the read-before-write path it pins is the same on every OS.
    if cfg!(windows) {
        return;
    }
    let db = turso::Builder::new_local(tmp.path().join("auth.db").to_str().unwrap())
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let holder = db.connect().unwrap();
    holder.execute("BEGIN IMMEDIATE", ()).await.unwrap();
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        eng.apply_env_readers_quietly(),
    )
    .await;
    holder.execute("ROLLBACK", ()).await.unwrap();
    second
        .expect("the second run waited for the write lock")
        .unwrap();
}
