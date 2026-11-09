//! Kept texts: what a co-editing room held when its last save could not
//! land, kept in the server's state folder (see
//! [`crystalline_core::kept`]). A person finds their text here and copies it
//! back. The rights are the engine's: a Reflection text needs `edit`
//! (`curate` for PACT.md), the right its room needs, and an engram text
//! needs write access to the domain's engrams, each entry by its own kind.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use crystalline_core::kept::KeptText;
use serde::Serialize;
use utoipa::ToSchema;

use super::auth::Identity;
use super::{ApiError, ApiPath, ProblemDetail, RestState, refuse_read_only};

/// The anonymous viewer's answer: kept texts belong to people who write.
const ANONYMOUS_KEPT: &str = "kept texts are for signed-in people who may write this domain";

/// One kept text, without its text.
#[derive(Debug, Serialize, ToSchema)]
pub struct KeptTextEntry {
    /// A short, stable id for the routes below.
    #[schema(example = "3f9a1c2b7d4e")]
    pub id: String,
    /// Whether it was kept from the caller's own review draft rather than
    /// from the domain's own text. A draft's kept text is only ever listed
    /// for its owner.
    pub draft: bool,
    /// `engram` or `reflection`: the kind of room (or page write) it was
    /// kept from; `comment` for a thread comment on the page at `path`.
    #[schema(example = "reflection")]
    pub kind: String,
    /// The engram's file path in its domain, or the page's path inside
    /// `reflections/`.
    #[schema(example = "guides/deploying.md")]
    pub path: String,
    /// When it was kept.
    #[schema(value_type = String, format = DateTime, example = "2026-10-15T08:00:00Z")]
    pub kept_at: DateTime<Utc>,
    /// Its size in bytes.
    #[schema(example = 1204)]
    pub bytes: u64,
    /// Who sent the text, by the name shown in threads (the login name for
    /// a person), for a text kept from a write that was refused after its
    /// instance lost the domain's host lock; `null` for a room's text.
    #[schema(example = "ada")]
    pub sender: Option<String>,
    /// The account that sent it, the login name the rights match a kept
    /// comment's sender on; `null` when no sender is recorded. A client
    /// compares it with the signed-in account to tell its own comments
    /// apart: the shown name is not proof of who sent it, the account is.
    #[schema(example = "ada")]
    pub sender_account: Option<String>,
}

/// What `GET /domains/{domain}/kept` answers.
#[derive(Debug, Serialize, ToSchema)]
pub struct KeptTextsResponse {
    /// The domain.
    #[schema(example = "eng")]
    pub domain: String,
    /// The kept texts this caller may handle, oldest first.
    pub kept: Vec<KeptTextEntry>,
}

/// What `GET /domains/{domain}/kept/{id}` answers.
#[derive(Debug, Serialize, ToSchema)]
pub struct KeptTextResponse {
    /// The domain.
    #[schema(example = "eng")]
    pub domain: String,
    /// The entry, as the listing shows it.
    #[serde(flatten)]
    pub entry: KeptTextEntry,
    /// The text the room held, as it was kept.
    #[schema(example = "# Deploying\n\nTyped while it was switched off.\n")]
    pub text: String,
}

fn entry(kept: KeptText) -> KeptTextEntry {
    KeptTextEntry {
        id: kept.id,
        draft: kept.owner.is_some(),
        kind: kept.kind.as_str().to_string(),
        path: kept.path,
        kept_at: kept.kept_at,
        bytes: kept.bytes,
        sender_account: kept.sender.as_ref().map(|s| s.account.clone()),
        sender: kept.sender.map(|s| s.name),
    }
}

/// A domain the anonymous viewer may not see answers as unknown; one it
/// may see answers 401.
async fn refuse_anonymous(
    state: &RestState,
    identity: &Identity,
    domain: &str,
) -> Result<(), ApiError> {
    if identity.user.is_some() {
        return Ok(());
    }
    state
        .engine
        .require_domain(domain, &identity.scope())
        .await?;
    Err(ApiError::unauthorized(ANONYMOUS_KEPT))
}

