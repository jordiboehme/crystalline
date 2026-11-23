//! The agents of the loops: identities of their own for the scheduled
//! reflect and evolve runs, with their own tokens.
//!
//! An agent is never a row of `users`. It lives in `agents`, its tokens in
//! `agent_tokens`, so no sign-in path (password, single sign-on, a proxy
//! header) can reach it and a listing of people needs no filter. The two
//! kinds share one namespace all the same: an account is never created under
//! an agent's name and an agent never under an account's, each checked inside
//! the transaction that inserts the row (`AuthStore::refuse_agent_name` on
//! every account insert, the `users` probe in [`AuthStore::add_agent`]). The
//! orphan prune and the sweep before a name is used again cover the agent
//! tokens, so a name used again inherits nothing.
//!
//! A token is [`AGENT_TOKEN_PREFIX`] plus 64 hex characters, shown once and
//! kept as its sha256 like every other credential here. It expires
//! ([`AGENT_TOKEN_DEFAULT_TTL_SECS`] unless asked otherwise), records when it
//! was last presented, and is revoked by stamping `revoked_at`, so a listing
//! still shows it. An agent holds at most [`AGENT_LIVE_TOKENS_MAX`] live
//! tokens at once: enough to roll one over without a gap, and no more.

use anyhow::{Context, Result, anyhow};
use crystalline_core::time::{self, Stamp};
use turso::Value;

use super::{
    AuthRow, AuthStore, PRUNE_AGENT_TOKENS, RefusalKind, cell_text, normalize_account_name,
    normalize_display_name, normalize_new_account_name, random_hex, refuse, rfc3339_from_unix,
    token_hash,
};
use crate::scope::Scope;

/// Prefix every agent token is minted with, so it is told apart from a
/// personal MCP token (`cmt_`) and an OAuth token at a glance and by the
/// gates before any lookup.
pub const AGENT_TOKEN_PREFIX: &str = "cag_";

/// What an agent and the admin making one are told while `auth.mcp` is off:
/// the open tier reads no token, so an agent token would open nothing. Said
/// by `crystalline agents add`, `POST /api/v1/agents`, the MCP door and the
/// ctl route.
pub const AGENTS_NEED_AUTH_MCP: &str = "Agents need auth.mcp on, and it is off on this instance, so an agent token opens nothing here. An admin turns it on with 'crystalline config set auth.mcp true' (or CRYSTALLINE_AUTH_MCP=true) and restarts the server.";

/// How long an agent token lives when nobody asks otherwise: 90 days.
pub const AGENT_TOKEN_DEFAULT_TTL_SECS: i64 = 90 * 86_400;

/// The longest life an agent token may be given: 365 days.
pub const AGENT_TOKEN_MAX_TTL_SECS: i64 = 365 * 86_400;

/// The shortest life an agent token may be given: one day.
pub const AGENT_TOKEN_MIN_TTL_SECS: i64 = 86_400;

/// How many live (not revoked, not expired) tokens an agent may hold at once.
pub const AGENT_LIVE_TOKENS_MAX: i64 = 2;

/// Which loops an agent runs. Chosen per agent; both by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLoops {
    /// Only the reflect loop, which curates the Reflection.
    Reflect,
    /// Only the evolve loop, which writes engrams.
    Evolve,
    /// Both loops.
    #[default]
    Both,
}

impl AgentLoops {
    /// The word the store, the API and the CLI use.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentLoops::Reflect => "reflect",
            AgentLoops::Evolve => "evolve",
            AgentLoops::Both => "both",
        }
    }

    /// Whether the reflect loop is one of them.
    pub fn reflect(self) -> bool {
        matches!(self, AgentLoops::Reflect | AgentLoops::Both)
    }

    /// Whether the evolve loop is one of them.
    pub fn evolve(self) -> bool {
        matches!(self, AgentLoops::Evolve | AgentLoops::Both)
    }

    /// The loops a comma-separated list names: `reflect`, `evolve`, both of
    /// them in either order, or `both`. `None` for anything else, an empty
    /// list included.
    pub fn parse(list: &str) -> Option<AgentLoops> {
        let mut reflect = false;
        let mut evolve = false;
        for word in list.split(',').map(str::trim) {
            match word.to_ascii_lowercase().as_str() {
                "reflect" => reflect = true,
                "evolve" => evolve = true,
                "both" => {
                    reflect = true;
                    evolve = true;
                }
                _ => return None,
            }
        }
        match (reflect, evolve) {
            (true, true) => Some(AgentLoops::Both),
            (true, false) => Some(AgentLoops::Reflect),
            (false, true) => Some(AgentLoops::Evolve),
            (false, false) => None,
        }
    }
}

