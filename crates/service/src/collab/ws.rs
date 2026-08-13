//! The collab upgrade route and its socket loop. Everything refusable is
//! refused BEFORE upgrade, as problem+json on the plain GET: editor role,
//! read_only, the strict same-host Origin rule, capacity and eligibility.
//! CSRF does not apply (a GET, which the guard exempts by method), and no
//! allowlist exists: the Origin must carry the request's own Host, nothing
//! else.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;

use super::control::{self, Control};
use super::session::{CollabSession, CollabSessions, ConnId, Frame, JoinError, Joined};
use crate::rest::{ApiError, ApiPath, Identity, ProblemDetail, RestState};

/// One session frame can legitimately carry a whole document (SyncStep2), so
/// the ceiling tracks the REST body limit plus protocol overhead - far below
/// axum's 64 MiB default.
pub const WS_MAX_MESSAGE_BYTES: usize = crate::rest::MAX_BODY_BYTES + 1024 * 1024;
/// How often the server pings an idle socket, so a dead peer behind a proxy
/// that keeps the connection nominally open is still noticed.
const PING_INTERVAL_SECS: u64 = 30;
/// 4400-4499 = permanent, do not reconnect (the y-websocket convention). The
/// `Closed` control that precedes it carries the reason.
pub const CLOSE_DELETED: u16 = 4404;

#[utoipa::path(
    get,
    path = "/api/v1/collab/{domain}/{permalink}",
    tag = "engrams",
    operation_id = "collab_join",
    summary = "Upgrade to a real-time co-editing session on one engram.",
    description = "WebSocket upgrade for the y-sync + awareness protocol over \
                   the engram's shared text. Editor role, a live session \
                   cookie and a same-host Origin header are all required and \
                   checked before the upgrade; a read-only instance refuses \
                   like every write. CSRF headers do not apply to the upgrade \
                   GET. Refusals are problem+json.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("permalink" = String, Path, description = "The engram's permalink; may contain slashes."),
    ),
    responses(
        (status = 101, description = "Switching protocols: the session is joined."),
        (status = 401, description = "No identity.", body = ProblemDetail, content_type = "application/problem+json"),
        (status = 403, description = "Viewer role, read-only instance, or a missing/foreign Origin.", body = ProblemDetail, content_type = "application/problem+json"),
        (status = 404, description = "No such engram.", body = ProblemDetail, content_type = "application/problem+json"),
        (status = 409, description = "Mixed line endings: this file cannot hold a shared session; edit solo.", body = ProblemDetail, content_type = "application/problem+json"),
        (status = 503, description = "Session or participant capacity reached.", body = ProblemDetail, content_type = "application/problem+json"),
    ),
)]
pub async fn join(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, permalink)): ApiPath<(String, String)>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    identity.require_editor()?;
    if state.engine.read_only() {
        return Err(ApiError::forbidden(
            "this instance is read-only, so collaborative editing is disabled",
        ));
    }
    require_same_host_origin(&headers)?;
    // The join runs under the state's join pass, and the pass is what makes a
    // domain unregistration's room sweep final: an unregister holds the same
    // fence for write across its sweep and the engine's `domain_remove`, so a
    // join is either complete (and swept) before that starts, or it waits and
    // then finds a domain that is gone. The guard covers exactly the join -
    // never the socket's life - and `CollabSessions::join` holding the
    // registry lock across its open is the other half of the argument (see
    // `CollabSessions::dispose_domain`).
    let joined = {
        let _pass = state.join_pass().await;
        state.collab.join(&domain, &permalink).await
    }
    .map_err(join_error)?;
    let sessions = state.collab.clone();
    // The failure twin of on_upgrade: the connection is REGISTERED in the
    // session already, so a handshake that dies after this handler returns
    // must run the same unwind - otherwise the participant slot leaks, the
    // session never reports empty, never final-saves and never disposes.
    let failed = {
        let sessions = sessions.clone();
        let session = joined.session.clone();
        let conn = joined.conn;
        move |_err| {
            tokio::spawn(async move {
                finish(&sessions, &session, conn).await;
            });
        }
    };
    Ok(ws
        .max_message_size(WS_MAX_MESSAGE_BYTES)
        .max_frame_size(WS_MAX_MESSAGE_BYTES)
        .on_failed_upgrade(failed)
        .on_upgrade(move |socket| run(socket, sessions, joined)))
}

