//! What an agent of the loops ([`Scope::Agent`]) may call, decided per call.
//!
//! An agent works only during a run of its loop (spec D2). Outside a live
//! lease of its loop its MCP session reaches the handshake, the tool list and
//! the `skills` tool, and nothing else; the automation steps (status, prompt,
//! lease take) are verbs of the ctl route. Inside a lease the server refuses,
//! at call time, what a headless run's prompt forbids: the tools of
//! [`HEADLESS_DISALLOWED_TOOLS`], the `write_reflection` action
//! `delete_asset` and any `overwrite: true`.
//!
//! The session is bound to one loop (spec D3): by the run-loop header
//! ([`crate::mcp::RUN_LOOP_HEADER`]), or, for an agent that runs one loop
//! only, by that loop. A write is noted only in the lease of the loop the call
//! belongs to ([`loop_of_write`]), so one agent with both loops never mixes
//! two pull requests.

use crystalline_core::automation::{HEADLESS_DISALLOWED_TOOLS, Loop};
use serde_json::{Map, Value};

use crate::rest::AgentLoops;
use crate::scope::Scope;

/// The one MCP tool an agent may call outside a run: its skills are what its
/// prompt opens with.
pub(crate) const OPEN_OUTSIDE_A_RUN: &str = "skills";

/// What an agent is told when it calls a tool while it holds no live lease of
/// its loop.
pub(crate) const AGENT_OUTSIDE_A_RUN: &str = "An agent works only during a run of its loop, and this agent holds no live lease of it. Its runner takes the lease of the loop on a domain first ('crystalline reflect run' or 'crystalline evolve run' does that). Until then this session may list the tools and read the skills with the skills tool.";

/// What an agent is told when it reads something other than a skill, or asks
/// for a prompt, outside a run.
pub(crate) const AGENT_READS_ONLY_IN_A_RUN: &str = "An agent reads only during a run of its loop, and this agent holds no live lease of it. Until its runner takes the lease, this session may list the tools and read the skills.";

/// What an agent token is told on the ctl route for any command but the
/// automation verbs.
pub(crate) const AGENT_CTL_AUTOMATION_ONLY: &str = "An agent token reaches only the automation verbs here (automation_status, automation_prompt, automation_lease_take and the verbs of a held lease). Every tool call goes over MCP, during a run.";

/// The refusal of a tool or a parameter a run may not use, inside a run.
/// `None` when the call may go ahead.
///
/// Checked on the arguments as sent, before they are read into a tool's
/// parameters, so a call is refused whether or not it would parse.
pub(crate) fn refused_in_run(tool: &str, args: Option<&Map<String, Value>>) -> Option<String> {
    if HEADLESS_DISALLOWED_TOOLS.contains(&tool) {
        return Some(format!(
            "A run may not call {tool}: it needs a person's yes, or it shares, pulls or throws away work a person owns. Leave it for a person and say so in the run's summary."
        ));
    }
    let args = args?;
    if tool == "write_reflection"
        && args.get("action").and_then(Value::as_str).map(str::trim) == Some("delete_asset")
    {
        return Some(
            "A run may not delete a file of the Reflection (write_reflection action delete_asset). Leave it for a person and say so in the run's summary."
                .to_string(),
        );
    }
    if args.get("overwrite").and_then(Value::as_bool) == Some(true) {
        return Some(format!(
            "A run may not call {tool} with overwrite: true, which replaces what is there. Write under a new path, or leave the change for a person and say so in the run's summary."
        ));
    }
    None
}

/// The loop an agent's session is bound to, from the run-loop header it sent
/// (`named`, the header's text) and the loops it runs:
///
/// - a header naming one of its loops binds it to that loop;
/// - no header binds an agent that runs one loop to that loop, and leaves an
///   agent with both loops unbound (`None`), so it may take either lease;
/// - a header naming no loop, or a loop the agent does not run, is refused
///   with a sentence saying why.
pub(crate) fn agent_loop(loops: AgentLoops, named: Option<&str>) -> Result<Option<Loop>, String> {
    let Some(named) = named else {
        return Ok(match loops {
            AgentLoops::Reflect => Some(Loop::Reflect),
            AgentLoops::Evolve => Some(Loop::Evolve),
            AgentLoops::Both => None,
        });
    };
    let lp: Loop = named.parse().map_err(|_| {
        format!(
            "{} names the loop this session runs, reflect or evolve; '{named}' is neither. Send one of the two, or leave the header out.",
            crate::mcp::RUN_LOOP_HEADER
        )
    })?;
    let runs = match lp {
        Loop::Reflect => loops.reflect(),
        Loop::Evolve => loops.evolve(),
    };
    if !runs {
        return Err(format!(
            "This agent does not run the {lp} loop, so a session bound to it by {} may call nothing. Name the loop the agent runs, or leave the header out.",
            crate::mcp::RUN_LOOP_HEADER
        ));
    }
    Ok(Some(lp))
}

