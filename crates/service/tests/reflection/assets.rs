//! A Reflection's files through the engine: rights, refusals, moves, the
//! findings, and the engram side never seeing them.

use std::collections::BTreeMap;
use std::sync::Arc;

use crystalline_service::params::{
    DiscussParams, EvolveParams, ReflectParams, WriteReflectionParams,
};
use crystalline_service::rest::{AuthStore, ReflectionLevel, Role};
use crystalline_service::{DomainAccess, ReflectionCaller, Scope};

use crate::docs::{agent, person, reflecting, write};
use crate::fixture::{Fx, engram, fixture, manifest};

pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00flow\xff\xfe\x00";

/// `eng` shared, Reflection on: vera an instance viewer (Read), nora at
/// knowledge `none` with Reflection `contributor` (Edit), ed an editor
/// (Curate).
async fn three_people() -> (Fx, ReflectionCaller, ReflectionCaller, ReflectionCaller) {
    let fx = reflecting().await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("vera", "vera", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    auth.add_user("nora", "nora", None, Role::None, "pw12345678")
        .await
        .unwrap();
    auth.set_reflection_level("nora", ReflectionLevel::Contributor)
        .await
        .unwrap();
    auth.add_user("ed", "ed", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let who = |name: &str| {
        ReflectionCaller::person(
            Scope::User {
                account: name.into(),
                admin: false,
            },
            name,
        )
    };
    (fx, who("vera"), who("nora"), who("ed"))
}

fn curated(path: &str, markdown: &str) -> WriteReflectionParams {
    let mut p = write(path, markdown);
    p.sources = BTreeMap::from([("_top".to_string(), vec!["runbooks/deploy".to_string()])]);
    p
}

async fn feedback_total(fx: &Fx) -> u64 {
    let p = ReflectParams {
        kinds: vec!["feedback".into()],
        ..Default::default()
    };
    fx.engine.reflect(&p, &agent()).await.unwrap()["total"]
        .as_u64()
        .unwrap()
}

#[tokio::test]
async fn a_reader_reads_a_contributor_adds_and_a_curator_replaces_and_removes() {
    let (fx, vera, nora, ed) = three_people().await;
    let err = fx
        .engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &vera)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("read") && err.contains("edit"), "{err}");
    let written = fx
        .engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &nora)
        .await
        .unwrap();
    assert!(written.created);
    assert_eq!(
        (written.row.path.as_str(), written.row.mime.as_str()),
        ("assets/flow.png", "image/png")
    );
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/flow.png")).unwrap(),
        PNG
    );
    // Knowledge none: no engram right at all, every file of the Reflection.
    assert!(fx.engine.require_domain("eng", &nora.scope).await.is_err());
    let (bytes, row) = fx
        .engine
        .reflection_asset_read("eng", "assets/flow.png", &nora)
        .await
        .unwrap();
    assert_eq!((bytes.as_slice(), row.size), (PNG, PNG.len() as u64));
    assert_eq!(
        fx.engine
            .reflection_asset_list("eng", &vera)
            .await
            .unwrap()
            .len(),
        1
    );
    let err = fx
        .engine
        .reflection_asset_write("eng", "assets/flow.png", b"other".to_vec(), &nora)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("curate") && err.contains("new name"),
        "replacing removes the old bytes: {err}"
    );
    let err = fx
        .engine
        .reflection_asset_delete("eng", "assets/flow.png", &nora)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("curate"), "{err}");
    let replaced = fx
        .engine
        .reflection_asset_write("eng", "assets/flow.png", b"other".to_vec(), &ed)
        .await
        .unwrap();
    assert!(!replaced.created);
    fx.engine
        .reflection_asset_delete("eng", "assets/flow.png", &ed)
        .await
        .unwrap();
    assert!(!fx.root.join("reflections/assets/flow.png").exists());
    assert!(
        fx.engine
            .reflection_asset_list("eng", &ed)
            .await
            .unwrap()
            .is_empty()
    );
    let err = fx
        .engine
        .reflection_asset_delete("eng", "assets/flow.png", &ed)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no file 'assets/flow.png'"), "{err}");
    assert!(
        fx.engine.attachment_list("eng").await.unwrap().is_empty(),
        "never an engram attachment"
    );
}

#[tokio::test]
async fn paths_follow_the_attachment_rules_and_stay_in_the_domain() {
    let fx = reflecting().await;
    for bad in [
        "assets/tool.exe",
        "flow.png",
        "assets/../x.png",
        "assets/a b.png",
        "assets/.hidden.png",
        "reflections/assets/x.png",
    ] {
        let err = fx
            .engine
            .reflection_asset_write("eng", bad, PNG.to_vec(), &person())
            .await;
        assert!(err.is_err(), "{bad} was accepted");
    }
    let big = vec![0u8; (crystalline_core::MAX_ATTACHMENT_BYTES + 1) as usize];
    let err = fx
        .engine
        .reflection_asset_write("eng", "assets/big.png", big, &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("ceiling"), "{err}");
    let off = fixture(&[]).await;
    let err = off
        .engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("Reflection is off"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_reflections_folder_that_leaves_the_domain_is_refused() {
    let fx = reflecting().await;
    let outside = fx.tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, fx.root.join("reflections")).unwrap();
    let err = fx
        .engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("outside the domain root"), "{err}");
    assert!(!outside.join("assets/flow.png").exists());
}

