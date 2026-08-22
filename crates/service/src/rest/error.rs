//! How a REST request fails, and how a conditional one succeeds without a
//! body: the shared vocabulary of failure and preconditions this surface
//! answers with.
//!
//! The failure half is [`ApiError`], the single error type every handler
//! returns, rendered as an RFC 9457 problem detail so a browser client can
//! branch on `status` alone. axum's own extractors reject in plain text, which
//! would put a second error shape on the wire for exactly the requests a
//! client is most likely to get wrong. [`ApiQuery`], [`ApiPath`] and
//! [`ApiJson`] wrap them so every rejection arrives as a problem detail too;
//! the router pairs them with a method-not-allowed fallback (see
//! [`ApiError::method_not_allowed`]) so the last plain-text answer axum can
//! produce is covered as well.
//!
//! The precondition half is the ETag machinery every conditional single
//! resource read and write shares, here rather than in one of the three route
//! modules that use it so all three agree by construction: [`if_match`] reads
//! the strong validator a write must carry, [`precondition_failed`] renders
//! the 412 it fails with (a [`ConflictDetail`], which carries the current
//! version so a client can merge), [`if_none_match_matches`] decides whether a
//! read may answer 304 and documents the shape of that 304, and [`REVALIDATE`]
//! is the `Cache-Control` both the 200 and the 304 carry.

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
    /// The one RFC 9457 extension member this surface sends. See
    /// [`ApiError::token_required`]; `None` on every other failure, and the
    /// member is then absent from the body entirely.
    pub token_required: Option<bool>,
}

impl ApiError {
    /// A 404 for a resource that does not exist.
    pub fn not_found(detail: impl Into<String>) -> ApiError {
        ApiError {
            status: StatusCode::NOT_FOUND,
            title: "not found",
            detail: detail.into(),
            token_required: None,
        }
    }

    /// A 401 for a request that carries no identity the server accepts. The
    /// browser client reads this as "show the login form".
    pub fn unauthorized(detail: impl Into<String>) -> ApiError {
        ApiError {
            status: StatusCode::UNAUTHORIZED,
            title: "unauthorized",
            detail: detail.into(),
            token_required: None,
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
            token_required: None,
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
            token_required: None,
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

    /// A 428 for a write that arrived without its `If-Match`. The detail names
    /// the header and where its token comes from (the detail read's ETag).
    pub fn precondition_required(detail: impl Into<String>) -> ApiError {
        ApiError {
            status: StatusCode::PRECONDITION_REQUIRED,
            title: "precondition required",
            detail: detail.into(),
            token_required: None,
        }
    }

    /// A 400 for a header the server can parse as HTTP but whose shape this
    /// surface refuses to accept - a comma-separated `If-Match` list, for
    /// instance, which RFC 9110 allows but this API's "exactly one strong
    /// checksum" contract does not.
    pub fn bad_request(detail: impl Into<String>) -> ApiError {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            title: "invalid request",
            detail: detail.into(),
            token_required: None,
        }
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
            token_required: None,
        }
    }

