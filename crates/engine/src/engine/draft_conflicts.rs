//! A draft's conflict, read from what stands: her row, her record of where it
//! started and the base snapshot. Nothing is stored, so there is nothing to
//! rebuild and nothing to go stale, and a pass that failed leaves a stale draft
//! listed rather than clean.
//!
//! Rows with a record take their answer from [`next_step`], so the gates and
//! the pass read one rule: `Landed` or `InStep` is none, `Conflict(k)` is `k`,
//! and `Take` is `EditEdit`, since a stale row that would merge cleanly still
//! holds the old text until a pass has merged it. A row with no record never
//! reaches [`next_step`], which would read it as her own new file: it gets
//! [`has_landed`] and then the old-draft rows (option A, below).
//!
//! Nothing here asks whether the domain reviews changes, because a fold's
//! second check runs with the key off, and nothing here takes the origin lock:
//! status asks it outside the per-domain lock.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crystalline_index::{EngramRecord, FileStamp, StoredEngram};
use crystalline_remote::state::{OriginState, base_path, read_base_file};
use serde::Serialize;
use serde_json::{Value, json};

use super::{
    Engine, EngineError, Result, folder_folds_case, folder_may_fold_case, is_within_domain,
    sha256_hex, utf8_side,
};
use crate::draft_merge::{
    DraftConflictKind, DraftSides, DraftStep, TeamText, answers_at_path, has_landed, next_step,
};
use crate::draft_start::{self, DraftStart};
use crate::origin::takes_part_in_local_change;

/// One draft of one person that does not fit the team's text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DraftConflict {
    /// `d` and the first seven hex digits of the sha-256 of
    /// `"{actor}\0{path}"`: stable while the draft stands, and nobody else's.
    pub id: String,
    /// The draft's path, or the tombstone's when it stands alone.
    pub path: String,
    pub kind: DraftConflictKind,
    /// The path a moved draft left, where her tombstone stands.
    pub moved_from: Option<String>,
    /// A file beside the pages rather than a page (Task 11).
    pub file: bool,
    /// For `AddressTaken`: the team's path that holds the draft's address.
    pub other: Option<String>,
}

/// What the team holds at one path, owned, for [`TeamText`] to borrow.
#[derive(Clone, Debug)]
pub(super) enum Held {
    Absent,
    Bytes(Vec<u8>),
    Unreadable(String),
}

impl Held {
    /// The team's text, when the bytes are UTF-8.
    pub(super) fn text(&self) -> Option<std::borrow::Cow<'_, str>> {
        match self {
            Held::Bytes(bytes) => std::str::from_utf8(bytes)
                .ok()
                .map(std::borrow::Cow::Borrowed),
            Held::Absent | Held::Unreadable(_) => None,
        }
    }

    fn team(&self) -> TeamText<'_> {
        match self {
            Held::Absent => TeamText::Absent,
            Held::Bytes(bytes) => TeamText::Held(bytes),
            Held::Unreadable(sha256) => TeamText::Unreadable { sha256 },
        }
    }
}

/// The base snapshot, asked only for its real keys.
///
/// On a disk that folds case `read_base_file("Plan.md")` also answers
/// `plan.md`'s bytes, so a path that is not a key is absent without a read. A
/// key with no readable copy is unreadable with its stamp, never absent (C5),
/// and so is a key whose folded spelling another key shares on a disk that
/// folds case, since the two have one copy between them.
struct Base<'s> {
    state_dir: &'s Path,
    state: &'s OriginState,
    shared_spelling: HashSet<String>,
    read: HashMap<String, Held>,
}

impl<'s> Base<'s> {
    fn new(state_dir: &'s Path, state: &'s OriginState) -> Result<Self> {
        let mut spellings: HashMap<String, Vec<&str>> = HashMap::new();
        for key in state.files.keys() {
            spellings
                .entry(crystalline_core::fold_path_case(key))
                .or_default()
                .push(key);
        }
        let mut shared_spelling = HashSet::new();
        for keys in spellings.values().filter(|keys| keys.len() > 1) {
            for key in keys {
                if copy_may_fold_case(state_dir, key)? {
                    shared_spelling.insert((*key).to_string());
                }
            }
        }
        Ok(Base {
            state_dir,
            state,
            shared_spelling,
            read: HashMap::new(),
        })
    }