/// `scope` with an agent's loops narrowed to `bound`: a session bound to one
/// loop has that loop's rights only. Any other scope, or no binding, is
/// handed back as it is.
pub(crate) fn narrowed(scope: Scope, bound: Option<Loop>) -> Scope {
    match (scope, bound) {
        (
            Scope::Agent {
                name,
                token_id,
                token_expires_at,
                ..
            },
            Some(lp),
        ) => Scope::Agent {
            name,
            loops: match lp {
                Loop::Reflect => AgentLoops::Reflect,
                Loop::Evolve => AgentLoops::Evolve,
            },
            token_id,
            token_expires_at,
        },
        (scope, _) => scope,
    }
}

/// The loop whose lease a write of `path` is noted in, for an agent whose
/// session is bound to `bound` (or unbound, `None`, for an agent with both
/// loops and no header): the bound loop, else the loop the path belongs to.
/// A file of the Reflection (`reflections/...`) is reflect's work, everything
/// else is an engram write and evolve's, which is also the only way each
/// right falls (reflect curates, evolve writes).
pub(crate) fn loop_of_write(bound: Option<Loop>, path: &str) -> Loop {
    bound.unwrap_or_else(|| {
        if path.starts_with("reflections/") {
            Loop::Reflect
        } else {
            Loop::Evolve
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn inside_a_run_exactly_the_prompts_forbidden_calls_are_refused() {
        for tool in HEADLESS_DISALLOWED_TOOLS {
            let refused = refused_in_run(tool, None).expect("a disallowed tool is refused");
            assert!(refused.contains(tool), "{refused}");
        }
        assert!(
            refused_in_run(
                "write_reflection",
                Some(&args(
                    json!({"domain": "eng", "action": "delete_asset", "path": "assets/a.png"})
                ))
            )
            .is_some_and(|m| m.contains("delete_asset"))
        );
        for tool in ["write_engram", "write_reflection"] {
            assert!(
                refused_in_run(tool, Some(&args(json!({"overwrite": true}))))
                    .is_some_and(|m| m.contains("overwrite: true")),
                "{tool}"
            );
        }
        // What a run does is not refused.
        for (tool, a) in [
            ("write_engram", json!({"domain": "eng", "overwrite": false})),
            ("write_engram", json!({"domain": "eng"})),
            ("edit_engram", json!({"domain": "eng"})),
            (
                "write_reflection",
                json!({"action": "write_asset", "path": "assets/a.png"}),
            ),
            ("write_reflection", json!({"action": "move_asset"})),
            ("read_engram", json!({})),
            ("skills", json!({})),
        ] {
            assert_eq!(refused_in_run(tool, Some(&args(a))), None, "{tool}");
        }
        for sentence in [
            AGENT_OUTSIDE_A_RUN,
            AGENT_READS_ONLY_IN_A_RUN,
            AGENT_CTL_AUTOMATION_ONLY,
        ] {
            assert!(!sentence.contains('\u{2014}') && !sentence.contains('\u{2013}'));
        }
    }

    #[test]
    fn the_session_is_bound_to_the_loop_the_header_or_the_agent_names() {
        assert_eq!(
            agent_loop(AgentLoops::Reflect, None),
            Ok(Some(Loop::Reflect))
        );
        assert_eq!(agent_loop(AgentLoops::Evolve, None), Ok(Some(Loop::Evolve)));
        assert_eq!(agent_loop(AgentLoops::Both, None), Ok(None));
        assert_eq!(
            agent_loop(AgentLoops::Both, Some("evolve")),
            Ok(Some(Loop::Evolve))
        );
        assert_eq!(
            agent_loop(AgentLoops::Reflect, Some(" reflect ")),
            Ok(Some(Loop::Reflect))
        );
        let other = agent_loop(AgentLoops::Reflect, Some("evolve")).unwrap_err();
        assert!(other.contains("does not run the evolve loop"), "{other}");
        let neither = agent_loop(AgentLoops::Both, Some("both")).unwrap_err();
        assert!(neither.contains("'both' is neither"), "{neither}");
    }

    #[test]
    fn a_write_is_noted_in_the_bound_loop_or_the_loop_its_path_belongs_to() {
        assert_eq!(
            loop_of_write(Some(Loop::Reflect), "notes/a.md"),
            Loop::Reflect
        );
        assert_eq!(
            loop_of_write(Some(Loop::Evolve), "reflections/p.md"),
            Loop::Evolve
        );
        assert_eq!(
            loop_of_write(None, "reflections/assets/a.png"),
            Loop::Reflect
        );
        assert_eq!(loop_of_write(None, "notes/a.md"), Loop::Evolve);
        assert_eq!(loop_of_write(None, "assets/a.png"), Loop::Evolve);
    }

    #[test]
    fn narrowing_keeps_the_agent_and_changes_only_its_loops() {
        let agent = Scope::Agent {
            name: "nightly".into(),
            loops: AgentLoops::Both,
            token_id: 7,
            token_expires_at: "2027-01-01T00:00:00Z".into(),
        };
        assert_eq!(narrowed(agent.clone(), None), agent);
        assert_eq!(
            narrowed(agent, Some(Loop::Evolve)),
            Scope::Agent {
                name: "nightly".into(),
                loops: AgentLoops::Evolve,
                token_id: 7,
                token_expires_at: "2027-01-01T00:00:00Z".into(),
            }
        );
        assert_eq!(
            narrowed(Scope::Unrestricted, Some(Loop::Reflect)),
            Scope::Unrestricted
        );
    }
}
