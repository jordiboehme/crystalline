//! What an agent reads when a write went out to a server and no answer came
//! back in time. The server may have done the write or not, and this machine
//! cannot tell which. So the agent is told both, and how to check before it
//! tries again.
//!
//! The sentences name only what the call carried in words the server does
//! not rename: a domain-relative path or an engram title. A domain or a
//! `crystalline://` address in the forwarded call is in the server's names
//! already, so it is never quoted back.

use serde_json::Value;

/// The check for one ctl request when it changes something on the server
/// and its answer never came: a forwarded tool call ([`unanswered_write`])
/// or an automation verb that changes data. `None` for a request that only
/// reads.
pub(crate) fn unanswered_check(request: &Value) -> Option<String> {
    let text = |key: &str| request.get(key).and_then(Value::as_str);
    let cmd = text("cmd")?;
    if cmd == "tool" {
        return unanswered_write(
            text("tool").unwrap_or_default(),
            request.get("args").unwrap_or(&Value::Null),
        );
    }
    // The loop as the CLI names it: `crystalline reflect status`.
    let status = format!(
        "crystalline {} status",
        text("loop").filter(|l| !l.is_empty()).unwrap_or("reflect")
    );
    Some(match cmd {
        "automation_lease_take" => format!(
            "The lease may or may not be taken. Run {status} to see whether a run is going \
             before you start one again."
        ),
        "automation_lease_renew" => {
            "The lease may or may not be renewed. The next renew tries again.".to_string()
        }
        "automation_lease_release" => format!(
            "The lease may or may not be released. Run {status} to see whether a run is still \
             going."
        ),
        "automation_run_record" => format!(
            "The run may or may not be recorded. Run {status} to see whether a run is still \
             going before you record it again."
        ),
        "automation_share" => "The changes may or may not be shared. Look for the run's pull \
                               request before you share them again."
            .to_string(),
        _ => return None,
    })
}

/// The check for `tool` called with `args`, when the call changes something
/// on the server and its answer never came: what may have happened, and how
/// to find out. `None` for a tool that only reads.
fn unanswered_write(tool: &str, args: &Value) -> Option<String> {
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let page = || text("path").unwrap_or("the page");
    Some(match tool {
        "write_engram" => match (text("path"), text("title")) {
            (Some(path), _) => upload(path, ""),
            (None, Some(title)) => format!(
                "The engram may or may not be written. Read '{title}' with read_engram to see \
                 whether it holds your text before you write it again."
            ),
            (None, None) => "The engram may or may not be written. Read it with read_engram to \
                             see whether it holds your text before you write it again."
                .to_string(),
        },
        "edit_engram" => "The edit may or may not be applied. Read the engram with read_engram \
                          to see whether it holds your change. If you edit it again, pass the \
                          checksum you read as expected_checksum."
            .to_string(),
        "delete_engram" => "The delete may or may not have happened. Read it with read_engram: \
                            if it is gone, the delete happened."
            .to_string(),
        "move_engram" => "The move may or may not have happened. Read the engram with \
                          read_engram at its old and its new place to see where it is before \
                          you move it again."
            .to_string(),
        "split_engram" => "The split may or may not have happened. Read the engram with \
                           read_engram: a split_into relation means it was split. If you split \
                           it again, pass the checksum you read as expected_checksum."
            .to_string(),
        "write_reflection" => match text("action").unwrap_or("write") {
            "write_asset" => upload(page(), " with read_reflection"),
            "move" | "move_asset" => format!(
                "The move may or may not have happened. Read the Reflection with \
                 read_reflection to see where {} is before you move it again.",
                page()
            ),
            "delete" | "delete_asset" => format!(
                "The delete may or may not have happened. Read {} with read_reflection: if it \
                 is gone, the delete happened.",
                page()
            ),
            _ => format!(
                "The change may or may not be made. Read {} with read_reflection to see \
                 whether it holds your change. If you write it again, pass the checksum you \
                 read as expected_checksum.",
                page()
            ),
        },
        "discuss" => "The change to the discussion may or may not have happened. Read the page \
                      with read_reflection and look at its threads before you try again, so \
                      nothing is posted twice."
            .to_string(),
        _ => return None,
    })
}

