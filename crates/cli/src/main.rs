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
        /// Load the global config from this file instead of the default path.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Rebuild the index. `--full` wipes it first, the corruption-recovery path.
    Reindex {
        /// Wipe the index (rebuilding the file if it will not open) then resync.
        #[arg(long)]
        full: bool,
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
        Some(Command::Sync { domain, config }) => on_runtime(cmd::sync(
            domain.as_deref(),
            config.as_deref(),
            cli.db.as_deref(),
            cli.json,
        )),
        Some(Command::Reindex { full, config }) => on_runtime(cmd::reindex(
            full,
            config.as_deref(),
            cli.db.as_deref(),
            cli.json,
        )),
        Some(Command::Status { config }) => {
            on_runtime(cmd::status(config.as_deref(), cli.db.as_deref(), cli.json))
        }
    }
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
