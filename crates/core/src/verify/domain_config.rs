//! Loading a domain's `.crystalline.yaml` for verify, together with what went
//! wrong reading it. The one loader `crystalline verify` and the engine's
//! `validate_engrams` share, so both report the same problems as rule `M108`
//! instead of one of them quietly running every rule at its default.
//!
//! A problem never fails a run on its own: `M108` is a warning, so only
//! `--strict` (the explicit "fail on anything" switch) turns a lost override
//! into a red build.

use std::path::Path;

use crate::config::DomainConfig;

use super::severity;

/// The per-domain config file, at the domain root.
pub const DOMAIN_CONFIG_FILE: &str = ".crystalline.yaml";

/// One thing wrong with a domain's `.crystalline.yaml`, as the `M108` message
/// says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProblem {
    /// The finding's message: the file, what is wrong and what applies instead.
    pub message: String,
    /// What fixes it: the severity-word hint for an unknown word, or a
    /// YAML-syntax hint for a file that does not parse. `M108` always carries
    /// one; the two problem kinds just carry different ones, since a
    /// finding about broken YAML pointing at severity words is nonsense.
    pub fix: Option<String>,
}

/// A domain's config and the problems met loading it. A missing file is the
/// default config and no problem; a file that does not parse is the default
/// config and one problem.
#[derive(Debug, Clone, Default)]
pub struct DomainConfigLoad {
    /// The config to apply.
    pub config: DomainConfig,
    /// What went wrong reading it, one entry per `M108` finding.
    pub problems: Vec<ConfigProblem>,
}

/// Load `<root>/.crystalline.yaml`.
pub fn load_domain_config(root: &Path) -> DomainConfigLoad {
    let path = root.join(DOMAIN_CONFIG_FILE);
    if !path.is_file() {
        return DomainConfigLoad::default();
    }
    // A file of blank lines, comments and document markers sets nothing. YAML
    // reads it as no document at all, and that is an empty file, not one that
    // does not parse.
    if std::fs::read_to_string(&path).is_ok_and(|text| is_blank_yaml(&text)) {
        return DomainConfigLoad::default();
    }
    match crate::config::load_yaml::<DomainConfig>(&path) {
        Ok(config) => {
            let problems = config_problems(&config);
            DomainConfigLoad { config, problems }
        }
        Err(e) => DomainConfigLoad {
            config: DomainConfig::default(),
            problems: vec![ConfigProblem {
                message: format!(
                    "`{DOMAIN_CONFIG_FILE}` does not parse, so none of its verify settings \
                     apply and every rule runs at its default: {e}"
                ),
                fix: Some(format!("fix the YAML syntax of {DOMAIN_CONFIG_FILE}")),
            }],
        },
    }
}

/// The problems inside a config that parsed: one per rule override whose
/// value is not a severity word, naming the rule and the word, and one per
/// override whose value is not a word at all, naming the rule.
pub fn config_problems(config: &DomainConfig) -> Vec<ConfigProblem> {
    let Some(verify) = &config.verify else {
        return Vec::new();
    };
    verify
        .rules
        .iter()
        .filter(|(_, word)| severity::parse_word(word).is_none())
        .map(|(rule, word)| ConfigProblem {
            message: format!(
                "`{DOMAIN_CONFIG_FILE}` sets {rule} to '{word}', which is not a severity \
                 (off, error, warning or info), so {rule} keeps its default"
            ),
            fix: Some("use off, error, warning or info for each rule".to_string()),
        })
        .chain(verify.not_words.iter().map(|rule| ConfigProblem {
            message: format!("{rule}: the value must be a word such as off, warning or error"),
            fix: Some("use off, error, warning or info for each rule".to_string()),
        }))
        .collect()
}

/// One change `configure` makes to a domain's `.crystalline.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigEdit {
    /// `verify.rules.<rule>: <word>`.
    SetRule {
        /// The rule id, as verify matches it.
        rule: String,
        /// The severity word.
        word: String,
    },
    /// Remove `verify.rules.<rule>`.
    UnsetRule {
        /// The rule id.
        rule: String,
    },
    /// `verify.token_budget: <n>`.
    SetTokenBudget(usize),
    /// Remove `verify.token_budget`.
    UnsetTokenBudget,
}

