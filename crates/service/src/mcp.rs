//! The rmcp tool router: the core tools of the v1 MCP surface plus the
//! collaboration tools, which are listed once team collaboration is on and
//! refuse - rather than vanish from dispatch - while it is off.
//!
//! # One list for every client, at any given instant (SEP-2567)
//!
//! MCP 2026-07-28 says a server's `tools/list` result "MAY change over time
//! [...] but MUST NOT vary per-connection or as a side effect of other
//! requests on the connection", and the same sentence governs
//! `resources/list` and `prompts/list`. The rule this file applies, which
//! covers both halves of that sentence:
//!
//! > A gate may stay on the listing if and only if (a) its input is not
//! > derived from the identity, capabilities or configuration of the
//! > connecting client, and (b) its input is a single value on the shared
//! > instance, so that every client listing at the same moment is served the
//! > same list. A gate whose input is instance-wide but mutable owes an
//! > announcement; one failing (a), or with no single value to point at,
//! > refuses at call time instead.
//!
//! `read_only` passes with nothing to announce: it is a construction field on
//! the engine (`Engine::with_read_only` takes `self` by value) and the engine
//! is shared behind an `Arc`, so nothing a client sends can move it.
//! `skills.serve` is snapshotted at the same point and behaves the same way.
//! `github.enabled` passes with an announcement: it is one setting on the
//! shared engine, `configure` can flip it, and a flip pushes
//! `notifications/tools/list_changed` to every open subscription. Whether any
//! domain declares provisioning has no single setting behind it - `add_domain`
//! and `update_domain` can create a declaration mid-call - so `provision` is
//! listed always and refuses its mutating actions. The client's
//! install-receipt match failed (a) and is gone from the listing entirely.
//!
//! Refusing rather than hiding is SEP-2567's own prescription where hiding
//! would be per-connection: expose the tool unconditionally and put the
//! dependency "in the tool's input schema and description rather than in the
//! list result". Every gate here hides without unregistering a route either
//! way, so a client calling a tool it cannot see is always told why rather
//! than answered "no such tool".
//!
//! Each tool is a thin wrapper over [`crate::engine::Engine`], which does the
//! real work and is shared with the CLI data commands. Tool descriptions are
//! agent-facing product copy framed around onboarding, teaching, learning and
//! experience. The recommended `type` and `status` value sets are stated once,
//! in the `write_engram` description, as guidance that is never enforced;
//! `edit_engram` points back to it rather than repeating the list. Every
//! mutating tool requires an explicit domain.
//!
//! The server handshake (`get_info`) hands each connecting agent the live
//! routing block as its `instructions`, rendered from the engine by
//! [`crate::engine::Engine::routing_text`]: the same CRYSTALLINE KNOWLEDGE
//! ROUTING onboarding the CLI `prompt system` emits, minus any workspace
//! scoping, so an agent is routed the moment it connects with no skill or hook
//! required. It re-fetches mid-session through `list_domains` with
//! `include_routing=true`, the same index the instructions carry.
//!
//! In read-only mode (the engine's `read_only` flag) the write-gated tools are
//! filtered out of `list_tools` and `get_tool`; the routes stay registered so
//! a client that calls a hidden tool by name reaches the engine's read-only
//! guard and gets a clean error. That gate is legitimate on the listing
//! because the mode is fixed for the engine's lifetime.
//!
//! The six collaboration tools (`configure`, `share_changes`, `update_domain`,
//! `origin_status`, `resolve_conflict`, `withdraw_proposal`) carry two gates
//! that compose. `configure`, `share_changes`, `resolve_conflict` and
//! `withdraw_proposal` disappear read-only. `add_domain` is deliberately not
//! one of the six: it creates domains of every kind, so it is write-gated like
//! any other writer (see `WRITE_TOOLS`) and only its team-domain branch needs
//! `github.enabled`, enforced in the engine rather than on the listing.
//! `github.enabled` is needed by every collaboration tool but `configure`, and
//! while it is off the five that need it are hidden from
//! the listing too, so a default install spends no context on a forge surface
//! nobody connected; `configure` is never hidden by it, since it is the only
//! way to turn the rest on. Calling a hidden one still answers with
//! `RemoteError::NotEnabled`'s message, which names the setting and both ways
//! to change it, so a stale cached list teaches rather than dead-ends. Turning
//! the setting on makes all five appear on the next list and announces the
//! change to every open subscription. See `COLLAB_TOOLS`,
//! `COLLAB_WRITE_TOOLS`, `hidden_collab_tool` and `refused_collab_tool`.
//!
//! `evolve_engrams` is gated a third way, on the read-only flag alone. It is a
//! pure read, so it is not one of the `WRITE_TOOLS`, but every finding it
//! returns prescribes a mutation and a queue of work that cannot be worked is
//! noise where mutation is impossible. See `hidden_evolve_tool`; the route
//! stays registered like every other hidden tool, so a call by name still
//! sweeps and answers.
//!
//! One more tool, `provision`, is gated a fourth way, and it splits across
//! the rule too: hidden read-only, since every action but `status` writes,
//! and refusing while no registered domain's MANIFEST declares a
//! `## Provisioning` section (see [`Engine::provisioning_declared`], which
//! `add_domain` and `update_domain` can flip on the same connection).
//! `status` is never refused - with nothing declared it answers a real, empty
//! report, which is how a caller learns there is nothing to decide - and the
//! three reconciling actions refuse rather than report a success that changed
//! nothing. See `hidden_provision_tool` and `refused_provision_action`.
//!
//! The shipped agent skills are served here too, so a remote client that never
//! runs `crystalline install` can still learn how to use Crystalline well:
//! the `skills` tool (an index with no arguments, one skill's full `SKILL.md`
//! with `name`), five `skill://<name>/SKILL.md` resources and two prompts,
//! `onboarding` (the live routing block) and `connector` (the static snippet
//! that teaches a client to onboard itself). The whole surface shares one
//! gate, the live `skills.serve` setting; when it hides the surface the tool,
//! the resource list and the prompt list are empty while direct reads keep
//! answering, the same hidden-not-disabled doctrine the tool gates follow.
//!
//! That gate is tri-state. `true` and `false` force always and never, and
//! `auto`, the default, withholds the surface from a session whose spawning
//! harness already has the five skills on disk. **That answer is resolved
//! before the session starts and is fixed for the serving process's life**:
//! `crystalline install` registers the server as `crystalline mcp --harness
//! <name>`, the spawned process asks this machine's install receipt whether
//! that harness has session hooks wired, and it carries the answer for the
//! connection's lifetime (over the daemon relay it rides the private
//! handshake line, re-sent on every reconnect, and the daemon never
//! re-derives it). Both inputs are deployment configuration and machine
//! state; neither is the connecting client, which is what clause (a) of the
//! rule above needs.
//!
//! It used to be decided from the client's own `initialize` name matched
//! against the same receipt, which is per-connection variation verbatim. The
//! saving survives the move; the mechanism could not.
//!
//! Two consequences worth stating where they cannot be missed. The value the
//! setting resolves to is snapshotted at engine construction
//! (`Engine::skills_serve`), so a `configure` write applies at the next daemon
//! start rather than to the connection that wrote it, which is clause (b). And
//! an HTTP session is never suppressed: one daemon serves every HTTP client, a
//! remote client never ran the CLI here, and a remote client is exactly who
//! the served surface exists for. `docs/deployment.md` documents that
//! asymmetry, how to see which answer a stdio session will get and how to turn
//! it off.
//!
//! # The routing block, and the one way it can silently not arrive
//!
//! `get_info` fills `instructions` with the live routing block and
//! `arrival_info` applies the decision above to it. Which channel carries it
//! to the client is the protocol revision's business, not ours: every
//! revision before 2026-07-28 reads it out of `InitializeResult`, and
//! 2026-07-28 deletes the handshake outright and moves `instructions` to
//! `DiscoverResult`, where `discover` answers it.
//!
//! **A modern client is free never to call `server/discover`, and if it does
//! not it receives no instructions and nothing errors anywhere.** That is the
//! failure mode this file's `discover` doc comment spells out with its
//! evidence. The mitigations - the `onboarding` prompt, `list_domains` with
//! `include_routing=true`, the served skills - are all pull-shaped and all
//! need the client to know to ask. `tests/mcp_instructions.rs` drives the
//! block over every advertised revision by that revision's own path, so a
//! revision added without an onboarding path fails there rather than shipping
//! silence.
//!
//! The resource shape follows the converging skills-over-MCP proposal
//! without advertising its extension id, which is not ratified yet. The
//! prompts are declared with rmcp's `#[prompt_router]`/`#[prompt]` macros but
//! `list_prompts` and `get_prompt` are hand-written, since
//! `#[prompt_handler]` replaces any `list_prompts` in its impl block and the
//! gate needs one it can empty.
//!
//! # One list can change, and it is announced to subscribers only
//!
//! `configure` flipping `github.enabled` moves the tool list, because the five
//! GitHub-gated collaboration tools are listed only while it is on. That is
//! the single mover on this server: `resources/list` and `prompts/list` read
//! `skills.serve` and `harness_onboarded`, both fixed before the first request
//! arrives, and the provisioning gate that `add_domain` and `update_domain`
//! could once move became a call-time refusal instead.
//!
//! MCP 2026-07-28 removes the unsolicited channel outright - a notification
//! either rides a `subscriptions/listen` stream the client opened or it does
//! not exist - so the flip announces itself through
//! [`McpServer::accepted_subscription_filter`] and [`McpServer::listen`], on
//! the sink registry the shared engine holds (`crate::subscribers`). A legacy
//! peer cannot subscribe and is therefore told nothing at all; it re-reads
//! `tools/list` at its own discretion, which is the same contract it had
//! before.
//!
//! # Asking before destroying (SEP-2322)
//!
//! A tool that cannot be undone answers with a question instead of acting,
//! whenever the peer can carry one: `delete_engram` returns an
//! `input_required` result holding a single form elicitation, the client puts
//! it to its user, and the same call arrives again with the answer beside the
//! original arguments. [`confirmation_supported`] decides whether to ask,
//! [`confirm_question`] builds the question and [`confirmed`] reads the
//! answer; all three are deliberately tool-agnostic.
//!
//! **Two more rounds are built on those three.** `edit_engram` asks before it
//! records an `evolve_ack` or takes one back, and `write_engram` asks on a
//! permalink collision - the one round whose question is not a yes-or-no, so
//! it brings a single-select of its own ([`collision_question`],
//! [`resolved_overwrite`]) and a third condition beside the gate: a call that
//! already passed `overwrite` answered the question before it was put.
//!
//! **A question is only put about a call that can run.** Both rounds that take
//! an identifier resolve it before they ask - `delete_engram` through
//! [`crate::engine::Engine::delete_preview`], the `evolve_ack` round through
//! [`crate::engine::Engine::ack_preview`] - so a read-only server, a domain
//! nobody registered and an identifier nobody has each fail in round one, and
//! the question names what resolution found rather than what was typed. The
//! collision round needs no such step: the write itself is what discovers the
//! collision, and the question is built from the failure.
//!
//! **An answer is not bound to the arguments it was asked about.** The client
//! re-sends the original arguments beside the answer and nothing on this side
//! remembers what was asked, so a buggy client that changes an argument on the
//! retry is honoured rather than caught - the price of the stateless design
//! (see [`confirm_question`] on why nothing is sealed into `requestState`),
//! and the reason each round's refusal is read before the act it guards rather
//! than after it.
//!
//! **The gate decides whether the flow exists at all, not how it behaves.** A
//! peer below 2026-07-28 has no result shape to receive a question in, and a
//! peer that never declared an elicitation capability has no way to ask its
//! user; either one is served exactly what 0.15.0 served it, one call and one
//! deletion, because a confirmation nobody can answer is a hang. Nothing here
//! is a permission system: a client is free to answer its own question, and
//! the CLI's own dispatch (`crate::client::dispatch_engine`) never asks at all,
//! because the human already typed the verb.
//!
//! Every tool also advertises MCP tool annotations: a display `title` plus the
//! readOnly/destructive/idempotent/openWorld hints, so a client can tune its
//! confirmation UX and batch the read-only calls. The hints are advisory only;
//! enforcement stays the runtime gating (`WRITE_TOOLS`, `hidden_collab_tool`,
//! `refused_collab_tool`) and the engine guards. Two calls are deliberate:
//! `write_engram` advertises
//! non-destructive because its default behaviour is additive (it errors on an
//! existing permalink unless `overwrite`), and `open_world` is true only for
//! the tools that talk to GitHub - `configure` through its connect flow,
//! `add_domain` through team mode, `share_changes`, `update_domain`,
//! `origin_status` and `withdraw_proposal`.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::handler::server::prompt::PromptContext;
use rmcp::handler::server::tool::InputResponses;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CacheScope, CallToolResponse, CallToolResult, ContentBlock, DiscoverResult, ElicitRequest,
    ElicitRequestParams, ElicitationSchema, EnumSchema, ErrorData, GetPromptRequestParams,
    GetPromptResponse, Implementation, InitializeRequestParams, InitializeResult, InputRequest,
    InputRequests, InputRequiredResult, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProgressNotificationParam,
    PromptMessage, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, ResourceTemplate, Role, ServerCapabilities,
    ServerInfo, SubscriptionFilter, Tool,
};
use rmcp::service::{RequestContext, SubscriptionContext};
use rmcp::{RoleServer, ServerHandler, prompt, prompt_router, tool, tool_handler, tool_router};
use serde_json::{Value, json};

use crystalline_core::{CrystallineUrl, SKILL_ASSETS};
use crystalline_remote::RemoteError;

/// The tools hidden in read-only mode: the four content-mutating engram tools
/// plus `add_domain`, which creates a domain (writing config, and files for a
/// local domain). In read-only mode they are hidden from `list_tools` and
/// `get_tool`, while their routes stay registered so a client that calls one by
/// name still reaches the engine guard and gets the read-only error rather than
/// a bare "tool not found".
const WRITE_TOOLS: [&str; 5] = [
    "write_engram",
    "edit_engram",
    "move_engram",
    "delete_engram",
    "add_domain",
];

/// Whether a tool name is one of the write-gated tools (hidden in read-only
/// mode).
fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

/// Every MCP protocol revision this server serves, oldest first.
///
/// **Spelled out literally rather than filtered from
/// [`ProtocolVersion::KNOWN_VERSIONS`]** so an rmcp upgrade can never widen what
/// we advertise as a side effect of a dependency bump: adding a revision here is
/// an edit somebody made on purpose. Both ends of the range are pinned by
/// `the_advertised_protocol_set_is_exactly_this` in
/// `tests/mcp_instructions.rs`, which also pins rmcp's own list, so a crate that
/// learns a new revision fails the build and asks for the decision instead of
/// taking it.
///
/// **The top is a decision, and it was taken on 2026-08-14.** `V_2026_07_28`
/// is served. That revision makes list endpoints connection-invariant, moves
/// `instructions` to `server/discover`, restricts `tools/list_changed` to
/// subscribers and requires caching hints on six operations; all four are
/// implemented here - see [`McpServer::list_tools`], [`McpServer::discover`],
/// [`McpServer::listen`] and [`CacheHinted`] - and so is the stdio bridge's
/// half, where a bare `server/discover` probe is normalized and forwarded
/// rather than answered `-32601` (`crate::client`). A fifth obligation,
/// `ping`'s removal, is rmcp's: it answers `method_not_found` to any peer that
/// is not on the legacy lifecycle (`handler/server.rs:112-118`), and we
/// implement no `ping`. `tests/mcp_modern_era.rs` is what a client at this
/// revision actually receives, over both transports.
///
/// **The bottom is deliberately NOT a decision.** `V_2024_11_05` is served today
/// and stays served: rmcp branches nowhere between it and `V_2025_11_25`
/// (`uses_legacy_lifecycle`, rmcp 3.1.2 `service.rs:196-202`, one `<` comparison
/// against 2026-07-28), so keeping the oldest costs one array element, and
/// dropping a revision is a deprecation with a release note rather than a
/// side effect of an upgrade.
pub const SERVED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];

/// The newest revision we serve: what a client asking for one we do not serve
/// is answered with over stdio, and the version the stdio bridge injects into a
/// bare `server/discover` probe (`crate::client`). Reads the last element, so
/// the ordering of [`SERVED_PROTOCOL_VERSIONS`] is load bearing rather than
/// cosmetic.
pub(crate) fn newest_served_protocol_version() -> ProtocolVersion {
    SERVED_PROTOCOL_VERSIONS
        .last()
        .expect("SERVED_PROTOCOL_VERSIONS is never empty")
        .clone()
}

/// The newest revision we serve that still **has** an `initialize` handshake,
/// which is what a legacy-shaped handshake naming a version we do not serve is
/// answered with.
///
/// **Not simply the newest we serve, and the difference is the point.** The
/// 2026-07-28 schema deletes the handshake outright (`grep -i initialize` over
/// its `schema.ts` returns zero hits), so answering an `initialize` with that
/// revision tells a client "speak the era that has no such request", and under
/// the legacy lifecycle rules a client that cannot speak the returned version
/// SHOULD disconnect rather than proceed. It also has a concrete cost: rmcp
/// keys `ping`'s removal, the `resultType` discriminator and the subscription
/// dispatch on the peer's **negotiated** version (`handler/server.rs:112-118`,
/// `:246-260`, `uses_legacy_lifecycle` at `service.rs:196-202`), so a client
/// downgraded onto the era would lose `ping` without ever having asked for the
/// era. Capping the downgrade means a peer reaches the modern lifecycle only
/// by asking for it - by opening with `server/discover`, or by naming
/// 2026-07-28 in its own handshake, both of which are still echoed verbatim.
pub(crate) fn newest_legacy_handshake_version() -> ProtocolVersion {
    SERVED_PROTOCOL_VERSIONS
        .iter()
        .rfind(|version| **version < ProtocolVersion::V_2026_07_28)
        .cloned()
        .unwrap_or_else(newest_served_protocol_version)
}

/// How long a cacheable result may be treated as fresh, in milliseconds.
///
/// Zero, which is what rmcp's own `#[tool_handler]` and `#[prompt_handler]`
/// macros emit for the endpoints they generate
/// (`rmcp-macros-3.1.2/src/tool_handler.rs:79-81`,
/// `prompt_handler.rs:71-73`). Deliberately the same number: a hand-written
/// endpoint and a generated one must be indistinguishable on the wire, and a
/// server that names a longer window is promising something about a future it
/// does not control - a daemon restart with different configuration serves a
/// different list.
const CACHE_TTL_MS: u64 = 0;

/// Who may cache a result of ours.
///
/// [`CacheScope::Public`] is truthful rather than convenient, and it became
/// truthful only once the list endpoints stopped varying per connection: every
/// list this server answers is decided before the first request from
/// deployment configuration and machine state, never from who is asking, and
/// none of it varies by the authorization presented on the request - which is
/// the one variation SEP-2567 explicitly permits and the one that would force
/// `private`. The shipped skills a `resources/read` returns are static copy
/// compiled into this binary.
const CACHE_SCOPE: CacheScope = CacheScope::Public;

/// Whether the peer this request belongs to gets SEP-2549 caching hints.
///
/// The gate is [`RequestContext::protocol_version`] (rmcp 3.1.2
/// `service.rs:1223-1229`: the request's own `_meta` version first, then the
/// version the peer negotiated), compared with `>=` exactly as rmcp's macros
/// compare it. `>=` rather than `==` on purpose: `ProtocolVersion` derives
/// `PartialOrd` over its string (`model.rs:153-155`) and ISO dates order
/// lexicographically, so a revision newer than 2026-07-28 keeps the obligation
/// instead of silently losing it.
///
/// The fields did not exist before 2026-07-28, so the negative half matters as
/// much as the positive one: emitting them to a legacy peer would be inventing
/// wire shape for a revision that has none.
fn peer_gets_cache_hints(context: &RequestContext<RoleServer>) -> bool {
    context
        .protocol_version()
        .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
}

/// The key the confirmation question and its answer are both filed under.
///
/// One name for the request in [`confirm_question`]'s `inputRequests` map, for
/// the single boolean property inside that question's schema, and for the
/// entry [`confirmed`] reads back out of the client's `inputResponses`.
const CONFIRM_KEY: &str = "confirm";

/// The key the collision question and its answer are both filed under, and the
/// two choices it offers. `overwrite` is spelled exactly as `write_engram`'s
/// own parameter, so the answer and the retry say the same word.
const RESOLUTION_KEY: &str = "resolution";
const RESOLUTION_OVERWRITE: &str = "overwrite";
const RESOLUTION_CANCEL: &str = "cancel";

/// The three resolutions `resolve_conflict` accepts, spelled once.
///
/// Each is spelled exactly as that tool's own `resolution` parameter, so the
/// word the question offers, the word a client answers with and the word the
/// dispatch acts on cannot drift apart. Only the first two are offered as
/// choices: merged is not a choice a form can collect, because it needs a
/// document rather than a pick, so it appears in the schema nowhere and in the
/// guidance everywhere.
const RESOLUTION_MINE: &str = "mine";
const RESOLUTION_THEIRS: &str = "theirs";
const RESOLUTION_MERGED: &str = "merged";