#[tokio::test]
async fn a_write_that_embeds_an_engram_attachment_is_refused() {
    let fx = reflecting().await;
    for (path, page) in [
        ("a.md", "# A\n\n![shot](../assets/shot.png)\n"),
        ("guides/b.md", "# B\n\n![shot](../../assets/shot.png)\n"),
        ("c.md", "# C\n\n[deck](crystalline://eng/assets/deck.pdf)\n"),
    ] {
        let err = fx
            .engine
            .write_reflection(&write(path, page), &person())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("engram attachment") && err.contains("reflections/assets/"),
            "{path}: {err}"
        );
        assert!(
            !fx.root.join("reflections").join(path).exists(),
            "{path} was written"
        );
    }
    fx.engine
        .write_reflection(
            &write(
                "guides/d.md",
                "# D\n\n![flow](../assets/flow.png#right,w=50%)\n",
            ),
            &person(),
        )
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &write("e.md", "# E\n\n![flow](assets/flow.png)\n"),
            &person(),
        )
        .await
        .unwrap();
    let err = fx
        .engine
        .write_reflection(&write("assets/notes.md", "# Notes\n"), &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("reflections/assets/"),
        "no reflection lives among the files: {err}"
    );
}

#[tokio::test]
async fn moving_a_reflection_keeps_its_file_links_and_owes_no_feedback() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &curated(
                "guides/deploying.md",
                "# Deploying\n\n![flow](../assets/flow.png#right)\n",
            ),
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(feedback_total(&fx).await, 0);
    fx.engine
        .move_reflection("eng", "guides/deploying.md", "deploying.md", &agent())
        .await
        .unwrap();
    let text = std::fs::read_to_string(fx.root.join("reflections/deploying.md")).unwrap();
    assert!(text.contains("![flow](assets/flow.png#right)"), "{text}");
    assert_eq!(
        feedback_total(&fx).await,
        0,
        "a link that follows a move is no person's change"
    );
}

#[tokio::test]
async fn moving_a_file_rewrites_every_reflection_that_references_it_and_owes_no_feedback() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/größe.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &curated(
                "guides/a.md",
                "# A\n\n![s](../assets/gr%C3%B6%C3%9Fe.png \"Size\")\n",
            ),
            &agent(),
        )
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &curated("b.md", "# B\n\n![s](assets/größe.png#left)\n"),
            &agent(),
        )
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &curated("PACT.md", "# Pact\n\n![s](assets/größe.png)\n"),
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(feedback_total(&fx).await, 0);
    let moved = fx
        .engine
        .move_reflection_asset(
            "eng",
            "assets/größe.png",
            "assets/diagrams/size.png",
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(moved["links_rewritten"], 3, "{moved}");
    let read = |rel: &str| std::fs::read_to_string(fx.root.join("reflections").join(rel)).unwrap();
    assert!(read("guides/a.md").contains("![s](../assets/diagrams/size.png \"Size\")"));
    assert!(read("b.md").contains("![s](assets/diagrams/size.png#left)"));
    assert!(
        read("PACT.md").contains("![s](assets/diagrams/size.png)"),
        "PACT.md references count"
    );
    assert!(
        fx.root
            .join("reflections/assets/diagrams/size.png")
            .is_file()
    );
    assert!(!fx.root.join("reflections/assets/größe.png").exists());
    let paths: Vec<String> = fx
        .engine
        .reflection_asset_list("eng", &agent())
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(paths, ["assets/diagrams/size.png"]);
    assert_eq!(
        feedback_total(&fx).await,
        0,
        "a link that follows a file is no person's change"
    );
    let err = fx
        .engine
        .move_reflection_asset(
            "eng",
            "assets/diagrams/size.png",
            "assets/diagrams/size.png",
            &agent(),
        )
        .await;
    assert!(err.is_err());
}

/// The file verbs announce `files` events, and a move announces each page
/// whose link followed the file.
#[tokio::test]
async fn the_file_verbs_announce_files_and_the_pages_a_move_relinked() {
    use crystalline_service::changes::{Change, ReflectionChangeKind as K};
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &write("guides/a.md", "# A\n\n![flow](../assets/flow.png)\n"),
            &person(),
        )
        .await
        .unwrap();
    let mut rx = fx.engine.changes().subscribe();
    fx.engine
        .reflection_asset_write("eng", "assets/flow.png", b"other".to_vec(), &person())
        .await
        .unwrap();
    fx.engine
        .move_reflection_asset("eng", "assets/flow.png", "assets/x/flow.png", &person())
        .await
        .unwrap();
    fx.engine
        .reflection_asset_delete("eng", "assets/x/flow.png", &person())
        .await
        .unwrap();
    let mut heard = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let Change::Reflection(change) = envelope.change {
            heard.push((change.kind, change.path, change.from));
        }
    }
    let s = |text: &str| text.to_string();
    assert_eq!(
        heard,
        vec![
            (K::Files, s("assets/flow.png"), None),
            (K::Files, s("assets/x/flow.png"), Some(s("assets/flow.png"))),
            (K::Changed, s("guides/a.md"), None),
            (K::Files, s("assets/x/flow.png"), None),
        ]
    );
}