/// What an edit leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditedConfig {
    /// Whether the edits change any key. `false` means the file is to be left
    /// exactly as it is, comments and all.
    pub changed: bool,
    /// The new file text, or `None` when no key is left and the file goes.
    pub text: Option<String>,
    /// Whether the old text held a YAML comment, which the edit does not keep.
    pub dropped_comments: bool,
}

/// `current` (the file's text, `None` when there is no file) with `edits`
/// applied through a YAML value, so every other key in the file is kept, in
/// its place. Comments are not kept. A file that does not parse, or whose top
/// level or `verify` or `verify.rules` is not a mapping, is refused with the
/// reason and nothing is changed.
pub fn edit_domain_config(
    current: Option<&str>,
    edits: &[ConfigEdit],
) -> Result<EditedConfig, String> {
    use serde_yaml_ng::{Mapping, Value};

    let mut root = match current {
        None => Mapping::new(),
        // YAML reads a file of blank lines and comments as no document at
        // all, which is an empty file rather than a broken one.
        Some(text) if is_blank_yaml(text) => Mapping::new(),
        Some(text) => match serde_yaml_ng::from_str::<Value>(text) {
            Ok(Value::Mapping(map)) => map,
            Ok(Value::Null) => Mapping::new(),
            Ok(_) => {
                return Err(format!(
                    "`{DOMAIN_CONFIG_FILE}` is not a mapping of keys, so it was not changed"
                ));
            }
            Err(e) => {
                return Err(format!(
                    "`{DOMAIN_CONFIG_FILE}` does not parse, so it was not changed: {e}"
                ));
            }
        },
    };
    let before = root.clone();
    let mut verify = match root.get("verify") {
        None | Some(Value::Null) => Mapping::new(),
        Some(Value::Mapping(map)) => map.clone(),
        Some(_) => {
            return Err(format!(
                "`verify` in `{DOMAIN_CONFIG_FILE}` is not a mapping, so the file was not changed"
            ));
        }
    };
    let mut rules = match verify.get("rules") {
        None | Some(Value::Null) => Mapping::new(),
        Some(Value::Mapping(map)) => map.clone(),
        Some(_) => {
            return Err(format!(
                "`verify.rules` in `{DOMAIN_CONFIG_FILE}` is not a mapping, so the file was not changed"
            ));
        }
    };
    let mut budget_edits = Vec::new();
    for edit in edits {
        match edit {
            ConfigEdit::SetRule { rule, word } => {
                remove_other_spellings(&mut rules, rule);
                rules.insert(Value::from(rule.as_str()), Value::from(word.as_str()));
            }
            ConfigEdit::UnsetRule { rule } => {
                remove_other_spellings(&mut rules, rule);
                rules.shift_remove(rule.as_str());
            }
            ConfigEdit::SetTokenBudget(_) | ConfigEdit::UnsetTokenBudget => {
                budget_edits.push(edit);
            }
        }
    }
    // `rules` goes back into `verify` before the token budget is set, so a
    // `verify` made from nothing reads rules first, then token_budget. An
    // insert of a key that is already there keeps its place, so a rewritten
    // `rules` or `verify` stays where the file had it.
    if rules.is_empty() {
        verify.shift_remove("rules");
    } else {
        verify.insert(Value::from("rules"), Value::Mapping(rules));
    }
    for edit in budget_edits {
        match edit {
            ConfigEdit::SetTokenBudget(budget) => {
                verify.insert(Value::from("token_budget"), Value::from(*budget));
            }
            _ => {
                verify.shift_remove("token_budget");
            }
        }
    }
    if verify.is_empty() {
        root.shift_remove("verify");
    } else {
        root.insert(Value::from("verify"), Value::Mapping(verify));
    }
    let changed = root != before;
    let dropped_comments = changed && current.is_some_and(has_comment);
    if root.is_empty() {
        return Ok(EditedConfig {
            changed,
            text: None,
            dropped_comments,
        });
    }
    let text = serde_yaml_ng::to_string(&Value::Mapping(root))
        .map_err(|e| format!("`{DOMAIN_CONFIG_FILE}` could not be written: {e}"))?;
    Ok(EditedConfig {
        changed,
        text: Some(text),
        dropped_comments,
    })
}