/// The substring of the engine's permalink-collision error that identifies it.
///
/// The engine words one message for this failure
/// (`crate::engine::Engine::write_engram_as`) and this is the phrase it is
/// recognized by; `a_permalink_collision_carries_the_marker_the_mcp_layer_intercepts`
/// in `tests/engine_writes.rs` pins it there, so a rewording breaks a test
/// beside the sentence rather than silently disarming the round here.
const COLLISION_MARKER: &str = "already exists in domain";

/// Whether this peer can be asked before a destructive tool acts.
///
/// Two conditions, both necessary. The revision has to be 2026-07-28 or newer,
/// because SEP-2322's `input_required` result is what carries a question back
/// on the same call and rmcp refuses to hand one to an older peer at all
/// (`model/mrtr.rs:18-20`); that half is [`peer_gets_cache_hints`], reused
/// rather than rewritten because it is the same era test under a name that
/// happens to mention the first obligation we needed it for. And the client
/// has to have said it can elicit, because a server that asks a client with no
/// way to ask its user has simply hung the call.
///
/// **Any declared elicitation counts, including a bare `{}`.** The capability
/// split into `form` and `url` sub-capabilities after elicitation shipped, so
/// a client from before the split declares the empty object and means "yes,
/// forms"; refusing that would silently drop the confirmation for the clients
/// most likely to need it. The other direction is the price: a url-only client
/// that cannot render a form question will decline it, and a decline is
/// already a clean refusal that deletes nothing. Asking and being told no
/// costs a round trip; not asking costs the confirmation.
fn confirmation_supported(context: &RequestContext<RoleServer>) -> bool {
    peer_gets_cache_hints(context)
        && context
            .client_capabilities()
            .is_some_and(|capabilities| capabilities.elicitation.is_some())
}

/// One yes-or-no question, as the MRTR round a tool returns instead of acting.
///
/// A form elicitation with exactly one required boolean property, so a client
/// has a schema to render and an unambiguous shape to send back. Nothing is
/// sealed into `requestState` and none is asked for: the flows that use this
/// are stateless by construction - the client echoes the original arguments on
/// the retry and its answer is the whole of the state - which is why the
/// `request-state` feature is not enabled on rmcp.
fn confirm_question(message: String) -> InputRequiredResult {
    let requested_schema = ElicitationSchema::builder()
        .required_bool_property(CONFIRM_KEY, |schema| {
            schema
                .title("Confirm")
                .description("Yes to go ahead. Anything else leaves everything as it is.")
        })
        .build()
        .expect("the confirmation schema names the property it requires");
    form_question(CONFIRM_KEY, message, requested_schema)
}

/// One form elicitation, filed under the key its single property carries.
///
/// The shape every round in this file returns: one request in the map, keyed
/// the same as the property inside it, so the client's answer comes back under
/// a name the reader already knows. Split out of [`confirm_question`] when the
/// second question stopped being a boolean.
fn form_question(
    key: &str,
    message: String,
    requested_schema: ElicitationSchema,
) -> InputRequiredResult {
    let mut requests = InputRequests::new();
    requests.insert(
        key.to_string(),
        InputRequest::Elicitation(ElicitRequest::new(
            ElicitRequestParams::FormElicitationParams {
                meta: None,
                message,
                requested_schema,
            },
        )),
    );
    InputRequiredResult::from_input_requests(requests)
}

/// What the client answered to [`confirm_question`], or `None` when it has not
/// been asked yet.
///
/// `Some(true)` only for an accepted question whose content says `true`.
/// `Some(false)` for everything else that is an answer: a decline, a cancel,
/// an accept carrying `false`, or an accept whose content is missing the
/// property it was asked for. `None` - the first round - when the call carries
/// no responses at all or none under [`CONFIRM_KEY`].
///
/// The value is read as plain JSON rather than deserialized into
/// `ElicitResult`, and the difference is the failure mode: `ElicitationAction`
/// is a closed three-variant enum, so a client answering with an action a
/// later revision adds would fail to deserialize and turn a "no" into an
/// error. Read this way, anything that is not exactly `accept` is a no, which
/// is the only reading that cannot delete something.
fn confirmed(responses: &Option<rmcp::model::InputResponses>) -> Option<bool> {
    let answer = responses.as_ref()?.get(CONFIRM_KEY)?;
    if answer["action"] != json!("accept") {
        return Some(false);
    }
    Some(answer["content"][CONFIRM_KEY] == json!(true))
}

/// The choice a permalink collision offers, as the MRTR round `write_engram`
/// returns instead of the bare error.
///
/// A single-select enum rather than [`confirm_question`]'s boolean, because
/// the collision is not a yes-or-no: "no" here means "leave what is there",
/// which is a decision worth a word of its own rather than the absence of a
/// yes. The options are titled, so a client renders two sentences instead of
/// two identifiers, and `cancel` is deliberately not the schema default -
/// nothing is preselected, because either answer is a real choice.
fn collision_question(message: String) -> InputRequiredResult {
    let choices = EnumSchema::builder(vec![
        RESOLUTION_OVERWRITE.to_string(),
        RESOLUTION_CANCEL.to_string(),
    ])
    .title("Resolution")
    .description("What to do about the engram already at that permalink.")
    .enum_titles(vec![
        "Overwrite the existing engram".to_string(),
        "Cancel and write nothing".to_string(),
    ])
    .expect("two titles for two choices")
    .build();
    let requested_schema = ElicitationSchema::builder()
        .required_enum_schema(RESOLUTION_KEY, choices)
        .build()
        .expect("the resolution schema names the property it requires");
    form_question(RESOLUTION_KEY, message, requested_schema)
}

/// Whether the client chose to overwrite, or `None` when it has not been asked
/// yet.
///
/// The same tri-state discipline as [`confirmed`], read as plain JSON for the
/// same reason: `Some(true)` for exactly one shape - an accepted question whose
/// content carries the string `overwrite` under the key it was asked for - and
/// `Some(false)` for every other answer, an explicit `cancel` and a decline
/// alike. Only the genuine absence of an answer is [`None`], because that is
/// what opens round one; anything malformed leaves the existing engram alone.
fn resolved_overwrite(responses: &Option<rmcp::model::InputResponses>) -> Option<bool> {
    let answer = responses.as_ref()?.get(RESOLUTION_KEY)?;
    if answer["action"] != json!("accept") {
        return Some(false);
    }
    Some(answer["content"][RESOLUTION_KEY] == json!(RESOLUTION_OVERWRITE))
}

/// The choice an unresolved conflict offers when the caller named no
/// resolution: mine or theirs, titled so a client renders two sentences.
///
/// merged is deliberately not an option - a free-text merge body does not fit
/// a confirm form; the tool description says to call again with
/// resolution merged and content instead. The two words are spelled exactly as
/// `resolve_conflict`'s own `resolution` parameter, so the answer and the
/// retry say the same thing, which is [`collision_question`]'s discipline
/// applied to a second pair of choices.
fn conflict_choice(message: String) -> InputRequiredResult {
    let choices = EnumSchema::builder(vec![
        RESOLUTION_MINE.to_string(),
        RESOLUTION_THEIRS.to_string(),
    ])
    .title("Resolution")
    .description("Which side of the conflict to keep.")
    .enum_titles(vec![
        "Keep my local version".to_string(),
        "Take the team's version".to_string(),
    ])
    .expect("two titles for two choices")
    .build();
    let requested_schema = ElicitationSchema::builder()
        .required_enum_schema(RESOLUTION_KEY, choices)
        .build()
        .expect("the resolution schema names the property it requires");
    form_question(RESOLUTION_KEY, message, requested_schema)
}

/// Which side the client chose, with [`confirmed`]'s tri-state discipline:
/// `None` has not been asked, `Some(None)` is any answer that is not exactly
/// an accepted mine or theirs, and only those two strings pass through.
///
/// Read as plain JSON for [`confirmed`]'s reason, and narrowed to two static
/// strings rather than handing the client's own text on to the engine: a
/// resolution that reaches [`crate::engine::Engine::origin_resolve`] is one of
/// ours, never one a malformed answer smuggled in.
fn chosen_resolution(
    responses: &Option<rmcp::model::InputResponses>,
) -> Option<Option<&'static str>> {
    let answer = responses.as_ref()?.get(RESOLUTION_KEY)?;
    if answer["action"] != json!("accept") {
        return Some(None);
    }
    Some(match answer["content"][RESOLUTION_KEY].as_str() {
        Some(RESOLUTION_MINE) => Some(RESOLUTION_MINE),
        Some(RESOLUTION_THEIRS) => Some(RESOLUTION_THEIRS),
        _ => None,
    })
}

/// Attach the SEP-2549 caching hints a modern peer is owed, and nothing to a
/// legacy one.
///
/// **The obligation, quoted, because six operations is not five call sites.**
/// `/server/utilities/caching`: "Servers MUST include caching hints on results
/// with `resultType: "complete"` returned by the following operations:
/// `server/discover`, `tools/list`, `prompts/list`, `resources/list`,
/// `resources/templates/list`, `resources/read`." `ttlMs` MUST be `>= 0` and
/// `cacheScope` is required because there is no safe default.
///
/// `server/discover` is the one operation nobody has to call this for: rmcp's
/// `DiscoverResult::from_server_info` sets `ttl_ms: 0` and
/// `cache_scope: Private` on non-optional fields (rmcp 3.1.2
/// `model.rs:1258-1263`), and [`McpServer::discover`] builds through it. The
/// other five are ours, on both this server and [`crate::stub::DegradedServer`],
/// including the ones neither of them writes by hand: rmcp's default
/// `list_resource_templates`, `list_resources` and `list_prompts`
/// (`handler/server.rs:373-395`) all return an empty **complete** result with
/// no hints, and `Service::handle_request` (`:50-245`) dispatches every method
/// regardless of the capabilities `get_info` advertises. An un-advertised
/// capability is therefore not a defence against this MUST; an override is.
pub(crate) trait CacheHinted: Sized {
    /// Set both hints, or neither.
    fn with_cache_hints(self, context: &RequestContext<RoleServer>) -> Self;
}

macro_rules! impl_cache_hinted {
    ($($t:ty),+ $(,)?) => {
        $(impl CacheHinted for $t {
            fn with_cache_hints(self, context: &RequestContext<RoleServer>) -> Self {
                if peer_gets_cache_hints(context) {
                    self.with_ttl_ms(CACHE_TTL_MS).with_cache_scope(CACHE_SCOPE)
                } else {
                    self
                }
            }
        })+
    };
}

impl_cache_hinted!(
    ListToolsResult,
    ListPromptsResult,
    ListResourcesResult,
    ListResourceTemplatesResult,
    ReadResourceResult,
);

/// The six GitHub collaboration tools, gated on the engine's live
/// `github.enabled` setting (all but `configure`) and `read_only` flag (see
/// `COLLAB_WRITE_TOOLS`). `add_domain` is not among them: it creates domains of
/// every kind, so it is a write-gated tool (see `WRITE_TOOLS`), and only its
/// team-domain branch needs `github.enabled`, enforced in the engine.
const COLLAB_TOOLS: [&str; 6] = [
    "configure",
    "share_changes",
    "update_domain",
    "origin_status",
    "resolve_conflict",
    "withdraw_proposal",
];

/// Of the six collaboration tools, the four also hidden in read-only mode:
/// `configure` (settings and this machine's GitHub identity are frozen the
/// same way content is), `share_changes`, `resolve_conflict` and
/// `withdraw_proposal` (each writes a proposal or config). `update_domain` and
/// `origin_status` stay visible read-only, mirroring their engine-level
/// exemption (a pull is a derived-truth update like sync; status is a pure
/// read).
const COLLAB_WRITE_TOOLS: [&str; 4] = [
    "configure",
    "share_changes",
    "resolve_conflict",
    "withdraw_proposal",
];

/// Appended to the initialize instructions while TOON responses are active,
/// so a client model reads list results as structured data rather than prose.
const TOON_INSTRUCTIONS_NOTE: &str = "\n\nList-shaped tool results (search hits, activity, listings and status reports) arrive TOON-encoded rather than as JSON: indentation nests objects, `name[N]{field1,field2}:` heads a uniform array with one comma-separated row per record and a tags cell joins its values with commas. Read them as data with exactly those fields.";

/// Whether `name` is one of the six collaboration tools.
fn is_collab_tool(name: &str) -> bool {
    COLLAB_TOOLS.contains(&name)
}

/// Whether the consolidation sweep is hidden given the engine's live read-only
/// state. The tool is a pure read, so it is not one of the `WRITE_TOOLS`, but
/// every finding it returns prescribes a mutation: a queue of work that cannot
/// be worked is noise on an instance where mutation is impossible. Its route
/// stays registered like every other hidden tool (see `list_tools` and
/// `get_tool`), so a call by name still reaches the engine and comes back with
/// a real sweep rather than a bare "tool not found".
fn hidden_evolve_tool(read_only: bool) -> bool {
    read_only
}

/// Whether the `provision` tool is hidden given the engine's read-only state.
/// Every action but `status` writes, so the tool follows the write gate.
///
/// Whether anything currently declares a `## Provisioning` section used to be
/// half of this predicate and is not any more: `add_domain` and
/// `update_domain` can create a declaration on the same connection, which is
/// SEP-2567's side-effect prohibition, so it gates the call instead. See
/// [`refused_provision_action`].
fn hidden_provision_tool(read_only: bool) -> bool {
    read_only
}

/// Whether one `provision` action is refused because no registered domain
/// declares a `## Provisioning` section.
///
/// `status` is never refused: with nothing declared it answers a real, empty
/// report, which is precisely how a caller learns there is nothing to decide.
/// The three actions that reconcile artifacts are refused, because with
/// nothing declared they would report success while doing nothing at all.
fn refused_provision_action(action: &ProvisionAction, provisioning_declared: bool) -> bool {
    !provisioning_declared && !matches!(action, ProvisionAction::Status)
}

/// What a refused `provision` action tells the caller: the thing that has to
/// exist before it can do anything, and the read that reports the state either
/// way.
const PROVISION_NOT_DECLARED: &str = "No registered domain declares a '## Provisioning' section in its MANIFEST, so there is nothing to allow, deny or reconcile. Add one to a domain's MANIFEST first; provision with action status reports the current state either way.";

/// The MIME type every shipped skill is served with, as a resource and in the
/// `skills` tool's full read: a `SKILL.md` is plain markdown.
const SKILL_MIME_TYPE: &str = "text/markdown";

/// The resource uri one shipped skill is served under. The shape follows the
/// converging skills-over-MCP proposal (`skill://<name>/SKILL.md`) without
/// advertising its extension id, which is not ratified yet: a client that
/// learns the shape reads the same bytes either way.
fn skill_uri(name: &str) -> String {
    format!("skill://{name}/SKILL.md")
}

/// The shipped skill a resource uri names, or `None` when the uri is not one
/// of the five this server serves.
fn skill_for_uri(uri: &str) -> Option<&'static crystalline_core::SkillAsset> {
    let name = uri.strip_prefix("skill://")?.strip_suffix("/SKILL.md")?;
    crystalline_core::skill(name)
}

