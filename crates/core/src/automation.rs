//! The reflect and evolve loops run headless: the words and the numbers a
//! runner, the engine and every surface share. Plain data; the engine keeps
//! the lease and the run record, a runner outside Crystalline does the work.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A lease that was not renewed for this many seconds is stale: its runner
/// is taken for crashed, the loop is due again and a new run may take it.
pub const LEASE_STALE_SECS: i64 = 300;

/// How often a runner renews its lease, in seconds.
pub const LEASE_RENEW_SECS: u64 = 60;

/// How many runs are kept per domain and loop.
pub const RUNS_KEPT: usize = 50;

/// The most bytes a run summary keeps; a longer one is cut at a char
/// boundary.
pub const SUMMARY_MAX_BYTES: usize = 2048;

/// How long a loop waits after one failed run before it is due again, in
/// seconds; each further failed run in a row doubles it.
pub const BACKOFF_FIRST_SECS: i64 = 300;

/// The longest wait after failed runs, in seconds (6 hours).
pub const BACKOFF_MAX_SECS: i64 = 6 * 3600;

/// How long a loop waits after `failed` failed runs in a row before it is due
/// again: 5 minutes after the first, doubled for each further one, at most 6
/// hours. Nothing after none.
pub fn backoff_secs(failed: u32) -> i64 {
    if failed == 0 {
        return 0;
    }
    // 300 s shifted by 7 is already past the cap, so a longer run of
    // failures cannot overflow the shift.
    let doublings = (failed - 1).min(7);
    (BACKOFF_FIRST_SECS << doublings).min(BACKOFF_MAX_SECS)
}

/// How every [`backoff_reason`] starts, so a reader can tell the wait after
/// failed runs from the other reasons a loop is not due.
pub const BACKOFF_REASON_PREFIX: &str = "backing off after ";

/// The status reason while a loop waits after `failed` failed runs in a row.
pub fn backoff_reason(failed: u32) -> String {
    match failed {
        1 => format!("{BACKOFF_REASON_PREFIX}1 failed run"),
        n => format!("{BACKOFF_REASON_PREFIX}{n} failed runs"),
    }
}

/// One of the two loops a runner works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loop {
    /// Reflection threads, feedback and page updates.
    Reflect,
    /// Maintenance findings.
    Evolve,
}

impl Loop {
    /// The wire word: `reflect` or `evolve`.
    pub fn as_str(self) -> &'static str {
        match self {
            Loop::Reflect => "reflect",
            Loop::Evolve => "evolve",
        }
    }

    /// The word a title starts with: `Reflect` or `Evolve`.
    pub fn title_word(self) -> &'static str {
        match self {
            Loop::Reflect => "Reflect",
            Loop::Evolve => "Evolve",
        }
    }
}

impl fmt::Display for Loop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Loop {
    type Err = String;

    fn from_str(s: &str) -> Result<Loop, String> {
        match s.trim() {
            "reflect" => Ok(Loop::Reflect),
            "evolve" => Ok(Loop::Evolve),
            other => Err(format!(
                "unknown loop '{other}'; valid loops: reflect, evolve"
            )),
        }
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// It worked everything it could; what it did not settle is left for a
    /// person.
    Done,
    /// It stopped at its item cap; the rest stays due.
    StoppedAtCap,
    /// It failed; everything stays due.
    Failed,
    /// It found nothing due.
    NothingDue,
    /// Its runner stopped renewing the lease.
    Expired,
}

impl RunOutcome {
    /// The wire word, as serde writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            RunOutcome::Done => "done",
            RunOutcome::StoppedAtCap => "stopped_at_cap",
            RunOutcome::Failed => "failed",
            RunOutcome::NothingDue => "nothing_due",
            RunOutcome::Expired => "expired",
        }
    }
}

/// What a runner reports when a run ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    /// When the run started by the runner's clock, RFC 3339, or empty. A
    /// value is checked to read as a time and is not kept: the server
    /// records the run as started when its lease was taken, on its own
    /// clock, which is what decides whether a run missed a page.
    pub started_at: String,
    /// How it ended.
    pub outcome: RunOutcome,
    /// Items done.
    pub done: u64,
    /// The run summary; kept up to [`SUMMARY_MAX_BYTES`].
    pub summary: String,
}

