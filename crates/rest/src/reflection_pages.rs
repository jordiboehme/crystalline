//! A Reflection page's life beside its text (design 2.1 to 2.4 and 3.2): its
//! settings, its retirement and its clock, and the pages a reflect run
//! missed. Every verb is the engine's; this module builds the caller and
//! gives the answers a typed shape, so Fluid draws them from generated
//! types.
//!
//! A page path can be the last segment of a route only (`reflections/{*path}`
//! is a wildcard), so these routes are static and carry the page in the body
//! or the query, as `reflection/move` and `reflection/disclose` do.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use super::auth::Identity;
use super::reflections::{ANONYMOUS_EDIT, caller, reflection_error, refuse_anonymous};
use super::{ApiError, ApiJson, ApiPath, ApiQuery, ProblemDetail, RestState, refuse_read_only};
use crate::params::{PageSettingsChange, WriteReflectionParams};

/// A page as another page names it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionPageRef {
    /// The page's path inside reflections/.
    #[schema(example = "arch/overview.md")]
    pub path: String,
    /// Its title; its path when the page does not exist.
    #[schema(example = "Architecture overview")]
    pub title: String,
    /// Whether the page exists.
    pub exists: bool,
}

/// A page a page follows, on the way up its pact chain.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionChainPage {
    /// The page's path inside reflections/.
    #[schema(example = "arch/overview.md")]
    pub path: String,
    /// Its title.
    #[schema(example = "Architecture overview")]
    pub title: String,
}

/// A page's own page pact file.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionPactFile {
    /// Its path inside reflections/: the page's path with `.pact.md`.
    #[schema(example = "arch/overview.pact.md")]
    pub path: String,
    /// Whether it exists. A curator creates it with a `PUT` of that path.
    pub exists: bool,
}

/// A page setting or link that did not read, and why.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionPageFinding {
    /// The page it is about: this page, or a page up its chain.
    #[schema(example = "notes.md")]
    pub page: String,
    /// `freshness`, `follows`, `owner` or `folder parent`.
    #[schema(example = "freshness")]
    pub key: String,
    /// The value as written.
    #[schema(example = "0h")]
    pub value: String,
    /// One sentence saying why it is ignored.
    #[schema(example = "is shorter than the minimum of 1 hour, so it is ignored")]
    pub reason: String,
}

/// How and when a page was retired.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionRetirement {
    /// When, RFC 3339; null for a page retired only by a retired word in
    /// its status.
    #[schema(example = "2026-11-20T09:00:00Z")]
    pub at: Option<String>,
    /// Who retired it: a person's or an agent's name; null as `at` is.
    #[schema(example = "claude-code")]
    pub by: Option<String>,
    /// The page that owned it then; a restore can put it back under that
    /// page.
    pub was_owned_by: Option<ReflectionPageRef>,
}

/// What a page says about itself beside its text: the `page` member of
/// `GET /domains/{domain}/reflections/{path}` for a page (never for
/// `PACT.md` or a page pact), and of a settings change's receipt.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionPageView {
    /// How often the page is brought in line with its pacts, as written
    /// (`6h`, `1d`); null when it has none, when it does not read or is
    /// under an hour (then named in `findings`), or the page is owned (it
    /// is reconciled with its owner).
    #[schema(example = "1d")]
    pub freshness: Option<String>,
    /// The page whose pact this page follows, as written; null when the
    /// page follows its owner or its folder parent.
    pub follows: Option<String>,
    /// The page's `status`, as written. Any word of the retired set, as an
    /// engram's (`archived`, which a retire writes, `superseded`,
    /// `deprecated`, `legacy`; case folded), means the page is retired, and
    /// `retired` is set.
    pub status: Option<String>,
    /// The page that owns this one: reconciling it keeps this page current.
    pub owner: Option<ReflectionPageRef>,
    /// Set when the page is retired (as the tree's `retired` says): hidden
    /// in the navigation, never reconciled, still found by search and by
    /// link, and restorable.
    pub retired: Option<ReflectionRetirement>,
    /// The pages this page follows, from the top down; the last one is the
    /// page it follows directly. Empty when it follows only `PACT.md`.
    pub chain: Vec<ReflectionChainPage>,
    /// The pact files that apply, in order: `PACT.md` (when it exists),
    /// then the page pact of each followed page that has one, then the
    /// page's own.
    #[schema(example = json!(["PACT.md", "arch/overview.pact.md"]))]
    pub pacts: Vec<String>,
    /// The page's own page pact file.
    pub pact: ReflectionPactFile,
    /// What did not read, on this page or up its chain.
    pub findings: Vec<ReflectionPageFinding>,
}

