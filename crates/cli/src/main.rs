//! `crystalline`: the command-line entry point. Wires the crate stack
//! (core, index, service) into subcommands.
//!
//! `verify`, `prompt` and `hook` are static: none of them opens a database,
//! a socket or a network connection, and none starts a Tokio runtime. Every
//! other subcommand lands in a later milestone.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crystalline_core::config;
use crystalline_core::verify::{self, VerifyOptions};

mod cmd;
mod doctor;
mod hook;
mod install;
mod receipt;

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
    /// Connect this machine to GitHub, for sharing and updating team domains.
    Connect {
        #[command(subcommand)]
        command: ConnectCommand,
    },
    /// Wire a coding harness up to Crystalline in one idempotent step:
    /// register the MCP server, install the SessionStart routing hook and the
    /// Stop capture-nudge hook and copy the four topical skills into place.
    /// Safe to re-run; a second run that finds everything already in place
    /// writes nothing and reports it as already present. Static like `verify`
    /// and `prompt`: no database, service or network connection. A missing or
    /// failing harness CLI is never fatal - the MCP command to run by hand is
    /// printed and the hooks and skills still install.
    Install {
        /// Which harness to wire up.
        #[arg(value_enum)]
        harness: install::HarnessKind,
        /// Write into the current repository's harness config (.claude,
        /// .codex or .agents under the working directory) instead of this
        /// user's global one. Codex still registers its MCP server per user.
        #[arg(long)]
        project: bool,
        /// Skip registering the MCP server.
        #[arg(long)]
        skip_mcp: bool,
        /// Skip installing the SessionStart and Stop hooks.
        #[arg(long)]
        skip_hooks: bool,
        /// Skip copying the topical skills.
        #[arg(long)]
        skip_skills: bool,
    },
    /// Reverse `crystalline install` for a harness: deregister the MCP server,
    /// remove the managed hooks and drop the copied skills, leaving every
    /// hook, key and skill that is not Crystalline's own untouched. A skill a
    /// person edited by hand is kept unless `--force` is given.
    Uninstall {
        /// Which harness to unwire.
        #[arg(value_enum)]
        harness: install::HarnessKind,
        /// Act on the current repository's harness config instead of this
        /// user's global one.
        #[arg(long)]
        project: bool,
        /// Remove a copied skill even when its SKILL.md was edited locally.
        #[arg(long)]
        force: bool,
    },
    /// Bring a team domain up to date with its origin, or check where it
    /// stands.
    Origin {
        #[command(subcommand)]
        command: OriginCommand,
    },
    /// Show, set or reset an agent-adjustable setting (see the settings
    /// registry, currently the `github.*` block).
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
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
        /// Ensure each folder is registered as a domain (creating it and a
        /// MANIFEST.md when needed) before serving. Repeatable, and a single
        /// occurrence may itself list more than one path, matching how Claude
        /// Desktop expands an MCPB bundle's picked-folders array into
        /// `--domain a b`. A restart with the same folders is a cheap no-op.
        #[arg(long, num_args = 1.., action = clap::ArgAction::Append)]
        domain: Vec<PathBuf>,
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
    /// Respond to a harness lifecycle hook event over stdin/stdout. Plumbing
    /// for `crystalline install`'s generated hook wiring, not something a
    /// person runs by hand - documented here so anyone who finds it in a
    /// harness's settings file can identify what it is. Static like `verify`
    /// and `prompt`: no database, service or network connection, and a call
    /// completes in tens of milliseconds. Silent (exit 0, empty stdout) on
    /// every call that is not the one earning a nudge, since a hook must
    /// never be the reason a harness's turn breaks.
    Hook {
        #[command(subcommand)]
        event: HookEvent,
    },
}

