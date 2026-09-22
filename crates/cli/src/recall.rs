//! `crystalline hook prompt`: the per-prompt recall of engrams that may apply.
//!
//! A harness (Claude Code, Codex and Copilot, though Copilot's copy is
//! written and left inert - a config-file prompt hook's output is dropped
//! there) wires this to its `UserPromptSubmit`
//! lifecycle event, feeding it a small JSON payload over stdin. On a prompt
//! that carries a subject, and only when a daemon is already running, it asks
//! that daemon for exactly the hybrid search an agent would have run and
//! hands back a short block naming the few engrams that may apply, each by
//! its `crystalline://` address, once per session per engram. Every other
//! call is silent: exit 0, empty stdout. This is not a style preference, it
//! is a correctness requirement - a harness that sees any other output on a
//! lifecycle hook's stdout can misinterpret it, so a bail path never prints a
//! word, logs a warning or returns a nonzero exit code. A hook must never be
//! the reason a harness's turn breaks.
//!
//! The bail order, every one of them silent:
//!
//! 1. stdin is unreadable or is not the expected JSON object;
//! 2. the payload names an event other than `UserPromptSubmit`;
//! 3. the session id fails [`crate::hook::valid_session_id`] (a traversal
//!    attempt must never turn into a filesystem path);
//! 4. the prompt fails [`gate_prompt`];
//! 5. the config overlay fails to load, `recall.enabled` is false, or no
//!    domain is registered;
//! 6. no daemon answers (the attach is passive, see below);
//! 7. the search fails, answers in text mode, or does not answer inside
//!    [`RECALL_BUDGET`];
//! 8. nothing survives the floor, the retirement cut and the dedupe.
//!
//! Read-only is not a bail: `service.read_only` removes the write tools, and
//! recall is a read. A read-only deployment's knowledge is curated for
//! exactly this use.
//!
//! The daemon is attached through
//! [`crystalline_service::instance::try_attach_passive`], which never spawns
//! one, never displaces one and never opens the index in this process: a
//! takeover costs seconds and loading the embedding model into a process that
//! lives for one prompt costs hundreds of milliseconds and hundreds of
//! megabytes. No daemon is silence. In practice a harness session has one for
//! its whole life, started by the `crystalline mcp` bridge the harness spawned.
//!
//! State is the Stop hook's file, `<state_dir>/hooks/<session_id>.json`: this
//! handler appends to [`crate::hook::SessionState::recalled`] and carries
//! `stops` and `nudged` through untouched, exactly as the Stop hook carries
//! the list through its own rewrite. It is written before the block is
//! printed, the same crash-safety order the Stop hook keeps: a crash between
//! the two costs one engram never shown again this session, never the same
//! engram twice. The list is emptied when the session's context is, on
//! `/clear` and on a compaction, through [`crate::hook::reset_recalled`].
//!
//! The whole attach and exchange runs under [`RECALL_BUDGET`], one second.
//! What runs outside it is bounded on its own: stdin is capped at a megabyte,
//! the overlay load is one file, the state file is a few kilobytes. Nothing
//! is retried, and a budget that runs out leaves nothing written and nothing
//! printed - a block that would arrive late is a block that shows the same
//! engrams next prompt instead.

use std::io::Read as _;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crystalline_core::HarnessKind;

use crate::hook::{self, SessionState};

/// How many whitespace-separated words a prompt needs before a search is
/// worth running. The lexical half of hybrid ranking ANDs every term, so a
/// one- or two-word prompt is exactly what turns a lexical match on a common
/// word into a false hit; "yes" and "ok go" carry no subject to recall
/// against either.
pub const MIN_PROMPT_WORDS: usize = 3;

/// How many characters a prompt needs, beside the word count. "run the tests"
/// clears three words and still names nothing to recall.
pub const MIN_PROMPT_CHARS: usize = 16;

/// Where a prompt is cut before it becomes the query. The embedder truncates
/// at 512 tokens anyway, and a pasted log should not cost a longer socket
/// write.
pub const MAX_QUERY_CHARS: usize = 1000;

