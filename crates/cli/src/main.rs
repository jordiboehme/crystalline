//! `crystalline`: the command-line entry point. Wires the crate stack
//! (core, index, service) into subcommands.
//!
//! `verify` and `prompt` are static: they call straight into
//! `crystalline-core` and touch no database, socket or network connection.
//! Every other subcommand lands in a later milestone.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crystalline_core::config;
use crystalline_core::verify::{self, VerifyOptions};

mod cmd;
mod doctor;

/// Local-first knowledge management for humans and AI agents.
#[derive(Parser, Debug)]
#[command(name = "crystalline", version, about, long_about = None)]
struct Cli {
    /// Emit machine-readable JSON instead of human-readable text. Shorthand
    /// for `--format json` on subcommands that support it.
    #[arg(long, global = true)]
    json: bool,

    /// Override the index database path. Defaults to the state-directory
    /// `index.db`. Essential for tests and for pointing at a scratch index.
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Statically verify one or more Domains against the rule catalog. No
    /// database, service or network connection is used.
    Verify {
        /// Domain root paths to verify. Defaults to the current directory.
        paths: Vec<PathBuf>,
        /// Promote every rule whose default severity is Warning to Error.
        #[arg(long)]
        strict: bool,
        /// Output format: human, json or github.
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
        /// Load this file as every scanned domain's verify config, instead
        /// of each domain's own `.crystalline.yaml`.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Generate the session-start knowledge routing prompt for a workspace.
    /// No database, service or network connection is used.
    Prompt {
        /// The workspace path to route for. Defaults to the current
        /// directory.
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Load the global config from this file instead of the default
        /// config path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Manage the domains registered in the global config.
    Domain {
        #[command(subcommand)]
        command: DomainCommand,
    },
    /// Sync one or all registered domains into the index.
    Sync {
        /// Sync only this domain instead of every registered domain.
        #[arg(long)]
        domain: Option<String>,
        /// After syncing, embed any chunks that need it for the active model.
        #[arg(long)]
        embed: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Rebuild the index. `--full` wipes it first, the corruption-recovery path.
    Reindex {
        /// Wipe the index (rebuilding the file if it will not open) then resync.
        #[arg(long)]
        full: bool,
        /// After reindexing, embed any chunks that need it for the active model.
        #[arg(long)]
        embed: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Show per-domain counts and index diagnostics.
    Status {
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Manage the local embedding model.
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// Import a markdown knowledge base with YAML frontmatter into a domain.
    /// Pure file transformation: never touches the index, the socket or the
    /// network.
    Import {
        /// The source directory to import from.
        src: PathBuf,
        /// The target domain. Must already be registered.
        #[arg(long)]
        domain: String,
        /// Override or extend the built-in legacy type mapping with a YAML
        /// file shaped `{ mappings: { old: new } }`.
        #[arg(long)]
        map: Option<PathBuf>,
        /// The permalink prefix segment to strip. Defaults to the source
        /// directory's own final path component.
        #[arg(long)]
        strip_prefix: Option<String>,
        /// Print the full change report without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Diagnose the index, registered domains and service state, optionally
    /// repairing what can be repaired automatically.
    Doctor {
        /// Restrict checks to this domain instead of every registered domain.
        #[arg(long)]
        domain: Option<String>,
        /// Repair what can be repaired automatically: orphan index rows and
        /// stale service lock or socket files.
        #[arg(long)]
        fix: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Run the single-instance daemon: watch domains, embed and serve MCP and ctl
    /// over the socket, optionally over HTTP.
    Serve {
        /// Serve the tool router over streamable HTTP at this localhost address.
        #[arg(long)]
        http: Option<String>,
        /// Run as a background daemon (quiet output).
        #[arg(long)]
        daemon: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Serve MCP over stdio for an agent: attach to (or start) the daemon, or run
    /// the full stack in-process. HTTP is served by the daemon, not this command.
    Mcp {
        /// Run the full stack in-process instead of attaching to a daemon.
        #[arg(long)]
        embedded: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Send a control command to a running daemon.
    Ctl {
        #[command(subcommand)]
        command: CtlCommand,
    },
    /// Capture a new engram into a domain (the body is read from --content or stdin).
    Write {
        /// The target domain (required; there is no default domain for writes).
        domain: String,
        /// The engram title.
        title: String,
        /// The markdown body. Read from stdin when omitted. Accepts a value that
        /// begins with `-` so observation and relation bullets work.
        #[arg(long, allow_hyphen_values = true)]
        content: Option<String>,
        /// A domain-relative subfolder.
        #[arg(long)]
        folder: Option<String>,
        /// The engram type. Defaults to engram.
        #[arg(long = "type")]
        engram_type: Option<String>,
        /// Comma-separated tags.
        #[arg(long)]
        tags: Option<String>,
        /// The status. Defaults to current.
        #[arg(long)]
        status: Option<String>,
        /// Extra frontmatter as a JSON object.
        #[arg(long)]
        metadata: Option<String>,
        /// Overwrite an existing engram with the same permalink.
        #[arg(long)]
        overwrite: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Read an engram's markdown and metadata.
    Read {
        /// A permalink, domain/permalink, title or crystalline:// URL.
        identifier: String,
        /// Restrict resolution to this domain.
        #[arg(long)]
        domain: Option<String>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Edit an engram in place.
    Edit {
        /// A permalink, domain/permalink, title or crystalline:// URL.
        identifier: String,
        /// The engram's domain.
        domain: String,
        /// One of append, prepend, find_replace, replace_section,
        /// insert_before_section, insert_after_section.
        operation: String,
        /// The content to add or the replacement. Read from stdin when omitted.
        /// Accepts a value that begins with `-`.
        #[arg(long, allow_hyphen_values = true)]
        content: Option<String>,
        /// The heading path for the *_section operations.
        #[arg(long)]
        section: Option<String>,
        /// The text to find, for find_replace.
        #[arg(long)]
        find_text: Option<String>,
        /// The exact replacement count expected, for find_replace.
        #[arg(long)]
        expected_replacements: Option<usize>,
        /// Replace deeper subsections too when replacing a section.
        #[arg(long)]
        include_subsections: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Move an engram to a new path or domain.
    Move {
        /// A permalink, domain/permalink, title or crystalline:// URL.
        identifier: String,
        /// The engram's current domain.
        domain: String,
        /// The new domain-relative path.
        destination: String,
        /// Move to a different domain.
        #[arg(long)]
        destination_domain: Option<String>,
        /// Do not rewrite inbound links on a cross-domain move.
        #[arg(long)]
        no_update_links: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Delete an engram.
    Delete {
        /// A permalink, domain/permalink, title or crystalline:// URL.
        identifier: String,
        /// The engram's domain.
        domain: String,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Search across domains.
    Search {
        /// The free-text query. Omit for a filter-only search.
        query: Option<String>,
        /// Restrict to these domains (repeatable).
        #[arg(long)]
        domain: Vec<String>,
        /// Filter by type.
        #[arg(long = "type")]
        engram_type: Option<String>,
        /// Require these tags (repeatable).
        #[arg(long)]
        tag: Vec<String>,
        /// Filter by status.
        #[arg(long)]
        status: Option<String>,
        /// Only engrams recorded on or after this ISO date.
        #[arg(long)]
        after: Option<String>,
        /// hybrid (default), text, semantic, title or permalink.
        #[arg(long)]
        search_type: Option<String>,
        /// Minimum cosine similarity for a semantic hit.
        #[arg(long)]
        min_similarity: Option<f32>,
        /// Page size.
        #[arg(long)]
        limit: Option<usize>,
        /// One-based page number.
        #[arg(long)]
        page: Option<usize>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Build a context graph around a crystalline:// anchor.
    Context {
        /// A crystalline://domain/permalink anchor; a /* suffix globs a prefix.
        anchor: String,
        /// Traversal depth, 1 to 3.
        #[arg(long)]
        depth: Option<u8>,
        /// Restrict the neighborhood to these domains (repeatable).
        #[arg(long)]
        domain: Vec<String>,
        /// Maximum related engrams beyond the anchors.
        #[arg(long)]
        max_related: Option<usize>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Show recent activity across domains.
    Recent {
        /// Restrict to these domains (repeatable).
        #[arg(long)]
        domain: Vec<String>,
        /// A recency window such as 24h, 7d or 2w. Defaults to 7d.
        #[arg(long)]
        timeframe: Option<String>,
        /// Restrict to these types (repeatable).
        #[arg(long = "type")]
        types: Vec<String>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum CtlCommand {
    /// Show daemon status: pid, uptime, sessions, per-domain stats and embeddings.
    Status,
    /// List the daemon's live sessions.
    Sessions,
    /// Ask the daemon to sync a domain (or all domains).
    Sync {
        /// Sync only this domain.
        #[arg(long)]
        domain: Option<String>,
        /// Embed new chunks after syncing.
        #[arg(long)]
        embed: bool,
    },
    /// Ask the daemon to reindex. `--full` wipes first.
    Reindex {
        /// Wipe the index before reindexing.
        #[arg(long)]
        full: bool,
        /// Embed new chunks after reindexing.
        #[arg(long)]
        embed: bool,
    },
    /// Ask the daemon to shut down cleanly.
    Shutdown,
}

#[derive(Subcommand, Debug)]
enum ModelCommand {
    /// Pre-fetch the local embedding model into the cache for offline or CI use.
    Download {
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum DomainCommand {
    /// Scaffold a MANIFEST.md at a domain root. Does not touch the config.
    Init {
        /// The domain root directory. Created if it does not exist.
        path: PathBuf,
        /// The domain name. Defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Register a domain in the global config. Refuses without a MANIFEST.md.
    Add {
        /// The domain name used everywhere it is referenced.
        name: String,
        /// The domain root directory.
        path: PathBuf,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// List registered domains, with engram counts when the index is present.
    List {
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Remove a domain from the global config. Leaves its files untouched.
    Remove {
        /// The domain name to remove.
        name: String,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Human,
    Json,
    Github,
}

fn main() -> anyhow::Result<()> {
    // No arguments at all falls through to this hidden default: print help
    // rather than doing nothing silently.
    if std::env::args_os().len() <= 1 {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    }

    let cli = Cli::parse();
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
        Some(Command::Verify {
            paths,
            strict,
            format,
            config,
        }) => run_verify(paths, strict, format, config, cli.json),
        Some(Command::Prompt { workspace, config }) => run_prompt(workspace, config, cli.json),
        Some(Command::Domain { command }) => run_domain(command, cli.db, cli.json),
        Some(Command::Sync {
            domain,
            embed,
            config,
        }) => on_runtime(sync_dispatch(domain, embed, config, cli.db, cli.json)),
        Some(Command::Reindex {
            full,
            embed,
            config,
        }) => on_runtime(reindex_dispatch(full, embed, config, cli.db, cli.json)),
        Some(Command::Status { config }) => on_runtime(status_dispatch(config, cli.db, cli.json)),
        Some(Command::Model { command }) => match command {
            ModelCommand::Download { config } => {
                on_runtime(cmd::model_download(config.as_deref(), cli.json))
            }
        },
        Some(Command::Import {
            src,
            domain,
            map,
            strip_prefix,
            dry_run,
            config,
        }) => cmd::import(
            &src,
            &domain,
            map.as_deref(),
            strip_prefix.as_deref(),
            dry_run,
            config.as_deref(),
            cli.json,
        ),
        Some(Command::Doctor {
            domain,
            fix,
            config,
        }) => on_runtime(run_doctor(domain, fix, config, cli.db, cli.json)),
        Some(Command::Serve {
            http,
            daemon,
            config,
        }) => on_runtime(crystalline_service::run_serve(daemon, http, cli.db, config)),
        Some(Command::Mcp { embedded, config }) => on_runtime(crystalline_service::run_mcp(
            embedded,
            cli.db.as_deref(),
            config.as_deref(),
        )),
        Some(Command::Ctl { command }) => on_runtime(run_ctl(command, cli.json)),
        Some(
            cmd @ (Command::Write { .. }
            | Command::Read { .. }
            | Command::Edit { .. }
            | Command::Move { .. }
            | Command::Delete { .. }
            | Command::Search { .. }
            | Command::Context { .. }
            | Command::Recent { .. }),
        ) => on_runtime(run_data(cmd, cli.db, cli.json)),
    }
}

/// Print a JSON value: compact under `--json`, pretty otherwise.
fn print_value(value: &serde_json::Value, json: bool) {
    if json {
        println!("{value}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        );
    }
}

/// Read a body from an option or stdin when the option is absent.
fn content_or_stdin(content: Option<String>) -> anyhow::Result<String> {
    match content {
        Some(c) => Ok(c),
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            Ok(buf)
        }
    }
}

fn split_commas(s: Option<String>) -> Option<Vec<String>> {
    s.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn opt_vec(v: Vec<String>) -> Option<Vec<String>> {
    if v.is_empty() { None } else { Some(v) }
}

/// `status`: report from the daemon over ctl when one is running, else open the
/// index directly.
async fn status_dispatch(
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    use serde_json::json;
    if let Some(data) =
        crystalline_service::ctl_if_running(json!({ "v": 1, "cmd": "status" })).await?
    {
        print_value(&data, json);
        return Ok(());
    }
    cmd::status(config.as_deref(), db.as_deref(), json).await
}

/// `sync`: route to the daemon when one owns the index, else sync directly.
async fn sync_dispatch(
    domain: Option<String>,
    embed: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    use serde_json::json;
    if let Some(data) = crystalline_service::ctl_if_running(
        json!({ "v": 1, "cmd": "sync", "domain": domain, "embed": embed }),
    )
    .await?
    {
        print_value(&data, json);
        return Ok(());
    }
    cmd::sync(
        domain.as_deref(),
        embed,
        config.as_deref(),
        db.as_deref(),
        json,
    )
    .await
}

/// `reindex`: route to the daemon when one owns the index, else reindex directly.
async fn reindex_dispatch(
    full: bool,
    embed: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    use serde_json::json;
    if let Some(data) = crystalline_service::ctl_if_running(
        json!({ "v": 1, "cmd": "reindex", "full": full, "embed": embed }),
    )
    .await?
    {
        print_value(&data, json);
        return Ok(());
    }
    cmd::reindex(full, embed, config.as_deref(), db.as_deref(), json).await
}

/// `doctor`: diagnose the index, domains and service state. Exits 1 when
/// unresolved problems remain, 0 otherwise (including with `--fix` once every
/// fixable problem is fixed).
async fn run_doctor(
    domain: Option<String>,
    fix: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    let report = doctor::run(domain.as_deref(), fix, config.as_deref(), db.as_deref()).await?;
    if json {
        print_value(&serde_json::to_value(&report)?, true);
    } else {
        print!("{}", doctor::render_human(&report));
    }
    std::process::exit(if report.remaining_problems() > 0 {
        1
    } else {
        0
    });
}

async fn run_ctl(command: CtlCommand, json: bool) -> anyhow::Result<()> {
    use serde_json::json;
    let request = match &command {
        CtlCommand::Status => json!({ "v": 1, "cmd": "status" }),
        CtlCommand::Sessions => json!({ "v": 1, "cmd": "sessions" }),
        CtlCommand::Sync { domain, embed } => {
            json!({ "v": 1, "cmd": "sync", "domain": domain, "embed": embed })
        }
        CtlCommand::Reindex { full, embed } => {
            json!({ "v": 1, "cmd": "reindex", "full": full, "embed": embed })
        }
        CtlCommand::Shutdown => json!({ "v": 1, "cmd": "shutdown" }),
    };
    let data = crystalline_service::ctl_required(request).await?;
    print_value(&data, json);
    Ok(())
}

async fn run_data(command: Command, db: Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    use serde_json::json;
    let (tool, args, config): (&str, serde_json::Value, Option<PathBuf>) = match command {
        Command::Write {
            domain,
            title,
            content,
            folder,
            engram_type,
            tags,
            status,
            metadata,
            overwrite,
            config,
        } => {
            let body = content_or_stdin(content)?;
            let metadata = match metadata {
                Some(m) => Some(
                    serde_json::from_str::<serde_json::Value>(&m)
                        .map_err(|e| anyhow::anyhow!("invalid --metadata JSON: {e}"))?,
                ),
                None => None,
            };
            (
                "write_engram",
                json!({
                    "domain": domain,
                    "title": title,
                    "content": body,
                    "folder": folder,
                    "type": engram_type,
                    "tags": split_commas(tags),
                    "status": status,
                    "metadata": metadata,
                    "overwrite": overwrite,
                }),
                config,
            )
        }
        Command::Read {
            identifier,
            domain,
            config,
        } => (
            "read_engram",
            json!({ "identifier": identifier, "domain": domain }),
            config,
        ),
        Command::Edit {
            identifier,
            domain,
            operation,
            content,
            section,
            find_text,
            expected_replacements,
            include_subsections,
            config,
        } => {
            let body = content_or_stdin(content)?;
            (
                "edit_engram",
                json!({
                    "identifier": identifier,
                    "domain": domain,
                    "operation": operation,
                    "content": body,
                    "section": section,
                    "find_text": find_text,
                    "expected_replacements": expected_replacements,
                    "include_subsections": include_subsections,
                }),
                config,
            )
        }
        Command::Move {
            identifier,
            domain,
            destination,
            destination_domain,
            no_update_links,
            config,
        } => (
            "move_engram",
            json!({
                "identifier": identifier,
                "domain": domain,
                "destination": destination,
                "destination_domain": destination_domain,
                "update_links": !no_update_links,
            }),
            config,
        ),
        Command::Delete {
            identifier,
            domain,
            config,
        } => (
            "delete_engram",
            json!({ "identifier": identifier, "domain": domain }),
            config,
        ),
        Command::Search {
            query,
            domain,
            engram_type,
            tag,
            status,
            after,
            search_type,
            min_similarity,
            limit,
            page,
            config,
        } => (
            "search_engrams",
            json!({
                "query": query,
                "domains": opt_vec(domain),
                "type": engram_type,
                "tags": opt_vec(tag),
                "status": status,
                "after": after,
                "search_type": search_type,
                "min_similarity": min_similarity,
                "limit": limit,
                "page": page,
            }),
            config,
        ),
        Command::Context {
            anchor,
            depth,
            domain,
            max_related,
            config,
        } => (
            "build_context",
            json!({
                "anchor": anchor,
                "depth": depth,
                "domains": opt_vec(domain),
                "max_related": max_related,
            }),
            config,
        ),
        Command::Recent {
            domain,
            timeframe,
            types,
            config,
        } => (
            "recent_activity",
            json!({
                "domains": opt_vec(domain),
                "timeframe": timeframe,
                "types": opt_vec(types),
            }),
            config,
        ),
        _ => unreachable!("run_data only handles data commands"),
    };

    let value = crystalline_service::run_tool(tool, args, db.as_deref(), config.as_deref()).await?;
    print_value(&value, json);
    Ok(())
}

/// Run an async command body on a fresh multi-threaded Tokio runtime. Kept off
/// the static `verify` and `prompt` paths so they never start a runtime.
fn on_runtime<F: std::future::Future<Output = anyhow::Result<()>>>(fut: F) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fut)
}

fn run_domain(command: DomainCommand, db: Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    match command {
        DomainCommand::Init { path, name } => cmd::domain_init(&path, name.as_deref(), json),
        DomainCommand::Add { name, path, config } => {
            cmd::domain_add(&name, &path, config.as_deref(), json)
        }
        DomainCommand::List { config } => {
            on_runtime(cmd::domain_list(config.as_deref(), db.as_deref(), json))
        }
        DomainCommand::Remove { name, config } => {
            cmd::domain_remove(&name, config.as_deref(), json)
        }
    }
}

fn run_verify(
    paths: Vec<PathBuf>,
    strict: bool,
    format: Option<OutputFormat>,
    config_path: Option<PathBuf>,
    json_flag: bool,
) -> anyhow::Result<()> {
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };

    let config_override = match config_path {
        Some(p) => Some(
            config::load_yaml(&p)
                .map_err(|e| anyhow::anyhow!("failed to load --config {}: {e}", p.display()))?,
        ),
        None => None,
    };

    let options = VerifyOptions {
        strict,
        config_override,
    };

    let report = match verify::verify_paths(paths, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("crystalline verify: {e}");
            std::process::exit(2);
        }
    };

    let format = resolve_format(format, json_flag);
    let color = format == OutputFormat::Human && std::io::stdout().is_terminal();
    let rendered = verify::render(to_core_format(format), &report, color);
    print!("{rendered}");
    if format != OutputFormat::Human && !rendered.ends_with('\n') {
        println!();
    }

    std::process::exit(report.exit_code());
}

fn run_prompt(
    workspace: Option<PathBuf>,
    config_path: Option<PathBuf>,
    json_flag: bool,
) -> anyhow::Result<()> {
    let workspace = workspace.unwrap_or_else(|| PathBuf::from("."));
    let cfg_path = match config_path {
        Some(p) => p,
        None => config::global_config_path()
            .map_err(|e| anyhow::anyhow!("could not resolve the default config path: {e}"))?,
    };

    let global = if cfg_path.is_file() {
        config::load_yaml(&cfg_path)
            .map_err(|e| anyhow::anyhow!("failed to load config {}: {e}", cfg_path.display()))?
    } else {
        config::GlobalConfig::default()
    };

    let output = crystalline_core::generate_prompt(&global, &workspace);
    for w in &output.warnings {
        eprintln!("crystalline prompt: warning: {w}");
    }

    if json_flag {
        println!("{}", crystalline_core::render_json(&output));
    } else {
        print!("{}", crystalline_core::render_text(&output));
    }
    Ok(())
}

fn resolve_format(format: Option<OutputFormat>, json_flag: bool) -> OutputFormat {
    match format {
        Some(f) => f,
        None if json_flag => OutputFormat::Json,
        None => OutputFormat::Human,
    }
}

fn to_core_format(f: OutputFormat) -> verify::Format {
    match f {
        OutputFormat::Human => verify::Format::Human,
        OutputFormat::Json => verify::Format::Json,
        OutputFormat::Github => verify::Format::Github,
    }
}