/// `text` cut to at most `max` bytes, at a char boundary.
pub fn cap_summary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The `X-Crystalline-Signature` value of a trigger webhook:
/// `sha256=<hex>`, the lowercase hex HMAC-SHA256 under `secret` of
/// `timestamp`, a dot and `body`. The timestamp is the one the
/// `X-Crystalline-Timestamp` header carries, so a receiver that checks its
/// age refuses a replay.
pub fn webhook_signature(secret: &[u8], timestamp: &str, body: &[u8]) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    let code = mac.finalize().into_bytes();
    let mut out = String::with_capacity(7 + code.len() * 2);
    out.push_str("sha256=");
    for byte in code {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The reflect loop's shipped prompt, with `{skills}`, `{domain}` and
/// `{max_items}` to fill in.
const REFLECT_PROMPT: &str = include_str!("automation/reflect-prompt.md");

/// The evolve loop's shipped prompt, with `{skills}`, `{domain}` and
/// `{max_items}` to fill in.
const EVOLVE_PROMPT: &str = include_str!("automation/evolve-prompt.md");

/// The MCP tools a headless run may never call, by name: each needs a yes,
/// or shares, pulls or throws away work a person owns. Both prompts name
/// exactly these, and a runner passes the same list to its harness (Claude
/// Code's `--disallowedTools`). Two more rules are parameters, not tools,
/// so they live in the prompts only: the `write_reflection` action
/// `delete_asset`, `write_asset` with `overwrite: true` and `write_engram`
/// with `overwrite: true` (which covers an attachment replace too).
pub const HEADLESS_DISALLOWED_TOOLS: &[&str] = &[
    "delete_engram",
    "move_engram",
    "remove_domain",
    "add_domain",
    "configure",
    "provision",
    "share_changes",
    "update_domain",
    "resolve_conflict",
    "withdraw_proposal",
    "discard_changes",
];

/// The item cap of a headless run when nobody names one: the CLI's
/// `--max-items` default and the `automation_prompt` verb's.
pub const DEFAULT_MAX_ITEMS: u32 = 20;

/// The words a prompt opens its start with when the agent's skills are
/// already loaded, in place of the call to the `skills` tool.
pub const SKILLS_LOADED_LINE: &str = "Your skills are already loaded";

/// The shipped skills a headless run of `lp` works with, in the order its
/// prompt names them: the ones the prompt has the agent read through the
/// `skills` tool, or says are loaded, and the ones `crystalline <loop> run`
/// hands its agent itself when the prompt says so ([`loaded_skills_text`]).
pub fn loop_skills(lp: Loop) -> &'static [&'static str] {
    match lp {
        Loop::Reflect => &["crystalline-intelligence", "crystalline-presentation"],
        Loop::Evolve => &["crystalline-intelligence"],
    }
}

/// The text of [`loop_skills`] as this binary ships them, for a runner that
/// hands the skills to its agent itself when the prompt says they are
/// loaded (`crystalline <loop> run` appends it to Claude Code's system
/// prompt). Each skill opens with a line that names it.
pub fn loaded_skills_text(lp: Loop) -> String {
    let mut out = String::from(
        "The Crystalline skills below are loaded for this run. Where the run's prompt says something else, the prompt wins.\n",
    );
    for name in loop_skills(lp) {
        let skill = crate::skills::skill(name).expect("every loop skill ships");
        out.push_str(&format!(
            "\n# Skill `{name}`\n\n{}\n",
            skill.content.trim_end()
        ));
    }
    out
}

/// The sentence the start of `lp`'s prompt opens with: the call to the
/// `skills` tool, or with `skills_loaded` the words that say they are there.
/// The rest of that paragraph (what the skills teach and that this prompt
/// wins over them) stays the same either way.
fn skills_sentence(lp: Loop, skills_loaded: bool) -> String {
    let skills = loop_skills(lp)
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    match (lp, skills_loaded) {
        (_, true) => format!("{SKILLS_LOADED_LINE}: {skills}."),
        (Loop::Reflect, false) => "Start: call the `skills` tool with name `crystalline-intelligence` and read it, then with name `crystalline-presentation`.".to_string(),
        (Loop::Evolve, false) => "Start: call the `skills` tool with name `crystalline-intelligence` and read it.".to_string(),
    }
}

/// Whether a headless prompt says the skills are already loaded instead of
/// calling the `skills` tool: what the request names when it names it,
/// else whether the server hides the `skills` tool from a headless run.
/// Only `skills.serve: false` hides it there: under `auto` an unattended
/// session and an HTTP session are both served the full surface.
pub fn skills_loaded(serve: crate::config::SkillsServe, requested: Option<bool>) -> bool {
    requested.unwrap_or(serve == crate::config::SkillsServe::Never)
}

/// The shipped prompt a headless run of `lp` starts its agent with, for
/// `domain` and at most `max_items` items. With `skills_loaded` its start
/// says the skills are already loaded instead of calling the `skills` tool
/// (see [`skills_loaded`]). It ends with the block [`parse_run_summary`]
/// reads.
pub fn headless_prompt(lp: Loop, domain: &str, max_items: u32, skills_loaded: bool) -> String {
    let template = match lp {
        Loop::Reflect => REFLECT_PROMPT,
        Loop::Evolve => EVOLVE_PROMPT,
    };
    template
        .replace("{skills}", &skills_sentence(lp, skills_loaded))
        .replace("{max_items}", &max_items.to_string())
        .replace("{domain}", domain)
}

