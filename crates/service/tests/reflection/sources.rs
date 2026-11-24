//! `sources` on a reflection write, keyed by section id or heading: a
//! heading this same write creates is found once its id is minted, a key
//! that names no single `##` section is refused with the sections the page
//! has, and nothing is written.

use std::collections::BTreeMap;

use crate::docs::{agent, reflecting, write};
use crate::fixture::Fx;

fn sourced(
    path: &str,
    markdown: &str,
    keys: &[&str],
) -> crystalline_service::params::WriteReflectionParams {
    let mut p = write(path, markdown);
    p.sources = keys
        .iter()
        .map(|k| (k.to_string(), vec!["runbooks/deploy".to_string()]))
        .collect::<BTreeMap<_, _>>();
    p
}

async fn sources_of(fx: &Fx, path: &str) -> Vec<(String, String)> {
    let doc = fx
        .engine
        .read_reflection("eng", path, &agent())
        .await
        .unwrap();
    doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| {
            let source = s["sources"][0]["permalink"].as_str()?;
            Some((s["heading"].as_str()?.to_string(), source.to_string()))
        })
        .collect()
}

fn exists(fx: &Fx, path: &str) -> bool {
    fx.root.join("reflections").join(path).exists()
}

#[tokio::test]
async fn a_create_keys_sources_by_the_headings_it_creates() {
    let fx = reflecting().await;
    let page = "# Deploying\n\nFor new people.\n\n## Night deploys\n\nAt night.\n\n## Rollback\n\nRoll back.\n";
    let written = fx
        .engine
        .write_reflection(
            &sourced("a.md", page, &["Night deploys", "## Rollback"]),
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(written["curated"], true, "{written}");
    assert_eq!(
        sources_of(&fx, "a.md").await,
        vec![
            ("Night deploys".to_string(), "runbooks/deploy".to_string()),
            ("Rollback".to_string(), "runbooks/deploy".to_string()),
        ],
        "a key written with its ## marks finds the section too"
    );
}

#[tokio::test]
async fn an_update_keys_sources_by_the_heading_of_a_section_it_adds() {
    let fx = reflecting().await;
    let page = "# Deploying\n\n## Night deploys\n\nAt night.\n";
    fx.engine
        .write_reflection(&write("a.md", page), &agent())
        .await
        .unwrap();
    let stored = std::fs::read_to_string(fx.root.join("reflections/a.md")).unwrap();
    let grown = format!("{}\n## Rollback\n\nRoll back.\n", stored.trim_end());
    let written = fx
        .engine
        .write_reflection(&sourced("a.md", &grown, &["Rollback"]), &agent())
        .await
        .unwrap();
    assert_eq!(written["ids_assigned"].as_array().unwrap().len(), 1);
    assert_eq!(
        sources_of(&fx, "a.md").await,
        vec![("Rollback".to_string(), "runbooks/deploy".to_string())]
    );
}

#[tokio::test]
async fn a_heading_two_sections_share_is_refused_with_both_headings_and_the_way_out() {
    let fx = reflecting().await;
    let page = "# Deploying\n\n## Example\n\nA.\n\n## Rollback\n\nB.\n\n## Example\n\nC.\n";
    let err = fx
        .engine
        .write_reflection(&sourced("a.md", page, &["Example"]), &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("'Example' names 2 sections in 'a.md' ('Example', 'Example')"),
        "{err}"
    );
    assert!(err.contains("write it again with sources"), "{err}");
    assert!(err.contains("ids_assigned"), "asks for the ids: {err}");
    assert!(!exists(&fx, "a.md"), "nothing written");
}

#[tokio::test]
async fn a_key_that_names_the_title_is_refused_with_the_sections_the_page_has() {
    let fx = reflecting().await;
    let page = "# Removing a tenant\n\nFor good.\n\n## Before you purge\n\nExport.\n\n## The purge\n\nGone.\n";
    let err = fx
        .engine
        .write_reflection(&sourced("a.md", page, &["Removing a tenant"]), &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("no section 'Removing a tenant' in 'a.md' to set sources on"),
        "{err}"
    );
    assert!(err.contains("'Before you purge', 'The purge'"), "{err}");
    assert!(
        err.contains("not sections"),
        "says why the title is not: {err}"
    );
    assert!(!exists(&fx, "a.md"), "nothing written");
}

/// The address of an engram in the second domain `harbor`.
const TEAM: &str = "crystalline://harbor/team/harbor-team";

/// `eng` with Reflection on and its deploy runbook, beside `harbor` with
/// three engrams a page of `eng` can cite.
pub async fn crossing() -> Fx {
    crate::fixture::beside(
        &[(
            "runbooks/deploy.md",
            &crate::fixture::engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        )],
        "harbor",
        &[
            (
                "team/harbor-team.md",
                &crate::fixture::engram("Harbor team", "team/harbor-team", "- [fact] five people"),
            ),
            (
                "team/rota.md",
                &crate::fixture::engram("Rota", "team/rota", "- [fact] weekly"),
            ),
            (
                "team/pager.md",
                &crate::fixture::engram("Pager", "team/pager", "- [fact] one pager"),
            ),
        ],
    )
    .await
}

fn sources_by_heading(doc: &serde_json::Value, heading: &str) -> serde_json::Value {
    doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["heading"] == heading)
        .unwrap()["sources"]
        .clone()
}

