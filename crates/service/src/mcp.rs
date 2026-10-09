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
//! **Over HTTP that block names no domain.** `get_info` is synchronous and rmcp
//! calls it with no request context, so the legacy handshake cannot know who is
//! connecting and cannot leave a private domain out of a per-caller block;
//! there `initialize` renders [`crate::engine::Engine::routing_text_counted`]
//! instead - every behavior rule, the count of the domains that are not
//! private, and the pointer at `list_domains`, which does resolve a caller and
//! does filter. Stdio keeps the
//! whole block, because a local session is the machine owner. Every channel
//! that *does* carry a request context is scoped per caller instead:
//! `server/discover` through [`McpServer::arrival_info_scoped`], the
//! `onboarding` prompt, and `list_domains` itself.
//!
//! In read-only mode (the engine's `read_only` flag) the write-gated tools are
//! filtered out of `list_tools` and `get_tool`; the routes stay registered so
//! a client that calls a hidden tool by name reaches the engine's read-only
//! guard and gets a clean error. That gate is legitimate on the listing
//! because the mode is fixed for the engine's lifetime.
//!
//! The seven collaboration tools (`configure`, `share_changes`,
//! `update_domain`, `origin_status`, `resolve_conflict`, `withdraw_proposal`,
//! `discard_changes`) carry two gates that compose. `configure`,
//! `share_changes`, `resolve_conflict`, `withdraw_proposal` and
//! `discard_changes` disappear read-only. `add_domain` is deliberately not
//! one of the seven: it creates domains of every kind, so it is write-gated
//! like any other writer (see `WRITE_TOOLS`) and only its team-domain branch
//! needs `github.enabled`, enforced in the engine rather than on the listing.
//! `github.enabled` is needed by every collaboration tool but `configure`, and
//! while it is off the six that need it are hidden from
//! the listing too, so a default install spends no context on a forge surface
//! nobody connected; `configure` is never hidden by it, since it is the only
//! way to turn the rest on. Calling a hidden one still answers with
//! `RemoteError::NotEnabled`'s message, which names the setting and both ways
//! to change it, so a stale cached list teaches rather than dead-ends. Turning
//! the setting on makes all six appear on the next list and announces the
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
//! need the client to know to ask. `tests/mcp/mcp_instructions.rs` drives the
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
//! `configure` flipping `github.enabled` moves the tool list, because the six
//! GitHub-gated collaboration tools are listed only while it is on. That is
//! the single mover on this server: `resources/list` and `prompts/list` read
//! `skills.serve` and the harness gate, both fixed before the first request
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
//! Two tools put a question to the person before they act, whenever the peer
//! can carry one: `remove_domain` and `discard_changes`. Each returns an
//! `input_required` result holding a single form elicitation, the client puts
//! it to its user, and the same call arrives again with the answer beside the
//! original arguments. [`confirmation_supported`] decides whether to ask,
//! [`confirm_question`] builds the question and [`confirmed`] reads the
//! answer; all three are deliberately tool-agnostic. The question's one
//! checkbox is checked by default, so Accept alone goes ahead, and an accept
//! that leaves the box out of its content is a yes.
//!
//! **Every other tool acts on the first call.** The agent's call is the
//! go-ahead: a delete, an acknowledgment, a share, a withdrawal and a conflict
//! resolution run as called for every peer, and the questions they used to
//! put are gone. A permalink collision is the engine's own error, which names
//! the `overwrite` argument, and an overwrite of a document somebody has open
//! is refused for every peer, naming who is in there.
//!
//! **A question is only put about a call that can run.** Both rounds resolve
//! their target before they ask - `remove_domain` through
//! [`crate::engine::Engine::domain_remove_preview`] and `discard_changes`
//! through the domain's change listing - so a read-only server, a domain
//! nobody registered and a path that is not an unshared change each fail in
//! round one, and the question names what resolution found rather than what
//! was typed.
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
//! user; either one is served one call and one removal, because a
//! confirmation nobody can answer is a hang. Nothing here is a permission
//! system: a client is free to answer its own question, and the CLI's own
//! dispatch (`crate::client::dispatch_engine`) never asks at all, because the
//! human already typed the verb.
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
    ElicitRequestParams, ElicitationSchema, ErrorData, GetPromptRequestParams, GetPromptResponse,
    Implementation, InitializeRequestParams, InitializeResult, InputRequest, InputRequests,
    InputRequiredResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, PromptMessage,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate, Role, ServerCapabilities, ServerConfig, SubscriptionFilter,
    Tool,
};
use rmcp::service::{RequestContext, SubscriptionContext};
use rmcp::{RoleServer, ServerHandler, prompt, prompt_router, tool, tool_handler, tool_router};
use serde_json::{Value, json};

use crystalline_core::{CrystallineUrl, SKILL_ASSETS};
use crystalline_remote::RemoteError;

