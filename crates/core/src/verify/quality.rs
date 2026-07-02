//! Q-family rules: content quality.
//!
//! `Q004`'s similarity ratio is a bigram Dice coefficient
//! (`2 * |intersection| / (|bigrams(a)| + |bigrams(b)|)`, multiset
//! intersection), chosen over a longest-common-subsequence ratio for its
//! O(n) simplicity and stability under paragraph reordering; see
//! [`dice_coefficient`].

use std::collections::HashMap;

use crate::engram::{Engram, Heading};
use crate::parse::body_lines;

use super::scanner::{Domain, ScannedFile};
use super::util::body_line_start;
use super::{Severity, Sink};

const DEFAULT_TOKEN_BUDGET: usize = 2500;
const MIN_CONTENT_LINES: usize = 3;
const SIMILARITY_THRESHOLD: f64 = 0.85;
const SIMILARITY_MIN_CHARS: usize = 80;

pub(crate) fn check(file: &ScannedFile, domain: &Domain, sink: &mut Sink) {
    let Ok(engram) = &file.parsed else { return };

    check_content(file, engram, sink);
    check_token_budget(file, engram, domain, sink);
    check_duplicate_headings(file, engram, sink);
    check_similar_sections(file, engram, sink);
    check_near_miss_bullets(file, engram, sink);
}

/// Q001: an Engram with no meaningful content beyond its frontmatter is
/// unlikely to be worth routing to. Counts non-blank body lines outside
/// fenced code, matching the "meaningful content" convention documented for
/// this rule family.
fn check_content(file: &ScannedFile, engram: &Engram, sink: &mut Sink) {
    let meaningful = body_lines(&engram.body, 1)
        .into_iter()
        .filter(|l| !l.in_fence && !l.text.trim().is_empty())
        .count();
    if meaningful < MIN_CONTENT_LINES {
        sink.emit(
            &file.path,
            None,
            "Q001",
            Severity::Error,
            format!(
                "no meaningful content beyond frontmatter ({meaningful} non-blank line(s), need at least {MIN_CONTENT_LINES})"
            ),
            None,
        );
    }
}

/// Q002: an approximate token budget, `body.chars() / 4` (frontmatter
/// excluded, fenced code included - a wall of code is context bloat too).
fn check_token_budget(file: &ScannedFile, engram: &Engram, domain: &Domain, sink: &mut Sink) {
    let budget = resolve_budget(file, domain);
    if budget == 0 {
        return;
    }
    let tokens = engram.body.chars().count() / 4;
    if tokens > budget {
        sink.emit(
            &file.path,
            None,
            "Q002",
            Severity::Error,
            format!("body is about {tokens} tokens, over the {budget} token budget"),
            Some("trim the body or split it into more than one engram".into()),
        );
    }
}

fn resolve_budget(file: &ScannedFile, domain: &Domain) -> usize {
    if let Some(v) = &domain.config.verify {
        let rel = file.rel_path.to_string_lossy();
        if let Some(&b) = v.token_budgets.get(rel.as_ref()) {
            return b;
        }
        if let Some(b) = v.token_budget {
            return b;
        }
    }
    DEFAULT_TOKEN_BUDGET
}

/// Q003: a duplicate `(level, text)` heading pair anywhere in one file,
/// across all levels (unlike the MANIFEST-specific `M103`, which is H2
/// only).
fn check_duplicate_headings(file: &ScannedFile, engram: &Engram, sink: &mut Sink) {
    let mut seen: HashMap<(u8, String), usize> = HashMap::new();
    for h in &engram.headings {
        let key = (h.level, normalize_heading(&h.text));
        if let Some(&first) = seen.get(&key) {
            sink.emit(
                &file.path,
                Some(h.line),
                "Q003",
                Severity::Warning,
                format!(
                    "duplicate heading `{}` (first seen at line {first})",
                    h.text
                ),
                None,
            );
        } else {
            seen.insert(key, h.line);
        }
    }
}

