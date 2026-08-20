//! The rmcp tool router: the core tools of the v1 MCP surface plus the
//! collaboration tools, which are always listed and refuse while team
//! collaboration is off.
//!
//! # One list for every client (SEP-2567)
//!
//! MCP 2026-07-28 says a server's `tools/list` result "MAY change over time
//! [...] but MUST NOT vary per-connection or as a side effect of other
//! requests on the connection", and the same sentence governs
//! `resources/list` and `prompts/list`. The rule this file applies, which
//! covers both halves of that sentence:
//!
//! > A gate may stay on the listing if and only if (a) its input is not
//! > derived from the identity, capabilities or configuration of the
//! > connecting client, and (b) its input cannot be changed by any request on
//! > that connection. Anything failing either half refuses at call time.
//!
//! `read_only` passes both: it is a construction field on the engine
//! (`Engine::with_read_only` takes `self` by value) and the engine is shared
//! behind an `Arc`, so nothing a client sends can move it. `github.enabled`
//! and whether any domain declares provisioning both fail (b) - `configure`,
//! `add_domain` and `update_domain` change them on the very connection that
//! then lists - so they refuse at call time. The client's install-receipt
//! match failed (a) and is gone from the listing entirely.
//!
//! Refusing rather than hiding is SEP-2567's own prescription: expose the tool
//! unconditionally and put the dependency "in the tool's input schema and
//! description rather than in the list result". Half of it was already true
//! here, since every gate hid a tool without unregistering its route, so what
//! changed is the listing rather than the guarding.
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
//! The collaboration tools (`configure`, `add_domain`, `share_changes`,
//! `update_domain`, `origin_status`, `resolve_conflict`) split their two
//! gates across the two sides of the rule above.
//! `configure`/`add_domain`/`share_changes`/`resolve_conflict` disappear
//! read-only, which stays on the listing. `github.enabled` is needed by every
//! collaboration tool but `configure`, and it refuses at call time: the four
//! tools that need it are listed whatever it says and answer with
//! `RemoteError::NotEnabled`'s message, which names the setting and both ways
//! to change it. Their descriptions say the same thing, so a client's tool
//! search reads the dependency without having to call. See `COLLAB_TOOLS`,
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
//! # Nothing is pushed, because nothing can change
//!
//! This server sends no `notifications/*/list_changed` at all. `configure`
//! used to push one whenever it flipped `github.enabled`, and `add_domain` and
//! `update_domain` whenever they flipped whether any domain declares
//! provisioning; after those gates moved to call-time refusals and
//! `skills.serve` was snapshotted at engine construction, every input to every
//! list is fixed before the first request arrives, so each of those pushes
//! announced a change that had not happened. MCP 2026-07-28 removes the
//! unsolicited channel outright - a notification either rides a
//! `subscriptions/listen` stream the client opened or it does not exist - so
//! [`McpServer::accepted_subscription_filter`] and [`McpServer::listen`] serve
//! that stream, and its doc comment carries what a future dynamic list would
//! have to do to announce itself.
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
//! `add_domain` through team mode, `share_changes`, `update_domain` and
//! `origin_status`.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::handler::server::prompt::PromptContext;
use rmcp::handler::server::tool::InputResponses;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CacheScope, CallToolResponse, CallToolResult, ContentBlock, DiscoverResult, ElicitRequest,
    ElicitRequestParams, ElicitationSchema, ErrorData, GetPromptRequestParams, GetPromptResponse,
    Implementation, InitializeRequestParams, InitializeResult, InputRequest, InputRequests,
    InputRequiredResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, PromptMessage,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate, Role, ServerCapabilities, ServerInfo, SubscriptionFilter,
    Tool,
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
    let mut requests = InputRequests::new();
    requests.insert(
        CONFIRM_KEY.to_string(),
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

/// The five GitHub collaboration tools, gated on the engine's live
/// `github.enabled` setting (all but `configure`) and `read_only` flag (see
/// `COLLAB_WRITE_TOOLS`). `add_domain` is not among them: it creates domains of
/// every kind, so it is a write-gated tool (see `WRITE_TOOLS`), and only its
/// team-domain branch needs `github.enabled`, enforced in the engine.
const COLLAB_TOOLS: [&str; 5] = [
    "configure",
    "share_changes",
    "update_domain",
    "origin_status",
    "resolve_conflict",
];

/// Of the five collaboration tools, the three also hidden in read-only mode:
/// `configure` (settings and this machine's GitHub identity are frozen the
/// same way content is), `share_changes` and `resolve_conflict` (each writes a
/// proposal or config). `update_domain` and `origin_status` stay visible
/// read-only, mirroring their engine-level exemption (a pull is a derived-truth
/// update like sync; status is a pure read).
const COLLAB_WRITE_TOOLS: [&str; 3] = ["configure", "share_changes", "resolve_conflict"];

/// Appended to the initialize instructions while TOON responses are active,
/// so a client model reads list results as structured data rather than prose.
const TOON_INSTRUCTIONS_NOTE: &str = "\n\nList-shaped tool results (search hits, activity, listings and status reports) arrive TOON-encoded rather than as JSON: indentation nests objects, `name[N]{field1,field2}:` heads a uniform array with one comma-separated row per record and a tags cell joins its values with commas. Read them as data with exactly those fields.";

/// Whether `name` is one of the five collaboration tools.
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

/// Whether collaboration tool `name` is hidden given the engine's `read_only`
/// state. Not meaningful for a non-collab tool name; callers check
/// [`is_write_tool`] separately for those. The net matrix: read-write shows
/// all five, read-only shows `update_domain` and `origin_status` only.
///
/// `github.enabled` used to be the other half of this predicate. It left the
/// listing for [`refused_collab_tool`]: `configure` can flip it on this very
/// connection, and SEP-2567 forbids a list varying "as a side effect of other
/// requests on the connection". `read_only` stays because it cannot move -
/// `Engine::with_read_only` (`engine.rs:788-791`) takes `self` by value at
/// construction and the engine is shared behind an `Arc`, so no request can
/// reach it.
fn hidden_collab_tool(name: &str, read_only: bool) -> bool {
    read_only && COLLAB_WRITE_TOOLS.contains(&name)
}

/// Whether collaboration tool `name` refuses at call time because
/// `github.enabled` is off. Every collaboration tool but `configure` needs it,
/// and `configure` is deliberately exempt: it is how the setting gets turned
/// on.
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
        description = "Capture a new engram - a unit of knowledge - into a domain. Writes the markdown file and indexes it. Body bullets: '- [decision] we chose X #tag' become observations, '- rel_type [[Target]]' become relations. domain is required so an engram never lands in the wrong place. Pass folder to file the engram under a topic prefix: reuse the domain's existing layout (browse_domain shows it), start a subfolder when a topic cluster is forming and keep singletons at the root; the folder path becomes the permalink prefix build_context globs as crystalline://domain/folder/*. permalink, status, recorded_at and generated (who wrote it and when) are filled in; valid_from/valid_to are never auto-set - absence means always valid; to bound validity pass them inside metadata as plain ISO dates (YYYY-MM-DD). Any other date format is rejected; a sentinel far-future valid_to and an explicit null are dropped, since absence already means valid forever. Recommended type values: engram, guide, decision, architecture, runbook, reference. Recommended status values (guidance, not enforced): stable, implemented, draft, proposed, idea, poc, deprecated, superseded, archived, legacy. stable is the default and the word for knowledge that holds now; current is the legacy alias for the same state, and a status filter on either word matches engrams carrying either. Of those, deprecated, superseded, archived and legacy are the recognized retirement set: a status inside it softly fades in search ranking, any other value ranks at full strength. Errors if the permalink exists unless overwrite is true, and refuses a title that would file the engram as the reserved index.md or log.md (Crystalline generates the folder index itself). The vocabulary tool lists tags already in use; reuse one before coining a new tag. Set an optional numeric salience metadata key (0-10) to mark exceptionally valuable knowledge; salient engrams are lifted in hybrid search ranking. Raise it later to elevate an engram that proved load-bearing.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .write_engram_as(&p, client_actor(&ctx).as_deref())
            .await
            .map_err(to_error)
            .and_then(ok)
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
        // failed.
        if confirmation_supported(&ctx)
            && let Ok(Some(intent)) = Engine::ack_intent(&p)
        {
            match confirmed(&responses.0) {
                None => return Ok(confirm_question(ack_question(&p, &intent)).into()),
                Some(false) => return refuse(ack_refusal(&intent)).map(CallToolResponse::from),
                Some(true) => {}
            }
        }
        self.engine
            .edit_engram_as(&p, client_actor(&ctx).as_deref())
            .await
            .map_err(to_error)
            .and_then(ok)
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
            .and_then(ok)
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
        description = "Search across every registered domain by default (an all-domain sweep) or a chosen few to recall relevant knowledge and experience. Defaults to hybrid lexical-plus-semantic ranking and falls back to plain text when embeddings are not ready. Filter by type, tags, status, arbitrary frontmatter or a recorded-after date; a filter-only search with no query text is allowed. Every hit is labelled with its domain, and a hit inside an observation carries its line. A hit's snippet is a short window around the match, never the whole engram: read_engram returns the full content, so read before citing or summarizing what a hit only previews. The result reports total, page, limit and count; when count is below total, request the next page to see the rest. A tags filter also matches through a domain's tag aliases (the MANIFEST `## Tag Aliases` section), so a merged old tag name still finds its engrams. A status filter on stable or current matches both, since they are one state under two spellings; any other status matches exactly. Hybrid ranking adds a small salience prior, so an engram marked salient at write time ranks above equally relevant unmarked ones without ever excluding a result. Engrams whose status is deprecated, superseded, archived or legacy are softly faded in ranking (the search.retired_weight setting, default 0.6, 1.0 disables), reordered but never excluded.",
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
            .and_then(|v| self.ok_list(v))
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
        description = "List the vocabulary in use: tags with engram and observation usage counts, observation categories with counts and relation types with counts, for one domain or across all domains. Check it before inventing a new tag or category so existing terms are reused instead of multiplied. Near-duplicate tag clusters are reported so they can be merged. Tag aliases recorded in a MANIFEST are listed too and clusters an alias already explains are not reported.",
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

        // No list-changed notification follows, and none is owed: nothing this
        // call can write moves a list. `github.enabled` refuses at call time
        // instead of shaping the listing, and `skills.serve` is frozen at
        // engine construction ([`Engine::skills_serve`]). The unsolicited push
        // that used to fire here announced a change that had not happened, and
        // from MCP 2026-07-28 an unsolicited notification has no channel at all
        // (see [`McpServer::listen`]).
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
        description = "Share this domain's new knowledge and experience with the team as a proposal they review on GitHub; returns the review URL to hand to the user. Refuses while conflicts are unsettled so the team always reviews a clean proposal. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
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
    ) -> Result<CallToolResult, ErrorData> {
        if refused_collab_tool("share_changes", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        self.engine
            .origin_share(&p.domain, p.title.as_deref(), p.description.as_deref())
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "update_domain",
        title = "Update domain",
        description = "Learn the team's latest knowledge: pulls what was merged upstream into the domain (or every shared domain), merging cleanly where possible and flagging real conflicts for resolve_conflict. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
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
        description = "Review each shared domain's standing: whether the team has new knowledge to learn, what is waiting to be shared, open and declined proposals and any conflicts to settle. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
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
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "resolve_conflict",
        title = "Resolve conflict",
        description = "Settle a flagged conflict by keeping your version (mine), taking the team's version (theirs) or providing merged content. The engram then counts as ordinary local knowledge you can share. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
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
    ) -> Result<CallToolResult, ErrorData> {
        if refused_collab_tool("resolve_conflict", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        let (keep, content): (Option<&str>, Option<&[u8]>) = match p.resolution.as_str() {
            "mine" => (Some("mine"), None),
            "theirs" => (Some("theirs"), None),
            "merged" => {
                let Some(content) = p.content.as_deref() else {
                    return Err(ErrorData::invalid_params(
                        "resolve_conflict requires content when resolution is merged".to_string(),
                        None,
                    ));
                };
                (None, Some(content.as_bytes()))
            }
            other => {
                return Err(ErrorData::invalid_params(
                    format!(
                        "resolve_conflict resolution must be mine, theirs or merged, got '{other}'"
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

    /// Hold one acknowledged subscription open until the client ends it.
    ///
    /// # This stream is silent, and that is the finding rather than a shortcut
    ///
    /// Nothing this server can be asked to do moves any of the three lists.
    /// [`McpServer::list_tools`] reads `Engine::read_only`,
    /// `Engine::skills_serve` and `harness_onboarded`; `list_resources` and
    /// `list_prompts` read the last two. All three are fixed before the first
    /// request arrives - read-only at engine construction, `skills.serve`
    /// snapshotted there too, the harness answer resolved by the spawned
    /// process before the session started - so no request on any transport can
    /// change what a later `tools/list`, `resources/list` or `prompts/list`
    /// returns. There is therefore no truthful `notifications/*/list_changed`
    /// to send, and the three unsolicited pushes that used to fire from
    /// `configure`, `add_domain` and `update_domain` were deleted rather than
    /// routed here: after the gates they keyed on moved to call-time refusals,
    /// each announced a change that had not happened.
    ///
    /// **So no sink is registered anywhere, deliberately.** The plan for this
    /// work called for a registry of cloned `SubscriptionSink`s on the shared
    /// `Arc<Engine>`; a registry nothing writes to is worse than none, because
    /// it reads as implemented. What the registry would have needed is recorded
    /// here instead, since the next person to add a genuinely dynamic list has
    /// to get it right: the sink cannot live on this handler, because on the
    /// stateless HTTP path rmcp builds a fresh service per request
    /// (`get_service()` at rmcp 3.1.2 `tower.rs:1822` and `:1948`) and every
    /// modern peer routes statelessly, so the handler that would push and the
    /// handler that took the subscription are different objects sharing only
    /// the engine. (The legacy session path builds one service per session,
    /// `tower.rs:1855`, and stdio one per connection, so a handler-local
    /// registry would have worked there and nowhere else.) `SubscriptionSink`
    /// is `Clone` and every field is `Send + Sync + 'static`
    /// (`service/server.rs:139-144`), so `Arc<Engine>` can hold one; it also
    /// holds a `Peer` and a child cancellation token, so it must be removed
    /// when this method returns or the engine accumulates dead peers.
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
        tracing::debug!(
            accepted = ?context.accepted(),
            "subscription opened; this server has no list that can change, so the stream stays silent"
        );
        context.cancelled().await;
        Ok(())
    }

    /// List the exposed tools.
    ///
    /// # Only inputs no request can move may gate this list
    ///
    /// MCP 2026-07-28 (SEP-2567, `/server/tools`) says a server's tool list
    /// "MAY change over time [...] but MUST NOT vary per-connection or as a
    /// side effect of other requests on the connection", and the identical
    /// sentence governs `resources/list` and `prompts/list`. So a gate may
    /// stay here only if its input is neither derived from the connecting
    /// client nor changeable by a request on that connection.
    ///
    /// What is left is `read_only`, which is fixed at engine construction
    /// (`Engine::with_read_only`, `engine.rs:788-791`, takes `self` by value;
    /// the engine is shared behind an `Arc`), plus the `skills.serve` setting,
    /// snapshotted at the same point (`Engine::skills_serve`), and the harness
    /// answer the spawned process resolved before the session started - see
    /// [`hidden_skills_surface`]. All three are fixed before the first request,
    /// which is why this list can never move and why [`McpServer::listen`] has
    /// nothing to announce.
    ///
    /// `github.enabled` and whether any domain declares provisioning both left
    /// this list for a call-time refusal, which is the remedy SEP-2567
    /// prescribes itself: expose the tool unconditionally and put the
    /// dependency "in the tool's input schema and description rather than in
    /// the list result". The routes were always registered - the gates hid
    /// rather than disabled - so what changed is the listing, not the
    /// guarding.
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
        let skills_hidden =
            hidden_skills_surface(self.engine.skills_serve(), self.harness_onboarded);
        let mut tools = Self::tool_router().list_all();
        tools.retain(|t| {
            if is_write_tool(&t.name) && read_only {
                return false;
            }
            if is_collab_tool(&t.name) && hidden_collab_tool(&t.name, read_only) {
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
        if is_collab_tool(name) && hidden_collab_tool(name, read_only) {
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
    let listed = preview["attachments"]
        .as_array()
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let attachments = if listed.is_empty() { "none" } else { &listed };
    format!(
        "Delete '{title}' ({domain}/{permalink})? This leaves its sole-referent attachments orphaned: {attachments}. This cannot be undone."
    )
}

/// The sentence an `evolve_ack` assignment asks before it acts.
///
/// Both halves name the consequence rather than the write, because that is
/// what the user is deciding: a record keeps a finding out of every future
/// sweep, a removal puts it back into the next one. The engram is named by the
/// identifier the call carried, which is what the user would recognize, and
/// resolving it to a permalink would cost a lookup to say the same thing.
fn ack_question(p: &EditParams, intent: &AckIntent) -> String {
    let identifier = p.identifier.trim();
    let domain = p.domain.trim();
    match intent {
        AckIntent::Record { rule, note } => {
            let note = note
                .as_deref()
                .map(|note| format!(" The note reads: '{note}'."))
                .unwrap_or_default();
            format!(
                "Acknowledge {rule} on '{identifier}' in '{domain}'? This records the finding as intentional until its evidence changes.{note}"
            )
        }
        AckIntent::Remove { rule } => format!(
            "Remove the {rule} acknowledgment on '{identifier}' in '{domain}'? The finding resurfaces on the next sweep."
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
/// no domain, unresolved conflicts blocking a share, or a proposal or
/// conflict path that does not exist - stay `invalid_params`-shaped. This
/// match is exhaustive over `RemoteError` so a new variant must be
/// classified here rather than silently defaulting.
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
    fn is_collab_tool_recognizes_exactly_the_five() {
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

    /// The listing half of the collaboration gating, which is `read_only`
    /// alone now: read-write shows all five, read-only shows the two that a
    /// read-only instance still exempts.
    #[test]
    fn hidden_collab_tool_matches_the_locked_read_only_matrix() {
        for name in COLLAB_TOOLS {
            assert!(!hidden_collab_tool(name, false), "{name}");
        }
        for name in ["update_domain", "origin_status"] {
            assert!(!hidden_collab_tool(name, true), "{name}");
        }
        for name in COLLAB_WRITE_TOOLS {
            assert!(hidden_collab_tool(name, true), "{name}");
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