    fn at(&mut self, path: &str) -> Result<Held> {
        if let Some(held) = self.read.get(path) {
            return Ok(held.clone());
        }
        let held = match self.state.files.get(path) {
            None => Held::Absent,
            Some(stamp) if self.shared_spelling.contains(path) => {
                Held::Unreadable(stamp.sha256.clone())
            }
            Some(stamp) => match read_base_file(self.state_dir, path)? {
                Some(bytes) => Held::Bytes(bytes),
                None => Held::Unreadable(stamp.sha256.clone()),
            },
        };
        self.read.insert(path.to_string(), held.clone());
        Ok(held)
    }
}

/// Whether the folder the base copy of `key` stands in may take two spellings
/// for one name. Asked where the copies live, which is what decides whether two
/// keys can have a copy each.
fn copy_may_fold_case(state_dir: &Path, key: &str) -> Result<bool> {
    let copy = base_path(state_dir, key)?;
    let (Some(folder), Some(name)) = (copy.parent(), copy.file_name()) else {
        return Ok(true);
    };
    Ok(folder_may_fold_case(folder, &name.to_string_lossy()))
}

/// One row found in conflict, with what its sides need.
struct Found {
    conflict: DraftConflict,
    record: Option<DraftStart>,
    local: Option<String>,
    upstream: Held,
}

/// A row's kind, with the path whose team text it is about.
#[derive(Clone, Debug)]
struct Kind {
    kind: DraftConflictKind,
    other: Option<String>,
    upstream_at: String,
}

fn conflict_id(actor: &str, path: &str) -> String {
    let hash = sha256_hex(format!("{actor}\0{path}").as_bytes());
    format!("d{}", &hash[..7])
}

/// What a row with a record is, from the one decision (C7).
fn from_next_step(step: DraftStep) -> Option<DraftConflictKind> {
    match step {
        DraftStep::Landed | DraftStep::InStep => None,
        // Not merged yet: what she would share still lacks the team's change.
        DraftStep::Take { .. } => Some(DraftConflictKind::EditEdit),
        DraftStep::Conflict(kind) => Some(kind),
    }
}

// --- Option A: drafts from before the record (owner question 2) -------------
//
// Reversible: this section and its tests in `origins/draft_conflicts.rs`
// (`an_old_*`). A row with no record is an old draft, or a draft from another
// machine on a shared database. We cannot tell what the team changed since, so
// nothing is merged: she compares the two texts and chooses.

/// The two "no record" rows of the design's table, asked after [`has_landed`]:
///
/// - The team holds the path, or a key with no readable copy stands there:
///   `UnknownStart`, for a draft and a tombstone alike. Never `AddAdd`: we
///   cannot tell she did not start from the team's file.
/// - A draft over a path the team does not hold is a new file and needs
///   nothing, unless the frozen convergence record lists it, which means a pull
///   from before saw the team delete it: `EditDelete`.
///
/// `listed_in_record` is asked only for that last row, so a broken record file
/// refuses only where it decides something.
fn old_draft_conflict(
    row: &StoredEngram,
    team_at_path: TeamText<'_>,
    listed_in_record: impl FnOnce() -> Result<bool>,
) -> Result<Option<DraftConflictKind>> {
    Ok(match team_at_path {
        TeamText::Held(_) | TeamText::Unreadable { .. } => Some(DraftConflictKind::UnknownStart),
        TeamText::Absent if row.tombstone => None,
        TeamText::Absent => listed_in_record()?.then_some(DraftConflictKind::EditDelete),
    })
}

/// The address a tombstone's row answers to as a page: its row holds the text it
/// deleted, and a tombstone's own permalink is its path.
fn deleted_address(tombstone: &StoredEngram) -> Option<String> {
    let engram = crystalline_core::parse_engram(&tombstone.content).ok()?;
    Some(EngramRecord::from_engram(&engram, &tombstone.path, no_stamp()).permalink)
}