    /// Mark this problem document with the `token_required` extension member,
    /// for the one refusal that has a machine-readable answer to "what would
    /// make this work": `POST /auth/setup` refused for a non-local caller by an
    /// instance that actually holds a setup token.
    ///
    /// RFC 9457 extension members are the standard way to say something a
    /// client can act on without parsing prose, and this one exists because the
    /// first-run wizard must decide whether to render a token field at all. It
    /// is set ONLY when a token exists to be entered: an instance that has none
    /// (the loopback bind, which generates no token) refuses without the
    /// member, so the wizard never draws an input that cannot lead anywhere.
    /// The detail stays display-only copy either way.
    ///
    /// A member on [`ProblemDetail`] rather than a second problem type in
    /// [`ConflictDetail`]'s style: this one adds a flag to an ordinary
    /// refusal rather than a payload the caller has to be handed, and a handler
    /// returning `Result<_, ApiError>` can carry it through `?` without giving
    /// up the shared error type.
    pub fn token_required(mut self) -> ApiError {
        self.token_required = Some(true);
        self
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
            token_required: None,
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
    /// An RFC 9457 extension member, present only on the `403` of
    /// `POST /auth/setup` and only when this instance holds a setup token: the
    /// first-run wizard renders its token field on this member rather than on
    /// the detail prose, so an instance that has no token to enter (the
    /// loopback bind generates none) never causes a dead-end input to be
    /// drawn. Absent from every other problem document on this surface.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = true)]
    pub token_required: Option<bool>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ProblemDetail {
            problem_type: "about:blank",
            status: self.status.as_u16(),
            title: self.title,
            detail: self.detail,
            token_required: self.token_required,
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

/// The If-Match token: the unquoted strong checksum a write compares against.
///
/// 428 when the header is absent - the client forgot the contract, and the
/// answer says where the token comes from. `*`, a weak `W/` validator and an
/// empty token are 422: this surface versions by strong content checksum only,
/// and matching "any version" would make If-Match decorative. A
/// comma-separated list (`"a", "b"`) is RFC 9110-legal but 400: naively
/// trimming quotes off it would silently mangle it into a malformed token
/// instead of refusing it, and this surface's tokens are hex, so a comma can
/// never appear in a legitimate one.
pub fn if_match(headers: &axum::http::HeaderMap) -> Result<String, ApiError> {
    let raw = headers
        .get(axum::http::header::IF_MATCH)
        .ok_or_else(|| {
            ApiError::precondition_required(
                "this write requires an If-Match header carrying the ETag \
                 from the detail read, so a stale save is refused instead of \
                 clobbering someone else's change",
            )
        })?
        .to_str()
        .map_err(|_| ApiError::unprocessable("the If-Match header is not readable text"))?
        .trim();
    if raw.contains(',') {
        return Err(ApiError::bad_request(
            "the If-Match header carries more than one entity tag; this \
             surface expects exactly one strong checksum from the detail \
             read, not a comma-separated list",
        ));
    }
    if raw == "*" || raw.starts_with("W/") {
        return Err(ApiError::unprocessable(
            "If-Match must carry the strong content checksum from the detail \
             read, not a wildcard or a weak validator",
        ));
    }
    let token = raw.trim_matches('"');
    if token.is_empty() {
        return Err(ApiError::unprocessable("the If-Match token is empty"));
    }
    Ok(token.to_string())
}

/// `Cache-Control` for every conditional single-resource read, sent on both
/// the 200 and the 304: store it, but revalidate before every use.
///
/// With no `Cache-Control`, no `Expires` and no `Last-Modified`, RFC 9111
/// section 4.2.2 lets a cache invent a heuristic freshness lifetime and reuse a
/// stored response with no request to this server at all - which skips
/// `If-None-Match` entirely rather than skipping only the body, so a save
/// elsewhere would go unnoticed by an already-cached reader. `no-cache`
/// despite its name means "store it, revalidate it every time", not "do not
/// store it": with the strong `ETag` these reads already carry, the
/// revalidation costs one cheap 304 and no body. The 304 repeats the header
/// too, since a 304 updates the stored response's own headers and dropping it
/// there would let the very response it refreshes go heuristically fresh.
pub const REVALIDATE: &str = "no-cache";

/// Whether the request's `If-None-Match` covers the given strong validator.
///
/// Deliberately more forgiving than [`if_match`], which guards a write: this
/// one only decides whether to resend bytes the client may already have, so a
/// list of candidates, a `*` wildcard and a weak validator are all honoured
/// rather than refused. Nothing is lost by being wrong in the permissive
/// direction either, since the worst outcome is a full response the client
/// discards.
///
/// # The 304 this gates
///
/// The canonical statement of that response's shape, kept here because three
/// routes build the same one (the manifest, an engram and an attachment) and
/// each of them points at this paragraph rather than repeating it. RFC 9110: a
/// 304 carries the validator it matched and no body, so the client goes on
/// caching under the same token; and it repeats [`REVALIDATE`] as well, since
/// a 304 updates the stored response's own headers and dropping the directive
/// there would let the very response it refreshes turn heuristically fresh.
pub fn if_none_match_matches(headers: &axum::http::HeaderMap, checksum: &str) -> bool {
    let Some(raw) = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    raw.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*"
            || candidate
                .strip_prefix("W/")
                .unwrap_or(candidate)
                .trim_matches('"')
                == checksum
    })
}

/// The wire form of a 412: a problem detail carrying the version the server
/// holds now, so a client can show a merge view instead of just failing.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ConflictDetail {
    #[serde(rename = "type")]
    pub problem_type: &'static str, // "about:blank"
    pub status: u16,         // 412
    pub title: &'static str, // "precondition failed"
    pub detail: String,
    /// The ETag of the version the server holds now, quoted.
    pub current_etag: String,
    /// The full markdown the server holds now, so a client can merge.
    pub current_content: String,
}

