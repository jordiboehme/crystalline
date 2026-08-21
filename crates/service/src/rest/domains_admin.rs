//! Domain lifecycle (admin): create in any of the three modes, unregister, and
//! a team domain's sync status and manual pull.
//! REST is the untrusted surface of the engine verbs MCP and the CLI already
//! use: names are validated here (an operator typing at the CLI is trusted; a
//! browser is not), and a local domain is always created under the configured
//! domains root - no folder parameter exists on this surface, by design.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

use super::auth::Identity;
use super::{ApiError, ApiJson, ApiPath, ProblemDetail, RestState, refuse_read_only};
use crate::engine::EngineError;

/// What `POST /domains` takes: a mode and whatever that mode needs.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "A domain to register. `mode` picks which of the three \
                        kinds is meant, and every other field belongs to one \
                        of them: a field that does not apply to the mode \
                        asked for is refused rather than ignored.")]
pub struct CreateDomainBody {
    /// local | virtual | github
    #[schema(example = "local")]
    pub mode: String,
    /// The domain name. Required for local and virtual; optional for github
    /// (defaults to the repository name). A local domain always lands at
    /// <domains_root>/<name>; there is no folder parameter on this surface.
    #[serde(default)]
    #[schema(example = "notes")]
    pub name: Option<String>,
    /// owner/name; github mode only.
    #[serde(default)]
    #[schema(example = "acme/knowledge")]
    pub repo: Option<String>,
    /// Branch to track; github mode only, defaults to the repo default.
    #[serde(default)]
    #[schema(example = "main")]
    pub branch: Option<String>,
    /// Subfolder within the repository; github mode only.
    #[serde(default)]
    #[schema(example = "domains/eng")]
    pub path: Option<String>,
}

/// A domain name that is safe as a path segment under the domains root, in
/// crystalline:// addresses, and on every operating system this server may
/// run on - not merely the one whoever last touched this file was using.
///
/// Enforced, as an ALLOWLIST rather than a denylist: every character must be
/// a Unicode alphanumeric (`char::is_alphanumeric`) or one of `-`, `_`, `.`.
/// A denylist has to remember every hostile character; an allowlist refuses
/// everything it does not name, which is why the Windows-illegal punctuation
/// `* ? < > | " :`, every path separator and all whitespace (including the
/// hostile invisibles - U+202E, U+200B, U+200D are category Cf, which
/// `is_alphanumeric` excludes) fall out of this rule for free rather than
/// needing their own line. On top of the allowlist: a 64-CHARACTER cap
/// (counted, not bytes - the cap is about legibility, a readable folder
/// segment and a readable piece of a `crystalline://` address, and claims no
/// filesystem guarantee; the OS's own segment limit is separate, larger in
/// every realistic case, and enforced by the OS with its own error); no
/// leading dot (a hidden file) and, the same hazard class as this whole
/// item, no trailing dot either (Windows strips one silently); and a refusal
/// of the Windows RESERVED DEVICE NAMES (see [`is_windows_device_name`]),
/// because an allowlist of alphanumerics cannot see that class at all - it
/// is illegal regardless of which characters make it up.
///
/// Trailing spaces, the other Windows hazard, are already closed by the
/// `trim()` below (pinned by the `" notes "` test case) - keep it, even
/// though most of what it catches the allowlist would also refuse on its
/// own.
///
/// Deliberately NOT defended, because closing it costs more than the gap is
/// worth: a decomposed (NFD) name - what macOS input and pasted macOS
/// filenames often produce - is refused outright, since a combining mark is
/// category Mn and not alphanumeric; normalizing to NFC first would need a
/// dependency this change does not take. Homoglyph confusables (Cyrillic
/// `а` beside Latin `a`) stay open, as does two distinct registered names
/// colliding on one directory on a case- or normalization-insensitive
/// filesystem (APFS, NTFS). Neither is a new hole: nothing screens either
/// today.
///
/// Nothing here re-validates an already-registered name: this function runs
/// only when a domain is CREATED (the three arms of `create` below). A
/// domain an earlier CLI call or a hand-edited config registered under a
/// name this function would now refuse keeps serving, keeps listing and can
/// still be unregistered from the browser; only a NEW registration through
/// this REST surface is refused. No migration, no lazy rename - there is no
/// backcompat obligation to keep.
fn check_domain_name(name: &str) -> Result<&str, ApiError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::unprocessable("the domain name is empty"));
    }
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    let allowed = trimmed.chars().count() <= 64
        && !trimmed.starts_with('.')
        && !trimmed.ends_with('.')
        && trimmed
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !is_windows_device_name(stem);
    if !allowed {
        return Err(ApiError::unprocessable(format!(
            "'{trimmed}' cannot name a domain: use letters, digits, hyphens, \
             underscores and dots (no leading or trailing dot), 64 \
             characters or fewer, and not a Windows device name (CON, PRN, \
             AUX, NUL, COM1-COM9, LPT1-LPT9)"
        )));
    }
    Ok(trimmed)
}

