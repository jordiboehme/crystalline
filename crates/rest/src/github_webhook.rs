//! The GitHub App's webhook: `POST /github/webhook` (under `/api/v1`).
//!
//! GitHub sends it the events the App subscribes to, so a change of who may
//! read a repository takes effect within seconds instead of when the five
//! minute cache runs out. It is a public route with no session, no CSRF
//! token and no account: the guard lets it through untouched
//! (`auth::guard`), and the only proof a delivery came from GitHub is its
//! `X-Hub-Signature-256`, the HMAC-SHA256 of the raw body under the App's
//! webhook secret. That is checked before anything else is read:
//!
//! 1. A body over [`WEBHOOK_BODY_BYTES`] is refused (413) before it is read
//!    to the end, so no HMAC runs over it.
//! 2. No webhook secret configured (or a blank one), no signature, or a
//!    signature that does not match: 401, and nothing else happens.
//! 3. No `X-GitHub-Delivery`: 400.
//! 4. A delivery id seen in the last day: 200, and no work (a replay).
//! 5. Only then is the body parsed and the event handled
//!    ([`Engine::apply_github_event`](crate::engine::Engine::apply_github_event)),
//!    which only ever makes the governor forget or close something. No role
//!    is ever read from a payload.
//!
//! The host a delivery comes from does not matter, so the route works for an
//! App on GitHub Enterprise Server unchanged. Log lines carry the delivery
//! id and the event name only, never the payload, the secret or the
//! signature.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::auth::constant_time_eq;
use super::{ApiError, ProblemDetail, RestState};

/// The route, under `/api/v1`. The manifest registers
/// `<service.public_url>/api/v1` + this as the App's webhook url.
pub const PATH: &str = "/github/webhook";

/// The largest delivery this route takes, 25 MiB: GitHub caps a webhook
/// payload at 25 MB and drops a larger one itself.
pub const WEBHOOK_BODY_BYTES: usize = 25 * 1024 * 1024;

/// How much of a header value a log line carries: a delivery id is a UUID,
/// and before the signature is checked anybody can send anything.
const LOGGED_CHARS: usize = 64;

/// The `X-Hub-Signature-256` value GitHub sends with `body` under `secret`:
/// `sha256=` and the lowercase hex HMAC-SHA256 of the raw body.
pub fn signature(secret: &[u8], body: &[u8]) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(body);
    let code = mac.finalize().into_bytes();
    format!("sha256={}", crystalline_index::hex_lower(&code))
}

/// Whether `presented` is the signature of `body` under `secret`, compared
/// in constant time. A blank secret verifies nothing: anybody can compute an
/// HMAC under an empty key.
fn verified(secret: &str, presented: &str, body: &[u8]) -> bool {
    if secret.trim().is_empty() {
        return false;
    }
    let expected = signature(secret.as_bytes(), body);
    constant_time_eq(presented.trim().as_bytes(), expected.as_bytes())
}

/// A header's value as text, if it has one.
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// A header value as a log line may carry it: cut short, so whatever a
/// stranger sent cannot fill the log. Logged with `?`, which escapes it.
fn for_log(value: Option<&str>) -> String {
    value.unwrap_or("none").chars().take(LOGGED_CHARS).collect()
}