#[tokio::test]
async fn a_curators_agent_moves_and_removes_files_through_write_reflection() {
    let (fx, _, nora, ed) = three_people().await;
    fx.engine
        .reflection_asset_write("eng", "assets/a.png", PNG.to_vec(), &ed)
        .await
        .unwrap();
    let mv = WriteReflectionParams {
        domain: "eng".into(),
        action: Some("move_asset".into()),
        path: "assets/a.png".into(),
        to: Some("assets/b.png".into()),
        ..Default::default()
    };
    let err = fx
        .engine
        .write_reflection(&mv, &nora)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("curate"), "{err}");
    fx.engine.write_reflection(&mv, &ed).await.unwrap();
    let rm = WriteReflectionParams {
        domain: "eng".into(),
        action: Some("delete_asset".into()),
        path: "assets/b.png".into(),
        ..Default::default()
    };
    fx.engine.write_reflection(&rm, &ed).await.unwrap();
    assert!(
        fx.engine
            .reflection_asset_list("eng", &ed)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Files and pages written outside Crystalline, then synced. Each page
/// carries a section id line, the page marker that keeps a hand-written page
/// from reading as an engram under `reflections/` (which would hold the
/// switch pending).
async fn with_findings() -> Fx {
    fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        ("reflections/assets/orphan.png", "nobody links me"),
        ("reflections/assets/used.png", "linked"),
        ("reflections/guides/a.md", "# A\n\n## Pics\n<!-- r:aaaa0001 -->\n\n![u](../assets/used.png) ![m](../assets/missing.png)\n"),
        ("reflections/PACT.md", "# Pact\n\n## Pictures\n<!-- r:aaaa0002 -->\n\n![p](assets/pact.png)\n"),
        ("reflections/b.md", "# B\n\n## Shot\n<!-- r:aaaa0003 -->\n\n![e](../assets/shot.png)\n"),
        ("assets/shot.png", "an engram attachment"),
    ])
    .await
}

#[tokio::test]
async fn reflect_names_unreferenced_and_missing_files_and_engram_attachments() {
    let fx = with_findings().await;
    let p = ReflectParams {
        kinds: vec!["update".into()],
        limit: Some(100),
        ..Default::default()
    };
    let queue = fx.engine.reflect(&p, &agent()).await.unwrap();
    let rows: Vec<&serde_json::Value> = queue["queue"].as_array().unwrap().iter().collect();
    let find = |what: &str, path: &str| {
        rows.iter()
            .find(|r| r["what"] == what && r["path"] == path)
            .copied()
    };
    let missing = find("asset_missing", "guides/a.md").expect("the missing file of a section");
    assert_eq!(missing["assets"], serde_json::json!(["assets/missing.png"]));
    assert_eq!(missing["heading"], "Pics", "grouped with its section");
    assert_eq!(
        find("asset_missing", "PACT.md").expect("PACT.md references count")["assets"],
        serde_json::json!(["assets/pact.png"])
    );
    let orphan = find("asset_unreferenced", "").expect("the file nobody links");
    assert_eq!(orphan["asset"], "assets/orphan.png");
    assert_eq!(
        find("engram_attachment", "b.md").expect("an embed written outside Crystalline")["targets"],
        serde_json::json!(["../assets/shot.png"])
    );
    assert!(
        !queue.to_string().contains("used.png"),
        "a linked file that is there owes nothing: {queue}"
    );
    assert!(
        queue["guidance"]
            .as_str()
            .unwrap()
            .contains("reflections/assets/")
    );
}

/// A file row whose bytes are not on this disk (a row a new host kept from
/// the previous host's disk, or a file deleted while nothing watched) is a
/// broken image here: the queue asks for it as missing, because the disk
/// is checked, not only the row (R26).
#[tokio::test]
async fn a_kept_asset_row_without_bytes_is_reported_missing() {
    let fx = with_findings().await;
    let used = fx.root.join("reflections/assets/used.png");
    std::fs::remove_file(&used).unwrap();
    let store = fx.engine.store();
    {
        let store = store.lock().await;
        let did = store.domain_id("eng").await.unwrap().unwrap();
        assert!(
            store
                .get_asset(
                    crystalline_index::AssetTable::Reflection,
                    did,
                    "assets/used.png"
                )
                .await
                .unwrap()
                .is_some(),
            "the row is still there"
        );
    }
    let p = ReflectParams {
        kinds: vec!["update".into()],
        limit: Some(100),
        ..Default::default()
    };
    let queue = fx.engine.reflect(&p, &agent()).await.unwrap();
    let missing = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["what"] == "asset_missing" && r["path"] == "guides/a.md")
        .cloned()
        .expect("the section's missing files");
    assert_eq!(
        missing["assets"],
        serde_json::json!(["assets/used.png", "assets/missing.png"]),
        "{missing}"
    );
}