/// The tools hidden in read-only mode: the five content-mutating engram tools
/// plus `add_domain` and `remove_domain`, which create and unregister domains
/// (writing config, and files for a local domain). In read-only mode they are
/// hidden from `list_tools` and `get_tool`, while their routes stay registered
/// so a client that calls one by name still reaches the engine guard and gets
/// the read-only error rather than a bare "tool not found".
const WRITE_TOOLS: [&str; 7] = [
    "write_engram",
    "edit_engram",
    "move_engram",
    "split_engram",
    "delete_engram",
    "add_domain",
    "remove_domain",
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
/// `tests/mcp/mcp_instructions.rs`, which also pins rmcp's own list, so a crate that
/// learns a new revision fails the build and asks for the decision instead of
/// taking it.
///
/// **The top is a decision, and it was taken on 2026-08-14.** `V_2026_07_28`
/// is served. That revision makes list endpoints connection-invariant, moves
/// `instructions` to `server/discover`, restricts `tools/list_changed` to
/// subscribers and requires caching hints on six operations; all four are
/// implemented here - see [`McpServer::list_tools`], [`McpServer::discover`],
/// [`McpServer::listen`] and [`CacheHinted`]. The stdio path has no probe
/// handling of its own - every client line is forwarded verbatim
/// (`crate::client`) and rmcp classifies and answers it. A fifth obligation,
/// `ping`'s removal, is rmcp's: it answers `method_not_found` to any peer that
/// is not on the legacy lifecycle (`handler/server.rs:112-118`), and we
/// implement no `ping`. `tests/mcp/mcp_modern_era.rs` is what a client at this
/// revision actually receives, over both transports.
///
/// **The bottom is deliberately NOT a decision.** `V_2024_11_05` is served today
/// and stays served: rmcp branches nowhere between it and `V_2025_11_25`
/// (`uses_legacy_lifecycle`, rmcp 3.4.0 `service.rs:204-215`, one `<` comparison
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

/// The newest revision we serve. Reads the last element, so the ordering of
/// [`SERVED_PROTOCOL_VERSIONS`] is load bearing rather than cosmetic.
///
/// **Not the downgrade target.** A client asking over stdio for a revision we
/// do not serve is answered [`newest_legacy_handshake_version`], which is a
/// different value and for a reason spelled out there.
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
/// `:246-260`, `uses_legacy_lifecycle` at `service.rs:210-215`), so a client
/// downgraded onto the era would lose `ping` without ever having asked for the
/// era.
///
/// **Since rmcp 3.2.0 this cap is upstream's rule too, and it binds every
/// handshake rather than only an unserved one.** `negotiate_protocol_version`
/// (`service/server.rs:479`) echoes a requested revision only when it is a
/// legacy one the server supports, and otherwise answers with the newest
/// legacy revision - so an `initialize` naming 2026-07-28 is answered this
/// value however the handler replies. A peer therefore reaches the modern
/// lifecycle only the way the specification provides for: by opening with
/// `server/discover`, or by carrying the SEP-2575 `_meta` on an inline
/// request. Naming the era in a handshake is not one of the routes.
pub fn newest_legacy_handshake_version() -> ProtocolVersion {
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
/// **protocol** list this server answers - `tools/list`, `prompts/list`,
/// `resources/list`, `resources/templates/list`, the five results this constant
/// governs - is decided before the first request from deployment configuration
/// and machine state, never from who is asking, and none of it varies by the
/// authorization presented on the request, which is the one variation SEP-2567
/// explicitly permits and the one that would force `private`. The shipped
/// skills a `resources/read` returns are static copy compiled into this binary.
///
/// It is a claim about those results and not about the server: a *tool* result
/// may vary by caller and several now do (`list_domains` answers each account
/// its own index). Tool results carry no caching hints at all, so nothing about
/// them is promised here.
///
/// **One result on this server is not that, and it says so itself.** An
/// attachment read through the same `resources/read` endpoint *does* vary by
/// the authorization on the request: a file inside a private domain is served
/// to its members and refused to everybody else. A shared cache holding that
/// answer under a public scope would hand one caller's attachment to the next
/// one, so that branch passes [`CacheScope::Private`] to
/// [`CacheHinted::with_cache_hints_as`] instead of taking this default.
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

/// The three resolutions `resolve_conflict` accepts, spelled once, exactly as
/// that tool's own `resolution` parameter.
const RESOLUTION_MINE: &str = "mine";
const RESOLUTION_THEIRS: &str = "theirs";
const RESOLUTION_MERGED: &str = "merged";

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
/// has a schema to render and an unambiguous shape to send back. The box is
/// checked by default (`default: true`, which rmcp's `BooleanSchema` carries),
/// so a client that honours defaults shows it ticked and Accept alone goes
/// ahead; its title and description say so in the words a person reads. The
/// buttons themselves (Accept, Decline) are the client's to draw and name.
///
/// Nothing is sealed into `requestState` and none is asked for: the flows that
/// use this are stateless by construction - the client echoes the original
/// arguments on the retry and its answer is the whole of the state - which is
/// why the `request-state` feature is not enabled on rmcp.
fn confirm_question(message: String) -> InputRequiredResult {
    let requested_schema = ElicitationSchema::builder()
        .required_bool_property(CONFIRM_KEY, |schema| {
            schema
                .title("Yes, go ahead")
                .description("Leave this checked and choose Accept to go ahead.")
                .with_default(true)
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
/// `Some(true)` for an accept that does not say no: its content carries `true`
/// under [`CONFIRM_KEY`], or it does not carry the property at all. The box is
/// checked by default, so a client that sends back only what the person
/// changed sends an accept with no property, and that is a yes. `Some(false)`
/// for everything else that is an answer: an accept whose content carries the
/// property with anything but `true` (`false`, `null`, a string), a decline, a
/// cancel and any action that is not `accept`. `None` - the first round - when
/// the call carries no responses at all or none under [`CONFIRM_KEY`].
///
/// The value is read as plain JSON rather than deserialized into
/// `ElicitResult`, and the difference is the failure mode: `ElicitationAction`
/// is a closed three-variant enum, so a client answering with an action a
/// later revision adds would fail to deserialize and turn a "no" into an
/// error. Read this way, anything that is not exactly `accept` is a no.
fn confirmed(responses: &Option<rmcp::model::InputResponses>) -> Option<bool> {
    let answer = responses.as_ref()?.get(CONFIRM_KEY)?;
    if answer["action"] != json!("accept") {
        return Some(false);
    }
    Some(match answer["content"].get(CONFIRM_KEY) {
        None => true,
        Some(value) => *value == json!(true),
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
    /// Set both hints, or neither, at the default [`CACHE_SCOPE`].
    fn with_cache_hints(self, context: &RequestContext<RoleServer>) -> Self {
        self.with_cache_hints_as(context, CACHE_SCOPE)
    }

    /// Set both hints, or neither, at a scope this result chose for itself.
    /// The one caller that does is the attachment branch of
    /// [`McpServer::read_resource`], whose answer varies by who asked.
    fn with_cache_hints_as(self, context: &RequestContext<RoleServer>, scope: CacheScope) -> Self;
}

macro_rules! impl_cache_hinted {
    ($($t:ty),+ $(,)?) => {
        $(impl CacheHinted for $t {
            fn with_cache_hints_as(
                self,
                context: &RequestContext<RoleServer>,
                scope: CacheScope,
            ) -> Self {
                if peer_gets_cache_hints(context) {
                    self.with_ttl_ms(CACHE_TTL_MS).with_cache_scope(scope)
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

/// The seven GitHub collaboration tools, gated on the engine's live
/// `github.enabled` setting (all but `configure`) and `read_only` flag (see
/// `COLLAB_WRITE_TOOLS`). `add_domain` is not among them: it creates domains of
/// every kind, so it is a write-gated tool (see `WRITE_TOOLS`), and only its
/// team-domain branch needs `github.enabled`, enforced in the engine.
const COLLAB_TOOLS: [&str; 7] = [
    "configure",
    "share_changes",
    "update_domain",
    "origin_status",
    "resolve_conflict",
    "withdraw_proposal",
    "discard_changes",
];

/// Of the seven collaboration tools, the five also hidden in read-only mode:
/// `configure` (settings and this machine's GitHub identity are frozen the
/// same way content is), `share_changes`, `resolve_conflict`,
/// `withdraw_proposal` and `discard_changes` (each writes a proposal, config
/// or the working tree). `update_domain` and `origin_status` stay visible
/// read-only, mirroring their engine-level exemption (a pull is a
/// derived-truth update like sync; status is a pure read).
const COLLAB_WRITE_TOOLS: [&str; 5] = [
    "configure",
    "share_changes",
    "resolve_conflict",
    "withdraw_proposal",
    "discard_changes",
];

/// Appended to the initialize instructions while TOON responses are active,
/// so a client model reads list results as structured data rather than prose.
const TOON_INSTRUCTIONS_NOTE: &str = "\n\nList-shaped tool results (search hits, activity, listings and status reports) arrive TOON-encoded rather than as JSON: indentation nests objects, `name[N]{field1,field2}:` heads a uniform array with one comma-separated row per record and a tags cell joins its values with commas. Read them as data with exactly those fields.";

/// Whether `name` is one of the seven collaboration tools.
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

/// The six skill resource uris, comma separated, for the same reason.
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
/// - `auto`: served, unless the harness that spawned this process has its
///   session hook installed **and** its onboarding verified.
///
/// Both inputs are fixed before this server exists, which is what makes the
/// gate legal on a listing at all. `skills_serve` is the effective setting
/// snapshotted at engine construction ([`Engine::skills_serve`]);
/// `gate` is [`McpServer::with_harness_gate`], resolved by
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
/// **A hook in the receipt is not enough.** The surface is hidden only when
/// the harness's onboarding is verified too: until a live check confirmed that
/// the harness loads the shipped skills as files, hiding them could leave it
/// with no way to reach them. A hook that is installed but unverified still
/// shrinks the instructions (see [`instructions_variant`]); it never hides
/// the surface.
///
/// An HTTP session never sets the gate: one daemon serves every HTTP client,
/// a remote client never ran `crystalline install` here, and a remote client
/// is exactly who the served surface exists for.
///
/// Hidden means hidden, not disabled: the lists come back empty while the
/// tool, the resources and the prompts all keep answering a direct call.
/// Read-only mode is not part of it either: reading a skill is a read.
fn hidden_skills_surface(skills_serve: SkillsServe, gate: HarnessGate) -> bool {
    match skills_serve {
        SkillsServe::Always => false,
        SkillsServe::Never => true,
        SkillsServe::Auto => gate.hook_installed && gate.onboarding_verified,
    }
}

/// What one `resolve_conflict` resolution keeps: `mine` or `theirs` by name,
/// or `merged` with the caller's content. Invalid params for `merged` without
/// content and for any other word.
fn resolution_parts<'a>(
    resolution: &str,
    content: Option<&'a str>,
) -> Result<(Option<&'static str>, Option<&'a [u8]>), ErrorData> {
    match resolution {
        RESOLUTION_MINE => Ok((Some(RESOLUTION_MINE), None)),
        RESOLUTION_THEIRS => Ok((Some(RESOLUTION_THEIRS), None)),
        RESOLUTION_MERGED => match content {
            Some(content) => Ok((None, Some(content.as_bytes()))),
            None => Err(ErrorData::invalid_params(
                format!("resolve_conflict requires content when resolution is {RESOLUTION_MERGED}"),
                None,
            )),
        },
        other => Err(ErrorData::invalid_params(
            format!(
                "resolve_conflict resolution must be {RESOLUTION_MINE}, {RESOLUTION_THEIRS} or {RESOLUTION_MERGED}, got '{other}'"
            ),
            None,
        )),
    }
}

/// Which `instructions` block this server hands out on arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstructionsVariant {
    /// The whole routing block.
    Full,
    /// The header plus the pointer that says the session hook has already
    /// delivered the block ([`crystalline_core::render_minimal_instructions`]).
    Minimal,
    /// The header plus the pointer that asks the agent to fetch the block when
    /// it is not already in its context
    /// ([`crystalline_core::prompt::render_conditional_minimal_instructions`]).
    Conditional,
}

/// Which `instructions` block this server hands out: the short ones only
/// under `auto`, and only when the harness that spawned this process has its
/// session hook installed. A verified harness gets [`InstructionsVariant::Minimal`]
/// (its hook is known to have delivered the full block); a hook that is
/// installed but unverified gets [`InstructionsVariant::Conditional`], which
/// does not claim what nobody has checked.
///
/// The same two inputs as [`hidden_skills_surface`], deliberately: the surface
/// and the instructions used to diverge (the surface keyed on the client's
/// name, then stopped), and one input for both is what makes the two eras
/// converge rather than split - a legacy peer reading `initialize` and a
/// modern peer reading `server/discover` are told the same thing. Under
/// `auto` the surface is hidden exactly when the variant is `Minimal`.
///
/// `skills.serve = false` deliberately does not shrink the instructions. That
/// setting gates serving skills, not onboarding: an operator who turns the
/// skill surface off still wants a connecting agent to learn which domains
/// exist.
fn instructions_variant(skills_serve: SkillsServe, gate: HarnessGate) -> InstructionsVariant {
    if skills_serve != SkillsServe::Auto || !gate.hook_installed {
        InstructionsVariant::Full
    } else if gate.onboarding_verified {
        InstructionsVariant::Minimal
    } else {
        InstructionsVariant::Conditional
    }
}

/// Whether collaboration tool `name` is hidden, given the engine's `read_only`
/// state and its live `github.enabled` setting. Not meaningful for a non-collab
/// tool name; callers check [`is_write_tool`] separately for those.
///
/// The net matrix, and the two gates compose rather than override:
///
/// - `github.enabled` off hides all six gated tools whatever the mode is, and
///   never hides `configure`, which is the only way to turn them on.
/// - read-only additionally hides the [`COLLAB_WRITE_TOOLS`] set, so a
///   read-only instance with collaboration on lists `update_domain` and
///   `origin_status` and nothing else of the seven.
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
/// (`crates/engine/src/engine/mod.rs:2172-2175`) takes `self` by value at
/// construction and the engine is shared behind an `Arc`, so no request can
/// reach it.
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
/// this guard (`crates/engine/src/engine/origins.rs`, repeated per verb) for
/// the REST and CLI surfaces; this one exists so the MCP caller reads the
/// reason as tool output rather than as a JSON-RPC error the client renders
/// opaquely.
fn refused_collab_tool(name: &str, github_enabled: bool) -> bool {
    !github_enabled && name != "configure"
}

use crystalline_core::config::{ResponseFormat, SkillsServe};

use crate::DiscardTarget;
use crate::collab::session::AgentPeer;
use crate::domain_view::DomainView;
use crate::engine::{
    ACTOR_MAX_CHARS, ConfigureAction, Engine, EngineError, LiveWriteTarget, OVERLAY_NEEDS_IDENTITY,
    ProvisionAction, ShareActor, sanitize_actor,
};
/// The two facts the `crystalline mcp` process resolved about its harness,
/// re-exported here because [`McpServer::with_harness_gate`] takes one.
pub use crate::instance::HarnessGate;
use crate::params::*;
use crate::scope::member_level_word;
use crate::scope::{DomainRight, Scope};
use crate::similar::SimilarProbe;

/// rmcp's `subscriptions/listen` sink as the engine's registry sees it.
#[derive(Debug)]
struct RmcpListSink(rmcp::service::SubscriptionSink);

#[async_trait::async_trait]
impl crate::subscribers::ToolListSink for RmcpListSink {
    async fn notify_tool_list_changed(&self) -> crate::subscribers::SinkDelivery {
        use crate::subscribers::SinkDelivery;
        use rmcp::service::SubscriptionSendError;
        match self.0.notify_tool_list_changed().await {
            Ok(()) => SinkDelivery::Delivered,
            Err(SubscriptionSendError::SubscriptionClosed) => SinkDelivery::Closed,
            Err(SubscriptionSendError::NotificationNotAccepted(_)) => SinkDelivery::NotAccepted,
            Err(e) => SinkDelivery::Failed(e.to_string()),
        }
    }
}

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

/// The account this call authenticated as, or `None` when nobody did.
///
/// The door does the resolving: with `auth.mcp` on, [`crate::mcp_gate::McpGate`]
/// turns the `Authorization` header into an account before the transport sees
/// the request and leaves it in the request's extensions. rmcp copies the
/// remaining `http::request::Parts` into every tool call's `ctx.extensions`
/// (3.2.0 `transport/streamable_http_server/tower.rs`, four injection sites:
/// `:1219` stateless negotiated, `:1775` session POST, `:1855` session
/// creation, `:1974` stateless POST), so the identity is readable here with no
/// second lookup, no per-connection state of our own and no second channel that
/// could disagree with the gate.
///
/// **What makes it trustworthy is the gate, not this read.** A request bearing
/// a session id is checked against the identity that opened that session before
/// it is routed, so an account cannot arrive on somebody else's session state;
/// a session-less request carries its own credential and is resolved on its
/// own. Nothing a client sends is read here - the extension is inserted
/// server-side or not at all - so an identity cannot be forged by a caller.
///
/// `None` in exactly two cases, both of which keep their legacy actor: stdio,
/// where there are no HTTP parts at all, and auth-off HTTP, where the gate is a
/// pass-through and inserts nothing.
pub(crate) fn mcp_account(ctx: &RequestContext<RoleServer>) -> Option<String> {
    mcp_identity(ctx).map(|identity| identity.name)
}

/// The whole identity the gate resolved, name and instance role together.
///
/// [`mcp_account`] wants only the name; an authorization decision wants the
/// role beside it, and the gate already read both out of the same row (see
/// [`crate::mcp_gate::McpIdentity`]), so taking them from one place is what
/// keeps the two from ever disagreeing.
fn mcp_identity(ctx: &RequestContext<RoleServer>) -> Option<crate::mcp_gate::McpIdentity> {
    let parts = ctx.extensions.get::<axum::http::request::Parts>()?;
    parts
        .extensions
        .get::<crate::mcp_gate::McpIdentity>()
        .cloned()
}

/// What stands in for the client half when a client declared no usable name.
///
/// The composed actor is always two halves, so a bare account name never
/// reaches `generated.by`: `ada` alone reads as "a client calling itself ada",
/// and drops the one fact this composition exists to record - that an agent,
/// not the person, did the writing.
const UNKNOWN_CLIENT: &str = "agent";

/// The join, and the cost of it in kept characters once the engine has folded
/// its spaces into hyphens: `-for-`. [`ACTOR_JOIN_WORD`] is the same word as
/// [`without_the_join`] has to recognize it, once the fold has made it a
/// hyphen-separated segment of its own.
const ACTOR_JOIN: &str = " for ";
const ACTOR_JOIN_CHARS: usize = 5;
const ACTOR_JOIN_WORD: &str = "for";

/// The client half with any join in it taken out, so the composed shape is
/// something only this server can produce.
///
/// [`sanitize_actor`] has already folded whitespace into hyphens by the time
/// this runs, so a client naming itself `x for ada` arrives here as
/// `x-for-ada` - byte-identical to what an authenticated ada session composes,
/// on an instance where nobody authenticated at all. An account name cannot
/// contain whitespace (the auth store's `normalize_account_name` refuses it), so the
/// join is the only way that shape arises honestly, and this is what keeps it
/// that way. A client that genuinely has `for` as a hyphen-separated word in
/// its name loses that word and keeps the rest.
///
/// **Structural rather than textual, because a text substitution can be
/// layered around.** Deleting the `-for-` runs one pass at a time leaves the
/// runs that pass created: `x-for-for-ada` has two overlapping joins, a single
/// non-overlapping left-to-right pass consumes the first and re-joins its
/// neighbours, and `x-for-ada` comes out the other side - the very bytes the
/// deletion exists to prevent. So the half is taken apart on its separator
/// instead: every segment that is the join word is dropped, empty runs
/// collapse with them, and what is rejoined cannot contain `-for-` at any
/// position or multiplicity, because a `-` in the result is only ever a
/// separator this function put there between two segments that are not the
/// word.
///
/// The comparison is ASCII case-insensitive even though `sanitize_actor` does
/// not lowercase and the server's own join is always lowercase, so `x-FOR-ada`
/// is not literally the composed bytes. Provenance gets read by people, and a
/// reader scanning for who a write was made for does not spell-check the case;
/// dropping it costs a client the word `for` in some capitalization and buys
/// the field a rule with no near misses in it.
fn without_the_join(sanitized_client: &str) -> String {
    sanitized_client
        .split('-')
        .filter(|segment| !segment.is_empty() && !segment.eq_ignore_ascii_case(ACTOR_JOIN_WORD))
        .collect::<Vec<_>>()
        .join("-")
}

/// The longest client half a presence label carries, in characters.
///
/// `clientInfo.name` is client-supplied and unbounded, and this label is
/// broadcast to every browser in the room and read back in every agent's
/// `present`. A chip is a chip: a name past this is a client saying more about
/// itself than a participant strip is for.
const AGENT_LABEL_CLIENT_CHARS: usize = 60;

/// The client half as a person reads it, rather than as provenance spells it.
///
/// [`sanitize_actor`] folds whitespace into hyphens because `generated.by` is
/// a token; a chip in a strip is a name, so the spaces stay and only what
/// cannot be drawn goes. `None` when nothing legible is left.
///
/// A cut at [`AGENT_LABEL_CLIENT_CHARS`] carries a trailing `...`, so a
/// truncated name reads as truncated rather than as the whole of what the
/// client reported.
fn display_client(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut gap = false;
    let mut truncated = false;
    for c in raw.trim().chars() {
        if out.chars().count() >= AGENT_LABEL_CLIENT_CHARS {
            truncated = true;
            break;
        }
        if c.is_whitespace() {
            gap = !out.is_empty();
            continue;
        }
        if c.is_control() {
            continue;
        }
        if gap {
            out.push(' ');
            gap = false;
        }
        out.push(c);
    }
    if truncated {
        out.push_str("...");
    }
    (!out.is_empty()).then_some(out)
}

/// How this call shows up in the participant strip of a room it works in.
///
/// **Display, and display only.** What a write RECORDS is
/// [`compose_actor`]'s hyphenated OKF token and is untouched by this; what a
/// person SEES beside their own name while an agent is in their document is
/// this, and the two are built from the same two halves so they can never
/// name different agents. The name leads because that is who the work is
/// being done for, and the harness follows it in the parenthesis because a
/// person watching two agents work needs to tell them apart.
///
/// `None` when no ACCOUNT is known, whatever the client calls itself: see
/// [`presence_label`].
///
/// Over plain values - the account the door resolved, the client half as
/// reported, the scope - so an rmcp call and a remote `tool` call, whose
/// client half the connected machine forwarded, draw the same chip. Read
/// through [`Caller::peer`].
fn peer_for(
    account: Option<String>,
    client: Option<&str>,
    scope: &crate::scope::Scope,
) -> Option<AgentPeer> {
    presence_label(
        presence_identity(account, scope),
        client.and_then(display_client),
    )
}

/// The word a local agent is drawn under, where the only person who can be
/// reading the strip is the person it is working for.
const PRESENCE_SELF: &str = "you";

/// Who a chip is for and what it is called: the account presence is KEYED by,
/// and the name a person READS.
///
/// The two are one string almost always, and the case where they part is the
/// machine owner's own session. A local stdio agent has no gate to resolve an
/// account, so the identity it acts with is
/// [`crate::engine::OWNER_IDENTITY_NAME`] - the name that session's drafts are
/// filed under, which is the right key and the wrong word. Drawn as it stands
/// it tells the owner that somebody called `owner` is in their document, and
/// that somebody is themselves; [`PRESENCE_SELF`] is what it is instead, with
/// the harness still following so two agents of one person are told apart.
///
/// **Decided here, where the two sources are still apart.** An account the
/// gate resolved that happens to be named `owner` is a remote person like any
/// other and keeps their own name in everybody's strip: only the absence of a
/// gate, on a session acting unrestricted, is you. Keying presence by the
/// account either way is what keeps a slot stable across the substitution -
/// the strip's key is who the work is filed under, never what it is captioned.
fn presence_identity(
    gate_account: Option<String>,
    scope: &crate::scope::Scope,
) -> Option<(String, String)> {
    match gate_account {
        Some(account) => Some((account.clone(), account)),
        None => match scope {
            crate::scope::Scope::Unrestricted => Some((
                crate::engine::OWNER_IDENTITY_NAME.to_string(),
                PRESENCE_SELF.to_string(),
            )),
            other => crate::scope::overlay_actor(other).map(|actor| (actor.clone(), actor)),
        },
    }
}

/// The two halves composed, and the rule about which of them may lead.
///
/// **The identity is what makes a chip worth drawing, so without one there is
/// no chip.** A name in somebody's participant strip says "this is who is in
/// your document", and on an instance with MCP authentication off the client
/// half is whatever an unauthenticated caller typed into its handshake - so a
/// label led by it would let anybody put any name beside a person's own. The
/// harness may only ever follow a name the server resolved
/// ([`presence_identity`]).
///
/// That name alone is a complete answer: an agent whose client sent no usable
/// name is "<name> (agent)", which says the true thing and says who it is for.
fn presence_label(identity: Option<(String, String)>, client: Option<String>) -> Option<AgentPeer> {
    let (account, shown) = identity?;
    let label = match client {
        Some(client) => format!("{shown} (agent: {client})"),
        None => format!("{shown} (agent)"),
    };
    Some(AgentPeer { account, label })
}

/// The actor a write records: the client that asked, and - when the call
/// authenticated - the account it asked on behalf of, as `"<client> for
/// <account>"`.
///
/// An agent is not a person, and with the gate on both halves are known: the
/// harness that made the call ([`client_actor`]) and the human whose token
/// opened the session ([`mcp_account`]). Recording only the first would leave
/// an audit of who taught this instance what stopping at "some agent";
/// recording only the second would lose which tool did the writing. So both are
/// kept, in one line, in the one field OKF has for it.
///
/// **Each half is sanitized on its own and the composition happens after**,
/// which is the whole of the integrity here. [`Engine::actor`] sanitizes
/// whatever it is handed and stops at [`ACTOR_MAX_CHARS`] kept characters;
/// `clientInfo.name` and `.version` are client-supplied and unbounded, so
/// composing first and sanitizing once would let a long enough client name
/// spend the entire budget and truncate the half the server asserts off the
/// end - silently, with the write still succeeding. Here the account is
/// measured first and the client half is budgeted against what is left, so the
/// account always lands whole; the second pass the engine makes over the
/// composition is then idempotent apart from folding the join's spaces into
/// hyphens.
///
/// So on disk: `claude-code/2.0-for-ada`. The word `for` is what survives the
/// fold as the join, which is why the composition reads as a phrase rather
/// than as punctuation, and [`without_the_join`] is what keeps a client from
/// writing that phrase itself.
///
/// `None` only when neither half is known, which is [`Engine::actor`]'s
/// fallback case and behaves exactly as it did before.
///
/// Over plain values, so a caller with no request context - a remote `tool`
/// call, whose client half the connected machine forwarded - composes exactly
/// the same way. Read through [`Caller::actor`].
pub(crate) fn compose_actor(account: Option<&str>, client: Option<&str>) -> Option<String> {
    let account = account
        .map(sanitize_actor)
        .filter(|account| !account.is_empty());
    let client = client
        .map(|client| without_the_join(&sanitize_actor(client)))
        .filter(|client| !client.is_empty());
    let Some(account) = account else {
        // Nobody authenticated: the client alone, exactly as before, minus a
        // join it was never entitled to write.
        return client;
    };
    // What is left for the client half once the account and the join are
    // spoken for. A pathological account name can leave nothing, and then the
    // account is the whole of it: the half a caller cannot choose is the half
    // that survives.
    let budget = ACTOR_MAX_CHARS.saturating_sub(account.chars().count() + ACTOR_JOIN_CHARS);
    if budget == 0 {
        return Some(account);
    }
    let client = client.unwrap_or_else(|| UNKNOWN_CLIENT.to_string());
    let client: String = client.chars().take(budget).collect();
    let client = client.trim_end_matches('-');
    if client.is_empty() {
        // A budget too small to hold anything of the client at all. The
        // account alone rather than a bare `for-ada`, which is neither half.
        return Some(account);
    }
    Some(format!("{client}{ACTOR_JOIN}{account}"))
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
/// existed: see `McpServer::gate`.
/// The draft joins one MCP server object opened, and the thing that ends them.
///
/// **A join belongs to a HOLDER, never to an account** (see [`crate::join`]),
/// and this object is the ending of one KIND of holder: a stdio process, one
/// connection on the daemon's own socket, or one legacy `Mcp-Session-Id`
/// session. Each of those is one `McpServer` for its whole life, so its last
/// clone going away is that holder ending and `Drop` ends its joins.
///
/// **A modern-era peer on streamable HTTP is not one of them**, and that is
/// the case this type has to get right rather than the cases it serves. Those
/// requests route statelessly: rmcp builds a fresh service per POST
/// (`daemon.rs`'s table of the five `get_service()` sites), so this object
/// would live for one call and its `Drop` would run at the end of the very
/// request that opened the join. So only keys whose holder
/// [`crate::join::Holder::ends_with_its_holder`] are remembered here at all;
/// a token identity's join is ended by idleness in the registry instead, and
/// nothing on the request path ends it.
///
/// What is NOT kept here is the set of joins the caller is inside. That is
/// read from the registry per call ([`crate::join::Joins::held_by`]), for the
/// same reason: a stateless peer's second request is a different object, and
/// anything remembered in this one it would have forgotten.
struct SessionJoins {
    registry: Arc<crate::join::Joins>,
    /// `(key, account, holder)` for every join this object opened whose
    /// holder ends when this object does. The holder rides along with each
    /// entry, rather than being assumed constant for the object, so `close`
    /// is always asked to end the exact join this object opened - not just
    /// one that happens to name the same account.
    keys: std::sync::Mutex<Vec<(String, String, crate::join::Holder)>>,
}

impl SessionJoins {
    fn new(registry: Arc<crate::join::Joins>) -> SessionJoins {
        SessionJoins {
            registry,
            keys: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Record a key this object was just handed, when this object is what ends
    /// it. De-duplicated, because joining a draft this holder is already
    /// inside answers the key it already holds.
    fn remember(&self, join: &crate::join::Join, key: String) {
        if !join.holder.ends_with_its_holder() {
            return;
        }
        let mut keys = self.lock();
        if !keys.iter().any(|(held, ..)| held == &key) {
            keys.push((key, join.account.clone(), join.holder.clone()));
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(String, String, crate::join::Holder)>> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// This holder has ended, so the drafts it was working inside are drafts it is
/// no longer inside. Empty for a stateless peer, by construction: nothing was
/// remembered for it.
impl Drop for SessionJoins {
    fn drop(&mut self) {
        for (key, account, holder) in self.lock().iter() {
            self.registry.close(key, account, holder);
        }
    }
}

/// Who a verb acts for, as plain values: what an rmcp request context says
/// about its caller, or what a remote `tool` call says about the agent a
/// connected machine forwarded. Every verb core takes one, so the two doors
/// run the same gate, record the same actor, draw the same chip in a room and
/// hold a draft under the same holder.
#[derive(Clone)]
pub(crate) struct Caller {
    /// The scope every engine call runs under.
    pub(crate) scope: Scope,
    /// The account the door resolved, `None` on stdio and the open tier.
    pub(crate) account: Option<String>,
    /// The client half as reported: `clientInfo` over MCP, the forwarded
    /// `agent.client` (or [`crate::remote_ctl::REMOTE_CLIENT`]) remotely.
    /// Sanitized where it is used, never here.
    pub(crate) client: Option<String>,
    /// Who a draft join of this call belongs to; `None` when the call has no
    /// account to bind a join to.
    pub(crate) holder: Option<crate::join::Holder>,
    /// Where this caller opens a page.
    pub(crate) base: crate::web_url::WebBase,
}

impl Caller {
    /// The actor a write records ([`compose_actor`]).
    pub(crate) fn actor(&self) -> Option<String> {
        compose_actor(self.account.as_deref(), self.client.as_deref())
    }

    /// The chip a room draws for this call ([`peer_for`]).
    pub(crate) fn peer(&self) -> Option<AgentPeer> {
        peer_for(self.account.clone(), self.client.as_deref(), &self.scope)
    }
}

/// What a verb core decided, before either door renders it.
///
/// No verb core asks anybody anything: a delete or an acknowledgment runs as
/// typed and an overwrite of a live document is refused, for an MCP peer that
/// can elicit, one that cannot and a remote `tool` call alike, so the three
/// run one branch of each verb rather than copies of it.
enum Verdict {
    /// The engine answered: raw JSON, with this caller's page addresses on it.
    Done(Value),
    /// Refused in words the caller must read: a tool error over MCP, an
    /// envelope error remotely.
    Refused(String),
}

/// The `Mcp-Session-Id` a request carried, if any.
///
/// Read off the HTTP parts rmcp injects into the request extensions, the same
/// place [`mcp_identity`] reads the gate's answer from. Carrying a session id
/// is not on its own what makes a request a session - see
/// [`McpServer::holder_of`], which is this function's only caller.
fn session_header(ctx: &RequestContext<RoleServer>) -> Option<String> {
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.headers.get("mcp-session-id"))
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Numbers one `McpServer` apart from another on the transports where the
/// server object IS the holder. A daemon serves many stdio-shaped connections
/// at once, so the process id alone would make them one holder and any one of
/// them closing would put the others out of their drafts.
static NEXT_SERVER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The MCP server for one connection: one tool router over one shared engine.
/// Cheap to clone; every serving path builds one per connection (the daemon
/// per accepted `mcp` socket, the HTTP transport per session, the stdio bridge
/// once for its single session).
///
/// **Nothing about the connecting client is read any more.** The
/// install-receipt match used to live here as an `AtomicBool` set from the
/// client's own `initialize` name, which is the per-connection variation
/// SEP-2567 forbids. What is here instead was decided before the connection
/// existed: see `McpServer::gate`.
#[derive(Clone)]
pub struct McpServer {
    engine: Arc<Engine>,
    transport: Transport,
    /// Whether the harness that spawned the serving process has Crystalline's
    /// session hook installed, and whether its onboarding (the hook's routing
    /// block and the shipped skills as files) is verified. See [`HarnessGate`].
    ///
    /// **Resolved before the session starts and constant for this server's
    /// life.** The `crystalline mcp` process reads its own `--harness`
    /// argument (written into the harness's MCP registration by `crystalline
    /// install`), asks this machine's install receipt whether that harness
    /// has session hooks wired and reads the harness's profile flag. Neither
    /// input is the client's identity: one is deployment configuration, the
    /// other is machine state. It is the default gate everywhere it cannot be
    /// known (HTTP, a registration predating the flag, an unrecognized harness
    /// id, a missing receipt), which serves the surface and the full instructions,
    /// the safe direction (an over-served client pays duplicated context, an
    /// under-served one loses onboarding it cannot rediscover).
    gate: HarnessGate,
    /// The drafts this server object joined by presenting a share-link, and
    /// the handle whose last clone ends them. See [`SessionJoins`].
    joins: Arc<SessionJoins>,
    /// This server object's number, which is the holder id on the transports
    /// where the object is the holder. See [`SessionJoins`].
    server: u64,
    /// The legacy sessions this process has minted and not yet ended, so
    /// [`McpServer::holder_of`] can tell a session id this server is actually
    /// serving from one a client simply sent. `None` on every construction
    /// that has no session manager behind it - stdio, the daemon's socket, a
    /// test building a server directly - where no request is a legacy session
    /// anyway.
    sessions: Option<Arc<crate::mcp_gate::SessionOwners>>,
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
        let joins = Arc::new(SessionJoins::new(engine.joins().clone()));
        McpServer {
            engine,
            transport,
            gate: HarnessGate::default(),
            joins,
            server: NEXT_SERVER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            sessions: None,
        }
    }

    /// Hand this server the map of live legacy sessions, so it can tell one it
    /// is serving from an id a client sent. Called by the streamable-HTTP
    /// service factory, which is the one construction that has a session
    /// manager behind it.
    pub fn with_session_owners(
        mut self,
        sessions: Arc<crate::mcp_gate::SessionOwners>,
    ) -> McpServer {
        self.sessions = Some(sessions);
        self
    }

    /// Which holder this call's joins belong to, and so what ends them.
    ///
    /// Resolved PER CALL rather than stored, because on the streamable-HTTP
    /// transport the server object is not the holder: rmcp builds a fresh one
    /// per stateless POST, and the thing that persists between two of a modern
    /// peer's requests is the identity its token resolved to. Three answers,
    /// one per way of being a caller here:
    ///
    /// * **stdio, and the daemon's own socket, are a process**, numbered per
    ///   server object so a daemon serving several at once keeps them apart.
    ///   Its joins end when the object does, which is when the connection does.
    /// * **a legacy-shaped HTTP request on a session this process minted is
    ///   that session.** The legacy lifecycle: the transport owns the
    ///   session's ending, and the server object built for it goes with it.
    /// * **every other HTTP request is a token identity**, which is what the
    ///   2026-07-28 era has instead of a session. Nothing on that path is an
    ///   ending, so the registry ends those by idleness.
    ///
    /// **Both halves of the session test are load bearing, and the first is
    /// the one that is easy to get wrong.** rmcp decides whether a request is
    /// served by a per-session object from the BODY and never from the session
    /// header, so an era-shaped `tools/call` carrying a session id is routed
    /// statelessly and its server object lives for that one request. Naming it
    /// a session here would put its join on an object that is about to be
    /// dropped, which is exactly the failure the holder exists to close,
    /// reached through a dual-era client or a proxy that echoes the header.
    ///
    /// So the shape is decided by rmcp's own rule, `uses_legacy_lifecycle`
    /// (rmcp 3.4.0 `service.rs:210-215`, reached from
    /// `tower.rs`'s `is_legacy_request`), which this mirrors clause for clause.
    /// It reads TWO things from the request and they are not the same thing:
    ///
    /// 1. **whether the era's required `_meta` keys are present at all** -
    ///    `protocolVersion` and `clientCapabilities`, tested by
    ///    `RequestMetaObject::missing_required_keys`, which checks presence and
    ///    never what revision the first of them names. A request carrying both
    ///    is stateless for rmcp EVEN WHEN it names a pre-era revision, and that
    ///    clause overrules the version.
    /// 2. **the version**, this request's own: from its `_meta` if it has one,
    ///    otherwise the negotiated one. Only consulted when the first clause
    ///    did not already answer.
    ///
    /// Reading the version alone was the bug of the round before this one: a
    /// dual-era harness that holds a legacy session and still attaches its
    /// SEP-2575 client context to later requests sends exactly the request the
    /// two clauses disagree about.
    /// `an_era_meta_call_naming_a_legacy_revision_is_stateless_on_a_minted_session`
    /// is that request on the wire.
    ///
    /// rmcp's third input, an `initialize` body, needs no clause here: a
    /// handshake reaches no tool and so never asks who holds a join.
    ///
    /// The second half of the session test is that the id has to be one this
    /// process is actually serving. An id nothing minted has no session behind
    /// it and no ending to wait for; the gate refuses a claim belonging to
    /// somebody ELSE with a 403, and an unclaimed one it lets through, so this
    /// is where an unclaimed one stops being a holder.
    ///
    /// `None` when the request carries no account at all, which is the
    /// anonymous open tier: a share-link binds to an account, so there is no
    /// join for that caller to hold and the verb refuses in those words. With
    /// MCP authentication off that is EVERY request, so the claim test below is
    /// never reached and no join is ever opened - an instance with no accounts
    /// has no drafts to join either, and the answer falls out of the first line
    /// rather than out of the session rule.
    ///
    /// **rmcp version-bump checklist.** Because this mirrors `uses_legacy_lifecycle`
    /// clause for clause rather than calling into it, an rmcp upgrade never
    /// fails the build over a drift here - it has to be re-checked by hand
    /// every time: re-read both of rmcp's clauses (the `_meta` presence test
    /// and the version comparison) against this function's two branches and
    /// adjust the branches, and the doc comment above, wherever rmcp's shape moved. See the
    /// `SERVED_PROTOCOL_VERSIONS` and `newest_legacy_handshake_version` doc
    /// comments near the top of this file for the sibling places an rmcp bump
    /// touches.
    ///
    /// Last walked at the 3.2.0 -> 3.4.0 bump (2026-09-17): both clauses are
    /// unchanged (`uses_legacy_lifecycle` and `is_legacy_version` are
    /// byte-identical, and so is `is_legacy_request`, which only moved to a
    /// let-chain and a new error alias), `negotiate_protocol_version` is
    /// byte-identical too, and 3.3.0's new `ServerHandler::negotiate_initialize`
    /// is an opt-in helper that restates the default `initialize` body - this
    /// server overrides `initialize` to supply its own downgrade target and does
    /// not call it.
    ///
    /// Walked again at the 3.4.1 -> 3.5.0 bump (2026-09-30): rmcp deleted
    /// `is_legacy_version` and answers the same comparison through the new public
    /// `ProtocolVersion::has_initialize` (`as_str() < "2026-07-28"`), so both
    /// clauses read the same; `negotiate_protocol_version` only renamed its
    /// locals; `is_legacy_request` is unchanged apart from header validation on
    /// the `initialize` branch. `ProtocolVersion::default()` now is `LATEST`
    /// (2026-07-28), so a default test client names the era in `initialize`
    /// and is answered 2025-11-25 with a session, the path rmcp 3.2.0 opened.
    fn holder_of(&self, ctx: &RequestContext<RoleServer>) -> Option<crate::join::Holder> {
        match self.transport {
            Transport::Stdio => Some(crate::join::Holder::Process(self.server)),
            Transport::Http => {
                let account = mcp_account(ctx)?;
                let era_meta = ctx
                    .meta
                    .missing_required_keys(&ProtocolVersion::V_2026_07_28)
                    .is_empty();
                let legacy_shaped = !era_meta
                    && ctx
                        .protocol_version()
                        .is_none_or(|version| version < ProtocolVersion::V_2026_07_28);
                if legacy_shaped
                    && let Some(session) = session_header(ctx)
                    && let Some(owners) = &self.sessions
                    && owners.owner(&session).as_deref() == Some(account.as_str())
                {
                    return Some(crate::join::Holder::McpSession(session));
                }
                Some(crate::join::Holder::Token(account))
            }
        }
    }

    /// The caller of this request, as plain values: the one thing a verb core
    /// reads about who asked.
    fn caller(&self, ctx: &RequestContext<RoleServer>) -> Caller {
        Caller {
            scope: self.scope_of(ctx),
            account: mcp_account(ctx),
            client: client_actor(ctx),
            holder: self.holder_of(ctx),
            base: self.web_base(ctx),
        }
    }

    /// Where this caller opens a page.
    ///
    /// The same question [`holder_of`](Self::holder_of) answers about identity,
    /// asked about an address: a stdio client is a process on this machine, so
    /// the daemon's own bind is what it can reach; an HTTP client reached this
    /// server at an origin of its own, and that origin is the only address a
    /// browser on the other side of it can open.
    ///
    /// `Unresolved` when an HTTP call carries no request parts at all, which
    /// is a server object built outside the transport (an in-process duplex).
    /// There is a page, nothing here knows this caller's address for it, and
    /// saying so names the setting that ends the guessing.
    fn web_base(&self, ctx: &RequestContext<RoleServer>) -> crate::web_url::WebBase {
        match self.transport {
            Transport::Stdio => self.engine.local_web_base(),
            Transport::Http => match ctx.extensions.get::<axum::http::request::Parts>() {
                Some(parts) => self.engine.request_web_base(&parts.headers),
                None => crate::web_url::WebBase::Unresolved,
            },
        }
    }

    /// Present a share-link: bind it to this account, open the draft it names
    /// for THIS session, and answer the join the verb routes through.
    ///
    /// Both of the browser's two steps at once (see
    /// [`Engine::open_share_link`]), because an agent that was handed a link
    /// and passed it to a verb has decided both: it means to see the draft and
    /// it means to work in it.
    ///
    /// `holder` is the caller's ([`Caller::holder`]): [`McpServer::holder_of`]
    /// over MCP, the token's account remotely (decision D10).
    async fn enter_draft_for(
        &self,
        scope: &Scope,
        holder: Option<crate::join::Holder>,
        token: &str,
    ) -> std::result::Result<crate::engine::OpenedLink, crate::engine::EngineError> {
        let Some(holder) = holder else {
            return Err(crate::engine::EngineError::Refused(
                "a draft share-link binds to an account, and this session has none: authenticate \
                 before presenting one"
                    .to_string(),
            ));
        };
        let opened = self.engine.open_share_link(token, scope, &holder).await?;
        if let crate::engine::OpenedLink::Joined { key, join } = &opened {
            self.joins.remember(join, key.clone());
        }
        Ok(opened)
    }

    /// Present a link on a WRITE: the join it opened, or the refusal that says
    /// why this caller may read the draft and not write in it.
    ///
    /// The other half of [`crate::engine::OpenedLink`]'s two answers. A read
    /// takes `ReadOnly` as an answer; a write asked to land inside the draft
    /// and cannot, so for it the sentence is the refusal it always was.
    async fn joined_by_holder(
        &self,
        scope: &Scope,
        holder: Option<crate::join::Holder>,
        token: &str,
    ) -> Result<crate::join::Join, ErrorData> {
        match self.enter_draft_for(scope, holder, token).await {
            Ok(crate::engine::OpenedLink::Joined { join, .. }) => Ok(join),
            Ok(crate::engine::OpenedLink::ReadOnly(reason)) => {
                Err(to_error(crate::engine::EngineError::Refused(reason)))
            }
            Err(err) => Err(to_error(err)),
        }
    }

    /// The join this session holds that `identifier` names, or `None`.
    ///
    /// **By the page, never by the domain**, and that is the whole of it. A
    /// session that joined one draft of a domain goes on editing its own
    /// engrams in that domain exactly as before; only a call that named the
    /// shared page is routed into its author's copy. Matching by the domain
    /// instead would bind every write the session made anywhere in that domain
    /// to the one page it was invited into.
    ///
    /// The name is checked against what the GRANT opens
    /// ([`Engine::granted_draft_named`]), which is the draft's own address or
    /// its title - and against the path the join was opened for,
    /// so one of the owner's other drafts answering the same name routes
    /// nothing.
    async fn joined_for_holder(
        &self,
        scope: &Scope,
        holder: Option<&crate::join::Holder>,
        domain: &str,
        identifier: &str,
    ) -> Option<crate::join::Join> {
        let account = crate::scope::overlay_actor(scope)?;
        let holder = holder?;
        // The registry, not a list kept on this object: a stateless peer's
        // second request is a different object, so anything this one
        // remembered it would have forgotten. See [`SessionJoins`].
        let held = self.engine.joins().held_by(&account, holder, domain);
        for join in held {
            let named = self
                .engine
                .granted_draft_named(domain, identifier, Some(&join.owner), scope)
                .await
                .ok()
                .flatten();
            if named.is_some_and(|(_, path, _)| path == join.path) {
                return Some(join);
            }
        }
        None
    }

    /// Record what the harness this process serves already does for itself
    /// (see the field). Set by the two stdio paths from the gate the
    /// `crystalline mcp` process resolved at startup: the embedded stack
    /// directly, the daemon relay from the token the bridge writes on its
    /// handshake line. Never set on the HTTP path.
    pub fn with_harness_gate(mut self, gate: HarnessGate) -> McpServer {
        self.gate = gate;
        self
    }

    /// [`McpServer::with_harness_gate`] for a fully onboarded harness or none:
    /// `true` is the verified gate (hook installed and onboarding verified),
    /// `false` the default gate.
    pub fn with_onboarded_harness(self, onboarded: bool) -> McpServer {
        self.with_harness_gate(HarnessGate {
            hook_installed: onboarded,
            onboarding_verified: onboarded,
        })
    }

    /// Who this call is acting as, as the one value every scoped verb on this
    /// server is threaded with.
    ///
    /// Resolved per call rather than stored, for the same reason
    /// [`McpServer::share_actor`] is: the account comes off the request the
    /// gate authenticated, and a copy kept on the server would be one more
    /// thing that could disagree with the door.
    ///
    /// Three answers, one per way of reaching this server:
    ///
    /// * **stdio is [`Scope::Unrestricted`]**, and not as a shortcut. A stdio
    ///   session is a process this machine's harness started, so its caller
    ///   already has every domain's files on disk; there is nothing here for a
    ///   check to protect, and the CLI and control socket pass the same value
    ///   for the same reason.
    /// * **an authenticated HTTP session is [`Scope::User`]**, carrying the
    ///   name and the instance role [`crate::mcp_gate::McpGate`] resolved once,
    ///   before the transport saw the request. Nothing a client sends is read
    ///   here: the extension is inserted server-side or not at all.
    /// * **an HTTP session with no identity is [`Scope::Anonymous`]**, which
    ///   exists only where `auth.mcp` is off - the legacy open tier, where the
    ///   gate is a pass-through. It reads what is shared and sees no private
    ///   domain, which is exactly the tier's promise: an instance that never
    ///   made anything private is byte-identical to its old self, and one that
    ///   did keeps it out of an unauthenticated agent's reach.
    fn scope_of(&self, ctx: &RequestContext<RoleServer>) -> Scope {
        match self.transport {
            Transport::Stdio => Scope::Unrestricted,
            Transport::Http => match mcp_identity(ctx) {
                Some(identity) => Scope::User {
                    account: identity.name,
                    admin: identity.admin,
                },
                None => Scope::Anonymous,
            },
        }
    }

    /// `p` with every domain it names spelled as a local name, for this
    /// caller. First thing in every handler whose params name an existing
    /// domain, because the checks a handler makes before the engine runs
    /// (writability, joins, a hidden-domain refusal) key on the local name
    /// too. A spelling of a domain this caller may not see stays as typed,
    /// so it is refused in the caller's own words.
    async fn localized<P: DomainArgs + Clone>(
        &self,
        p: P,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<P, ErrorData> {
        self.localized_in(p, &self.scope_of(ctx)).await
    }

    /// The gate every write verb passes before it touches a domain, answering
    /// the same two refusals the REST write routes answer and in the same
    /// order.
    ///
    /// 1. **A domain this caller may not see is the not-found**, decided first,
    ///    so a stranger writing into a private domain learns exactly what a
    ///    stranger writing into a domain nobody registered learns.
    /// 2. **Then the right**, which is what a private domain adds and what the
    ///    instance role decides on a shared one. The refusal names the level
    ///    the caller holds, because "forbidden" on a domain they can see and
    ///    read is otherwise indistinguishable from a bug.
    ///
    /// The right read here is [`Engine::write_right`], the domain answer capped
    /// by the instance role, and it is the same call the JSON API's write gate
    /// makes. An instance viewer invited into a private domain as an editor is
    /// therefore refused here exactly as their browser is refused there: an
    /// invitation widens what an account may reach, never what its instance
    /// role lets it do. Reading the uncapped `domain_right` here instead is
    /// what let one person's agent write what that same person could not.
    ///
    /// **The domain gated is the one the call named, and that is the whole of
    /// it.** An identifier cannot move a write to another domain: the absolute
    /// `crystalline://` form is refused outright when its domain is not the
    /// `domain` argument (`Engine::resolve_in`, which every write verb resolves
    /// through), so the named domain is the only domain a write can
    /// reach. An earlier draft of this gate resolved the identifier and checked
    /// *its* domain instead, which left the named one unchecked and let a call
    /// whose two halves disagree reach the unscoped `content_source` behind
    /// them - an error naming every registered domain, private ones included.
    /// The gate and the engine now read the same field.
    ///
    /// `Ok(None)` is the allowed case. `Ok(Some(text))` is a refusal to hand
    /// back through [`refuse`], so the model reads why rather than a bare
    /// error. `Err` is step one's not-found, which is an engine error because
    /// it has to be the engine's own bytes.
    ///
    /// **The legacy open tier is never refused by step two**, and that is the
    /// tier rather than an oversight: an instance with `auth.mcp` off has no
    /// accounts to hold a level, and every agent reaching it writes exactly
    /// what it always wrote. Step one still runs for it, and a private domain
    /// is invisible to it, so there is nothing there for step two to protect.
    ///
    /// That carve-out is read from the setting that creates the tier rather
    /// than from the absence of an identity, which are not the same statement.
    /// With `auth.mcp` on, an unauthenticated request is refused at the door
    /// and never reaches a tool at all; if a gate regression ever let one
    /// through it would arrive here as [`Scope::Anonymous`] too, and it must
    /// not inherit the open tier's writes. So the condition is "the door is
    /// open", not "nobody is there". [`Scope::Unrestricted`] needs no arm at
    /// all - it resolves to [`DomainRight::Own`] on every domain.
    async fn refuse_unwritable(
        &self,
        domain: &str,
        scope: &Scope,
    ) -> Result<Option<String>, ErrorData> {
        self.engine
            .require_domain(domain, scope)
            .await
            .map_err(to_error)?;
        if matches!(scope, Scope::Anonymous) && !self.engine.auth_mcp() {
            return Ok(None);
        }
        let right = self
            .engine
            .write_right(scope, domain)
            .await
            .map_err(to_error)?;
        if right < DomainRight::Write {
            return Ok(Some(format!(
                "your access to '{domain}' is {}, and editor access is required to change it",
                member_level_word(right)
            )));
        }
        Ok(None)
    }

    /// The gate on changing what this instance IS: which domains are
    /// registered on it, how it is configured, and which of the artifacts its
    /// domains ship are installed into the harnesses on the machine it runs
    /// on.
    ///
    /// Three verbs pass through here: [`McpServer::add_domain`], a `configure`
    /// that sets, unsets or connects, and `provision` in its `allow`, `deny`
    /// and `apply` actions - the last because a decision is written into the
    /// same `config.yaml` a `configure set` writes and `apply` then runs the
    /// harness CLIs on the server. A `provision` `status` is a read and is
    /// scoped rather than gated.
    ///
    /// Three answers, and each is a rule rather than a consequence:
    ///
    /// * **a local stdio session is the machine owner.** Whoever runs it
    ///   already has the config file and the domains on disk, so there is
    ///   nothing here for a check to protect;
    /// * **the legacy open tier keeps exactly what it had.** With `auth.mcp`
    ///   off there are no accounts to hold a role, and refusing here would take
    ///   away what every single-user install does on every session. The
    ///   condition is spelled the same way [`McpServer::refuse_unwritable`]
    ///   spells it - the door is open, not merely that nobody is there - so an
    ///   unauthenticated request that somehow got past a gate that is ON cannot
    ///   inherit the open tier's powers;
    /// * **an authenticated agent needs the instance admin role**, which is
    ///   what the JSON API has always required of the same actions. Anything
    ///   else is refused with [`INSTANCE_ADMIN_ONLY`].
    ///
    /// `remove_domain` deliberately does NOT go through here: ending a domain
    /// is gated in the engine, where REST reads the same rule, and that rule is
    /// narrower in one direction (a private domain's owner may end it without
    /// being an admin) and wider in none.
    fn refuse_instance_change(&self, scope: &Scope) -> Option<&'static str> {
        match scope {
            Scope::Unrestricted => None,
            Scope::Anonymous if !self.engine.auth_mcp() => None,
            Scope::User { admin: true, .. } => None,
            _ => Some(INSTANCE_ADMIN_ONLY),
        }
    }

    /// Who a write verb over this connection acts as, when this instance
    /// shares with personal GitHub identities (`github.share_identity =
    /// personal`). Inert in the default `instance` mode, where one credential
    /// does everything.
    ///
    /// **An authenticated session IS its account.** A stdio session is a
    /// process this machine's harness started, so it is the machine owner in
    /// exactly the sense the CLI is - the same local `owner` credential,
    /// connected once with `crystalline connect github --personal`. An HTTP
    /// session that authenticated at the door acts as the account it
    /// authenticated as, so a share goes out on that person's own connected
    /// GitHub identity and their name is on the proposal: the agent acts as the
    /// user rather than as one shared bot everybody's work is filed under.
    ///
    /// **The transport-only answer survives where there is nothing else.** An
    /// HTTP session on an instance that does not make agents authenticate
    /// carries no user auth at all - there is nobody to be - so it stays
    /// [`ShareActor::HttpAgent`] and resolves through `github.agent_identity`,
    /// refusing with a text naming that setting when an admin has named none.
    /// That is the legacy tier unchanged, which is what keeps a default install
    /// behaving as it did.
    ///
    /// Read per call rather than stored: the account comes off the request
    /// (see [`mcp_account`]), and a copy of it kept on the server would be one
    /// more thing that could disagree with the door.
    fn share_actor(&self, ctx: &RequestContext<RoleServer>) -> ShareActor {
        match self.transport {
            Transport::Stdio => ShareActor::Owner,
            Transport::Http => match mcp_account(ctx) {
                Some(account) => ShareActor::Account(account),
                None => ShareActor::HttpAgent,
            },
        }
    }

    /// A write receipt with the neighbours advisory attached, for the two
    /// verbs whose caller is an agent in the loop. Scoped by the caller, so a
    /// private domain's engram is a neighbour only to someone who may see it.
    ///
    /// Called from here rather than from inside the engine verb on purpose:
    /// the probe takes the store lock, which is not reentrant, so it may only
    /// run once the write has returned and released it.
    async fn with_similar(
        &self,
        mut receipt: Value,
        probe: SimilarProbe<'_>,
        scope: &Scope,
    ) -> Value {
        self.engine.attach_similar(&mut receipt, probe, scope).await;
        receipt
    }

    /// A finished write result with the ride-along ask appended to it, when one
    /// is due for the caller ([`crate::nudge::write_verb_trailer`]).
    ///
    /// **Last, on every write verb.** It runs after the receipt is whole -
    /// after the neighbours advisory, after the live-document keys, after the
    /// link the verb attaches - because it is addressed to the agent rather
    /// than to the receipt: nothing downstream reads it, and a trailer that
    /// moved earlier would sit inside a shape somebody parses.
    ///
    /// Only the first content block is touched, and only when it is text: that
    /// block is the receipt, and the blocks after it are the resource links a
    /// client follows. A result whose first block is not text (none today) is
    /// handed back unchanged rather than grown a block of its own, since a
    /// second text block would read as a second receipt.
    ///
    /// Never on a refusal and never on a question: both are answered before a
    /// write happens, so neither reaches this.
    async fn nudged(&self, mut result: CallToolResult, caller: &Caller) -> CallToolResult {
        let Some(trailer) = crate::nudge::write_verb_trailer(
            &self.engine,
            caller.account.as_deref(),
            &caller.scope,
        )
        .await
        else {
            return result;
        };
        if let Some(ContentBlock::Text(text)) = result.content.first_mut() {
            text.text.push_str(&format!("\n\n---\n{trailer}"));
        }
        result
    }

    /// The call routed to the source that holds its domain, or refused here
    /// for one that cannot act on a mounted domain; `None` for a call this
    /// machine answers itself, which then runs the handler exactly as before.
    ///
    /// **Owner sessions only** (decision D8): a stdio session is this
    /// machine's owner; an HTTP session on this daemon is whoever reached the
    /// port, and must never borrow the owner's sign-in to a server. The agent
    /// forwarded with the call is this session's `clientInfo` (decision D11).
    ///
    /// Every limit is the router's own (spec A8): one domain waits at most
    /// [`crystalline_remote::ONE_DOMAIN_LIMIT`], except `evolve_engrams` for
    /// one source, which runs a sweep on the server and waits at most
    /// [`crystalline_remote::CTL_TIMEOUT`] (120 s); a sweep over all domains
    /// waits at most the fan-out deadline per source; and a source in its
    /// down window answers at once.
    ///
    /// A refusal this machine makes itself (a write on a read-only instance)
    /// has the shape the local call gets; every other routed refusal is a
    /// tool error the agent reads.
    async fn mounted<P: serde::Serialize>(
        &self,
        tool: &str,
        p: &P,
        ctx: &RequestContext<RoleServer>,
    ) -> Option<Result<CallToolResult, ErrorData>> {
        if self.transport != Transport::Stdio {
            return None;
        }
        let args = serde_json::to_value(p).ok()?;
        let agent = crystalline_remote::ForwardedAgent {
            client: client_actor(ctx),
        };
        let routed = crate::route::routed(&self.engine, tool, &args, &agent, None).await?;
        Some(match routed {
            Ok(value) => self.render_routed(tool, value),
            // The local part of a sweep failed, or this machine refused: the
            // shape a local call gets.
            Err(crate::route::RouteError::Local(e)) => Err(match e.downcast::<EngineError>() {
                Ok(engine) => to_error(engine),
                Err(e) => ErrorData::internal_error(format!("{e:#}"), None),
            }),
            Err(other) => refuse(other.to_string()),
        })
    }

    /// A routed answer as the handler of `tool` renders its own. The page
    /// address stays the server's (the receipt already carries it, and
    /// nothing here attaches one of this machine's over it), and a mounted
    /// read carries no attachment links (decision D24).
    fn render_routed(&self, tool: &str, value: Value) -> Result<CallToolResult, ErrorData> {
        match tool {
            "write_engram" | "edit_engram" => ok_written(value),
            "move_engram" => ok_moved(value),
            "split_engram" => ok_split(value),
            "read_engram" | "delete_engram" | "infer_schema" => ok(value),
            "search_engrams" => self.ok_found(value),
            _ => self.ok_list(value),
        }
    }
}

/// The verb cores: each engine verb's body once, for whichever door asked.
///
/// An rmcp handler builds a [`Caller`] from its request context
/// ([`McpServer::caller`]); a remote `tool` call ([`McpServer::remote_tool`])
/// builds a `Caller` from the token's account and the agent the connected
/// machine forwarded. Neither asks anybody anything. Everything that decides
/// what happens - localization, the write gate, the join a share link opens,
/// the actor, the chip in a room, the refusals and the page addresses - lives
/// here, so the two doors cannot drift apart. Only argument parsing and the
/// rendering differ: TOON, resource links and the ride-along trailer are the
/// MCP side's ([`McpServer::answered`]), raw engine JSON the remote side's.
///
/// **Routing a call to a connected source belongs in the handlers, never in
/// a core.** The remote door runs these cores directly, and a remote call is
/// answered from this server's own domains (decision D8); a core that routed
/// would let two servers connected to each other forward one call for ever.
impl McpServer {
    /// `p` with every domain it names spelled as a local name, for `scope`.
    /// See [`McpServer::localized`].
    async fn localized_in<P: DomainArgs + Clone>(
        &self,
        p: P,
        scope: &Scope,
    ) -> Result<P, ErrorData> {
        self.engine.localized_for(&p, scope).await.map_err(to_error)
    }

    /// `write_engram` for one caller.
    async fn write_core(&self, p: WriteParams, caller: &Caller) -> Result<Verdict, ErrorData> {
        let mut p = self.localized_in(p, &caller.scope).await?;
        // The model an agent reports is client-supplied text exactly as the
        // client identity is, so it is sanitized the same way and an id that
        // sanitizes away counts as none reported. Belt and braces rather than
        // the load-bearing pass: `Engine::stamped_model` sanitizes whatever
        // reaches it, which is what covers the surfaces that never come through
        // here (the CLI and the control socket decode these params themselves).
        // Whether the model is recorded at all is the engine's call rather than
        // this one either: it resolves the actor the write lands under, and a
        // person's write never carries a model.
        p.model = p
            .model
            .as_deref()
            .map(sanitize_actor)
            .filter(|m| !m.is_empty());
        let scope = &caller.scope;
        if let Some(refusal) = self.refuse_unwritable(&p.domain, scope).await? {
            return Ok(Verdict::Refused(refusal));
        }
        let actor = caller.actor();
        // A capture inside somebody's draft names that draft on the call: this
        // verb derives its destination from the title rather than resolving a
        // page, so there is no identifier to work out which draft was meant.
        let join = match p.share_link.as_deref() {
            Some(token) => Some(
                self.joined_by_holder(scope, caller.holder.clone(), token)
                    .await?,
            ),
            None => None,
        };

        let peer = caller.peer();

        // **A wholesale replacement of a document somebody has open is
        // refused.** An `edit_engram` composes into that document; this verb
        // with `overwrite` replaces it, and the work it would replace is on
        // somebody's screen and not saved anywhere yet. Nobody is asked: the
        // agent's call is the go-ahead everywhere else, and here the go-ahead
        // is not the agent's to give, so the refusal names who is in there
        // and the verb that composes instead. A capture that did not pass
        // `overwrite` onto a taken permalink with a room open over it is
        // refused the same way, before the engine's collision error.
        if let Some(target) = self
            .engine
            .live_write_target(&p, scope, join.as_ref(), peer.as_ref())
            .await
        {
            return Ok(Verdict::Refused(live_overwrite_refusal(&target)));
        }

        // A permalink collision is the engine's own error, which names the
        // argument that would replace the engram; the agent decides.
        let receipt = match self
            .engine
            .write_engram_present(&p, actor.as_deref(), scope, join.as_ref(), peer.as_ref())
            .await
        {
            Ok(receipt) => receipt,
            Err(e) => return overlay_refusal(e).map(Verdict::Refused),
        };
        let receipt = self
            .with_similar(receipt, SimilarProbe::for_write(&p), scope)
            .await;
        Ok(done_written(receipt, &caller.base))
    }

    /// `read_engram` for one caller.
    async fn read_core(&self, p: ReadParams, caller: &Caller) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let scope = &caller.scope;
        // A link presented here binds it to this account and opens the draft
        // for this holder, so the read below answers the draft it names and a
        // later edit of that page lands in its author's copy. The read itself
        // needs no join - a grant is what a read crosses on - but an agent
        // that was handed a link and is reading with it has decided both, the
        // same way a person pressing the button in a browser has.
        //
        // **A refused JOIN is not a failed READ, and the type is what says so.**
        // A grantee who may read the draft and not edit it, and one already
        // working in as many drafts as this instance keeps open for one
        // account, are [`crate::engine::OpenedLink::ReadOnly`] rather than
        // errors: each redeemed the link and asked for exactly what their
        // grant is for. Every way the link itself opens NOTHING is still an
        // error and is raised here - a dead link, a draft that has gone, a
        // caller with no account at all - because that caller is reading a
        // page they were told they had been given, and answering them the
        // domain's own page in silence would let them report it as somebody's
        // draft.
        if let Some(token) = p.share_link.as_deref() {
            self.enter_draft_for(scope, caller.holder.clone(), token)
                .await
                .map_err(to_error)?;
        }
        // Reading somebody's open document is being in the room with them,
        // for as long as the claim stands: the strip names this agent while
        // it works, exactly as it names a person who has the page open.
        let mut value = self
            .engine
            .read_engram_present(&p, scope, caller.peer().as_ref())
            .await
            .map_err(to_error)?;
        // The page this caller opens the engram at, worked out here because
        // the base is a fact about the caller rather than about the engram.
        crate::web_url::attach_engram_url(&mut value, &caller.base);
        Ok(value)
    }

    /// `edit_engram` for one caller.
    async fn edit_core(&self, p: EditParams, caller: &Caller) -> Result<Verdict, ErrorData> {
        let mut p = self.localized_in(p, &caller.scope).await?;
        // The model an agent reports is client-supplied text exactly as the
        // client identity is, so it is sanitized the same way and an id that
        // sanitizes away counts as none reported. Belt and braces rather than
        // the load-bearing pass: `Engine::stamped_model` sanitizes whatever
        // reaches it, which is what covers the surfaces that never come through
        // here (the CLI and the control socket decode these params themselves).
        // Whether the model is recorded at all is the engine's call rather than
        // this one either: it resolves the actor the write lands under, and a
        // person's write never carries a model.
        p.model = p
            .model
            .as_deref()
            .map(sanitize_actor)
            .filter(|m| !m.is_empty());
        let scope = &caller.scope;
        if let Some(refusal) = self.refuse_unwritable(&p.domain, scope).await? {
            return Ok(Verdict::Refused(refusal));
        }
        // Which draft this edit is inside, if any: the link presented on this
        // call, or - for a session already inside one - the join whose draft
        // the identifier names. An edit of anything else in that domain is the
        // session's own, exactly as it was before it joined anything.
        let join = match p.share_link.as_deref() {
            Some(token) => Some(
                self.joined_by_holder(scope, caller.holder.clone(), token)
                    .await?,
            ),
            None => {
                self.joined_for_holder(scope, caller.holder.as_ref(), &p.domain, &p.identifier)
                    .await
            }
        };
        let receipt = match self
            .engine
            .edit_engram_present(
                &p,
                caller.actor().as_deref(),
                scope,
                join.as_ref(),
                caller.peer().as_ref(),
            )
            .await
        {
            Ok(receipt) => receipt,
            Err(e) => return overlay_refusal(e).map(Verdict::Refused),
        };
        // `for_edit` is `None` for `set_frontmatter` and for any operation that
        // carried no content, which is what keeps a lifecycle flip silent.
        let receipt = match SimilarProbe::for_edit(&p) {
            Some(probe) => self.with_similar(receipt, probe, scope).await,
            None => receipt,
        };
        Ok(done_written(receipt, &caller.base))
    }

    /// `move_engram` for one caller. It never asks.
    async fn move_core(&self, p: MoveParams, caller: &Caller) -> Result<Verdict, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let scope = &caller.scope;
        // Both ends, because a move writes at both: a caller who may write only
        // one of the two could otherwise carry knowledge out of a private
        // domain into a shared one, or into a domain it was never invited to.
        // A destination it may not see answers the same not-found the source
        // would - naming a domain is not a way to learn that it exists.
        //
        // The destination is gated whether or not it repeats the source's
        // spelling: an omitted or equal `destination_domain` means the source
        // domain, which the first gate already passed, so the second call is a
        // no-op there rather than a case to skip - and a skip is how a check
        // goes missing when the two spellings stop coinciding.
        //
        // Read exactly as `Engine::move_engram` reads it, untrimmed and
        // unfiltered, the way the REST move route reads it too: a gate that
        // normalizes what the verb does not is gating a different string from
        // the one that gets written to, which is the same disagreement between
        // the gate and the engine that this gate exists to end.
        let destination = p.destination_domain.as_deref().unwrap_or(&p.domain);
        for end in [p.domain.as_str(), destination] {
            if let Some(refusal) = self.refuse_unwritable(end, scope).await? {
                return Ok(Verdict::Refused(refusal));
            }
        }
        let mut receipt = match self
            .engine
            .move_engram_as(&p, caller.actor().as_deref(), scope)
            .await
        {
            Ok(receipt) => receipt,
            Err(e) => return overlay_refusal(e).map(Verdict::Refused),
        };
        // The page address lands on `to` and nowhere else: `from` is the
        // address the engram stopped answering to.
        if let Some(to) = receipt.get_mut("to") {
            crate::web_url::attach_engram_url(to, &caller.base);
        }
        Ok(Verdict::Done(receipt))
    }

    /// `split_engram` for one caller. It never asks.
    async fn split_core(&self, p: SplitParams, caller: &Caller) -> Result<Verdict, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        // One domain, because a split writes twice inside it: the new engram
        // lands in the source's domain, so the source's gate is the whole gate.
        let scope = &caller.scope;
        if let Some(refusal) = self.refuse_unwritable(&p.domain, scope).await? {
            return Ok(Verdict::Refused(refusal));
        }
        let receipt = match self
            .engine
            .split_engram_as(&p, caller.actor().as_deref(), scope)
            .await
        {
            Ok(receipt) => receipt,
            Err(e) => return overlay_refusal(e).map(Verdict::Refused),
        };
        Ok(Verdict::Done(receipt))
    }

    /// `delete_engram` for one caller.
    async fn delete_core(&self, p: DeleteParams, caller: &Caller) -> Result<Verdict, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let scope = &caller.scope;
        if let Some(refusal) = self.refuse_unwritable(&p.domain, scope).await? {
            return Ok(Verdict::Refused(refusal));
        }
        let receipt = match self
            .engine
            .delete_engram_as(&p, caller.actor().as_deref(), scope)
            .await
        {
            Ok(receipt) => receipt,
            Err(e) => return overlay_refusal(e).map(Verdict::Refused),
        };
        Ok(Verdict::Done(receipt))
    }

    /// `search_engrams` for one caller.
    async fn search_engrams_core(
        &self,
        p: SearchParams,
        caller: &Caller,
    ) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let mut v = self
            .engine
            .search_engrams(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        crate::web_url::attach_template(&mut v, &caller.base);
        Ok(v)
    }

    /// `build_context` for one caller.
    async fn build_context_core(
        &self,
        p: ContextParams,
        caller: &Caller,
    ) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let mut v = self
            .engine
            .build_context(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        crate::web_url::attach_template(&mut v, &caller.base);
        Ok(v)
    }

    /// `recent_activity` for one caller.
    async fn recent_activity_core(
        &self,
        p: RecentParams,
        caller: &Caller,
    ) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let mut v = self
            .engine
            .recent_activity(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        crate::web_url::attach_template(&mut v, &caller.base);
        Ok(v)
    }

    /// `list_domains` for one caller.
    async fn list_domains_core(
        &self,
        p: ListDomainsParams,
        caller: &Caller,
    ) -> Result<Value, ErrorData> {
        let v = self
            .engine
            .list_domains(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        Ok(v)
    }

    /// `browse_domain` for one caller.
    async fn browse_domain_core(
        &self,
        p: BrowseParams,
        caller: &Caller,
    ) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let mut v = self
            .engine
            .browse_domain(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        crate::web_url::attach_template(&mut v, &caller.base);
        Ok(v)
    }

    /// `validate_engrams` for one caller.
    async fn validate_engrams_core(
        &self,
        p: ValidateParams,
        caller: &Caller,
    ) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let v = self
            .engine
            .validate_engrams(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        Ok(v)
    }

    /// `infer_schema` for one caller.
    async fn infer_schema_core(&self, p: InferParams, caller: &Caller) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let v = self
            .engine
            .infer_schema(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        Ok(v)
    }

    /// `vocabulary` for one caller.
    async fn vocabulary_core(
        &self,
        p: VocabularyParams,
        caller: &Caller,
    ) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let v = self
            .engine
            .vocabulary(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        Ok(v)
    }

    /// `evolve_engrams` for one caller.
    async fn evolve_engrams_core(
        &self,
        p: EvolveParams,
        caller: &Caller,
    ) -> Result<Value, ErrorData> {
        let p = self.localized_in(p, &caller.scope).await?;
        let v = self
            .engine
            .evolve_engrams(&p, &caller.scope)
            .await
            .map_err(to_error)?;
        Ok(v)
    }

    /// A write verb's [`Verdict`] as the MCP answer: the receipt rendered by
    /// `render` with the ride-along trailer after it, a refusal as a tool
    /// error.
    async fn answered(
        &self,
        verdict: Verdict,
        caller: &Caller,
        render: fn(Value) -> Result<CallToolResult, ErrorData>,
    ) -> Result<CallToolResult, ErrorData> {
        match verdict {
            Verdict::Done(receipt) => Ok(self.nudged(render(receipt)?, caller).await),
            Verdict::Refused(text) => refuse(text),
        }
    }
}

/// Why a remote `tool` call did not produce an answer.
pub(crate) enum RemoteToolError {
    /// Refused in the words an MCP call by the same caller gets as a tool
    /// error: a write gate, a live document, a call off the allow-list.
    Refused(String),
    /// The arguments did not parse, or the engine failed: the message an MCP
    /// call gets as its protocol error.
    Failed(String),
}

impl RemoteToolError {
    /// The sentence the envelope carries.
    pub(crate) fn into_message(self) -> String {
        match self {
            RemoteToolError::Refused(text) | RemoteToolError::Failed(text) => text,
        }
    }
}

/// A remote call's `args` as the params the MCP handler of the same verb
/// parses.
fn remote_args<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, RemoteToolError> {
    serde_json::from_value(args)
        .map_err(|e| RemoteToolError::Failed(format!("invalid arguments: {e}")))
}

/// A verb core's answer for a remote caller: the raw receipt, or the refusal
/// or error message an MCP call would carry.
fn remote_verdict(verdict: Result<Verdict, ErrorData>) -> Result<Value, RemoteToolError> {
    match verdict {
        Ok(Verdict::Done(value)) => Ok(value),
        Ok(Verdict::Refused(text)) => Err(RemoteToolError::Refused(text)),
        Err(error) => Err(remote_failed(error)),
    }
}

fn remote_failed(error: ErrorData) -> RemoteToolError {
    RemoteToolError::Failed(error.message.to_string())
}

impl McpServer {
    /// One engine verb for a connected machine's `tool` command, through the
    /// verb core the MCP handler of the same name runs: localized for this
    /// caller, gated by [`McpServer::refuse_unwritable`], recorded as the
    /// caller's actor, drawn in an open room as the caller's chip, and inside
    /// a draft when a share link opened one for the caller's holder. Every
    /// core answers exactly as it answers an MCP call: a delete or an
    /// acknowledgment runs as typed, an overwrite of a live document is
    /// refused, a permalink collision is the engine's error. The answer is raw
    /// engine JSON, as the control socket's `tool` answers, never the
    /// session's TOON.
    ///
    /// `tool` is one of `remote_ctl::REMOTE_TOOLS`; the route has refused
    /// anything else, and so does the last arm here. This never reaches the
    /// engine's sources: a remote call is answered from this server's own
    /// domains (decision D8).
    pub(crate) async fn remote_tool(
        &self,
        tool: &str,
        args: Value,
        caller: &Caller,
    ) -> Result<Value, RemoteToolError> {
        match tool {
            "write_engram" => remote_verdict(self.write_core(remote_args(args)?, caller).await),
            "read_engram" => self
                .read_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            "edit_engram" => remote_verdict(self.edit_core(remote_args(args)?, caller).await),
            "move_engram" => remote_verdict(self.move_core(remote_args(args)?, caller).await),
            "split_engram" => remote_verdict(self.split_core(remote_args(args)?, caller).await),
            "delete_engram" => remote_verdict(self.delete_core(remote_args(args)?, caller).await),
            "search_engrams" => self
                .search_engrams_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            "build_context" => self
                .build_context_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            "recent_activity" => self
                .recent_activity_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            "list_domains" => self
                .list_domains_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            "browse_domain" => self
                .browse_domain_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            "validate_engrams" => self
                .validate_engrams_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            "infer_schema" => self
                .infer_schema_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            "vocabulary" => self
                .vocabulary_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            t if t == crate::EVOLVE_TOOL_NAME => self
                .evolve_engrams_core(remote_args(args)?, caller)
                .await
                .map_err(remote_failed),
            other => Err(RemoteToolError::Refused(format!(
                "the tool '{other}' {}",
                crate::remote_ctl::NOT_REMOTE
            ))),
        }
    }
}

#[tool_router]
impl McpServer {
    #[tool(
        name = "write_engram",
        title = "Capture engram",
        description = "Capture a new engram - a unit of knowledge - into a domain. Writes the markdown file and indexes it. Body bullets: '- [decision] we chose X #tag' become observations, '- rel_type [[Target]]' become relations. domain is required so an engram never lands in the wrong place. Pass folder to file the engram under a topic prefix: reuse the domain's existing layout (browse_domain shows it), start a subfolder when a topic cluster is forming and keep singletons at the root; the folder path becomes the permalink prefix build_context globs as crystalline://domain/folder/*. permalink, status, recorded_at and generated (who wrote it, with which model, and when) are filled in; pass model with your own model id, the one you were told you are (for example claude-opus-5), on every capture, so a later reader can weigh the page by which model wrote it - leave it out only when you do not know it; valid_from/valid_to are never auto-set - absence means always valid; to bound validity pass them inside metadata as plain ISO dates (YYYY-MM-DD). Any other date format is rejected; a sentinel far-future valid_to and an explicit null are dropped, since absence already means valid forever. Recommended type values: engram, guide, decision, architecture, runbook, reference. Recommended status values (guidance, not enforced): stable, implemented, draft, proposed, idea, poc, deprecated, superseded, archived, legacy. stable is the default and the word for knowledge that holds now; current is the legacy alias for the same state, and a status filter on either word matches engrams carrying either. Of those, deprecated, superseded, archived and legacy are the recognized retirement set: a status inside it softly fades in search ranking, any other value ranks at full strength. Errors if the permalink exists in the same folder unless overwrite is true, and an overwrite replaces the engram that owns the permalink in its own file, whatever that file is called; a permalink owned by an engram in another folder is refused whether or not overwrite is set, and no overwrite is offered for it (move_engram it first, or change it in place with edit_engram); it refuses a title that would file the engram as the reserved index.md or log.md (Crystalline generates the folder index itself). The vocabulary tool lists tags already in use; reuse one before coining a new tag. Set an optional numeric salience metadata key (0-10) to mark exceptionally valuable knowledge; salient engrams are lifted in hybrid search ranking. Raise it later to elevate an engram that proved load-bearing. The receipt may carry a similar list: up to three existing engrams closest in meaning to what was just written, with guidance - read the one that fits and merge into it, supersede it or link it, and say so; never ignore the list silently. Replacing an engram somebody has open in the web editor is never silent: an overwrite of a live document is refused and names who is in there, so use edit_engram for the change, which composes into their document. In a domain in review mode (review: overlay) your write lands in your own private draft; share_changes proposes exactly your drafts for review, and a receipt marked draft means the tree did not move. To capture into somebody's shared draft rather than a copy of your own, pass the draft share-link they handed you (dl_...) as share_link on that call: it opens their draft for this session and the write lands in their copy, at the page the link was minted on and nowhere else.",
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
        if let Some(answer) = self.mounted("write_engram", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        let verdict = self.write_core(p, &caller).await?;
        self.answered(verdict, &caller, ok_written).await
    }

    #[tool(
        name = "read_engram",
        title = "Read engram",
        description = "Read an engram's full markdown and resolved frontmatter to learn what is already known before acting or writing. Identify it by bare permalink, title or a crystalline:// URL; pass domain to disambiguate. An identifier without crystalline:// is domain-relative: 'onboarding/setup', never 'mydomain/onboarding/setup'. A file path is not an identifier: a miss names the permalink it probably meant. The response flags whether each relation and prose link resolves, summarizes what links back and names a build_context anchor for exploring nearby knowledge. Attachments the engram references come back as resource links; fetch one with resources/read when the file itself matters. Somebody may have the engram open in the web editor while you read it: the reply then carries live: true, present (who is in there) and their unsaved text, which is what the engram says right now - read it as work in progress and expect it to move. An engram open in a live editor is read through the live document whenever you are reading your own view of it, so you see what the person sees; a draft you reach with a share_link answers its author's last saved text instead, so an edit inside one is best sent without expected_checksum. Reading a live document is not a private act: you usually join that person's participant strip by name for a minute, so they can see an agent is reading along. If somebody handed you a draft share-link (dl_...), pass it as share_link to read their draft of the page instead of the page the domain holds; that also opens the draft for this connection, so a later edit_engram of it lands in their copy. A stdio server or an MCP session holds that open until the session ends; a sessionless HTTP connection holds it for 30 minutes after your last call about that draft, so present the link again whenever an edit is refused as unjoined. A link you may only read still opens the draft for reading. Where this instance serves the web UI the reply carries web_url, the address a person opens the engram at, to hand to somebody who wants to see it; add # and a heading's slug to open it at a section.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn read_engram(
        &self,
        Parameters(p): Parameters<ReadParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("read_engram", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        let mut value = self.read_core(p, &caller).await?;
        attach_configure_hint(
            &mut value,
            !hidden_collab_tool(
                "configure",
                self.engine.read_only(),
                self.engine.github_enabled(),
            ),
        );
        let links = self.attachment_links(&value, &caller.scope).await;
        let mut result = ok(value)?;
        result.content.extend(links);
        Ok(result)
    }

    #[tool(
        name = "edit_engram",
        title = "Edit engram",
        description = "Refine an existing engram in place as understanding evolves. Sections are addressed by heading path such as '## API > ### Auth'. replace_section keeps the section's heading line and replaces the section body under it, keeping deeper subsections unless include_subsections is set; insert_after_section puts content right under that same heading line. For both, content is the section body only - send only the body, never the heading. Content that repeats the section's own heading has the repeat dropped (a duplicated heading would leave an empty section) and the receipt reports heading_stripped. An edit of a domain's MANIFEST that leaves it broken - an empty When to Use or Scope section, a duplicate heading, a wrong type - still lands, and the receipt lists manifest_findings (the validate M rules: code, severity, message, line) so you can fix the routing right away. operation is one of append, prepend, find_replace, replace_section, insert_before_section, insert_after_section, set_frontmatter. find_replace takes find_text and an optional expected_replacements guard that fails on a count mismatch. set_frontmatter assigns one frontmatter field by key instead of text-substituting a frontmatter line. tags takes values, the whole new list: to add a tag, remove a tag or set tags, read the engram, check vocabulary and pass every tag it should carry (each folded to lowercase-with-hyphens, a tag that cannot be folded refused by name; an empty list removes them). The other settable keys take one value: status, valid_from, valid_to, stale_after, source_date, resource, source_version, salience, verified and evolve_ack; identity, recorded_at and the generated block are refused. Use it to retag or retire an engram, record where knowledge came from (resource, source_version), close or reopen a validity window, push a review date forward, mark knowledge salient or record that you re-checked something. Omit value to remove the field (that is how a valid_to that should never have been set is cleared); status cannot be removed. The four date keys take a plain ISO date (YYYY-MM-DD), resource and source_version take plain text and salience a number from 0 to 10. verified never removes: it stamps { by, at } with the current instant, taking value as the verifying actor or else your own identity. evolve_ack is never cleared by an omitted value either: it acknowledges an evolve finding the user ruled intentional, taking value as the rule id optionally followed by a note ('V101' or 'V101 lineage citation, keep'), and the server records what evidence the finding fired on so the acknowledgment holds while that evidence holds and comes back marked stale when it changes; acknowledging the same finding again replaces its entry, and V301 and V302 are the two rules that keep more than one, an entry per pair (a twin pair for V301, a pair of observation lines for V302), so acknowledging a second pair on the same engram records it beside the first and each pair is silenced on its own. Every other rule keeps exactly one entry however often it fires on that engram, so a second acknowledgment of it replaces what the first said and the finding it was not given for comes back marked stale. To unacknowledge a finding - to unack it, to take back an acknowledgment so the finding resurfaces on the next sweep - pass the value 'remove <rule-id>' ('remove V101') on the same key; it takes back every entry for that rule, which for V301 and V302 means every pair you acknowledged on that engram, it errors when the engram carries no entry for that rule, and the receipt reports evolve_ack_removed. Take an acknowledgment back only when the user asks. Pass expected_checksum (from read_engram) to guard an edit against a change since your read: a conflict is refused if it changed, so re-read and retry; omit it for last-write-wins. An edit of an engram somebody has open in the web editor composes into their live document instead of the file - it arrives under their cursor, keeps what they have typed, and the receipt says landed: live with present naming who is in there; their session saves it. You are usually named in their participant strip while you work there, for a minute after each call, so they can tell which agent a change came from. To edit somebody's shared draft rather than your own copy of the page, pass the draft share-link they handed you (dl_...) as share_link: it opens that draft for this connection and the edit lands in its author's copy, with the receipt saying whose. A draft reached that way is read from its author's last saved text rather than from their open document, so send an edit inside one without expected_checksum: the text composes correctly either way, and a checksum taken from a granted read is refused as stale for as long as its author keeps typing. That stays open until your session ends, or - on a sessionless HTTP connection - for 30 minutes after your last call about the draft, so present the link again whenever an edit is refused as unjoined. Without it, an edit at a path somebody shared with you is refused and told the two ways forward. The generated provenance block is refreshed with who edited it, with which model, and when: pass model with your own model id (for example claude-opus-5) on every edit, and a verification you record carries it too. A content edit's receipt may carry a similar list, the existing engrams closest in meaning to the text just added, with guidance to merge, supersede, link or leave them; set_frontmatter never probes. Status values to reflect a changed lifecycle (recommended values: see write_engram). Temporal frontmatter fields (recorded_at, valid_from, valid_to, source_date, stale_after, plus the legacy last_verified and review_after spellings) must stay plain ISO dates (YYYY-MM-DD): an edit that leaves one malformed is rejected and a sentinel far-future valid_to or an explicit null is dropped, except recorded_at which is required and cannot be nulled. In a domain in review mode (review: overlay) your write lands in your own private draft; share_changes proposes exactly your drafts for review, and a receipt marked draft means the tree did not move.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("edit_engram", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        let verdict = self.edit_core(p, &caller).await?;
        self.answered(verdict, &caller, ok_written).await
    }

    #[tool(
        name = "move_engram",
        title = "Move engram",
        description = "Move, rename or re-home an engram: a new path, a new permalink, a new domain, or any mix, as the knowledge base is reorganized. A move is a refactoring, so use this tool, never write_engram plus delete_engram, which loses recorded_at and the generated provenance. The destination may stay inside the same domain: re-filing an engram into a topic subfolder as a cluster forms is a normal move. Every reference follows the engram to its new address: [[old]] and [[domain:old]] wikilinks, relation bullets and crystalline://domain/old URLs (a #fragment is kept) are rewritten in every domain you can see, and the receipt counts them (links_rewritten engrams, references_rewritten references, rewritten names them); a link by title is left alone unless the domain changed. Set update_links to false to skip the rewrite. The permalink: omit it and it follows the move when it was in step with the old path, and stays when it was a deliberate custom one; permalink \"path\" derives it from the destination path, \"keep\" keeps it, any other value is the new permalink. Rename a permalink in place, for example to repair a permalink that drifted off its folder (evolve_engrams V109), by passing the engram's own current path as destination with permalink \"path\". A permalink another engram already holds, or an engram open in the editor, is refused. A destination filename of index.md or log.md is refused: both names are reserved for the generated directory index and log. In a domain in review mode (review: overlay) your write lands in your own private draft; share_changes proposes exactly your drafts for review, and a receipt marked draft means the tree did not move; a draft keeps its permalink unless you pass one, and references in other engrams follow when the team moves the engram.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("move_engram", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        let verdict = self.move_core(p, &caller).await?;
        self.answered(verdict, &caller, ok_moved).await
    }

    #[tool(
        name = "split_engram",
        title = "Split engram",
        description = "Split an engram: move part of it into a new engram of its own, in one step, when a bundle mixes lifecycles. Split before you retire. Validity is set per engram rather than per bullet, so when one fact in an engram stops holding while the rest still does, move the facts that still hold out with this tool and retire only what remains - never retire the bundle whole and re-type its surviving facts into the successor, which loses their history and repeats the copy on every later expiry. Use it too when an engram grew a second topic that deserves its own engram, and whenever an evolve_engrams V010 carry-forward-gap finding names it. Select what moves with observations (the one-based line numbers read_engram reports for each observation bullet) or with sections (heading paths such as '## Notes' or '## API > ### Auth', which move with every deeper subsection under them), or both; at least one is required. The new engram is written with the moved content, the source's tags and type, status stable and no validity window - the facts moving out are the ones that still hold - plus a '- derived_from [[Source]]' relation, and the source gets '- split_into [[New]]' back so the pair resolves from both ends and stays out of the one-sided-relation finding. Pass folder to file the new engram under a topic prefix, as write_engram does. Both writes are guarded by expected_checksum (from read_engram): a source that changed since your read refuses the split and nothing is created, and any refusal before the source is rewritten takes the new engram back out again. Once the source has been rewritten nothing is undone - a failure after that point keeps both engrams and says so, since the moved bullets then live only in the new one - so re-read both before splitting again. Refused when the selection would leave the source under the verify minimum of three content lines, which is the case where the answer is to retire the whole engram rather than split it. In a domain in review mode (review: overlay) your write lands in your own private draft; share_changes proposes exactly your drafts for review, and a receipt marked draft means the tree did not move.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn split_engram(
        &self,
        Parameters(p): Parameters<SplitParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("split_engram", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        let verdict = self.split_core(p, &caller).await?;
        self.answered(verdict, &caller, ok_split).await
    }

    #[tool(
        name = "delete_engram",
        title = "Delete engram",
        description = "Remove an engram when its knowledge is retired. Deletes the file and its index rows. Prefer setting status to deprecated or superseded when the history still matters. An identifier under assets/ deletes that attachment instead - the stored file and its row - which is how an orphaned-attachment finding is completed after the user says yes; expected_checksum guards engram markdown and is refused for an attachment. In a domain in review mode (review: overlay) your write lands in your own private draft; share_changes proposes exactly your drafts for review, and a receipt marked draft means the tree did not move.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("delete_engram", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        let verdict = self.delete_core(p, &caller).await?;
        self.answered(verdict, &caller, ok).await
    }

    #[tool(
        name = "search_engrams",
        title = "Search engrams",
        description = "Search across every registered domain by default (an all-domain sweep) or a chosen few to recall relevant knowledge and experience. Defaults to hybrid lexical-plus-semantic ranking and falls back to plain text when embeddings are not ready. Filter by type, tags, status, arbitrary frontmatter or a recorded-after date; a filter-only search with no query text is allowed. Every hit is labelled with its domain, and a hit inside an observation carries its line. A hit's snippet is a short window around the match, never the whole engram: read_engram returns the full content, so read before citing or summarizing what a hit only previews. The result reports total, page, limit and count; when count is below total, request the next page to see the rest. A tags filter also matches through a domain's tag aliases (the MANIFEST `## Tag Aliases` section), so a merged old tag name still finds its engrams. A status filter on stable or current matches both, since they are one state under two spellings; any other status matches exactly. Hybrid ranking adds a small salience prior, so an engram marked salient at write time ranks above equally relevant unmarked ones without ever excluding a result. Engrams whose status is deprecated, superseded, archived or legacy are softly faded in ranking (the search.retired_weight setting, default 0.6, 1.0 disables), reordered but never excluded. Every hit on the returned page also comes back as a resource_link block beside the text, in hit order: follow the crystalline:// handle with resources/read instead of assembling the address out of the row's domain and permalink. The result carries one web_url_template; fill in a hit's domain and permalink to hand a person that engram's page.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search_engrams(
        &self,
        Parameters(p): Parameters<SearchParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("search_engrams", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.search_engrams_core(p, &caller)
            .await
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("build_context", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.build_context_core(p, &caller)
            .await
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("recent_activity", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.recent_activity_core(p, &caller)
            .await
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "list_domains",
        title = "List domains",
        description = "List the registered domains with their engram counts to see what the agent has been taught. If no CRYSTALLINE KNOWLEDGE ROUTING block reached you this session, call this at session start with include_routing=true: it returns each domain's When to Use routing bullets plus the behavior rules for this server's tools; follow them and route searches through those domains before answering from memory. The same call re-fetches the index mid-session. Every domain in the answer says whether it is private - visible only to its owner, the accounts invited into it and instance admins - so which domains are private is answered from this one call rather than domain by domain.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_domains(
        &self,
        Parameters(p): Parameters<ListDomainsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("list_domains", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.list_domains_core(p, &caller)
            .await
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("browse_domain", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.browse_domain_core(p, &caller)
            .await
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("validate_engrams", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.validate_engrams_core(p, &caller)
            .await
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("infer_schema", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.infer_schema_core(p, &caller).await.and_then(ok)
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("vocabulary", &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.vocabulary_core(p, &caller)
            .await
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "evolve_engrams",
        title = "Evolve engrams",
        description = "Sweep one domain or every domain for the maintenance the knowledge needs and return a ranked work queue: a to-do list that walks you through tidying, cleaning up, auditing, reviewing or health-checking what has been taught. Detects temporal and lifecycle debt (an elapsed valid_to still marked stable, stale_after past due, long-unverified knowledge, a superseded engram with no successor relation and the half-finished converse, a retired engram still cited as current by live ones, and a team domain holding substantive work nobody has shared for over a week), structural gaps (unresolved [[links]], one-sided supersedes or summarizes pairs, orphans, an engram over the split budget, near-empty stubs, V109 a permalink off its folder - one whose folder part differs from the folder its file sits in, which a build_context folder glob silently misses; repair it with move_engram, destination the engram's own path and permalink \"path\" - V110 a link that spells a domain by this machine's local name instead of its canonical name; rewrite it with edit_engram - and V111 an engram an ingestion record fed that names no resource, so an answer from it cannot cite where it came from; set it with edit_engram set_frontmatter key resource), redundancy (near-duplicate clusters, drifted tags) and meaning (V301 semantic twins, two current engrams that say the same thing in different words, and V302 possible contradiction, two observation lines a local NLI model read as contradicting each other, raised only when evolve.contradictions is on). It detects by dates, links, graph shape, embedding similarity and stored model scores, and it never confirms a contradiction: similarity is agreement about a topic, and a V302 row is the model's question, never proof - read both engrams and decide. It also surfaces engrams people captured directly (through the Fluid web UI) that nobody reviewed yet, so what a person taught gets verified, tagged against the vocabulary and woven into the graph - those findings are judgment class. Attachments are swept too: a file a human added that no engram references, and a reference that points at no stored file, both come back as findings naming the attachment path. Read-only: it changes nothing itself. In a review-mode domain the sweep covers your own drafts too. Each finding names the engram, the evidence and the exact next action with the tool that performs it, and a finding marked mechanical completes intent the archive already records while one marked judgment changes what the archive claims and needs a yes from the user first. Work the queue with the write tools and re-run the same scope to confirm it shrank. Call it when the user asks whether knowledge is still accurate, what needs attention or review, or to tidy, audit, consolidate or spring-clean a domain; after a large ingest lands many engrams at once; and when a search returns hits that disagree, since a half-finished retirement often explains the disagreement. Do not call it at session start, after routine captures or before ordinary recall - it is deliberate maintenance, on demand. When the user rules a finding intentional, acknowledge it (edit_engram set_frontmatter key evolve_ack, value like 'V101 lineage citation, keep') so it stops reappearing while its evidence holds; the sweep reports how many findings acknowledgments suppressed, and an acknowledgment whose evidence changed comes back marked stale. limit caps the queue (default 10), families narrows to detector families (temporal, structure, redundancy or meaning), domains narrows the sweep, include_acknowledged returns the suppressed findings too.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn evolve_engrams(
        &self,
        Parameters(p): Parameters<EvolveParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted(crate::EVOLVE_TOOL_NAME, &p, &ctx).await {
            return answer;
        }
        let caller = self.caller(&ctx);
        self.evolve_engrams_core(p, &caller)
            .await
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "configure",
        title = "Configure Crystalline",
        description = "View and adjust Crystalline's settings, like an app's preferences page: call with no arguments to see them, set to change them (for example github.enabled to turn on team collaboration) and connect to link your GitHub account with a short code you confirm in the browser. With a token it accepts a personal access token instead. Connecting works before or after enabling; only team domains need github.enabled turned on. The instance's network, database and sign-in are not listed here: the operator sets them with the crystalline CLI.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("configure", &p, &ctx).await {
            return answer;
        }
        if self.engine.read_only() {
            return Err(ErrorData::invalid_params(CONFIGURE_READ_ONLY_REFUSAL, None));
        }
        // A call naming a domain is about that domain: its view, and its
        // policy keys and rule overrides. It never touches an instance
        // setting, so the instance-change gate below is not its gate.
        if p.domain.is_some() {
            return self.configure_domain(p, &ctx).await;
        }

        // A bare `configure` is the settings page, which is a read and stays
        // open to every caller of a read-write instance. Everything that
        // CHANGES this instance - a set, an unset, and the three connect
        // fields that decide which GitHub identity it acts as - is an
        // instance change and is gated as one.
        let changes = !p.set.is_empty()
            || !p.unset.is_empty()
            || p.connect.is_some()
            || p.token.is_some()
            || p.host.is_some();
        if changes && let Some(refusal) = self.refuse_instance_change(&self.scope_of(&ctx)) {
            return refuse(refusal);
        }
        // The instance's network, database and sign-in are the operator's,
        // changed only with the crystalline CLI. One operator key refuses
        // the whole call before anything in it is applied, the connect
        // fields included, and the refusal names neither the key's value nor
        // its current setting.
        if p.set
            .keys()
            .chain(p.unset.iter())
            .any(|key| crate::settings::is_operator_key(key))
        {
            return refuse(crate::settings::OPERATOR_SETTING_REFUSAL);
        }

        if p.token.is_some() || p.connect.is_some() {
            let result = match (p.token.as_deref(), p.connect.as_deref()) {
                (Some(token), _) => {
                    self.engine
                        .connect_with_token(token, p.host.as_deref())
                        .await
                }
                (None, Some("github")) => {
                    self.engine
                        .start_device_connect(p.host.as_deref(), p.restart)
                        .await
                }
                (None, Some(other)) => Err(EngineError::Invalid(format!(
                    "configure connect must be 'github', got '{other}'"
                ))),
                (None, None) => unreachable!("checked above: token or connect is set"),
            };
            return result.map_err(to_error).and_then(ok);
        }

        // `github.enabled` gates the listing of six collaboration tools
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

        let mut snapshot = self.engine.configure_snapshot().await.map_err(to_error)?;
        snapshot["domain_settings"] = json!(CONFIGURE_DOMAIN_HINT);
        self.ok_list(snapshot)
    }

    #[tool(
        name = "add_domain",
        title = "Add domain",
        description = "Create or connect a domain to store engrams in - the way to give the agent somewhere to capture knowledge, so it works even on an instance with no domains yet. Three modes follow the arguments: a local domain of markdown files on disk (pass folder, or just domain to use the default root at <domains_root>/<domain>) that is created with a starter MANIFEST when new and adopted in place when it already holds engrams; a virtual database-backed domain with no files (virtual: true with a domain name); or a GitHub team domain that downloads shared knowledge to learn from and share back (repo is owner/name, needs GitHub enabled via configure). repo and virtual are mutually exclusive. Without an explicit domain name, the name defaults to the MANIFEST's domain_name, then the folder or repository name. Available whenever the instance is writable; only the team mode needs GitHub turned on. Connecting a repository this domain is already connected to is safe and simply reports the connected state. Connecting a repository reports progress while it downloads and registers the knowledge, then keeps embedding it for search in the background after the call returns.",
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
        if let Some(answer) = self.mounted("add_domain", &p, &ctx).await {
            return answer;
        }
        // Read-only first, matching `configure` and `remove_domain`: on an
        // instance where nobody may register a domain, "this instance is
        // read-only" is the more useful of the two true answers, and it is the
        // one that does not depend on who is asking.
        if self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }
        // Then the role: registering a domain changes what this instance is, so
        // it is gated before anything is validated - an agent that may not
        // create one is told so rather than told its arguments were wrong.
        if let Some(refusal) = self.refuse_instance_change(&self.scope_of(&ctx)) {
            return refuse(refusal);
        }
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
        // Decides only how a note names a connected server: by name for the
        // machine owner, not at all for an HTTP caller.
        let scope = self.scope_of(&ctx);
        let result: Result<Value, EngineError> = if let Some(repo) = p.repo.as_deref() {
            // Caught here rather than left to the engine's own url building:
            // the same shared check the JSON API's create and domain-name
            // peek use (crates/remote/src/github/mod.rs), so a malformed
            // `repo` or `path` is refused with one classification wherever
            // it is caught, before anything is asked of the forge.
            if let Err(e) = crystalline_remote::validate_repo(repo) {
                return Err(to_error(e.into()));
            }
            if let Some(path) = p.path.as_deref()
                && let Err(e) = crystalline_remote::validate_repo_path(path)
            {
                return Err(to_error(e.into()));
            }
            self.engine
                .origin_add_with_progress_as(
                    repo,
                    p.domain.as_deref(),
                    p.path.as_deref(),
                    p.branch.as_deref(),
                    p.folder.as_deref(),
                    progress,
                    &scope,
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
                    Some(domain) => self.engine.domain_add_virtual_as(domain, &scope).await,
                    None => Err(EngineError::Invalid(
                        "add_domain: a virtual domain requires a domain name".to_string(),
                    )),
                }
            }
        } else {
            self.engine
                .domain_add_local_as(p.domain.as_deref(), p.folder.as_deref(), &scope)
                .await
        };
        result.map_err(to_error).and_then(ok)
    }

    #[tool(
        name = "remove_domain",
        title = "Remove domain",
        description = "Unregister a domain when its knowledge no longer belongs on this instance - the counterpart to add_domain, and the way to remove, unregister, drop or disconnect a domain the agent should stop learning from and searching. What goes is the registration and the search index rows, not the knowledge: a local folder domain is unregistered and its markdown files stay exactly where they are on disk, so pointing add_domain at that folder again re-adopts them; a team domain is unregistered with its local folder left in place and its GitHub repository never touched, so nothing is removed for the rest of the team. A virtual domain is the exception, because its engrams live in the database and ARE its knowledge: it refuses unless you pass purge: true, and there is no folder left to re-adopt afterwards, so export or share what is worth keeping first. Any open co-editing rooms in the domain are saved and closed before it goes; rooms_closed counts them. Private drafts go too: an overlay draft of a path lives in this instance's index and its journal alone, so unregistering the domain ends every actor's unshared drafts in it and nothing brings them back. Drafts that are not yours are named rather than assumed: a domain where somebody else is drafting refuses until end_drafts lists each of them, and the refusal says who and how many drafts each holds (never what is in them). Your own drafts need no naming, and naming yourself as well is accepted and changes nothing, so the actor list a refusal or the confirmation question reports can be sent straight back. On a 2026-07-28 peer that declared an elicitation capability the first call removes nothing and answers input_required instead: a confirmation question naming the domain, how many engrams it holds, what happens to its files and how many private drafts each actor would lose, with one checkbox that is checked by default, so Accept alone removes it; the client puts it to the user and answers by re-sending the same call with the answer. An accept removes the domain unless the box was unchecked; a decline or a cancel removes nothing. A local session is the machine owner and may remove any domain; over HTTP this is for an instance admin, or for the owner of a private domain, and a caller who may not see a domain is answered exactly as if nobody had registered it. A domain defined by an environment variable belongs to that variable: unset it instead. Refuses on a read-only instance, like every mutating tool.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn remove_domain(
        &self,
        Parameters(p): Parameters<RemoveDomainParams>,
        responses: InputResponses,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Some(answer) = self.mounted("remove_domain", &p, &ctx).await {
            return answer.map(CallToolResponse::from);
        }
        let p = self.localized(p, &ctx).await?;
        if self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }
        let scope = self.scope_of(&ctx);
        // The preview raises every refusal the removal itself would raise, in
        // the same order: a domain this caller may not see is refused as an
        // unregistered one, a caller who may see it and may not end it is told
        // who can, an environment-defined domain raises its conflict, and a
        // virtual domain holding engrams is refused until `purge` says the loss
        // was intended. All of them come before the question: never ask about
        // an action that would refuse anyway. `Engine::unregister_domain`
        // decides all four again inside the domain-admin lock, which is where
        // they actually have to hold, so this round is advisory and the engine
        // is the authority.
        let preview = match self
            .engine
            .domain_remove_preview(&p.domain, &scope, p.purge, &p.end_drafts)
            .await
        {
            Ok(preview) => preview,
            Err(e) => return refusal_or_error(e),
        };
        if confirmation_supported(&ctx) {
            match confirmed(&responses.0) {
                None => {
                    return Ok(confirm_question(remove_domain_question(&preview)).into());
                }
                Some(false) => {
                    return refuse(format!(
                        "The removal was not confirmed, so domain '{}' is still registered and \
                         nothing was touched. Call remove_domain again if the user asks for it.",
                        p.domain
                    ))
                    .map(CallToolResponse::from);
                }
                Some(true) => {}
            }
        }
        match self
            .engine
            .unregister_domain(&p.domain, &scope, p.purge, &p.end_drafts)
            .await
        {
            Ok(report) => ok(report).map(CallToolResponse::from),
            Err(e) => refusal_or_error(e),
        }
    }

    #[tool(
        name = "share_changes",
        title = "Share changes",
        description = "Share this domain's new knowledge and experience with the team as a proposal they review on GitHub; returns the review URL to hand to the user. In a review-mode domain the share is exactly your draft entries. Where the forge serves stacked pull requests, sharing while a proposal is open STACKS a new proposal on top of it - each share gets its own focused review - and reviewers merge layers bottom-up (merging the top lands the whole chain). Pass proposal to amend that open layer instead (the way to act on its review feedback); layers above it are re-based automatically. An edit to a file an open higher layer already changed belongs in that higher layer - pass its number - rather than in a lower amend, which would only be overwritten by the layer above it. On forges without stacks the open proposal is updated in place as before: same proposal number, same URL, a fresh commit reviewers are notified about, never a duplicate. Review feedback (approvals, change requests, comments) arrives through update_domain and origin_status, so the loop is: share, read the feedback, refine the engrams, share again naming the layer the feedback belongs to. If a reviewer pushed commits onto the proposal branch the update refuses with guidance: let the review finish on GitHub, or withdraw_proposal and share afresh. A domain whose MANIFEST declares sharing: direct has no review step: the share commits the selected files straight onto the connected branch, in one commit authored by the acting identity, and returns the commit's sha and URL instead of a proposal; it refuses while any proposal is still open (merge or withdraw it first) and answers branch_protected when the branch's rules do not accept direct commits. Pass files to share only some of the changed files - an array of domain-relative paths, with the generated folder indexes of the folders they live in riding along; anything left out stays an unshared local change for a later share, and a path that is not among this domain's unshared changes refuses and names itself. Refuses while conflicts are unsettled so the team always reviews a clean proposal. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure. Where the instance sets github.share_identity to personal, the proposal is authored by the sharer's own personal GitHub identity rather than by the one instance credential: connect one in Fluid (profile > GitHub identity) or with 'crystalline connect github --personal' - without a connection the share refuses and says so - while agent shares over HTTP run as the account the agent authenticated as, or as the account github.agent_identity names where agents are not made to authenticate. The call shares at once, with no question to the user, so make it when the person wants the work shared.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("share_changes", &p, &ctx).await {
            return answer;
        }
        let p = self.localized(p, &ctx).await?;
        if refused_collab_tool("share_changes", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        // Read-only first: it does not depend on the name, so it answers the
        // same for every domain, and it comes before the name check below.
        if self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }
        // A named domain this caller may not see is refused as an unregistered
        // one, and an unregistered name is refused right here too, with the
        // visible set: further in, an unscoped lookup would list every
        // registered domain, private ones included. A read gate rather than a
        // write one: what may be shared is `github.share_identity`'s question
        // and answered further in.
        self.engine
            .require_domain(&p.domain, &self.scope_of(&ctx))
            .await
            .map_err(to_error)?;
        // Nobody is asked: the agent's call is the go-ahead, so every plan
        // publishes on this call. The refusal an agent with no identity meets
        // here is teaching text - "connect with your MCP token and try again" -
        // and it is the same sentence a write of that domain answers, so it
        // goes back the same way: `isError` with the words in it, never a
        // protocol error the client renders opaquely. Every other engine error
        // keeps the shape it had.
        match self
            .engine
            .origin_share(
                &p.domain,
                p.title.as_deref(),
                p.description.as_deref(),
                p.proposal,
                p.files.as_deref(),
                self.share_actor(&ctx),
            )
            .await
        {
            Ok(shared) => ok(shared),
            Err(e) => overlay_write_error(e),
        }
    }

    #[tool(
        name = "update_domain",
        title = "Update domain",
        description = "Learn the team's latest knowledge: pulls what was merged upstream into the domain (or every shared domain), merging cleanly where possible and flagging real conflicts for resolve_conflict. The response carries each still-open proposal's review state and the reviewers' comments verbatim, so this is also how review feedback reaches you: read it, refine the engrams, then call share_changes with proposal set to the number the feedback is on, which amends that layer and re-bases the layers above it. On forges without stacks that same call updates the one open proposal in place. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("update_domain", &p, &ctx).await {
            return answer;
        }
        let p = self.localized(p, &ctx).await?;
        if refused_collab_tool("update_domain", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        if let Some(domain) = p.domain.as_deref() {
            self.engine
                .require_domain(domain, &self.scope_of(&ctx))
                .await
                .map_err(to_error)?;
        }
        // A pull can rewrite a domain's MANIFEST, so `provisioning_declared`
        // can flip here too, and like `add_domain` that announces nothing: the
        // gate it feeds refuses at call time instead of shaping a list.
        let result = self
            .engine
            .origin_update(p.domain.as_deref(), &self.scope_of(&ctx))
            .await;
        result.map_err(to_error).and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "origin_status",
        title = "Origin status",
        description = "Review each shared domain's standing: whether the team has new knowledge to learn, what is waiting to be shared, each open proposal's number, URL, review state (approved, changes requested, commented), whether a reviewer amended its branch, its feedback count, plus declined proposals and any conflicts to settle, and the domain's sharing policy (proposal or direct) with the commits this machine put straight on the branch (direct_shares). Unshared work is a bare count by default (local_changes): pass detail: true to have it named instead, which returns the unshared, uncommitted, not-yet-proposed files as domain-relative paths grouped by change kind - added, modified, deleted - beside a count of the generated folder listings that ride along with a share. Ask for detail whenever you have to say WHICH files are unshared or what would go into the next proposal, and report those paths as given; never work the change set out from the filesystem with a directory listing, a timestamp scan or git, because a deleted file is gone from disk and no scan can see it, and a scan whose count happens to match is not confirmation. Pass diff: true with a domain to also get both sides of every unshared file, the team's and yours, which is what to read before discard_changes. Where the forge serves stacked pull requests every open proposal also carries its position in the chain - layer 1 is the bottom, and reviewers merge bottom-up - beside the domain's stack number, the declined layers still wedged under open work, and whether this chain is mid-repair, which means the next share or withdraw finishes it. Those keys are absent while nothing is stacked, and a position with no stack number means these layers are not grouped on the forge - either the link is still owed, or this domain is not stacking at all. Feedback bodies are not repeated here - update_domain returns the reviewers' comment text. Each proposal carries the author_login it was shared under where one was recorded, which is how a chain whose layers belong to different people says so: an instance that sets github.share_identity to personal shares under each sharer's own connected personal GitHub identity (Fluid's profile > GitHub identity, or 'crystalline connect github --personal'), while agent shares over HTTP run as the account the agent authenticated as, or as the account github.agent_identity names where agents are not made to authenticate; reading and pulling always stay on the one instance credential. kept_branches names a share branch Crystalline keeps upstream: an open pull request is based on it or comes from it, the branch a pull request should move to no longer exists, GitHub refused the delete or a waiting move still needs the branch. Each entry says why (merged, declined or withdrawn share) and what kept it; relay its message as given. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn origin_status(
        &self,
        Parameters(p): Parameters<OriginStatusParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("origin_status", &p, &ctx).await {
            return answer;
        }
        let p = self.localized(p, &ctx).await?;
        if refused_collab_tool("origin_status", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        if let Some(domain) = p.domain.as_deref() {
            self.engine
                .require_domain(domain, &self.scope_of(&ctx))
                .await
                .map_err(to_error)?;
        }
        self.engine
            .origin_status(p.domain.as_deref(), p.detail, p.diff, &self.scope_of(&ctx))
            .await
            .map(lean_origin_status)
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "resolve_conflict",
        title = "Resolve conflict",
        description = "Settle a flagged conflict by keeping your version (mine), taking the team's version (theirs) or providing merged content. The engram then counts as ordinary local knowledge you can share. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure. Resolving touches only this machine and reaches the forge on the next share, which is where an instance that sets github.share_identity to personal needs the sharer's connected personal GitHub identity (Fluid's profile > GitHub identity, or 'crystalline connect github --personal'; agent shares over HTTP run as the account the agent authenticated as, or as the account github.agent_identity names where agents are not made to authenticate). Pass resolution on every call, and content with merged: nobody is asked to choose a side, and a call without a resolution is refused with text that names the three values. Read the local side with read_engram before you choose.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("resolve_conflict", &p, &ctx).await {
            return answer;
        }
        let p = self.localized(p, &ctx).await?;
        if refused_collab_tool("resolve_conflict", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        // Read-only first: it does not depend on the name, so it answers the
        // same for every domain, and it comes before the name check below.
        if self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }
        // A malformed explicit resolution is refused first: that check does not
        // depend on the name, so it answers the same for every domain.
        if let Some(resolution) = p.resolution.as_deref() {
            resolution_parts(resolution, p.content.as_deref())?;
        }
        self.engine
            .require_domain(&p.domain, &self.scope_of(&ctx))
            .await
            .map_err(to_error)?;
        // The resolution comes from the call's own arguments for every peer,
        // and a call that named none is refused in words that name the
        // argument to pass: nobody is asked to choose a side.
        let Some(resolution) = p.resolution.as_deref() else {
            return refuse(RESOLVE_NEEDS_RESOLUTION);
        };
        let (keep, content) = resolution_parts(resolution, p.content.as_deref())?;
        // The same teaching refusal a share answers: settling a conflict in a
        // reviewing domain settles it in somebody's draft, so an agent with no
        // identity is told how to get one rather than handed a protocol error.
        match self
            .engine
            .origin_resolve(&p.domain, &p.path, keep, content, self.share_actor(&ctx))
            .await
        {
            Ok(settled) => ok(settled),
            Err(e) => overlay_write_error(e),
        }
    }

    #[tool(
        name = "withdraw_proposal",
        title = "Withdraw proposal",
        description = "Withdraw, retract, cancel or abandon a share proposal the team no longer wants: closes the open pull request on the forge, retires its share branch and clears the proposal record from this domain's state. The branch is deleted unless an open pull request is based on it or comes from it; then it is kept, and origin_status names it under kept_branches until the next sync can delete it. Pass proposal to name a number, or omit it to withdraw the domain's single open proposal; a declined proposal can be withdrawn too, which tidies its record away. Where the forge stacks proposals, withdrawing a layer that is not the top one closes it and re-bases every layer above it onto what is left, so the chain stays reviewable and nothing above the withdrawal is lost. Set revert true to also restore the shared files to their pre-share content - files edited since sharing are never touched - and leave it off to keep the knowledge local while only the proposal goes away. Use it when a review stalled, a proposal was superseded by better work, or a reviewer amended the branch and share_changes refuses to update it. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure. Where the instance sets github.share_identity to personal, closing the proposal goes out on your own personal GitHub identity: connect one in Fluid (profile > GitHub identity) or with 'crystalline connect github --personal' - without a connection the withdrawal refuses and says so - while agent withdrawals over HTTP run as the account the agent authenticated as, or as the account github.agent_identity names where agents are not made to authenticate. The call withdraws at once, with no question to the user.",
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("withdraw_proposal", &p, &ctx).await {
            return answer;
        }
        let p = self.localized(p, &ctx).await?;
        if refused_collab_tool("withdraw_proposal", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string());
        }
        // Read-only first: it does not depend on the name, so it answers the
        // same for every domain, and it comes before the name check below.
        if self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }
        self.engine
            .require_domain(&p.domain, &self.scope_of(&ctx))
            .await
            .map_err(to_error)?;
        let revert = p.revert.unwrap_or(false);
        // Nobody is asked: the agent's call is the go-ahead. Teaching text
        // rather than a protocol error, for the reason `share_changes` answers
        // it that way: a withdrawal in a reviewing domain is a withdrawal of
        // somebody's proposal of their draft.
        match self
            .engine
            .origin_withdraw(&p.domain, p.proposal, revert, self.share_actor(&ctx))
            .await
        {
            Ok(withdrawn) => ok(withdrawn),
            Err(e) => overlay_write_error(e),
        }
    }

    #[tool(
        name = "discard_changes",
        title = "Discard changes",
        description = "Discard, revert, undo or throw away unshared local changes in a team domain, file by file, before they are shared: each named path is put back the way the team has it - a modified engram gets the team's copy back, an added file is deleted, a deleted file is restored - and the index is updated at once. In a domain in review mode (review: overlay) it clears your own drafts of those paths and never anybody else's. Use it when a change turned out wrong, when an edit should not go into the next proposal, or when the user asks to drop a change; pass the paths from origin_status with detail: true, which is also where diff: true shows what each change is before you decide. Pass expected, a map of path to the sha origin_status reported with diff: true, to have a file that moved since you read that list refused as changed_since instead of overwritten; without it there is no guard and each path is discarded as it stands when the call runs. Refuses by name a path that is not among the domain's unshared changes, refuses a path whose earlier content only an open proposal below the top layer holds (withdraw that layer instead), and refuses a draft somebody has open in a live editor. Never touches GitHub, never closes a proposal (that is withdraw_proposal) and never deletes knowledge the team already has. Needs github.enabled turned on: with team collaboration off this refuses and says how to turn it on with configure. On a 2026-07-28 peer that declared an elicitation capability the first call discards nothing and answers input_required instead: a confirmation question naming the domain, each path and what undoing it does, with one checkbox that is checked by default, so Accept alone discards; the client puts it to the user and answers by re-sending the same call with the answer. An accept discards unless the box was unchecked; a decline or a cancel discards nothing.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn discard_changes(
        &self,
        Parameters(p): Parameters<DiscardChangesParams>,
        responses: InputResponses,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Some(answer) = self.mounted("discard_changes", &p, &ctx).await {
            return answer.map(CallToolResponse::from);
        }
        let p = self.localized(p, &ctx).await?;
        if refused_collab_tool("discard_changes", self.engine.github_enabled()) {
            return refuse(RemoteError::NotEnabled.to_string()).map(CallToolResponse::from);
        }
        if self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }
        self.engine
            .require_domain(&p.domain, &self.scope_of(&ctx))
            .await
            .map_err(to_error)?;
        let actor = self.share_actor(&ctx);
        let expected = p.expected.unwrap_or_default();
        let targets: Vec<DiscardTarget> = p
            .paths
            .iter()
            .map(|path| DiscardTarget {
                path: path.clone(),
                sha256: expected.get(path).cloned(),
            })
            .collect();
        if confirmation_supported(&ctx) {
            match confirmed(&responses.0) {
                None => {
                    // The preview is the list narrowed to the named paths, and
                    // a path the list does not carry refuses here, by name:
                    // never a question about an action that would refuse
                    // anyway (the rule `remove_domain` states).
                    let listed = match self.engine.local_changes(&p.domain, &actor).await {
                        Ok(listed) => listed,
                        Err(e) => return overlay_write_error(e).map(CallToolResponse::from),
                    };
                    let known: Vec<Value> =
                        listed["changes"].as_array().cloned().unwrap_or_default();
                    let mut chosen = Vec::new();
                    let mut unknown = Vec::new();
                    for path in &p.paths {
                        match known.iter().find(|c| c["path"] == json!(path)) {
                            Some(change) => chosen.push(change.clone()),
                            None => unknown.push(path.clone()),
                        }
                    }
                    if !unknown.is_empty() {
                        return refuse(format!(
                            "not among this domain's unshared changes: {}; take the paths from origin_status with detail: true, which lists every file that differs",
                            unknown.join(", ")
                        ))
                        .map(CallToolResponse::from);
                    }
                    // Guarded only where the caller actually named a digest
                    // for one of these paths: an `expected` map that names
                    // none of them guards nothing.
                    let guarded = p.paths.iter().any(|path| expected.contains_key(path));
                    let reviewing = listed["mode"] == json!("review");
                    return Ok(confirm_question(discard_question(
                        &p.domain, &chosen, guarded, reviewing,
                    ))
                    .into());
                }
                Some(false) => {
                    return refuse(DISCARD_REFUSAL).map(CallToolResponse::from);
                }
                Some(true) => {}
            }
        }
        match self
            .engine
            .discard_local_changes(&p.domain, &targets, &actor)
            .await
        {
            Ok(report) => ok(report).map(CallToolResponse::from),
            Err(e) => overlay_write_error(e).map(CallToolResponse::from),
        }
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("provision", &p, &ctx).await {
            return answer;
        }
        let p = self.localized(p, &ctx).await?;
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
        // Read-only first, matching `configure`, `add_domain` and
        // `remove_domain`: on an instance where nothing may be provisioned at
        // all, "this instance is read-only" is the more useful of the two true
        // answers, and it is the one that does not depend on who is asking.
        // The engine refuses these three arms for the same reason; asking here
        // is what keeps the ORDER the same as the sibling verbs'.
        if !matches!(action, ProvisionAction::Status) && self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }
        // Then the role, because allow, deny and apply change what this
        // instance IS: each writes a provisioning decision into the same
        // `config.yaml` that `configure set` edits, and `apply` reconciles
        // artifacts into the harnesses on the machine the daemon runs on. That
        // is the class [`McpServer::refuse_instance_change`] gates, so it is
        // gated here too rather than merely being absent from the list of
        // verbs somebody remembered. `status` is a read and stays open, scoped
        // to the domains this caller may see like every other listing.
        if !matches!(action, ProvisionAction::Status)
            && let Some(refusal) = self.refuse_instance_change(&self.scope_of(&ctx))
        {
            return refuse(refusal);
        }
        // The declaration gate, which used to hide this tool from the listing
        // and now refuses the actions it would make pointless. `status` is
        // deliberately not one of them: it answers an empty report, which is
        // how a caller learns there is nothing to decide.
        if refused_provision_action(&action, self.engine.provisioning_declared()) {
            return refuse(PROVISION_NOT_DECLARED);
        }
        // Only now the name. Deciding about a domain is a way of asking whether
        // it exists, so every check above answers the same whatever name was
        // passed, and here a domain this caller may not see is refused with
        // exactly the bytes an unregistered name gets.
        if let ProvisionAction::Allow { domain } | ProvisionAction::Deny { domain } = &action {
            self.engine
                .require_domain(domain, &self.scope_of(&ctx))
                .await
                .map_err(to_error)?;
        }
        self.engine
            .provision(&action, &self.scope_of(&ctx))
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "skills",
        title = "Skills",
        description = "List the agent skills this server ships and read any skill's full SKILL.md playbook: how to route, capture, model schemas, collaborate and provision tools well with Crystalline. Call with no arguments for the index of names and descriptions; pass name to read one skill before its kind of task.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn skills(
        &self,
        Parameters(p): Parameters<SkillsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(answer) = self.mounted("skills", &p, &ctx).await {
            return answer;
        }
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
    /// The routing block, re-rendered per call and scoped to whoever asked:
    /// the cache refresh first is what makes a virtual domain's bullets
    /// current, exactly as the daemon does before `get_info`.
    ///
    /// This is the pull-shaped mitigation for a client that never received the
    /// block on arrival, and unlike the legacy handshake it carries a request
    /// context - so it hands out that caller's own index rather than either
    /// everybody's or nobody's.
    #[prompt(
        name = "onboarding",
        title = "Knowledge routing",
        description = "The live knowledge routing block for this server: one routing line per domain plus the behavior rules. Insert at session start."
    )]
    async fn onboarding_prompt(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Vec<PromptMessage>, ErrorData> {
        self.engine.refresh_routing_cache().await;
        // Scoped, unlike the legacy handshake block: a prompt request carries a
        // context, so this channel knows who is asking and hands out that
        // caller's own index. On stdio the scope is unrestricted and the bytes
        // are the whole block, exactly as before.
        let text = self
            .routing_text_for(&self.scope_of(&ctx))
            .await
            .map_err(to_error)?;
        Ok(vec![PromptMessage::new_text(Role::User, text)])
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
    /// `tests/mcp/mcp_instructions.rs` pins.
    ///
    /// When [`instructions_variant`] says so, the full routing prose is
    /// replaced by the header plus a pointer: the harness that spawned this
    /// process delivers the block itself at session start, so repeating it
    /// would spend the tokens twice. A verified harness gets the minimal
    /// pointer; a hook that is installed but unverified gets the conditional
    /// one, which asks the agent to fetch the block when it is missing. The
    /// TOON note is appended all the same - no hook carries it, it describes
    /// this connection's wire format rather than the knowledge, and a client
    /// that cannot read a tool result is worse off than one that read the
    /// routing block twice.
    fn arrival_info(&self) -> ServerConfig {
        let mut info = self.get_info();
        let short = match instructions_variant(self.engine.skills_serve(), self.gate) {
            InstructionsVariant::Full => None,
            InstructionsVariant::Minimal => Some(crystalline_core::render_minimal_instructions()),
            InstructionsVariant::Conditional => {
                Some(crystalline_core::prompt::render_conditional_minimal_instructions())
            }
        };
        if let Some(mut instructions) = short {
            if self.engine.response_format() == ResponseFormat::Toon {
                instructions.push_str(TOON_INSTRUCTIONS_NOTE);
            }
            info.instructions = Some(instructions);
        }
        info
    }

    /// [`McpServer::arrival_info`] for the one arrival path that knows who is
    /// asking: the 2026-07-28 era's `server/discover`, which carries a request
    /// context where `initialize` carries none.
    ///
    /// Only the routing block is substituted, and only when the deployment's
    /// onboarding decision left one there. Everything else - the minimal-block
    /// decision, the TOON note, the server info, the capabilities - comes from
    /// the shared builder, so the two arrival paths cannot grow a variant the
    /// other lacks.
    ///
    /// A scope that cannot be resolved is an error rather than the unfiltered
    /// block: onboarding that names a domain the caller may not see is exactly
    /// what this exists to prevent, and a client that gets an error re-asks.
    async fn arrival_info_scoped(&self, scope: &Scope) -> Result<ServerConfig, ErrorData> {
        let mut info = self.arrival_info();
        if instructions_variant(self.engine.skills_serve(), self.gate) != InstructionsVariant::Full
        {
            return Ok(info);
        }
        let mut instructions = self.routing_text_for(scope).await.map_err(to_error)?;
        if self.engine.response_format() == ResponseFormat::Toon {
            instructions.push_str(TOON_INSTRUCTIONS_NOTE);
        }
        info.instructions = Some(instructions);
        Ok(info)
    }

    /// The routing block a scoped channel (`server/discover`, the
    /// `onboarding` prompt) hands this session: on stdio, this machine's
    /// owner, it lists the mounted domains after the local ones, as the stdio
    /// `initialize` block does (decision D25); an HTTP session gets the
    /// scoped block alone. Decided by the transport, never by the scope
    /// (decision D8).
    async fn routing_text_for(&self, scope: &Scope) -> Result<String, EngineError> {
        match self.transport {
            Transport::Stdio => self.engine.routing_text_scoped_with_mounts(scope).await,
            Transport::Http => self.engine.routing_text_scoped(scope).await,
        }
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
    ///
    /// `scope` is the caller's, and the domain is re-checked against it before
    /// the listing is read. Belt and braces: the value handed in came out of a
    /// scoped `read_engram`, so its domain is one this caller may already see.
    /// The engine's attachment listing takes a domain by name and no scope of
    /// its own, though, so the check is made where the name is used rather than
    /// assumed from where it came - and a resolver that cannot answer costs the
    /// links rather than widening them.
    async fn attachment_links(&self, value: &Value, scope: &Scope) -> Vec<ContentBlock> {
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
        if self.engine.require_domain(domain, scope).await.is_err() {
            return Vec::new();
        }
        // This reader's own view, so a draft-only attachment a draft
        // references is a resource link for its author and nothing at all for
        // anybody else. A screen that cannot be computed costs the links rather
        // than widening them, exactly as an unresolvable view does.
        let Ok(hidden) = self.engine.hidden_for(scope).await else {
            return Vec::new();
        };
        let Ok(view) = DomainView::for_read(&self.engine, domain, &hidden, scope) else {
            return Vec::new();
        };
        let Ok(rows) = view.attachments().await else {
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
                Some(engram_link(domain, permalink, title, None))
            })
            .collect();
        let mut result = self.ok_list(value)?;
        result.content.extend(links);
        Ok(result)
    }

    /// `configure` with a domain: the view of how that domain behaves. A
    /// domain this caller may not see is the not-found a read of it gets; a
    /// refusal the engine raises is text the model reads.
    async fn configure_domain(
        &self,
        p: ConfigureParams,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if p.connect.is_some() || p.token.is_some() || p.host.is_some() || p.restart {
            return refuse(CONFIGURE_DOMAIN_NO_CONNECT);
        }
        // The operator keys stay out of configure whatever else the call names.
        if p.set
            .keys()
            .chain(p.unset.iter())
            .any(|key| crate::settings::is_operator_key(key))
        {
            return refuse(crate::settings::OPERATOR_SETTING_REFUSAL);
        }
        let p = self.localized(p, ctx).await?;
        let domain = p.domain.clone().unwrap_or_default();
        let scope = self.scope_of(ctx);
        let answer = if p.set.is_empty() && p.unset.is_empty() {
            self.engine.domain_settings(&domain, &scope).await
        } else {
            // A key only an instance admin may change is this layer's check,
            // as it is REST's: the engine checks the owner rule only. A hidden
            // domain is the not-found first.
            if let Err(e) = self.engine.require_domain(&domain, &scope).await {
                return Err(to_error(e));
            }
            let admin = matches!(scope, Scope::Unrestricted | Scope::User { admin: true, .. });
            if let Some(text) = admin_policy_refusal(
                p.set.keys().chain(p.unset.iter()),
                crystalline_core::policy_registry(),
                admin,
            ) {
                return refuse(text);
            }
            self.engine
                .change_domain_settings(&domain, &p.set, &p.unset, &scope)
                .await
        };
        match answer {
            Ok(view) => self.ok_list(view),
            Err(
                EngineError::Invalid(text)
                | EngineError::Forbidden(text)
                | EngineError::Refused(text),
            ) => refuse(text),
            Err(e) => Err(to_error(e)),
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
            if !crate::settings::is_known_key(key) {
                return Err(applied_failure(
                    &applied,
                    key,
                    crate::settings::unknown_agent_key(key).into(),
                ));
            }
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
            if !crate::settings::is_known_key(key) {
                return Err(applied_failure(
                    &applied,
                    key,
                    crate::settings::unknown_agent_key(key).into(),
                ));
            }
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
    /// also set explicitly: `ServerConfig::default()` leaves
    /// `Implementation::from_build_env()`, which would report the rmcp crate's
    /// own name and version to harness logs rather than crystalline's.
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.server_info = Implementation::new("crystalline", crystalline_core::VERSION);
        // This field is the default `initialize` answer, and `initialize`
        // belongs to the legacy lifecycle, so it names the newest revision that
        // still has a handshake rather than the newest we serve. What we serve
        // is advertised through `supported_protocol_versions` and echoed by
        // `initialize` when a client asks for it. Set explicitly because
        // `ServerConfig::default()` would leave rmcp's own `ProtocolVersion::
        // LATEST` here, which moves when the crate does.
        info.protocol_version = newest_legacy_handshake_version();
        // **Which block, and why the transport decides it.** Stdio keeps the
        // full block, which names and counts every domain: that caller is the
        // machine owner and has the files already.
        //
        // Over HTTP the `initialize` handshake has no caller to resolve, so it
        // cannot leave a private domain's bullets out of a per-caller block. It
        // gets the countable half instead - every behavior rule, the number of
        // domains that are not private, and the pointer at `list_domains`,
        // which does resolve a caller and does filter. Leaving the private
        // domains out of the count needs the accounts database, which this
        // synchronous method cannot read, so [`McpServer::initialize`] puts
        // that block in. What stands here is the conditional pointer, which
        // names and counts nothing: it is what an HTTP peer gets if the
        // private set cannot be read, never a count that includes it.
        //
        // The era's own instructions channel does not go through here at all
        // ([`McpServer::discover`] carries a request context and is scoped);
        // this is the legacy lifecycle only.
        let mut instructions = match self.transport {
            Transport::Stdio => self.engine.routing_text(),
            Transport::Http => crystalline_core::prompt::render_conditional_minimal_instructions(),
        };
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
    /// spawned process (see `McpServer::gate`). What survives is
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
    /// SEP-2575 asks for. `tests/mcp/mcp_modern_era.rs` drives exactly that. The
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
        // The HTTP routing block, counted over the domains that are not
        // private (see [`Engine::routing_text_counted`]). Only when the
        // deployment's onboarding decision left the full block in place; a
        // private set that cannot be read keeps `get_info`'s pointer, which
        // names and counts nothing.
        if self.transport == Transport::Http
            && instructions_variant(self.engine.skills_serve(), self.gate)
                == InstructionsVariant::Full
        {
            match self.engine.routing_text_counted().await {
                Ok(mut instructions) => {
                    if self.engine.response_format() == ResponseFormat::Toon {
                        instructions.push_str(TOON_INSTRUCTIONS_NOTE);
                    }
                    info.instructions = Some(instructions);
                }
                Err(e) => tracing::warn!(
                    "the handshake carries the short routing pointer, the private domains are unreadable: {e}"
                ),
            }
        }
        // **We supply the downgrade target; rmcp decides the echo.** Whatever
        // this handler returns is post-processed by rmcp's
        // `negotiate_protocol_version` (`service/server.rs:480`) on every
        // transport - stdio at `service/server.rs:652`, HTTP at
        // `tower.rs:348` - which echoes the requested revision itself when it
        // is a legacy one we support and otherwise adopts this value, or the
        // newest legacy revision we advertise if this value is not legacy. So
        // an echoing branch here would be inert: it can only ever hand rmcp a
        // value it discards. What is left is the one thing rmcp reads, and it
        // has to stay legacy for rmcp to take it.
        //
        // Read from our own list rather than left at `ServerConfig::default()`'s
        // `ProtocolVersion::LATEST`, so an rmcp whose LATEST moves cannot make
        // us offer a revision we do not serve, and capped below the era for
        // the reasons on [`newest_legacy_handshake_version`].
        info.protocol_version = newest_legacy_handshake_version();
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
    /// `tests/mcp/mcp_instructions.rs` pins that this server offers the block by
    /// every era's own path; no server-side test can prove a client pulled it.
    ///
    /// Overridden rather than inherited for one reason: rmcp's default builds
    /// straight from `get_info()`, and the routing cache has to be refreshed
    /// first, exactly as the daemon does before an `initialize`
    /// (`daemon.rs`), or a discover-first client reads a stale virtual-domain
    /// index. The rest is rmcp's own construction:
    /// `DiscoverResult::from_server_info` carries `instructions` out of
    /// `ServerConfig` untouched and sets `ttl_ms: 0` with `cache_scope: Private`
    /// (rmcp 3.1.2 `model.rs:1246-1268`), which already satisfies the
    /// caching MUST for this operation.
    ///
    /// The client's own `_meta.clientInfo` is deliberately not read here. The
    /// specification says implementations "SHOULD NOT use them to change the
    /// behavior of the client or server", so nothing a client *says* about
    /// itself shapes this answer.
    ///
    /// What does shape it is the authorization on the request, which is the one
    /// variation the caching rules provide for and which rmcp's own
    /// construction already accounts for: `DiscoverResult::from_server_info`
    /// sets `cache_scope: Private`, so a per-caller block is never cached
    /// across callers. The block is rendered through
    /// [`McpServer::arrival_info_scoped`], so a domain this caller may not see
    /// is absent from its onboarding rather than named to it.
    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        self.engine.refresh_routing_cache().await;
        let info = self.arrival_info_scoped(&self.scope_of(&context)).await?;
        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            info,
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
    /// flip `github.enabled`, and six collaboration tools appear or disappear
    /// with it (see [`hidden_collab_tool`]). That is the only mover.
    /// `resources/list` and `prompts/list` read `skills.serve` and
    /// the harness gate, both fixed before the first request arrives, so
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
    /// `tests/mcp/mcp_subscriptions.rs` off the wire, not assumed.
    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let _registered = crate::subscribers::ListSubscribers::register(
            self.engine.list_subscribers(),
            Arc::new(RmcpListSink(context.sink().clone())),
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
    /// engine construction (`Engine::with_read_only`,
    /// `crates/engine/src/engine/mod.rs:2172-2175`, takes `self` by value;
    /// the engine is shared behind an `Arc`),
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
        let skills_hidden = hidden_skills_surface(self.engine.skills_serve(), self.gate);
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
        if name == "skills" && hidden_skills_surface(self.engine.skills_serve(), self.gate) {
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
        if hidden_skills_surface(self.engine.skills_serve(), self.gate) {
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
                // An attachment uri names its domain outright, and nothing had
                // to be read first to learn the name, so this is the one
                // attachment path a caller can reach cold. A domain it may not
                // see is refused exactly as an unregistered one - the same
                // bytes, from the engine's own line - before the file is
                // touched. The domain may be spelled by any of its names; one
                // this caller may not see keeps the spelling they sent.
                let scope = self.scope_of(&context);
                let hidden = self.engine.hidden_for(&scope).await.map_err(to_error)?;
                let domain = self.engine.localize_visible(&url.domain, &hidden).await;
                self.engine
                    .require_domain(&domain, &scope)
                    .await
                    .map_err(to_error)?;
                let (bytes, row) = DomainView::for_read(&self.engine, &domain, &hidden, &scope)
                    .map_err(to_error)?
                    .attachment_bytes(path)
                    .await
                    .map_err(to_error)?;
                return Ok(ReadResourceResult::new(vec![attachment_contents(
                    &request.uri,
                    bytes,
                    &row.mime,
                )])
                // Private: this answer depends on who asked, unlike every
                // other result this server hints (see [`CACHE_SCOPE`]).
                .with_cache_hints_as(&context, CacheScope::Private)
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
        let prompts = if hidden_skills_surface(self.engine.skills_serve(), self.gate) {
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
///
/// **On the five write verbs that text can carry a trailer**, and a caller that
/// re-parses one has to cut before it: a receipt may end with a horizontal rule
/// on its own line (`\n\n---\n`) and one ride-along sentence after it
/// ([`crate::nudge`]). The JSON in front of the rule is compact and therefore
/// holds no raw newline of its own, so the first occurrence of that sequence is
/// the cut. Nothing promises the text is JSON at the protocol level - these
/// tools declare no output schema and set no structured content - so the rule
/// is the contract.
fn ok(value: Value) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&value)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// A `resource_link` content block addressing the engram a write touched, so
/// an era-aware client can follow the handle instead of rebuilding the address
/// out of two payload fields. Same unconditional policy as `read_engram`'s
/// attachment links: a link, never bytes.
///
/// `web_url` rides in the block's `_meta` when the caller's page address is
/// known: the handle a client follows and the page a person opens name one
/// engram, so a client that kept the link kept both. Absent when there is no
/// page for this caller, rather than present and empty.
fn engram_link(domain: &str, permalink: &str, title: &str, web_url: Option<&str>) -> ContentBlock {
    let mut resource = Resource::new(
        format!("crystalline://{domain}/{permalink}"),
        title.to_string(),
    )
    .with_mime_type("text/markdown");
    if let Some(url) = web_url {
        let mut meta = serde_json::Map::new();
        meta.insert("web_url".to_string(), Value::String(url.to_string()));
        resource = resource.with_meta(rmcp::model::MetaObject(meta));
    }
    ContentBlock::resource_link(resource)
}

/// [`ok`] for a `write_engram` or `edit_engram` result, with the link to the
/// engram appended: `domain` and `permalink` read off the result itself, named
/// by its `title` when it carries one (a write does, an edit does not).
///
/// A shape this does not recognize simply gets no link. A result that grew a
/// different spelling costs a client one lookup it was doing anyway; a link
/// built from half a shape would send it somewhere else entirely, and no
/// engine result is worth a panic in the layer that only reports it.
///
/// The verb core has already put `web_url` on the receipt for this caller
/// ([`done_written`]), so the link reads the URL back off it, which is what keeps
/// the two from ever naming different pages.
fn ok_written(value: Value) -> Result<CallToolResult, ErrorData> {
    let link = (|| {
        let domain = value.get("domain").and_then(Value::as_str)?;
        let permalink = value.get("permalink").and_then(Value::as_str)?;
        let title = value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(permalink);
        let web_url = value.get("web_url").and_then(Value::as_str);
        Some(engram_link(domain, permalink, title, web_url))
    })();
    let mut result = ok(value)?;
    result.content.extend(link);
    Ok(result)
}

/// [`ok`] for a `move_engram` result, with the link to where the engram
/// landed: the destination is the point of the call, so the handle names the
/// address the engram answers to now, off the result's own `to` block. Same
/// tolerance as [`ok_written`] for a shape that is not there.
///
/// The page address lands on `to` and nowhere else (the verb core put it
/// there): `from` is the address the engram stopped answering to, which is the
/// one page a person following the receipt must not be sent to.
fn ok_moved(value: Value) -> Result<CallToolResult, ErrorData> {
    let link = (|| {
        let to = value.get("to")?;
        let domain = to.get("domain").and_then(Value::as_str)?;
        let permalink = to.get("permalink").and_then(Value::as_str)?;
        let web_url = to.get("web_url").and_then(Value::as_str);
        Some(engram_link(domain, permalink, permalink, web_url))
    })();
    let mut result = ok(value)?;
    result.content.extend(link);
    Ok(result)
}

/// [`ok`] for a `split_engram` result, with the link pointing at the engram the
/// split created: the one of the two the caller has not read yet.
fn ok_split(value: Value) -> Result<CallToolResult, ErrorData> {
    let link = (|| {
        let domain = value.get("domain").and_then(Value::as_str)?;
        let new = value.get("new")?;
        let permalink = new.get("permalink").and_then(Value::as_str)?;
        let title = new
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(permalink);
        Some(engram_link(domain, permalink, title, None))
    })();
    let mut result = ok(value)?;
    result.content.extend(link);
    Ok(result)
}

/// The refusal for the first of `keys` that only an instance admin may change
/// ([`crystalline_core::PolicyRole::Admin`] in `registry`), when the caller is
/// not one. No key needs the role today; REST refuses the same keys.
fn admin_policy_refusal<'a>(
    keys: impl IntoIterator<Item = &'a String>,
    registry: &[crystalline_core::PolicyKey],
    admin: bool,
) -> Option<String> {
    if admin {
        return None;
    }
    keys.into_iter()
        .find(|key| {
            registry.iter().any(|spec| {
                spec.key == key.as_str() && spec.changed_by == crystalline_core::PolicyRole::Admin
            })
        })
        .map(|key| format!("only an instance admin may change `{key}`"))
}

/// What an authenticated non-admin agent is told when it tries to change what
/// this instance is: which domains are registered, how it is configured, and
/// what it provisions into the harnesses on the machine it runs on.
///
/// The JSON API has always gated those admin-only; over MCP they were open to
/// any account whose agent held a token, which is the last place the two
/// surfaces disagreed. It names the role rather than the person, and it names
/// the way out, because an agent that reads this has to be able to tell its
/// user what to ask for.
const INSTANCE_ADMIN_ONLY: &str = "Changing this instance itself - the domains registered on it, its settings and what it provisions into the harnesses on its machine - is reserved for an instance admin, and the account this session is authenticated as does not hold that role. Ask an admin to make the change (they can do it in Fluid under Settings, or with the crystalline CLI on the server). Capturing, reading and refining knowledge in the domains you can already see is unaffected.";

/// What `configure` answers on a read-only instance, for every call including
/// a bare one. Deliberate, not an oversight: an agent here can act on none of
/// the settings, whatever affects it reaches it another way (the response
/// format through the instructions, `github.enabled` through the tool list, a
/// refused write through its own message), and read-only is the public
/// serving mode, where even masked settings would show paths, the sign-in
/// setup and service addresses to anonymous readers.
const CONFIGURE_READ_ONLY_REFUSAL: &str = "this instance is read-only, and a read-only instance \
     does not show its configuration to connected agents; whoever runs it reads the settings \
     on the server with `crystalline config show`";

/// The one line a bare `configure` adds about the domain view.
const CONFIGURE_DOMAIN_HINT: &str = "Call configure with domain to see and change a domain's policies, sections and rule overrides.";

/// What a domain's MANIFEST carries when it is read over MCP.
const CONFIGURE_MANIFEST_HINT: &str = "This is the domain's MANIFEST: configure with domain lists what each key and section does, and sets the policy keys and rule overrides.";

/// Add the `configure` hint to a read of a domain's MANIFEST (the engram with
/// permalink `manifest` at the domain root), while `configure` is listed.
/// Attached by the rmcp handler alone: `read_core` also answers the remote
/// ctl door, which gets no hint.
fn attach_configure_hint(value: &mut Value, configure_listed: bool) {
    if !configure_listed {
        return;
    }
    let is_manifest = value["permalink"] == "manifest" && value["path"] == "MANIFEST.md";
    if is_manifest && let Some(obj) = value.as_object_mut() {
        obj.insert("configure".to_string(), json!(CONFIGURE_MANIFEST_HINT));
    }
}

/// Why a call naming a domain refuses a connect.
const CONFIGURE_DOMAIN_NO_CONNECT: &str = "connect, token, host and restart are about this instance's GitHub sign-in, not about a domain; call configure without domain to connect";

/// The sentence `remove_domain` asks before it acts, rendered from
/// [`crate::engine::Engine::domain_remove_preview`].
///
/// A client may show only the first line of the message and cut the rest, so
/// the action and its object come first and stay short, the consequence for
/// this kind of domain follows, and the meaning of the two buttons closes it.
/// Plain words throughout: a file domain's markdown stays on disk and comes
/// back when the folder is added again, a team domain comes back by adding it
/// with the repository (never with the folder, which would drop the team
/// connection), and a virtual domain's engrams go with it.
fn remove_domain_question(preview: &Value) -> String {
    let domain = preview["domain"].as_str().unwrap_or_default();
    // Two different absences, and a question about deleting somebody's
    // knowledge owes them the difference. An index that could not be read is
    // a number that exists and is unavailable, so the question says so; a
    // plain absence or a zero is a domain with nothing in it, and the count is
    // left out rather than claimed.
    let count = match preview["engrams"].as_u64() {
        Some(1) => Some("1 engram".to_string()),
        Some(0) => None,
        Some(n) => Some(format!("{n} engrams")),
        None if preview["engrams_unknown"].as_bool().unwrap_or(false) => {
            Some("engram count unknown".to_string())
        }
        None => None,
    };
    let unknown = preview["engrams_unknown"].as_bool().unwrap_or(false);
    let drafts = removal_drafts_clause(preview);
    match preview["kind"].as_str().unwrap_or("file") {
        "virtual" => {
            let held = match &count {
                Some(count) if unknown => format!("and its engrams ({count})"),
                Some(count) => format!("and its {count}"),
                None => "and its engrams".to_string(),
            };
            let they = if preview["engrams"].as_u64() == Some(1) {
                "It lives"
            } else {
                "They live"
            };
            format!(
                "Delete the domain '{domain}' {held}? {they} only in the database, so this \
                 cannot be undone.{drafts} Accept deletes it. Decline keeps everything."
            )
        }
        "team" => {
            let held = count.map(|c| format!(" ({c})")).unwrap_or_default();
            format!(
                "Remove the team domain '{domain}'{held} from this machine? The local folder and \
                 the GitHub repository stay as they are. To use it again, add it with the \
                 repository.{drafts} Accept removes it. Decline keeps everything."
            )
        }
        _ => {
            let held = count.map(|c| format!(" ({c})")).unwrap_or_default();
            format!(
                "Remove the domain '{domain}'{held} from Crystalline? Its files stay on disk. \
                 Adding the folder again brings them back.{drafts} Accept removes it. Decline \
                 keeps everything."
            )
        }
    }
}

/// What the removal question says about the private drafts it would end, or
/// nothing at all when there are none.
///
/// The drafts are the half of a removal that is really lost. A file domain's
/// markdown stays on disk and a team domain's repository is never touched, but
/// an actor's draft of a path lives in the index and in its mirror under the
/// state directory and nowhere else, so ending the domain ends the drafts -
/// including other people's, which is why the sentence names them per actor
/// rather than as one number.
fn removal_drafts_clause(preview: &Value) -> String {
    if preview["drafts_unknown"].as_bool().unwrap_or(false) {
        return " Whether anyone has private drafts here could not be read.".to_string();
    }
    let Some(rows) = preview["drafts"].as_array().filter(|r| !r.is_empty()) else {
        return String::new();
    };
    let total: u64 = rows
        .iter()
        .map(|r| r["entries"].as_u64().unwrap_or_default())
        .sum();
    let per_actor: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{} {}",
                r["actor"].as_str().unwrap_or("someone"),
                r["entries"].as_u64().unwrap_or_default()
            )
        })
        .collect();
    format!(
        " It also ends {total} private {} ({}). They cannot be brought back.",
        if total == 1 { "draft" } else { "drafts" },
        per_actor.join(", ")
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
/// `author_login` joins them, and is the one key here that is omitted rather
/// than nulled: a chain whose layers belong to different people is worth the
/// line, while "nobody recorded" is a line that buys a reader nothing.
///
/// **The four domain-level stack keys are dropped while they are quiet**, and
/// that is deliberately not what [`crate::origin::status_report_json`] does:
/// the JSON surface emits all four always so one reader handles either path,
/// while this one is a context budget. A `stack_number` of null, an empty
/// `stack_wedged` and either debt flag false say nothing a caller can act on,
/// so they say nothing at all. A null `stack_number` beside real positions is
/// read off `stack_link_pending`: with the debt still owed it is the degraded
/// chain - the layers exist and are simply not grouped on the forge yet - and
/// with no debt it is a domain whose open records were never a chain at all,
/// the unstacked forge or `github.stacks` turned off over leftover open
/// proposals.
fn lean_origin_status(mut value: Value) -> Value {
    if let Some(domains) = value.get_mut("domains").and_then(Value::as_array_mut) {
        for domain in domains {
            for key in ["open_proposals", "declined_proposals"] {
                let in_the_chain = key == "open_proposals";
                if let Some(entries) = domain.get_mut(key).and_then(Value::as_array_mut) {
                    for (index, entry) in entries.iter_mut().enumerate() {
                        let author = entry.get("author_login").cloned();
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
                        // A tenth key only where there is somebody to name.
                        if let Some(author) = author.filter(|a| !a.is_null())
                            && let Some(object) = entry.as_object_mut()
                        {
                            object.insert("author_login".to_string(), author);
                        }
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
/// `stack_link_pending` set false, and an empty `kept_branches`. Anything
/// else stays, including a `stack_wedged` list, because a wedged layer is
/// named by the number a caller withdraws or shares against, and a kept
/// branch, because its message is a fact a caller relays.
fn drop_quiet_stack_keys(domain: &mut Value) {
    let Some(object) = domain.as_object_mut() else {
        return;
    };
    if object
        .get("kept_branches")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        object.remove("kept_branches");
    }
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

/// What an unconfirmed discard tells the model, naming what is still true
/// rather than what failed.
const DISCARD_REFUSAL: &str = "The discard was not confirmed, so nothing was discarded. Call discard_changes again if the user asks for it.";

/// The sentence `discard_changes` asks before it touches anything.
///
/// A client may show only the first line of the message, so the action, the
/// count and the domain come first; then what each path gets, capped at ten;
/// then the facts a person deciding needs - in a reviewing domain only their
/// own drafts are cleared, and nothing reaches GitHub; then the guard; and the
/// meaning of the two buttons closes it.
///
/// `guarded` is whether the caller named an `expected` digest for any of these
/// paths, and the guard sentence turns on it because the guard is the
/// caller's to ask for. With digests, a file edited between this question and
/// the yes is refused rather than overwritten. Without them the engine fills
/// every digest at discard time, so what is undone is whatever the file holds
/// by then - which is what the question says, rather than promising a guard
/// nobody asked for. `reviewing` is whether the domain keeps drafts (the
/// change listing's `mode` is `review`).
fn discard_question(domain: &str, changes: &[Value], guarded: bool, reviewing: bool) -> String {
    let noun = if changes.len() == 1 {
        "change"
    } else {
        "changes"
    };
    let mut question = format!("Undo {} unshared {noun} in '{domain}'?", changes.len());
    for c in changes.iter().take(10) {
        let path = c["path"].as_str().unwrap_or_default();
        let what = match c["kind"].as_str().unwrap_or_default() {
            "added" => "your new file is deleted",
            "modified" => "the team's version comes back",
            "deleted" => "the deleted file comes back",
            other => other,
        };
        question.push_str(&format!(" {path}: {what}."));
    }
    if changes.len() > 10 {
        question.push_str(&format!(" And {} more.", changes.len() - 10));
    }
    if reviewing {
        question.push_str(" Only your own drafts are cleared.");
    }
    question.push_str(" Nothing goes to GitHub.");
    question.push_str(if guarded {
        " A file edited after you looked is left as it is."
    } else {
        " Edits made after you looked are undone too."
    });
    question.push_str(if changes.len() == 1 {
        " Accept undoes it. Decline keeps your change."
    } else {
        " Accept undoes them. Decline keeps your changes."
    });
    question
}

/// The refusal for a call that named no resolution: a tool error the model can
/// read, replacing the framework's opaque InvalidParams.
///
/// Nobody is asked to choose a side, so the caller is told what to send
/// instead: the `resolution` argument with each of its three values, and the
/// `content` argument merged needs.
const RESOLVE_NEEDS_RESOLUTION: &str = "resolve_conflict needs a resolution: pass resolution mine (keep your version) or theirs (take the team's version), or resolution merged with the reconciled text in content.";

/// The refusal every client gets for a full replace of an engram somebody has
/// open in the editor, naming who is in there.
///
/// Nobody is asked, and `overwrite=true` is refused the same way for as long
/// as the room is open, so the text names the two ways that work: a targeted
/// edit, which composes into their document, or the replace once they have
/// closed it.
fn live_overwrite_refusal(target: &LiveWriteTarget) -> String {
    format!(
        "The engram is open in the editor right now (present: {}), so a full replace was refused and nothing was written. Use edit_engram for a targeted change, or replace it after they close it.",
        present_names(&target.present)
    )
}

/// Who is in a room, for a sentence a person reads.
///
/// A room with connections but no published name is a real state - a browser
/// that has not sent its awareness frame yet - and saying so plainly is better
/// than an empty parenthesis that reads like a bug.
fn present_names(present: &[String]) -> String {
    match present.is_empty() {
        true => "nobody has published a name".to_string(),
        false => present.join(", "),
    }
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

/// An engine error as the shape its content deserves: a refusal the model must
/// read, or a protocol error.
///
/// The split [`McpServer::refuse_unwritable`] already makes, applied to an
/// engine error rather than to a right. [`EngineError::Forbidden`] is raised
/// only about something the caller can already see, and its whole content is
/// teaching text naming who can - the skills tell an agent to relay exactly
/// that rather than retry - so it goes back as a tool error the client renders,
/// for the reason [`refuse`] states. [`EngineError::ConfirmationRequired`] is
/// there for the same reason and a stronger one: its whole content is the flag
/// that would let the call through, so a model that cannot read it cannot
/// complete the task it was given. Everything else keeps [`to_error`]'s
/// protocol shape, and the not-found in particular must: its bytes are what a
/// hidden domain is answered with, and the two have to stay identical.
fn refusal_or_error(e: EngineError) -> Result<CallToolResponse, ErrorData> {
    match e {
        EngineError::Forbidden(text) | EngineError::ConfirmationRequired(text) => {
            refuse(text).map(CallToolResponse::from)
        }
        other => Err(to_error(other)),
    }
}

/// An overlay write's own reading of [`EngineError::Refused`]: when the
/// message is [`OVERLAY_NEEDS_IDENTITY`], it is teaching text a caller must
/// see - "connect with your MCP token and try again" - not a mistake to
/// retry blindly, so it goes back as a tool error the client renders, the
/// same way [`refusal_or_error`] already reads `Forbidden` and
/// `ConfirmationRequired`. `refuse_unwritable` already let this caller through
/// by the time a write reaches this: the legacy open tier has no accounts to
/// hold a member level, so that gate is not the one a review-mode domain's
/// missing identity trips. Every other `Refused` message keeps [`to_error`]'s
/// protocol shape, unchanged.
///
/// **Every verb that resolves a sharer's identity reads its errors through
/// this**, not only the write verbs: a share, a withdrawal and a conflict
/// resolution in a reviewing domain are all about somebody's draft, so an
/// agent with no identity has nothing to share, withdraw or settle and is told
/// how to get one. The tool descriptions teach that sentence, so a caller that
/// meets this refusal did what it was told, and an opaque protocol error would
/// leave it nothing to do next.
///
/// **One helper, and the shape is the call site's.** A tool function that
/// answers [`CallToolResponse`] rather than the bare [`CallToolResult`] this
/// reads adds `.map(CallToolResponse::from)` where it knows which it is; a
/// second helper that did only that wrapping was one name for no decision.
fn overlay_write_error(e: EngineError) -> Result<CallToolResult, ErrorData> {
    refuse(overlay_refusal(e)?)
}

/// [`overlay_write_error`]'s rule before it is rendered: `Ok` with the
/// teaching text a caller must read, `Err` with the protocol error for every
/// other engine error. The verb cores read their errors through this, so both
/// doors refuse in the same words.
fn overlay_refusal(e: EngineError) -> Result<String, ErrorData> {
    match e {
        EngineError::Refused(message) if message == OVERLAY_NEEDS_IDENTITY => Ok(message),
        other => Err(to_error(other)),
    }
}

/// A write receipt as a finished [`Verdict`], with the page this caller opens
/// the engram at.
fn done_written(mut receipt: Value, base: &crate::web_url::WebBase) -> Verdict {
    crate::web_url::attach_engram_url(&mut receipt, base);
    Verdict::Done(receipt)
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
        // The caller asked for something they are not allowed to do, and the
        // message says who is: input-class guidance, like the read-only
        // refusal above it.
        | EngineError::Forbidden(_)
        | EngineError::ConfirmationRequired(_)
        | EngineError::EnvTokenConnect
        // The caller asked at the wrong moment rather than for the wrong
        // thing, and the message says to try again once the other sign-in is
        // done: actionable input-class guidance, like the two above it.
        | EngineError::ConnectInProgress
        // The domain takes changes in a shape this call did not satisfy, and
        // the message is the way in: input-class guidance again, and an agent
        // that reads it can retry correctly in one step.
        | EngineError::Refused(_) => ErrorData::invalid_params(e.to_string(), None),
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
/// tells the caller the opposite of what the message says. The two
/// organization-policy refusals sit in that same class for the same reason:
/// nothing is broken here, and the message names the page that clears it.
/// This match is exhaustive over `RemoteError` so a new variant must be
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
        | RemoteError::ServerCredential { .. }
        | RemoteError::BaseUnavailable => ErrorData::internal_error(message, None),
        RemoteError::NotEnabled
        | RemoteError::NotConnected
        | RemoteError::RepoNotFound { .. }
        | RemoteError::NotADomain { .. }
        | RemoteError::ConflictsPending { .. }
        | RemoteError::ProposalNotFound { .. }
        | RemoteError::NoWithdrawTarget { .. }
        | RemoteError::StacksUnsupported
        | RemoteError::NotFastForward { .. }
        | RemoteError::BranchProtected { .. }
        | RemoteError::Refused(_)
        | RemoteError::SsoAuthorizationRequired { .. }
        | RemoteError::OauthAppRestricted { .. }
        | RemoteError::ConflictNotFound { .. } => ErrorData::invalid_params(message, None),
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::ErrorCode;

    use super::*;

    /// A policy key only an instance admin may change is refused to anyone
    /// else, set or unset; an owner key passes. No such key exists yet, so
    /// the registry here is a stand-in.
    #[test]
    fn an_admin_policy_key_is_refused_to_a_caller_who_is_not_an_admin() {
        let registry = [
            crystalline_core::PolicyKey {
                key: "guarded",
                kind: crystalline_core::PolicyKind::Choice,
                values: &["on", "off"],
                default: "off",
                meaning: "A stand-in key only an admin changes.",
                changed_by: crystalline_core::PolicyRole::Admin,
            },
            crystalline_core::PolicyKey {
                key: "sharing",
                kind: crystalline_core::PolicyKind::Choice,
                values: &["proposal", "direct"],
                default: "proposal",
                meaning: "A stand-in owner key.",
                changed_by: crystalline_core::PolicyRole::Owner,
            },
        ];
        let keys = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            admin_policy_refusal(&keys(&["sharing", "guarded"]), &registry, false).as_deref(),
            Some("only an instance admin may change `guarded`")
        );
        assert_eq!(
            admin_policy_refusal(&keys(&["guarded"]), &registry, true),
            None,
            "an admin may"
        );
        assert_eq!(
            admin_policy_refusal(&keys(&["sharing", "rules.E007"]), &registry, false),
            None,
            "an owner key and a rule override are the engine's to gate"
        );
    }

    /// A client name past [`AGENT_LABEL_CLIENT_CHARS`] is cut, and the cut
    /// carries a trailing `...` so it reads as a cut rather than as the
    /// whole of what the client reported. A name at or under the cap is
    /// untouched.
    #[test]
    fn display_client_marks_a_cut_name_as_cut() {
        let long = "x".repeat(AGENT_LABEL_CLIENT_CHARS + 10);
        let shown = display_client(&long).expect("a name of legible characters");
        assert!(
            shown.ends_with("..."),
            "a truncated name must say so: {shown}"
        );
        assert_eq!(
            shown.chars().count(),
            AGENT_LABEL_CLIENT_CHARS + 3,
            "the cap's characters plus the three that mark the cut: {shown}"
        );

        let exact = "y".repeat(AGENT_LABEL_CLIENT_CHARS);
        let shown = display_client(&exact).expect("a name of legible characters");
        assert_eq!(
            shown, exact,
            "a name at the cap exactly is not truncated and carries no mark"
        );
    }

    /// **Ruling M2.** On the tier where MCP authentication is off, nobody is
    /// authenticated and nothing is filed under a name - so however the client
    /// names itself, it gets no chip in anybody's strip.
    ///
    /// The half a caller chooses may only ever FOLLOW an account the server
    /// resolved. Letting it lead would put a name of the caller's choosing
    /// beside a person's own name in their own document, which is the one
    /// thing a participant strip must not be able to say.
    #[test]
    fn an_unauthenticated_caller_gets_no_chip_however_it_names_itself() {
        // What `peer_for` resolves on that tier: no gate identity at all,
        // and no draft identity either.
        let nobody = presence_identity(None, &Scope::Anonymous);
        assert!(nobody.is_none(), "the open tier holds nobody in particular");
        assert!(
            presence_label(nobody, display_client("Grace Hopper")).is_none(),
            "so there is nobody to put in the strip"
        );
        // An account is what earns one, and the harness then follows it.
        let peer = presence_label(
            presence_identity(Some("ada".to_string()), &Scope::Anonymous),
            display_client("claude-code/2.0"),
        )
        .expect("an authenticated caller is a peer");
        assert_eq!(peer.account, "ada");
        assert_eq!(peer.label, "ada (agent: claude-code/2.0)");
        let bare = presence_label(
            presence_identity(Some("ada".to_string()), &Scope::Anonymous),
            None,
        )
        .expect("an account is enough");
        assert_eq!(bare.label, "ada (agent)");
    }

    /// **A local agent is "you" in the strip, never the owner's filing name.**
    ///
    /// A stdio session has no gate to resolve an account, so the identity it
    /// acts with is the machine owner's - the name that session's drafts are
    /// filed under. Drawn as it stands, the only person who can be reading
    /// that strip is told somebody called `owner` is in their document, which
    /// is themselves. The word for that is "you", and the harness still
    /// follows it so two agents of one person are told apart.
    ///
    /// **The substitution is made where the two sources are still apart.** An
    /// account the gate resolved that happens to be named `owner` is a remote
    /// person like anybody else and keeps their own name; only the local
    /// session with no gate at all is you.
    #[test]
    fn a_local_sessions_agent_is_you_in_the_owners_own_strip() {
        let peer = presence_label(
            presence_identity(None, &Scope::Unrestricted),
            display_client("claude-code/2.0"),
        )
        .expect("a local session acts as somebody");
        assert_eq!(peer.label, "you (agent: claude-code/2.0)");
        assert_eq!(
            peer.account,
            crate::engine::OWNER_IDENTITY_NAME,
            "while presence stays keyed by the identity the work is filed under"
        );
        let bare = presence_label(presence_identity(None, &Scope::Unrestricted), None)
            .expect("an identity is enough");
        assert_eq!(bare.label, "you (agent)");

        let remote = presence_label(
            presence_identity(
                Some(crate::engine::OWNER_IDENTITY_NAME.to_string()),
                &Scope::Anonymous,
            ),
            None,
        )
        .expect("an authenticated account is a peer");
        assert_eq!(
            remote.label,
            format!("{} (agent)", crate::engine::OWNER_IDENTITY_NAME),
            "an account the gate resolved is never you, whatever it is called"
        );
    }

    /// A join a server object opened ends when THAT OBJECT ends - and only
    /// when the object is what holds it.
    ///
    /// The pin under ruling I1's first half: a stdio process and a legacy MCP
    /// session are each one `McpServer` for their whole life, so the object
    /// going away is the holder ending. Nothing else is ended by it: the same
    /// account's browser session is a different holder with a key of its own,
    /// which is what keeps an agent's ending out of a person's window.
    #[test]
    fn a_server_objects_joins_end_with_it_when_it_is_the_holder() {
        let registry = Arc::new(crate::join::Joins::default());
        let agents = crate::join::Join {
            account: "bob".to_string(),
            holder: crate::join::Holder::McpSession("session-1".to_string()),
            domain: "team".to_string(),
            path: "fresh.md".to_string(),
            owner: "alice".to_string(),
            expires_at: None,
        };
        let browsers = crate::join::Join {
            holder: crate::join::Holder::Browser("csrf-bob".to_string()),
            ..agents.clone()
        };
        let agent_key = registry.open(agents.clone()).unwrap();
        let browser_key = registry.open(browsers).unwrap();
        assert_ne!(agent_key, browser_key, "two holders, two keys");

        let session = SessionJoins::new(registry.clone());
        session.remember(&agents, agent_key.clone());
        session.remember(&agents, agent_key.clone());
        assert_eq!(session.lock().len(), 1, "one key, however often remembered");

        drop(session);
        assert_eq!(
            registry.get(&agent_key, "bob"),
            None,
            "the session ended, so its join did"
        );
        assert!(
            registry.get(&browser_key, "bob").is_some(),
            "and the person's browser is still inside the draft it joined"
        );
        assert!(registry.holds(
            "bob",
            &crate::join::Holder::Browser("csrf-bob".to_string()),
            "team",
            "alice",
            "fresh.md"
        ));
    }

    /// A stateless peer's join is not ended by the request that opened it.
    ///
    /// The pin under ruling I1's second half. On the streamable-HTTP transport
    /// a modern-era peer gets a fresh server object per POST, so this object's
    /// `Drop` runs at the end of the very call that presented the link.
    /// Nothing is remembered for a token identity, so nothing is ended, and
    /// the registry is where the next request finds the join.
    #[test]
    fn a_stateless_peers_join_is_not_ended_by_the_request_that_opened_it() {
        let registry = Arc::new(crate::join::Joins::default());
        let join = crate::join::Join {
            account: "bob".to_string(),
            holder: crate::join::Holder::Token("bob".to_string()),
            domain: "team".to_string(),
            path: "fresh.md".to_string(),
            owner: "alice".to_string(),
            expires_at: None,
        };
        let key = registry.open(join.clone()).unwrap();

        // The POST that presented the link.
        let first = SessionJoins::new(registry.clone());
        first.remember(&join, key.clone());
        assert!(
            first.lock().is_empty(),
            "a token identity's join is not this object's to end"
        );
        drop(first);
        assert!(
            registry.get(&key, "bob").is_some(),
            "so the join outlives the request that opened it"
        );

        // The next POST, a different object, finds it in the registry.
        let second = SessionJoins::new(registry.clone());
        assert_eq!(
            registry.held_by(
                "bob",
                &crate::join::Holder::Token("bob".to_string()),
                "team"
            ),
            vec![join],
            "which is where a stateless peer's second request looks"
        );
        drop(second);
    }

    /// A join another holder is holding is not this object's, whatever account
    /// it belongs to.
    ///
    /// Said from the browser's side: bob's browser is inside alice's draft, so
    /// the registry holds a join for his account - and bob's agent, which
    /// authenticates as bob, opened nothing and is therefore inside nothing.
    #[test]
    fn another_holders_join_is_not_this_objects() {
        let registry = Arc::new(crate::join::Joins::default());
        let browser = crate::join::Holder::Browser("csrf-bob".to_string());
        let browser_key = registry
            .open(crate::join::Join {
                account: "bob".to_string(),
                holder: browser.clone(),
                domain: "team".to_string(),
                path: "fresh.md".to_string(),
                owner: "alice".to_string(),
                expires_at: None,
            })
            .unwrap();
        let agent = SessionJoins::new(registry.clone());
        assert!(
            registry
                .held_by(
                    "bob",
                    &crate::join::Holder::Token("bob".to_string()),
                    "team"
                )
                .is_empty(),
            "the agent's holder opened nothing, so it is inside nothing"
        );
        assert!(
            registry.holds("bob", &browser, "team", "alice", "fresh.md"),
            "while the window that joined it is inside it"
        );
        drop(agent);
        assert!(
            registry.get(&browser_key, "bob").is_some(),
            "and an agent ending leaves a join it never opened alone"
        );
    }

    /// **The join is the server's word, at any position and any multiplicity.**
    ///
    /// The rule this pins is structural: whatever a client calls itself, the
    /// half that reaches the composition cannot contain `-for-`, so the shape
    /// `<client>-for-<account>` on disk can only have been written by a server
    /// that resolved an account. The `x-for-for-ada` case is the one a textual
    /// deletion gets wrong - one non-overlapping pass consumes the first join
    /// and re-joins its neighbours into a second one.
    #[test]
    fn no_client_name_survives_carrying_the_join() {
        for (client, expected) in [
            // The straightforward attempt, and the layered one.
            ("x-for-ada", "x-ada"),
            ("x-for-for-ada", "x-ada"),
            ("x-for-for-for-ada", "x-ada"),
            // Case is not a hiding place, even though the server's own join is
            // always lowercase.
            ("x-FOR-ada", "x-ada"),
            ("x-For-ada", "x-ada"),
            // The join at either end is not a join, and goes all the same.
            ("for-ada", "ada"),
            ("x-for", "x"),
            // Empty runs collapse with the words that made them, so a doubled
            // separator cannot smuggle one back in either.
            ("x-for--ada", "x-ada"),
            ("x--for--ada", "x-ada"),
            // A name that is nothing but the word leaves nothing, which
            // `compose_actor` reads as no client at all.
            ("for", ""),
            ("for-for", ""),
            // `for` inside a word is a word, not the join, and is untouched.
            ("waiting-forever/1.0", "waiting-forever/1.0"),
            ("xfor-ada", "xfor-ada"),
            ("x-fora-ada", "x-fora-ada"),
            // The ordinary case pays nothing.
            ("claude-code/2.0", "claude-code/2.0"),
        ] {
            let stripped = without_the_join(client);
            assert_eq!(stripped, expected, "stripping {client}");
            assert!(
                !stripped.contains("-for-"),
                "no client half may carry the join: {client} -> {stripped}"
            );
        }
    }

    /// One `inputResponses` map holding `value` under the `confirm` key.
    fn responses(value: Value) -> Option<rmcp::model::InputResponses> {
        let mut map = rmcp::model::InputResponses::new();
        map.insert(CONFIRM_KEY.to_string(), value);
        Some(map)
    }

    /// The removal question, byte for byte, for every kind and every count.
    ///
    /// These are the sentences a person reads before a removal, and a client
    /// may show only the first line of them, so the action and its object come
    /// first and the meaning of the two buttons closes every one.
    #[test]
    fn the_removal_question_reads_the_same_for_every_kind_and_count() {
        let question = |kind: &str, engrams: Value, unknown: bool| {
            remove_domain_question(&json!({
                "domain": "kb",
                "kind": kind,
                "engrams": engrams,
                "engrams_unknown": unknown,
                "drafts": [],
                "drafts_unknown": false,
                "files_kept": kind != "virtual",
            }))
        };
        let cases = [
            (
                "file",
                json!(4),
                false,
                "Remove the domain 'kb' (4 engrams) from Crystalline? Its files stay on disk. \
                 Adding the folder again brings them back. Accept removes it. Decline keeps \
                 everything.",
            ),
            (
                "file",
                json!(1),
                false,
                "Remove the domain 'kb' (1 engram) from Crystalline? Its files stay on disk. \
                 Adding the folder again brings them back. Accept removes it. Decline keeps \
                 everything.",
            ),
            (
                "file",
                Value::Null,
                false,
                "Remove the domain 'kb' from Crystalline? Its files stay on disk. Adding the \
                 folder again brings them back. Accept removes it. Decline keeps everything.",
            ),
            (
                "file",
                json!(0),
                false,
                "Remove the domain 'kb' from Crystalline? Its files stay on disk. Adding the \
                 folder again brings them back. Accept removes it. Decline keeps everything.",
            ),
            (
                "file",
                Value::Null,
                true,
                "Remove the domain 'kb' (engram count unknown) from Crystalline? Its files stay \
                 on disk. Adding the folder again brings them back. Accept removes it. Decline \
                 keeps everything.",
            ),
            (
                "team",
                json!(3),
                false,
                "Remove the team domain 'kb' (3 engrams) from this machine? The local folder and \
                 the GitHub repository stay as they are. To use it again, add it with the \
                 repository. Accept removes it. Decline keeps everything.",
            ),
            (
                "team",
                Value::Null,
                true,
                "Remove the team domain 'kb' (engram count unknown) from this machine? The local \
                 folder and the GitHub repository stay as they are. To use it again, add it with \
                 the repository. Accept removes it. Decline keeps everything.",
            ),
            (
                "virtual",
                json!(2),
                false,
                "Delete the domain 'kb' and its 2 engrams? They live only in the database, so \
                 this cannot be undone. Accept deletes it. Decline keeps everything.",
            ),
            (
                "virtual",
                json!(1),
                false,
                "Delete the domain 'kb' and its 1 engram? It lives only in the database, so \
                 this cannot be undone. Accept deletes it. Decline keeps everything.",
            ),
            (
                "virtual",
                Value::Null,
                false,
                "Delete the domain 'kb' and its engrams? They live only in the database, so this \
                 cannot be undone. Accept deletes it. Decline keeps everything.",
            ),
            (
                "virtual",
                Value::Null,
                true,
                "Delete the domain 'kb' and its engrams (engram count unknown)? They live only in \
                 the database, so this cannot be undone. Accept deletes it. Decline keeps \
                 everything.",
            ),
        ];
        for (kind, engrams, unknown, expected) in cases {
            assert_eq!(
                question(kind, engrams.clone(), unknown),
                expected,
                "{kind} with {engrams} (unknown: {unknown})"
            );
        }
    }

    /// Private drafts are the half of a removal nobody can get back, so the
    /// question names them per actor, and says so when it could not read them.
    #[test]
    fn the_removal_question_names_the_private_drafts_it_would_end() {
        let with = remove_domain_question(&json!({
            "domain": "kb",
            "kind": "file",
            "engrams": 4,
            "drafts": [
                { "actor": "alice", "entries": 2 },
                { "actor": "bob", "entries": 1 },
            ],
            "drafts_unknown": false,
            "files_kept": true,
        }));
        assert_eq!(
            with,
            "Remove the domain 'kb' (4 engrams) from Crystalline? Its files stay on disk. Adding \
             the folder again brings them back. It also ends 3 private drafts (alice 2, bob 1). \
             They cannot be brought back. Accept removes it. Decline keeps everything."
        );

        let unknown = remove_domain_question(&json!({
            "domain": "kb",
            "kind": "virtual",
            "engrams": 2,
            "drafts": [],
            "drafts_unknown": true,
            "files_kept": false,
        }));
        assert_eq!(
            unknown,
            "Delete the domain 'kb' and its 2 engrams? They live only in the database, so this \
             cannot be undone. Whether anyone has private drafts here could not be read. Accept \
             deletes it. Decline keeps everything."
        );
    }

    /// The box is checked before anybody touches it, and its words say what
    /// Accept does with it: a client that honours `default` shows it ticked,
    /// so Accept alone goes ahead.
    #[test]
    fn the_confirm_question_checks_the_box_by_default() {
        let asked = serde_json::to_value(confirm_question("Go?".to_string())).unwrap();
        let schema = &asked["inputRequests"][CONFIRM_KEY]["params"]["requestedSchema"];
        assert_eq!(
            schema["properties"][CONFIRM_KEY],
            json!({
                "type": "boolean",
                "title": "Yes, go ahead",
                "description": "Leave this checked and choose Accept to go ahead.",
                "default": true,
            }),
            "{asked}"
        );
        assert_eq!(schema["required"], json!([CONFIRM_KEY]), "{asked}");
        assert_eq!(
            asked["inputRequests"][CONFIRM_KEY]["params"]["message"],
            json!("Go?"),
            "{asked}"
        );
    }

    /// An accept is a yes unless it says no: the box starts checked, so a
    /// client that sends the accept without the property it never changed has
    /// said yes. An accept carrying anything but `true` under the key, a
    /// decline, a cancel and every shape that is not an accept are no.
    #[test]
    fn confirmed_reads_an_accept_without_the_property_as_yes() {
        let yes = [
            json!({ "action": "accept", "content": { "confirm": true } }),
            // The property left out, which a client that honours the default
            // and sends only what changed does.
            json!({ "action": "accept" }),
            json!({ "action": "accept", "content": {} }),
            json!({ "action": "accept", "content": null }),
            json!({ "action": "accept", "content": { "confirmed": false } }),
        ];
        for value in yes {
            assert_eq!(
                confirmed(&responses(value.clone())),
                Some(true),
                "an accept that does not say no confirms: {value}"
            );
        }

        let no = [
            // An accept that carries the property and does not say true.
            json!({ "action": "accept", "content": { "confirm": false } }),
            json!({ "action": "accept", "content": { "confirm": null } }),
            json!({ "action": "accept", "content": { "confirm": "true" } }),
            json!({ "action": "accept", "content": { "confirm": 1 } }),
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
                "this answer is a no: {value}"
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

    /// The discard question, byte for byte: what each path gets, capped at
    /// ten, the review-mode clause, the guard and the two buttons.
    #[test]
    fn the_discard_question_reads_the_same_for_every_shape() {
        let changes: Vec<Value> = (0..12)
            .map(|i| {
                json!({
                    "path": format!("notes/{i}.md"),
                    "kind": match i % 3 {
                        0 => "added",
                        1 => "modified",
                        _ => "deleted",
                    },
                })
            })
            .collect();
        assert_eq!(
            discard_question("kb", &changes, false, false),
            "Undo 12 unshared changes in 'kb'? notes/0.md: your new file is deleted. \
             notes/1.md: the team's version comes back. notes/2.md: the deleted file comes back. \
             notes/3.md: your new file is deleted. notes/4.md: the team's version comes back. \
             notes/5.md: the deleted file comes back. notes/6.md: your new file is deleted. \
             notes/7.md: the team's version comes back. notes/8.md: the deleted file comes back. \
             notes/9.md: your new file is deleted. And 2 more. Nothing goes to GitHub. Edits \
             made after you looked are undone too. Accept undoes them. Decline keeps your \
             changes."
        );
        assert_eq!(
            discard_question("kb", &changes[1..2], true, false),
            "Undo 1 unshared change in 'kb'? notes/1.md: the team's version comes back. \
             Nothing goes to GitHub. A file edited after you looked is left as it is. Accept \
             undoes it. Decline keeps your change."
        );
        assert_eq!(
            discard_question("kb", &changes[..1], false, true),
            "Undo 1 unshared change in 'kb'? notes/0.md: your new file is deleted. Only your \
             own drafts are cleared. Nothing goes to GitHub. Edits made after you looked are \
             undone too. Accept undoes it. Decline keeps your change."
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
            // Carries a candidate so this loop's `assert_eq!(err.message,
            // message)` below actually exercises that the suggestion clause
            // reaches the MCP caller verbatim rather than being trimmed to
            // the refusal's first line.
            RemoteError::NotADomain {
                repo: "acme/brand-knowledge".to_string(),
                path: None,
                candidates: vec!["memory".to_string()],
                more_candidates: 0,
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
            // An organization policy refusal is nobody's server fault: the
            // token works, and the message names the GitHub page that clears
            // it. An internal-error verdict in front of that would tell the
            // caller to wait out a failure they are meant to go and fix.
            RemoteError::SsoAuthorizationRequired {
                org: "acme".to_string(),
                url: "https://github.com/orgs/acme/sso?authorization_request=abc".to_string(),
            },
            RemoteError::OauthAppRestricted {
                org: "acme".to_string(),
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
    fn is_collab_tool_recognizes_exactly_the_seven() {
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
    /// The three gates a spawned process can resolve to: the default (serve
    /// everything), a hook installed but unverified, and a verified harness.
    const GATES: [HarnessGate; 3] = [
        HarnessGate {
            hook_installed: false,
            onboarding_verified: false,
        },
        HarnessGate {
            hook_installed: true,
            onboarding_verified: false,
        },
        HarnessGate {
            hook_installed: true,
            onboarding_verified: true,
        },
    ];

    #[test]
    fn hidden_skills_surface_matches_the_locked_matrix() {
        let [serve, conditional, verified] = GATES;
        for gate in GATES {
            assert!(
                !hidden_skills_surface(SkillsServe::Always, gate),
                "true always serves, whoever spawned us"
            );
            assert!(
                hidden_skills_surface(SkillsServe::Never, gate),
                "false never serves, whoever spawned us"
            );
        }
        assert!(
            hidden_skills_surface(SkillsServe::Auto, verified),
            "auto plus a verified harness is the whole point of the feature"
        );
        assert!(
            !hidden_skills_surface(SkillsServe::Auto, conditional),
            "a hook that is installed but unverified keeps the skills served"
        );
        assert!(
            !hidden_skills_surface(SkillsServe::Auto, serve),
            "auto serves everyone else, which is every case we cannot resolve"
        );
    }

    /// Only `auto` plus an installed hook shrinks the instructions, to the
    /// minimal pointer for a verified harness and the conditional one for an
    /// unverified hook: `false` gates skill serving, never onboarding.
    #[test]
    fn short_instructions_are_auto_and_an_installed_hook_only() {
        use InstructionsVariant::{Conditional, Full, Minimal};
        let [serve, conditional, verified] = GATES;
        assert_eq!(instructions_variant(SkillsServe::Auto, verified), Minimal);
        assert_eq!(
            instructions_variant(SkillsServe::Auto, conditional),
            Conditional
        );
        assert_eq!(instructions_variant(SkillsServe::Auto, serve), Full);
        for gate in GATES {
            assert_eq!(instructions_variant(SkillsServe::Always, gate), Full);
            assert_eq!(
                instructions_variant(SkillsServe::Never, gate),
                Full,
                "turning the skill surface off must not cost a client its routing block"
            );
        }
    }

    /// The two gates take the same two inputs, so the surface and the
    /// instructions can never disagree about whether this deployment is
    /// already onboarded. They diverged once, when one keyed on the client's
    /// name and the other did not, and that divergence is what SEP-2567
    /// forbade.
    ///
    /// Since the gate became two facts the rule is no longer "hidden exactly
    /// when the instructions are short": a hook that is installed but
    /// unverified gets the short conditional instructions and keeps the
    /// surface. Under `auto` the surface is hidden exactly when the variant
    /// is `Minimal`, so `Conditional` never hides it.
    #[test]
    fn the_surface_and_the_instructions_read_the_same_answer() {
        for gate in GATES {
            assert_eq!(
                hidden_skills_surface(SkillsServe::Auto, gate),
                instructions_variant(SkillsServe::Auto, gate) == InstructionsVariant::Minimal,
                "auto decides both together for {gate:?}"
            );
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

    /// Who shared a layer is worth a line only when there is somebody to name.
    ///
    /// A mixed-author chain is what the key exists for, so it survives the
    /// trim; a null one says nothing a caller can act on, so it is dropped
    /// rather than spent - the same budget rule the quiet stack keys follow.
    #[test]
    fn lean_origin_status_keeps_an_author_and_drops_a_null_one() {
        let leaned = lean_origin_status(json!({
            "domains": [{
                "domain": "kb",
                "open_proposals": [
                    {
                        "number": 7,
                        "status": "Open",
                        "feedback": [],
                        "author_login": "alice",
                    },
                    { "number": 8, "status": "Open", "feedback": [], "author_login": null },
                    // A record from before proposals named their author at all.
                    { "number": 9, "status": "Open", "feedback": [] },
                ],
            }],
        }));

        let open = &leaned["domains"][0]["open_proposals"];
        assert_eq!(open[0]["author_login"], "alice");
        for index in [1, 2] {
            assert!(
                open[index].get("author_login").is_none(),
                "an unnamed author costs no key: {}",
                open[index]
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

    /// A kept branch is a fact a caller relays, so it stays; an empty list
    /// says nothing and goes, like the quiet stack keys.
    #[test]
    fn lean_origin_status_keeps_a_kept_branch_and_drops_an_empty_list() {
        let leaned = lean_origin_status(json!({
            "domains": [{
                "domain": "kb",
                "kept_branches": [{ "branch": "crystalline/share-1", "message": "Branch crystalline/share-1 is kept: ..." }],
            }, {
                "domain": "quiet",
                "kept_branches": [],
            }],
        }));
        assert_eq!(
            leaned["domains"][0]["kept_branches"][0]["branch"],
            json!("crystalline/share-1")
        );
        assert!(
            leaned["domains"][1]
                .as_object()
                .unwrap()
                .get("kept_branches")
                .is_none(),
            "{leaned}"
        );
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
    /// the six whatever the mode is and never hides `configure`; on top of
    /// that read-only hides the write set, so an enabled read-only instance
    /// shows the two collaboration tools it still exempts and nothing else.
    #[test]
    fn hidden_collab_tool_matches_the_locked_matrix() {
        // github off: the six are hidden whatever the mode is.
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

        // github on, writable: all seven.
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
            "discard_changes",
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

    /// The refusal a non-admin agent reads names every class of change it
    /// gates, provisioning included: a message that names two of three leaves
    /// somebody refused on `provision apply` reading a sentence about
    /// something else.
    #[test]
    fn the_instance_admin_refusal_names_provisioning_too() {
        assert!(
            INSTANCE_ADMIN_ONLY.contains("domains registered on it"),
            "{INSTANCE_ADMIN_ONLY}"
        );
        assert!(
            INSTANCE_ADMIN_ONLY.contains("settings"),
            "{INSTANCE_ADMIN_ONLY}"
        );
        assert!(
            INSTANCE_ADMIN_ONLY.contains("provision"),
            "the third class is the one the message forgot: {INSTANCE_ADMIN_ONLY}"
        );
    }
    #[test]
    fn the_configure_hint_rides_only_a_root_manifest_while_configure_is_listed() {
        let manifest = || json!({ "permalink": "manifest", "path": "MANIFEST.md" });
        let mut listed = manifest();
        attach_configure_hint(&mut listed, true);
        assert_eq!(listed["configure"], CONFIGURE_MANIFEST_HINT);
        let mut hidden = manifest();
        attach_configure_hint(&mut hidden, false);
        assert!(hidden.get("configure").is_none());
        let mut nested = json!({ "permalink": "manifest", "path": "notes/MANIFEST.md" });
        attach_configure_hint(&mut nested, true);
        assert!(nested.get("configure").is_none(), "only the domain root's");
    }
}