/// How many hits the search is asked for: the cap plus headroom for what the
/// retirement cut, the floor and the dedupe remove.
pub const RECALL_PAGE: usize = 8;

/// Where a hit's snippet is cut. The store's own window is up to about 210
/// characters; 160 keeps a three-engram block near 200 tokens.
pub const SNIPPET_CHARS: usize = 160;

/// How many addresses one session's dedupe list keeps. A session that recalls
/// on every prompt for hours stays a few kilobytes of state; the oldest fall
/// off, which at worst shows a long-ago engram a second time.
pub const RECALLED_MAX: usize = 200;

/// The budget for the whole attach and exchange: reading the lock record,
/// connecting, the daemon's query embedding and search, and the reply. A
/// measured hybrid search on a 10,000-engram index costs 0.17 to 0.42 seconds
/// through a daemon that is also embedding, so one second is the bound that
/// keeps the person from noticing the hook at all. The harness-side timeout
/// is five seconds, headroom rather than a budget.
pub const RECALL_BUDGET: Duration = Duration::from_millis(1000);

/// The block's first line. Load-bearing in the way the Stop hook's nudge
/// reason is: it names the tool, says the address is the identifier (so the
/// agent never glues a domain onto a permalink), tells the agent to read
/// before relying on any of it and licenses it to ignore what does not fit.
pub const RECALL_HEADER: &str = "Knowledge that may apply, from your Crystalline domains. Read an engram with read_engram (pass the crystalline:// address as identifier) before relying on it; ignore what does not fit.";

/// The four retired statuses, dropped from a block whatever they rank: the
/// ranking only fades a retired engram, and unbidden context is the wrong
/// place to surface what is no longer true. Spelled here rather than
/// imported, because this crate does not depend on the index crate;
/// `crystalline_index::store::RETIRED_STATUSES` is the original and
/// [`the_retired_set_is_the_stores_own_four`] pins the two together.
pub const RETIRED: [&str; 4] = ["deprecated", "superseded", "archived", "legacy"];

/// The stdin payload a `UserPromptSubmit` hook sends. Every field carries a
/// serde default, so all three harness payload shapes parse and every field
/// this handler does not know about is ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct PromptInput {
    /// The harness's identifier for this session. Validated by
    /// [`crate::hook::valid_session_id`] before it ever becomes part of a
    /// filesystem path.
    #[serde(default)]
    pub session_id: String,
    /// The event name the harness attaches to the payload, when it does. A
    /// name other than `UserPromptSubmit` is a defensive bail; an absent name
    /// is treated as fine, since not every harness stamps it.
    #[serde(default)]
    pub hook_event_name: Option<String>,
    /// The prompt the person submitted.
    #[serde(default)]
    pub prompt: String,
}

/// One engram worth naming, already folded, cut and ready to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recalled {
    /// `crystalline://<domain>/<permalink>`, the identifier `read_engram`
    /// takes.
    pub address: String,
    /// The engram's title, printed as it is.
    pub title: String,
    /// The hit's snippet, whitespace-collapsed and cut at [`SNIPPET_CHARS`].
    /// Empty when the hit carried none, which drops the `: ` in the line.
    pub snippet: String,
}

/// Whether this prompt is worth a search, and the query if it is. Pure, so
/// the whole gate is decided before anything is loaded or connected.
pub fn gate_prompt(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() || trimmed.starts_with('/') {
        return None;
    }
    if trimmed.split_whitespace().count() < MIN_PROMPT_WORDS
        || trimmed.chars().count() < MIN_PROMPT_CHARS
    {
        return None;
    }
    Some(trimmed.chars().take(MAX_QUERY_CHARS).collect())
}

/// The control command: exactly what an agent's own `search_engrams` call
/// sends, so the ranking is the search tool's and nothing is built beside it.
/// No `domains` (an all-domain sweep), no `min_similarity` (the store
/// default) and no status filter.
pub fn search_request(query: &str) -> Value {
    json!({
        "v": 1,
        "cmd": "tool",
        "tool": "search_engrams",
        "args": { "query": query, "search_type": "hybrid", "limit": RECALL_PAGE },
    })
}