/// The five shipped skill names, comma separated, for an error that names
/// what the caller could have asked for instead.
fn skill_names() -> String {
    SKILL_ASSETS
        .iter()
        .map(|s| s.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The five skill resource uris, comma separated, for the same reason.
fn skill_uris() -> String {
    SKILL_ASSETS
        .iter()
        .map(|s| skill_uri(s.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The RFC 6570 template every attachment is addressed by. `{+path}` is the
/// reserved expansion, so a nested path keeps its separators: an attachment two
/// folders deep is one uri, not one segment.
const ATTACHMENT_URI_TEMPLATE: &str = "crystalline://{domain}/assets/{+path}";

/// The template's programmatic name, and what a client shows beside it.
const ATTACHMENT_TEMPLATE_NAME: &str = "attachment";

/// What the template is for, in the words a model reads before deciding to
/// fetch one.
const ATTACHMENT_TEMPLATE_DESCRIPTION: &str = "A file attachment a human added to a domain: read it here when an engram's resource links or an evolve finding point at it.";

/// Whether the whole skill-serving surface (the `skills` tool, the `skill://`
/// resources and the two prompts) is hidden.
///
/// The three rows are exactly the setting's value set:
///
/// - `true`: never hidden.
/// - `false`: always hidden.
/// - `auto`: served.
///
/// Both inputs are fixed before this server exists, which is what makes the
/// gate legal on a listing at all. `skills_serve` is the effective setting
/// snapshotted at engine construction ([`Engine::skills_serve`]);
/// `harness_onboarded` is [`McpServer::with_onboarded_harness`], resolved by
/// the `crystalline mcp` process from its own `--harness` argument plus this
/// machine's install receipt before the session starts.
///
/// **`auto` used to consult the connecting client**, hiding the surface from a
/// stdio client this machine's install receipt knew as an onboarded harness by
/// its `initialize` name. That is SEP-2567's first prohibition - a list
/// endpoint "MUST NOT vary per-connection" - so the client's identity is gone
/// from here. What replaces it is the same fact learned from the deployment
/// instead of from the wire: the harness that spawned this process already has
/// the five skills on disk and is onboarded by its own session hook, so
/// serving them again spends the tokens twice.
///
/// An HTTP session never sets `harness_onboarded`: one daemon serves every
/// HTTP client, a remote client never ran `crystalline install` here, and a
/// remote client is exactly who the served surface exists for.
///
/// Hidden means hidden, not disabled: the lists come back empty while the
/// tool, the resources and the prompts all keep answering a direct call.
/// Read-only mode is not part of it either: reading a skill is a read.
fn hidden_skills_surface(skills_serve: SkillsServe, harness_onboarded: bool) -> bool {
    match skills_serve {
        SkillsServe::Always => false,
        SkillsServe::Never => true,
        SkillsServe::Auto => harness_onboarded,
    }
}

/// Whether this server hands out the minimal `instructions` block instead of
/// the full routing block: only under `auto`, and only when the harness that
/// spawned this process is one whose own session hook has already delivered
/// the full block.
///
/// The same two inputs as [`hidden_skills_surface`], deliberately: the surface
/// and the instructions used to diverge (the surface keyed on the client's
/// name, then stopped), and one input for both is what makes the two eras
/// converge rather than split - a legacy peer reading `initialize` and a
/// modern peer reading `server/discover` are told the same thing.
///
/// `skills.serve = false` deliberately does not shrink the instructions. That
/// setting gates serving skills, not onboarding: an operator who turns the
/// skill surface off still wants a connecting agent to learn which domains
/// exist.
fn minimal_instructions(skills_serve: SkillsServe, harness_onboarded: bool) -> bool {
    skills_serve == SkillsServe::Auto && harness_onboarded
}

/// Whether collaboration tool `name` is hidden, given the engine's `read_only`
/// state and its live `github.enabled` setting. Not meaningful for a non-collab
/// tool name; callers check [`is_write_tool`] separately for those.
///
/// The net matrix, and the two gates compose rather than override:
///
/// - `github.enabled` off hides all five gated tools whatever the mode is, and
///   never hides `configure`, which is the only way to turn them on.
/// - read-only additionally hides the [`COLLAB_WRITE_TOOLS`] set, so a
///   read-only instance with collaboration on lists `update_domain` and
///   `origin_status` and nothing else of the six.
///
/// # Invariance is per instant, not per process
///
/// SEP-2567 says a tool list "MUST NOT vary per-connection or as a side effect
/// of other requests on the connection". `github.enabled` is read live here,
/// the same way [`refused_collab_tool`] reads it, so a `configure` call does
/// move this list - and that is the deliberate reading of the rule taken on
/// 2026-08-21: what may not vary is the answer two clients get at the same
/// moment, and this gate reads one shared setting, so it never does. A list
/// that may "change over time" is the same sentence's first clause; what it
/// owes is an announcement, which `Engine::configure` sends to every open
/// subscription - from whichever surface wrote the setting, the tool, the
/// control socket or the REST API (see [`crate::subscribers`]).
///
/// `read_only` is the gate that genuinely cannot move: `Engine::with_read_only`
/// (`engine.rs:788-791`) takes `self` by value at construction and the engine
/// is shared behind an `Arc`, so no request can reach it.
///
/// Hidden means hidden, not disabled. Every route stays registered and
/// [`refused_collab_tool`] still answers a direct call with the message naming
/// the setting, so a client holding a stale list is taught rather than told
/// "no such tool".
fn hidden_collab_tool(name: &str, read_only: bool, github_enabled: bool) -> bool {
    refused_collab_tool(name, github_enabled) || (read_only && COLLAB_WRITE_TOOLS.contains(&name))
}

/// Whether collaboration tool `name` refuses at call time because
/// `github.enabled` is off. Every collaboration tool but `configure` needs it,
/// and `configure` is deliberately exempt: it is how the setting gets turned
/// on.
///
/// [`hidden_collab_tool`] is built on this predicate rather than beside it, so
/// the listing and the refusal can never disagree about which tools the
/// setting governs: whatever is withheld from the list is exactly what refuses
/// when it is called anyway.
///
/// The refusal itself is [`RemoteError::NotEnabled`]'s message, which names
/// the setting and both ways to change it. The engine keeps its own copy of
/// this guard (`engine.rs:6075` and friends) for the REST and CLI surfaces;
/// this one exists so the MCP caller reads the reason as tool output rather
/// than as a JSON-RPC error the client renders opaquely.
fn refused_collab_tool(name: &str, github_enabled: bool) -> bool {
    !github_enabled && name != "configure"
}

use crystalline_core::config::{ResponseFormat, SkillsServe};

use crate::engine::{AckIntent, ConfigureAction, Engine, EngineError, ProvisionAction};
use crate::params::*;

/// The connected client's identity in the OKF agent form `name/version`, read
/// from the initialize handshake rmcp keeps on the peer.
///
/// The peer is per connection and the request context carries it into every
/// tool call, so a write records who actually asked for it even when several
/// HTTP sessions share one engine. `None` when the handshake carried no usable
/// name; [`Engine::actor`] then falls back. A version of `0.0.0` (rmcp's
/// stand-in for a client that sent none) is dropped rather than recorded.
fn client_actor(ctx: &RequestContext<RoleServer>) -> Option<String> {
    // The modern era first: with no handshake there is no peer info to read,
    // and rmcp synthesizes one carrying `Implementation::default()` (an empty
    // name), so without this every write by a 2026-07-28 peer would fall back
    // to the generic actor rather than naming who asked.
    //
    // **Reading `clientInfo` here is what the specification intends it for and
    // is not the thing it forbids.** The SHOULD NOT on `clientInfo` is about
    // changing *behaviour* on the client's self-reported identity: which tools
    // it is listed, what instructions it is handed. Recording who wrote an
    // engram is provenance, and the same page names "display, logging, and
    // debugging" as the intended uses.
    let from_meta = ctx.meta.client_info();
    let info = match from_meta.as_ref() {
        Some(info) => info,
        None => &ctx.peer.peer_info()?.client_info,
    };
    let name = info.name.trim();
    if name.is_empty() {
        return None;
    }
    let version = info.version.trim();
    if version.is_empty() || version == "0.0.0" {
        return Some(name.to_string());
    }
    Some(format!("{name}/{version}"))
}

/// Which transport a server instance serves, the one distinction the `auto`
/// value of `skills.serve` turns on.
///
/// A stdio connection is by construction same-machine: the client is a process
/// this machine's harness started, so this machine's install receipt is
/// authoritative about what that client already has on disk. An HTTP session
/// says nothing of the kind, so it is never suppressed - a remote client is
/// exactly the case the served skill surface exists for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Served over stdio: the `crystalline mcp` bridge, its daemon relay and
    /// the embedded in-process stack.
    Stdio,
    /// Served over the streamable HTTP transport.
    Http,
}

/// The MCP server for one connection: one tool router over one shared engine.
/// Cheap to clone; every serving path builds one per connection (the daemon
/// per accepted `mcp` socket, the HTTP transport per session, the stdio bridge
/// once for its single session).
///
/// **Nothing about the connecting client is read any more.** The
/// install-receipt match used to live here as an `AtomicBool` set from the
/// client's own `initialize` name, which is the per-connection variation
/// SEP-2567 forbids. What is here instead was decided before the connection
/// existed: see `harness_onboarded`.
#[derive(Clone)]
pub struct McpServer {
    engine: Arc<Engine>,
    transport: Transport,
    /// Whether the harness that spawned the serving process already has the
    /// shipped skills on disk and onboards itself at session start.
    ///
    /// **Resolved before the session starts and constant for this server's
    /// life.** The `crystalline mcp` process reads its own `--harness`
    /// argument (written into the harness's MCP registration by `crystalline
    /// install`) and asks this machine's install receipt whether that harness
    /// has session hooks wired. Neither input is the client's identity: one is
    /// deployment configuration, the other is machine state. False everywhere
    /// it cannot be known - HTTP, a registration predating the flag, an
    /// unrecognized harness id, a missing receipt - which serves the surface,
    /// the safe direction (an over-served client pays duplicated context, an
    /// under-served one loses onboarding it cannot rediscover).
    harness_onboarded: bool,
}

impl McpServer {
    /// Build a server around a shared engine for a stdio connection.
    pub fn new(engine: Arc<Engine>) -> McpServer {
        McpServer::with_transport(engine, Transport::Stdio)
    }

    /// Build a server around a shared engine for one HTTP session.
    pub fn new_http(engine: Arc<Engine>) -> McpServer {
        McpServer::with_transport(engine, Transport::Http)
    }

    fn with_transport(engine: Arc<Engine>, transport: Transport) -> McpServer {
        McpServer {
            engine,
            transport,
            harness_onboarded: false,
        }
    }

    /// Record that the harness this process serves is already onboarded (see
    /// the field). Set by the two stdio paths from the resolved answer the
    /// `crystalline mcp` process computed at startup: the embedded stack
    /// directly, the daemon relay from the value the bridge writes on its
    /// handshake line. Never set on the HTTP path.
    pub fn with_onboarded_harness(mut self, onboarded: bool) -> McpServer {
        self.harness_onboarded = onboarded;
        self
    }
}

#[tool_router]
impl McpServer {
    #[tool(
        name = "write_engram",
        title = "Capture engram",
        description = "Capture a new engram - a unit of knowledge - into a domain. Writes the markdown file and indexes it. Body bullets: '- [decision] we chose X #tag' become observations, '- rel_type [[Target]]' become relations. domain is required so an engram never lands in the wrong place. Pass folder to file the engram under a topic prefix: reuse the domain's existing layout (browse_domain shows it), start a subfolder when a topic cluster is forming and keep singletons at the root; the folder path becomes the permalink prefix build_context globs as crystalline://domain/folder/*. permalink, status, recorded_at and generated (who wrote it and when) are filled in; valid_from/valid_to are never auto-set - absence means always valid; to bound validity pass them inside metadata as plain ISO dates (YYYY-MM-DD). Any other date format is rejected; a sentinel far-future valid_to and an explicit null are dropped, since absence already means valid forever. Recommended type values: engram, guide, decision, architecture, runbook, reference. Recommended status values (guidance, not enforced): stable, implemented, draft, proposed, idea, poc, deprecated, superseded, archived, legacy. stable is the default and the word for knowledge that holds now; current is the legacy alias for the same state, and a status filter on either word matches engrams carrying either. Of those, deprecated, superseded, archived and legacy are the recognized retirement set: a status inside it softly fades in search ranking, any other value ranks at full strength. Errors if the permalink exists unless overwrite is true, and refuses a title that would file the engram as the reserved index.md or log.md (Crystalline generates the folder index itself). On a 2026-07-28 peer that declared an elicitation capability a permalink collision is not the bare error: the call writes nothing and answers input_required instead, a single-select question offering overwrite or cancel, which the client puts to the user and answers by re-sending the same call with the choice; cancel leaves the existing engram exactly as it is, and an explicit overwrite=true never asks. The vocabulary tool lists tags already in use; reuse one before coining a new tag. Set an optional numeric salience metadata key (0-10) to mark exceptionally valuable knowledge; salient engrams are lifted in hybrid search ranking. Raise it later to elevate an engram that proved load-bearing.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn write_engram(
        &self,
        Parameters(p): Parameters<WriteParams>,
        responses: InputResponses,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let actor = client_actor(&ctx);

        // **A refusal is read before the engine runs, never after it.** A
        // collision is discovered by attempting the write, so the shape that
        // suggests itself - call, then read the answer off the failure - is
        // wrong in exactly one case, and it is the case that matters: if the
        // engram in the way is deleted or renamed between the two rounds
        // (another agent, Fluid, the CLI), the round-two call no longer
        // collides, there is no error left to intercept, and the engram the
        // user answered "cancel and write nothing" about is written. Reading
        // the no first makes that impossible. It costs one case in the other
        // direction - a stale cancel carried on a call that no longer collides
        // refuses a write that would have succeeded, which the caller fixes by
        // re-sending without the answer - and that is the only direction a
        // confirmation is allowed to fail in.
        if confirmation_supported(&ctx)
            && !p.overwrite
            && resolved_overwrite(&responses.0) == Some(false)
        {
            return refuse(COLLISION_REFUSAL).map(CallToolResponse::from);
        }

        let written = self.engine.write_engram_as(&p, actor.as_deref()).await;

        // A permalink collision is the one failure here with a real choice
        // behind it, so a peer that can put that choice to its user is offered
        // it instead of the error. `overwrite` already being set is belt and
        // braces - the engine cannot raise this error when it is - but it
        // keeps the condition readable without a trip through engine
        // internals, and it is what the flow promises: a caller that asked for
        // the overwrite is never asked about it.
        let collision = match &written {
            Err(e) if !p.overwrite && confirmation_supported(&ctx) => {
                let message = e.to_string();
                message
                    .contains(COLLISION_MARKER)
                    .then(|| collision_permalink(&message).map(str::to_string))
                    .flatten()
            }
            _ => None,
        };
        let Some(permalink) = collision else {
            return written
                .map_err(to_error)
                .and_then(ok_written)
                .map(CallToolResponse::from);
        };

        match resolved_overwrite(&responses.0) {
            None => Ok(collision_question(collision_question_text(&p, &permalink)).into()),
            // Unreachable while the guard above stands, and written out anyway:
            // the arm that must never fall through to a write is not one to
            // leave implicit under a `_`.
            Some(false) => refuse(COLLISION_REFUSAL).map(CallToolResponse::from),
            // The retry is the original call with the answer applied, so
            // everything else about the write - folder, tags, metadata, the
            // actor - is the caller's, not a reconstruction. What it is not is
            // consent to particular bytes: the existing engram's content can
            // change between the collision error and this retry, and the user
            // agreed to replace whatever is at that permalink rather than the
            // version that was there when they were asked.
            Some(true) => {
                let mut retry = p.clone();
                retry.overwrite = true;
                self.engine
                    .write_engram_as(&retry, actor.as_deref())
                    .await
                    .map_err(to_error)
                    .and_then(ok_written)
                    .map(CallToolResponse::from)
            }
        }
    }

    #[tool(
        name = "read_engram",
        title = "Read engram",
        description = "Read an engram's full markdown and resolved frontmatter to learn what is already known before acting or writing. Identify it by bare permalink, title or a crystalline:// URL; pass domain to disambiguate. An identifier without crystalline:// is domain-relative: 'onboarding/setup', never 'mydomain/onboarding/setup'. The response flags whether each relation and prose link resolves, summarizes what links back and names a build_context anchor for exploring nearby knowledge. Attachments the engram references come back as resource links; fetch one with resources/read when the file itself matters.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn read_engram(
        &self,
        Parameters(p): Parameters<ReadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self.engine.read_engram(&p).await.map_err(to_error)?;
        let links = self.attachment_links(&value).await;
        let mut result = ok(value)?;
        result.content.extend(links);
        Ok(result)
    }

    #[tool(
        name = "edit_engram",
        title = "Edit engram",
        description = "Refine an existing engram in place as understanding evolves. Sections are addressed by heading path such as '## API > ### Auth'; replace_section keeps deeper subsections unless include_subsections is set. operation is one of append, prepend, find_replace, replace_section, insert_before_section, insert_after_section, set_frontmatter. find_replace takes find_text and an optional expected_replacements guard that fails on a count mismatch. set_frontmatter assigns one lifecycle field by key and value instead of text-substituting a frontmatter line: the settable keys are status, valid_from, valid_to, stale_after, source_date, salience, verified and evolve_ack, and nothing else (identity, tags, recorded_at and the generated block are refused). Use it to retire an engram, close or reopen a validity window, push a review date forward, mark knowledge salient or record that you re-checked something. Omit value to remove the field (that is how a valid_to that should never have been set is cleared); status cannot be removed. The four date keys take a plain ISO date (YYYY-MM-DD) and salience a number from 0 to 10. verified never removes: it stamps { by, at } with the current instant, taking value as the verifying actor and falling back to your own identity when value is omitted. evolve_ack is never cleared by an omitted value either: it acknowledges an evolve finding the user ruled intentional, taking value as the rule id optionally followed by a note ('V101' or 'V101 lineage citation, keep'), and the server records what evidence the finding fired on so the acknowledgment holds while that evidence holds and comes back marked stale when it changes; acknowledging the same rule again replaces the entry. To unacknowledge a finding - to unack it, to take back an acknowledgment so the finding resurfaces on the next sweep - pass the value 'remove <rule-id>' ('remove V101') on the same key; it errors when the engram carries no entry for that rule and the receipt reports evolve_ack_removed. Take an acknowledgment back only when the user asks. On a 2026-07-28 peer that declared an elicitation capability, an evolve_ack assignment - recording one or taking one back, and only that key - writes nothing on the first call and answers input_required instead: a confirmation question naming the rule and the engram, which the client puts to the user and answers by re-sending the same call with the confirmation; every other operation and key runs on the first call as before. Pass expected_checksum (from read_engram) to guard an edit against a change since your read: a conflict is refused if it changed, so re-read and retry; omit it for last-write-wins. The generated provenance block is refreshed with who edited it and when. Status values to reflect a changed lifecycle (recommended values: see write_engram). Temporal frontmatter fields (recorded_at, valid_from, valid_to, source_date, stale_after, plus the legacy last_verified and review_after spellings) must stay plain ISO dates (YYYY-MM-DD): an edit that leaves one malformed is rejected and a sentinel far-future valid_to or an explicit null is dropped, except recorded_at which is required and cannot be nulled.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn edit_engram(
        &self,
        Parameters(p): Parameters<EditParams>,
        responses: InputResponses,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // One key arms the round and every other edit runs untouched. The
        // parse failure is swallowed rather than reported here on purpose: the
        // engine is the one place that words it, and asking a user about an
        // edit that cannot run is worse than letting it fail where it always
        // failed. Round one resolves before it asks, exactly as the delete's
        // preview does, so the same rule holds for the identifier as for the
        // value: what cannot run is never put to a user.
        if confirmation_supported(&ctx)
            && let Ok(Some(intent)) = Engine::ack_intent(&p)
        {
            match confirmed(&responses.0) {
                None => {
                    let preview = self.engine.ack_preview(&p).await.map_err(to_error)?;
                    return Ok(confirm_question(ack_question(&preview, &intent)).into());
                }
                Some(false) => return refuse(ack_refusal(&intent)).map(CallToolResponse::from),
                Some(true) => {}
            }
        }
        self.engine
            .edit_engram_as(&p, client_actor(&ctx).as_deref())
            .await
            .map_err(to_error)
            .and_then(ok_written)
            .map(CallToolResponse::from)
    }

    #[tool(
        name = "move_engram",
        title = "Move engram",
        description = "Re-home an engram to a new path or domain as the knowledge base is reorganized. The destination may stay inside the same domain: re-filing an engram into a topic subfolder as a cluster forms is a normal move. On a cross-domain move, inbound bare links from other domains are rewritten to the domain-prefixed [[domain:Target]] form so nothing dangles. Set update_links to false to skip that. A destination filename of index.md or log.md is refused: both names are reserved for the generated directory index and log.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn move_engram(
        &self,
        Parameters(p): Parameters<MoveParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .move_engram(&p)
            .await
            .map_err(to_error)
            .and_then(ok_moved)
    }

    #[tool(
        name = "delete_engram",
        title = "Delete engram",
        description = "Remove an engram when its knowledge is retired. Deletes the file and its index rows. Prefer setting status to deprecated or superseded when the history still matters. An identifier under assets/ deletes that attachment instead - the stored file and its row - which is how an orphaned-attachment finding is completed after the user says yes; expected_checksum guards engram markdown and is refused for an attachment. On a 2026-07-28 peer that declared an elicitation capability the first call deletes nothing and answers input_required instead: a confirmation question naming the engram, its domain and permalink and the attachments only it references, which the client puts to the user and answers by re-sending the same call with the confirmation; anything but a yes deletes nothing.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn delete_engram(
        &self,
        Parameters(p): Parameters<DeleteParams>,
        responses: InputResponses,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // The whole confirmation flow lives inside this gate, so a peer that
        // cannot be asked is served exactly what it was served before the flow
        // existed: one call, one delete, one `CallToolResult`.
        if confirmation_supported(&ctx) {
            match confirmed(&responses.0) {
                None => {
                    let preview = self.engine.delete_preview(&p).await.map_err(to_error)?;
                    return Ok(confirm_question(delete_question(&preview)).into());
                }
                Some(false) => {
                    return refuse(
                        "The delete was not confirmed, so nothing was deleted. Call delete_engram again if the user asks for it.",
                    )
                    .map(CallToolResponse::from);
                }
                Some(true) => {}
            }
        }
        self.engine
            .delete_engram(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
            .map(CallToolResponse::from)
    }

    #[tool(
        name = "search_engrams",
        title = "Search engrams",
        description = "Search across every registered domain by default (an all-domain sweep) or a chosen few to recall relevant knowledge and experience. Defaults to hybrid lexical-plus-semantic ranking and falls back to plain text when embeddings are not ready. Filter by type, tags, status, arbitrary frontmatter or a recorded-after date; a filter-only search with no query text is allowed. Every hit is labelled with its domain, and a hit inside an observation carries its line. A hit's snippet is a short window around the match, never the whole engram: read_engram returns the full content, so read before citing or summarizing what a hit only previews. The result reports total, page, limit and count; when count is below total, request the next page to see the rest. A tags filter also matches through a domain's tag aliases (the MANIFEST `## Tag Aliases` section), so a merged old tag name still finds its engrams. A status filter on stable or current matches both, since they are one state under two spellings; any other status matches exactly. Hybrid ranking adds a small salience prior, so an engram marked salient at write time ranks above equally relevant unmarked ones without ever excluding a result. Engrams whose status is deprecated, superseded, archived or legacy are softly faded in ranking (the search.retired_weight setting, default 0.6, 1.0 disables), reordered but never excluded. Every hit on the returned page also comes back as a resource_link block beside the text, in hit order: follow the crystalline:// handle with resources/read instead of assembling the address out of the row's domain and permalink.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search_engrams(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .search_engrams(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_found(v))
    }

    #[tool(
        name = "build_context",
        title = "Build context",
        description = "Assemble the neighbourhood around an anchor engram by following its relations and links, across domains too, to gather related context before a task. Related engrams come back ranked by how strongly they connect to the anchor, salience-aware and status-aware (retired statuses rank lower), and max_related keeps the top-ranked. The anchor is a crystalline:// URL; a /* suffix globs a permalink prefix. depth is 1 to 3.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn build_context(
        &self,
        Parameters(p): Parameters<ContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .build_context(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "recent_activity",
        title = "Recent activity",
        description = "Review what has been captured recently across domains to catch up on new knowledge and experience. Defaults to the last 7 days; timeframe accepts values like 24h, 7d or 2w.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn recent_activity(
        &self,
        Parameters(p): Parameters<RecentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .recent_activity(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "list_domains",
        title = "List domains",
        description = "List the registered domains with their engram counts to see what the agent has been taught. If no CRYSTALLINE KNOWLEDGE ROUTING block reached you this session, call this at session start with include_routing=true: it returns each domain's When to Use routing bullets plus the behavior rules for this server's tools; follow them and route searches through those domains before answering from memory. The same call re-fetches the index mid-session.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_domains(
        &self,
        Parameters(p): Parameters<ListDomainsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .list_domains(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "browse_domain",
        title = "Browse domain",
        description = "Browse a domain's engrams by folder to explore how its knowledge is organized. path defaults to the root; depth controls how many folder levels are listed. One level at a time and bounded: a folder holding more engrams than a level shows comes back cut, with truncated true beside a total for the level, so read that as \"descend or search\" rather than as the whole folder. That total counts the level rather than the folder, so it moves with depth and leaves out anything nested deeper. The folder list is never cut, so every subfolder is there to descend into. A glob narrows only the engrams the level returned - on a cut level that is a choice within the cut - and it does not filter the folder list.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn browse_domain(
        &self,
        Parameters(p): Parameters<BrowseParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .browse_domain(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "validate_engrams",
        title = "Validate engrams",
        description = "Check a domain's engrams against its schema engrams to keep captured knowledge well-formed. Optionally narrow to one engram by identifier or to one type. Also runs the temporal checks so malformed dates, inverted validity windows, sentinel far-future dates and malformed generated or verified provenance entries are reported. Set drift to also report observation categories and relation types that drift from the schema - in use but undeclared or declared but unused.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn validate_engrams(
        &self,
        Parameters(p): Parameters<ValidateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .validate_engrams(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "infer_schema",
        title = "Infer schema",
        description = "Suggest a Picoschema for a type by generalizing over the engrams already captured in a domain, as a starting point for a schema engram. threshold is the frequency at or above which a field is suggested.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn infer_schema(
        &self,
        Parameters(p): Parameters<InferParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .infer_schema(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "vocabulary",
        title = "Vocabulary in use",
        description = "List the vocabulary in use: tags with engram and observation usage counts, observation categories with counts, relation types with counts and the engram types and statuses in use with counts, for one domain or across all domains. Check it before inventing a new tag, category, type or status so existing terms are reused instead of multiplied. The types and statuses lists report what the engrams are literally written in, counted as stored: nothing is folded (stable and current stay two entries) and a retired status is listed like any other, so they answer 'what does this domain actually use' rather than 'what is recommended'. Near-duplicate tag clusters are reported so they can be merged. Tag aliases recorded in a MANIFEST are listed too and clusters an alias already explains are not reported.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn vocabulary(
        &self,
        Parameters(p): Parameters<VocabularyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .vocabulary(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "evolve_engrams",
        title = "Evolve engrams",
        description = "Sweep one domain or every domain for the maintenance the knowledge needs and return a ranked work queue: a to-do list that walks you through tidying, cleaning up, auditing, reviewing or health-checking what has been taught. Detects temporal and lifecycle debt (an elapsed valid_to still marked stable, stale_after past due, long-unverified knowledge, a superseded engram with no successor relation and the half-finished converse, a retired engram still cited as current by live ones), structural gaps (unresolved [[links]], one-sided supersedes or summarizes pairs, orphans, an engram over the split budget, near-empty stubs) and redundancy (near-duplicate clusters, drifted tags). It detects by dates, links and graph shape only, never by meaning, so it cannot find or confirm a contradiction between what two engrams say. It also surfaces engrams people captured directly (through the Fluid web UI) that nobody reviewed yet, so what a person taught gets verified, tagged against the vocabulary and woven into the graph - those findings are judgment class. Attachments are swept too: a file a human added that no engram references, and a reference that points at no stored file, both come back as findings naming the attachment path. Read-only: it changes nothing itself. Each finding names the engram, the evidence and the exact next action with the tool that performs it, and a finding marked mechanical completes intent the archive already records while one marked judgment changes what the archive claims and needs a yes from the user first. Work the queue with the write tools and re-run the same scope to confirm it shrank. Call it when the user asks whether knowledge is still accurate, what needs attention or review, or to tidy, audit, consolidate or spring-clean a domain; after a large ingest lands many engrams at once; and when a search returns hits that disagree, since a half-finished retirement often explains the disagreement. Do not call it at session start, after routine captures or before ordinary recall - it is deliberate maintenance, on demand. When the user rules a finding intentional, acknowledge it (edit_engram set_frontmatter key evolve_ack, value like 'V101 lineage citation, keep') so it stops reappearing while its evidence holds; the sweep reports how many findings acknowledgments suppressed, and an acknowledgment whose evidence changed comes back marked stale. limit caps the queue (default 10), families narrows to one detector family, domains narrows the sweep, include_acknowledged returns the suppressed findings too.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn evolve_engrams(
        &self,
        Parameters(p): Parameters<EvolveParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .evolve_engrams(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "configure",
        title = "Configure Crystalline",
        description = "View and adjust Crystalline's settings, like an app's preferences page: call with no arguments to see them, set to change them (for example github.enabled to turn on team collaboration) and connect to link your GitHub account with a short code you confirm in the browser. With a token it accepts a personal access token instead. Connecting works before or after enabling; only team domains need github.enabled turned on.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn configure(
        &self,
        Parameters(p): Parameters<ConfigureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }

        if p.token.is_some() || p.connect.is_some() {
            let result = match (p.token.as_deref(), p.connect.as_deref()) {
                (Some(token), _) => {
                    self.engine
                        .connect_with_token(token, p.host.as_deref())
                        .await
                }
                (None, Some("github")) => self.engine.start_device_connect(p.host.as_deref()).await,
                (None, Some(other)) => Err(EngineError::Invalid(format!(
                    "configure connect must be 'github', got '{other}'"
                ))),
                (None, None) => unreachable!("checked above: token or connect is set"),
            };
            return result.map_err(to_error).and_then(ok);
        }

        // `github.enabled` gates the listing of five collaboration tools
        // ([`hidden_collab_tool`]), so a call that flips it moves this
        // server's tool list and owes subscribers an announcement. That does
        // not live here: it lives on `Engine::configure`, which every key in
        // this batch goes through and which the control socket and the REST
        // API write the same setting through, so the notification does not
        // depend on the route the flip took.
        //
        // The announcement therefore rides the individual key that flipped
        // rather than the batch. A `configure` that turns collaboration on and
        // then fails on a later key has still moved the list, has still
        // announced it, and reports what applied before it stopped.
        self.apply_settings(&p).await?;

        self.engine
            .configure_snapshot()
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "add_domain",
        title = "Add domain",
        description = "Create or connect a domain to store engrams in - the way to give the agent somewhere to capture knowledge, so it works even on an instance with no domains yet. Three modes follow the arguments: a local domain of markdown files on disk (pass folder, or just domain to use the default root at <domains_root>/<domain>) that is created with a starter MANIFEST when new and adopted in place when it already holds engrams; a virtual database-backed domain with no files (virtual: true with a domain name); or a GitHub team domain that downloads shared knowledge to learn from and share back (repo is owner/name, needs GitHub enabled via configure). repo and virtual are mutually exclusive. Available whenever the instance is writable; only the team mode needs GitHub turned on. Connecting a repository this domain is already connected to is safe and simply reports the connected state. Connecting a repository reports progress while it downloads and registers the knowledge, then keeps embedding it for search in the background after the call returns.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn add_domain(
        &self,
        Parameters(p): Parameters<AddDomainParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if p.repo.is_some() && p.is_virtual {
            return Err(to_error(EngineError::Invalid(
                "add_domain: repo and virtual are mutually exclusive; a team domain is file-backed"
                    .to_string(),
            )));
        }

        // When the client sent a progress token, forward stage boundaries as
        // MCP progress notifications so its request timeout stays alive during
        // the download; a channel plus one forwarder task keeps them ordered.
        let progress = ctx.meta.get_progress_token().map(|token| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, u64, String)>();
            let peer = ctx.peer.clone();
            tokio::spawn(async move {
                while let Some((step, total, message)) = rx.recv().await {
                    let _ = peer
                        .notify_progress(
                            ProgressNotificationParam::new(token.clone(), step as f64)
                                .with_total(total as f64)
                                .with_message(message),
                        )
                        .await;
                }
            });
            std::sync::Arc::new(move |step: u64, total: u64, msg: &str| {
                let _ = tx.send((step, total, msg.to_string()));
            }) as crate::engine::OriginProgress
        });

        // A newly added (or adopted) domain may already carry a MANIFEST that
        // declares a `Provisioning` section, so `provisioning_declared` can
        // flip on this call. That no longer moves any list - `provision` is
        // listed whatever is declared and refuses its mutating actions instead
        // - so nothing is announced; see [`McpServer::listen`].
        let result: Result<Value, EngineError> = if let Some(repo) = p.repo.as_deref() {
            self.engine
                .origin_add_with_progress(
                    repo,
                    p.domain.as_deref(),
                    p.path.as_deref(),
                    p.branch.as_deref(),
                    p.folder.as_deref(),
                    progress,
                )
                .await
        } else if p.is_virtual {
            if p.folder.is_some() {
                Err(EngineError::Invalid(
                    "add_domain: a virtual domain has no folder; omit folder or drop virtual"
                        .to_string(),
                ))
            } else {
                match p.domain.as_deref() {
                    Some(domain) => self.engine.domain_add_virtual(domain).await,
                    None => Err(EngineError::Invalid(
                        "add_domain: a virtual domain requires a domain name".to_string(),
                    )),
                }
            }
        } else {
            self.engine
                .domain_add_local(p.domain.as_deref(), p.folder.as_deref())
                .await
        };
        result.map_err(to_error).and_then(ok)
    }

    #[tool(
        name = "share_changes",
        title = "Share changes",
        description = "Share this domain's new knowledge and experience with the team as a proposal they review on GitHub; returns the review URL to hand to the user. Where the forge serves stacked pull requests, sharing while a proposal is open STACKS a new proposal on top of it - each share gets its own focused review - and reviewers merge layers bottom-up (merging the top lands the whole chain). Pass proposal to amend that open layer instead (the way to act on its review feedback); layers above it are re-based automatically. An edit to a file an open higher layer already changed belongs in that higher layer - pass its number - rather than in a lower amend, which would only be overwritten by the layer above it. On forges without stacks the open proposal is updated in place as before: same proposal number, same URL, a fresh commit reviewers are notified about, never a duplicate. Review feedback (approvals, change requests, comments) arrives through update_domain and origin_status, so the loop is: share, read the feedback, refine the engrams, share again naming the layer the feedback belongs to. If a reviewer pushed commits onto the proposal branch the update refuses with guidance: let the review finish on GitHub, or withdraw_proposal and share afresh. Refuses while conflicts are unsettled so the team always reviews a clean proposal. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure. On a 2026-07-28 peer that declared an elicitation capability the first call shares nothing and answers input_required instead: a confirmation question naming the action (open a new proposal, stack one on the open layer, amend a named layer or update the open proposal in place), the title or commit message and the changed files, answered by re-sending the same call; anything but a yes shares nothing.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn share_changes(
        &self,
        Parameters(p): Parameters<ShareChangesParams>,
        responses: InputResponses,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if refused_collab_tool("share_changes", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string()).map(CallToolResponse::from);
        }
        if confirmation_supported(&ctx) {
            match confirmed(&responses.0) {
                None => {
                    let preview = self
                        .engine
                        .origin_share_preview(&p.domain, p.title.as_deref(), p.proposal)
                        .await
                        .map_err(to_error)?;
                    // Only a share that would publish gets a question;
                    // nothing_to_share, conflicts_pending and
                    // proposal_diverged answer in round one, because
                    // executing the share produces exactly those canonical
                    // shapes with no publishing write - no commit, no branch
                    // update, no proposal opened or patched. Stated that way
                    // rather than as "no provider write": the pull the share
                    // runs first can reconcile a proposal the forge already
                    // closed, so a diverged answer may be preceded by
                    // bookkeeping calls. Those record what the forge already
                    // decided; they never publish this domain's changes.
                    //
                    // The two stacked plans belong on the asking side for the
                    // same reason the other two do: a stack opens a pull
                    // request the team can see, and an amend moves a layer
                    // they are already reviewing.
                    if matches!(
                        preview["action"].as_str(),
                        Some("create") | Some("update") | Some("stack") | Some("amend")
                    ) {
                        return Ok(confirm_question(share_question(&preview)).into());
                    }
                }
                Some(false) => {
                    return refuse(SHARE_REFUSAL).map(CallToolResponse::from);
                }
                Some(true) => {}
            }
        }
        self.engine
            .origin_share(
                &p.domain,
                p.title.as_deref(),
                p.description.as_deref(),
                p.proposal,
            )
            .await
            .map_err(to_error)
            .and_then(ok)
            .map(CallToolResponse::from)
    }

    #[tool(
        name = "update_domain",
        title = "Update domain",
        description = "Learn the team's latest knowledge: pulls what was merged upstream into the domain (or every shared domain), merging cleanly where possible and flagging real conflicts for resolve_conflict. The response carries each still-open proposal's review state and the reviewers' comments verbatim, so this is also how review feedback reaches you: read it, refine the engrams, then share_changes again to update the same proposal. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn update_domain(
        &self,
        Parameters(p): Parameters<UpdateDomainParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if refused_collab_tool("update_domain", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        // A pull can rewrite a domain's MANIFEST, so `provisioning_declared`
        // can flip here too, and like `add_domain` that announces nothing: the
        // gate it feeds refuses at call time instead of shaping a list.
        let result = self.engine.origin_update(p.domain.as_deref()).await;
        result.map_err(to_error).and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "origin_status",
        title = "Origin status",
        description = "Review each shared domain's standing: whether the team has new knowledge to learn, what is waiting to be shared, each open proposal's number, URL, review state (approved, changes requested, commented), whether a reviewer amended its branch, its feedback count, plus declined proposals and any conflicts to settle. Where the forge serves stacked pull requests every open proposal also carries its position in the chain - layer 1 is the bottom, and reviewers merge bottom-up - beside the domain's stack number, the declined layers still wedged under open work, and whether this chain is mid-repair, which means the next share or withdraw finishes it. Those keys are absent while nothing is stacked, and a position with no stack number means the layers exist but the forge has not grouped them yet. Feedback bodies are not repeated here - update_domain returns the reviewers' comment text. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn origin_status(
        &self,
        Parameters(p): Parameters<OriginStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if refused_collab_tool("origin_status", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        self.engine
            .origin_status(p.domain.as_deref())
            .await
            .map(lean_origin_status)
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "resolve_conflict",
        title = "Resolve conflict",
        description = "Settle a flagged conflict by keeping your version (mine), taking the team's version (theirs) or providing merged content. The engram then counts as ordinary local knowledge you can share. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure. resolution may be omitted on a 2026-07-28 peer that declared an elicitation capability: the call then answers input_required with a mine-or-theirs question previewing both sides, and the client re-sends the call with the answer. A hand-merged result never travels through the question - call with resolution merged plus content.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn resolve_conflict(
        &self,
        Parameters(p): Parameters<ResolveConflictParams>,
        responses: InputResponses,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if refused_collab_tool("resolve_conflict", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string()).map(CallToolResponse::from);
        }
        // Three ways to arrive at a resolution, and the arm order is the
        // behaviour: an explicit one is honoured for every peer and never
        // asked about, an eliciting peer that named none is asked, and any
        // other peer is refused in words it can read.
        let resolution: String = match &p.resolution {
            Some(resolution) => resolution.clone(),
            None if confirmation_supported(&ctx) => match chosen_resolution(&responses.0) {
                None => {
                    let detail = self
                        .engine
                        .origin_conflict_detail(&p.domain, None, Some(&p.path))
                        .await
                        .map_err(to_error)?;
                    return Ok(conflict_choice(conflict_resolution_question(&detail)).into());
                }
                Some(None) => {
                    return refuse(RESOLVE_REFUSAL).map(CallToolResponse::from);
                }
                Some(Some(choice)) => choice.to_string(),
            },
            None => return refuse(RESOLVE_NEEDS_RESOLUTION).map(CallToolResponse::from),
        };
        let (keep, content): (Option<&str>, Option<&[u8]>) = match resolution.as_str() {
            RESOLUTION_MINE => (Some(RESOLUTION_MINE), None),
            RESOLUTION_THEIRS => (Some(RESOLUTION_THEIRS), None),
            RESOLUTION_MERGED => {
                let Some(content) = p.content.as_deref() else {
                    return Err(ErrorData::invalid_params(
                        format!(
                            "resolve_conflict requires content when resolution is {RESOLUTION_MERGED}"
                        ),
                        None,
                    ));
                };
                (None, Some(content.as_bytes()))
            }
            other => {
                return Err(ErrorData::invalid_params(
                    format!(
                        "resolve_conflict resolution must be {RESOLUTION_MINE}, {RESOLUTION_THEIRS} or {RESOLUTION_MERGED}, got '{other}'"
                    ),
                    None,
                ));
            }
        };
        self.engine
            .origin_resolve(&p.domain, &p.path, keep, content)
            .await
            .map_err(to_error)
            .and_then(ok)
            .map(CallToolResponse::from)
    }

    #[tool(
        name = "withdraw_proposal",
        title = "Withdraw proposal",
        description = "Withdraw, retract, cancel or abandon a share proposal the team no longer wants: closes the open pull request on the forge, deletes its branch, and clears the proposal record from this domain's state. Pass proposal to name a number, or omit it to withdraw the domain's single open proposal; a declined proposal can be withdrawn too, which tidies its record away. Where the forge stacks proposals, withdrawing a layer that is not the top one closes it and re-bases every layer above it onto what is left, so the chain stays reviewable and nothing above the withdrawal is lost. Set revert true to also restore the shared files to their pre-share content - files edited since sharing are never touched - and leave it off to keep the knowledge local while only the proposal goes away. Use it when a review stalled, a proposal was superseded by better work, or a reviewer amended the branch and share_changes refuses to update it. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn withdraw_proposal(
        &self,
        Parameters(p): Parameters<WithdrawProposalParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if refused_collab_tool("withdraw_proposal", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        self.engine
            .origin_withdraw(&p.domain, p.proposal, p.revert.unwrap_or(false))
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "provision",
        title = "Provision harness artifacts",
        description = "Provision the skills, commands, agents and MCP servers a domain ships into the user's coding harnesses. A domain declares artifact folders in its MANIFEST; each domain needs a one-time allow or deny decision from the user before anything installs. status shows decisions and pending domains, allow or deny records a decision and applies it, apply reconciles updates and removals. Installed artifacts update when the domain's files change and disappear when the domain is denied or removed. Until some registered domain's MANIFEST declares a '## Provisioning' section, status reports an empty state and allow, deny and apply refuse and say so.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn provision(
        &self,
        Parameters(p): Parameters<ProvisionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let action = match p.action.as_str() {
            "status" => ProvisionAction::Status,
            "apply" => ProvisionAction::Apply,
            "allow" | "deny" => {
                let Some(domain) = p.domain.clone() else {
                    return Err(ErrorData::invalid_params(
                        format!("provision {} requires domain", p.action),
                        None,
                    ));
                };
                if p.action == "allow" {
                    ProvisionAction::Allow { domain }
                } else {
                    ProvisionAction::Deny { domain }
                }
            }
            other => {
                return Err(ErrorData::invalid_params(
                    format!("provision action must be status, allow, deny or apply, got '{other}'"),
                    None,
                ));
            }
        };
        // The declaration gate, which used to hide this tool from the listing
        // and now refuses the actions it would make pointless. `status` is
        // deliberately not one of them: it answers an empty report, which is
        // how a caller learns there is nothing to decide.
        if refused_provision_action(&action, self.engine.provisioning_declared()) {
            return refuse(PROVISION_NOT_DECLARED);
        }
        self.engine
            .provision(&action)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "skills",
        title = "Skills",
        description = "List the agent skills this server ships and read any skill's full SKILL.md playbook: how to route, capture, model schemas and collaborate well with Crystalline. Call with no arguments for the index of names and descriptions; pass name to read one skill before its kind of task.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn skills(
        &self,
        Parameters(p): Parameters<SkillsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(name) = p.name.as_deref() else {
            let index: Vec<Value> = SKILL_ASSETS
                .iter()
                .map(|s| json!({ "name": s.name, "description": s.description() }))
                .collect();
            return self.ok_list(json!({ "skills": index }));
        };
        match crystalline_core::skill(name) {
            // The playbook is markdown a model reads directly, so it is the
            // single text block verbatim rather than a JSON-wrapped string.
            Some(asset) => Ok(CallToolResult::success(vec![ContentBlock::text(
                asset.content,
            )])),
            None => Err(ErrorData::invalid_params(
                format!(
                    "skills: no skill named '{name}'; this server ships {}",
                    skill_names()
                ),
                None,
            )),
        }
    }
}

/// The two prompts a client can insert verbatim: the live routing block for
/// this server and the static snippet that teaches any client to onboard
/// itself. Declared with the rmcp prompt macros so each one's name,
/// description and (absent) arguments live at its handler; `list_prompts` and
/// `get_prompt` are hand-written in the `ServerHandler` impl instead of
/// generated, so the `skills.serve` gate can empty the list. See the module
/// docs.
#[prompt_router]
impl McpServer {
    /// The routing block the initialize `instructions` also carry, re-rendered
    /// per call: the cache refresh first is what makes a virtual domain's
    /// bullets current, exactly as the daemon does before `get_info`.
    #[prompt(
        name = "onboarding",
        title = "Knowledge routing",
        description = "The live knowledge routing block for this server: one routing line per domain plus the behavior rules. Insert at session start."
    )]
    async fn onboarding_prompt(&self) -> Vec<PromptMessage> {
        self.engine.refresh_routing_cache().await;
        vec![PromptMessage::new_text(
            Role::User,
            self.engine.routing_text(),
        )]
    }

    /// The static bootstrap snippet, identical to what `crystalline prompt
    /// connector` prints: a client whose custom instructions carry it onboards
    /// itself through `list_domains` even when nothing else reaches it.
    #[prompt(
        name = "connector",
        title = "Connector instructions",
        description = "A short static snippet to paste into a client's custom instructions so every session onboards itself through list_domains."
    )]
    async fn connector_prompt(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            crystalline_core::CONNECTOR_SNIPPET,
        )]
    }
}

