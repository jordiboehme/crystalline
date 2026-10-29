//! A person may share text from a domain fewer people may read on a page
//! more people read, on purpose: the write that would be refused for reach
//! alone lands once that person confirmed it, and the confirmation is kept
//! on the server with who gave it, when, and who could read the page then.
//!
//! The confirmation comes from a person and never from the agent: an MCP
//! session asks its person through elicitation (a 2026-07-28 peer gets
//! `input_required` bound to a single-use state, a legacy peer an
//! `elicitation/create` request), and a session marked unattended, a session
//! whose caller holds a live loop lease, a peer that cannot ask and any flag
//! in the arguments confirm nothing. Fluid confirms a `source_too_wide` item.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crystalline_core::automation::Loop;
use crystalline_service::McpServer;
use crystalline_service::params::{ReflectParams, WriteReflectionParams};
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use crystalline_service::{DomainAccess, ReflectionCaller, Scope};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::docs::write;
use crate::fixture::Fx;
use crate::support::mcp_session::{ServedSession, chooses_lifecycle};

/// The address of an engram in `harbor`.
const TEAM: &str = "crystalline://harbor/team/harbor-team";

/// What a write that cites a domain fewer people may read is refused with.
const TOO_WIDE: &str = "this source is in a domain fewer people may read than this page's domain";

/// The sentence a refusal adds when nobody could be asked.
const NOBODY_ASKED: &str = "a person who may read that domain can share it on purpose";

const OVERVIEW: &str = "# Overview\n\nFor people.\n\n## Team\n\nFive people.\n";

/// `eng` (Reflection on) beside `harbor`, with keeper, ann and sam as
/// instance editors and the accounts resolver installed.
async fn reach() -> (Fx, Arc<AuthStore>) {
    let fx = crate::sources::crossing().await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["keeper", "ann", "sam"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    (fx, auth)
}

/// [`reach`] with harbor private to keeper and sam: a shared page of `eng`
/// citing harbor is too wide.
pub(crate) async fn private_harbor() -> (Fx, Arc<AuthStore>) {
    let (fx, auth) = reach().await;
    private(&auth, "harbor", &["sam"]).await;
    (fx, auth)
}

/// Make `domain` private to keeper, with `members` as editors.
async fn private(auth: &AuthStore, domain: &str, members: &[&str]) {
    auth.set_domain_visibility(domain, true, "keeper")
        .await
        .unwrap();
    for member in members {
        auth.upsert_domain_member(domain, member, MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
    }
}

fn user(name: &str) -> Scope {
    Scope::User {
        account: name.into(),
        admin: false,
    }
}

fn agent_as(name: &str) -> ReflectionCaller {
    ReflectionCaller::agent(user(name), "claude-code", false)
}

/// `overview.md` in `eng` with its `Team` section built from harbor.
fn cite() -> WriteReflectionParams {
    let mut p = write("overview.md", OVERVIEW);
    p.sources = BTreeMap::from([("Team".to_string(), vec![TEAM.to_string()])]);
    p
}

/// [`cite`] as the person behind `caller` confirmed it: the basis of the
/// question they were asked, or a made-up one when nothing was asked.
async fn confirmed(fx: &Fx, caller: &ReflectionCaller) -> WriteReflectionParams {
    let mut p = cite();
    let asked = fx
        .engine
        .reflection_disclosure_question(&p, caller)
        .await
        .ok()
        .flatten();
    p.disclosed = Some(asked.map_or_else(|| "never asked".to_string(), |ask| ask.basis));
    p
}

fn page_exists(fx: &Fx) -> bool {
    fx.root.join("reflections/overview.md").exists()
}

/// The `source_too_wide` items of `eng`'s queue as `caller` gets it.
async fn too_wide(fx: &Fx, caller: &ReflectionCaller) -> Vec<Value> {
    let queue = fx
        .engine
        .reflect(
            &ReflectParams {
                domains: vec!["eng".into()],
                limit: Some(100),
                ..Default::default()
            },
            caller,
        )
        .await
        .unwrap();
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == "source_too_wide")
        .cloned()
        .collect()
}

/// The `Team` section of `overview.md` as `caller` reads it.
async fn team_section(fx: &Fx, caller: &ReflectionCaller) -> Value {
    let doc = fx
        .engine
        .read_reflection("eng", "overview.md", caller)
        .await
        .unwrap();
    doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["heading"] == "Team")
        .cloned()
        .unwrap()
}

