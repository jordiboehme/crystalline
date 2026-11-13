//! The `Host` allow-list in front of the JSON API, and the `Origin` allow-list
//! the MCP transport is given, both drawn from `service.allowed_hosts`.
//!
//! The MCP transport has always refused a `Host` that is not loopback or on
//! `service.allowed_hosts`: that is what stops DNS rebinding, where a web page
//! points its own host name at 127.0.0.1 and then talks to this daemon as if
//! it were same-origin. The JSON API under `/api/v1` had no such check, so a
//! rebinding page could reach it, for example to claim first-run setup on an
//! instance nobody had signed in to yet. [`HostGuard`] applies the transport's
//! rule to the API too, from the same setting and in the same spelling.
//!
//! Three answers, the same three the transport gives:
//!
//! * an unset or empty list is loopback only;
//! * a single `*` in it accepts any `Host` and turns the guard off;
//! * anything else is loopback plus the listed hosts.
//!
//! The host of `service.public_url` is added in every case but `*`, for the
//! JSON API and the transport alike: it is the operator's own answer to what
//! this instance is called, and a rebinding page cannot choose it.
//!
//! A request with no `Host` at all passes. A browser always sends one, so a
//! rebinding page always arrives with its own name in it; a request without
//! one comes from a program, which could just as well have sent
//! `Host: localhost`. Refusing it would protect nothing.
//!
//! This is not access control. A program that is not a browser chooses its
//! `Host` freely; the guard keeps a web page in somebody's browser out, and
//! sign-in keeps everybody else out.

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;
use crate::oauth::{ALWAYS_ALLOWED_HOSTS, AllowedHost};

/// The `Host` rule for the JSON API. Built once at startup from the resolved
/// `service.allowed_hosts`.
#[derive(Clone, Debug)]
pub struct HostGuard {
    /// The hosts a request may name, or `None` for any host (a single `*`).
    allowed: Option<Vec<AllowedHost>>,
}

impl HostGuard {
    /// The guard for this `service.allowed_hosts` (already split and
    /// normalized, as the daemon resolves it) and `service.public_url`.
    pub fn new(allowed_hosts: &[String], public_url: Option<&str>) -> HostGuard {
        HostGuard {
            allowed: effective_hosts(allowed_hosts, public_url).map(|hosts| {
                hosts
                    .iter()
                    .filter_map(|host| AllowedHost::entry(host))
                    .collect()
            }),
        }
    }

    /// Whether a request that names `host` is answered. `None` is a request
    /// with no `Host` header, which passes (see the module comment).
    fn admits(&self, host: Option<&str>) -> bool {
        let Some(allowed) = &self.allowed else {
            return true;
        };
        let Some(host) = host else {
            return true;
        };
        AllowedHost::arriving(host)
            .is_some_and(|arrived| allowed.iter().any(|entry| entry.covers(&arrived)))
    }
}

/// The hosts a request may name, in the spelling `service.allowed_hosts`
/// uses: loopback, the list and the host of `service.public_url`, or `None`
/// when a single `*` turns the check off. The one place the JSON API's guard,
/// the MCP transport's `Host` list and its `Origin` list read the settings
/// from.
pub fn effective_hosts(allowed_hosts: &[String], public_url: Option<&str>) -> Option<Vec<String>> {
    if allowed_hosts.iter().any(|host| host.trim() == "*") {
        return None;
    }
    Some(
        ALWAYS_ALLOWED_HOSTS
            .iter()
            .map(|host| host.to_string())
            .chain(
                allowed_hosts
                    .iter()
                    .map(|host| host.trim().to_string())
                    .filter(|host| !host.is_empty()),
            )
            .chain(public_url.and_then(public_host))
            .collect(),
    )
}