impl McpServer {
    /// [`ServerHandler::get_info`] with this deployment's onboarding decision
    /// applied: the one place the routing block is shaped, so every era's
    /// arrival path hands out the same bytes.
    ///
    /// A legacy peer reads the result out of `InitializeResult.instructions`
    /// and a 2026-07-28 peer out of `DiscoverResult.instructions`; both call
    /// this, which is what the per-era arrival test in
    /// `tests/mcp_instructions.rs` pins.
    ///
    /// When [`minimal_instructions`] says so, the full routing prose is
    /// replaced by the header plus a pointer: the harness that spawned this
    /// process delivers the block itself at session start, so repeating it
    /// would spend the tokens twice. The TOON note is appended all the same -
    /// no hook carries it, it describes this connection's wire format rather
    /// than the knowledge, and a client that cannot read a tool result is
    /// worse off than one that read the routing block twice.
    fn arrival_info(&self) -> ServerInfo {
        let mut info = self.get_info();
        if minimal_instructions(self.engine.skills_serve(), self.harness_onboarded) {
            let mut instructions = crystalline_core::render_minimal_instructions();
            if self.engine.response_format() == ResponseFormat::Toon {
                instructions.push_str(TOON_INSTRUCTIONS_NOTE);
            }
            info.instructions = Some(instructions);
        }
        info
    }

