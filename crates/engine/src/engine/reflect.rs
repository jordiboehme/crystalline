//! `reflect`: the Reflection's work queue - feedback, update, review - recomputed
//! on every call and never stored, the shape of evolve's queue.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crystalline_core::conventional::parse_conventional;
use crystalline_core::reflection::{self as r, ReflectionState, Structure};
use crystalline_core::threads::{self as th, SectionRef, ThreadStatus, Waiting};

use super::reflection::{
    EngramFact, ReflectionAccess, reflection_abs, source_state, source_view, thread_view,
};
use super::sources::{HIDDEN_SOURCE_NOTE, TOO_WIDE_NOTE, unavailable_note};
use super::*;
use crate::scope::{ReflectionCaller, ReflectionRight, Scope};

/// How an agent works the queue, returned with every page.
pub const REFLECT_GUIDANCE: &str = "Work feedback items first: read what the person changed and decide - take it into the engrams (change them, then write_reflection the section with its sources), keep it as their text (write_reflection action=settle outcome=kept), or answer it in a thread (discuss with answers_feedback). Taking feedback into the engrams of another domain needs write access there; without it, answer in a thread or settle it as kept. Then update items: bring changed engrams into their sections and write them with sources; for a source_too_wide item, the section cites a domain fewer people may read than the page's domain: write the section again without that source and without the text drawn from it (the check sees only named sources, so dropping the source and keeping its text still leaks it); when write_reflection refuses a source for the same reason, leave out its text as well, unless your person, asked, wants to share it on purpose (the write asks them when their client can, and Fluid's Review offers it on the item; this is a deliberate step for a person, not a guard against their own agent; an unattended run never asks, cannot read such a domain at all, and leaves the text out); cover unreflected engrams with a new section or reflection; move pages the pact's structure does not fit (its ## Structure bullets are globs, one per top-level bullet); apply a changed pact to the pages it affects, and when the pages follow the pact again, settle PACT.md (write_reflection action=settle without sections settles the whole page). When a person asks you for a page or space, add a bullet to the pact's ## Pages (add the heading if the pact has none) with its path or glob, their request in their words, who asked and the date. Add a matching ## Structure glob and name each section's sources. To place new engrams read ## Pages, and to recreate a page rebuild it from its bullet. Files under reflections/assets/: link an unreferenced file from the section it belongs to, or remove it with write_reflection action=delete_asset (ask first: removing is irreversible); for a missing file, correct the link, or ask a person to add the file when no engram holds it; an engram attachment cannot be embedded, so copy it into reflections/assets/ yourself (read_engram with include_images=false, then write_reflection action=write_asset) and point the link there. A host_moved item means this instance took the domain over on a shared database and its disk lacks files the previous host wrote: their rows are kept and counted, never written here, so do not rewrite or delete those pages and files; ask a person to copy the domain folder over (or restore it) and sync, or to run the sync with --accept-disk if the files are gone for good. Answer review threads in plain words - the people reading them may not see engrams. Answer a person's thread and leave it open. The person who opened it resolves it. A thread item carries the label of its opening comment and whether it blocks: answer blocking threads first; a praise needs no answer unless it asks something. Present every page you write with the crystalline-presentation skill (the skills tool serves it).";

/// An off-structure item's hint when no Structure glob fits any page.
pub const STRUCTURE_HINT: &str = "The pact's ## Structure bullets are read as globs, one per top-level bullet, for example - guides/*.md. None of them matches a page.";

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 100;

/// One open item of a domain's Reflection.
#[derive(Debug, Clone)]
struct Item {
    kind: &'static str,
    what: &'static str,
    domain: String,
    path: String,
    /// The section's id; `None` for an item about a whole page, the pact or
    /// a folder, and for a section with no id yet (then `heading` names it).
    section: Option<String>,
    heading: String,
    age_secs: i64,
    extra: Value,
    /// What the item's version is a hash of: only what moves when the item
    /// really changes (a thread's last entry, a feedback's text, a section's
    /// source checksums), never an age. See [`item_basis`].
    basis: String,
}

/// Which section an item is about, for grouping: its id, else its heading.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SectionKey {
    Id(String),
    Heading(String),
}

impl Item {
    /// The group an item belongs to: the section it names, or none (a group
    /// of its own).
    fn group(&self) -> Option<(String, String, SectionKey)> {
        let key = match &self.section {
            Some(id) => SectionKey::Id(id.clone()),
            None if !self.heading.is_empty() => SectionKey::Heading(self.heading.clone()),
            None => return None,
        };
        Some((self.domain.clone(), self.path.clone(), key))
    }

    /// Feedback, then update, then review inside a group.
    fn rank(&self) -> u8 {
        match self.kind {
            "feedback" => 0,
            "update" => 1,
            _ => 2,
        }
    }

    /// Whether the item is a thread whose opening comment blocks: it comes
    /// before the other items of its rank in its group.
    fn blocks(&self) -> bool {
        self.extra.get("blocking") == Some(&Value::Bool(true))
    }
}

fn secs_since_rfc3339(at: Option<&str>) -> Option<i64> {
    let at = chrono::DateTime::parse_from_rfc3339(at?).ok()?;
    Some(
        (chrono::Utc::now() - at.with_timezone(&chrono::Utc))
            .num_seconds()
            .max(0),
    )
}

fn secs_since_epoch(mtime: i64) -> i64 {
    (chrono::Utc::now().timestamp() - mtime).max(0)
}

/// An item's version: a sha256 over `parts`, each closed by a NUL so two
/// splits of one text never hash alike.
fn item_basis<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    crystalline_index::hex_lower(&hasher.finalize())
}

/// A thread's version: how many entries it holds and its last one, so a
/// new message moves it and the passing of time does not.
fn thread_basis(thread: &th::Thread) -> String {
    let count = thread.entries.len().to_string();
    match thread.entries.last() {
        Some(last) => item_basis([
            thread.id.as_str(),
            count.as_str(),
            last.at.as_str(),
            last.author.name.as_str(),
            last.text.as_str(),
        ]),
        None => item_basis([thread.id.as_str(), count.as_str()]),
    }
}

/// Rank items: groups oldest first, feedback before update before review
/// inside, and a blocking thread before the other threads of its group.
fn rank(items: Vec<Item>) -> Vec<Item> {
    let mut groups: Vec<Vec<Item>> = Vec::new();
    let mut index: HashMap<(String, String, SectionKey), usize> = HashMap::new();
    for item in items {
        match item.group() {
            Some(key) => {
                let at = *index.entry(key).or_insert_with(|| {
                    groups.push(Vec::new());
                    groups.len() - 1
                });
                groups[at].push(item);
            }
            None => groups.push(vec![item]),
        }
    }
    for group in &mut groups {
        group.sort_by_key(|item| (item.rank(), !item.blocks()));
    }
    let oldest = |g: &Vec<Item>| g.iter().map(|i| i.age_secs).max().unwrap_or(0);
    groups.sort_by(|a, b| {
        oldest(b)
            .cmp(&oldest(a))
            .then_with(|| a[0].path.cmp(&b[0].path))
    });
    groups.into_iter().flatten().collect()
}

