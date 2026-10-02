//! The MCP entry in a harness JSON file: what install writes, how an
//! existing entry is read back, and the edits for install and uninstall.
//! Pure functions; the caller reads and writes the file.

use std::path::Path;

use crystalline_core::harness::HarnessKind;
use crystalline_core::jsonc_edit::{Edit, JsonIn, Prune, SegBuf};
use serde_json::Value;

use crate::harness_command::is_crystalline_program;

/// The `mcpServers` key the entry lives under.
const SERVERS_KEY: &str = "mcpServers";
/// The entry's own key.
const ENTRY_KEY: &str = "crystalline";

/// What an existing `mcpServers.crystalline` entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryState {
    /// No entry.
    Missing,
    /// Our entry as install writes it today (harness defaults beside it
    /// are allowed).
    UpToDate,
    /// Our entry in an older shape (no `--harness`, another harness id, or
    /// bare where an absolute path is wanted): install rewrites it.
    Ours,
    /// Anything else: left alone unless uninstall is forced.
    Customised,
}

fn s(text: &str) -> JsonIn {
    JsonIn::Str(text.to_string())
}

fn args_for(harness: HarnessKind) -> JsonIn {
    JsonIn::Array(vec![s("mcp"), s("--harness"), s(harness.id())])
}

/// The minimal stdio entry for a harness (spec 3.3). `env`, `disabled`,
/// `autoApprove`, `timeout` and `trust` are left out.
pub(crate) fn desired_entry(harness: HarnessKind, program: &str) -> JsonIn {
    let mut members = Vec::new();
    if harness == HarnessKind::Cursor {
        members.push(("type".to_string(), s("stdio")));
    }
    members.push(("command".to_string(), s(program)));
    members.push(("args".to_string(), args_for(harness)));
    JsonIn::Object(members)
}

/// Whether a key and value is one a harness fills in by default (spec
/// decision 9), so it does not make an entry customised.
fn is_harness_default(key: &str, value: &Value) -> bool {
    match key {
        "env" => value.as_object().is_some_and(|o| o.is_empty()),
        "disabled" | "trust" => value == &Value::Bool(false),
        "autoApprove" => value.as_array().is_some_and(|a| a.is_empty()),
        _ => false,
    }
}

/// Read an existing entry back. The command spelling is not judged here
/// (see [`plan_mcp_install`]); a crystalline program in either spelling
/// counts.
pub(crate) fn classify(harness: HarnessKind, entry: &Value) -> EntryState {
    let Some(map) = entry.as_object() else {
        return EntryState::Customised;
    };
    let command_ours = map
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_crystalline_program);
    if !command_ours {
        return EntryState::Customised;
    }
    let others_ok = map.iter().all(|(key, value)| match key.as_str() {
        "command" | "args" => true,
        "type" => value.as_str() == Some("stdio"),
        other => is_harness_default(other, value),
    });
    if !others_ok {
        return EntryState::Customised;
    }
    let Some(args) = map.get("args").and_then(Value::as_array) else {
        return EntryState::Customised;
    };
    let strings: Vec<&str> = args.iter().filter_map(Value::as_str).collect();
    if strings.len() != args.len() {
        return EntryState::Customised;
    }
    let args = strings;
    match args.as_slice() {
        ["mcp"] => EntryState::Ours,
        ["mcp", "--harness", id] if *id == harness.id() => EntryState::UpToDate,
        ["mcp", "--harness", id] if HarnessKind::ALL.iter().any(|k| k.id() == *id) => {
            EntryState::Ours
        }
        _ => EntryState::Customised,
    }
}

fn entry_path() -> Vec<SegBuf> {
    vec![
        SegBuf::Key(SERVERS_KEY.to_string()),
        SegBuf::Key(ENTRY_KEY.to_string()),
    ]
}

