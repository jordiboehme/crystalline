//! Every verify rule, with its default severity and one sentence on what it
//! checks: what `configure` shows beside an override and validates a rule id
//! against. The guard test below scans every verify module, so a rule added
//! to an emit call without a row here fails it.

/// One verify rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyRule {
    /// The rule id, for example `E007`.
    pub id: &'static str,
    /// The severity it is emitted at by default: `error`, `warning` or `info`.
    pub default: &'static str,
    /// One sentence on what it checks.
    pub summary: &'static str,
}

const fn rule(id: &'static str, default: &'static str, summary: &'static str) -> VerifyRule {
    VerifyRule {
        id,
        default,
        summary,
    }
}

/// The verify rules, in id order.
pub const VERIFY_RULES: &[VerifyRule] = &[
    rule(
        "E001",
        "error",
        "The frontmatter is valid YAML and a mapping.",
    ),
    rule(
        "E002",
        "error",
        "The required fields title and permalink are present.",
    ),
    rule(
        "E003",
        "error",
        "The required field type is present; a type outside the recommended set is only info.",
    ),
    rule(
        "E004",
        "error",
        "The required field tags is present and not empty.",
    ),
    rule("E005", "error", "The permalink is a lowercase slug path."),
    rule(
        "E006",
        "error",
        "The file is clean UTF-8, with no byte order mark and no null byte.",
    ),
    rule("E007", "warning", "Every tag is lowercase-with-hyphens."),
    rule(
        "E008",
        "warning",
        "A permalink does not start with the domain name.",
    ),
    rule(
        "E009",
        "error",
        "No two paths in the domain differ only in case.",
    ),
    rule("E010", "error", "No frontmatter key appears twice."),
    rule("L001", "warning", "Every wikilink resolves."),
    rule("L002", "error", "No two engrams share a permalink."),
    rule("L003", "warning", "No two engrams share a title."),
    rule("L004", "warning", "An engram does not link to itself."),
    rule("L005", "warning", "Every crystalline:// URL resolves."),
    rule(
        "L006",
        "info",
        "A link names a domain outside the scan, so it cannot be checked.",
    ),
    rule("M001", "error", "The MANIFEST, or a required file, exists."),
    rule(
        "M002",
        "error",
        "The MANIFEST has frontmatter with type: manifest.",
    ),
    rule("M003", "error", "The required sections are present."),
    rule("M004", "error", "A required section has top-level bullets."),
    rule(
        "M005",
        "error",
        "A Provisioning path is a valid relative path.",
    ),
    rule("M006", "error", "generated_indexes is local or shared."),
    rule("M007", "error", "sharing is proposal or direct."),
    rule("M008", "error", "domain_name can name a domain."),
    rule("M101", "warning", "When to Use is not empty."),
    rule("M102", "warning", "Section bullets stay short."),
    rule("M103", "warning", "No section heading appears twice."),
    rule(
        "M104",
        "warning",
        "A Provisioning bullet names a known kind.",
    ),
    rule("M105", "warning", "A declared Provisioning folder exists."),
    rule(
        "M106",
        "warning",
        "A Provisioning bullet parses and is not a duplicate.",
    ),
    rule("M107", "warning", "The Tag Aliases bullets are sound."),
    rule(
        "M108",
        "warning",
        "The .crystalline.yaml parses and every severity in it is a word.",
    ),
    rule(
        "Q001",
        "error",
        "An engram has content beyond its frontmatter.",
    ),
    rule("Q002", "error", "An engram stays within its token budget."),
    rule("Q003", "warning", "No heading appears twice."),
    rule("Q004", "warning", "No two sections are near duplicates."),
    rule(
        "Q005",
        "info",
        "A bullet almost matches the observation or relation grammar.",
    ),
    rule("S001", "error", "A schema names its entity."),
    rule("S002", "error", "A schema declares fields."),
    rule("S003", "error", "A field declaration is well formed."),
    rule(
        "S004",
        "error",
        "settings.validation is warn, strict or off.",
    ),
    rule(
        "S010",
        "warning",
        "A schema reference matches a schema in the domain.",
    ),
    rule(
        "S020",
        "warning",
        "A required relation is present; a strict schema makes it an error.",
    ),
    rule(
        "S021",
        "warning",
        "A required observation is present; a strict schema makes it an error.",
    ),
    rule(
        "S022",
        "warning",
        "An observation value is one the schema allows; a strict schema makes it an error.",
    ),
    rule(
        "S023",
        "warning",
        "An observation value has the schema's type; a strict schema makes it an error.",
    ),
    rule(
        "S030",
        "warning",
        "A required frontmatter field is present; a strict schema makes it an error.",
    ),
    rule(
        "S031",
        "warning",
        "A frontmatter value is one the schema allows; a strict schema makes it an error.",
    ),
    rule(
        "S032",
        "warning",
        "A frontmatter field the schema lists as an array is one; a strict schema makes it an error.",
    ),
    rule(
        "S033",
        "warning",
        "A frontmatter value has the schema's type; a strict schema makes it an error.",
    ),
    rule(
        "T001",
        "error",
        "The required fields status and recorded_at are present.",
    ),
    rule("T002", "info", "The status is in the recommended set."),
    rule("T003", "error", "The date and provenance fields parse."),
    rule("T004", "error", "valid_from is not after valid_to."),
    rule(
        "T005",
        "warning",
        "A superseded engram names its successor.",
    ),
    rule(
        "T006",
        "warning",
        "The write provenance (generated) is present.",
    ),
    rule(
        "T007",
        "warning",
        "temporal_confidence is explicit or inferred.",
    ),
    rule(
        "T008",
        "warning",
        "valid_to is not a far-future sentinel date.",
    ),
];