    /// The resource links a `read_engram` result carries: one per distinct
    /// `assets/` reference in the body that resolves to a stored attachment.
    ///
    /// Links rather than bytes, deliberately. A screenshot inlined as base64
    /// would spend a model's whole context on a file it may not need; a link
    /// names it - uri, filename, mime and size - and `resources/read` fetches
    /// the bytes when the model decides the file itself matters.
    ///
    /// A reference that resolves to nothing produces no link and no complaint:
    /// a dangling attachment reference is knowledge debt, and `evolve_engrams`
    /// is where debt is reported. A listing that cannot be read (a domain
    /// dropped between the read and this call) costs the links, never the read.
    async fn attachment_links(&self, value: &Value) -> Vec<ContentBlock> {
        let (Some(domain), Some(content)) = (
            value.get("domain").and_then(Value::as_str),
            value.get("content").and_then(Value::as_str),
        ) else {
            return Vec::new();
        };
        let refs = crystalline_core::find_asset_refs(content);
        if refs.is_empty() {
            return Vec::new();
        }
        let Ok(rows) = self.engine.attachment_list(domain).await else {
            return Vec::new();
        };
        refs.iter()
            .filter_map(|target| rows.iter().find(|row| row.path == *target))
            .map(|row| {
                let name = row.path.rsplit('/').next().unwrap_or(row.path.as_str());
                ContentBlock::resource_link(
                    Resource::new(format!("crystalline://{domain}/{}", row.path), name)
                        .with_mime_type(row.mime.clone())
                        .with_size(row.size),
                )
            })
            .collect()
    }

    /// Wrap a list-shaped engine value as a successful tool result: TOON
    /// under the default `service.response_format`, byte-identical to [`ok`]
    /// under `json`. The format is read per response, so a runtime configure
    /// switch applies from the next tool call on.
    fn ok_list(&self, value: Value) -> Result<CallToolResult, ErrorData> {
        match self.engine.response_format() {
            ResponseFormat::Json => ok(value),
            ResponseFormat::Toon => Ok(CallToolResult::success(vec![ContentBlock::text(
                crate::toon::render(&value),
            )])),
        }
    }

    /// [`Self::ok_list`] for a search result, with one `resource_link` per hit
    /// on the returned page appended behind the text block, in hit order.
    ///
    /// The rows already carry `domain` and `permalink`, so this makes the same
    /// trade the write receipts make: an address a client assembles out of two
    /// fields is an address every client has to be taught, and a link is one
    /// the era already knows how to follow. The links are blocks beside the
    /// text rather than anything inside it, so a TOON table and a JSON body
    /// hand back the same handles.
    ///
    /// One link per row, so the nth link pairs with the nth hit - two
    /// observation hits inside one engram therefore link that engram twice,
    /// which is the pairing holding rather than a duplicate. Only the returned
    /// page is linked, and a hit missing a field is skipped rather than
    /// guessed at, the same tolerance as [`ok_written`].
    fn ok_found(&self, value: Value) -> Result<CallToolResult, ErrorData> {
        let links: Vec<ContentBlock> = value
            .get("hits")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|hit| {
                let domain = hit.get("domain").and_then(Value::as_str)?;
                let permalink = hit.get("permalink").and_then(Value::as_str)?;
                let title = hit
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or(permalink);
                Some(engram_link(domain, permalink, title))
            })
            .collect();
        let mut result = self.ok_list(value)?;
        result.content.extend(links);
        Ok(result)
    }

    /// Applies `configure`'s `set` map then `unset` list, one key at a time
    /// through the engine's existing per-key [`ConfigureAction`], stopping at
    /// the first failure. On success every applied key has already taken
    /// effect (and been persisted); on failure the error names which key
    /// failed and which keys before it were already applied, so the caller
    /// never has to guess the resulting state.
    async fn apply_settings(&self, p: &ConfigureParams) -> Result<(), ErrorData> {
        let mut applied: Vec<String> = Vec::new();
        for (key, value) in &p.set {
            match self
                .engine
                .configure(&ConfigureAction::Set {
                    key: key.clone(),
                    value: value.clone(),
                })
                .await
            {
                Ok(_) => applied.push(key.clone()),
                Err(e) => return Err(applied_failure(&applied, key, e)),
            }
        }
        for key in &p.unset {
            match self
                .engine
                .configure(&ConfigureAction::Unset { key: key.clone() })
                .await
            {
                Ok(_) => applied.push(key.clone()),
                Err(e) => return Err(applied_failure(&applied, key, e)),
            }
        }
        Ok(())
    }
}

/// Builds `configure`'s partial-application error: the underlying error's
/// class (invalid params vs internal) is kept, only the message is enriched
/// with which keys already applied and which one failed.
fn applied_failure(applied: &[String], failed_key: &str, e: EngineError) -> ErrorData {
    let base = to_error(e);
    let message = if applied.is_empty() {
        format!("failed to apply '{failed_key}': {}", base.message)
    } else {
        format!(
            "applied [{}]; failed to apply '{failed_key}': {}",
            applied.join(", "),
            base.message
        )
    };
    ErrorData::new(base.code, message, base.data)
}

#[tool_handler]
impl ServerHandler for McpServer {
    /// The revisions this server serves, narrowed from rmcp's crate-wide
    /// `KNOWN_VERSIONS` default to [`SERVED_PROTOCOL_VERSIONS`].
    ///
    /// rmcp consults this in three places, so overriding it once covers every
    /// path: `negotiate_protocol_version` after `initialize` on stdio
    /// (rmcp 3.1.2 `service/server.rs:590`), the same call inside
    /// `NegotiatingStatelessHttpService` on the HTTP stateless path
    /// (`tower.rs:322-326`), and the inline per-request version check modern
    /// requests take instead of a handshake (`handler/server.rs:65-72`). It
    /// also fills `DiscoverResult.supportedVersions`.
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(SERVED_PROTOCOL_VERSIONS)
    }

    /// The server handshake: hand the connecting agent the live routing block
    /// as its `instructions`. rmcp calls `get_info` once per connection at
    /// initialize, so [`Engine::routing_text`] renders the currently registered
    /// domains (a domain added since startup shows up on the next connection)
    /// and follows the engine's read-only mode, read-write and read-only intros
    /// alike. The daemon and the embedded stdio stack refresh the
    /// virtual-domain routing cache just before this runs, so the sync render
    /// reads a current cache and never blocks on the store. `server_info` is
    /// also set explicitly: `ServerInfo::default()` leaves
    /// `Implementation::from_build_env()`, which would report the rmcp crate's
    /// own name and version to harness logs rather than crystalline's.
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.server_info = Implementation::new("crystalline", crystalline_core::VERSION);
        // This field is the default `initialize` answer, and `initialize`
        // belongs to the legacy lifecycle, so it names the newest revision that
        // still has a handshake rather than the newest we serve. What we serve
        // is advertised through `supported_protocol_versions` and echoed by
        // `initialize` when a client asks for it. Set explicitly because
        // `ServerInfo::default()` would leave rmcp's own `ProtocolVersion::
        // LATEST` here, which moves when the crate does.
        info.protocol_version = newest_legacy_handshake_version();
        let mut instructions = self.engine.routing_text();
        if self.engine.response_format() == ResponseFormat::Toon {
            instructions.push_str(TOON_INSTRUCTIONS_NOTE);
        }
        info.instructions = Some(instructions);
        // Capabilities are initialize facts a session cannot renegotiate, so
        // resources and prompts stay advertised whatever `skills.serve` says;
        // the gate empties the two lists instead of retracting the capability
        // mid-session. Resource subscribe is deliberately not enabled: the
        // shipped skills are static for a binary's lifetime, so there is
        // nothing to subscribe to.
        //
        // The three list-changed capabilities are what a modern client is
        // allowed to open a `subscriptions/listen` stream for: rmcp intersects
        // any requested filter with exactly this set (rmcp 3.1.2
        // `handler/server.rs:157-160`), and
        // [`McpServer::accepted_subscription_filter`] names the same three, so
        // the advertisement and the accepted filter cannot drift.
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .enable_resources_list_changed()
            .enable_prompts()
            .enable_prompts_list_changed()
            .build();
        info
    }

    /// Complete the legacy handshake: publish the peer info and echo the
    /// negotiated protocol version.
    ///
    /// **This is the onboarding path for the four revisions below 2026-07-28
    /// and no others.** In the 2026-07-28 schema there is no
    /// `InitializeResult` at all - the handshake is deleted and `instructions`
    /// lives on `DiscoverResult` - so a modern peer is onboarded through
    /// [`McpServer::discover`] instead. Both build their block from
    /// [`McpServer::arrival_info`], so the two eras hand out the same bytes.
    ///
    /// **Nothing here reads who is connecting any more.** This used to be the
    /// only point at which the instructions could depend on the client, so the
    /// receipt match lived here; that is exactly what SEP-2567's
    /// per-connection prohibition forbids, and the decision moved to the
    /// spawned process (see `McpServer::harness_onboarded`). What survives is
    /// what rmcp's own default does: publishing the peer info, which is what
    /// `client_actor` and every `generated.by` write read afterwards, and the
    /// version echo.
    ///
    /// # A version we do not serve is refused here, but only over HTTP
    ///
    /// On the streamable-HTTP transport rmcp decides session routing from the
    /// *request*, not from our advertised set: `use_session =
    /// legacy_session_mode && is_legacy_request(...)` (rmcp 3.1.2
    /// `tower.rs:1727`), and `is_legacy_request` (`tower.rs:358-408`) reads the
    /// version out of the request body and compares it against 2026-07-28,
    /// never against [`SERVED_PROTOCOL_VERSIONS`]. `Mcp-Session-Id` is inserted
    /// at exactly one site (`tower.rs:1911`), inside the session branch. So an
    /// `initialize` naming a version at or above 2026-07-28 routes statelessly
    /// and gets no session id, while our answer named a version we do serve;
    /// the client's next request declared that older version, took the session
    /// branch with no session id to present, and got `422 Unprocessable Entity:
    /// Unexpected message, expect initialize request` (`tower.rs:1833`/`:1851`)
    /// for the rest of its life. A successful handshake followed by permanent
    /// failures, observed on this endpoint before this refusal existed.
    ///
    /// **What is left of that once 2026-07-28 is served, which is much less.**
    /// A client naming the era is now answered the era, so it stays on the
    /// stateless routing its own request chose and the two halves agree; if it
    /// goes on to send the era's request shape (per-request `_meta` plus the
    /// standard headers) it is served with no session at all, which is what
    /// SEP-2575 asks for. `tests/mcp_modern_era.rs` drives exactly that. The
    /// one ragged corner left is a client that declares the era in a handshake
    /// and then sends *legacy-shaped* requests: those ask for the session
    /// branch, there is no session, and rmcp answers 422. That is a client
    /// contradicting itself - the revision it named has no handshake - and it
    /// is pinned rather than papered over.
    ///
    /// **What the refusal still protects, and why it is not deleted.**
    /// `ProtocolVersion` deserializes any string (`model.rs:204-220`) and the
    /// comparison is lexicographic, so `"2027-01-01"`, `"banana"` or any other
    /// string sorting at or above `"2026-07-28"` routes statelessly while being
    /// a revision nobody implements. Answering it with one of ours would leave
    /// the original wedge exactly as it was. That class exists independently of
    /// anything we advertise, which is why the branch narrows rather than goes.
    ///
    /// `ErrorData::unsupported_protocol_version` (`model.rs:601-613`, code
    /// `-32022` at `model.rs:546`) is the shape the specification's versioning
    /// page documents, and it carries the set we do serve so a client can
    /// retry. It has two wire shapes, both observed rather than derived: a
    /// plain `initialize` carrying no per-request `_meta` lands on
    /// `stateless_sse_response` (`tower.rs:2027`) and arrives as **HTTP 200
    /// with an SSE-framed JSON-RPC error**, because `json_response` defaults to
    /// false (`tower.rs:169`); a request that took the negotiated-direct path
    /// (`tower.rs:1255`) goes through `jsonrpc_http_status` (`:617-630`) and
    /// arrives as **400 with `application/json`**. Assert the code, never a
    /// status.
    ///
    /// **Stdio keeps warn-and-downgrade.** There is no session routing there,
    /// so the wedge cannot occur, and a hard refusal would regress the day a
    /// harness bumps its version string ahead of us: a client that asks for
    /// tomorrow's revision over stdio gets a working session at the newest
    /// revision that still has a handshake (see
    /// [`newest_legacy_handshake_version`]).
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        let requested = request.protocol_version.clone();
        let client_name = request.client_info.name.clone();
        let served = SERVED_PROTOCOL_VERSIONS.contains(&requested);
        if !served {
            if self.transport == Transport::Http {
                return Err(ErrorData::unsupported_protocol_version(
                    requested,
                    SERVED_PROTOCOL_VERSIONS,
                ));
            }
            // Named rather than counted: the first time this line appears in a
            // field log for a client we support is the signal that the newest
            // revision has stopped being a follow-up and become urgent.
            tracing::warn!(
                client = %client_name,
                requested = %requested,
                "client requested a protocol version this server does not serve; \
                 serving {} instead",
                newest_legacy_handshake_version()
            );
        }
        context.peer.set_peer_info(request);

        let mut info = self.arrival_info();
        // Echo what the client asked for when we serve it - 2026-07-28
        // included, which is how a client asks for the modern lifecycle
        // through a handshake - and downgrade to the newest revision that
        // still has a handshake otherwise. The fallback is read from our own
        // list rather than left at `ServerInfo::default()`'s
        // `ProtocolVersion::LATEST`, so an rmcp whose LATEST moves cannot make
        // us answer with a revision we do not serve, and it is capped below
        // the era for the reasons on [`newest_legacy_handshake_version`].
        info.protocol_version = if served {
            requested
        } else {
            newest_legacy_handshake_version()
        };
        Ok(info)
    }

    /// Answer `server/discover`: **the modern era's only onboarding channel,
    /// and a channel the client is free never to open.**
    ///
    /// From 2026-07-28 there is no `initialize` and no `InitializeResult`;
    /// `grep -i initialize` over `schema/2026-07-28/schema.ts` returns zero
    /// hits. `instructions` appears exactly twice in that schema, once in an
    /// unrelated doc comment and once as `DiscoverResult.instructions` (line
    /// 696). No reserved `_meta` key carries onboarding, no notification does,
    /// and the method list is closed. So this method is the whole of it.
    ///
    /// **A modern client that never calls `server/discover` is never handed
    /// the routing block, and nothing errors when that happens.** The
    /// specification permits it in as many words ("Clients MAY call it but are
    /// not required to - version negotiation can also happen inline via
    /// per-request `_meta`"). The mitigations are all pull-shaped and all
    /// require the client to already know to ask: the `onboarding` prompt,
    /// `list_domains` with `include_routing=true`, and the served skills.
    /// `tests/mcp_instructions.rs` pins that this server offers the block by
    /// every era's own path; no server-side test can prove a client pulled it.
    ///
    /// Overridden rather than inherited for one reason: rmcp's default builds
    /// straight from `get_info()`, and the routing cache has to be refreshed
    /// first, exactly as the daemon does before an `initialize`
    /// (`daemon.rs`), or a discover-first client reads a stale virtual-domain
    /// index. The rest is rmcp's own construction:
    /// `DiscoverResult::from_server_info` carries `instructions` out of
    /// `ServerInfo` untouched and sets `ttl_ms: 0` with `cache_scope: Private`
    /// (rmcp 3.1.2 `model.rs:1246-1268`), which already satisfies the
    /// caching MUST for this operation.
    ///
    /// The client's own `_meta.clientInfo` is deliberately not read here. The
    /// specification says implementations "SHOULD NOT use them to change the
    /// behavior of the client or server", and keying instructions on it would
    /// additionally force a private cache scope on a result the spec wants
    /// cacheable.
    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        self.engine.refresh_routing_cache().await;
        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.arrival_info(),
        ))
    }

    /// What a `subscriptions/listen` stream may carry: the three list-changed
    /// categories this server advertises in [`ServerHandler::get_info`], and
    /// nothing else.
    ///
    /// Returning `Some` is what makes the method exist at all: rmcp's default
    /// returns `None` and answers `method not found` (rmcp 3.1.2
    /// `handler/server.rs:151-155`, default at `:411-416`). rmcp then intersects
    /// this candidate with the request and again with the capabilities
    /// `get_info` advertises (`:157-160`), so `resource_subscriptions`
    /// (`notifications/resources/updated`) is dropped twice over: it is absent
    /// here, and `resources.subscribe` is deliberately not advertised because
    /// the shipped skills are static for a binary's lifetime.
    ///
    /// The requested filter is not read. Accepting a category is a statement
    /// about what this server can deliver, not about who is asking, which is
    /// the same rule the list endpoints follow.
    fn accepted_subscription_filter(
        &self,
        _requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(
            SubscriptionFilter::builder()
                .tools_list_changed()
                .prompts_list_changed()
                .resources_list_changed()
                .build(),
        )
    }

    /// Hold one acknowledged subscription open until the client ends it, and
    /// keep its sink where a list change can find it.
    ///
    /// # What can move, and what cannot
    ///
    /// One thing this server can be asked to do moves a list: `configure` can
    /// flip `github.enabled`, and five collaboration tools appear or disappear
    /// with it (see [`hidden_collab_tool`]). That is the only mover.
    /// `resources/list` and `prompts/list` read `skills.serve` and
    /// `harness_onboarded`, both fixed before the first request arrives, so
    /// those two categories are accepted on a subscription and then never
    /// carry anything - accepting a category is a statement about what this
    /// server may deliver, not a promise that it will.
    ///
    /// # The sink lives on the engine, not on this handler
    ///
    /// On the stateless HTTP path rmcp builds a fresh service per request
    /// (`get_service()` at rmcp 3.1.2 `tower.rs:1822` and `:1948`) and every
    /// modern peer routes statelessly, so the handler that takes this
    /// subscription and the handler that later runs `configure` are different
    /// objects sharing only the `Arc<Engine>`. (The legacy session path builds
    /// one service per session, `tower.rs:1855`, and stdio one per connection,
    /// so a handler-local registry would have worked there and nowhere else -
    /// which is exactly the bug that would have shipped silently.) The registry
    /// is therefore [`crate::subscribers::ListSubscribers`], reached through
    /// `Engine::list_subscribers`.
    ///
    /// `SubscriptionSink` holds a `Peer` and a child cancellation token
    /// (`service/server.rs:139-144`), so an entry outliving its stream would
    /// pin a dead peer. The guard returned by `register` is held for exactly
    /// the body of this method and drops the entry however the stream ends.
    ///
    /// # What the client is guaranteed before this runs
    ///
    /// rmcp has already sent `notifications/subscriptions/acknowledged` with
    /// the subscription id in its `_meta`
    /// (`SubscriptionContext::establish`, `service/server.rs:337-375`), which
    /// is the specification's "acknowledgment first, id in `_meta`" pair, and
    /// `SubscriptionSink::send` re-attaches that id and enforces the accepted
    /// filter on anything sent later (`:184-257`). Both are pinned by
    /// `tests/mcp_subscriptions.rs` off the wire, not assumed.
    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let _registered = crate::subscribers::ListSubscribers::register(
            self.engine.list_subscribers(),
            context.sink().clone(),
        );
        tracing::debug!(
            accepted = ?context.accepted(),
            listening = self.engine.list_subscribers().len(),
            "subscription opened"
        );
        context.cancelled().await;
        Ok(())
    }

    /// List the exposed tools.
    ///
    /// # Every client listing at the same instant sees the same list
    ///
    /// MCP 2026-07-28 (SEP-2567, `/server/tools`) says a server's tool list
    /// "MAY change over time [...] but MUST NOT vary per-connection or as a
    /// side effect of other requests on the connection", and the identical
    /// sentence governs `resources/list` and `prompts/list`. The invariant this
    /// method keeps is the first half read literally: a gate here may read
    /// deployment or instance state, never anything derived from who is asking.
    ///
    /// Three of the four gates cannot move at all. `read_only` is fixed at
    /// engine construction (`Engine::with_read_only`, `engine.rs:788-791`,
    /// takes `self` by value; the engine is shared behind an `Arc`),
    /// `skills.serve` is snapshotted at the same point
    /// (`Engine::skills_serve`) and the harness answer was resolved by the
    /// spawned process before the session started - see
    /// [`hidden_skills_surface`].
    ///
    /// The fourth, `github.enabled`, is read **live**, and is the one gate that
    /// makes this list dynamic (see [`hidden_collab_tool`]). It is a single
    /// setting on the shared engine, so two clients listing at the same moment
    /// still get the same answer; what varies is the moment, which is the
    /// "MAY change over time" clause rather than a violation of the one after
    /// it. The obligation that comes with it is discharged in
    /// `Engine::configure`, the seam every writer of that setting goes
    /// through: a flip announces itself on every open subscription stream, and
    /// to nobody who did not open one.
    ///
    /// Whether any domain declares provisioning is the gate that did leave this
    /// list for a call-time refusal, which is the remedy SEP-2567 prescribes
    /// itself: expose the tool unconditionally and put the dependency "in the
    /// tool's input schema and description rather than in the list result".
    /// It stays gone, because `add_domain` and `update_domain` can create a
    /// declaration mid-call and there is no one setting to point at.
    ///
    /// Every route stays registered whatever is hidden, so a client calling a
    /// tool it cannot see reaches the handler and is refused with a reason.
    ///
    /// Both this method and `get_tool` run every surviving tool's schema
    /// through `crate::tool_schema::sanitize_tool` before returning it, so
    /// advertised schemas stay in the conservative client-compatible shape.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let read_only = self.engine.read_only();
        let github_enabled = self.engine.github_enabled();
        let skills_hidden =
            hidden_skills_surface(self.engine.skills_serve(), self.harness_onboarded);
        let mut tools = Self::tool_router().list_all();
        tools.retain(|t| {
            if is_write_tool(&t.name) && read_only {
                return false;
            }
            if is_collab_tool(&t.name) && hidden_collab_tool(&t.name, read_only, github_enabled) {
                return false;
            }
            if t.name == crate::EVOLVE_TOOL_NAME && hidden_evolve_tool(read_only) {
                return false;
            }
            if t.name == "provision" && hidden_provision_tool(read_only) {
                return false;
            }
            if t.name == "skills" && skills_hidden {
                return false;
            }
            true
        });
        for tool in &mut tools {
            crate::tool_schema::sanitize_tool(tool);
        }
        Ok(ListToolsResult::with_all_items(tools).with_cache_hints(&context))
    }

    /// Resolve a tool definition by name, hiding exactly what `list_tools`
    /// hides, so the two enforcement points cannot drift apart.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        let read_only = self.engine.read_only();
        if is_write_tool(name) && read_only {
            return None;
        }
        if is_collab_tool(name) && hidden_collab_tool(name, read_only, self.engine.github_enabled())
        {
            return None;
        }
        if name == crate::EVOLVE_TOOL_NAME && hidden_evolve_tool(read_only) {
            return None;
        }
        if name == "provision" && hidden_provision_tool(read_only) {
            return None;
        }
        if name == "skills"
            && hidden_skills_surface(self.engine.skills_serve(), self.harness_onboarded)
        {
            return None;
        }
        let mut tool = Self::tool_router().get(name).cloned()?;
        crate::tool_schema::sanitize_tool(&mut tool);
        Some(tool)
    }

    /// List the shipped agent skills as `skill://<name>/SKILL.md` resources,
    /// so a remote client that never runs the CLI can read the same playbooks
    /// an installed harness gets. Empty while the surface is withheld, on the
    /// same two construction-time inputs the tool gate reads (see
    /// [`hidden_skills_surface`]); nothing about this list can move under a
    /// live connection.
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        if hidden_skills_surface(self.engine.skills_serve(), self.harness_onboarded) {
            return Ok(ListResourcesResult::with_all_items(Vec::new()).with_cache_hints(&context));
        }
        let resources = SKILL_ASSETS
            .iter()
            .map(|s| {
                Resource::new(skill_uri(s.name), s.name)
                    .with_description(s.description())
                    .with_mime_type(SKILL_MIME_TYPE)
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources).with_cache_hints(&context))
    }

    /// The one template this server serves: every attachment a domain carries,
    /// addressed as `crystalline://<domain>/assets/<path>`.
    ///
    /// A template rather than a listing because the set is open and per domain:
    /// enumerating every screenshot of every registered domain would spend a
    /// client's context on files it will never open, while the template plus the
    /// resource links `read_engram` returns name exactly the ones an engram
    /// actually references.
    ///
    /// The override also carries the caching hints on its own account: rmcp's
    /// default returns `ListResourceTemplatesResult::default()` (rmcp 3.1.2
    /// `handler/server.rs:387-395`), a **complete** result with no hints on one
    /// of the six operations SEP-2549 names. See [`CacheHinted`].
    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let template = ResourceTemplate::new(ATTACHMENT_URI_TEMPLATE, ATTACHMENT_TEMPLATE_NAME)
            .with_description(ATTACHMENT_TEMPLATE_DESCRIPTION);
        Ok(ListResourceTemplatesResult::with_all_items(vec![template]).with_cache_hints(&context))
    }

    /// Read one shipped skill, or one attachment, by its resource uri.
    ///
    /// A skill answers even while `skills.serve` is off, like every hidden
    /// tool: the gate hides the surface from a listing rather than disabling
    /// it, and a skill is static public copy this binary already carries, so a
    /// client holding a uri from an earlier listing gets the bytes rather than
    /// a puzzle.
    ///
    /// An attachment uri is anything [`CrystallineUrl::asset_path`] recognizes
    /// once its path has been percent-decoded (see [`decoded_uri_path`]), and
    /// the bytes come back the way the file is read rather than the way it was
    /// asked for: a text mime as `TextResourceContents`, everything else base64
    /// in `BlobResourceContents`. This is the one place base64 is ever emitted
    /// - a tool result carries links, never bytes - so a model spends the
    /// context on an image or a deck only when it decided to open it.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if let Some(asset) = skill_for_uri(&request.uri) {
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(asset.content, &request.uri).with_mime_type(SKILL_MIME_TYPE),
            ])
            .with_cache_hints(&context)
            .into());
        }
        if let Some(url) = CrystallineUrl::parse(&request.uri) {
            let url = CrystallineUrl {
                permalink: decoded_uri_path(&url.permalink)?,
                ..url
            };
            if let Some(path) = url.asset_path() {
                let (bytes, row) = self
                    .engine
                    .attachment_read(&url.domain, path)
                    .await
                    .map_err(to_error)?;
                return Ok(ReadResourceResult::new(vec![attachment_contents(
                    &request.uri,
                    bytes,
                    &row.mime,
                )])
                .with_cache_hints(&context)
                .into());
            }
        }
        Err(ErrorData::invalid_params(
            format!(
                "unknown resource '{}'; this server serves {} and every attachment addressed as {ATTACHMENT_URI_TEMPLATE}",
                request.uri,
                skill_uris()
            ),
            None,
        ))
    }

    /// List the two onboarding prompts, empty while `skills.serve` is off.
    /// Hand-written rather than `#[prompt_handler]`-generated for exactly that
    /// gate: the macro replaces any `list_prompts` in the impl block it is
    /// applied to, so a generated one could never be emptied.
    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let prompts = if hidden_skills_surface(self.engine.skills_serve(), self.harness_onboarded) {
            Vec::new()
        } else {
            Self::prompt_router().list_all()
        };
        Ok(ListPromptsResult::with_all_items(prompts).with_cache_hints(&context))
    }

    /// Render one prompt through the macro-declared router. Answers while the
    /// gate is off for the same reason `read_resource` does.
    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        Self::prompt_router()
            .get_prompt(PromptContext::new(
                self,
                request.name,
                request.arguments,
                context,
            ))
            .await
    }
}

