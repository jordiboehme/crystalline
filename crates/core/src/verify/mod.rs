//! Static verification: the rule engine behind `crystalline verify`.
//!
//! Runs with no database, no service, no network and no async runtime.
//! Every path given directly is treated as one Domain root (a `MANIFEST.md`
//! is expected at that root; its absence is rule `M001`, not a scan
//! failure). All `.md` files found recursively under a root (skipping
//! dotfiles and dot-directories) are parsed and checked against the full
//! rule catalog: `E` (format), `T` (temporal), `M` (manifest and
//! configurable required-file structure), `L` (links), `S` (schema
//! conformance) and `Q` (quality).
//!
//! Severities are `Error`, `Warning` and `Info`. A domain's
//! `.crystalline.yaml` can override a rule's severity (including turning it
//! off); `--strict` additionally promotes every rule whose default severity
//! is `Warning` to `Error`.

mod format;
mod links;
mod manifest_rules;
mod quality;
mod report;
mod scanner;
mod schema_rules;
mod severity;
mod temporal;
mod util;

use std::path::Path;

use serde::Serialize;

pub use report::{Format, render, to_github, to_human, to_json};
pub use scanner::ScanError;

/// The severity of a verify [`Issue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Blocks CI: the exit code is 1 when any issue is at this level.
    Error,
    /// Reported but does not block CI on its own.
    Warning,
    /// Informational only.
    Info,
}

impl Severity {
    /// The single-letter tag used in human output (`E`, `W`, `I`).
    pub fn tag(self) -> &'static str {
        match self {
            Severity::Error => "E",
            Severity::Warning => "W",
            Severity::Info => "I",
        }
    }
}

/// One verify finding.
#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    /// The file the issue was found in.
    pub path: std::path::PathBuf,
    /// The one-based source line, when the issue points at a specific line.
    /// Always present in the JSON schema, `null` when there is none, so the
    /// issue object shape never varies.
    pub line: Option<usize>,
    /// The rule id, for example `E002` or `Q004`.
    pub rule: &'static str,
    /// The resolved severity, after config overrides and `--strict`.
    pub severity: Severity,
    /// A human-readable message.
    pub message: String,
    /// A suggested fix, when one is available. Always present in the JSON
    /// schema, `null` when there is none.
    pub fix: Option<String>,
}

/// Aggregate counts for a [`VerifyReport`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    /// Number of `Error` issues.
    pub errors: usize,
    /// Number of `Warning` issues.
    pub warnings: usize,
    /// Number of `Info` issues.
    pub infos: usize,
    /// Number of markdown files scanned.
    pub files_scanned: usize,
    /// Number of Domain roots scanned.
    pub domains: usize,
}

/// The result of a verify run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct VerifyReport {
    /// Aggregate counts.
    pub summary: Summary,
    /// Every issue found, in no particular order.
    pub issues: Vec<Issue>,
}

impl VerifyReport {
    /// Exit code semantics: 0 when no errors are present, 1 otherwise. A
    /// scan failure (bad path, IO error) is a separate [`ScanError`], not
    /// represented here, and maps to exit code 2 at the CLI layer.
    pub fn exit_code(&self) -> i32 {
        if self.summary.errors > 0 { 1 } else { 0 }
    }
}

/// Options controlling a verify run.
#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    /// Promote every rule whose default severity is `Warning` to `Error`.
    pub strict: bool,
    /// When set, applied to every scanned domain instead of discovering
    /// each domain's own `.crystalline.yaml`.
    pub config_override: Option<crate::config::DomainConfig>,
}

/// Verify one or more paths. Each path given directly is treated as a
/// single Domain root; nested directories under it are scanned for markdown
/// files but are never themselves treated as separate Domain roots.
pub fn verify_paths<I, P>(paths: I, options: &VerifyOptions) -> Result<VerifyReport, ScanError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let domains = scanner::scan(paths, options)?;
    Ok(run_rules(&domains, options))
}

/// Routes resolved issues into the shared issue list and summary tally,
/// applying config overrides and `--strict` promotion at the point of
/// emission so every rule module has a single, consistent way to report a
/// finding.
pub(crate) struct Sink<'a> {
    issues: &'a mut Vec<Issue>,
    summary: &'a mut Summary,
    verify_cfg: Option<&'a crate::config::VerifyConfig>,
    strict: bool,
}

impl<'a> Sink<'a> {
    fn new(
        issues: &'a mut Vec<Issue>,
        summary: &'a mut Summary,
        verify_cfg: Option<&'a crate::config::VerifyConfig>,
        strict: bool,
    ) -> Sink<'a> {
        Sink {
            issues,
            summary,
            verify_cfg,
            strict,
        }
    }

    /// Emit a finding at its rule's default severity. Resolves the
    /// effective severity (config override, then `--strict` promotion) and
    /// drops the issue entirely when the rule is configured `off`.
    pub(crate) fn emit(
        &mut self,
        path: &Path,
        line: Option<usize>,
        rule: &'static str,
        default_severity: Severity,
        message: impl Into<String>,
        fix: Option<String>,
    ) {
        let Some(severity) =
            severity::resolve(rule, default_severity, self.verify_cfg, self.strict)
        else {
            return;
        };
        match severity {
            Severity::Error => self.summary.errors += 1,
            Severity::Warning => self.summary.warnings += 1,
            Severity::Info => self.summary.infos += 1,
        }
        self.issues.push(Issue {
            path: path.to_path_buf(),
            line,
            rule,
            severity,
            message: message.into(),
            fix,
        });
    }
}

fn run_rules(domains: &[scanner::Domain], options: &VerifyOptions) -> VerifyReport {
    let domain_names: std::collections::BTreeSet<&str> =
        domains.iter().map(|d| d.name.as_str()).collect();
    let lookup = links::build_lookup(domains);

    let mut issues = Vec::new();
    let mut summary = Summary {
        files_scanned: domains.iter().map(|d| d.files.len()).sum(),
        domains: domains.len(),
        ..Default::default()
    };

    for domain in domains {
        let mut sink = Sink::new(
            &mut issues,
            &mut summary,
            domain.config.verify.as_ref(),
            options.strict,
        );
        manifest_rules::check(domain, &mut sink);
        schema_rules::check(domain, &mut sink);
        links::check(domain, &domain_names, &lookup, &mut sink);
        for file in &domain.files {
            format::check(file, &mut sink);
            temporal::check(file, &mut sink);
            quality::check(file, domain, &mut sink);
        }
    }

    VerifyReport { summary, issues }
}
