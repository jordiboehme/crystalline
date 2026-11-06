//! The in-memory session registry and the per-session document room. The file
//! stays the source of truth: a session is a live LF-space view of it, and
//! everything durable flows back through the engine (Tasks 6-7).
//!
//! **A room is one DOCUMENT, and in a domain that reviews changes a document
//! belongs to somebody.** The registry key carries that third component: the
//! domain, the permalink, and the overlay owner whose draft of it this room is
//! (`None` for the document a direct domain keeps, which is the folder's own
//! and everybody's). Two authors drafting one page are two rooms, each opening
//! on its own author's text and saving into that author's draft row - never
//! into the folder the team reviewed and never into somebody else's overlay.
//! Who may open a room over whose document is decided at the door, in
//! `super::ws::join`: your own needs nothing, and somebody else's needs the
//! share-link their author minted plus the join this session opened on it.
//!
//! **One room's awareness ceiling is 32 names, not [`MAX_PARTICIPANTS`]'s 16.**
//! A connection slot (`state.conns`) and an agent presence slot
//! (`state.agents`) are two separate budgets, each capped at
//! [`MAX_PARTICIPANTS`], and both publish into the same room's awareness map,
//! which `participants` (`CollabSessions::participants`) and the header strip
//! (`fluid/src/collab/PresenceChips.tsx`) render in full, unbounded by either
//! cap on its own. The 16-and-16 split follows from the ruling that set the
//! agent cap to mirror the connection cap rather than to share its budget, so
//! a room at both ceilings shows 32 names for as long as it takes one to
//! leave. Recorded here so it is a known number rather than a rediscovery.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Instant;

use bytes::Bytes;
use tokio::sync::{Mutex, broadcast};
use yrs::sync::awareness::AwarenessUpdateEntry;
use yrs::sync::{Awareness, AwarenessUpdate, Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, GetString, Options, ReadTxn, Text, Transact, Update};

use super::control::{self, Control};
use super::merge::{self, MergeOutcome};
use crate::domain_view::DomainView;
use crate::engine::{Engine, EngineError, EngramText, SectionAuthors};
use crystalline_core::to_lf;

/// The address a room reads and saves its engram at: the `crystalline://`
/// URL, the one absolute form, which names the room's domain and permalink
/// together, so the address a room reads and saves at is its own key. A miss
/// at it names the bare permalink, as before the room used the URL.
fn room_address(domain: &str, permalink: &str) -> String {
    format!("{}{domain}/{permalink}", crystalline_core::address::SCHEME)
}

/// The name of the one shared Y.Text every session document carries. The
/// client binds the same name, so the two agree without negotiation.
pub const TEXT_NAME: &str = "content";
/// The most documents one daemon keeps open at a time.
pub const MAX_SESSIONS: usize = 64;
/// The most connections one document accepts.
pub const MAX_PARTICIPANTS: usize = 16;
/// Fan-out queue depth per connection; a receiver this far behind is closed
/// by the socket loop and reconnects rather than silently losing frames.
const BROADCAST_CAPACITY: usize = 256;
/// How long a pause in typing lands the save after.
pub const SAVE_DEBOUNCE_MS: u64 = 2_000;
/// The longest continuous typing goes without a save landing.
pub const SAVE_MAX_LAG_MS: u64 = 15_000;
/// How long a session sits idle before the external-change probe runs.
pub const IDLE_CHECK_MS: u64 = 10_000;
/// How often the per-session saver wakes up to ask whether anything is due.
const SAVER_TICK_MS: u64 = 250;
/// What a room answers when its text would switch `read_only` on. The saver
/// writes as the machine owner and cannot name who typed the line, so it
/// lands no switch-on for anybody (decision D10); the policies card, which
/// knows the caller, is the way.
const ROOM_SWITCH_ON_REFUSAL: &str =
    "a co-editing room does not switch read_only on; the owner switches it in the domain policies";
/// How long an agent stands in the participant strip after its last action.
///
/// An agent holds no socket, so nothing tells the room when it has finished:
/// there is no disconnect to hear. The slot is a claim with an expiry on it
/// instead, refreshed by every read and every write the agent makes in this
/// document and swept by the saver's own pass once a minute of silence has
/// gone by. Long enough that a person watching a chip does not see it blink
/// between two calls of one piece of work, short enough that a strip is never
/// a list of agents that left.
pub const AGENT_PRESENCE_TTL_MS: u64 = 60_000;

/// One connection's identity inside a session, minted at join.
pub type ConnId = u64;

/// One broadcast frame: protocol bytes, the connection they came from and,
/// for the one frame that is not everybody's business, the connection they are
/// for.
#[derive(Clone)]
pub struct Frame {
    /// The connection an update came from, so the socket loop can skip
    /// echoing it back; None for server-originated frames (merge edits,
    /// control broadcasts), which everyone gets.
    pub from: Option<ConnId>,
    /// The connection this frame is FOR, when it is for one of them. `None` on
    /// every ordinary frame, which the whole room hears. `Some(conn)` is the
    /// eviction of a session whose join into this draft has ended: it closes
    /// that socket and no other, because the room belongs to its owner and
    /// they are still in it.
    pub to: Option<ConnId>,
    /// The encoded y-protocol messages to send.
    pub bytes: Bytes,
}

/// An open room's text and the checksum that stands for it: the version
/// token a read over the room hands out and a guarded write over it accepts.
#[derive(Debug, Clone)]
pub struct LiveText {
    /// The room's text, LF only.
    pub text: String,
    /// The checksum of the stored bytes while the room is clean (its text is
    /// what it loaded or last saved), so a guard taken before the room opened
    /// or after it closes still holds even when the file is CRLF; the
    /// checksum of `text` once anything was typed.
    pub checksum: String,
    /// Whether the room will save what it holds: its save state is ok and its
    /// final save has not run yet. A room in conflict, with a failed save, or
    /// whose final save ran, holds text that may never reach the file. Read
    /// at the moment of the look; [`CollabSessions::compose_while_saving`]
    /// decides it again under the compose's own lock.
    pub saving: bool,
    /// The checksum of the stored bytes the room last loaded or saved: the
    /// file or row its text stands over. `checksum` equals it while the room
    /// is clean. A token for the room's text says nothing about a file that
    /// has moved past this base since.
    pub base_checksum: String,
    /// Whether the room is open with a conflict or a failed save of its own:
    /// it holds text a person typed that cannot be saved until they settle
    /// it. False for a saving room, and for one that has closed or whose
    /// final save ran, which nobody is in any more.
    pub waiting: bool,
}

/// Why a join did not produce a session.
#[derive(Debug)]
pub enum JoinError {
    /// The engram could not be read.
    Engine(EngineError),
    /// This daemon already holds [`MAX_SESSIONS`] documents.
    ServerFull,
    /// This document already holds [`MAX_PARTICIPANTS`] connections.
    SessionFull,
    /// The document is not here any more because a move took it to the
    /// address named, a short while ago ([`MOVED_MEMORY`]). The co-editing
    /// surface answers it with the `moved` close a room open at the time of
    /// the move got, so an editor that was still joining lands on the same
    /// notice.
    Moved(MovedTo),
}

/// How long the registry remembers where a moved document went, for a join
/// that comes after the move ([`JoinError::Moved`]).
pub const MOVED_MEMORY: std::time::Duration = std::time::Duration::from_secs(600);

/// The words a join is refused with while a move holds the document. A
/// surface answers it with 409: the request is fine, the document is busy.
pub const BEING_MOVED: &str = "is being moved; open it again at its new place in a moment";

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinError::Engine(err) => write!(f, "{err}"),
            JoinError::ServerFull => write!(f, "too many documents are open for co-editing"),
            JoinError::SessionFull => write!(f, "too many people are editing this engram"),
            JoinError::Moved(to) => write!(f, "this document was moved to '{}'", to.path),
        }
    }
}

impl std::error::Error for JoinError {}

/// A joined connection: the room, its identity in it, its fan-out receiver and
/// the greeting to send first.
pub struct Joined {
    /// The room this connection joined.
    pub session: Arc<CollabSession>,
    /// This connection's id inside the room.
    pub conn: ConnId,
    /// The fan-out receiver; frames tagged with this conn are its own echo.
    pub rx: broadcast::Receiver<Frame>,
    /// hello control + SyncStep1 + full awareness, ready to send as one frame.
    pub greeting: Vec<u8>,
}

/// Terse by hand: the room behind a [`Joined`] holds a yrs document and a
/// mutex, neither of which belongs in a log line or a failed `unwrap_err`.
impl std::fmt::Debug for Joined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Joined")
            .field("epoch", &self.session.epoch())
            .field("conn", &self.conn)
            .field("greeting_len", &self.greeting.len())
            .finish()
    }
}

/// What an agent's write into a live document did.
///
/// One field, and it is the one an agent cannot work out for itself: a write
/// that composed into somebody's open editor is a write somebody is about to
/// see land under their cursor, and the receipt says who that is so the agent
/// can say so too.
pub struct LiveApplied {
    /// Who is in the room the text landed in, by the name their client
    /// publishes in awareness. Sorted and de-duplicated, so one person in two
    /// windows is one name and the receipt does not reorder between calls.
    ///
    /// The writing agent's own slot is not in it: this answers "who is in
    /// there with me", and another agent working in the same document is.
    pub participants: Vec<String>,
}

/// What a room did with a change written to its file
/// ([`CollabSessions::absorb_doc_change`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absorbed {
    /// The change is in the room's text: merged into it, or already there
    /// because the room saved it in between.
    Merged,
    /// The change collides with text in the room nobody has saved: the room
    /// raised its edit conflict, and a person chooses.
    Conflict,
    /// The room has a conflict or a failed save of its own, or the page is
    /// gone: it meets the change when that is resolved.
    Deferred,
}

/// [`Absorbed`] and who was in the room.
#[derive(Debug, Clone)]
pub struct AbsorbOutcome {
    pub absorbed: Absorbed,
    /// Who is in the room, as [`LiveApplied::participants`] names them.
    pub participants: Vec<String>,
}

/// Why [`CollabSessions::compose_while_saving`] did not compose. Either way
/// nothing reached the room, and the caller writes the file or the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeRefused {
    /// No room is open over the document any more, it closed, or it will
    /// not save what it is given: its save is in conflict or failed, or its
    /// final save has run. Text composed into it now could be lost without a
    /// trace.
    NotSaving,
    /// The room's text is no longer the text the target was computed from:
    /// somebody typed in between. Composing would undo their typing.
    Moved,
}

/// An agent in a room, as the room shows it.
///
/// Two halves because presence is per (account, label) rather than per
/// connection: the account is who the agent is working for, and the label is
/// the name a person reads in the strip. One account working through two
/// harnesses is two peers, which is the true thing to draw - they are two
/// agents - and the same harness calling ten times is one.
///
/// The label is composed where the two halves are known, in `crate::mcp`, and
/// it is for display alone: what a write records as its provenance is the
/// hyphenated OKF actor and is untouched by anything here.
#[derive(Clone, Debug)]
pub struct AgentPeer {
    /// The account the call authenticated as, or the identity a local session
    /// acts with. Half the presence key.
    pub account: String,
    /// The name the participant strip shows, `<account> (agent)` or
    /// `<account> (agent: <client>)`.
    pub label: String,
}

/// One agent's standing claim on a slot in this room.
struct AgentSlot {
    /// The awareness client id it publishes under, minted from its key so the
    /// same agent reclaims the same slot after a sweep.
    id: ClientID,
    /// When it last did something here; the TTL is measured from this.
    touched: Instant,
}

/// The awareness client id one agent peer publishes under.
///
/// Derived from the key rather than minted from a counter, so the slot an
/// agent reclaims after a TTL sweep is the slot it had before and a room that
/// has seen it twice holds one chip. 53 bits because that is what a yjs client
/// id is (`ClientID::new` asserts it), and the space is wide enough that a
/// collision with a browser's random id is not a thing to design against -
/// except for the one id in it that is already spoken for, the room document's
/// own, which is stepped over rather than shared.
fn agent_client_id(account: &str, label: &str, avoid: ClientID) -> ClientID {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    const BITS: u64 = (1 << 53) - 1;
    let mut hash = OFFSET;
    for byte in account
        .as_bytes()
        .iter()
        .chain(std::iter::once(&0u8))
        .chain(label.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    let id = ClientID::new(hash & BITS);
    if id == avoid {
        return ClientID::new((id.get() + 1) & BITS);
    }
    id
}

/// What an agent publishes about itself: the name to draw and the one flag
/// that makes the strip draw it as an agent rather than as a person.
///
/// No color: the room's own palette is keyed by the name on every client
/// already, so an agent gets a chip color the same way everybody else does and
/// there is no second palette to keep in step. Built through `serde_json` and
/// never by formatting, because the label carries a client-supplied half.
fn agent_state_json(label: &str) -> String {
    serde_json::json!({ "user": { "name": label, "agent": true } }).to_string()
}

/// What a room is a room over. A closed enum: a room over one kind of
/// document can never answer a lookup for another kind, whatever its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RoomDoc {
    /// An engram, by the permalink it answers to.
    Engram(String),
    /// A reflection page, by its path inside `reflections/`. It saves
    /// straight to its file, so it has no overlay and is never re-keyed.
    Reflection(String),
}

impl RoomDoc {
    /// The kind of document the room is over.
    pub fn kind(&self) -> crystalline_core::ContentKind {
        match self {
            RoomDoc::Engram(_) => crystalline_core::ContentKind::Engram,
            RoomDoc::Reflection(_) => crystalline_core::ContentKind::Reflection,
        }
    }

    /// The name the room's controls carry: an engram's permalink, or a
    /// reflection's path inside `reflections/`.
    pub fn name(&self) -> &str {
        match self {
            RoomDoc::Engram(permalink) => permalink,
            RoomDoc::Reflection(path) => path,
        }
    }
}

/// Where one room is filed: the domain, the document it is over, and the
/// overlay owner whose document it is.
///
/// `None` in the third slot is the document a domain that takes changes
/// directly keeps - the folder's own text, which is what every room was before
/// drafts existed. `Some(actor)` is that actor's draft of the page, which is
/// the only kind of document a reviewing domain has.
pub type RoomKey = (String, RoomDoc, Option<String>);

/// The registry of open documents, keyed by [`RoomKey`].
pub struct CollabSessions {
    engine: Arc<Engine>,
    sessions: Mutex<HashMap<RoomKey, Arc<CollabSession>>>,
    /// The documents a move is moving right now, each with how many moves
    /// hold it: a join over one of them is refused until the move is done
    /// (see [`CollabSessions::close_for_move`]). A std mutex, read and
    /// written only under the registry lock or in a fence's drop, and never
    /// held across an await.
    moving: std::sync::Mutex<HashMap<RoomKey, usize>>,
    /// Where documents moved by a move of the last [`MOVED_MEMORY`] went,
    /// by the address they left, for a join that comes after the move.
    moved: std::sync::Mutex<HashMap<RoomKey, (MovedTo, Instant)>>,
    next_conn: AtomicU64,
    next_epoch: AtomicU64,
}

/// Where a moved document went, as the `moved` close names it to the room's
/// sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedTo {
    /// The domain it lives in now, when the move changed it.
    pub domain: Option<String>,
    /// Its new path: inside `reflections/` for a page, inside the domain for
    /// an engram.
    pub path: String,
    /// The permalink an engram answers to now; `None` for a page.
    pub permalink: Option<String>,
}

impl MovedTo {
    /// The `Closed { reason: "moved" }` control naming this address.
    pub fn control(&self) -> Control {
        Control::Closed {
            reason: "moved".to_string(),
            domain: self.domain.clone(),
            path: Some(self.path.clone()),
            permalink: self.permalink.clone(),
        }
    }
}

/// A move's hold on one document: while it stands, no room opens over the
/// document (a join is refused with a conflict). Taken by
/// [`CollabSessions::close_for_move`] and lifted on drop, which every way
/// out of a move reaches, an error included.
#[must_use = "the fence lifts when it is dropped"]
pub struct MoveFence {
    registry: Weak<CollabSessions>,
    key: RoomKey,
}

impl Drop for MoveFence {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut moving = registry
            .moving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = moving.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                moving.remove(&self.key);
            }
        }
    }
}

/// Rooms that were saved and ended but whose sockets are not told yet: a
/// domain rename's sweep, or the room a move closed. The caller tells them
/// where the document went once the move or the rename is done
/// ([`SweptRooms::moved_to_domain`], [`MoveClose::moved`]); dropped untold,
/// which a refused, stopped or cancelled move or rename does, they are told
/// `internal`, as an unregistration tells them.
#[must_use = "dropped untold, the rooms are told `internal`"]
pub struct SweptRooms {
    rooms: Vec<Arc<CollabSession>>,
}