impl std::fmt::Display for AgentLoops {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where one agent token stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTokenState {
    /// Not revoked and not expired: it opens what the agent may reach.
    Live,
    /// Its time ran out.
    Expired,
    /// Revoked by an admin.
    Revoked,
}

/// One token of an agent, never carrying the token itself.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AgentToken {
    /// The row id, which `agents revoke` names.
    pub id: i64,
    /// What the admin called it.
    pub label: String,
    /// When it was issued, RFC 3339.
    pub created_at: String,
    /// When it stops working, RFC 3339.
    pub expires_at: String,
    /// When it was last presented, RFC 3339, or `None` when never.
    pub last_used: Option<String>,
    /// When it was revoked, RFC 3339, or `None`.
    pub revoked_at: Option<String>,
    /// Live, expired or revoked, as of the read.
    pub state: AgentTokenState,
}

/// One agent with its tokens, newest first.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Agent {
    /// The unique name, folded like a login name. This is what a commit, a
    /// lease and `generated.by` carry.
    pub name: String,
    /// The name shown to people.
    pub display: String,
    /// The loops it runs.
    pub loops: AgentLoops,
    /// When it was created, RFC 3339.
    pub created_at: String,
    /// Who created it: the admin's account, or `cli` for `crystalline agents`.
    pub created_by: String,
    /// A disabled agent keeps its rows, and none of its tokens opens anything.
    pub disabled: bool,
    /// Its tokens, live or not, newest first.
    pub tokens: Vec<AgentToken>,
}

/// A freshly issued agent token. `token` is the only unhashed copy, shown
/// once.
#[derive(Clone)]
pub struct IssuedAgentToken {
    /// The row id.
    pub id: i64,
    /// The token itself: [`AGENT_TOKEN_PREFIX`] plus 64 hex characters.
    pub token: String,
    /// Its label.
    pub label: String,
    /// When it stops working, RFC 3339.
    pub expires_at: String,
}

/// Hand-written so the token never reaches a log line, as for
/// [`super::IssuedMcpToken`].
impl std::fmt::Debug for IssuedAgentToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedAgentToken")
            .field("id", &self.id)
            .field("token", &"cag_[redacted]")
            .field("label", &self.label)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// What a live agent token resolves to ([`AuthStore::agent_for_token`]).
#[derive(Clone, Debug, PartialEq)]
pub struct AgentSession {
    /// The agent's unique name.
    pub name: String,
    /// Its display name.
    pub display: String,
    /// The loops it runs.
    pub loops: AgentLoops,
    /// The id of the token that was presented.
    pub token_id: i64,
    /// When that token stops working, RFC 3339.
    pub token_expires_at: String,
}

impl AgentSession {
    /// The scope a request carrying this agent's token acts in.
    pub fn scope(&self) -> Scope {
        Scope::Agent {
            name: self.name.clone(),
            loops: self.loops,
            token_id: self.token_id,
            token_expires_at: self.token_expires_at.clone(),
        }
    }
}

/// Check a token's life in seconds against [`AGENT_TOKEN_MIN_TTL_SECS`] and
/// [`AGENT_TOKEN_MAX_TTL_SECS`].
pub fn check_agent_token_ttl(ttl_secs: i64) -> Result<i64> {
    if (AGENT_TOKEN_MIN_TTL_SECS..=AGENT_TOKEN_MAX_TTL_SECS).contains(&ttl_secs) {
        return Ok(ttl_secs);
    }
    Err(refuse(
        RefusalKind::InvalidTokenExpiry,
        format!(
            "an agent token lives from 1 to {} days: pick a time in that range",
            AGENT_TOKEN_MAX_TTL_SECS / 86_400
        ),
    ))
}

/// The label a token gets when it was given none: the day it was issued.
fn default_label(label: &str, now: Stamp) -> String {
    let label = label.trim();
    if label.is_empty() {
        let day: String = now.rfc3339().chars().take(10).collect();
        return format!("issued {day}");
    }
    label.to_string()
}

const AGENT_COLUMNS: &str = "name, display, loops, created_at, created_by, disabled";