// --- the engine ---------------------------------------------------------

/// The question names the page's readers in plain words, the source's
/// domain, that the text becomes readable by them and that the source's name
/// stays hidden from those who may not read it.
#[tokio::test]
async fn the_question_names_the_readers_and_what_happens() {
    let (fx, _auth) = private_harbor().await;
    let question = fx
        .engine
        .reflection_disclosure_question(&cite(), &agent_as("sam"))
        .await
        .unwrap()
        .expect("a reach refusal is a question")
        .question;
    assert!(question.contains("harbor"), "{question}");
    assert!(
        question.contains("everyone who can open this instance"),
        "{question}"
    );
    assert!(
        question.contains("becomes readable by all of them"),
        "{question}"
    );
    assert!(question.contains("stay hidden"), "{question}");
    // A source every reader may read needs no question.
    let mut own = write("overview.md", OVERVIEW);
    own.sources = BTreeMap::from([("Team".to_string(), vec!["runbooks/deploy".to_string()])]);
    assert!(
        fx.engine
            .reflection_disclosure_question(&own, &agent_as("sam"))
            .await
            .unwrap()
            .is_none()
    );
}

/// A private page names how many accounts read it, never who.
#[tokio::test]
async fn a_private_page_is_named_by_how_many_read_it() {
    let (fx, auth) = reach().await;
    private(&auth, "harbor", &[]).await;
    private(&auth, "eng", &["sam", "ann"]).await;
    let question = fx
        .engine
        .reflection_disclosure_question(&cite(), &agent_as("keeper"))
        .await
        .unwrap()
        .unwrap()
        .question;
    assert!(
        question.contains("the 3 accounts that may read eng"),
        "{question}"
    );
    assert!(!question.contains("sam"), "{question}");
}

/// A confirmed write lands, records who confirmed and when, and the queue
/// does not name the section.
#[tokio::test]
async fn a_confirmed_write_lands_and_records_who_confirmed() {
    let (fx, _auth) = private_harbor().await;
    fx.engine
        .write_reflection(&confirmed(&fx, &agent_as("sam")).await, &agent_as("sam"))
        .await
        .unwrap();
    assert!(page_exists(&fx));
    let section = team_section(&fx, &agent_as("sam")).await;
    assert_eq!(section["disclosed"]["by"], json!("sam"), "{section}");
    assert!(section["disclosed"]["at"].is_string(), "{section}");
    assert!(too_wide(&fx, &agent_as("keeper")).await.is_empty());
    // Not in the page file: nothing about the confirmation is written there.
    let text = std::fs::read_to_string(fx.root.join("reflections/overview.md")).unwrap();
    assert!(!text.contains("disclos"), "{text}");
}