impl SweptRooms {
    /// How many rooms the sweep ended.
    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    /// Whether the sweep ended no room.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Tell every swept room's sockets that the document moved with its
    /// domain to `domain`: the same path, the same permalink, the new name.
    pub async fn moved_to_domain(mut self, domain: &str) {
        for session in std::mem::take(&mut self.rooms) {
            let (path, permalink) = session.address().await;
            let to = MovedTo {
                domain: Some(domain.to_string()),
                path,
                permalink,
            };
            session.close_moved(&to).await;
        }
    }
}

impl Drop for SweptRooms {
    fn drop(&mut self) {
        for session in self.rooms.drain(..) {
            session.close_with(Control::closed("internal"));
        }
    }
}

/// A move's hold on the document it moves ([`CollabSessions::close_for_move`]):
/// the room over it, saved and ended but not told yet, and the fence that
/// keeps a new room from opening. The move calls [`MoveClose::moved`] once it
/// has committed; on any other way out the guard drops, the room is told
/// `internal` (the document did not move) and the fence lifts.
#[must_use = "dropped without `moved`, the room is told `internal`"]
pub struct MoveClose {
    // Field order is drop order: the room is told before the fence lifts.
    room: SweptRooms,
    to: MovedTo,
    _fence: MoveFence,
}

/// Whether `scope` may see where `to` points: a move inside one domain names
/// nothing new; a move into another domain names that domain, which `scope`
/// must be able to see. A failed lookup answers no (fail closed).
async fn sees_destination(engine: &Engine, to: &MovedTo, scope: &crate::scope::Scope) -> bool {
    match to.domain.as_deref() {
        None => true,
        Some(domain) => engine
            .hidden_for(scope)
            .await
            .is_ok_and(|hidden| !hidden.contains(domain)),
    }
}

