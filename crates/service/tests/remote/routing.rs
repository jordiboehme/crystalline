//! The local daemon's router over the ctl `tool` path: one-domain calls to
//! their one source, fan-outs with deadlines, the server's refusals word for
//! word, a server that went down, and translation in every answer. The last
//! part pins the two rules that keep routing to the machine owner: an HTTP
//! session or a REST call is never routed, and a server never forwards a
//! forwarded call.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use crystalline_remote::{
    Cached, CredentialKind, ForwardedAgent, MountRecord, ROUTING_FILE, ServerCredential, SourceSet,
    update_sources, write_cached,
};
use crystalline_service::route::run_tool_routed;
use serde_json::{Value, json};

use crate::client::{save_credential, source};
use crate::fixture::{Options, PASSWORD, RemoteServer};
use crate::local::LocalMachine;
use crate::support::McpTestSession;

fn agent() -> ForwardedAgent {
    ForwardedAgent {
        client: Some("claude-code/2.1.290".to_string()),
    }
}

async fn call(machine: &LocalMachine, tool: &str, args: Value) -> anyhow::Result<Value> {
    run_tool_routed(&machine.engine, tool, args, &agent(), None).await
}

/// A server that takes ten seconds over every ctl request, connected to
/// `machine` as `slow` with the domain `drafts` in its cached routing model.
///
/// The process's first HTTP client takes most of a second to build (the
/// platform's certificate roots load once); built here, so the budgets the
/// tests set measure the call and not that.
async fn connect_slow(machine: &LocalMachine) {
    let _ = crystalline_remote::http_client().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/api/v1/ctl",
            axum::routing::post(|| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                axum::Json(json!({ "v": 1, "ok": true, "data": {} }))
            }),
        );
        let _ = axum::serve(listener, app).await;
    });
    let mut slow = source(&slow_url, "slow", CredentialKind::Token, None);
    slow.mounts.push(MountRecord {
        remote: "drafts".into(),
        local: "drafts".into(),
    });
    save_credential(
        &machine.remote_dir(),
        &slow,
        &ServerCredential::token(
            "cmt_slow".into(),
            slow_url.clone(),
            "keeper".into(),
            Utc::now(),
        ),
    );
    write_cached(
        &slow.host_dir(&machine.remote_dir()),
        ROUTING_FILE,
        &Cached {
            account: "keeper".into(),
            etag: "e".into(),
            fetched_at: Utc::now(),
            data: json!({ "read_only": false, "domains": [{ "name": "drafts", "bullets": [], "origin": null }] }),
            last_failure: None,
        },
    )
    .unwrap();
    update_sources(&machine.remote_dir(), |file| {
        file.upsert(slow.clone());
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn a_one_domain_call_reaches_only_its_source() {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let beta = RemoteServer::start_with(Options::TOKENS, &["specs"]).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&acme, "acme", "keeper").await;
    machine.connect(&beta, "beta", "keeper").await;
    machine.mount();
    call(
        &machine,
        "write_engram",
        json!({ "domain": "specs", "title": "Only Beta", "content": "- [fact] beta only" }),
    )
    .await
    .unwrap();
    assert!(beta.file("specs", "only-beta.md").exists());
    assert!(
        !acme.tmp.path().join("specs").exists(),
        "acme was never asked"
    );
    assert!(!machine.tmp.path().join("specs").exists(), "nothing local");
    let local = call(
        &machine,
        "read_engram",
        json!({ "identifier": "local-note", "domain": "notes" }),
    )
    .await
    .unwrap();
    assert_eq!(local["domain"], "notes");
    assert!(local.get("source").is_none());
}

/// One source too slow, one gone. The answer comes back within the deadline
/// with the local hits and the healthy source's, and names both of the
/// others.
#[tokio::test]
async fn a_fan_out_with_one_slow_and_one_unreachable_source_answers_within_the_deadline_and_names_both()
 {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let gone = RemoteServer::start_with(Options::TOKENS, &["archive"]).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&acme, "acme", "keeper").await;
    machine.connect(&gone, "gone", "keeper").await;
    gone.stop().await;
    connect_slow(&machine).await;
    machine.mount();

    let started = Instant::now();
    let answer = run_tool_routed(
        &machine.engine,
        "search_engrams",
        json!({ "query": "vent" }),
        &agent(),
        Some(Duration::from_millis(700)),
    )
    .await
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    let text = answer.to_string();
    assert!(
        text.contains("\"notes\""),
        "the local hits are kept: {answer}"
    );
    assert!(text.contains("\"open\""), "acme's hits are kept: {answer}");
    let missing: Vec<&str> = answer["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["source"].as_str().unwrap())
        .collect();
    assert_eq!(missing, vec!["gone", "slow"], "{answer}");
    let note = answer["note"].as_str().unwrap();
    assert!(
        note.contains(
            "slow cannot be reached right now (it did not answer within 0.7 s; check the VPN or the network); its domains are missing from these results"
        ),
        "{note}"
    );
    assert!(
        note.contains("gone cannot be reached right now (check the VPN or the network); its domains are missing from these results"),
        "{note}"
    );
}

/// A caller's own budget holds on one hop too: a one-domain call to a source
/// that does not answer ends at the budget and says what happened.
#[tokio::test]
async fn a_one_domain_call_to_a_slow_source_ends_at_the_callers_budget() {
    let machine = LocalMachine::start(false).await;
    connect_slow(&machine).await;
    machine.mount();
    let started = Instant::now();
    let failure = run_tool_routed(
        &machine.engine,
        "read_engram",
        json!({ "identifier": "x", "domain": "drafts" }),
        &agent(),
        Some(Duration::from_millis(500)),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    assert!(
        failure.starts_with(
            "slow cannot be reached right now (check the VPN or the network); its domains are unavailable."
        ),
        "{failure}"
    );
    assert!(
        failure.contains("(it did not answer within 0.5 s)"),
        "{failure}"
    );
}

#[tokio::test]
async fn a_search_with_domains_asks_only_the_sources_that_hold_them() {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&acme, "acme", "keeper").await;
    machine.mount();
    let local = call(
        &machine,
        "search_engrams",
        json!({ "query": "vent", "domains": ["notes"] }),
    )
    .await
    .unwrap();
    assert!(
        local.get("parts").is_none(),
        "a local-only search is exactly what it was: {local}"
    );
    let remote = call(
        &machine,
        "search_engrams",
        json!({ "query": "vent", "domains": ["open"] }),
    )
    .await
    .unwrap();
    assert!(
        remote.get("parts").is_none(),
        "one source is one hop, not a fan-out"
    );
    assert!(
        remote["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["domain"] == "open"),
        "{remote}"
    );
}

/// Spec A4 Testing: a viewer writing to a mounted domain gets the server's
/// refusal word for word.
#[tokio::test]
async fn a_viewer_writing_to_a_mounted_domain_gets_the_servers_refusal_word_for_word() {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&acme, "acme", "looker").await;
    machine.mount();
    let refusal = call(
        &machine,
        "write_engram",
        json!({ "domain": "open", "title": "Nope", "content": "- [fact] no" }),
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(
        refusal,
        "your access to 'open' is viewer, and editor access is required to change it"
    );
}

/// The server goes down. A call for its domain says so in the A8 sentence,
/// a second one inside the down window answers at once, and the hidden local
/// copy of a domain it replaces is never answered instead.
#[tokio::test]
async fn a_call_to_a_source_that_went_down_says_so_and_never_answers_from_the_hidden_copy() {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(true).await;
    machine.connect(&acme, "acme", "keeper").await;
    machine.mount();
    acme.stop().await;
    let failure = call(
        &machine,
        "read_engram",
        json!({ "identifier": "local-note", "domain": "platform" }),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        failure.starts_with(
            "acme cannot be reached right now (check the VPN or the network); its domains are unavailable."
        ),
        "{failure}"
    );
    assert!(failure.contains("'platform' comes from acme"), "{failure}");
    assert!(
        failure.contains(&acme.origin()),
        "the address is named: {failure}"
    );
    assert!(failure.contains("it recovers by itself"), "{failure}");
    assert!(
        !failure.contains("green"),
        "nothing of the hidden copy: {failure}"
    );

    let started = Instant::now();
    let again = call(
        &machine,
        "read_engram",
        json!({ "identifier": "local-note", "domain": "platform" }),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "skipped at once inside the down window: {:?}",
        started.elapsed()
    );
    assert!(again.contains("'platform' comes from acme"), "{again}");

    let swept = call(&machine, "search_engrams", json!({ "query": "vent" }))
        .await
        .unwrap();
    assert!(
        !swept.to_string().contains("local platform vent"),
        "the hidden copy is no hit either: {swept}"
    );
    assert_eq!(swept["missing"][0]["source"], "acme");
}

/// With `open` taken by acme, beta's `open` is `open-beta`. No structured
/// field of any answer about it may carry beta's own name.
#[tokio::test]
async fn no_server_name_survives_in_any_structured_field_of_any_forwarded_tool() {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let beta = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&acme, "acme", "keeper").await;
    machine.connect(&beta, "beta", "keeper").await;
    machine.mount();
    assert_eq!(
        machine
            .engine
            .sources()
            .unwrap()
            .table()
            .mount("open-beta")
            .unwrap()
            .source,
        "beta"
    );
    let calls: Vec<(&str, Value)> = vec![
        (
            "write_engram",
            json!({ "domain": "open-beta", "title": "Pinned", "content": "- [fact] see [[Open Note]]\n- relates_to [[Open Note]]" }),
        ),
        (
            "read_engram",
            json!({ "identifier": "pinned", "domain": "open-beta" }),
        ),
        (
            "read_engram",
            json!({ "identifier": "crystalline://open-beta/open-note" }),
        ),
        (
            "edit_engram",
            json!({ "identifier": "pinned", "domain": "open-beta", "operation": "append", "content": "- [fact] more" }),
        ),
        (
            "search_engrams",
            json!({ "query": "vent", "domains": ["open-beta"] }),
        ),
        (
            "build_context",
            json!({ "anchor": "crystalline://open-beta/pinned" }),
        ),
        ("recent_activity", json!({ "domains": ["open-beta"] })),
        ("browse_domain", json!({ "domain": "open-beta" })),
        ("validate_engrams", json!({ "domain": "open-beta" })),
        (
            "infer_schema",
            json!({ "domain": "open-beta", "type": "engram" }),
        ),
        ("vocabulary", json!({ "domain": "open-beta" })),
        ("evolve_engrams", json!({ "domains": ["open-beta"] })),
        (
            "move_engram",
            json!({ "identifier": "pinned", "domain": "open-beta", "destination": "moved/pinned" }),
        ),
        (
            "delete_engram",
            json!({ "identifier": "moved/pinned", "domain": "open-beta" }),
        ),
    ];
    for (tool, args) in calls {
        let answer = call(&machine, tool, args.clone())
            .await
            .unwrap_or_else(|e| panic!("{tool} {args}: {e}"));
        let mut leaks = Vec::new();
        find_server_names(&answer, None, &mut leaks);
        assert!(
            leaks.is_empty(),
            "{tool}: beta's own name in {leaks:?}\n{answer}"
        );
        assert!(
            answer.get("web_url_template").is_none(),
            "{tool}: a server's template would be filled with this machine's name: {answer}"
        );
    }
    let listing = call(&machine, "list_domains", json!({})).await.unwrap();
    let row = listing["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["source"] == "beta" && d["remote_name"] == "open")
        .unwrap();
    assert_eq!(row["name"], "open-beta");
    assert_eq!(
        row["path"],
        Value::Null,
        "a server's path means nothing here"
    );
    assert!(
        !listing["domains"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["source"] == "beta" && d["remote_name"] == "platform"),
        "beta's platform is acme's domain, skipped here, and not listed under beta's name: {listing}"
    );
    assert!(listing.get("missing").is_none(), "{listing}");
}

/// Every place a structured field names beta's `open` instead of `open-beta`.
fn find_server_names(value: &Value, key: Option<&str>, out: &mut Vec<String>) {
    const TEXT: &[&str] = &[
        "content",
        "snippet",
        "frontmatter",
        "observations",
        "text",
        "finding",
        "evidence",
        "fix",
        "web_url",
        "web_url_template",
        "line_text",
        "counterpart_line_text",
    ];
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if TEXT.contains(&k.as_str()) {
                    continue;
                }
                find_server_names(v, Some(k), out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| find_server_names(v, key, out)),
        Value::String(s) => {
            let name_key = matches!(
                key,
                Some(
                    "domain"
                        | "destination_domain"
                        | "canonical_name"
                        | "domains"
                        | "pending_domains"
                )
            );
            if (name_key && s == "open") || s.contains("crystalline://open/") {
                out.push(format!("{}: {s}", key.unwrap_or("?")));
            }
        }
        _ => {}
    }
}

/// A hand edit of config.yaml that gives a new local domain a name a source
/// already mounts reaches the table by the next routed call: the mount keeps
/// the name, the local domain is hidden, and the call goes to the source.
#[tokio::test]
async fn a_hand_edit_of_the_config_reaches_the_table_at_the_next_call() {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&acme, "acme", "keeper").await;
    let set = machine.mount();
    assert!(!set.shadowed().contains("open"));
    let dir = machine.tmp.path().join("open");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: open\npermalink: manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# open\n",
    )
    .unwrap();
    let config_path = machine.tmp.path().join("config.yaml");
    let mut cfg: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(&config_path).unwrap();
    cfg.domains.insert(
        "open".into(),
        crystalline_core::config::DomainEntry::file(dir),
    );
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let read = call(
        &machine,
        "read_engram",
        json!({ "identifier": "open-note", "domain": "open" }),
    )
    .await
    .unwrap();
    assert_eq!(read["domain"], "open", "{read}");
    assert!(
        set.shadowed().contains("open"),
        "the hand-edited local domain is hidden: {:?}",
        set.shadowed()
    );
}

