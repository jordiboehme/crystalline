//! A Reflection page never carries text drawn from a domain fewer people may
//! read than the page's own: every write that names such a source is
//! refused, on every surface and for the machine owner too, and a page that
//! carries one anyway (written before the rule, edited on disk, or left
//! behind when the source's readers narrowed) is named in the queue as
//! `source_too_wide`.

use std::collections::BTreeMap;
use std::sync::Arc;

use crystalline_service::params::{ReflectParams, WriteReflectionParams};
use crystalline_service::rest::{AuthStore, MemberLevel, ReflectionLevel, Role};
use crystalline_service::{DomainAccess, ReflectionCaller, Scope};
use serde_json::{Value, json};

use crate::docs::{agent, write};
use crate::fixture::Fx;

/// The address of an engram in `harbor`.
const TEAM: &str = "crystalline://harbor/team/harbor-team";
/// A second engram in `harbor`.
const ROTA: &str = "crystalline://harbor/team/rota";

/// What a write that cites a domain fewer people may read is refused with.
const TOO_WIDE: &str = "this source is in a domain fewer people may read than this page's domain; leave out the source and every sentence drawn from it, since the check sees only named sources, or write that text on a page in that domain instead";

const OVERVIEW: &str = "# Overview\n\nFor people.\n\n## Team\n\nFive people.\n";

