//! Integration tests for `crystalline prompt system` against a fixture
//! config and workspace, snapshotted with `insta`.

use std::time::Instant;

use predicates::prelude::*;

use crate::common::fixtures_dir;

#[test]
fn prompt_text_matches_snapshot() {
    let output = crate::common::crystalline()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();
    insta::assert_snapshot!(text);
}

#[test]
fn prompt_json_matches_snapshot() {
    let output = crate::common::crystalline()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json = String::from_utf8(output).unwrap();
    insta::assert_snapshot!(json);
}

#[test]
fn prompt_text_read_only_matches_snapshot() {
    let output = crate::common::crystalline()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
            "--read-only",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();
    // The read-only variant drops the write-tools line and names none of the
    // four content-mutating tools the block names.
    for tool in [
        "write_engram",
        "edit_engram",
        "move_engram",
        "delete_engram",
    ] {
        assert!(!text.contains(tool), "{tool} must not appear:\n{text}");
    }
    insta::assert_snapshot!(text);
}

#[test]
fn prompt_json_read_only_matches_snapshot() {
    let output = crate::common::crystalline()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
            "--read-only",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json = String::from_utf8(output).unwrap();
    insta::assert_snapshot!(json);
}

#[test]
fn missing_manifest_warns_on_stderr_but_still_exits_0() {
    crate::common::crystalline()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("gardening"));
}