#[tokio::test]
async fn the_engram_orphan_check_never_sees_a_reflection_file() {
    let fx = with_findings().await;
    let p = EvolveParams {
        rules: vec!["V107".into(), "V108".into()],
        ..Default::default()
    };
    let found = fx
        .engine
        .evolve_detect(&p, &Scope::Unrestricted)
        .await
        .unwrap()
        .to_string();
    for name in ["orphan.png", "missing.png", "pact.png", "used.png"] {
        assert!(
            !found.contains(name),
            "{name} reached the engram sweep: {found}"
        );
    }
    let attachments: Vec<String> = fx
        .engine
        .attachment_list("eng")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(attachments, ["assets/shot.png"]);
    // The read heal (D4's one missed filter): a file that reached the disk
    // with no sync has no row, so the read writes one, into the home's own
    // table and never into `attachment` (pre-flight scan 2, P8).
    std::fs::write(fx.root.join("reflections/assets/late.png"), PNG).unwrap();
    let (bytes, row) = fx
        .engine
        .reflection_asset_read("eng", "assets/late.png", &agent())
        .await
        .unwrap();
    assert_eq!(
        (bytes.as_slice(), row.path.as_str()),
        (PNG, "assets/late.png")
    );
    let attachments: Vec<String> = fx
        .engine
        .attachment_list("eng")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(
        attachments,
        ["assets/shot.png"],
        "the heal never writes an engram attachment row"
    );
    let files: Vec<String> = fx
        .engine
        .reflection_asset_list("eng", &agent())
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert!(
        files.iter().any(|p| p == "assets/late.png"),
        "the heal writes the Reflection's own row: {files:?}"
    );
}

#[tokio::test]
async fn a_changed_file_reference_is_a_feedback_change_of_its_section() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&curated("a.md", "# A\n\n![a](assets/a.png)\n"), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("a.md", "# A\n\n![b](assets/b.png)\n"), &person())
        .await
        .unwrap();
    let p = ReflectParams {
        kinds: vec!["feedback".into()],
        ..Default::default()
    };
    let queue = fx.engine.reflect(&p, &agent()).await.unwrap();
    assert_eq!(queue["total"], 1, "{queue}");
    let diff = queue["queue"][0]["diff"].as_str().unwrap();
    assert!(
        diff.contains("-![a](assets/a.png)") && diff.contains("+![b](assets/b.png)"),
        "{diff}"
    );
}

/// The write list caps the whole account, so an off-list contributor
/// lists the files and adds none, and is told why.
#[tokio::test]
async fn an_off_list_contributor_adds_no_file() {
    let fx = reflecting().await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("lisa", "lisa", None, Role::None, "pw12345678")
        .await
        .unwrap();
    auth.set_reflection_level("lisa", ReflectionLevel::Contributor)
        .await
        .unwrap();
    auth.set_write_list("lisa", &["agents-*".to_string()])
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let lisa = ReflectionCaller::person(
        Scope::User {
            account: "lisa".into(),
            admin: false,
        },
        "lisa",
    );
    fx.engine.reflection_asset_list("eng", &lisa).await.unwrap();
    let err = fx
        .engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &lisa)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        crystalline_service::WriteBlock::WriteList {
            account: "lisa".into()
        }
        .sentence("eng")
    );
    assert!(!fx.root.join("reflections/assets/flow.png").exists());
}

/// A `Reflections` folder spelled another way on disk keeps the files: no
/// second spelling appears beside it (the pages' rule, Task 12).
#[tokio::test]
async fn files_live_in_the_reflection_folder_spelling_on_disk() {
    let fx = reflecting().await;
    std::fs::create_dir_all(fx.root.join("Reflections")).unwrap();
    fx.engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    let folders: Vec<String> = std::fs::read_dir(&fx.root)
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.eq_ignore_ascii_case("reflections"))
        .collect();
    assert_eq!(folders, ["Reflections"]);
    assert_eq!(
        std::fs::read(fx.root.join("Reflections/assets/flow.png")).unwrap(),
        PNG
    );
    let (bytes, _) = fx
        .engine
        .reflection_asset_read("eng", "assets/flow.png", &person())
        .await
        .unwrap();
    assert_eq!(bytes, PNG);
}