/// `eng` (Reflection on, shared) beside `harbor` (shared), with keeper, ann
/// and sam as instance editors, and no resolver installed yet.
async fn accounts() -> (Fx, Arc<AuthStore>) {
    let fx = crate::sources::crossing().await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["keeper", "ann", "sam"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    (fx, auth)
}

/// [`accounts`] with the resolver installed.
async fn reach() -> (Fx, Arc<AuthStore>) {
    let (fx, auth) = accounts().await;
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    (fx, auth)
}

/// Make `domain` private to keeper, with `members` as editors (who curate
/// its Reflection).
async fn private(auth: &AuthStore, domain: &str, members: &[&str]) {
    auth.set_domain_visibility(domain, true, "keeper")
        .await
        .unwrap();
    for member in members {
        auth.upsert_domain_member(domain, member, MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
    }
}

fn user(name: &str) -> Scope {
    Scope::User {
        account: name.into(),
        admin: false,
    }
}

fn agent_as(name: &str) -> ReflectionCaller {
    ReflectionCaller::agent(user(name), "claude-code", false)
}

/// `overview.md` in `eng` with its `Team` section built from `sources`.
fn cite(sources: &[&str]) -> WriteReflectionParams {
    let mut p = write("overview.md", OVERVIEW);
    p.sources = BTreeMap::from([(
        "Team".to_string(),
        sources.iter().map(|s| s.to_string()).collect(),
    )]);
    p
}

fn page_exists(fx: &Fx) -> bool {
    fx.root.join("reflections/overview.md").exists()
}

/// The `source_too_wide` items of `eng`'s queue as `caller` gets it, and
/// the whole queue.
async fn too_wide(fx: &Fx, caller: &ReflectionCaller) -> (Vec<Value>, Value) {
    let queue = fx
        .engine
        .reflect(
            &ReflectParams {
                domains: vec!["eng".into()],
                limit: Some(100),
                ..Default::default()
            },
            caller,
        )
        .await
        .unwrap();
    let items = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == "source_too_wide")
        .cloned()
        .collect();
    (items, queue)
}

/// The sources the file of `path` stores, over all its sections.
fn stored_domains(fx: &Fx, path: &str) -> Vec<Option<String>> {
    let text = std::fs::read_to_string(fx.root.join("reflections").join(path)).unwrap();
    crystalline_core::reflection::read_provenance(&text)
        .into_values()
        .flat_map(|record| record.sources.into_iter().map(|src| src.domain))
        .collect()
}

#[tokio::test]
async fn a_shared_page_cannot_cite_a_private_domain() {
    let (fx, auth) = reach().await;
    private(&auth, "harbor", &["sam"]).await;
    // sam may read harbor, and still may not carry it onto a page everyone reads.
    let err = fx
        .engine
        .write_reflection(&cite(&[TEAM]), &agent_as("sam"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains(TOO_WIDE), "{err}");
    assert!(!page_exists(&fx), "nothing written");
}

#[tokio::test]
async fn a_private_page_can_cite_a_shared_domain() {
    let (fx, auth) = reach().await;
    private(&auth, "eng", &["sam"]).await;
    fx.engine
        .write_reflection(&cite(&[TEAM]), &agent_as("sam"))
        .await
        .unwrap();
    assert_eq!(
        stored_domains(&fx, "overview.md"),
        vec![Some("harbor".to_string())]
    );
}

#[tokio::test]
async fn a_private_page_can_cite_a_private_domain_with_more_readers() {
    let (fx, auth) = reach().await;
    private(&auth, "eng", &["sam"]).await;
    private(&auth, "harbor", &["sam", "ann"]).await;
    fx.engine
        .write_reflection(&cite(&[TEAM]), &agent_as("sam"))
        .await
        .unwrap();
    assert_eq!(
        stored_domains(&fx, "overview.md"),
        vec![Some("harbor".to_string())]
    );
}

#[tokio::test]
async fn a_private_page_cannot_cite_one_with_fewer_readers() {
    let (fx, auth) = reach().await;
    private(&auth, "eng", &["sam", "ann"]).await;
    private(&auth, "harbor", &["sam"]).await;
    let err = fx
        .engine
        .write_reflection(&cite(&[TEAM]), &agent_as("sam"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains(TOO_WIDE), "{err}");
    assert!(!page_exists(&fx), "nothing written");

    // A member who reads the Reflection only is a reader of the page too.
    let (fx, auth) = reach().await;
    private(&auth, "eng", &["sam"]).await;
    private(&auth, "harbor", &["sam"]).await;
    auth.upsert_domain_member(
        "eng",
        "ann",
        MemberLevel::None,
        Some(ReflectionLevel::Reader),
        "keeper",
    )
    .await
    .unwrap();
    let err = fx
        .engine
        .write_reflection(&cite(&[TEAM]), &agent_as("sam"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains(TOO_WIDE), "{err}");
}

/// A shared domain is public to the anonymous viewer, which never reads a
/// private one: even when the only account reads both domains, a shared
/// page may not cite a private one.
#[tokio::test]
async fn a_public_page_cannot_cite_a_domain_anonymous_cannot_read() {
    let fx = crate::sources::crossing().await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("keeper", "keeper", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    private(&auth, "harbor", &[]).await;
    let err = fx
        .engine
        .write_reflection(&cite(&[TEAM]), &agent_as("keeper"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains(TOO_WIDE), "{err}");
}

/// A stack that could not open its accounts store knows no reader: it
/// refuses every source outside the page's own domain, a shared one
/// included, says why, and names every such source already on a page in
/// the queue. A source of the page's own domain still goes. (The real
/// stacks that serve the machine owner on stdio are run end to end in the
/// CLI suite, `team::reflection_reach`.)
#[tokio::test]
async fn without_the_accounts_store_every_other_domain_is_refused() {
    let (fx, _) = accounts().await;
    fx.engine
        .write_reflection(&cite(&[TEAM]), &agent())
        .await
        .unwrap();
    fx.engine
        .set_domain_access_unavailable("the file is locked".to_string());
    let err = fx
        .engine
        .write_reflection(&cite(&[TEAM]), &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("the accounts store could not be read"),
        "{err}"
    );
    assert!(err.contains("the file is locked"), "{err}");
    assert!(err.contains("the text drawn from it"), "{err}");
    fx.engine
        .write_reflection(&cite(&["runbooks/deploy"]), &agent())
        .await
        .unwrap();
    let mut back = write("other.md", OVERVIEW);
    back.sources = BTreeMap::from([("Team".to_string(), vec![TEAM.to_string()])]);
    assert!(fx.engine.write_reflection(&back, &agent()).await.is_err());

    let mut kept = write("kept.md", OVERVIEW);
    kept.sources = BTreeMap::from([("Team".to_string(), vec!["runbooks/deploy".to_string()])]);
    fx.engine.write_reflection(&kept, &agent()).await.unwrap();
    // The page written before the store went missing is named.
    std::fs::write(
        fx.root.join("reflections/forged.md"),
        "---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: harbor\n      engram: team/harbor-team\n      checksum: abc\n---\n\n# Forged\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n",
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let (items, queue) = too_wide(&fx, &agent()).await;
    assert_eq!(items.len(), 1, "{queue}");
    assert_eq!(items[0]["path"], "forged.md", "{queue}");
}

/// A stack that found no accounts store at its start knows there is no
/// account and no private domain; should the store appear while it runs,
/// it no longer knows its readers and refuses other domains' sources.
#[tokio::test]
async fn an_accounts_store_that_appears_later_closes_the_check() {
    let fx = crate::sources::crossing().await;
    let path = fx.tmp.path().join("later-auth.db");
    fx.engine.set_domain_access_absent(path.clone());
    fx.engine
        .write_reflection(&cite(&[TEAM]), &agent())
        .await
        .unwrap();
    std::fs::write(&path, b"").unwrap();
    let err = fx
        .engine
        .write_reflection(&cite(&[TEAM]), &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("restart it"), "{err}");
}

/// A writer who may not open the source's domain is told what a domain
/// that does not exist gets: never that it reaches fewer people.
#[tokio::test]
async fn the_refusal_names_no_hidden_domain() {
    let (fx, auth) = reach().await;
    private(&auth, "harbor", &["sam"]).await;
    let err = fx
        .engine
        .write_reflection(&cite(&[TEAM]), &agent_as("ann"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("domain 'harbor' not registered"), "{err}");
    assert!(!err.contains("fewer"), "{err}");
    let nowhere = fx
        .engine
        .write_reflection(
            &cite(&["crystalline://nowhere/team/nowhere-team"]),
            &agent_as("ann"),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(err.replace("harbor", "nowhere"), nowhere);
}

#[tokio::test]
async fn own_domain_sources_are_always_allowed() {
    let (fx, auth) = reach().await;
    private(&auth, "eng", &[]).await;
    auth.upsert_domain_member(
        "eng",
        "ann",
        MemberLevel::None,
        Some(ReflectionLevel::Reader),
        "keeper",
    )
    .await
    .unwrap();
    fx.engine
        .write_reflection(
            &cite(&["runbooks/deploy", "crystalline://eng/runbooks/deploy"]),
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(stored_domains(&fx, "overview.md"), vec![None]);
    let (items, queue) = too_wide(&fx, &agent()).await;
    assert!(items.is_empty(), "{queue}");
}

/// A source domain whose readers narrow after the page cited it: the queue
/// names the section, with the source, to a curator who may open that
/// domain, and tells one who may not nothing. Dropping the source settles
/// it.
#[tokio::test]
async fn narrowing_a_source_domain_raises_source_too_wide() {
    let (fx, auth) = reach().await;
    fx.engine
        .write_reflection(&cite(&[TEAM]), &agent())
        .await
        .unwrap();
    let (items, queue) = too_wide(&fx, &agent()).await;
    assert!(items.is_empty(), "both shared: {queue}");

    private(&auth, "harbor", &["sam"]).await;
    let (items, queue) = too_wide(&fx, &agent()).await;
    assert_eq!(items.len(), 1, "{queue}");
    let item = &items[0];
    assert_eq!(item["kind"], "update", "{item}");
    assert_eq!(item["path"], "overview.md", "{item}");
    assert_eq!(item["heading"], "Team", "{item}");
    assert_eq!(item["sources"][0]["domain"], "harbor", "{item}");
    let guidance = queue["guidance"].as_str().unwrap();
    assert!(
        guidance.contains("source_too_wide") && guidance.contains("text drawn from it"),
        "{guidance}"
    );
    let note = item["note"].as_str().unwrap();
    assert!(note.contains("fewer people may read"), "{note}");
    assert!(note.contains("remove that source"), "{note}");
    assert!(note.contains("until"), "says the page stays: {note}");

    let (sam, _) = too_wide(&fx, &agent_as("sam")).await;
    assert_eq!(sam.len(), 1);
    assert_eq!(sam[0]["sources"][0]["domain"], "harbor");

    // ann may not open harbor: told nothing, so a hidden source in a
    // registered domain looks like one in a name nothing resolves.
    let (ann, queue) = too_wide(&fx, &agent_as("ann")).await;
    assert!(ann.is_empty(), "{queue}");
    assert!(!queue.to_string().contains("harbor"), "{queue}");

    // The source left out: the item is gone.
    fx.engine
        .write_reflection(&cite(&["runbooks/deploy"]), &agent())
        .await
        .unwrap();
    let (items, queue) = too_wide(&fx, &agent()).await;
    assert!(items.is_empty(), "{queue}");
}

/// A page written before the rule (here, before a resolver knew the
/// domains' readers) is named once: one item for its section, however many
/// of its sources are too wide, on every call until it is settled.
#[tokio::test]
async fn a_page_written_before_the_rule_surfaces_once_in_the_queue() {
    let (fx, auth) = accounts().await;
    private(&auth, "harbor", &["sam"]).await;
    fx.engine
        .write_reflection(&cite(&[TEAM, ROTA, "runbooks/deploy"]), &agent())
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    let (first, queue) = too_wide(&fx, &agent()).await;
    assert_eq!(first.len(), 1, "{queue}");
    assert_eq!(first[0]["sources"].as_array().unwrap().len(), 2, "{queue}");
    let (second, _) = too_wide(&fx, &agent()).await;
    assert_eq!(first, second, "the same item, not a new one");
}

/// A `reflection:` block edited on disk cannot be refused: the queue names
/// it instead.
#[tokio::test]
async fn a_file_edited_on_disk_with_a_too_wide_source_raises_the_item() {
    let (fx, auth) = reach().await;
    private(&auth, "harbor", &["sam"]).await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(
        fx.root.join("reflections/forged.md"),
        "---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: harbor\n      engram: team/harbor-team\n      checksum: abc\n---\n\n# Forged\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n",
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let (items, queue) = too_wide(&fx, &agent()).await;
    assert_eq!(items.len(), 1, "{queue}");
    assert_eq!(items[0]["path"], "forged.md", "{queue}");
    assert_eq!(items[0]["section"], "aaaa1111", "{queue}");
}

/// The REST page save (what Fluid sends) and the live editor's save take
/// no sources: a `reflection:` block made up in the text naming a source
/// is not stored, so neither can carry one onto a page.
#[tokio::test]
async fn the_page_save_and_the_live_editor_cannot_set_a_source() {
    let fx = crate::sources::crossing().await;
    fx.engine
        .write_reflection(&write("live.md", OVERVIEW), &agent())
        .await
        .unwrap();
    let s = crate::rest::serve_fx(fx, false).await;
    private(&s.auth, "harbor", &["ed"]).await;
    let room =
        s.fx.engine
            .reflection_room_text("eng", "live.md")
            .await
            .unwrap();
    let id = crystalline_core::reflection::split_sections(&room.content)
        .into_iter()
        .find(|section| section.heading == "Team")
        .and_then(|section| section.id)
        .unwrap();
    let forged = |body: &str| {
        format!(
            "---\nreflection:\n  {id}:\n    sources:\n    - domain: harbor\n      engram: team/harbor-team\n      checksum: abc\n---\n\n{body}"
        )
    };

    let ed = crate::rest::Session::login(s.addr, "ed").await;
    let (status, body) = ed
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/live.md",
            Some(json!({ "markdown": forged(&room.content) })),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(stored_domains(&s.fx, "live.md").is_empty());

    let room =
        s.fx.engine
            .reflection_room_text("eng", "live.md")
            .await
            .unwrap();
    s.fx.engine
        .save_reflection_room(
            "eng",
            "live.md",
            &forged(&room.content.replace("Five people.", "Six people.")),
            &room.checksum,
            &crystalline_service::SectionAuthors::plain(vec!["ed".into()]),
            true,
        )
        .await
        .unwrap();
    let text = std::fs::read_to_string(s.fx.root.join("reflections/live.md")).unwrap();
    assert!(text.contains("Six people."), "{text}");
    assert!(stored_domains(&s.fx, "live.md").is_empty());
    let (items, queue) = too_wide(&s.fx, &agent()).await;
    assert!(items.is_empty(), "{queue}");
}
