//! `crystalline agents`: the agents of the loops and their tokens.
//!
//! An agent is an identity of its own for the scheduled reflect and evolve
//! runs, never an account: it cannot sign in, it does not show up in
//! `crystalline users list`, and its name is taken from the one namespace it
//! shares with the accounts. Every command here writes the accounts store
//! directly, exactly as `crystalline users` does, so it works with or without
//! a running daemon and beside an instance that owns a PostgreSQL database
//! ([`crystalline_service::open_accounts_for_cli`]). A running daemon sees a
//! change on its next lookup.
//!
//! A token is printed once, when it is issued. Only its hash is stored.

use anyhow::{Result, anyhow, bail};

use crystalline_service::rest::{
    AGENT_TOKEN_MAX_TTL_SECS, Agent, AgentLoops, AgentTokenState, IssuedAgentToken,
};

use crate::AgentsCommand;

/// Who an agent made here was created by.
const CREATED_BY_CLI: &str = "cli";

/// What to do with a freshly printed token.
const TOKEN_TEACHING: &str = "Give it to the loop runner as 'Authorization: Bearer <token>'. \
It is shown only now.";

/// Run one `agents` subcommand against the accounts store of the
/// configuration.
pub async fn run(command: AgentsCommand, json: bool) -> Result<()> {
    let loaded = crate::cmd::load(None)?;
    // `add` clears the name first as `users add` does, which may delete a
    // GitHub token left under it; where that happens is decided up front.
    let route = match &command {
        AgentsCommand::Add { .. } => {
            crystalline_service::route_secret_command(&loaded.effective).await?
        }
        _ => crystalline_service::SecretRoute::Local,
    };
    let store = crystalline_service::open_accounts_for_cli(&loaded.effective).await?;
    match command {
        AgentsCommand::Add {
            name,
            display,
            loops,
            expires,
            label,
        } => {
            // An agent token opens nothing while `auth.mcp` is off.
            if !loaded.effective.auth_mcp() {
                bail!("{}", crystalline_service::rest::AGENTS_NEED_AUTH_MCP);
            }
            let loops = parse_loops(&loops)?;
            let ttl = parse_expires(&expires)?;
            crate::users::clear_name_for_new_account(&store, &name, &route).await?;
            let (agent, issued) = store
                .add_agent(
                    &name,
                    display.as_deref(),
                    loops,
                    CREATED_BY_CLI,
                    label.as_deref().unwrap_or_default(),
                    ttl,
                )
                .await?;
            if json {
                crate::print_value(
                    &serde_json::json!({
                        "agent": agent,
                        "token": token_json(&issued),
                    }),
                    true,
                );
                eprintln!("{TOKEN_TEACHING}");
            } else {
                println!(
                    "Added agent '{}' for {}.",
                    agent.name,
                    loops_words(agent.loops)
                );
                print_token(&issued);
            }
        }
        AgentsCommand::List => {
            let agents = store.agents().await?;
            if json {
                crate::print_value(&serde_json::json!({ "agents": agents }), true);
            } else {
                print_agents(&agents);
            }
        }
        AgentsCommand::Token {
            name,
            expires,
            label,
        } => {
            let ttl = parse_expires(&expires)?;
            let issued = store
                .issue_agent_token(&name, label.as_deref().unwrap_or_default(), ttl)
                .await?;
            if json {
                crate::print_value(&token_json(&issued), true);
                eprintln!("{TOKEN_TEACHING}");
            } else {
                print_token(&issued);
            }
        }
        AgentsCommand::Revoke { name, token_id } => {
            if !store.revoke_agent_token(&name, token_id).await? {
                bail!(
                    "agent '{}' holds no unrevoked token {token_id}; see crystalline agents list",
                    folded(&name)
                );
            }
            if json {
                crate::print_value(&serde_json::json!({ "revoked": token_id }), true);
            } else {
                println!("Revoked token {token_id} of agent '{}'.", folded(&name));
            }
        }
        AgentsCommand::Remove { name } => {
            store.remove_agent(&name).await?;
            // An agent's drafts go with it, unlike a person's: nobody comes
            // back for them, and its name would stay refused for a new account
            // or agent. The same end `users discard-drafts` gives a removed
            // account's drafts, and the links it gave on them end too.
            store.end_overlay_grants_of_owner(&name).await?;
            let report = crystalline_service::client::discard_account_drafts(&folded(&name))
                .await
                .map_err(|e| {
                    anyhow!(
                        "the agent was removed, but its drafts could not be ended: {e:#}. End them with: crystalline users discard-drafts {}",
                        folded(&name)
                    )
                })?;
            let rows = report["domains"].as_array().cloned().unwrap_or_default();
            let kept: Vec<String> = rows
                .iter()
                .filter(|r| {
                    r.get("error").is_some()
                        || r["left"].as_array().is_some_and(|l| !l.is_empty())
                        || r.get("not_hosted").is_some()
                })
                .filter_map(|r| r["domain"].as_str().map(str::to_string))
                .collect();
            if !kept.is_empty() || report.get("error").is_some() {
                eprintln!(
                    "Not every draft of '{}' could be ended ({}). End the rest with: crystalline users discard-drafts {}",
                    folded(&name),
                    if kept.is_empty() {
                        "the kept drafts".to_string()
                    } else {
                        kept.join(", ")
                    },
                    folded(&name)
                );
            }
            if json {
                crate::print_value(&serde_json::json!({ "removed": folded(&name) }), true);
            } else {
                println!("Removed agent '{}' and its tokens.", folded(&name));
            }
        }
    }
    Ok(())
}

