//! The people picker's search: `GET /accounts/search`.
//!
//! A domain manager is not an instance admin and cannot read `GET /users`, yet
//! needs to find the account to invite. This route is that narrow door. What
//! it answers is deliberately small: a login name, a display name and whether
//! the account is already a member of the asked domain, at most
//! [`MAX_RESULTS`] of them, for a query of at least [`MIN_QUERY_CHARS`]
//! characters. Email addresses are never in the answer, and a caller below
//! admin cannot search by them at all; an admin may find an account by its
//! whole address (never by a prefix of one).
//!
//! **Who may ask.** With `domain`, whoever may manage that domain's members
//! (its owner, a manager, an instance admin), with the same order of refusals
//! as [`super::members`]: a domain the caller may not see is 404, exactly as
//! one nobody registered, and only then a caller below manager is 403.
//! Without `domain` the search is for instance admins only.
//!
//! **Rate limit.** [`SearchLimiter`] admits [`SEARCH_BUDGET`] searches per
//! caller per [`SEARCH_WINDOW`]; the next one is 429 with a `Retry-After`.
//! A typeahead sends a request per keystroke, so the budget is generous for a
//! person and still stops a script from walking the account list.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use crystalline_identity::account_match::AccountMatcher;
use serde::Deserialize;

use super::auth::Identity;
use super::auth_store::Role;
use super::{ApiError, ApiQuery, ProblemDetail, RestState, require_domain_read};
use crate::scope::DomainRight;

/// The fewest characters a query needs; a shorter one answers an empty list.
pub const MIN_QUERY_CHARS: usize = 2;

/// The most accounts one search answers with.
pub const MAX_RESULTS: usize = 10;

/// How many searches one caller may make per [`SEARCH_WINDOW`].
pub const SEARCH_BUDGET: usize = 60;

/// The window [`SEARCH_BUDGET`] counts in.
pub const SEARCH_WINDOW: Duration = Duration::from_secs(60);

/// The sentence a refused search carries.
const TOO_MANY_SEARCHES: &str = "too many account searches: wait a moment and try again";

/// A sliding-window counter per caller: [`SEARCH_BUDGET`] searches within
/// [`SEARCH_WINDOW`].
///
/// The same shape as the registration limiter in [`super::oauth`], keyed by
/// the caller's account name instead of being global. `now` is a parameter so
/// a test can drive a whole window without sleeping. Keys are authenticated
/// account names, so the table is bounded by the number of accounts; entries
/// whose window has passed are dropped whenever one is touched. A poisoned
/// lock is taken anyway, for the reason given there.
#[derive(Debug, Default)]
pub struct SearchLimiter {
    callers: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl SearchLimiter {
    /// Take a slot for `caller` at `now`, or answer how many seconds until one
    /// frees up (never zero).
    pub fn admit(&self, caller: &str, now: Instant) -> Result<(), u64> {
        let mut callers = self.callers.lock().unwrap_or_else(|e| e.into_inner());
        callers.retain(|_, window| {
            window
                .back()
                .is_some_and(|last| now.duration_since(*last) < SEARCH_WINDOW)
        });
        let window = callers.entry(caller.to_string()).or_default();
        while let Some(oldest) = window.front() {
            if now.duration_since(*oldest) >= SEARCH_WINDOW {
                window.pop_front();
            } else {
                break;
            }
        }
        if window.len() >= SEARCH_BUDGET {
            let oldest = *window.front().expect("the budget is never zero");
            let left = SEARCH_WINDOW.saturating_sub(now.duration_since(oldest));
            return Err(left.as_secs().max(1));
        }
        window.push_back(now);
        Ok(())
    }
}

/// The query string `GET /accounts/search` takes.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// What was typed: a prefix or word prefix of a login or display name
    /// (case and accents folded). At least two characters, else the answer is
    /// empty. An admin may also give a whole email address.
    #[serde(default)]
    #[param(example = "ada")]
    q: String,
    /// The domain the person is picked for. Without it the search is for
    /// instance admins only.
    #[serde(default)]
    #[param(example = "kb")]
    domain: Option<String>,
}

/// One account the search found.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct AccountHit {
    /// The login name.
    #[schema(example = "ada")]
    name: String,
    /// The display name.
    #[schema(example = "Ada Lovelace")]
    display: String,
    /// Whether the account is already a member of the asked domain (its
    /// owner counts). Always `false` without `domain`.
    member: bool,
}