/// The existing entry under `mcpServers`, or the sentence the report
/// prints when the file's shape does not allow one.
fn existing_entry(root: Option<&Value>) -> Result<Option<&Value>, String> {
    let Some(root) = root else {
        return Ok(None);
    };
    let Some(top) = root.as_object() else {
        return Err("the file does not hold a JSON object at the top level. Make it an object ({}) and run the command again".to_string());
    };
    match top.get(SERVERS_KEY) {
        None => Ok(None),
        Some(Value::Object(servers)) => Ok(servers.get(ENTRY_KEY)),
        Some(_) => Err(format!(
            "\"{SERVERS_KEY}\" is not a JSON object. Make it an object ({{}}) or remove it and run the command again"
        )),
    }
}

/// The state of the entry and the edits install makes. `root` is `None`
/// for a file that does not exist.
pub(crate) fn plan_mcp_install(
    harness: HarnessKind,
    root: Option<&Value>,
    program: &str,
) -> Result<(EntryState, Vec<Edit>), String> {
    let set = || Edit::Set {
        path: entry_path(),
        value: desired_entry(harness, program),
    };
    let Some(entry) = existing_entry(root)? else {
        return Ok((EntryState::Missing, vec![set()]));
    };
    let mut state = classify(harness, entry);
    // A command that is not the wanted program is rewritten: bare where an
    // absolute path is wanted, or another path (maybe dead or old). An
    // explicit install passes the crystalline on PATH, so a stale path is
    // replaced.
    if state == EntryState::UpToDate
        && entry.get("command").and_then(Value::as_str) != Some(program)
    {
        state = EntryState::Ours;
    }
    let edits = if state == EntryState::Ours {
        vec![set()]
    } else {
        Vec::new()
    };
    Ok((state, edits))
}

/// The state of the entry and the edits uninstall makes: a removal that
/// also drops an `mcpServers` this left empty.
pub(crate) fn plan_mcp_uninstall(
    harness: HarnessKind,
    root: &Value,
    force: bool,
) -> Result<(EntryState, Vec<Edit>), String> {
    let Some(entry) = existing_entry(Some(root))? else {
        return Ok((EntryState::Missing, Vec::new()));
    };
    let state = classify(harness, entry);
    let remove = state != EntryState::Customised || force;
    let edits = if remove {
        vec![Edit::Remove {
            path: entry_path(),
            prune: Prune::Emptied,
        }]
    } else {
        Vec::new()
    };
    Ok((state, edits))
}

/// The report status for an install that found this state.
pub(crate) fn install_status(state: EntryState) -> &'static str {
    match state {
        EntryState::Missing => "registered",
        EntryState::Ours => "repaired",
        EntryState::UpToDate => "already-present",
        EntryState::Customised => "already-present-customised",
    }
}

/// The report status for an uninstall that found this state.
pub(crate) fn uninstall_status(state: EntryState, force: bool) -> &'static str {
    match state {
        EntryState::Missing => "not-present",
        EntryState::UpToDate | EntryState::Ours => "removed",
        EntryState::Customised if force => "removed-forced",
        EntryState::Customised => "kept-customised",
    }
}

