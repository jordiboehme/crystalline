//! A domain whose MANIFEST says `read_only: on` refuses every write made
//! through the engine, whoever asks, with one sentence; its owner switching
//! the key back off is the one change it takes.

use std::sync::Arc;

use crystalline_core::config::ReviewMode;
use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::ReviewModeConfirm;
use crystalline_service::engine::PolicyEdit;
use crystalline_service::engine::read_only_sentence;
use crystalline_service::params::{
    DeleteParams, EditParams, MoveParams, ReadParams, RetireParams, SaveParams, SplitParams,
    WriteParams,
};
use crystalline_service::{Engine, EngineError, Scope};
use tokio::sync::Mutex;

pub(crate) const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\n## Part\n\n- [fact] a fact about alpha #eng\n- [fact] a second fact about alpha #eng\n- [fact] a third fact about alpha #eng\n";

pub(crate) fn manifest(name: &str, read_only: Option<&str>) -> String {
    let line = read_only
        .map(|v| format!("read_only: {v}\n"))
        .unwrap_or_default();
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n{line}---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
    )
}

/// Two file domains: `archive` (read-only, holding alpha) and `eng`
/// (writable, holding alpha too), synced into an in-memory store.
pub(crate) async fn fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    for (name, flag) in [("archive", Some("on")), ("eng", None)] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MANIFEST.md"), manifest(name, flag)).unwrap();
        std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
        cfg.domains.insert(name.to_string(), DomainEntry::file(dir));
    }
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

pub(crate) fn assert_refused(err: EngineError, domain: &str) {
    assert!(matches!(err, EngineError::DomainReadOnly(_)), "{err:?}");
    assert_eq!(err.to_string(), read_only_sentence(domain));
}

