//! The router in front of every owner entry point: which source a call goes
//! to, the forwarding with the agent it is for, the fan-out with a deadline
//! per source, and the translation back to this machine's names.
//!
//! **Owner only.** Routing happens where the caller is this machine's owner:
//! the control socket, the in-process CLI, and (from the MCP handlers) a
//! stdio session. Two rules hold for everything else, and tests pin both:
//!
//! - An HTTP session or a REST call on this daemon is never routed. Neither
//!   reaches this module: the REST crate cannot name it, and the MCP
//!   handlers ask it only for a stdio session.
//! - A remote `tool` call a server answers is never routed again. It comes
//!   in through `remote_ctl`, and `McpServer::remote_tool` runs the verb
//!   cores on the server's own engine directly, so a server never forwards a
//!   forwarded call, whatever sources it has itself.
//!
//! The pure decisions live in `crystalline_remote::mounts`; this module
//! reads the names out of the engine's own params (`DomainArgs`), which only
//! this crate can name.
//!
//! **Limits.** A call for one domain waits at most
//! [`crystalline_remote::ONE_DOMAIN_LIMIT`] for its source
//! ([`crystalline_remote::CTL_TIMEOUT`] for `evolve_engrams`, which runs a
//! sweep on the server), and a call over all domains waits at most
//! `remote.deadline_ms` for each source; a caller's own deadline (the recall
//! hook's) shortens either. A source inside its down window is skipped at
//! once and named; nothing is ever queued for later.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use crystalline_remote::{
    CTL_TIMEOUT, ForwardedAgent, MountTable, NameMap, ONE_DOMAIN_LIMIT, Part, RemoteFailure, Route,
    SourceSet, ToolShape, attach_row_urls, merge_evolve, merge_list_domains, merge_recent,
    merge_reflect, merge_search, missing_from, missing_from_evolve, part_request_limit, seconds,
    translate_answer,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::engine::{
    EVOLVE_DEFAULT_LIMIT, EVOLVE_MAX_LIMIT, Engine, EngineError, MAX_PAGE_LIMIT,
    SEARCH_DEFAULT_LIMIT,
};

/// The tools that change content: refused on a read-only instance for a
/// mounted domain too, as they are for a local one (the two Reflection
/// writers are hidden there).
const WRITE_TOOLS: &[&str] = &[
    "write_engram",
    "edit_engram",
    "split_engram",
    "delete_engram",
    "move_engram",
    "write_reflection",
    "discuss",
];
use crate::params::*;

/// The tools a call can be forwarded with: the server's `REMOTE_TOOLS`, the
/// fifteen engine verbs and the four Reflection tools.
pub const FORWARDED_TOOLS: &[&str] = crate::remote_ctl::REMOTE_TOOLS;

/// How a tool reaches domains, `None` for one the router leaves alone.
pub fn shape_of(tool: &str) -> Option<ToolShape> {
    Some(match tool {
        "write_engram" | "read_engram" | "edit_engram" | "split_engram" | "delete_engram"
        | "browse_domain" | "validate_engrams" | "infer_schema" | "vocabulary" => {
            ToolShape::OneDomain
        }
        "search_engrams" | "recent_activity" | "list_domains" => ToolShape::AllDomains,
        t if t == crate::EVOLVE_TOOL_NAME => ToolShape::AllDomains,
        "read_reflection" | "write_reflection" | "discuss" => ToolShape::OneDomain,
        "reflect" => ToolShape::AllDomains,
        "build_context" => ToolShape::Anchored,
        "move_engram" => ToolShape::Move,
        "share_changes" | "update_domain" | "origin_status" | "resolve_conflict"
        | "withdraw_proposal" | "discard_changes" => ToolShape::Collab,
        "configure" | "provision" | "skills" | "add_domain" | "remove_domain" => {
            ToolShape::LocalOnly
        }
        _ => return None,
    })
}

fn names_of<P: DomainArgs + DeserializeOwned>(args: &Value) -> anyhow::Result<Vec<String>> {
    let mut params: P = serde_json::from_value(args.clone())
        .map_err(|e| anyhow::anyhow!("invalid arguments: {e}"))?;
    let seen = RefCell::new(Vec::new());
    params.localize_domains(&|name: &str| {
        seen.borrow_mut().push(name.to_string());
        name.to_string()
    });
    let mut names = seen.into_inner();
    names.dedup();
    Ok(names)
}