/// The percent-decoded path of a `crystalline://` resource uri.
///
/// RFC 3986 lets a path carry only a restricted set of characters, so a
/// conforming client percent-encodes the rest before sending a uri back - every
/// non-ASCII letter in a filename a human chose, for a start. The REST file
/// route gets this step for free from axum's `Path` extractor, which decodes
/// before the handler runs (`crate::rest::files`); the MCP surface has no
/// extractor in front of it, so it decodes here. Without it a browser and an
/// agent following the same link reach different answers on the same file.
///
/// Decoding is not lenient. A sequence that is not valid UTF-8 once decoded is
/// refused, and so is a decoded control character:
/// [`crystalline_core::validate_asset_path`] refuses control characters too,
/// but a NUL reaching a filesystem call truncates the name it is part of, so
/// the guarantee is made here rather than borrowed. A path with nothing to
/// decode comes back unchanged.
fn decoded_uri_path(path: &str) -> Result<String, ErrorData> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_err(|e| {
            ErrorData::invalid_params(
                format!("resource uri path '{path}' is not valid UTF-8 once percent-decoded: {e}"),
                None,
            )
        })?;
    if decoded.chars().any(char::is_control) {
        return Err(ErrorData::invalid_params(
            format!("resource uri path '{path}' percent-decodes to a control character"),
            None,
        ));
    }
    Ok(decoded.into_owned())
}

/// One attachment's bytes as resource contents, in the shape its mime asks
/// for: text for the readable formats
/// [`crystalline_core::is_text_attachment_mime`] names, base64 for everything
/// else.
///
/// A text mime whose bytes are not valid UTF-8 falls back to the blob shape
/// rather than losing them to a lossy conversion: a `.txt` in some other
/// encoding is still the file the caller asked for, and a client that decodes
/// the base64 gets it byte for byte.
fn attachment_contents(uri: &str, bytes: Vec<u8>, mime: &str) -> ResourceContents {
    if crystalline_core::is_text_attachment_mime(mime) {
        match String::from_utf8(bytes) {
            Ok(text) => return ResourceContents::text(text, uri).with_mime_type(mime),
            Err(e) => {
                return ResourceContents::blob(BASE64.encode(e.into_bytes()), uri)
                    .with_mime_type(mime);
            }
        }
    }
    ResourceContents::blob(BASE64.encode(bytes), uri).with_mime_type(mime)
}

/// Wrap an engine value as a successful tool result. The compact JSON is the
/// single text content block; callers that need structured data re-parse it.
fn ok(value: Value) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&value)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// A `resource_link` content block addressing the engram a write touched, so
/// an era-aware client can follow the handle instead of rebuilding the address
/// out of two payload fields. Same unconditional policy as `read_engram`'s
/// attachment links: a link, never bytes.
fn engram_link(domain: &str, permalink: &str, title: &str) -> ContentBlock {
    ContentBlock::resource_link(
        Resource::new(
            format!("crystalline://{domain}/{permalink}"),
            title.to_string(),
        )
        .with_mime_type("text/markdown"),
    )
}

/// [`ok`] for a `write_engram` or `edit_engram` result, with the link to the
/// engram appended: `domain` and `permalink` read off the result itself, named
/// by its `title` when it carries one (a write does, an edit does not).
///
/// A shape this does not recognize simply gets no link. A result that grew a
/// different spelling costs a client one lookup it was doing anyway; a link
/// built from half a shape would send it somewhere else entirely, and no
/// engine result is worth a panic in the layer that only reports it.
fn ok_written(value: Value) -> Result<CallToolResult, ErrorData> {
    let link = (|| {
        let domain = value.get("domain").and_then(Value::as_str)?;
        let permalink = value.get("permalink").and_then(Value::as_str)?;
        let title = value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(permalink);
        Some(engram_link(domain, permalink, title))
    })();
    let mut result = ok(value)?;
    result.content.extend(link);
    Ok(result)
}

/// [`ok`] for a `move_engram` result, with the link to where the engram
/// landed: the destination is the point of the call, so the handle names the
/// address the engram answers to now, off the result's own `to` block. Same
/// tolerance as [`ok_written`] for a shape that is not there.
fn ok_moved(value: Value) -> Result<CallToolResult, ErrorData> {
    let link = (|| {
        let to = value.get("to")?;
        let domain = to.get("domain").and_then(Value::as_str)?;
        let permalink = to.get("permalink").and_then(Value::as_str)?;
        Some(engram_link(domain, permalink, permalink))
    })();
    let mut result = ok(value)?;
    result.content.extend(link);
    Ok(result)
}

/// The sentence `delete_engram` asks before it acts, rendered from
/// [`crate::engine::Engine::delete_preview`]'s two shapes.
///
/// **The attachment clause says "orphaned" rather than "deleted" because that
/// is what happens.** Deleting an engram removes its markdown and its index
/// rows; the files it referenced stay in the domain, and the ones nothing else
/// referenced become exactly the orphaned attachments the maintenance sweep
/// reports. Naming them is what lets the user delete those too, in the same
/// breath, with the `assets/` form of this verb.
fn delete_question(preview: &Value) -> String {
    let domain = preview["domain"].as_str().unwrap_or_default();
    if preview["attachment"] == json!(true) {
        let path = preview["path"].as_str().unwrap_or_default();
        let size = preview["size"].as_u64().unwrap_or_default();
        return format!(
            "Delete attachment '{path}' ({size} bytes) from '{domain}'? This cannot be undone."
        );
    }
    let title = preview["title"].as_str().unwrap_or_default();
    let permalink = preview["permalink"].as_str().unwrap_or_default();
    let enumerated: Option<Vec<String>> = preview["attachments"].as_array().map(|paths| {
        paths
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    });
    let clause = preview_attachment_clause(enumerated.as_deref());
    format!("Delete '{title}' ({domain}/{permalink})? {clause} This cannot be undone.")
}

/// The sentence `share_changes` asks before it publishes, rendered from
/// [`crate::engine::Engine::origin_share_preview`]'s plan: the action (update
/// keeps the proposal's number and URL in front of the user), the effective
/// title and the change mix, naming at most ten files.
///
/// **The two actions label the title line differently, because the value
/// means different things to each.** On a create it is the proposal's title
/// on GitHub, so it is labelled `Title`. On an update it is always the fresh
/// commit's message, so it is labelled `Commit message` - which stays honest
/// either way the caller went: with no `title` argument
/// [`crystalline_remote::ops`]'s update forwards `None` and the pull request
/// keeps whatever title it was opened with, and with a `title` argument the
/// same value is both the commit message and the retitling PATCH the update
/// sends. Labelling it `Title` would promise a retitle in the first case,
/// which is the one a caller lands on by default.
///
/// **The two stacked plans split the same way.** A `stack` opens a pull
/// request of its own, so it is titled and labelled `Title` like a create,
/// and it names the layer it lands on because that is the whole difference
/// between it and a lone proposal. An `amend` puts a fresh commit on a
/// proposal that already exists, so it labels the value `Commit message` like
/// an update, and it says how many layers above it will be re-based: the user
/// is being asked about work they already put in front of reviewers, not only
/// about the layer they named.
fn share_question(preview: &Value) -> String {
    // `label` rides along with the action for exactly the reason above.
    let (action, label) = match preview["action"].as_str().unwrap_or_default() {
        "update" => (
            format!(
                "Update open proposal #{} ({})",
                preview["number"].as_u64().unwrap_or_default(),
                preview["url"].as_str().unwrap_or_default()
            ),
            "Commit message",
        ),
        "stack" => (
            format!(
                "Stacks a new proposal on top of #{} ({})",
                preview["top_number"].as_u64().unwrap_or_default(),
                preview["top_title"].as_str().unwrap_or_default()
            ),
            "Title",
        ),
        "amend" => (
            format!(
                "Amends proposal #{}; {} layer(s) above will be re-based",
                preview["number"].as_u64().unwrap_or_default(),
                preview["layers_above"].as_u64().unwrap_or_default()
            ),
            "Commit message",
        ),
        _ => ("Open a new proposal".to_string(), "Title"),
    };
    let title = preview["effective_title"].as_str().unwrap_or_default();
    let empty = Vec::new();
    let changes = preview["changes"].as_array().unwrap_or(&empty);
    let (mut added, mut updated, mut deleted) = (0usize, 0usize, 0usize);
    for c in changes {
        match c["kind"].as_str() {
            Some("added") => added += 1,
            Some("modified") => updated += 1,
            Some("deleted") => deleted += 1,
            _ => {}
        }
    }
    let names: Vec<&str> = changes
        .iter()
        .take(10)
        .filter_map(|c| c["path"].as_str())
        .collect();
    let more = changes.len().saturating_sub(10);
    let listed = if more > 0 {
        format!("{} and {more} more", names.join(", "))
    } else {
        names.join(", ")
    };
    format!(
        "{action}? {label}: '{title}'. {added} added, {updated} modified, {deleted} deleted: {listed}. Reviewers see the result on GitHub."
    )
}

/// Trims `origin_status`'s per-domain proposal records to what a status
/// glance needs: number, url, title, status, review_state, amended_upstream,
/// feedback_count, updated_at, position. The bodies stay out on purpose -
/// update_domain and the REST payload carry them - so status never bloats a
/// session with comment text the agent did not ask for.
///
/// `position` is the layer's place in the open chain, 1-based from the bottom,
/// read off the open list's own order (the engine builds it in chain order).
/// It is what a reader keys off to know it is looking at a layer at all, so it
/// is present on both arrays for one shape, and null on a declined proposal,
/// which stands in no chain.
///
/// **The four domain-level stack keys are dropped while they are quiet**, and
/// that is deliberately not what [`crate::origin::status_report_json`] does:
/// the JSON surface emits all four always so one reader handles either path,
/// while this one is a context budget. A `stack_number` of null, an empty
/// `stack_wedged` and either debt flag false say nothing a caller can act on,
/// so they say nothing at all. A null `stack_number` beside real positions is
/// the degraded chain rather than an unstacked domain - the layers exist and
/// are simply not grouped on the forge yet - and `stack_link_pending` is the
/// key that survives to carry that debt.
fn lean_origin_status(mut value: Value) -> Value {
    if let Some(domains) = value.get_mut("domains").and_then(Value::as_array_mut) {
        for domain in domains {
            for key in ["open_proposals", "declined_proposals"] {
                let in_the_chain = key == "open_proposals";
                if let Some(entries) = domain.get_mut(key).and_then(Value::as_array_mut) {
                    for (index, entry) in entries.iter_mut().enumerate() {
                        *entry = json!({
                            "number": entry["number"],
                            "url": entry["url"],
                            "title": entry["title"],
                            "status": entry["status"],
                            "review_state": entry["review_state"],
                            "amended_upstream": entry
                                .get("amended_upstream")
                                .cloned()
                                .unwrap_or(json!(false)),
                            "feedback_count": entry["feedback"]
                                .as_array()
                                .map(Vec::len)
                                .unwrap_or(0),
                            "updated_at": entry["updated_at"],
                            "position": if in_the_chain {
                                json!(index + 1)
                            } else {
                                Value::Null
                            },
                        });
                    }
                }
            }
            drop_quiet_stack_keys(domain);
        }
    }
    value
}

/// Removes the stack keys that carry no fact from one lean domain entry: a
/// null `stack_number`, an empty `stack_wedged`, and `repair_pending` or
/// `stack_link_pending` set false. Anything else stays, including a
/// `stack_wedged` list, because a wedged layer is named by the number a
/// caller withdraws or shares against.
fn drop_quiet_stack_keys(domain: &mut Value) {
    let Some(object) = domain.as_object_mut() else {
        return;
    };
    if object.get("stack_number").is_some_and(Value::is_null) {
        object.remove("stack_number");
    }
    if object
        .get("stack_wedged")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        object.remove("stack_wedged");
    }
    for key in ["repair_pending", "stack_link_pending"] {
        if object.get(key) == Some(&json!(false)) {
            object.remove(key);
        }
    }
}

/// What an unconfirmed share tells the model, naming what did not happen.
const SHARE_REFUSAL: &str = "The share was not confirmed, so nothing was shared. Call share_changes again if the user asks for it.";

/// The sentence a conflict asks when the caller named no resolution: the
/// conflict's path and kind, then a bounded preview of both sides.
///
/// The preview is the whole point of asking rather than refusing - a user
/// choosing between "mine" and "theirs" is choosing between two texts, and
/// only one of them is anywhere near the conversation. It is bounded at the
/// first 20 lines a side because a question is rendered in a dialog, not in a
/// pager; a cut side ends in an ellipsis line so the reader knows there is
/// more, and a side that is absent or unreadable says so rather than
/// rendering as empty (an empty file and a deleted one are different
/// decisions).
///
/// **A null side is two different facts, and `note` is what tells them
/// apart.** [`crate::engine::Engine::origin_conflict_detail`] nulls a side that
/// is not there *and* a side that is there but is not UTF-8, setting `note`
/// only in the second case. Reading every null as a deletion would tell a user
/// a file they can see on disk was deleted, so a null under a standing note
/// quotes the note instead. The engine's `note` is one field for the whole
/// detail, overwritten by whichever side was last found *unreadable* as it
/// checks base, then local, then upstream - base included, and the question
/// never previews base. A readable later side leaves an earlier note standing,
/// so any unreadable side at all makes a genuinely absent side quote a note
/// about another side. What it can no longer do is claim a present file was
/// deleted, which is the reading that would have cost the user the choice.
fn conflict_resolution_question(detail: &Value) -> String {
    let path = detail["path"].as_str().unwrap_or_default();
    let kind = detail["kind"].as_str().unwrap_or("conflict");
    let note = detail["note"].as_str();
    let preview = |side: &Value| -> String {
        match side.as_str() {
            None => match note {
                Some(note) => format!("(no readable content: {note})"),
                None => "(file deleted)".to_string(),
            },
            Some(text) => {
                let mut out = text
                    .lines()
                    .take(CONFLICT_PREVIEW_LINES)
                    .collect::<Vec<&str>>()
                    .join("\n");
                if text.lines().count() > CONFLICT_PREVIEW_LINES {
                    out.push_str("\n...");
                }
                out
            }
        }
    };
    format!(
        "Conflict on {path} ({kind}). Keep which side?\n\n--- local (mine) ---\n{}\n\n--- upstream (theirs) ---\n{}",
        preview(&detail["local"]),
        preview(&detail["upstream"]),
    )
}

/// How much of each side [`conflict_resolution_question`] shows.
const CONFLICT_PREVIEW_LINES: usize = 20;

/// The non-eliciting refusal for a call that named no resolution: a tool error
/// the model can read, replacing the framework's opaque InvalidParams.
///
/// A peer that cannot be asked has to be told what to send instead, so all
/// three resolutions are named, merged included - it is the one the question
/// itself never offers.
const RESOLVE_NEEDS_RESOLUTION: &str = "resolve_conflict needs a resolution: mine (keep your version), theirs (take the team's version), or merged with the reconciled content.";

