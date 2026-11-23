//! Invite links: a single-use way into this instance that an admin (or,
//! where `auth.invites.managers` allows it, the owner or a manager of a
//! private domain) hands to a person to copy, never mailed.
//!
//! A link carries the instance role an account it creates gets, optionally a
//! private domain and the membership level there, a label, whether it may
//! create a local account where an identity provider would otherwise be the
//! only way in, its creator and its expiry. The token is
//! [`INVITE_TOKEN_PREFIX`] plus 32 random bytes in URL-safe base64, shown
//! once in the create answer and kept as its sha256 like every other
//! credential here.
//!
//! Two caps hold at creation, both checked inside the transaction that
//! inserts the row: a creator holds at most a given number of pending links
//! (the REST layer passes [`INVITE_PENDING_MAX_ADMIN`] or
//! [`INVITE_PENDING_MAX_MANAGER`]), and the accounts plus every pending link
//! may not reach `auth.max_users`, since each pending link may become an
//! account. A pending link is one not used, not revoked and not expired.
//!
//! No name is taken when a link is made: the account a link creates is named
//! at redemption, where the shared namespace of accounts and agents is
//! checked. A removed account's links go with it, so a name used again
//! inherits none of them.
//!
//! Redemption ([`AuthStore::redeem_invite`]) is one transaction: the link is
//! marked used only while it is unused, unrevoked and unexpired, the account
//! is created or resolved, and the membership is written, raised and never
//! lowered. The link's role applies only to an account the redemption
//! creates. What the link gives is checked again at that moment against its
//! creator: still an enabled account, still an admin where the link needs
//! one, still managing the domain at the link's level, and the domain still
//! private. An account is never matched by its address.

use anyhow::{Context, Result};
use base64::Engine as _;
use crystalline_core::time::{self, Stamp};
use turso::Value;

use super::{
    AuthRow, AuthStore, LINKED_BY_JIT, MemberLevel, PRINCIPAL_USER, RefusalKind, Role,
    USER_COLUMNS, User, cell_text, hash_password, identity_value, member_level_from_db,
    normalize_account_name, normalize_display_name, normalize_new_account_name,
    provided_display_name, random_bytes, refuse, rfc3339_from_unix, token_hash, user_from_row,
};

/// Prefix every invite token is minted with, so it is told apart from every
/// other credential at a glance.
pub const INVITE_TOKEN_PREFIX: &str = "cin_";

/// How long a link lives when nobody asks otherwise: 7 days.
pub const INVITE_DEFAULT_TTL_SECS: i64 = 7 * 86_400;

/// The longest life a link may be given: 30 days.
pub const INVITE_MAX_TTL_SECS: i64 = 30 * 86_400;

/// The shortest life a link may be given: one hour.
pub const INVITE_MIN_TTL_SECS: i64 = 3_600;

/// How many pending links one admin may hold at once.
pub const INVITE_PENDING_MAX_ADMIN: i64 = 50;

/// How many pending links one domain owner or manager may hold at once.
pub const INVITE_PENDING_MAX_MANAGER: i64 = 10;

/// The longest label a link may carry, in characters.
pub const INVITE_LABEL_MAX_CHARS: usize = 100;

/// Where one invite link stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteState {
    /// Not used, not revoked and not expired: it still opens the way in.
    Pending,
    /// Redeemed.
    Used,
    /// Revoked before it was used.
    Revoked,
    /// Its time ran out before it was used.
    Expired,
}

impl InviteState {
    /// The word the store and the API use.
    pub fn as_str(self) -> &'static str {
        match self {
            InviteState::Pending => "pending",
            InviteState::Used => "used",
            InviteState::Revoked => "revoked",
            InviteState::Expired => "expired",
        }
    }
}

/// One invite link, never carrying its token.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Invite {
    /// The row id, which a revoke names.
    pub id: i64,
    /// The instance role an account this link creates gets.
    pub role: Role,
    /// The private domain the link gives membership of, if any.
    pub domain: Option<String>,
    /// The membership level on that domain; set exactly when `domain` is.
    pub level: Option<MemberLevel>,
    /// What its creator called it.
    pub label: Option<String>,
    /// Whether it may create a local account with a password where an
    /// identity provider would otherwise be the only way in.
    pub local_account: bool,
    /// The account that made it.
    pub created_by: String,
    /// When it was made, RFC 3339.
    pub created_at: String,
    /// When it stops working, RFC 3339.
    pub expires_at: String,
    /// When it was used, RFC 3339, or `None`.
    pub used_at: Option<String>,
    /// The account that used it, or `None`.
    pub used_by: Option<String>,
    /// When it was revoked, RFC 3339, or `None`.
    pub revoked_at: Option<String>,
    /// Pending, used, revoked or expired, as of the read.
    pub state: InviteState,
}