/// A 412 for a write whose `If-Match` no longer matches the server's copy,
/// carrying that copy so the caller can merge instead of retrying blind.
pub fn precondition_failed(detail: String, checksum: &str, content: String) -> Response {
    let body = ConflictDetail {
        problem_type: "about:blank",
        status: StatusCode::PRECONDITION_FAILED.as_u16(),
        title: "precondition failed",
        detail,
        current_etag: format!("\"{checksum}\""),
        current_content: content,
    };
    let mut resp = (StatusCode::PRECONDITION_FAILED, axum::Json(body)).into_response();
    // `axum::Json` writes `application/json`; RFC 9457 requires the problem
    // media type, so the header is replaced rather than appended.
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/problem+json"),
    );
    resp
}

impl From<EngineError> for ApiError {
    /// Mirror `mcp::to_error`'s classification, projected onto HTTP: the
    /// variants it calls caller errors split into "the thing is not there"
    /// (404) and "the request was wrong" (422), and the variants it calls
    /// internal errors become 500. `ReadOnly` is the one divergence from
    /// `mcp::to_error`: HTTP has a status for "the server knows you and
    /// refuses" that MCP does not reach for, and a browser client already
    /// learns `read_only` from `/auth/me`, so this surface answers 403
    /// instead of folding it into the generic 422 caller-error class. The
    /// match is exhaustive so a new variant must be classified here instead
    /// of silently defaulting.
    fn from(e: EngineError) -> ApiError {
        let detail = e.to_string();
        match e {
            EngineError::UnknownDomain { .. } | EngineError::NotFound(_) => {
                ApiError::not_found(detail)
            }
            EngineError::ReadOnly => ApiError::forbidden(detail),
            EngineError::Ambiguous(_)
            | EngineError::Conflict(_)
            | EngineError::Invalid(_)
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
        | RemoteError::NoWithdrawTarget { .. }
        | RemoteError::ConflictsPending { .. } => unprocessable_error(detail),
    }
}

/// A 422 for a request the server understood but cannot act on.
fn unprocessable_error(detail: String) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        title: "invalid request",
        detail,
        token_required: None,
    }
}

