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
use std::path::Path;

use crystalline_index::{EngramRecord, FileStamp, StoredEngram};
use crystalline_remote::state::{OriginState, base_path, read_base_file};
use serde::Serialize;
use serde_json::{Value, json};

use super::{Engine, EngineError, Result, folder_may_fold_case, sha256_hex, utf8_side};
use crate::domain_view::DomainView;
use crate::draft_merge::{
    DraftConflictKind, DraftSides, DraftStep, TeamText, has_landed, next_step,
};
use crate::draft_start::{self, DraftStart};
use crate::origin::takes_part_in_local_change;
use crate::review::ActorDrafts;

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
enum Held {
    Absent,
    Bytes(Vec<u8>),
    Unreadable(String),
}

impl Held {
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

fn conflict_id(actor: &str, path: &str) -> String {
    let hash = sha256_hex(format!("{actor}\0{path}").as_bytes());
    format!("d{}", &hash[..7])
}

/// What a row with a record is, from the one decision (C7).
fn from_next_step(sides: &DraftSides<'_>) -> Option<DraftConflictKind> {
    match next_step(sides) {
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
    let stamp = FileStamp {
        mtime: 0,
        size: 0,
        sha256: String::new(),
    };
    Some(EngramRecord::from_engram(&engram, &tombstone.path, stamp).permalink)
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

impl Engine {
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
    /// and `upstream` the team's text the conflict is against (at her path for
    /// `AddAdd`, at `other` for `AddressTaken`, else where she started). An id
    /// of somebody else's is not found.
    pub async fn draft_conflict_sides(&self, domain: &str, actor: &str, id: &str) -> Result<Value> {
        let found = self
            .found_conflicts(domain, actor)
            .await?
            .into_iter()
            .find(|found| found.conflict.id == id)
            .ok_or_else(|| {
                EngineError::NotFound(format!("no conflict {id} among your drafts in '{domain}'"))
            })?;
        let mut note: Option<String> = None;
        let base = match &found.record {
            Some(DraftStart {
                start: Some(start), ..
            }) => Value::String(start.clone()),
            Some(DraftStart {
                start_sha256: Some(_),
                ..
            }) => {
                note = Some("the base side has no text and is omitted".to_string());
                Value::Null
            }
            _ => Value::Null,
        };
        let local = found.local.clone().map_or(Value::Null, Value::String);
        let upstream = match found.upstream {
            Held::Absent => Value::Null,
            Held::Bytes(bytes) => utf8_side(Some(bytes), "upstream", &mut note),
            Held::Unreadable(_) => {
                note = Some("the upstream side has no readable copy and is omitted".to_string());
                Value::Null
            }
        };
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
        reason = "the pass and status read it from Tasks 6 and 12 on"
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

    /// The table of the design, first match wins:
    ///
    /// 1. It has landed ([`has_landed`]): none.
    /// 2. A path that takes no part in a share: none.
    /// 3. The tombstone half of a move: none; the draft stands for both and
    ///    names it in `moved_from`.
    /// 4. A draft whose address another team file holds in her view:
    ///    `AddressTaken`, since Keep mine cannot clear it.
    /// 5. No record: the old-draft rows. With a record: [`next_step`].
    async fn found_conflicts(&self, domain: &str, actor: &str) -> Result<Vec<Found>> {
        if self.domain_entry(domain)?.origin.is_none() {
            return Ok(Vec::new());
        }
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let Some(state) = OriginState::load(&state_dir)? else {
            return Ok(Vec::new());
        };
        // The read-only id lookup: asking must not register a domain.
        let rows = {
            let store = self.store.lock().await;
            match store.domain_id(domain).await? {
                Some(id) => store.overlay_entries(id, actor).await?,
                None => Vec::new(),
            }
        };
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let journal_dir = self.journal_state_dir()?;
        // C1: per row, and an error refuses the whole answer.
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
        let held = ActorDrafts {
            actor: actor.to_string(),
            entries: rows,
            files: Vec::new(),
            files_unreadable: false,
        };
        let rows = &held.entries;

        // Her view of the team's addresses, as the fold plan asks it: the base
        // rows less the paths she deletes.
        let base_rows = if rows.iter().any(|row| !row.tombstone) {
            let store = self.store.lock().await;
            store.list_engrams(domain, None, None).await?
        } else {
            Vec::new()
        };
        let addresses = DomainView::address_map(&[&held], &base_rows);

        // A moved draft names where it started, whether or not its tombstone still
        // stands there: a conflict is about that path too, so a share or a
        // resolve that names it finds the conflict. A tombstone standing there is
        // the old half of the move: some draft of hers started from its path,
        // compared exactly.
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
        let old_pairs = old_moves(rows, &records);
        for (&t, &d) in &old_pairs {
            moved_from.insert(d, rows[t].path.clone());
        }

        let mut base = Base::new(&state_dir, &state)?;
        let root_is_there = root.is_dir();
        let mut frozen: Option<HashSet<String>> = None;
        let mut kinds: Vec<Option<(DraftConflictKind, Option<String>)>> = vec![None; rows.len()];
        for (i, row) in rows.iter().enumerate() {
            if !takes_part_in_local_change(&row.path) || halves.contains(&i) {
                continue;
            }
            let record = records[i].as_ref();
            let team_at_path = base.at(&row.path)?;
            let from = record.and_then(|record| record.from.as_deref());
            let team_at_from = match from {
                Some(from) if from != row.path => base.at(from)?,
                Some(_) => team_at_path.clone(),
                None => Held::Absent,
            };
            let sides = DraftSides {
                path: &row.path,
                from,
                start: record.and_then(|record| record.start.as_deref()),
                start_sha256: record.and_then(|record| record.start_sha256.as_deref()),
                shared: record
                    .and_then(|record| record.shared.as_ref())
                    .and_then(|shared| shared.text.as_deref()),
                folds_case: root_is_there && folder_may_fold_case(&root, &row.path),
                local: (!row.tombstone).then_some(row.content.as_str()),
                team_at_path: team_at_path.team(),
                team_at_from: team_at_from.team(),
            };
            if has_landed(&sides) {
                continue;
            }
            if !row.tombstone
                && let Some(other) = addresses
                    .get(row.permalink.as_str())
                    .filter(|other| **other != row.path)
            {
                kinds[i] = Some((DraftConflictKind::AddressTaken, Some((*other).to_string())));
                continue;
            }
            // C9: branch on the record before the decision.
            let kind = match record {
                Some(_) => from_next_step(&sides),
                None => old_draft_conflict(row, sides.team_at_path, || {
                    let listed = match &frozen {
                        Some(listed) => listed,
                        None => {
                            let read =
                                crate::overlay_journal::journal_record_strict(&journal_dir, domain)
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
                })?,
            };
            kinds[i] = kind.map(|kind| (kind, None));
        }
        // An old move lists once, at the draft: the tombstone's answer is the
        // draft's when the draft has none of its own.
        for (&t, &d) in &old_pairs {
            let half = kinds[t].take();
            if kinds[d].is_none() {
                kinds[d] = half;
            }
        }

        let mut found = Vec::new();
        for (i, kind) in kinds.into_iter().enumerate() {
            let Some((kind, other)) = kind else {
                continue;
            };
            let row = &rows[i];
            let record = records[i].clone();
            let moved_from = moved_from.get(&i).cloned();
            let upstream_at = match (&kind, &other) {
                (DraftConflictKind::AddAdd, _) => row.path.clone(),
                (DraftConflictKind::AddressTaken, Some(other)) => other.clone(),
                _ => moved_from
                    .clone()
                    .or_else(|| record.as_ref().and_then(|record| record.from.clone()))
                    .unwrap_or_else(|| row.path.clone()),
            };
            found.push(Found {
                conflict: DraftConflict {
                    id: conflict_id(actor, &row.path),
                    path: row.path.clone(),
                    kind,
                    moved_from,
                    file: false,
                    other,
                },
                record,
                local: (!row.tombstone).then(|| row.content.clone()),
                upstream: base.at(&upstream_at)?,
            });
        }
        found.sort_by(|a, b| a.conflict.path.cmp(&b.conflict.path));
        Ok(found)
    }
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
        }
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