/// The person agreed to the readers named in the question. When the page's
/// readers change between the question and the write, the write is refused
/// and nothing is recorded: the person is asked again.
#[tokio::test]
async fn a_yes_to_readers_that_changed_since_is_refused() {
    let (fx, auth) = reach().await;
    private(&auth, "harbor", &[]).await;
    private(&auth, "eng", &["sam"]).await;
    let p = confirmed(&fx, &agent_as("keeper")).await;
    // ann joins eng after keeper was asked.
    auth.upsert_domain_member("eng", "ann", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    let err = fx
        .engine
        .write_reflection(&p, &agent_as("keeper"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("changed since"), "{err}");
    assert!(!page_exists(&fx));
    // Asked again, the yes lands.
    fx.engine
        .write_reflection(
            &confirmed(&fx, &agent_as("keeper")).await,
            &agent_as("keeper"),
        )
        .await
        .unwrap();
}

/// The same in Fluid: a question shown before the page's readers changed
/// confirms nothing.
#[tokio::test]
async fn a_person_confirming_a_stale_question_is_refused() {
    let (fx, auth) = reach().await;
    fx.engine
        .write_reflection(&cite(), &agent_as("keeper"))
        .await
        .unwrap();
    private(&auth, "harbor", &[]).await;
    private(&auth, "eng", &["sam"]).await;
    let items = too_wide(&fx, &agent_as("keeper")).await;
    let section = items[0]["section"].as_str().unwrap().to_string();
    let basis = items[0]["basis"].as_str().unwrap().to_string();
    auth.upsert_domain_member("eng", "ann", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    let keeper = ReflectionCaller::person(user("keeper"), "keeper");
    let err = fx
        .engine
        .confirm_reflection_disclosure("eng", "overview.md", &section, &basis, &keeper)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("changed since"), "{err}");
    assert_eq!(too_wide(&fx, &agent_as("keeper")).await.len(), 1);
}

/// Without the confirmation the write is refused exactly as before.
#[tokio::test]
async fn an_unconfirmed_write_is_refused_as_before() {
    let (fx, _auth) = private_harbor().await;
    let err = fx
        .engine
        .write_reflection(&cite(), &agent_as("sam"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains(TOO_WIDE), "{err}");
    assert!(!page_exists(&fx));
}

/// A confirmation never opens a domain the writer may not read: that
/// refusal comes first and is an unknown domain's.
#[tokio::test]
async fn a_confirmation_never_opens_a_domain_the_writer_cannot_read() {
    let (fx, _auth) = private_harbor().await;
    let err = fx
        .engine
        .write_reflection(&confirmed(&fx, &agent_as("ann")).await, &agent_as("ann"))
        .await
        .unwrap_err()
        .to_string();
    assert!(!err.contains(TOO_WIDE), "{err}");
    assert!(!page_exists(&fx));
}

/// Nobody is behind the anonymous caller, so it confirms nothing.
#[tokio::test]
async fn the_anonymous_caller_confirms_nothing() {
    let (fx, _auth) = private_harbor().await;
    let open = ReflectionCaller::agent(Scope::Anonymous, "agent", true);
    assert!(
        fx.engine
            .write_reflection(&confirmed(&fx, &open).await, &open)
            .await
            .is_err()
    );
    assert!(!page_exists(&fx));
}

/// When nobody here knows who reads which domain, nothing is asked and
/// nothing can be confirmed.
#[tokio::test]
async fn unknown_readers_are_never_confirmable() {
    let fx = crate::sources::crossing().await;
    fx.engine
        .set_domain_access_unavailable("the file is locked".to_string());
    assert!(
        fx.engine
            .reflection_disclosure_question(&cite(), &agent_as("sam"))
            .await
            .is_err()
    );
    assert!(
        fx.engine
            .write_reflection(&confirmed(&fx, &agent_as("sam")).await, &agent_as("sam"))
            .await
            .is_err()
    );
    assert!(!page_exists(&fx));
}

/// The page's readers widen after the confirmation: the section is named
/// again. The source narrowing afterwards changes nothing: those people read
/// the page already.
#[tokio::test]
async fn widening_the_page_names_the_section_again_and_narrowing_the_source_does_not() {
    let (fx, auth) = reach().await;
    auth.add_user("lee", "lee", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    private(&auth, "harbor", &["ann"]).await;
    private(&auth, "eng", &["ann", "sam"]).await;
    fx.engine
        .write_reflection(
            &confirmed(&fx, &agent_as("keeper")).await,
            &agent_as("keeper"),
        )
        .await
        .unwrap();
    assert!(too_wide(&fx, &agent_as("keeper")).await.is_empty());

    // harbor narrows: ann no longer reads it, and read the page all along.
    auth.remove_domain_member("harbor", "ann").await.unwrap();
    assert!(too_wide(&fx, &agent_as("keeper")).await.is_empty());

    // eng widens: lee reads the page now and was never asked about.
    auth.upsert_domain_member("eng", "lee", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    let items = too_wide(&fx, &agent_as("keeper")).await;
    assert_eq!(items.len(), 1, "{items:?}");
}

/// A page made shared after the confirmation is named again.
#[tokio::test]
async fn sharing_a_private_page_names_the_section_again() {
    let (fx, auth) = reach().await;
    private(&auth, "harbor", &[]).await;
    private(&auth, "eng", &["sam"]).await;
    fx.engine
        .write_reflection(
            &confirmed(&fx, &agent_as("keeper")).await,
            &agent_as("keeper"),
        )
        .await
        .unwrap();
    assert!(too_wide(&fx, &agent_as("keeper")).await.is_empty());
    auth.set_domain_visibility("eng", false, "keeper")
        .await
        .unwrap();
    assert_eq!(too_wide(&fx, &agent_as("keeper")).await.len(), 1);
}

/// A confirmation written into the page file by hand (or pulled in) has no
/// effect: the record lives on the server.
#[tokio::test]
async fn a_confirmation_in_the_page_file_has_no_effect() {
    let (fx, _auth) = private_harbor().await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(
        fx.root.join("reflections/overview.md"),
        "---\nreflection:\n  aaaa1111:\n    sources:\n      - domain: harbor\n        engram: team/harbor-team\n        checksum: x\n    disclosed_by: keeper\n    disclosed_at: 2026-10-28T00:00:00Z\n---\n# Overview\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive people.\n",
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    assert_eq!(too_wide(&fx, &agent_as("keeper")).await.len(), 1);
}

/// A later write that sets the section's sources again without the
/// confirmation is refused: an unattended update of a shared section would
/// draw new text from the source that nobody agreed to share.
#[tokio::test]
async fn a_later_unconfirmed_write_of_the_source_is_refused() {
    let (fx, _auth) = private_harbor().await;
    fx.engine
        .write_reflection(&confirmed(&fx, &agent_as("sam")).await, &agent_as("sam"))
        .await
        .unwrap();
    let err = fx
        .engine
        .write_reflection(&cite(), &agent_as("sam"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains(TOO_WIDE), "{err}");
}

/// The confirmation goes with the page when it moves, and with it when it
/// is deleted.
#[tokio::test]
async fn the_confirmation_moves_and_goes_with_the_page() {
    let (fx, _auth) = private_harbor().await;
    fx.engine
        .write_reflection(&confirmed(&fx, &agent_as("sam")).await, &agent_as("sam"))
        .await
        .unwrap();
    let mut mv = WriteReflectionParams {
        domain: "eng".into(),
        path: "overview.md".into(),
        action: Some("move".into()),
        to: Some("guides/overview.md".into()),
        ..Default::default()
    };
    fx.engine
        .write_reflection(&mv, &agent_as("sam"))
        .await
        .unwrap();
    assert!(too_wide(&fx, &agent_as("keeper")).await.is_empty());
    // The same page put back on disk after a delete finds no record.
    let moved = fx.root.join("reflections/guides/overview.md");
    let text = std::fs::read_to_string(&moved).unwrap();
    mv.path = "guides/overview.md".into();
    mv.action = Some("delete".into());
    mv.to = None;
    fx.engine
        .write_reflection(&mv, &agent_as("sam"))
        .await
        .unwrap();
    std::fs::write(&moved, text).unwrap();
    fx.engine.sync(None).await.unwrap();
    assert_eq!(too_wide(&fx, &agent_as("keeper")).await.len(), 1);
}

/// Fluid's confirmation of a `source_too_wide` item: the item carries the
/// question, and a confirmation by a curator who may read the source takes
/// the item away.
#[tokio::test]
async fn a_person_confirms_a_named_section() {
    let (fx, auth) = reach().await;
    // Written before harbor narrowed: the queue names it.
    fx.engine
        .write_reflection(&cite(), &agent_as("sam"))
        .await
        .unwrap();
    private(&auth, "harbor", &["sam"]).await;
    let items = too_wide(&fx, &agent_as("keeper")).await;
    assert_eq!(items.len(), 1);
    let question = items[0]["question"].as_str().unwrap();
    assert!(
        question.contains("everyone who can open this instance"),
        "{question}"
    );
    let section = items[0]["section"].as_str().unwrap().to_string();
    let basis = items[0]["basis"].as_str().unwrap().to_string();

    // ann may not read harbor: she cannot confirm, and learns nothing.
    assert!(
        fx.engine
            .confirm_reflection_disclosure(
                "eng",
                "overview.md",
                &section,
                &basis,
                &ReflectionCaller::person(user("ann"), "ann"),
            )
            .await
            .is_err()
    );
    let done = fx
        .engine
        .confirm_reflection_disclosure(
            "eng",
            "overview.md",
            &section,
            &basis,
            &ReflectionCaller::person(user("sam"), "sam"),
        )
        .await
        .unwrap();
    assert_eq!(done["disclosed"]["by"], json!("sam"), "{done}");
    assert!(too_wide(&fx, &agent_as("keeper")).await.is_empty());
}

/// A loop run holding a lease: its caller confirms nothing on any surface
/// the engine is asked through.
#[tokio::test]
async fn a_caller_holding_a_live_lease_acts_for_a_run() {
    let (fx, _auth) = private_harbor().await;
    assert!(
        !fx.engine
            .acts_for_a_run(&Scope::Unrestricted)
            .await
            .unwrap()
    );
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        fx.engine
            .acts_for_a_run(&Scope::Unrestricted)
            .await
            .unwrap()
    );
    // Another account holds no lease.
    assert!(!fx.engine.acts_for_a_run(&user("sam")).await.unwrap());
}

// --- MCP ----------------------------------------------------------------

const ERA: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";

pub(crate) struct Wire {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    server: ServedSession<McpServer>,
}

impl Wire {
    pub(crate) fn over(server: McpServer) -> Wire {
        let (client_io, server_io) = tokio::io::duplex(1 << 18);
        let server = ServedSession::spawn(server, server_io);
        let (read, write) = tokio::io::split(client_io);
        Wire {
            write,
            lines: tokio::io::BufReader::new(read).lines(),
            server,
        }
    }

    pub(crate) async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    async fn recv(&mut self) -> Option<Value> {
        match tokio::time::timeout(Duration::from_millis(3000), self.lines.next_line()).await {
            Ok(Ok(Some(line))) => Some(serde_json::from_str(&line).unwrap()),
            _ => None,
        }
    }

    /// Send `message` and read the next message the server sends.
    pub(crate) async fn call(&mut self, message: Value) -> Value {
        let chooses = !self.server.is_started() && chooses_lifecycle(&message);
        self.send(message).await;
        let answer = self.recv().await.expect("the server answered");
        if chooses {
            self.server.started().await;
        }
        answer
    }
}

pub(crate) fn request(id: u32, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A modern request, from a client that can put a question to its user when
/// `eliciting`.
fn modern(id: u32, eliciting: bool, mut params: Value) -> Value {
    let capabilities = if eliciting {
        json!({ "elicitation": { "form": {} } })
    } else {
        json!({})
    };
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": capabilities,
        "io.modelcontextprotocol/clientInfo": { "name": "disclosure-test", "version": "1.0.0" },
    });
    request(id, "tools/call", params)
}

/// The `write_reflection` call that cites harbor on a shared page.
fn citing(extra: Value) -> Value {
    let mut arguments = json!({
        "domain": "eng",
        "path": "overview.md",
        "markdown": OVERVIEW,
        "sources": { "Team": [TEAM] },
    });
    for (key, value) in extra.as_object().unwrap() {
        arguments[key] = value.clone();
    }
    json!({ "name": "write_reflection", "arguments": arguments })
}

/// The text a refusal carries: a protocol error's message, or a refused
/// result's text.
fn refused_text(answer: &Value) -> String {
    if let Some(message) = answer["error"]["message"].as_str() {
        return message.to_string();
    }
    assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
    answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn landed(answer: &Value) {
    assert!(
        answer["error"].is_null() && answer["result"]["isError"] != json!(true),
        "the write lands: {answer}"
    );
}

/// The `confirm` answer of the person.
fn answer(action: &str, content: Value) -> Value {
    json!({ "confirm": { "action": action, "content": content } })
}

/// Round one of a modern eliciting call: the question, and the state the
/// second round has to carry.
async fn asked(wire: &mut Wire, id: u32) -> (String, String) {
    let first = wire.call(modern(id, true, citing(json!({})))).await;
    assert_eq!(
        first["result"]["resultType"],
        json!("input_required"),
        "{first}"
    );
    let message = first["result"]["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    let schema = &first["result"]["inputRequests"]["confirm"]["params"]["requestedSchema"];
    assert_eq!(
        schema["properties"]["confirm"]["default"],
        json!(false),
        "the box starts unchecked: {schema}"
    );
    let state = first["result"]["requestState"]
        .as_str()
        .expect("the question is bound to a state")
        .to_string();
    (message, state)
}

fn second_round(id: u32, state: &str, responses: Value) -> Value {
    let mut params = citing(json!({}));
    params["inputResponses"] = responses;
    params["requestState"] = json!(state);
    modern(id, true, params)
}

/// A modern eliciting peer is asked; the person's yes lands the write with
/// the owner recorded as the one who confirmed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_person_is_asked_and_their_yes_lands_the_write() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()));
    let (message, state) = asked(&mut wire, 1).await;
    assert!(
        message.contains("everyone who can open this instance"),
        "{message}"
    );
    assert!(!page_exists(&fx), "round one writes nothing");
    let done = wire
        .call(second_round(
            2,
            &state,
            answer("accept", json!({ "confirm": true })),
        ))
        .await;
    landed(&done);
    let section = team_section(&fx, &ReflectionCaller::owner()).await;
    assert_eq!(section["disclosed"]["by"], json!("owner"), "{section}");
}

/// A decline, an accept with the box left unchecked and an accept that
/// leaves the box out are all no.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_decline_or_an_unchecked_box_refuses() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()));
    for (n, response) in [
        answer("decline", json!({})),
        answer("accept", json!({ "confirm": false })),
        answer("accept", json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let id = 10 * (n as u32 + 1);
        let (_, state) = asked(&mut wire, id).await;
        let done = wire.call(second_round(id + 1, &state, response)).await;
        let text = refused_text(&done);
        assert!(text.contains(TOO_WIDE), "{text}");
        assert!(!page_exists(&fx));
    }
}

/// An accept nobody was asked for (no state, a made-up state, or a state
/// already used) confirms nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_to_a_question_never_asked_confirms_nothing() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()));
    let yes = answer("accept", json!({ "confirm": true }));

    let mut forged = citing(json!({}));
    forged["inputResponses"] = yes.clone();
    let done = wire.call(modern(1, true, forged)).await;
    assert_ne!(done["result"]["resultType"], json!("complete"), "{done}");
    assert!(!page_exists(&fx));

    let done = wire.call(second_round(2, "made-up", yes.clone())).await;
    assert!(refused_text(&done).contains(TOO_WIDE));
    assert!(!page_exists(&fx));

    let (_, state) = asked(&mut wire, 3).await;
    let done = wire
        .call(second_round(4, &state, answer("decline", json!({}))))
        .await;
    refused_text(&done);
    let done = wire.call(second_round(5, &state, yes)).await;
    assert!(
        refused_text(&done).contains(TOO_WIDE),
        "a state is used once"
    );
    assert!(!page_exists(&fx));
}

/// A state answers only the call it was asked about.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_state_does_not_carry_over_to_another_call() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()));
    let (_, state) = asked(&mut wire, 1).await;
    let mut other = citing(json!({ "path": "other.md" }));
    other["inputResponses"] = answer("accept", json!({ "confirm": true }));
    other["requestState"] = json!(state);
    let done = wire.call(modern(2, true, other)).await;
    assert!(refused_text(&done).contains(TOO_WIDE), "{done}");
    assert!(!fx.root.join("reflections/other.md").exists());
}

/// A flag the agent sets in the arguments has no effect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flag_in_the_arguments_confirms_nothing() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()));
    let done = wire
        .call(modern(
            1,
            false,
            citing(json!({ "disclosed": true, "disclose": true, "confirm": true })),
        ))
        .await;
    let text = refused_text(&done);
    assert!(text.contains(TOO_WIDE), "{text}");
    assert!(!page_exists(&fx));
}

/// A client that cannot ask its person: refused, and the refusal says a
/// person can share it on purpose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_cannot_ask_is_refused_with_how_a_person_can() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()));
    let done = wire.call(modern(1, false, citing(json!({})))).await;
    let text = refused_text(&done);
    assert!(text.contains(TOO_WIDE), "{text}");
    assert!(text.contains(NOBODY_ASKED), "{text}");
    assert!(!page_exists(&fx));
}

/// A session marked unattended (a headless run's) is never asked, even
/// when its client declares it could.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unattended_session_is_never_asked() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()).with_unattended(true));
    let done = wire.call(modern(1, true, citing(json!({})))).await;
    // It does not even see harbor (`read_limit`): the source is refused as
    // a domain that does not exist, and nobody is asked.
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "{done}"
    );
    let text = refused_text(&done);
    assert!(!text.contains(NOBODY_ASKED), "{text}");
    assert!(!page_exists(&fx));
}