/// Whether `stem` - the segment before the first dot, so `CON.txt` is
/// checked as `CON` while `a.CON` is checked as `a` - names a Windows
/// reserved device: `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`,
/// matched case-insensitively (ASCII case only - the reserved names are
/// ASCII, so a non-ASCII lookalike is correctly left alone), plus the
/// superscript-digit spellings `COM\u{b9}`/`COM\u{b2}`/`COM\u{b3}` and their
/// LPT equivalents (U+00B9, U+00B2, U+00B3), which Windows resolves to the
/// same device as the ASCII digit and which sail through a bare
/// `is_alphanumeric` allowlist because Unicode category No (\"other
/// number\") counts as numeric. `COM10` and up are NOT reserved (they need
/// the `\\.\` device syntax), so `console`, `com10` and `a.CON` all stay
/// legal - this only ever matches an EXACT stem, never a prefix.
fn is_windows_device_name(stem: &str) -> bool {
    let mut chars = stem.chars();
    let head: String = chars
        .by_ref()
        .take(3)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let rest: Vec<char> = chars.collect();
    match (head.as_str(), rest.as_slice()) {
        ("CON" | "PRN" | "AUX" | "NUL", []) => true,
        ("COM" | "LPT", [c]) => {
            c.is_ascii_digit() && *c != '0' || matches!(c, '\u{b9}' | '\u{b2}' | '\u{b3}')
        }
        _ => false,
    }
}

