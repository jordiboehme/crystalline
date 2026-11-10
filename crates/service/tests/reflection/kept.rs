//! Kept texts over the JSON API: each entry goes only to a person who may
//! write its source (a Reflection text: edit, PACT.md: curate, as its room
//! needs; an engram text: write access to the engrams), and a reader is
//! refused.

use chrono::TimeZone;
use crystalline_core::kept::{self, KeptKind};
use crystalline_service::rest::{MemberLevel, Role};
use reqwest::Method;
use serde_json::Value;

use crate::rest::{Opts, Served, Session, anonymous, serve, serve_opts, serve_with};

const PAGE: &str = "guides/deploying.md";
const ENGRAM: &str = "runbooks/deploy.md";

/// Keep one Reflection text and one engram text for `eng`, and one for
/// another domain, the way a closing room does. Answers the ids of the two
/// `eng` texts, page first.
fn keep(s: &Served) -> (String, String) {
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 8, 0, 0).unwrap();
    kept::write(
        &root,
        "eng",
        None,
        KeptKind::Reflection,
        PAGE,
        at,
        "Typed on the page.\n",
    )
    .unwrap();
    let later = at + chrono::Duration::minutes(1);
    kept::write(
        &root,
        "eng",
        None,
        KeptKind::Engram,
        ENGRAM,
        later,
        "Typed on the engram.\n",
    )
    .unwrap();
    kept::write(
        &root,
        "ops",
        None,
        KeptKind::Reflection,
        PAGE,
        at,
        "Another domain.\n",
    )
    .unwrap();
    let found = kept::scan(&root);
    let id = |kind: KeptKind| {
        found
            .iter()
            .find(|k| k.domain == "eng" && k.kind == kind)
            .unwrap()
            .id
            .clone()
    };
    (id(KeptKind::Reflection), id(KeptKind::Engram))
}