/// The pure decision core over the daemon's answer: the mode gate, the
/// retirement cut, the floor, the address fold, the dedupe and the cap, in
/// that order and in rank order throughout.
pub fn select_hits(
    search: &Value,
    shown: &[String],
    limit: usize,
    min_score: f64,
) -> Vec<Recalled> {
    // Text mode means the daemon has no provider or no vectors for its model
    // yet, and a text score is an unbounded term frequency: unrankable
    // against a floor and, on prose, noise. The literal is what
    // `engine.rs`'s `mode_str` emits (pinned by the service crate's own
    // `origin.rs` search test).
    if search.get("mode").and_then(Value::as_str) != Some("hybrid") {
        return Vec::new();
    }
    let mut out: Vec<Recalled> = Vec::new();
    for hit in search
        .get("hits")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let (Some(domain), Some(permalink)) = (
            hit.get("domain").and_then(Value::as_str),
            hit.get("permalink").and_then(Value::as_str),
        ) else {
            continue;
        };
        if hit
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| RETIRED.contains(&s))
        {
            continue;
        }
        if hit.get("score").and_then(Value::as_f64).unwrap_or(0.0) < min_score {
            continue;
        }
        let address = format!("crystalline://{domain}/{permalink}");
        // An observation hit and an engram hit of the same engram are one
        // engram: the first, highest-ranked one wins and keeps its snippet.
        if shown.iter().any(|s| s == &address) || out.iter().any(|r| r.address == address) {
            continue;
        }
        let title = hit
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(permalink)
            .to_string();
        let snippet = cut_snippet(hit.get("snippet").and_then(Value::as_str).unwrap_or(""));
        out.push(Recalled {
            address,
            title,
            snippet,
        });
        if out.len() == limit {
            break;
        }
    }
    out
}

/// Collapse a snippet's whitespace and cut it at [`SNIPPET_CHARS`] on a
/// character boundary, marking a cut with a trailing ` ...`.
fn cut_snippet(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= SNIPPET_CHARS {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(SNIPPET_CHARS).collect();
    cut.push_str(" ...");
    cut
}

/// The block: one header line, then `- <address> - <title>: <snippet>` per
/// engram. A snippet that came back empty drops the `: ` with it.
pub fn render_block(hits: &[Recalled]) -> String {
    let mut out = RECALL_HEADER.to_string();
    for hit in hits {
        out.push_str("\n- ");
        out.push_str(&hit.address);
        out.push_str(" - ");
        out.push_str(&hit.title);
        if !hit.snippet.is_empty() {
            out.push_str(": ");
            out.push_str(&hit.snippet);
        }
    }
    out
}

/// One shape for every harness: both wired harnesses document exactly this
/// object, and an unknown id gets it too, because it is the one channel a
/// harness cannot mistake for a parse failure. Nothing here can block, erase
/// or annotate the prompt - no `decision`, no `reason`, no `systemMessage`,
/// no `continue`. `harness` is accepted so the signature matches
/// `stop_payload`'s and a later per-harness difference has somewhere to land.
pub fn prompt_payload(_harness: Option<HarnessKind>, block: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": block,
        }
    })
}

/// Hold the dedupe list at [`RECALLED_MAX`], dropping the oldest first.
pub fn trim_recalled(recalled: &mut Vec<String>) {
    if recalled.len() > RECALLED_MAX {
        let drop = recalled.len() - RECALLED_MAX;
        recalled.drain(..drop);
    }
}