/// The folder as bytes, so "nothing changed" is a byte-for-byte claim.
pub(crate) fn snapshot(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path().strip_prefix(dir).unwrap().display().to_string(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

pub(crate) fn sha256(text: &str) -> String {
    crate::support::sha256_hex(text.as_bytes())
}

pub(crate) fn write_params(domain: &str, title: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: domain.into(),
        title: title.into(),
        content: Some("- [fact] new #eng".into()),
        folder: None,
        engram_type: None,
        tags: vec!["eng".into()],
        status: None,
        metadata: None,
        overwrite: false,
        model: None,
        share_link: None,
        expected_checksum: None,
    }
}

async fn checksum(engine: &Engine, domain: &str, id: &str) -> String {
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: id.into(),
                domain: Some(domain.into()),
                share_link: None,
                include_images: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    read["checksum"].as_str().unwrap().to_string()
}

#[test]
fn the_sentence_is_the_one_the_spec_fixes() {
    assert_eq!(
        read_only_sentence("archive"),
        "'archive' is read-only; its owner switches read_only off in the domain policies"
    );
}

#[tokio::test]
async fn every_write_verb_refuses_a_read_only_domain_with_one_sentence() {
    let (tmp, engine) = fixture().await;
    let before = snapshot(&tmp.path().join("archive"));
    let scope = Scope::Unrestricted;
    let sum = checksum(&engine, "archive", "alpha").await;

    assert_refused(
        engine
            .write_engram_as(&write_params("archive", "Beta"), None, &scope)
            .await
            .unwrap_err(),
        "archive",
    );

    let edit = EditParams {
        identifier: "alpha".into(),
        domain: "archive".into(),
        operation: "append".into(),
        content: Some("- [fact] more #eng".into()),
        ..EditParams::default()
    };
    assert_refused(
        engine
            .edit_engram_as(&edit, None, &scope)
            .await
            .unwrap_err(),
        "archive",
    );

    let set = EditParams {
        identifier: "alpha".into(),
        domain: "archive".into(),
        operation: "set_frontmatter".into(),
        key: Some("status".into()),
        value: Some("draft".into()),
        ..EditParams::default()
    };
    assert_refused(
        engine.edit_engram_as(&set, None, &scope).await.unwrap_err(),
        "archive",
    );

    assert_refused(
        engine
            .acknowledge_finding_as("archive", "alpha", "V104", None, None, None, &scope)
            .await
            .unwrap_err(),
        "archive",
    );

    let save = SaveParams {
        domain: "archive".into(),
        identifier: "alpha".into(),
        content: ALPHA.replace("a fact", "another fact"),
        expected_checksum: sum.clone(),
    };
    assert_refused(
        engine.save_engram(&save, &scope).await.unwrap_err(),
        "archive",
    );

    let split = SplitParams {
        domain: "archive".into(),
        identifier: "alpha".into(),
        title: "Part".into(),
        sections: vec!["## Part".into()],
        ..SplitParams::default()
    };
    assert_refused(
        engine
            .split_engram_as(&split, None, &scope)
            .await
            .unwrap_err(),
        "archive",
    );

    let retire = RetireParams {
        domain: "archive".into(),
        identifier: "alpha".into(),
        status: "archived".into(),
        successor: None,
        valid_to: None,
    };
    assert_refused(
        engine
            .retire_engram_as(&retire, None, &scope)
            .await
            .unwrap_err(),
        "archive",
    );

    let delete = DeleteParams {
        identifier: "alpha".into(),
        domain: "archive".into(),
        expected_checksum: None,
    };
    assert_refused(
        engine
            .delete_engram_as(&delete, None, &scope)
            .await
            .unwrap_err(),
        "archive",
    );

    let out = MoveParams {
        identifier: "alpha".into(),
        domain: "archive".into(),
        destination: "moved".into(),
        destination_domain: Some("eng".into()),
        permalink: None,
        update_links: None,
    };
    assert_refused(
        engine.move_engram(&out, &scope).await.unwrap_err(),
        "archive",
    );

    assert_refused(
        engine
            .restore_engram("archive", "alpha.md", ALPHA, &scope)
            .await
            .unwrap_err(),
        "archive",
    );

    assert_eq!(
        snapshot(&tmp.path().join("archive")),
        before,
        "nothing in the folder moved"
    );
}

#[tokio::test]
async fn a_move_into_a_read_only_domain_is_refused_and_nothing_moves() {
    let (tmp, engine) = fixture().await;
    let archive = snapshot(&tmp.path().join("archive"));
    let eng = snapshot(&tmp.path().join("eng"));
    let into = MoveParams {
        identifier: "alpha".into(),
        domain: "eng".into(),
        destination: "from-eng".into(),
        destination_domain: Some("archive".into()),
        permalink: None,
        update_links: None,
    };
    assert_refused(
        engine
            .move_engram(&into, &Scope::Unrestricted)
            .await
            .unwrap_err(),
        "archive",
    );
    assert_eq!(snapshot(&tmp.path().join("archive")), archive);
    assert_eq!(
        snapshot(&tmp.path().join("eng")),
        eng,
        "the source stays where it was"
    );
}

/// The flag is read off the folder at each write: a person who edits
/// MANIFEST.md in a plain editor while the engine runs locks the domain for
/// the very next write, with no restart and no sync in between.
#[tokio::test]
async fn a_plain_edit_locks_the_next_write() {
    let (tmp, engine) = fixture().await;
    engine
        .write_engram_as(&write_params("eng", "Before"), None, &Scope::Unrestricted)
        .await
        .expect("eng takes changes");
    std::fs::write(
        tmp.path().join("eng/MANIFEST.md"),
        manifest("eng", Some("on")),
    )
    .unwrap();
    assert!(engine.domain_is_read_only("eng").await);
    assert_refused(
        engine
            .write_engram_as(&write_params("eng", "After"), None, &Scope::Unrestricted)
            .await
            .unwrap_err(),
        "eng",
    );
    assert!(!tmp.path().join("eng/after.md").exists());
}

#[tokio::test]
async fn the_owner_switches_read_only_off_and_writes_land_again() {
    let (tmp, engine) = fixture().await;
    let scope = Scope::Unrestricted;
    // Any other policy change, alone or beside the switch-off, is refused.
    for changes in [
        vec![("sharing".to_string(), "direct".to_string())],
        vec![
            ("read_only".to_string(), "off".to_string()),
            ("sharing".to_string(), "direct".to_string()),
        ],
    ] {
        assert_refused(
            engine
                .set_manifest_policies("archive", &changes, &scope)
                .await
                .unwrap_err(),
            "archive",
        );
    }
    // The MANIFEST editor is refused too: the policies card is the way back.
    let manifest_now = std::fs::read_to_string(tmp.path().join("archive/MANIFEST.md")).unwrap();
    assert_refused(
        engine
            .save_manifest(
                "archive",
                &manifest_now.replace("read_only: on", "read_only: off"),
                &sha256(&manifest_now),
                &scope,
                false,
            )
            .await
            .unwrap_err(),
        "archive",
    );

    let answer = engine
        .set_manifest_policies("archive", &[("read_only".into(), "off".into())], &scope)
        .await
        .unwrap();
    assert_eq!(answer["draft"], false);
    let manifest_after = std::fs::read_to_string(tmp.path().join("archive/MANIFEST.md")).unwrap();
    assert!(
        manifest_after.contains("read_only: off"),
        "{manifest_after}"
    );
    assert!(!engine.domain_is_read_only("archive").await);

    let edit = EditParams {
        identifier: "alpha".into(),
        domain: "archive".into(),
        operation: "append".into(),
        content: Some("- [fact] written after the switch #eng".into()),
        ..EditParams::default()
    };
    engine
        .edit_engram_as(&edit, None, &scope)
        .await
        .expect("the domain takes changes again");

    // The owner switches it on again: `Unrestricted` is the machine owner,
    // who holds every right (decision D10).
    engine
        .set_manifest_policies("archive", &[("read_only".into(), "on".into())], &scope)
        .await
        .unwrap();
    assert!(engine.domain_is_read_only("archive").await);

    // Removing the key is a switch-off too: its default is off (spec A-1).
    let unset = [PolicyEdit::Unset {
        key: "read_only".into(),
    }];
    engine
        .edit_manifest_policies("archive", &unset, &scope)
        .await
        .unwrap();
    assert!(!engine.domain_is_read_only("archive").await);
    let manifest_unset = std::fs::read_to_string(tmp.path().join("archive/MANIFEST.md")).unwrap();
    assert!(!manifest_unset.contains("read_only"), "{manifest_unset}");
}

#[tokio::test]
async fn a_virtual_domain_is_locked_and_unlocked_through_the_same_switch() {
    let (_tmp, engine) = fixture().await;
    engine.domain_add_virtual("scratch").await.unwrap();
    let scope = Scope::Unrestricted;
    engine
        .set_manifest_policies("scratch", &[("read_only".into(), "on".into())], &scope)
        .await
        .unwrap();
    assert!(engine.domain_is_read_only("scratch").await);
    let write = write_params("scratch", "Note");
    assert_refused(
        engine
            .write_engram_as(&write, None, &scope)
            .await
            .unwrap_err(),
        "scratch",
    );
    engine
        .set_manifest_policies("scratch", &[("read_only".into(), "off".into())], &scope)
        .await
        .unwrap();
    engine
        .write_engram_as(&write, None, &scope)
        .await
        .expect("open again");
}

#[tokio::test]
async fn the_paths_without_a_write_view_refuse_too() {
    let (tmp, engine) = fixture().await;
    let before = snapshot(&tmp.path().join("archive"));
    let scope = Scope::Unrestricted;
    assert_refused(
        engine
            .attachment_write("archive", "assets/a.txt", b"x".to_vec())
            .await
            .unwrap_err(),
        "archive",
    );
    assert_refused(
        engine
            .attachment_write_as("archive", "assets/a.txt", b"x".to_vec(), &scope)
            .await
            .unwrap_err(),
        "archive",
    );
    assert_refused(
        engine
            .attachment_delete("archive", "assets/a.txt")
            .await
            .unwrap_err(),
        "archive",
    );
    assert_refused(
        engine
            .attachment_delete_as("archive", "assets/a.txt", &scope)
            .await
            .unwrap_err(),
        "archive",
    );
    assert_refused(
        engine
            .import_domain_files(
                "archive",
                &[("beta.md".into(), ALPHA.replace("alpha", "beta"))],
                false,
                false,
            )
            .await
            .unwrap_err(),
        "archive",
    );
    assert_refused(
        engine
            .retag("eng", "kept", Some("archive"), false, false, false)
            .await
            .unwrap_err(),
        "archive",
    );
    assert_refused(
        engine
            .rename_domain("archive", "attic", false, &scope)
            .await
            .unwrap_err(),
        "archive",
    );
    assert_refused(
        engine
            .set_review_mode(
                "archive",
                Some(ReviewMode::Overlay),
                ReviewModeConfirm::Preview,
                &scope,
            )
            .await
            .unwrap_err(),
        "archive",
    );
    assert_eq!(snapshot(&tmp.path().join("archive")), before);
}

/// `domain import` loads a folder into a virtual domain: refused like any
/// write once the domain is read-only (spec A-3).
#[tokio::test]
async fn an_import_into_a_read_only_virtual_domain_is_refused() {
    let (tmp, engine) = fixture().await;
    engine.domain_add_virtual("scratch").await.unwrap();
    engine
        .set_manifest_policies(
            "scratch",
            &[("read_only".into(), "on".into())],
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let src = tmp.path().join("import-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("beta.md"), ALPHA.replace("alpha", "beta")).unwrap();
    assert_refused(
        engine
            .import_domain("scratch", &src, false, false)
            .await
            .unwrap_err(),
        "scratch",
    );
    let listed = engine
        .browse_domain(
            &crystalline_service::params::BrowseParams {
                domain: "scratch".into(),
                path: None,
                depth: None,
                glob: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(!listed.to_string().contains("beta"), "{listed}");
}

#[tokio::test]
async fn an_unscoped_retag_skips_a_read_only_domain_and_names_it() {
    let (tmp, engine) = fixture().await;
    let before = snapshot(&tmp.path().join("archive"));
    let dry = engine
        .retag("eng", "engineering", None, false, true, false)
        .await
        .unwrap();
    assert_eq!(
        dry["read_only_skipped"],
        serde_json::json!(["archive"]),
        "{dry}"
    );
    let answer = engine
        .retag("eng", "engineering", None, false, false, false)
        .await
        .unwrap();
    assert_eq!(
        answer["read_only_skipped"],
        serde_json::json!(["archive"]),
        "{answer}"
    );
    assert_eq!(snapshot(&tmp.path().join("archive")), before);
    let eng = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(eng.contains("engineering"), "{eng}");
}

/// The generated listings are written into a writable folder and never into
/// a read-only one (a regenerated listing after a pull would be an unshared
/// change that can never be shared). `index.files` is on by default.
#[tokio::test]
async fn generated_index_files_are_not_written_into_a_read_only_domain() {
    let (tmp, engine) = fixture().await;
    engine.sync(None).await.unwrap();
    assert!(
        tmp.path().join("eng/index.md").exists(),
        "a writable folder still gets its listing"
    );
    assert!(
        !tmp.path().join("archive/index.md").exists(),
        "the listing refresh skips a read-only folder"
    );
}

const SHOT: &[u8] = b"\x89PNG\r\n\x1a\n\x00shot";

const WITH_SHOT: &str = "---\ntype: engram\ntitle: Shot\npermalink: shot\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Shot\n\n- [fact] the dashboard looked like this #eng\n- [fact] a second fact about the dashboard #eng\n- [fact] a third fact about the dashboard #eng\n\n![The dashboard](assets/shot.png)\n";

/// A cross-domain move into a blocked domain and out of one is refused before
/// anything moves: no engram, no carried attachment, no link rewrite, at
/// either end. The carry writes with the raw attachment writer, so this pins
/// that the refusal comes before it.
#[tokio::test]
async fn a_move_into_or_out_of_a_blocked_domain_moves_nothing() {
    let (tmp, engine) = fixture().await;
    for dir in ["eng", "archive"] {
        let root = tmp.path().join(dir);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/shot.png"), SHOT).unwrap();
        std::fs::write(root.join("shot.md"), WITH_SHOT).unwrap();
    }
    engine.sync(None).await.unwrap();
    let archive = snapshot(&tmp.path().join("archive"));
    let eng = snapshot(&tmp.path().join("eng"));
    for (from, to) in [("eng", "archive"), ("archive", "eng")] {
        let p = MoveParams {
            identifier: "shot".into(),
            domain: from.into(),
            destination: "moved/shot".into(),
            destination_domain: Some(to.into()),
            permalink: None,
            update_links: None,
        };
        assert_refused(
            engine
                .move_engram(&p, &Scope::Unrestricted)
                .await
                .unwrap_err(),
            "archive",
        );
        assert_eq!(
            snapshot(&tmp.path().join("archive")),
            archive,
            "{from} -> {to}"
        );
        assert_eq!(snapshot(&tmp.path().join("eng")), eng, "{from} -> {to}");
    }
}

#[tokio::test]
async fn the_listing_browse_read_and_routing_say_read_only() {
    let (tmp, engine) = fixture().await;
    let scope = Scope::Unrestricted;
    engine.domain_add_virtual("scratch").await.unwrap();
    engine
        .set_manifest_policies("scratch", &[("read_only".into(), "on".into())], &scope)
        .await
        .unwrap();
    let listed = engine
        .list_domains(
            &crystalline_service::params::ListDomainsParams::default(),
            &scope,
        )
        .await
        .unwrap();
    let row = |name: &str| {
        listed["domains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(row("archive")["read_only"], true);
    assert_eq!(row("scratch")["read_only"], true);
    assert!(
        row("eng").get("read_only").is_none(),
        "a writable row leaves the key out, like an absent mark"
    );

    let browse = |domain: &str| crystalline_service::params::BrowseParams {
        domain: domain.into(),
        path: None,
        depth: None,
        glob: None,
    };
    let locked = engine
        .browse_domain(&browse("archive"), &scope)
        .await
        .unwrap();
    assert_eq!(locked["read_only"], true);
    let open = engine.browse_domain(&browse("eng"), &scope).await.unwrap();
    assert!(
        open.get("read_only").is_none(),
        "a writable domain's answer is unchanged"
    );

    let read = |domain: &str| ReadParams {
        identifier: "alpha".into(),
        domain: Some(domain.into()),
        share_link: None,
        include_images: None,
    };
    let locked = engine.read_engram(&read("archive"), &scope).await.unwrap();
    assert_eq!(locked["read_only"], true);
    let open = engine.read_engram(&read("eng"), &scope).await.unwrap();
    assert!(open.get("read_only").is_none());

    for routing in [
        engine.routing_text(),
        engine
            .routing_text_scoped_with_mounts(&scope)
            .await
            .unwrap(),
    ] {
        assert!(routing.contains("- archive (read-only): "), "{routing}");
        assert!(routing.contains("- scratch (read-only): "), "{routing}");
        assert!(routing.contains("- eng: "), "{routing}");
    }

    // A plain edit of MANIFEST.md on disk, with no sync in between (the
    // case of Task 10's `a_plain_edit_locks_the_next_write`): the very next
    // listing and routing block say so.
    std::fs::write(
        tmp.path().join("eng/MANIFEST.md"),
        manifest("eng", Some("on")),
    )
    .unwrap();
    let listed = engine
        .list_domains(
            &crystalline_service::params::ListDomainsParams::default(),
            &scope,
        )
        .await
        .unwrap();
    let eng = listed["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "eng")
        .unwrap();
    assert_eq!(
        eng["read_only"], true,
        "a plain edit marks the next listing: {eng}"
    );
    let routing = engine
        .routing_text_scoped_with_mounts(&scope)
        .await
        .unwrap();
    assert!(routing.contains("- eng (read-only): "), "{routing}");
}