/// What the report prints when the file could not be edited or holds a
/// customised entry: the entry itself, for the person to add by hand.
pub(crate) fn manual_hint(harness: HarnessKind, program: &str, path: &Path) -> String {
    let json = serde_json::to_string_pretty(&desired_entry(harness, program).to_value())
        .unwrap_or_default();
    format!("Add this under {SERVERS_KEY} in {}: {json}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_command::{TEST_OTHER_PROGRAM, TEST_PROGRAM};
    use crystalline_core::jsonc_edit::{apply_edits, parse_value};
    const CURSOR_MCP: &str = include_str!("../../core/tests/fixtures/harness/cursor-mcp.json");
    const KIRO_MCP: &str = include_str!("../../core/tests/fixtures/harness/kiro-mcp.json");

    #[test]
    fn each_harness_gets_its_documented_minimal_entry() {
        assert_eq!(
            desired_entry(HarnessKind::Cursor, TEST_PROGRAM).to_value(),
            serde_json::json!({"type": "stdio", "command": TEST_PROGRAM, "args": ["mcp", "--harness", "cursor"]})
        );
        for (k, id) in [
            (HarnessKind::Kiro, "kiro"),
            (HarnessKind::Gemini, "gemini"),
            (HarnessKind::Qwen, "qwen"),
        ] {
            assert_eq!(
                desired_entry(k, "crystalline").to_value(),
                serde_json::json!({"command": "crystalline", "args": ["mcp", "--harness", id]})
            );
        }
    }

    #[test]
    fn harness_defaults_beside_our_entry_still_read_as_ours() {
        let e = serde_json::json!({"command": TEST_PROGRAM, "args": ["mcp", "--harness", "kiro"], "env": {}, "disabled": false, "autoApprove": []});
        assert_eq!(classify(HarnessKind::Kiro, &e), EntryState::UpToDate);
        for customised in [
            serde_json::json!({"command": TEST_PROGRAM, "args": ["mcp", "--harness", "kiro"], "env": {"RUST_LOG": "debug"}}),
            serde_json::json!({"command": TEST_PROGRAM, "args": ["mcp", "--harness", "kiro", "--db", "/tmp/x"]}),
            serde_json::json!({"url": "https://team.example/mcp"}),
            serde_json::json!({"command": TEST_PROGRAM, "args": ["mcp", "--harness", "kiro"], "timeout": 600000}),
            serde_json::json!({"command": "other", "args": ["mcp", "--harness", "kiro"]}),
        ] {
            assert_eq!(
                classify(HarnessKind::Kiro, &customised),
                EntryState::Customised,
                "{customised}"
            );
        }
        assert_eq!(
            classify(
                HarnessKind::Kiro,
                &serde_json::json!({"command": "crystalline", "args": ["mcp"]})
            ),
            EntryState::Ours
        );
        assert_eq!(
            classify(
                HarnessKind::Kiro,
                &serde_json::json!({"command": "crystalline", "args": ["mcp", "--harness", "gemini"]})
            ),
            EntryState::Ours
        );
    }

    #[test]
    fn install_then_uninstall_gives_the_users_file_back() {
        for (k, original, program) in [
            (HarnessKind::Cursor, CURSOR_MCP, TEST_PROGRAM),
            (HarnessKind::Kiro, KIRO_MCP, TEST_PROGRAM),
        ] {
            let (state, edits) =
                plan_mcp_install(k, Some(&parse_value(original).unwrap()), program).unwrap();
            assert_eq!(state, EntryState::Missing);
            let with = apply_edits(Some(original), &edits).unwrap().unwrap();
            let (again, none) =
                plan_mcp_install(k, Some(&parse_value(&with).unwrap()), program).unwrap();
            assert_eq!((again, none.len()), (EntryState::UpToDate, 0));
            let (_, edits) = plan_mcp_uninstall(k, &parse_value(&with).unwrap(), false).unwrap();
            assert_eq!(apply_edits(Some(&with), &edits).unwrap().unwrap(), original);
        }
    }

    #[test]
    fn a_customised_entry_survives_uninstall_unless_forced() {
        let text = "{\"mcpServers\": {\"crystalline\": {\"command\": \"crystalline\", \"args\": [\"mcp\", \"--harness\", \"gemini\"], \"env\": {\"X\": \"1\"}}}}";
        let root = parse_value(text).unwrap();
        let (state, edits) = plan_mcp_uninstall(HarnessKind::Gemini, &root, false).unwrap();
        assert_eq!((state, edits.len()), (EntryState::Customised, 0));
        let (_, edits) = plan_mcp_uninstall(HarnessKind::Gemini, &root, true).unwrap();
        assert_eq!(edits.len(), 1);
    }

    #[test]
    fn an_mcp_servers_that_is_not_an_object_is_refused() {
        let root = parse_value("{\"mcpServers\": [1]}").unwrap();
        let err = plan_mcp_install(HarnessKind::Gemini, Some(&root), "crystalline").unwrap_err();
        assert!(err.contains("mcpServers"), "{err}");
        let root = parse_value("[1]").unwrap();
        assert!(plan_mcp_install(HarnessKind::Gemini, Some(&root), "crystalline").is_err());
    }

    #[test]
    fn an_older_entry_is_repaired_in_one_set_and_statuses_follow() {
        let text = "{\"mcpServers\": {\"crystalline\": {\"command\": \"crystalline\", \"args\": [\"mcp\"]}}}";
        let root = parse_value(text).unwrap();
        let (state, edits) =
            plan_mcp_install(HarnessKind::Kiro, Some(&root), "crystalline").unwrap();
        assert_eq!((state, edits.len()), (EntryState::Ours, 1));
        assert_eq!(install_status(state), "repaired");
        // Bare written where the profile now wants an absolute path.
        let bare = parse_value(
            "{\"mcpServers\": {\"crystalline\": {\"type\": \"stdio\", \"command\": \"crystalline\", \"args\": [\"mcp\", \"--harness\", \"cursor\"]}}}",
        )
        .unwrap();
        let (state, edits) =
            plan_mcp_install(HarnessKind::Cursor, Some(&bare), TEST_PROGRAM).unwrap();
        assert_eq!((state, edits.len()), (EntryState::Ours, 1));
        assert_eq!(install_status(EntryState::UpToDate), "already-present");
        assert_eq!(
            install_status(EntryState::Customised),
            "already-present-customised"
        );
        assert_eq!(
            uninstall_status(EntryState::Customised, false),
            "kept-customised"
        );
        assert_eq!(
            uninstall_status(EntryState::Customised, true),
            "removed-forced"
        );
        assert_eq!(uninstall_status(EntryState::Missing, false), "not-present");
        assert_eq!(uninstall_status(EntryState::Ours, false), "removed");
    }

    fn cursor_root(command: &str) -> serde_json::Value {
        serde_json::json!({"mcpServers": {"crystalline": {"type": "stdio", "command": command, "args": ["mcp", "--harness", "cursor"]}}})
    }

    #[test]
    fn a_command_that_differs_from_the_wanted_program_is_repaired() {
        let wanted = TEST_PROGRAM;
        // A dead absolute path from another prefix, and a bare command.
        for old in [TEST_OTHER_PROGRAM, "crystalline"] {
            let (state, edits) =
                plan_mcp_install(HarnessKind::Cursor, Some(&cursor_root(old)), wanted).unwrap();
            assert_eq!((state, edits.len()), (EntryState::Ours, 1), "{old}");
            assert_eq!(install_status(state), "repaired");
        }
        // Already equal to the wanted program: nothing to do.
        let (state, edits) =
            plan_mcp_install(HarnessKind::Cursor, Some(&cursor_root(wanted)), wanted).unwrap();
        assert_eq!((state, edits.len()), (EntryState::UpToDate, 0));
        assert_eq!(install_status(state), "already-present");
        // A live stored path arrives as the wanted program, so it stays.
        let live = TEST_OTHER_PROGRAM;
        let (state, edits) =
            plan_mcp_install(HarnessKind::Cursor, Some(&cursor_root(live)), live).unwrap();
        assert_eq!((state, edits.len()), (EntryState::UpToDate, 0));
    }

    #[test]
    fn a_customised_entry_is_untouched_whatever_the_program() {
        let root = serde_json::json!({"mcpServers": {"crystalline": {"command": TEST_OTHER_PROGRAM, "args": ["mcp", "--harness", "kiro", "--db", "/x"], "env": {"A": "1"}}}});
        let (state, edits) =
            plan_mcp_install(HarnessKind::Kiro, Some(&root), TEST_PROGRAM).unwrap();
        assert_eq!((state, edits.len()), (EntryState::Customised, 0));
    }

    #[test]
    fn uninstall_refuses_a_file_it_cannot_read_like_install_does() {
        let root = parse_value("{\"mcpServers\": [1]}").unwrap();
        let err = plan_mcp_uninstall(HarnessKind::Gemini, &root, false).unwrap_err();
        assert!(err.contains("mcpServers"), "{err}");
    }

    #[test]
    fn the_manual_hint_names_the_file_and_the_entry() {
        let hint = manual_hint(
            HarnessKind::Gemini,
            "crystalline",
            std::path::Path::new("/h/.gemini/settings.json"),
        );
        assert!(hint.starts_with("Add this under mcpServers in /h/.gemini/settings.json: "));
        assert!(
            hint.contains("\"--harness\"") && hint.contains("\"gemini\""),
            "{hint}"
        );
    }
}