/// Which harness lifecycle event a `hook` invocation answers. `Stop` is the
/// only kind today; future events attach here without reshaping `hook`
/// again.
#[derive(Subcommand, Debug)]
enum HookEvent {
    /// Once per substantive session, on the first Stop call that earns it:
    /// print a reminder to review the conversation for durable learnings and
    /// propose capturing them. Every other call - a stale or malformed
    /// payload, a hook-caused continuation, an unconfigured or read-only
    /// install, a session already nudged or a session too short to be worth
    /// interrupting - is silent.
    Stop,
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
    /// database-backed domain (no path) and scaffolds its MANIFEST in the index;
    /// `--origin` connects it to a GitHub repository, downloading its tracked
    /// subtree. A non-empty target folder, or a domain name that is already
    /// registered without an origin, connects in place: local files are kept
    /// and ones that differ from the repository become shareable local changes.
    Add {
        /// The domain name used everywhere it is referenced.
        name: String,
        /// The domain root directory. Omitted for a virtual domain. With
        /// `--origin`, this is where the team domain lives on this machine
        /// (existing files are kept); defaults to
        /// ~/Documents/Crystalline/<name> when omitted.
        path: Option<PathBuf>,
        /// Register a virtual domain: engrams live in the database, not on disk.
        /// Incompatible with a path argument.
        #[arg(long = "virtual")]
        is_virtual: bool,
        /// Connect to a GitHub repository: owner/repo, or owner/repo/subpath
        /// when the team domain is a subfolder of the repository. Requires
        /// github.enabled and a prior `crystalline connect github`.
        #[arg(long)]
        origin: Option<String>,
        /// The branch to track. Only meaningful with --origin; defaults to
        /// main.
        #[arg(long)]
        branch: Option<String>,
        /// Register only; skip indexing (run `crystalline sync` later). Applies
        /// to file domains only; incompatible with --origin, which always
        /// indexes what it downloads.
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

#[derive(Subcommand, Debug)]
enum ConnectCommand {
    /// Sign in to GitHub: paste a personal access token, or sign in with a
    /// short code in the browser. Works whether or not team domains are
    /// turned on yet (signing in is this machine's identity, not content).
    Github {
        /// A GitHub personal access token, skipping the browser sign-in.
        #[arg(long)]
        token: Option<String>,
        /// A GitHub Enterprise Server host, for example
        /// github.acme.example. Defaults to github.com, or the configured
        /// github.api_url.
        #[arg(long)]
        host: Option<String>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum OriginCommand {
    /// Bring one team domain (or every one connected to an origin) up to
    /// date with its origin.
    Update {
        /// Update only this domain instead of every team domain.
        #[arg(long)]
        domain: Option<String>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Show where one team domain (or every one connected to an origin)
    /// stands relative to its origin, and whether this machine is connected
    /// to GitHub.
    Status {
        /// Report only this domain instead of every team domain.
        #[arg(long)]
        domain: Option<String>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Propose a team domain's local changes as a pull request against its
    /// origin.
    Share {
        /// The team domain to share local changes from.
        domain: String,
        /// The proposal's title. Defaults to a generated one-liner from the
        /// change mix.
        #[arg(long)]
        title: Option<String>,
        /// The proposal's description. Defaults to a generated summary.
        #[arg(long)]
        message: Option<String>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Discard a declined, or still-open ("never mind"), share proposal,
    /// restoring local files that were not changed since sharing them.
    Discard {
        /// The team domain the proposal belongs to.
        domain: String,
        /// The proposal number to discard.
        #[arg(long)]
        proposal: u64,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Resolve one recorded conflict for a team domain.
    Resolve {
        /// The team domain the conflict belongs to.
        domain: String,
        /// The conflicting file's path, relative to the domain root.
        path: String,
        /// Keep the local copy (mine) or take upstream's (theirs).
        /// Exactly one of --keep or --content-file is required.
        #[arg(long)]
        keep: Option<KeepSide>,
        /// Write this file's bytes as the resolved merge instead of keeping
        /// either side verbatim. Exactly one of --keep or --content-file is
        /// required.
        #[arg(long)]
        content_file: Option<PathBuf>,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

/// Which side of a conflict `origin resolve --keep` keeps.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum KeepSide {
    Mine,
    Theirs,
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Show every setting's effective value.
    Show {
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Set a setting to a value.
    Set {
        /// The setting key, for example github.enabled.
        key: String,
        /// The value to set.
        value: String,
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Reset a setting to its default.
    Unset {
        /// The setting key, for example github.enabled.
        key: String,
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
        Some(Command::Connect { command }) => on_runtime(move || run_connect(command, cli.json)),
        Some(Command::Install {
            harness,
            project,
            skip_mcp,
            skip_hooks,
            skip_skills,
        }) => install::run_install(
            install::InstallOptions {
                harness,
                project,
                skip_mcp,
                skip_hooks,
                skip_skills,
            },
            cli.json,
        ),
        Some(Command::Uninstall {
            harness,
            project,
            force,
        }) => install::run_uninstall(harness, project, force, cli.json),
        Some(Command::Origin { command }) => {
            on_runtime(move || run_origin(command, cli.db, cli.json))
        }
        Some(Command::Config { command }) => on_runtime(move || config_dispatch(command, cli.json)),
        Some(Command::Sync {
            domain,
            embed,
            take_over,
            config,
        }) => on_runtime(move || sync_dispatch(domain, embed, take_over, config, cli.db, cli.json)),
        Some(Command::Reindex {
            full,
            embed,
            config,
        }) => on_runtime(move || reindex_dispatch(full, embed, config, cli.db, cli.json)),
        Some(Command::Status { config }) => {
            on_runtime(move || status_dispatch(config, cli.db, cli.json))
        }
        Some(Command::Model { command }) => match command {
            ModelCommand::Download { config } => {
                let json = cli.json;
                on_runtime(
                    move || async move { cmd::model_download(config.as_deref(), json).await },
                )
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
        }) => on_runtime(move || run_doctor(domain, fix, config, cli.db, cli.json)),
        Some(Command::Serve {
            http,
            daemon,
            read_only,
            take_over,
            config,
        }) => on_runtime(move || {
            crystalline_service::run_serve(daemon, http, cli.db, config, read_only, take_over)
        }),
        Some(Command::Mcp {
            embedded,
            read_only,
            domain,
            config,
        }) => on_runtime(move || mcp_dispatch(domain, embedded, read_only, config, cli.db)),
        Some(Command::Ctl { command }) => on_runtime(move || run_ctl(command, cli.json)),
        Some(
            cmd @ (Command::Write { .. }
            | Command::Read { .. }
            | Command::Edit { .. }
            | Command::Move { .. }
            | Command::Delete { .. }
            | Command::Search { .. }
            | Command::Context { .. }
            | Command::Recent { .. }),
        ) => on_runtime(move || run_data(cmd, cli.db, cli.json)),
        Some(Command::Hook { event }) => match event {
            HookEvent::Stop => {
                hook::run_stop();
                Ok(())
            }
        },
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

/// `status`: report from the daemon over ctl when one is running and no explicit
/// `--config`/`--db` override was given, else open the index directly. An
/// override names an exact config and index the running daemon may not serve, so
/// it always takes the direct path (see [`crystalline_service::use_daemon`]).
/// Both paths render the same human shape (or, with `--json`, the same JSON
/// shape), and the first output line always says which view this is - the
/// daemon's, a direct read because none runs, or a direct read because an
/// override bypassed it.
async fn status_dispatch(
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    use serde_json::json;
    let bypassed = db.is_some() || config.is_some();
    if !bypassed {
        if let Some(data) =
            crystalline_service::ctl_if_running(json!({ "v": 1, "cmd": "status" })).await?
        {
            if json {
                println!("{data}");
            } else {
                let note = format!(
                    "running (pid {}, v{}, up {})",
                    data["pid"].as_u64().unwrap_or(0),
                    data["version"].as_str().unwrap_or("unknown"),
                    format_uptime(data["uptime_secs"].as_u64().unwrap_or(0)),
                );
                cmd::render_status(&data, &note);
            }
            return Ok(());
        }
        // A live daemon that did not answer means the numbers below come
        // from a different index than the one agents are using; say so
        // instead of silently reporting an empty view.
        if let Some(info) = crystalline_service::instance::read_lock_info()
            && crystalline_service::instance::process_alive(info.pid)
        {
            eprintln!(
                "note: a daemon (pid {}, v{}) holds {} but did not answer; reporting from a direct index read instead",
                info.pid, info.version, info.socket_path
            );
        }
    }
    let note = if bypassed {
        "bypassed (--db/--config override); reading the index directly"
    } else {
        "not running; reading the index directly"
    };
    cmd::status(config.as_deref(), db.as_deref(), json, note).await
}

/// Render seconds of uptime compactly: `42s`, `12m` or `3h07m`.
fn format_uptime(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// `sync`: route to the daemon when one owns the index and no explicit
/// `--config`/`--db` override was given, else sync directly. An override names
/// an exact config and index the running daemon may not serve, so it always
/// takes the direct path (see [`crystalline_service::use_daemon`]).
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
    if crystalline_service::use_daemon(db.as_deref(), config.as_deref())
        && let Some(data) = crystalline_service::ctl_if_running(
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

/// `reindex`: route to the daemon when one owns the index and no explicit
/// `--config`/`--db` override was given, else reindex directly. An override
/// names an exact config and index the running daemon may not serve, so it
/// always takes the direct path (see [`crystalline_service::use_daemon`]).
async fn reindex_dispatch(
    full: bool,
    embed: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    use serde_json::json;
    if crystalline_service::use_daemon(db.as_deref(), config.as_deref())
        && let Some(data) = crystalline_service::ctl_if_running(
            json!({ "v": 1, "cmd": "reindex", "full": full, "embed": embed }),
        )
        .await?
    {
        print_value(&data, json);
        return Ok(());
    }
    cmd::reindex(full, embed, config.as_deref(), db.as_deref(), json).await
}

/// `config show|set|unset`: route over the daemon's ctl `configure` command
/// when one is running, else act on the config file directly. Neither path
/// opens the index; settings live in `config.yaml`, not the database.
async fn config_dispatch(command: ConfigCommand, json: bool) -> anyhow::Result<()> {
    match command {
        ConfigCommand::Show { config } => {
            let data =
                crystalline_service::configure("show", None, None, config.as_deref()).await?;
            if json {
                print_value(&data, true);
                return Ok(());
            }
            let views: Vec<crystalline_service::settings::SettingView> =
                serde_json::from_value(data["settings"].clone())?;
            render_settings_table(&views);
            Ok(())
        }
        ConfigCommand::Set { key, value, config } => {
            let data =
                crystalline_service::configure("set", Some(&key), Some(&value), config.as_deref())
                    .await?;
            print_setting_result(&data, json);
            Ok(())
        }
        ConfigCommand::Unset { key, config } => {
            let data = crystalline_service::configure("unset", Some(&key), None, config.as_deref())
                .await?;
            print_setting_result(&data, json);
            Ok(())
        }
    }
}

/// Render `config show`'s snapshot: key, effective value (with a source
/// marker for defaulted or env-overridden entries) and doc line, columns
/// aligned to the widest entry.
fn render_settings_table(views: &[crystalline_service::settings::SettingView]) {
    let key_width = views.iter().map(|v| v.key.len()).max().unwrap_or(0);
    let displayed: Vec<String> = views.iter().map(setting_display_value).collect();
    let value_width = displayed.iter().map(|v| v.len()).max().unwrap_or(0);
    for (view, value) in views.iter().zip(&displayed) {
        println!(
            "{:<key_width$}  {:<value_width$}  {}",
            view.key, value, view.doc
        );
    }
}

/// A setting's effective value with a "(default)" or "(env)" marker when it
/// is not read straight from the config file.
fn setting_display_value(view: &crystalline_service::settings::SettingView) -> String {
    use crystalline_service::settings::SettingSource;
    match view.source {
        SettingSource::Default => format!("{} (default)", view.value),
        SettingSource::Config => view.value.clone(),
        SettingSource::Env => format!("{} (env)", view.value),
    }
}

/// Print a `config set`/`config unset` result: the resulting setting, and
/// any note attached to it (for example, that a startup-effective key needs
/// a daemon restart to take effect).
fn print_setting_result(data: &serde_json::Value, json: bool) {
    if json {
        print_value(data, true);
        return;
    }
    let key = data["key"].as_str().unwrap_or("");
    let value = data["value"].as_str().unwrap_or("");
    match data["source"].as_str().unwrap_or("config") {
        "default" => println!("{key} = {value} (default)"),
        "env" => println!("{key} = {value} (env)"),
        _ => println!("{key} = {value}"),
    }
    if let Some(note) = data["note"].as_str() {
        println!("  {note}");
    }
}

/// `connect github`: sign this machine in to GitHub, always in-process (no
/// daemon involved - signing in is this machine's identity, not content).
async fn run_connect(command: ConnectCommand, json: bool) -> anyhow::Result<()> {
    match command {
        ConnectCommand::Github {
            token,
            host,
            config,
        } => cmd::connect_github(token.as_deref(), host.as_deref(), config.as_deref(), json).await,
    }
}

/// `origin update`/`origin status`/`origin share`/`origin discard`/
/// `origin resolve`: socket-first with an in-process fallback, all already
/// handled inside their respective `crystalline_service` entry points.
async fn run_origin(command: OriginCommand, db: Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    match command {
        OriginCommand::Update { domain, config } => {
            let data = crystalline_service::origin_update(
                domain.as_deref(),
                db.as_deref(),
                config.as_deref(),
            )
            .await?;
            print_origin_update(&data, json);
            Ok(())
        }
        OriginCommand::Status { domain, config } => {
            let data = crystalline_service::origin_status(
                domain.as_deref(),
                db.as_deref(),
                config.as_deref(),
            )
            .await?;
            print_origin_status(&data, json);
            Ok(())
        }
        OriginCommand::Share {
            domain,
            title,
            message,
            config,
        } => {
            let data = crystalline_service::origin_share(
                &domain,
                title.as_deref(),
                message.as_deref(),
                db.as_deref(),
                config.as_deref(),
            )
            .await?;
            cmd::print_origin_share(&domain, &data, json);
            Ok(())
        }
        OriginCommand::Discard {
            domain,
            proposal,
            config,
        } => {
            let data = crystalline_service::origin_discard(
                &domain,
                proposal,
                db.as_deref(),
                config.as_deref(),
            )
            .await?;
            cmd::print_origin_discard(&data, json);
            Ok(())
        }
        OriginCommand::Resolve {
            domain,
            path,
            keep,
            content_file,
            config,
        } => {
            if keep.is_some() == content_file.is_some() {
                anyhow::bail!("`origin resolve` requires exactly one of --keep or --content-file");
            }
            let content = match &content_file {
                Some(f) => Some(cmd::read_resolve_content(f)?),
                None => None,
            };
            let keep_str = keep.map(|k| match k {
                KeepSide::Mine => "mine",
                KeepSide::Theirs => "theirs",
            });
            let data = crystalline_service::origin_resolve(
                &domain,
                &path,
                keep_str,
                content.as_deref(),
                db.as_deref(),
                config.as_deref(),
            )
            .await?;
            cmd::print_origin_resolve(&data, json);
            Ok(())
        }
    }
}

/// Render `origin update`'s aggregate result: one line per team domain (up
/// to date, or the applied/merged counts with conflicts and proposal
/// transitions called out), then one line per domain that failed to update.
/// Conflicts only name the path: resolution tooling arrives in a later task.
fn print_origin_update(data: &serde_json::Value, json: bool) {
    if json {
        print_value(data, true);
        return;
    }
    let empty = Vec::new();
    let domains = data["domains"].as_array().unwrap_or(&empty);
    if domains.is_empty() {
        println!("No team domains to update.");
    }
    for d in domains {
        let name = d["domain"].as_str().unwrap_or("");
        if d["provisioned"].as_bool().unwrap_or(false) {
            println!(
                "{name}: provisioned {} engram(s) at {}",
                d["engrams"].as_u64().unwrap_or(0),
                d["base_commit"].as_str().unwrap_or("")
            );
            continue;
        }
        if d["up_to_date"].as_bool().unwrap_or(false) {
            println!("{name}: up to date");
            continue;
        }
        let applied = d["applied"].as_array().map(Vec::len).unwrap_or(0);
        let merged = d["merged"].as_array().map(Vec::len).unwrap_or(0);
        println!("{name}: {applied} file(s) applied ({merged} merged)");
        for c in d["conflicts"].as_array().unwrap_or(&empty) {
            println!(
                "  conflict: {} (resolution tooling is coming; left as it was)",
                c["path"].as_str().unwrap_or("")
            );
        }
        for p in d["proposals"].as_array().unwrap_or(&empty) {
            let number = p["number"].as_u64().unwrap_or(0);
            let status = p["status"].as_str().unwrap_or("");
            match p["url"].as_str() {
                Some(url) => {
                    let title = p["title"].as_str().unwrap_or("");
                    println!("  proposal #{number}: {status} - {title} ({url})");
                }
                None => println!("  proposal #{number}: {status}"),
            }
        }
    }
    for e in data["errors"].as_array().unwrap_or(&empty) {
        println!(
            "{}: could not update: {}",
            e["domain"].as_str().unwrap_or(""),
            e["error"].as_str().unwrap_or("")
        );
    }
}

/// Render `origin status`'s result: the connection line, then per team
/// domain its repo, branch, how far ahead (local changes) and behind it is,
/// a note when the live probe itself failed (offline, rate limited, an
/// expired connection) rather than the whole domain, open and declined
/// proposals with their urls, unresolved conflicts and when it was last
/// checked, then one line per domain that genuinely failed to report.
fn print_origin_status(data: &serde_json::Value, json: bool) {
    if json {
        print_value(data, true);
        return;
    }
    let connection = &data["connection"];
    if connection["connected"].as_bool().unwrap_or(false) {
        if connection["token_store"].as_str() == Some("environment") {
            println!("GitHub: connected via CRYSTALLINE_GITHUB_TOKEN (environment token store)");
        } else {
            println!(
                "GitHub: connected as {} ({} token store)",
                connection["user"].as_str().unwrap_or("?"),
                connection["token_store"].as_str().unwrap_or("?")
            );
        }
    } else {
        println!("GitHub: not connected. Run: crystalline connect github");
    }

    let empty = Vec::new();
    let domains = data["domains"].as_array().unwrap_or(&empty);
    if domains.is_empty() {
        println!("No team domains connected to an origin.");
    }
    for d in domains {
        let name = d["domain"].as_str().unwrap_or("");
        let repo = d["repo"].as_str().unwrap_or("");
        let branch = d["branch"].as_str().unwrap_or("");
        println!("{name}: {repo}@{branch}");
        println!(
            "  ahead: {} local change(s)",
            d["local_changes"].as_u64().unwrap_or(0)
        );
        println!(
            "  behind: {}",
            match d["behind"].as_bool() {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown (offline)",
            }
        );
        if let Some(probe_error) = d["probe_error"].as_str() {
            println!("  probe failed, reporting from local state only: {probe_error}");
        }
        for p in d["open_proposals"].as_array().unwrap_or(&empty) {
            println!(
                "  open proposal #{}: {} - {}",
                p["number"].as_u64().unwrap_or(0),
                p["title"].as_str().unwrap_or(""),
                p["url"].as_str().unwrap_or("")
            );
        }
        for p in d["declined_proposals"].as_array().unwrap_or(&empty) {
            println!(
                "  declined proposal #{}: {} - {}",
                p["number"].as_u64().unwrap_or(0),
                p["title"].as_str().unwrap_or(""),
                p["url"].as_str().unwrap_or("")
            );
        }
        for c in d["conflicts"].as_array().unwrap_or(&empty) {
            println!(
                "  unresolved conflict: {}",
                c["path"].as_str().unwrap_or("")
            );
        }
        println!(
            "  last checked: {}",
            d["last_checked"].as_str().unwrap_or("never")
        );
    }
    for e in data["errors"].as_array().unwrap_or(&empty) {
        println!(
            "{}: could not report status: {}",
            e["domain"].as_str().unwrap_or(""),
            e["error"].as_str().unwrap_or("")
        );
    }
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
fn on_runtime<F, Fut>(make: F) -> anyhow::Result<()>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    on_runtime_value(make)?
}

/// Like [`on_runtime`] but for a future that yields a plain value. Used by
/// `prompt system` to resolve virtual-domain routing bullets only when the
/// config actually has a virtual domain, so the common all-file path never
/// starts a runtime.
///
/// Takes a closure that builds the future, not the future itself, and calls
/// it on a dedicated thread with an explicit 8 MiB stack. An async fn's
/// future is materialized on the stack of whoever calls it, so passing a
/// built future would land its whole state machine on the process main
/// thread first - and Windows gives that thread 1 MiB where Linux and macOS
/// give 8, which the larger command futures overflow in unoptimized builds.
/// Building inside the deep thread keeps the main thread's stack usage
/// independent of how big any command's future grows.
fn on_runtime_value<T, F, Fut>(make: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T>,
{
    let worker = std::thread::Builder::new()
        .name("crystalline-cmd".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            anyhow::Ok(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?
                    .block_on(make()),
            )
        })?;
    match worker.join() {
        Ok(result) => result,
        // The default hook already printed the panic message on the worker;
        // re-raising on the main thread preserves the process-level behavior
        // block_on-on-main had.
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn run_domain(command: DomainCommand, db: Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    match command {
        DomainCommand::Init { path, name } => cmd::domain_init(&path, name.as_deref(), json),
        DomainCommand::Add {
            name,
            path,
            is_virtual,
            origin,
            branch,
            no_sync,
            config,
        } => on_runtime(move || {
            domain_add_dispatch(
                name, path, is_virtual, origin, branch, config, db, no_sync, json,
            )
        }),
        DomainCommand::List { config } => on_runtime(move || async move {
            cmd::domain_list(config.as_deref(), db.as_deref(), json).await
        }),
        DomainCommand::Import {
            path,
            domain,
            overwrite,
            dry_run,
            config,
        } => on_runtime(move || {
            domain_import_dispatch(domain, path, overwrite, dry_run, config, db, json)
        }),
        DomainCommand::Export {
            path,
            domain,
            force,
            dry_run,
            config,
        } => on_runtime(move || {
            domain_export_dispatch(domain, path, force, dry_run, config, db, json)
        }),
        DomainCommand::Remove { name, config } => {
            on_runtime(move || domain_remove_dispatch(name, config, json))
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

/// `mcp`: ensure every `--domain` folder is registered (the Claude Desktop
/// MCPB bundle entry point) before deciding whether to attach to a running
/// daemon or start one, so both paths see the registration - a freshly
/// spawned daemon reads it from its own fresh config load, an already-running
/// one is told to sync (and start watching) it here, mirroring
/// `domain_add_dispatch`. A folder that fails to register (a bad path, an
/// unwritable parent) or fails to sync with a live daemon is reported to
/// stderr and skipped rather than aborting: the MCP server must still start
/// even when one picked folder is broken.
async fn mcp_dispatch(
    domains: Vec<PathBuf>,
    embedded: bool,
    read_only: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
) -> anyhow::Result<()> {
    use serde_json::json;
    for path in &domains {
        match cmd::ensure_domain_registered(path, config.as_deref()) {
            Ok(Some(name)) => {
                eprintln!(
                    "crystalline mcp: registered domain '{name}' at {}",
                    path.display()
                );
                // Nudge a running daemon to sync (and start watching) the new
                // root only when the registration landed in the daemon's own
                // config; an explicit --config wrote a different file the daemon
                // does not serve, so it has nothing to sync. The --db override
                // is deliberately not gated on here: `run_mcp` below attaches to
                // a running daemon regardless of --db, so the notify must still
                // fire whenever the domain reached the daemon's config, keeping
                // this session and the daemon in step.
                if config.is_none()
                    && let Err(e) = crystalline_service::ctl_if_running(
                        json!({ "v": 1, "cmd": "sync", "domain": name, "embed": false }),
                    )
                    .await
                {
                    eprintln!(
                        "crystalline mcp: warning: could not sync newly registered domain '{name}' with the running daemon: {e}"
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "crystalline mcp: warning: could not register domain at {}: {e}",
                    path.display()
                );
            }
        }
    }

    crystalline_service::run_mcp(embedded, db.as_deref(), config.as_deref(), read_only).await
}

/// `domain add`: register locally (always, regardless of a running daemon),
/// then index immediately - routed to the daemon's ctl sync when one is
/// running, else synced directly, the same dispatch `sync` itself uses.
/// `--no-sync` registers only. `--virtual` registers a database-backed domain
/// and scaffolds its MANIFEST into the index instead of syncing files.
#[allow(clippy::too_many_arguments)]
async fn domain_add_dispatch(
    name: String,
    path: Option<PathBuf>,
    is_virtual: bool,
    origin: Option<String>,
    branch: Option<String>,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    no_sync: bool,
    json: bool,
) -> anyhow::Result<()> {
    if let Some(origin_spec) = origin {
        return domain_add_origin_dispatch(
            name,
            path,
            origin_spec,
            branch,
            is_virtual,
            no_sync,
            config,
            db,
            json,
        )
        .await;
    }
    if branch.is_some() {
        anyhow::bail!("`domain add --branch` requires --origin");
    }

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

    // Index over the daemon only when no explicit --config/--db override was
    // given: an override names an exact config and index, so the sync must land
    // there via the direct path rather than in whatever the running daemon
    // serves (see [`crystalline_service::use_daemon`]).
    use serde_json::json as j;
    let report: crystalline_index::SyncReport =
        if crystalline_service::use_daemon(db.as_deref(), config.as_deref())
            && let Some(data) = crystalline_service::ctl_if_running(
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

/// `domain add --origin`: connects a team domain to a GitHub repository
/// instead of an existing local folder. Routes socket-first to the daemon's
/// `origin_add` ctl command (via `crystalline_service::origin_add`, which
/// falls back to the in-process engine path other routable verbs use when no
/// daemon is running); the config write, the download and the indexing all
/// happen on whichever side answers, never split across the CLI and the
/// engine the way the local-folder path splits registration from sync.
#[allow(clippy::too_many_arguments)]
async fn domain_add_origin_dispatch(
    name: String,
    path: Option<PathBuf>,
    origin_spec: String,
    branch: Option<String>,
    is_virtual: bool,
    no_sync: bool,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    if is_virtual {
        anyhow::bail!("`domain add --origin` cannot be combined with --virtual");
    }
    if no_sync {
        anyhow::bail!(
            "`domain add --origin` cannot be combined with --no-sync; a team domain is indexed as part of connecting it"
        );
    }
    let (repo, subpath) = cmd::parse_origin_spec(&origin_spec)?;
    let folder = match path {
        Some(p) => Some(cmd::absolute_path(&p)?),
        None => None,
    };
    let folder_str = folder.as_ref().map(|p| p.display().to_string());

    let data = crystalline_service::origin_add(
        &repo,
        Some(&name),
        subpath.as_deref(),
        branch.as_deref(),
        folder_str.as_deref(),
        db.as_deref(),
        config.as_deref(),
    )
    .await?;
    cmd::print_origin_add(&repo, &data, json);
    Ok(())
}

/// `domain remove`: drop it from the config, then best-effort tell a running
/// daemon to stop watching its path. Never fails on the ctl round trip; the
/// config edit already succeeded by the time it runs. The notify fires only
/// when the removal happened in the daemon's own config: an explicit --config
/// edited a different file the daemon does not serve, so its watch set is
/// unaffected and there is nothing to forget.
async fn domain_remove_dispatch(
    name: String,
    config: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    cmd::domain_remove(&name, config.as_deref(), json)?;
    use serde_json::json as j;
    if config.is_none() {
        let _ = crystalline_service::ctl_if_running(
            j!({ "v": 1, "cmd": "forget_domain", "domain": name }),
        )
        .await;
    }
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
    // Session-start auto-update: a binary upgraded since the last install
    // refreshes the installed hooks and skills before this session's routing
    // prompt goes out. Cheap when versions match (one small-file read) and
    // best-effort always; outcomes surface only as trailing notice lines on
    // the text output.
    let reconcile_notices = install::auto_reconcile(
        env!("CARGO_PKG_VERSION"),
        &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    );

    let workspace = workspace.unwrap_or_else(|| PathBuf::from("."));
    // The single load chokepoint resolves the config path (flag, then
    // CRYSTALLINE_CONFIG, then the default) and applies the environment overlay,
    // so the routing prompt reflects env-configured settings.
    let global = crystalline_service::overlay::load(config_path.as_deref())?.effective;

    // Virtual domains have no MANIFEST on disk; their routing bullets come from
    // the daemon (warm) or a direct store read. The all-file common case never
    // opens a runtime, so it stays as fast and deterministic as before.
    let virtual_bullets = if global.domains.values().any(|e| e.is_virtual()) {
        // Owned copies: the future moves onto the runtime worker thread. The
        // raw --config override rides along so it bypasses the daemon just like
        // every other verb, even though `global` was already resolved from it.
        let global_bullets = global.clone();
        let db_bullets = db.clone();
        let config_bullets = config_path.clone();
        on_runtime_value(move || async move {
            crystalline_service::virtual_routing_bullets(
                &global_bullets,
                db_bullets.as_deref(),
                config_bullets.as_deref(),
            )
            .await
        })?
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
        for note in &reconcile_notices {
            println!("{note}");
        }
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