/// `vic`: an instance viewer invited into `eng` as an editor. The
/// invitation gives curate on the Reflection, and the instance role keeps
/// the engrams read only.
async fn add_vic(s: &Served) {
    s.auth
        .add_user("vic", "vic", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    s.auth
        .upsert_domain_member("eng", "vic", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
}

fn listed(body: &Value) -> Vec<(String, String)> {
    body["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["kind"].as_str().unwrap().to_string(),
                k["path"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// An editor sees both kinds, oldest first, never another domain's; one
/// entry carries its text; a delete takes exactly that file.
#[tokio::test]
async fn an_editor_lists_reads_and_drops_the_kept_texts() {
    let s = serve().await;
    let (page_id, engram_id) = keep(&s);
    let ed = Session::login(s.addr, "ed").await;

    let (status, body) = ed.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["domain"], "eng");
    assert_eq!(
        listed(&body),
        vec![
            ("reflection".to_string(), PAGE.to_string()),
            ("engram".to_string(), ENGRAM.to_string()),
        ]
    );
    let first = &body["kept"][0];
    assert_eq!(first["id"], page_id.as_str());
    assert_eq!(first["kept_at"], "2026-10-15T08:00:00Z");
    assert_eq!(first["bytes"], 19);
    // A text, and no server path: `file` is a flag, never the file's place.
    assert_eq!(first["file"], false, "{first}");
    assert_eq!(first["draft"], false);

    let (status, one) = ed.get(&format!("/api/v1/domains/eng/kept/{page_id}")).await;
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["text"], "Typed on the page.\n");
    assert_eq!(one["path"], PAGE);
    assert_eq!(one["kind"], "reflection");

    let (status, gone) = ed
        .send(
            Method::DELETE,
            &format!("/api/v1/domains/eng/kept/{engram_id}"),
            None,
        )
        .await;
    assert_eq!(status, 204, "{gone}");
    let (_, body) = ed.get("/api/v1/domains/eng/kept").await;
    assert_eq!(
        listed(&body),
        vec![("reflection".to_string(), PAGE.to_string())]
    );
    let (status, _) = ed
        .get(&format!("/api/v1/domains/eng/kept/{engram_id}"))
        .await;
    assert_eq!(status, 404);
    let root = s.fx.tmp.path().join("state/kept");
    assert_eq!(kept::scan(&root).len(), 2, "the other two stay");
}

/// Each entry by its own right: `vic` curates the Reflection and may not
/// write engrams, so it sees, reads and drops the page's text only; the
/// engram's text answers 404 as if it were not there, and stays.
#[tokio::test]
async fn each_entry_needs_the_right_to_write_its_own_source() {
    let s = serve().await;
    add_vic(&s).await;
    let (page_id, engram_id) = keep(&s);
    let vic = Session::login(s.addr, "vic").await;

    let (status, body) = vic.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        listed(&body),
        vec![("reflection".to_string(), PAGE.to_string())]
    );
    let (status, _) = vic
        .get(&format!("/api/v1/domains/eng/kept/{engram_id}"))
        .await;
    assert_eq!(status, 404);
    let (status, _) = vic
        .send(
            Method::DELETE,
            &format!("/api/v1/domains/eng/kept/{engram_id}"),
            None,
        )
        .await;
    assert_eq!(status, 404);
    let (status, one) = vic
        .get(&format!("/api/v1/domains/eng/kept/{page_id}"))
        .await;
    assert_eq!(status, 200, "{one}");
    let (status, _) = vic
        .send(
            Method::DELETE,
            &format!("/api/v1/domains/eng/kept/{page_id}"),
            None,
        )
        .await;
    assert_eq!(status, 204);
    let root = s.fx.tmp.path().join("state/kept");
    let left: Vec<KeptKind> = kept::scan(&root)
        .into_iter()
        .filter(|k| k.domain == "eng")
        .map(|k| k.kind)
        .collect();
    assert_eq!(left, vec![KeptKind::Engram], "the engram's text stays");
}

/// A reader of the domain (`vera`) may handle only kept comments she sent:
/// she gets an empty listing and none of the page or engram texts; a person who
/// may not see the domain (`nora`) meets an unknown domain. Nothing is
/// dropped.
#[tokio::test]
async fn a_reader_gets_no_page_text_and_a_stranger_meets_an_unknown_domain() {
    let s = serve().await;
    let (page_id, engram_id) = keep(&s);
    // Vera reads the Reflection, so she may handle a kept comment she sent:
    // the listing answers, with none of the texts kept here, and each of
    // them answers as unknown.
    let vera = Session::login(s.addr, "vera").await;
    let (status, body) = vera.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["kept"], serde_json::json!([]), "{body}");
    for (name, expected) in [("vera", 404), ("nora", 404)] {
        let who = Session::login(s.addr, name).await;
        let listing =
            (name == "nora").then(|| (Method::GET, "/api/v1/domains/eng/kept".to_string()));
        for (method, path) in listing.into_iter().chain([
            (Method::GET, format!("/api/v1/domains/eng/kept/{page_id}")),
            (Method::GET, format!("/api/v1/domains/eng/kept/{engram_id}")),
            (
                Method::DELETE,
                format!("/api/v1/domains/eng/kept/{page_id}"),
            ),
            (
                Method::DELETE,
                format!("/api/v1/domains/eng/kept/{engram_id}"),
            ),
        ]) {
            let (status, body) = who.send(method.clone(), &path, None).await;
            assert_eq!(status, expected, "{name} {method} {path}: {body}");
            assert!(
                !body.to_string().contains("Typed on"),
                "{name} gets no text: {body}"
            );
        }
    }
    let root = s.fx.tmp.path().join("state/kept");
    assert_eq!(kept::scan(&root).len(), 3, "nothing is dropped");
}

/// A Reflection text needs what saving its page in a room needs: `ann`, a
/// contributor who edits the Reflection and reads no engram, gets the
/// page's text, but neither the engram's text nor the PACT's, which needs
/// curate; both of those answer 404 and stay.
#[tokio::test]
async fn a_contributor_gets_a_pages_text_and_not_the_pacts() {
    let s = serve().await;
    let (page_id, engram_id) = keep(&s);
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 9, 0, 0).unwrap();
    kept::write(
        &root,
        "eng",
        None,
        KeptKind::Reflection,
        "PACT.md",
        at,
        "Typed on the pact.\n",
    )
    .unwrap();
    let pact_id = kept::scan(&root)
        .into_iter()
        .find(|k| k.path == "PACT.md")
        .unwrap()
        .id;
    let ann = Session::login(s.addr, "ann").await;
    let (status, body) = ann.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        listed(&body),
        vec![("reflection".to_string(), PAGE.to_string())]
    );
    let (status, one) = ann
        .get(&format!("/api/v1/domains/eng/kept/{page_id}"))
        .await;
    assert_eq!(status, 200, "{one}");
    for id in [&engram_id, &pact_id] {
        for method in [Method::GET, Method::DELETE] {
            let (status, body) = ann
                .send(
                    method.clone(),
                    &format!("/api/v1/domains/eng/kept/{id}"),
                    None,
                )
                .await;
            assert_eq!(status, 404, "{method} {id}: {body}");
        }
    }
    assert_eq!(kept::scan(&root).len(), 4, "nothing is dropped");
    let keeper = Session::login(s.addr, "keeper").await;
    let (status, _) = keeper
        .get(&format!("/api/v1/domains/eng/kept/{pact_id}"))
        .await;
    assert_eq!(status, 200, "a curator gets the pact's text");
}

