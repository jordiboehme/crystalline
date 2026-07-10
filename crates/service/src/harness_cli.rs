//! Shelling out to a coding harness's own CLI (`claude`, `codex` or
//! `copilot`) to register or deregister the Crystalline MCP server. Used by
//! the cli crate's `install`/`uninstall` commands and, in a later milestone,
//! by the daemon's own artifact provisioning.

use std::process::{Command, Stdio};

use crystalline_core::HarnessKind;

/// The outcome of running a harness CLI once.
pub enum CliRun {
    /// The command exited zero.
    Ok,
    /// The command ran but exited non-zero.
    Failed,
    /// The command could not be spawned because the binary is not on PATH.
    NotFound,
}

/// Run a harness CLI, discarding its stdout and stderr so nothing leaks into
/// this command's own output, and never surfacing an error: a spawn failure
/// for a missing binary is reported as [`CliRun::NotFound`], any other spawn
/// or wait failure as [`CliRun::Failed`].
fn run_cli(program: &str, args: &[&str]) -> CliRun {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => CliRun::Ok,
        Ok(_) => CliRun::Failed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CliRun::NotFound,
        Err(_) => CliRun::Failed,
    }
}

/// Run a harness CLI trying each of its candidate invocations in order: a
/// missing binary moves on to the next candidate, any other outcome is
/// final. Only when every candidate is missing does the whole run read as
/// [`CliRun::NotFound`].
pub fn run_harness_cli(harness: HarnessKind, args: &[&str]) -> CliRun {
    for candidate in harness.cli_invocations() {
        let (program, prefix) = candidate
            .split_first()
            .expect("a candidate invocation always names a program");
        let full: Vec<&str> = prefix.iter().chain(args.iter()).copied().collect();
        match run_cli(program, &full) {
            CliRun::NotFound => continue,
            outcome => return outcome,
        }
    }
    CliRun::NotFound
}
