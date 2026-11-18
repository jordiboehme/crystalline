//! The eight automation verbs a headless runner sends: `automation_status`,
//! `automation_prompt`, `automation_lease_take`, `automation_lease_renew`,
//! `automation_lease_release`, `automation_run_record`, `automation_share`
//! and `automation_runs`.
//!
//! The control socket (the machine owner, [`Scope::Unrestricted`]) and the
//! remote ctl route (a token's account, [`Scope::User`]) share this one
//! translation from a request to the engine call, so the two doors cannot
//! drift. Each door names its own scope and localizes `domain` its own way.

use std::sync::Arc;

use crystalline_core::automation::{DEFAULT_MAX_ITEMS, Loop, RunReport};
use serde_json::Value;

use crate::control::{envelope_err, envelope_err_kind, envelope_ok};
use crate::engine::{Engine, EngineError};
use crate::scope::Scope;

/// The verbs of this module, in the order a run uses them.
pub const AUTOMATION_COMMANDS: [&str; 8] = [
    "automation_status",
    "automation_prompt",
    "automation_lease_take",
    "automation_lease_renew",
    "automation_lease_release",
    "automation_run_record",
    "automation_share",
    "automation_runs",
];

/// Runs of one `automation_runs` answer when the request names no limit.
const DEFAULT_RUNS: usize = 10;

/// The most domains one status request may name; each costs a lookup.
const MAX_STATUS_DOMAINS: usize = 100;

/// A failed verb: the message, and whether the request itself was wrong.
struct Failure {
    message: String,
    invalid: bool,
}

impl Failure {
    fn invalid(message: impl Into<String>) -> Failure {
        Failure {
            message: message.into(),
            invalid: true,
        }
    }
}

impl From<EngineError> for Failure {
    /// An argument error keeps its kind; a refusal ("already running", "runs
    /// on <name>", a lost lease) travels as its bare sentence.
    fn from(error: EngineError) -> Failure {
        Failure {
            invalid: matches!(error, EngineError::Invalid(_)),
            message: error.to_string(),
        }
    }
}

fn text<'a>(request: &'a Value, key: &str) -> Result<&'a str, Failure> {
    match request.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s),
        _ => Err(Failure::invalid(format!("{key} is required"))),
    }
}

fn loop_of(request: &Value) -> Result<Loop, Failure> {
    text(request, "loop")?.parse().map_err(Failure::invalid)
}

/// Run automation verb `cmd` as `scope` and wrap the answer in an envelope.
/// `request` already names the local spelling of `domain`; `domains` of a
/// status request are localized here, since the two doors share this.
pub(crate) async fn answer(
    engine: &Arc<Engine>,
    cmd: &str,
    request: &Value,
    scope: &Scope,
) -> Value {
    match run(engine, cmd, request, scope).await {
        Ok(data) => envelope_ok(data),
        Err(f) if f.invalid => envelope_err_kind(f.message, crystalline_remote::INVALID_KIND),
        Err(f) => envelope_err(f.message),
    }
}

async fn run(
    engine: &Arc<Engine>,
    cmd: &str,
    request: &Value,
    scope: &Scope,
) -> Result<Value, Failure> {
    let lp = loop_of(request)?;
    if cmd == "automation_status" {
        let mut domains = Vec::new();
        match request.get("domains") {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => {
                if items.len() > MAX_STATUS_DOMAINS {
                    return Err(Failure::invalid(format!(
                        "domains names at most {MAX_STATUS_DOMAINS} domains"
                    )));
                }
                let hidden = engine.hidden_for(scope).await?;
                for item in items {
                    let Some(typed) = item.as_str() else {
                        return Err(Failure::invalid("domains must be an array of names"));
                    };
                    domains.push(engine.localize_visible(typed, &hidden).await);
                }
            }
            Some(_) => return Err(Failure::invalid("domains must be an array of names")),
        }
        // `force` skips the wait after failed runs, for a run a person
        // starts by hand.
        let force = match request.get("force") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(force)) => *force,
            Some(_) => return Err(Failure::invalid("force must be true or false")),
        };
        return Ok(engine
            .automation_status_forced(lp, &domains, scope, force)
            .await?);
    }
    let domain = text(request, "domain")?;
    let answer = match cmd {
        "automation_lease_take" => {
            let runner = text(request, "runner")?;
            engine
                .automation_lease_take(lp, domain, runner, scope)
                .await?
        }
        "automation_lease_renew" => {
            let token = text(request, "token")?;
            engine
                .automation_lease_renew(lp, domain, token, scope)
                .await?
        }
        "automation_lease_release" => {
            let token = text(request, "token")?;
            engine
                .automation_lease_release(lp, domain, token, scope)
                .await?
        }
        "automation_run_record" => {
            let token = text(request, "token")?;
            let report: RunReport =
                serde_json::from_value(request.get("report").cloned().unwrap_or(Value::Null))
                    .map_err(|e| Failure::invalid(format!("invalid report: {e}")))?;
            engine
                .automation_run_record(lp, domain, token, &report, scope)
                .await?
        }
        "automation_share" => {
            let token = text(request, "token")?;
            let summary = match request.get("summary") {
                None | Some(Value::Null) => "",
                Some(Value::String(s)) => s.as_str(),
                Some(_) => return Err(Failure::invalid("summary must be a string")),
            };
            engine
                .automation_share(lp, domain, token, summary, scope)
                .await?
        }
        "automation_prompt" => {
            let max_items = match request.get("max_items") {
                None | Some(Value::Null) => DEFAULT_MAX_ITEMS,
                Some(v) => v
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| {
                        Failure::invalid(format!(
                            "max_items must be a whole number from 0 to {}",
                            u32::MAX
                        ))
                    })?,
            };
            let skills_loaded = match request.get("skills_loaded") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(loaded)) => Some(*loaded),
                Some(_) => return Err(Failure::invalid("skills_loaded must be true or false")),
            };
            engine
                .automation_prompt(lp, domain, max_items, skills_loaded, scope)
                .await?
        }
        "automation_runs" => {
            let limit = match request.get("limit") {
                None | Some(Value::Null) => DEFAULT_RUNS,
                Some(v) => v
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| Failure::invalid("limit must be a whole number"))?,
            };
            engine.automation_runs(lp, domain, limit, scope).await?
        }
        other => return Err(Failure::invalid(format!("unknown command '{other}'"))),
    };
    Ok(answer)
}