/// The handler. Every bail is silent; see the module doc for the order.
pub async fn run_prompt(harness: Option<String>) {
    let harness = harness.as_deref().and_then(HarnessKind::from_id);
    let mut raw = String::new();
    if std::io::stdin()
        .take(1024 * 1024)
        .read_to_string(&mut raw)
        .is_err()
    {
        return;
    }
    let Ok(input) = serde_json::from_str::<PromptInput>(&raw) else {
        return;
    };
    if input
        .hook_event_name
        .as_deref()
        .is_some_and(|n| n != "UserPromptSubmit")
    {
        return;
    }
    if !hook::valid_session_id(&input.session_id) {
        return;
    }
    let Some(query) = gate_prompt(&input.prompt) else {
        return;
    };
    let Ok(loaded) = crystalline_service::overlay::load(None) else {
        return;
    };
    let config = &loaded.effective;
    if !config.recall_enabled() || config.domains.is_empty() {
        return;
    }
    let Ok(path) = hook::state_path(&input.session_id) else {
        return;
    };
    let state = hook::read_state(&path).unwrap_or_else(SessionState::fresh);
    let answer = tokio::time::timeout(
        RECALL_BUDGET,
        crystalline_service::ctl_if_running_passive(search_request(&query)),
    )
    .await;
    let Ok(Ok(Some(search))) = answer else {
        return;
    };
    let hits = select_hits(
        &search,
        &state.recalled,
        config.recall_limit(),
        config.recall_min_score(),
    );
    if hits.is_empty() {
        return;
    }
    let mut new_state = state.clone();
    new_state
        .recalled
        .extend(hits.iter().map(|h| h.address.clone()));
    trim_recalled(&mut new_state.recalled);
    new_state.updated_at = chrono::Utc::now();
    // Persisted before the print, the Stop hook's order: a crash between the
    // two costs one engram never shown again this session, never a repeat.
    let _ = hook::write_state(&path, &new_state);
    if let Ok(line) = serde_json::to_string(&prompt_payload(harness, &render_block(&hits))) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the gate ------------------------------------------------------------

    /// A prompt that names nothing never reaches the socket. The word bar and
    /// the character bar both have to hold: "ok go on now" clears three words
    /// and still says nothing, "continue please" clears sixteen characters and
    /// still says nothing.
    #[test]
    fn the_gate_refuses_what_carries_no_subject() {
        for prompt in [
            "",
            "   ",
            "/clear",
            "/compact now please",
            "yes",
            "continue please",
            "ok go on now",
            // A scheduled wakeup's sentinel: one word, so the word bar
            // already refuses it and a loop that runs for hours costs no
            // search until it says something with a subject in it.
            "<<autonomous-loop>>",
        ] {
            assert_eq!(
                gate_prompt(prompt),
                None,
                "prompt {prompt:?} must not search"
            );
        }
        assert_eq!(
            gate_prompt("how does the vent driver retry"),
            Some("how does the vent driver retry".to_string())
        );
        // Trimmed, not merely accepted.
        assert_eq!(
            gate_prompt("  how does the vent driver retry \n"),
            Some("how does the vent driver retry".to_string())
        );
    }

    /// A pasted wall of text is cut at a character boundary, never a byte one:
    /// the cut has to count characters, so a multi-byte paste stays valid text
    /// and stays 1,000 characters rather than 1,000 bytes.
    #[test]
    fn the_gate_cuts_a_long_paste_on_a_character_boundary() {
        let paste = format!("{0} {0} {0}", "ä".repeat(500));
        assert_eq!(paste.chars().count(), 1502);
        let query = gate_prompt(&paste).expect("a three-word paste searches");
        assert_eq!(query.chars().count(), MAX_QUERY_CHARS);
        assert!(
            query.len() > MAX_QUERY_CHARS,
            "multi-byte text is cut by characters, not bytes"
        );
        assert!(
            paste.starts_with(&query),
            "the cut is a prefix of the prompt"
        );
    }

    // --- the request ---------------------------------------------------------

    /// The hook asks the search tool its own question, so the ranking is the
    /// one an agent would have got and nothing is built beside it.
    #[test]
    fn the_search_request_asks_the_tools_own_question() {
        assert_eq!(
            search_request("how does the vent driver retry"),
            json!({
                "v": 1,
                "cmd": "tool",
                "tool": "search_engrams",
                "args": {
                    "query": "how does the vent driver retry",
                    "search_type": "hybrid",
                    "limit": 8,
                },
            })
        );
    }

    // --- selection -----------------------------------------------------------

    fn hit(domain: &str, permalink: &str, score: f64, status: &str) -> Value {
        json!({
            "domain": domain,
            "permalink": permalink,
            "title": format!("Title of {permalink}"),
            "snippet": format!("snippet of {permalink}"),
            "score": score,
            "engram_type": "engram",
            "status": status,
            "tags": [],
            "kind": "engram",
        })
    }

    fn answer(mode: &str, hits: Vec<Value>) -> Value {
        json!({ "mode": mode, "total": hits.len(), "page": 1, "limit": 8, "count": hits.len(), "hits": hits })
    }

    /// Text mode is a daemon with no provider or no vectors yet, and a text
    /// score is an unbounded term frequency: unrankable against a floor.
    #[test]
    fn select_hits_is_silent_in_text_mode() {
        let search = answer("text", vec![hit("ship-ops", "docking-gear", 5.0, "stable")]);
        assert!(select_hits(&search, &[], 3, 0.5).is_empty());
    }

    /// The retirement cut and the floor, with rank order kept through both.
    /// The floor is inclusive: a hit exactly at `min_score` is shown.
    #[test]
    fn select_hits_drops_retired_and_sub_floor_hits_and_keeps_rank_order() {
        let search = answer(
            "hybrid",
            vec![
                hit("ship-ops", "old-way", 0.9, "superseded"),
                hit("ship-ops", "docking-gear", 0.8, "stable"),
                hit("ship-ops", "almost", 0.49, "stable"),
                hit("ship-ops", "exactly", 0.5, "stable"),
            ],
        );
        let picked = select_hits(&search, &[], 3, 0.5);
        assert_eq!(
            picked
                .iter()
                .map(|r| r.address.as_str())
                .collect::<Vec<_>>(),
            vec![
                "crystalline://ship-ops/docking-gear",
                "crystalline://ship-ops/exactly",
            ]
        );
    }

    /// An observation hit and an engram hit of the same engram are one
    /// engram; the higher-ranked one wins and keeps its snippet.
    #[test]
    fn an_observation_and_its_engram_fold_to_one_address() {
        let mut observation = hit("ship-ops", "docking-gear", 0.7, "stable");
        observation["kind"] = json!("observation");
        observation["line"] = json!(7);
        observation["snippet"] = json!("the observation line");
        let mut engram = hit("ship-ops", "docking-gear", 0.6, "stable");
        engram["snippet"] = json!("the engram body");
        let search = answer("hybrid", vec![observation, engram]);

        let picked = select_hits(&search, &[], 3, 0.5);

        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].address, "crystalline://ship-ops/docking-gear");
        assert_eq!(picked[0].snippet, "the observation line");
    }

    /// An address this session has already been shown is skipped, and the cap
    /// counts what is left rather than what came back.
    #[test]
    fn a_shown_address_is_skipped_and_the_cap_holds() {
        let search = answer(
            "hybrid",
            vec![
                hit("d", "one", 0.9, "stable"),
                hit("d", "two", 0.8, "stable"),
                hit("d", "three", 0.7, "stable"),
                hit("d", "four", 0.6, "stable"),
                hit("d", "five", 0.55, "stable"),
                hit("d", "six", 0.5, "stable"),
            ],
        );
        let shown = vec!["crystalline://d/one".to_string()];

        let picked = select_hits(&search, &shown, 3, 0.5);

        assert_eq!(
            picked
                .iter()
                .map(|r| r.address.as_str())
                .collect::<Vec<_>>(),
            vec![
                "crystalline://d/two",
                "crystalline://d/three",
                "crystalline://d/four",
            ]
        );
    }

    /// The snippet is one line whatever the store cut, and a cut one says so.
    #[test]
    fn the_snippet_is_collapsed_and_cut_on_a_character_boundary() {
        assert_eq!(
            cut_snippet("the vent driver\n  retries   three\ttimes"),
            "the vent driver retries three times"
        );

        let long = "x".repeat(300);
        let cut = cut_snippet(&long);
        assert_eq!(cut.chars().count(), SNIPPET_CHARS + 4);
        assert!(cut.ends_with(" ..."));

        let exact = "ä".repeat(SNIPPET_CHARS);
        assert_eq!(
            cut_snippet(&exact),
            exact,
            "a snippet at the bar is untouched"
        );
    }

    // --- output --------------------------------------------------------------

    const EXAMPLE_BLOCK: &str = "Knowledge that may apply, from your Crystalline domains. Read an engram with read_engram (pass the crystalline:// address as identifier) before relying on it; ignore what does not fit.