/// A stamp for a record built only to read its permalink.
fn no_stamp() -> FileStamp {
    FileStamp {
        mtime: 0,
        size: 0,
        sha256: String::new(),
    }
}

/// Old move pairs: a tombstone and a draft, neither with a record, with equal
/// text or equal address, are one move. Answers tombstone index to draft index.
fn old_moves(rows: &[StoredEngram], records: &[Option<DraftStart>]) -> HashMap<usize, usize> {
    let mut pairs = HashMap::new();
    let mut taken = HashSet::new();
    for (t, tombstone) in rows.iter().enumerate() {
        if !tombstone.tombstone || records[t].is_some() {
            continue;
        }
        let address = deleted_address(tombstone);
        let draft = rows.iter().enumerate().find(|(d, draft)| {
            !draft.tombstone
                && records[*d].is_none()
                && draft.path != tombstone.path
                && !taken.contains(d)
                && (draft.content == tombstone.content
                    || address.as_deref() == Some(draft.permalink.as_str()))
        });
        if let Some((d, _)) = draft {
            taken.insert(d);
            pairs.insert(t, d);
        }
    }
    pairs
}

// --- end of option A ---------------------------------------------------------

/// Whether a key of the base snapshot can hold an address: a page inside the
/// domain that takes part in a share. A log, a listing or a dotfile is never
/// indexed, so it holds no address whatever its frontmatter says. The pass's
/// rename test and the address map read this one rule.
pub(super) fn holds_an_address(key: &str) -> bool {
    key.ends_with(".md") && is_within_domain(key) && takes_part_in_local_change(key)
}

/// What the team holds for one row: at its path, and where it started.
pub(super) struct RowTeam {
    pub(super) at_path: Held,
    pub(super) at_from: Held,
}

/// What one row is, by the rule the gates and the pass share. First match
/// wins, in this order.
pub(super) enum Decision {
    /// A path that takes no part in a share, or the tombstone half of a move,
    /// for which her draft stands.
    NotAsked,
    /// It has landed ([`has_landed`]).
    Landed,
    /// A draft whose address another team file holds in her view: that path.
    AddressTaken(String),
    /// A row with no record, which never reaches [`next_step`].
    NoRecord,
    /// A row with a record: the one decision.
    Step(DraftStep),
}

/// One actor's rows read against the base snapshot: her records, the team's
/// addresses in her view and which rows are two halves of one move. The
/// conflict list and the convergence pass both decide from this, so the two
/// cannot answer one row two ways.
pub(super) struct Reading<'s> {
    pub(super) rows: Vec<StoredEngram>,
    pub(super) records: Vec<Option<DraftStart>>,
    base: Base<'s>,
    addresses: HashMap<String, String>,
    root: PathBuf,
    root_is_there: bool,
    /// A moved draft names where it started, whether or not its tombstone still
    /// stands there; an old pair names its tombstone's path.
    pub(super) moved_from: HashMap<usize, String>,
    /// Tombstones standing where a draft of hers started, compared exactly.
    halves: HashSet<usize>,
    /// Old move pairs, tombstone index to draft index (option A).
    pub(super) old_pairs: HashMap<usize, usize>,
}