/// What a thread item carries beside its place: the thread's id and, when
/// its opening comment is a conventional comment, that comment's label and
/// whether it blocks. A plain opening comment adds neither key.
fn thread_extra(thread: &th::Thread) -> Value {
    let opening = thread
        .entries
        .first()
        .filter(|entry| entry.kind == th::EntryKind::Comment)
        .and_then(|entry| parse_conventional(&entry.text));
    match opening {
        Some(comment) => json!({
            "thread": thread.id,
            "label": comment.label.as_str(),
            "blocking": comment.is_blocking(),
        }),
        None => json!({ "thread": thread.id }),
    }
}

/// The text of `sections`, each normalized as the hash sees it, empty ones
/// left out: the two sides of the pact's diff.
fn joined_text<'a>(sections: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for text in sections.map(r::normalize_section_text) {
        if text.is_empty() {
            continue;
        }
        out.push_str(&text);
        out.push_str("\n\n");
    }
    out
}

/// The domains a queue runs over, and the ones it leaves out and names.
#[derive(Debug, Default)]
struct QueueScope {
    /// The domains whose queue is read.
    domains: Vec<String>,
    /// Left out: their Reflection's state could not be read.
    unreadable: Vec<String>,
    /// Left out: their Reflection waits for engrams to leave its folder.
    pending: Vec<String>,
}

/// Whether a queue item is shown to a caller at all. An unreflected item
/// is about engrams (the folder they sit in and their names), so a caller
/// who may not see engrams is not told of it, not even as a count.
fn shown_to(item: &Item, engrams_visible: bool) -> bool {
    engrams_visible || item.what != "unreflected"
}

/// One item a headless reflect run would work: its key and version, and
/// the page it is on (empty for an item of no page).
pub(super) struct ReflectDue {
    pub(super) key: crystalline_index::DueKey,
    pub(super) page: String,
}

/// How much of each item [`Engine::detect_reflection`] builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    /// Every member a queue row shows: diffs, sources.
    Full,
    /// Only what ranks and counts an item; no diff is rendered and no
    /// source view is built, though every source's state is still read (the
    /// nudge counts).
    Count,
    /// Only the update items, as [`Detail::Count`] builds them: no page is
    /// compared with its baseline for feedback and no thread is queued (the
    /// Reflection home's update count).
    Updates,
}

