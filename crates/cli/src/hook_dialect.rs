#![allow(dead_code)] // removed by Task 8, which wires the callers
//! The session hook dialects of the profile harnesses.
//!
//! Each dialect is planned as a list of span-preserving edits
//! ([`crystalline_core::jsonc_edit`]) against the parsed file, never as a
//! re-serialized document: an install is at most one `Set` (an older
//! spelling of ours rewritten in place) or one `Push` (our entry appended
//! after the person's), and an uninstall removes exactly what an install
//! added, so the person's other hooks, comments and layout come back byte
//! for byte.
//!
//! Ownership is the profile rule from [`crate::harness_command`]: a hook is
//! ours when its program is `crystalline` in either spelling followed by
//! `prompt system`, and only a spelling install wrote is rewritten. A
//! hand-written variant counts as present and is left alone.

use crystalline_core::jsonc_edit::{Edit, JsonIn, Prune, SegBuf};
use crystalline_core::{HarnessKind, HookDialect};
use serde_json::Value;

use crate::harness_command::{
    command_line, is_own_profile_spelling, profile_command_kind, split_program,
};
use crate::install::SESSION_START_COMMAND;

/// The event key Claude-style group files use for the session-start hook.
const CLAUDE_EVENT: &str = "SessionStart";

/// The name of the one entry in Kiro's owned hook file.
const KIRO_ENTRY_NAME: &str = "crystalline-routing";

/// The session-start command for `harness` with `program` as its program:
/// `prompt system`, the profile's `--format` when it has one, and
/// `--harness <id>`. A program holding whitespace is quoted.
pub(crate) fn session_command(harness: HarnessKind, program: &str) -> String {
    let words = SESSION_START_COMMAND
        .strip_prefix("crystalline")
        .expect("the routing command starts with the program");
    let format = harness
        .profile()
        .session_format
        .flag()
        .map(|f| format!(" --format {f}"))
        .unwrap_or_default();
    command_line(
        program,
        &format!("{words}{format} --harness {}", harness.id()),
    )
}

/// The document a missing hooks file starts as, before the install edits:
/// Cursor's `{"version": 1, "hooks": {}}` and Kiro's
/// `{"version": "v1", "hooks": []}`. `None` for a dialect whose file is a
/// shared settings file the install only adds to.
pub(crate) fn new_file_skeleton(dialect: &HookDialect) -> Option<JsonIn> {
    match dialect {
        HookDialect::FlatEventMap { .. } => Some(JsonIn::Object(vec![
            ("version".to_string(), JsonIn::Int(1)),
            ("hooks".to_string(), JsonIn::Object(Vec::new())),
        ])),
        HookDialect::KiroOwned { .. } => Some(kiro_skeleton()),
        HookDialect::ClaudeGroups { .. } | HookDialect::Legacy | HookDialect::None => None,
    }
}

/// Kiro's owned hook file with no entry in it. The orchestrator deletes the
/// file when an uninstall leaves exactly this.
pub(crate) fn kiro_skeleton() -> JsonIn {
    JsonIn::Object(vec![
        ("version".to_string(), JsonIn::Str("v1".to_string())),
        ("hooks".to_string(), JsonIn::Array(Vec::new())),
    ])
}

/// The edits that bring the session hook in `root` up to `command`: one
/// `Set` on the command of the first hook of ours in a spelling install
/// wrote, when it differs; nothing when a hook of ours is already there;
/// one `Push` of our entry when there is none. A value of an unexpected
/// type on the way is foreign data and plans nothing, and so does a
/// `command` that is not one of ours.
pub(crate) fn plan_install(dialect: &HookDialect, root: &Value, command: &str) -> Vec<Edit> {
    if !profile_command_kind(command) {
        return Vec::new();
    }
    let (Some(container), Some(entry)) = (container_path(dialect), new_entry(dialect, command))
    else {
        return Vec::new();
    };
    let Some(found) = own_hooks(dialect, root) else {
        return Vec::new();
    };
    if found.is_empty() {
        return vec![Edit::Push {
            path: container,
            value: entry,
        }];
    }
    match found
        .into_iter()
        .find(|h| h.command.as_deref().is_some_and(is_own_profile_spelling))
    {
        Some(hook) if hook.command.as_deref() != Some(command) => vec![Edit::Set {
            path: hook.command_path,
            value: JsonIn::Str(command.to_string()),
        }],
        _ => Vec::new(),
    }
}