/// A source in another domain is named by its address, keyed by heading or
/// by id. It is stored with the domain's name and read in its own domain;
/// an address of the page's own domain is the bare permalink, and one
/// engram named both ways is one source.
#[tokio::test]
async fn a_source_in_another_domain_is_named_by_its_address() {
    let fx = crossing().await;
    let mut p = write(
        "overview.md",
        "# Overview\n\nFor people.\n\n## Team\n\nFive people.\n\n## Deploys\n\nAt night.\n",
    );
    p.sources = BTreeMap::from([
        ("Team".to_string(), vec![TEAM.to_string()]),
        (
            "Deploys".to_string(),
            vec![
                "runbooks/deploy".to_string(),
                "crystalline://eng/runbooks/deploy".to_string(),
            ],
        ),
    ]);
    let written = fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert_eq!(written["curated"], true, "{written}");
    let doc = fx
        .engine
        .read_reflection("eng", "overview.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        sources_by_heading(&doc, "Team"),
        serde_json::json!([{ "permalink": "team/harbor-team", "domain": "harbor",
            "title": "Harbor team", "state": "current", "status": "stable" }]),
        "{doc}"
    );
    assert_eq!(
        sources_by_heading(&doc, "Deploys"),
        serde_json::json!([{ "permalink": "runbooks/deploy", "title": "Deploy",
            "state": "current", "status": "stable" }]),
        "{doc}"
    );
    let stored = std::fs::read_to_string(fx.root.join("reflections/overview.md")).unwrap();
    assert_eq!(stored.matches("domain: harbor").count(), 1, "{stored}");
    assert!(!stored.contains("domain: eng"), "{stored}");

    // By id: the section's sources are replaced, again by address.
    let team_id = doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["heading"] == "Team")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut by_id = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "overview.md".into(),
        ..Default::default()
    };
    by_id.sources = BTreeMap::from([(team_id, vec!["crystalline://harbor/team/rota".to_string()])]);
    fx.engine.write_reflection(&by_id, &agent()).await.unwrap();
    let doc = fx
        .engine
        .read_reflection("eng", "overview.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        sources_by_heading(&doc, "Team")[0]["permalink"],
        "team/rota",
        "{doc}"
    );
}

/// An engram the other domain lacks is refused by its address, and an
/// address that names no single engram is refused with what a source is.
/// Nothing is written either way.
#[tokio::test]
async fn a_source_another_domain_lacks_is_refused_by_its_address() {
    let fx = crossing().await;
    for (source, words) in [
        (
            "crystalline://harbor/team/nobody",
            "no engram 'crystalline://harbor/team/nobody' to name as a source",
        ),
        ("crystalline://harbor/team/*", "a source is one engram"),
        ("crystalline://harbor", "a source is one engram"),
    ] {
        let mut p = write("b.md", "# B\n\nFor people.\n\n## Only\n\nText.\n");
        p.sources = BTreeMap::from([("Only".to_string(), vec![source.to_string()])]);
        let err = fx
            .engine
            .write_reflection(&p, &agent())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(words), "{source}: {err}");
        assert!(!exists(&fx, "b.md"), "{source}");
    }
}