/// What an unanswered resolution question tells the model, naming what is
/// still true rather than what failed.
const RESOLVE_REFUSAL: &str = "The resolution was not chosen, so the conflict is still open. Call resolve_conflict again if the user asks for it.";

/// The middle sentence of [`delete_question`]: what the delete does to the
/// engram's attachments.
///
/// `Some` is an answer and `None` is the absence of one. A list - empty
/// included - was enumerated by
/// [`crate::engine::Engine::delete_preview`] and names exactly what the delete
/// orphans. `None` is a domain past
/// [`crate::engine::MAX_PREVIEW_SCAN_ENGRAMS`], where naming them would mean
/// reading every engram in the domain to write one sentence.
///
/// **The unenumerated wording promises less, never more.** It says the
/// attachments were not looked at and that any sole-referent ones are left
/// orphaned, which is true of every delete this verb performs; what changes
/// past the bound is what the question can tell the user, not what saying yes
/// to it does.
fn preview_attachment_clause(enumerated: Option<&[String]>) -> String {
    let Some(paths) = enumerated else {
        return "Its attachments are not enumerated on this large domain; any sole-referent ones are left orphaned.".to_string();
    };
    let listed = paths.join(", ");
    let attachments = if listed.is_empty() { "none" } else { &listed };
    format!("This leaves its sole-referent attachments orphaned: {attachments}.")
}

/// The permalink the engine's collision message names, when it names one.
///
/// The engine words that failure `permalink '<permalink>' already exists in
/// domain '<domain>' (at <path>); ...`, and reading the value back out of it is
/// both cheaper and truer than re-deriving it here: slugification is the
/// engine's (the folder prefix, the reserved names, the lot), and a second
/// implementation on this side could name a permalink the write would never
/// have used. `None` when the phrase is not there, which sends the caller back
/// to the bare error rather than to a question naming the wrong thing.
fn collision_permalink(message: &str) -> Option<&str> {
    let (_, rest) = message.split_once("permalink '")?;
    let (permalink, _) = rest.split_once('\'')?;
    (!permalink.is_empty()).then_some(permalink)
}

/// The sentence a permalink collision asks instead of reporting.
///
/// It names all three things the user needs to decide with - what would be
/// written, where it would land and what is already there - because the choice
/// is between two engrams, and only one of them is in front of the caller.
fn collision_question_text(p: &WriteParams, permalink: &str) -> String {
    format!(
        "'{}' would land at permalink '{permalink}' which already exists in '{}'. Overwrite it, or cancel?",
        p.title.trim(),
        p.domain.trim()
    )
}

/// What an unresolved collision tells the model: what is still there, and the
/// one argument that would have replaced it.
const COLLISION_REFUSAL: &str = "The overwrite was not confirmed, so the existing engram was left in place; nothing was written. Call write_engram again with overwrite=true if the user asks for it.";

/// The sentence an `evolve_ack` assignment asks before it acts, rendered from
/// [`crate::engine::Engine::ack_preview`].
///
/// Both halves name the consequence rather than the write, because that is
/// what the user is deciding: a record keeps a finding out of every future
/// sweep, a removal puts it back into the next one. The engram is named by the
/// permalink the identifier resolved to rather than by the identifier itself,
/// so a yes is given to the engram the write lands on - a title, a bare
/// permalink and a `crystalline://` URL all reach the same question, and an
/// identifier that reaches nothing never becomes one.
fn ack_question(preview: &Value, intent: &AckIntent) -> String {
    let permalink = preview["permalink"].as_str().unwrap_or_default();
    let domain = preview["domain"].as_str().unwrap_or_default();
    match intent {
        AckIntent::Record { rule, note } => {
            let note = note
                .as_deref()
                .map(|note| format!(" The note reads: '{note}'."))
                .unwrap_or_default();
            format!(
                "Acknowledge {rule} on '{permalink}' in '{domain}'? This records the finding as intentional until its evidence changes.{note}"
            )
        }
        AckIntent::Remove { rule } => format!(
            "Remove the {rule} acknowledgment on '{permalink}' in '{domain}'? The finding resurfaces on the next sweep."
        ),
    }
}

/// What an unconfirmed `evolve_ack` assignment tells the model, naming what did
/// not happen so it does not retry blind.
fn ack_refusal(intent: &AckIntent) -> String {
    let (act, state) = match intent {
        AckIntent::Record { .. } => ("acknowledgment", "nothing was recorded"),
        AckIntent::Remove { .. } => ("removal", "the acknowledgment is still there"),
    };
    format!(
        "The {act} was not confirmed, so {state}. Call edit_engram again if the user asks for it."
    )
}

/// A call-time refusal: the tool ran and could not do its job because a
/// server-side condition is off.
///
/// **Deliberately a tool-level error rather than a JSON-RPC one.** Every gate
/// that stopped shaping the tool list under SEP-2567 refuses here instead, so
/// the model that called the tool has to be able to read why: rmcp's own
/// guidance is that "MCP clients typically render protocol errors opaquely
/// [...] the caller will not see your message" and that `CallToolResult::error`
/// is the right shape for a failure the caller should act on (rmcp 3.1.2
/// `handler/server.rs:454-480`, `model.rs:3892-3913`). The message names the
/// condition and how to change it, which is the same thing the tool's
/// description says, exactly as SEP-2567 prescribes.
fn refuse(message: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        message.into(),
    )]))
}

/// Map an engine error to an rmcp tool error with an actionable message.
fn to_error(e: EngineError) -> ErrorData {
    match e {
        EngineError::UnknownDomain { .. }
        | EngineError::NotFound(_)
        | EngineError::Ambiguous(_)
        | EngineError::Conflict(_)
        | EngineError::Invalid(_)
        | EngineError::ReadOnly
        | EngineError::EnvTokenConnect => ErrorData::invalid_params(e.to_string(), None),
        EngineError::Remote(remote) => remote_to_error(remote),
        EngineError::Io { .. } | EngineError::Internal(_) => {
            ErrorData::internal_error(e.to_string(), None)
        }
    }
}