/// Determinism contract, part (b): two entirely separate invocations of the
/// binary against the same on-disk fixture must produce byte-identical
/// stdout. This is the process-boundary counterpart to the in-process
/// double-render test in `crystalline_core::prompt`, catching anything
/// (stray env var, hash-based ordering) the in-process test would miss.
#[test]
fn prompt_system_output_is_byte_identical_across_separate_invocations() {
    let run = || {
        crate::common::crystalline()
            .current_dir(fixtures_dir().join("prompt-fixture"))
            .args([
                "prompt",
                "system",
                "--workspace",
                "workspace",
                "--config",
                "config.yaml",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    };

    let first = run();
    let second = run();
    assert_eq!(
        first, second,
        "crystalline prompt system must produce byte-identical stdout across separate process invocations"
    );
}

/// The copilot format answers a GitHub Copilot CLI SessionStart hook: one
/// JSON line whose `additionalContext` string carries the same routing
/// prompt the text format prints.
#[test]
fn prompt_copilot_format_matches_snapshot() {
    let output = crate::common::crystalline()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
            "--format",
            "copilot",
        ])
        .write_stdin(r#"{"source":"startup"}"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let line = String::from_utf8(output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    let context = parsed["additionalContext"].as_str().unwrap();
    assert!(
        !context.is_empty(),
        "additionalContext must carry the routing prompt"
    );
    insta::assert_snapshot!(line);
}

/// A resumed session already carries the earlier routing block in its
/// transcript, so the copilot format prints nothing at all for it.
#[test]
fn prompt_copilot_format_suppresses_resume() {
    crate::common::crystalline()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
            "--format",
            "copilot",
        ])
        .write_stdin(r#"{"source":"resume"}"#)
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

/// The stdin read is tolerant: an empty payload and a garbage payload both
/// proceed as a fresh start and emit the full JSON line.
#[test]
fn prompt_copilot_format_tolerates_missing_and_garbage_stdin() {
    for stdin in ["", "not json"] {
        let output = crate::common::crystalline()
            .current_dir(fixtures_dir().join("prompt-fixture"))
            .args([
                "prompt",
                "system",
                "--workspace",
                "workspace",
                "--config",
                "config.yaml",
                "--format",
                "copilot",
            ])
            .write_stdin(stdin)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let line = String::from_utf8(output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("stdin {stdin:?} must still emit one JSON line: {e}"));
        let context = parsed["additionalContext"].as_str().unwrap();
        assert!(
            context.contains("astronomy"),
            "stdin {stdin:?} must emit the routing prompt naming the fixture domain:\n{context}"
        );
    }
}

/// Run `prompt system` in the prompt fixture with extra args and a stdin
/// payload, under `home` when given (for a receipt), and return stdout.
fn prompt_in_fixture(extra: &[&str], stdin: &str, home: Option<&std::path::Path>) -> String {
    let mut cmd = crate::common::crystalline();
    if let Some(home) = home {
        crate::common::isolate(&mut cmd, home);
    }
    let mut args = vec![
        "prompt",
        "system",
        "--workspace",
        "workspace",
        "--config",
        "config.yaml",
    ];
    args.extend_from_slice(extra);
    let out = cmd
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args(&args)
        .write_stdin(stdin.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

/// The two new SessionStart shapes: Cursor's `additional_context` and the
/// `hookSpecificOutput` envelope Gemini CLI and Qwen Code read. Both are one
/// JSON document on stdout and nothing else.
#[test]
fn prompt_cursor_and_hook_specific_formats_match_snapshot() {
    let cursor = prompt_in_fixture(&["--format", "cursor"], r#"{"source":"startup"}"#, None);
    let v: serde_json::Value = serde_json::from_str(cursor.trim()).expect("one JSON document");
    assert!(
        v["additional_context"]
            .as_str()
            .unwrap()
            .contains("astronomy")
    );
    assert_eq!(v.as_object().unwrap().len(), 1, "only additional_context");
    insta::assert_snapshot!("prompt_cursor_format", cursor);

    let hook = prompt_in_fixture(
        &["--format", "hook-specific"],
        r#"{"source":"startup"}"#,
        None,
    );
    let v: serde_json::Value = serde_json::from_str(hook.trim()).expect("one JSON document");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("astronomy")
    );
    insta::assert_snapshot!("prompt_hook_specific_format", hook);
}

#[test]
fn the_new_formats_print_nothing_on_resume() {
    for format in ["cursor", "hook-specific"] {
        let out = prompt_in_fixture(&["--format", format], r#"{"source":"resume"}"#, None);
        assert!(out.is_empty(), "{format}: {out}");
    }
}

/// Decision 13: an imported Claude Code hook inside Cursor stays silent
/// when Cursor's own hook is installed, and speaks otherwise.
#[test]
fn an_imported_claude_hook_inside_cursor_is_silent_when_cursor_has_its_own() {
    let home = tempfile::tempdir().unwrap();
    let receipt = crate::common::isolated_state_dir(home.path()).join("installs.json");
    std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
    let row = |h: &str| {
        format!(
            r#"{{"harness":"{h}","scope":"user","version":"{}","parts":{{"mcp":true,"hooks":true,"skills":true}},"skills":[]}}"#,
            env!("CARGO_PKG_VERSION")
        )
    };
    let in_cursor = r#"{"source":"startup","session_id":"s1","cursor_version":"1.9.0","hook_event_name":"sessionStart"}"#;
    let plain = r#"{"source":"startup","session_id":"s1"}"#;
    let h = Some(home.path());

    std::fs::write(
        &receipt,
        format!(
            r#"{{"format":1,"installs":[{},{}]}}"#,
            row("claude-code"),
            row("cursor")
        ),
    )
    .unwrap();
    assert_eq!(
        prompt_in_fixture(&["--harness", "claude-code"], in_cursor, h),
        "",
        "silenced inside Cursor"
    );
    assert_eq!(
        prompt_in_fixture(&[], in_cursor, h),
        "",
        "a hook written before --harness counts as claude-code"
    );
    assert!(
        prompt_in_fixture(&["--format", "cursor", "--harness", "cursor"], in_cursor, h)
            .contains("additional_context"),
        "Cursor's own hook always prints"
    );
    assert!(
        prompt_in_fixture(&["--harness", "claude-code"], plain, h).contains("astronomy"),
        "plain Claude Code prints"
    );
    assert!(
        prompt_in_fixture(&["--harness", "nextgen"], in_cursor, h).contains("astronomy"),
        "an unknown id is never silenced"
    );

    std::fs::write(
        &receipt,
        format!(r#"{{"format":1,"installs":[{}]}}"#, row("claude-code")),
    )
    .unwrap();
    assert!(
        prompt_in_fixture(&["--harness", "claude-code"], in_cursor, h).contains("astronomy"),
        "no Cursor hook installed: the imported hook is the only route and speaks"
    );
}

/// Bare `crystalline prompt` (no kind) is a missing-subcommand error: clap
/// prints its standard subcommand help and exits non-zero, never silently
/// doing nothing.
#[test]
fn bare_prompt_without_a_kind_fails_with_subcommand_help() {
    crate::common::crystalline()
        .args(["prompt"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("system"));
}

/// The connector snippet is the standing instruction a remote client pastes
/// into its custom instructions: static copy, printed as one paragraph.
#[test]
fn prompt_connector_matches_snapshot() {
    let output = crate::common::crystalline()
        .args(["prompt", "connector"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains("include_routing"),
        "the snippet must point at the onboarding call:\n{text}"
    );
    insta::assert_snapshot!(text);
}

#[test]
fn prompt_connector_json_matches_snapshot() {
    let output = crate::common::crystalline()
        .args(["prompt", "connector", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let line = String::from_utf8(output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["kind"], "connector");
    insta::assert_snapshot!(line);
}

/// Determinism contract: the snippet is a constant, so two separate
/// invocations must agree byte for byte.
#[test]
fn prompt_connector_is_byte_identical_across_invocations() {
    let run = || {
        crate::common::crystalline()
            .args(["prompt", "connector"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    };

    assert_eq!(
        run(),
        run(),
        "crystalline prompt connector must produce byte-identical stdout across separate process invocations"
    );
}

/// The snippet is what a user reaches for before anything is configured, so
/// the command must answer from a directory with no config at all.
#[test]
fn prompt_connector_works_with_no_config() {
    let tmp = tempfile::tempdir().unwrap();
    crate::common::crystalline()
        .current_dir(tmp.path())
        .env("CRYSTALLINE_CONFIG", tmp.path().join("config.yaml"))
        .args(["prompt", "connector"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list_domains"));
}

/// Latency contract, part (c): a CI-safe guard, generous against runner
/// noise, that `prompt system` stays well under budget even at 30 scaffolded
/// domains. The real target (under 50ms wall-clock in a release build) is
/// measured and reported separately, since a debug test binary under a
/// possibly loaded CI runner is not a fair proxy for that number.
#[test]
fn prompt_system_scaffolded_30_domains_stays_under_500ms() {
    let tmp = tempfile::tempdir().unwrap();

    let mut config_yaml = String::from("domains:\n");
    for i in 0..30 {
        let name = format!("domain{i:02}");
        let dir = tmp.path().join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            format!(
                "---\ntype: manifest\ntitle: MANIFEST\npermalink: {name}/manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n# {name}\n\n## Scope\n\n- scope for {name}\n\n## When to Use\n\n- When a task touches {name}\n"
            ),
        )
        .unwrap();
        config_yaml.push_str(&format!("  {name}:\n    path: {name}\n"));
    }
    std::fs::write(tmp.path().join("config.yaml"), config_yaml).unwrap();

    let start = Instant::now();
    crate::common::crystalline()
        .current_dir(tmp.path())
        .args(["prompt", "system", "--config", "config.yaml"])
        .assert()
        .success();
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 500,
        "prompt system took {elapsed:?} for 30 domains, expected well under the 500ms CI-safe bound"
    );
}

/// `--harness` is accepted on the routing command because both managed hook
/// commands now carry it, and it changes nothing about the routing block: a
/// known id, an id this binary does not know (a hook a newer release wired up,
/// run by an older binary) and no flag at all all print the same bytes. A
/// lifecycle hook that started refusing arguments would take the session's
/// routing block down with it.
#[test]
fn the_harness_flag_never_changes_the_routing_block() {
    let run = |extra: &[&str]| {
        let mut args = vec![
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
        ];
        args.extend_from_slice(extra);
        crate::common::crystalline()
            .current_dir(fixtures_dir().join("prompt-fixture"))
            .args(&args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    };

    let plain = run(&[]);
    assert_eq!(
        run(&["--harness", "claude-code"]),
        plain,
        "a known harness prints the same routing block"
    );
    assert_eq!(
        run(&["--harness", "a-harness-from-a-future-release"]),
        plain,
        "an unknown harness is inert, never an error"
    );
}

/// Scaffold a config with one registered domain per name, each holding a
/// minimal readable `MANIFEST.md`, in a fresh temp directory. Returns the
/// directory so a test can point `--config` and `--workspace` at it.
fn scaffold_domains(names: &[&str]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let mut config_yaml = String::from("domains:\n");
    for name in names {
        let dir = tmp.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            format!(
                "---\ntype: manifest\ntitle: MANIFEST\npermalink: {name}/manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n# {name}\n\n## Scope\n\n- scope for {name}\n\n## When to Use\n\n- When a task touches {name}\n"
            ),
        )
        .unwrap();
        config_yaml.push_str(&format!("  {name}:\n    path: {name}\n"));
    }
    std::fs::write(tmp.path().join("config.yaml"), config_yaml).unwrap();
    tmp
}

/// `--domain` renders only the named domains, in the order the prompt would
/// have put them in (registry order, not the order the flags arrived in),
/// and it composes with the workspace scoping rather than replacing it. It
/// works the same way in every output format.
#[test]
fn prompt_system_renders_only_the_named_domains() {
    let tmp = scaffold_domains(&["alpha", "beta", "gamma"]);

    let out = crate::common::crystalline()
        .current_dir(tmp.path())
        .args([
            "prompt",
            "system",
            "--config",
            "config.yaml",
            "--domain",
            "gamma",
            "--domain",
            "alpha",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("alpha"), "{out}");
    assert!(out.contains("gamma"), "{out}");
    assert!(
        !out.contains("beta"),
        "a domain nobody named is not rendered: {out}"
    );
    // Registry order, not the order the flags arrived in: the flag picks, the
    // configured preference orders.
    assert!(
        out.find("alpha").unwrap() < out.find("gamma").unwrap(),
        "{out}"
    );

    // Every format, not just the default one.
    let json = crate::common::crystalline()
        .current_dir(tmp.path())
        .args([
            "prompt",
            "system",
            "--config",
            "config.yaml",
            "--domain",
            "alpha",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json = String::from_utf8(json).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let names: Vec<&str> = value["domains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha"]);

    let copilot = crate::common::crystalline()
        .current_dir(tmp.path())
        .args([
            "prompt",
            "system",
            "--config",
            "config.yaml",
            "--domain",
            "alpha",
            "--format",
            "copilot",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let copilot = String::from_utf8(copilot).unwrap();
    let value: serde_json::Value = serde_json::from_str(&copilot).unwrap();
    let context = value["additionalContext"].as_str().unwrap();
    assert!(
        context.contains("alpha") && !context.contains("beta"),
        "{context}"
    );
}

/// A name nobody registered is an error naming what IS registered, so the
/// person who typoed can read the answer off the failure.
#[test]
fn prompt_system_refuses_a_domain_that_is_not_registered() {
    let tmp = scaffold_domains(&["alpha", "beta"]);

    let output = crate::common::crystalline()
        .current_dir(tmp.path())
        .args([
            "prompt",
            "system",
            "--config",
            "config.yaml",
            "--domain",
            "alfa",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8(output).unwrap();
    assert!(err.contains("alfa"), "{err}");
    assert!(
        err.contains("alpha") && err.contains("beta"),
        "the registered names are listed: {err}"
    );
    assert!(
        err.contains("crystalline domain list"),
        "and a command that shows them: {err}"
    );
}

/// The `--workspace` help says what actually scopes and what only reorders.
#[test]
fn the_workspace_help_names_prompt_rules_and_says_what_repo_config_does() {
    let output = crate::common::crystalline()
        .args(["prompt", "system", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(output).unwrap();
    assert!(help.contains("prompt.rules"), "{help}");
    assert!(help.contains(".crystalline.yaml"), "{help}");
    assert!(
        help.contains("every registered domain"),
        "the no-match answer is stated: {help}"
    );
}

/// A domain the workspace's `prompt.rules` exclude stays excluded even when
/// it is named with `--domain`: the flag composes with the workspace scoping
/// rather than replacing it, exactly as `--domain`'s own help promises
/// ("stays excluded"). `beta` is registered (so naming it is not a typo, and
/// the command does not refuse it), but this workspace's `prompt.rules`
/// exclude it, so it never reaches the routing block regardless of what is
/// named.
#[test]
fn prompt_system_domain_flag_cannot_undo_a_workspace_exclusion() {
    let tmp = scaffold_domains(&["alpha", "beta"]);
    std::fs::write(
        tmp.path().join("config.yaml"),
        "domains:\n  alpha:\n    path: alpha\n  beta:\n    path: beta\nprompt:\n  rules:\n    \"**\":\n      exclude:\n      - beta\n",
    )
    .unwrap();

    let out = crate::common::crystalline()
        .current_dir(tmp.path())
        .args([
            "prompt",
            "system",
            "--config",
            "config.yaml",
            "--workspace",
            ".",
            "--domain",
            "beta",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(
        !out.contains("beta"),
        "a domain the workspace excludes stays out even when named: {out}"
    );

    // Naming an excluded domain alongside an included one still renders the
    // included one: the flag narrows what survived the workspace scoping, it
    // does not replace it.
    let out = crate::common::crystalline()
        .current_dir(tmp.path())
        .args([
            "prompt",
            "system",
            "--config",
            "config.yaml",
            "--workspace",
            ".",
            "--domain",
            "alpha",
            "--domain",
            "beta",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("alpha"), "{out}");
    assert!(
        !out.contains("beta"),
        "a domain the workspace excludes stays out even when named alongside one that is not: {out}"
    );

    // The `--domain` flag's own help states the guarantee this test proves,
    // in the same words: the workspace's `prompt.rules` exclusion wins.
    let help = crate::common::crystalline()
        .args(["prompt", "system", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(help).unwrap();
    assert!(
        help.contains("stays excluded"),
        "the --domain help states the guarantee this test proves: {help}"
    );
}

/// The text format reads the SessionStart payload too now (it is what resets
/// the per-prompt recall list on a clear or a compaction), so its stdin read
/// has to be exactly as tolerant as the copilot one: garbage on stdin still
/// prints the routing block, byte for byte what the snapshot holds.
#[test]
fn prompt_system_text_format_tolerates_garbage_stdin() {
    let output = crate::common::crystalline()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
        ])
        .write_stdin("{ not json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();
    // The same stored snapshot `prompt_text_matches_snapshot` asserts, named
    // explicitly so this is byte-identity with that expectation rather than a
    // second snapshot that could drift away from it.
    insta::assert_snapshot!("prompt_text_matches_snapshot", text);
}