- crystalline://ship-ops/coolant/vent-driver-retries - Vent driver retries: the vent driver retries three times at 200 ms before it raises a fault, and the third retry is what the coolant loop alarm actually measures ...
- crystalline://ship-ops/docking-gear - Docking gear: the clamp sequence is armed from the bridge console and never from the gear itself";

    /// The block is what the spec prints, to the byte: one header line then
    /// one line per engram.
    #[test]
    fn the_block_is_byte_exact() {
        let hits = vec![
            Recalled {
                address: "crystalline://ship-ops/coolant/vent-driver-retries".to_string(),
                title: "Vent driver retries".to_string(),
                snippet: "the vent driver retries three times at 200 ms before it raises a fault, and the third retry is what the coolant loop alarm actually measures ...".to_string(),
            },
            Recalled {
                address: "crystalline://ship-ops/docking-gear".to_string(),
                title: "Docking gear".to_string(),
                snippet: "the clamp sequence is armed from the bridge console and never from the gear itself".to_string(),
            },
        ];
        assert_eq!(render_block(&hits), EXAMPLE_BLOCK);
    }

    /// A hit whose snippet came back empty drops the separator with it.
    #[test]
    fn an_empty_snippet_drops_the_colon() {
        let hits = vec![Recalled {
            address: "crystalline://d/p".to_string(),
            title: "A title".to_string(),
            snippet: String::new(),
        }];
        assert_eq!(
            render_block(&hits),
            format!("{RECALL_HEADER}\n- crystalline://d/p - A title")
        );
    }

    /// Every harness gets the one documented object, an unknown id included:
    /// asserted on the serialized line rather than on parsed fields, because a
    /// check that reads two fields would pass just as happily with a third
    /// beside them under a name nobody thought to look for.
    #[test]
    fn every_harness_gets_the_one_documented_shape() {
        for harness in [
            None,
            Some(HarnessKind::ClaudeCode),
            Some(HarnessKind::Codex),
            Some(HarnessKind::Copilot),
        ] {
            let line = serde_json::to_string(&prompt_payload(harness, "x")).unwrap();
            assert_eq!(
                line,
                r#"{"hookSpecificOutput":{"additionalContext":"x","hookEventName":"UserPromptSubmit"}}"#,
                "harness {harness:?} gets the documented shape and nothing beside it"
            );
        }
    }

    // --- the dedupe list -----------------------------------------------------

    /// The list is held at its cap by dropping the oldest, so a long session
    /// at worst repeats an engram it last showed hundreds of prompts ago.
    #[test]
    fn trim_recalled_keeps_the_newest_two_hundred() {
        let mut recalled: Vec<String> = (0..RECALLED_MAX + 5).map(|i| format!("a{i}")).collect();
        trim_recalled(&mut recalled);
        assert_eq!(recalled.len(), RECALLED_MAX);
        assert_eq!(recalled.first().unwrap(), "a5");
        assert_eq!(recalled.last().unwrap(), &format!("a{}", RECALLED_MAX + 4));

        let mut short = vec!["a".to_string()];
        trim_recalled(&mut short);
        assert_eq!(short, vec!["a".to_string()]);
    }

    /// The four words are the store's own retirement set
    /// (`crates/index/src/store.rs`'s `RETIRED_STATUSES`), spelled here
    /// because this crate does not depend on the index crate. If that list
    /// ever grows, this is the copy that has to grow with it.
    #[test]
    fn the_retired_set_is_the_stores_own_four() {
        assert_eq!(RETIRED, ["deprecated", "superseded", "archived", "legacy"]);
    }
}
