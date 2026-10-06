//! A server under a path: every route answers whether the proxy strips the
//! prefix or passes it through, and a path that only looks like the prefix is
//! routed unchanged.

use serde_json::{Value, json};

use crate::fixture::{Options, Proxy, RemoteServer};

const PREFIX: &str = "/crystalline";
const BOTH: [Proxy; 2] = [Proxy::Strips, Proxy::PassesThrough];

/// The MCP `initialize` body a client opens with.
fn initialize() -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": { "name": "prefix-test", "version": "1" } }
    })
}

async fn mcp(server: &RemoteServer, url: &str, token: &str) -> reqwest::Response {
    server
        .http
        .post(url)
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .body(initialize().to_string())
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn every_route_answers_under_the_prefix_whether_the_proxy_strips_it_or_not() {
    for proxy in BOTH {
        let server = RemoteServer::start(Options::TOKENS.under(PREFIX, proxy)).await;
        let base = server.base();
        let token = server.token_for("keeper").await;

        let health: Value = server
            .http
            .get(format!("{base}/health"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["status"], "ok", "{proxy:?}");
        if proxy == Proxy::Strips {
            // The front strips once and the daemon once, so a doubled prefix
            // only answers when the front really stripped the first one.
            let doubled: Value = server
                .http
                .get(format!("{}{PREFIX}{PREFIX}/health", server.origin()))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(doubled["status"], "ok", "the front stripped the prefix");
        }

        let (code, reply) = server
            .ctl(Some(&token), json!({ "v": 1, "cmd": "status" }))
            .await;
        assert_eq!(code, 200, "{proxy:?}: {reply}");
        assert_eq!(reply["data"]["account"], "keeper");

        let me = server
            .http
            .get(format!("{base}/api/v1/auth/me"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            me.status(),
            200,
            "{proxy:?}: the REST API answers under the prefix"
        );

        for url in [format!("{base}/"), base.clone()] {
            let opened = mcp(&server, &url, &token).await;
            assert_eq!(
                opened.status(),
                200,
                "{proxy:?} {url}: MCP is the fallback under the prefix"
            );
        }
    }
}

#[tokio::test]
async fn the_bare_prefix_reaches_mcp_without_a_redirect() {
    for proxy in BOTH {
        let server = RemoteServer::start(Options::TOKENS.under(PREFIX, proxy)).await;
        let token = server.token_for("keeper").await;
        let opened = mcp(&server, &server.base(), &token).await;
        assert!(
            !opened.status().is_redirection(),
            "{proxy:?}: a client never follows a redirect"
        );
        assert_eq!(opened.status(), 200, "{proxy:?}");
    }
}

#[tokio::test]
async fn a_request_without_the_prefix_is_routed_unchanged() {
    let server = RemoteServer::start(Options::TOKENS.under(PREFIX, Proxy::PassesThrough)).await;
    let health: Value = server
        .http
        .get(format!("{}/health", server.origin()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        health["status"], "ok",
        "what a stripping proxy sends is answered too"
    );
}

#[tokio::test]
async fn a_path_that_only_looks_like_the_prefix_is_routed_unchanged() {
    let server = RemoteServer::start(Options::TOKENS.under(PREFIX, Proxy::PassesThrough)).await;
    let origin = server.origin();
    for path in [
        "/crystallinex/health",
        "/CRYSTALLINE/health",
        "/crystalline/crystalline/health",
    ] {
        let answer = server
            .http
            .get(format!("{origin}{path}"))
            .send()
            .await
            .unwrap();
        let body = answer.text().await.unwrap();
        assert!(
            !body.contains("\"status\":\"ok\""),
            "{path} must not reach /health: {body}"
        );
    }
}

#[tokio::test]
async fn a_root_server_routes_exactly_as_before() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let health: Value = server
        .http
        .get(format!("{}/health", server.origin()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["status"], "ok");
    let prefixed = server
        .http
        .get(format!("{}/crystalline/health", server.origin()))
        .send()
        .await
        .unwrap();
    assert!(
        !prefixed.text().await.unwrap().contains("\"status\":\"ok\""),
        "a server without a prefix strips nothing"
    );
}

async fn document(server: &RemoteServer, url: &str) -> (u16, Value) {
    let answer = server.http.get(url).send().await.unwrap();
    let status = answer.status().as_u16();
    (status, answer.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_oauth_documents_answer_at_both_addresses_and_name_the_base() {
    for proxy in BOTH {
        let server = RemoteServer::start(Options::OAUTH.under(PREFIX, proxy)).await;
        let (origin, base) = (server.origin(), server.base());
        for url in [
            format!("{origin}/.well-known/oauth-protected-resource{PREFIX}"),
            format!("{base}/.well-known/oauth-protected-resource"),
            // The bare root address answers too, and names the base.
            format!("{origin}/.well-known/oauth-protected-resource"),
        ] {
            let (status, doc) = document(&server, &url).await;
            assert_eq!(status, 200, "{proxy:?} {url}");
            assert_eq!(doc["resource"], base.as_str(), "{proxy:?} {url}");
            assert_eq!(
                doc["authorization_servers"],
                json!([base]),
                "{proxy:?} {url}"
            );
        }
        for url in [
            format!("{origin}/.well-known/oauth-authorization-server{PREFIX}"),
            format!("{base}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/oauth-authorization-server"),
        ] {
            let (status, doc) = document(&server, &url).await;
            assert_eq!(status, 200, "{proxy:?} {url}");
            assert_eq!(doc["issuer"], base.as_str(), "{proxy:?} {url}");
            for endpoint in [
                "authorization_endpoint",
                "token_endpoint",
                "registration_endpoint",
                "revocation_endpoint",
            ] {
                assert!(
                    doc[endpoint]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("{base}/api/v1/oauth/")),
                    "{proxy:?} {endpoint}: {doc}"
                );
            }
        }
    }
}

#[tokio::test]
async fn without_a_prefix_only_the_two_root_documents_exist() {
    let server = RemoteServer::start(Options::OAUTH).await;
    let origin = server.origin();
    let (status, doc) = document(
        &server,
        &format!("{origin}/.well-known/oauth-protected-resource"),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(doc["resource"], origin.as_str(), "the 0.23.0 document");
    for path in [
        "/.well-known/oauth-protected-resource/crystalline",
        "/.well-known/oauth-authorization-server/crystalline",
        "/crystalline/.well-known/oauth-protected-resource",
    ] {
        let (_, doc) = document(&server, &format!("{origin}{path}")).await;
        assert!(
            doc.get("resource").is_none() && doc.get("issuer").is_none(),
            "{path}: {doc}"
        );
    }
}

#[tokio::test]
async fn the_challenge_names_the_rfc_9728_address() {
    let server = RemoteServer::start(Options::OAUTH.under(PREFIX, Proxy::PassesThrough)).await;
    let refused = server
        .http
        .post(server.base())
        .header("content-type", "application/json")
        .body(initialize().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401);
    assert_eq!(
        refused.headers()["www-authenticate"].to_str().unwrap(),
        format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource{PREFIX}\"",
            server.origin()
        )
    );
    let root = RemoteServer::start(Options::OAUTH).await;
    let refused = root
        .http
        .post(root.origin())
        .header("content-type", "application/json")
        .body(initialize().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.headers()["www-authenticate"].to_str().unwrap(),
        format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
            root.origin()
        ),
        "the 0.23.0 challenge at the root"
    );
}

#[tokio::test]
async fn a_token_for_the_base_works_and_one_for_the_bare_origin_is_refused() {
    let server = RemoteServer::start(Options::OAUTH.under(PREFIX, Proxy::PassesThrough)).await;
    let pair = server.oauth_pair("keeper").await;
    let (code, _) = server
        .ctl(Some(&pair.access), json!({ "v": 1, "cmd": "status" }))
        .await;
    assert_eq!(code, 200, "minted for the base through the real flow");

    let foreign = server
        .auth
        .issue_oauth_grant("keeper", &pair.client_id, &server.origin())
        .await
        .unwrap();
    let (code, _) = server
        .ctl(
            Some(&foreign.access_token),
            json!({ "v": 1, "cmd": "status" }),
        )
        .await;
    assert_eq!(
        code, 401,
        "a token for the root of the same host is not a token for the base"
    );

    let root = RemoteServer::start(Options::OAUTH).await;
    let root_pair = root.oauth_pair("keeper").await;
    let prefixed = root
        .auth
        .issue_oauth_grant(
            "keeper",
            &root_pair.client_id,
            &format!("{}{PREFIX}", root.origin()),
        )
        .await
        .unwrap();
    let (code, _) = root
        .ctl(
            Some(&prefixed.access_token),
            json!({ "v": 1, "cmd": "status" }),
        )
        .await;
    assert_eq!(code, 401, "and the other way round");
}

#[tokio::test]
async fn the_consent_redirect_lands_under_the_prefix() {
    for (options, page) in [
        (
            Options::OAUTH.under(PREFIX, Proxy::Strips),
            "/crystalline/authorize?request=",
        ),
        (Options::OAUTH, "/authorize?request="),
    ] {
        let server = RemoteServer::start(options).await;
        let registered: Value = server
            .http
            .post(format!("{}/api/v1/oauth/register", server.base()))
            .json(&json!({ "client_name": "t", "redirect_uris": ["http://127.0.0.1/callback"] }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let client_id = registered["client_id"].as_str().unwrap();
        let started = server
            .http
            .get(format!(
                "{}/api/v1/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2Fcallback&code_challenge={}&code_challenge_method=S256&state=s",
                server.base(),
                crate::fixture::CHALLENGE
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(started.status(), 302);
        let location = started.headers()["location"].to_str().unwrap();
        assert!(location.starts_with(page), "{location}");
    }
}

#[tokio::test]
async fn cookies_carry_the_prefix_as_their_path() {
    for (options, path) in [
        (
            Options::TOKENS.under(PREFIX, Proxy::PassesThrough),
            "Path=/crystalline",
        ),
        (
            Options::TOKENS.under(PREFIX, Proxy::Strips),
            "Path=/crystalline",
        ),
        (Options::TOKENS, "Path=/"),
    ] {
        let server = RemoteServer::start(options).await;
        let login = server
            .http
            .post(format!("{}/api/v1/auth/login", server.base()))
            .json(&json!({ "name": "keeper", "password": crate::fixture::PASSWORD }))
            .send()
            .await
            .unwrap();
        let set: Vec<String> = login
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        let session = set
            .iter()
            .find(|c| c.starts_with("fluid_session="))
            .unwrap();
        assert!(session.split("; ").any(|part| part == path), "{session}");
        let cookie = session.split(';').next().unwrap().to_string();
        let csrf = login.json::<Value>().await.unwrap()["csrf"]
            .as_str()
            .unwrap()
            .to_string();
        let logout = server
            .http
            .post(format!("{}/api/v1/auth/logout", server.base()))
            .header("cookie", cookie)
            .header("x-csrf-token", csrf)
            .send()
            .await
            .unwrap();
        let removal = logout
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .find(|c| c.starts_with("fluid_session="))
            .unwrap();
        assert!(
            removal.split("; ").any(|part| part == path),
            "the removal names the same path: {removal}"
        );
    }
}

#[tokio::test]
async fn web_url_carries_the_prefix() {
    let server = RemoteServer::start(Options::TOKENS.under(PREFIX, Proxy::PassesThrough)).await;
    let token = server.token_for("keeper").await;
    let (code, reply) = server
        .ctl(
            Some(&token),
            json!({
                "v": 1, "cmd": "tool", "tool": "read_engram",
                "args": { "identifier": "open-note", "domain": "open" }
            }),
        )
        .await;
    assert_eq!(code, 200, "{reply}");
    let web_url = reply["data"]["web_url"].as_str().unwrap();
    assert!(
        web_url.starts_with(&format!("{}/d/open/e/", server.base())),
        "{web_url}"
    );
}