/// What a new link is made of. The caller has decided that its creator may
/// make it; the store checks only the shape and the caps.
#[derive(Clone, Debug)]
pub struct NewInvite<'a> {
    /// The instance role for an account the link creates. Never admin.
    pub role: Role,
    /// The private domain to join, or `None`.
    pub domain: Option<&'a str>,
    /// The level there; given exactly when `domain` is, and never `none`.
    pub level: Option<MemberLevel>,
    /// A label, trimmed; empty means none.
    pub label: Option<&'a str>,
    /// Whether it may create a local account.
    pub local_account: bool,
    /// The creating account.
    pub created_by: &'a str,
    /// The life in seconds, [`INVITE_MIN_TTL_SECS`] to
    /// [`INVITE_MAX_TTL_SECS`].
    pub ttl_secs: i64,
}

/// The two caps a new link is checked against.
#[derive(Clone, Copy, Debug)]
pub struct InviteLimits {
    /// How many pending links the creator may hold, this one included.
    pub per_creator: i64,
    /// `auth.max_users`: the accounts plus every pending link stay below it.
    pub max_users: usize,
}

/// A freshly made link. `token` is the only unhashed copy, shown once.
#[derive(Clone)]
pub struct IssuedInvite {
    /// The link as stored.
    pub invite: Invite,
    /// The token: [`INVITE_TOKEN_PREFIX`] plus 43 URL-safe base64 characters.
    pub token: String,
}

/// Hand-written so the token never reaches a log line.
impl std::fmt::Debug for IssuedInvite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedInvite")
            .field("invite", &self.invite)
            .field("token", &"cin_[redacted]")
            .finish()
    }
}

/// Refuse a link whose shape cannot work.
fn invalid(message: impl Into<String>) -> anyhow::Error {
    refuse(RefusalKind::InvalidInvite, message.into())
}

/// The label as stored: trimmed, `None` when empty, refused when too long or
/// holding a control character.
fn checked_label(label: Option<&str>) -> Result<Option<String>> {
    let Some(label) = label.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > INVITE_LABEL_MAX_CHARS {
        return Err(invalid(format!(
            "an invite label holds at most {INVITE_LABEL_MAX_CHARS} characters"
        )));
    }
    if label.chars().any(char::is_control) {
        return Err(invalid("an invite label cannot hold a control character"));
    }
    Ok(Some(label.to_string()))
}

/// Check what a link is made of, before any statement runs.
fn check_new(new: &NewInvite<'_>) -> Result<Option<String>> {
    if new.role == Role::Admin {
        return Err(invalid(
            "an invite link gives a role below admin: none, viewer or editor",
        ));
    }
    match (new.domain, new.level) {
        (Some(_), Some(MemberLevel::None)) => {
            return Err(invalid(
                "an invite link gives a level that sees the domain: viewer, editor or manager",
            ));
        }
        (Some(_), Some(_)) | (None, None) => {}
        (Some(_), None) => return Err(invalid("an invite link to a domain names a level")),
        (None, Some(_)) => return Err(invalid("a level needs a domain to apply to")),
    }
    if !(INVITE_MIN_TTL_SECS..=INVITE_MAX_TTL_SECS).contains(&new.ttl_secs) {
        return Err(invalid(format!(
            "an invite link lives from one hour to {} days: pick a time in that range",
            INVITE_MAX_TTL_SECS / 86_400
        )));
    }
    checked_label(new.label)
}