/// Every domain a call names, in the order its params carry them, read
/// through the params' own `DomainArgs` so a new field is never missed.
pub fn named_domains(tool: &str, args: &Value) -> anyhow::Result<Vec<String>> {
    match tool {
        "write_engram" => names_of::<WriteParams>(args),
        "read_engram" => names_of::<ReadParams>(args),
        "edit_engram" => names_of::<EditParams>(args),
        "split_engram" => names_of::<SplitParams>(args),
        "delete_engram" => names_of::<DeleteParams>(args),
        "move_engram" => names_of::<MoveParams>(args),
        "browse_domain" => names_of::<BrowseParams>(args),
        "validate_engrams" => names_of::<ValidateParams>(args),
        "infer_schema" => names_of::<InferParams>(args),
        "vocabulary" => names_of::<VocabularyParams>(args),
        "search_engrams" => names_of::<SearchParams>(args),
        "recent_activity" => names_of::<RecentParams>(args),
        "build_context" => names_of::<ContextParams>(args),
        t if t == crate::EVOLVE_TOOL_NAME => names_of::<EvolveParams>(args),
        "share_changes" => names_of::<ShareChangesParams>(args),
        "update_domain" => names_of::<UpdateDomainParams>(args),
        "origin_status" => names_of::<OriginStatusParams>(args),
        "resolve_conflict" => names_of::<ResolveConflictParams>(args),
        "withdraw_proposal" => names_of::<WithdrawProposalParams>(args),
        "discard_changes" => names_of::<DiscardChangesParams>(args),
        "remove_domain" => names_of::<RemoveDomainParams>(args),
        "provision" => names_of::<ProvisionParams>(args),
        "configure" => names_of::<ConfigureParams>(args),
        "reflect" => names_of::<ReflectParams>(args),
        "read_reflection" => names_of::<ReadReflectionParams>(args),
        "write_reflection" => names_of::<WriteReflectionParams>(args),
        "discuss" => names_of::<DiscussParams>(args),
        _ => Ok(Vec::new()),
    }
}

/// Where a call goes.
pub fn plan(tool: &str, args: &Value, table: &MountTable) -> anyhow::Result<Route> {
    let Some(shape) = shape_of(tool) else {
        return Ok(Route::Local);
    };
    let named = named_domains(tool, args)?;
    Ok(table.route(tool, shape, &named))
}

/// The args of a call to one source, in that source's names (outbound
/// translation). The keys are exactly the `domain_args!` fields of
/// `crates/engine/src/params.rs`: `domain`, `destination_domain`, `domains`,
/// `identifier`, `anchor`, `successor`.
///
/// A `domains` filter keeps only this source's domains. When it keeps none
/// of them, or with `whole_source` and no filter, it names every domain this
/// source mounts here: a domain the source offers that is skipped here, or
/// one of this machine's own names, never reaches the server.
pub fn forwarded_args(args: &Value, names: &NameMap, whole_source: bool) -> Value {
    let mut args = args.clone();
    let Some(obj) = args.as_object_mut() else {
        return args;
    };
    for key in ["domain", "destination_domain"] {
        if let Some(Value::String(s)) = obj.get_mut(key)
            && let Some(remote) = names.to_remote.get(s.as_str())
        {
            *s = remote.clone();
        }
    }
    for key in ["identifier", "anchor", "successor"] {
        if let Some(Value::String(s)) = obj.get_mut(key) {
            *s = crystalline_remote::translate_address_to(s, &names.to_remote);
        }
    }
    let asked: Vec<String> = obj
        .get("domains")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !asked.is_empty() || whole_source {
        let kept: Vec<Value> = asked
            .iter()
            .filter_map(|d| names.to_remote.get(d))
            .map(|r| json!(r))
            .collect();
        let domains = if kept.is_empty() {
            names.to_remote.values().map(|r| json!(r)).collect()
        } else {
            kept
        };
        obj.insert("domains".into(), Value::Array(domains));
    }
    args
}