/// The saves and closes of a sweep, as a task of its own (see
/// [`CollabSessions::sweep_domain_rooms`]). Boxed as a `Send` future so the
/// task's type is named here and nowhere else: a room's last save can reach
/// a sweep again (a MANIFEST that switches the Reflection off closes the
/// reflection rooms), and an unnamed type would make that cycle one the
/// compiler cannot prove `Send` through.
fn sweep_task(
    victims: Vec<Arc<CollabSession>>,
    discarded: HashSet<String>,
    reason: Option<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = SweptRooms> + Send>> {
    Box::pin(async move {
        let mut untold = SweptRooms { rooms: Vec::new() };
        // Every victim not reached yet waits in a `SweptRooms` of its own,
        // so a panic below (a yrs bug in a save) still disposes and tells
        // each of them `internal` as the unwind drops it.
        let mut waiting = SweptRooms { rooms: victims };
        waiting.rooms.reverse();
        while let Some(session) = waiting.rooms.pop() {
            // The one in hand is guarded the same way until it is done.
            let mut current = SweptRooms {
                rooms: vec![session.clone()],
            };
            let discarding = session
                .key()
                .2
                .is_some_and(|owner| discarded.contains(&owner));
            // The save comes FIRST: poison disposes the session, and a
            // disposed session's save paths are all no-ops. Which is exactly
            // how a discarded actor's room is closed without one.
            if !discarding {
                session.final_save().await;
            }
            // Done without a panic: disarm the guard, the close below tells.
            current.rooms.clear();
            match &reason {
                Some(reason) => session.close(reason).await,
                None => {
                    session.dispose();
                    untold.rooms.push(session);
                }
            }
        }
        untold
    })
}

impl MoveClose {
    /// The move committed: tell the room's sockets the new address, then
    /// lift the fence.
    pub async fn moved(mut self) {
        for session in std::mem::take(&mut self.room.rooms) {
            session.close_moved(&self.to).await;
        }
        if let Some(registry) = self._fence.registry.upgrade() {
            registry.note_moved(self._fence.key.clone(), self.to.clone());
        }
    }
}

impl CollabSessions {
    /// Build an empty registry over the engine sessions read and write through.
    pub fn new(engine: Arc<Engine>) -> Arc<CollabSessions> {
        Arc::new(CollabSessions {
            engine,
            sessions: Mutex::new(HashMap::new()),
            moving: std::sync::Mutex::new(HashMap::new()),
            moved: std::sync::Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            next_epoch: AtomicU64::new(1),
        })
    }

    /// Join a document, opening it from the file on the first join.
    ///
    /// Takes `&Arc<Self>` because an opened session keeps a [`Weak`] handle
    /// back to the registry: a frontmatter rename has to move its key, and the
    /// session is the only one who learns about the rename (from the save
    /// receipt).
    ///
    /// `overlay` is whose document to open: `None` for the one a direct domain
    /// keeps, `Some(actor)` for that actor's draft of the page. It is the
    /// caller's job to have decided that the caller may be in that document -
    /// see `super::ws::join`, which is the one surface that opens rooms.
    ///
    /// The connection is the machine owner's (an in-process caller); the
    /// upgrade route names the signed-in caller with
    /// [`CollabSessions::join_as`].
    pub async fn join(
        self: &Arc<Self>,
        domain: &str,
        permalink: &str,
        overlay: Option<&str>,
    ) -> Result<Joined, JoinError> {
        self.join_as(
            domain,
            permalink,
            overlay,
            crate::scope::Scope::Unrestricted,
        )
        .await
    }

    /// [`CollabSessions::join`] for the connection of `viewer`, as the
    /// surface that opened it authenticated it: a `moved` close names to it
    /// only what `viewer` may see.
    pub async fn join_as(
        self: &Arc<Self>,
        domain: &str,
        permalink: &str,
        overlay: Option<&str>,
        viewer: crate::scope::Scope,
    ) -> Result<Joined, JoinError> {
        self.join_doc(
            domain,
            RoomDoc::Engram(permalink.to_string()),
            overlay,
            None,
            Some(viewer),
        )
        .await
    }

    /// Join the room over a reflection page, opening it from the file on the
    /// first join. A read-only domain opens none. The caller decided that
    /// the caller may edit the page (`super::ws::join_reflection`).
    ///
    /// `author` is the signed-in account the connection belongs to, as the
    /// surface that opened it authenticated it. A save records it in the
    /// page's bookkeeping when this connection sent an edit since the save
    /// before; what a client puts into awareness is for display only.
    ///
    /// The connection is the machine owner's, as on [`CollabSessions::join`];
    /// the upgrade route uses [`CollabSessions::join_reflection_as`].
    pub async fn join_reflection(
        self: &Arc<Self>,
        domain: &str,
        path: &str,
        author: &str,
    ) -> Result<Joined, JoinError> {
        self.join_reflection_as(domain, path, author, crate::scope::Scope::Unrestricted)
            .await
    }

    /// [`CollabSessions::join_reflection`] for the connection of `viewer`,
    /// as on [`CollabSessions::join_as`].
    pub async fn join_reflection_as(
        self: &Arc<Self>,
        domain: &str,
        path: &str,
        author: &str,
        viewer: crate::scope::Scope,
    ) -> Result<Joined, JoinError> {
        crystalline_core::reflection::validate_reflection_path(path)
            .map_err(|e| JoinError::Engine(EngineError::Invalid(e)))?;
        self.engine
            .refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await
            .map_err(JoinError::Engine)?;
        self.join_doc(
            domain,
            RoomDoc::Reflection(path.to_string()),
            None,
            Some(author),
            Some(viewer),
        )
        .await
    }

    /// [`CollabSessions::join`] for a room over any kind of document.
    /// `author` is the account the connection belongs to, where the room
    /// records who changed what (a reflection room); `None` elsewhere.
    /// `viewer` is who the connection is, recorded with the connection;
    /// `None` is somebody unknown, who a `moved` close names no address.
    pub async fn join_doc(
        self: &Arc<Self>,
        domain: &str,
        doc: RoomDoc,
        overlay: Option<&str>,
        author: Option<&str>,
        viewer: Option<crate::scope::Scope>,
    ) -> Result<Joined, JoinError> {
        let key = (domain.to_string(), doc, overlay.map(str::to_string));
        // The registry lock is held across open AND the membership check, so a
        // stampede of joins can neither open the same document twice nor
        // overshoot MAX_PARTICIPANTS between the check and the add.
        let mut sessions = self.sessions.lock().await;
        // A move of this document is under way: its room was saved and
        // closed, and a room opened now would load the old text and save it
        // to an address the move is taking away. Checked under the registry
        // lock, the one the move's fence is set under, so a join is either
        // in the map before the fence (and closed by the move) or refused.
        if self.is_moving(&key) {
            return Err(JoinError::Engine(EngineError::Conflict(format!(
                "'{}' {BEING_MOVED}",
                key.1.name()
            ))));
        }
        // A disposed session still sitting in the map is a corpse: its saver
        // loop has ended, so a join that adopted it would edit a room nothing
        // ever writes back. Treated as absent and replaced.
        let live = sessions
            .get(&key)
            .filter(|session| !session.is_disposed())
            .cloned();
        let (session, fresh) = match live {
            Some(existing) => (existing, false),
            None => {
                // Replacing a corpse under this key does not grow the map, so
                // capacity is only in question when the key is new.
                if !sessions.contains_key(&key) && sessions.len() >= MAX_SESSIONS {
                    return Err(JoinError::ServerFull);
                }
                let opened = match CollabSession::open(
                    self.engine.clone(),
                    key.clone(),
                    self.mint_epoch(),
                    Arc::downgrade(self),
                )
                .await
                {
                    Ok(opened) => opened,
                    // Nothing at this address any more: a move that took it
                    // away a short while ago answers where it went.
                    Err(JoinError::Engine(EngineError::NotFound(miss))) => {
                        return Err(match self.moved_from(&key) {
                            Some(to) => JoinError::Moved(to),
                            None => JoinError::Engine(EngineError::NotFound(miss)),
                        });
                    }
                    Err(err) => return Err(err),
                };
                sessions.insert(key.clone(), opened.clone());
                tokio::spawn(run_saver(opened.clone()));
                (opened, true)
            }
        };
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        match session.add_conn(conn, author, viewer).await {
            Ok((rx, greeting)) => Ok(Joined {
                session,
                conn,
                rx,
                greeting,
            }),
            Err(err) => {
                // A document opened for this join that then refused it would
                // otherwise sit in the registry with nobody in it, its saver
                // ticking over a room no one will ever edit.
                if fresh {
                    session.dispose();
                    sessions.remove(&key);
                }
                Err(err)
            }
        }
    }

    /// Drop the registry entry once a session reports empty. Split from
    /// [`CollabSession::remove_conn`] so the final save can run between the
    /// two.
    ///
    /// Takes the session rather than its address on purpose: a frontmatter
    /// rename moves the registry key mid-life, so the (domain, permalink) a
    /// socket joined under is not always the one the room is filed under when
    /// that socket closes. The route holds the [`Joined`] handle, so passing
    /// the session is also the simpler call.
    pub async fn dispose_if_empty(&self, session: &CollabSession) {
        // The key is read UNDER the registry lock: a rename between reading it
        // and taking the lock would look up the old permalink, find nothing of
        // its own there and leave the emptied room in the map forever.
        let mut sessions = self.sessions.lock().await;
        let key = session.key();
        // Identity by epoch: a room opened under this key while the last
        // socket of the previous one was closing must not be disposed by its
        // predecessor's teardown.
        if sessions
            .get(&key)
            .is_none_or(|held| held.epoch() != session.epoch())
        {
            return;
        }
        // A poisoned session counts as gone whether or not sockets still hang
        // off it: it saves nothing and its saver has stopped, so it must not
        // survive in the registry for a later join to find.
        if session.is_disposed() || session.is_empty().await {
            // Ends the saver loop, and makes every later save path a no-op.
            session.dispose();
            sessions.remove(&key);
        }
    }

    /// Close every open session of `domain`: the domain is being
    /// unregistered, so each room saves its text first (the files stay on
    /// disk; in-flight edits must not be lost) and is then poisoned so its
    /// participants get a Closed control and drop cleanly. The registry lock
    /// is held only to collect and remove the victims - never across a save
    /// (lock order: registry then session, and an engine save can be slow).
    /// Returns how many rooms were closed.
    ///
    /// A room removed here is gone from the registry before it is saved, so
    /// the two entry-moving paths leave it alone from that moment on:
    /// `CollabSessions::rekey` and [`CollabSessions::dispose_if_empty`] both
    /// look the session up by key and find nothing of theirs, so a rename that
    /// lands during the sweep cannot re-file a swept room.
    ///
    /// The sweep closes what is open, it does not lock the domain out: a join
    /// that takes the registry lock after the victims were collected opens a
    /// fresh room. So the caller sweeps FIRST and unregisters second, behind a
    /// fence of its own that refuses new joins for the closing domain (an admin
    /// gate in the REST state, or a registry-level fence - the route's choice).
    /// The order is not free to invert: unregistering first would strand every
    /// save this verb promises, because the engine can no longer resolve the
    /// engram (the write is refused outright, and inside the window between the
    /// config write and the index clear it would resolve as virtual and land in
    /// the DATABASE rather than in the file that `files_kept` deliberately left
    /// on disk). Note that the fence is what closes the join window, not luck:
    /// [`CollabSessions::join`] holds the registry lock ACROSS
    /// `CollabSession::open` (the engine read), so a join either inserts
    /// before the sweep collects and is swept, or arrives after and is refused
    /// by the fence - no join is in flight across the sweep. If that lock is
    /// ever released around the open, this argument silently breaks.
    ///
    /// Note that the sweep closes each room with `Closed { reason: "internal" }`,
    /// so a room closed by an unregistration reads as an internal error on the
    /// client side; the visible behavior (the editor closes, the room is unusable) is
    /// right either way.
    pub async fn dispose_domain(&self, domain: &str) -> usize {
        self.dispose_domain_discarding(domain, &HashSet::new())
            .await
    }

    /// [`CollabSessions::dispose_domain`], told which actors' drafts are being
    /// DISCARDED rather than folded.
    ///
    /// A room over one of those drafts is closed **without being saved**. Every
    /// other room saves first, exactly as it always did: an unregistration
    /// saves every room (the files stay on disk), and a fold saves every room
    /// because the folder is about to take those bytes anyway.
    ///
    /// Why the exception is not optional. The sweep runs one step after the
    /// review key comes off, so a room over a draft has already fallen back to
    /// the folder (`room_view`) and its final save is an ordinary file
    /// write. Discard is the one fold choice whose whole meaning is "this
    /// never reaches the tree" - the rows are dropped unwritten - so saving
    /// such a room would publish, into the reviewed folder, the one text the
    /// operator just said must not go there. It can be a grantee's text, typed
    /// inside a share-link, which is the sharpest form of the same thing.
    ///
    /// The unsaved text ends with the draft it was typed into, which is what
    /// discarding that draft means.
    pub async fn dispose_domain_discarding(
        &self,
        domain: &str,
        discarded: &HashSet<String>,
    ) -> usize {
        self.sweep_domain(domain, None, discarded, "internal").await
    }

    /// The domain of every room open over a draft of `account`, one per room. A
    /// socket can outlive its session, so a room is a place a removed account's
    /// text can still land.
    pub async fn draft_rooms_of(&self, account: &str) -> Vec<String> {
        self.sessions
            .lock()
            .await
            .keys()
            .filter(|key| key.2.as_deref() == Some(account))
            .map(|key| key.0.clone())
            .collect()
    }

    /// Final-save and close every room over a draft of `account`, because the
    /// account was removed: what was typed lands in the draft, which stays until
    /// an admin ends it, and nothing lands there later. Answers how many closed.
    pub async fn close_drafts_of(&self, account: &str) -> usize {
        let victims: Vec<Arc<CollabSession>> = {
            let mut sessions = self.sessions.lock().await;
            let keys: Vec<RoomKey> = sessions
                .keys()
                .filter(|key| key.2.as_deref() == Some(account))
                .cloned()
                .collect();
            keys.iter().filter_map(|key| sessions.remove(key)).collect()
        };
        let closed = victims.len();
        let reason = Some("internal".to_string());
        if let Err(err) = tokio::spawn(sweep_task(victims, HashSet::new(), reason)).await {
            tracing::warn!(%err, account, "a room sweep did not finish");
        }
        closed
    }

    /// Final-save every room over a document of `kind` in `domain`, then
    /// close it, telling each socket `Closed { reason }`. Answers how many
    /// rooms it closed.
    ///
    /// The switch-off of a Reflection calls this BEFORE the switch takes
    /// effect, with `reason` `switched_off`: the state is still on, so each
    /// room's last save lands in the page instead of being refused into a
    /// room nobody can save any more. Rooms over other kinds of document
    /// stay open.
    pub async fn close_docs(
        &self,
        domain: &str,
        kind: crystalline_core::ContentKind,
        reason: &str,
    ) -> usize {
        self.sweep_domain(domain, Some(kind), &HashSet::new(), reason)
            .await
    }

    /// The save-then-close sweep behind [`CollabSessions::dispose_domain`],
    /// [`CollabSessions::dispose_domain_discarding`] and
    /// [`CollabSessions::close_docs`]: every room of `domain`, or only those
    /// over a document of `kind` when one is named.
    async fn sweep_domain(
        &self,
        domain: &str,
        kind: Option<crystalline_core::ContentKind>,
        discarded: &HashSet<String>,
        reason: &str,
    ) -> usize {
        self.sweep_domain_rooms(domain, kind, discarded, Some(reason))
            .await
            .0
    }

    /// The sweep itself: answers how many rooms it ended, and the rooms
    /// themselves when no `reason` is given. With a `reason` each room is
    /// told `Closed { reason }`; without one each is only disposed, and the
    /// caller tells it (see [`SweptRooms`]).
    ///
    /// The saves run in a task of its own, so a caller that is cancelled
    /// while they run (its future dropped) leaves no room half closed: the
    /// task finishes every save and every close, and rooms nobody told are
    /// told `internal` when the task's answer is dropped.
    async fn sweep_domain_rooms(
        &self,
        domain: &str,
        kind: Option<crystalline_core::ContentKind>,
        discarded: &HashSet<String>,
        reason: Option<&str>,
    ) -> (usize, SweptRooms) {
        let victims: Vec<Arc<CollabSession>> = {
            let mut sessions = self.sessions.lock().await;
            // The DOMAIN component alone, whatever document each room is a
            // room over: leaving review mode ends every draft in the domain,
            // so a room over any of them is a room over nothing a moment
            // later. `rooms_closed` counts exactly what it always counted -
            // the rooms this domain had open - and the owner component neither
            // hides one from the sweep nor adds one to it. `close_docs` narrows
            // the sweep by the KIND of document only, never by owner.
            let keys: Vec<RoomKey> = sessions
                .keys()
                .filter(|key| key.0 == domain && kind.is_none_or(|kind| key.1.kind() == kind))
                .cloned()
                .collect();
            keys.iter().filter_map(|key| sessions.remove(key)).collect()
        };
        let closed = victims.len();
        let discarded = discarded.clone();
        let reason = reason.map(str::to_string);
        let sweeping = tokio::spawn(sweep_task(victims, discarded, reason));
        let untold = match sweeping.await {
            Ok(untold) => untold,
            Err(err) => {
                tracing::warn!(%err, domain, "a room sweep did not finish");
                SweptRooms { rooms: Vec::new() }
            }
        };
        (closed, untold)
    }

    /// End the room over a document a move is about to move, and fence the
    /// document so no room opens over it until the move is done.
    ///
    /// The room saves its text first by the rules every closing room follows
    /// (`final_save`: a save that cannot land keeps the text in the kept
    /// folder) and is then disposed, so nothing typed after it is saved; the
    /// editor keeps that as a draft. A save already running finishes first:
    /// the final save waits for the room's state, which a save holds. The
    /// sockets are not told yet: the move tells them `moved` with the new
    /// address once it has committed ([`MoveClose::moved`]), and a move that
    /// fails after this point tells them `internal`, since the document did
    /// not move.
    ///
    /// The save runs in a task of its own, so a move that is cancelled while
    /// it runs (its future dropped) still lands the save, and the room is
    /// told `internal` when the task's answer is dropped.
    ///
    /// **Why a fence.** The close has to run before the move takes the
    /// page's file lock, because the final save takes that lock under the
    /// room's state and the registry (lock order: registry, then session,
    /// then file). Without the fence a join between this close and the
    /// move's commit would open a fresh room on the old text: the window R81
    /// named. The fence is set under the registry lock in the same step that
    /// takes the room out of the map, and `join_doc` checks it under that
    /// lock, so a join is either closed here or refused until the fence
    /// drops with the returned guard.
    ///
    /// Only the document under `overlay` is closed: a draft of the same page
    /// by somebody else is another document, and the move does not move it.
    pub async fn close_for_move(
        self: &Arc<Self>,
        domain: &str,
        doc: &RoomDoc,
        overlay: Option<&str>,
        to: MovedTo,
    ) -> MoveClose {
        let key: RoomKey = (domain.to_string(), doc.clone(), overlay.map(str::to_string));
        let victim = {
            let mut sessions = self.sessions.lock().await;
            *self
                .moving
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(key.clone())
                .or_insert(0) += 1;
            sessions.remove(&key)
        };
        let fence = MoveFence {
            registry: Arc::downgrade(self),
            key,
        };
        let Some(session) = victim else {
            return MoveClose {
                room: SweptRooms { rooms: Vec::new() },
                to,
                _fence: fence,
            };
        };
        // The save comes first, as in every sweep: a disposed room's save
        // paths are no-ops. The fence travels with the task, so a move
        // cancelled while the save runs still keeps the document fenced until
        // the save is done.
        let saving = tokio::spawn(async move {
            let room = sweep_task(vec![session], HashSet::new(), None).await;
            (room, fence)
        });
        match saving.await {
            Ok((room, fence)) => MoveClose {
                room,
                to,
                _fence: fence,
            },
            // A panic below the save: the task's guards told the room
            // `internal` and lifted the fence as it unwound.
            Err(err) => {
                tracing::warn!(%err, "a moved room's last save did not finish");
                MoveClose {
                    room: SweptRooms { rooms: Vec::new() },
                    to,
                    _fence: MoveFence {
                        registry: Weak::new(),
                        key: (String::new(), RoomDoc::Engram(String::new()), None),
                    },
                }
            }
        }
    }

    /// Remember that the document at `key` moved to `to`, for a join that
    /// comes after the move. Forgets what is older than [`MOVED_MEMORY`].
    fn note_moved(&self, key: RoomKey, to: MovedTo) {
        let now = Instant::now();
        let mut moved = self
            .moved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        moved.retain(|_, (_, at)| now.saturating_duration_since(*at) < MOVED_MEMORY);
        moved.insert(key, (to, now));
    }

    /// Where the document at (`domain`, `doc`, `overlay`) went, when a move
    /// took it away in the last [`MOVED_MEMORY`]. Asked only once the address
    /// answers nothing, so a page created at the old path since wins.
    pub fn moved_from(&self, key: &RoomKey) -> Option<MovedTo> {
        let moved = self
            .moved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        moved
            .get(key)
            .filter(|(_, at)| at.elapsed() < MOVED_MEMORY)
            .map(|(to, _)| to.clone())
    }

    /// Whether `scope` may be told where a move took a document: always for
    /// a move inside one domain, and for a move into another domain only
    /// when `scope` may see that domain. A join that comes after the move
    /// asks this before it is told [`JoinError::Moved`]'s address, so the
    /// name of a domain somebody may not see never reaches them.
    pub async fn may_see_move(&self, to: &MovedTo, scope: &crate::scope::Scope) -> bool {
        sees_destination(&self.engine, to, scope).await
    }

    /// Whether a move holds `key` right now.
    fn is_moving(&self, key: &RoomKey) -> bool {
        self.moving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(key)
    }

    /// Save and end every room of `domain` for a domain rename, without
    /// telling the sockets yet: the rename tells them where the domain went
    /// once it is done, and a rename refused or stopped after the sweep
    /// tells them `internal` instead (see [`SweptRooms`]). Until the
    /// tell a room is disposed, so nothing typed into it is saved; the
    /// client keeps that text as a draft when the close arrives.
    pub async fn sweep_domain_for_rename(&self, domain: &str) -> SweptRooms {
        self.sweep_domain_rooms(domain, None, &HashSet::new(), None)
            .await
            .1
    }

    /// Move a session's registry entry after a frontmatter rename, so the room
    /// is found under the permalink it now answers to and a client that
    /// follows the `Saved { permalink }` broadcast rejoins THIS room instead of
    /// opening a second one over the same file.
    ///
    /// Called with no session guard held: the lock order is registry ->
    /// session, never the reverse. `epoch` identifies the session that renamed
    /// itself, so an entry that was replaced meanwhile is left alone.
    async fn rekey(&self, from: &RoomKey, to_permalink: &str, epoch: &str) {
        // A rename moves an engram's permalink, and only an engram room is
        // ever re-keyed: a reflection's receipt names the same path.
        let from_permalink = match &from.1 {
            RoomDoc::Engram(permalink) => permalink.as_str(),
            RoomDoc::Reflection(_) => return,
        };
        if from_permalink == to_permalink {
            return;
        }
        // The permalink moves and nothing else does: a rename is a new address
        // for the same document, and whose document it is has not changed.
        let to = (
            from.0.clone(),
            RoomDoc::Engram(to_permalink.to_string()),
            from.2.clone(),
        );
        let mut sessions = self.sessions.lock().await;
        if sessions.get(from).is_none_or(|held| held.epoch() != epoch) {
            return; // disposed or replaced meanwhile: not ours to move
        }
        // A live room already at the new key is pathological (two sessions
        // over one file). Clobbering it would strand its participants, so both
        // stand and the CAS token settles it: the second save conflicts and
        // that room goes save-blocked rather than overwriting the first.
        if let Some(existing) = sessions.get(&to)
            && !existing.is_disposed()
        {
            tracing::warn!(
                domain = %to.0,
                permalink = %to.1.name(),
                "a rename collided with a live session; both rooms stand"
            );
            return;
        }
        if let Some(session) = sessions.remove(from) {
            session.adopt_key(to_permalink);
            sessions.insert(to, session);
        }
    }

    /// The live text of one open document, or `None` when no room is open
    /// over it.
    ///
    /// **The seam an agent's read and write meet the editor through, and the
    /// whole of the contract is in the `Option`.** `Some` means somebody has
    /// this document open and the bytes here are theirs - typed, unsaved, and
    /// the truth about what the engram says right now. `None` means the file
    /// or the row is the truth, exactly as it always was, which is what nearly
    /// every read and write on this instance gets.
    ///
    /// The text is the bytes the room's next save writes, which are LF: the
    /// caller is an engine verb that parses markdown and compares checksums.
    ///
    /// `overlay` is whose document to ask about, and it is the caller's own
    /// actor - see [`RoomKey`]. An agent writing in a direct domain asks about
    /// `None`, and in a reviewing domain about its own account's draft (or,
    /// inside a join, about the owner's, which is the actor its view already
    /// resolved to). It is never a name taken from a request.
    pub async fn live_text(
        &self,
        domain: &str,
        permalink: &str,
        overlay: Option<&str>,
    ) -> Option<LiveText> {
        let session = self
            .live_room(domain, &RoomDoc::Engram(permalink.to_string()), overlay)
            .await?;
        Some(session.live().await)
    }

    /// Compose `target` into the live document as one transaction tagged with
    /// the agent that produced it, and arm the saver.
    ///
    /// The text is applied as a minimal line-based edit script
    /// ([`merge::apply_target`]) rather than as a replacement, which is what
    /// makes it compose: a person typing at the bottom of the page keeps their
    /// cursor, their selection and their undo stack, and only the lines the
    /// agent actually changed move. That is also why the whole document is
    /// handed in rather than a patch - the engine computed the target from the
    /// live text, and the diff back to it is this function's job.
    ///
    /// `peer` is who the agent shows up as in the room's participant strip
    /// while the change lands, and for the minute after it
    /// ([`AGENT_PRESENCE_TTL_MS`]). `None` for a write with nobody to name -
    /// the CLI, the control socket - which composes exactly as it did and puts
    /// nothing in the strip.
    ///
    /// `agent_label` is the transaction origin. It never leaves this process -
    /// a yrs origin is local and does not travel on an update - so it is for
    /// the in-process observer (a later event handler, a log line) rather than
    /// for the client, which sees the edit as an ordinary remote update.
    ///
    /// The saver is armed the way typing arms it, not forced: the agent's text
    /// IS the room's text now, and it lands on the same debounce a person's
    /// does. `Err` when the room closed between the read and the write, which
    /// the caller reports rather than retries - the document it computed
    /// against is gone.
    pub async fn apply_text(
        &self,
        domain: &str,
        permalink: &str,
        overlay: Option<&str>,
        target: String,
        agent_label: &str,
        peer: Option<&AgentPeer>,
    ) -> Result<LiveApplied, String> {
        self.apply_doc_text(
            domain,
            &RoomDoc::Engram(permalink.to_string()),
            overlay,
            target,
            agent_label,
            peer,
        )
        .await
    }

    /// [`CollabSessions::apply_text`] for a write that has a source to fall
    /// back on: it composes only while the room will save it, and only onto
    /// the very text `target` was computed from.
    ///
    /// Both are decided under the room's state lock, the one the compose
    /// itself holds, so neither can change between the check and the
    /// compose. A room saves what it holds while its save state is ok and
    /// its final save has not run; a compose after that final save would sit
    /// in a room nobody saves again. Between the last connection leaving and
    /// the final save it is still taken, and the final save writes it.
    /// A room in conflict or with a failed save may never write it either.
    /// `base` is the live text `target` was computed from: when it moved,
    /// composing the diff to `target` would take back what was typed since.
    /// `None` is a wholesale replacement, which was computed from no text
    /// and so checks only that the room saves.
    ///
    /// `overlay` and `peer` are [`CollabSessions::apply_text`]'s: whose
    /// document, and who the agent shows up as in its strip.
    #[allow(clippy::too_many_arguments)]
    pub async fn compose_while_saving(
        &self,
        domain: &str,
        permalink: &str,
        overlay: Option<&str>,
        base: Option<&str>,
        target: String,
        agent_label: &str,
        peer: Option<&AgentPeer>,
    ) -> Result<LiveApplied, ComposeRefused> {
        let Some(session) = self
            .live_room(domain, &RoomDoc::Engram(permalink.to_string()), overlay)
            .await
        else {
            return Err(ComposeRefused::NotSaving);
        };
        session
            .apply_agent_text_while_saving(base, &target, agent_label, peer)
            .await
    }

    /// Have the room over `doc` take in a change an engine verb just wrote
    /// to its file, now rather than on its next idle probe: a three-way merge
    /// of the file into the room's own live text, against the text the room
    /// last saw on disk, so typing nobody has saved yet stays. A collision
    /// raises the room's edit conflict; nothing in the room is erased.
    ///
    /// Takes no target text, unlike [`CollabSessions::apply_doc_text`]: the
    /// file is the change, so a caller that composed it from the file cannot
    /// hand the room a target that was never computed from the room. The
    /// caller holds no page lock, because a room's save takes that lock
    /// under the room's own state.
    pub async fn absorb_doc_change(
        &self,
        domain: &str,
        doc: &RoomDoc,
        overlay: Option<&str>,
    ) -> Result<AbsorbOutcome, String> {
        let Some(session) = self.live_room(domain, doc, overlay).await else {
            return Err(format!(
                "the co-editing session over '{}' in domain '{domain}' closed",
                doc.name()
            ));
        };
        session.absorb_file_change().await
    }

    /// [`CollabSessions::apply_text`] for a room over any kind of document.
    pub async fn apply_doc_text(
        &self,
        domain: &str,
        doc: &RoomDoc,
        overlay: Option<&str>,
        target: String,
        agent_label: &str,
        peer: Option<&AgentPeer>,
    ) -> Result<LiveApplied, String> {
        let Some(session) = self.live_room(domain, doc, overlay).await else {
            return Err(format!(
                "the co-editing session over '{}' in domain '{domain}' closed while this \
                 write was being prepared; read it again and repeat the edit",
                doc.name()
            ));
        };
        session.apply_agent_text(&target, agent_label, peer).await
    }

    /// Stand an agent in the room over one document, when one is open.
    ///
    /// The read side of the same claim [`CollabSessions::apply_text`] makes:
    /// an agent that was answered somebody's unsaved text is reading over
    /// their shoulder, and the strip says so for as long as the TTL stands.
    /// Nothing at all when no room is open, which is nearly every read.
    /// Answers the awareness id the agent stands under, which is what a
    /// caller asking who else is in the room leaves out of the answer.
    pub async fn touch_agent_presence(
        &self,
        domain: &str,
        permalink: &str,
        overlay: Option<&str>,
        peer: &AgentPeer,
    ) -> Option<ClientID> {
        let session = self
            .live_room(domain, &RoomDoc::Engram(permalink.to_string()), overlay)
            .await?;
        session.touch_agent_presence(peer).await
    }

    /// Whether a room is open over one document, without rendering a word of
    /// it.
    ///
    /// The existence half of [`CollabSessions::live_text`], for the callers
    /// that only need to know whether the room is there: `live_text` renders
    /// the whole Yrs document under the session lock, which is a real cost on
    /// a large engram and is pure waste when the answer is a yes-or-no. The
    /// three conditions are `CollabSessions::live_room`'s, so a swept,
    /// poisoned or closed room answers `false` here exactly as it answers
    /// `None` there.
    pub async fn has_live_room(
        &self,
        domain: &str,
        permalink: &str,
        overlay: Option<&str>,
    ) -> bool {
        self.has_live_doc(domain, &RoomDoc::Engram(permalink.to_string()), overlay)
            .await
    }

    /// Whether any room over an engram of `domain`'s own text (not a draft)
    /// is registered. A cheap look at the registry alone, for a caller about
    /// to ask the index which engram a path holds only when a room might be
    /// open over it.
    pub async fn has_engram_rooms_in(&self, domain: &str) -> bool {
        self.sessions.lock().await.keys().any(|(d, doc, overlay)| {
            d == domain && overlay.is_none() && matches!(doc, RoomDoc::Engram(_))
        })
    }

    /// [`CollabSessions::has_live_room`] for a room over any kind of document.
    pub async fn has_live_doc(&self, domain: &str, doc: &RoomDoc, overlay: Option<&str>) -> bool {
        self.live_room(domain, doc, overlay).await.is_some()
    }

    /// Who is in the room over one document right now, or an empty list when
    /// no room is open over it.
    pub async fn participants(
        &self,
        domain: &str,
        doc: &RoomDoc,
        overlay: Option<&str>,
        except: Option<ClientID>,
    ) -> Vec<String> {
        match self.live_room(domain, doc, overlay).await {
            Some(session) => session.participants(except).await,
            None => Vec::new(),
        }
    }

    /// The room over one document, when one is open and still writing.
    ///
    /// Three conditions, and each drops a room that is not the truth about the
    /// engram any more: absent from the map (nobody has it open), disposed (a
    /// swept or poisoned room, whose saver has ended), and closed (the room
    /// accepted an external deletion, so its text is deliberately not going
    /// anywhere). A caller handed one of those would compose into a document
    /// nothing will ever write back.
    async fn live_room(
        &self,
        domain: &str,
        doc: &RoomDoc,
        overlay: Option<&str>,
    ) -> Option<Arc<CollabSession>> {
        let key = (domain.to_string(), doc.clone(), overlay.map(str::to_string));
        let session = self
            .sessions
            .lock()
            .await
            .get(&key)
            .filter(|session| !session.is_disposed())
            .cloned()?;
        (!session.is_closed().await).then_some(session)
    }

    /// How many documents are open right now.
    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    fn mint_epoch(&self) -> String {
        // Unique across restarts: wall-clock nanos plus an in-process counter.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!(
            "{:x}.{:x}",
            nanos,
            self.next_epoch.fetch_add(1, Ordering::Relaxed)
        )
    }
}