/// What a runner that starts its own agent sets it up with: the answer of
/// the `automation_prompt` verb and of `<loop> prompt --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunnerSetup {
    /// The filled-in prompt, as [`headless_prompt`] gives it.
    pub prompt: String,
    /// [`HEADLESS_DISALLOWED_TOOLS`], in order: the bare MCP tool names the
    /// agent's harness must refuse.
    pub disallowed_tools: Vec<String>,
    /// The Crystalline version that built this answer.
    pub version: String,
    /// [`setup_checksum`] of the prompt and the tools, so a runner notices
    /// when either changed.
    pub checksum: String,
}

/// The [`RunnerSetup`] of a headless run of `lp` on `domain` with at most
/// `max_items` items.
pub fn runner_setup(lp: Loop, domain: &str, max_items: u32, skills_loaded: bool) -> RunnerSetup {
    let prompt = headless_prompt(lp, domain, max_items, skills_loaded);
    let checksum = setup_checksum(&prompt, HEADLESS_DISALLOWED_TOOLS);
    RunnerSetup {
        prompt,
        disallowed_tools: HEADLESS_DISALLOWED_TOOLS
            .iter()
            .map(|t| t.to_string())
            .collect(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        checksum,
    }
}

/// `sha256:<hex>`: the lowercase hex SHA-256 of the prompt's UTF-8 bytes,
/// one NUL byte, then the tool names joined by a line feed (`\n`), with
/// nothing after the last one. Neither a prompt nor a tool name holds a
/// NUL, so a change to either part, or to the order of the tools, changes
/// the sum. The version is not part of it: an upgrade that changes neither
/// keeps the sum.
pub fn setup_checksum<S: AsRef<str>>(prompt: &str, tools: &[S]) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(prompt.as_bytes());
    hasher.update([0u8]);
    for (i, tool) in tools.iter().enumerate() {
        if i > 0 {
            hasher.update(b"\n");
        }
        hasher.update(tool.as_ref().as_bytes());
    }
    let mut out = String::from("sha256:");
    for byte in hasher.finalize() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The fixed block a headless run's agent ends with, read into numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    /// Items the run settled.
    pub done: u64,
    /// Items it left for a person.
    pub left: u64,
    /// Errors it met.
    pub errors: u64,
    /// The free text under the three counts.
    pub text: String,
}

/// The summary block at the end of `last_message`, or `None` without one.
/// The block is the last `Done: <n>`, `Left for a person: <n>` and
/// `Errors: <n>` lines in that order (blank lines between them, spaces
/// around them, a code fence around the block, markdown bold or italics, a
/// leading list marker and words after the number are fine); the lines
/// under it are its text.
///
/// The block carries no cap signal: a runner records a run whose `done`
/// reached its `max_items` as stopped at the cap, not as done.
pub fn parse_run_summary(last_message: &str) -> Option<RunSummary> {
    let lines: Vec<&str> = last_message.lines().map(str::trim).collect();
    (0..lines.len()).rev().find_map(|at| {
        let done = count(lines[at], "Done")?;
        let (left_at, left) = next_count(&lines, at + 1, "Left for a person")?;
        let (errors_at, errors) = next_count(&lines, left_at + 1, "Errors")?;
        let text: Vec<&str> = lines[errors_at + 1..]
            .iter()
            .copied()
            .filter(|line| !line.starts_with("```"))
            .collect();
        Some(RunSummary {
            done,
            left,
            errors,
            text: text.join("\n").trim().to_string(),
        })
    })
}

/// The number on a `<key>: <n>` line, or `None` for any other line. Bold
/// and italic marks and a leading `-` or `+` list marker are ignored, and
/// the number is the value's leading digits.
fn count(line: &str, key: &str) -> Option<u64> {
    let plain: String = line.chars().filter(|c| !matches!(c, '*' | '_')).collect();
    let plain = plain.trim();
    let plain = plain
        .strip_prefix("- ")
        .or_else(|| plain.strip_prefix("+ "))
        .unwrap_or(plain);
    let (name, value) = plain.split_once(':')?;
    if !name.trim().eq_ignore_ascii_case(key) {
        return None;
    }
    let value = value.trim_start();
    let digits = value.len() - value.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    value[..digits].parse().ok()
}