impl Engine {
    /// The files under `reflections/assets/` (by path inside
    /// `reflections/`) that only `held` pages of `domain` link: they carry
    /// what those pages show, so the backup holds them back with them. A
    /// file a page that goes out links too stays.
    ///
    /// The links are read from `files`, the bytes the backup is about to
    /// push (by repo path, see
    /// [`crystalline_remote::reflections_backup::collect_backup`]), so a
    /// page known only from its file on disk counts; a held page's links in
    /// the index count as well.
    pub(crate) async fn held_assets(
        &self,
        domain: &str,
        held: &[String],
        spec: &crystalline_remote::provider::OriginSpec,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<BTreeSet<String>> {
        if held.is_empty() {
            return Ok(BTreeSet::new());
        }
        let rows = {
            let store = self.store.lock().await;
            let Some(id) = store.domain_id(domain).await? else {
                return Ok(BTreeSet::new());
            };
            store.list_reflections(id).await?
        };
        let mut by_held = BTreeSet::new();
        for row in &rows {
            if held.contains(&row.path) {
                by_held.extend(r::reflection_asset_refs(&row.content, &row.path));
            }
        }
        let mut by_others = BTreeSet::new();
        for (repo_path, bytes) in files {
            let Some(inner) =
                crystalline_remote::reflections_backup::reflection_path(spec, repo_path)
            else {
                continue;
            };
            let r::ReflectionFile::Page(page) = r::classify(inner) else {
                continue;
            };
            let refs = r::reflection_asset_refs(&String::from_utf8_lossy(bytes), &page);
            if held.contains(&page) {
                by_held.extend(refs);
            } else {
                by_others.extend(refs);
            }
        }
        Ok(by_held.difference(&by_others).cloned().collect())
    }

    /// Schedule a backup of every Reflection this instance backs up that has
    /// a page newly citing a domain fewer people may read (or one not shared
    /// through GitHub): a page its last backup did not hold back. Answers
    /// the domains marked, sorted.
    ///
    /// Who may read a domain changes in the accounts store (a domain made
    /// private, a member removed, a role lowered), often from another
    /// process, and nothing in the Reflection is marked by it. Without this
    /// look such a page would stay on the backup branch head until the next
    /// change. The daemon's recount runs it every few minutes, and the JSON
    /// API's accounts routes run it after each change.
    pub async fn schedule_too_wide_backups(&self) -> Vec<String> {
        let mut marked = Vec::new();
        let domains: Vec<String> = self.registered_domain_entries().keys().cloned().collect();
        for domain in domains {
            if self.reflection_switch(&domain) == crystalline_core::ReflectionSwitch::Off
                || !self.backs_up_reflection_here(&domain).await
            {
                continue;
            }
            let pages = match self.too_wide_pages(&domain).await {
                Ok(pages) => pages,
                Err(e) => {
                    tracing::debug!(domain = %domain, "too wide pages not read: {e}");
                    continue;
                }
            };
            let held = self.reflection_backup_held_back(&domain);
            if pages.iter().any(|page| !held.contains(page)) {
                self.mark_reflection_dirty(&domain, crate::reflection_backup::BackupCause::Settled);
                marked.push(domain);
            }
        }
        marked.sort();
        marked
    }

    /// The pages of `domain` (by path inside `reflections/`, sorted) with a
    /// section that cites a domain fewer people may read than `domain`: the
    /// pages the queue names with `source_too_wide`, for every source, seen
    /// by whom or not. A source whose domain cannot be checked here is not
    /// counted, as in the queue.
    pub(crate) async fn too_wide_pages(&self, domain: &str) -> Result<Vec<String>> {
        let (rows, id) = {
            let store = self.store.lock().await;
            let Some(id) = store.domain_id(domain).await? else {
                return Ok(Vec::new());
            };
            (store.list_reflections(id).await?, id)
        };
        let mut check = TooWide::new(self, domain, id).await?;
        let mut out = Vec::new();
        for row in rows {
            if check.text(&row.path, &row.content).await? {
                out.push(row.path);
            }
        }
        out.sort();
        Ok(out)
    }

    /// The pages of `domain` (by path inside `reflections/`, sorted) whose
    /// bytes among `files` (a backup's collected files, by repo path, see
    /// [`crystalline_remote::reflections_backup::collect_backup`]) cite a
    /// domain fewer people may read, or may and cannot be read: bytes that
    /// are not UTF-8, or records [`r::provenance_unreadable`] says so of.
    /// What [`Engine::too_wide_pages`] says of the index, said of what is
    /// about to go out, so a file edited on disk before the index read it is
    /// held back too.
    pub(crate) async fn too_wide_in_files(
        &self,
        domain: &str,
        spec: &crystalline_remote::provider::OriginSpec,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Vec<String>> {
        let Some(id) = self.store.lock().await.domain_id(domain).await? else {
            return Ok(Vec::new());
        };
        let mut check = TooWide::new(self, domain, id).await?;
        let mut out = Vec::new();
        for (inner, bytes) in reflection_files(spec, files) {
            let r::ReflectionFile::Page(page) = r::classify(inner) else {
                continue;
            };
            let wide = match std::str::from_utf8(bytes) {
                Ok(text) => check.text(&page, text).await?,
                Err(_) => true,
            };
            if wide {
                out.push(page);
            }
        }
        out.sort();
        Ok(out)
    }

    /// Every open item of one domain's Reflection, unranked, as `scope`
    /// may see them: a source in a domain it may not open is counted on its
    /// item and never named. `threads` is [`Engine::all_threads`] of the
    /// same domain, read once by a caller that needs the threads too.
    async fn detect_reflection(
        &self,
        domain: &str,
        access: &ReflectionAccess,
        detail: Detail,
        threads: &[(String, th::Thread, String)],
        scope: &Scope,
    ) -> Result<Vec<Item>> {
        let full = detail == Detail::Full;
        let feedback = detail != Detail::Updates;
        let (rows, host_move) = {
            let store = self.store.lock().await;
            (
                store.list_reflections(access.domain_id).await?,
                store.host_move(access.domain_id).await?,
            )
        };
        // The domain's own engrams are scanned once (the unreflected items
        // need all of them); every other domain a source names is read one
        // seek per cited permalink, for every page at once.
        let provenances: Vec<r::Provenance> = rows
            .iter()
            .map(|row| r::read_provenance(&row.content))
            .collect();
        let foreign: Vec<&r::SourceRef> = provenances
            .iter()
            .flat_map(|prov| prov.values())
            .flat_map(|record| record.sources.iter())
            .filter(|src| src.domain.is_some())
            .collect();
        let sight = if foreign.is_empty() {
            None
        } else {
            Some(self.source_sight(scope).await?)
        };
        let sees = |src: &r::SourceRef| sight.as_ref().is_none_or(|sight| sight.sees(src));
        let own = self.engram_facts(access.domain_id, domain).await?;
        let facts = self
            .source_facts(domain, access.domain_id, Some(own), foreign.into_iter())
            .await?;
        let open_thread = |id: &str| {
            threads
                .iter()
                .any(|(_, t, _)| t.id == id && t.status == ThreadStatus::Open)
        };
        let pact_globs = rows
            .iter()
            .find(|row| row.path == r::PACT_FILE)
            .map(|row| r::structure_globs(&row.content))
            .unwrap_or_default();
        // Compiled once for every page of the domain.
        let structure = Structure::new(&pact_globs);
        // Globs that fit no page at all most likely are not meant as globs:
        // every off-structure item then says how the bullets are read.
        let no_glob_fits = !pact_globs.is_empty()
            && !rows
                .iter()
                .any(|row| row.path != r::PACT_FILE && structure.fits(&row.path));
        let mut cited: HashSet<String> = HashSet::new();
        // Whether this domain's pages may cite each other domain, asked
        // once per domain for the whole scan.
        let mut covers: HashMap<String, bool> = HashMap::new();
        let disclosures = self.reflection_disclosures(access.domain_id).await?;
        let mut items = Vec::new();
        for (row, prov) in rows.iter().zip(&provenances) {
            // The baseline is read for feedback alone, and the pact's is
            // always read: a changed pact is an update.
            let baseline = if feedback || row.path == r::PACT_FILE {
                std::fs::read_to_string(access.abs(&r::baseline_path(&row.path))).ok()
            } else {
                None
            };
            // While a host move is unresolved, a page whose baseline is not on
            // this disk is one the previous host reflected: without the
            // baseline every section would read as a person's change, so the
            // move's one item stands in for that feedback.
            let no_feedback = baseline.is_none() && host_move.is_some();
            // Without a baseline file (a restore from the backup branch,
            // which does not carry them) the page stands for its baseline.
            let baseline = super::reflection::baseline_or_seed(baseline, Some(&row.content));
            let row_age = secs_since_rfc3339(Some(&row.modified)).unwrap_or(0);
            if row.path == r::PACT_FILE {
                let changes = if no_feedback {
                    Vec::new()
                } else {
                    r::feedback_changes(&row.content, &baseline)
                };
                if !changes.is_empty() {
                    let extra = if full {
                        let sections = r::split_sections(&row.content);
                        let current = joined_text(sections.iter().map(|s| s.text.as_str()));
                        let reflected_entries = r::parse_baseline(&baseline);
                        let reflected =
                            joined_text(reflected_entries.iter().map(|(_, t)| t.as_str()));
                        // The ids a settle names; a heading typed without
                        // an id gets one on the next write through Crystalline.
                        let ids: Vec<&str> =
                            changes.iter().filter_map(|c| c.id.as_deref()).collect();
                        json!({
                            "diff": diffy::create_patch(&reflected, &current).to_string(),
                            "sections": ids,
                        })
                    } else {
                        Value::Null
                    };
                    items.push(Item {
                        kind: "update",
                        what: "pact_changed",
                        domain: domain.to_string(),
                        path: row.path.clone(),
                        section: None,
                        heading: String::new(),
                        age_secs: row_age,
                        extra,
                        basis: item_basis([row.content.as_str()]),
                    });
                }
                continue;
            }
            let changes = if no_feedback || !feedback {
                Vec::new()
            } else {
                r::feedback_changes(&row.content, &baseline)
            };
            for change in changes {
                let record = change.id.as_deref().and_then(|id| prov.get(id));
                if record
                    .and_then(|rec| rec.answered_by.as_deref())
                    .is_some_and(open_thread)
                {
                    continue;
                }
                let age = secs_since_rfc3339(record.and_then(|rec| rec.changed_at.as_deref()))
                    .unwrap_or(row_age);
                let extra = if full {
                    let all = record.map(|rec| rec.sources.as_slice()).unwrap_or_default();
                    let sources: Vec<Value> = all
                        .iter()
                        .filter(|s| sees(s))
                        .map(|s| source_view(s, &facts))
                        .collect();
                    let mut extra = json!({
                        "changed_by": record.map(|rec| rec.changed_by.clone()).unwrap_or_default(),
                        "diff": diffy::create_patch(
                            &r::normalize_section_text(&change.before),
                            &r::normalize_section_text(&change.after),
                        )
                        .to_string(),
                        "sources": sources,
                    });
                    let hidden = all.iter().filter(|s| !sees(s)).count();
                    if hidden > 0 {
                        extra["hidden_sources"] = json!(hidden);
                        extra["note"] = json!(HIDDEN_SOURCE_NOTE);
                    }
                    extra
                } else {
                    Value::Null
                };
                items.push(Item {
                    kind: "feedback",
                    what: change.kind.as_str(),
                    domain: domain.to_string(),
                    path: row.path.clone(),
                    section: change.id.clone(),
                    heading: change.heading.clone(),
                    age_secs: age,
                    extra,
                    basis: item_basis([
                        r::normalize_section_text(&change.before).as_str(),
                        r::normalize_section_text(&change.after).as_str(),
                    ]),
                });
            }
            // The sources of domains that cannot be checked here, by
            // domain: the count and the sections that cite them.
            let mut unavailable: BTreeMap<&str, (usize, Vec<String>)> = BTreeMap::new();
            for section in r::split_sections(&row.content) {
                let Some(id) = section.id.as_deref() else {
                    continue;
                };
                let Some(record) = prov.get(id) else {
                    continue;
                };
                // Per `what`: the oldest age, the sources shown, how many the
                // caller may not see, and every source's engram with its
                // checksum now, the item's version basis.
                type ByWhat = (i64, Vec<Value>, usize, Vec<String>);
                let mut by_what: BTreeMap<&'static str, ByWhat> = BTreeMap::new();
                let stamp = |src: &r::SourceRef| {
                    format!(
                        "{}:{}:{}",
                        src.domain.as_deref().unwrap_or_default(),
                        src.engram,
                        facts
                            .fact(src)
                            .map(|f| f.checksum.as_str())
                            .unwrap_or_default()
                    )
                };
                for src in &record.sources {
                    // Unreflected stays per domain: citing another domain's
                    // engram covers nothing of this one.
                    if src.domain.is_none() {
                        cited.insert(src.engram.clone());
                    }
                    // The read and the queue decide a source's state in one place.
                    let state = source_state(src, &facts).0;
                    // A source the caller may not see tells nothing of
                    // itself: whatever its state, and whether its domain is
                    // registered here or not, it counts as one changed
                    // source, aged by the page, never by its engram.
                    if !sees(src) {
                        if state != "current" {
                            let entry = by_what.entry("source_changed").or_insert((
                                0,
                                Vec::new(),
                                0,
                                Vec::new(),
                            ));
                            entry.0 = entry.0.max(row_age);
                            entry.2 += 1;
                            entry.3.push(stamp(src));
                        }
                        continue;
                    }
                    let engram_age = || {
                        facts
                            .fact(src)
                            .map_or(row_age, |f| secs_since_epoch(f.mtime))
                    };
                    let (what, age) = match state {
                        "missing" => ("source_missing", row_age),
                        "retired" => ("source_retired", engram_age()),
                        "changed" => ("source_changed", engram_age()),
                        "unavailable" => {
                            // By the name the read view shows (the local
                            // one), the stored name when nothing resolves.
                            if let Some(domain) = facts.shown_domain(src).or(src.domain.as_deref())
                            {
                                let entry = unavailable.entry(domain).or_default();
                                entry.0 += 1;
                                if !entry.1.iter().any(|s| s == id) {
                                    entry.1.push(id.to_string());
                                }
                            }
                            continue;
                        }
                        _ => continue,
                    };
                    let entry = by_what
                        .entry(what)
                        .or_insert((0, Vec::new(), 0, Vec::new()));
                    entry.0 = entry.0.max(age);
                    entry.3.push(stamp(src));
                    if full {
                        entry.1.push(source_view(src, &facts));
                    }
                }
                // Sources in a domain fewer people may read than this one
                // (`readers_cover`): written before the rule, edited on disk,
                // pulled in, or left behind when that domain's readers
                // narrowed or this one's widened. Nothing refuses a file, so
                // the queue names the section until a curator takes the
                // source and the text drawn from it out; deleting text is a
                // person's decision, so the page keeps serving meanwhile.
                // One item per section, whatever the number of sources.
                // Only a caller who may open the source's domain is told:
                // for anyone else the item would set a hidden source in a
                // domain registered here apart from one in a name nothing
                // resolves, and a hidden source tells nothing of itself.
                let mut wide_shown: Vec<Value> = Vec::new();
                let mut wide_stamps: Vec<String> = Vec::new();
                let mut wide_domains: Vec<String> = Vec::new();
                for src in record.sources.iter().filter(|src| sees(src)) {
                    let Some(local) = facts.checked_domain(src) else {
                        continue;
                    };
                    let covered = match covers.get(local) {
                        Some(covered) => *covered,
                        None => {
                            let covered = self.readers_cover(domain, local).await?;
                            covers.insert(local.to_string(), covered);
                            covered
                        }
                    };
                    if covered {
                        continue;
                    }
                    // Shared on purpose by a person, for the page's readers
                    // and repository as they still are.
                    if let Some(stored) = src.domain.as_deref()
                        && self
                            .source_covered_on_page(domain, &row.path, local, stored, &disclosures)
                            .await?
                    {
                        continue;
                    }
                    if !wide_domains.iter().any(|d| d == local) {
                        wide_domains.push(local.to_string());
                    }
                    wide_stamps.push(format!(
                        "{}:{}",
                        src.domain.as_deref().unwrap_or_default(),
                        src.engram
                    ));
                    if full {
                        wide_shown.push(source_view(src, &facts));
                    }
                }
                if !wide_stamps.is_empty() {
                    wide_stamps.sort();
                    wide_domains.sort();
                    // What a person is asked to share it on purpose (Fluid's
                    // Review asks it), and the basis their yes carries back;
                    // it names only domains this caller may open.
                    let ask = self
                        .disclosure_ask_for(
                            domain,
                            &row.path,
                            &wide_domains,
                            std::slice::from_ref(&section.text),
                        )
                        .await?;
                    let extra = json!({
                        "sources": wide_shown,
                        "note": TOO_WIDE_NOTE,
                        "question": ask.question,
                        "basis": ask.basis,
                    });
                    items.push(Item {
                        kind: "update",
                        what: "source_too_wide",
                        domain: domain.to_string(),
                        path: row.path.clone(),
                        section: Some(id.to_string()),
                        heading: section.heading.clone(),
                        age_secs: row_age,
                        extra,
                        basis: item_basis(wide_stamps.iter().map(String::as_str)),
                    });
                }
                for (what, (age, sources, hidden, mut stamps)) in by_what {
                    stamps.sort();
                    let mut extra = json!({ "sources": sources });
                    if hidden > 0 {
                        extra["hidden_sources"] = json!(hidden);
                        extra["note"] = json!(HIDDEN_SOURCE_NOTE);
                    }
                    items.push(Item {
                        kind: "update",
                        what,
                        domain: domain.to_string(),
                        path: row.path.clone(),
                        section: Some(id.to_string()),
                        heading: section.heading.clone(),
                        age_secs: age,
                        extra,
                        basis: item_basis(stamps.iter().map(String::as_str)),
                    });
                }
            }
            // One item per page and domain that cannot be checked here, so
            // no agent drops a source because a domain is offline. Those in
            // a domain the caller may not open are on the section's changed
            // item above, as any other source it may not see.
            for (source_domain, (count, sections)) in unavailable {
                let basis = item_basis(
                    [source_domain, &count.to_string()]
                        .into_iter()
                        .chain(sections.iter().map(String::as_str)),
                );
                items.push(Item {
                    kind: "update",
                    what: "source_unavailable",
                    domain: domain.to_string(),
                    path: row.path.clone(),
                    section: None,
                    heading: String::new(),
                    age_secs: row_age,
                    extra: json!({
                        "source_domain": source_domain,
                        "count": count,
                        "sections": sections,
                        "note": unavailable_note(source_domain),
                    }),
                    basis,
                });
            }
            if !structure.fits(&row.path) {
                items.push(Item {
                    kind: "update",
                    what: "off_structure",
                    domain: domain.to_string(),
                    path: row.path.clone(),
                    section: None,
                    heading: String::new(),
                    age_secs: row_age,
                    extra: if no_glob_fits {
                        json!({ "structure": pact_globs, "hint": STRUCTURE_HINT })
                    } else {
                        json!({ "structure": pact_globs })
                    },
                    basis: item_basis(pact_globs.iter().map(String::as_str)),
                });
            }
        }
        // A Reflection's files: a reference to a file that is not
        // there, an engram attachment embedded outside Crystalline (a plain
        // editor, a room), and a file no page links. Reflections and
        // PACT.md are the pages that count; threads and the baseline never do.
        let assets = self.asset_list_in(AssetHome::Reflection, domain).await?;
        let mut referenced: HashSet<String> = HashSet::new();
        for row in &rows {
            let row_age = secs_since_rfc3339(Some(&row.modified)).unwrap_or(0);
            for section in r::split_sections(&row.content) {
                let mut missing: Vec<String> = Vec::new();
                for asset in r::reflection_asset_refs(&section.text, &row.path) {
                    // The disk, not the row: a file the walker has not indexed
                    // yet is there, and a row whose bytes are not on this disk
                    // (kept through a host move, or deleted while nothing
                    // watched) is a broken image here (R26).
                    if !reflection_abs(&access.root, &asset).is_file() {
                        missing.push(asset.clone());
                    }
                    referenced.insert(asset);
                }
                let embedded = r::engram_attachment_embeds(&section.text, &row.path);
                for (what, key, list) in [
                    ("asset_missing", "assets", missing),
                    ("engram_attachment", "targets", embedded),
                ] {
                    if list.is_empty() {
                        continue;
                    }
                    let basis = item_basis(list.iter().map(String::as_str));
                    let mut extra = serde_json::Map::new();
                    extra.insert(key.to_string(), json!(list));
                    items.push(Item {
                        kind: "update",
                        what,
                        domain: domain.to_string(),
                        path: row.path.clone(),
                        section: section.id.clone(),
                        heading: section.heading.clone(),
                        age_secs: row_age,
                        extra: Value::Object(extra),
                        basis,
                    });
                }
            }
        }
        for asset in &assets {
            // A file the previous host's disk keeps is the move's to report;
            // nobody is asked to link or remove what is not here.
            if referenced.contains(&asset.path)
                || (host_move.is_some() && !reflection_abs(&access.root, &asset.path).is_file())
            {
                continue;
            }
            items.push(Item {
                kind: "update",
                what: "asset_unreferenced",
                domain: domain.to_string(),
                path: String::new(),
                section: None,
                heading: String::new(),
                age_secs: secs_since_rfc3339(Some(&asset.modified)).unwrap_or(0),
                extra: json!({ "asset": asset.path }),
                basis: item_basis([asset.path.as_str()]),
            });
        }
        // An unresolved host move with rows this disk lacks: one item for
        // the domain, counts only (Task 50).
        if let Some(host_move) = host_move
            .as_ref()
            .filter(|m| m.walked && m.only_elsewhere.total() > 0)
        {
            items.push(Item {
                kind: "update",
                what: "host_moved",
                domain: domain.to_string(),
                path: String::new(),
                section: None,
                heading: String::new(),
                age_secs: secs_since_rfc3339(Some(&host_move.at)).unwrap_or(0),
                extra: json!({ "only_elsewhere": host_move.only_elsewhere }),
                basis: item_basis([host_move.at.as_str()]),
            });
        }
        let mut loose: BTreeMap<String, Vec<&EngramFact>> = BTreeMap::new();
        for fact in facts.own.values() {
            if cited.contains(&fact.permalink)
                || fact.permalink == "manifest"
                || crystalline_index::is_retired_status(&fact.status.to_lowercase())
            {
                continue;
            }
            let folder = fact
                .permalink
                .split_once('/')
                .map(|(f, _)| f)
                .unwrap_or("")
                .to_string();
            loose.entry(folder).or_default().push(fact);
        }
        for (folder, mut engrams) in loose {
            engrams.sort_by(|a, b| a.permalink.cmp(&b.permalink));
            let oldest = engrams
                .iter()
                .map(|f| secs_since_epoch(f.mtime))
                .max()
                .unwrap_or(0);
            items.push(Item {
                kind: "update",
                what: "unreflected",
                domain: domain.to_string(),
                path: String::new(),
                section: None,
                heading: String::new(),
                age_secs: oldest,
                basis: item_basis(engrams.iter().map(|f| f.permalink.as_str())),
                extra: json!({
                    "folder": folder,
                    "count": engrams.len(),
                    "engrams": engrams.iter().take(10).map(|f| f.permalink.clone()).collect::<Vec<_>>(),
                }),
            });
        }
        for (path, thread, text) in threads.iter().filter(|_| feedback) {
            if thread.status != ThreadStatus::Open || th::waiting_on(thread) != Waiting::Agent {
                continue;
            }
            // The anchor names its section by id or, on a heading that had
            // none when the thread was opened, by heading: either way the
            // item names the section as it stands now, so it joins that
            // section's group. A section that is gone keeps the anchor's own
            // form.
            let sections = r::split_sections(text);
            let (section, heading) = match (
                r::find_section(&sections, thread.anchor.section.key()),
                &thread.anchor.section,
            ) {
                (Some(found), _) => (found.id.clone(), found.heading.clone()),
                (None, SectionRef::Id(id)) => (Some(id.clone()), String::new()),
                (None, SectionRef::Heading(heading)) => (None, heading.clone()),
            };
            items.push(Item {
                kind: "review",
                what: "thread",
                domain: domain.to_string(),
                path: path.clone(),
                section,
                heading,
                age_secs: secs_since_rfc3339(Some(thread.last_at())).unwrap_or(0),
                extra: thread_extra(thread),
                basis: thread_basis(thread),
            });
        }
        Ok(items)
    }

    fn page_json(
        items: &[Item],
        kinds: &[String],
        limit: Option<usize>,
        page: Option<usize>,
        engrams_visible: bool,
    ) -> Value {
        let filtered: Vec<&Item> = items
            .iter()
            .filter(|i| kinds.is_empty() || kinds.iter().any(|k| k == i.kind))
            .filter(|i| shown_to(i, engrams_visible))
            .collect();
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let page = page.unwrap_or(1).max(1);
        let offset = (page - 1).saturating_mul(limit);
        let count = |k: &str| filtered.iter().filter(|i| i.kind == k).count();
        let queue: Vec<Value> = filtered
            .iter()
            .enumerate()
            .skip(offset)
            .take(limit)
            .map(|(n, i)| {
                let mut row = json!({
                    "n": n + 1,
                    "kind": i.kind,
                    "what": i.what,
                    "domain": i.domain,
                    "path": i.path,
                    "section": i.section,
                    "heading": i.heading,
                    "age_secs": i.age_secs,
                });
                if let (Value::Object(target), Value::Object(extra)) = (&mut row, i.extra.clone()) {
                    for (k, v) in extra {
                        target.insert(k, v);
                    }
                }
                if !engrams_visible {
                    let obj = row.as_object_mut().expect("an object");
                    obj.remove("sources");
                    obj.remove("engrams");
                    obj.remove("folder");
                    // The engram file paths a page links.
                    obj.remove("targets");
                    // What an item says of sources in other domains.
                    obj.remove("source_domain");
                    obj.remove("hidden_sources");
                    obj.remove("note");
                    obj.remove("sections");
                }
                row
            })
            .collect();
        json!({
            "total": filtered.len(),
            "page": page,
            "limit": limit,
            "count": queue.len(),
            "kinds": { "feedback": count("feedback"), "update": count("update"), "review": count("review") },
            "queue": queue,
            "guidance": REFLECT_GUIDANCE,
        })
    }

    /// The domains a queue or a count runs over: the named ones (each once,
    /// in the order first named), or every registered domain whose
    /// Reflection is in effect here. Beside them, the domains of an unnamed
    /// scope whose state could not be read: they are left out, not a
    /// failure of the whole scope, and those whose Reflection is pending.
    /// `lenient` screens the named ones the way the unnamed scope screens
    /// every domain (see `ReflectParams::skip_unavailable`).
    async fn reflect_scope(&self, named: &[String], lenient: bool) -> Result<QueueScope> {
        let mut once: Vec<String> = Vec::new();
        for name in named {
            if !once.contains(name) {
                once.push(name.clone());
            }
        }
        if !named.is_empty() && !lenient {
            return Ok(QueueScope {
                domains: once,
                ..QueueScope::default()
            });
        }
        let names = if named.is_empty() {
            let mut names = self.known_domain_names();
            names.sort();
            names.dedup();
            names
        } else {
            // A name nobody registered is left out like any other domain
            // the queue cannot work, never reported as unreadable.
            let known = self.known_domain_names();
            once.into_iter().filter(|n| known.contains(n)).collect()
        };
        // An unnamed queue leaves out a read-only domain (like evolve) and a
        // local copy a connected server replaces (decision D19).
        let shadowed = self.shadowed_domains();
        let mut out = Vec::new();
        let mut unreadable = Vec::new();
        let mut pending = Vec::new();
        for name in names {
            if shadowed.contains(&name) || self.domain_is_read_only(&name).await {
                continue;
            }
            match self.reflection_state(&name).await {
                Ok(ReflectionState::On { .. }) => out.push(name),
                Ok(ReflectionState::Pending { .. }) => pending.push(name),
                // Not served here: another instance's queue, or nobody's yet.
                Ok(
                    ReflectionState::Off
                    | ReflectionState::Homeless
                    | ReflectionState::Elsewhere { .. }
                    | ReflectionState::HostedElsewhere { .. },
                ) => {}
                Err(e) => {
                    tracing::warn!(
                        "reflect: the Reflection state of '{name}' could not be read: {e}"
                    );
                    unreadable.push(name);
                }
            }
        }
        Ok(QueueScope {
            domains: out,
            unreadable,
            pending,
        })
    }

    /// The `reflect` tool.
    pub async fn reflect(&self, p: &ReflectParams, caller: &ReflectionCaller) -> Result<Value> {
        for kind in &p.kinds {
            if !matches!(kind.as_str(), "feedback" | "update" | "review") {
                return Err(EngineError::Invalid(format!(
                    "`{kind}` is not a reflect kind; use feedback, update or review"
                )));
            }
        }
        // A named domain that cannot be worked is refused, unless the caller
        // asked for the unnamed queue's leniency over its names.
        let named = !p.domains.is_empty() && !p.skip_unavailable;
        let mut items = Vec::new();
        let mut swept = Vec::new();
        let mut visible_everywhere = true;
        let QueueScope {
            domains: scope,
            mut unreadable,
            pending,
        } = self.reflect_scope(&p.domains, p.skip_unavailable).await?;
        // A domain whose Reflection waits for engrams to leave its folder is
        // named, for a caller who reads that Reflection, so the person learns
        // why its queue is not there (names only, as for unreadable).
        let mut waiting = Vec::new();
        for domain in pending {
            if matches!(
                self.caller_reflection_right(&domain, caller).await,
                Ok((right, _)) if right >= ReflectionRight::Read
            ) {
                waiting.push(domain);
            }
        }
        for domain in scope {
            let access = match self
                .require_reflection(&domain, caller, ReflectionRight::Read)
                .await
            {
                Ok(access) => access,
                Err(e) if named => return Err(e),
                Err(_) => continue,
            };
            // One page answers every swept domain, so a domain that hides
            // engrams from the caller strips the sources, folders, engrams and targets of the whole page.
            visible_everywhere &= access.engrams_visible;
            let found = match self.all_threads(&access).await {
                Ok(threads) => {
                    self.detect_reflection(&domain, &access, Detail::Full, &threads, &caller.scope)
                        .await
                }
                Err(e) => Err(e),
            };
            match found {
                Ok(found) => items.extend(found),
                Err(e) if named => return Err(e),
                // One domain that cannot be read never costs the whole
                // queue: it is named in the answer and left out.
                Err(e) => {
                    tracing::warn!("reflect: the queue of '{domain}' could not be read: {e}");
                    unreadable.push(domain);
                    continue;
                }
            }
            swept.push(domain);
        }
        let mut value =
            Self::page_json(&rank(items), &p.kinds, p.limit, p.page, visible_everywhere);
        value["scope"] = json!({ "domains": swept, "kinds": p.kinds });
        // Present only when a domain was left out, so a whole answer keeps
        // its shape.
        if !unreadable.is_empty() {
            value["scope"]["unreadable"] = json!(unreadable);
        }
        if !waiting.is_empty() {
            value["scope"]["pending"] = json!(waiting);
        }
        Ok(value)
    }

    /// One domain's queue for the REST view: the same detection, nothing recorded.
    pub async fn reflect_queue(
        &self,
        domain: &str,
        limit: Option<usize>,
        page: Option<usize>,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        let threads = self.all_threads(&access).await?;
        let items = rank(
            self.detect_reflection(domain, &access, Detail::Full, &threads, &caller.scope)
                .await?,
        );
        let mut value = Self::page_json(&items, &[], limit, page, access.engrams_visible);
        value["domain"] = json!(domain);
        Ok(value)
    }

    /// The review screen's data: open threads split by who they wait on,
    /// grouped by reflection, oldest first, and the queue's first page.
    pub async fn reflection_review(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        let mut people: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut agent: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut titles: HashMap<String, String> = HashMap::new();
        let threads = self.all_threads(&access).await?;
        for (path, thread, text) in &threads {
            if thread.status != ThreadStatus::Open {
                continue;
            }
            titles
                .entry(path.clone())
                .or_insert_with(|| r::title_of(text, path));
            let bucket = match th::waiting_on(thread) {
                Waiting::People => &mut people,
                Waiting::Agent => &mut agent,
            };
            // `thread_view` resolves the anchor by id or by heading.
            bucket
                .entry(path.clone())
                .or_default()
                .push(thread_view(path, thread, text));
        }
        // "Oldest" is the opening time, as in the thread list: threads
        // inside a group and the groups by their first thread.
        let opened = |t: &Value| t["opened_at"].as_str().unwrap_or_default().to_string();
        let groups = |map: BTreeMap<String, Vec<Value>>| -> Vec<Value> {
            let mut out: Vec<(String, Value)> = map
                .into_iter()
                .map(|(path, mut threads)| {
                    threads.sort_by_key(|t| opened(t));
                    let oldest = threads.first().map(opened).unwrap_or_default();
                    let title = titles.get(&path).cloned().unwrap_or_default();
                    (
                        oldest,
                        json!({ "path": path, "title": title, "threads": threads }),
                    )
                })
                .collect();
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out.into_iter().map(|(_, v)| v).collect()
        };
        let items = rank(
            self.detect_reflection(domain, &access, Detail::Full, &threads, &caller.scope)
                .await?,
        );
        let mut queue = Self::page_json(&items, &[], Some(50), Some(1), access.engrams_visible);
        queue["domain"] = json!(domain);
        Ok(json!({
            "domain": domain,
            "waiting_for_people": groups(people),
            "waiting_for_the_agent": groups(agent),
            "queue": queue,
        }))
    }

    /// The Reflection home's summary: what [`Engine::reflection_review`]
    /// answers, cut to what the home draws. The open threads by who they
    /// wait on, the queue's update count (`updates`, the review's
    /// `kinds.update`, counted with no page compared for feedback, no diff
    /// rendered and no source looked up), the distinct pages those updates
    /// are on (`pages_to_update`) and the open
    /// threads by page, each thread with its first quote and the people in
    /// it, never its entries. Pages and their threads come oldest first, by
    /// when a thread was opened. The threads files are read once.
    pub async fn reflection_summary(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        let threads = self.all_threads(&access).await?;
        let (mut for_people, mut for_agent) = (0usize, 0usize);
        // Per page: its title and its open threads with their opening time.
        type Opened<'a> = Vec<(&'a str, Value)>;
        let mut pages: BTreeMap<&str, (String, Opened)> = BTreeMap::new();
        for (path, thread, text) in &threads {
            if thread.status != ThreadStatus::Open {
                continue;
            }
            let waiting = th::waiting_on(thread);
            match waiting {
                Waiting::People => for_people += 1,
                Waiting::Agent => for_agent += 1,
            }
            // Everyone who wrote in the thread, once, in the order they
            // first wrote; an author's last entry says whether it is an agent.
            let mut people: Vec<(&str, bool)> = Vec::new();
            for entry in &thread.entries {
                match people.iter_mut().find(|(n, _)| *n == entry.author.name) {
                    Some(seen) => seen.1 = entry.author.agent,
                    None => people.push((&entry.author.name, entry.author.agent)),
                }
            }
            let view = json!({
                "id": thread.id,
                "quote": thread.anchor.quote,
                "waiting_on": waiting.as_str(),
                "opened_at": thread.opened_at(),
                "people": people
                    .iter()
                    .map(|(name, agent)| json!({ "name": name, "agent": agent }))
                    .collect::<Vec<_>>(),
            });
            pages
                .entry(path.as_str())
                .or_insert_with(|| (r::title_of(text, path), Vec::new()))
                .1
                .push((thread.opened_at(), view));
        }
        let mut open_by_page: Vec<(&str, Value)> = pages
            .into_iter()
            .map(|(path, (title, mut list))| {
                list.sort_by(|a, b| a.0.cmp(b.0));
                let oldest = list.first().map(|(at, _)| *at).unwrap_or_default();
                let threads: Vec<Value> = list.into_iter().map(|(_, v)| v).collect();
                (
                    oldest,
                    json!({ "path": path, "title": title, "threads": threads }),
                )
            })
            .collect();
        open_by_page.sort_by(|a, b| a.0.cmp(b.0));
        let items = self
            .detect_reflection(domain, &access, Detail::Updates, &threads, &caller.scope)
            .await?;
        let shown: Vec<&Item> = items
            .iter()
            .filter(|i| i.kind == "update" && shown_to(i, access.engrams_visible))
            .collect();
        // A page counts once however many of its sections wait; an item of
        // no page (a loose file, unreflected engrams, a host move) is in
        // `updates` only.
        let pages_to_update = shown
            .iter()
            .filter(|i| !i.path.is_empty())
            .map(|i| i.path.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        Ok(json!({
            "domain": domain,
            "waiting_for_the_agent": for_agent,
            "waiting_for_people": for_people,
            "updates": shown.len(),
            "pages_to_update": pages_to_update,
            "open_by_page": open_by_page.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        }))
    }

    /// Every item of `domain`'s reflect queue that a headless run would
    /// work, each with its stable key and its version (R3), for the
    /// automation status: what the caller may see, and no update of a page
    /// a person has open in a live editor. The caller curates the
    /// Reflection; the gate refuses anyone else.
    pub(super) async fn reflect_due_items(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
    ) -> Result<Vec<ReflectDue>> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Curate)
            .await?;
        let threads = self.all_threads(&access).await?;
        let items = self
            .detect_reflection(domain, &access, Detail::Count, &threads, &caller.scope)
            .await?;
        let rooms = self.collab_rooms();
        let mut out = Vec::new();
        for item in items {
            if !shown_to(&item, access.engrams_visible) {
                continue;
            }
            if item.kind == "update"
                && !item.path.is_empty()
                && let Some(rooms) = &rooms
                && rooms
                    .has_live_doc(
                        domain,
                        &crate::collab::session::RoomDoc::Reflection(item.path.clone()),
                        None,
                    )
                    .await
            {
                continue;
            }
            let key = match item.extra.get("thread").and_then(Value::as_str) {
                Some(id) if item.what == "thread" => format!("thread:{id}"),
                _ => {
                    let place = item.section.as_deref().unwrap_or(&item.heading);
                    let detail = ["folder", "asset", "source_domain"]
                        .iter()
                        .find_map(|k| item.extra.get(*k).and_then(Value::as_str))
                        .unwrap_or_default();
                    format!("{}:{}#{place}:{detail}", item.what, item.path)
                }
            };
            out.push(ReflectDue {
                key: crystalline_index::DueKey {
                    kind: item.kind.to_string(),
                    key,
                    version: item.basis,
                },
                page: item.path,
            });
        }
        out.sort_by(|a, b| (&a.key.kind, &a.key.key).cmp(&(&b.key.kind, &b.key.key)));
        out.dedup_by(|a, b| a.key == b.key);
        Ok(out)
    }

