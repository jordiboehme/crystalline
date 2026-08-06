//! The accounts that may sign in to this instance, managed over HTTP.
//!
//! The same four operations `crystalline users` performs, behind the same
//! store, so the CLI and the UI are one source of truth rather than two: the
//! listing is the `{"users": [...]}` envelope `users list --json` already
//! prints, and every refusal here is the store's own, surfaced with a status a
//! browser client can branch on.
//!
//! These are the first mutating routes on this surface, so three rules are
//! written down rather than left to be re-derived:
//!
//! 1. **Admin only, in the handler.** [`super::auth::guard`] enforces viewer
//!    and nothing more, so every handler below opens with
//!    [`Identity::require_admin`]. A route added here without it would be
//!    reachable by any account that can log in.
//! 2. **Cross-site protection.** A cookie session must echo its CSRF token on
//!    every unsafe method; the middleware does that. The other two identities
//!    carry no token, so each needs its own reason to be safe:
//!    - The anonymous viewer never gets past `require_admin`. It has no
//!      account, so it is answered 401 rather than served.
//!    - A trusted-header admin is protected by the request shape. `PATCH` and
//!      `DELETE` are not simple methods, so a cross-origin caller cannot send
//!      them at all without a CORS preflight; `POST` it can send, but only with
//!      `application/x-www-form-urlencoded`, `text/plain` or
//!      `multipart/form-data`, and [`create`] takes its body through
//!      [`ApiJson`], which demands `application/json` and refuses all three.
//!      **No CORS layer exists on this surface and one must not be added
//!      without revisiting the CSRF check in [`super::auth`] first**: a
//!      permitted preflight would remove the only thing standing between
//!      another origin and a trusted-header admin's account.
//! 3. **No lockout escape hatch.** The store refuses to remove, disable or
//!    demote the last enabled admin, and that refusal is passed through as a
//!    409 rather than overridden by any flag. An installation must never be
//!    lockable out of its own user management over HTTP; `crystalline users`,
//!    which runs on the machine that holds the database, is the recovery path.
//!    For the same reason an admin may not delete or disable *itself*, which is
//!    a distinct refusal: another admin may well exist, and the account being
//!    protected is the caller's own way back in.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use super::auth::{Caller, Identity};
use super::auth_store::{Role, User};
use super::{ApiError, ApiJson, ApiPath, ProblemDetail, RestState};

/// What `GET /users` answers with. A wrapper rather than a bare array, matching
/// the `{"users": [...]}` envelope `crystalline users list --json` prints.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct UsersResponse {
    /// Every account, by name.
    users: Vec<User>,
}

/// What the two writing routes answer with: the account as stored, read back
/// after the write rather than echoed from the request.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct UserResponse {
    /// The account as it now stands.
    user: User,
}