/// Remove every key of `rules` that spells `rule` in another case, such as
/// `e008` for `E008`: verify matches ids exactly, so such a key does nothing,
/// and an edit of the rule replaces or removes it rather than leaving it
/// beside the id verify reads.
fn remove_other_spellings(rules: &mut serde_yaml_ng::Mapping, rule: &str) {
    let others: Vec<serde_yaml_ng::Value> = rules
        .keys()
        .filter(|key| {
            key.as_str()
                .is_some_and(|key| key != rule && key.eq_ignore_ascii_case(rule))
        })
        .cloned()
        .collect();
    for key in others {
        rules.shift_remove(&key);
    }
}

/// Whether a YAML text holds nothing but blank lines, comments and the
/// document markers `---` and `...` (a marker may carry a comment after it).
/// A leading `---` is how many people start a YAML file.
fn is_blank_yaml(text: &str) -> bool {
    text.lines().all(|line| {
        let trimmed = line.trim();
        let rest = trimmed
            .strip_prefix("---")
            .or_else(|| trimmed.strip_prefix("..."))
            .map_or(trimmed, str::trim_start);
        rest.is_empty() || rest.starts_with('#')
    })
}

/// Whether a YAML text holds a comment: a `#` at the start of a line or after
/// a blank. A `#` inside a quoted value can count too, which only means the
/// answer says a comment was dropped when none was.
fn has_comment(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('#') || line.contains(" #")
    })
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    fn rule(rule: &str, word: &str) -> ConfigEdit {
        ConfigEdit::SetRule {
            rule: rule.to_string(),
            word: word.to_string(),
        }
    }

    #[test]
    fn a_missing_file_is_created_with_the_override() {
        let edited = edit_domain_config(None, &[rule("E007", "off")]).unwrap();
        assert!(edited.changed);
        assert_eq!(
            edited.text.as_deref(),
            Some("verify:\n  rules:\n    E007: off\n")
        );
        assert!(!edited.dropped_comments);
    }

    /// Setting what the file already says changes nothing, so a caller
    /// leaves the file, and its comments, alone.
    #[test]
    fn setting_the_value_it_already_has_changes_nothing() {
        let current = "# tuned by hand\nverify:\n  rules:\n    T001: warning\n";
        let edited = edit_domain_config(Some(current), &[rule("T001", "warning")]).unwrap();
        assert!(!edited.changed);
        assert!(!edited.dropped_comments);
        let edited = edit_domain_config(
            Some(current),
            &[ConfigEdit::UnsetRule {
                rule: "E007".to_string(),
            }],
        )
        .unwrap();
        assert!(!edited.changed, "removing an override that is not there");
    }

    /// An id the file spells in lower case is the same rule: an edit of it
    /// replaces or removes that key, never leaves it beside the upper-case
    /// one verify reads.
    #[test]
    fn an_edit_of_a_rule_takes_its_other_spelling_with_it() {
        let current = "verify:\n  rules:\n    T001: warning\n    e008: off\n";
        let edited = edit_domain_config(Some(current), &[rule("E008", "error")]).unwrap();
        assert_eq!(
            edited.text.as_deref(),
            Some("verify:\n  rules:\n    T001: warning\n    E008: error\n")
        );
        let edited = edit_domain_config(
            Some(current),
            &[ConfigEdit::UnsetRule {
                rule: "E008".to_string(),
            }],
        )
        .unwrap();
        assert!(edited.changed);
        assert_eq!(
            edited.text.as_deref(),
            Some("verify:\n  rules:\n    T001: warning\n")
        );
    }

    #[test]
    fn every_other_key_is_kept_in_its_place() {
        let current = "verify:\n  rules:\n    T001: warning\n    e008: off\n    X999: off\n  token_budgets:\n    notes/a.md: 900\nother: kept\n";
        let edited = edit_domain_config(
            Some(current),
            &[rule("E007", "off"), ConfigEdit::SetTokenBudget(5000)],
        )
        .unwrap();
        assert_eq!(
            edited.text.as_deref(),
            Some(
                "verify:\n  rules:\n    T001: warning\n    e008: off\n    X999: off\n    E007: off\n  token_budgets:\n    notes/a.md: 900\n  token_budget: 5000\nother: kept\n"
            )
        );
    }

    #[test]
    fn a_comment_is_not_kept_and_the_edit_says_so() {
        let edited = edit_domain_config(
            Some("# tuned by hand\nverify:\n  rules:\n    T001: warning # keep\n"),
            &[rule("T001", "off")],
        )
        .unwrap();
        assert!(edited.dropped_comments);
        assert_eq!(
            edited.text.as_deref(),
            Some("verify:\n  rules:\n    T001: off\n")
        );
    }

    #[test]
    fn an_empty_or_comment_only_file_takes_the_key() {
        for current in [
            "",
            "\n",
            "# nothing here yet\n",
            "---\n",
            "---\n# nothing here yet\n",
            "--- # start\n...\n",
        ] {
            let edited = edit_domain_config(Some(current), &[rule("E007", "off")]).unwrap();
            assert_eq!(
                edited.text.as_deref(),
                Some("verify:\n  rules:\n    E007: off\n"),
                "{current:?}"
            );
        }
    }

    #[test]
    fn the_last_key_going_deletes_the_file() {
        let edited = edit_domain_config(
            Some("verify:\n  rules:\n    E007: off\n  token_budget: 5000\n"),
            &[
                ConfigEdit::UnsetRule {
                    rule: "E007".to_string(),
                },
                ConfigEdit::UnsetTokenBudget,
            ],
        )
        .unwrap();
        assert_eq!(edited.text, None);
    }

    /// Today an empty or comment-only file reads as one that does not
    /// parse (M108), because YAML sees no document in it. It sets nothing,
    /// and that is all it is.
    #[test]
    fn a_blank_or_comment_only_file_sets_nothing_and_is_no_problem() {
        for text in [
            "",
            "\n",
            "# nothing here yet\n",
            "---\n",
            "---\n# nothing here yet\n",
            "  ---  \n\n...\n",
            "--- # start\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(DOMAIN_CONFIG_FILE), text).unwrap();
            let load = load_domain_config(dir.path());
            assert!(load.problems.is_empty(), "{text:?}: {:?}", load.problems);
            assert_eq!(load.config, DomainConfig::default());
        }
    }

    #[test]
    fn a_file_that_does_not_parse_is_refused() {
        let err =
            edit_domain_config(Some("verify: [unclosed\n"), &[rule("E007", "off")]).unwrap_err();
        assert!(err.contains("does not parse"), "{err}");
        let err = edit_domain_config(Some("- a list\n"), &[rule("E007", "off")]).unwrap_err();
        assert!(err.contains("not a mapping"), "{err}");
        let err = edit_domain_config(Some("verify: 7\n"), &[rule("E007", "off")]).unwrap_err();
        assert!(err.contains("`verify`"), "{err}");
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;

    /// Task 6 relies on this exact text: rules before token_budget.
    #[test]
    fn a_missing_file_with_a_rule_and_a_budget_reads_rules_first() {
        let edited = edit_domain_config(
            None,
            &[
                ConfigEdit::SetRule {
                    rule: "E007".to_string(),
                    word: "off".to_string(),
                },
                ConfigEdit::SetTokenBudget(5000),
            ],
        )
        .unwrap();
        assert_eq!(
            edited.text.as_deref(),
            Some(
                "verify:
  rules:
    E007: off
  token_budget: 5000
"
            )
        );
    }
}
