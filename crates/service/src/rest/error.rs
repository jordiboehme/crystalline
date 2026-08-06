//! The single error type every REST handler returns, rendered as an RFC 9457
//! problem detail so a browser client can branch on `status` alone.
//!
//! axum's own extractors reject in plain text, which would put a second error
//! shape on the wire for exactly the requests a client is most likely to get
//! wrong. [`ApiQuery`], [`ApiPath`] and [`ApiJson`] wrap them so every
//! rejection arrives as a problem detail too; the router pairs them with a
//! method-not-allowed fallback (see [`ApiError::method_not_allowed`]) so the
//! last plain-text answer axum can produce is covered as well.

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

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

    /// A 409 for a request the server understood, is allowed to serve, and
    /// refuses because carrying it out would break a rule about the state
    /// rather than about the request: a name already taken, or an edit that
    /// would leave the installation without an admin. Nothing about the request
    /// can be corrected to make it succeed - something else has to change
    /// first, which is what separates it from [`ApiError::unprocessable`].
    pub fn conflict(detail: impl Into<String>) -> ApiError {
        ApiError {
            status: StatusCode::CONFLICT,
            title: "conflict",
            detail: detail.into(),
        }
    }

    /// A 422 for a request the server understood but cannot act on, matching
    /// the status the engine's own caller errors take.
    pub fn unprocessable(detail: impl Into<String>) -> ApiError {
        unprocessable_error(detail.into())
    }

    /// A 500 for a failure that is not the caller's fault.
    pub fn internal(detail: impl Into<String>) -> ApiError {
        internal_error(detail.into())
    }

    /// A 405 for a path that exists but does not serve this method. Mounted as
    /// the router's `method_not_allowed_fallback`, which is the one answer axum
    /// would otherwise produce with an empty body; the `Allow` header axum adds
    /// on the way out survives, so the response still names what does work.
    pub fn method_not_allowed() -> ApiError {
        ApiError {
            status: StatusCode::METHOD_NOT_ALLOWED,
            title: "method not allowed",
            detail: "this path does not serve that method".to_string(),
        }
    }

    /// Re-render an axum extractor rejection as a problem detail.
    ///
    /// The status axum chose is kept rather than folded into one of this
    /// module's own: its rejections already distinguish unparseable from
    /// well-formed-but-wrong (400 against 422) and a missing JSON content type
    /// (415), and that classification is more specific than anything this layer
    /// could recover after the fact. Only the rendering changes. The two
    /// accessors each wrapper reads it with are inherent methods on unrelated
    /// rejection types rather than one shared trait, so the call sites repeat
    /// rather than routing through a seam that would only exist to hide three
    /// identical lines.
    fn from_rejection(status: StatusCode, detail: String) -> ApiError {
        let title = match status {
            StatusCode::METHOD_NOT_ALLOWED => "method not allowed",
            StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported media type",
            StatusCode::PAYLOAD_TOO_LARGE => "payload too large",
            s if s.is_server_error() => "internal error",
            _ => "invalid request",
        };
        ApiError {
            status,
            title,
            detail,
        }
    }
}

/// [`axum::extract::Query`] with this module's rejection contract: a query
/// string that will not deserialize answers in problem+json instead of text.
pub struct ApiQuery<T>(
    /// The deserialized query parameters.
    pub T,
);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<ApiQuery<T>, ApiError> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(ApiQuery(value)),
            Err(rejection) => Err(ApiError::from_rejection(
                rejection.status(),
                rejection.body_text(),
            )),
        }
    }
}

/// [`axum::extract::Path`] with this module's rejection contract.
pub struct ApiPath<T>(
    /// The deserialized path parameters.
    pub T,
);

impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequestParts<S> for ApiPath<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<ApiPath<T>, ApiError> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(ApiPath(value)),
            Err(rejection) => Err(ApiError::from_rejection(
                rejection.status(),
                rejection.body_text(),
            )),
        }
    }
}

/// [`axum::Json`] with this module's rejection contract, for request bodies. A
/// response still uses `axum::Json`, which never rejects.
pub struct ApiJson<T>(
    /// The deserialized body.
    pub T,
);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<ApiJson<T>, ApiError> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(ApiError::from_rejection(
                rejection.status(),
                rejection.body_text(),
            )),
        }
    }
}

/// The wire form of an [`ApiError`]: the RFC 9457 body every failure on this
/// surface carries, and the one schema the OpenAPI document names for every
/// error response.
///
/// Written as a type rather than as an inline `json!` so the document and the
/// response are the same definition: a field renamed here changes both at once,
/// which is the whole reason a generated client can trust the error shape.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "An RFC 9457 problem detail, sent as \
                        `application/problem+json`. Every failure on this \
                        surface has this shape, so a client can branch on \
                        `status` alone.")]
pub struct ProblemDetail {
    /// The problem type URI. Always `about:blank`: `status` and `title` carry
    /// the classification, and this surface publishes no per-problem pages to
    /// point at.
    #[serde(rename = "type")]
    #[schema(example = "about:blank")]
    pub problem_type: &'static str,
    /// The HTTP status, mirrored into the body so a client that only has the
    /// parsed payload can still branch on it.
    #[schema(example = 404)]
    pub status: u16,
    /// A short, stable, human-readable summary of the problem type.
    #[schema(example = "not found")]
    pub title: &'static str,
    /// The specific occurrence, safe to show to the caller.
    #[schema(example = "no engram 'ghost' in domain 'eng'")]
    pub detail: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ProblemDetail {
            problem_type: "about:blank",
            status: self.status.as_u16(),
            title: self.title,
            detail: self.detail,
        };
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
            | EngineError::EnvTokenConnect => unprocessable_error(detail),
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
        | RemoteError::ConflictsPending { .. } => unprocessable_error(detail),
    }
}

/// A 422 for a request the server understood but cannot act on.
fn unprocessable_error(detail: String) -> ApiError {
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

    /// An axum rejection keeps the status axum chose and gains a title from
    /// this module's stable set, so a client can branch on either.
    #[test]
    fn a_rejection_keeps_its_status_and_gains_a_title() {
        let title = |status| ApiError::from_rejection(status, "why".to_string()).title;
        assert_eq!(title(StatusCode::BAD_REQUEST), "invalid request");
        assert_eq!(title(StatusCode::UNPROCESSABLE_ENTITY), "invalid request");
        assert_eq!(
            title(StatusCode::UNSUPPORTED_MEDIA_TYPE),
            "unsupported media type"
        );
        assert_eq!(title(StatusCode::PAYLOAD_TOO_LARGE), "payload too large");
        assert_eq!(
            title(StatusCode::METHOD_NOT_ALLOWED),
            "method not allowed",
            "the same title the router's own 405 carries"
        );
        assert_eq!(title(StatusCode::INTERNAL_SERVER_ERROR), "internal error");

        let rendered = ApiError::from_rejection(StatusCode::BAD_REQUEST, "why".to_string());
        assert_eq!(rendered.status, StatusCode::BAD_REQUEST);
        assert_eq!(rendered.detail, "why", "axum's own message is kept");
        assert_eq!(ApiError::method_not_allowed().title, "method not allowed");
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
