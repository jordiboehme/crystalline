//! What an unattended session (an agent of the loops, or a session that
//! said it is unattended: its registration's mark, the stdio run domain or
//! a run header) leaves to a curator on a Reflection: a page's `freshness`
//! and `follows`. It still writes the page, sets its status, creates owned
//! pages, retires and restores.

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
        }
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
