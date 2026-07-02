//! Rendering a [`VerifyReport`] as human, JSON or GitHub Actions output.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use serde::Serialize;

use super::{Issue, Severity, VerifyReport};

/// Output format for `crystalline verify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// Grouped by file; ANSI-colored when the caller says the destination
    /// is a terminal.
    #[default]
    Human,
    /// The stable JSON schema (`version: 1`).
    Json,
    /// GitHub Actions workflow commands (`::error`/`::warning`/`::notice`).
    Github,
}

/// The stable JSON report shape (`version: 1`).
#[derive(Debug, Clone, Serialize)]
struct JsonReport<'a> {
    version: u32,
    summary: &'a super::Summary,
    issues: &'a [Issue],
}

/// Render a report in the given format. `color` only affects [`Format::Human`]
/// and should reflect whether the destination is a terminal.
pub fn render(format: Format, report: &VerifyReport, color: bool) -> String {
    match format {
        Format::Human => to_human(report, color),
        Format::Json => to_json(report),
        Format::Github => to_github(report),
    }
}

/// Render a report as JSON: `{version, summary, issues}`.
pub fn to_json(report: &VerifyReport) -> String {
    let wrapped = JsonReport {
        version: 1,
        summary: &report.summary,
        issues: &report.issues,
    };
    serde_json::to_string_pretty(&wrapped).unwrap_or_default()
}

/// Render a report for a human, grouped by file and sorted by line.
pub fn to_human(report: &VerifyReport, color: bool) -> String {
    let mut out = String::new();
    let mut by_file: BTreeMap<PathBuf, Vec<&Issue>> = BTreeMap::new();
    for issue in &report.issues {
        by_file.entry(issue.path.clone()).or_default().push(issue);
    }

    for (path, mut issues) in by_file {
        issues.sort_by_key(|i| (i.line.unwrap_or(0), i.rule));
        let _ = writeln!(out, "{}", path.display());
        for issue in issues {
            let tag = issue.severity.tag();
            let tag = if color {
                colorize(issue.severity, tag)
            } else {
                tag.to_string()
            };
            let loc = issue.line.map(|l| format!(":{l}")).unwrap_or_default();
            let _ = writeln!(out, "  [{tag}] {}{loc} {}", issue.rule, issue.message);
            if let Some(fix) = &issue.fix {
                let _ = writeln!(out, "      fix: {fix}");
            }
        }
        out.push('\n');
    }

    let s = &report.summary;
    let _ = writeln!(
        out,
        "{} domain(s), {} file(s) scanned: {} error(s), {} warning(s), {} info",
        s.domains, s.files_scanned, s.errors, s.warnings, s.infos
    );
    out
}

fn colorize(sev: Severity, tag: &str) -> String {
    let code = match sev {
        Severity::Error => "31",
        Severity::Warning => "33",
        Severity::Info => "36",
    };
    format!("\u{1b}[{code}m{tag}\u{1b}[0m")
}

/// Render a report as GitHub Actions workflow commands. Errors and warnings
/// become `::error`/`::warning`; info-level findings become `::notice` so
/// every issue still surfaces as an annotation without affecting the job's
/// pass/fail state.
pub fn to_github(report: &VerifyReport) -> String {
    let mut out = String::new();
    for issue in &report.issues {
        let cmd = match issue.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "notice",
        };
        let mut props = format!("file={}", issue.path.display());
        if let Some(line) = issue.line {
            let _ = write!(props, ",line={line}");
        }
        let _ = write!(props, ",title={}", issue.rule);
        let _ = writeln!(out, "::{cmd} {props}::{}", escape(&issue.message));
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}