/// A domain whose MANIFEST names it differently from its local name: the
/// record stores the canonical name, and every answer names the domain by
/// the local name a reader reaches it by, so a link into it lands on the
/// engram.
#[tokio::test]
async fn a_source_is_shown_by_the_local_name_its_domain_is_reached_by() {
    let fx = crossing().await;
    fx.engine
        .edit_engram(&crystalline_service::params::EditParams {
            identifier: "manifest".to_string(),
            domain: "harbor".to_string(),
            operation: "find_replace".to_string(),
            find_text: Some("domain_name: harbor\n".to_string()),
            content: Some("domain_name: crew\n".to_string()),
            expected_replacements: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    fx.engine.sync(None).await.unwrap();
    let mut p = write(
        "overview.md",
        "# Overview\n\nFor people.\n\n## Team\n\nFive.\n",
    );
    p.sources = BTreeMap::from([(
        "Team".to_string(),
        vec!["crystalline://crew/team/harbor-team".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let stored = std::fs::read_to_string(fx.root.join("reflections/overview.md")).unwrap();
    assert!(
        stored.contains("domain: crew"),
        "the canonical name is stored: {stored}"
    );
    let doc = fx
        .engine
        .read_reflection("eng", "overview.md", &agent())
        .await
        .unwrap();
    let source = &sources_by_heading(&doc, "Team")[0];
    assert_eq!(source["domain"], "harbor", "{doc}");
    // The link a reader follows: the engram by the name shown.
    let engram = fx
        .engine
        .read_engram(
            &crystalline_service::params::ReadParams {
                identifier: source["permalink"].as_str().unwrap().to_string(),
                domain: Some(source["domain"].as_str().unwrap().to_string()),
                ..Default::default()
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(engram["title"], "Harbor team", "{engram}");
}

/// What carries a page beyond its readers. The admin's domain archive holds
/// the page as stored, the other domain's name included (the docs say so).
/// A search hit and the change events carry no frontmatter, so no source
/// and no other domain's name.
#[tokio::test]
async fn the_archive_keeps_the_names_and_search_and_events_carry_none() {
    let fx = crossing().await;
    let mut rx = fx.engine.changes().subscribe();
    let mut p = write(
        "overview.md",
        "# Overview\n\nFor people.\n\n## Team\n\nFive people on call.\n",
    );
    p.sources = BTreeMap::from([("Team".to_string(), vec![TEAM.to_string()])]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();

    let files = fx.engine.domain_files("eng").await.unwrap();
    let page = files
        .iter()
        .find(|(path, _)| path == "reflections/overview.md")
        .expect("the archive carries the page");
    assert!(
        String::from_utf8_lossy(&page.1).contains("domain: harbor"),
        "as stored"
    );

    for query in ["harbor", "team/harbor-team", "people"] {
        let hits = fx
            .engine
            .search_reflection(
                "eng",
                &crystalline_service::engine::ReflectionSearch {
                    query,
                    search_type: Some("text"),
                    limit: 10,
                    page: 1,
                },
                &agent(),
            )
            .await
            .unwrap();
        // The answer repeats the query; the hits are what the index holds.
        let text = hits["hits"].to_string();
        assert!(!text.contains("harbor"), "{query}: {hits}");
        assert!(!text.contains("reflection:"), "{query}: {hits}");
        if query == "people" {
            assert!(hits["total"].as_u64().unwrap() > 0, "{hits}");
        }
    }

    let mut seen = 0;
    while let Ok(envelope) = rx.try_recv() {
        let text = format!("{envelope:?}");
        assert!(!text.contains("harbor"), "{text}");
        seen += 1;
    }
    assert!(seen > 0, "the write was announced");
}

/// On a shared database another instance may register a domain this one
/// does not: its rows are in the index, but the Reflection's host checks
/// sources by its own registrations, so they read unavailable, never
/// missing or current.
#[tokio::test]
async fn a_domain_only_another_instance_registers_is_unavailable_here() {
    use std::sync::Arc;
    let store: Arc<tokio::sync::Mutex<dyn crystalline_index::Store>> =
        Arc::new(tokio::sync::Mutex::new(
            crystalline_index::TursoStore::open_in_memory()
                .await
                .unwrap(),
        ));
    // The other instance registers harbor and indexes it into the store.
    let team = crate::fixture::engram("Harbor team", "team/harbor-team", "- [fact] five");
    let other = crate::fixture::fixture_on(
        store.clone(),
        &[],
        |cfg, base| {
            let root = base.join("harbor");
            std::fs::create_dir_all(root.join("team")).unwrap();
            std::fs::write(
                root.join("MANIFEST.md"),
                crystalline_core::manifest_template("harbor", "2026-01-01"),
            )
            .unwrap();
            std::fs::write(root.join("team/harbor-team.md"), &team).unwrap();
            cfg.domains.insert(
                "harbor".into(),
                crystalline_core::config::DomainEntry::file(root),
            );
        },
        None,
        None,
        None,
    )
    .await;
    // This instance serves eng's Reflection and registers no harbor. Its
    // page names a harbor source by hand, with the indexed checksum.
    let host = crate::fixture::fixture_on(
        store.clone(),
        &[("MANIFEST.md", &crate::fixture::manifest("reflection: on\n"))],
        |_, _| {},
        None,
        None,
        None,
    )
    .await;
    let checksum = crystalline_core::provision::receipt::sha256_hex(
        std::fs::read(other.tmp.path().join("harbor/team/harbor-team.md"))
            .unwrap()
            .as_slice(),
    );
    std::fs::create_dir_all(host.root.join("reflections")).unwrap();
    std::fs::write(
        host.root.join("reflections/overview.md"),
        format!(
            "---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: harbor\n      engram: team/harbor-team\n      checksum: {checksum}\n---\n\n# Overview\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n"
        ),
    )
    .unwrap();
    host.engine.sync(None).await.unwrap();
    let doc = host
        .engine
        .read_reflection("eng", "overview.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        sources_by_heading(&doc, "Team")[0]["state"],
        "unavailable",
        "{doc}"
    );
}

/// A source in a domain that resolves here but cannot be checked (here a
/// connected server serves the local `harbor` in its place) is named in
/// the queue by the same local name the read view shows, not by the
/// canonical name the page stores.
#[tokio::test]
async fn an_unavailable_source_is_named_in_the_queue_as_on_the_page() {
    use std::sync::Arc;
    let fx = crossing().await;
    fx.engine
        .edit_engram(&crystalline_service::params::EditParams {
            identifier: "manifest".to_string(),
            domain: "harbor".to_string(),
            operation: "find_replace".to_string(),
            find_text: Some("domain_name: harbor\n".to_string()),
            content: Some("domain_name: crew\n".to_string()),
            expected_replacements: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    fx.engine.sync(None).await.unwrap();
    let mut p = write(
        "overview.md",
        "# Overview\n\nFor people.\n\n## Team\n\nFive.\n",
    );
    p.sources = BTreeMap::from([(
        "Team".to_string(),
        vec!["crystalline://crew/team/harbor-team".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();

    let remote = fx.tmp.path().join("remote");
    crystalline_remote::update_sources(&remote, |file| {
        file.upsert(crystalline_remote::SourceRecord {
            url: "http://127.0.0.1:9".to_string(),
            name: "acme".to_string(),
            account: "ada".to_string(),
            kind: crystalline_remote::CredentialKind::Token,
            token_endpoint: None,
            revocation_endpoint: None,
            connected_at: crystalline_core::time::now(),
            mounts: vec![crystalline_remote::MountRecord {
                remote: "harbor".to_string(),
                local: "harbor".to_string(),
            }],
            domains: None,
            from_env: false,
        });
        Ok(())
    })
    .unwrap();
    fx.engine
        .set_sources(Arc::new(crystalline_remote::SourceSet::load(
            remote,
            fx.engine.local_domains(),
            |_| None,
        )));

    let doc = fx
        .engine
        .read_reflection("eng", "overview.md", &agent())
        .await
        .unwrap();
    let source = &sources_by_heading(&doc, "Team")[0];
    assert_eq!(source["state"], "unavailable", "{doc}");
    let shown = source["domain"].as_str().unwrap().to_string();
    let queue = fx
        .engine
        .reflect(
            &crystalline_service::params::ReflectParams {
                domains: vec!["eng".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let item = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["what"] == "source_unavailable")
        .unwrap_or_else(|| panic!("{queue}"));
    assert_eq!(item["source_domain"], shown.as_str(), "{queue}");
    assert!(item["note"].as_str().unwrap().contains(&shown), "{queue}");
}

const TWO_SECTIONS: &str = "# Deploying\n\nFor new people.\n\n## Night deploys\n\nAt night.\n\n## Rollback\n\nRoll back.\n";

#[tokio::test]
async fn a_section_written_without_sources_is_named_in_the_receipt() {
    let fx = reflecting().await;
    let written = fx
        .engine
        .write_reflection(&sourced("a.md", TWO_SECTIONS, &["Night deploys"]), &agent())
        .await
        .unwrap();
    assert_eq!(
        written["unsourced_sections"],
        serde_json::json!(["Rollback"]),
        "{written}"
    );
    assert!(
        written["guidance"]
            .to_string()
            .contains("These sections name no source engrams: pass sources for each section you built from engrams, so the queue can follow them."),
        "{written}"
    );
}

#[tokio::test]
async fn a_sourced_write_has_no_unsourced_sections() {
    let fx = reflecting().await;
    let written = fx
        .engine
        .write_reflection(
            &sourced("a.md", TWO_SECTIONS, &["Night deploys", "Rollback"]),
            &agent(),
        )
        .await
        .unwrap();
    assert!(written.get("unsourced_sections").is_none(), "{written}");
    assert!(
        !written.to_string().contains("name no source engrams"),
        "{written}"
    );
}

#[tokio::test]
async fn a_pact_write_never_reports_unsourced_sections() {
    let fx = reflecting().await;
    let pact = "# How we write\n\n## Audience\n\n- New people.\n\n## Pages\n\n- guides/*.md: a guide, asked by Ada\n";
    let written = fx
        .engine
        .write_reflection(&write("PACT.md", pact), &crate::docs::person())
        .await
        .unwrap();
    assert!(written.get("unsourced_sections").is_none(), "{written}");
}

#[tokio::test]
async fn a_later_write_names_only_the_unsourced_section_it_changed() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let stored = std::fs::read_to_string(fx.root.join("reflections/a.md")).unwrap();
    let written = fx
        .engine
        .write_reflection(
            &write("a.md", &stored.replace("Roll back.", "Roll back fast.")),
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(
        written["unsourced_sections"],
        serde_json::json!(["Rollback"]),
        "{written}"
    );
}

#[tokio::test]
async fn a_sources_only_write_reports_no_unsourced_sections() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let mut p = write("a.md", "");
    p.markdown = None;
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    let written = fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert_eq!(written["curated"], true, "{written}");
    assert!(written.get("unsourced_sections").is_none(), "{written}");
}

#[tokio::test]
async fn a_settle_reports_no_unsourced_sections() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let stored = std::fs::read_to_string(fx.root.join("reflections/a.md")).unwrap();
    std::fs::write(
        fx.root.join("reflections/a.md"),
        stored.replace("Roll back.", "Roll back by hand."),
    )
    .unwrap();
    let id = crystalline_core::reflection::split_sections(
        &std::fs::read_to_string(fx.root.join("reflections/a.md")).unwrap(),
    )[2]
    .id
    .clone()
    .unwrap();
    let settled = fx
        .engine
        .settle_reflection("eng", "a.md", &[id], "kept", &agent())
        .await
        .unwrap();
    assert!(settled.get("unsourced_sections").is_none(), "{settled}");
}

/// **A section's source carries the cited engram's `status` beside its
/// `state`**, so a reader sees that a section rests on a proposal. The status
/// is as the engram writes it; the state stays `current` for a tentative
/// engram, since nothing about the citation changed.
#[tokio::test]
async fn a_source_carries_the_status_of_the_engram_it_cites() {
    let fx = crate::fixture::fixture(&[
        ("MANIFEST.md", &crate::fixture::manifest("reflection: on\n")),
        (
            "runbooks/deploy.md",
            &crate::fixture::engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
        (
            "ideas/kafka.md",
            "---\ntype: engram\ntitle: Kafka\npermalink: ideas/kafka\ntags:\n  - eng\nstatus: Proposed\nrecorded_at: 2026-09-01\n---\n\n# Kafka\n\n- [idea] move the queue to Kafka\n",
        ),
    ])
    .await;
    let mut p = write(
        "queue.md",
        "# Queue\n\nFor people.\n\n## Today\n\nRedis.\n\n## Maybe\n\nProposed: Kafka.\n",
    );
    p.sources = BTreeMap::from([
        ("Today".to_string(), vec!["runbooks/deploy".to_string()]),
        ("Maybe".to_string(), vec!["ideas/kafka".to_string()]),
    ]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let doc = fx
        .engine
        .read_reflection("eng", "queue.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        sources_by_heading(&doc, "Today"),
        serde_json::json!([{ "permalink": "runbooks/deploy", "title": "Deploy",
            "state": "current", "status": "stable" }]),
        "{doc}"
    );
    assert_eq!(
        sources_by_heading(&doc, "Maybe"),
        serde_json::json!([{ "permalink": "ideas/kafka", "title": "Kafka",
            "state": "current", "status": "Proposed" }]),
        "{doc}"
    );
}
