//! The in-memory session registry and the per-session document room. The file
//! stays the source of truth: a session is a live LF-space view of it, and
//! everything durable flows back through the engine (Tasks 6-7).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use bytes::Bytes;
use tokio::sync::{Mutex, broadcast};
use yrs::sync::awareness::AwarenessUpdateEntry;
use yrs::sync::{Awareness, AwarenessUpdate, Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, GetString, Options, ReadTxn, Text, Transact, Update};

use super::control::{self, Control};
use super::text::{Separator, collab_eligible, file_text, separator_of, session_text};
use crate::engine::{Engine, EngineError};

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
    pub async fn join(&self, domain: &str, permalink: &str) -> Result<Joined, JoinError> {
        let key = (domain.to_string(), permalink.to_string());
        // The registry lock is held across open AND the membership check, so a
        // stampede of joins can neither open the same document twice nor
        // overshoot MAX_PARTICIPANTS between the check and the add.
        let mut sessions = self.sessions.lock().await;
        let (session, fresh) = match sessions.get(&key) {
            Some(existing) => (existing.clone(), false),
            None => {
                if sessions.len() >= MAX_SESSIONS {
                    return Err(JoinError::ServerFull);
                }
                let opened =
                    CollabSession::open(self.engine.clone(), key.clone(), self.mint_epoch())
                        .await?;
                sessions.insert(key.clone(), opened.clone());
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
                // otherwise sit in the registry with nobody in it.
                if fresh {
                    sessions.remove(&key);
                }
                Err(err)
            }
        }
    }

    /// Drop the registry entry once a session reports empty. Split from
    /// [`CollabSession::remove_conn`] so the final save (Task 6) can run
    /// between the two.
    pub async fn dispose_if_empty(&self, domain: &str, permalink: &str) {
        let key = (domain.to_string(), permalink.to_string());
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(&key)
            && session.is_empty().await
        {
            sessions.remove(&key);
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
    /// The registry key, (domain, permalink); the address saves write back to
    /// (Task 6) and external-change checks probe.
    #[allow(dead_code, reason = "the save path in Task 6 addresses writes with it")]
    key: (String, String),
    /// The engine every durable read and write goes through (Tasks 6-7).
    #[allow(dead_code, reason = "the save path in Task 6 writes through it")]
    engine: Arc<Engine>,
    tx: broadcast::Sender<Frame>,
    state: Mutex<SessionState>,
}

struct SessionState {
    /// Owns the Doc (yrs::sync::Awareness::new takes it); the doc is built
    /// with OffsetKind::Utf16 so every index agrees with JS clients.
    awareness: Awareness,
    separator: Separator,
    /// The domain-relative file path, as loaded.
    #[allow(dead_code, reason = "the save path in Task 6 reports it")]
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
    #[allow(dead_code, reason = "the debounce in Task 6 reads it")]
    last_edit: Option<Instant>,
    /// When the first unsaved update landed: the max-wait timer's input.
    #[allow(dead_code, reason = "the debounce in Task 6 reads it")]
    oldest_unsaved: Option<Instant>,
    /// A client asked for a save now rather than on the debounce.
    #[allow(dead_code, reason = "the save loop in Task 6 consumes it")]
    flush_requested: bool,
    save_state: SaveStateTag,
}

/// The wire label in hello/save_state controls ("ok" | "failed" | "conflict").
#[derive(Clone, Copy, PartialEq)]
#[allow(
    dead_code,
    reason = "Failed and Conflict are set by the save and merge paths in Tasks 6-7"
)]
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
            key,
            engine,
            tx,
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
                Message::Custom(control::CONTROL_TAG, payload) => {
                    match control::decode(&payload) {
                        Some(Control::Flush) => {
                            state.flush_requested = true;
                        }
                        Some(Control::Resolve { .. }) => {
                            // Task 7 wires resolution; until then a resolve is
                            // recorded nowhere and the conflict stands.
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
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
        let session = {
            // The text handle is taken before the transaction:
            // `get_or_insert_text` opens one of its own, which would deadlock
            // against a read transaction already held here.
            let doc = state.awareness.doc();
            let text = doc.get_or_insert_text(TEXT_NAME);
            let txn = doc.transact();
            text.get_string(&txn)
        };
        let file = file_text(&session, state.separator);
        let dirty = state.dirty && file != state.last_saved_text;
        (file, dirty)
    }
}