/// A move whose rows cannot follow is undone: the file, its row and every
/// link stay as they were.
#[tokio::test]
async fn a_move_whose_rows_fail_is_undone() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/a.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("p.md", "# P\n\n![a](assets/a.png)\n"), &person())
        .await
        .unwrap();
    fx.engine.fail_next_asset_move(false);
    let err = fx
        .engine
        .move_reflection_asset("eng", "assets/a.png", "assets/b.png", &agent())
        .await;
    assert!(err.is_err(), "{err:?}");
    assert!(
        fx.root.join("reflections/assets/a.png").is_file(),
        "the rename is undone"
    );
    assert!(!fx.root.join("reflections/assets/b.png").exists());
    let paths: Vec<String> = fx
        .engine
        .reflection_asset_list("eng", &agent())
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(paths, ["assets/a.png"]);
    let page = std::fs::read_to_string(fx.root.join("reflections/p.md")).unwrap();
    assert!(page.contains("![a](assets/a.png)"), "{page}");
    // The failpoint is spent: the next move goes through.
    fx.engine
        .move_reflection_asset("eng", "assets/a.png", "assets/b.png", &agent())
        .await
        .unwrap();
}

/// When the undo fails too, the file stays moved, the links follow it, and
/// the receipt says plainly that the row is behind until the next sync.
#[tokio::test]
async fn a_move_that_cannot_be_undone_says_what_is_left() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/a.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("p.md", "# P\n\n![a](assets/a.png)\n"), &person())
        .await
        .unwrap();
    fx.engine.fail_next_asset_move(true);
    let moved = fx
        .engine
        .move_reflection_asset("eng", "assets/a.png", "assets/b.png", &agent())
        .await
        .unwrap();
    assert!(
        moved["note"].as_str().unwrap().contains("next sync"),
        "{moved}"
    );
    assert_eq!(moved["links_rewritten"], 1, "{moved}");
    assert!(fx.root.join("reflections/assets/b.png").is_file());
    let page = std::fs::read_to_string(fx.root.join("reflections/p.md")).unwrap();
    assert!(page.contains("![a](assets/b.png)"), "{page}");
    fx.engine.sync(None).await.unwrap();
    let paths: Vec<String> = fx
        .engine
        .reflection_asset_list("eng", &agent())
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(paths, ["assets/b.png"], "the sync heals the row");
}

#[tokio::test]
async fn a_move_onto_a_file_or_of_a_missing_file_changes_nothing() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/a.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    fx.engine
        .reflection_asset_write("eng", "assets/b.png", b"bee".to_vec(), &person())
        .await
        .unwrap();
    let err = fx
        .engine
        .move_reflection_asset("eng", "assets/a.png", "assets/b.png", &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("already holds a file"), "{err}");
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/a.png")).unwrap(),
        PNG
    );
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/b.png")).unwrap(),
        b"bee"
    );
    let err = fx
        .engine
        .move_reflection_asset("eng", "assets/gone.png", "assets/c.png", &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no file 'assets/gone.png'"), "{err}");
    assert!(!fx.root.join("reflections/assets/c.png").exists());
    let err = fx
        .engine
        .move_reflection_asset("eng", "assets/a.png", "flow.png", &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("attachment path"),
        "a bad path is named first: {err}"
    );
}

/// A page renamed only by letter case keeps its text and ends up with the new
/// spelling on a case-sensitive and a case-insensitive disk alike.
#[tokio::test]
async fn a_page_renamed_only_by_letter_case_keeps_its_text() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&curated("deploying.md", "# Deploying\n"), &agent())
        .await
        .unwrap();
    let home = fx.root.join("reflections");
    std::fs::create_dir_all(home.join(".reflected")).unwrap();
    std::fs::write(home.join(".reflected/deploying.md"), "# Deploying\n").unwrap();
    std::fs::write(home.join("deploying.threads.md"), "threads").unwrap();
    fx.engine
        .move_reflection("eng", "deploying.md", "Deploying.md", &agent())
        .await
        .unwrap();
    let listed = |dir: &std::path::Path| -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_lowercase().starts_with("deploying"))
            .collect()
    };
    let mut names = listed(&home);
    names.sort();
    assert_eq!(names, ["Deploying.md", "Deploying.threads.md"]);
    assert_eq!(listed(&home.join(".reflected")), ["Deploying.md"]);
    assert_eq!(
        std::fs::read_to_string(home.join("Deploying.threads.md")).unwrap(),
        "threads"
    );
    let text = std::fs::read_to_string(home.join("Deploying.md")).unwrap();
    assert!(text.contains("# Deploying"), "{text}");
    // The store row follows: the page reads under the new name.
    fx.engine
        .read_reflection("eng", "Deploying.md", &agent())
        .await
        .unwrap();
}

/// A move keeps the extension, in any letter case, and a refusal changes
/// nothing: no page is rewritten and no file moves.
#[tokio::test]
async fn a_move_keeps_the_extension_and_a_refusal_changes_nothing() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/a.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    let err = fx
        .engine
        .move_reflection_asset("eng", "assets/a.png", "assets/a.jpg", &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("keeps the .png extension"), "{err}");
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/a.png")).unwrap(),
        PNG
    );
    assert!(!fx.root.join("reflections/assets/a.jpg").exists());
    fx.engine
        .move_reflection_asset("eng", "assets/a.png", "assets/a.PNG", &agent())
        .await
        .unwrap();
    // The listing shows the new spelling on a case-sensitive and a
    // case-insensitive disk alike.
    let names: Vec<String> = std::fs::read_dir(fx.root.join("reflections/assets"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.PNG"]);
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/a.PNG")).unwrap(),
        PNG
    );
}

