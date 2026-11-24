//! What an unattended session (an agent of the loops, or a session that
//! said it is unattended: its registration's mark, the stdio run domain or
//! a run header) leaves to a curator on a Reflection: a page's `freshness`
//! and `follows`. It still writes the page, sets its status, creates owned
//! pages, retires and restores.

use crystalline_core::reflection as r;
use crystalline_service::params::{PageSettingsChange, WriteReflectionParams};
use crystalline_service::read_limit::{Limit, within};
use crystalline_service::{EngineError, ReflectionCaller};
use serde_json::Value;

use crate::docs::{person, reflecting, write};
use crate::fixture::Fx;
use crate::page_lifecycle::{curator_agent, unattended};

const BODY: &str = "# A\n\n## One\n\nFirst.\n";

fn file(fx: &Fx, rel: &str) -> Option<String> {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).ok()
}

/// A page with `keys` in its frontmatter.
fn page(keys: &str) -> String {
    if keys.is_empty() {
        BODY.to_string()
    } else {
        format!("---\n{keys}---\n{BODY}")
    }
}

/// The unattended callers: an agent token of the reflect loop with no
/// limit around it, and a person's session whose limit says it is
/// unattended (what a run header or the stdio run domain sets).
#[derive(Clone)]
enum Run {
    Agent(ReflectionCaller),
    Header,
}

impl Run {
    async fn both(fx: &Fx) -> [Run; 2] {
        [Run::Agent(curator_agent(fx).await), Run::Header]
    }

    fn name(&self) -> &'static str {
        match self {
            Run::Agent(_) => "agent",
            Run::Header => "run header",
        }
    }

    fn caller(&self) -> ReflectionCaller {
        match self {
            Run::Agent(caller) => caller.clone(),
            Run::Header => person(),
        }
    }

    fn limit(&self) -> Option<Limit> {
        matches!(self, Run::Header).then(unattended)
    }

    async fn write(&self, fx: &Fx, p: &WriteReflectionParams) -> Result<Value, EngineError> {
        within(self.limit(), fx.engine.write_reflection(p, &self.caller())).await
    }

    async fn settings(&self, fx: &Fx, change: &PageSettingsChange) -> Result<Value, EngineError> {
        within(
            self.limit(),
            fx.engine
                .set_reflection_page_settings("eng", change, &self.caller()),
        )
        .await
    }
}

async fn put(fx: &Fx, path: &str, text: &str) {
    fx.engine
        .write_reflection(&write(path, text), &person())
        .await
        .unwrap();
}

fn settings_refused(e: &EngineError, run: &Run, what: &str) {
    assert!(
        matches!(e, EngineError::Forbidden(_)),
        "{} {what}: {e}",
        run.name()
    );
    let text = e.to_string();
    assert!(
        text.contains("unattended") && text.contains("freshness or follows"),
        "{} {what}: {text}",
        run.name()
    );
    assert!(text.contains("curator"), "{} {what}: {text}", run.name());
}

fn act(action: &str, path: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        domain: "eng".into(),
        action: Some(action.into()),
        path: path.into(),
        ..Default::default()
    }
}

/// Every route to a page's freshness or follows is refused to an
/// unattended caller, and nothing is written: a whole-page write that
/// changes, clears or adds one, a create that sets one, and the settings
/// change itself.
#[tokio::test]
async fn an_unattended_session_does_not_change_freshness_or_follows() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", &page("freshness: 1d\n")).await;
    put(&fx, "guides/b.md", &page("")).await;
    let stored = file(&fx, "guides/a.md").unwrap();
    for run in Run::both(&fx).await {
        let tries = [
            ("a new freshness", "guides/a.md", page("freshness: 6h\n")),
            ("a cleared freshness", "guides/a.md", page("freshness:\n")),
            (
                "a follows",
                "guides/a.md",
                page("freshness: 1d\nfollows: guides/b.md\n"),
            ),
            (
                "a new page's freshness",
                "guides/new.md",
                page("freshness: 1d\n"),
            ),
            (
                "a new page's follows",
                "guides/new.md",
                page("follows: guides/b.md\n"),
            ),
        ];
        for (what, path, text) in tries {
            let refused = run.write(&fx, &write(path, &text)).await.unwrap_err();
            settings_refused(&refused, &run, what);
            // A new page is told so: there is nothing stored to send back.
            let new = path == "guides/new.md";
            assert_eq!(
                refused.to_string().contains("on a new page"),
                new,
                "{} {what}: {refused}",
                run.name()
            );
        }
        // An owned page takes no freshness either: the run leaves it out.
        let refused = run
            .write(
                &fx,
                &WriteReflectionParams {
                    owner: Some("guides/b.md".into()),
                    ..write("guides/b/owned.md", &page("freshness: 1d\n"))
                },
            )
            .await
            .unwrap_err();
        settings_refused(&refused, &run, "an owned page's freshness");
        assert!(file(&fx, "guides/b/owned.md").is_none(), "{}", run.name());
        assert_eq!(file(&fx, "guides/a.md").as_deref(), Some(stored.as_str()));
        assert!(file(&fx, "guides/new.md").is_none(), "{}", run.name());

        for (what, change) in [
            (
                "a freshness change",
                PageSettingsChange {
                    path: "guides/a.md".into(),
                    freshness: Some("6h".into()),
                    ..Default::default()
                },
            ),
            (
                "a follows change",
                PageSettingsChange {
                    path: "guides/a.md".into(),
                    follows: Some("guides/b.md".into()),
                    ..Default::default()
                },
            ),
        ] {
            let refused = run.settings(&fx, &change).await.unwrap_err();
            settings_refused(&refused, &run, what);
        }
        assert_eq!(file(&fx, "guides/a.md").as_deref(), Some(stored.as_str()));
    }
}