/// What `PATCH /domains/{domain}/reflection/settings` takes.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(description = "A page's settings, the frontmatter keys only a \
                        curator changes. A key left out, or sent as null, \
                        keeps its value; an empty string (\"\") takes the \
                        key off the page; any other value sets it. At least \
                        one key.")]
pub struct ReflectionSettingsBody {
    /// The page's path inside reflections/.
    #[schema(example = "arch/overview.md")]
    pub path: String,
    /// How often the page is brought in line with its pacts: `1h`, `6h`,
    /// `1d`, `7d`, `30d` or any span of an hour or more (`h`, `d`, `w`, `m`
    /// for months of 30 days, `y`). Refused on an owned page.
    #[serde(default)]
    #[schema(example = "1d")]
    pub freshness: Option<String>,
    /// The page of this Reflection whose pact this page follows.
    #[serde(default)]
    pub follows: Option<String>,
    /// The page's status. A retired word is refused: retire the page with
    /// `POST /domains/{domain}/reflection/retire`.
    #[serde(default)]
    pub status: Option<String>,
}

/// What a settings change answers: the write's receipt with the page as it
/// stands now.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReflectionSettingsReceipt {
    /// The domain.
    pub domain: String,
    /// The page's path inside reflections/.
    pub path: String,
    /// The checksum of the file as written.
    pub checksum: String,
    /// The page's settings, chain, owner and retirement now.
    pub page: ReflectionPageView,
    /// While a co-editing room is open over the page: `live` when the room
    /// took the change in, `file` when it is on disk only.
    #[serde(default)]
    pub landed: Option<String>,
    /// With `landed: file`: what the people in the room meet.
    #[serde(default)]
    pub room_note: Option<String>,
}

/// `PATCH /domains/{domain}/reflection/settings`
#[utoipa::path(
    patch,
    path = "/api/v1/domains/{domain}/reflection/settings",
    tag = "reflection",
    operation_id = "set_reflection_settings",
    summary = "Change a page's freshness, follows or status.",
    description = "Needs `curate`. Only the frontmatter keys named \
                   (`freshness`, `follows`, `status`) change; the page's \
                   text, its sections and their sources stay, so nobody \
                   owes feedback. A co-editing room open over the page takes \
                   the change in, and the editor never shows these keys. A \
                   change of `freshness` or `status` keeps a recorded \
                   reconcile current. A `follows` that brings other pacts \
                   raises a `page_pact_changed` item, since the page now \
                   follows another pact.",
    params(("domain" = String, Path, description = "The domain.")),
    request_body = ReflectionSettingsBody,
    responses(
        (status = 200, description = "Changed.", body = ReflectionSettingsReceipt),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: it reads \
                           the Reflection and never changes it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below curate access (the detail names the right held \
                           and the right needed), the domain is off the \
                           caller's write list, or this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or page, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The page changed while it was written (read it again \
                           and repeat), or the Reflection is not here.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "No key to change; a pact file; a freshness that does \
                           not read, is under an hour, or is set on an owned \
                           page; a follows that names no page of this \
                           Reflection or closes a cycle; a retired word in \
                           status (retire the page instead); a status change on \
                           a retired page (restore it instead).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn settings(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<ReflectionSettingsBody>,
) -> Result<Json<ReflectionSettingsReceipt>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    let change = PageSettingsChange {
        path: body.path,
        freshness: body.freshness,
        follows: body.follows,
        status: body.status,
    };
    let receipt = state
        .engine
        .set_reflection_page_settings(&domain, &change, &caller(&identity))
        .await
        .map_err(reflection_error)?;
    typed(receipt, "settings receipt")
}

/// What `POST /domains/{domain}/reflection/retire` takes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RetireReflectionBody {
    /// The page's path inside reflections/.
    #[schema(example = "arch/overview/old.md")]
    pub path: String,
}

/// What a retire answers.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RetiredReflection {
    /// The domain.
    pub domain: String,
    /// The page's path inside reflections/.
    pub path: String,
    /// Always true.
    pub retired: bool,
    /// The page that owned it, kept so a restore can put it back.
    pub was_owned_by: Option<String>,
}