impl Reading<'_> {
    /// The team's text at the row's path and where it started.
    pub(super) fn team(&mut self, i: usize) -> Result<RowTeam> {
        let path = self.rows[i].path.clone();
        let from = self.records[i]
            .as_ref()
            .and_then(|record| record.from.clone());
        let at_path = self.base.at(&path)?;
        let at_from = match from {
            Some(from) if from != path => self.base.at(&from)?,
            Some(_) => at_path.clone(),
            None => Held::Absent,
        };
        Ok(RowTeam { at_path, at_from })
    }

    /// The row's sides for the decision, with `team` from [`Reading::team`].
    pub(super) fn sides<'a>(&'a self, i: usize, team: &'a RowTeam) -> DraftSides<'a> {
        let row = &self.rows[i];
        let record = self.records[i].as_ref();
        DraftSides {
            path: &row.path,
            from: record.and_then(|record| record.from.as_deref()),
            start: record.and_then(|record| record.start.as_deref()),
            start_sha256: record.and_then(|record| record.start_sha256.as_deref()),
            shared: record
                .and_then(|record| record.shared.as_ref())
                .and_then(|shared| shared.text.as_deref()),
            // C11: a folder that cannot say does not fold here, so a case move
            // is never read as landed on a guess.
            folds_case: self.root_is_there
                && folder_folds_case(&self.root, &row.path) == Some(true),
            merging: record.is_some_and(|record| record.merging.is_some()),
            local: (!row.tombstone).then_some(row.content.as_str()),
            team_at_path: team.at_path.team(),
            team_at_from: team.at_from.team(),
        }
    }

    /// The decision for row `i`, with the team texts it was made from.
    pub(super) fn decide(&mut self, i: usize) -> Result<(Decision, RowTeam)> {
        if !takes_part_in_local_change(&self.rows[i].path) || self.halves.contains(&i) {
            let unread = RowTeam {
                at_path: Held::Absent,
                at_from: Held::Absent,
            };
            return Ok((Decision::NotAsked, unread));
        }
        let team = self.team(i)?;
        let row = &self.rows[i];
        let sides = self.sides(i, &team);
        if has_landed(&sides) {
            return Ok((Decision::Landed, team));
        }
        if !row.tombstone
            && let Some(other) = self
                .addresses
                .get(row.permalink.as_str())
                .filter(|other| **other != row.path)
        {
            return Ok((Decision::AddressTaken(other.clone()), team));
        }
        // C9: branch on the record before the decision.
        let decision = match self.records[i] {
            Some(_) => Decision::Step(next_step(&sides)),
            None => Decision::NoRecord,
        };
        Ok((decision, team))
    }

    /// Her tombstone at exactly `path`, if she holds one.
    pub(super) fn tombstone_at(&self, path: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.tombstone && row.path == path)
    }

    /// The tombstone half of draft `d`'s move: at its record's `from`, or its
    /// old pair's.
    pub(super) fn half_of(&self, d: usize) -> Option<usize> {
        let from = self.records[d]
            .as_ref()
            .and_then(|record| record.from.as_deref())
            .filter(|from| *from != self.rows[d].path);
        if let Some(from) = from {
            return self.tombstone_at(from);
        }
        self.old_pairs
            .iter()
            .find(|(_, draft)| **draft == d)
            .map(|(tombstone, _)| *tombstone)
    }

    /// Whether row `i` is the tombstone of an old move pair, which is decided
    /// with its draft and never alone.
    pub(super) fn is_old_half(&self, i: usize) -> bool {
        self.old_pairs.contains_key(&i)
    }
}