fn normalize_heading(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Q004: two `## ` sections within the same file that are near-duplicates of
/// each other, most often a copy-paste that was not later diverged.
fn check_similar_sections(file: &ScannedFile, engram: &Engram, sink: &mut Sink) {
    let sections = h2_section_texts(engram, &file.source);
    for i in 0..sections.len() {
        for j in (i + 1)..sections.len() {
            let (_, a_text, a_norm) = &sections[i];
            let (b_line, b_text, b_norm) = &sections[j];
            if a_norm.chars().count() < SIMILARITY_MIN_CHARS
                || b_norm.chars().count() < SIMILARITY_MIN_CHARS
            {
                continue;
            }
            let sim = dice_coefficient(a_norm, b_norm);
            if sim >= SIMILARITY_THRESHOLD {
                sink.emit(
                    &file.path,
                    Some(*b_line),
                    "Q004",
                    Severity::Warning,
                    format!(
                        "`## {b_text}` is {:.0}% similar to `## {a_text}` (near-duplicate content)",
                        sim * 100.0
                    ),
                    None,
                );
            }
        }
    }
}

/// Extract every `## ` section's heading line, heading text and normalized
/// content (lowercased, non-alphanumeric collapsed to spaces, fenced code
/// excluded).
fn h2_section_texts(engram: &Engram, source: &str) -> Vec<(usize, String, String)> {
    let start = body_line_start(source);
    let lines = body_lines(&engram.body, start);
    let headings: Vec<&Heading> = engram.headings.iter().collect();

    let mut result = Vec::new();
    for (i, h) in headings.iter().enumerate() {
        if h.level != 2 {
            continue;
        }
        let end = headings[i + 1..]
            .iter()
            .find(|nh| nh.level <= 2)
            .map(|nh| nh.line)
            .unwrap_or(usize::MAX);
        let mut buf = String::new();
        for l in &lines {
            if l.line_no > h.line && l.line_no < end && !l.in_fence {
                let t = l.text.trim();
                if !t.is_empty() {
                    if !buf.is_empty() {
                        buf.push(' ');
                    }
                    buf.push_str(t);
                }
            }
        }
        result.push((h.line, h.text.clone(), normalize(&buf)));
    }
    result
}

fn normalize(s: &str) -> String {
    let filtered: String = s
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    filtered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bigram Dice coefficient: `2 * |intersection| / (|bigrams(a)| +
/// |bigrams(b)|)`, using multiset (count-aware) bigram intersection.
fn dice_coefficient(a: &str, b: &str) -> f64 {
    let ba = bigrams(a);
    let bb = bigrams(b);
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<(char, char), i32> = HashMap::new();
    for bg in &ba {
        *counts.entry(*bg).or_insert(0) += 1;
    }
    let mut intersection = 0usize;
    for bg in &bb {
        if let Some(c) = counts.get_mut(bg)
            && *c > 0
        {
            *c -= 1;
            intersection += 1;
        }
    }
    2.0 * intersection as f64 / (ba.len() + bb.len()) as f64
}

fn bigrams(s: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 2 {
        return Vec::new();
    }
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Q005: a top-level bullet that almost, but does not quite, match the
/// observation or relation grammar - most often a typo (missing space,
/// unclosed bracket) that would otherwise silently vanish from the body's
/// structured data with no warning at all.
fn check_near_miss_bullets(file: &ScannedFile, engram: &Engram, sink: &mut Sink) {
    let start = body_line_start(&file.source);
    for bl in body_lines(&engram.body, start) {
        if bl.in_fence {
            continue;
        }
        let indent = bl.text.len() - bl.text.trim_start().len();
        if indent != 0 {
            continue;
        }
        if let Some(reason) = near_miss(bl.text) {
            sink.emit(
                &file.path,
                Some(bl.line_no),
                "Q005",
                Severity::Info,
                reason,
                None,
            );
        }
    }
}

fn near_miss(line: &str) -> Option<String> {
    // `-[category]`: a dash directly followed by `[`, missing the space
    // that makes it a real bullet.
    if let Some(rest) = line.strip_prefix('-')
        && !rest.starts_with(' ')
        && rest.starts_with('[')
    {
        return Some("observation-like bullet is missing a space after `-`".to_string());
    }

    let content = line.strip_prefix("- ")?;

    // `- [category text` with no closing `]`: an unclosed observation
    // category bracket.
    if let Some(rest) = content.strip_prefix('[')
        && !rest.contains(']')
    {
        return Some("observation bullet has an unclosed `[category]` bracket".to_string());
    }

    // `- rel_type [[Target]` with no `]]`: an unclosed relation link.
    if let Some(open) = content.find("[[") {
        let after = &content[open + 2..];
        if !after.contains("]]") && !after.is_empty() {
            return Some("relation bullet has an unclosed `[[Target]]` link".to_string());
        }
    }

    None
}