/// A read-only domain freezes the files: no add, replace, move or remove.
#[tokio::test]
async fn a_read_only_domain_freezes_the_files() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/a.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    std::fs::write(
        fx.root.join("MANIFEST.md"),
        manifest("reflection: on\nread_only: on\n"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let frozen = |e: crystalline_service::EngineError| {
        matches!(e, crystalline_service::EngineError::DomainReadOnly(_))
    };
    assert!(frozen(
        fx.engine
            .reflection_asset_write("eng", "assets/n.png", PNG.to_vec(), &person())
            .await
            .unwrap_err()
    ));
    assert!(frozen(
        fx.engine
            .reflection_asset_write("eng", "assets/a.png", b"x".to_vec(), &person())
            .await
            .unwrap_err()
    ));
    assert!(frozen(
        fx.engine
            .move_reflection_asset("eng", "assets/a.png", "assets/b.png", &agent())
            .await
            .unwrap_err()
    ));
    assert!(frozen(
        fx.engine
            .reflection_asset_delete("eng", "assets/a.png", &person())
            .await
            .unwrap_err()
    ));
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/a.png")).unwrap(),
        PNG
    );
    assert!(!fx.root.join("reflections/assets/n.png").exists());
    assert!(!fx.root.join("reflections/assets/b.png").exists());
    fx.engine
        .reflection_asset_read("eng", "assets/a.png", &person())
        .await
        .unwrap();
}

/// The replace check reads the disk: a file no sync has seen yet has no row
/// and is still not replaced by a contributor.
#[tokio::test]
async fn a_contributor_never_replaces_a_file_the_index_has_not_seen() {
    let (fx, _, nora, _) = three_people().await;
    std::fs::create_dir_all(fx.root.join("reflections/assets")).unwrap();
    std::fs::write(fx.root.join("reflections/assets/late.png"), PNG).unwrap();
    let err = fx
        .engine
        .reflection_asset_write("eng", "assets/late.png", b"other".to_vec(), &nora)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("new name"), "{err}");
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/late.png")).unwrap(),
        PNG
    );
}

/// A create-only write never replaces a file, whatever the right: not one an
/// upload stored, not one only the disk holds (no sync has seen it), not even
/// a curator's. The refusal is the create-only one for a contributor too, not
/// the curate refusal of a plain replace.
#[tokio::test]
async fn a_create_only_write_never_replaces_a_file() {
    use crystalline_service::EngineError;
    use crystalline_service::engine::ASSET_EXISTS;
    let (fx, vera, nora, ed) = three_people().await;
    let err = fx
        .engine
        .reflection_asset_create("eng", "assets/flow.png", PNG.to_vec(), &vera)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Forbidden(_)),
        "a reader adds nothing: {err}"
    );
    let created = fx
        .engine
        .reflection_asset_create("eng", "assets/flow.png", PNG.to_vec(), &nora)
        .await
        .unwrap();
    assert!(created.created);
    std::fs::write(fx.root.join("reflections/assets/late.png"), PNG).unwrap();
    for path in ["assets/flow.png", "assets/late.png"] {
        for who in [&nora, &ed] {
            let err = fx
                .engine
                .reflection_asset_create("eng", path, b"other".to_vec(), who)
                .await
                .unwrap_err();
            assert!(
                matches!(&err, EngineError::Conflict(text) if *text == format!("'{path}' {ASSET_EXISTS}")),
                "{path}: {err}"
            );
        }
        assert_eq!(
            std::fs::read(fx.root.join("reflections").join(path)).unwrap(),
            PNG,
            "{path} keeps its bytes"
        );
    }
}

/// An engram embed a page already holds (written by hand) does not hold up a
/// change to another section; a write that adds one is refused.
#[tokio::test]
async fn only_an_added_engram_embed_is_refused() {
    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        ("assets/shot.png", "an engram attachment"),
        ("reflections/b.md", "# B\n\n## Shot\n<!-- r:aaaa0003 -->\n\n![e](../assets/shot.png)\n\n## Notes\n<!-- r:aaaa0004 -->\n\nFirst.\n"),
    ])
    .await;
    let section = |content: &str| WriteReflectionParams {
        domain: "eng".into(),
        path: "b.md".into(),
        operation: Some("replace_section".into()),
        section: Some("## Notes".into()),
        content: Some(content.into()),
        ..Default::default()
    };
    fx.engine
        .write_reflection(&section("Second.\n"), &person())
        .await
        .unwrap();
    let err = fx
        .engine
        .write_reflection(&section("![d](../assets/deck.pdf)\n"), &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("engram attachment"), "{err}");
}