/// One open document: the shared yrs doc, its participants and the fan-out
/// channel every connection listens on.
pub struct CollabSession {
    epoch: String,
    /// The domain this room's engram lives in; no edit ever moves it.
    domain: String,
    /// The document this room is REGISTERED under. A frontmatter rename moves
    /// it together with the registry entry, so `(domain, key_doc)` is always
    /// where the registry holds this session. A std mutex, never held across
    /// an await.
    key_doc: std::sync::Mutex<RoomDoc>,
    /// Whose document this room is a room over: `Some(actor)` for that actor's
    /// draft of the page, `None` for the one a direct domain keeps. Fixed for
    /// the life of the room - a rename moves the permalink, never the owner -
    /// and read by everything this room reads and writes through.
    overlay: Option<String>,
    /// The path this room's document stood at when it opened, and the only
    /// path an overlay room ever writes.
    ///
    /// A room addresses its saves by the permalink its own text carries, and
    /// that line is typed by whoever is in the room. So the path is pinned
    /// here at the open and every save is screened against it
    /// ([`Engine::save_engram_in_overlay`]): a document that starts claiming
    /// to be a different engram is refused rather than followed, because a
    /// room is one document and a person invited into one page was invited
    /// into one page. `None` for a room over the document a direct domain
    /// keeps, which is not inside anybody's overlay and has the whole folder
    /// in front of it either way.
    ///
    /// It is also what keeps the guest eviction's lookup asking about the
    /// right draft on every tick rather than only at the upgrade: `state.path`
    /// moves only on an accepted save's receipt, and an accepted save is one
    /// that landed here.
    pinned_path: Option<String>,
    /// The registry this room lives in, for the rename move. Weak because the
    /// registry owns the session and never the other way round.
    registry: Weak<CollabSessions>,
    /// The engine every durable read and write goes through.
    engine: Arc<Engine>,
    tx: broadcast::Sender<Frame>,
    /// Whether anybody is in this room as somebody's guest, so a tick over a
    /// room nobody joined into costs no lock at all.
    ///
    /// Written only under the state guard, and always to `!guests.is_empty()`
    /// as that guard sees it, so it cannot say "nobody" while a guest stands
    /// in the map - which would be a session left inside a draft it had been
    /// put out of.
    has_guests: AtomicBool,
    /// Whether any agent stands in this room, so a tick over a room no agent
    /// has ever worked in costs no lock at all - the same pre-filter
    /// `has_guests` is, for the same reason.
    ///
    /// Written only under the state guard, and always to `!agents.is_empty()`
    /// as that guard sees it.
    has_agents: AtomicBool,
    /// The room is over: the registry dropped it, or a saver pass panicked.
    /// Ends the saver loop and makes every save path a no-op, so nothing can
    /// write through a session no one owns any more.
    disposed: AtomicBool,
    /// The save under way is the room's last one (`last_save`). Set only
    /// around that save and cleared after it, so a room saved on close and
    /// kept open by a late joiner does not assign section ids under the next
    /// cursor. Read by a reflection room, whose ids come with the close.
    final_pass: AtomicBool,
    state: Mutex<SessionState>,
}

struct SessionState {
    /// Owns the Doc (yrs::sync::Awareness::new takes it); the doc is built
    /// with OffsetKind::Utf16 so every index agrees with JS clients.
    awareness: Awareness,
    /// The domain-relative file path, as loaded and as each save receipt
    /// reports it back. The address a restore writes back to.
    path: String,
    permalink: String,
    /// The checksum backing last_saved_text: the CAS token of the next save.
    checksum: String,
    /// The text as last loaded or saved, as LF: the room's own text is LF,
    /// and a CRLF file it opened on is the same document as its LF form, so a
    /// room that was only opened never writes.
    last_saved_text: String,
    /// Awareness client ids seen per connection, nulled on its disconnect.
    conns: HashMap<ConnId, HashSet<ClientID>>,
    /// Whether the room's final save has run (whatever it ended in) since the
    /// last connection joined. Text composed in after it is saved by nobody,
    /// so an engine write that has a source to fall back on is refused from
    /// here on. Between the last connection leaving and the final save the
    /// room still saves what it holds, which an empty `conns` cannot tell.
    final_saved: bool,
    /// The signed-in account each connection of a reflection room belongs
    /// to, as the surface that opened it authenticated it. Never read from
    /// awareness, which is whatever a client sends.
    authors: HashMap<ConnId, String>,
    /// Who each connection is, as the surface that opened it authenticated
    /// it, recorded in the same step that adds the connection: what a
    /// `moved` close may name to that connection is screened against it, so
    /// a move into a domain somebody may not see does not tell them its name
    /// ([`CollabSession::close_moved`]). A connection with no entry is told
    /// no address at all (fail closed).
    viewers: HashMap<ConnId, crate::scope::Scope>,
    /// The accounts whose connections sent an edit since the last save
    /// landed: the names that save records. Kept past a disconnect, so the
    /// last person to leave is still named by the final save, and cleared
    /// only when a save lands, so a refused save keeps them for its retry.
    editors: std::collections::BTreeSet<String>,
    /// The accounts that typed each heading of a reflection room that has
    /// no id yet, gathered from `editors` as each save lands. A heading gets
    /// its id only with the room's close, when its typists have usually
    /// left and `editors` no longer names them; the close records these for
    /// the section it mints, then they are dropped. Empty in an engram room.
    /// Keyed as [`SectionAuthors::new_by_heading`] is.
    new_heading_authors:
        std::collections::BTreeMap<(String, usize), std::collections::BTreeSet<String>>,
    /// The agents standing in this room, keyed by (account, label).
    ///
    /// Beside `conns` and never in it, which is the whole shape of the
    /// feature: an agent holds no socket, so counting its slot as a connection
    /// would keep a room open that nobody is in and hand a disconnect the job
    /// of clearing something no disconnect is about. `remove_conn` leaves
    /// these alone and only the TTL sweep takes one away.
    agents: HashMap<(String, String), AgentSlot>,
    /// The connections that are in this room as somebody's guest, and the
    /// account and join holder each of them is inside on. Empty in every room
    /// over a document its participants own, which is nearly all of them.
    ///
    /// The holder rather than the account decides: a join belongs to one
    /// browser session, so the question "is this socket still inside the
    /// draft" is about that session's join and not about whatever else the
    /// account may have joined from somewhere else. The account is carried
    /// beside it because the registry asks for both - see
    /// [`crate::join::Joins::holds`], where it is the same defence.
    guests: HashMap<ConnId, (String, crate::join::Holder)>,
    dirty: bool,
    /// When the most recent update landed: the debounce timer's input.
    last_edit: Option<Instant>,
    /// When the first unsaved update landed: the max-wait timer's input.
    oldest_unsaved: Option<Instant>,
    /// A client asked for a save now rather than on the debounce.
    flush_requested: bool,
    /// When the engine was last asked to write. Only a save-blocked session
    /// reads it, to space its retries out instead of hammering the engine
    /// every tick for as long as the document stays unsaveable.
    last_attempt: Option<Instant>,
    /// When the idle external-change probe last ran.
    last_probe: Option<Instant>,
    /// The detail of the standing save failure, so a refusal that changes its
    /// reason re-broadcasts instead of leaving the room reading a stale one.
    failure_detail: Option<String>,
    /// The external change the room has to decide about; saving is suspended
    /// while it stands.
    pending: Option<PendingConflict>,
    /// The room accepted an external deletion: the session is over, saves
    /// included, and the socket loop disconnects everyone.
    closed: bool,
    save_state: SaveStateTag,
}

/// The external change a room is being asked to resolve.
enum PendingConflict {
    /// Both sides edited: `theirs` is the file's text, `theirs_checksum` the
    /// CAS token that lets "mine" land over it.
    Edit {
        theirs: String,
        theirs_checksum: String,
    },
    /// The file is gone; "mine" restores it, "theirs" closes the room.
    Deleted,
}

/// The wire label in hello/save_state controls ("ok" | "failed" | "conflict").
#[derive(Clone, Copy, PartialEq)]
enum SaveStateTag {
    Ok,
    Failed,
    Conflict,
}

impl SaveStateTag {
    fn as_str(self) -> &'static str {
        match self {
            SaveStateTag::Ok => "ok",
            SaveStateTag::Failed => "failed",
            SaveStateTag::Conflict => "conflict",
        }
    }
}

/// The view a room over `overlay` reads and writes through.
///
/// One function, so the open, the external-change probe, the merge and the
/// restore cannot disagree about whose document a room is. `None` is the
/// document a direct domain keeps and answers exactly what it answered before
/// drafts existed: the folder's own text through the base view.
///
/// **This is the co-editing saver's seam onto another actor's rows**, and the
/// allow-list guard `another_actors_view_is_reached_only_by_the_owner_gated_surfaces`
/// in crates/service/tests/overlay/overlay_domains.rs names it. What makes it safe is
/// where the owner comes from: never from the socket, never from a path
/// segment, only from the key the room was opened under - and that key was
/// decided by [`super::ws::join`], which lets a caller name somebody else's
/// document only when a live share-link of that author's names them and this
/// session holds a live join on it.
///
/// **The seam Task 14 needs is this same key.** An agent joining a draft
/// through its MCP session opens the join record that route reads, so a room
/// asked for over that draft carries the owner here by exactly the path a
/// browser's does; nothing in this module has to learn what an MCP session is.
///
/// **A domain that has stopped reviewing changes has no overlay documents
/// left, so a room over one falls back to the base view** - the same reading
/// [`DomainView::for_write_joined`] takes of a join into a domain that left
/// review mode. It is load bearing during a fold: the key comes off, then the
/// rooms are swept, and the sweep's final save has to land in the folder the
/// folds are about to be written over. Landing it in a draft row instead would
/// put an actor's last typing somewhere the fold drops a moment later. The
/// other half of that pair is worth saying too: an UNREGISTRATION sweeps while
/// the domain is still registered and still reviewing, so a room over a draft
/// saves into that draft and goes with it - which is what unregistering a
/// reviewing domain promises, rather than spilling unreviewed work into a
/// folder that stays on disk.
fn room_view<'a>(
    engine: &'a Engine,
    domain: &str,
    overlay: Option<&str>,
) -> Result<DomainView<'a>, EngineError> {
    match overlay.filter(|_| engine.reviews_changes(domain)) {
        Some(owner) => DomainView::for_actor(engine, domain, &HashSet::new(), owner),
        None => DomainView::base(engine, domain, &HashSet::new()),
    }
}