/// `POST /domains` - register a domain: a local folder under the server's
/// domains root, a database-backed virtual one, or a GitHub team domain.
///
/// The local mode is deliberately NAME-ONLY. A browser client cannot name a
/// folder on the server, so the only path this surface can produce is
/// `<domains_root>/<name>`, and traversal is closed by construction rather
/// than by sanitizing a path: the name is checked to be one plain segment
/// ([`check_domain_name`]) and the engine joins it under the configured root.
/// An operator who does need a specific folder has the `crystalline` CLI,
/// which is trusted in a way an HTTP body is not.
///
/// The github mode pre-checks readiness and refuses with a 409 pointing at
/// the settings screen when no credential is on file, rather than letting the
/// engine fail deep inside a download. It is a SYNCHRONOUS registration: the
/// repository is fetched and indexed inside this request, so a large team
/// repository holds the response open. That is accepted for this slice; a
/// progress surface over `origin_add_with_progress` is a follow-up.
#[utoipa::path(
    post,
    path = "/api/v1/domains",
    tag = "domains",
    operation_id = "create_domain",
    summary = "Register a domain: local, virtual or a GitHub team domain.",
    description = "Admin only. A local domain is name-only and always lands \
                   at `<domains_root>/<name>` on the server, so no request \
                   can place a folder anywhere else; a virtual domain lives \
                   in the database; a team domain needs a GitHub connection \
                   and downloads the repository inside this request, which \
                   can take a while for a large one.",
    request_body = CreateDomainBody,
    responses(
        (
            status = 201,
            description = "The engine's own registration report, unchanged. \
                           Its shape follows the mode: a local domain reports \
                           where it landed, a virtual one reports that it was \
                           registered, a team domain reports what was fetched.",
            body = Object,
            example = json!({
                "domain": "notes",
                "root": "/Users/ada/Documents/Crystalline/notes",
                "kind": "file",
                "manifest_created": true,
                "adopted": false
            }),
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The name is taken by another domain, or mode \
                           `github` was asked for on an instance with no \
                           GitHub connection - the detail says where to make \
                           one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 415,
            description = "The body is not `application/json`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "An unknown mode, a name that could escape the \
                           domains root, or a field that does not belong to \
                           the mode asked for.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn create(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<CreateDomainBody>,
) -> Result<Response, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    // Serialized against a concurrent unregister of the same name: see
    // [`RestState::domain_admin`] for the engine-level race this closes.
    let _admin = state.domain_admin().await;
    let report = match body.mode.as_str() {
        "local" => {
            let name = check_domain_name(body.name.as_deref().unwrap_or_default())?;
            require_absent(&body.repo, "repo", "local")?;
            require_absent(&body.branch, "branch", "local")?;
            require_absent(&body.path, "path", "local")?;
            state.engine.domain_add_local(Some(name), None).await
        }
        "virtual" => {
            let name = check_domain_name(body.name.as_deref().unwrap_or_default())?;
            require_absent(&body.repo, "repo", "virtual")?;
            require_absent(&body.branch, "branch", "virtual")?;
            require_absent(&body.path, "path", "virtual")?;
            state.engine.domain_add_virtual(name).await
        }
        "github" => {
            let repo = body
                .repo
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .ok_or_else(|| {
                    ApiError::unprocessable("a team domain requires repo as owner/name")
                })?;
            // The TRIMMED name, and it has to be the one that travels on:
            // `origin_add` uses what it is handed verbatim as the config key
            // and as the folder segment under the domains root, so passing
            // the raw body field would register a domain named "\tnotes "
            // and create a folder to match - exactly what this validator
            // exists to prevent, and what the other two modes avoid by
            // passing on what `check_domain_name` gave back.
            let name = body.name.as_deref().map(check_domain_name).transpose()?;
            // Checked here rather than left to the engine: without a
            // connection the registration cannot work at all, and this way
            // the refusal names the screen that fixes it instead of arriving
            // as a generic remote failure.
            if !state.engine.github_ready().await {
                return Err(ApiError::conflict(
                    "GitHub is not connected on this instance: connect it under \
                     Settings > GitHub, then register the team domain",
                ));
            }
            state
                .engine
                .origin_add(
                    repo,
                    name,
                    body.path.as_deref(),
                    body.branch.as_deref(),
                    // Never from this surface: a team domain lands under the
                    // domains root like a local one, for the same reason.
                    None,
                )
                .await
        }
        other => {
            return Err(ApiError::unprocessable(format!(
                "mode must be local, virtual or github, got '{other}'"
            )));
        }
    };
    match report {
        Ok(report) => Ok((StatusCode::CREATED, Json(report)).into_response()),
        // A taken name (or an already-registered folder) is a conflict on
        // this surface, as on engram create; the generic From keeps 422 for
        // MCP's classification.
        Err(EngineError::Conflict(detail)) => Err(ApiError::conflict(detail)),
        Err(e) => Err(e.into()),
    }
}

/// A mode-mismatched field is a 422 up front, not silently ignored.
fn require_absent(field: &Option<String>, field_name: &str, mode: &str) -> Result<(), ApiError> {
    match field.as_deref().map(str::trim) {
        Some(v) if !v.is_empty() => Err(ApiError::unprocessable(format!(
            "{field_name} does not apply to a {mode} domain"
        ))),
        _ => Ok(()),
    }
}

/// `DELETE /domains/{domain}` - unregister a domain: the registration and the
/// index rows go, the files do not.
///
/// The order of the three steps below is the whole content of this handler,
/// and it is not free to rearrange (see [`crate::collab::session::CollabSessions::dispose_domain`],
/// which records the argument in full):
///
/// 1. The join fence goes up first, so no socket can open a room in this
///    domain from here on. Without it the sweep would close what is open and
///    a join arriving one instant later would open a fresh room over a domain
///    that is about to vanish.
/// 2. The rooms are swept while the domain is STILL registered, so each
///    room's final save lands in the file that stays on disk.
/// 3. Only then is the domain unregistered. Inverted, those final saves would
///    be refused outright or - inside the window between the config write and
///    the index clear - resolve as virtual and land in the DATABASE rather
///    than in the file `files_kept` promises was left alone.
///
/// The response is the engine's report plus `rooms_closed`, so a client can
/// say how many co-editing sessions it just ended.
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}",
    tag = "domains",
    operation_id = "unregister_domain",
    summary = "Unregister a domain. Files on disk are never touched.",
    description = "Admin only. The registration and the domain's index rows \
                   go; a file domain's files stay exactly where they are \
                   (re-adding the folder adopts them again), which is what \
                   `files_kept` reports. A virtual domain has no files, so \
                   `files_kept` is false and its knowledge is gone - a client \
                   must confirm that difference in words. Any open \
                   co-editing rooms in the domain are saved and closed first; \
                   `rooms_closed` counts them.",
    params(("domain" = String, Path, description = "The registered domain.")),
    responses(
        (
            status = 200,
            description = "The engine's own unregistration report, plus the \
                           number of co-editing rooms this call closed.",
            body = Object,
            example = json!({
                "domain": "eng",
                "unregistered": true,
                "files_kept": true,
                "index_cleared": true,
                "rooms_closed": 0
            }),
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The domain is defined by an environment variable, \
                           which owns it: unset the variable instead.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    // Step 1 and step 2 of this handler's ordering; see the doc comment.
    let _admin = state.domain_admin().await;
    let _fence = state.fence_joins().await;
    let rooms_closed = state.collab.dispose_domain(&domain).await;
    // Step 3, still behind both guards.
    let mut report = state.engine.domain_remove(&domain).await.map_err(|e| {
        match e {
            // An env-defined domain cannot be unregistered by anyone but the
            // environment: a conflict on this surface rather than the generic
            // 422, since no version of this request would succeed.
            EngineError::Conflict(detail) => ApiError::conflict(detail),
            other => other.into(),
        }
    })?;
    if let Value::Object(map) = &mut report {
        map.insert("rooms_closed".to_string(), Value::from(rooms_closed));
    }
    Ok(Json(report))
}

/// One domain's entry out of an aggregate origin report.
///
/// `origin_status` and `origin_update` both answer for a SET of domains -
/// `{ connection, domains: [...], errors: [...] }` - and collect a per-domain
/// failure into `errors` rather than failing the call, so one domain going
/// wrong never aborts the others. Addressed at a single domain in the path,
/// that collected failure IS this request's failure: it becomes a problem
/// detail carrying the engine's own message, never a 200 whose body quietly
/// says the sync did not happen. What comes back is the flat single-domain
/// object the card reads (`repo`, `branch`, `local_changes` and the rest).
fn single_domain(
    mut report: Value,
    domain: &str,
    what: &str,
) -> Result<Map<String, Value>, ApiError> {
    if let Some(Value::Array(domains)) = report.get_mut("domains")
        && let Some(Value::Object(entry)) = domains.first_mut()
    {
        return Ok(std::mem::take(entry));
    }
    let reason = report
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|errors| errors.first())
        .and_then(|error| error.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("the origin reported neither a result nor a reason")
        .to_string();
    Err(ApiError::internal(format!(
        "{what} for '{domain}' failed: {reason}"
    )))
}

/// `GET /domains/{domain}/sync` - where a team domain stands relative to its
/// origin: the repository and branch it tracks, how many local changes are
/// unshared, how many proposals are open, whether it is behind and when it was
/// last checked.
///
/// A pure read, so it is served on a read-only instance (that mirror still has
/// a sync card to show) - the engine documents `origin_status` as allowed
/// there, and the group's ruling is that read_only refuses writes, not reads.
///
/// A domain with no origin answers 404 rather than an empty status: the
/// resource "team sync status" does not exist for a local or virtual domain.
/// A client can therefore treat any 404 here as "show no card" while the
/// detail still says which of the two 404s it got.
///
/// A missing GitHub connection is NOT refused here, and that asymmetry with
/// the pull beside it is deliberate. `Engine::origin_status` is documented to
/// degrade rather than fail - no saved connection means `behind: None` and a
/// connection block reporting `connected: false`, never an error - and this
/// route exists to report state, so it reports that state instead of replacing
/// it with a refusal. A domain registered while connected and later
/// disconnected therefore still renders its card, showing what local state
/// knows and saying the connection is gone. The `connection` block travels out
/// with the flat per-domain report for exactly that reason: without it a card
/// could not tell "not connected" from "offline", since both leave `behind`
/// null.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/sync",
    tag = "domains",
    operation_id = "get_domain_sync_status",
    summary = "Where a team domain stands relative to its GitHub origin.",
    description = "Admin only. A pure read, served even on a read-only \
                   instance. Answers 404 for a domain with no origin - only \
                   a GitHub team domain has sync status - so a client can \
                   treat any 404 as `no sync card`. An instance with no \
                   GitHub connection is reported, not refused: the report \
                   comes back from local state with `connection.connected` \
                   false.",
    params(("domain" = String, Path, description = "The registered team domain.")),
    responses(
        (
            status = 200,
            description = "The engine's own status report for this one \
                           domain, plus the mode it is synced in and this \
                           instance's GitHub connection. `local_changes` is \
                           the unshared-work count a client shows as pending; \
                           `probe_error` is set when the live check could not \
                           reach GitHub and the rest of the report came from \
                           local state alone; `connection.connected` is false \
                           when no credential is on file, which is why a \
                           disconnected instance still answers here instead \
                           of refusing.",
            body = Object,
            example = json!({
                "domain": "eng",
                "mode": "github",
                "repo": "acme/knowledge",
                "branch": "main",
                "base_commit": "9f3c1a2",
                "behind": false,
                "local_changes": 2,
                "open_proposals": [],
                "declined_proposals": [],
                "conflicts": [],
                "last_checked": "2026-08-10T08:00:00Z",
                "probe_error": null,
                "connection": { "connected": true, "user": "octo", "token_store": "keychain" }
            }),
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or a domain with no team origin.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "GitHub is switched off on this instance, so no \
                           origin can be reached - the detail says where to \
                           turn it on. A missing connection is NOT refused \
                           here: the report comes back with \
                           `connection.connected` false instead.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn sync_status(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    identity.require_admin()?;
    // No refuse_read_only: this is a read. See the doc comment.
    // No connection check either: this route reports the connection rather
    // than refusing over it. See the doc comment.
    require_team_domain(&state, &domain, Refusal::Missing)?;
    let aggregate = state.engine.origin_status(Some(&domain)).await?;
    // Lifted before `single_domain` takes the per-domain entry, which is all
    // that survives of the aggregate.
    let connection = aggregate.get("connection").cloned();
    let mut report = single_domain(aggregate, &domain, "reading the origin status")?;
    // The engine's per-domain report already names the domain; the mode is
    // this surface's own, since a client that sees a sync card has to know
    // which kind of origin it is looking at.
    report.insert("mode".to_string(), Value::from("github"));
    // The connection rides along so a card can say WHY a degraded report is
    // degraded: `connected: false` is "connect GitHub", while a set
    // `probe_error` on a connected instance is "could not reach it".
    if let Some(connection) = connection {
        report.insert("connection".to_string(), connection);
    }
    Ok(Json(Value::Object(report)))
}

/// `POST /domains/{domain}/sync` - pull this domain's origin now, rather than
/// waiting for the daemon's next poll.
///
/// The pull writes: it applies what the origin has to this instance's copy, so
/// a read-only instance refuses it even though the status read beside it is
/// served.
///
/// A domain with no origin is a 409 rather than the GET's 404: the resource
/// addressed here is the ACTION, which exists on every domain path, and the
/// server state is what refuses it. That asymmetry is deliberate - a client
/// hides the card on a 404 and shows the reason on a 409.
///
/// A missing GitHub connection is a 409 here and only here. The pull cannot
/// degrade the way the status read beside it does: with no credential it has
/// nothing to pull WITH, and left to travel it comes back as the remote's own
/// "no such repository", which sends the reader hunting for a repository
/// problem that does not exist. Refusing up front names the connection and the
/// screen that fixes it, and a missing repository keeps the remote's own
/// `RemoteError::RepoNotFound` to itself, so the two read differently.
/// [`sync_status`] deliberately does NOT share this check.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/sync",
    tag = "domains",
    operation_id = "sync_domain",
    summary = "Pull a team domain's origin now.",
    description = "Admin only. Brings this instance's copy up to date with \
                   the domain's GitHub origin immediately, instead of waiting \
                   for the daemon's next poll: the same pull the poller runs, \
                   under the same per-domain lock. Refused on a read-only \
                   instance, and a conflict on a domain that has no origin to \
                   pull from or on an instance with no GitHub connection to \
                   pull through.",
    params(("domain" = String, Path, description = "The registered team domain.")),
    responses(
        (
            status = 200,
            description = "The engine's own pull report for this one domain: \
                           whether it was already up to date, which files were \
                           applied or merged, which conflicts are waiting and \
                           which proposals changed state.",
            body = Object,
            example = json!({
                "domain": "eng",
                "up_to_date": false,
                "applied": ["notes/a.md"],
                "merged": [],
                "conflicts": [],
                "proposals": [],
                "skipped_large": [],
                "re_baselined": false
            }),
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The domain has no team origin to pull from, GitHub \
                           is switched off on this instance, or it is on but \
                           no account is connected - the detail says which, \
                           and where to fix it. A repository that is simply \
                           missing is a different answer, carrying the \
                           remote's own not-found error.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn sync_now(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    require_team_domain(&state, &domain, Refusal::Conflict)?;
    // The pull's own gate, not the status read's: see the doc comment.
    if !state.engine.github_ready().await {
        return Err(ApiError::conflict(
            "GitHub is not connected on this instance: connect it under \
             Settings > GitHub, then retry the sync",
        ));
    }
    let report = single_domain(
        state.engine.origin_update(Some(&domain)).await?,
        &domain,
        "the pull",
    )?;
    Ok(Json(Value::Object(report)))
}

/// How a sync endpoint refuses a domain that is not a team domain: the status
/// resource does not exist there (GET), while the pull action exists on every
/// domain path and conflicts with the server's state (POST).
enum Refusal {
    Missing,
    Conflict,
}

/// The shared pre-check both sync endpoints open with: the domain exists (a
/// 404 straight out of the engine otherwise), it has an origin, and the
/// feature that reaches origins at all is on.
///
/// The GitHub check is here rather than left to the engine because both origin
/// verbs answer `RemoteError::NotEnabled`, which this surface classifies as a
/// bare 422: true, but useless to a card that would then have to explain a
/// state it cannot see. The order matters as much as the checks - a domain
/// with no origin is refused before the feature flag is consulted, so a local
/// domain reads as "no sync card" on every instance rather than flipping to a
/// GitHub complaint when someone turns the feature off.
///
/// What this does NOT check is whether an account is connected. The feature
/// flag is a property of the instance and reads the same to both verbs, while
/// a missing credential does not: the pull cannot proceed without one (see
/// [`sync_now`]) and the status read is expected to report the state instead
/// (see [`sync_status`]). Keeping the connection out of the shared helper is
/// what lets those two answers differ.
fn require_team_domain(state: &RestState, domain: &str, refusal: Refusal) -> Result<(), ApiError> {
    if !state.engine.domain_has_origin(domain)? {
        return Err(match refusal {
            Refusal::Missing => ApiError::not_found(format!(
                "domain '{domain}' has no team origin; only a GitHub team \
                 domain has sync status"
            )),
            Refusal::Conflict => ApiError::conflict(format!(
                "domain '{domain}' is not a team domain; syncing applies to \
                 GitHub team domains only"
            )),
        });
    }
    if !state.engine.github_enabled() {
        return Err(ApiError::conflict(
            "GitHub is switched off on this instance, so its origin cannot be \
             reached: turn it on under Settings > GitHub",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_name_is_one_plain_segment() {
        assert_eq!(check_domain_name(" notes ").unwrap(), "notes");
        assert_eq!(
            check_domain_name("brand-knowledge_2").unwrap(),
            "brand-knowledge_2"
        );
        for bad in [
            "", "   ", "../up", "a/b", "a\\b", "a:b", ".hidden", "a b", "a\tb",
        ] {
            assert_eq!(
                check_domain_name(bad).unwrap_err().status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{bad:?} names no domain"
            );
        }
    }

    #[test]
    fn the_allowlist_refuses_windows_hostile_punctuation_a_cap_and_bare_dots() {
        for bad in ["a*b", "a?b", "a<b", "a>b", "a|b", "a\"b"] {
            assert_eq!(
                check_domain_name(bad).unwrap_err().status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{bad:?} names no domain"
            );
        }
        let too_long: String = "a".repeat(65);
        assert_eq!(
            check_domain_name(&too_long).unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "65 characters is over the cap"
        );
        assert_eq!(
            check_domain_name("notes.").unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "a trailing dot is a Windows hazard, same class as a leading one"
        );

        // Must stay legal: the cap itself, dotted names, non-ASCII letters
        // (an operator's own language buys no safety by being refused), and
        // a decomposed accent is refused for a documented reason, not left
        // to chance.
        let exactly_64: String = "a".repeat(64);
        assert_eq!(check_domain_name(&exactly_64).unwrap(), exactly_64);
        assert_eq!(check_domain_name("notes.v2").unwrap(), "notes.v2");
        assert_eq!(check_domain_name("wissen").unwrap(), "wissen");
        assert_eq!(check_domain_name("知识库").unwrap(), "知识库");
    }

    #[test]
    fn the_allowlist_refuses_nfd_decomposed_names() {
        // "café" spelled with a combining acute accent (U+0301) rather than
        // the precomposed U+00E9: category Mn, not alphanumeric, so this is
        // refused - a documented limitation (decision 9), not a surprise.
        // Normalizing to NFC first would need a new dependency this change
        // does not take.
        let nfd_name = "cafe\u{0301}";
        assert_eq!(
            check_domain_name(nfd_name).unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn the_allowlist_refuses_windows_reserved_device_names() {
        for bad in [
            "con",
            "CON",
            "PRN",
            "AUX",
            "NUL",
            "com1",
            "LPT9",
            "CON.txt",
            "nul.md",
            // The superscript-digit spellings resolve to the same device as
            // the ASCII digit on Windows and sail through a bare
            // alphanumeric allowlist unless matched explicitly (decision 7).
            "COM\u{b9}",
            "LPT\u{b9}",
        ] {
            assert_eq!(
                check_domain_name(bad).unwrap_err().status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{bad:?} is a Windows reserved device name"
            );
        }
        // Must stay legal - none of these are reserved: COM10 and up need
        // the `\\.\` device syntax, "console" merely starts with "con", and
        // the reserved check only looks at the segment before the first
        // dot, so an extension named "CON" is not the device stem.
        for ok in ["console", "com10", "a.CON"] {
            assert_eq!(check_domain_name(ok).unwrap(), ok, "{ok:?} is legal");
        }
    }

    #[test]
    fn a_field_from_another_mode_is_refused_but_an_empty_one_is_not() {
        assert!(require_absent(&None, "repo", "local").is_ok());
        assert!(require_absent(&Some("  ".to_string()), "repo", "local").is_ok());
        let err = require_absent(&Some("acme/kb".to_string()), "repo", "local").unwrap_err();
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.detail.contains("repo"), "{}", err.detail);
    }

    /// The aggregate the origin verbs answer with is flattened to the one
    /// domain that was asked for, and a failure they collected instead of
    /// raising becomes this request's failure rather than a cheerful 200.
    #[test]
    fn one_domain_is_unwrapped_and_a_collected_failure_is_raised() {
        let report = serde_json::json!({
            "connection": { "connected": true },
            "domains": [{ "domain": "eng", "repo": "acme/kb", "local_changes": 2 }],
            "errors": [],
        });
        let one = single_domain(report, "eng", "reading the origin status").unwrap();
        assert_eq!(one["repo"], "acme/kb");
        assert_eq!(one["local_changes"], 2);
        assert!(!one.contains_key("domains"), "flattened, not nested");

        let failed = serde_json::json!({
            "domains": [],
            "errors": [{ "domain": "eng", "error": "offline" }],
        });
        let err = single_domain(failed, "eng", "the pull").unwrap_err();
        assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            err.detail.contains("the pull") && err.detail.contains("offline"),
            "the detail names what failed and why: {}",
            err.detail
        );

        // Neither a result nor a reason: still a failure, never a 200 with an
        // empty body a client would read as a successful sync.
        let empty = serde_json::json!({ "domains": [], "errors": [] });
        let err = single_domain(empty, "eng", "the pull").unwrap_err();
        assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
