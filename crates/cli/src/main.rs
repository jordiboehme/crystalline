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
    /// Generate a session-start knowledge routing prompt. No database,
    /// service or network connection is used.
    Prompt {
        #[command(subcommand)]
        kind: PromptKind,
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
        /// Force the host-lock claim in a shared database, migrating hosting of
        /// the synced domain to this instance even when another holds a live
        /// lock. Only meaningful when a daemon owns the shared index.
        #[arg(long)]
        take_over: bool,
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
        /// Serve the content API read-only: the four content-mutating tools are
        /// hidden and refused, while sync, watching and embedding still run.
        /// Overrides service.read_only when set; the mode is fixed for the
        /// daemon's lifetime.
        #[arg(long)]
        read_only: bool,
        /// Force host-lock claims for every file domain, migrating hosting to
        /// this instance in a shared database even when another instance holds a
        /// live lock. For a deliberate host migration.
        #[arg(long)]
        take_over: bool,
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
        /// Serve the content API read-only. Applies when this command runs the
        /// stack in-process or spawns a new daemon; attaching to an
        /// already-running daemon uses that daemon's mode instead.
        #[arg(long)]
        read_only: bool,
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
        /// The checksum from a prior read (guards a virtual-domain edit against a
        /// change since it was read; the edit is refused as a conflict if it
        /// changed). Omit for last-write-wins.
        #[arg(long)]
        expected_checksum: Option<String>,
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

/// The kind of prompt to generate. `system` is the only kind today; future
/// kinds attach here without reshaping the `prompt` command again.
#[derive(Subcommand, Debug)]
enum PromptKind {
    /// Generate the knowledge routing prompt for a workspace: registered
    /// domains, their routing bullets and the crystalline MCP tool names
    /// that read and write them.
    System {
        /// The workspace path to route for. Defaults to the current
        /// directory.
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Render the read-only variant: drop the write-tools line and state
        /// that this deployment's knowledge is curated externally. Forces the
        /// mode on regardless of service.read_only.
        #[arg(long)]
        read_only: bool,
        /// Load the global config from this file instead of the default
        /// config path.
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
        /// Force the host-lock claim, migrating hosting of the synced domain to
        /// this daemon in a shared database.
        #[arg(long)]
        take_over: bool,
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
    /// Register a domain in the global config, then index it immediately. A file
    /// domain refuses without a MANIFEST.md; `--virtual` registers a
    /// database-backed domain (no path) and scaffolds its MANIFEST in the index.
    Add {
        /// The domain name used everywhere it is referenced.
        name: String,
        /// The domain root directory. Omitted for a virtual domain.
        path: Option<PathBuf>,
        /// Register a virtual domain: engrams live in the database, not on disk.
        /// Incompatible with a path argument.
        #[arg(long = "virtual")]
        is_virtual: bool,
        /// Register only; skip indexing (run `crystalline sync` later). Applies
        /// to file domains only.
        #[arg(long)]
        no_sync: bool,
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
    /// Load already-well-formed engram files into a virtual domain, verbatim.
    /// Distinct from `crystalline import`, which converts a legacy tree into a
    /// file domain's directory.
    Import {
        /// The source directory of engram `.md` files.
        path: PathBuf,
        /// The target virtual domain. Must already be registered.
        #[arg(long)]
        domain: String,
        /// Overwrite engrams whose path or permalink already exists.
        #[arg(long)]
        overwrite: bool,
        /// Report what would be imported without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Materialize a domain's engrams from the index to a filesystem folder.
    /// Works for both file and virtual domains; most useful for taking a virtual
    /// domain's data out so `crystalline verify` can run on the snapshot.
    Export {
        /// The destination directory. Created if absent.
        path: PathBuf,
        /// The domain to export.
        #[arg(long)]
        domain: String,
        /// Write into a non-empty directory.
        #[arg(long)]
        force: bool,
        /// Report what would be exported without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Remove a domain from the global config. Leaves its files and index
    /// rows untouched (the rows are dropped by a later full reindex).
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
        Some(Command::Prompt { kind }) => match kind {
            PromptKind::System {
                workspace,
                read_only,
                config,
            } => run_prompt(workspace, read_only, config, cli.db, cli.json),
        },
        Some(Command::Domain { command }) => run_domain(command, cli.db, cli.json),
        Some(Command::Sync {
            domain,
            embed,
            take_over,
            config,
        }) => on_runtime(sync_dispatch(
            domain, embed, take_over, config, cli.db, cli.json,
        )),
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
            read_only,
            take_over,
            config,
        }) => on_runtime(crystalline_service::run_serve(
            daemon, http, cli.db, config, read_only, take_over,
        )),
        Some(Command::Mcp {
            embedded,
            read_only,
            config,
        }) => on_runtime(crystalline_service::run_mcp(
            embedded,
            cli.db.as_deref(),
            config.as_deref(),
            read_only,
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
/// `take_over` forces the daemon's host-lock claim for a shared-database host
/// migration; it is meaningless without a daemon (standalone sync holds no host
/// lock), so the direct path ignores it.
async fn sync_dispatch(
    domain: Option<String>,
    embed: bool,
    take_over: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    use serde_json::json;
    if let Some(data) = crystalline_service::ctl_if_running(
        json!({ "v": 1, "cmd": "sync", "domain": domain, "embed": embed, "take_over": take_over }),
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
        CtlCommand::Sync {
            domain,
            embed,
            take_over,
        } => {
            json!({ "v": 1, "cmd": "sync", "domain": domain, "embed": embed, "take_over": take_over })
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
            expected_checksum,
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
                    "expected_checksum": expected_checksum,
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

/// Like [`on_runtime`] but for a future that yields a plain value. Used by
/// `prompt system` to resolve virtual-domain routing bullets only when the
/// config actually has a virtual domain, so the common all-file path never
/// starts a runtime.
fn on_runtime_value<T, F: std::future::Future<Output = T>>(fut: F) -> anyhow::Result<T> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fut))
}

fn run_domain(command: DomainCommand, db: Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    match command {
        DomainCommand::Init { path, name } => cmd::domain_init(&path, name.as_deref(), json),
        DomainCommand::Add {
            name,
            path,
            is_virtual,
            no_sync,
            config,
        } => on_runtime(domain_add_dispatch(
            name, path, is_virtual, config, db, no_sync, json,
        )),
        DomainCommand::List { config } => {
            on_runtime(cmd::domain_list(config.as_deref(), db.as_deref(), json))
        }
        DomainCommand::Import {
            path,
            domain,
            overwrite,
            dry_run,
            config,
        } => on_runtime(domain_import_dispatch(
            domain, path, overwrite, dry_run, config, db, json,
        )),
        DomainCommand::Export {
            path,
            domain,
            force,
            dry_run,
            config,
        } => on_runtime(domain_export_dispatch(
            domain, path, force, dry_run, config, db, json,
        )),
        DomainCommand::Remove { name, config } => {
            on_runtime(domain_remove_dispatch(name, config, json))
        }
    }
}

/// `domain import`: verbatim load engram files into a virtual domain, over the
/// daemon when one owns the index, else against a directly opened store.
async fn domain_import_dispatch(
    domain: String,
    path: PathBuf,
    overwrite: bool,
    dry_run: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    let data = crystalline_service::domain_import(
        &domain,
        &path,
        overwrite,
        dry_run,
        db.as_deref(),
        config.as_deref(),
    )
    .await?;
    print_value(&data, json);
    Ok(())
}

/// `domain export`: materialize a domain's engrams to a filesystem folder, over
/// the daemon when one owns the index, else against a directly opened store.
async fn domain_export_dispatch(
    domain: String,
    path: PathBuf,
    force: bool,
    dry_run: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    let data = crystalline_service::domain_export(
        &domain,
        &path,
        force,
        dry_run,
        db.as_deref(),
        config.as_deref(),
    )
    .await?;
    print_value(&data, json);
    Ok(())
}

/// `domain add`: register locally (always, regardless of a running daemon),
/// then index immediately - routed to the daemon's ctl sync when one is
/// running, else synced directly, the same dispatch `sync` itself uses.
/// `--no-sync` registers only. `--virtual` registers a database-backed domain
/// and scaffolds its MANIFEST into the index instead of syncing files.
async fn domain_add_dispatch(
    name: String,
    path: Option<PathBuf>,
    is_virtual: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    no_sync: bool,
    json: bool,
) -> anyhow::Result<()> {
    if is_virtual {
        if path.is_some() {
            anyhow::bail!(
                "`domain add --virtual` takes no path; a virtual domain has no directory"
            );
        }
        let markdown = cmd::domain_add_register_virtual(&name, config.as_deref())?;
        let scaffold = crystalline_service::scaffold_virtual_manifest(
            &name,
            &markdown,
            db.as_deref(),
            config.as_deref(),
        )
        .await?;
        cmd::print_domain_add_virtual(&name, &scaffold, json);
        return Ok(());
    }

    let path =
        path.ok_or_else(|| anyhow::anyhow!("`domain add` requires a path (or use --virtual)"))?;
    let abs = cmd::domain_add_register(&name, &path, config.as_deref())?;
    if no_sync {
        cmd::print_domain_add_no_sync(&name, &abs, json);
        return Ok(());
    }

    use serde_json::json as j;
    let report: crystalline_index::SyncReport = if let Some(data) =
        crystalline_service::ctl_if_running(
            j!({ "v": 1, "cmd": "sync", "domain": name, "embed": false }),
        )
        .await?
    {
        let first = data
            .get("reports")
            .and_then(|r| r.get(0))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(first)
            .map_err(|e| anyhow::anyhow!("could not parse the daemon's sync report: {e}"))?
    } else {
        cmd::sync_domain_direct(&name, &abs, config.as_deref(), db.as_deref()).await?
    };

    cmd::print_domain_add(&name, &abs, &report, json);
    Ok(())
}

/// `domain remove`: drop it from the config, then best-effort tell a running
/// daemon to stop watching its path. Never fails on the ctl round trip; the
/// config edit already succeeded by the time it runs.
async fn domain_remove_dispatch(
    name: String,
    config: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    cmd::domain_remove(&name, config.as_deref(), json)?;
    use serde_json::json as j;
    let _ =
        crystalline_service::ctl_if_running(j!({ "v": 1, "cmd": "forget_domain", "domain": name }))
            .await;
    Ok(())
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
    read_only_flag: bool,
    config_path: Option<PathBuf>,
    db: Option<PathBuf>,
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

    // Virtual domains have no MANIFEST on disk; their routing bullets come from
    // the daemon (warm) or a direct store read. The all-file common case never
    // opens a runtime, so it stays as fast and deterministic as before.
    let virtual_bullets = if global.domains.values().any(|e| e.is_virtual()) {
        on_runtime_value(crystalline_service::virtual_routing_bullets(
            &global,
            db.as_deref(),
        ))?
    } else {
        std::collections::BTreeMap::new()
    };

    let mut output = crystalline_core::generate_prompt(&global, &workspace, &virtual_bullets);
    // The flag forces the read-only variant on top of service.read_only; it can
    // only turn the mode on, matching the daemon precedence.
    if read_only_flag {
        output.read_only = true;
    }
    for w in &output.warnings {
        eprintln!("crystalline prompt system: warning: {w}");
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