/// The name as the store keys it, for a message.
fn folded(name: &str) -> String {
    crystalline_service::rest::normalize_account_name(name)
        .unwrap_or_else(|_| name.trim().to_string())
}

/// The loops `--loops` names.
fn parse_loops(raw: &str) -> Result<AgentLoops> {
    AgentLoops::parse(raw).ok_or_else(|| {
        anyhow!("--loops takes reflect, evolve or both as reflect,evolve; got '{raw}'")
    })
}

/// A token's life from `--expires`: `90d` or `90`, in days, 1 to 365.
fn parse_expires(raw: &str) -> Result<i64> {
    let max = AGENT_TOKEN_MAX_TTL_SECS / 86_400;
    let trimmed = raw.trim();
    let digits = trimmed.strip_suffix('d').unwrap_or(trimmed);
    match digits.parse::<i64>() {
        Ok(days) if (1..=max).contains(&days) => Ok(days * 86_400),
        _ => bail!("--expires takes a number of days from 1 to {max}, like 90d; got '{raw}'"),
    }
}

/// The loops in words.
fn loops_words(loops: AgentLoops) -> &'static str {
    match loops {
        AgentLoops::Reflect => "the reflect loop",
        AgentLoops::Evolve => "the evolve loop",
        AgentLoops::Both => "the reflect and evolve loops",
    }
}

/// A token as `--json` prints it.
fn token_json(issued: &IssuedAgentToken) -> serde_json::Value {
    serde_json::json!({
        "id": issued.id,
        "token": issued.token,
        "label": issued.label,
        "expires_at": issued.expires_at,
    })
}

/// Print a freshly issued token, the one time anybody sees it.
fn print_token(issued: &IssuedAgentToken) {
    println!(
        "Token {} ('{}'), valid until {}:",
        issued.id, issued.label, issued.expires_at
    );
    println!("{}", issued.token);
    println!("{TOKEN_TEACHING}");
}

/// One block per agent and one line per token.
fn print_agents(agents: &[Agent]) {
    if agents.is_empty() {
        println!("No agents. Add one with: crystalline agents add <name>");
        return;
    }
    for agent in agents {
        let disabled = if agent.disabled { ", disabled" } else { "" };
        println!(
            "{} ({}): {}{disabled}",
            agent.name,
            agent.display,
            loops_words(agent.loops)
        );
        if agent.tokens.is_empty() {
            println!("  no tokens");
        }
        for token in &agent.tokens {
            let state = match token.state {
                AgentTokenState::Live => format!("live until {}", token.expires_at),
                AgentTokenState::Expired => format!("expired {}", token.expires_at),
                AgentTokenState::Revoked => format!(
                    "revoked {}",
                    token.revoked_at.as_deref().unwrap_or_default()
                ),
            };
            let used = token
                .last_used
                .as_deref()
                .map_or("never used".to_string(), |at| format!("last used {at}"));
            println!("  {} '{}': {state}, {used}", token.id, token.label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_reads_days() {
        assert_eq!(parse_expires("90d").unwrap(), 90 * 86_400);
        assert_eq!(parse_expires("7").unwrap(), 7 * 86_400);
        assert!(parse_expires("0d").is_err());
        assert!(parse_expires("366d").is_err());
        assert!(parse_expires("12h").is_err());
    }

    #[test]
    fn loops_read_a_list() {
        assert_eq!(parse_loops("reflect,evolve").unwrap(), AgentLoops::Both);
        assert_eq!(parse_loops("reflect").unwrap(), AgentLoops::Reflect);
        assert!(parse_loops("nap").is_err());
    }
}