/// A 500 for a failure that is not the caller's fault.
fn internal_error(detail: String) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        title: "internal error",
        detail,
        token_required: None,
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

    /// The write-endpoint refinement: a read-only instance answers 403, matching
    /// the /auth/me read_only flag a client already branches on. MCP keeps its own
    /// classification; this is the HTTP projection only.
    #[test]
    fn read_only_is_forbidden_on_this_surface() {
        let api = ApiError::from(EngineError::ReadOnly);
        assert_eq!(api.status, StatusCode::FORBIDDEN);
        assert!(api.detail.contains("read-only"), "{}", api.detail);
    }

    /// If-Match parsing: 428 without the header, the unquoted checksum with it,
    /// and the shapes RFC 9110 allows but a strong-checksum contract refuses.
    #[test]
    fn if_match_demands_one_quoted_strong_validator() {
        use axum::http::{HeaderMap, HeaderValue, header};
        let with = |raw: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::IF_MATCH, HeaderValue::from_str(raw).unwrap());
            h
        };

        assert_eq!(
            if_match(&HeaderMap::new()).unwrap_err().status,
            StatusCode::PRECONDITION_REQUIRED
        );
        assert_eq!(if_match(&with("\"abc123\"")).unwrap(), "abc123");
        assert_eq!(
            if_match(&with("abc123")).unwrap(),
            "abc123",
            "quotes optional on the way in"
        );
        for bad in ["*", "W/\"abc\"", "\"\"", ""] {
            assert_eq!(
                if_match(&with(bad)).unwrap_err().status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{bad:?} is not a strong checksum"
            );
        }
    }

    /// RFC 9110 allows a comma-separated `If-Match` list; this surface refuses
    /// it outright (400) rather than mangling it into a bogus single token by
    /// trimming quotes off the whole thing.
    #[test]
    fn if_match_rejects_a_comma_separated_list() {
        use axum::http::{HeaderMap, HeaderValue, header};
        let mut h = HeaderMap::new();
        h.insert(
            header::IF_MATCH,
            HeaderValue::from_str("\"abc123\", \"def456\"").unwrap(),
        );
        let err = if_match(&h).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(
            err.detail.contains("one") && err.detail.to_lowercase().contains("if-match"),
            "the detail says one ETag is expected: {}",
            err.detail
        );
    }

    /// `if_none_match_matches` honours every form a real cache sends, unlike
    /// `if_match`'s strict contract - a list, a wildcard and a weak validator
    /// all count, since the worst outcome of being wrong here is a full
    /// response the client discards rather than a clobbered write.
    #[test]
    fn if_none_match_honours_the_forms_a_cache_sends() {
        use axum::http::{HeaderMap, header};
        let with = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, value.parse().unwrap());
            if_none_match_matches(&headers, "abc123")
        };
        assert!(with("\"abc123\""));
        assert!(with("*"), "a wildcard asks about any version at all");
        assert!(with("W/\"abc123\""), "a weak validator still identifies it");
        assert!(with("\"other\", \"abc123\""), "one of a list is enough");
        assert!(!with("\"other\""));
        assert!(!with(""));
        assert!(
            !if_none_match_matches(&HeaderMap::new(), "abc123"),
            "no header asks for the bytes"
        );
    }

    /// The 412 payload is a problem detail with extension members, sent as
    /// application/problem+json like every other failure here.
    #[tokio::test]
    async fn a_precondition_failure_carries_the_current_version() {
        let resp = precondition_failed(
            "the engram changed since it was read".to_string(),
            "abc123",
            "---\ntitle: Now\n---\n".to_string(),
        );
        assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(
            resp.headers()[axum::http::header::CONTENT_TYPE],
            "application/problem+json"
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], 412);
        assert_eq!(body["title"], "precondition failed");
        assert_eq!(body["current_etag"], "\"abc123\"");
        assert!(
            body["current_content"]
                .as_str()
                .unwrap()
                .contains("title: Now")
        );
    }

    /// The one extension member: present when a refusal was asked to carry it,
    /// and absent from the body entirely otherwise - not `null`, which a client
    /// checking for the key would have to special-case.
    #[tokio::test]
    async fn the_token_required_member_appears_only_when_it_is_set() {
        let body = |error: ApiError| async {
            let resp = error.into_response();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        };

        let plain = body(ApiError::forbidden("no")).await;
        assert!(
            plain.get("token_required").is_none(),
            "an ordinary refusal carries no extension member: {plain}"
        );
        let marked = body(ApiError::forbidden("no").token_required()).await;
        assert_eq!(marked["token_required"], true);
        assert_eq!(marked["status"], 403, "and the rest of the shape is intact");
        assert_eq!(marked["title"], "forbidden");
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