/// `GET /users` - every account, by name.
///
/// [`User`] carries no password material, so the rows go out as they come back
/// from the store. Admin only: the account list names every way into the
/// instance and who holds it.
#[utoipa::path(
    get,
    path = "/api/v1/users",
    tag = "users",
    operation_id = "list_users",
    summary = "Every account, by name.",
    description = "An account carries no password material, so the rows go out \
                   as they come back from the store. Admin only: the account \
                   list names every way into the instance and who holds it.",
    responses(
        (status = 200, description = "Every account.", body = UsersResponse),
        (
            status = 401,
            description = "No identity.",
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
    ),
)]
pub async fn list(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<UsersResponse>, ApiError> {
    identity.require_admin()?;
    let users = state
        .auth
        .list_users()
        .await
        .map_err(|e| store_error(e, ""))?;
    Ok(Json(UsersResponse { users }))
}

/// What `POST /users` takes. `display` defaults to the name as typed, matching
/// `crystalline users add`, and `email` is optional and never used for login.
///
/// [`Debug`] is written by hand rather than derived, here and on [`PatchBody`]:
/// the derived one would print the plaintext password, and this type is one
/// `tracing::debug!` or one `unwrap` on a rejection away from a log file.
#[derive(Deserialize, utoipa::ToSchema)]
#[schema(description = "A new account. `display` defaults to `name` as typed, \
                        and `email` is optional and never used for login.")]
pub struct CreateBody {
    /// The login name, in any casing: the store folds it.
    #[schema(example = "bob")]
    name: String,
    /// Human-readable name for the UI. Defaults to `name` as typed.
    #[serde(default)]
    #[schema(example = "Bob")]
    display: Option<String>,
    /// Optional contact address.
    #[serde(default)]
    #[schema(example = "bob@example.com")]
    email: Option<String>,
    /// What the new account may do.
    role: Role,
    /// The initial password. Never stored in the clear; the store hashes it.
    #[schema(example = "correct horse battery staple")]
    password: String,
}

/// `POST /users` - add an account, answering 201 with the account as stored.
///
/// The response is read back out of the store rather than echoed from the
/// request, so what the client renders is what was written: the folded name,
/// the defaulted display name, and the disabled flag the row starts with.
#[utoipa::path(
    post,
    path = "/api/v1/users",
    tag = "users",
    operation_id = "create_user",
    request_body = CreateBody,
    responses(
        (status = 201, description = "The account as stored.", body = UserResponse),
        (
            status = 400,
            description = "The body is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "That name is already taken.",
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
            description = "The body is JSON but not an account, or the password \
                           is empty.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn create(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<(StatusCode, Json<UserResponse>), ApiError> {
    identity.require_admin()?;
    check_password(&body.password)?;
    // The login name as typed makes the better default display name: the store
    // folds the login name but keeps this one as given.
    let display = body.display.unwrap_or_else(|| body.name.trim().to_string());
    // Under the login limiter: `add_user` hashes with argon2id, which costs
    // about 19 MiB on a blocking thread, and an admin client looping over a
    // list of new accounts would otherwise reserve that memory without bound
    // while logins on the same instance are being held to four.
    state
        .with_login_slot(state.auth.add_user(
            &body.name,
            &display,
            body.email.as_deref(),
            body.role,
            &body.password,
        ))
        .await?
        .map_err(|e| store_error(e, &body.name))?;
    let user = read_back(&state, &body.name).await?;
    Ok((StatusCode::CREATED, Json(UserResponse { user })))
}

/// What `PATCH /users/{name}` takes: whichever of the three an admin wants to
/// change. All absent is refused rather than served as a no-op, so a client
/// sending the wrong field names hears about it.
///
/// [`Debug`] is hand-written and redacts the password, as on [`CreateBody`].
#[derive(Deserialize, utoipa::ToSchema)]
#[schema(description = "Whichever of the three an admin wants to change. All \
                        absent is refused with a 422 rather than served as a \
                        no-op, so a client sending the wrong field names hears \
                        about it.")]
pub struct PatchBody {
    /// The new role.
    #[serde(default)]
    role: Option<Role>,
    /// Whether the account is disabled. Disabling deletes every session the
    /// account holds, so it is a revocation rather than a flag a later
    /// re-enabling hands back; it also stops any new login.
    #[serde(default)]
    disabled: Option<bool>,
    /// A replacement password. Setting it revokes every session the account
    /// holds, so whoever was signed in under the old one is signed out.
    #[serde(default)]
    password: Option<String>,
}

impl std::fmt::Debug for CreateBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateBody")
            .field("name", &self.name)
            .field("display", &self.display)
            .field("email", &self.email)
            .field("role", &self.role)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for PatchBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatchBody")
            .field("role", &self.role)
            .field("disabled", &self.disabled)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// `PATCH /users/{name}` - change a role, a disabled flag, a password, or any
/// combination, answering with the account as it now stands.
///
/// Two of the three fields are revocations as well as edits: setting a password
/// and disabling an account each delete every session that account holds, in
/// the store and in the same transaction as the change. That is what makes this
/// route useful against a compromised account - a reset that left the intruder's
/// cookie alive for the rest of its 30-day life would be theatre - and it means
/// an admin resetting their own password signs their own other sessions out too,
/// this one included.
///
/// Every check runs before the first write, so a request that will be refused
/// changes nothing. The writes themselves are one store call each rather than
/// one transaction: the store's guarded statements each own their own
/// invariant, and a failure part-way leaves the earlier fields applied and says
/// which operation was refused. That is visible to a client only in the
/// pathological case of a concurrent edit racing this one, since the refusals a
/// caller can provoke on purpose - the last-admin guard and an unknown account
/// - are decided identically by all three statements.
#[utoipa::path(
    patch,
    path = "/api/v1/users/{name}",
    tag = "users",
    operation_id = "update_user",
    params(("name" = String, Path, description = "The account, in any casing.")),
    request_body = PatchBody,
    responses(
        (status = 200, description = "The account as it now stands.", body = UserResponse),
        (
            status = 400,
            description = "The body is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The change would disable the caller's own account, or \
                           would leave the installation without an enabled admin.",
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
            description = "The body changes nothing, or the new password is empty.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn update(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
    ApiJson(body): ApiJson<PatchBody>,
) -> Result<Json<UserResponse>, ApiError> {
    let caller = identity.require_admin()?;
    if body.role.is_none() && body.disabled.is_none() && body.password.is_none() {
        return Err(ApiError::unprocessable(
            "this request changes nothing: send role, disabled or password",
        ));
    }
    if let Some(password) = &body.password {
        check_password(password)?;
    }
    if body.disabled == Some(true) {
        refuse_self(&caller, &name, "disable")?;
    }
    if let Some(role) = body.role {
        state
            .auth
            .set_role(&name, role)
            .await
            .map_err(|e| store_error(e, &name))?;
    }
    if let Some(disabled) = body.disabled {
        state
            .auth
            .set_disabled(&name, disabled)
            .await
            .map_err(|e| store_error(e, &name))?;
    }
    if let Some(password) = &body.password {
        // Under the login limiter, for the reason `create` gives.
        state
            .with_login_slot(state.auth.set_password(&name, password))
            .await?
            .map_err(|e| store_error(e, &name))?;
    }
    let user = read_back(&state, &name).await?;
    Ok(Json(UserResponse { user }))
}

/// `DELETE /users/{name}` - remove an account and every session it holds,
/// answering 204.
///
/// An admin may not remove its own account (409), and the store refuses to
/// remove the last enabled admin whoever asks (409 as well, in its own words).
#[utoipa::path(
    delete,
    path = "/api/v1/users/{name}",
    tag = "users",
    operation_id = "delete_user",
    params(("name" = String, Path, description = "The account, in any casing.")),
    responses(
        (status = 204, description = "The account and its sessions are gone."),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The account is the caller's own, or is the last \
                           enabled admin.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let caller = identity.require_admin()?;
    refuse_self(&caller, &name, "delete")?;
    state
        .auth
        .remove_user(&name)
        .await
        .map_err(|e| store_error(e, &name))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Refuse an admin acting destructively on its own account. `verb` names what
/// was asked for, and the message is deliberately unlike the store's last-admin
/// one: this refusal can fire while other admins exist, and what it protects is
/// the caller's own session rather than the installation.
fn refuse_self(caller: &Caller, target: &str, verb: &str) -> Result<(), ApiError> {
    if folded(target) != caller.name() {
        return Ok(());
    }
    Err(ApiError::conflict(format!(
        "refusing to {verb} your own account ('{}'): ask another admin to do it, \
         or use `crystalline users` on the server",
        caller.name()
    )))
}

/// Refuse an empty password before it is hashed into an account nobody can log
/// in as. The store would accept it; `crystalline users` refuses it, and this
/// surface matches.
fn check_password(password: &str) -> Result<(), ApiError> {
    if password.is_empty() {
        return Err(ApiError::unprocessable(
            "the password is empty; pick one with at least one character",
        ));
    }
    Ok(())
}

/// The account as it now stands, read back out of the store after a write.
///
/// The listing rather than a single-row read because the store exposes no
/// by-name getter, and the cost is one small query over a table with a handful
/// of rows in it.
async fn read_back(state: &RestState, name: &str) -> Result<User, ApiError> {
    let name = folded(name);
    state
        .auth
        .list_users()
        .await
        .map_err(|e| store_error(e, &name))?
        .into_iter()
        .find(|user| user.name == name)
        .ok_or_else(|| {
            // Only reachable if something removed the account between the write
            // and this read. The write did happen, so this is not a failure of
            // the request - but answering with a body that was made up here
            // would be worse than saying the read did not work.
            ApiError::internal(format!(
                "the account '{name}' was written but could not be read back"
            ))
        })
}

/// The form the store keys on: trimmed and lowercased, mirroring the store's
/// own `normalize_name`.
///
/// Mirrored rather than shared because the store's folding is private to it and
/// is the authority; nothing here writes with this value, it only compares
/// (`refuse_self`) and looks up (`read_back`). `folding_matches_the_store`
/// pins the two together, so a change on either side fails a test rather than
/// silently opening a way around the self-account check.
fn folded(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Classify an [`AuthStore`](super::AuthStore) failure for the wire.
///
/// The store reports in `anyhow`, so this reads its message. That is a string
/// seam, and it is tested rather than assumed: `store_errors_are_classified`
/// below drives a real store into each of these failures and asserts the status
/// that comes out, so rewording a message on the store side fails here instead
/// of quietly turning a 409 into a 500.
///
/// Anything unrecognized stays a 500: an error this layer cannot name is not
/// the caller's fault by default. `subject` is the account the failed call was
/// about, for the one message that has to be rewritten rather than passed
/// through; pass `""` where there is no single one.
fn store_error(e: anyhow::Error, subject: &str) -> ApiError {
    let detail = format!("{e:#}");
    if detail.contains("UNIQUE constraint") {
        // The store's message here is the database's, and it names a column.
        // The caller gets product copy instead, naming the account and what to
        // do about it, the way `crystalline users add` does.
        return ApiError::conflict(format!(
            "a user named '{}' already exists; edit that account instead",
            folded(subject)
        ));
    }
    // Before the last-admin branch, not after: `no such user: 'the last admin'`
    // is a legal name for an account nobody created, and reading the refusals
    // in the other order would answer that miss with a 409.
    if detail.contains("no such user") {
        return ApiError::not_found(detail);
    }
    if detail.contains("the last admin") {
        return ApiError::conflict(detail);
    }
    if detail.contains("a user name cannot be empty") {
        return ApiError::unprocessable(detail);
    }
    ApiError::internal(detail)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::rest::AuthStore;

    async fn store() -> (tempfile::TempDir, AuthStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        (dir, store)
    }

    /// The classifier against the store's real errors rather than against
    /// strings written down twice: every one of these is provoked by asking the
    /// store to do the thing it refuses.
    #[tokio::test]
    async fn store_errors_are_classified() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "s3cret")
            .await
            .unwrap();

        let duplicate = store
            .add_user("ADA", "Ada again", None, Role::Viewer, "other")
            .await
            .expect_err("the primary key refuses a second 'ada'");
        let api = store_error(duplicate, "ADA");
        assert_eq!(api.status, StatusCode::CONFLICT);
        assert!(
            !api.detail.contains("UNIQUE"),
            "in product copy: {}",
            api.detail
        );
        assert!(
            api.detail.contains("'ada'"),
            "naming the account as it is stored: {}",
            api.detail
        );

        let last_admin = store
            .set_role("ada", Role::Viewer)
            .await
            .expect_err("the last admin cannot be demoted");
        let api = store_error(last_admin, "ada");
        assert_eq!(api.status, StatusCode::CONFLICT);
        assert!(
            api.detail.contains("last admin"),
            "the store's own words: {}",
            api.detail
        );

        let removed = store.remove_user("ada").await.expect_err("nor removed");
        let api = store_error(removed, "ada");
        assert_eq!(api.status, StatusCode::CONFLICT);
        assert!(api.detail.contains("last admin"), "{}", api.detail);

        let missing = store
            .set_password("ghost", "s3cret")
            .await
            .expect_err("no such account");
        let api = store_error(missing, "ghost");
        assert_eq!(api.status, StatusCode::NOT_FOUND);
        assert!(api.detail.contains("ghost"), "{}", api.detail);

        // A miss stays a miss even when the account name is the phrase the
        // last-admin refusal is recognized by, which is why that branch is
        // read second.
        let named = store
            .set_password("the last admin", "s3cret")
            .await
            .expect_err("no such account");
        assert_eq!(
            store_error(named, "the last admin").status,
            StatusCode::NOT_FOUND
        );

        let empty = store
            .set_disabled("   ", true)
            .await
            .expect_err("an empty name is not a name");
        assert_eq!(
            store_error(empty, "   ").status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        assert_eq!(
            store_error(anyhow::anyhow!("the disk fell over"), "ada").status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "an error this layer cannot name is not the caller's fault"
        );
    }

    /// [`folded`] and the store must agree, because the self-account check
    /// compares this layer's folding against the name the store handed back.
    #[tokio::test]
    async fn folding_matches_the_store() {
        let (_dir, store) = store().await;
        for raw in ["  AdA ", "ADA", "ada", "Ada\t"] {
            store
                .add_user(raw, "Ada", None, Role::Viewer, "s3cret")
                .await
                .ok();
            let users = store.list_users().await.unwrap();
            assert_eq!(users.len(), 1, "every spelling is the one account");
            assert_eq!(users[0].name, folded(raw), "'{raw}' folds the same way");
        }
    }

    /// The self-account refusal fires on the folded name, so a differently
    /// spelled path is not a way around it, and leaves another account alone.
    #[test]
    fn an_admin_cannot_target_its_own_account() {
        let caller = Caller::Account(User {
            name: "root".to_string(),
            display: "Root".to_string(),
            email: None,
            role: Role::Admin,
            disabled: false,
        });
        for spelling in ["root", "ROOT", "  Root  "] {
            let refused = refuse_self(&caller, spelling, "delete")
                .expect_err("'{spelling}' is the caller's own account");
            assert_eq!(refused.status, StatusCode::CONFLICT);
            assert!(refused.detail.contains("your own account"));
        }
        assert!(refuse_self(&caller, "ada", "delete").is_ok());
    }

    /// The plaintext password must never be one `tracing::debug!` away, on
    /// either body.
    #[test]
    fn a_debugged_body_redacts_the_password() {
        let create: CreateBody = serde_json::from_value(json!({
            "name": "bob",
            "role": "viewer",
            "password": "hunter2",
        }))
        .unwrap();
        let text = format!("{create:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
        assert!(text.contains("bob"), "the rest of the body is still there");

        let patch: PatchBody = serde_json::from_value(json!({"password": "hunter2"})).unwrap();
        let text = format!("{patch:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");

        let empty: PatchBody = serde_json::from_value(json!({})).unwrap();
        assert!(
            format!("{empty:?}").contains("None"),
            "an absent password still reads as absent"
        );
    }

    /// An empty password is refused before it reaches the store.
    #[test]
    fn an_empty_password_is_refused() {
        assert_eq!(
            check_password("").unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(check_password(" ").is_ok(), "a space is a character");
    }
}