/// What `GET /accounts/search` answers with.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct AccountSearchResponse {
    /// At most ten matching enabled accounts, by login name.
    results: Vec<AccountHit>,
}

/// `GET /accounts/search` - find an account to invite or pick.
#[utoipa::path(
    get,
    path = "/api/v1/accounts/search",
    tag = "users",
    operation_id = "search_accounts",
    summary = "Find enabled accounts by typed text, for a people picker.",
    description = "With `domain`, open to whoever may manage that domain's \
                   members (its owner, a manager, an instance admin); a \
                   domain the caller may not see is answered 404, exactly as \
                   for one nobody registered, and a caller who sees it but \
                   does not manage it is 403. Without `domain`, instance \
                   admins only. `q` matches a prefix or word prefix of the \
                   login or display name, case and accents folded; an admin \
                   may also give a whole email address. A `q` shorter than \
                   two characters answers an empty list. At most ten \
                   accounts, disabled ones left out, each with whether it is \
                   already a member of `domain`. At most 60 searches per \
                   minute per caller, then 429 with `Retry-After`.",
    params(SearchQuery),
    responses(
        (status = 200, description = "The matching accounts.", body = AccountSearchResponse),
        (
            status = 401,
            description = "No account identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller may not manage the domain's members, or \
                           (without `domain`) is not an admin.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 429,
            description = "Too many searches; `Retry-After` says when to retry.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn search(
    State(state): State<RestState>,
    identity: Identity,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<Json<AccountSearchResponse>, ApiError> {
    let caller = identity.require_account()?;
    let is_admin = caller.role == Role::Admin;
    let domain = query
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    match domain {
        Some(domain) => {
            // 404 before 403, as in `members`: the existence of a private
            // domain is the secret it keeps.
            require_domain_read(&state, &identity, domain).await?;
            super::members::refuse_below(&state, &identity, domain, DomainRight::Manage, "manager")
                .await?;
        }
        None => {
            identity.require_admin()?;
        }
    }
    if query.q.trim().chars().count() < MIN_QUERY_CHARS {
        return Ok(Json(AccountSearchResponse { results: vec![] }));
    }
    state
        .search_limiter()
        .admit(&caller.name, Instant::now())
        .map_err(|seconds| ApiError::too_many_requests(TOO_MANY_SEARCHES).retry_after(seconds))?;

    let members: Vec<String> = match domain {
        Some(domain) => {
            let mut names: Vec<String> = state
                .auth
                .domain_members(domain)
                .await
                .map_err(|e| ApiError::internal(format!("listing this domain's members: {e:#}")))?
                .into_iter()
                .map(|m| m.principal)
                .collect();
            if let Some(acl) = state.auth.domain_visibility(domain).await.map_err(|e| {
                ApiError::internal(format!("reading this domain's visibility: {e:#}"))
            })? {
                names.push(acl.owner);
            }
            names
        }
        None => Vec::new(),
    };
    let matcher = AccountMatcher::new(&query.q, is_admin);
    let mut users = state
        .auth
        .list_users()
        .await
        .map_err(|e| ApiError::internal(format!("listing accounts: {e:#}")))?;
    // Agents (Task 5) are not people and must be filtered out here once they
    // exist; today every row of `list_users` is an account.
    users.retain(|u| !u.disabled && matcher.matches(&u.name, &u.display, u.email.as_deref()));
    users.sort_by(|a, b| a.name.cmp(&b.name));
    let results = users
        .into_iter()
        .take(MAX_RESULTS)
        .map(|u| AccountHit {
            member: members.contains(&u.name),
            name: u.name,
            display: u.display,
        })
        .collect();
    Ok(Json(AccountSearchResponse { results }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_per_caller_and_refills_with_the_window() {
        let limiter = SearchLimiter::default();
        let t0 = Instant::now();
        for _ in 0..SEARCH_BUDGET {
            assert!(limiter.admit("ada", t0).is_ok());
        }
        let wait = limiter.admit("ada", t0).unwrap_err();
        assert!((1..=60).contains(&wait));
        assert!(
            limiter.admit("bob", t0).is_ok(),
            "another caller has its own"
        );
        assert!(limiter.admit("ada", t0 + SEARCH_WINDOW).is_ok());
    }
}