/// The check for a file upload: a retry of the same bytes is accepted
/// (`unchanged: true`), so it is safe.
fn upload(path: &str, reader: &str) -> String {
    format!(
        "The file may or may not be written. Read {path}{reader} to check its sha256, or upload \
         the same file again: the same bytes are accepted."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_automation_write_gets_a_check_and_its_reads_get_none() {
        for (cmd, said) in [
            (
                "automation_lease_take",
                "The lease may or may not be taken. Run crystalline reflect status to see \
                 whether a run is going before you start one again.",
            ),
            (
                "automation_lease_renew",
                "The lease may or may not be renewed. The next renew tries again.",
            ),
            (
                "automation_lease_release",
                "The lease may or may not be released. Run crystalline reflect status to see \
                 whether a run is still going.",
            ),
            (
                "automation_run_record",
                "The run may or may not be recorded. Run crystalline reflect status to see \
                 whether a run is still going before you record it again.",
            ),
            (
                "automation_share",
                "The changes may or may not be shared. Look for the run's pull request before \
                 you share them again.",
            ),
        ] {
            let request = json!({ "v": 1, "cmd": cmd, "loop": "reflect", "domain": "ops" });
            assert_eq!(unanswered_check(&request).as_deref(), Some(said), "{cmd}");
        }
        for read in ["automation_status", "automation_runs", "status", "file"] {
            let request = json!({ "v": 1, "cmd": read, "loop": "reflect" });
            assert_eq!(unanswered_check(&request), None, "{read}");
        }
        let tool = json!({ "v": 1, "cmd": "tool", "tool": "delete_engram",
            "args": { "identifier": "x" } });
        assert!(unanswered_check(&tool).unwrap().starts_with("The delete"));
    }

    #[test]
    fn an_upload_names_its_path_and_says_a_retry_is_safe() {
        assert_eq!(
            unanswered_write(
                "write_engram",
                &json!({ "domain": "ops", "title": "Shot", "path": "assets/shot.png" })
            )
            .unwrap(),
            "The file may or may not be written. Read assets/shot.png to check its sha256, or \
             upload the same file again: the same bytes are accepted."
        );
        assert_eq!(
            unanswered_write(
                "write_reflection",
                &json!({ "domain": "ops", "action": "write_asset", "path": "assets/flow.png" })
            )
            .unwrap(),
            "The file may or may not be written. Read assets/flow.png with read_reflection to \
             check its sha256, or upload the same file again: the same bytes are accepted."
        );
    }

    #[test]
    fn an_engram_write_names_its_title_and_never_a_domain() {
        let said = unanswered_write(
            "write_engram",
            &json!({ "domain": "server-ops", "title": "Night deploys", "path": null }),
        )
        .unwrap();
        assert_eq!(
            said,
            "The engram may or may not be written. Read 'Night deploys' with read_engram to see \
             whether it holds your text before you write it again."
        );
        assert!(!said.contains("server-ops"));
    }

    #[test]
    fn every_write_tool_gets_a_check_and_a_read_gets_none() {
        let address = json!({ "domain": "server-ops",
            "identifier": "crystalline://server-ops/runbooks/deploy" });
        for tool in [
            "edit_engram",
            "delete_engram",
            "move_engram",
            "split_engram",
            "discuss",
        ] {
            let said = unanswered_write(tool, &address).unwrap();
            assert!(said.contains("may or may not"), "{tool}: {said}");
            assert!(!said.contains("server-ops"), "{tool}: {said}");
        }
        for tool in ["edit_engram", "split_engram"] {
            assert!(
                unanswered_write(tool, &address)
                    .unwrap()
                    .contains("expected_checksum"),
                "{tool}"
            );
        }
        for action in [
            "write",
            "settle",
            "move",
            "move_asset",
            "delete",
            "delete_asset",
        ] {
            let said = unanswered_write(
                "write_reflection",
                &json!({ "domain": "ops", "action": action, "path": "guides/deploy.md" }),
            )
            .unwrap();
            assert!(said.contains("guides/deploy.md"), "{action}: {said}");
            assert!(said.contains("read_reflection"), "{action}: {said}");
        }
        for read in [
            "read_engram",
            "search_engrams",
            "read_reflection",
            "evolve_engrams",
        ] {
            assert_eq!(unanswered_write(read, &address), None, "{read}");
        }
    }
}