/// The edits that take every hook of ours out of `root`, deepest first so
/// each path still points where it did. A Claude-style group that holds
/// only hooks of ours goes as a whole. Containers a removal empties go too
/// (`Prune::Emptied`), except in Kiro's owned file, which keeps its
/// skeleton so the orchestrator can recognise it and delete the file.
pub(crate) fn plan_uninstall(dialect: &HookDialect, root: &Value) -> Vec<Edit> {
    let prune = match dialect {
        HookDialect::KiroOwned { .. } => Prune::Never,
        _ => Prune::Emptied,
    };
    let mut paths: Vec<Vec<SegBuf>> = own_hooks(dialect, root)
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.removal)
        .collect();
    paths.dedup();
    paths
        .into_iter()
        .rev()
        .map(|path| Edit::Remove { path, prune })
        .collect()
}

/// Whether `root` holds a session hook of ours in any spelling.
pub(crate) fn session_hook_present(dialect: &HookDialect, root: &Value) -> bool {
    own_hooks(dialect, root).is_some_and(|found| !found.is_empty())
}

/// The program of the first hook of ours in a spelling install wrote, so a
/// reconcile can keep an absolute path that still runs.
pub(crate) fn stored_program(dialect: &HookDialect, root: &Value) -> Option<String> {
    own_hooks(dialect, root)?
        .into_iter()
        .filter_map(|h| h.command)
        .find(|c| is_own_profile_spelling(c))
        .and_then(|c| split_program(&c).map(|(program, _)| program.to_string()))
}

/// One hook of ours found in a parsed file.
struct OwnHook {
    /// The stored command, when it is a string.
    command: Option<String>,
    /// The path to the command string.
    command_path: Vec<SegBuf>,
    /// What uninstall removes for this hook: the hook itself, or its whole
    /// group when every hook in the group is ours.
    removal: Vec<SegBuf>,
}

fn key(name: &str) -> SegBuf {
    SegBuf::Key(name.to_string())
}

/// The array our entry is pushed onto.
fn container_path(dialect: &HookDialect) -> Option<Vec<SegBuf>> {
    match dialect {
        HookDialect::ClaudeGroups { .. } => Some(vec![key("hooks"), key(CLAUDE_EVENT)]),
        HookDialect::FlatEventMap { event, .. } => Some(vec![key("hooks"), key(event)]),
        HookDialect::KiroOwned { .. } => Some(vec![key("hooks")]),
        HookDialect::Legacy | HookDialect::None => None,
    }
}