/// An unfinished keep (a temporary file a crash left) is never served over
/// the JSON API: neither listed nor read.
#[tokio::test]
async fn an_unfinished_keep_is_not_served() {
    let s = serve().await;
    let folder = s.fx.tmp.path().join("state/kept/eng/reflection/guides");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join(".deploying.md.20261015T080000Z.42-0.keeping");
    std::fs::write(&file, "Half kept.\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(120))
        .unwrap();
    let root = s.fx.tmp.path().join("state/kept");
    let id = kept::scan(&root)[0].id.clone();
    let keeper = Session::login(s.addr, "keeper").await;
    let (status, body) = keeper.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert!(listed(&body).is_empty(), "{body}");
    let (status, _) = keeper.get(&format!("/api/v1/domains/eng/kept/{id}")).await;
    assert_eq!(status, 404);
}

/// The anonymous viewer of a public domain is asked to sign in.
#[tokio::test]
async fn the_anonymous_viewer_is_asked_to_sign_in() {
    let s = serve_with(true).await;
    let (page_id, _) = keep(&s);
    for (method, path) in [
        (Method::GET, "/api/v1/domains/eng/kept".to_string()),
        (Method::GET, format!("/api/v1/domains/eng/kept/{page_id}")),
        (
            Method::DELETE,
            format!("/api/v1/domains/eng/kept/{page_id}"),
        ),
    ] {
        let (status, body) = anonymous(s.addr, method.clone(), &path, None).await;
        assert_eq!(status, 401, "{method} {path}: {body}");
    }
    let (status, _) = anonymous(s.addr, Method::GET, "/api/v1/domains/nope/kept", None).await;
    assert_eq!(status, 404);
}

/// A text kept because the Reflection was switched off is still listed: the
/// Reflection's state plays no part.
#[tokio::test]
async fn a_switched_off_reflection_still_lists_its_kept_texts() {
    let s = serve().await;
    let (page_id, _) = keep(&s);
    let manifest = s.fx.root.join("MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap().replacen(
        "reflection: on\n",
        "reflection: off\n",
        1,
    );
    std::fs::write(&manifest, text).unwrap();
    s.fx.engine.sync(Some("eng")).await.unwrap();
    let keeper = Session::login(s.addr, "keeper").await;
    let (status, one) = keeper
        .get(&format!("/api/v1/domains/eng/kept/{page_id}"))
        .await;
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["text"], "Typed on the page.\n");
}

