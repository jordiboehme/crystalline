//! The in-memory session registry and the per-session document room. The file
//! stays the source of truth: a session is a live LF-space view of it, and
//! everything durable flows back through the engine (Tasks 6-7).

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
use super::text::{Separator, collab_eligible, file_text, separator_of, session_text};
use crate::engine::{Engine, EngineError, EngramText};

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

/// One connection's identity inside a session, minted at join.
pub type ConnId = u64;

/// One broadcast frame: protocol bytes plus the connection they came from.
#[derive(Clone)]
pub struct Frame {
    /// The connection an update came from, so the socket loop can skip
    /// echoing it back; None for server-originated frames (merge edits,
    /// control broadcasts), which everyone gets.
    pub from: Option<ConnId>,
    /// The encoded y-protocol messages to send.
    pub bytes: Bytes,
}

/// Why a join did not produce a session.
#[derive(Debug)]
pub enum JoinError {
    /// The engram could not be read.
    Engine(EngineError),
    /// The file mixes line endings, so the LF session transform would not be
    /// invertible; it edits solo rather than being silently rewritten.
    MixedEndings,
    /// This daemon already holds [`MAX_SESSIONS`] documents.
    ServerFull,
    /// This document already holds [`MAX_PARTICIPANTS`] connections.
    SessionFull,
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinError::Engine(err) => write!(f, "{err}"),
            JoinError::MixedEndings => write!(
                f,
                "this engram mixes line endings, so it cannot host a shared session"
            ),
            JoinError::ServerFull => write!(f, "too many documents are open for co-editing"),
            JoinError::SessionFull => write!(f, "too many people are editing this engram"),
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

/// The registry of open documents, keyed by (domain, permalink).
pub struct CollabSessions {
    engine: Arc<Engine>,
    sessions: Mutex<HashMap<(String, String), Arc<CollabSession>>>,
    next_conn: AtomicU64,
    next_epoch: AtomicU64,
}

impl CollabSessions {
    /// Build an empty registry over the engine sessions read and write through.
    pub fn new(engine: Arc<Engine>) -> Arc<CollabSessions> {
        Arc::new(CollabSessions {
            engine,
            sessions: Mutex::new(HashMap::new()),
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
    pub async fn join(
        self: &Arc<Self>,
        domain: &str,
        permalink: &str,
    ) -> Result<Joined, JoinError> {
        let key = (domain.to_string(), permalink.to_string());
        // The registry lock is held across open AND the membership check, so a
        // stampede of joins can neither open the same document twice nor
        // overshoot MAX_PARTICIPANTS between the check and the add.
        let mut sessions = self.sessions.lock().await;
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
                let opened = CollabSession::open(
                    self.engine.clone(),
                    key.clone(),
                    self.mint_epoch(),
                    Arc::downgrade(self),
                )
                .await?;
                sessions.insert(key.clone(), opened.clone());
                tokio::spawn(run_saver(opened.clone()));
                (opened, true)
            }
        };
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        match session.add_conn(conn).await {
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

    /// Move a session's registry entry after a frontmatter rename, so the room
    /// is found under the permalink it now answers to and a client that
    /// follows the `Saved { permalink }` broadcast rejoins THIS room instead of
    /// opening a second one over the same file.
    ///
    /// Called with no session guard held: the lock order is registry ->
    /// session, never the reverse. `epoch` identifies the session that renamed
    /// itself, so an entry that was replaced meanwhile is left alone.
    async fn rekey(&self, from: &(String, String), to_permalink: &str, epoch: &str) {
        if from.1 == to_permalink {
            return;
        }
        let to = (from.0.clone(), to_permalink.to_string());
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
                permalink = %to.1,
                "a rename collided with a live session; both rooms stand"
            );
            return;
        }
        if let Some(session) = sessions.remove(from) {
            session.adopt_key(to_permalink);
            sessions.insert(to, session);
        }
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
    /// The permalink this room is REGISTERED under. A frontmatter rename moves
    /// it together with the registry entry, so `(domain, key_permalink)` is
    /// always where the registry holds this session. A std mutex, never held
    /// across an await.
    key_permalink: std::sync::Mutex<String>,
    /// The registry this room lives in, for the rename move. Weak because the
    /// registry owns the session and never the other way round.
    registry: Weak<CollabSessions>,
    /// The engine every durable read and write goes through.
    engine: Arc<Engine>,
    tx: broadcast::Sender<Frame>,
    /// The room is over: the registry dropped it, or a saver pass panicked.
    /// Ends the saver loop and makes every save path a no-op, so nothing can
    /// write through a session no one owns any more.
    disposed: AtomicBool,
    state: Mutex<SessionState>,
}

struct SessionState {
    /// Owns the Doc (yrs::sync::Awareness::new takes it); the doc is built
    /// with OffsetKind::Utf16 so every index agrees with JS clients.
    awareness: Awareness,
    separator: Separator,
    /// The domain-relative file path, as loaded and as each save receipt
    /// reports it back. The address a restore writes back to.
    path: String,
    permalink: String,
    /// The checksum backing last_saved_text: the CAS token of the next save.
    checksum: String,
    /// FILE-space text as last loaded or saved.
    last_saved_text: String,
    /// Awareness client ids seen per connection, nulled on its disconnect.
    conns: HashMap<ConnId, HashSet<ClientID>>,
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

impl CollabSession {
    async fn open(
        engine: Arc<Engine>,
        key: (String, String),
        epoch: String,
        registry: Weak<CollabSessions>,
    ) -> Result<Arc<CollabSession>, JoinError> {
        let loaded = engine
            .engram_text(&key.0, &key.1)
            .await
            .map_err(JoinError::Engine)?;
        if !collab_eligible(&loaded.content) {
            return Err(JoinError::MixedEndings);
        }
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
            text.insert(&mut txn, 0, &session_text(&loaded.content));
        }
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Ok(Arc::new(CollabSession {
            epoch,
            domain: key.0,
            key_permalink: std::sync::Mutex::new(key.1),
            registry,
            engine,
            tx,
            disposed: AtomicBool::new(false),
            state: Mutex::new(SessionState {
                separator: separator_of(&loaded.content),
                awareness: Awareness::new(doc),
                path: loaded.path,
                permalink: loaded.permalink,
                checksum: loaded.checksum,
                last_saved_text: loaded.content,
                conns: HashMap::new(),
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
    ) -> Result<(broadcast::Receiver<Frame>, Vec<u8>), JoinError> {
        let mut state = self.state.lock().await;
        if state.conns.len() >= MAX_PARTICIPANTS {
            return Err(JoinError::SessionFull);
        }
        state.conns.insert(conn, HashSet::new());
        // The greeting: hello, then the server's SyncStep1, then the full
        // awareness picture - the DefaultProtocol::start choreography with our
        // control message in front. One buffer; concatenation is legal.
        let mut greeting = control::encode(&Control::Hello {
            epoch: self.epoch.clone(),
            separator: state.separator.as_str().to_string(),
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
                    let applied = {
                        let mut txn = state.awareness.doc().transact_mut();
                        txn.apply_update(decoded)
                    };
                    if applied.is_ok() {
                        let now = Instant::now();
                        state.dirty = true;
                        state.last_edit = Some(now);
                        state.oldest_unsaved.get_or_insert(now);
                        let _ = self.tx.send(Frame {
                            from: Some(conn),
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

    /// Drop a connection: null + broadcast its awareness states. True = last one.
    pub async fn remove_conn(&self, conn: ConnId) -> bool {
        let mut state = self.state.lock().await;
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
                bytes: Bytes::from(Message::Awareness(AwarenessUpdate { clients }).encode_v1()),
            });
        }
        state.conns.is_empty()
    }

    /// Whether nobody is connected any more.
    pub async fn is_empty(&self) -> bool {
        self.state.lock().await.conns.is_empty()
    }

    /// The session text back in FILE space, and whether it differs from the
    /// last saved text. The dirty FLAG says "an update arrived"; the equality
    /// check is what stops a no-op session from ever writing (the byte
    /// fidelity property for open-then-close).
    pub async fn snapshot(&self) -> (String, bool) {
        let state = self.state.lock().await;
        let file = Self::file_text_locked(&state);
        let dirty = state.dirty && file != state.last_saved_text;
        (file, dirty)
    }

    /// The session text in FILE space, read off the locked state.
    fn file_text_locked(state: &SessionState) -> String {
        let session = {
            // The text handle is taken before the transaction:
            // `get_or_insert_text` opens one of its own, which would deadlock
            // against a read transaction already held here.
            let doc = state.awareness.doc();
            let text = doc.get_or_insert_text(TEXT_NAME);
            let txn = doc.transact();
            text.get_string(&txn)
        };
        file_text(&session, state.separator)
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

    /// The registry key this room is filed under right now: `(domain,
    /// permalink)`, with the permalink a frontmatter rename may have moved.
    pub fn key(&self) -> (String, String) {
        (
            self.domain.clone(),
            self.key_permalink.lock().expect("key mutex").clone(),
        )
    }

    /// Record the permalink the registry just re-filed this room under.
    fn adopt_key(&self, permalink: &str) {
        *self.key_permalink.lock().expect("key mutex") = permalink.to_string();
    }

    /// One saver pass at `now`: decides whether a save is due and runs it.
    /// Takes `now` so tests drive time synthetically instead of sleeping.
    pub async fn tick_save(&self, now: Instant) {
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
        self.save_locked(&mut state, now).await
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
        if state.closed {
            // The room accepted an external deletion: a final save here would
            // resurrect the file the author agreed to let go.
            return None;
        }
        if matches!(state.save_state, SaveStateTag::Conflict) {
            return None; // an unresolved conflict never saves; drafts hold the text
        }
        // The last chance to land this text, so no retry backoff applies.
        self.save_locked(&mut state, Instant::now()).await
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
        self.dispose();
        let _ = self.tx.send(Frame {
            from: None,
            bytes: Bytes::from(control::encode(&Control::Closed {
                reason: "internal".to_string(),
            })),
        });
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
                    let theirs = match self
                        .engine
                        .engram_text(&self.domain, &state.permalink)
                        .await
                    {
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
        if file == state.last_saved_text {
            // Nothing effective changed: never write, never touch the mtime.
            state.dirty = false;
            state.oldest_unsaved = None;
            if !matches!(state.save_state, SaveStateTag::Ok) {
                // The document matches the file again, so the standing save
                // failure is over; the room is told so its alert clears.
                state.save_state = SaveStateTag::Ok;
                state.failure_detail = None;
                let _ = self.tx.send(Frame {
                    from: None,
                    bytes: Bytes::from(control::encode(&Control::Saved {
                        checksum: state.checksum.clone(),
                        permalink: state.permalink.clone(),
                    })),
                });
            }
            return SaveOutcome::Done(None);
        }
        state.last_attempt = Some(now);
        let receipt = self
            .engine
            .save_engram(&crate::params::SaveParams {
                domain: self.domain.clone(),
                identifier: state.permalink.clone(),
                content: file.clone(),
                expected_checksum: state.checksum.clone(),
            })
            .await;
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
                state.last_saved_text = file;
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

    /// Record and announce a save refusal that is nobody's conflict.
    ///
    /// The re-broadcast is guarded on the DETAIL as well as the state: a
    /// parse refusal replacing an io failure (or either replacing a resolved
    /// conflict) has to reach the room, or its alert keeps naming a reason
    /// that no longer applies.
    fn fail_save(&self, state: &mut SessionState, detail: String) {
        let repeat = matches!(state.save_state, SaveStateTag::Failed)
            && state.failure_detail.as_deref() == Some(detail.as_str());
        if !repeat {
            let _ = self.tx.send(Frame {
                from: None,
                bytes: Bytes::from(control::encode(&Control::SaveFailed {
                    detail: detail.clone(),
                })),
            });
        }
        tracing::debug!(domain = %self.domain, permalink = %state.permalink, %detail, "a session save was refused");
        state.save_state = SaveStateTag::Failed;
        state.failure_detail = Some(detail);
    }

    /// Suspend saving on an externally deleted engram and let the room decide
    /// between restoring its text and accepting the deletion.
    fn raise_deleted(&self, state: &mut SessionState, detail: String) {
        state.save_state = SaveStateTag::Conflict;
        state.failure_detail = None;
        state.pending = Some(PendingConflict::Deleted);
        let _ = self.tx.send(Frame {
            from: None,
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
        let mine = session_text(&Self::file_text_locked(state));
        match merge::three_way(&state.last_saved_text, &mine, &theirs.content) {
            MergeOutcome::Clean(merged) => {
                self.converge(state, &merged);
                // Their text is what the file holds now, so it is the base of
                // the next merge and its checksum is the next CAS token.
                state.last_saved_text = theirs.content;
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
        match self
            .engine
            .engram_text(&self.domain, &state.permalink)
            .await
        {
            Ok(theirs) if theirs.checksum != state.checksum => {
                let detail = format!(
                    "'{}' changed on disk while this session was idle",
                    state.permalink
                );
                if self.merge_external(state, theirs, detail).await {
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
                self.converge(state, &session_text(&theirs));
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
                    bytes: Bytes::from(control::encode(&Control::Closed {
                        reason: "deleted".to_string(),
                    })),
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
        match self
            .engine
            .engram_text_at_path(&self.domain, &state.path)
            .await
        {
            Ok(Some(theirs)) if theirs.content != state.last_saved_text => {
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
                state.pending = Some(PendingConflict::Deleted);
                let _ = self.tx.send(Frame {
                    from: None,
                    bytes: Bytes::from(control::encode(&Control::SaveFailed {
                        detail: err.to_string(),
                    })),
                });
                return None;
            }
        }
        // save_engram refuses a missing file by design, so the room's text
        // goes back through the restore verb instead.
        let file = Self::file_text_locked(state);
        match self
            .engine
            .restore_engram(&self.domain, &state.path, &file)
            .await
        {
            Ok(receipt) => {
                let checksum = receipt["checksum"].as_str().unwrap_or_default().to_string();
                let permalink = receipt["permalink"]
                    .as_str()
                    .unwrap_or(&state.permalink)
                    .to_string();
                let renamed = (permalink != state.permalink).then(|| permalink.clone());
                state.checksum = checksum.clone();
                state.last_saved_text = file;
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
                state.pending = Some(PendingConflict::Deleted);
                let _ = self.tx.send(Frame {
                    from: None,
                    bytes: Bytes::from(control::encode(&Control::SaveFailed {
                        detail: err.to_string(),
                    })),
                });
                None
            }
        }
    }

    /// Suspend saving on colliding edits and hand the room both sides.
    fn raise_edit(&self, state: &mut SessionState, theirs: EngramText, detail: String) {
        state.save_state = SaveStateTag::Conflict;
        state.failure_detail = None;
        state.pending = Some(PendingConflict::Edit {
            theirs: theirs.content.clone(),
            theirs_checksum: theirs.checksum,
        });
        let _ = self.tx.send(Frame {
            from: None,
            bytes: Bytes::from(control::encode(&Control::Conflict {
                conflict_kind: "edit".to_string(),
                theirs: Some(theirs.content),
                detail,
            })),
        });
    }

    /// Morph the live text into `target` (SESSION space) as one minimal edit
    /// script in ONE transaction - every client sees a single update rather
    /// than a flicker of half-applied lines - broadcast that update to the
    /// room and tell it the external change is in.
    fn converge(&self, state: &mut SessionState, target: &str) {
        let update = {
            let doc = state.awareness.doc();
            // Taken before the transaction: get_or_insert_text opens one of
            // its own and would deadlock inside ours.
            let text = doc.get_or_insert_text(TEXT_NAME);
            let mut txn = doc.transact_mut();
            let current = text.get_string(&txn);
            merge::apply_target(&text, &mut txn, &current, target);
            txn.encode_update_v1()
        };
        let _ = self.tx.send(Frame {
            from: None,
            bytes: Bytes::from(Message::Sync(SyncMessage::Update(update)).encode_v1()),
        });
        let _ = self.tx.send(Frame {
            from: None,
            bytes: Bytes::from(control::encode(&Control::Merged)),
        });
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
