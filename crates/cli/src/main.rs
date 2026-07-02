//! `crystalline`: the command-line entry point. Wires the crate stack
//! (core, index, service) into subcommands. No subcommands are defined
//! yet in this milestone.

use clap::{CommandFactory, Parser};

/// Local-first knowledge management for humans and AI agents.
#[derive(Parser, Debug)]
#[command(name = "crystalline", version, about, long_about = None)]
struct Cli {
    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    json: bool,
}

fn main() -> anyhow::Result<()> {
    // No subcommands exist yet, so a bare invocation (no arguments at all)
    // falls through to this hidden default: print help rather than doing
    // nothing silently.
    if std::env::args_os().len() <= 1 {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    }

    let cli = Cli::parse();
    let _ = cli.json;
    Ok(())
}