/// Why a routed call did not answer.
#[derive(Debug)]
pub enum RouteError {
    /// Refused here, before any source was asked (a move between sources, a
    /// collaboration or machine-local tool on a mounted domain).
    Refused(String),
    /// The source that holds the domain did not answer with a result: it
    /// refused in its own words, could not be reached, or the sign-in to it
    /// is gone.
    Source {
        /// The source.
        source: String,
        /// The domain the call named.
        domain: String,
        /// What happened.
        failure: RemoteFailure,
    },
    /// The local part failed, or this machine refused the call itself in
    /// the words and shape a local call gets (a write on a read-only
    /// instance).
    Local(anyhow::Error),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::Refused(text) => f.write_str(text),
            // The server's refusal or argument error, word for word.
            RouteError::Source {
                failure: RemoteFailure::Refused(text) | RemoteFailure::Invalid(text),
                ..
            } => f.write_str(text),
            RouteError::Source {
                source,
                domain,
                failure: RemoteFailure::Unreachable { url, detail, .. },
            } => write!(f, "{}", unreachable(source, domain, url, detail)),
            RouteError::Source {
                source,
                domain,
                failure: RemoteFailure::TimedOut { url, after, .. },
            } => write!(
                f,
                "{}",
                unreachable(
                    source,
                    domain,
                    url,
                    &format!("it did not answer within {}", seconds(*after))
                )
            ),
            // A write that went out and was never answered: what may have
            // happened and how to check, in the source's own sentence.
            RouteError::Source {
                failure: failure @ RemoteFailure::Unanswered(_),
                ..
            } => write!(f, "{failure}"),
            // The source was never asked: this process was still starting,
            // so nothing about the source failed.
            RouteError::Source {
                source,
                domain,
                failure: RemoteFailure::Starting { .. },
            } => write!(
                f,
                "{source} was not asked yet: this process was still setting up its network \
                 connection. '{domain}' comes from {source}; ask again and the next call reaches it"
            ),
            RouteError::Source {
                source,
                domain,
                failure,
            } => write!(
                f,
                "{failure}. '{domain}' comes from {source}, and it is not available until {source} answers again"
            ),
            RouteError::Local(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for RouteError {}

/// What a call for one domain answers when its source cannot be reached:
/// the source, its address and the likely cause, and that it recovers by
/// itself. Every part is this machine's own words.
fn unreachable(source: &str, domain: &str, url: &str, cause: &str) -> String {
    format!(
        "{source} cannot be reached right now (check the VPN or the network); its domains are \
         unavailable. '{domain}' comes from {source} at {url} ({cause}); it recovers by itself \
         once the server answers again"
    )
}

/// Install this machine's sources on `engine`, from `<state_dir>/remote` and
/// the environment, even when there are none yet: a later `connect` then
/// reaches a running daemon through `sources_reload` or its poller. Nothing
/// happens when the state directory cannot be resolved. A daemon calls this
/// only after it finished a rename an earlier run left half done, because
/// that recovery reads names the mount table would otherwise hide.
pub fn install_sources(engine: &Engine) {
    let Ok(dir) = crystalline_remote::remote_dir() else {
        return;
    };
    let set = SourceSet::load(
        dir,
        engine.local_domains(),
        crystalline_core::secret_env::process_var,
    );
    engine.set_sources(Arc::new(set));
}

/// What `status` says about every source: its record, its last failure, its
/// names, and the local domains it hides.
pub fn sources_status(engine: &Arc<Engine>) -> Value {
    let Some(sources) = engine.sources() else {
        return json!([]);
    };
    let table = sources.table();
    let failures = sources.failures();
    let details = sources.failure_details();
    Value::Array(
        sources
            .records()
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "url": s.url,
                    "account": s.account,
                    "kind": s.kind.as_str(),
                    "from_env": s.from_env,
                    "failure": failures.get(&s.name),
                    // What the server itself wrote, for a person reading
                    // `status` or `doctor`; never part of an agent's answer.
                    "failure_detail": details.get(&s.name),
                    "mounts": table
                        .of_source(&s.name)
                        .map(|m| json!({
                            "local": m.local,
                            "remote": m.remote,
                            "replaces_local": m.replaces_local,
                        }))
                        .collect::<Vec<_>>(),
                    "skipped": table
                        .skipped
                        .iter()
                        .filter(|k| {
                            k.source == s.name
                                && k.reason != crystalline_remote::SkipReason::NotChosen
                        })
                        .map(|k| json!({
                            "remote": k.remote,
                            "kept_by": k.kept_by,
                            "reason": match k.reason {
                                crystalline_remote::SkipReason::SameDomain => "same_domain",
                                crystalline_remote::SkipReason::InvalidName => "invalid_name",
                                crystalline_remote::SkipReason::NotChosen => "not_chosen",
                            },
                        }))
                        .collect::<Vec<_>>(),
                    "shadowed": table
                        .shadowed
                        .iter()
                        .filter(|h| h.source == s.name)
                        .map(|h| json!({
                            "local": h.local,
                            "reason": match h.reason {
                                crystalline_remote::HiddenReason::Copy => "copy",
                                crystalline_remote::HiddenReason::Collision => "collision",
                            },
                            "by": h.by,
                        }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// Route one call. `None` when it is a local call (or this machine has no
/// sources), and the caller runs its own local path exactly as before.
pub async fn routed(
    engine: &Arc<Engine>,
    tool: &str,
    args: &Value,
    agent: &ForwardedAgent,
    deadline: Option<Duration>,
) -> Option<Result<Value, RouteError>> {
    let sources = engine.sources()?;
    if sources.is_empty() {
        return None;
    }
    // A hand edit of config.yaml (a domain added, renamed or removed) shows
    // in the table before this call is planned.
    sync_local(engine, &sources).await;
    let table = sources.routing_table();
    let route = plan(tool, args, &table).ok()?;
    match route {
        Route::Local => None,
        Route::Refused(text) => Some(Err(RouteError::Refused(text))),
        Route::Remote { source } => {
            let names = table.names(&source);
            let named = named_domains(tool, args).unwrap_or_default();
            let domain = named
                .iter()
                .find(|d| names.to_remote.contains_key(*d))
                .cloned()
                .unwrap_or_default();
            // Every name the call carries must be this source's: a second
            // domain is never sent to a server under this machine's name.
            if let Some(other) = named.iter().find(|d| !names.to_remote.contains_key(*d)) {
                return Some(Err(RouteError::Refused(format!(
                    "this call names '{domain}', a domain from {source}, and also '{other}', \
                     which is not; a call for a domain from {source} names only that \
                     server's domains, so ask about each domain in a call of its own"
                ))));
            }
            // This machine's own refusal, in the shape a local write gets.
            if engine.read_only() && WRITE_TOOLS.contains(&tool) {
                return Some(Err(RouteError::Local(EngineError::ReadOnly.into())));
            }
            let forwarded = forwarded_args(args, &names, false);
            let transfer = file_transfer(tool, args);
            // A forwarded sweep and a file transfer get the long ctl limit:
            // a sweep runs long on the server, and a full-size file over a
            // slow uplink outlasts ten seconds (the proxy allows two minutes).
            let own = if tool == crate::EVOLVE_TOOL_NAME || transfer.is_some() {
                CTL_TIMEOUT
            } else {
                ONE_DOMAIN_LIMIT
            };
            let limit = deadline.map_or(own, |d| d.min(own));
            // Running out of time on one file says nothing about the source,
            // so a file transfer never opens the down window (as
            // `Connection::file_within`); a refused connection still counts.
            let budget = if transfer.is_some() {
                crystalline_remote::Budget::Cut
            } else {
                crystalline_remote::Budget::of(limit, own)
            };
            let started = std::time::Instant::now();
            if let Some(transfer) = transfer {
                // An older server ignores the fields that make this a file;
                // with a title beside them it would write an engram instead.
                // The status question has a short cut of its own, taken from
                // the transfer's limit, and its timeout refuses the transfer
                // without marking the source down.
                let asked = limit.min(ONE_DOMAIN_LIMIT);
                match sources
                    .takes_agent_files(&source, asked, crystalline_remote::Budget::Cut)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        let url = sources
                            .source_url(&source)
                            .filter(|url| !url.is_empty())
                            .unwrap_or_else(|| source.clone());
                        return Some(Err(RouteError::Refused(older_server_sentence(
                            &url, transfer,
                        ))));
                    }
                    Err(failure) => {
                        return Some(Err(RouteError::Source {
                            source,
                            domain,
                            failure: within(failure, asked),
                        }));
                    }
                }
            }
            // A guard the caller asked for is never dropped without a word:
            // a server older than the release that takes it would ignore
            // `expected_checksum` and write last-write-wins. One that names
            // no version is refused the same way rather than trusted.
            //
            // The status question's time is not taken from the write: once it
            // is answered the write gets its own full limit, as if it had been
            // sent at once. A caller's own deadline still bounds both.
            let mut gate_spent = std::time::Duration::ZERO;
            if guards_a_write(tool, args) {
                let asked_at = std::time::Instant::now();
                let asked = limit
                    .saturating_sub(started.elapsed())
                    .min(ONE_DOMAIN_LIMIT);
                match sources
                    .server_version(&source, asked, crystalline_remote::Budget::Cut)
                    .await
                {
                    Ok(version)
                        if version.as_deref().is_some_and(|v| {
                            crystalline_remote::version_at_least(
                                v,
                                crystalline_remote::WRITE_CHECKSUM_RELEASE,
                            )
                        }) =>
                    {
                        if deadline.is_none() {
                            gate_spent = asked_at.elapsed();
                        }
                    }
                    Ok(version) => {
                        let url = sources
                            .source_url(&source)
                            .filter(|url| !url.is_empty())
                            .unwrap_or_else(|| source.clone());
                        return Some(Err(RouteError::Refused(unguarded_server_sentence(
                            &url,
                            version.as_deref(),
                        ))));
                    }
                    Err(failure) => {
                        return Some(Err(RouteError::Source {
                            source,
                            domain,
                            failure: within(failure, asked),
                        }));
                    }
                }
            }
            let left = limit.saturating_sub(started.elapsed().saturating_sub(gate_spent));
            // A caller's shorter budget running out says nothing about the
            // source, so it never opens the down window (Budget::Cut), and
            // neither does a file transfer's.
            let answer = sources
                .forward_as(&source, tool, forwarded, agent, left, budget)
                .await;
            Some(match answer {
                Ok(mut value) => {
                    attach_row_urls(&mut value);
                    translate_answer(&mut value, &names);
                    Ok(value)
                }
                Err(failure) => Err(RouteError::Source {
                    source,
                    domain,
                    failure: within(failure, limit),
                }),
            })
        }
        Route::FanOut {
            local,
            sources: asked,
        } => {
            let own = Duration::from_millis(engine.config().remote_deadline_ms());
            let deadline = deadline.map_or(own, |d| d.min(own));
            // The recall's 700 ms is a cut inside the configured deadline: a
            // source still answering when it ends is not counted as down.
            let budget = crystalline_remote::Budget::of(deadline, own);
            Some(
                fan_out(
                    engine, &sources, &table, tool, args, agent, local, asked, deadline, budget,
                )
                .await,
            )
        }
    }
}

/// Which way a file moves in a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transfer {
    /// An agent sends a file: `write_engram` with `path`, `write_reflection`
    /// `write_asset`.
    Upload,
    /// An agent reads a file by its `assets/` path.
    Download,
}

/// Whether this call moves a file, and which way. Only the cases an older
/// server would misread: the write verbs with the fields that make them
/// uploads, and the read verbs on an `assets/` path.
fn file_transfer(tool: &str, args: &Value) -> Option<Transfer> {
    let text = |key: &str| args.get(key).and_then(Value::as_str);
    match tool {
        "write_engram" if args.get("path").is_some_and(|p| !p.is_null()) => Some(Transfer::Upload),
        "write_reflection" if text("action") == Some("write_asset") => Some(Transfer::Upload),
        "read_engram" => text("identifier")
            .and_then(crate::engine::attachment_identifier)
            .map(|_| Transfer::Download),
        "read_reflection" if args.get("query").is_none_or(Value::is_null) => text("path")
            .and_then(crate::engine::attachment_identifier)
            .map(|_| Transfer::Download),
        _ => None,
    }
}

/// What an agent reads when the server behind its call predates agent files.
fn older_server_sentence(url: &str, transfer: Transfer) -> String {
    let what = match transfer {
        Transfer::Upload => "take files from agents",
        Transfer::Download => "hand files to agents",
    };
    format!(
        "The server at {url} runs an older Crystalline that cannot {what} yet; update it to {} or later.",
        crystalline_remote::AGENT_FILES_RELEASE
    )
}

/// Whether this call is a `write_engram` that carries `expected_checksum`.
fn guards_a_write(tool: &str, args: &Value) -> bool {
    tool == "write_engram"
        && args
            .get("expected_checksum")
            .is_some_and(|value| !value.is_null())
}

/// What an agent reads when the server behind a guarded write cannot keep the
/// guard: it is older than [`crystalline_remote::WRITE_CHECKSUM_RELEASE`], or
/// it does not say which version it runs.
fn unguarded_server_sentence(url: &str, version: Option<&str>) -> String {
    let runs = match version {
        Some(version) => format!("runs Crystalline {version}"),
        None => "runs a Crystalline that does not name its version".to_string(),
    };
    let needed = crystalline_remote::WRITE_CHECKSUM_RELEASE
        .strip_suffix(".0")
        .unwrap_or(crystalline_remote::WRITE_CHECKSUM_RELEASE);
    format!(
        "The server at {url} {runs}, which cannot guard a write with expected_checksum; \
         it needs {needed} or newer. Leave expected_checksum out to write without the guard."
    )
}

/// One file of a mounted domain, as its server answered it.
#[derive(Debug)]
pub struct RoutedFile {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// Its MIME type, as the server resolved it.
    pub mime: String,
}

/// Read one file of a mounted domain from the source that holds it: an
/// engram attachment (`assets/...`) or a Reflection's file
/// (`reflections/assets/...`), `path` domain-relative. `None` when no source
/// mounts `domain`, and the caller reads its own file as before.
///
/// The server reads the file under the account's own right for its kind
/// (engram rights for an attachment, the Reflection read right for a
/// Reflection's file) and answers its own refusal, which comes back word for
/// word. That includes a Reflection whose home is another instance on the
/// server's shared database (`hosted_elsewhere`): the refusal names that
/// instance, and this machine does not follow it there. A server older than
/// 0.27.0 has no `file` command; that refusal is said in this machine's
/// words and names the version.
///
/// Owner only, as [`routed`]: a stdio session asks, an HTTP session never
/// does.
pub async fn routed_file(
    engine: &Arc<Engine>,
    domain: &str,
    path: &str,
) -> Option<Result<RoutedFile, RouteError>> {
    let sources = engine.sources()?;
    if sources.is_empty() {
        return None;
    }
    sync_local(engine, &sources).await;
    let table = sources.routing_table();
    let source = table.source_of(domain)?.to_string();
    let names = table.names(&source);
    let remote = names.to_remote.get(domain)?.clone();
    // The mime is this machine's, from the file name, never the server's: it
    // decides whether the bytes are served as text.
    let Some(mime) = file_mime(path) else {
        return Some(Err(RouteError::Refused(format!(
            "'{path}' is not a file a domain keeps: a file lives under assets/ or \
             reflections/assets/ and has a known file type"
        ))));
    };
    // A file has the long limit: a large one over a slow link takes time,
    // and the read runs as a cut that never marks the source down.
    let answer = sources
        .fetch_file(&source, &remote, path, CTL_TIMEOUT)
        .await;
    let failed = |failure: RemoteFailure| RouteError::Source {
        source: source.clone(),
        domain: domain.to_string(),
        failure,
    };
    Some(match answer {
        Ok(value) => {
            decoded_file(&value, mime).map_err(|text| failed(RemoteFailure::Refused(text)))
        }
        Err(RemoteFailure::Refused(text)) if is_no_file_command(&text) => {
            let url = sources.source_url(&source).unwrap_or_default();
            Err(RouteError::Refused(format!(
                "{source} at {url} cannot hand out files yet: reading a file of '{domain}' needs \
                 Crystalline 0.27 or newer on that server"
            )))
        }
        Err(failure) => Err(failed(within(failure, CTL_TIMEOUT))),
    })
}

/// The MIME type of a file of a mounted domain, decided on this machine:
/// `path` (domain-relative) must name a file of either kind
/// (`assets/...` or `reflections/assets/...`) that a domain may hold, and
/// the type comes from its name. `None` for anything else. A server's own
/// word on either is never taken.
pub fn file_mime(path: &str) -> Option<&'static str> {
    let url = crystalline_core::CrystallineUrl {
        domain: String::new(),
        permalink: path.to_string(),
        glob: false,
    };
    let (_, inner) = url.asset_ref()?;
    crystalline_core::validate_asset_path(inner).ok()?;
    crystalline_core::attachment_mime(inner.rsplit('/').next().unwrap_or(inner))
}

/// Whether a server's refusal is the one a server without the `file`
/// command answers (0.26 and older): the allow-list's own sentence.
fn is_no_file_command(text: &str) -> bool {
    text == format!(
        "'{}' {}",
        crystalline_remote::FILE_COMMAND,
        crate::remote_ctl::NOT_REMOTE
    )
}

/// A `file` answer's bytes, checked against what it says about them: valid
/// base64, the size it names, and never past the largest file a domain
/// keeps. `mime` is this machine's ([`file_mime`]); the server's is ignored.
fn decoded_file(value: &Value, mime: &'static str) -> Result<RoutedFile, String> {
    use base64::Engine as _;
    let broken = || "the server answered a file this machine cannot read".to_string();
    let size = value["size"].as_u64().ok_or_else(broken)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value["base64"].as_str().ok_or_else(broken)?)
        .map_err(|_| broken())?;
    if bytes.len() as u64 != size || size > crystalline_core::MAX_ATTACHMENT_BYTES {
        return Err(broken());
    }
    Ok(RoutedFile {
        bytes,
        mime: mime.to_string(),
    })
}

/// Read this machine's domains again when the configuration file changed
/// since the last look (a stat, no parse when it did not), on a blocking
/// thread, and hand them to the sources.
pub async fn sync_local(engine: &Arc<Engine>, sources: &Arc<SourceSet>) {
    let path = engine.config_file_path();
    let stamp = tokio::task::spawn_blocking(move || {
        path.and_then(|p| std::fs::metadata(p).ok())
            .map(|m| (m.modified().ok(), m.len()))
    })
    .await
    .unwrap_or(None);
    if !sources.note_config_stamp(stamp) {
        return;
    }
    let reading = engine.clone();
    if let Ok(local) = tokio::task::spawn_blocking(move || reading.local_domains()).await {
        sources.set_local_if_changed(local);
    }
}

/// A timed-out failure names the limit the caller set, not what was left of
/// it once the connection was open.
fn within(failure: RemoteFailure, limit: Duration) -> RemoteFailure {
    match failure {
        RemoteFailure::TimedOut { source, url, .. } => RemoteFailure::TimedOut {
            source,
            url,
            after: limit,
        },
        RemoteFailure::Unanswered(mut write) => {
            write.after = write.after.map(|_| limit);
            RemoteFailure::Unanswered(write)
        }
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
async fn fan_out(
    engine: &Arc<Engine>,
    set: &Arc<SourceSet>,
    table: &MountTable,
    tool: &str,
    args: &Value,
    agent: &ForwardedAgent,
    local: bool,
    asked: Vec<String>,
    deadline: Duration,
    budget: crystalline_remote::Budget,
) -> Result<Value, RouteError> {
    let page = args.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let limit_default = if tool == "search_engrams" {
        SEARCH_DEFAULT_LIMIT
    } else {
        EVOLVE_DEFAULT_LIMIT
    };
    let limit_max = if tool == "search_engrams" {
        MAX_PAGE_LIMIT
    } else {
        EVOLVE_MAX_LIMIT
    };
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(limit_default, |l| l as usize)
        .clamp(1, limit_max);
    let paged = tool == "search_engrams" || tool == crate::EVOLVE_TOOL_NAME || tool == "reflect";
    let part_args = |mut a: Value| {
        if paged && let Some(obj) = a.as_object_mut() {
            obj.insert("page".into(), json!(1));
            obj.insert("limit".into(), json!(part_request_limit(page, limit)));
        }
        a
    };
    let named_filter = args
        .get("domains")
        .and_then(Value::as_array)
        .is_some_and(|d| !d.is_empty());

    let mut remote = tokio::task::JoinSet::new();
    let mut spawned = std::collections::BTreeMap::new();
    for source in asked {
        let names = table.names(&source);
        // A part names its source's domains: the ones the filter asked for,
        // or every one this source mounts here, so the source's counts and
        // pages cover exactly the rows shown here. `list_domains` has no
        // filter to name them in; its rows are narrowed below instead.
        let mut forwarded = part_args(forwarded_args(args, &names, tool != "list_domains"));
        // An unnamed `reflect` names the mounted domains only to scope the
        // part: one whose Reflection is off there is left out, as an unnamed
        // queue leaves it out, never refused.
        if tool == "reflect"
            && !named_filter
            && let Some(obj) = forwarded.as_object_mut()
        {
            obj.insert("skip_unavailable".into(), json!(true));
        }
        let set = set.clone();
        let tool = tool.to_string();
        let agent = agent.clone();
        let asked_name = source.clone();
        let task = remote.spawn(async move {
            let answer = set
                .forward_as(&source, &tool, forwarded, &agent, deadline, budget)
                .await;
            (source, names, answer)
        });
        spawned.insert(task.id(), asked_name);
    }
    // The local part runs while the sources are asked.
    let local_part = if local {
        let mut local_args = part_args(args.clone());
        if named_filter && let Some(obj) = local_args.as_object_mut() {
            let kept: Vec<Value> = args["domains"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| d.as_str().is_some_and(|d| table.mount(d).is_none()))
                .cloned()
                .collect();
            obj.insert("domains".into(), Value::Array(kept));
        }
        let local_args = localized_local(engine, tool, local_args).await?;
        let mut value = run_local_part(engine, tool, local_args)
            .await
            .map_err(RouteError::Local)?;
        crate::web_url::attach_template(&mut value, &engine.local_web_base());
        Some(Part {
            source: None,
            answer: value,
        })
    } else {
        None
    };

    let mut parts: Vec<Part> = local_part.into_iter().collect();
    let mut missing = Vec::new();
    let mut answered: Vec<(String, Part)> = Vec::new();
    while let Some(done) = remote.join_next_with_id().await {
        let (source, names, answer) = match done {
            Ok((_, part)) => part,
            // The task asking a source stopped (a panic on this machine):
            // that source is named, never silently left out.
            Err(stopped) => {
                if let Some(source) = spawned.get(&stopped.id()) {
                    missing.push(crystalline_remote::Missing {
                        source: source.clone(),
                        reason: "was not asked to the end (the request stopped on this machine; the next call asks it again)".to_string(),
                        recovers: false,
                    });
                }
                continue;
            }
        };
        match answer {
            Ok(mut value) => {
                attach_row_urls(&mut value);
                if tool == "list_domains"
                    && let Some(rows) = value.get_mut("domains").and_then(Value::as_array_mut)
                {
                    // Only the domains this machine mounts from this source:
                    // a duplicate another source keeps, or one nobody here
                    // mounts, is not listed under its server name.
                    rows.retain(|row| {
                        row.get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|name| names.to_local.contains_key(name))
                    });
                    for row in rows.iter_mut() {
                        row["remote_name"] = row["name"].clone();
                        // A server's path means nothing on this machine.
                        row["path"] = Value::Null;
                    }
                }
                translate_answer(&mut value, &names);
                answered.push((
                    source.clone(),
                    Part {
                        source: Some(source),
                        answer: value,
                    },
                ));
            }
            Err(failure) => {
                let failure = within(failure, deadline);
                missing.push(if tool == crate::EVOLVE_TOOL_NAME {
                    let example = names.to_remote.keys().next().map(String::as_str);
                    missing_from_evolve(&source, &failure, example)
                } else {
                    missing_from(&source, &failure)
                });
            }
        }
    }
    // Part order is connect order, whatever order the answers arrived in.
    let order = set.records();
    let position = |name: &str| order.iter().position(|r| r.name == name);
    answered.sort_by_key(|(name, _)| position(name));
    missing.sort_by_key(|m| position(&m.source));
    parts.extend(answered.into_iter().map(|(_, part)| part));
    Ok(match tool {
        "search_engrams" => merge_search(parts, page, limit, &missing),
        "recent_activity" => merge_recent(parts, &missing),
        "list_domains" => merge_list_domains(parts, &missing),
        "reflect" => merge_reflect(parts, page, limit, &missing),
        _ => merge_evolve(parts, page, limit, &missing),
    })
}

/// The local part of a fan-out: the control socket's dispatcher for the
/// fifteen engine verbs, and the engine's own `reflect` for the machine
/// owner (only owner sessions route, decision D8; `dispatch_engine` serves
/// no Reflection tool, and no CLI verb or hook calls one).
async fn run_local_part(engine: &Arc<Engine>, tool: &str, args: Value) -> anyhow::Result<Value> {
    if tool == "reflect" {
        let p: ReflectParams =
            serde_json::from_value(args).map_err(|e| anyhow::anyhow!("invalid arguments: {e}"))?;
        return Ok(engine
            .reflect(&p, &crate::ReflectionCaller::owner())
            .await?);
    }
    crate::client::dispatch_engine(engine, tool, args).await
}

/// The local half of a sweep with every domain it names spelled as the
/// local name, the way the MCP verb cores spell theirs
/// (`Engine::localized_for`): an old name or an alias resolves exactly as on
/// a machine without sources, and a domain being renamed is waited for.
async fn localized_local(engine: &Engine, tool: &str, args: Value) -> Result<Value, RouteError> {
    async fn respelled<P>(engine: &Engine, args: Value) -> Result<Value, RouteError>
    where
        P: DomainArgs + Clone + DeserializeOwned + serde::Serialize,
    {
        let p: P = serde_json::from_value(args)
            .map_err(|e| RouteError::Local(anyhow::anyhow!("invalid arguments: {e}")))?;
        let p = engine
            .localized_for(&p, &crate::scope::Scope::Unrestricted)
            .await
            .map_err(|e| RouteError::Local(e.into()))?;
        serde_json::to_value(p).map_err(|e| RouteError::Local(e.into()))
    }
    match tool {
        "search_engrams" => respelled::<SearchParams>(engine, args).await,
        "recent_activity" => respelled::<RecentParams>(engine, args).await,
        t if t == crate::EVOLVE_TOOL_NAME => respelled::<EvolveParams>(engine, args).await,
        "reflect" => respelled::<ReflectParams>(engine, args).await,
        _ => Ok(args),
    }
}

/// The ctl `tool` command's whole answer, and the in-process CLI's: routed
/// when the call names a mounted domain or all domains, local otherwise. A
/// routed read answers as a local one does: the server's `files` list,
/// which only an MCP session turns into links, is left out.
pub async fn run_tool_routed(
    engine: &Arc<Engine>,
    tool: &str,
    args: Value,
    agent: &ForwardedAgent,
    deadline: Option<Duration>,
) -> anyhow::Result<Value> {
    match routed(engine, tool, &args, agent, deadline).await {
        Some(Ok(mut value)) => {
            if matches!(tool, "read_engram" | "read_reflection")
                && let Some(obj) = value.as_object_mut()
            {
                obj.remove("files");
            }
            Ok(value)
        }
        Some(Err(e)) => Err(anyhow::anyhow!(e.to_string())),
        None => crate::client::dispatch_engine(engine, tool, args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 0.26 server's refusal of `file` is told apart from every other
    /// refusal, and a file answer is checked against what it says.
    #[test]
    fn a_server_without_the_file_command_and_a_broken_file_answer_are_recognized() {
        assert!(is_no_file_command(
            "'file' is not available over a remote connection"
        ));
        assert!(!is_no_file_command(
            "domain 'open' not registered; registered: [notes]"
        ));
        let file = decoded_file(
            &json!({ "mime": "text/html", "size": 2, "base64": "aGk=" }),
            "image/png",
        )
        .unwrap();
        assert_eq!(file.bytes, b"hi");
        assert_eq!(file.mime, "image/png", "the server's mime is ignored");
        for broken in [
            json!({ "size": 3, "base64": "aGk=" }),
            json!({ "size": 2, "base64": "not base64!" }),
            json!({ "base64": "aGk=" }),
        ] {
            assert!(decoded_file(&broken, "image/png").is_err(), "{broken}");
        }
    }

    /// A file's mime is decided from its own name, for a path a domain may
    /// hold, and nothing else gets one.
    #[test]
    fn a_mounted_files_mime_comes_from_its_name() {
        assert_eq!(file_mime("assets/shot.png"), Some("image/png"));
        assert_eq!(
            file_mime("reflections/assets/deck.pdf"),
            Some("application/pdf")
        );
        for refused in [
            "MANIFEST.md",
            "assets/../x.png",
            "reflections/assets/../../assets/shot.png",
            "../notes/assets/x.png",
            "/etc/passwd",
            "Assets/shot.png",
            "assets/run.exe",
        ] {
            assert_eq!(file_mime(refused), None, "{refused}");
        }
    }

    /// A one-domain call whose source was not asked yet (this process was
    /// still starting) says so, and never reads as if the source had failed.
    #[test]
    fn a_source_not_asked_yet_is_not_called_unavailable() {
        let text = RouteError::Source {
            source: "acme".into(),
            domain: "open".into(),
            failure: RemoteFailure::Starting {
                source: "acme".into(),
            },
        }
        .to_string();
        assert_eq!(
            text,
            "acme was not asked yet: this process was still setting up its network connection. \
             'open' comes from acme; ask again and the next call reaches it"
        );
        assert!(!text.contains("not available until"), "{text}");
    }

    /// A write that went out and was never answered reads as the source's
    /// sentence alone, with the call's whole limit, and never as a source
    /// that is unavailable.
    #[test]
    fn a_write_never_answered_says_it_may_have_happened() {
        let check = "The file may or may not be written. Read assets/shot.png to check its \
                     sha256, or upload the same file again: the same bytes are accepted.";
        let text = RouteError::Source {
            source: "acme".into(),
            domain: "open".into(),
            failure: within(
                RemoteFailure::Unanswered(Box::new(crystalline_remote::UnansweredWrite {
                    source: "acme".into(),
                    url: "https://crystalline.acme.test".into(),
                    after: Some(Duration::from_millis(119_870)),
                    check: check.into(),
                })),
                CTL_TIMEOUT,
            ),
        }
        .to_string();
        assert_eq!(
            text,
            format!("acme (https://crystalline.acme.test) did not answer within 120 s. {check}")
        );
    }
}