impl CollabSession {
    async fn open(
        engine: Arc<Engine>,
        key: RoomKey,
        epoch: String,
        registry: Weak<CollabSessions>,
    ) -> Result<Arc<CollabSession>, JoinError> {
        // Read through the room's own view: the owner's draft where the room
        // is a room over one, the text the team reviewed where it is not. The
        // domain was screened by the surface that asked for the room, and so
        // was the right to be in this document.
        let loaded = Self::load_doc(&engine, &key)
            .await
            .map_err(JoinError::Engine)?;
        // The room's text is LF whatever the file holds; a CRLF or mixed
        // file is written as LF on the room's first save. The checksum stays
        // the one of the bytes on disk, the CAS token that save presents.
        let lf_content = to_lf(&loaded.content).into_owned();
        // OffsetKind::Utf16 rather than the default Bytes: every index the
        // server computes then counts the same units the UTF-16 indexed JS
        // client counts, so a non-ASCII document does not desync.
        let doc = Doc::with_options(Options {
            offset_kind: yrs::OffsetKind::Utf16,
            ..Options::default()
        });
        let text = doc.get_or_insert_text(TEXT_NAME);
        {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, &lf_content);
        }
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Ok(Arc::new(CollabSession {
            epoch,
            domain: key.0,
            key_doc: std::sync::Mutex::new(key.1),
            pinned_path: key.2.is_some().then(|| loaded.path.clone()),
            overlay: key.2,
            registry,
            engine,
            tx,
            has_guests: AtomicBool::new(false),
            has_agents: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
            final_pass: AtomicBool::new(false),
            state: Mutex::new(SessionState {
                awareness: Awareness::new(doc),
                path: loaded.path,
                permalink: loaded.permalink,
                checksum: loaded.checksum,
                last_saved_text: lf_content,
                conns: HashMap::new(),
                final_saved: false,
                authors: HashMap::new(),
                viewers: HashMap::new(),
                editors: std::collections::BTreeSet::new(),
                new_heading_authors: std::collections::BTreeMap::new(),
                agents: HashMap::new(),
                guests: HashMap::new(),
                dirty: false,
                last_edit: None,
                oldest_unsaved: None,
                flush_requested: false,
                last_attempt: None,
                // The probe window starts at open, so a room that sits idle
                // from its first tick still checks one window later.
                last_probe: Some(Instant::now()),
                failure_detail: None,
                pending: None,
                closed: false,
                save_state: SaveStateTag::Ok,
            }),
        }))
    }

    /// This session's epoch: a client that reconnects to a different one knows
    /// the document it was editing is gone and must reload.
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    async fn add_conn(
        &self,
        conn: ConnId,
        author: Option<&str>,
        viewer: Option<crate::scope::Scope>,
    ) -> Result<(broadcast::Receiver<Frame>, Vec<u8>), JoinError> {
        let mut state = self.state.lock().await;
        if state.conns.len() >= MAX_PARTICIPANTS {
            return Err(JoinError::SessionFull);
        }
        state.conns.insert(conn, HashSet::new());
        // A join before the room was disposed revives it: its saver runs and
        // a final save comes again when this connection leaves.
        state.final_saved = false;
        if let Some(author) = author {
            state.authors.insert(conn, author.to_string());
        }
        if let Some(viewer) = viewer {
            state.viewers.insert(conn, viewer);
        }
        // The greeting: hello, then the server's SyncStep1, then the full
        // awareness picture - the DefaultProtocol::start choreography with our
        // control message in front. One buffer; concatenation is legal.
        let mut greeting = control::encode(&Control::Hello {
            epoch: self.epoch.clone(),
            // Always LF: the room's text is, and so is every save of it.
            separator: "\n".to_string(),
            checksum: state.checksum.clone(),
            permalink: state.permalink.clone(),
            save_state: state.save_state.as_str().to_string(),
            // A room whose save is standing refused says why in its greeting.
            // The SaveFailed broadcast is not repeated for a detail already
            // announced, so without this a joiner would read "Saved" over a
            // room that has not written anything since the refusal.
            detail: state.failure_detail.clone(),
        });
        let sv = state.awareness.doc().transact().state_vector();
        greeting.extend(Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1());
        if let Ok(full) = state.awareness.update()
            && !full.clients.is_empty()
        {
            greeting.extend(Message::Awareness(full).encode_v1());
        }
        // Subscribing last means this connection's receiver starts at the
        // frames that follow its own greeting, never before it.
        Ok((self.tx.subscribe(), greeting))
    }

    /// Handle one incoming WS frame; returns direct replies for THIS conn.
    /// Everything other connections need is broadcast instead, tagged with
    /// `conn` so the socket loop can skip the echo.
    pub async fn handle_frame(&self, conn: ConnId, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut replies = Vec::new();
        let mut renamed = None;
        let mut state = self.state.lock().await;
        let mut decoder = DecoderV1::from(bytes);
        // Collect first: MessageReader borrows the decoder.
        let messages: Vec<Message> = match MessageReader::new(&mut decoder).collect() {
            Ok(messages) => messages,
            Err(err) => {
                // A frame that does not parse is dropped whole: a partial
                // application would leave the document in a state no client
                // agrees with.
                tracing::debug!(%err, conn, "dropping an unparseable collab frame");
                return replies;
            }
        };
        for message in messages {
            match message {
                Message::Sync(SyncMessage::SyncStep1(sv)) => {
                    let update = state
                        .awareness
                        .doc()
                        .transact()
                        .encode_state_as_update_v1(&sv);
                    replies.push(Message::Sync(SyncMessage::SyncStep2(update)).encode_v1());
                }
                Message::Sync(SyncMessage::SyncStep2(update))
                | Message::Sync(SyncMessage::Update(update)) => {
                    let Ok(decoded) = Update::decode_v1(&update) else {
                        tracing::debug!(conn, "dropping an undecodable collab update");
                        continue;
                    };
                    // Whether the update changed the document: a provider's
                    // answer to the greeting is a SyncStep2 that applies
                    // cleanly and carries nothing new, and its sender has
                    // edited nothing.
                    let (applied, edited) = {
                        let mut txn = state.awareness.doc().transact_mut();
                        let before = txn.state_vector();
                        let applied = txn.apply_update(decoded);
                        let edited = txn.state_vector() != before || !txn.delete_set().is_empty();
                        (applied, edited)
                    };
                    if applied.is_ok() {
                        if edited && let Some(author) = state.authors.get(&conn).cloned() {
                            state.editors.insert(author);
                        }
                        let now = Instant::now();
                        state.dirty = true;
                        state.last_edit = Some(now);
                        state.oldest_unsaved.get_or_insert(now);
                        let _ = self.tx.send(Frame {
                            from: Some(conn),
                            to: None,
                            bytes: Bytes::from(
                                Message::Sync(SyncMessage::Update(update)).encode_v1(),
                            ),
                        });
                    }
                }
                Message::Awareness(update) => {
                    // Awareness payloads are opaque: the server tracks which
                    // client ids a connection announced so it can null them on
                    // disconnect, and never parses the JSON inside.
                    let ids: Vec<ClientID> = update.clients.keys().copied().collect();
                    if state.awareness.apply_update_summary(update.clone()).is_ok() {
                        let tracked = state.conns.entry(conn).or_default();
                        tracked.extend(ids);
                        let _ = self.tx.send(Frame {
                            from: Some(conn),
                            to: None,
                            bytes: Bytes::from(Message::Awareness(update).encode_v1()),
                        });
                    }
                }
                Message::AwarenessQuery => {
                    if let Ok(full) = state.awareness.update() {
                        replies.push(Message::Awareness(full).encode_v1());
                    }
                }
                Message::Custom(control::CONTROL_TAG, payload) => match control::decode(&payload) {
                    Some(Control::Flush) => {
                        state.flush_requested = true;
                    }
                    Some(Control::Resolve { choice }) => {
                        // Kept rather than overwritten: a frame carrying two
                        // resolves must not drop the move the first one made.
                        if let Some(moved) = self.resolve_conflict(&mut state, &choice).await {
                            renamed = Some(moved);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        // The session guard goes before the registry lock a rename takes: a
        // restore may have landed the engram under a new permalink.
        drop(state);
        self.adopt_rename(renamed).await;
        replies
    }

    /// Record that `conn` is in this room on a join rather than on its own
    /// document, so the saver can put it outside the draft when that join
    /// ends.
    ///
    /// Called by the upgrade route once it has decided the connection may be
    /// here at all; a room over nobody's draft never has one.
    pub async fn watch_guest(&self, conn: ConnId, account: &str, holder: &crate::join::Holder) {
        let mut state = self.state.lock().await;
        state
            .guests
            .insert(conn, (account.to_string(), holder.clone()));
        self.has_guests.store(true, Ordering::Relaxed);
    }

    /// Close every connection whose join into this draft has ended.
    ///
    /// A join ends in one place - the registry - however it ended: the author
    /// took the link back (which ends the joins on that draft), the draft was
    /// folded, discarded or renamed, the person pressed Leave, the link's own
    /// window ran out, or the daemon was restarted under them. So this asks one
    /// question per guest per tick, of a map in this process's memory, and
    /// never reads the grant rows: a revocation is already an ending, and
    /// asking the database four times a second would be asking it something it
    /// has already answered.
    ///
    /// **An expiry is the one ending nobody announces**, and it is covered the
    /// same way rather than by a second question: the link's window is stamped
    /// onto the join when it is opened
    /// ([`crate::join::Join::expires_at`]), so the moment it passes the
    /// registry answers no here and the socket is closed on the next pass -
    /// out of the same map, at the same cost.
    ///
    /// **And this pass is what keeps an open room's join alive.** The question
    /// is a use, so a person reading somebody's draft without typing a word
    /// refreshes their join four times a second for as long as the room is
    /// open, and the registry's idle window
    /// ([`crate::join::IDLE_JOIN_LIMIT`]) never reaches them.
    ///
    /// The frame is addressed to that connection alone. The owner is not a
    /// guest and is never in this map, so their socket stands through every
    /// revocation there is - which is the difference between a link being
    /// taken back and a room being closed.
    async fn evict_ended_joins(&self) {
        let Some(owner) = self.overlay.as_deref() else {
            return; // a document nobody joined into cannot be left
        };
        if !self.has_guests.load(Ordering::Relaxed) {
            return; // asked four times a second, and almost always here
        }
        let mut state = self.state.lock().await;
        let path = state.path.clone();
        let joins = self.engine.joins();
        let ended: Vec<ConnId> = state
            .guests
            .iter()
            .filter(|(_, (account, holder))| {
                !joins.holds(account, holder, &self.domain, owner, &path)
            })
            .map(|(conn, _)| *conn)
            .collect();
        for conn in ended {
            state.guests.remove(&conn);
            self.has_guests
                .store(!state.guests.is_empty(), Ordering::Relaxed);
            let _ = self.tx.send(Frame {
                from: None,
                to: Some(conn),
                bytes: Bytes::from(control::encode(&Control::closed("left"))),
            });
        }
    }

    /// Stand an agent in this room's participant strip, or refresh the claim
    /// it already holds.
    ///
    /// **What makes an agent a peer rather than an event.** A person watching
    /// their document move under an agent's edit is owed the same thing they
    /// are owed when a colleague types into it: a name in the strip saying who
    /// is in here. So every action an agent takes in this document - a write
    /// that composes into it, a read answered from it - puts the agent in the
    /// room for the next [`AGENT_PRESENCE_TTL_MS`].
    ///
    /// Idempotent per (account, label): the second call finds the slot
    /// standing, refreshes its expiry and publishes nothing, because the state
    /// it would publish is the state the room already holds and yrs would drop
    /// a re-publish at the same clock anyway.
    pub async fn touch_agent_presence(&self, peer: &AgentPeer) -> Option<ClientID> {
        let mut state = self.state.lock().await;
        self.touch_agent_locked(&mut state, peer)
    }

    /// [`CollabSession::touch_agent_presence`] over the locked state, for the
    /// write path, which is holding the guard across the whole of its edit.
    ///
    /// Answers the slot's client id, which is how the caller leaves itself out
    /// of the list of who is in the room with it, and `None` when there is no
    /// slot: a room already holding [`MAX_PARTICIPANTS`] agents, or a publish
    /// the awareness state refused. A caller handed `None` excludes nothing,
    /// which is the true thing to do with an agent that is not in the strip.
    fn touch_agent_locked(&self, state: &mut SessionState, peer: &AgentPeer) -> Option<ClientID> {
        let key = (peer.account.clone(), peer.label.clone());
        let now = Instant::now();
        if let Some(id) = state.agents.get(&key).map(|slot| slot.id) {
            // **A slot the room cannot see is not a slot.** The id is a hash
            // of a label that is on screen, so a connection in the room can
            // publish under it, and when that connection leaves the room nulls
            // every id it sent - the agent's among them. The map would still
            // say the agent is standing, and a standing slot publishes
            // nothing, so the agent would go on working with no chip until the
            // TTL swept a slot nobody could see. Treated as new instead, and
            // republished at a clock above whatever took it away.
            if state.awareness.state::<serde_json::Value>(id).is_some() {
                if let Some(slot) = state.agents.get_mut(&key) {
                    slot.touched = now;
                }
                return Some(id);
            }
            state.agents.remove(&key);
        }
        // **Bounded exactly as the connection map is** ([`add_conn`]), and for
        // the same reason: half this key is client-supplied per request, so a
        // caller that names itself differently every call - by accident, since
        // a modern-era peer may carry `clientInfo` on one call and omit it on
        // the next, or on purpose - would otherwise grow one room's strip
        // without limit. The chip is what is refused and nothing else: the
        // read or the write that asked for it goes on exactly as it would
        // have, because an agent's work is not a thing a full strip may
        // refuse. And a person is never evicted to make room for an agent -
        // the two maps are separate, so this cap cannot reach a connection.
        if state.agents.len() >= MAX_PARTICIPANTS {
            return None;
        }
        let id = agent_client_id(&peer.account, &peer.label, state.awareness.client_id());
        // The clock one past whatever this id last carried, exactly as
        // `remove_conn` does it: a slot that was swept and is being reclaimed
        // has a clock the room remembers, and a state published under it would
        // otherwise be dropped as old news.
        let clock = state.awareness.meta(id).map(|meta| meta.0 + 1).unwrap_or(1);
        let mut clients = HashMap::new();
        clients.insert(
            id,
            AwarenessUpdateEntry {
                clock,
                json: agent_state_json(&peer.label).into(),
            },
        );
        let update = AwarenessUpdate { clients };
        if state.awareness.apply_update_summary(update.clone()).is_ok() {
            let _ = self.tx.send(Frame {
                from: None,
                to: None,
                bytes: Bytes::from(Message::Awareness(update).encode_v1()),
            });
            state.agents.insert(key, AgentSlot { id, touched: now });
            self.has_agents.store(true, Ordering::Relaxed);
            return Some(id);
        }
        None
    }

    /// Take away the agent slots nothing has refreshed inside the TTL.
    ///
    /// On the saver's own pass rather than on a timer of its own: the room
    /// already wakes up four times a second to ask whether a save is due, and
    /// an expiry that needed a second timer would be a second thing to stop
    /// when a room is disposed.
    async fn sweep_agent_presence(&self, now: Instant) {
        // Nothing to sweep is the ordinary case - most rooms never see an
        // agent - and it costs no lock at all.
        if !self.has_agents.load(Ordering::Relaxed) {
            return;
        }
        let mut state = self.state.lock().await;
        let mut gone = Vec::new();
        state.agents.retain(|_, slot| {
            let expired = now.saturating_duration_since(slot.touched).as_millis() as u64
                >= AGENT_PRESENCE_TTL_MS;
            if expired {
                gone.push(slot.id);
            }
            !expired
        });
        self.has_agents
            .store(!state.agents.is_empty(), Ordering::Relaxed);
        if gone.is_empty() {
            return;
        }
        // The same removal a disconnect broadcasts: the JSON string "null" at
        // a clock one past the last one seen, which is how the chip leaves
        // every strip in the room.
        let mut clients = HashMap::new();
        for id in gone {
            let clock = state.awareness.meta(id).map(|meta| meta.0 + 1).unwrap_or(1);
            state.awareness.remove_state(id);
            clients.insert(
                id,
                AwarenessUpdateEntry {
                    clock,
                    json: "null".into(),
                },
            );
        }
        let _ = self.tx.send(Frame {
            from: None,
            to: None,
            bytes: Bytes::from(Message::Awareness(AwarenessUpdate { clients }).encode_v1()),
        });
    }

    /// Drop a connection: null + broadcast its awareness states. True = last one.
    pub async fn remove_conn(&self, conn: ConnId) -> bool {
        let mut state = self.state.lock().await;
        state.guests.remove(&conn);
        state.authors.remove(&conn);
        state.viewers.remove(&conn);
        self.has_guests
            .store(!state.guests.is_empty(), Ordering::Relaxed);
        let ids = state.conns.remove(&conn).unwrap_or_default();
        if !ids.is_empty() {
            // Null this connection's awareness states for everyone else: the
            // wire form of removal is the JSON string "null" with a clock one
            // past the last one seen (null wins ties).
            let mut clients = HashMap::new();
            for id in ids {
                let clock = state.awareness.meta(id).map(|meta| meta.0 + 1).unwrap_or(1);
                state.awareness.remove_state(id);
                clients.insert(
                    id,
                    AwarenessUpdateEntry {
                        clock,
                        json: "null".into(),
                    },
                );
            }
            let _ = self.tx.send(Frame {
                from: Some(conn),
                to: None,
                bytes: Bytes::from(Message::Awareness(AwarenessUpdate { clients }).encode_v1()),
            });
        }
        state.conns.is_empty()
    }

    /// The domain-relative path this room's document stands at, as the open
    /// resolved it and as every save receipt has reported it since.
    ///
    /// Read by the surface that opened the room, to check that the document it
    /// landed on is the document it decided the caller may be in: a room is
    /// asked for by ADDRESS and a share-link is held on a PATH, and the two
    /// are resolved by different ladders.
    pub async fn path(&self) -> String {
        self.state.lock().await.path.clone()
    }

    /// Whether nobody is connected any more.
    pub async fn is_empty(&self) -> bool {
        self.state.lock().await.conns.is_empty()
    }

    /// The room's text and the checksum that stands for it; see [`LiveText`].
    pub async fn live(&self) -> LiveText {
        let state = self.state.lock().await;
        let text = Self::file_text_locked(&state);
        let checksum = if text == state.last_saved_text {
            state.checksum.clone()
        } else {
            crate::engine::sha256_hex(text.as_bytes())
        };
        let saving =
            !state.closed && !state.final_saved && matches!(state.save_state, SaveStateTag::Ok);
        let waiting =
            !state.closed && !state.final_saved && !matches!(state.save_state, SaveStateTag::Ok);
        LiveText {
            text,
            checksum,
            saving,
            base_checksum: state.checksum.clone(),
            waiting,
        }
    }

    /// The session text, and whether it differs from the last saved text.
    /// The dirty FLAG says "an update arrived"; the equality check is what
    /// stops a no-op session from ever writing (open-then-close never touches
    /// the file, whatever line endings it holds).
    pub async fn snapshot(&self) -> (String, bool) {
        let state = self.state.lock().await;
        let file = Self::file_text_locked(&state);
        let dirty = state.dirty && file != state.last_saved_text;
        (file, dirty)
    }

    /// The session text, read off the locked state: LF, exactly what a save
    /// writes.
    fn file_text_locked(state: &SessionState) -> String {
        // The text handle is taken before the transaction:
        // `get_or_insert_text` opens one of its own, which would deadlock
        // against a read transaction already held here.
        let doc = state.awareness.doc();
        let text = doc.get_or_insert_text(TEXT_NAME);
        let txn = doc.transact();
        text.get_string(&txn)
    }

    /// Whether this room is over: disposed by the registry or poisoned by a
    /// panicked saver pass.
    pub fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Relaxed)
    }

    /// End the room: the saver loop stops on its next tick and every save path
    /// turns into a no-op.
    pub fn dispose(&self) {
        self.disposed.store(true, Ordering::Relaxed);
    }

    /// The registry key this room is filed under right now: the domain, the
    /// document (whose permalink a frontmatter rename may have moved), and
    /// whose document it is.
    pub fn key(&self) -> RoomKey {
        (self.domain.clone(), self.doc(), self.overlay.clone())
    }

    /// The document this room is registered under right now.
    fn doc(&self) -> RoomDoc {
        self.key_doc.lock().expect("key mutex").clone()
    }

    /// The view this room reads and writes through, built fresh per use the
    /// way every other engine caller builds one: a view is a lens over the
    /// engine for the length of one operation, never something to hold.
    fn view(&self) -> Result<DomainView<'_>, EngineError> {
        room_view(&self.engine, &self.domain, self.overlay.as_deref())
    }

    /// Record the permalink the registry just re-filed this room under.
    fn adopt_key(&self, permalink: &str) {
        *self.key_doc.lock().expect("key mutex") = RoomDoc::Engram(permalink.to_string());
    }

    /// One saver pass at `now`: decides whether a save is due and runs it.
    /// Takes `now` so tests drive time synthetically instead of sleeping.
    pub async fn tick_save(&self, now: Instant) {
        // Before the save, so a session whose join ended a moment ago is put
        // outside the draft rather than watching one more save land in it.
        self.evict_ended_joins().await;
        // And before it for the same kind of reason: an agent that has gone
        // quiet leaves the strip on the tick it expires on, rather than one
        // save later.
        self.sweep_agent_presence(now).await;
        let renamed = self.due_save(now).await;
        // The session guard is dropped by now: the rename move takes the
        // registry lock, and the lock order is registry -> session.
        self.adopt_rename(renamed).await;
    }

    /// The saver pass itself, over the locked state. Returns the permalink a
    /// frontmatter rename moved this engram to, for the caller to re-key
    /// outside the guard.
    async fn due_save(&self, now: Instant) -> Option<String> {
        if self.is_disposed() {
            return None;
        }
        let mut state = self.state.lock().await;
        if matches!(state.save_state, SaveStateTag::Conflict) {
            return None; // saving is suspended until the room resolves (Task 7)
        }
        if !state.flush_requested && !state.dirty && state.oldest_unsaved.is_none() {
            // Nothing has arrived at all since the last save, so there is
            // nothing to render or compare. A cheap pre-filter that keeps an
            // idle room free, NOT the debounce: that gates on the text below.
            // An idle room is exactly where the external-change probe belongs.
            return self.maybe_probe(&mut state, now).await;
        }
        let elapsed = |since: Option<Instant>, ms: u64| {
            since.is_some_and(|at| now.saturating_duration_since(at).as_millis() as u64 >= ms)
        };
        // What arms the timers is the TEXT comparison, never the `dirty` flag:
        // a joining provider answers the greeting with a SyncStep2 that marks
        // the session dirty while carrying no edit, and a flag-gated debounce
        // would arm a save on every unedited join.
        let file = Self::file_text_locked(&state);
        let changed = file != state.last_saved_text;
        let due = state.flush_requested
            || (changed && elapsed(state.last_edit, SAVE_DEBOUNCE_MS))
            || (changed && elapsed(state.oldest_unsaved, SAVE_MAX_LAG_MS));
        if !due {
            if changed {
                return None; // the windows are still open; keep typing
            }
            // Nothing effective is pending: disarm what a no-op update set, so
            // the next real edit starts both timers from scratch.
            state.dirty = false;
            // The document is the saved text again: nobody's change is pending.
            state.editors.clear();
            state.last_edit = None;
            state.oldest_unsaved = None;
            if matches!(state.save_state, SaveStateTag::Ok) {
                return self.maybe_probe(&mut state, now).await;
            }
            // A save-blocked session whose text matches the file again has
            // nothing left to warn about: fall through so the state heals.
        } else if !state.flush_requested
            && matches!(state.save_state, SaveStateTag::Failed)
            && !elapsed(state.last_attempt, SAVE_DEBOUNCE_MS)
        {
            // A refused save must not become a hot loop: the edit timers stay
            // armed while the document is unsaveable, so without this every
            // tick would render the document and call the engine again for as
            // long as the author leaves it broken. Retries are spaced one
            // debounce window apart - and an explicit Flush skips the wait, so
            // the Save button always retries at once.
            return None;
        }
        if self.switches_reflection_off_locked(&state) {
            state = match self.close_reflection_rooms_unlocked(state).await {
                Some(state) => state,
                None => return None,
            };
        }
        self.save_locked(&mut state, now).await
    }

    /// Whether this room's text, saved now, switches its domain's Reflection
    /// off: an engram room over the folder's own MANIFEST (never a draft)
    /// whose text does not say on while the switch is on. Read under the
    /// guard; takes no other lock.
    fn switches_reflection_off_locked(&self, state: &SessionState) -> bool {
        matches!(self.doc(), RoomDoc::Engram(_))
            && crate::engine::is_manifest_path(&state.path)
            && self.view().is_ok_and(|view| view.actor().is_none())
            && self
                .engine
                .switches_reflection_off(&self.domain, Some(&Self::file_text_locked(state)))
    }

    /// Close the domain's reflection rooms ahead of this room's save that
    /// switches the Reflection off, then take the guard again. The close
    /// takes the registry, and the lock order is registry, then session, so
    /// the guard is dropped for it: a join holds the registry while it waits
    /// for this room's guard. `None` when the room may not save any more
    /// once the guard is back. A close that the text no longer needs (it
    /// changed meanwhile) costs those rooms their sockets, never their text.
    async fn close_reflection_rooms_unlocked(
        &self,
        state: tokio::sync::MutexGuard<'_, SessionState>,
    ) -> Option<tokio::sync::MutexGuard<'_, SessionState>> {
        drop(state);
        self.engine.close_reflection_rooms(&self.domain).await;
        let state = self.state.lock().await;
        if self.is_disposed() || state.closed || matches!(state.save_state, SaveStateTag::Conflict)
        {
            return None;
        }
        Some(state)
    }

    /// The last participant left, or the daemon is shutting the session down:
    /// land whatever is unsaved, unconditionally due.
    pub async fn final_save(&self) {
        let renamed = self.last_save().await;
        self.adopt_rename(renamed).await;
    }

    /// [`CollabSession::final_save`] over the locked state; see
    /// [`CollabSession::due_save`] for why the rename travels outward.
    async fn last_save(&self) -> Option<String> {
        if self.is_disposed() {
            return None; // a disposed or poisoned room never writes again
        }
        let mut state = self.state.lock().await;
        // Marked on every way out below, under the lock a compose takes too:
        // whatever lands after this is saved by nobody.
        state.final_saved = true;
        if state.closed {
            // The room accepted an external deletion: a final save here would
            // resurrect the file the author agreed to let go.
            return None;
        }
        if matches!(state.save_state, SaveStateTag::Conflict) {
            // An unresolved conflict never saves, and nobody is left to
            // resolve it, so the server keeps the room's side of it.
            self.keep_unsaved_text(&state);
            return None;
        }
        if self.switches_reflection_off_locked(&state) {
            state = match self.close_reflection_rooms_unlocked(state).await {
                Some(state) => state,
                None => {
                    let state = self.state.lock().await;
                    if matches!(state.save_state, SaveStateTag::Conflict) {
                        self.keep_unsaved_text(&state);
                    }
                    return None;
                }
            };
        }
        // The last chance to land this text, so no retry backoff applies.
        self.final_pass.store(true, Ordering::Relaxed);
        let renamed = self.save_locked(&mut state, Instant::now()).await;
        self.final_pass.store(false, Ordering::Relaxed);
        // The last chance was refused or ran into a conflict: nothing will
        // ever save this text again, so the server keeps it rather than drop
        // it.
        if matches!(
            state.save_state,
            SaveStateTag::Failed | SaveStateTag::Conflict
        ) {
            self.keep_unsaved_text(&state);
        }
        renamed
    }

    /// [`CollabSession::keep_refused_text`] when the room holds text its
    /// last save did not write. A reflection room's last pass runs even with
    /// nothing typed (for its ids), and a refusal of THAT loses nothing, so
    /// it keeps nothing.
    fn keep_unsaved_text(&self, state: &SessionState) {
        if Self::file_text_locked(state) != state.last_saved_text {
            self.keep_refused_text(state);
        }
    }

    /// Write a closing room's text that could not be saved to the kept
    /// folder (layout in [`crystalline_core::kept`]) and log where. Best
    /// effort: a failure to write it is a second warning, never a panic.
    fn keep_refused_text(&self, state: &SessionState) {
        use crystalline_core::kept::KeptKind;
        let (kind, path) = match self.doc() {
            RoomDoc::Engram(_) => (KeptKind::Engram, state.path.clone()),
            RoomDoc::Reflection(path) => (KeptKind::Reflection, path),
        };
        let text = Self::file_text_locked(state);
        let Some(kept) = self.engine.kept_dir() else {
            tracing::warn!(
                domain = %self.domain,
                path = %path,
                "a closing room's text could not be saved, and this process knows no state folder to keep it in"
            );
            return;
        };
        match crystalline_core::kept::write(
            &kept,
            &self.domain,
            self.overlay.as_deref(),
            kind,
            &path,
            chrono::Utc::now(),
            &text,
        ) {
            Ok(file) => tracing::warn!(
                domain = %self.domain,
                path = %path,
                kept = %file.display(),
                "a closing room's text could not be saved and is kept here; crystalline kept list shows it"
            ),
            Err(err) => tracing::warn!(
                domain = %self.domain,
                path = %path,
                kept = %kept.display(),
                %err,
                "a closing room's text could not be saved, and keeping it failed too"
            ),
        }
    }

    /// Move the registry entry after a rename, holding no session guard.
    async fn adopt_rename(&self, renamed: Option<String>) {
        let Some(permalink) = renamed else {
            return;
        };
        let Some(registry) = self.registry.upgrade() else {
            return; // the daemon dropped the registry; nothing left to move
        };
        registry.rekey(&self.key(), &permalink, &self.epoch).await;
    }

    /// A panic below yrs killed a saver pass (y-crdt/y-crdt#386): close the
    /// room permanently instead of stalling saves silently. Every socket sees
    /// `Closed { reason: "internal" }` and closes with the permanent code, the
    /// session never saves again, and participants' drafts hold their text.
    pub async fn poison(&self) {
        self.close("internal").await;
    }

    /// End the room and tell every socket why: `Closed { reason }`, after
    /// which each socket closes with the permanent code and the room never
    /// saves again.
    pub async fn close(&self, reason: &str) {
        self.close_with(Control::closed(reason));
    }

    /// [`CollabSession::close`] with the whole `Closed` control, for a close
    /// that names more than its reason (`moved` names the new address).
    fn close_with(&self, closed: Control) {
        self.dispose();
        let _ = self.tx.send(Frame {
            from: None,
            to: None,
            bytes: Bytes::from(control::encode(&closed)),
        });
    }

    /// Tell every connection that the document moved to `to`, each in what
    /// it may see: a connection whose account cannot see the destination
    /// domain is told only that the document moved (`Closed { reason:
    /// "moved" }` with no address), so the name of a domain it may not see
    /// never reaches it. A move inside one domain names nothing new, so
    /// everybody is told the whole address.
    pub async fn close_moved(&self, to: &MovedTo) {
        let full = to.control();
        if to.domain.is_none() {
            self.close_with(full);
            return;
        }
        // Every connection, with who it is where that is known: one with no
        // entry is somebody unknown and is told no address (fail closed).
        let viewers: Vec<(ConnId, Option<crate::scope::Scope>)> = {
            let state = self.state.lock().await;
            state
                .conns
                .keys()
                .map(|conn| (*conn, state.viewers.get(conn).cloned()))
                .collect()
        };
        let mut blind = HashSet::new();
        for (conn, scope) in viewers {
            let sees = match &scope {
                Some(scope) => sees_destination(&self.engine, to, scope).await,
                None => false,
            };
            if !sees {
                blind.insert(conn);
            }
        }
        if blind.is_empty() {
            self.close_with(full);
            return;
        }
        self.dispose();
        let conns: Vec<ConnId> = self.state.lock().await.conns.keys().copied().collect();
        for conn in conns {
            let control = if blind.contains(&conn) {
                Control::Closed {
                    reason: "moved".to_string(),
                    domain: None,
                    path: None,
                    permalink: None,
                }
            } else {
                full.clone()
            };
            let _ = self.tx.send(Frame {
                from: None,
                to: Some(conn),
                bytes: Bytes::from(control::encode(&control)),
            });
        }
    }

    /// The room's document as its last save left it: its path, and for an
    /// engram the permalink it answers to.
    async fn address(&self) -> (String, Option<String>) {
        let state = self.state.lock().await;
        match self.doc() {
            RoomDoc::Engram(_) => (state.path.clone(), Some(state.permalink.clone())),
            RoomDoc::Reflection(path) => (path, None),
        }
    }

    /// The save pass over the locked state: one attempt, and - when the file
    /// moved under the session - the merge flow plus the retry that lands the
    /// merged text over it.
    ///
    /// Returns the permalink the receipt reports when a frontmatter rename
    /// moved it: the caller re-keys the registry once the guard is gone.
    async fn save_locked(&self, state: &mut SessionState, now: Instant) -> Option<String> {
        // Two attempts at most. The merge adopts the external file's checksum
        // as the CAS token, so the retry lands unless the file changed AGAIN
        // inside that window - and then the next tick picks it up rather than
        // spinning here with the guard held.
        for _ in 0..2 {
            match self.save_attempt(state, now).await {
                SaveOutcome::Done(renamed) => return renamed,
                SaveOutcome::Deleted(detail) => {
                    self.raise_deleted(state, detail);
                    return None;
                }
                SaveOutcome::External(detail) => {
                    let view = match self.view() {
                        Ok(view) => view,
                        Err(err) => {
                            self.fail_save(state, err.to_string());
                            return None;
                        }
                    };
                    let theirs = match self.read_doc(&view, state).await {
                        Ok(theirs) => theirs,
                        // The engram the CAS refused is not there to read: the
                        // write and the delete raced, so this is the deletion.
                        Err(EngineError::NotFound(detail)) => {
                            self.raise_deleted(state, detail);
                            return None;
                        }
                        Err(err) => {
                            self.fail_save(state, err.to_string());
                            return None;
                        }
                    };
                    if !self.merge_external(state, theirs, detail).await {
                        return None; // a conflict the room has to resolve
                    }
                }
            }
        }
        None
    }

    /// One save attempt over the locked state, at `now`. Holding the lock
    /// across the engine call serializes edits against the save, so the text
    /// that lands is exactly the text recorded as saved; sessions are small
    /// and a save is milliseconds, so simplicity wins over concurrency here.
    async fn save_attempt(&self, state: &mut SessionState, now: Instant) -> SaveOutcome {
        if self.is_disposed() || state.closed {
            // Disposed, poisoned, or closed by an accepted deletion: this
            // session is done writing.
            return SaveOutcome::Done(None);
        }
        let file = Self::file_text_locked(state);
        state.flush_requested = false;
        // A reflection room's ids come with its last save, so that save runs
        // even when nothing new was typed since the one before it.
        let closing_reflection =
            matches!(self.doc(), RoomDoc::Reflection(_)) && self.final_pass.load(Ordering::Relaxed);
        if file == state.last_saved_text && !closing_reflection {
            // Nothing effective changed: never write, never touch the mtime.
            state.dirty = false;
            // The document is the saved text again: nobody's change is pending.
            state.editors.clear();
            state.oldest_unsaved = None;
            if !matches!(state.save_state, SaveStateTag::Ok) {
                // The document matches the file again, so the standing save
                // failure is over; the room is told so its alert clears.
                state.save_state = SaveStateTag::Ok;
                state.failure_detail = None;
                let _ = self.tx.send(Frame {
                    from: None,
                    to: None,
                    bytes: Bytes::from(control::encode(&Control::Saved {
                        checksum: state.checksum.clone(),
                        permalink: state.permalink.clone(),
                    })),
                });
            }
            return SaveOutcome::Done(None);
        }
        state.last_attempt = Some(now);
        // Normalize before the save ever sees this text, and converge the
        // document to match: a cross-domain link spelled with an alias or a
        // non-canonical local name is rewritten to its canonical one, the
        // same funnel every other write's final text passes through. Done
        // HERE rather than left to the engine's own pass inside `save_engram`
        // for two reasons. Every connected client has to see the same bytes
        // that are about to land, not keep showing the spelling as typed
        // while the file already reads differently. And `last_saved_text`
        // below has to agree with the checksum the receipt hands back: both
        // are set from the SAME normalized text, so the next tick's compare
        // (`file == state.last_saved_text`, above) can go quiet once nothing
        // new has been typed. Setting `last_saved_text` to the room's own
        // pre-normalization text instead - without converging the document -
        // would leave that compare false forever, since the live text itself
        // never gained the respelling, and the room would save on every
        // debounce window for good.
        let file = match self.normalize_doc(&file).await {
            Ok((normalized, count)) => {
                if count > 0 {
                    self.converge(state, &to_lf(&normalized));
                }
                normalized
            }
            Err(err) => {
                self.fail_save(state, err.to_string());
                return SaveOutcome::Done(None);
            }
        };
        if self.doc_switches_read_only_on(state, &file).await {
            self.fail_save(state, ROOM_SWITCH_ON_REFUSAL.to_string());
            return SaveOutcome::Done(None);
        }
        let receipt = self.write_doc(state, &file).await;
        match receipt {
            Ok(receipt) => {
                let checksum = receipt["checksum"].as_str().unwrap_or_default().to_string();
                // The permalink the engram answers to AFTER the write: an
                // author who edited the frontmatter line just moved the
                // address, and the next save must use the new one.
                let permalink = receipt["permalink"]
                    .as_str()
                    .unwrap_or(&state.permalink)
                    .to_string();
                let renamed = (permalink != state.permalink).then(|| permalink.clone());
                // Checksum and text move together: the checksum is the CAS
                // token for exactly this text, and it is also what a
                // reconnecting client is greeted with.
                state.checksum = checksum.clone();
                self.land_text(state, file, closing_reflection);
                state.permalink = permalink.clone();
                // The path a rename can move too (the receipt is the authority
                // on where the engram now lives), so Task 7's idle probe never
                // stats a dead path.
                if let Some(path) = receipt["path"].as_str() {
                    state.path = path.to_string();
                }
                state.dirty = false;
                state.oldest_unsaved = None;
                state.save_state = SaveStateTag::Ok;
                state.failure_detail = None;
                let _ = self.tx.send(Frame {
                    from: None,
                    to: None,
                    bytes: Bytes::from(control::encode(&Control::Saved {
                        checksum,
                        permalink,
                    })),
                });
                SaveOutcome::Done(renamed)
            }
            // The CAS token no longer matches the file: somebody else wrote
            // it while this room was editing. Not a failure - the merge flow
            // pulls their work in.
            Err(EngineError::Conflict(detail)) if detail.contains("stale edit") => {
                SaveOutcome::External(detail)
            }
            // The engram is not there to save: an external delete, which is
            // its own conflict kind rather than a save-blocked room.
            Err(EngineError::NotFound(detail)) => SaveOutcome::Deleted(detail),
            Err(err) => {
                // Every other refusal is save-blocked. The session stays open
                // and editable, and every later flush retries: dirty and
                // oldest_unsaved stand.
                self.fail_save(state, err.to_string());
                SaveOutcome::Done(None)
            }
        }
    }

    /// Whether landing `text` at `path` would switch this room's domain to
    /// read-only: the path is the MANIFEST, the base MANIFEST says off (a
    /// missing one reads as off), and the text says on. Read against the
    /// base, never a draft, the same transition the engine's own check reads.
    async fn room_switches_read_only_on(&self, path: &str, text: &str) -> bool {
        crate::engine::is_manifest_path(path)
            && self.engine.switches_read_only_on(&self.domain, text).await
    }

    /// Record and announce a save refusal that is nobody's conflict.
    ///
    /// The re-broadcast is guarded on the DETAIL as well as the state: a
    /// parse refusal replacing an io failure (or either replacing a resolved
    /// conflict) has to reach the room, or its alert keeps naming a reason
    /// that no longer applies.
    ///
    /// `detail` comes from the caller: the engine's own refusal of the save
    /// (`err.to_string()`), so a move-blocked author's room says the exact
    /// sentence [`crate::engine::joined_write_is_elsewhere`] gives the request
    /// path, or a room constant such as `ROOM_SWITCH_ON_REFUSAL` when the room
    /// itself refuses. It is never composed here.
    fn fail_save(&self, state: &mut SessionState, detail: String) {
        let repeat = matches!(state.save_state, SaveStateTag::Failed)
            && state.failure_detail.as_deref() == Some(detail.as_str());
        if !repeat {
            let _ = self.tx.send(Frame {
                from: None,
                to: None,
                bytes: Bytes::from(control::encode(&Control::SaveFailed {
                    detail: detail.clone(),
                })),
            });
        }
        tracing::debug!(domain = %self.domain, permalink = %state.permalink, %detail, "a session save was refused");
        state.save_state = SaveStateTag::Failed;
        state.failure_detail = Some(detail);
    }

    /// A restore of the room's text that cannot run: the deletion stays the
    /// open question, and the room is told why its "mine" did not land.
    fn refuse_restore(&self, state: &mut SessionState, detail: String) {
        state.pending = Some(PendingConflict::Deleted);
        let _ = self.tx.send(Frame {
            from: None,
            to: None,
            bytes: Bytes::from(control::encode(&Control::SaveFailed { detail })),
        });
    }

    /// Suspend saving on an externally deleted engram and let the room decide
    /// between restoring its text and accepting the deletion.
    fn raise_deleted(&self, state: &mut SessionState, detail: String) {
        state.save_state = SaveStateTag::Conflict;
        state.failure_detail = None;
        state.pending = Some(PendingConflict::Deleted);
        let _ = self.tx.send(Frame {
            from: None,
            to: None,
            bytes: Bytes::from(control::encode(&Control::Conflict {
                conflict_kind: "deleted".to_string(),
                theirs: None,
                detail,
            })),
        });
    }

    /// The merge flow for an external write: three-way in LF space against
    /// the text this session last saw on disk. A clean merge flows straight
    /// into the live document (true, so the caller writes the result back);
    /// a collision suspends saving and hands the room both sides (false).
    ///
    /// Conflict markers never enter this path: `three_way` discards diffy's
    /// marked text, so nothing here can carry a marker into the document or
    /// on to the file.
    async fn merge_external(
        &self,
        state: &mut SessionState,
        theirs: EngramText,
        detail: String,
    ) -> bool {
        let mine = Self::file_text_locked(state);
        match merge::three_way(&state.last_saved_text, &mine, &theirs.content) {
            MergeOutcome::Clean(merged) => {
                self.converge(state, &merged);
                // Their text is what the file holds now, so it is the base of
                // the next merge and its checksum is the next CAS token.
                state.last_saved_text = to_lf(&theirs.content).into_owned();
                state.checksum = theirs.checksum;
                // The save state is deliberately left alone: the save that
                // follows this merge owns the whole failed/ok lifecycle, and
                // it is the one that knows whether the merged text lands.
                true
            }
            MergeOutcome::Conflict => {
                self.raise_edit(state, theirs, detail);
                false
            }
        }
    }

    /// The idle external-change probe: once a window, while the room is clean
    /// and saving healthy, ask the engine whether the file moved under it.
    ///
    /// ACCEPTED LIMITATION: `engram_text` reads through `load_content`, which
    /// serves a non-host or virtual read from the store's content column, so
    /// an external change (a deletion especially) can be detected only after
    /// the sync engine has reindexed it - the probe may run late, and the save
    /// CAS remains the hard guard. Do not "fix" the probe by reading the file
    /// directly; the engine owns path resolution.
    async fn maybe_probe(&self, state: &mut SessionState, now: Instant) -> Option<String> {
        if state.closed || state.dirty || !matches!(state.save_state, SaveStateTag::Ok) {
            return None;
        }
        let due = state.last_probe.is_some_and(|at| {
            now.saturating_duration_since(at).as_millis() as u64 >= IDLE_CHECK_MS
        });
        if !due {
            return None;
        }
        state.last_probe = Some(now);
        let Ok(view) = self.view() else {
            return None;
        };
        match self.read_doc(&view, state).await {
            Ok(theirs) if theirs.checksum != state.checksum => {
                let detail = format!(
                    "'{}' changed on disk while this session was idle",
                    state.permalink
                );
                if self.merge_external(state, theirs, detail).await {
                    // Defensive, and not reached today: the probe runs only
                    // while the room's text is the saved text, so a merge here
                    // is the file's own text, which does not switch anything.
                    // Should that change, a merged MANIFEST that switches the
                    // Reflection off is left to the next tick, which closes the
                    // reflection rooms with this guard dropped (see
                    // `due_save`): the guard is held here, and the close takes
                    // the registry.
                    if self.switches_reflection_off_locked(state) {
                        state.flush_requested = true;
                        return None;
                    }
                    // A clean merge over an unedited room IS their text, so
                    // this writes nothing; a room that edited between the last
                    // save and the probe has its merged text landed instead.
                    return self.save_locked(state, now).await;
                }
                None
            }
            Ok(_) => None,
            Err(EngineError::NotFound(detail)) => {
                self.raise_deleted(state, detail);
                None
            }
            Err(err) => {
                // A probe is best-effort: a read that fails leaves the session
                // exactly as it was, and the save CAS still guards the write.
                tracing::debug!(domain = %self.domain, permalink = %state.permalink, %err, "the idle collab probe could not read the engram");
                None
            }
        }
    }

    /// Resolve a standing conflict with the room's choice. The first resolve
    /// wins - it takes the pending conflict - and a resolve with none pending
    /// is ignored, so a stale button press can never overwrite a file.
    ///
    /// Returns the permalink a restore reports when it landed under a new one.
    async fn resolve_conflict(&self, state: &mut SessionState, choice: &str) -> Option<String> {
        if !matches!(state.save_state, SaveStateTag::Conflict) {
            return None;
        }
        let pending = state.pending.take()?;
        match (choice, pending) {
            (
                "mine",
                PendingConflict::Edit {
                    theirs,
                    theirs_checksum,
                },
            ) => {
                // Checksum and text move together, as everywhere else: their
                // version is what the file holds, so it is both the CAS token
                // my text lands over and the base the next merge diffs
                // against. Adopting only the checksum would re-offer the very
                // edit this room just rejected, and would leave a session that
                // never edited (a mixed-endings theirs) believing its choice
                // landed while the save found nothing to write.
                state.checksum = theirs_checksum;
                state.last_saved_text = theirs;
                state.save_state = SaveStateTag::Ok;
                state.failure_detail = None;
                state.flush_requested = true;
                None
            }
            ("mine", PendingConflict::Deleted) => self.restore_mine(state).await,
            (
                "theirs",
                PendingConflict::Edit {
                    theirs,
                    theirs_checksum,
                },
            ) => {
                // Their version wins whole: the live text becomes the file's,
                // and this room's unsaved edits are what the author gave up.
                self.converge(state, &to_lf(&theirs));
                state.last_saved_text = theirs;
                state.checksum = theirs_checksum;
                state.dirty = false;
                state.last_edit = None;
                state.oldest_unsaved = None;
                state.flush_requested = false;
                state.save_state = SaveStateTag::Ok;
                state.failure_detail = None;
                None
            }
            ("theirs", PendingConflict::Deleted) => {
                // The deletion stands: nothing is written back, the room is
                // told, and the socket loop closes every connection.
                state.closed = true;
                let _ = self.tx.send(Frame {
                    from: None,
                    to: None,
                    bytes: Bytes::from(control::encode(&Control::closed("deleted"))),
                });
                None
            }
            (_, pending) => {
                // An unknown choice decides nothing; the conflict stands.
                state.pending = Some(pending);
                None
            }
        }
    }

    /// "Mine" over an external deletion: put this room's text back where the
    /// engram was, but only over ground that is still empty.
    ///
    /// "Deleted" means "the engram is not there to save", which an external
    /// RENAME, a delete-and-recreate, or a probe reading a stale index all
    /// produce while a file sits at that path holding somebody else's work.
    /// Restoring is a plain overwrite with no CAS to stop it, so the path is
    /// read first and anything that is not the text this room last saved
    /// re-opens as an edit conflict instead. A session never silently
    /// overwrites external work.
    async fn restore_mine(&self, state: &mut SessionState) -> Option<String> {
        let Ok(view) = self.view() else {
            return None;
        };
        match self.peek_doc(&view, state).await {
            Ok(Some(theirs)) if to_lf(&theirs.content) != state.last_saved_text => {
                let detail = format!(
                    "'{}' is on disk again with somebody else's text, so restoring \
                     would overwrite it; pick again with their version in view",
                    state.path
                );
                self.raise_edit(state, theirs, detail);
                return None;
            }
            // Nothing there, or exactly the text this room last saved: the
            // restore puts back what was lost and overwrites nobody.
            Ok(_) => {}
            Err(err) => {
                // The path could not be read, so nothing is known about what
                // is there: the conflict stands rather than risking a blind
                // overwrite.
                self.refuse_restore(state, err.to_string());
                return None;
            }
        }
        // save_engram refuses a missing file by design, so the room's text
        // goes back through the restore verb instead.
        let file = Self::file_text_locked(state);
        // Normalized and converged first, exactly as `save_attempt` does and
        // for the same reason: `restore_engram` itself is deliberately exempt
        // (it restores the prior text as it is apart from its line endings,
        // see its own doc comment), so this
        // is the one place that has to make sure what gets restored, what the
        // document shows and what `last_saved_text` is set to below all agree
        // on the canonical spelling, rather than restoring - and then living
        // with - whatever the room typed before it ever saved.
        let file = match self.normalize_doc(&file).await {
            Ok((normalized, count)) => {
                if count > 0 {
                    self.converge(state, &to_lf(&normalized));
                }
                normalized
            }
            Err(err) => {
                self.refuse_restore(state, err.to_string());
                return None;
            }
        };
        // The restore writes as the machine owner too (base arm) or through
        // the room's view (draft arm), so the room is the only place that can
        // refuse a switch-on here.
        if self.doc_switches_read_only_on(state, &file).await {
            self.refuse_restore(state, ROOM_SWITCH_ON_REFUSAL.to_string());
            return None;
        }
        let restored = self.restore_doc(&view, state, &file).await;
        match restored {
            Ok(receipt) => {
                let checksum = receipt["checksum"].as_str().unwrap_or_default().to_string();
                let permalink = receipt["permalink"]
                    .as_str()
                    .unwrap_or(&state.permalink)
                    .to_string();
                let renamed = (permalink != state.permalink).then(|| permalink.clone());
                state.checksum = checksum.clone();
                // A restore never mints ids.
                self.land_text(state, file, false);
                state.permalink = permalink.clone();
                if let Some(path) = receipt["path"].as_str() {
                    state.path = path.to_string();
                }
                state.dirty = false;
                state.oldest_unsaved = None;
                state.save_state = SaveStateTag::Ok;
                state.failure_detail = None;
                let _ = self.tx.send(Frame {
                    from: None,
                    to: None,
                    bytes: Bytes::from(control::encode(&Control::Saved {
                        checksum,
                        permalink,
                    })),
                });
                renamed
            }
            Err(err) => {
                // The restore was refused (a document that is not an engram, a
                // read-only daemon): the conflict stands so the room can try
                // the other resolution.
                self.refuse_restore(state, err.to_string());
                None
            }
        }
    }

    /// Suspend saving on colliding edits and hand the room both sides.
    fn raise_edit(&self, state: &mut SessionState, theirs: EngramText, detail: String) {
        state.save_state = SaveStateTag::Conflict;
        state.failure_detail = None;
        // Their text as LF, the only form a room shows or keeps; the checksum
        // stays the one of the bytes they wrote.
        let theirs_text = to_lf(&theirs.content).into_owned();
        state.pending = Some(PendingConflict::Edit {
            theirs: theirs_text.clone(),
            theirs_checksum: theirs.checksum,
        });
        let _ = self.tx.send(Frame {
            from: None,
            to: None,
            bytes: Bytes::from(control::encode(&Control::Conflict {
                conflict_kind: "edit".to_string(),
                theirs: Some(theirs_text),
                detail,
            })),
        });
    }

    /// Morph the live text into `target` (LF text) as one minimal edit
    /// script in ONE transaction - every client sees a single update rather
    /// than a flicker of half-applied lines - broadcast that update to the
    /// room and tell it the external change is in.
    fn converge(&self, state: &mut SessionState, target: &str) {
        self.converge_with(state, target, None)
    }

    /// [`CollabSession::converge`], naming who produced the change.
    ///
    /// `origin` tags the yrs transaction. It is local to this process - an
    /// encoded update carries no origin - so it is for an in-process observer
    /// rather than for the clients, which see the same remote update either
    /// way. `None` is the external-change merge, whose author is a file.
    fn converge_with(&self, state: &mut SessionState, target: &str, origin: Option<&str>) {
        let update = {
            let doc = state.awareness.doc();
            // Taken before the transaction: get_or_insert_text opens one of
            // its own and would deadlock inside ours.
            let text = doc.get_or_insert_text(TEXT_NAME);
            let mut txn = match origin {
                Some(origin) => doc.transact_mut_with(origin),
                None => doc.transact_mut(),
            };
            let current = text.get_string(&txn);
            merge::apply_target(&text, &mut txn, &current, target);
            txn.encode_update_v1()
        };
        let _ = self.tx.send(Frame {
            from: None,
            to: None,
            bytes: Bytes::from(Message::Sync(SyncMessage::Update(update)).encode_v1()),
        });
        let _ = self.tx.send(Frame {
            from: None,
            to: None,
            bytes: Bytes::from(control::encode(&Control::Merged)),
        });
    }

    /// Take in a change written to the room's file by an engine verb (see
    /// [`CollabSessions::absorb_doc_change`]). The idle probe's merge, run at
    /// once: a file the room has already taken in (its checksum is the
    /// room's, because the room saved in between) is left alone, a room with
    /// a standing conflict or failure meets the file on its next save, and
    /// otherwise the file is merged three ways into the live text. A merge
    /// that kept unsaved typing arms the save timers, as typing does.
    ///
    /// A room whose final save has run is ended for this purpose: it saves
    /// nothing again, so text merged into it would be reported as taken in
    /// while it lands nowhere.
    async fn absorb_file_change(&self) -> Result<AbsorbOutcome, String> {
        if self.is_disposed() {
            return Err("this co-editing session ended while the write was being prepared".into());
        }
        let mut state = self.state.lock().await;
        if state.closed || state.final_saved {
            return Err(
                "this co-editing session ended while the write was being prepared".to_string(),
            );
        }
        let absorbed = if !matches!(state.save_state, SaveStateTag::Ok) {
            Absorbed::Deferred
        } else {
            let view = self.view().map_err(|e| e.to_string())?;
            match self.read_doc(&view, &state).await {
                Ok(theirs) if theirs.checksum != state.checksum => {
                    let detail = format!(
                        "'{}' was changed by an agent while this room was open",
                        state.permalink
                    );
                    if self.merge_external(&mut state, theirs, detail).await {
                        if Self::file_text_locked(&state) != state.last_saved_text {
                            let now = Instant::now();
                            state.dirty = true;
                            state.last_edit = Some(now);
                            state.oldest_unsaved.get_or_insert(now);
                        }
                        Absorbed::Merged
                    } else {
                        Absorbed::Conflict
                    }
                }
                Ok(_) => Absorbed::Merged,
                // Gone again already: the room's save or probe raises it.
                Err(EngineError::NotFound(_)) => Absorbed::Deferred,
                Err(err) => return Err(err.to_string()),
            }
        };
        Ok(AbsorbOutcome {
            absorbed,
            participants: Self::participants_locked(&state, None),
        })
    }

    /// Compose an agent's text into this room's document.
    ///
    /// The engine computed `target` from this room's own live text a moment
    /// ago (and compared the caller's `expected_checksum` against it), so the
    /// diff applied here is the agent's edit and nothing else. Everything the
    /// room does with a typed change it does with this one: the update is
    /// broadcast to every socket, the merge notice tells the editor its
    /// document moved under it, and the save timers are armed so the text
    /// lands on the ordinary debounce.
    ///
    /// The lock is held across the whole of it, as every other write on this
    /// room is, so an agent's edit and a save cannot interleave.
    async fn apply_agent_text(
        &self,
        target: &str,
        origin: &str,
        peer: Option<&AgentPeer>,
    ) -> Result<LiveApplied, String> {
        if self.is_disposed() {
            return Err("this co-editing session ended while the write was being prepared".into());
        }
        let mut state = self.state.lock().await;
        if state.closed {
            return Err(
                "this co-editing session ended while the write was being prepared".to_string(),
            );
        }
        Ok(self.compose_locked(&mut state, target, origin, peer))
    }

    /// [`CollabSession::apply_agent_text`] for
    /// [`CollabSessions::compose_while_saving`]: refused, with nothing
    /// composed, unless the room will save the text and still holds `base`.
    async fn apply_agent_text_while_saving(
        &self,
        base: Option<&str>,
        target: &str,
        origin: &str,
        peer: Option<&AgentPeer>,
    ) -> Result<LiveApplied, ComposeRefused> {
        if self.is_disposed() {
            return Err(ComposeRefused::NotSaving);
        }
        let mut state = self.state.lock().await;
        if state.closed || state.final_saved || !matches!(state.save_state, SaveStateTag::Ok) {
            return Err(ComposeRefused::NotSaving);
        }
        if base.is_some_and(|base| Self::file_text_locked(&state) != base) {
            return Err(ComposeRefused::Moved);
        }
        Ok(self.compose_locked(&mut state, target, origin, peer))
    }

    /// The compose itself, over the locked state: converge the document to
    /// `target`, arm the saver and stand the agent in the strip.
    fn compose_locked(
        &self,
        state: &mut SessionState,
        target: &str,
        origin: &str,
        peer: Option<&AgentPeer>,
    ) -> LiveApplied {
        // The room's text is LF only, whatever line endings the engine verb's
        // text carried.
        let target = to_lf(target);
        self.converge_with(state, &target, Some(origin));
        // Armed exactly as an update from a socket arms it (see
        // `CollabSession::handle_frame`), so the agent's text saves on the
        // same debounce a person's typing does. Forcing a flush instead would
        // make an agent's edit the one write on this instance that lands
        // mid-composition.
        let now = Instant::now();
        state.dirty = true;
        state.last_edit = Some(now);
        state.oldest_unsaved.get_or_insert(now);
        // The agent joins the strip under the same guard its text landed
        // under, so a person sees the chip and the change together.
        let mine = peer.and_then(|peer| self.touch_agent_locked(state, peer));
        LiveApplied {
            // Everybody in the room EXCEPT this agent. `present` is what the
            // agent is told about who is in there with it, and its own slot -
            // minted a line ago, or standing from a call a moment ago - is not
            // news to the one that put it there. Another agent's is.
            participants: Self::participants_locked(state, mine),
        }
    }

    /// Who is in this room, by the name their client publishes in awareness.
    pub async fn participants(&self, except: Option<ClientID>) -> Vec<String> {
        Self::participants_locked(&*self.state.lock().await, except)
    }

    /// [`CollabSession::participants`] over the locked state.
    ///
    /// Read off awareness rather than off the connection map, because the
    /// connection map holds ids and this is for a person to read. A client
    /// that publishes no name at all (a provider that never set one, a socket
    /// that has not sent its first awareness frame) contributes nothing rather
    /// than an invented placeholder: the list says who is known to be there,
    /// not how many sockets are open.
    ///
    /// Sorted and de-duplicated, so one person in two windows is one name and
    /// two calls a second apart do not reorder the same room.
    ///
    /// `except` is the one slot the caller is not asking about: an agent
    /// asking who is in the room with it leaves itself out, however many times
    /// it has been in here already.
    fn participants_locked(state: &SessionState, except: Option<ClientID>) -> Vec<String> {
        let mut names: Vec<String> = state
            .awareness
            .iter()
            .filter(|(id, _)| Some(*id) != except)
            .filter_map(|(_, client)| client.data)
            .filter_map(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .filter_map(|value| {
                value
                    .get("user")
                    .and_then(|user| user.get("name"))
                    .and_then(|name| name.as_str())
                    .map(str::to_string)
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Whether the room accepted an external deletion: the session is over,
    /// and the socket loop closes every connection with the permanent code.
    pub async fn is_closed(&self) -> bool {
        self.state.lock().await.closed
    }
}

/// What one [`CollabSession::save_attempt`] did.
enum SaveOutcome {
    /// Nothing needed writing, or the write landed - carrying the permalink a
    /// frontmatter rename moved the engram to.
    Done(Option<String>),
    /// The file changed under the session; the detail is the CAS refusal.
    External(String),
    /// The file is gone; the detail says so.
    Deleted(String),
}

/// The engine-bound steps of a room, each dispatched on the document the room
/// is over. Every match is exhaustive, so a new kind of document names each
/// step it has to answer.
impl CollabSession {
    /// The text a room opens over (`open`).
    async fn load_doc(engine: &Engine, key: &RoomKey) -> Result<EngramText, EngineError> {
        match &key.1 {
            RoomDoc::Engram(permalink) => {
                room_view(engine, &key.0, key.2.as_deref())?
                    .engram_text(&room_address(&key.0, permalink))
                    .await
            }
            RoomDoc::Reflection(path) => engine.reflection_room_text(&key.0, path).await,
        }
    }

    /// The document as stored now, through the room's view (the external
    /// change arm of `save_locked` and the idle probe).
    async fn read_doc(
        &self,
        view: &DomainView<'_>,
        state: &SessionState,
    ) -> Result<EngramText, EngineError> {
        match self.doc() {
            RoomDoc::Engram(_) => {
                view.engram_text(&room_address(&self.domain, &state.permalink))
                    .await
            }
            // No overlay: the view is not consulted.
            RoomDoc::Reflection(path) => {
                self.engine.reflection_room_text(&self.domain, &path).await
            }
        }
    }

    /// What stands at the room's path before a restore writes it back.
    async fn peek_doc(
        &self,
        view: &DomainView<'_>,
        state: &SessionState,
    ) -> Result<Option<EngramText>, EngineError> {
        match self.doc() {
            RoomDoc::Engram(_) => view.engram_text_at_path(&state.path).await,
            RoomDoc::Reflection(path) => {
                match self.engine.reflection_room_text(&self.domain, &path).await {
                    Ok(text) => Ok(Some(text)),
                    Err(EngineError::NotFound(_)) => Ok(None),
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// The room's text with its cross-domain spellings made canonical.
    async fn normalize_doc(&self, file: &str) -> Result<(String, usize), EngineError> {
        match self.doc() {
            RoomDoc::Engram(_) => {
                self.engine
                    .normalize_domain_spellings_for(file, &crate::scope::Scope::Unrestricted)
                    .await
            }
            // A reflection is never parsed as an engram: the body gate splits
            // off the frontmatter, as a reflection write's does.
            RoomDoc::Reflection(_) => {
                self.engine
                    .normalize_domain_spellings_in(
                        file,
                        crystalline_core::relink::BodyGate::Split,
                        &crate::scope::Scope::Unrestricted,
                    )
                    .await
            }
        }
    }

    /// Whether the save would switch the domain to read-only.
    async fn doc_switches_read_only_on(&self, state: &SessionState, file: &str) -> bool {
        match self.doc() {
            RoomDoc::Engram(_) => self.room_switches_read_only_on(&state.path, file).await,
            // A reflection is never the MANIFEST.
            RoomDoc::Reflection(_) => false,
        }
    }

    /// The save itself; answers the receipt a save reads its checksum,
    /// permalink and path off.
    async fn write_doc(
        &self,
        state: &SessionState,
        file: &str,
    ) -> Result<serde_json::Value, EngineError> {
        match self.doc() {
            RoomDoc::Engram(_) => {
                let params = crate::params::SaveParams {
                    domain: self.domain.clone(),
                    identifier: room_address(&self.domain, &state.permalink),
                    content: file.to_string(),
                    expected_checksum: state.checksum.clone(),
                };
                // Whose save this is, which is whose document the room is over. A room
                // over one actor's draft writes that actor's row and nothing else -
                // not the folder the team reviewed, and not the draft of whoever
                // happens to be typing. A room over the document a direct domain keeps
                // saves as the machine owner, exactly as every room did before there
                // was anything else to be: the surface that opened it is the gate, as
                // it is for every other write on this instance.
                let receipt = match self.view() {
                    // The VIEW decides, never the key on its own: a domain that has
                    // stopped reviewing changes has no draft left for this room to be
                    // over, and its text belongs in the folder (see `room_view`).
                    Ok(view) if view.actor().is_some() => {
                        // The path this room opened on, which is the only one it
                        // writes. Unreachable as a `None` here - an overlay room is
                        // the only kind whose view carries an actor - and answered
                        // with the room's current path rather than by unwrapping.
                        let pinned = self
                            .pinned_path
                            .clone()
                            .unwrap_or_else(|| state.path.clone());
                        self.engine
                            .save_engram_in_overlay(&view, &params, &pinned)
                            .await
                    }
                    Ok(_) => self.engine.save_engram_from_room(&params).await,
                    Err(err) => Err(err),
                };
                if receipt.is_ok() {
                    // A human just taught this domain something through the
                    // editor, which reaches the engine directly rather than
                    // through the REST write handler: the domain owes a
                    // consolidation sweep exactly as it would from a PUT, and
                    // this is the surface most human authoring actually comes
                    // through.
                    crate::maintenance::record_pending(&self.domain);
                }
                receipt
            }
            // No `record_pending`: a Reflection edit owes no consolidation
            // sweep.
            RoomDoc::Reflection(path) => {
                let authors = Self::reflection_authors_locked(state);
                self.engine
                    .save_reflection_room(
                        &self.domain,
                        &path,
                        file,
                        &state.checksum,
                        &authors,
                        self.final_pass.load(Ordering::Relaxed),
                    )
                    .await
            }
        }
    }

    /// Who a reflection room's save records as having changed a section:
    /// the signed-in accounts whose connections sent an edit since the last
    /// save landed, and for a section that gets its id in this save also
    /// the accounts that typed its heading. Nobody else in the room, and no
    /// name a client put into awareness. "someone" only for a section with
    /// nobody named while nobody is connected, such as the close of a room
    /// whose edits all landed before; a save with people connected and no
    /// edit since the last one names nobody.
    fn reflection_authors_locked(state: &SessionState) -> SectionAuthors {
        SectionAuthors {
            changed: state.editors.iter().cloned().collect(),
            new_by_heading: state.new_heading_authors.clone(),
            someone_if_empty: state.conns.is_empty(),
        }
    }

    /// Record `file` as the text that just landed, by a save or a restore,
    /// and clear `editors`, whose change it holds now. In a reflection room
    /// the names of who typed a heading without an id are carried first
    /// ([`CollabSession::carry_new_heading_authors`]), or dropped when this
    /// landing `minted` the ids and so recorded them. One place for both
    /// landings, so they cannot drift apart.
    fn land_text(&self, state: &mut SessionState, file: String, minted: bool) {
        let previous = std::mem::replace(&mut state.last_saved_text, file);
        if matches!(self.doc(), RoomDoc::Reflection(_)) {
            if minted {
                state.new_heading_authors.clear();
            } else {
                Self::carry_new_heading_authors(state, &previous);
            }
        }
        state.editors.clear();
    }

    /// After a reflection room's save landed and before `editors` is
    /// cleared: every heading without an id whose section is new or changed
    /// against `previous` (the text saved before) gains the names in
    /// `editors`. Sections are matched by heading text and their place among
    /// the id-less sections under that heading text. A key the saved text no
    /// longer holds is dropped, so a name is never carried onto a later
    /// heading of the same name that somebody else typed.
    fn carry_new_heading_authors(state: &mut SessionState, previous: &str) {
        use crystalline_core::reflection::split_sections;
        let idless = |text: &str| -> std::collections::BTreeMap<(String, usize), String> {
            let mut out = std::collections::BTreeMap::new();
            let mut seen: HashMap<String, usize> = HashMap::new();
            // The top section always has an id, so only headings get here.
            for section in split_sections(text).into_iter().filter(|s| s.id.is_none()) {
                let place = seen.entry(section.heading.clone()).or_default();
                out.insert((section.heading, *place), section.text);
                *place += 1;
            }
            out
        };
        let before = idless(previous);
        let now = idless(&state.last_saved_text);
        state
            .new_heading_authors
            .retain(|key, _| now.contains_key(key));
        for (key, text) in &now {
            if before.get(key) != Some(text) {
                state
                    .new_heading_authors
                    .entry(key.clone())
                    .or_default()
                    .extend(state.editors.iter().cloned());
            }
        }
    }

    /// The restore of a deleted document from the room's text.
    async fn restore_doc(
        &self,
        view: &DomainView<'_>,
        state: &SessionState,
        file: &str,
    ) -> Result<serde_json::Value, EngineError> {
        match self.doc() {
            RoomDoc::Engram(_) => {
                // Through the room's own view when the room is over a draft - it puts
                // its text back in that actor's draft - and through the scope its save
                // uses otherwise. The two arms have to agree, and the save's `None`
                // arm goes through `Scope::Unrestricted`: routing a base room's
                // restore through the base view instead would write the folder of a
                // domain that reviews changes, which is the one thing review mode
                // exists to stop. Neither arm is reachable from the collab route in a
                // reviewing domain, which always names an owner; the registry API can
                // still ask for it, and this is the answer it gets.
                match view.actor() {
                    Some(_) => {
                        self.engine
                            .restore_engram_in_view(
                                view,
                                &self.domain,
                                &state.path,
                                file,
                                None,
                                false,
                            )
                            .await
                    }
                    None => {
                        self.engine
                            .restore_engram_from_room(&self.domain, &state.path, file)
                            .await
                    }
                }
            }
            RoomDoc::Reflection(path) => {
                // The file is gone or holds exactly the room's last text
                // (`restore_mine` checked); the save compares against what is
                // there, or nothing.
                let expected = match self.engine.reflection_room_text(&self.domain, &path).await {
                    Ok(text) => text.checksum,
                    Err(EngineError::NotFound(_)) => String::new(),
                    Err(err) => return Err(err),
                };
                let authors = Self::reflection_authors_locked(state);
                self.engine
                    .save_reflection_room(&self.domain, &path, file, &expected, &authors, false)
                    .await
            }
        }
    }
}

/// The per-session saver, spawned by the registry when it opens a session:
/// ticks [`CollabSession::tick_save`] until the session is disposed.
///
/// Each pass runs under `catch_unwind` because yrs can panic on pathological
/// text shapes (y-crdt/y-crdt#386, ZWJ emoji deletions). Such a panic is
/// session-fatal, never process-fatal and never a silent stall: a panicked
/// pass tells the room and ends the session instead of killing this task
/// quietly and stranding unsaved text.
async fn run_saver(session: Arc<CollabSession>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(SAVER_TICK_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if session.is_disposed() {
            break;
        }
        let pass = std::panic::AssertUnwindSafe(session.tick_save(Instant::now()));
        if futures::FutureExt::catch_unwind(pass).await.is_err() {
            tracing::error!(
                epoch = %session.epoch(),
                "a collab saver pass panicked; closing the session"
            );
            session.poison().await;
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_engram_room_is_keyed_by_its_permalink_as_a_room_doc() {
        let doc = RoomDoc::Engram("runbooks/deploy".into());
        assert_eq!(doc.name(), "runbooks/deploy");
        let key: RoomKey = ("eng".into(), doc.clone(), None);
        let mut keys = std::collections::HashSet::new();
        keys.insert(key.clone());
        assert!(keys.contains(&(
            "eng".to_string(),
            RoomDoc::Engram("runbooks/deploy".into()),
            None
        )));
    }
}