/// While a loop's lease is held with no account (the owner's own run), the
/// owner's sessions are never asked: a run and its person look alike there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_is_not_asked_while_its_run_holds_a_lease() {
    let (fx, _auth) = private_harbor().await;
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()));
    let done = wire.call(modern(1, true, citing(json!({})))).await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "{done}"
    );
    refused_text(&done);
    assert!(!page_exists(&fx));
}

/// A legacy peer that declared elicitation gets the question as an
/// `elicitation/create` request on the same call; its yes lands the write,
/// its no refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_person_is_asked_through_an_elicitation_request() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = Wire::over(McpServer::new(fx.engine.clone()));
    let opened = wire
        .call(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "legacy-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(opened["result"]["protocolVersion"], json!(LEGACY));
    wire.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;

    for (id, yes) in [(2u32, false), (3u32, true)] {
        let question = wire
            .call(request(id, "tools/call", citing(json!({}))))
            .await;
        assert_eq!(
            question["method"],
            json!("elicitation/create"),
            "{question}"
        );
        let message = question["params"]["message"].as_str().unwrap();
        assert!(message.contains("harbor"), "{message}");
        assert_eq!(
            question["params"]["requestedSchema"]["properties"]["confirm"]["default"],
            json!(false)
        );
        let reply = if yes {
            json!({ "action": "accept", "content": { "confirm": true } })
        } else {
            json!({ "action": "decline" })
        };
        let done = wire
            .call(json!({ "jsonrpc": "2.0", "id": question["id"], "result": reply }))
            .await;
        if yes {
            landed(&done);
        } else {
            assert!(refused_text(&done).contains(TOO_WIDE));
            assert!(!page_exists(&fx));
        }
    }
    assert!(page_exists(&fx));
}

