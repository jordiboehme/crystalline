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

/// Local-first knowledge management for humans and AI agents.
#[derive(Parser, Debug)]
#[command(name = "crystalline", version, about, long_about = None)]
struct Cli {
    /// Emit machine-readable JSON instead of human-readable text. Shorthand
    /// for `--format json` on subcommands that support it.
    #[arg(long, global = true)]
    json: bool,

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