/// What a reconcile writes still lands for an unattended caller: the page
/// sent back with its freshness as stored, a page written without its
/// frontmatter (the stored keys stay), a section, a status, an owned page
/// with no freshness, a retire and a restore.
#[tokio::test]
async fn an_unattended_session_writes_the_page_and_keeps_its_settings() {
    let fx = reflecting().await;
    put(&fx, "guides/b.md", &page("")).await;
    put(
        &fx,
        "guides/a.md",
        &page("freshness: 1d\nfollows: guides/b.md\n"),
    )
    .await;
    for run in Run::both(&fx).await {
        let name = run.name();
        run.write(
            &fx,
            &write(
                "guides/a.md",
                &page("freshness: 1d\nfollows: guides/b.md\n").replace("First.", "Second."),
            ),
        )
        .await
        .unwrap_or_else(|e| panic!("{name} as stored: {e}"));
        run.write(
            &fx,
            &write("guides/a.md", &BODY.replace("First.", "Third.")),
        )
        .await
        .unwrap_or_else(|e| panic!("{name} without frontmatter: {e}"));
        let text = file(&fx, "guides/a.md").unwrap();
        assert!(
            text.contains("freshness: 1d") && text.contains("follows: guides/b.md"),
            "{name}: {text}"
        );
        run.write(
            &fx,
            &WriteReflectionParams {
                operation: Some("replace_section".into()),
                section: Some("One".into()),
                content: Some("Fourth.\n".into()),
                markdown: None,
                ..write("guides/a.md", "")
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{name} a section: {e}"));
        run.write(
            &fx,
            &write(
                "guides/a.md",
                &page("freshness: 1d\nfollows: guides/b.md\nstatus: draft\n"),
            ),
        )
        .await
        .unwrap_or_else(|e| panic!("{name} a status: {e}"));
        run.settings(
            &fx,
            &PageSettingsChange {
                path: "guides/a.md".into(),
                status: Some("review".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{name} a status change: {e}"));
        let text = file(&fx, "guides/a.md").unwrap();
        assert!(
            text.contains("status: review") && text.contains("freshness: 1d"),
            "{name}: {text}"
        );

        let child = format!("guides/a/{}.md", name.replace(' ', "-"));
        run.write(
            &fx,
            &WriteReflectionParams {
                owner: Some("guides/a.md".into()),
                ..write(&child, BODY)
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{name} an owned page: {e}"));
        run.write(&fx, &act("retire", &child))
            .await
            .unwrap_or_else(|e| panic!("{name} retire: {e}"));
        run.write(&fx, &act("restore", &child))
            .await
            .unwrap_or_else(|e| panic!("{name} restore: {e}"));
    }
}

/// A curator in an attended session still sets, changes and clears both.
#[tokio::test]
async fn an_attended_curator_changes_freshness_and_follows() {
    let fx = reflecting().await;
    put(&fx, "guides/b.md", &page("")).await;
    put(
        &fx,
        "guides/a.md",
        &page("freshness: 1d\nfollows: guides/b.md\n"),
    )
    .await;
    put(&fx, "guides/a.md", &page("freshness: 6h\n")).await;
    fx.engine
        .set_reflection_page_settings(
            "eng",
            &PageSettingsChange {
                path: "guides/a.md".into(),
                freshness: Some("7d".into()),
                follows: Some("guides/b.md".into()),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();
    let text = file(&fx, "guides/a.md").unwrap();
    assert!(
        text.contains("freshness: 7d") && text.contains("follows: guides/b.md"),
        "{text}"
    );
    put(&fx, "guides/a.md", &page("freshness:\n")).await;
    assert!(!file(&fx, "guides/a.md").unwrap().contains("freshness"));
}

// --- pact text --------------------------------------------------------------

const PACT: &str = "# Pact\n\n## Tone\n\nPlain. See [B](guides/b.md).\n";
const PAGE_PACT: &str = "# Pact of A\n\n## Tone\n\nShort.\n";

/// PACT.md, a page with its page pact, and a page without one.
async fn pacted(fx: &Fx) {
    put(fx, "guides/a.md", BODY).await;
    put(fx, "guides/b.md", BODY).await;
    put(fx, "PACT.md", PACT).await;
    put(fx, "guides/a.pact.md", PAGE_PACT).await;
}

fn pact_refused(e: &EngineError, run: &Run, what: &str) {
    assert!(
        matches!(e, EngineError::Forbidden(_)),
        "{} {what}: {e}",
        run.name()
    );
    let text = e.to_string();
    assert!(
        text.contains("unattended") && text.contains("pact text") && text.contains("curator"),
        "{} {what}: {text}",
        run.name()
    );
    assert!(text.contains("thread"), "{} {what}: {text}", run.name());
}

/// Every pact file is a curator's to write in person: an unattended caller
/// may not overwrite PACT.md (not even with the text it holds), edit a
/// section of a page pact or create one, in any letter case, and may not
/// move a page that takes its pact along. Nothing is written.
#[tokio::test]
async fn an_unattended_session_does_not_write_pact_text() {
    let fx = reflecting().await;
    pacted(&fx).await;
    let before: Vec<Option<String>> = ["PACT.md", "guides/a.pact.md", "guides/a.md"]
        .iter()
        .map(|rel| file(&fx, rel))
        .collect();
    for run in Run::both(&fx).await {
        let tries = [
            ("PACT.md", write("PACT.md", "# Pact\n\n## Tone\n\nLoud.\n")),
            (
                "PACT.md as it is",
                write("PACT.md", &file(&fx, "PACT.md").unwrap()),
            ),
            (
                "a page pact's section",
                WriteReflectionParams {
                    operation: Some("replace_section".into()),
                    section: Some("Tone".into()),
                    content: Some("Long.\n".into()),
                    markdown: None,
                    ..write("guides/a.pact.md", "")
                },
            ),
            ("a new page pact", write("guides/b.pact.md", PAGE_PACT)),
        ];
        for (what, p) in tries {
            let refused = run.write(&fx, &p).await.unwrap_err();
            pact_refused(&refused, &run, what);
            // A new pact is told it is not created; an existing one stays.
            let new = p.path == "guides/b.pact.md";
            assert_eq!(
                refused.to_string().contains("does not create a page pact"),
                new,
                "{} {what}: {refused}",
                run.name()
            );
        }
        assert!(file(&fx, "guides/b.pact.md").is_none(), "{}", run.name());
        // Another letter case names the pact too, and is refused before
        // anything is read.
        let refused = run
            .write(&fx, &write("pact.md", "# Pact\n\n## Tone\n\nLoud.\n"))
            .await
            .unwrap_err();
        assert!(matches!(refused, EngineError::Invalid(_)), "{refused}");
        assert!(refused.to_string().contains("PACT.md"), "{refused}");
        let names: Vec<String> = std::fs::read_dir(fx.root.join("reflections"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n == "pact.md"),
            "{}: {names:?}",
            run.name()
        );

        let refused = run
            .write(
                &fx,
                &WriteReflectionParams {
                    to: Some("design/a.md".into()),
                    ..act("move", "guides/a.md")
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(refused, EngineError::Forbidden(_)), "{refused}");
        let text = refused.to_string();
        assert!(
            text.contains("unattended")
                && text.contains("pact")
                && text.contains("guides/a.md")
                && text.contains("curator"),
            "{text}"
        );
        assert!(file(&fx, "design/a.md").is_none(), "{}", run.name());
        let after: Vec<Option<String>> = ["PACT.md", "guides/a.pact.md", "guides/a.md"]
            .iter()
            .map(|rel| file(&fx, rel))
            .collect();
        assert_eq!(after, before, "{}", run.name());
    }
}

/// An unattended caller still settles a pact, answers a thread on one and
/// moves a page that has no pact of its own (the links PACT.md has to it
/// follow the move, as a follows does: nobody's change).
#[tokio::test]
async fn an_unattended_session_settles_and_answers_on_a_pact() {
    use crystalline_service::params::DiscussParams;
    let fx = reflecting().await;
    pacted(&fx).await;
    for (n, run) in Run::both(&fx).await.into_iter().enumerate() {
        let name = run.name();
        run.write(&fx, &act("settle", "PACT.md"))
            .await
            .unwrap_or_else(|e| panic!("{name} settle: {e}"));
        // Settle with sections too: it writes the bookkeeping, never pact
        // prose.
        let tone = r::split_sections(&file(&fx, "guides/a.pact.md").unwrap())
            .into_iter()
            .find(|s| s.heading.contains("Tone"))
            .and_then(|s| s.id)
            .expect("the Tone section has an id");
        run.write(
            &fx,
            &WriteReflectionParams {
                sections: vec![tone],
                outcome: Some("kept".into()),
                ..act("settle", "guides/a.pact.md")
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{name} settle with sections: {e}"));
        let opened = fx
            .engine
            .discuss(
                &DiscussParams {
                    domain: "eng".into(),
                    action: Some("open".into()),
                    path: Some("guides/a.pact.md".into()),
                    section: Some("Tone".into()),
                    text: Some("Could the tone be longer?".into()),
                    ..Default::default()
                },
                &person(),
            )
            .await
            .unwrap();
        let thread = opened["thread"]["id"]
            .as_str()
            .or(opened["id"].as_str())
            .expect("the thread's id")
            .to_string();
        within(
            run.limit(),
            fx.engine.discuss(
                &DiscussParams {
                    domain: "eng".into(),
                    action: Some("answer".into()),
                    path: Some("guides/a.pact.md".into()),
                    thread: Some(thread),
                    text: Some("A curator changes the pact.".into()),
                    ..Default::default()
                },
                &run.caller(),
            ),
        )
        .await
        .unwrap_or_else(|e| panic!("{name} answer: {e}"));

        let (from, to) = if n == 0 {
            ("guides/b.md", "design/b.md")
        } else {
            ("design/b.md", "guides/b.md")
        };
        run.write(
            &fx,
            &WriteReflectionParams {
                to: Some(to.into()),
                ..act("move", from)
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{name} move: {e}"));
        assert!(file(&fx, to).is_some(), "{name}");
        assert!(
            file(&fx, "PACT.md").unwrap().contains(to),
            "{name}: PACT.md's link follows"
        );
    }
}

/// A curator in an attended session writes and moves pact text as before.
#[tokio::test]
async fn an_attended_curator_writes_and_moves_pacts() {
    let fx = reflecting().await;
    pacted(&fx).await;
    put(&fx, "PACT.md", "# Pact\n\n## Tone\n\nLoud.\n").await;
    put(&fx, "guides/b.pact.md", PAGE_PACT).await;
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                to: Some("design/a.md".into()),
                ..act("move", "guides/a.md")
            },
            &person(),
        )
        .await
        .unwrap();
    assert!(file(&fx, "design/a.pact.md").is_some());
}

/// Over MCP, a stdio session started for a run (its run domain marks it
/// unattended, with no lease and no hand-built limit) is refused the same
/// writes and still settles the pact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_session_over_mcp_keeps_off_settings_and_pact_text() {
    use crate::read_limit::{landed, refused, session, tool};
    use crystalline_service::McpServer;
    use serde_json::json;

    let fx = reflecting().await;
    pacted(&fx).await;
    put(&fx, "guides/c.md", &page("freshness: 1d\n")).await;
    let mut wire = session(McpServer::new(fx.engine.clone()).with_run_domain("eng")).await;

    let answer = tool(
        &mut wire,
        20,
        "write_reflection",
        json!({ "domain": "eng", "path": "PACT.md", "markdown": "# Pact\n\n## Tone\n\nLoud.\n" }),
    )
    .await;
    let text = refused(&answer);
    assert!(text.contains("pact text"), "{text}");

    let answer = tool(
        &mut wire,
        21,
        "write_reflection",
        json!({ "domain": "eng", "path": "guides/c.md", "markdown": page("freshness: 6h\n") }),
    )
    .await;
    let text = refused(&answer);
    assert!(text.contains("freshness or follows"), "{text}");
    assert!(file(&fx, "guides/c.md").unwrap().contains("freshness: 1d"));

    let answer = tool(
        &mut wire,
        22,
        "write_reflection",
        json!({ "domain": "eng", "path": "PACT.md", "action": "settle" }),
    )
    .await;
    landed(&answer);
}