/// The Origin header is REQUIRED and its authority must equal the request's
/// own Host. No allowlist, no configuration: cross-origin upgrades are
/// refused before any protocol traffic, which is the whole of the CSRF story
/// for a WebSocket - a browser sends Origin on an upgrade it cannot be talked
/// out of, and no CORS layer exists here to widen it.
///
/// Compared case-insensitively because a host is: a user who types
/// `http://LocalHost:8765` is sent `Host: LocalHost:8765` beside a lowercased
/// `Origin`, and a byte comparison would refuse their own machine.
fn require_same_host_origin(headers: &HeaderMap) -> Result<(), ApiError> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ApiError::forbidden("a collab upgrade requires a same-host Origin header")
        })?;
    // An opaque origin ("null", from a sandboxed frame) strips to nothing and
    // fails the comparison below, which is the intended answer.
    let authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or("");
    if host.is_empty() || !authority.eq_ignore_ascii_case(host) {
        return Err(ApiError::forbidden(
            "this Origin does not match the host being asked, so the upgrade is refused",
        ));
    }
    Ok(())
}

fn join_error(err: JoinError) -> ApiError {
    match err {
        JoinError::Engine(engine) => engine.into(),
        JoinError::MixedEndings => ApiError::conflict(
            "this file mixes CRLF and LF line endings, which a shared session \
             cannot hold without rewriting bytes; it opens in solo mode instead",
        ),
        JoinError::ServerFull => busy("this server is at its concurrent session limit"),
        JoinError::SessionFull => busy("this engram's session is at its participant limit"),
    }
}

/// A 503 for capacity: the request is fine and will work later, which is what
/// separates it from every other refusal on this route.
fn busy(detail: &str) -> ApiError {
    ApiError {
        status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
        title: "busy",
        detail: detail.to_string(),
        token_required: None,
    }
}

/// The per-connection loop: frames in to the session, fan-out frames back, a
/// keepalive ping, and the unwind (awareness removal, final save, disposal).
///
/// One task per socket, deliberately: reading and closing in two tasks would
/// let a frame be processed after `remove_conn` ran, and `handle_frame`'s
/// `entry().or_default()` would resurrect the connection it had just removed.
/// So [`finish`] runs strictly after this loop ends, here and nowhere else.
async fn run(socket: WebSocket, sessions: Arc<CollabSessions>, joined: Joined) {
    let Joined {
        session,
        conn,
        mut rx,
        greeting,
    } = joined;
    let (mut sink, mut stream) = socket.split();
    if sink
        .send(WsMessage::Binary(Bytes::from(greeting)))
        .await
        .is_err()
    {
        finish(&sessions, &session, conn).await;
        return;
    }
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(PING_INTERVAL_SECS));
    ping.tick().await; // the immediate first tick is not a keepalive
    'conn: loop {
        tokio::select! {
            incoming = stream.next() => match incoming {
                Some(Ok(WsMessage::Binary(bytes))) => {
                    // catch_unwind, not bare: yrs can panic on pathological
                    // text (y-crdt#386), and this path - conflict resolution
                    // converging the document - runs outside the saver's own
                    // catch_unwind seam. An unwind past this loop would skip
                    // the finish() below and leak the connection.
                    let handled = futures::FutureExt::catch_unwind(
                        std::panic::AssertUnwindSafe(session.handle_frame(conn, &bytes)),
                    )
                    .await;
                    let Ok(replies) = handled else {
                        // Session-fatal, exactly as it is in the saver: the
                        // room is told, saving stops, and the unwind below
                        // still removes this connection. `final_save` is
                        // disposal-guarded, so poisoning first makes it a
                        // no-op rather than a write through a broken doc.
                        tracing::error!(
                            epoch = %session.epoch(),
                            "a collab frame panicked; closing the session"
                        );
                        session.poison().await;
                        break 'conn;
                    };
                    for reply in replies {
                        if sink.send(WsMessage::Binary(Bytes::from(reply))).await.is_err() {
                            break 'conn;
                        }
                    }
                }
                Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break 'conn,
                Some(Ok(_)) => {} // Ping/Pong are answered by axum; Text is not ours
            },
            frame = rx.recv() => match frame {
                Ok(Frame { from, bytes }) => {
                    if from == Some(conn) {
                        continue; // never echo a sender's own update back
                    }
                    let closing = is_session_closed(&bytes);
                    if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                        break 'conn;
                    }
                    if closing {
                        let _ = sink
                            .send(WsMessage::Close(Some(CloseFrame {
                                code: CLOSE_DELETED,
                                reason: "closed".into(),
                            })))
                            .await;
                        break 'conn;
                    }
                }
                // Too far behind the room: this receiver has permanently
                // missed updates and is desynced with no recovery but a
                // reload, so closing is the only honest answer. Never
                // continue: the next frame would be applied onto a document
                // missing the ones in between.
                Err(broadcast::error::RecvError::Lagged(_)) => break 'conn,
                Err(broadcast::error::RecvError::Closed) => break 'conn,
            },
            _ = ping.tick() => {
                if sink.send(WsMessage::Ping(Bytes::new())).await.is_err() {
                    break 'conn;
                }
            }
        }
    }
    finish(&sessions, &session, conn).await;
}