// --- the JSON API -------------------------------------------------------

/// Fluid's Review confirms a `source_too_wide` item over the JSON API: the
/// queue item carries the question, a curator who may read the source shares
/// it, and one who may not is refused without the domain being named.
#[tokio::test]
async fn the_json_api_confirms_a_named_section_for_a_person() {
    let fx = crate::sources::crossing().await;
    fx.engine
        .write_reflection(&cite(), &agent_as("keeper"))
        .await
        .unwrap();
    let served = crate::rest::serve_fx(fx, false).await;
    // harbor narrows to keeper: the page everyone reads is too wide now.
    served
        .auth
        .set_domain_visibility("harbor", true, "keeper")
        .await
        .unwrap();
    let keeper = crate::rest::Session::login(served.addr, "keeper").await;
    let (status, review) = keeper.get("/api/v1/domains/eng/reflection/review").await;
    assert_eq!(status, 200, "{review}");
    let item = review["queue"]["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["what"] == "source_too_wide")
        .cloned()
        .expect("the section is named");
    assert!(
        item["question"]
            .as_str()
            .unwrap()
            .contains("everyone who can open this instance"),
        "{item}"
    );
    let body = json!({
        "path": "overview.md",
        "section": item["section"],
        "basis": item["basis"],
    });

    // ed curates eng and may not read harbor.
    let ed = crate::rest::Session::login(served.addr, "ed").await;
    let (status, refused) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/disclose",
            Some(body.clone()),
        )
        .await;
    assert_eq!(status, 403, "{refused}");
    assert!(!refused.to_string().contains("harbor"), "{refused}");

    let (status, done) = keeper
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/disclose",
            Some(body),
        )
        .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["disclosed"]["by"], json!("keeper"), "{done}");
    let (_, review) = keeper.get("/api/v1/domains/eng/reflection/review").await;
    assert!(
        !review["queue"]["queue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["what"] == "source_too_wide"),
        "{review}"
    );
}