    /// Open items per domain the caller curates here, for the nudges (R9):
    /// a domain with no open item is left out, and so is one whose queue
    /// cannot be read (a nudge is a hint, never a failure).
    pub async fn reflect_open_counts(
        &self,
        domains: &[String],
        caller: &ReflectionCaller,
    ) -> Result<Vec<(String, usize)>> {
        let mut out = Vec::new();
        for domain in self.reflect_scope(domains, false).await?.domains {
            let Ok(access) = self
                .require_reflection(&domain, caller, ReflectionRight::Curate)
                .await
            else {
                continue;
            };
            let found = match self.all_threads(&access).await {
                Ok(threads) => {
                    self.detect_reflection(&domain, &access, Detail::Count, &threads, &caller.scope)
                        .await
                }
                Err(e) => Err(e),
            };
            let n = match found {
                Ok(items) => items.len(),
                Err(e) => {
                    tracing::warn!(
                        "reflect: the open items of '{domain}' could not be counted: {e}"
                    );
                    continue;
                }
            };
            if n > 0 {
                out.push((domain, n));
            }
        }
        Ok(out)
    }

    /// The domains whose Reflection `caller` curates here: what every nudge
    /// counts over.
    pub async fn curated_reflections(&self, caller: &ReflectionCaller) -> Result<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        for domain in self.reflect_scope(&[], false).await?.domains {
            if self
                .require_reflection(&domain, caller, ReflectionRight::Curate)
                .await
                .is_ok()
            {
                out.insert(domain);
            }
        }
        Ok(out)
    }

    /// Count every Reflection's open items as the machine owner and record
    /// the counts in `<state_dir>/hooks/maintenance.json`, for the hooks and
    /// for `hook_status`. The daemon's count tick calls this; nothing else
    /// writes `reflect_open`.
    pub async fn record_reflect_counts(&self) -> Result<BTreeMap<String, u64>> {
        // The same pass looks for pages a change of readers left too wide,
        // so they drop off the backup branch without waiting for an edit.
        self.schedule_too_wide_backups().await;
        // The unnamed scope already leaves out a hidden copy and a read-only
        // domain, so nothing is filtered here.
        let counts: BTreeMap<String, u64> = self
            .reflect_open_counts(&[], &ReflectionCaller::owner())
            .await?
            .into_iter()
            .map(|(domain, n)| (domain, n as u64))
            .collect();
        if let Ok(state_dir) = self.journal_state_dir() {
            crate::maintenance::record_reflect_open_under(&state_dir, &counts);
        }
        Ok(counts)
    }
}