/// Map a GitHub collaboration error to an rmcp tool error, splitting by
/// whether the caller is at fault. Transient or environmental variants -
/// offline, rate limited, an expired connection or a still-pending sign-in,
/// plus an unexpected upstream answer, a filesystem or credential-store
/// failure and a rewritten repository history that re-baselines on its own -
/// are never the caller's mistake, so they map to the internal/server error
/// class rather than `invalid_params`; the message (already actionable
/// product copy, see `crystalline_remote::error`) is carried verbatim
/// either way. Genuine input problems - collaboration turned off, no
/// connection yet, an unreachable repository, a repository or subpath with
/// no domain, unresolved conflicts blocking a share, a forge that does not
/// stack proposals, a teaching refusal (`Refused`: a proposal number that
/// names no open layer, a chain that has to be pulled or withdrawn first) or
/// a proposal or conflict path that does not exist - stay
/// `invalid_params`-shaped. A refusal in particular must never land in the
/// server-error class: its whole content is the way out of the situation the
/// caller put themselves in, and an "internal error" verdict in front of it
/// tells the caller the opposite of what the message says. This match is
/// exhaustive over `RemoteError` so a new variant must be classified here
/// rather than silently defaulting.
fn remote_to_error(e: RemoteError) -> ErrorData {
    let message = e.to_string();
    match e {
        RemoteError::Offline
        | RemoteError::RateLimited { .. }
        | RemoteError::AuthExpired
        | RemoteError::AuthPending
        | RemoteError::Api { .. }
        | RemoteError::Io(_)
        | RemoteError::State(_)
        | RemoteError::Credential { .. }
        | RemoteError::BaseUnavailable => ErrorData::internal_error(message, None),
        RemoteError::NotEnabled
        | RemoteError::NotConnected
        | RemoteError::RepoNotFound { .. }
        | RemoteError::NotADomain { .. }
        | RemoteError::ConflictsPending { .. }
        | RemoteError::ProposalNotFound { .. }
        | RemoteError::NoWithdrawTarget { .. }
        | RemoteError::StacksUnsupported
        | RemoteError::Refused(_)
        | RemoteError::ConflictNotFound { .. } => ErrorData::invalid_params(message, None),
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::ErrorCode;

    use super::*;

    /// One `inputResponses` map holding `value` under the `confirm` key.
    fn responses(value: Value) -> Option<rmcp::model::InputResponses> {
        let mut map = rmcp::model::InputResponses::new();
        map.insert(CONFIRM_KEY.to_string(), value);
        Some(map)
    }

    /// **The parser two more confirmation flows will be built on, so every
    /// shape it can be handed is pinned rather than the two that are obvious.**
    ///
    /// The invariant, and the only one that matters: nothing malformed
    /// confirms. `Some(true)` is reachable from exactly one shape - an
    /// accepted question whose content carries the boolean `true` under the
    /// key it was asked for. Everything else that is an answer is a no, and
    /// only the genuine absence of an answer is [`None`], because that is what
    /// opens round one.
    #[test]
    fn confirmed_says_yes_to_one_shape_and_no_to_every_other() {
        let yes = [json!({ "action": "accept", "content": { "confirm": true } })];
        for value in yes {
            assert_eq!(
                confirmed(&responses(value.clone())),
                Some(true),
                "an accepted yes confirms: {value}"
            );
        }

        let no = [
            // Accepted, but not a yes.
            json!({ "action": "accept", "content": { "confirm": false } }),
            // Accepted with nothing in it, or with the wrong thing in it.
            json!({ "action": "accept" }),
            json!({ "action": "accept", "content": {} }),
            json!({ "action": "accept", "content": null }),
            json!({ "action": "accept", "content": { "confirm": "true" } }),
            json!({ "action": "accept", "content": { "confirm": 1 } }),
            json!({ "action": "accept", "content": { "confirmed": true } }),
            // The two refusals the specification names.
            json!({ "action": "decline" }),
            json!({ "action": "cancel", "content": { "confirm": true } }),
            // An action a later revision might add, which we have never heard
            // of and therefore must not read as consent.
            json!({ "action": "deferred", "content": { "confirm": true } }),
            // Shapes that are not an `ElicitResult` at all.
            json!({ "content": { "confirm": true } }),
            json!({ "action": null, "content": { "confirm": true } }),
            json!({ "action": ["accept"], "content": { "confirm": true } }),
            json!("accept"),
            json!(true),
            json!(null),
            json!([{ "action": "accept", "content": { "confirm": true } }]),
        ];
        for value in no {
            assert_eq!(
                confirmed(&responses(value.clone())),
                Some(false),
                "nothing but an accepted yes confirms: {value}"
            );
        }

        // Round one: no answer at all, or an answer to some other question.
        assert_eq!(confirmed(&None), None, "no responses is round one");
        assert_eq!(
            confirmed(&Some(rmcp::model::InputResponses::new())),
            None,
            "an empty map is round one"
        );
        let mut elsewhere = rmcp::model::InputResponses::new();
        elsewhere.insert(
            "something_else".to_string(),
            json!({ "action": "accept", "content": { "confirm": true } }),
        );
        assert_eq!(
            confirmed(&Some(elsewhere)),
            None,
            "a yes filed under another key answers another question"
        );
    }

    /// The same invariant as [`confirmed`], on the parser that decides whether
    /// an existing engram is replaced: nothing malformed overwrites.
    ///
    /// The failure mode this rules out is specific to a string answer. A
    /// boolean has two values and a typo cannot land on the wrong one; a
    /// single-select can be answered with the wrong case, the title instead of
    /// the value, an array of one, or nothing at all, and every one of those
    /// has to read as "leave what is there" rather than as consent to clobber
    /// it.
    #[test]
    fn only_an_accepted_overwrite_replaces_an_existing_engram() {
        let responses = |value: Value| {
            let mut map = rmcp::model::InputResponses::new();
            map.insert(RESOLUTION_KEY.to_string(), value);
            Some(map)
        };

        assert_eq!(
            resolved_overwrite(&responses(
                json!({ "action": "accept", "content": { "resolution": "overwrite" } })
            )),
            Some(true),
            "an accepted overwrite replaces"
        );

        let no = [
            // The other choice, accepted.
            json!({ "action": "accept", "content": { "resolution": "cancel" } }),
            // Accepted with nothing in it, or with the wrong thing in it.
            json!({ "action": "accept" }),
            json!({ "action": "accept", "content": {} }),
            json!({ "action": "accept", "content": null }),
            json!({ "action": "accept", "content": { "resolution": "Overwrite" } }),
            json!({ "action": "accept", "content": { "resolution": "overwrite " } }),
            json!({ "action": "accept", "content": { "resolution": true } }),
            json!({ "action": "accept", "content": { "resolution": ["overwrite"] } }),
            // The title rather than the value behind it.
            json!({ "action": "accept", "content": { "resolution": "Overwrite the existing engram" } }),
            // The right value under the wrong key.
            json!({ "action": "accept", "content": { "confirm": "overwrite" } }),
            // The two refusals the specification names, and one it does not.
            json!({ "action": "decline" }),
            json!({ "action": "cancel", "content": { "resolution": "overwrite" } }),
            json!({ "action": "deferred", "content": { "resolution": "overwrite" } }),
            // Shapes that are not an `ElicitResult` at all.
            json!({ "content": { "resolution": "overwrite" } }),
            json!("overwrite"),
            json!(null),
        ];
        for value in no {
            assert_eq!(
                resolved_overwrite(&responses(value.clone())),
                Some(false),
                "nothing but an accepted overwrite replaces: {value}"
            );
        }

        // Round one: no answer at all, or an answer to some other question.
        assert_eq!(resolved_overwrite(&None), None, "no responses is round one");
        assert_eq!(
            resolved_overwrite(&Some(rmcp::model::InputResponses::new())),
            None,
            "an empty map is round one"
        );
        let mut elsewhere = rmcp::model::InputResponses::new();
        elsewhere.insert(
            CONFIRM_KEY.to_string(),
            json!({ "action": "accept", "content": { "resolution": "overwrite" } }),
        );
        assert_eq!(
            resolved_overwrite(&Some(elsewhere)),
            None,
            "an answer filed under the confirmation key answers another question"
        );
    }

    /// The third parser under the same key, and the same invariant read for a
    /// three-valued answer: only the two words the schema offered come back,
    /// and everything else that is an answer resolves nothing.
    ///
    /// The distinction this one has to keep that the boolean ones do not is
    /// between two yeses. `mine` and `theirs` write opposite files, so an
    /// answer that is nearly one of them must not fall through to the other;
    /// it falls into `Some(None)`, which leaves the conflict open.
    #[test]
    fn only_the_two_offered_sides_resolve_a_conflict() {
        let responses = |value: Value| {
            let mut map = rmcp::model::InputResponses::new();
            map.insert(RESOLUTION_KEY.to_string(), value);
            Some(map)
        };

        assert_eq!(
            chosen_resolution(&responses(
                json!({ "action": "accept", "content": { "resolution": "mine" } })
            )),
            Some(Some("mine")),
            "an accepted mine keeps the local version"
        );
        assert_eq!(
            chosen_resolution(&responses(
                json!({ "action": "accept", "content": { "resolution": "theirs" } })
            )),
            Some(Some("theirs")),
            "an accepted theirs takes the team's version"
        );

        let unresolved = [
            // The one resolution the question never offers.
            json!({ "action": "accept", "content": { "resolution": "merged" } }),
            // Accepted with nothing in it, or with the wrong thing in it.
            json!({ "action": "accept" }),
            json!({ "action": "accept", "content": {} }),
            json!({ "action": "accept", "content": null }),
            json!({ "action": "accept", "content": { "resolution": "Mine" } }),
            json!({ "action": "accept", "content": { "resolution": "theirs " } }),
            json!({ "action": "accept", "content": { "resolution": true } }),
            json!({ "action": "accept", "content": { "resolution": ["mine"] } }),
            // The title rather than the value behind it.
            json!({ "action": "accept", "content": { "resolution": "Keep my local version" } }),
            // The right value under the wrong key.
            json!({ "action": "accept", "content": { "confirm": "mine" } }),
            // The two refusals the specification names, and one it does not.
            json!({ "action": "decline" }),
            json!({ "action": "cancel", "content": { "resolution": "theirs" } }),
            json!({ "action": "deferred", "content": { "resolution": "theirs" } }),
            // Shapes that are not an `ElicitResult` at all.
            json!({ "content": { "resolution": "mine" } }),
            json!("mine"),
            json!(null),
        ];
        for value in unresolved {
            assert_eq!(
                chosen_resolution(&responses(value.clone())),
                Some(None),
                "nothing but an accepted mine or theirs resolves: {value}"
            );
        }

        // Round one: no answer at all, or an answer to some other question.
        assert_eq!(chosen_resolution(&None), None, "no responses is round one");
        assert_eq!(
            chosen_resolution(&Some(rmcp::model::InputResponses::new())),
            None,
            "an empty map is round one"
        );
        let mut elsewhere = rmcp::model::InputResponses::new();
        elsewhere.insert(
            CONFIRM_KEY.to_string(),
            json!({ "action": "accept", "content": { "resolution": "mine" } }),
        );
        assert_eq!(
            chosen_resolution(&Some(elsewhere)),
            None,
            "an answer filed under the confirmation key answers another question"
        );
    }

    /// The question shows both sides, bounded, and says which kind of nothing
    /// it is showing when a side has no text.
    ///
    /// The three things asserted are the three a user's decision rests on: a
    /// side longer than the budget is visibly cut rather than silently
    /// truncated, a null side with no note is a deleted file rather than an
    /// empty one, and a null side under a standing note is a file that is
    /// there and cannot be read - which must never be reported as a deletion,
    /// because a user told their file was deleted decides differently from one
    /// told it is binary.
    #[test]
    fn the_conflict_question_previews_both_sides_within_a_budget() {
        let long: String = (1..=25)
            .map(|n| format!("line {n}\n"))
            .collect::<Vec<String>>()
            .join("");
        let detail = json!({
            "path": "notes/a.md",
            "kind": "EditEdit",
            "local": long,
            "upstream": Value::Null,
        });
        let message = conflict_resolution_question(&detail);

        assert!(
            message.starts_with("Conflict on notes/a.md (EditEdit). Keep which side?"),
            "{message}"
        );
        assert!(
            message.contains("--- local (mine) ---\nline 1\n"),
            "{message}"
        );
        assert!(
            message.contains("line 20\n..."),
            "cut at the budget: {message}"
        );
        assert!(
            !message.contains("line 21"),
            "and nothing past it: {message}"
        );
        assert!(
            message.contains("--- upstream (theirs) ---\n(file deleted)"),
            "an absent side says what it is: {message}"
        );

        // A side exactly at the budget is whole and carries no ellipsis.
        let exact: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let detail = json!({ "path": "a.md", "local": exact, "upstream": "one line" });
        let message = conflict_resolution_question(&detail);
        assert!(!message.contains("\n..."), "nothing was cut: {message}");
        // And a detail with no kind still reads as a sentence.
        assert!(
            message.starts_with("Conflict on a.md (conflict)."),
            "{message}"
        );

        // The same null side, under the note the engine sets when it nulled a
        // side that is there but is not UTF-8: the file is present, so the
        // question must not say it was deleted.
        let detail = json!({
            "path": "notes/a.md",
            "kind": "EditEdit",
            "local": "alpha, my local edit",
            "upstream": Value::Null,
            "note": "the upstream side is not UTF-8 and is omitted",
        });
        let message = conflict_resolution_question(&detail);
        assert!(
            message.contains(
                "--- upstream (theirs) ---\n(no readable content: the upstream side is not UTF-8 and is omitted)"
            ),
            "an unreadable side quotes the note: {message}"
        );
        assert!(
            !message.contains("(file deleted)"),
            "and is never reported as a deletion: {message}"
        );
    }

    /// The engine message is parsed for the permalink, and a message that does
    /// not carry one never becomes a question naming the wrong thing.
    ///
    /// The positive case is worded exactly as `engine.rs` words it - the same
    /// sentence `a_permalink_collision_carries_the_marker_the_mcp_layer_intercepts`
    /// pins from the engine side - so the two halves of the seam are asserted
    /// against the same string.
    #[test]
    fn the_colliding_permalink_is_read_out_of_the_engine_message() {
        assert_eq!(
            collision_permalink(
                "permalink 'topic/taken' already exists in domain 'eng' (at topic/taken.md); pass overwrite=true to replace"
            ),
            Some("topic/taken"),
            "the folder prefix survives, because the engine put it there"
        );

        for message in [
            "the domain 'eng' is read only",
            "permalink 'unterminated, so there is nothing to read out",
            "permalink '' already exists in domain 'eng' (at .md)",
        ] {
            assert_eq!(
                collision_permalink(message),
                None,
                "a message naming no permalink yields none: {message}"
            );
        }
    }

    /// The three things the attachment clause can say, and the one it must
    /// never say: that a domain too large to enumerate has no sole-referent
    /// attachments.
    ///
    /// A list is an answer, an empty list is the answer "none", and `None` is
    /// the absence of one. The unenumerated wording is asserted verbatim
    /// because it is the sentence a user decides a delete on.
    #[test]
    fn the_attachment_clause_says_nothing_it_did_not_look_for() {
        assert_eq!(
            preview_attachment_clause(Some(&[
                "assets/solo.png".to_string(),
                "assets/deck.pptx".to_string(),
            ])),
            "This leaves its sole-referent attachments orphaned: assets/solo.png, assets/deck.pptx.",
            "an enumerated list names every path the delete orphans"
        );
        assert_eq!(
            preview_attachment_clause(Some(&[])),
            "This leaves its sole-referent attachments orphaned: none.",
            "an empty enumeration is the answer 'none', not a missing one"
        );
        assert_eq!(
            preview_attachment_clause(None),
            "Its attachments are not enumerated on this large domain; any sole-referent ones are left orphaned.",
            "past the scan bound the question says nobody looked"
        );
    }

    /// And the whole sentence, both ways round, because the clause is only
    /// ever read inside it: the engram is named, the consequence is stated and
    /// the delete is still called what it is.
    #[test]
    fn the_delete_question_wraps_whichever_clause_the_preview_earned() {
        let asked = delete_question(&json!({
            "domain": "eng",
            "permalink": "eng/doomed",
            "title": "Doomed",
            "path": "eng/doomed.md",
            "attachments": ["assets/solo.png"],
        }));
        assert_eq!(
            asked,
            "Delete 'Doomed' (eng/eng/doomed)? This leaves its sole-referent attachments orphaned: assets/solo.png. This cannot be undone."
        );

        let unenumerated = delete_question(&json!({
            "domain": "eng",
            "permalink": "eng/doomed",
            "title": "Doomed",
            "path": "eng/doomed.md",
            "attachments": Value::Null,
        }));
        assert_eq!(
            unenumerated,
            "Delete 'Doomed' (eng/eng/doomed)? Its attachments are not enumerated on this large domain; any sole-referent ones are left orphaned. This cannot be undone."
        );
    }

    #[test]
    fn the_share_question_names_update_create_and_caps_the_file_list() {
        let update = share_question(&json!({
            "action": "update", "number": 4, "url": "https://github.test/pulls/4",
            "effective_title": "Refine 1 engram in kb",
            "changes": [{ "path": "notes/a.md", "kind": "modified" }],
        }));
        assert!(
            update.contains("Update open proposal #4 (https://github.test/pulls/4)"),
            "{update}"
        );
        // An update's title line is the commit message, never a promise to
        // retitle a proposal the caller passed no title for.
        assert!(
            update.contains("Commit message: 'Refine 1 engram in kb'"),
            "{update}"
        );
        assert!(
            !update.contains("Title: '"),
            "an update never labels it Title: {update}"
        );
        assert!(
            update.contains("0 added, 1 modified, 0 deleted: notes/a.md"),
            "{update}"
        );

        let changes: Vec<Value> = (0..12)
            .map(|i| json!({ "path": format!("notes/f{i}.md"), "kind": "added" }))
            .collect();
        let create = share_question(&json!({
            "action": "create", "effective_title": "Share 12 new engrams from kb",
            "changes": changes,
        }));
        assert!(create.contains("Open a new proposal"), "{create}");
        // A create really does title the proposal, so it says so.
        assert!(
            create.contains("Title: 'Share 12 new engrams from kb'"),
            "{create}"
        );
        assert!(create.contains("and 2 more"), "{create}");
        assert!(!create.contains("notes/f10.md"), "capped at ten: {create}");
    }

    /// The two stacked plans the same question renders, in the same framing
    /// the create and update legs use.
    ///
    /// A stack names the layer it lands on, because "on top of what" is the
    /// only thing that distinguishes it from opening a lone proposal; an
    /// amend names the cascade, because saying yes to it moves work the user
    /// already put in front of reviewers.
    #[test]
    fn share_question_names_the_stack_and_amend_actions() {
        let stacked = share_question(&json!({
            "action": "stack", "top_number": 6, "top_title": "Refine alpha",
            "effective_title": "Share 1 new engram from kb",
            "changes": [{ "path": "notes/b.md", "kind": "added" }],
        }));
        assert!(
            stacked.contains("Stacks a new proposal on top of #6 (Refine alpha)"),
            "{stacked}"
        );
        // A new layer really is titled on the forge, so it labels the value
        // Title exactly as a create does.
        assert!(
            stacked.contains("Title: 'Share 1 new engram from kb'"),
            "{stacked}"
        );
        assert!(
            stacked.contains("1 added, 0 modified, 0 deleted: notes/b.md"),
            "{stacked}"
        );

        let amended = share_question(&json!({
            "action": "amend", "number": 9, "url": "https://github.test/pulls/9",
            "layers_above": 1,
            "effective_title": "Answer the review on layer 2",
            "changes": [{ "path": "notes/a.md", "kind": "modified" }],
        }));
        assert!(
            amended.contains("Amends proposal #9; 1 layer(s) above will be re-based"),
            "{amended}"
        );
        // An amend is a fresh commit on an existing proposal, so the label is
        // the update leg's, never a promise to retitle.
        assert!(
            amended.contains("Commit message: 'Answer the review on layer 2'"),
            "{amended}"
        );
        assert!(!amended.contains("Title: '"), "{amended}");
    }

    #[test]
    fn transient_remote_errors_map_to_the_internal_error_class() {
        let cases = [
            RemoteError::Offline,
            RemoteError::RateLimited { reset: None },
            RemoteError::AuthExpired,
            RemoteError::AuthPending,
            RemoteError::Api {
                status: 502,
                message: "bad gateway".to_string(),
            },
            RemoteError::State("corrupt".to_string()),
            RemoteError::Credential {
                detail: "locked".to_string(),
            },
            RemoteError::BaseUnavailable,
        ];
        for e in cases {
            let message = e.to_string();
            let err = remote_to_error(e);
            assert_eq!(
                err.code,
                ErrorCode::INTERNAL_ERROR,
                "{message} should not read as a client mistake"
            );
            assert_eq!(err.message, message, "the actionable message is verbatim");
        }
    }

    #[test]
    fn genuine_input_remote_errors_map_to_invalid_params() {
        let cases = [
            RemoteError::NotEnabled,
            RemoteError::NotConnected,
            RemoteError::RepoNotFound {
                repo: "acme/brand-knowledge".to_string(),
            },
            RemoteError::NotADomain {
                repo: "acme/brand-knowledge".to_string(),
                path: None,
            },
            RemoteError::ConflictsPending { count: 2 },
            RemoteError::ProposalNotFound { number: 7 },
            RemoteError::StacksUnsupported,
            // A teaching refusal is a client mistake with the way out
            // attached: the caller named a proposal that is not an open
            // layer. Classing it as a server fault would tell them the
            // opposite of what its own text says.
            RemoteError::Refused(
                "proposal #9 is not an open layer of this domain; open layers: #3 (layer 1)"
                    .to_string(),
            ),
            RemoteError::ConflictNotFound {
                path: "notes/a.md".to_string(),
                open: vec![],
            },
        ];
        for e in cases {
            let message = e.to_string();
            let err = remote_to_error(e);
            assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{message}");
            assert_eq!(err.message, message);
        }
    }

    #[test]
    fn to_error_routes_remote_through_the_same_class_split() {
        let err = to_error(EngineError::Remote(RemoteError::NotEnabled));
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

        let err = to_error(EngineError::Remote(RemoteError::Offline));
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
    }

    #[test]
    fn is_collab_tool_recognizes_exactly_the_six() {
        for name in COLLAB_TOOLS {
            assert!(is_collab_tool(name), "{name}");
        }
        // add_domain is write-gated, not collab-gated.
        assert!(!is_collab_tool("add_domain"));
        assert!(is_write_tool("add_domain"));
        assert!(!is_collab_tool("write_engram"));
        assert!(!is_collab_tool("search_engrams"));
    }

    /// The locked matrix, one row per (setting, resolved answer) pair. The
    /// second argument is never the connecting client: it is what the spawned
    /// process resolved from its `--harness` argument and this machine's
    /// receipt before the session started.
    #[test]
    fn hidden_skills_surface_matches_the_locked_matrix() {
        for onboarded in [true, false] {
            assert!(
                !hidden_skills_surface(SkillsServe::Always, onboarded),
                "true always serves, whoever spawned us"
            );
            assert!(
                hidden_skills_surface(SkillsServe::Never, onboarded),
                "false never serves, whoever spawned us"
            );
        }
        assert!(
            hidden_skills_surface(SkillsServe::Auto, true),
            "auto plus an onboarded harness is the whole point of the feature"
        );
        assert!(
            !hidden_skills_surface(SkillsServe::Auto, false),
            "auto serves everyone else, which is every case we cannot resolve"
        );
    }

    /// Only `auto` plus an onboarded harness shrinks the instructions: `false`
    /// gates skill serving, never onboarding.
    #[test]
    fn minimal_instructions_are_auto_and_onboarded_only() {
        assert!(minimal_instructions(SkillsServe::Auto, true));
        assert!(!minimal_instructions(SkillsServe::Auto, false));
        assert!(!minimal_instructions(SkillsServe::Always, true));
        assert!(
            !minimal_instructions(SkillsServe::Never, true),
            "turning the skill surface off must not cost a client its routing block"
        );
        assert!(!minimal_instructions(SkillsServe::Never, false));
    }

    /// The two gates take the same two inputs, so the surface and the
    /// instructions can never disagree about whether this deployment is
    /// already onboarded. They diverged once, when one keyed on the client's
    /// name and the other did not, and that divergence is what SEP-2567
    /// forbade.
    #[test]
    fn the_surface_and_the_instructions_read_the_same_answer() {
        for serve in [SkillsServe::Auto, SkillsServe::Always, SkillsServe::Never] {
            for onboarded in [true, false] {
                if serve == SkillsServe::Auto {
                    assert_eq!(
                        hidden_skills_surface(serve, onboarded),
                        minimal_instructions(serve, onboarded),
                        "auto decides both together"
                    );
                }
            }
        }
    }

    #[test]
    fn skill_uris_round_trip_to_their_assets() {
        for asset in SKILL_ASSETS {
            let uri = skill_uri(asset.name);
            assert_eq!(uri, format!("skill://{}/SKILL.md", asset.name));
            assert_eq!(skill_for_uri(&uri).map(|a| a.name), Some(asset.name));
        }
        assert!(skill_for_uri("skill://crystalline-routing").is_none());
        assert!(skill_for_uri("skill://nonesuch/SKILL.md").is_none());
        assert!(skill_for_uri("https://example.com/SKILL.md").is_none());
    }

    /// `origin_status`'s trim, over both proposal arrays at once.
    ///
    /// The bodies leaving is the assertion that earns this test: a status
    /// glance that carried reviewer comment text would spend a session's
    /// context on prose nobody asked for, and `update_domain` is the surface
    /// that returns it. The declined entry is the second half: the engine
    /// decorates only the open list with `amended_upstream`, so the trim has
    /// to supply the missing key rather than leave the two arrays different
    /// shapes.
    #[test]
    fn lean_origin_status_trims_both_proposal_arrays_to_the_nine_keys() {
        const LEAN_KEYS: [&str; 9] = [
            "number",
            "url",
            "title",
            "status",
            "review_state",
            "amended_upstream",
            "feedback_count",
            "updated_at",
            "position",
        ];

        let leaned = lean_origin_status(json!({
            "domains": [{
                "domain": "kb",
                "repo": "team/knowledge",
                "stack_number": Value::Null,
                "stack_wedged": [],
                "repair_pending": false,
                "stack_link_pending": false,
                "open_proposals": [{
                    "number": 7,
                    "url": "https://example.invalid/pull/7",
                    "branch": "crystalline/kb-7",
                    "title": "Refine alpha",
                    "status": "Open",
                    "review_state": "changes_requested",
                    "amended_upstream": true,
                    "files": [{ "path": "notes/a.md" }],
                    "feedback": [
                        { "author": "ana", "body": "needs a source" },
                        { "author": "bo", "body": "and a date" },
                    ],
                    "updated_at": "2026-08-21T10:00:00Z",
                }],
                // No `amended_upstream` here: the engine decorates the open
                // list only, so the trim has to default it.
                "declined_proposals": [{
                    "number": 4,
                    "url": "https://example.invalid/pull/4",
                    "branch": "crystalline/kb-4",
                    "title": "Superseded",
                    "status": "Declined",
                    "review_state": null,
                    "files": [],
                    "feedback": [{ "author": "cy", "body": "not this one" }],
                    "updated_at": "2026-08-20T09:00:00Z",
                }],
            }],
        }));

        let domain = &leaned["domains"][0];
        assert_eq!(domain["domain"], "kb", "the domain's own fields survive");
        assert_eq!(domain["repo"], "team/knowledge");

        let open = &domain["open_proposals"][0];
        let declined = &domain["declined_proposals"][0];
        for (label, entry) in [("open", open), ("declined", declined)] {
            let object = entry.as_object().unwrap_or_else(|| panic!("{label}"));
            let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
            keys.sort_unstable();
            let mut expected = LEAN_KEYS.to_vec();
            expected.sort_unstable();
            assert_eq!(keys, expected, "{label} carries exactly the lean keys");
            assert!(
                entry.get("feedback").is_none(),
                "{label} must not carry comment bodies: {entry}"
            );
            // The other fat fields go too, for the same reason.
            assert!(entry.get("files").is_none(), "{label}: {entry}");
            assert!(entry.get("branch").is_none(), "{label}: {entry}");
        }

        assert_eq!(open["number"], json!(7));
        assert_eq!(open["title"], "Refine alpha");
        assert_eq!(open["review_state"], "changes_requested");
        assert_eq!(open["feedback_count"], json!(2));
        assert_eq!(open["amended_upstream"], json!(true));
        assert_eq!(open["updated_at"], "2026-08-21T10:00:00Z");

        assert_eq!(declined["number"], json!(4));
        assert_eq!(declined["status"], "Declined");
        assert_eq!(declined["review_state"], Value::Null);
        assert_eq!(declined["feedback_count"], json!(1));
        assert_eq!(
            declined["amended_upstream"],
            json!(false),
            "a declined proposal the engine never decorated defaults to false"
        );

        // The chain position is the open list's own order, 1-based from the
        // bottom, so a caller reads "which layer is this" without a second
        // call. A declined proposal stands in no chain, so it carries the key
        // (one shape for both arrays) with nothing in it.
        assert_eq!(open["position"], json!(1));
        assert_eq!(declined["position"], Value::Null);

        // A domain with nothing stacked says nothing about stacks: the four
        // keys the engine always emits are dropped when they are quiet, which
        // is where this trim differs from `origin::status_report_json` on
        // purpose.
        let domain = domain.as_object().unwrap();
        for key in [
            "stack_number",
            "stack_wedged",
            "repair_pending",
            "stack_link_pending",
        ] {
            assert!(
                !domain.contains_key(key),
                "{key} is dropped while it is quiet: {domain:?}"
            );
        }
    }

    /// The other half of the stack trim: a domain that really is stacked
    /// keeps every key that carries a fact, and every open layer numbers
    /// itself bottom-up.
    ///
    /// `stack_number` null beside a real position is the degraded chain, not
    /// an unstacked domain - the layers exist and are simply not grouped yet -
    /// so it drops out here while `stack_link_pending` stays to carry the
    /// debt. A reader keys off `position`, never off a stack number it may
    /// not have.
    #[test]
    fn lean_origin_status_keeps_the_stack_keys_that_carry_a_fact() {
        let leaned = lean_origin_status(json!({
            "domains": [{
                "domain": "kb",
                "stack_number": 42,
                "stack_wedged": [4],
                "repair_pending": true,
                "stack_link_pending": false,
                "open_proposals": [
                    { "number": 7, "feedback": [], "status": "Open" },
                    { "number": 8, "feedback": [], "status": "Open" },
                ],
            }, {
                "domain": "degraded",
                "stack_number": Value::Null,
                "stack_wedged": [],
                "repair_pending": false,
                "stack_link_pending": true,
                "open_proposals": [{ "number": 11, "feedback": [], "status": "Open" }],
            }],
        }));

        let stacked = &leaned["domains"][0];
        assert_eq!(stacked["stack_number"], json!(42));
        assert_eq!(stacked["stack_wedged"], json!([4]));
        assert_eq!(stacked["repair_pending"], json!(true));
        assert!(
            stacked
                .as_object()
                .unwrap()
                .get("stack_link_pending")
                .is_none(),
            "a paid link says nothing: {stacked}"
        );
        assert_eq!(stacked["open_proposals"][0]["position"], json!(1));
        assert_eq!(stacked["open_proposals"][1]["position"], json!(2));

        let degraded = &leaned["domains"][1];
        assert!(
            degraded.as_object().unwrap().get("stack_number").is_none(),
            "an unlinked chain names no stack number: {degraded}"
        );
        assert_eq!(degraded["stack_link_pending"], json!(true));
        assert_eq!(degraded["open_proposals"][0]["position"], json!(1));
    }

    /// A payload with no `domains` array, and one whose entries carry no
    /// proposal arrays, pass through untouched rather than gaining empty keys.
    #[test]
    fn lean_origin_status_leaves_a_payload_with_nothing_to_trim_alone() {
        let bare = json!({ "domains": [] });
        assert_eq!(lean_origin_status(bare.clone()), bare);

        let no_arrays = json!({ "domains": [{ "domain": "kb", "conflicts": [] }] });
        assert_eq!(lean_origin_status(no_arrays.clone()), no_arrays);

        let not_a_report = json!({ "error": "offline" });
        assert_eq!(lean_origin_status(not_a_report.clone()), not_a_report);
    }

    /// The listing gate's full matrix, both inputs. `github.enabled` off hides
    /// the five whatever the mode is and never hides `configure`; on top of
    /// that read-only hides the write set, so an enabled read-only instance
    /// shows the two collaboration tools it still exempts and nothing else.
    #[test]
    fn hidden_collab_tool_matches_the_locked_matrix() {
        // github off: the five are hidden whatever the mode is.
        for read_only in [false, true] {
            for name in COLLAB_TOOLS.iter().filter(|n| **n != "configure") {
                assert!(hidden_collab_tool(name, read_only, false), "{name}");
            }
        }
        assert!(
            !hidden_collab_tool("configure", false, false),
            "a writable default install still lists the enable path"
        );
        assert!(
            hidden_collab_tool("configure", true, false),
            "read-only hides configure on its own gate, unchanged"
        );

        // github on, writable: all six.
        for name in COLLAB_TOOLS {
            assert!(!hidden_collab_tool(name, false, true), "{name}");
        }

        // github on, read-only: the two exempt reads only.
        for name in ["update_domain", "origin_status"] {
            assert!(!hidden_collab_tool(name, true, true), "{name}");
        }
        for name in COLLAB_WRITE_TOOLS {
            assert!(hidden_collab_tool(name, true, true), "{name}");
        }
    }

    /// The listing and the refusal cannot disagree about which tools the
    /// setting governs: whatever the github gate withholds is exactly what
    /// refuses when a stale client calls it anyway.
    #[test]
    fn the_github_listing_gate_and_the_refusal_name_the_same_tools() {
        for name in COLLAB_TOOLS {
            assert_eq!(
                hidden_collab_tool(name, false, false),
                refused_collab_tool(name, false),
                "{name}"
            );
        }
    }

    /// The call-time half: `github.enabled` off refuses everything but
    /// `configure`, which is exempt because it is how the setting is turned
    /// on. On it, nothing is refused.
    #[test]
    fn refused_collab_tool_matches_the_locked_github_matrix() {
        assert!(!refused_collab_tool("configure", false));
        for name in [
            "share_changes",
            "update_domain",
            "origin_status",
            "resolve_conflict",
            "withdraw_proposal",
        ] {
            assert!(refused_collab_tool(name, false), "{name}");
        }
        for name in COLLAB_TOOLS {
            assert!(!refused_collab_tool(name, true), "{name}");
        }
    }

    /// `provision` splits the same way: read-only hides it, and an
    /// undeclared instance refuses the three actions that would otherwise
    /// report a success that reconciled nothing.
    #[test]
    fn provision_gating_splits_between_the_listing_and_the_call() {
        assert!(hidden_provision_tool(true));
        assert!(!hidden_provision_tool(false));

        let status = ProvisionAction::Status;
        let apply = ProvisionAction::Apply;
        let allow = ProvisionAction::Allow {
            domain: "eng".to_string(),
        };
        let deny = ProvisionAction::Deny {
            domain: "eng".to_string(),
        };
        assert!(
            !refused_provision_action(&status, false),
            "an empty report is the answer, not a refusal"
        );
        for action in [&apply, &allow, &deny] {
            assert!(refused_provision_action(action, false), "{action:?}");
        }
        for action in [&status, &apply, &allow, &deny] {
            assert!(!refused_provision_action(action, true), "{action:?}");
        }
    }

    /// The refusal a caller reads has to name what to change; a listed tool
    /// that fails opaquely is worse than a hidden one.
    #[test]
    fn the_call_time_refusals_name_what_to_change() {
        let github = RemoteError::NotEnabled.to_string();
        assert!(github.contains("github.enabled"), "{github}");
        assert!(github.contains("configure"), "{github}");
        assert!(
            PROVISION_NOT_DECLARED.contains("## Provisioning"),
            "{PROVISION_NOT_DECLARED}"
        );
        assert!(
            PROVISION_NOT_DECLARED.contains("status"),
            "{PROVISION_NOT_DECLARED}"
        );
    }
}