/// The agent a row of [`AGENT_COLUMNS`] holds, without its tokens.
fn agent_from_row(row: &AuthRow) -> Result<Agent> {
    let name = cell_text(row, 0).unwrap_or_default();
    let raw = cell_text(row, 2).unwrap_or_default();
    let loops = AgentLoops::parse(&raw).ok_or_else(|| {
        anyhow!("agent '{name}' runs loops '{raw}', which this binary does not know")
    })?;
    Ok(Agent {
        display: cell_text(row, 1).unwrap_or_default(),
        loops,
        created_at: cell_text(row, 3).unwrap_or_default(),
        created_by: cell_text(row, 4).unwrap_or_default(),
        disabled: matches!(row.get_value(5), Ok(Value::Integer(n)) if n != 0),
        tokens: Vec::new(),
        name,
    })
}

const TOKEN_COLUMNS: &str = "id, agent, label, created_at, expires_at, last_used, revoked_at";

/// The token a row of [`TOKEN_COLUMNS`] holds, and the agent it belongs to.
fn token_from_row(row: &AuthRow, now: i64) -> Option<(String, AgentToken)> {
    let Ok(Value::Integer(id)) = row.get_value(0) else {
        return None;
    };
    let expires = match row.get_value(4) {
        Ok(Value::Integer(n)) => n,
        _ => 0,
    };
    let revoked = match row.get_value(6) {
        Ok(Value::Integer(n)) => Some(n),
        _ => None,
    };
    let state = if revoked.is_some() {
        AgentTokenState::Revoked
    } else if expires <= now {
        AgentTokenState::Expired
    } else {
        AgentTokenState::Live
    };
    Some((
        cell_text(row, 1).unwrap_or_default(),
        AgentToken {
            id,
            label: cell_text(row, 2).unwrap_or_default(),
            created_at: cell_text(row, 3).unwrap_or_default(),
            expires_at: rfc3339_from_unix(expires),
            last_used: cell_text(row, 5),
            revoked_at: revoked.map(rfc3339_from_unix),
            state,
        },
    ))
}