/// A text kept from a review draft is its owner's alone: `ed` lists, reads
/// and drops the one from ed's draft, marked `draft`; `keeper`, who may
/// write every engram of the domain, never sees it, and its id answers 404.
#[tokio::test]
async fn a_drafts_kept_text_goes_to_its_owner_only() {
    let s = serve().await;
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 9, 0, 0).unwrap();
    kept::write(
        &root,
        "eng",
        Some("ed"),
        KeptKind::Engram,
        ENGRAM,
        at,
        "Typed in ed's draft.\n",
    )
    .unwrap();
    let id = kept::scan(&root)[0].id.clone();

    let keeper = Session::login(s.addr, "keeper").await;
    let (status, body) = keeper.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert!(listed(&body).is_empty(), "{body}");
    for method in [Method::GET, Method::DELETE] {
        let (status, body) = keeper
            .send(
                method.clone(),
                &format!("/api/v1/domains/eng/kept/{id}"),
                None,
            )
            .await;
        assert_eq!(status, 404, "{method}: {body}");
    }
    assert_eq!(kept::scan(&root).len(), 1, "keeper drops nothing");

    let ed = Session::login(s.addr, "ed").await;
    let (_, body) = ed.get("/api/v1/domains/eng/kept").await;
    assert_eq!(body["kept"][0]["draft"], true, "{body}");
    let (status, one) = ed.get(&format!("/api/v1/domains/eng/kept/{id}")).await;
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["text"], "Typed in ed's draft.\n");
    let (status, _) = ed
        .send(
            Method::DELETE,
            &format!("/api/v1/domains/eng/kept/{id}"),
            None,
        )
        .await;
    assert_eq!(status, 204);
    assert!(kept::scan(&root).is_empty());
}

/// A draft file kept as its bytes is listed as a file, is no text to read,
/// and downloads as its bytes, named for the file, never rendered in place;
/// another person meets it as if it were not there.
#[tokio::test]
async fn a_kept_file_is_listed_as_a_file_and_downloads_as_its_bytes() {
    let s = serve().await;
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 9, 0, 0).unwrap();
    let bytes = b"%PDF-1.7\n\xff\xfe her deck".to_vec();
    kept::write_bytes(
        &root,
        "eng",
        Some("ed"),
        KeptKind::Engram,
        "assets/deck.pdf",
        at,
        &bytes,
    )
    .unwrap();
    let id = kept::scan(&root)[0].id.clone();

    let ed = Session::login(s.addr, "ed").await;
    let (status, body) = ed.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["kept"][0]["path"], "assets/deck.pdf", "{body}");
    assert_eq!(body["kept"][0]["file"], true, "{body}");

    let (status, one) = ed.get(&format!("/api/v1/domains/eng/kept/{id}")).await;
    assert_eq!(status, 422, "{one}");
    assert_eq!(
        one["detail"],
        "This kept copy is a file, not text. Download it from the kept list."
    );

    let (status, headers, got) = ed
        .get_raw(&format!("/api/v1/domains/eng/kept/{id}/file"))
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&got));
    assert_eq!(got, bytes);
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    };
    assert_eq!(header("content-type"), "application/octet-stream");
    assert_eq!(
        header("content-disposition"),
        "attachment; filename=\"deck.pdf\""
    );
    assert_eq!(header("x-content-type-options"), "nosniff");
    assert_eq!(
        header("content-security-policy"),
        "default-src 'none'; sandbox"
    );

    let keeper = Session::login(s.addr, "keeper").await;
    let (status, _, _) = keeper
        .get_raw(&format!("/api/v1/domains/eng/kept/{id}/file"))
        .await;
    assert_eq!(status, 404, "a draft's copy is its owner's alone");
    let (status, _, _) = ed
        .get_raw("/api/v1/domains/eng/kept/000000000000/file")
        .await;
    assert_eq!(status, 404);
}

/// A draft file kept before kept files took their own extension is named
/// `.md`. It is still listed as a file, downloads as its bytes and can be
/// dropped.
#[tokio::test]
async fn an_old_md_named_kept_file_lists_downloads_and_drops() {
    let s = serve().await;
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let folder = root.join("eng/draft/ed/engram/assets");
    std::fs::create_dir_all(&folder).unwrap();
    let bytes = b"%PDF-1.7\n\xff\xfe her old deck".to_vec();
    std::fs::write(folder.join("deck.pdf.20261015T080000Z.md"), &bytes).unwrap();
    let id = kept::scan(&root)[0].id.clone();

    let ed = Session::login(s.addr, "ed").await;
    let (_, body) = ed.get("/api/v1/domains/eng/kept").await;
    assert_eq!(body["kept"][0]["path"], "assets/deck.pdf", "{body}");
    assert_eq!(body["kept"][0]["file"], true, "{body}");
    let (status, headers, got) = ed
        .get_raw(&format!("/api/v1/domains/eng/kept/{id}/file"))
        .await;
    assert_eq!(status, 200);
    assert_eq!(got, bytes);
    assert_eq!(
        headers.get("content-disposition").unwrap(),
        "attachment; filename=\"deck.pdf\""
    );
    let (status, _) = ed
        .send(
            Method::DELETE,
            &format!("/api/v1/domains/eng/kept/{id}"),
            None,
        )
        .await;
    assert_eq!(status, 204);
    assert!(kept::scan(&root).is_empty());
}