/// Whether a fan-out frame is the session's `Closed` control, which ends every
/// connection with the permanent close code.
fn is_session_closed(bytes: &[u8]) -> bool {
    use yrs::sync::Message;
    use yrs::updates::decoder::Decode;
    matches!(
        Message::decode_v1(bytes),
        Ok(Message::Custom(control::CONTROL_TAG, payload))
            if matches!(control::decode(&payload), Some(Control::Closed { .. }))
    )
}

/// The unwind every exit path shares: drop the connection, and when it was the
/// last one, land whatever is unsaved and let the registry drop the room.
async fn finish(sessions: &Arc<CollabSessions>, session: &Arc<CollabSession>, conn: ConnId) {
    let last = session.remove_conn(conn).await;
    if last {
        session.final_save().await;
        // Takes the session rather than its key: a frontmatter rename can move
        // the permalink this connection joined under while it was open.
        sessions.dispose_if_empty(session).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::try_from(*name).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    /// The whole Origin rule: the header is required, its authority must be
    /// the Host this very request named, and nothing widens that.
    #[test]
    fn only_the_requests_own_host_may_open_a_session() {
        let ok = |pairs: &[(&str, &str)]| require_same_host_origin(&headers(pairs)).is_ok();

        assert!(ok(&[
            ("host", "127.0.0.1:8765"),
            ("origin", "http://127.0.0.1:8765")
        ]));
        assert!(ok(&[
            ("host", "notes.example"),
            ("origin", "https://notes.example")
        ]));
        // A host is case-insensitive, so the browser's lowercased Origin still
        // matches a Host the user typed with capitals.
        assert!(ok(&[
            ("host", "LocalHost:8765"),
            ("origin", "http://localhost:8765")
        ]));

        // Missing entirely: refused, never waved through.
        assert!(!ok(&[("host", "notes.example")]));
        // Another origin, a bare host, an opaque origin, a port mismatch, a
        // prefix that only looks like the host, and no Host at all.
        for origin in [
            "http://evil.example",
            "notes.example",
            "null",
            "http://notes.example:8765",
            "http://notes.example.evil",
            "https://evil.example/?x=http://notes.example",
        ] {
            assert!(
                !ok(&[("host", "notes.example"), ("origin", origin)]),
                "{origin} must not open a session on notes.example"
            );
        }
        assert!(!ok(&[("origin", "http://notes.example")]));
    }

    /// Capacity is a 503 and the two content refusals keep their own statuses,
    /// so a client can tell "try again later" from "this file cannot".
    #[test]
    fn every_join_refusal_maps_to_the_status_its_client_branches_on() {
        use axum::http::StatusCode;
        assert_eq!(
            join_error(JoinError::MixedEndings).status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            join_error(JoinError::ServerFull).status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            join_error(JoinError::SessionFull).status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            join_error(JoinError::Engine(crate::engine::EngineError::NotFound(
                "no engram".into()
            )))
            .status,
            StatusCode::NOT_FOUND
        );
    }

    /// The closing arm fires on the `Closed` control and on nothing else: an
    /// ordinary control or update must not tear a socket down.
    #[test]
    fn only_the_closed_control_ends_every_connection() {
        assert!(is_session_closed(&control::encode(&Control::Closed {
            reason: "deleted".to_string(),
        })));
        assert!(is_session_closed(&control::encode(&Control::Closed {
            reason: "internal".to_string(),
        })));
        assert!(!is_session_closed(&control::encode(&Control::Merged)));
        assert!(!is_session_closed(&control::encode(&Control::Saved {
            checksum: "abc".to_string(),
            permalink: "alpha".to_string(),
        })));
        assert!(!is_session_closed(&[0xff, 0xff, 0xff]));
    }
}
