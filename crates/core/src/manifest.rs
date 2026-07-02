//! The Manifest model.
//!
//! A MANIFEST Engram sits at a domain root and drives routing. It must carry an
//! H2 `Scope` and an H2 `When to Use` section (matched case-insensitively after
//! trimming whitespace). Each section contributes its zero-indent bullets;
//! `When to Use` bullets are the routing input, falling back to `Scope`.

use indexmap::IndexMap;

use crate::engram::Engram;
use crate::parse::{body_lines, locate, parse_heading};

const SCOPE: &str = "scope";
const WHEN_TO_USE: &str = "when to use";

/// A parsed Manifest: the H2 sections and their top-level bullets.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Canonical H2 section key (lowercased, whitespace-collapsed) to its
    /// zero-indent bullets, in document order. The first of any duplicate H2
    /// wins.
    pub sections: IndexMap<String, Vec<String>>,
}

impl Manifest {
    /// Build a Manifest from an Engram and its source text.
    pub fn from_engram(engram: &Engram, source: &str) -> Manifest {
        let (_, _, body_start) = locate(source);
        let body = &source[body_start..];
        let body_line_start = source[..body_start].bytes().filter(|b| *b == b'\n').count() + 1;
        let _ = &engram.frontmatter; // frontmatter validation belongs to verify.

        let mut sections: IndexMap<String, Vec<String>> = IndexMap::new();
        let mut current: Option<String> = None;

        for bl in body_lines(body, body_line_start) {
            if bl.in_fence {
                continue;
            }
            if let Some((level, text)) = parse_heading(bl.text) {
                if level == 2 {
                    let key = section_key(&text);
                    if sections.contains_key(&key) {
                        // First duplicate H2 wins: ignore the later section.
                        current = None;
                    } else {
                        sections.insert(key.clone(), Vec::new());
                        current = Some(key);
                    }
                } else if level < 2 {
                    // A new top-level section ends bullet collection.
                    current = None;
                }
                continue;
            }
            if let Some(key) = &current
                && let Some(bullet) = zero_indent_bullet(bl.text)
                && let Some(list) = sections.get_mut(key)
            {
                list.push(bullet.to_string());
            }
        }

        Manifest { sections }
    }

    /// The `Scope` bullets, if the section is present.
    pub fn scope(&self) -> &[String] {
        self.sections
            .get(SCOPE)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The `When to Use` bullets, if the section is present.
    pub fn when_to_use(&self) -> &[String] {
        self.sections
            .get(WHEN_TO_USE)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The routing bullets: `When to Use` bullets, falling back to `Scope` when
    /// `When to Use` is absent or empty.
    pub fn routing_bullets(&self) -> &[String] {
        let wtu = self.when_to_use();
        if wtu.is_empty() { self.scope() } else { wtu }
    }

    /// Whether the required `Scope` section is present.
    pub fn has_scope(&self) -> bool {
        self.sections.contains_key(SCOPE)
    }

    /// Whether the required `When to Use` section is present.
    pub fn has_when_to_use(&self) -> bool {
        self.sections.contains_key(WHEN_TO_USE)
    }

    /// The required sections that are missing, by canonical name.
    pub fn missing_required_sections(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.has_scope() {
            missing.push("Scope");
        }
        if !self.has_when_to_use() {
            missing.push("When to Use");
        }
        missing
    }
}

fn section_key(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn zero_indent_bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ").map(str::trim)
}