impl Engine {
    /// Read `actor`'s `rows` against the base snapshot. A record that cannot be
    /// read is an error, never a row with no record (C1).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn read_drafts<'s>(
        &self,
        domain: &str,
        domain_id: crystalline_index::DomainId,
        actor: &str,
        rows: Vec<StoredEngram>,
        root: &Path,
        state_dir: &'s Path,
        state: &'s OriginState,
    ) -> Result<Reading<'s>> {
        let journal_dir = self.journal_state_dir()?;
        let records = rows
            .iter()
            .map(|row| {
                draft_start::load(&journal_dir, domain, actor, &row.path).map_err(|source| {
                    EngineError::Io {
                        path: format!("the draft start record of '{}'", row.path),
                        source,
                    }
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut base = Base::new(state_dir, state)?;
        let addresses = if rows.iter().any(|row| !row.tombstone) {
            self.team_addresses(domain, domain_id, &rows, &mut base)
                .await?
        } else {
            HashMap::new()
        };

        let mut moved_from: HashMap<usize, String> = HashMap::new();
        let mut halves: HashSet<usize> = HashSet::new();
        let tombstones: HashMap<&str, usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.tombstone)
            .map(|(i, row)| (row.path.as_str(), i))
            .collect();
        for (d, row) in rows.iter().enumerate().filter(|(_, row)| !row.tombstone) {
            let from = records[d]
                .as_ref()
                .and_then(|record| record.from.as_deref());
            if let Some(from) = from.filter(|from| *from != row.path) {
                moved_from.insert(d, from.to_string());
                if let Some(&t) = tombstones.get(from) {
                    halves.insert(t);
                }
            }
        }
        let old_pairs = old_moves(&rows, &records);
        for (&t, &d) in &old_pairs {
            moved_from.insert(d, rows[t].path.clone());
        }
        Ok(Reading {
            rows,
            records,
            base,
            addresses,
            root: root.to_path_buf(),
            root_is_there: root.is_dir(),
            moved_from,
            halves,
            old_pairs,
        })
    }

    /// The drafts of `actor` in `domain` that do not fit the team's text, ordered
    /// by path. Empty for a domain with no origin or no origin state. A record
    /// that cannot be read is an error, never a clean row (C1).
    pub async fn draft_conflicts(&self, domain: &str, actor: &str) -> Result<Vec<DraftConflict>> {
        Ok(self
            .found_conflicts(domain, actor)
            .await?
            .into_iter()
            .map(|found| found.conflict)
            .collect())
    }

    /// One of `actor`'s conflicts by id, in the shape `origin_conflict_detail`
    /// answers, so Fluid reads one shape: `base` is her start, `local` her draft
    /// and `upstream` the team's text at the path the kind came from: her own
    /// path when the team holds a file there she did not start from (what Keep
    /// mine would overwrite), `other` for `AddressTaken`, else where she started.
    /// An id of somebody else's is not found.
    pub async fn draft_conflict_sides(&self, domain: &str, actor: &str, id: &str) -> Result<Value> {
        let found = self
            .found_conflicts(domain, actor)
            .await?
            .into_iter()
            .find(|found| found.conflict.id == id)
            .ok_or_else(|| {
                EngineError::NotFound(format!("no conflict {id} among your drafts in '{domain}'"))
            })?;
        // One note per side that needs one, kept together: a start with no text
        // and a team file that is not text are two facts.
        let mut notes: Vec<String> = Vec::new();
        let base = match &found.record {
            Some(DraftStart {
                start: Some(start), ..
            }) => Value::String(start.clone()),
            Some(DraftStart {
                start_sha256: Some(_),
                ..
            }) => {
                notes.push("the base side has no text and is omitted".to_string());
                Value::Null
            }
            _ => Value::Null,
        };
        let local = found.local.clone().map_or(Value::Null, Value::String);
        let upstream = match found.upstream {
            Held::Absent => Value::Null,
            Held::Bytes(bytes) => {
                let mut note = None;
                let side = utf8_side(Some(bytes), "upstream", &mut note);
                notes.extend(note);
                side
            }
            Held::Unreadable(_) => {
                notes.push("the upstream side has no readable copy and is omitted".to_string());
                Value::Null
            }
        };
        let note = (!notes.is_empty()).then(|| notes.join("; "));
        let conflict = found.conflict;
        Ok(json!({
            "id": conflict.id,
            "path": conflict.path,
            "kind": conflict.kind,
            "draft": true,
            "detected_at": null,
            "base": base,
            "local": local,
            "upstream": upstream,
            "moved_from": conflict.moved_from,
            "other": conflict.other,
            "note": note,
        }))
    }

    /// The paths of `actor`'s drafts a merge changed since they last settled:
    /// the records with `merged_at`. A notice, not a gate, so a records folder
    /// that cannot be read answers empty with a warning.
    #[allow(
        dead_code,
        reason = "read once status names the drafts a merge changed"
    )]
    pub(crate) fn drafts_merged(&self, domain: &str, actor: &str) -> Vec<String> {
        let read = self
            .journal_state_dir()
            .map_err(|e| e.to_string())
            .and_then(|dir| draft_start::all(&dir, domain, actor).map_err(|e| e.to_string()));
        match read {
            Ok(records) => records
                .into_iter()
                .filter(|record| record.merged_at.is_some())
                .map(|record| record.path)
                .collect(),
            Err(e) => {
                tracing::warn!(
                    domain,
                    actor,
                    "the drafts a merge changed could not be listed: {e}"
                );
                Vec::new()
            }
        }
    }

    /// The team's addresses in her view, permalink to path: the base snapshot's
    /// pages ([`holds_an_address`]) less every path she holds a row at, which is
    /// what the pass reads. The index is asked only for a page whose row matches
    /// the snapshot's stamp; a page it does not list, or lists with other bytes,
    /// is read from its base copy. So a page a direct commit deleted holds no
    /// address while its row lingers, and a page no sync has indexed yet holds
    /// its own. A key with no readable copy, or bytes that are no page, answers
    /// to nothing we can name.
    async fn team_addresses(
        &self,
        domain: &str,
        domain_id: crystalline_index::DomainId,
        rows: &[StoredEngram],
        base: &mut Base<'_>,
    ) -> Result<HashMap<String, String>> {
        let (indexed_rows, stamps) = {
            let store = self.store.lock().await;
            (
                store.list_engrams(domain, None, None).await?,
                store.file_stamps(domain_id).await?,
            )
        };
        let indexed: HashMap<&str, &str> = indexed_rows
            .iter()
            .map(|row| (row.path.as_str(), row.permalink.as_str()))
            .collect();
        // Every path she holds a row at, as the pass reads it: a tombstone takes
        // the page away, and a draft says in her view what that path answers to,
        // so the base copy's address there is not the team's to hold.
        let own: HashSet<&str> = rows.iter().map(|row| row.path.as_str()).collect();
        let keys: Vec<(String, String)> = base
            .state
            .files
            .iter()
            .filter(|(key, _)| holds_an_address(key) && !own.contains(key.as_str()))
            .map(|(key, stamp)| (key.clone(), stamp.sha256.clone()))
            .collect();
        let mut addresses = HashMap::new();
        for (key, sha256) in keys {
            let current = stamps.get(&key).is_some_and(|stamp| stamp.sha256 == sha256);
            let permalink = match indexed.get(key.as_str()) {
                Some(permalink) if current => Some((*permalink).to_string()),
                _ => match base.at(&key)? {
                    Held::Bytes(bytes) => address_of(&key, &bytes),
                    Held::Absent | Held::Unreadable(_) => None,
                },
            };
            if let Some(permalink) = permalink {
                addresses.insert(permalink, key);
            }
        }
        Ok(addresses)
    }

    /// The table of the design, first match wins:
    ///
    /// 1. It has landed ([`has_landed`]): none.
    /// 2. A path that takes no part in a share: none.
    /// 3. The tombstone half of a move: none; the draft stands for both and
    ///    names it in `moved_from`.
    /// 4. A draft whose address another team file holds in her view:
    ///    `AddressTaken`, since Keep mine cannot clear it.
    /// 5. No record: the old-draft rows. With a record: [`next_step`].
    ///
    /// Rows 2 and 3 are asked before row 1 in [`Reading::decide`]; a row they
    /// silence has no answer of its own either way.
    async fn found_conflicts(&self, domain: &str, actor: &str) -> Result<Vec<Found>> {
        if self.domain_entry(domain)?.origin.is_none() {
            return Ok(Vec::new());
        }
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let Some(state) = OriginState::load(&state_dir)? else {
            return Ok(Vec::new());
        };
        // The read-only id lookup: asking must not register a domain.
        let (domain_id, rows) = {
            let store = self.store.lock().await;
            match store.domain_id(domain).await? {
                Some(id) => (id, store.overlay_entries(id, actor).await?),
                None => return Ok(Vec::new()),
            }
        };
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let journal_dir = self.journal_state_dir()?;
        let mut reading = self
            .read_drafts(domain, domain_id, actor, rows, &root, &state_dir, &state)
            .await?;

        let count = reading.rows.len();
        let mut frozen: Option<HashSet<String>> = None;
        let mut kinds: Vec<Option<Kind>> = vec![None; count];
        // For an old move whose source the team deleted (M1): which rows landed
        // by the team holding nothing at their path, and which were asked and
        // stand open.
        let mut landed_by_absence = vec![false; count];
        let mut open = vec![false; count];
        for i in 0..count {
            let (decision, team) = reading.decide(i)?;
            let kind = match decision {
                Decision::NotAsked => continue,
                Decision::Landed => {
                    landed_by_absence[i] = matches!(team.at_path, Held::Absent);
                    continue;
                }
                Decision::AddressTaken(other) => {
                    open[i] = true;
                    kinds[i] = Some(Kind {
                        kind: DraftConflictKind::AddressTaken,
                        other: Some(other.clone()),
                        upstream_at: other,
                    });
                    continue;
                }
                Decision::NoRecord => {
                    let row = &reading.rows[i];
                    old_draft_conflict(row, team.at_path.team(), || {
                        let listed =
                            match &frozen {
                                Some(listed) => listed,
                                None => {
                                    let read = crate::overlay_journal::journal_record_strict(
                                        &journal_dir,
                                        domain,
                                    )
                                    .map_err(|source| EngineError::Io {
                                        path: format!("the convergence record of '{domain}'"),
                                        source,
                                    })?;
                                    frozen.insert(
                                        read.conflicts
                                            .get(actor)
                                            .into_iter()
                                            .flatten()
                                            .cloned()
                                            .collect(),
                                    )
                                }
                            };
                        Ok(listed.contains(&row.path))
                    })?
                }
                Decision::Step(step) => from_next_step(step),
            };
            open[i] = true;
            // Where the kind came from, by the decision's own rule: what Keep
            // mine would overwrite when a file of the team's stands at her path,
            // otherwise the text where she started.
            let row = &reading.rows[i];
            let sides = reading.sides(i, &team);
            let upstream_at = if answers_at_path(&sides) {
                row.path.clone()
            } else {
                sides.from.unwrap_or(&row.path).to_string()
            };
            kinds[i] = kind.map(|kind| Kind {
                kind,
                other: None,
                upstream_at,
            });
        }
        // An old move lists once, at the draft: the tombstone's answer is the
        // draft's when the draft has none of its own, and it still reads at the
        // tombstone's path. A source the team deleted under a draft that has not
        // landed is a deletion she has to see, as `EditDelete` is with a record:
        // a share would otherwise bring the page back under her new name.
        for (&t, &d) in &reading.old_pairs {
            let half = kinds[t].take();
            if kinds[d].is_none() {
                kinds[d] = half;
            }
            if kinds[d].is_none() && landed_by_absence[t] && open[d] {
                kinds[d] = Some(Kind {
                    kind: DraftConflictKind::EditDelete,
                    other: None,
                    upstream_at: reading.rows[t].path.clone(),
                });
            }
        }

        let mut found = Vec::new();
        for (i, kind) in kinds.into_iter().enumerate() {
            let Some(Kind {
                kind,
                other,
                upstream_at,
            }) = kind
            else {
                continue;
            };
            let row = &reading.rows[i];
            found.push(Found {
                conflict: DraftConflict {
                    id: conflict_id(actor, &row.path),
                    path: row.path.clone(),
                    kind,
                    moved_from: reading.moved_from.get(&i).cloned(),
                    file: false,
                    other,
                },
                record: reading.records[i].clone(),
                local: (!row.tombstone).then(|| row.content.clone()),
                upstream: reading.base.at(&upstream_at)?,
            });
        }
        found.sort_by(|a, b| a.conflict.path.cmp(&b.conflict.path));
        Ok(found)
    }
}

