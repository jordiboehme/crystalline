//! M-family rules: the MANIFEST and configurable required-file structure.
//!
//! `MANIFEST.md` is checked with a hardcoded shape (`## Scope` and
//! `## When to Use`, `M001`-`M004`, `M101`-`M103`). Every entry under a
//! domain's `.crystalline.yaml` `verify.required_files` is checked with the
//! same rule ids against its own configured sections, so a domain can apply
//! the identical structural checks to any other file it wants enforced (a
//! glossary, a runbook index, and so on).

use indexmap::IndexMap;

use crate::engram::Heading;
use crate::manifest::Manifest;

use super::scanner::Domain;
use super::{Severity, Sink};

const DEFAULT_MAX_BULLET_LENGTH: usize = 180;

pub(crate) fn check(domain: &Domain, sink: &mut Sink) {
    check_manifest(domain, sink);

    if let Some(verify_cfg) = &domain.config.verify {
        for rf in &verify_cfg.required_files {
            check_required_file(domain, rf, sink);
        }
    }
}

fn check_manifest(domain: &Domain, sink: &mut Sink) {
    let manifest_path = domain.root.join("MANIFEST.md");
    let Some(idx) = domain.manifest_index else {
        sink.emit(
            &manifest_path,
            None,
            "M001",
            Severity::Error,
            "MANIFEST.md is missing",
            Some("create a MANIFEST.md at the domain root with `type: manifest`".into()),
        );
        return;
    };
    let file = &domain.files[idx];
    let Ok(engram) = &file.parsed else { return };

    if engram.frontmatter.engram_type != "manifest" {
        sink.emit(
            &file.path,
            None,
            "M002",
            Severity::Error,
            format!(
                "MANIFEST.md must have `type: manifest`, found `{}`",
                engram.frontmatter.engram_type
            ),
            None,
        );
    }

    let manifest = Manifest::from_engram(engram, &file.source);
    for missing in manifest.missing_required_sections() {
        sink.emit(
            &file.path,
            None,
            "M003",
            Severity::Error,
            format!("MANIFEST.md is missing the required `## {missing}` section"),
            None,
        );
    }

    for name in ["Scope", "When to Use"] {
        if let Some(bullets) = manifest.sections.get(&section_key(name))
            && bullets.is_empty()
        {
            sink.emit(
                &file.path,
                None,
                "M004",
                Severity::Error,
                format!("`## {name}` has no top-level bullets"),
                None,
            );
        }
    }

    if manifest.when_to_use().is_empty() && !manifest.scope().is_empty() {
        sink.emit(
            &file.path,
            None,
            "M101",
            Severity::Warning,
            "`## When to Use` is empty; routing falls back to `## Scope`",
            Some("add bullets to `## When to Use` directly".into()),
        );
    }

    for bullets in manifest.sections.values() {
        for bullet in bullets {
            if bullet.chars().count() > DEFAULT_MAX_BULLET_LENGTH {
                sink.emit(
                    &file.path,
                    None,
                    "M102",
                    Severity::Warning,
                    format!(
                        "bullet exceeds {DEFAULT_MAX_BULLET_LENGTH} characters: `{}...`",
                        truncate(bullet, 40)
                    ),
                    None,
                );
            }
        }
    }

    check_duplicate_h2(&engram.headings, &file.path, sink);
}

fn check_required_file(domain: &Domain, rf: &crate::config::RequiredFile, sink: &mut Sink) {
    let abs = domain.root.join(&rf.path);
    let found = domain
        .files
        .iter()
        .find(|f| f.rel_path == std::path::Path::new(&rf.path));
    let Some(file) = found else {
        sink.emit(
            &abs,
            None,
            "M001",
            Severity::Error,
            format!("required file `{}` is missing", rf.path),
            None,
        );
        return;
    };
    let Ok(engram) = &file.parsed else { return };

    if rf.require_frontmatter == Some(true) && !engram.has_frontmatter_fields() {
        sink.emit(
            &file.path,
            None,
            "M002",
            Severity::Error,
            format!("`{}` must carry frontmatter", rf.path),
            None,
        );
    }

    let manifest = Manifest::from_engram(engram, &file.source);
    for required in &rf.required_sections {
        if !manifest.sections.contains_key(&section_key(required)) {
            sink.emit(
                &file.path,
                None,
                "M003",
                Severity::Error,
                format!(
                    "`{}` is missing the required `## {required}` section",
                    rf.path
                ),
                None,
            );
        }
    }

    for (name, rule) in &rf.sections {
        let bullets = manifest
            .sections
            .get(&section_key(name))
            .cloned()
            .unwrap_or_default();
        let min = rule.min_top_level_bullets.unwrap_or(0);
        if bullets.len() < min {
            let fallback_ok = rule.fallback_section.as_ref().is_some_and(|fb| {
                manifest
                    .sections
                    .get(&section_key(fb))
                    .is_some_and(|b| b.len() >= min)
            });
            if fallback_ok {
                sink.emit(
                    &file.path,
                    None,
                    "M101",
                    Severity::Warning,
                    format!(
                        "`## {name}` in `{}` falls back to `## {}`",
                        rf.path,
                        rule.fallback_section.clone().unwrap_or_default()
                    ),
                    None,
                );
            } else {
                sink.emit(
                    &file.path,
                    None,
                    "M004",
                    Severity::Error,
                    format!(
                        "`## {name}` in `{}` has {} bullet(s), needs at least {min}",
                        rf.path,
                        bullets.len()
                    ),
                    None,
                );
            }
        }
        let max_len = rule.max_bullet_length.unwrap_or(DEFAULT_MAX_BULLET_LENGTH);
        for bullet in &bullets {
            if bullet.chars().count() > max_len {
                sink.emit(
                    &file.path,
                    None,
                    "M102",
                    Severity::Warning,
                    format!(
                        "bullet in `## {name}` of `{}` exceeds {max_len} characters",
                        rf.path
                    ),
                    None,
                );
            }
        }
    }

    check_duplicate_h2(&engram.headings, &file.path, sink);
}

fn check_duplicate_h2(headings: &[Heading], path: &std::path::Path, sink: &mut Sink) {
    let mut seen: IndexMap<String, usize> = IndexMap::new();
    for h in headings {
        if h.level != 2 {
            continue;
        }
        let key = section_key(&h.text);
        if let Some(&first_line) = seen.get(&key) {
            sink.emit(
                path,
                Some(h.line),
                "M103",
                Severity::Warning,
                format!(
                    "duplicate `## {}` heading (first seen at line {first_line})",
                    h.text
                ),
                None,
            );
        } else {
            seen.insert(key, h.line);
        }
    }
}

fn section_key(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}
