//! The single error type every REST handler returns, rendered as an RFC 9457
//! problem detail so a browser client can branch on `status` alone.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::engine::EngineError;

/// An error response in RFC 9457 shape. `title` is a short stable slug a
/// client may match on; `detail` carries the engine's own message verbatim,
/// which is already actionable product copy.
#[derive(Debug)]
pub struct ApiError {
    /// The HTTP status, also mirrored into the body's `status` member.
    pub status: StatusCode,
    /// A short, stable, human-readable summary of the problem type.
    pub title: &'static str,
    /// The specific occurrence, safe to show to the caller.
    pub detail: String,
}

impl ApiError {
    /// A 404 for a resource that does not exist.
    pub fn not_found(detail: impl Into<String>) -> ApiError {
        ApiError {
            status: StatusCode::NOT_FOUND,
            title: "not found",
            detail: detail.into(),
        }
    }

    /// A 401 for a request that carries no identity the server accepts. The
    /// browser client reads this as "show the login form".
    pub fn unauthorized(detail: impl Into<String>) -> ApiError {
        ApiError {
            status: StatusCode::UNAUTHORIZED,
            title: "unauthorized",
            detail: detail.into(),
        }
    }

    /// A 403 for a caller the server knows and will not serve this to: an
    /// account without the role, a disabled account, or a mutating request
    /// missing its CSRF token. Logging in again cannot help, which is what
    /// separates it from [`ApiError::unauthorized`].
    pub fn forbidden(detail: impl Into<String>) -> ApiError {
        ApiError {
            status: StatusCode::FORBIDDEN,
            title: "forbidden",
            detail: detail.into(),
        }
    }

    /// A 500 for a failure that is not the caller's fault.
    pub fn internal(detail: impl Into<String>) -> ApiError {
        internal_error(detail.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "type": "about:blank",
            "status": self.status.as_u16(),
            "title": self.title,
            "detail": self.detail,
        });
        let mut resp = (self.status, axum::Json(body)).into_response();
        // `axum::Json` writes `application/json`; RFC 9457 requires the
        // problem media type, so the header is replaced rather than appended.
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/problem+json"),
        );
        resp
    }
}

impl From<EngineError> for ApiError {
    /// Mirror `mcp::to_error`'s classification, projected onto HTTP: the
    /// variants it calls caller errors split into "the thing is not there"
    /// (404) and "the request was wrong" (422), and the variants it calls
    /// internal errors become 500. `ReadOnly` stays in the caller-error class
    /// it has on the MCP side rather than becoming a 403, so the two surfaces
    /// keep one classification; the task that adds write endpoints owns any
    /// refinement. The match is exhaustive so a new variant must be
    /// classified here instead of silently defaulting.
    fn from(e: EngineError) -> ApiError {
        let detail = e.to_string();
        match e {
            EngineError::UnknownDomain { .. } | EngineError::NotFound(_) => {
                ApiError::not_found(detail)
            }
            EngineError::Ambiguous(_)
            | EngineError::Conflict(_)
            | EngineError::Invalid(_)
            | EngineError::ReadOnly
            | EngineError::EnvTokenConnect => unprocessable(detail),
            EngineError::Remote(remote) => remote_to_api_error(remote, detail),
            EngineError::Io { .. } | EngineError::Internal(_) => internal_error(detail),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    /// Classify an engine error that has already been erased into `anyhow`,
    /// so a handler can use `?` on either error type. An error that is not an
    /// `EngineError` has no caller-fault claim to make and stays a 500.
    fn from(e: anyhow::Error) -> ApiError {
        match e.downcast::<EngineError>() {
            Ok(engine) => engine.into(),
            Err(other) => internal_error(other.to_string()),
        }
    }
}

/// Split a collaboration error the way `mcp::remote_to_error` does: transient
/// or environmental variants are never the caller's mistake and stay 500,
/// genuine input problems become 422, and a repository or proposal that does
/// not exist becomes a 404.
fn remote_to_api_error(e: crystalline_remote::RemoteError, detail: String) -> ApiError {
    use crystalline_remote::RemoteError;
    match e {
        RemoteError::Offline
        | RemoteError::RateLimited { .. }
        | RemoteError::AuthExpired
        | RemoteError::AuthPending
        | RemoteError::Api { .. }
        | RemoteError::Io(_)
        | RemoteError::State(_)
        | RemoteError::Credential { .. }
        | RemoteError::BaseUnavailable => internal_error(detail),
        RemoteError::RepoNotFound { .. }
        | RemoteError::NotADomain { .. }
        | RemoteError::ProposalNotFound { .. }
        | RemoteError::ConflictNotFound { .. } => ApiError::not_found(detail),
        RemoteError::NotEnabled
        | RemoteError::NotConnected
        | RemoteError::ConflictsPending { .. } => unprocessable(detail),
    }
}

/// A 422 for a request the server understood but cannot act on.
fn unprocessable(detail: String) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        title: "invalid request",
        detail,
    }
}

/// A 500 for a failure that is not the caller's fault.
fn internal_error(detail: String) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        title: "internal error",
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_things_are_404_and_bad_requests_are_422() {
        let unknown_domain = EngineError::UnknownDomain {
            domain: "nope".into(),
            registered: vec!["eng".into()],
        };
        assert_eq!(ApiError::from(unknown_domain).status, StatusCode::NOT_FOUND);
        assert_eq!(
            ApiError::from(EngineError::NotFound("no engram".into())).status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::from(EngineError::Invalid("bad depth".into())).status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            ApiError::from(EngineError::Internal("store blew up".into())).status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn an_anyhow_error_keeps_its_engine_classification() {
        let erased: anyhow::Error = EngineError::NotFound("no engram".into()).into();
        let api = ApiError::from(erased);
        assert_eq!(api.status, StatusCode::NOT_FOUND);
        assert_eq!(api.detail, "no engram");

        let plain = ApiError::from(anyhow::anyhow!("something else"));
        assert_eq!(plain.status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