/// The rule with exactly this id. Verify matches override keys exactly, so
/// `e007` is no rule.
pub fn verify_rule(id: &str) -> Option<&'static VerifyRule> {
    VERIFY_RULES.iter().find(|rule| rule.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every verify source file, read at compile time, so a rule id written
    /// into an emit call is checked against the catalog.
    const SOURCES: &[(&str, &str)] = &[
        ("format.rs", include_str!("format.rs")),
        ("links.rs", include_str!("links.rs")),
        ("manifest_rules.rs", include_str!("manifest_rules.rs")),
        ("mod.rs", include_str!("mod.rs")),
        ("quality.rs", include_str!("quality.rs")),
        ("schema_rules.rs", include_str!("schema_rules.rs")),
        ("temporal.rs", include_str!("temporal.rs")),
    ];

    /// Every `"X000"` literal in `text` whose letter is a verify family.
    fn rule_literals(text: &str) -> Vec<String> {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        for i in 0..bytes.len().saturating_sub(5) {
            let window = &bytes[i..i + 6];
            if window[0] == b'"'
                && b"ETMLSQ".contains(&window[1])
                && window[2..5].iter().all(u8::is_ascii_digit)
                && window[5] == b'"'
            {
                out.push(String::from_utf8_lossy(&window[1..5]).into_owned());
            }
        }
        out
    }

    #[test]
    fn every_rule_a_verify_module_emits_is_in_the_catalog() {
        for (file, text) in SOURCES {
            for id in rule_literals(text) {
                assert!(
                    verify_rule(&id).is_some(),
                    "{file} names {id}, which VERIFY_RULES does not list"
                );
            }
        }
    }

    #[test]
    fn the_catalog_is_in_id_order_with_known_defaults() {
        let ids: Vec<&str> = VERIFY_RULES.iter().map(|r| r.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted, "id order, each id once");
        assert_eq!(ids.len(), 58);
        for rule in VERIFY_RULES {
            assert!(
                ["error", "warning", "info"].contains(&rule.default),
                "{} has default {}",
                rule.id,
                rule.default
            );
            assert!(rule.summary.ends_with('.'), "{}", rule.id);
        }
        assert_eq!(verify_rule("E007").unwrap().default, "warning");
        assert_eq!(verify_rule("M108").unwrap().default, "warning");
        assert!(verify_rule("e007").is_none(), "verify matches ids exactly");
        assert!(
            verify_rule("V105").is_none(),
            "evolve rules are not verify rules"
        );
    }

    #[test]
    fn the_severity_words_are_the_ones_verify_reads() {
        for word in [
            "off", "error", "e", "warning", "warn", "w", "info", "i", " OFF ",
        ] {
            assert!(crate::verify::is_severity_word(word), "{word}");
        }
        for word in ["of", "fatal", "", "1"] {
            assert!(!crate::verify::is_severity_word(word), "{word}");
        }
    }
}