/// An undo never replaces a file that appeared at the old path: the moved
/// file stays at the new one, and the receipt says why.
#[tokio::test]
async fn an_undo_never_replaces_a_file_that_appeared_at_the_old_path() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/a.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    fx.engine.fail_next_asset_move_with_a_file_at_the_old_path();
    let moved = fx
        .engine
        .move_reflection_asset("eng", "assets/a.png", "assets/b.png", &agent())
        .await
        .unwrap();
    let note = moved["note"].as_str().unwrap();
    assert!(
        note.contains("a new file appeared at 'assets/a.png'") && note.contains("next sync"),
        "{moved}"
    );
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/a.png")).unwrap(),
        b"new",
        "the new file is kept"
    );
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/b.png")).unwrap(),
        PNG,
        "the moved file stays"
    );
}

/// A domain export carries the Reflection's files while the switch is on,
/// and with it off the folder is nobody's.
#[tokio::test]
async fn a_domain_export_carries_the_reflection_files() {
    let fx = reflecting().await;
    fx.engine
        .reflection_asset_write("eng", "assets/flow.png", PNG.to_vec(), &person())
        .await
        .unwrap();
    let files = fx.engine.domain_files("eng").await.unwrap();
    let got = files
        .iter()
        .find(|(p, _)| p == "reflections/assets/flow.png")
        .map(|(_, b)| b.as_slice());
    assert_eq!(got, Some(PNG));
    // Markdown in the folder that is no page is an engram waiting to move
    // out: it travels as it did before the Reflection.
    std::fs::write(
        fx.root.join("reflections/assets/notes.md"),
        engram("Notes", "notes", "- [fact] notes"),
    )
    .unwrap();
    let files = fx.engine.domain_files("eng").await.unwrap();
    assert!(
        files
            .iter()
            .any(|(p, _)| p == "reflections/assets/notes.md"),
        "an engram waiting in the folder is exported"
    );
    let dest = fx.tmp.path().join("export");
    fx.engine
        .export_domain("eng", &dest, false, false)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(dest.join("reflections/assets/flow.png")).unwrap(),
        PNG
    );
    let off = fixture(&[("reflections/assets/stray.png", "x")]).await;
    let files = off.engine.domain_files("eng").await.unwrap();
    assert!(
        files.iter().all(|(p, _)| !p.starts_with("reflections/")),
        "with the switch off the folder is nobody's"
    );
}