/// A `connect` that ran while nothing told the set (a daemon that could not
/// be reached, or a hand edit) is picked up by the cheap look the poller
/// takes.
#[tokio::test]
async fn a_connect_behind_the_sets_back_is_picked_up_by_the_look() {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    let set = machine.mount();
    assert!(set.is_empty());
    assert!(set.reload_if_changed().is_none(), "nothing changed yet");
    machine.connect(&acme, "acme", "keeper").await;
    assert!(set.reload_if_changed().is_some(), "sources.json changed");
    assert_eq!(set.table().mount("open").unwrap().source, "acme");
    assert!(set.reload_if_changed().is_none(), "and only once");
    set.refresh(Duration::from_secs(5)).await;
    assert!(
        set.reload_if_changed().is_none(),
        "what a refresh saved is what the set holds, so the look never asks the servers again"
    );
    assert!(
        set.set_local_if_changed(machine.engine.local_domains())
            .is_none(),
        "the local domains did not change"
    );
}

#[tokio::test]
async fn status_names_every_source_its_mounts_and_what_it_hides() {
    let acme = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(true).await;
    machine.connect(&acme, "acme", "keeper").await;
    machine.mount();
    let status = crystalline_service::route::sources_status(&machine.engine);
    let acme_row = &status[0];
    assert_eq!(acme_row["name"], "acme", "{status}");
    assert_eq!(acme_row["url"], acme.origin());
    assert_eq!(acme_row["account"], "keeper");
    assert_eq!(acme_row["kind"], "token");
    assert_eq!(acme_row["from_env"], false);
    assert!(
        acme_row["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["local"] == "platform" && m["replaces_local"] == true),
        "{status}"
    );
    assert_eq!(
        acme_row["shadowed"],
        json!([{ "local": "platform", "reason": "copy", "by": "platform" }]),
        "{status}"
    );
}

/// A server with sources of its own, connected the way a machine is. Its own
/// domains keep their names, so they stay visible beside the mounts.
async fn server_with_a_source() -> (RemoteServer, RemoteServer) {
    let upstream = RemoteServer::start_with(Options::TOKENS, &["upstream"]).await;
    let server = RemoteServer::start(Options::TOKENS).await;
    let remote_dir = server.tmp.path().join("remote");
    crystalline_remote::connect_with_token(
        &upstream.origin(),
        Some("up"),
        &upstream.token_for("keeper").await,
        &remote_dir,
        &server.engine.local_domains(),
    )
    .await
    .unwrap();
    server.engine.set_sources(Arc::new(SourceSet::load(
        remote_dir,
        server.engine.local_domains(),
        |_| None,
    )));
    assert!(
        server
            .engine
            .sources()
            .unwrap()
            .table()
            .mount("upstream")
            .is_some(),
        "the server mounts its source's domain"
    );
    (upstream, server)
}

/// A server with sources of its own answers a remote call from its own
/// domains only.
#[tokio::test]
async fn a_remote_tool_call_never_fans_out_to_the_servers_own_sources() {
    let (_upstream, server) = server_with_a_source().await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server
        .ctl(
            Some(&keeper),
            json!({ "v": 1, "cmd": "tool", "tool": "search_engrams", "args": { "query": "vent" } }),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(reply["data"].get("parts").is_none(), "no fan-out: {reply}");
    assert!(
        reply.to_string().contains("\"open\""),
        "its own hits: {reply}"
    );
    assert!(!reply.to_string().contains("upstream"), "{reply}");
    let (_, named) = server
        .ctl(
            Some(&keeper),
            json!({ "v": 1, "cmd": "tool", "tool": "read_engram", "args": { "identifier": "upstream-note", "domain": "upstream" } }),
        )
        .await;
    assert_eq!(named["ok"], false, "never forwarded: {named}");
}

/// An MCP session over HTTP and a REST call on a daemon with sources are
/// answered from its own domains: neither is ever routed.
#[tokio::test]
async fn an_http_session_and_a_rest_call_never_route_to_a_source() {
    let (_upstream, server) = server_with_a_source().await;
    let keeper = server.token_for("keeper").await;
    let session = McpTestSession::open(&server.addr, Some(&keeper)).await;
    let searched = session
        .call_tool(
            "search_engrams",
            json!({ "query": "vent", "domains": ["upstream"] }),
        )
        .await;
    assert!(searched.starts_with("HTTP/1.1 200 "), "{searched}");
    assert!(!searched.contains("upstream-note"), "{searched}");
    assert!(!searched.contains("is blue"), "{searched}");
    let swept = session
        .call_tool("search_engrams", json!({ "query": "vent" }))
        .await;
    assert!(swept.contains("open-note"), "its own hits: {swept}");
    assert!(!swept.contains("upstream-note"), "{swept}");

    let login = server
        .http
        .post(format!("{}/api/v1/auth/login", server.origin()))
        .json(&json!({ "name": "keeper", "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookies = login
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_string))
        .collect::<Vec<_>>()
        .join("; ");
    let rest = server
        .http
        .get(format!(
            "{}/api/v1/search?q=vent&mode=text",
            server.origin()
        ))
        .header("cookie", cookies)
        .send()
        .await
        .unwrap();
    assert_eq!(rest.status(), 200);
    let body = rest.text().await.unwrap();
    assert!(body.contains("open-note"), "its own hits: {body}");
    assert!(!body.contains("upstream-note"), "{body}");
}

#[test]
fn the_planner_reads_the_names_a_call_carries() {
    use crystalline_service::route::named_domains;
    assert_eq!(
        named_domains("read_engram", &json!({ "identifier": "crystalline://a/x" })).unwrap(),
        vec!["a"]
    );
    assert_eq!(
        named_domains(
            "move_engram",
            &json!({ "identifier": "x", "domain": "a", "destination": "y", "destination_domain": "b" })
        )
        .unwrap(),
        vec!["a", "b"]
    );
    assert_eq!(
        named_domains(
            "search_engrams",
            &json!({ "query": "q", "domains": ["a", "b"] })
        )
        .unwrap(),
        vec!["a", "b"]
    );
    assert!(
        named_domains("list_domains", &json!({}))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_forwarded_call_carries_only_the_sources_names() {
    use crystalline_remote::NameMap;
    use crystalline_service::route::forwarded_args;
    let names = NameMap {
        source: "beta".into(),
        to_remote: [("open-beta".to_string(), "open".to_string())].into(),
        to_local: [("open".to_string(), "open-beta".to_string())].into(),
    };
    let one = forwarded_args(
        &json!({ "domain": "open-beta", "identifier": "crystalline://open-beta/x" }),
        &names,
        false,
    );
    assert_eq!(
        one,
        json!({ "domain": "open", "identifier": "crystalline://open/x" })
    );
    let filtered = forwarded_args(
        &json!({ "query": "q", "domains": ["notes", "open-beta"] }),
        &names,
        true,
    );
    assert_eq!(filtered["domains"], json!(["open"]));
    let none_of_its_own = forwarded_args(
        &json!({ "anchor": "crystalline://open-beta/x", "domains": ["notes"] }),
        &names,
        false,
    );
    assert_eq!(
        none_of_its_own["domains"],
        json!(["open"]),
        "this machine's own names never reach the server"
    );
    let unfiltered = forwarded_args(&json!({ "query": "q" }), &names, false);
    assert!(unfiltered.get("domains").is_none());
}