/// `POST /domains/{domain}/reflection/retire`
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/reflection/retire",
    tag = "reflection",
    operation_id = "retire_reflection",
    summary = "Retire a page: hidden, kept and restorable.",
    description = "Needs `curate`. The page's status becomes `archived`, it \
                   leaves its owner's set (the owner is kept as the page it \
                   was owned by) and it is hidden in the navigation, never \
                   reconciled, and still reached by link and by search. Its \
                   threads stay. This is what an unattended run does where a \
                   person would delete.",
    params(("domain" = String, Path, description = "The domain.")),
    request_body = RetireReflectionBody,
    responses(
        (status = 200, description = "Retired.", body = RetiredReflection),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below curate access, the domain is off the caller's \
                           write list, or this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or page, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A pact file, which is no page, or a page already retired.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn retire(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<RetireReflectionBody>,
) -> Result<Json<RetiredReflection>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    let p = WriteReflectionParams {
        domain,
        action: Some("retire".into()),
        path: body.path,
        ..Default::default()
    };
    let receipt = state
        .engine
        .write_reflection(&p, &caller(&identity))
        .await
        .map_err(reflection_error)?;
    typed(receipt, "retire receipt")
}

/// What `POST /domains/{domain}/reflection/restore` takes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RestoreReflectionBody {
    /// The retired page's path inside reflections/.
    #[schema(example = "arch/overview/old.md")]
    pub path: String,
    /// The page it was owned by, to put it back under that page; leave it
    /// out to restore it as a page of its own.
    #[serde(default)]
    #[schema(example = "arch/overview.md")]
    pub owner: Option<String>,
}

/// What a restore answers.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RestoredReflection {
    /// The domain.
    pub domain: String,
    /// The page's path inside reflections/.
    pub path: String,
    /// Always true.
    pub restored: bool,
    /// The page that owns it again; null when it stands on its own.
    pub owner: Option<String>,
}

/// `POST /domains/{domain}/reflection/restore`
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/reflection/restore",
    tag = "reflection",
    operation_id = "restore_reflection",
    summary = "Bring a retired page back, on its own or under its owner.",
    description = "Needs `curate`. The page gets back the status it had \
                   before it was retired (none when it had none). With \
                   `owner`, which must be the page it was owned by and still \
                   a page that may own it, it goes back under that page.",
    params(("domain" = String, Path, description = "The domain.")),
    request_body = RestoreReflectionBody,
    responses(
        (status = 200, description = "Restored.", body = RestoredReflection),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below curate access, the domain is off the caller's \
                           write list, or this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or page, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A page that is not retired, a pact file, an empty \
                           `owner`, an `owner` that is not the page it was \
                           owned by, or an owner that is gone, retired or \
                           would close a cycle.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn restore(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<RestoreReflectionBody>,
) -> Result<Json<RestoredReflection>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    let p = WriteReflectionParams {
        domain,
        action: Some("restore".into()),
        path: body.path,
        owner: body.owner,
        ..Default::default()
    };
    let receipt = state
        .engine
        .write_reflection(&p, &caller(&identity))
        .await
        .map_err(reflection_error)?;
    typed(receipt, "restore receipt")
}

/// The page an owned page is part of.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionPartOf {
    /// The owner's path inside reflections/.
    #[schema(example = "arch/overview.md")]
    pub path: String,
    /// Its title.
    #[schema(example = "Architecture overview")]
    pub title: String,
}

/// One page's freshness clock, in seconds relative to the server's
/// automation clock (design 2.2 and 2.3).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionPageClock {
    /// The page's path inside reflections/.
    pub path: String,
    /// The freshness that keeps the page current, as written: its own, or
    /// for an owned page that of `clock_of`; null when none does.
    #[schema(example = "1d")]
    pub freshness: Option<String>,
    /// When the page (or `clock_of`) was last reconciled, RFC 3339; the
    /// page's change time when no reconcile is recorded.
    pub last_reconciled: Option<String>,
    /// Seconds since then, never negative.
    #[schema(example = 7200)]
    pub last_checked_secs: Option<i64>,
    /// Seconds until a fresh page falls due; null once it is due, and for
    /// an owned page.
    #[schema(example = 3600)]
    pub due_in_secs: Option<i64>,
    /// Whether the page is due: its freshness ran out.
    pub stale: bool,
    /// Seconds since the page turned overdue, one whole period after it
    /// fell due; null before. Show it on the page ("Overdue since ...").
    pub overdue_secs: Option<i64>,
    /// Seconds since a reflect run of the domain that started after the
    /// page fell due ended without reconciling it; null while no run left
    /// it. Only this is warned about (the missed list, doctor).
    pub missed_secs: Option<i64>,
    /// The page is overdue, reflect runs are known and none left it: the
    /// loop runs less often than the page's freshness.
    pub loop_slower_than_freshness: bool,
    /// A due page that a co-editing room has open: it waits for the room.
    pub waiting: bool,
    /// The page that owns this one; null for a page on its own.
    pub part_of: Option<ReflectionPartOf>,
    /// The page whose freshness keeps this one current: the page itself,
    /// or for an owned page the first page up its owners that has a
    /// freshness; null when none has.
    #[schema(example = "arch/overview.md")]
    pub clock_of: Option<String>,
}