/// A kept copy the server cannot read is a fault on the server's side: the
/// read and the download answer 500 in a sentence that names no place on the
/// server's disk; the cause goes to the log.
#[cfg(unix)]
#[tokio::test]
async fn a_kept_copy_that_cannot_be_read_names_no_server_path() {
    use std::os::unix::fs::PermissionsExt;

    let s = serve().await;
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 9, 0, 0).unwrap();
    let file = kept::write_bytes(
        &root,
        "eng",
        Some("ed"),
        KeptKind::Engram,
        "assets/deck.pdf",
        at,
        b"her deck",
    )
    .unwrap();
    let id = kept::scan(&root)[0].id.clone();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();

    let ed = Session::login(s.addr, "ed").await;
    let tmp = s.fx.tmp.path().display().to_string();
    for path in [
        format!("/api/v1/domains/eng/kept/{id}"),
        format!("/api/v1/domains/eng/kept/{id}/file"),
    ] {
        let (status, _, body) = ed.get_raw(&path).await;
        let body = String::from_utf8_lossy(&body).into_owned();
        assert_eq!(status, 500, "{path}: {body}");
        assert!(!body.contains(&tmp), "{path}: {body}");
        assert!(!body.contains("kept/"), "{path}: {body}");
        assert!(
            body.contains("This kept copy could not be read."),
            "{path}: {body}"
        );
    }
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
}