/// A fresh token: the prefix and 32 random bytes in URL-safe base64.
fn new_token() -> String {
    let bytes = random_bytes::<32>();
    format!(
        "{INVITE_TOKEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

const INVITE_COLUMNS: &str = "id, role, domain, level, label, local_account, created_by, \
                              created_at, expires_at, used_at, used_by, revoked_at";

/// The link a row of [`INVITE_COLUMNS`] holds, its state as of `now`.
fn invite_from_row(row: &AuthRow, now: i64) -> Option<Invite> {
    let Ok(Value::Integer(id)) = row.get_value(0) else {
        return None;
    };
    let int = |idx: usize| match row.get_value(idx) {
        Ok(Value::Integer(n)) => Some(n),
        _ => None,
    };
    let role = cell_text(row, 1)?.parse::<Role>().ok()?;
    let level = match cell_text(row, 3) {
        Some(raw) => Some(raw.parse::<MemberLevel>().ok()?),
        None => None,
    };
    let expires = int(8).unwrap_or(0);
    let used = int(9);
    let revoked = int(11);
    let state = if used.is_some() {
        InviteState::Used
    } else if revoked.is_some() {
        InviteState::Revoked
    } else if expires <= now {
        InviteState::Expired
    } else {
        InviteState::Pending
    };
    Some(Invite {
        id,
        role,
        domain: cell_text(row, 2),
        level,
        label: cell_text(row, 4),
        local_account: int(5).is_some_and(|n| n != 0),
        created_by: cell_text(row, 6).unwrap_or_default(),
        created_at: cell_text(row, 7).unwrap_or_default(),
        expires_at: rfc3339_from_unix(expires),
        used_at: used.map(rfc3339_from_unix),
        used_by: cell_text(row, 10),
        revoked_at: revoked.map(rfc3339_from_unix),
        state,
    })
}

impl AuthStore {
    /// Make an invite link and answer it with its token, this once.
    ///
    /// Refused when its shape cannot work ([`RefusalKind::InvalidInvite`]:
    /// an admin role, a domain without a level or a level without a domain,
    /// a level of `none`, a life outside one hour to 30 days, a label too
    /// long or with a control character), when the creator already holds
    /// `limits.per_creator` pending links ([`RefusalKind::InviteLimit`]), and
    /// when the accounts plus the pending links already reach
    /// `limits.max_users` ([`RefusalKind::CapReached`]). Both counts are read
    /// in the transaction that inserts the row.
    pub async fn create_invite(
        &self,
        new: &NewInvite<'_>,
        limits: InviteLimits,
    ) -> Result<IssuedInvite> {
        self.create_invite_at(new, limits, time::now()).await
    }

    /// [`AuthStore::create_invite`] at `now`, the seam the expiry tests use.
    pub(super) async fn create_invite_at(
        &self,
        new: &NewInvite<'_>,
        limits: InviteLimits,
        now: Stamp,
    ) -> Result<IssuedInvite> {
        let label = check_new(new)?;
        let creator = new.created_by.to_string();
        let token = new_token();
        let expires = now.unix_seconds() + new.ttl_secs;
        let _guard = self.lock().await;
        self.db
            .begin_immediate()
            .await
            .context("making an invite link")?;
        let result = async {
            let pending_of_creator = self
                .count(
                    "SELECT COUNT(*) FROM invites WHERE created_by = ?1 AND used_at IS NULL
                     AND revoked_at IS NULL AND expires_at > ?2",
                    vec![
                        Value::Text(creator.clone()),
                        Value::Integer(now.unix_seconds()),
                    ],
                )
                .await?;
            if pending_of_creator >= limits.per_creator {
                return Err(refuse(
                    RefusalKind::InviteLimit,
                    format!(
                        "you hold {pending_of_creator} pending invite links, the most allowed: \
                         revoke one or wait for one to expire"
                    ),
                ));
            }
            let accounts = self.count("SELECT COUNT(*) FROM users", vec![]).await?;
            let pending = self
                .count(
                    "SELECT COUNT(*) FROM invites WHERE used_at IS NULL
                     AND revoked_at IS NULL AND expires_at > ?1",
                    vec![Value::Integer(now.unix_seconds())],
                )
                .await?;
            let cap = limits.max_users;
            if usize::try_from(accounts + pending).unwrap_or(usize::MAX) >= cap {
                return Err(refuse(
                    RefusalKind::CapReached,
                    format!(
                        "refusing an invite link: the accounts ({accounts}) and the pending \
                         invite links ({pending}) already reach the account cap \
                         (auth.max_users = {cap})"
                    ),
                ));
            }
            self.returned_id(
                "INSERT INTO invites (token_hash, role, domain, level, label, local_account,
                 created_by, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
                vec![
                    Value::Text(token_hash(&token)),
                    Value::Text(new.role.as_str().to_string()),
                    new.domain
                        .map_or(Value::Null, |d| Value::Text(d.to_string())),
                    new.level
                        .map_or(Value::Null, |l| Value::Text(l.as_str().to_string())),
                    label.clone().map_or(Value::Null, Value::Text),
                    Value::Integer(i64::from(new.local_account)),
                    Value::Text(creator.clone()),
                    Value::Text(now.rfc3339()),
                    Value::Integer(expires),
                ],
            )
            .await
            .context("making an invite link")
        }
        .await;
        let id = self.db.finish(result).await?;
        Ok(IssuedInvite {
            invite: Invite {
                id,
                role: new.role,
                domain: new.domain.map(str::to_string),
                level: new.level,
                label,
                local_account: new.local_account,
                created_by: creator,
                created_at: now.rfc3339(),
                expires_at: rfc3339_from_unix(expires),
                used_at: None,
                used_by: None,
                revoked_at: None,
                state: InviteState::Pending,
            },
            token,
        })
    }

    /// One `COUNT(*)` answer. The caller holds the guard.
    ///
    /// An answer that is not a number is an error rather than a zero: both
    /// callers are caps, and a cap that read nothing as zero would let
    /// everything through.
    async fn count(&self, sql: &str, params: Vec<Value>) -> Result<i64> {
        match self
            .db
            .query_first(sql, params)
            .await?
            .map(|row| row.get_value(0))
        {
            Some(Ok(Value::Integer(n))) => Ok(n),
            _ => Err(anyhow::anyhow!(
                "counting for an invite cap gave no number, so the cap cannot be checked"
            )),
        }
    }

    /// The invite links, newest first: every one, or those `created_by`
    /// made; all states, or only `state`. Never a token: only hashes are
    /// stored.
    pub async fn invites(
        &self,
        created_by: Option<&str>,
        state: Option<InviteState>,
    ) -> Result<Vec<Invite>> {
        let now = time::now().unix_seconds();
        let _guard = self.lock().await;
        let (sql, params) = match created_by {
            Some(creator) => (
                format!(
                    "SELECT {INVITE_COLUMNS} FROM invites WHERE created_by = ?1
                     ORDER BY created_at DESC, id DESC"
                ),
                vec![Value::Text(creator.to_string())],
            ),
            None => (
                format!("SELECT {INVITE_COLUMNS} FROM invites ORDER BY created_at DESC, id DESC"),
                vec![],
            ),
        };
        Ok(self
            .db
            .query(&sql, params)
            .await
            .context("listing invite links")?
            .iter()
            .filter_map(|row| invite_from_row(row, now))
            .filter(|invite| state.is_none_or(|s| invite.state == s))
            .collect())
    }

    /// Revoke an unused link by id: any link when `created_by` is `None` (an
    /// admin), else only one that account made. `false` when there is no
    /// such link, it is not the caller's, or it is used or revoked already.
    /// The row stays, stamped, so a listing still shows it.
    pub async fn revoke_invite(&self, id: i64, created_by: Option<&str>) -> Result<bool> {
        let _guard = self.lock().await;
        let now = Value::Integer(time::now().unix_seconds());
        let changed = match created_by {
            Some(creator) => {
                self.db
                    .execute(
                        "UPDATE invites SET revoked_at = ?2
                         WHERE id = ?1 AND created_by = ?3
                         AND used_at IS NULL AND revoked_at IS NULL",
                        vec![Value::Integer(id), now, Value::Text(creator.to_string())],
                    )
                    .await
            }
            None => {
                self.db
                    .execute(
                        "UPDATE invites SET revoked_at = ?2
                         WHERE id = ?1 AND used_at IS NULL AND revoked_at IS NULL",
                        vec![Value::Integer(id), now],
                    )
                    .await
            }
        }
        .context("revoking an invite link")?;
        Ok(changed > 0)
    }

    /// Revoke every unused link to `domain`, inside the caller's
    /// transaction: the domain went shared or is gone, so no link may give
    /// membership of it, nor of a later domain that takes its name.
    pub(super) async fn revoke_invites_to_domain(&self, domain: &str) -> Result<()> {
        self.db
            .execute(
                "UPDATE invites SET revoked_at = ?2
                 WHERE domain = ?1 AND used_at IS NULL AND revoked_at IS NULL",
                vec![
                    Value::Text(domain.to_string()),
                    Value::Integer(time::now().unix_seconds()),
                ],
            )
            .await
            .with_context(|| format!("revoking the invite links to domain '{domain}'"))?;
        Ok(())
    }

    /// Drop every link `name` made, inside the caller's transaction, so a
    /// name used again inherits none of them.
    pub(super) async fn delete_invites_of(&self, name: &str) -> Result<()> {
        self.db
            .execute(
                "DELETE FROM invites WHERE created_by = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing the invite links of user '{name}'"))?;
        Ok(())
    }
}

/// Who redeems an invite link.
///
/// No `Debug`: [`InviteRedeemer::Local`] carries a password.
pub enum InviteRedeemer<'a> {
    /// An account that exists already, by login name. It gets the membership
    /// only, never the link's role.
    Account(&'a str),
    /// A new account with a password, named by the person redeeming.
    Local {
        /// The login name, folded; refused when an account or an agent holds
        /// it.
        name: &'a str,
        /// The display name; the login name when absent or blank.
        display: Option<&'a str>,
        /// The password, hashed before the transaction opens.
        password: &'a str,
    },
    /// A new account for a sign-on identity this instance has not seen,
    /// linked to it in the same transaction, the way a first sign-on is.
    Linked {
        /// The provider (or the proxy) that asserted the identity.
        issuer: &'a str,
        /// Its stable identifier for the person.
        subject: &'a str,
        /// A naming hint, uniquified with `-2`, `-3` and so on when taken.
        desired_name: &'a str,
        /// The provider's display name, written this once.
        display: Option<&'a str>,
        /// The provider's address, presentation only.
        email: Option<&'a str>,
    },
}

/// What a redemption did.
#[derive(Clone, Debug)]
pub struct RedeemedInvite {
    /// The link as it now stands: used, by `user`.
    pub invite: Invite,
    /// The account that redeemed it, as stored after the redemption.
    pub user: User,
    /// Whether the redemption created that account.
    pub created: bool,
    /// The level the account holds on the link's domain now: the link's
    /// level, or a higher one it held before. The domain's owner reads as
    /// `manager`. `None` when the link names no domain.
    pub level: Option<MemberLevel>,
}

/// What a pending link offers, for the page that asks a person to accept it.
#[derive(Clone, Debug)]
pub struct InviteOffer {
    /// The link.
    pub invite: Invite,
    /// Its creator's display name.
    pub inviter_display: String,
}

/// What a redeemer has become once its input is checked, before the lock.
enum Prepared {
    Account(String),
    Local {
        name: String,
        display: String,
        hash: String,
    },
    Linked {
        issuer: String,
        subject: String,
        base: String,
        display: Option<String>,
        email: Option<String>,
    },
}

/// Membership levels least to most, for the raise-only rule and the
/// creator's level check.
fn level_rank(level: MemberLevel) -> u8 {
    match level {
        MemberLevel::None => 0,
        MemberLevel::Viewer => 1,
        MemberLevel::Editor => 2,
        MemberLevel::Manager => 3,
    }
}

/// The refusal for a link that is not pending, naming which way it ended.
fn refuse_unless_pending(invite: &Invite) -> Result<()> {
    match invite.state {
        InviteState::Pending => Ok(()),
        InviteState::Used => Err(refuse(
            RefusalKind::InviteUsed,
            "this invite link was used already".to_string(),
        )),
        InviteState::Revoked => Err(refuse(
            RefusalKind::InviteRevoked,
            "this invite link was revoked".to_string(),
        )),
        InviteState::Expired => Err(refuse(
            RefusalKind::InviteExpired,
            "this invite link has expired".to_string(),
        )),
    }
}

/// The refusal for a link whose creator may no longer give what it gives.
fn creator_lapsed() -> anyhow::Error {
    refuse(
        RefusalKind::InviteCreatorLapsed,
        "the person who made this invite link may not give what it gives any more: ask them for \
         a new one"
            .to_string(),
    )
}

impl AuthStore {
    /// The link a token names, its state as of `now`. The caller holds the
    /// guard.
    async fn invite_by_token(&self, token: &str, now: i64) -> Result<Option<Invite>> {
        Ok(self
            .db
            .query_first(
                &format!("SELECT {INVITE_COLUMNS} FROM invites WHERE token_hash = ?1"),
                vec![Value::Text(token_hash(token.trim()))],
            )
            .await
            .context("reading an invite link")?
            .and_then(|row| invite_from_row(&row, now)))
    }

    /// Check again that the link's creator may give what the link gives, and
    /// that its domain is still private. Answers the domain's owner when the
    /// link names a domain. The caller holds the guard.
    ///
    /// The creator must be an enabled account. A link that gives a role other
    /// than `none`, may create a local account or names no domain is one only
    /// an admin makes, so its creator must still be an admin. A link to a
    /// domain needs its creator to be an admin, the domain's owner, or a
    /// manager there whose level is not below the link's.
    async fn check_invite_creator(&self, invite: &Invite) -> Result<Option<String>> {
        let row = self
            .db
            .query_first(
                "SELECT role, disabled FROM users WHERE name = ?1",
                vec![Value::Text(invite.created_by.clone())],
            )
            .await
            .context("reading the maker of an invite link")?;
        let Some(row) = row else {
            return Err(creator_lapsed());
        };
        if !matches!(row.get_value(1), Ok(Value::Integer(0))) {
            return Err(creator_lapsed());
        }
        let admin = cell_text(&row, 0).and_then(|r| r.parse::<Role>().ok()) == Some(Role::Admin);
        let needs_admin =
            invite.role != Role::None || invite.local_account || invite.domain.is_none();
        if needs_admin && !admin {
            return Err(creator_lapsed());
        }
        let Some(domain) = invite.domain.as_deref() else {
            return Ok(None);
        };
        let Some(acl) = self.acl_of(domain).await? else {
            return Err(refuse(
                RefusalKind::NotPrivate,
                format!(
                    "domain '{domain}' is not private any more, so this invite link gives nothing"
                ),
            ));
        };
        if !admin && acl.owner != invite.created_by {
            let held = self
                .db
                .query_first(
                    "SELECT level FROM domain_member
                     WHERE domain = ?1 AND principal_kind = ?2 AND principal = ?3",
                    vec![
                        Value::Text(domain.to_string()),
                        Value::Text(PRINCIPAL_USER.to_string()),
                        Value::Text(invite.created_by.clone()),
                    ],
                )
                .await
                .context("reading the maker's membership")?
                .and_then(|row| cell_text(&row, 0))
                .map(|raw| member_level_from_db(&raw));
            let given = invite.level.unwrap_or(MemberLevel::Viewer);
            match held {
                Some(level)
                    if level == MemberLevel::Manager && level_rank(level) >= level_rank(given) => {}
                _ => return Err(creator_lapsed()),
            }
        }
        Ok(Some(acl.owner))
    }

    /// What a pending link offers: the link and its creator's display name,
    /// for the page that asks a person to accept it.
    ///
    /// Refused like a redemption is for the link itself
    /// ([`RefusalKind::InviteUnknown`], [`RefusalKind::InviteUsed`],
    /// [`RefusalKind::InviteRevoked`], [`RefusalKind::InviteExpired`]) and
    /// for the re-checks of its creator and domain
    /// ([`RefusalKind::InviteCreatorLapsed`], [`RefusalKind::NotPrivate`]).
    /// A surface that must not say which may answer all of them alike.
    pub async fn invite_offer(&self, token: &str) -> Result<InviteOffer> {
        let now = time::now().unix_seconds();
        let _guard = self.lock().await;
        let Some(invite) = self.invite_by_token(token, now).await? else {
            return Err(refuse(
                RefusalKind::InviteUnknown,
                "no invite link has this token".to_string(),
            ));
        };
        refuse_unless_pending(&invite)?;
        self.check_invite_creator(&invite).await?;
        let inviter_display = self
            .db
            .query_first(
                "SELECT display FROM users WHERE name = ?1",
                vec![Value::Text(invite.created_by.clone())],
            )
            .await
            .context("reading the maker of an invite link")?
            .and_then(|row| cell_text(&row, 0))
            .unwrap_or_else(|| invite.created_by.clone());
        Ok(InviteOffer {
            invite,
            inviter_display,
        })
    }

    /// Redeem an invite link, in one transaction: mark it used where it is
    /// unused, unrevoked and unexpired; create or resolve the account; write
    /// the membership, raised and never lowered.
    ///
    /// The link's role applies only to an account this creates; an existing
    /// account keeps its role and gets the membership only. An account is
    /// never matched by its address. A new account counts against
    /// `max_users` (`auth.max_users`), counting the accounts only: this link
    /// already held one of the pending places.
    ///
    /// Refused, and nothing written (the link stays pending unless it was not
    /// pending already):
    ///
    /// * the link is unknown, used, revoked or expired
    ///   ([`RefusalKind::InviteUnknown`], [`RefusalKind::InviteUsed`],
    ///   [`RefusalKind::InviteRevoked`], [`RefusalKind::InviteExpired`]);
    /// * its creator may no longer give what it gives
    ///   ([`RefusalKind::InviteCreatorLapsed`]), or its domain is no longer
    ///   private ([`RefusalKind::NotPrivate`]);
    /// * an existing account is gone or disabled
    ///   ([`RefusalKind::NoSuchAccount`]), or the link names no domain and so
    ///   gives an existing account nothing
    ///   ([`RefusalKind::InviteNeedsNewAccount`]);
    /// * a new account would pass the cap ([`RefusalKind::CapReached`]), its
    ///   name cannot be a login ([`RefusalKind::InvalidName`],
    ///   [`RefusalKind::InvalidDisplayName`]) or is held by an account or an
    ///   agent ([`RefusalKind::NameTaken`]), or the sign-on identity is linked
    ///   already ([`RefusalKind::IdentityAlreadyLinked`]).
    ///
    /// Whether the domain's access follows GitHub is not known here; the
    /// caller checks that before.
    pub async fn redeem_invite(
        &self,
        token: &str,
        redeemer: InviteRedeemer<'_>,
        max_users: usize,
    ) -> Result<RedeemedInvite> {
        self.redeem_invite_at(token, redeemer, max_users, time::now())
            .await
    }

    /// [`AuthStore::redeem_invite`] at `now`, the seam the expiry tests use.
    pub(super) async fn redeem_invite_at(
        &self,
        token: &str,
        redeemer: InviteRedeemer<'_>,
        max_users: usize,
        now: Stamp,
    ) -> Result<RedeemedInvite> {
        // Everything that needs no table is checked, and the password hashed,
        // before the lock: argon2 is CPU, not database.
        let prepared = match redeemer {
            InviteRedeemer::Account(name) => Prepared::Account(normalize_account_name(name)?),
            InviteRedeemer::Local {
                name,
                display,
                password,
            } => {
                let name = normalize_new_account_name(name)?;
                let display = normalize_display_name(&name, display)?;
                let hash = hash_password(password).await?;
                Prepared::Local {
                    name,
                    display,
                    hash,
                }
            }
            InviteRedeemer::Linked {
                issuer,
                subject,
                desired_name,
                display,
                email,
            } => Prepared::Linked {
                issuer: identity_value(issuer, "issuer")?,
                subject: identity_value(subject, "subject")?,
                base: normalize_new_account_name(desired_name)?,
                display: display.map(str::to_string),
                email: email
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            },
        };
        let now_unix = now.unix_seconds();
        let _guard = self.lock().await;
        self.db
            .begin_immediate()
            .await
            .context("redeeming an invite link")?;
        let result = async {
            let Some(invite) = self.invite_by_token(token, now_unix).await? else {
                return Err(refuse(
                    RefusalKind::InviteUnknown,
                    "no invite link has this token".to_string(),
                ));
            };
            refuse_unless_pending(&invite)?;
            let owner = self.check_invite_creator(&invite).await?;
            let (name, created) = match prepared {
                Prepared::Account(name) => {
                    let live = self
                        .db
                        .query_first(
                            "SELECT disabled FROM users WHERE name = ?1",
                            vec![Value::Text(name.clone())],
                        )
                        .await?
                        .is_some_and(|row| matches!(row.get_value(0), Ok(Value::Integer(0))));
                    if !live {
                        return Err(refuse(
                            RefusalKind::NoSuchAccount,
                            format!("account '{name}' is disabled or gone"),
                        ));
                    }
                    if invite.domain.is_none() {
                        return Err(refuse(
                            RefusalKind::InviteNeedsNewAccount,
                            "this invite link makes a new account and gives no domain, so it \
                             gives an existing account nothing"
                                .to_string(),
                        ));
                    }
                    (name, false)
                }
                Prepared::Local {
                    name,
                    display,
                    hash,
                } => {
                    self.refuse_agent_name(&name).await?;
                    if self
                        .db
                        .query_first(
                            "SELECT 1 FROM users WHERE name = ?1",
                            vec![Value::Text(name.clone())],
                        )
                        .await?
                        .is_some()
                    {
                        return Err(refuse(
                            RefusalKind::NameTaken,
                            format!("the login name '{name}' is taken: pick another"),
                        ));
                    }
                    self.refuse_past_cap(max_users).await?;
                    self.sweep_orphans_of(&name).await?;
                    self.db
                        .execute(
                            "INSERT INTO users
                                 (name, display, email, role, pass_hash, disabled, created_at)
                             VALUES (?1, ?2, NULL, ?3, ?4, 0, ?5)",
                            vec![
                                Value::Text(name.clone()),
                                Value::Text(display),
                                Value::Text(invite.role.as_str().to_string()),
                                Value::Text(hash),
                                Value::Text(now.rfc3339()),
                            ],
                        )
                        .await
                        .with_context(|| format!("adding user '{name}' for an invite link"))?;
                    (name, true)
                }
                Prepared::Linked {
                    issuer,
                    subject,
                    base,
                    display,
                    email,
                } => {
                    self.refuse_past_cap(max_users).await?;
                    let name = self.free_account_name(&base).await?;
                    self.sweep_orphans_of(&name).await?;
                    self.db
                        .execute(
                            "INSERT INTO users
                                 (name, display, email, role, pass_hash, disabled, created_at)
                             VALUES (?1, ?2, ?3, ?4, NULL, 0, ?5)",
                            vec![
                                Value::Text(name.clone()),
                                Value::Text(provided_display_name(&name, display.as_deref())),
                                email.clone().map_or(Value::Null, Value::Text),
                                Value::Text(invite.role.as_str().to_string()),
                                Value::Text(now.rfc3339()),
                            ],
                        )
                        .await
                        .with_context(|| {
                            format!("provisioning user '{name}' for an invite link")
                        })?;
                    self.insert_link(&issuer, &subject, &name, LINKED_BY_JIT)
                        .await?;
                    (name, true)
                }
            };
            let level = match (invite.domain.as_deref(), invite.level) {
                (Some(domain), Some(level)) => Some(
                    self.raise_membership(
                        domain,
                        &name,
                        level,
                        owner.as_deref(),
                        &invite.created_by,
                        &now,
                    )
                    .await?,
                ),
                _ => None,
            };
            let marked = self
                .db
                .execute(
                    "UPDATE invites SET used_at = ?2, used_by = ?3
                     WHERE id = ?1 AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?2",
                    vec![
                        Value::Integer(invite.id),
                        Value::Integer(now_unix),
                        Value::Text(name.clone()),
                    ],
                )
                .await
                .context("marking an invite link used")?;
            if marked != 1 {
                return Err(refuse(
                    RefusalKind::InviteUsed,
                    "this invite link was used already".to_string(),
                ));
            }
            let user = self
                .db
                .query_first(
                    &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
                    vec![Value::Text(name.clone())],
                )
                .await?
                .map(|row| user_from_row(&row))
                .ok_or_else(|| anyhow::anyhow!("user '{name}' vanished while redeeming a link"))?;
            Ok(RedeemedInvite {
                invite: Invite {
                    used_at: Some(rfc3339_from_unix(now_unix)),
                    used_by: Some(name),
                    state: InviteState::Used,
                    ..invite
                },
                user,
                created,
                level,
            })
        }
        .await;
        self.db.finish(result).await
    }

    /// Refuse a new account once the accounts reach `cap`. Inside the
    /// caller's transaction.
    async fn refuse_past_cap(&self, cap: usize) -> Result<()> {
        let accounts = self.count("SELECT COUNT(*) FROM users", vec![]).await?;
        if usize::try_from(accounts).unwrap_or(usize::MAX) >= cap {
            return Err(refuse(
                RefusalKind::CapReached,
                format!(
                    "refusing to create an account for an invite link: the account cap is \
                     reached (auth.max_users = {cap}). Remove unused accounts or raise the cap"
                ),
            ));
        }
        Ok(())
    }

    /// Give `account` at least `level` on `domain`, inside the caller's
    /// transaction, and answer the level it holds now. A higher level it
    /// already holds is kept, and so is its Reflection level there. The
    /// domain's `owner` holds every level and gets no row.
    async fn raise_membership(
        &self,
        domain: &str,
        account: &str,
        level: MemberLevel,
        owner: Option<&str>,
        added_by: &str,
        now: &Stamp,
    ) -> Result<MemberLevel> {
        if owner == Some(account) {
            return Ok(MemberLevel::Manager);
        }
        let key = || {
            vec![
                Value::Text(domain.to_string()),
                Value::Text(PRINCIPAL_USER.to_string()),
                Value::Text(account.to_string()),
            ]
        };
        let held = self
            .db
            .query_first(
                "SELECT level FROM domain_member
                 WHERE domain = ?1 AND principal_kind = ?2 AND principal = ?3",
                key(),
            )
            .await?
            .and_then(|row| cell_text(&row, 0))
            .map(|raw| member_level_from_db(&raw));
        if let Some(held) = held
            && level_rank(held) >= level_rank(level)
        {
            return Ok(held);
        }
        let reflection = self.member_reflection_in_tx(domain, account).await?;
        let context = || format!("adding '{account}' to domain '{domain}' for an invite link");
        self.db
            .execute(
                "DELETE FROM domain_member
                 WHERE domain = ?1 AND principal_kind = ?2 AND principal = ?3",
                key(),
            )
            .await
            .with_context(context)?;
        self.db
            .execute(
                "INSERT INTO domain_member
                     (domain, principal_kind, principal, level, reflection, added_by, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                vec![
                    Value::Text(domain.to_string()),
                    Value::Text(PRINCIPAL_USER.to_string()),
                    Value::Text(account.to_string()),
                    Value::Text(level.as_str().to_string()),
                    reflection.to_db(),
                    Value::Text(added_by.to_string()),
                    Value::Text(now.rfc3339()),
                ],
            )
            .await
            .with_context(context)?;
        Ok(level)
    }
}