/// `GET /domains/{domain}/kept`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/kept",
    tag = "domains",
    operation_id = "list_kept_texts",
    summary = "The texts the server kept for this domain when a room could not save.",
    description = "When the last person leaves a co-editing room and its final \
                   save is refused or ends in a conflict, the server keeps the \
                   room's text in its state folder. This lists them. A \
                   Reflection page's text needs `edit` on the Reflection \
                   (`curate` for PACT.md), an \
                   engram's text needs write access to the engrams; each entry \
                   is listed only for a caller who holds its own right. A text \
                   kept from a review draft is listed for the draft's owner \
                   only (`draft: true`). The \
                   Reflection's state plays no part: a text kept because the \
                   Reflection was switched off is listed too.",
    params(("domain" = String, Path, description = "The domain.")),
    responses(
        (status = 200, description = "The kept texts this caller may handle.", body = KeptTextsResponse),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Neither write access to the engrams nor edit \
                           access to the Reflection.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn list(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<KeptTextsResponse>, ApiError> {
    refuse_anonymous(&state, &identity, &domain).await?;
    let kept = state.engine.kept_texts(&domain, &identity.scope()).await?;
    Ok(Json(KeptTextsResponse {
        domain,
        kept: kept.into_iter().map(entry).collect(),
    }))
}

/// `GET /domains/{domain}/kept/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/kept/{id}",
    tag = "domains",
    operation_id = "read_kept_text",
    summary = "One kept text with its text.",
    description = "The same rights as the listing, decided for this entry's \
                   kind. An entry this caller may not handle answers 404, like \
                   an id that names none.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("id" = String, Path, description = "The id from the listing."),
    ),
    responses(
        (status = 200, description = "The entry and its text.", body = KeptTextResponse),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Neither write access to the engrams nor edit \
                           access to the Reflection.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or kept text for this caller.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The kept copy is a file, not text. The detail says \
                           where it is.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn read(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, id)): ApiPath<(String, String)>,
) -> Result<Json<KeptTextResponse>, ApiError> {
    refuse_anonymous(&state, &identity, &domain).await?;
    let (kept, text) = state
        .engine
        .kept_text(&domain, &id, &identity.scope())
        .await?;
    Ok(Json(KeptTextResponse {
        domain,
        entry: entry(kept),
        text,
    }))
}

/// `DELETE /domains/{domain}/kept/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}/kept/{id}",
    tag = "domains",
    operation_id = "delete_kept_text",
    summary = "Delete one kept text, once its text is back where it belongs.",
    description = "The same rights as reading it. The server never deletes a \
                   kept text on its own; this is the only way one goes, \
                   besides removing the file by hand.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("id" = String, Path, description = "The id from the listing."),
    ),
    responses(
        (status = 204, description = "Deleted."),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Neither write access to the engrams nor edit \
                           access to the Reflection, or this instance is \
                           read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or kept text for this caller.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, id)): ApiPath<(String, String)>,
) -> Result<StatusCode, ApiError> {
    refuse_anonymous(&state, &identity, &domain).await?;
    refuse_read_only(&state)?;
    state
        .engine
        .drop_kept_text(&domain, &id, &identity.scope())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crystalline_core::kept::{KeptKind, Sender};

    fn comment(sender: Option<Sender>) -> KeptText {
        KeptText {
            id: "3f9a1c2b7d4e".to_string(),
            domain: "eng".to_string(),
            removed: false,
            unfinished: false,
            owner: None,
            kind: KeptKind::Comment,
            path: "guides/a.md".to_string(),
            kept_at: Utc.with_ymd_and_hms(2026, 10, 15, 8, 0, 0).unwrap(),
            bytes: 12,
            file: std::path::PathBuf::from("kept/eng/comment/guides/a.md.20261015T080000Z.md"),
            sender,
        }
    }

    /// The entry names the sender's account beside the shown name, and
    /// neither when no sender is recorded.
    #[test]
    fn an_entry_carries_the_senders_account() {
        let named = entry(comment(Some(Sender {
            account: "vera".to_string(),
            name: "Rita".to_string(),
        })));
        assert_eq!(named.sender.as_deref(), Some("Rita"));
        assert_eq!(named.sender_account.as_deref(), Some("vera"));
        let json = serde_json::to_value(&named).unwrap();
        assert_eq!(json["sender_account"], "vera");

        let nobody = entry(comment(None));
        assert_eq!(nobody.sender, None);
        assert_eq!(nobody.sender_account, None);
        assert!(serde_json::to_value(&nobody).unwrap()["sender_account"].is_null());
    }
}