/// A removed domain's kept texts are set aside, never deleted: a new domain
/// registered under the same name sees none of them over the JSON API,
/// while the files stay, listed as from a removed domain.
#[tokio::test]
async fn a_new_domain_with_a_removed_domains_name_sees_none_of_its_kept_texts() {
    let s = serve_opts(Opts {
        anonymous: false,
        private: false,
        origin: false,
    })
    .await;
    keep(&s);
    let keeper = Session::login(s.addr, "keeper").await;
    let (_, body) = keeper.get("/api/v1/domains/eng/kept").await;
    assert_eq!(listed(&body).len(), 2, "{body}");

    let engine = &s.fx.engine;
    engine
        .unregister_domain("eng", &crystalline_service::Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    engine
        .domain_add_local(Some("eng"), Some(s.fx.root.to_str().unwrap()))
        .await
        .unwrap();

    let (status, body) = keeper.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert!(listed(&body).is_empty(), "{body}");
    let root = s.fx.tmp.path().join("state/kept");
    let found = kept::scan(&root);
    let removed: Vec<&str> = found
        .iter()
        .filter(|k| k.domain == "eng" && k.removed)
        .map(|k| k.path.as_str())
        .collect();
    assert_eq!(removed, vec![PAGE, ENGRAM], "set aside, not deleted");
    for k in found.iter().filter(|k| k.removed) {
        let (status, _) = keeper
            .get(&format!("/api/v1/domains/eng/kept/{}", k.id))
            .await;
        assert_eq!(status, 404);
    }
}

/// A renamed domain takes its kept texts along: they answer under the new
/// name, and the old name holds none.
#[tokio::test]
async fn a_renamed_domain_takes_its_kept_texts_along() {
    let s = serve_opts(Opts {
        anonymous: false,
        private: false,
        origin: false,
    })
    .await;
    keep(&s);
    s.fx.engine
        .rename_domain(
            "eng",
            "platform",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let keeper = Session::login(s.addr, "keeper").await;
    let (status, body) = keeper.get("/api/v1/domains/platform/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        listed(&body),
        vec![
            ("reflection".to_string(), PAGE.to_string()),
            ("engram".to_string(), ENGRAM.to_string()),
        ]
    );
    let root = s.fx.tmp.path().join("state/kept");
    assert!(!root.join("eng").exists());
    assert!(root.join("platform").is_dir());
}

/// Kept texts left under a name no registered domain owns (a removal whose
/// set-aside failed, or one from before removals set them aside) are set
/// aside when a domain takes the name, by a registration or as a rename's
/// target; the domain never reaches them.
#[tokio::test]
async fn a_name_taken_later_never_reaches_texts_left_under_it() {
    let s = serve_opts(Opts {
        anonymous: false,
        private: false,
        origin: false,
    })
    .await;
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 8, 0, 0).unwrap();
    for name in ["fresh", "platform"] {
        kept::write(
            &root,
            name,
            None,
            KeptKind::Engram,
            "old.md",
            at,
            "left over\n",
        )
        .unwrap();
    }
    let engine = &s.fx.engine;
    engine.domain_add_virtual("fresh").await.unwrap();
    engine
        .rename_domain(
            "eng",
            "platform",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let keeper = Session::login(s.addr, "keeper").await;
    let (_, body) = keeper.get("/api/v1/domains/fresh/kept").await;
    assert!(listed(&body).is_empty(), "{body}");
    // `eng` kept nothing, so only the check at the rename's start can set
    // the left-over folder aside (the rename step sets `kept/<new>` aside
    // only when the domain has kept texts of its own to move there).
    let (_, body) = keeper.get("/api/v1/domains/platform/kept").await;
    assert!(listed(&body).is_empty(), "{body}");
    let removed: Vec<(String, String)> = kept::scan(&root)
        .into_iter()
        .filter(|k| k.removed)
        .map(|k| (k.domain, k.path))
        .collect();
    assert_eq!(
        removed,
        vec![
            ("fresh".to_string(), "old.md".to_string()),
            ("platform".to_string(), "old.md".to_string()),
        ]
    );
}

/// At start (the opener's step, after any rename journal): a `kept/<name>`
/// folder no registered domain owns is set aside, a registered domain's
/// own stays. And a domain that reaches the configuration file by hand
/// after start has its name's left-over texts set aside the first time this
/// engine meets it, before it is served.
#[tokio::test]
async fn start_and_a_hand_added_domain_set_left_over_texts_aside() {
    let s = serve_opts(Opts {
        anonymous: false,
        private: false,
        origin: false,
    })
    .await;
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 8, 0, 0).unwrap();
    for name in ["gone", "eng"] {
        kept::write(&root, name, None, KeptKind::Engram, "a.md", at, "text\\n").unwrap();
    }
    s.fx.engine.finish_leftover_rename().await;
    let state: Vec<(String, bool)> = kept::scan(&root)
        .into_iter()
        .map(|k| (k.domain, k.removed))
        .collect();
    assert_eq!(
        state,
        vec![("eng".to_string(), false), ("gone".to_string(), true)]
    );

    kept::write(&root, "fresh", None, KeptKind::Engram, "a.md", at, "old\\n").unwrap();
    let config = s.fx.tmp.path().join("config.yaml");
    let mut cfg: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(&config).unwrap();
    cfg.domains.insert(
        "fresh".to_string(),
        crystalline_core::config::DomainEntry::virtual_domain(),
    );
    crystalline_core::config::save_yaml(&config, &cfg).unwrap();
    let listed =
        s.fx.engine
            .kept_texts("fresh", &crystalline_service::Scope::Unrestricted)
            .await
            .unwrap();
    assert!(listed.is_empty(), "{listed:?}");
    assert!(
        kept::scan(&root)
            .iter()
            .any(|k| k.domain == "fresh" && k.removed)
    );
}

/// A domain added to the configuration by hand under a name that differs
/// only in letter case from a registered one never sets that domain's kept
/// texts aside: on a disk that ignores case they share one folder.
#[tokio::test]
async fn a_hand_added_case_twin_leaves_the_registered_domains_texts() {
    let s = serve_opts(Opts {
        anonymous: false,
        private: false,
        origin: false,
    })
    .await;
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 8, 0, 0).unwrap();
    kept::write(&root, "eng", None, KeptKind::Engram, "a.md", at, "eng's\\n").unwrap();
    // The twin's own folder name, so the check runs on a disk that tells
    // case apart too.
    kept::write(
        &root,
        "ENG",
        None,
        KeptKind::Engram,
        "b.md",
        at,
        "eng's too\\n",
    )
    .unwrap();
    let config = s.fx.tmp.path().join("config.yaml");
    let mut cfg: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(&config).unwrap();
    cfg.domains.insert(
        "ENG".to_string(),
        crystalline_core::config::DomainEntry::virtual_domain(),
    );
    crystalline_core::config::save_yaml(&config, &cfg).unwrap();
    let _ =
        s.fx.engine
            .kept_texts("ENG", &crystalline_service::Scope::Unrestricted)
            .await;
    assert!(
        kept::scan(&root).iter().all(|k| !k.removed),
        "{:?}",
        kept::scan(&root)
    );
}