/// What `GET /domains/{domain}/reflection/clock` answers.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReflectionClockResponse {
    /// The clock; null for a page on its own with no freshness, a retired
    /// page and a missing one.
    pub clock: Option<ReflectionPageClock>,
}

/// The query of `GET /domains/{domain}/reflection/clock`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ClockQuery {
    /// The page's path inside reflections/.
    #[param(example = "arch/overview.md")]
    path: String,
}

/// `GET /domains/{domain}/reflection/clock`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/clock",
    tag = "reflection",
    operation_id = "get_reflection_clock",
    summary = "When a page was last checked, when it is due, and whether it is overdue.",
    description = "Needs `read`. For the line under a page's title: \"Last \
                   checked 2 hours ago, due again in 1 hour\", \"Overdue \
                   since 3 hours\", \"Waiting: open in the editor\", or for \
                   an owned page \"Part of: Architecture overview\" with its \
                   owner's last check.",
    params(("domain" = String, Path, description = "The domain."), ClockQuery),
    responses(
        (status = 200, description = "The page's clock.", body = ReflectionClockResponse),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn clock(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(q): ApiQuery<ClockQuery>,
) -> Result<Json<ReflectionClockResponse>, ApiError> {
    let clock = state
        .engine
        .reflection_page_clock(&domain, &q.path, &caller(&identity))
        .await
        .map_err(reflection_error)?;
    let clock = match clock {
        Value::Null => None,
        clock => Some(typed_value(clock, "page clock")?),
    };
    Ok(Json(ReflectionClockResponse { clock }))
}

/// A page a reflect run missed.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MissedReflectionPage {
    /// The domain.
    #[schema(example = "eng")]
    pub domain: String,
    /// The page's path inside reflections/.
    #[schema(example = "status/this-week.md")]
    pub path: String,
    /// Its freshness, as written.
    #[schema(example = "1d")]
    pub freshness: String,
    /// Seconds since it was last reconciled.
    #[schema(example = 172800)]
    pub last_checked_secs: i64,
    /// Seconds since the end of the first reflect run that started after
    /// the page fell due and left it.
    #[schema(example = 10800)]
    pub missed_secs: i64,
    /// A co-editing room has the page open: it waits for the room.
    pub waiting: bool,
}

/// What `GET /reflection/missed` answers.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MissedReflectionPages {
    /// Longest missed first.
    pub pages: Vec<MissedReflectionPage>,
}

/// `GET /reflection/missed`
#[utoipa::path(
    get,
    path = "/api/v1/reflection/missed",
    tag = "reflection",
    operation_id = "list_missed_reflection_pages",
    summary = "The Reflection pages a reflect run left behind.",
    description = "For the admin banner and the maintenance view: every page \
                   of a Reflection served here that the caller may read, \
                   where a reflect run of its domain started after the page \
                   fell due and ended without reconciling it. A page that is \
                   only overdue because the loop runs less often than its \
                   freshness is not here: the loop did not fail it. A \
                   domain whose pages cannot be read is left out.",
    responses(
        (status = 200, description = "The missed pages.", body = MissedReflectionPages),
    ),
)]
pub async fn missed(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<MissedReflectionPages>, ApiError> {
    let answer = state
        .engine
        .overdue_reflection_pages(&caller(&identity))
        .await
        .map_err(reflection_error)?;
    typed(answer, "missed pages")
}

/// An engine answer in its typed shape.
fn typed<T: serde::de::DeserializeOwned>(value: Value, what: &str) -> Result<Json<T>, ApiError> {
    typed_value(value, what).map(Json)
}

fn typed_value<T: serde::de::DeserializeOwned>(value: Value, what: &str) -> Result<T, ApiError> {
    serde_json::from_value(value)
        .map_err(|e| ApiError::internal(format!("the {what} is unreadable: {e}")))
}