/// The conflicts that block a share of `files`: all of them when no files are
/// named, since she asked to share everything; otherwise those whose path or
/// `moved_from` is named. A share is a staged tree filtered by `files`, so a
/// stale row outside them cannot travel. A name is read as the share reads it
/// (`normalize_selected_path`), so `./plan.md` names `plan.md` here too.
pub(super) fn conflicts_block(
    conflicts: &[DraftConflict],
    files: Option<&[String]>,
) -> Vec<DraftConflict> {
    let Some(files) = files else {
        return conflicts.to_vec();
    };
    let named: HashSet<String> = files
        .iter()
        .map(|raw| crystalline_remote::ops::normalize_selected_path(raw))
        .collect();
    conflicts
        .iter()
        .filter(|conflict| {
            named.contains(&conflict.path)
                || conflict
                    .moved_from
                    .as_ref()
                    .is_some_and(|from| named.contains(from))
        })
        .cloned()
        .collect()
}

/// The address a page's text answers to at `key`, or `None` for bytes that are
/// no page.
pub(super) fn address_of(key: &str, bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| crystalline_core::parse_engram(text).ok())
        .map(|engram| EngramRecord::from_engram(&engram, key, no_stamp()).permalink)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crystalline_core::config::GlobalConfig;
    use crystalline_index::TursoStore;
    use tokio::sync::Mutex;

    use super::*;

    fn record(path: &str, merged: bool) -> DraftStart {
        DraftStart {
            path: path.to_string(),
            from: Some(path.to_string()),
            start: Some("start\n".to_string()),
            start_sha256: None,
            shared: None,
            merged_at: merged.then(chrono::Utc::now),
            merging: None,
        }
    }

    fn listed(path: &str, moved_from: Option<&str>) -> DraftConflict {
        DraftConflict {
            id: conflict_id("alice", path),
            path: path.to_string(),
            kind: DraftConflictKind::EditEdit,
            moved_from: moved_from.map(str::to_string),
            file: false,
            other: None,
        }
    }

    fn named(files: &[&str]) -> Vec<String> {
        files.iter().map(|file| file.to_string()).collect()
    }

    #[test]
    fn with_no_files_every_conflict_blocks() {
        let all = vec![listed("a.md", None), listed("b.md", Some("c.md"))];
        assert_eq!(conflicts_block(&all, None), all);
        assert_eq!(conflicts_block(&[], None), vec![]);
    }

    #[test]
    fn named_files_block_by_path_or_by_where_the_move_started() {
        let all = vec![listed("a.md", None), listed("b.md", Some("c.md"))];
        assert_eq!(
            conflicts_block(&all, Some(&named(&["a.md"]))),
            vec![all[0].clone()]
        );
        assert_eq!(
            conflicts_block(&all, Some(&named(&["c.md"]))),
            vec![all[1].clone()]
        );
        assert_eq!(
            conflicts_block(&all, Some(&named(&["b.md", "z.md"]))),
            vec![all[1].clone()]
        );
        assert_eq!(conflicts_block(&all, Some(&named(&["z.md"]))), vec![]);
        assert_eq!(conflicts_block(&all, Some(&[])), vec![]);
    }

    #[test]
    fn a_name_is_read_as_the_share_reads_it() {
        let all = vec![listed("notes/a.md", None), listed("b.md", Some("c.md"))];
        assert_eq!(
            conflicts_block(&all, Some(&named(&[" ./notes\\a.md "]))),
            vec![all[0].clone()]
        );
        assert_eq!(
            conflicts_block(&all, Some(&named(&["./c.md"]))),
            vec![all[1].clone()]
        );
    }

    #[tokio::test]
    async fn drafts_merged_lists_the_records_a_merge_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Engine::new(
            Arc::new(Mutex::new(store)),
            GlobalConfig::default(),
            None,
            None,
        )
        .with_state_dir(tmp.path().to_path_buf());
        assert_eq!(engine.drafts_merged("team", "alice"), Vec::<String>::new());
        for (path, merged) in [("a.md", false), ("b.md", true), ("c/d.md", true)] {
            draft_start::save(tmp.path(), "team", "alice", &record(path, merged)).unwrap();
        }
        draft_start::save(tmp.path(), "team", "bob", &record("e.md", true)).unwrap();
        let mut merged = engine.drafts_merged("team", "alice");
        merged.sort();
        assert_eq!(merged, vec!["b.md".to_string(), "c/d.md".to_string()]);
    }
}