/// A kept comment goes to its sender, matched on the account recorded
/// beside it, and to people who edit the Reflection; another reader gets
/// nothing. A kept comment with no recorded sender is for editors only.
#[tokio::test]
async fn a_kept_comment_goes_to_its_sender_and_to_editors() {
    let s = serve().await;
    s.auth
        .add_user("rita", "rita", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    s.auth
        .upsert_domain_member("eng", "rita", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap();
    let root = s.fx.tmp.path().join("state").join(kept::KEPT_DIR);
    let at = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 8, 0, 0).unwrap();
    let hers = kept::write(
        &root,
        "eng",
        None,
        KeptKind::Comment,
        PAGE,
        at,
        "Vera asks.\n",
    )
    .unwrap();
    kept::record_sender(
        &hers,
        &kept::Sender {
            account: "vera".to_string(),
            // A display name proves nothing: rights match the account.
            name: "rita".to_string(),
        },
    )
    .unwrap();
    let later = at + chrono::Duration::minutes(1);
    kept::write(
        &root,
        "eng",
        None,
        KeptKind::Comment,
        PAGE,
        later,
        "Nobody's.\n",
    )
    .unwrap();
    let found = kept::scan(&root);
    let id_of = |text: &str| {
        found
            .iter()
            .find(|k| std::fs::read_to_string(&k.file).unwrap() == text)
            .unwrap()
            .id
            .clone()
    };
    let (hers, nobodys) = (id_of("Vera asks.\n"), id_of("Nobody's.\n"));

    let vera = Session::login(s.addr, "vera").await;
    let (status, body) = vera.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        listed(&body),
        vec![("comment".to_string(), PAGE.to_string())]
    );
    assert_eq!(body["kept"][0]["id"], hers.as_str());
    assert_eq!(body["kept"][0]["sender"], "rita");
    // The account the rights matched, beside the display name that proves
    // nothing: a client tells its own comments apart by this.
    assert_eq!(body["kept"][0]["sender_account"], "vera");
    let (status, one) = vera.get(&format!("/api/v1/domains/eng/kept/{hers}")).await;
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["sender_account"], "vera");
    let (status, _) = vera
        .get(&format!("/api/v1/domains/eng/kept/{nobodys}"))
        .await;
    assert_eq!(status, 404, "no recorded sender: editors only");

    let rita = Session::login(s.addr, "rita").await;
    let (status, body) = rita.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["kept"],
        serde_json::json!([]),
        "another reader gets nothing"
    );
    let (status, _) = rita.get(&format!("/api/v1/domains/eng/kept/{hers}")).await;
    assert_eq!(status, 404);

    let ed = Session::login(s.addr, "ed").await;
    let (status, body) = ed.get("/api/v1/domains/eng/kept").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["kept"].as_array().unwrap().len(),
        2,
        "an editor sees both: {body}"
    );
    let accounts: Vec<&Value> = body["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| &k["sender_account"])
        .collect();
    assert_eq!(
        accounts,
        vec![&Value::from("vera"), &Value::Null],
        "the sender's account on hers, none on the one with no sender: {body}"
    );

    let (status, _) = vera
        .send(
            Method::DELETE,
            &format!("/api/v1/domains/eng/kept/{hers}"),
            None,
        )
        .await;
    assert_eq!(status, 204, "the sender drops her own");
    assert_eq!(kept::scan(&root).len(), 1);
}