/// The first line from `from` on that is not blank, when it is a `<key>: <n>`
/// line: its index and its number.
fn next_count(lines: &[&str], from: usize, key: &str) -> Option<(usize, u64)> {
    let at = (from..lines.len()).find(|&i| !lines[i].is_empty())?;
    Some((at, count(lines[at], key)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The n8n loop workflows exclude exactly the tools a headless run may
    /// not call, and their guard stops a run when the server forbids another
    /// list: a new forbidden tool fails this test instead of drifting.
    #[test]
    fn the_n8n_workflows_exclude_and_guard_exactly_the_headless_tools() {
        let joined = HEADLESS_DISALLOWED_TOOLS.join(",");
        for (name, text) in [
            (
                "reflect",
                include_str!("../../../examples/n8n/reflect-loop.json"),
            ),
            (
                "evolve",
                include_str!("../../../examples/n8n/evolve-loop.json"),
            ),
        ] {
            let workflow: serde_json::Value = serde_json::from_str(text).unwrap();
            let node = |wanted: &str| {
                workflow["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|n| n["name"] == wanted)
                    .unwrap_or_else(|| panic!("{name}: no node {wanted}"))
                    .clone()
            };
            let mcp = node("Crystalline MCP");
            let excluded: Vec<&str> = mcp["parameters"]["excludeTools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap())
                .collect();
            assert_eq!(excluded, HEADLESS_DISALLOWED_TOOLS, "{name}");
            let guard = node("Same forbidden tools?");
            let condition = guard["parameters"]["conditions"]["conditions"][0]["leftValue"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(
                condition.contains(&format!("=== '{joined}'")),
                "{name}: {condition}"
            );
            let connections = &workflow["connections"];
            assert_eq!(
                connections["Prompt fetched?"]["main"][0][0]["node"], "Same forbidden tools?",
                "{name}"
            );
            assert_eq!(
                connections["Same forbidden tools?"]["main"][0][0]["node"], "Status",
                "{name}"
            );
            assert_eq!(
                connections["Same forbidden tools?"]["main"][1][0]["node"],
                "Forbidden tools changed",
                "{name}"
            );
        }
    }

    /// The skills a loop's prompt names are the ones a run hands over, and
    /// the handed text carries each shipped skill whole.
    #[test]
    fn the_handed_skills_are_the_ones_the_prompt_names() {
        for lp in [Loop::Reflect, Loop::Evolve] {
            let prompt = headless_prompt(lp, "eng", 5, true);
            let text = loaded_skills_text(lp);
            for name in loop_skills(lp) {
                assert!(prompt.contains(&format!("`{name}`")), "{name}");
                let skill = crate::skills::skill(name).unwrap();
                assert!(text.contains(skill.content.trim_end()), "{name}");
                assert!(text.contains(&format!("# Skill `{name}`")), "{name}");
            }
        }
        assert_eq!(
            headless_prompt(Loop::Reflect, "eng", 5, true)
                .lines()
                .find(|l| l.starts_with(SKILLS_LOADED_LINE))
                .unwrap()
                .split(" They teach")
                .next()
                .unwrap(),
            "Your skills are already loaded: `crystalline-intelligence` and `crystalline-presentation`."
        );
    }

    #[test]
    fn the_backoff_doubles_from_five_minutes_up_to_six_hours() {
        assert_eq!(backoff_secs(0), 0);
        assert_eq!(backoff_secs(1), 300);
        assert_eq!(backoff_secs(2), 600);
        assert_eq!(backoff_secs(3), 1200);
        assert_eq!(backoff_secs(7), 19200);
        assert_eq!(backoff_secs(8), 21600);
        assert_eq!(backoff_secs(50), 21600);
        assert_eq!(backoff_reason(1), "backing off after 1 failed run");
        assert_eq!(backoff_reason(3), "backing off after 3 failed runs");
    }

    #[test]
    fn a_loop_reads_and_writes_its_word() {
        for lp in [Loop::Reflect, Loop::Evolve] {
            assert_eq!(lp.as_str().parse::<Loop>().unwrap(), lp);
            assert_eq!(serde_json::to_value(lp).unwrap(), lp.as_str());
        }
        assert_eq!(
            "maintain".parse::<Loop>().unwrap_err(),
            "unknown loop 'maintain'; valid loops: reflect, evolve"
        );
    }

    #[test]
    fn an_outcome_writes_the_word_serde_writes() {
        for outcome in [
            RunOutcome::Done,
            RunOutcome::StoppedAtCap,
            RunOutcome::Failed,
            RunOutcome::NothingDue,
            RunOutcome::Expired,
        ] {
            assert_eq!(serde_json::to_value(outcome).unwrap(), outcome.as_str());
        }
    }

    #[test]
    fn a_summary_is_cut_at_a_char_boundary() {
        assert_eq!(cap_summary("short", 2048), "short");
        let long = "ä".repeat(1100);
        let cut = cap_summary(&long, SUMMARY_MAX_BYTES);
        assert_eq!(cut.len(), 2048);
        assert_eq!(cap_summary("aä", 2), "a");
    }

    const BLOCK: &str = "Done: <n>\nLeft for a person: <n>\nErrors: <n>\n<free text>";

    fn both() -> [(Loop, String); 2] {
        [
            (
                Loop::Reflect,
                headless_prompt(Loop::Reflect, "work", 7, false),
            ),
            (
                Loop::Evolve,
                headless_prompt(Loop::Evolve, "work", 7, false),
            ),
        ]
    }

    #[test]
    fn a_prompt_fills_in_the_domain_and_the_cap() {
        for (lp, prompt) in both() {
            assert!(!prompt.contains("{domain}"), "{lp} keeps {{domain}}");
            assert!(!prompt.contains("{max_items}"), "{lp} keeps {{max_items}}");
            assert!(prompt.contains("`work`"), "{lp} lacks the domain");
            assert!(prompt.contains("7 items"), "{lp} lacks the cap");
        }
    }

    #[test]
    fn a_prompt_loads_the_skills_before_any_other_tool() {
        for (lp, prompt) in both() {
            let skills = prompt.find("`skills`").unwrap_or(usize::MAX);
            for tool in [
                "`reflect`",
                "`evolve_engrams`",
                "`read_reflection`",
                "`write_reflection`",
                "`edit_engram`",
                "`discuss`",
            ] {
                if let Some(at) = prompt.find(tool) {
                    assert!(skills < at, "{lp} names {tool} before `skills`");
                }
            }
            assert!(skills < usize::MAX, "{lp} never names `skills`");
        }
    }

    #[test]
    fn a_prompt_never_waits_for_a_yes() {
        for (lp, prompt) in both() {
            assert!(prompt.contains("Nobody is there to answer"), "{lp}");
            assert!(prompt.contains("Never wait for a yes"), "{lp}");
            assert!(prompt.contains("never in this session"), "{lp}");
        }
    }

    #[test]
    fn reflect_asks_in_threads_and_evolve_in_the_summary() {
        let reflect = headless_prompt(Loop::Reflect, "work", 7, false);
        assert!(
            reflect.contains("Questions for people go into a Reflection thread with `discuss`")
        );
        let evolve = headless_prompt(Loop::Evolve, "work", 7, false);
        assert!(evolve.contains("Name every question for people in the summary"));
        assert!(evolve.contains("never in a Reflection thread"));
        assert!(!evolve.contains("`discuss`"));
    }

    #[test]
    fn a_prompt_ends_its_queue_when_only_worked_items_come_back() {
        for (lp, prompt) in both() {
            assert!(!prompt.contains("until it is empty"), "{lp}");
            assert!(
                prompt.contains("already worked or left for a person"),
                "{lp}"
            );
            assert!(
                prompt.contains("counts once toward the cap, also when it comes back"),
                "{lp}"
            );
        }
    }

    #[test]
    fn a_prompt_forbids_exactly_the_tools_a_runner_disallows() {
        for (lp, prompt) in both() {
            let rule = prompt
                .lines()
                .find(|line| line.trim_start_matches("- ").starts_with("Do not use"))
                .unwrap_or_else(|| panic!("{lp} has no Do not use line"));
            let (tools, params) = rule.split_once(". ").unwrap();
            let named: Vec<&str> = tools.split('`').skip(1).step_by(2).collect();
            assert_eq!(named, HEADLESS_DISALLOWED_TOOLS, "{lp}");
            for needle in [
                "`write_reflection` action `delete_asset`",
                "`write_reflection` action `write_asset` with `overwrite: true`",
                "`write_engram` with `overwrite: true`",
            ] {
                assert!(params.contains(needle), "{lp} does not forbid {needle}");
            }
        }
    }

    #[test]
    fn a_prompt_forbids_the_tools_that_need_a_yes() {
        for (lp, prompt) in both() {
            let rule = prompt
                .lines()
                .find(|line| line.trim_start_matches("- ").starts_with("Do not use"))
                .unwrap_or_else(|| panic!("{lp} has no Do not use line"));
            for needle in [
                "`delete_engram`",
                "`move_engram`",
                "`remove_domain`",
                "`write_reflection` action `delete_asset`",
            ] {
                assert!(rule.contains(needle), "{lp} does not forbid {needle}");
            }
        }
    }

    #[test]
    fn a_prompt_keeps_every_change_in_the_run() {
        for (lp, prompt) in both() {
            assert!(prompt.contains("through the crystalline MCP tools"), "{lp}");
            assert!(prompt.contains("pull request"), "{lp}");
            assert!(prompt.contains("Do not share"), "{lp}");
            assert!(prompt.contains("`[crystalline]`"), "{lp}");
        }
    }

    #[test]
    fn a_prompt_stops_at_the_cap_and_leaves_a_rename() {
        for (lp, prompt) in both() {
            assert!(prompt.contains("Stop after 7 items"), "{lp}");
            assert!(prompt.contains("rename"), "{lp}");
            assert!(prompt.contains("Leave it for a person"), "{lp}");
        }
    }

    #[test]
    fn a_prompt_ends_with_the_summary_block() {
        for (lp, prompt) in both() {
            assert!(prompt.trim_end().ends_with(BLOCK), "{lp} ends otherwise");
            assert!(
                !prompt.contains('\u{2014}') && !prompt.contains('\u{2013}'),
                "{lp} holds a long dash"
            );
        }
    }

    #[test]
    fn the_reflect_prompt_reads_the_pages_and_names_sources() {
        let prompt = headless_prompt(Loop::Reflect, "work", 7, false);
        for needle in [
            "`## Pages`",
            "`sources`",
            "settle `PACT.md`",
            "Your own writes never come back as feedback",
            "leave it open",
        ] {
            assert!(prompt.contains(needle), "reflect prompt lacks {needle}");
        }
        assert!(
            prompt
                .lines()
                .any(|line| line.contains("read the pact's `## Pages`")
                    && line.contains("new engrams")),
            "reflect prompt does not read ## Pages to place new engrams"
        );
    }

    /// A section built from tentative engrams shows a status chip, in the
    /// update step where sections are written with their sources.
    #[test]
    fn the_reflect_prompt_names_a_section_from_tentative_engrams() {
        let prompt = headless_prompt(Loop::Reflect, "work", 7, false);
        let updates = prompt
            .lines()
            .find(|line| line.starts_with("3. Updates:"))
            .expect("the update step");
        assert!(
            updates.contains("A section built from tentative engrams (status draft, proposed, idea, poc or open) shows a status chip, for example `:status[Proposed]{purple}`."),
            "{updates}"
        );
        assert!(
            updates.contains("A chip is text, so it stays on a page whose PACT allows only plain text and tables, unless the PACT says no chips."),
            "{updates}"
        );
    }

    /// Reflection pages are read by people: every section is written and
    /// changed with the presentation skill, read with the `skills` tool when
    /// the harness has not loaded it, and the PACT's `## Presentation` wins
    /// over it. The rule reads the same whether the start calls the `skills`
    /// tool or says the skills are loaded, and neither start contradicts it.
    #[test]
    fn the_reflect_prompt_writes_pages_for_people_with_the_presentation_skill() {
        for loaded in [false, true] {
            let prompt = headless_prompt(Loop::Reflect, "work", 7, loaded);
            let rule = prompt
                .lines()
                .find(|line| line.starts_with("- Reflection pages are read by people."))
                .expect("a rule on the readers of a page");
            for needle in [
                "every section",
                "`crystalline-presentation` skill (if it is not loaded, read it with the `skills` tool, name `crystalline-presentation`)",
                "`## Presentation`",
                "wins over the skill",
            ] {
                assert!(
                    rule.contains(needle),
                    "loaded={loaded}: the readers rule lacks {needle}: {rule}"
                );
            }
            let loaded_start = "Your skills are already loaded: `crystalline-intelligence` and `crystalline-presentation`.";
            let tool_start = "Start: call the `skills` tool with name `crystalline-intelligence` and read it, then with name `crystalline-presentation`.";
            let (start, other) = if loaded {
                (loaded_start, tool_start)
            } else {
                (tool_start, loaded_start)
            };
            assert!(prompt.contains(start), "loaded={loaded}: {prompt}");
            assert!(!prompt.contains(other), "loaded={loaded}: {prompt}");
            // The rule's fallback is conditional, so it never orders a
            // `skills` call where the start says the skills are loaded.
            assert!(rule.contains("if it is not loaded"), "{rule}");
        }
    }

    /// A page whose freshness ran out is reconciled with its pacts (design
    /// 3.5): the chain run, the readable domains searched, only what new
    /// information requires changed, every source named, owned pages created
    /// with `owner`, retired and never deleted, and `reconciled` recorded
    /// with the item's version, owned pages first and the owner last, each
    /// page one item toward the cap.
    #[test]
    fn the_reflect_prompt_reconciles_a_stale_page() {
        let prompt = headless_prompt(Loop::Reflect, "work", 7, false);
        let reconcile = prompt
            .lines()
            .find(|line| line.contains("`stale`") && line.contains("freshness ran out"))
            .expect("a reconcile step for stale items");
        for needle in [
            "pacts",
            "search the domains",
            "change only what new information requires",
            "name every source",
            "`owner`",
            "retire",
            "never delete",
            "`reconciled`",
            "`version`",
            "`changed_engrams`",
            "`unchanged`",
            "null",
            "owned pages first and the owner last",
            "`owned_reconciled`",
            "`refresh_requested`",
            "`owned_marked`",
            "never takes that shortcut",
            "presentation rules",
            "`still_marked`",
        ] {
            assert!(
                reconcile.contains(needle),
                "the reconcile step lacks {needle}: {reconcile}"
            );
        }
        let other = prompt
            .lines()
            .find(|line| line.contains("`page_pact_changed`"))
            .expect("a step for the other page items");
        assert!(
            other.contains("`pact_hash_changed`") && other.contains("record `reconciled`"),
            "a page_pact_changed with no changed pact text is settled with reconciled: {other}"
        );
        let cap = prompt
            .lines()
            .find(|line| line.starts_with("- Stop after"))
            .expect("the cap rule");
        assert!(
            cap.contains("the owner and each owned page count as one item each"),
            "{cap}"
        );
    }

    #[test]
    fn the_reflect_prompt_leaves_pact_text_and_page_settings_to_a_curator() {
        let prompt = headless_prompt(Loop::Reflect, "work", 7, false);
        let rule = prompt
            .lines()
            .find(|line| line.starts_with("- ") && line.contains("pact text"))
            .expect("a rule on pact text");
        for needle in [
            "`PACT.md`",
            "page pact",
            "`freshness`",
            "`follows`",
            "move a page",
            "curator",
            "thread",
            "an item on `PACT.md` or a page pact",
            "left for a person",
        ] {
            assert!(rule.contains(needle), "the rule lacks {needle}: {rule}");
        }
        let feedback = prompt
            .lines()
            .find(|line| line.starts_with("2. Feedback"))
            .expect("the feedback step");
        assert!(
            feedback.contains("pact file") && feedback.contains("never write pact text"),
            "{feedback}"
        );
        let updates = prompt
            .lines()
            .find(|line| line.starts_with("3. Updates"))
            .expect("the update step");
        assert!(updates.contains("own page pact"), "{updates}");
        let threads = prompt
            .lines()
            .find(|line| line.starts_with("6. Threads"))
            .expect("the thread step");
        assert!(
            threads.contains("pact") && threads.contains("curator"),
            "{threads}"
        );
        // Nothing asks a run to add to a pact.
        assert!(!prompt.contains("add a bullet"), "{prompt}");
    }

    #[test]
    fn a_reflect_run_refused_an_engram_write_answers_the_feedback_another_way() {
        let prompt = headless_prompt(Loop::Reflect, "work", 7, false);
        let line = prompt
            .lines()
            .find(|line| line.starts_with("2. Feedback"))
            .expect("the feedback step");
        // The first way stays for a run that may write the engrams.
        assert!(line.contains("Take it into the engrams"), "{line}");
        assert!(
            line.contains("When an engram write is refused in this run")
                && line.contains("answer the feedback in a thread or settle it as kept")
                && line.contains("say so in your summary"),
            "{line}"
        );
    }

    #[test]
    fn the_evolve_prompt_asks_for_mechanical_findings_only() {
        let prompt = headless_prompt(Loop::Evolve, "work", 7, false);
        for needle in [
            "class: \"mechanical\"",
            "`judgment`",
            "`evolve_ack`",
            "domains: [\"work\"]",
        ] {
            assert!(prompt.contains(needle), "evolve prompt lacks {needle}");
        }
    }

    #[test]
    fn the_shipped_prompt_calls_the_skills_tool_and_a_loaded_one_does_not() {
        for lp in [Loop::Reflect, Loop::Evolve] {
            let shipped = headless_prompt(lp, "work", 7, false);
            assert!(!shipped.contains("{skills}"), "{lp} keeps {{skills}}");
            assert!(
                shipped.contains(
                    "\n\nStart: call the `skills` tool with name `crystalline-intelligence`"
                ),
                "{lp}"
            );
            let loaded = headless_prompt(lp, "work", 7, true);
            assert!(!loaded.contains("{skills}"), "{lp} keeps {{skills}}");
            assert!(!loaded.contains("call the `skills` tool"), "{lp}");
            assert!(
                loaded.contains("\n\nYour skills are already loaded"),
                "{lp} lacks the loaded line"
            );
            // The rule that the prompt wins over a skill stays either way.
            assert!(loaded.contains("this prompt wins"), "{lp}");
            assert_eq!(
                shipped.lines().count(),
                loaded.lines().count(),
                "{lp} changes more than the one line"
            );
        }
    }

    #[test]
    fn the_skills_line_follows_the_setting_unless_the_request_names_it() {
        use crate::config::SkillsServe;
        for serve in [SkillsServe::Auto, SkillsServe::Always] {
            assert!(!skills_loaded(serve, None), "{serve:?}");
        }
        assert!(skills_loaded(SkillsServe::Never, None));
        for serve in [SkillsServe::Auto, SkillsServe::Always, SkillsServe::Never] {
            assert!(skills_loaded(serve, Some(true)), "{serve:?}");
            assert!(!skills_loaded(serve, Some(false)), "{serve:?}");
        }
    }

    #[test]
    fn a_runner_setup_carries_the_prompt_the_tools_the_version_and_the_checksum() {
        let setup = runner_setup(Loop::Evolve, "work", 7, false);
        assert_eq!(
            setup.prompt,
            headless_prompt(Loop::Evolve, "work", 7, false)
        );
        assert_eq!(setup.disallowed_tools, HEADLESS_DISALLOWED_TOOLS);
        assert_eq!(setup.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            setup.checksum,
            setup_checksum(&setup.prompt, HEADLESS_DISALLOWED_TOOLS)
        );
        let value = serde_json::to_value(&setup).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["checksum", "disallowed_tools", "prompt", "version"]);
    }

    #[test]
    fn the_checksum_is_sha256_of_the_prompt_a_nul_and_the_tools_one_per_line() {
        use sha2::Digest as _;
        let tools = ["a_tool", "b_tool"];
        let digest = sha2::Sha256::digest(b"the prompt\0a_tool\nb_tool");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            setup_checksum("the prompt", &tools),
            format!("sha256:{hex}")
        );
        // The same input hashes the same; a change to either part changes it.
        let base = setup_checksum("the prompt", &tools);
        assert_eq!(base, setup_checksum("the prompt", &tools));
        assert_ne!(base, setup_checksum("the prompt.", &tools));
        assert_ne!(base, setup_checksum("the prompt", &["a_tool"]));
        assert_ne!(
            base,
            setup_checksum("the prompt", &["a_tool", "b_tool", "c_tool"])
        );
        assert_ne!(base, setup_checksum("the prompt", &["b_tool", "a_tool"]));
        // The skills line is part of the prompt, so it is part of the sum.
        assert_ne!(
            runner_setup(Loop::Reflect, "work", 7, false).checksum,
            runner_setup(Loop::Reflect, "work", 7, true).checksum
        );
    }

    #[test]
    fn a_summary_block_is_read_from_the_end_of_the_message() {
        let message = "I answered two threads.\n\nDone: 3\nLeft for a person: 1\nErrors: 0\nThe thread on Setup needs a person.\nNo rename.";
        assert_eq!(
            parse_run_summary(message),
            Some(RunSummary {
                done: 3,
                left: 1,
                errors: 0,
                text: "The thread on Setup needs a person.\nNo rename.".to_string(),
            })
        );
    }

    #[test]
    fn a_summary_block_takes_extra_whitespace_and_a_fence() {
        let message = "Work done.\r\n```\r\n  Done:3  \r\n\r\nLeft for a person:   2\r\n\r\n   Errors: 1\r\n\r\n  One write failed.  \r\n```\r\n";
        assert_eq!(
            parse_run_summary(message),
            Some(RunSummary {
                done: 3,
                left: 2,
                errors: 1,
                text: "One write failed.".to_string(),
            })
        );
    }

    #[test]
    fn the_last_summary_block_wins() {
        let message = "Done: 1\nLeft for a person: 1\nErrors: 1\nan example\n\nDone: 4\nLeft for a person: 0\nErrors: 0\n";
        let summary = parse_run_summary(message).unwrap();
        assert_eq!((summary.done, summary.left, summary.errors), (4, 0, 0));
        assert_eq!(summary.text, "");
    }

    #[test]
    fn a_summary_block_takes_bold_and_list_markers() {
        let message =
            "**Done:** 2\n- **Left for a person:** 1\n* Errors: **0**\nThe Setup thread waits.";
        assert_eq!(
            parse_run_summary(message),
            Some(RunSummary {
                done: 2,
                left: 1,
                errors: 0,
                text: "The Setup thread waits.".to_string(),
            })
        );
    }

    #[test]
    fn a_summary_count_reads_the_leading_digits() {
        let message = "Done: 3 (two threads, one page)\nLeft for a person: 1 thread\nErrors: 0.\n";
        let summary = parse_run_summary(message).unwrap();
        assert_eq!((summary.done, summary.left, summary.errors), (3, 1, 0));
    }

    #[test]
    fn a_message_without_the_block_has_no_summary() {
        for message in [
            "",
            "All done, nothing left.",
            "Done: 3\nErrors: 0\n",
            "Done: three\nLeft for a person: 0\nErrors: 0\n",
            "Done: 3\nLeft for a person: 0\n",
            "Left for a person: 0\nDone: 3\nErrors: 0\n",
        ] {
            assert_eq!(parse_run_summary(message), None, "{message:?}");
        }
    }
}