/// The entry an install appends for `command`.
fn new_entry(dialect: &HookDialect, command: &str) -> Option<JsonIn> {
    let s = |v: &str| JsonIn::Str(v.to_string());
    let obj = |members: Vec<(&str, JsonIn)>| {
        JsonIn::Object(
            members
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    };
    match *dialect {
        HookDialect::ClaudeGroups {
            timeout, matcher, ..
        } => Some(obj(vec![
            ("matcher", s(matcher)),
            (
                "hooks",
                JsonIn::Array(vec![obj(vec![
                    ("type", s("command")),
                    ("command", s(command)),
                    (
                        "timeout",
                        JsonIn::Int(i64::try_from(timeout).unwrap_or(i64::MAX)),
                    ),
                ])]),
            ),
        ])),
        HookDialect::FlatEventMap { .. } => Some(obj(vec![("command", s(command))])),
        HookDialect::KiroOwned { .. } => Some(obj(vec![
            ("name", s(KIRO_ENTRY_NAME)),
            ("trigger", s("SessionStart")),
            (
                "action",
                obj(vec![("type", s("command")), ("command", s(command))]),
            ),
        ])),
        HookDialect::Legacy | HookDialect::None => None,
    }
}

fn ours(command: Option<&str>) -> bool {
    command.is_some_and(profile_command_kind)
}

/// Every hook of ours in `root`, in document order. `None` when a value on
/// the way to the container has an unexpected type, which is foreign data
/// the install leaves alone; a missing container is an empty list.
fn own_hooks(dialect: &HookDialect, root: &Value) -> Option<Vec<OwnHook>> {
    let container = container_path(dialect)?;
    let mut node = root;
    for (depth, seg) in container.iter().enumerate() {
        let SegBuf::Key(k) = seg else { return None };
        match node.as_object()?.get(k) {
            Some(next) => node = next,
            None => return Some(Vec::new()),
        }
        if depth + 1 < container.len() && !node.is_object() {
            return None;
        }
    }
    let items = node.as_array()?;
    let at = |rest: &[SegBuf]| {
        let mut path = container.clone();
        path.extend_from_slice(rest);
        path
    };
    let command_of = |v: &Value| v.get("command").and_then(Value::as_str).map(str::to_string);
    let mut found = Vec::new();
    match dialect {
        HookDialect::ClaudeGroups { .. } => {
            for (gi, group) in items.iter().enumerate() {
                let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
                    continue;
                };
                let whole = hooks
                    .iter()
                    .all(|h| ours(h.get("command").and_then(Value::as_str)));
                for (hi, hook) in hooks.iter().enumerate() {
                    let command = command_of(hook);
                    if !ours(command.as_deref()) {
                        continue;
                    }
                    let hook_path = at(&[SegBuf::Index(gi), key("hooks"), SegBuf::Index(hi)]);
                    let mut command_path = hook_path.clone();
                    command_path.push(key("command"));
                    found.push(OwnHook {
                        command,
                        command_path,
                        removal: if whole {
                            at(&[SegBuf::Index(gi)])
                        } else {
                            hook_path
                        },
                    });
                }
            }
        }
        HookDialect::FlatEventMap { .. } => {
            for (i, entry) in items.iter().enumerate() {
                let command = command_of(entry);
                if ours(command.as_deref()) {
                    found.push(OwnHook {
                        command,
                        command_path: at(&[SegBuf::Index(i), key("command")]),
                        removal: at(&[SegBuf::Index(i)]),
                    });
                }
            }
        }
        HookDialect::KiroOwned { .. } => {
            for (i, entry) in items.iter().enumerate() {
                let command = entry.get("action").and_then(command_of);
                let named = entry.get("name").and_then(Value::as_str) == Some(KIRO_ENTRY_NAME);
                if named || ours(command.as_deref()) {
                    found.push(OwnHook {
                        command,
                        command_path: at(&[SegBuf::Index(i), key("action"), key("command")]),
                        removal: at(&[SegBuf::Index(i)]),
                    });
                }
            }
        }
        HookDialect::Legacy | HookDialect::None => return None,
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crystalline_core::jsonc_edit::{apply_edits, parse_value};

    const GEMINI: &str = include_str!("../../core/tests/fixtures/harness/gemini-settings.json");
    const CURSOR: &str = include_str!("../../core/tests/fixtures/harness/cursor-hooks.json");
    const CURSOR_COMMENTED: &str =
        include_str!("../../core/tests/fixtures/harness/cursor-hooks-commented.jsonc");

    fn run(dialect: &HookDialect, text: Option<&str>, command: &str) -> String {
        let root = text
            .map(|t| parse_value(t).unwrap())
            .unwrap_or_else(|| serde_json::json!({}));
        let mut edits = Vec::new();
        if text.is_none()
            && let Some(skeleton) = new_file_skeleton(dialect)
        {
            edits.push(Edit::Set {
                path: vec![],
                value: skeleton,
            });
        }
        edits.extend(plan_install(dialect, &root, command));
        apply_edits(text, &edits).unwrap().expect("a change")
    }

    fn undo(dialect: &HookDialect, text: &str) -> String {
        apply_edits(
            Some(text),
            &plan_uninstall(dialect, &parse_value(text).unwrap()),
        )
        .unwrap()
        .expect("a change")
    }

    /// Review focus 2.
    #[test]
    fn the_gemini_group_is_one_array_element_beside_the_foreign_one() {
        let d = &HarnessKind::Gemini.profile().hooks;
        let cmd = session_command(HarnessKind::Gemini, "crystalline");
        assert_eq!(
            cmd,
            "crystalline prompt system --format hook-specific --harness gemini"
        );
        let with = run(d, Some(GEMINI), &cmd);
        let v = parse_value(&with).unwrap();
        let groups = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[1],
            serde_json::json!({"matcher": "startup", "hooks": [{"type": "command", "command": cmd, "timeout": 10000}]})
        );
        assert!(with.contains("/Users/someone/.othertool/hooks/gemini-hook.sh"));
        let root = parse_value(&with).unwrap();
        assert!(
            plan_install(d, &root, &cmd).is_empty(),
            "a second install plans nothing"
        );
        assert!(session_hook_present(d, &root));
        assert_eq!(undo(d, &with), GEMINI, "byte for byte");
    }

    #[test]
    fn the_cursor_entry_sits_in_the_shared_flat_map_and_leaves_it_as_found() {
        let d = &HarnessKind::Cursor.profile().hooks;
        let cmd = session_command(HarnessKind::Cursor, "/opt/homebrew/bin/crystalline");
        assert_eq!(
            cmd,
            "/opt/homebrew/bin/crystalline prompt system --format cursor --harness cursor"
        );
        for original in [CURSOR, CURSOR_COMMENTED] {
            let with = run(d, Some(original), &cmd);
            let v = parse_value(&with).unwrap();
            assert_eq!(
                v["hooks"]["sessionStart"][1],
                serde_json::json!({"command": cmd})
            );
            assert_eq!(v["version"], 1);
            assert_eq!(undo(d, &with), original);
        }
        let fresh = run(d, None, &cmd);
        assert_eq!(
            parse_value(&fresh).unwrap(),
            serde_json::json!({"version": 1, "hooks": {"sessionStart": [{"command": cmd}]}})
        );
    }

    #[test]
    fn an_older_spelling_of_ours_is_rewritten_in_place_and_a_hand_written_one_is_left() {
        let d = &HarnessKind::Cursor.profile().hooks;
        let old = "{\"version\": 1, \"hooks\": {\"sessionStart\": [{\"command\": \"crystalline prompt system --harness cursor\"}]}}";
        let cmd = session_command(HarnessKind::Cursor, "/opt/homebrew/bin/crystalline");
        let edits = plan_install(d, &parse_value(old).unwrap(), &cmd);
        assert_eq!(edits.len(), 1);
        assert!(matches!(&edits[0], Edit::Set { .. }));
        let hand = "{\"version\": 1, \"hooks\": {\"sessionStart\": [{\"command\": \"crystalline prompt system --workspace /repo\"}]}}";
        assert!(plan_install(d, &parse_value(hand).unwrap(), &cmd).is_empty());
    }

    #[test]
    fn the_kiro_file_is_ours_and_empties_to_its_skeleton() {
        let d = &HarnessKind::Kiro.profile().hooks;
        let cmd = session_command(HarnessKind::Kiro, "/opt/homebrew/bin/crystalline");
        let fresh = run(d, None, &cmd);
        assert_eq!(
            parse_value(&fresh).unwrap(),
            serde_json::json!({"version": "v1", "hooks": [{
            "name": "crystalline-routing", "trigger": "SessionStart",
            "action": {"type": "command", "command": cmd}}]})
        );
        assert_eq!(
            parse_value(&undo(d, &fresh)).unwrap(),
            serde_json::json!({"version": "v1", "hooks": []})
        );
    }

    #[test]
    fn stored_program_reads_the_absolute_spelling_back() {
        let d = &HarnessKind::Cursor.profile().hooks;
        let with = run(
            d,
            Some(CURSOR),
            &session_command(HarnessKind::Cursor, "/a b/crystalline"),
        );
        assert_eq!(
            stored_program(d, &parse_value(&with).unwrap()).as_deref(),
            Some("/a b/crystalline")
        );
    }
}