/// One domain's too-wide check, with what it reads once: the confirmations
/// on its pages, the name table and each source domain's coverage.
struct TooWide<'a> {
    engine: &'a Engine,
    domain: &'a str,
    disclosures: super::disclosure::Disclosures,
    table: Arc<crystalline_core::names::NameTable>,
    covers: HashMap<String, bool>,
}

impl<'a> TooWide<'a> {
    async fn new(engine: &'a Engine, domain: &'a str, id: DomainId) -> Result<TooWide<'a>> {
        Ok(TooWide {
            engine,
            domain,
            disclosures: engine.reflection_disclosures(id).await?,
            table: engine.name_table_now().await,
            covers: HashMap::new(),
        })
    }

    /// Whether page `path` with `text` has a section citing a domain fewer
    /// people may read than the page's own, or records that cannot be read
    /// in full ([`r::provenance_unreadable`]), which may hide one. A source
    /// whose domain cannot be checked here is not counted, as in the queue.
    async fn text(&mut self, path: &str, text: &str) -> Result<bool> {
        if r::provenance_unreadable(text) {
            return Ok(true);
        }
        let (engine, domain) = (self.engine, self.domain);
        for record in r::read_provenance(text).into_values() {
            for src in &record.sources {
                let Some(name) = src.domain.as_deref() else {
                    continue;
                };
                let Some(local) = engine.checkable_domain(&self.table, name) else {
                    continue;
                };
                if local == domain {
                    continue;
                }
                let covered = match self.covers.get(&local) {
                    Some(covered) => *covered,
                    None => {
                        let covered = engine.readers_cover(domain, &local).await?;
                        self.covers.insert(local.clone(), covered);
                        covered
                    }
                };
                // A person shared it on purpose, for the readers and the
                // repository the page has now (`source_covered`).
                let covered = covered
                    || engine
                        .source_covered_on_page(domain, path, &local, name, &self.disclosures)
                        .await?;
                if !covered {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// The files of a backup's collected `files` (by repo path) inside the
/// Reflection folder of `spec`'s domain, by their path inside it.
fn reflection_files<'a>(
    spec: &'a crystalline_remote::provider::OriginSpec,
    files: &'a BTreeMap<String, Vec<u8>>,
) -> impl Iterator<Item = (&'a str, &'a [u8])> + 'a {
    files.iter().filter_map(move |(repo_path, bytes)| {
        crystalline_remote::reflections_backup::reflection_path(spec, repo_path)
            .map(|inner| (inner, bytes.as_slice()))
    })
}
