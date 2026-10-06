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