impl AuthStore {
    /// Refuse `name` for a new account when an agent holds it. The caller
    /// holds the guard and the transaction the account insert runs in, so
    /// the answer still holds when the insert lands.
    pub(super) async fn refuse_agent_name(&self, name: &str) -> Result<()> {
        let taken = self
            .db
            .query_first(
                "SELECT 1 FROM agents WHERE name = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await?
            .is_some();
        if taken {
            return Err(refuse(
                RefusalKind::NameTaken,
                format!(
                    "the name '{name}' belongs to an agent, and accounts and agents share one \
                     namespace: pick another name"
                ),
            ));
        }
        Ok(())
    }

    /// Create an agent and issue its first token, in one transaction: an
    /// agent never stands without a way in, and a token never without its
    /// agent.
    ///
    /// The name is folded like a login name and refused when an account or
    /// another agent holds it ([`RefusalKind::NameTaken`]), both checked in
    /// the transaction. `display` defaults to the name; `created_by` names
    /// the admin (or `cli`). `ttl_secs` is the token's life, checked by
    /// [`check_agent_token_ttl`].
    pub async fn add_agent(
        &self,
        name: &str,
        display: Option<&str>,
        loops: AgentLoops,
        created_by: &str,
        label: &str,
        ttl_secs: i64,
    ) -> Result<(Agent, IssuedAgentToken)> {
        self.add_agent_at(
            name,
            display,
            loops,
            created_by,
            label,
            ttl_secs,
            time::now(),
        )
        .await
    }

    /// [`AuthStore::add_agent`] at `now`, the seam the expiry tests use.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn add_agent_at(
        &self,
        name: &str,
        display: Option<&str>,
        loops: AgentLoops,
        created_by: &str,
        label: &str,
        ttl_secs: i64,
        now: Stamp,
    ) -> Result<(Agent, IssuedAgentToken)> {
        let name = normalize_new_account_name(name)?;
        let display = normalize_display_name(&name, display)?;
        let ttl_secs = check_agent_token_ttl(ttl_secs)?;
        let label = default_label(label, now);
        let _guard = self.lock().await;
        self.db
            .begin_immediate()
            .await
            .with_context(|| format!("adding agent '{name}'"))?;
        let result = async {
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
                    format!(
                        "the name '{name}' belongs to an account, and accounts and agents share \
                         one namespace: pick another name"
                    ),
                ));
            }
            if self
                .db
                .query_first(
                    "SELECT 1 FROM agents WHERE name = ?1",
                    vec![Value::Text(name.clone())],
                )
                .await?
                .is_some()
            {
                return Err(refuse(
                    RefusalKind::NameTaken,
                    format!("an agent named '{name}' already exists"),
                ));
            }
            // The credentials an earlier account or agent of this name left
            // behind go before the name is used again: the accounts' on
            // Postgres (Turso prunes them on every lookup), and the agent
            // tokens of this name on both, because no agent holds the name
            // here and a token row left under it would be inherited.
            self.sweep_orphans_of(&name).await?;
            self.db
                .execute(
                    "DELETE FROM agent_tokens WHERE agent = ?1",
                    vec![Value::Text(name.clone())],
                )
                .await
                .with_context(|| format!("sweeping the orphaned tokens of agent '{name}'"))?;
            self.db
                .execute(
                    "INSERT INTO agents (name, display, loops, created_at, created_by, disabled)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                    vec![
                        Value::Text(name.clone()),
                        Value::Text(display.clone()),
                        Value::Text(loops.as_str().to_string()),
                        Value::Text(now.rfc3339()),
                        Value::Text(created_by.to_string()),
                    ],
                )
                .await
                .with_context(|| format!("adding agent '{name}'"))?;
            self.insert_agent_token(&name, &label, ttl_secs, now).await
        }
        .await;
        let issued = self.db.finish(result).await?;
        let agent = Agent {
            name,
            display,
            loops,
            created_at: now.rfc3339(),
            created_by: created_by.to_string(),
            disabled: false,
            tokens: vec![AgentToken {
                id: issued.id,
                label: issued.label.clone(),
                created_at: now.rfc3339(),
                expires_at: issued.expires_at.clone(),
                last_used: None,
                revoked_at: None,
                state: AgentTokenState::Live,
            }],
        };
        Ok((agent, issued))
    }

    /// Mint a token and write its row. The caller holds the guard and an open
    /// transaction, and has checked the agent exists.
    async fn insert_agent_token(
        &self,
        agent: &str,
        label: &str,
        ttl_secs: i64,
        now: Stamp,
    ) -> Result<IssuedAgentToken> {
        let token = format!("{AGENT_TOKEN_PREFIX}{}", random_hex());
        let expires = now.unix_seconds() + ttl_secs;
        let id = self
            .returned_id(
                "INSERT INTO agent_tokens (agent, token_hash, label, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
                vec![
                    Value::Text(agent.to_string()),
                    Value::Text(token_hash(&token)),
                    Value::Text(label.to_string()),
                    Value::Text(now.rfc3339()),
                    Value::Integer(expires),
                ],
            )
            .await
            .with_context(|| format!("issuing a token for agent '{agent}'"))?;
        Ok(IssuedAgentToken {
            id,
            token,
            label: label.to_string(),
            expires_at: rfc3339_from_unix(expires),
        })
    }

    /// Issue another token for an existing agent. Refused when the agent is
    /// unknown ([`RefusalKind::NoSuchAgent`]) or already holds
    /// [`AGENT_LIVE_TOKENS_MAX`] live tokens ([`RefusalKind::AgentTokenLimit`]).
    pub async fn issue_agent_token(
        &self,
        name: &str,
        label: &str,
        ttl_secs: i64,
    ) -> Result<IssuedAgentToken> {
        self.issue_agent_token_at(name, label, ttl_secs, time::now())
            .await
    }

    /// [`AuthStore::issue_agent_token`] at `now`.
    pub(super) async fn issue_agent_token_at(
        &self,
        name: &str,
        label: &str,
        ttl_secs: i64,
        now: Stamp,
    ) -> Result<IssuedAgentToken> {
        let name = normalize_account_name(name)?;
        let ttl_secs = check_agent_token_ttl(ttl_secs)?;
        let label = default_label(label, now);
        let _guard = self.lock().await;
        self.db
            .begin_immediate()
            .await
            .with_context(|| format!("issuing a token for agent '{name}'"))?;
        let result = async {
            self.require_agent(&name).await?;
            let live = match self
                .db
                .query_first(
                    "SELECT COUNT(*) FROM agent_tokens
                     WHERE agent = ?1 AND revoked_at IS NULL AND expires_at > ?2",
                    vec![
                        Value::Text(name.clone()),
                        Value::Integer(now.unix_seconds()),
                    ],
                )
                .await?
                .map(|row| row.get_value(0))
            {
                Some(Ok(Value::Integer(n))) => n,
                _ => 0,
            };
            if live >= AGENT_LIVE_TOKENS_MAX {
                return Err(refuse(
                    RefusalKind::AgentTokenLimit,
                    format!(
                        "agent '{name}' already holds {AGENT_LIVE_TOKENS_MAX} live tokens: revoke \
                         one first with crystalline agents revoke {name} <token-id>"
                    ),
                ));
            }
            self.insert_agent_token(&name, &label, ttl_secs, now).await
        }
        .await;
        self.db.finish(result).await
    }

    /// Refuse an unknown agent. The caller holds the guard.
    async fn require_agent(&self, name: &str) -> Result<()> {
        if self
            .db
            .query_first(
                "SELECT 1 FROM agents WHERE name = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await?
            .is_none()
        {
            return Err(refuse(
                RefusalKind::NoSuchAgent,
                format!("no such agent: '{name}'"),
            ));
        }
        Ok(())
    }

    /// Revoke one of `name`'s tokens by id. `false` when the id names no
    /// token of that agent or one already revoked. The row stays, stamped,
    /// so a listing still shows it.
    pub async fn revoke_agent_token(&self, name: &str, id: i64) -> Result<bool> {
        let name = normalize_account_name(name)?;
        let _guard = self.lock().await;
        let changed = self
            .db
            .execute(
                "UPDATE agent_tokens SET revoked_at = ?3
                 WHERE id = ?1 AND agent = ?2 AND revoked_at IS NULL",
                vec![
                    Value::Integer(id),
                    Value::Text(name.clone()),
                    Value::Integer(time::now().unix_seconds()),
                ],
            )
            .await
            .with_context(|| format!("revoking a token of agent '{name}'"))?;
        Ok(changed > 0)
    }

    /// Remove an agent and every token it holds, in one transaction.
    /// Refused for an unknown name ([`RefusalKind::NoSuchAgent`]).
    pub async fn remove_agent(&self, name: &str) -> Result<()> {
        let name = normalize_account_name(name)?;
        let _guard = self.lock().await;
        self.db
            .begin_immediate()
            .await
            .with_context(|| format!("removing agent '{name}'"))?;
        let result = async {
            self.db
                .execute(
                    "DELETE FROM agent_tokens WHERE agent = ?1",
                    vec![Value::Text(name.clone())],
                )
                .await
                .with_context(|| format!("removing the tokens of agent '{name}'"))?;
            let changed = self
                .db
                .execute(
                    "DELETE FROM agents WHERE name = ?1",
                    vec![Value::Text(name.clone())],
                )
                .await
                .with_context(|| format!("removing agent '{name}'"))?;
            if changed == 0 {
                return Err(refuse(
                    RefusalKind::NoSuchAgent,
                    format!("no such agent: '{name}'"),
                ));
            }
            Ok(())
        }
        .await;
        self.db.finish(result).await
    }

    /// Every agent with its tokens, by name.
    pub async fn agents(&self) -> Result<Vec<Agent>> {
        self.agents_where(None).await
    }

    /// The loops `name` runs, or `None` when there is no such agent or it is
    /// disabled: what a rights decision reads, without the tokens.
    pub async fn agent_loops(&self, name: &str) -> Result<Option<AgentLoops>> {
        let Ok(name) = normalize_account_name(name) else {
            return Ok(None);
        };
        let _guard = self.lock().await;
        let Some(row) = self
            .db
            .query_first(
                "SELECT loops, disabled FROM agents WHERE name = ?1",
                vec![Value::Text(name)],
            )
            .await
            .context("reading an agent's loops")?
        else {
            return Ok(None);
        };
        if matches!(row.get_value(1), Ok(Value::Integer(n)) if n != 0) {
            return Ok(None);
        }
        Ok(AgentLoops::parse(&cell_text(&row, 0).unwrap_or_default()))
    }

    /// The name `name` is shown under, or `None` when there is no such agent.
    pub async fn agent_display(&self, name: &str) -> Result<Option<String>> {
        let Ok(name) = normalize_account_name(name) else {
            return Ok(None);
        };
        let _guard = self.lock().await;
        let row = self
            .db
            .query_first(
                "SELECT display FROM agents WHERE name = ?1",
                vec![Value::Text(name)],
            )
            .await
            .context("reading an agent's display name")?;
        Ok(row.and_then(|row| cell_text(&row, 0)))
    }

    /// One agent with its tokens, or `None`.
    pub async fn agent(&self, name: &str) -> Result<Option<Agent>> {
        let name = normalize_account_name(name)?;
        Ok(self.agents_where(Some(&name)).await?.into_iter().next())
    }

    /// The agents (all, or the one named) with their tokens.
    async fn agents_where(&self, name: Option<&str>) -> Result<Vec<Agent>> {
        let now = time::now().unix_seconds();
        let _guard = self.lock().await;
        let (agent_sql, token_sql, params) = match name {
            Some(name) => (
                format!("SELECT {AGENT_COLUMNS} FROM agents WHERE name = ?1"),
                format!(
                    "SELECT {TOKEN_COLUMNS} FROM agent_tokens WHERE agent = ?1
                     ORDER BY created_at DESC, id DESC"
                ),
                vec![Value::Text(name.to_string())],
            ),
            None => (
                format!("SELECT {AGENT_COLUMNS} FROM agents ORDER BY name"),
                format!(
                    "SELECT {TOKEN_COLUMNS} FROM agent_tokens ORDER BY created_at DESC, id DESC"
                ),
                vec![],
            ),
        };
        let mut agents = Vec::new();
        for row in self
            .db
            .query(&agent_sql, params.clone())
            .await
            .context("listing agents")?
        {
            agents.push(agent_from_row(&row)?);
        }
        for row in self
            .db
            .query(&token_sql, params)
            .await
            .context("listing agent tokens")?
        {
            if let Some((agent, token)) = token_from_row(&row, now)
                && let Some(owner) = agents.iter_mut().find(|a| a.name == agent)
            {
                owner.tokens.push(token);
            }
        }
        Ok(agents)
    }

    /// Resolve an agent token to its agent: `None` for an unknown, revoked or
    /// expired token and for one of a disabled agent, all alike, as
    /// [`AuthStore::mcp_token_user`] does for accounts. Stamps `last_used` on
    /// a hit. Prunes the tokens of agents that are gone first (on Turso on
    /// every call, on Postgres with the hourly prune).
    pub async fn agent_for_token(&self, token: &str) -> Result<Option<AgentSession>> {
        self.agent_for_token_at(token, time::now()).await
    }

    /// [`AuthStore::agent_for_token`] at `now`.
    pub(super) async fn agent_for_token_at(
        &self,
        token: &str,
        now: Stamp,
    ) -> Result<Option<AgentSession>> {
        if !token.starts_with(AGENT_TOKEN_PREFIX) {
            return Ok(None);
        }
        let hash = token_hash(token);
        let _guard = self.lock().await;
        if self.db.is_postgres() {
            self.prune_orphans_if_due().await?;
        } else {
            self.db
                .execute(PRUNE_AGENT_TOKENS, vec![])
                .await
                .context("pruning orphaned agent tokens")?;
        }
        let Some(row) = self
            .db
            .query_first(
                "SELECT a.name, a.display, a.loops, a.disabled, t.id, t.expires_at
                 FROM agent_tokens t JOIN agents a ON a.name = t.agent
                 WHERE t.token_hash = ?1 AND t.revoked_at IS NULL AND t.expires_at > ?2",
                vec![Value::Text(hash), Value::Integer(now.unix_seconds())],
            )
            .await?
        else {
            return Ok(None);
        };
        if matches!(row.get_value(3), Ok(Value::Integer(n)) if n != 0) {
            return Ok(None);
        }
        let Some(loops) = AgentLoops::parse(&cell_text(&row, 2).unwrap_or_default()) else {
            return Ok(None);
        };
        let Ok(Value::Integer(token_id)) = row.get_value(4) else {
            return Ok(None);
        };
        let Ok(Value::Integer(expires)) = row.get_value(5) else {
            return Ok(None);
        };
        self.stamp_used("agent_tokens", token_id)
            .await
            .context("stamping an agent token's last_used")?;
        Ok(Some(AgentSession {
            name: cell_text(&row, 0).unwrap_or_default(),
            display: cell_text(&row, 1).unwrap_or_default(),
            loops,
            token_id,
            token_expires_at: rfc3339_from_unix(expires),
        }))
    }
}