/// While the switch is on the export carries the hidden baseline and
/// the threads, so a restore keeps every reflected section; and no import
/// reads a file under reflections/ as an engram where a Reflection keeps it.
#[tokio::test]
async fn an_export_carries_the_baseline_and_no_import_reads_reflections_as_engrams() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(
            &curated("guides/a.md", "# A\n\n## One\n\nText.\n"),
            &agent(),
        )
        .await
        .unwrap();
    let opened = DiscussParams {
        domain: "eng".into(),
        path: Some("guides/a.md".into()),
        section: Some("One".into()),
        text: Some("question (non-blocking): still true?".into()),
        ..Default::default()
    };
    fx.engine.discuss(&opened, &person()).await.unwrap();
    let files = fx.engine.domain_files("eng").await.unwrap();
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    for want in [
        "reflections/guides/a.md",
        "reflections/guides/a.threads.md",
        "reflections/.reflected/guides/a.md",
    ] {
        assert!(paths.contains(&want), "{want} missing from {paths:?}");
    }
    let off = fixture(&[("reflections/.reflected/x.md", "# X\n")]).await;
    let files = off.engine.domain_files("eng").await.unwrap();
    assert!(
        files.iter().all(|(p, _)| !p.contains("/.reflected/")),
        "switch off: a dot folder is nobody's"
    );

    let stray = engram("Stray", "reflections/stray", "- [fact] stray");
    // The receiving domain keeps a Reflection: the entry is reported, not imported.
    let out = fx
        .engine
        .import_domain_files(
            "eng",
            &[("reflections/stray.md".to_string(), stray.clone())],
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(out["files"][0]["action"], "ignored", "{out}");
    assert_eq!(
        out["files"][0]["reason"],
        crystalline_service::engine::REFLECTION_NOT_IMPORTED,
        "{out}"
    );
    assert!(!fx.root.join("reflections/stray.md").exists());
    // Switch off here, but the batch's own MANIFEST says on: the same.
    let batch = vec![
        ("MANIFEST.md".to_string(), manifest("reflection: on\n")),
        ("reflections/stray.md".to_string(), stray.clone()),
        (
            "notes/kept.md".to_string(),
            engram("Kept", "notes/kept", "- [fact] kept"),
        ),
    ];
    let out = off
        .engine
        .import_domain_files("eng", &batch, false, false)
        .await
        .unwrap();
    let action = |path: &str| {
        out["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .unwrap()["action"]
            .clone()
    };
    assert_eq!(action("reflections/stray.md"), "ignored", "{out}");
    assert_eq!(action("notes/kept.md"), "create", "{out}");
    // Without a Reflection anywhere, reflections/ is an ordinary folder, as today.
    let out = off
        .engine
        .import_domain_files(
            "eng",
            &[(
                "reflections/old.md".to_string(),
                engram("Old", "reflections/old", "- [fact] old"),
            )],
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(out["files"][0]["action"], "create", "{out}");

    // A directory import into a virtual domain whose MANIFEST says on.
    let vfx = crate::fixture::fixture_with(&[], |cfg, _| {
        cfg.domains.insert(
            "vd".into(),
            crystalline_core::config::DomainEntry::virtual_domain(),
        );
    })
    .await;
    let src = vfx.tmp.path().join("src");
    std::fs::create_dir_all(src.join("reflections")).unwrap();
    std::fs::create_dir_all(src.join("notes")).unwrap();
    std::fs::write(src.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    std::fs::write(src.join("reflections/stray.md"), &stray).unwrap();
    std::fs::write(
        src.join("notes/kept.md"),
        engram("Kept", "notes/kept", "- [fact] kept"),
    )
    .unwrap();
    let out = vfx
        .engine
        .import_domain("vd", &src, false, false)
        .await
        .unwrap();
    let written: Vec<&str> = out["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        written.contains(&"notes/kept.md") && !written.contains(&"reflections/stray.md"),
        "{out}"
    );
    assert!(
        out["warnings"].to_string().contains("reflections/stray.md"),
        "{out}"
    );
}

/// `write_asset` params for `path` with `base64` of `bytes`.
fn upload(path: &str, bytes: &[u8], overwrite: bool) -> WriteReflectionParams {
    use base64::Engine as _;
    WriteReflectionParams {
        domain: "eng".into(),
        action: Some("write_asset".into()),
        path: path.into(),
        base64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        overwrite,
        ..Default::default()
    }
}

/// An agent working for `caller`'s account, as the MCP server names one.
fn agent_of(caller: &ReflectionCaller) -> ReflectionCaller {
    ReflectionCaller::agent(caller.scope.clone(), "claude-code", false)
}

/// An agent's upload is a person's upload: a reader adds nothing, a
/// contributor (Edit) creates and is refused a replace, a curator replaces.
#[tokio::test]
async fn an_agent_uploads_through_write_reflection_under_the_reflection_rights() {
    use crystalline_service::EngineError;
    use crystalline_service::engine::ASSET_EXISTS;
    let (fx, vera, nora, ed) = three_people().await;
    let (vera, nora, ed) = (agent_of(&vera), agent_of(&nora), agent_of(&ed));
    let err = fx
        .engine
        .write_reflection(&upload("assets/flow.png", PNG, false), &vera)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Forbidden(_)), "{err}");
    assert!(!fx.root.join("reflections/assets/flow.png").exists());

    let created = fx
        .engine
        .write_reflection(&upload("assets/flow.png", PNG, false), &nora)
        .await
        .unwrap();
    assert_eq!(created["created"], true, "{created}");
    assert_eq!(created["path"], "assets/flow.png", "{created}");
    assert_eq!(created["domain"], "eng", "{created}");
    assert_eq!(created["mime"], "image/png", "{created}");
    assert_eq!(created["size"], PNG.len(), "{created}");
    assert!(created["sha256"].as_str().is_some_and(|s| s.len() == 64));

    let err = fx
        .engine
        .write_reflection(&upload("assets/flow.png", b"other", false), &nora)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::Conflict(t) if t.ends_with(ASSET_EXISTS)),
        "{err}"
    );
    let err = fx
        .engine
        .write_reflection(&upload("assets/flow.png", b"other", true), &nora)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("curate"), "replacing needs curate: {err}");
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/flow.png")).unwrap(),
        PNG
    );

    let replaced = fx
        .engine
        .write_reflection(&upload("assets/flow.png", b"other", true), &ed)
        .await
        .unwrap();
    assert_eq!(replaced["created"], false, "{replaced}");
    assert_eq!(
        std::fs::read(fx.root.join("reflections/assets/flow.png")).unwrap(),
        b"other"
    );
}

#[tokio::test]
async fn a_read_only_domain_refuses_an_agents_upload() {
    let fx = reflecting().await;
    std::fs::write(
        fx.root.join("MANIFEST.md"),
        manifest("reflection: on\nread_only: on\n"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let err = fx
        .engine
        .write_reflection(&upload("assets/n.png", PNG, false), &agent())
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::DomainReadOnly(_)),
        "{err}"
    );
    assert!(!fx.root.join("reflections/assets/n.png").exists());
}

#[tokio::test]
async fn an_unknown_action_names_write_asset() {
    let fx = reflecting().await;
    let p = WriteReflectionParams {
        domain: "eng".into(),
        action: Some("upload".into()),
        path: "assets/a.png".into(),
        ..Default::default()
    };
    let err = fx
        .engine
        .write_reflection(&p, &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("write_asset"), "{err}");
}