/// The host of `service.public_url` as an allow-list entry, with its port
/// when the url names one, or `None` for a value that is not an absolute
/// `http` or `https` url with a host.
fn public_host(url: &str) -> Option<String> {
    let url = url::Url::parse(url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// The browser origins the MCP transport accepts in an `Origin` header, or
/// `None` when a single `*` in `service.allowed_hosts` turns the `Host` check
/// off, in which case the `Origin` check stays off too.
///
/// Every allowed host becomes an `http` and an `https` origin: with the port
/// the entry names, or any port where it names none, which is the `Host`
/// rule's own reading of a portless entry. The host of `service.public_url`
/// is among them, so a deployment behind a proxy that rewrites the `Host` to
/// loopback still accepts a browser at its public address. A request with no
/// `Origin` header is not affected at all: that is every client that is not
/// a browser.
///
/// Every entry carries a port or `:*`, because rmcp warns at startup about an
/// entry without one.
pub fn mcp_allowed_origins(
    allowed_hosts: &[String],
    public_url: Option<&str>,
) -> Option<Vec<String>> {
    let hosts = effective_hosts(allowed_hosts, public_url)?;
    let mut origins = Vec::new();
    for entry in hosts.iter().filter_map(|host| AllowedHost::entry(host)) {
        let host = if entry.host.contains(':') {
            format!("[{}]", entry.host)
        } else {
            entry.host.clone()
        };
        let port = entry
            .port
            .map_or_else(|| "*".to_string(), |port| port.to_string());
        for scheme in ["http", "https"] {
            origins.push(format!("{scheme}://{host}:{port}"));
        }
    }
    Some(origins)
}

/// The middleware: refuse a request whose `Host` is not allowed with `403`,
/// before any route or the session guard runs.
///
/// The GitHub App's webhook is exempt. GitHub delivers it to the public
/// address, and each delivery proves itself with an HMAC signature that a web
/// page cannot make, so the `Host` check would add nothing there and would
/// only drop access updates on an instance whose public host is not listed.
pub async fn check(State(guard): State<HostGuard>, request: Request, next: Next) -> Response {
    if request.uri().path() == crate::github_webhook::PATH {
        return next.run(request).await;
    }
    // HTTP/2 carries the host in `:authority`, and a nested router can lose the
    // `Host` header hyper made from it, so the URI's authority is the
    // fallback, as in the MCP transport.
    let host = request
        .headers()
        .get(header::HOST)
        .map(|value| value.to_str().unwrap_or("\u{0}"))
        .or_else(|| request.uri().authority().map(|a| a.as_str()));
    if guard.admits(host) {
        return next.run(request).await;
    }
    tracing::warn!(
        host = host.unwrap_or_default(),
        "refused a JSON API request whose Host is not allowed (possible DNS rebinding)"
    );
    // The refusal names the setting and never the entries in it, like the
    // OAuth documents' own refusal.
    ApiError::forbidden(
        "This request's Host is not one this instance answers to. An administrator adds the \
         host people use to service.allowed_hosts (CRYSTALLINE_SERVICE_ALLOWED_HOSTS).",
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|h| h.to_string()).collect()
    }

    #[test]
    fn an_empty_list_is_loopback_only() {
        let guard = HostGuard::new(&[], None);
        for host in [
            "localhost",
            "localhost:7411",
            "127.0.0.1:7411",
            "[::1]:7411",
        ] {
            assert!(guard.admits(Some(host)), "{host}");
        }
        for host in ["evil.example", "evil.example:7411", "192.168.1.5:7411"] {
            assert!(!guard.admits(Some(host)), "{host}");
        }
    }

    #[test]
    fn a_listed_host_is_answered_with_any_port_or_the_one_it_names() {
        let guard = HostGuard::new(&hosts(&["muthur.lan", "kb.example:8443"]), None);
        assert!(guard.admits(Some("muthur.lan")));
        assert!(guard.admits(Some("MUTHUR.lan:7411")));
        assert!(guard.admits(Some("kb.example:8443")));
        assert!(!guard.admits(Some("kb.example")));
        assert!(!guard.admits(Some("kb.example:443")));
        assert!(
            guard.admits(Some("localhost:7411")),
            "loopback stays allowed"
        );
        assert!(!guard.admits(Some("evil.example")));
    }

    #[test]
    fn the_public_url_host_is_answered_too() {
        let guard = HostGuard::new(&[], Some("https://kb.example/crystalline"));
        assert!(guard.admits(Some("kb.example")));
        assert!(guard.admits(Some("kb.example:443")));
        assert!(!guard.admits(Some("evil.example")));
        let ported = HostGuard::new(&[], Some("http://kb.example:8080"));
        assert!(ported.admits(Some("kb.example:8080")));
        assert!(!ported.admits(Some("kb.example:9090")));
    }

    #[test]
    fn a_star_turns_the_guard_off() {
        let guard = HostGuard::new(&hosts(&["*"]), None);
        assert!(guard.admits(Some("anything.example:1")));
    }

    #[test]
    fn a_request_without_a_host_passes_and_a_garbled_one_does_not() {
        let guard = HostGuard::new(&[], None);
        assert!(guard.admits(None));
        assert!(!guard.admits(Some("\u{0}")));
        assert!(!guard.admits(Some("")));
    }

    #[test]
    fn the_origins_follow_the_hosts_with_explicit_ports() {
        let origins = mcp_allowed_origins(
            &hosts(&["muthur.lan", "kb.example:8443"]),
            Some("https://crystalline.example.com/crystalline"),
        )
        .unwrap();
        for expected in [
            "http://localhost:*",
            "https://localhost:*",
            "http://127.0.0.1:*",
            "http://[::1]:*",
            "https://[::1]:*",
            "http://muthur.lan:*",
            "https://muthur.lan:*",
            "http://kb.example:8443",
            "https://kb.example:8443",
            "http://crystalline.example.com:*",
            "https://crystalline.example.com:*",
        ] {
            assert!(
                origins.contains(&expected.to_string()),
                "{expected} in {origins:?}"
            );
        }
        assert!(
            origins.iter().all(|origin| origin
                .rsplit_once(':')
                .is_some_and(|(_, port)| port == "*" || port.parse::<u16>().is_ok())),
            "every entry carries a port: {origins:?}"
        );
    }

    #[test]
    fn a_star_leaves_the_origins_open() {
        assert_eq!(mcp_allowed_origins(&hosts(&["*"]), None), None);
    }

    #[test]
    fn an_unusable_public_url_adds_nothing() {
        let base = mcp_allowed_origins(&[], None).unwrap();
        assert_eq!(
            mcp_allowed_origins(&[], Some("ftp://x.example")).unwrap(),
            base
        );
        assert_eq!(mcp_allowed_origins(&[], Some("not a url")).unwrap(), base);
    }
}