/// `POST /github/webhook` - one delivery from the GitHub App.
#[utoipa::path(
    post,
    path = "/api/v1/github/webhook",
    tag = "settings",
    operation_id = "receive_github_webhook",
    summary = "Where the GitHub App delivers its webhook events.",
    description = "Called by GitHub, never by a browser. Public: no session, \
                   no CSRF token. The only proof is `X-Hub-Signature-256`, \
                   the HMAC-SHA256 of the raw body under the App's webhook \
                   secret, checked before anything else is read. A delivery \
                   id seen in the last day is answered 200 and does nothing. \
                   An event only makes the instance forget what it knew \
                   about a repository, a person or an installation (or \
                   close a domain whose repository the App lost); the next \
                   request asks GitHub. Nothing in a payload is believed \
                   about who may do what.",
    request_body(
        content = Object,
        content_type = "application/json",
        description = "The event's payload, as GitHub sends it, at most \
                       25 MiB.",
    ),
    responses(
        (
            status = 200,
            description = "Handled, a replay, or an event this instance does \
                           not act on.",
        ),
        (
            status = 400,
            description = "Signed, but without `X-GitHub-Delivery` or \
                           `X-GitHub-Event`, or a body that is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No webhook secret is configured, or the signature \
                           is missing or wrong. Nothing happened.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 413,
            description = "A body over 25 MiB, refused before the signature \
                           is checked.",
        ),
        (
            status = 500,
            description = "A change could not be saved. The delivery is not \
                           remembered, so GitHub's redelivery is handled.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn receive(
    State(state): State<RestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let delivery = header(&headers, "x-github-delivery");
    let event = header(&headers, "x-github-event");
    // The App slot may still have to be loaded, which can read the
    // keychain: not on this thread.
    let engine = state.engine.clone();
    let secret = tokio::task::spawn_blocking(move || engine.github_webhook_secret())
        .await
        .ok()
        .flatten();
    let Some(secret) = secret else {
        tracing::warn!(
            delivery = ?for_log(delivery),
            event = ?for_log(event),
            "a GitHub webhook delivery was refused: no webhook secret is configured for the GitHub App"
        );
        return Err(ApiError::unauthorized(
            "this instance has no GitHub App webhook secret to check the delivery with",
        ));
    };
    let signed = header(&headers, "x-hub-signature-256")
        .is_some_and(|presented| verified(&secret, presented, &body));
    drop(secret);
    if !signed {
        tracing::warn!(
            delivery = ?for_log(delivery),
            event = ?for_log(event),
            "a GitHub webhook delivery was refused: its signature is missing or wrong"
        );
        return Err(ApiError::unauthorized(
            "the delivery's X-Hub-Signature-256 is missing or does not match",
        ));
    }
    // From here on GitHub sent it.
    let Some(delivery) = delivery else {
        tracing::warn!(event = ?for_log(event), "a signed GitHub webhook delivery carried no delivery id");
        return Err(ApiError::bad_request(
            "the delivery carries no X-GitHub-Delivery",
        ));
    };
    let Some(event) = event else {
        tracing::warn!(delivery = ?for_log(Some(delivery)), "a signed GitHub webhook delivery carried no event name");
        return Err(ApiError::bad_request(
            "the delivery carries no X-GitHub-Event",
        ));
    };
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
        tracing::warn!(
            delivery = ?for_log(Some(delivery)),
            event = ?for_log(Some(event)),
            "a signed GitHub webhook delivery is not JSON"
        );
        ApiError::bad_request("the delivery's body is not JSON")
    })?;
    let new =
        state.auth.remember_delivery(delivery).await.map_err(|e| {
            ApiError::internal(format!("the delivery could not be recorded: {e:#}"))
        })?;
    if !new {
        tracing::info!(
            delivery = ?for_log(Some(delivery)),
            event = ?for_log(Some(event)),
            "a GitHub webhook delivery was seen before, so it does nothing"
        );
        return Ok(StatusCode::OK.into_response());
    }
    match state.engine.apply_github_event(event, &payload).await {
        Ok(outcome) => {
            tracing::info!(
                delivery = ?for_log(Some(delivery)),
                event = ?for_log(Some(event)),
                "a GitHub webhook delivery was handled"
            );
            if outcome.readers_changed {
                super::readers_changed(&state);
            }
            Ok(StatusCode::OK.into_response())
        }
        Err(e) => {
            // Not remembered, so GitHub's redelivery is handled.
            if let Err(forget) = state.auth.forget_delivery(delivery).await {
                tracing::warn!(
                    delivery = ?for_log(Some(delivery)),
                    "a failed GitHub webhook delivery could not be forgotten: {forget:#}"
                );
            }
            tracing::warn!(
                delivery = ?for_log(Some(delivery)),
                event = ?for_log(Some(event)),
                "a GitHub webhook delivery could not be handled: {e}"
            );
            Err(ApiError::internal(format!(
                "the delivery could not be handled: {e}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test vector of GitHub's "Validating webhook deliveries" page.
    #[test]
    fn the_signature_matches_githubs_own_example() {
        assert_eq!(
            signature(b"It's a Secret to Everybody", b"Hello, World!"),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
    }

    #[test]
    fn only_the_right_signature_verifies() {
        let body = b"{\"zen\":\"Keep it simple.\"}";
        let good = signature(b"s3cret", body);
        assert!(verified("s3cret", &good, body));
        assert!(!verified(
            "s3cret",
            &good,
            b"{\"zen\":\"Keep it simpler.\"}"
        ));
        assert!(!verified("another", &good, body));
        assert!(!verified("s3cret", &good.replace("sha256=", "sha1="), body));
        assert!(!verified("s3cret", "", body));
    }

    #[test]
    fn a_blank_secret_verifies_nothing() {
        let body = b"{}";
        assert!(!verified("", &signature(b"", body), body));
        assert!(!verified("  ", &signature(b"  ", body), body));
    }

    #[test]
    fn a_logged_header_is_cut_short() {
        let long = "x".repeat(500);
        assert_eq!(for_log(Some(&long)).len(), LOGGED_CHARS);
        assert_eq!(for_log(None), "none");
    }
}
