//! The backend-agnostic storage interface.
//!
//! [`Store`] is defined at the repository level, not the SQL level, so a second
//! backend (PostgreSQL with tsvector and pgvector) can implement the same trait
//! later without changing the sync engine, the search planner or any caller.
//! The Turso implementation lives in [`crate::turso`].
//!
//! Temporal semantics are open ended: an absent `valid_from` means always valid
//! and an absent `valid_to` means valid forever. The canonical current filter is
//! `status IN ('stable', 'current') AND (valid_from IS NULL OR valid_from <=
//! :today) AND (valid_to IS NULL OR valid_to > :today)`; it maps onto the
//! promoted, indexed `(status, valid_from, valid_to)` columns rather than the
//! metadata JSON. `stable` and `current` are one equivalence class throughout
//! (see [`CURRENT_STATUS_CLASS`]), so a domain written before the vocabulary
//! flip and a foreign OKF bundle both stay first-class.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::NaiveDate;
pub use crystalline_core::config::DomainKind;
use crystalline_core::{Engram, slugify};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::sweep::{SpelledRef, UnresolvedRef};

/// A domain's primary key in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct DomainId(pub i64);

/// An engram's primary key in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct EngramId(pub i64);

/// The recorded file identity used by the sync prefilter: modification time and
/// size are the cheap comparison, the SHA-256 is the authoritative one.
///
/// Serializable because the daemon serves a domain's stamps over the ctl
/// `file_stamps` command, which is how `crystalline doctor` reads the index
/// while the daemon holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    /// Modification time in whole seconds since the Unix epoch.
    pub mtime: i64,
    /// File size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the file contents.
    pub sha256: String,
}

/// One observation bullet, ready to index.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRecord {
    /// One-based source line.
    pub line: usize,
    /// The bracket category.
    pub category: String,
    /// The observation text without trailing tags or context.
    pub content: String,
    /// Trailing hashtags without the leading `#`.
    pub tags: Vec<String>,
    /// A trailing parenthesized context group, if present.
    pub context: Option<String>,
}

/// The SQL expression that resolves one reference row to an engram id, or to
/// NULL when nothing answers to it.
///
/// One rule, shared by both backends and by both reference tables, because a
/// second copy of it would be a second answer to "does this link resolve".
/// `table` is `relation` or `link` (or the alias a statement gave one); every
/// construct here is spelled the same in both dialects, and the one predicate
/// that is not, the actor screen, is handed in ready-made by the backend that
/// owns its spelling.
///
/// The tie-break is `e.id` on both backends, and it is `e.id` for two reasons
/// rather than one. It is collation-free - an integer sorts the same way under
/// every locale, so neither dialect has to spell a collation to agree with the
/// other. And it is free of an index cost that a path tie-break is not: turso
/// can satisfy `ORDER BY e.path` from `idx_engram_path_actor`, so naming the
/// path here talked the planner out of the title index and turned the title
/// arm of every unresolved reference into a scan of the whole domain (measured
/// at 2.4 s for 500 dangling references where the seek takes 25 ms, and the
/// resolve pass runs on every sync of every domain, reviewed or not). Ordering
/// by the primary key costs no sorter at all, on either arm.
///
/// `candidates` says whose rows may answer. [`ReferenceCandidates::Base`]
/// emits the text this expression has always emitted plus the new tie-break,
/// so a base row's references and a direct domain's index pass are otherwise
/// byte for byte what they were; the other two read one actor's view, with
/// that actor's own row preferred at an address the base also answers.
///
/// Five readings. The first three and the last are the order
/// [`crystalline_core::address::resolve`] tries them: the whole target text
/// with its `#heading` (`to_whole`, set only when the parse split a fragment
/// off) as a title in the target domain, so an engram titled `C# Basics` is
/// still found by `[[C# Basics]]`; then the split target as a permalink in the
/// target domain, the target as a title there, and - only when the row names a
/// domain nobody registered - the whole bracket text before the fragment as a
/// permalink and then a title in the row's OWN domain. The fourth is the target
/// as the path
/// of an engram in the target domain: a relative Markdown link
/// (`[p](../pumps/p-12.md)`) is stored as the domain-relative path it lands on
/// ([`crystalline_core::LinkTarget::stored_target`]), and only the path can
/// bind it, since its target's permalink may be anything. No row carries its
/// form, so the arm is open to every row; a valid permalink never ends in
/// `.md` (E005 holds it to a slug, and `slugify` drops the dot), so the first
/// arm never answers a relative row, and a wikilink written with a file path,
/// `[[pumps/p-12.md]]`, binds to that file the same way. The arm seeks
/// `idx_engram_path_actor` on `(domain_id, path)`.
///
/// The target domain of a bare reference is the row's own domain. The target
/// domain of a prefixed one is the domain its prefix spells and nothing else:
/// an explicit `[[domain:Target]]` resolves only within the named domain, as
/// core's rule says. A prefix that spells no domain has no target domain, so
/// the first three readings find nothing and the bare target is never looked up
/// at home; only the whole bracket text is. `[[ops:Runbook]]` written where
/// `ops` is not registered stays unresolved even when the home domain holds a
/// Runbook. It heals on the next resolve pass over home after `ops` is
/// registered; the engine runs one over every domain right after a
/// registration, so the link does not wait for home's next sync.
///
/// "Target domain" and "registered" both mean a row of `domain_spelling`: a
/// reference may name its domain by the local name, the canonical name its
/// MANIFEST declares or a former name, and every one of those that resolves
/// is recorded there against the domain row. Every domain row is at least its
/// own name, so a reference by local name reads what it read when this
/// consulted `domain.name` directly; the lookup is a primary-key seek either
/// way.
///
/// That last reading is what makes an engram titled `Log: Weekly Garden Notes`
/// reachable. The parser splits that into a domain and a target exactly as it
/// splits `ops:Runbook`, because nothing inside the brackets says which it is,
/// and only the spelling table can settle it. It stays a second question rather than
/// a softer answer: a prefix that does name a domain never reaches it, and a
/// row written before `to_raw` existed compares against NULL, which is never
/// true, so it never takes this reading until its engram is reindexed.
pub(crate) fn reference_match(table: &str, candidates: ReferenceCandidates<'_>) -> String {
    // A bare reference reads its own domain; a prefixed one reads only the
    // domain its prefix spells, and a prefix that spells nothing gives NULL,
    // which no engram's `domain_id` equals. There is no fallback to the row's
    // own domain here: that is what the third and fourth arms are for, and
    // they read the whole bracket text, never the bare target.
    let target_domain = format!(
        "CASE WHEN {table}.to_domain IS NULL THEN {table}.domain_id \
         ELSE (SELECT s.domain_id FROM domain_spelling s WHERE s.spelling = {table}.to_domain) END"
    );
    let unregistered = format!(
        "{table}.to_domain IS NOT NULL \
         AND NOT EXISTS (SELECT 1 FROM domain_spelling s WHERE s.spelling = {table}.to_domain)"
    );
    // The tie-break itself: a bare `LIMIT 1` with no secondary sort key leaves
    // a tie between two candidate rows unpinned, and the row it hands back
    // then depends on physical layout rather than on the address. The row with
    // the lower id wins, which is deterministic, spelled the same in both
    // dialects and served from the primary key.
    //
    // `find_engram` pins its own tie to the byte-lower path instead, so the two
    // can name different rows where two engrams in one domain share a title -
    // the oldest of them answers a reference, the byte-first of them answers a
    // lookup. Both are deterministic and locale-free, which is what a tie-break
    // owes; making them the same key would cost the lookup the same index the
    // path ordering costs this one, so they are two answers on purpose.
    let id_order = "e.id";
    // Which rows may answer, and - when more than one may - which of them wins
    // at the same address. `Base` is the literal base predicate and no
    // preference beyond the id tie-break.
    let (actor_screen, prefer) = match candidates {
        ReferenceCandidates::Base => ("e.actor = ''", format!(" ORDER BY {id_order}")),
        ReferenceCandidates::View { screen } | ReferenceCandidates::DraftsOnly { screen } => (
            screen,
            // The author's own row first, said as an ordering rather than as
            // a second COALESCE arm so one arm stays one statement; `0` sorts
            // before `1` in every dialect and needs no collation of its own.
            // The id tie-break settles a further tie within that class.
            format!(" ORDER BY CASE WHEN e.actor = '' THEN 1 ELSE 0 END, {id_order}"),
        ),
    };
    format!(
        "COALESCE(\
         (SELECT e.id FROM engram e WHERE {actor_screen} AND {table}.to_whole IS NOT NULL \
          AND lower(e.title) = lower({table}.to_whole) \
          AND e.domain_id = {target_domain}{prefer} LIMIT 1), \
         (SELECT e.id FROM engram e WHERE {actor_screen} AND e.permalink = {table}.to_target \
          AND e.domain_id = {target_domain}{prefer} LIMIT 1), \
         (SELECT e.id FROM engram e WHERE {actor_screen} AND lower(e.title) = lower({table}.to_target) \
          AND e.domain_id = {target_domain}{prefer} LIMIT 1), \
         (SELECT e.id FROM engram e WHERE {actor_screen} AND e.domain_id = {target_domain} \
          AND e.path = {table}.to_target{prefer} LIMIT 1), \
         (SELECT e.id FROM engram e WHERE {actor_screen} AND {unregistered} AND e.permalink = {table}.to_raw \
          AND e.domain_id = {table}.domain_id{prefer} LIMIT 1), \
         (SELECT e.id FROM engram e WHERE {actor_screen} AND {unregistered} \
          AND lower(e.title) = lower({table}.to_raw) \
          AND e.domain_id = {table}.domain_id{prefer} LIMIT 1))"
    )
}

/// Whose rows may answer a reference, for [`reference_match`].
///
/// Three candidate sets and no fourth. The share-link grants of a later task
/// widen a set by adding one more `OR` to the screen a caller hands in here,
/// never by adding a rule of their own, so this stays the one place that says
/// what a reference may land on.
pub(crate) enum ReferenceCandidates<'a> {
    /// The base rows alone: what the domain's files say, which is what a base
    /// row's own references resolve against and what every reader who names no
    /// actor reads. There is one base and many readers, so a base row's edges
    /// are never resolved in anybody's view.
    Base,
    /// One actor's view - their own live drafts laid over the base - with their
    /// own row preferred wherever both answer at the same address. `screen` is
    /// that actor's composed screen, built by the caller's own backend so the
    /// tombstone column keeps each dialect's spelling; it excludes tombstones,
    /// which is what makes a path its author deleted answer nothing they write.
    View { screen: &'a str },
    /// One actor's live drafts and nothing else: what a base reference that
    /// binds to nothing may still reach at READ time for that one reader,
    /// without the base fallback that is already stored in `to_id`.
    DraftsOnly { screen: &'a str },
}

/// The resolve pass over one reference table: bind every row whose `to_id` is
/// still NULL to the engram its bracket text names.
///
/// One statement. Target domain is the domain `to_domain` spells when set
/// (none when it spells nothing), else the row's own domain. Prefer a
/// permalink match, then a title match, then - only for a prefix that spells
/// no domain - the whole bracket text at home; see [`reference_match`].
///
/// Shared by both backends because the text is the same in both dialects down
/// to the bind placeholder, which is the one argument: `?1` for turso, `$1` for
/// postgres. Built here rather than inline in the four trait methods so the
/// plan guards - `the_reference_resolve_pass_seeks_the_title_index` in
/// `turso/mod.rs` and the registry in `tests/it/plans.rs` - explain the statement
/// the stores actually issue. The pass runs on every sync of every domain and
/// is O(dangling references), so each of its six arms has to be an index seek;
/// a hand-copied literal in a test is what let the title arm lose its index
/// once already.
#[doc(hidden)]
pub fn resolve_pending_sql(table: &str, placeholder: &str) -> String {
    format!(
        "UPDATE {table} SET to_id = {resolved} \
         WHERE {table}.to_id IS NULL AND {table}.domain_id = {placeholder} \
         AND {resolved} IS NOT NULL",
        resolved = reference_match(table, ReferenceCandidates::Base)
    )
}

/// Whether [`resolve_pending_sql`] would bind any row of one reference table
/// in one domain: the same predicate as a read, which stops at the first row.
///
/// Turso asks this before the update. An `UPDATE` of `to_id` (an indexed
/// column) is backed by an ephemeral table in turso_core 0.8.x, which creates
/// a temporary directory on disk as the statement opens, whether or not a row
/// matches; the resolve pass runs on every sync, and almost every time it
/// binds nothing. This read needs no ephemeral table, so a pass with nothing
/// to bind touches neither the database nor the temporary folder.
#[doc(hidden)]
pub fn resolve_pending_probe_sql(table: &str, placeholder: &str) -> String {
    format!(
        "SELECT 1 FROM {table} \
         WHERE {table}.to_id IS NULL AND {table}.domain_id = {placeholder} \
         AND {resolved} IS NOT NULL LIMIT 1",
        resolved = reference_match(table, ReferenceCandidates::Base)
    )
}

/// The unbind a base write runs, one statement per reference table, when the
/// engram it writes carries a title that a `[[C# Basics]]` reference may name
/// whole: every row whose whole text (`to_whole`) is that title, whose target
/// domain is `domain` (the caller's placeholder, the written engram's domain)
/// and which is bound to an engram that does not carry that title. Such a row
/// was bound through its split target (`C`) before the title existed; the
/// resolve passes that follow the write (`resolve_pending_*` in a sync, the
/// engine's `bind_references_to` across domains) bind it again through the
/// whole-text arm of [`reference_match`].
///
/// Seeks the partial `lower(to_whole)` index of the table. A row bound to an
/// engram that carries the title already, in any dimension, is left as it is,
/// so writing the same engram again changes nothing.
///
/// Draft rows are unbound too, on purpose: the base pass after the write binds
/// them again, so a draft's `[[C# Basics]]` follows the new title instead of
/// staying on `C` until its author saves it again
/// (`a_draft_link_follows_a_later_base_hash_title` in the store suite).
#[doc(hidden)]
pub fn unbind_shadowed_by_title_sql(table: &str, domain: &str, title: &str) -> String {
    // -- actor: all - the bound engram already carrying the title settles the
    // row whoever's row it is; this asks nothing else of it.
    format!(
        "UPDATE {table} SET to_id = NULL \
         WHERE {table}.to_whole IS NOT NULL AND lower({table}.to_whole) = lower({title}) \
         AND {table}.to_id IS NOT NULL \
         AND (CASE WHEN {table}.to_domain IS NULL THEN {table}.domain_id \
              ELSE (SELECT s.domain_id FROM domain_spelling s WHERE s.spelling = {table}.to_domain) END) = {domain} \
         AND NOT EXISTS (SELECT 1 FROM engram b WHERE b.id = {table}.to_id \
              AND lower(b.title) = lower({title}))"
    )
}

/// The `$contains` / `$contains_any` predicate over `engram_meta_value`, as
/// postgres issues it: `key` and `values` are the caller's `$n` bind
/// placeholders, and `values` is never empty (an empty filter is answered
/// before this, as a constant false). Several values are one `IN` list, which
/// postgres seeks as `value = ANY(...)` in a single index scan. The engram is
/// matched by `e.id IN (...)`, the form both planners drive from the side
/// table. Built here rather than inline so the plan registry in
/// `tests/it/plans.rs` explains the statement the stores issue.
#[doc(hidden)]
pub fn meta_value_match_sql(key: &str, values: &[String]) -> String {
    let test = match values {
        [one] => format!("m.value = {one}"),
        many => format!("m.value IN ({})", many.join(",")),
    };
    format!(
        "e.id IN (SELECT m.engram_id FROM engram_meta_value m \
         WHERE m.key = {key} AND {test})"
    )
}

/// The same predicate as turso issues it, with `?n` placeholders: one
/// `UNION ALL` arm per value, so each value is its own `(key, value)` seek.
/// Turso seeks an `IN` list on the second index column as a walk of every row
/// for the key, which for a key like `sources` is every anchor in the index.
/// The arms sit in a derived table: a bare compound inside `IN (...)` made
/// turso walk the domain through `idx_engram_domain` instead of seeking `e`
/// by primary key from the list. A scoped search also spells its scope as
/// `+d.name` while this predicate is in it (`turso::domain_scope_sql`), or
/// turso walks the domain from about ten values on. One value gives exactly
/// [`meta_value_match_sql`].
#[doc(hidden)]
pub fn meta_value_union_sql(key: &str, values: &[String]) -> String {
    let arms: Vec<String> = values
        .iter()
        .map(|value| {
            format!(
                "SELECT m.engram_id FROM engram_meta_value m \
                 WHERE m.key = {key} AND m.value = {value}"
            )
        })
        .collect();
    match arms.as_slice() {
        [one] => format!("e.id IN ({one})"),
        many => format!(
            "e.id IN (SELECT engram_id FROM ({}))",
            many.join(" UNION ALL ")
        ),
    }
}

/// The reset behind [`Store::reset_references_to_spellings`], one statement
/// per reference table: unbind the rows that name one of the spellings in
/// `list` (the caller's placeholders) and are bound now.
///
/// A seek through the table's partial `to_domain` index. It runs only when a
/// spelling changed meaning (a canonical name declared or changed, an alias
/// added or dropped, a domain registered under a held spelling), never on a
/// sync or a read.
///
/// The `to_domain IS NOT NULL` term repeats the partial index's own condition:
/// turso uses a partial index only when the query states it, and does not read
/// it off the `IN` list.
#[doc(hidden)]
pub fn reset_spelled_references_sql(table: &str, list: &str) -> String {
    format!(
        "UPDATE {table} SET to_id = NULL WHERE to_domain IS NOT NULL AND to_domain IN ({list}) AND to_id IS NOT NULL"
    )
}

/// The pass behind [`Store::resolve_references_to_spellings`], one statement
/// per reference table: bind the pending rows, in every domain, whose domain
/// prefix is one of the spellings in `list` (the caller's placeholders).
///
/// A seek through the table's partial `to_domain` index, so it reads only the
/// rows that carry one of the prefixes rather than every pending row of every
/// domain. The binding is [`reference_match`] with the base candidates, the
/// rule every resolve pass uses. The `to_domain IS NOT NULL` term repeats the
/// partial index's own condition, as in [`reset_spelled_references_sql`].
#[doc(hidden)]
pub fn resolve_spelled_references_sql(table: &str, list: &str) -> String {
    format!(
        "UPDATE {table} SET to_id = {resolved} \
         WHERE {table}.to_domain IS NOT NULL AND {table}.to_domain IN ({list}) \
         AND {table}.to_id IS NULL AND {resolved} IS NOT NULL",
        resolved = reference_match(table, ReferenceCandidates::Base)
    )
}

/// The rows [`Store::resolve_references_to`] binds, as a `WHERE` clause over one
/// reference table: pending rows in every domain but `domain` (the caller's
/// placeholder), spelled with one of `spellings` and naming one of `targets`
/// by the split target or by the whole text with its `#` (placeholder lists;
/// each target placeholder comes wrapped in `lower()`),
/// that the base candidates now answer. The `to_domain IS NOT NULL` term
/// repeats the partial index's own condition, as in
/// [`reset_spelled_references_sql`].
fn references_to_where(table: &str, domain: &str, spellings: &str, targets: &str) -> String {
    format!(
        "{table}.to_domain IS NOT NULL AND {table}.to_domain IN ({spellings}) \
         AND {table}.to_id IS NULL AND {table}.domain_id <> {domain} \
         AND (lower({table}.to_target) IN ({targets}) OR lower({table}.to_whole) IN ({targets})) \
         AND {resolved} IS NOT NULL",
        resolved = reference_match(table, ReferenceCandidates::Base)
    )
}

/// The count half of [`Store::resolve_references_to`]: how many rows per
/// domain the bind below will take. No `RETURNING`, so both dialects share
/// one shape.
#[doc(hidden)]
pub fn count_references_to_sql(
    table: &str,
    domain: &str,
    spellings: &str,
    targets: &str,
) -> String {
    format!(
        "SELECT {table}.domain_id, count(*) FROM {table} WHERE {} GROUP BY {table}.domain_id",
        references_to_where(table, domain, spellings, targets)
    )
}

/// The bind half of [`Store::resolve_references_to`].
#[doc(hidden)]
pub fn resolve_references_to_sql(
    table: &str,
    domain: &str,
    spellings: &str,
    targets: &str,
) -> String {
    format!(
        "UPDATE {table} SET to_id = {resolved} WHERE {}",
        references_to_where(table, domain, spellings, targets),
        resolved = reference_match(table, ReferenceCandidates::Base)
    )
}

/// The edge half of [`ReadStore::engrams_referencing_domains`]: every base engram
/// with a relation or link row naming one of the spellings in `list`.
///
/// Driven from the reference tables through their partial `to_domain`
/// indexes, so it reads only the rows that carry one of the prefixes, and each
/// source engram is reached by primary key. `UNION` keeps each engram once. No
/// `ORDER BY`: the caller merges this with the URL half and sorts the union
/// byte-wise in Rust, which is the order both backends owe.
///
/// The `to_domain IS NOT NULL` term repeats the partial index's own condition:
/// turso uses a partial index only when the query states it, and does not read
/// it off the `IN` list.
#[doc(hidden)]
pub fn referencing_domains_sql(list: &str) -> String {
    format!(
        "SELECT d.name, e.path FROM relation r JOIN engram e ON e.id=r.engram_id \
         JOIN domain d ON d.id=e.domain_id \
         WHERE r.to_domain IS NOT NULL AND r.to_domain IN ({list}) AND e.actor = '' \
         UNION \
         SELECT d.name, e.path FROM link l JOIN engram e ON e.id=l.engram_id \
         JOIN domain d ON d.id=e.domain_id \
         WHERE l.to_domain IS NOT NULL AND l.to_domain IN ({list}) AND e.actor = ''"
    )
}

/// The read behind [`ReadStore::spelled_references`]: one domain's base
/// references whose domain prefix is one of the spellings in `list`, relation
/// rows then link rows. `domain` is the caller's placeholder for the domain id.
///
/// Reaches each reference table through its partial `to_domain` index, so it
/// reads only the rows that carry one of the prefixes, whichever domain they
/// sit in, and keeps this domain's. The source path is selected to sort by;
/// the caller sorts in Rust.
///
/// The `to_domain IS NOT NULL` term repeats the partial index's own condition:
/// turso uses a partial index only when the query states it, and does not read
/// it off the `IN` list.
#[doc(hidden)]
pub fn spelled_references_sql(domain: &str, list: &str) -> String {
    format!(
        "SELECT r.engram_id, r.line, r.to_domain, r.to_raw, e.path, r.to_alias, r.to_whole \
         FROM relation r JOIN engram e ON e.id=r.engram_id \
         WHERE r.to_domain IS NOT NULL AND r.to_domain IN ({list}) AND r.domain_id={domain} AND e.actor = '' \
         UNION ALL \
         SELECT l.engram_id, l.line, l.to_domain, l.to_raw, e.path, l.to_alias, l.to_whole \
         FROM link l JOIN engram e ON e.id=l.engram_id \
         WHERE l.to_domain IS NOT NULL AND l.to_domain IN ({list}) AND l.domain_id={domain} AND e.actor = ''"
    )
}

/// The checked form of a [`Store::replace_domain_spellings`] list.
pub(crate) struct SpellingPlan {
    /// Each `(spelling, domain)` pair once, sorted by spelling.
    pub(crate) rows: Vec<(String, DomainId)>,
    /// The domain ids the replace is scoped to: every id the list names.
    pub(crate) scope: Vec<DomainId>,
}

/// Check a [`Store::replace_domain_spellings`] list. One spelling listed for
/// two domains is refused, since a spelling resolves to one domain.
pub(crate) fn spelling_plan(spellings: &[(String, DomainId)]) -> Result<SpellingPlan> {
    let mut rows: std::collections::BTreeMap<&str, DomainId> = std::collections::BTreeMap::new();
    for (spelling, id) in spellings {
        match rows.insert(spelling.as_str(), *id) {
            Some(other) if other != *id => {
                return Err(crate::IndexError::Invalid(format!(
                    "the spelling `{spelling}` is listed for two domains ({} and {}); \
                     a spelling resolves to one domain, so build the list from one name table",
                    other.0, id.0
                )));
            }
            _ => {}
        }
    }
    let mut scope: Vec<DomainId> = rows.values().copied().collect();
    scope.sort_by_key(|id| id.0);
    scope.dedup();
    Ok(SpellingPlan {
        rows: rows
            .into_iter()
            .map(|(s, id)| (s.to_string(), id))
            .collect(),
        scope,
    })
}

/// The tag-alias rows [`Store::replace_tag_aliases`] leaves for `pairs`: the
/// first canonical listed for each alias, as the insert that ignores a
/// duplicate alias keeps it. A store compares its rows with this before it
/// writes, so a refresh of an unchanged map writes nothing.
pub(crate) fn wanted_tag_aliases(
    pairs: &[(String, String)],
) -> std::collections::BTreeMap<String, String> {
    let mut wanted = std::collections::BTreeMap::new();
    for (alias, canonical) in pairs {
        wanted
            .entry(alias.clone())
            .or_insert_with(|| canonical.clone());
    }
    wanted
}

/// Every spelling whose mapping differs between two snapshots of the spelling
/// table: added, removed or pointing at another domain. Sorted byte-wise.
pub(crate) fn changed_spellings(
    before: &[(String, DomainId)],
    after: &[(String, DomainId)],
) -> Vec<String> {
    let before: HashMap<&str, DomainId> = before.iter().map(|(s, id)| (s.as_str(), *id)).collect();
    let after: HashMap<&str, DomainId> = after.iter().map(|(s, id)| (s.as_str(), *id)).collect();
    let mut changed: Vec<String> = before
        .iter()
        .filter(|(s, id)| after.get(*s) != Some(*id))
        .map(|(s, _)| s.to_string())
        .chain(
            after
                .keys()
                .filter(|s| !before.contains_key(*s))
                .map(|s| s.to_string()),
        )
        .collect();
    changed.sort();
    changed
}

/// The `crystalline://<spelling>` needles the URL half of
/// [`ReadStore::engrams_referencing_domains`] searches bodies for. A needle also
/// matches a longer name that starts with the spelling, which
/// [`names_a_domain_url`] then rules out.
pub(crate) fn domain_url_needles(spellings: &[String]) -> Vec<String> {
    spellings
        .iter()
        .map(|s| format!("{}{s}", crystalline_core::address::SCHEME))
        .collect()
}

/// Whether `content` names one of `spellings` as a domain in a
/// `crystalline://` URL or a wikilink, read with the scan a rename respells
/// with, so a body counts here exactly when the rename would change it: code
/// spans and fences are skipped, and `eng-other` is not `eng`.
pub(crate) fn names_a_domain_url(content: &str, spellings: &[String]) -> bool {
    let (_, count) = crystalline_core::relink::respell_domains(content, &|name| {
        spellings
            .iter()
            .any(|s| s == name)
            .then(|| format!("{name}-respelled"))
    });
    count > 0
}

/// The refusal of [`Store::rename_domain_row`] when another row already has
/// the new name, naming both.
pub(crate) fn rename_onto_taken_row(old: &str, new: &str) -> crate::IndexError {
    crate::IndexError::Constraint(format!(
        "cannot rename the index row of domain `{old}` to `{new}`: another domain row is \
         already named `{new}`; rename or remove domain `{new}` first, or pick another name"
    ))
}

/// What a store's `guarded_compare` answers when its no-op UPDATE reported no
/// changed row, from the sha the fallback SELECT read: no row is `Ok(false)`,
/// a row that moved on is [`crate::IndexError::StaleEdit`], and a row that
/// still carries the expected sha is `Ok(true)`. That last arm cannot happen
/// while the database counts matched rows, but a release that counted only
/// changed values would report zero changes for every no-op UPDATE, and this
/// keeps such a release from refusing every guarded edit. Both backends call
/// it, so they agree.
pub(crate) fn compare_outcome(stored: Option<String>, expected: &str) -> Result<bool> {
    match stored {
        Some(found) if found == expected => Ok(true),
        Some(found) => Err(crate::IndexError::StaleEdit {
            expected: expected.to_string(),
            found,
        }),
        None => Ok(false),
    }
}

#[cfg(test)]
mod compare_outcome_tests {
    use super::compare_outcome;
    use crate::IndexError;

    // Mutation caught: a matching row read back after an UPDATE that
    // reported zero changes answered as a StaleEdit with two equal shas.
    #[test]
    fn a_guarded_compare_whose_row_still_matches_is_a_match_even_with_zero_changes() {
        assert!(matches!(
            compare_outcome(Some("abc".into()), "abc"),
            Ok(true)
        ));
        assert!(matches!(compare_outcome(None, "abc"), Ok(false)));
        match compare_outcome(Some("def".into()), "abc") {
            Err(IndexError::StaleEdit { expected, found }) => {
                assert_eq!(expected, "abc");
                assert_eq!(found, "def");
            }
            other => panic!("expected a StaleEdit, got {other:?}"),
        }
    }
}

/// How many hashes one `observation_vector` statement names at most. The
/// postgres store cuts every hash list into runs of this many, so a domain of
/// any size costs bounded statements and stays far below the bind limit. The
/// turso store issues one point statement per hash instead, because turso
/// seeks an `IN` list on the key's leading column only.
pub const OBSERVATION_VECTOR_CHUNK: usize = 256;

/// A vector as the raw little-endian f32 bytes both backends store it as.
pub(crate) fn pack_f32_le(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for f in v {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b
}

/// The bytes [`pack_f32_le`] wrote, back as floats. A length that is not a
/// whole number of floats cannot come from that packing and decodes to
/// nothing, so a caller's width check skips it rather than trusting a prefix.
pub(crate) fn unpack_f32_le(bytes: &[u8]) -> Vec<f32> {
    let (quads, partial) = bytes.as_chunks::<4>();
    if !partial.is_empty() {
        return Vec::new();
    }
    quads.iter().copied().map(f32::from_le_bytes).collect()
}

/// Run `body` inside one write transaction on `store`: commit when it
/// succeeds, roll back when it fails. `body` is a future, so nothing in it
/// runs before the transaction is open. The store's own `begin` refuses to
/// nest, so a caller already inside a transaction gets an error rather than a
/// silent early commit of its own work.
pub(crate) async fn in_transaction<S, T, F>(store: &S, body: F) -> Result<T>
where
    S: Store + ?Sized,
    F: std::future::Future<Output = Result<T>>,
{
    store.begin().await?;
    match body.await {
        Ok(value) => {
            store.commit().await?;
            Ok(value)
        }
        Err(e) => {
            let _ = store.rollback().await;
            Err(e)
        }
    }
}

/// One step of the graph traversal over the `relation` table: every edge with
/// an endpoint on the frontier, both endpoints screened.
///
/// Shared by both backends because the template is byte-identical in the two
/// dialects: every dialect-specific piece is inside one of the three fragments
/// the caller builds with its own placeholders and binds through its own
/// parameter list. A second copy of this is a second thing that can lose the
/// `dst` screen, which is why there is only one.
///
/// A draft's relation and link rows are written exactly as a base row's, so a
/// frontier that stops at those tables walks them without ever naming the table
/// that knows whose they are: the traversal would push a row's id into the
/// visited set and the node hydrate - which does carry the screen - would then
/// drop it, leaving an edge with no node and engrams pulled into the
/// neighbourhood through somebody else's private draft. Both endpoints are
/// screened, because an edge reaching INTO a row this reader may not see is as
/// far outside their graph as one leaving it.
///
/// `tgt` carries no screen and wants none: it is the row the reference was
/// bound to, read for its address alone, and the screen that decides what this
/// reader may meet is the one on `dst` beside it.
#[doc(hidden)]
pub fn relation_frontier_sql(
    list: &str,
    src_screen: &str,
    dst_screen: &str,
    rel_pending: &str,
) -> String {
    format!(
        "SELECT r.engram_id, dst.id, r.rel_type FROM relation r \
         JOIN engram src ON src.id=r.engram_id AND {src_screen} \
         JOIN engram tgt ON tgt.id=r.to_id \
         JOIN engram dst ON dst.domain_id=tgt.domain_id AND dst.path=tgt.path \
           AND {dst_screen} \
         WHERE r.to_id IS NOT NULL \
           AND (r.engram_id IN ({list}) OR r.to_id IN ({list})){rel_pending}"
    )
}

/// The prose-link twin of [`relation_frontier_sql`] over the `link` table, and
/// shared, screened and redirected for the same reasons.
#[doc(hidden)]
pub fn link_frontier_sql(
    list: &str,
    src_screen: &str,
    dst_screen: &str,
    link_pending: &str,
) -> String {
    format!(
        "SELECT l.engram_id, dst.id FROM link l \
         JOIN engram src ON src.id=l.engram_id AND {src_screen} \
         JOIN engram tgt ON tgt.id=l.to_id \
         JOIN engram dst ON dst.domain_id=tgt.domain_id AND dst.path=tgt.path \
           AND {dst_screen} \
         WHERE l.to_id IS NOT NULL \
           AND (l.engram_id IN ({list}) OR l.to_id IN ({list})){link_pending}"
    )
}

/// One relation bullet, ready to index. `to_id` is filled by
/// [`Store::resolve_pending_relations`] once the target exists.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationRecord {
    /// One-based source line.
    pub line: usize,
    /// The relation type token or quoted phrase.
    pub rel_type: String,
    /// The link target text (a permalink or a title).
    pub to_target: String,
    /// An explicit cross-domain target domain, or `None` for same-domain.
    pub to_domain: Option<String>,
    /// The bracket text exactly as it was written, colon and all.
    ///
    /// Kept beside the split because the split is domain-agnostic and can be
    /// wrong: `[[Log: Weekly Garden Notes]]` and `[[ops:Runbook]]` are the same
    /// shape, and only the registry tells them apart. Resolution reads this
    /// when the prefix names no registered domain, and it is the only thing
    /// that can - `to_domain` and `to_target` have by then lost the whitespace
    /// the colon was trimmed around.
    pub to_raw: String,
    /// The display text of a `[[target|text]]` reference, `None` without one.
    /// Only read to quote the bracket text as the file holds it.
    pub to_alias: Option<String>,
    /// The target with its `#heading`, as the parse read it before the
    /// fragment split ([`crystalline_core::LinkTarget::whole`]); `None` when
    /// nothing was split off. Resolution tries it as a title first, so a title
    /// that holds a `#` stays reachable, and a quote rebuilds the fragment
    /// from it.
    pub to_whole: Option<String>,
}

/// One prose wikilink, treated as a direct link edge.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRecord {
    /// One-based source line.
    pub line: usize,
    /// The link target text (a permalink or a title).
    pub to_target: String,
    /// An explicit cross-domain target domain, or `None` for same-domain.
    pub to_domain: Option<String>,
    /// The bracket text exactly as it was written, colon and all.
    ///
    /// Kept beside the split because the split is domain-agnostic and can be
    /// wrong: `[[Log: Weekly Garden Notes]]` and `[[ops:Runbook]]` are the same
    /// shape, and only the registry tells them apart. Resolution reads this
    /// when the prefix names no registered domain, and it is the only thing
    /// that can - `to_domain` and `to_target` have by then lost the whitespace
    /// the colon was trimmed around.
    pub to_raw: String,
    /// The display text of a `[[target|text]]` reference, `None` without one.
    /// Only read to quote the bracket text as the file holds it.
    pub to_alias: Option<String>,
    /// The target with its `#heading`, as the parse read it before the
    /// fragment split ([`crystalline_core::LinkTarget::whole`]); `None` when
    /// nothing was split off. Resolution tries it as a title first, so a title
    /// that holds a `#` stays reachable, and a quote rebuilds the fragment
    /// from it.
    pub to_whole: Option<String>,
}

/// A fully prepared engram row plus its child rows and file stamp. Built from a
/// parsed [`Engram`] and the file metadata via [`EngramRecord::from_engram`].
#[derive(Debug, Clone, PartialEq)]
pub struct EngramRecord {
    /// Domain-relative file path, forward-slashed, including the `.md` suffix.
    pub path: String,
    /// Effective permalink: the frontmatter permalink, else `slugify(path)`.
    pub permalink: String,
    /// Title, empty when absent.
    pub title: String,
    /// `type` frontmatter, empty when absent.
    pub engram_type: String,
    /// `status` frontmatter, empty when absent.
    pub status: String,
    /// `recorded_at` as an ISO date string.
    pub recorded_at: Option<String>,
    /// `valid_from` as an ISO date string; absent = always valid.
    pub valid_from: Option<String>,
    /// `valid_to` as an ISO date string; absent = valid forever.
    pub valid_to: Option<String>,
    /// When the engram was last written, as an RFC 3339 string: `generated.at`
    /// with the legacy `timestamp` key as the fallback. The column keeps its
    /// `timestamp` name and wire format, so no migration or resync is needed.
    pub timestamp: Option<String>,
    /// `description`, feeds search snippets.
    pub description: Option<String>,
    /// The raw body text, used for the candidate scan and snippet generation.
    pub content: String,
    /// A JSON object of every filterable non-promoted frontmatter key.
    pub metadata: serde_json::Value,
    /// Normalized tags.
    pub tags: Vec<String>,
    /// Observation bullets.
    pub observations: Vec<ObservationRecord>,
    /// Relation bullets.
    pub relations: Vec<RelationRecord>,
    /// Prose wikilinks.
    pub links: Vec<LinkRecord>,
    /// The file stamp for the sync prefilter.
    pub stamp: FileStamp,
    /// Whose row this is: the empty string for the base row - the one the
    /// domain's files on disk say exists - and an actor key for that actor's
    /// private draft of the same path.
    ///
    /// [`EngramRecord::from_engram`] never sets it, and that is the rule rather
    /// than an omission: a record built by parsing a file describes what is on
    /// disk, which is the base row by definition. An overlay write names its
    /// actor as an argument, so the only records carrying one here are those a
    /// caller built for an actor on purpose.
    pub actor: String,
    /// Whether this row is an actor's draft deletion of the base row at the
    /// same path rather than a draft replacement of it. Never true on a base
    /// row: the base is what the files say, and a file that is gone has no row
    /// at all.
    pub tombstone: bool,
}

fn date_str(d: Option<NaiveDate>) -> Option<String> {
    d.map(|d| d.format("%Y-%m-%d").to_string())
}

impl EngramRecord {
    /// Build a record from a parsed [`Engram`], its domain-relative path and its
    /// file stamp. The effective permalink is the frontmatter permalink, or the
    /// slug of the path minus `.md` when the frontmatter omits it.
    pub fn from_engram(engram: &Engram, path: &str, stamp: FileStamp) -> EngramRecord {
        let fm = &engram.frontmatter;
        let permalink = fm
            .permalink
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| slugify(path));

        // Filterable metadata: every frontmatter key without a column of its
        // own. Core builds it, so verify sees the same keys the index stores.
        let meta = fm.index_metadata();

        let observations = engram
            .observations
            .iter()
            .map(|o| ObservationRecord {
                line: o.line,
                category: o.category.clone(),
                content: o.content.clone(),
                tags: o.tags.clone(),
                context: o.context.clone(),
            })
            .collect();
        let relations = engram
            .relations
            .iter()
            .map(|r| RelationRecord {
                line: r.line,
                rel_type: r.rel_type.clone(),
                to_target: r.target.stored_target(path),
                to_domain: r.target.domain.clone(),
                to_raw: r.target.raw.clone(),
                to_alias: r.target.alias.clone(),
                to_whole: r.target.whole.clone(),
            })
            .collect();
        let links = engram
            .links
            .iter()
            .map(|l| LinkRecord {
                line: l.line,
                to_target: l.target.stored_target(path),
                to_domain: l.target.domain.clone(),
                to_raw: l.target.raw.clone(),
                to_alias: l.target.alias.clone(),
                to_whole: l.target.whole.clone(),
            })
            .collect();

        EngramRecord {
            path: path.to_string(),
            permalink,
            title: fm.title.clone(),
            engram_type: fm.engram_type.clone(),
            status: fm.status.clone().unwrap_or_default(),
            recorded_at: date_str(fm.recorded_at),
            valid_from: date_str(fm.valid_from),
            valid_to: date_str(fm.valid_to),
            timestamp: fm.written_at().map(|t| t.to_rfc3339()),
            description: fm.description.clone(),
            content: engram.body.clone(),
            metadata: serde_json::Value::Object(meta),
            tags: fm.tags.clone(),
            observations,
            relations,
            links,
            stamp,
            actor: String::new(),
            tombstone: false,
        }
    }
}

/// How a search query is matched.
///
/// `Text`, `Title` and `Permalink` are the lexical modes. `Semantic` and
/// `Hybrid` require [`SearchQuery::query_embedding`] and
/// [`SearchQuery::active_model`] to be set by the caller (who owns the embedding
/// provider). `Text` stays the enum default so lexical search never depends on a
/// model; a caller picks `Hybrid` as the interactive default only when the active
/// model has embeddings (see [`StoreInfo`] and the embedding coverage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// Match query terms against title, description and content.
    #[default]
    Text,
    /// Match against the title only.
    Title,
    /// Match against the permalink only.
    Permalink,
    /// Vector similarity search over chunk embeddings.
    Semantic,
    /// Blended lexical and vector search.
    Hybrid,
}

/// How a filter-only listing is ordered.
///
/// A text search is ranked by relevance and never reads this. A listing has
/// no relevance to rank by, so the reader chooses: by the date the knowledge
/// was recorded, newest or oldest first, or by name, which is the path in
/// byte order. An engram carrying no `recorded_at` is last in both directions
/// of the recorded order - a listing must never lead with the one engram
/// nobody dated - and every order breaks its ties on the path, so a page
/// boundary never wobbles between two requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchOrder {
    /// Newest recorded first, undated last, then the path. The default: the
    /// order the domain page opens on.
    #[default]
    RecordedDesc,
    /// Oldest recorded first, undated last, then the path.
    RecordedAsc,
    /// By path, in byte order.
    PathAsc,
    /// By path, reversed.
    PathDesc,
}

/// A comparison operator for a metadata filter.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterOp {
    /// Equals.
    Eq(serde_json::Value),
    /// One of a set.
    In(Vec<serde_json::Value>),
    /// Greater than.
    Gt(serde_json::Value),
    /// Greater than or equal.
    Gte(serde_json::Value),
    /// Less than.
    Lt(serde_json::Value),
    /// Less than or equal.
    Lte(serde_json::Value),
    /// Inclusive range `[lo, hi]`.
    Between(serde_json::Value, serde_json::Value),
    /// The value at the key is a list holding an element equal to this one,
    /// exactly (JSON equality, no case folding). A single value counts as a
    /// one-element list, so on a scalar key this is `$eq`.
    Contains(serde_json::Value),
    /// The list at the key holds any of these elements. Empty matches
    /// nothing.
    ContainsAny(Vec<serde_json::Value>),
}

/// A single metadata filter: a frontmatter key and an operator.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataFilter {
    /// The frontmatter key. Promoted keys map to columns; on the rest,
    /// `$contains` and `$contains_any` read `engram_meta_value` and every other
    /// operator reads the key's value in `metadata`.
    pub key: String,
    /// The comparison to apply.
    pub op: FilterOp,
}

/// Parse the wire form of `metadata_filters` into typed filters.
///
/// The wire form is a JSON object keyed by frontmatter key. Each value is either
/// a bare scalar (shorthand for `$eq`) or an operator object with exactly one of
/// `$eq`, `$in`, `$gt`, `$gte`, `$lt`, `$lte`, `$between`, `$contains` or
/// `$contains_any`. `$in` takes an array and `$between` takes a two-element
/// `[lo, hi]` array. `$contains` takes one value and matches an engram whose
/// list at the key holds an element equal to it, exactly (JSON equality, no
/// case folding): `{"sources": {"$contains": "https://example.com/a"}}`.
/// `$contains_any` takes an array of values and matches when the list holds
/// any of them: `{"sources": {"$contains_any": ["https://example.com/a",
/// "https://example.com/b"]}}`; an empty array matches nothing. For both, a
/// single value counts as a one-element list, and a key that is not plain
/// (ASCII letters, digits, `_` or `-`) is refused. This is the boundary the M5 MCP
/// and CLI layers parse tool arguments through. Models routinely
/// double-encode nested tool arguments, so the whole object arriving as a JSON
/// string is also accepted and parsed first.
pub fn parse_metadata_filters(value: &serde_json::Value) -> Result<Vec<MetadataFilter>> {
    let decoded;
    let value = match value {
        serde_json::Value::String(raw) => {
            decoded = serde_json::from_str::<serde_json::Value>(raw).map_err(|_| {
                crate::IndexError::Invalid("metadata_filters must be an object".into())
            })?;
            &decoded
        }
        other => other,
    };
    let obj = value
        .as_object()
        .ok_or_else(|| crate::IndexError::Invalid("metadata_filters must be an object".into()))?;
    let mut out = Vec::with_capacity(obj.len());
    for (key, spec) in obj {
        let op = match spec {
            serde_json::Value::Object(ops) => {
                if ops.len() != 1 {
                    return Err(crate::IndexError::Invalid(format!(
                        "metadata filter for '{key}' must have exactly one operator"
                    )));
                }
                let (op_name, arg) = ops.iter().next().expect("one entry");
                parse_op(key, op_name, arg)?
            }
            // A bare scalar is shorthand for $eq.
            other => FilterOp::Eq(other.clone()),
        };
        out.push(MetadataFilter {
            key: key.clone(),
            op,
        });
    }
    Ok(out)
}

/// The most distinct values `$contains_any` takes, and `$in` on `tags`.
///
/// Turso builds one `UNION ALL` arm per `$contains_any` value and compiles a
/// compound select by recursing once per arm: a debug build overflowed a
/// 2 MiB thread at 250 arms, and a stack overflow aborts the whole process
/// rather than failing the one search. 100 leaves room for the rest of the
/// statement, and one rule at parse time keeps both backends answering the
/// same input.
pub(crate) const CONTAINS_ANY_MAX_VALUES: usize = 100;

/// The values with duplicates dropped, first occurrence kept, refused when
/// more than [`CONTAINS_ANY_MAX_VALUES`] distinct ones are left.
fn capped_values(
    key: &str,
    op: &str,
    values: &[serde_json::Value],
) -> Result<Vec<serde_json::Value>> {
    let mut seen = std::collections::HashSet::new();
    let distinct: Vec<serde_json::Value> = values
        .iter()
        .filter(|v| seen.insert(v.to_string()))
        .cloned()
        .collect();
    if distinct.len() > CONTAINS_ANY_MAX_VALUES {
        return Err(crate::IndexError::Invalid(format!(
            "{op} on '{key}' takes at most {CONTAINS_ANY_MAX_VALUES} values; \
             split the list over several searches"
        )));
    }
    Ok(distinct)
}

/// `$contains` and `$contains_any` need a key of ASCII letters, digits, `_`
/// or `-`. A key the side table cannot be asked for (a dot, a quote, a letter
/// outside ASCII) is refused by name: dropping the filter, as the older
/// operators do, would widen the search to every engram.
fn refuse_unplain_key(key: &str, op: &str) -> Result<()> {
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Ok(());
    }
    Err(crate::IndexError::Invalid(format!(
        "{op} on '{key}' needs a plain key (letters, digits, '_' or '-')"
    )))
}

fn parse_op(key: &str, op: &str, arg: &serde_json::Value) -> Result<FilterOp> {
    let arr2 = |arg: &serde_json::Value| -> Result<(serde_json::Value, serde_json::Value)> {
        match arg.as_array() {
            Some(a) if a.len() == 2 => Ok((a[0].clone(), a[1].clone())),
            _ => Err(crate::IndexError::Invalid(format!(
                "{op} on '{key}' expects a two-element array"
            ))),
        }
    };
    Ok(match op {
        "$eq" => FilterOp::Eq(arg.clone()),
        "$gt" => FilterOp::Gt(arg.clone()),
        "$gte" => FilterOp::Gte(arg.clone()),
        "$lt" => FilterOp::Lt(arg.clone()),
        "$lte" => FilterOp::Lte(arg.clone()),
        "$in" => {
            let values = arg.as_array().ok_or_else(|| {
                crate::IndexError::Invalid(format!("$in on '{key}' expects an array"))
            })?;
            // `$in` on `tags` builds the same one-`EXISTS` list as
            // `$contains_any` does on any key, so it takes the same cap.
            if key == "tags" {
                FilterOp::In(capped_values(key, op, values)?)
            } else {
                FilterOp::In(values.clone())
            }
        }
        "$between" => {
            let (lo, hi) = arr2(arg)?;
            FilterOp::Between(lo, hi)
        }
        "$contains" => {
            refuse_unplain_key(key, op)?;
            if arg.is_array() || arg.is_object() {
                return Err(crate::IndexError::Invalid(format!(
                    "$contains on '{key}' takes one value; use $contains_any for several"
                )));
            }
            FilterOp::Contains(arg.clone())
        }
        "$contains_any" => {
            refuse_unplain_key(key, op)?;
            let values = arg
                .as_array()
                .filter(|values| values.iter().all(|v| !v.is_array() && !v.is_object()))
                .ok_or_else(|| {
                    crate::IndexError::Invalid(format!(
                        "$contains_any on '{key}' takes an array of single values"
                    ))
                })?;
            FilterOp::ContainsAny(capped_values(key, op, values)?)
        }
        other => {
            return Err(crate::IndexError::Invalid(format!(
                "unknown operator '{other}' on '{key}'"
            )));
        }
    })
}

/// Every `(key, value)` row an engram's `metadata` gives `engram_meta_value`:
/// one per element of a list and one per scalar, for each top-level key.
/// `metadata` holds exactly the frontmatter keys that are not promoted to a
/// column ([`crystalline_core::Frontmatter::index_metadata`]). A key longer
/// than [`crystalline_core::META_KEY_MAX_BYTES`] gives no row, and each value
/// text comes from [`crystalline_core::meta_value_text`], so a list inside a
/// list, an object and a long value give no row either. Sorted and without
/// duplicates, so `[a, a]` is one row and the insert order never depends on
/// the frontmatter's.
pub(crate) fn meta_value_rows(metadata: &serde_json::Value) -> Vec<(String, String)> {
    let Some(map) = metadata.as_object() else {
        return Vec::new();
    };
    let mut rows = std::collections::BTreeSet::new();
    for (key, value) in map {
        if key.len() > crystalline_core::META_KEY_MAX_BYTES {
            continue;
        }
        let elements: &[serde_json::Value] = match value {
            serde_json::Value::Array(items) => items,
            scalar => std::slice::from_ref(scalar),
        };
        for element in elements {
            if let Some(text) = crystalline_core::meta_value_text(element) {
                rows.insert((key.clone(), text));
            }
        }
    }
    rows.into_iter().collect()
}

/// A search request. Filter-only searches (no `text`) are allowed.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// The free-text query, or `None` for a filter-only search.
    pub text: Option<String>,
    /// Restrict to these domain names; `None` searches all domains.
    pub domains: Option<Vec<String>>,
    /// Filter by `type`.
    pub engram_type: Option<String>,
    /// Filter by `status`. `stable` and `current` are one equivalence class in
    /// both directions (see `CURRENT_STATUS_CLASS`); any other value is an
    /// exact match.
    pub status: Option<String>,
    /// Require all of these tags.
    pub tags: Option<Vec<String>>,
    /// Arbitrary metadata filters.
    pub metadata_filters: Vec<MetadataFilter>,
    /// Only engrams with `recorded_at >= after`.
    pub after: Option<String>,
    /// Apply the canonical current filter as of `today` (or the real today when
    /// `today` is `None`). Overrides `status` with the current status class.
    pub current_only: bool,
    /// The date used by `current_only`, ISO `YYYY-MM-DD`.
    pub today: Option<String>,
    /// The match mode.
    pub mode: SearchMode,
    /// How a filter-only search is ordered. Ignored whenever `text` is set:
    /// a ranked search has its own order.
    pub order: SearchOrder,
    /// Minimum cosine similarity for a semantic hit, `None` uses the store
    /// default (`0.78`). Ignored by the text, title and permalink modes.
    pub min_similarity: Option<f32>,
    /// The salience-prior weight for hybrid ranking; `None` uses the store
    /// default (`DEFAULT_SALIENCE_WEIGHT`). The maximum lift a fully-salient
    /// engram receives on the normalized relevance score. A soft additive
    /// prior, never a filter: it can reorder within a relevance band but never
    /// excludes a result.
    pub salience_weight: Option<f64>,
    /// The multiplicative retired-status fade weight, in `[0, 1]`; `None` uses
    /// the store default (`DEFAULT_RETIRED_WEIGHT`). A soft reorder, never a
    /// filter: it scales down a retired engram's final score but never
    /// excludes it from the results.
    pub retired_weight: Option<f64>,
    /// The query text already embedded by the active provider, required by the
    /// semantic and hybrid modes. The caller owns the provider and embeds the
    /// query (with the query instruction prefix) before calling the store, which
    /// keeps the backend free of any model dependency. Its length is the active
    /// dimensionality.
    pub query_embedding: Option<Vec<f32>>,
    /// The active provider's model id, paired with `query_embedding` for the
    /// staleness check. Required by the semantic and hybrid modes.
    pub active_model: Option<String>,
    /// Restrict to engrams filed under this domain-relative folder, given with
    /// its trailing slash (`notes/`). The slash is what makes the match a
    /// folder rather than a string: `notes/deep/y.md` is under `notes/` and
    /// `notes-misc/z.md` is not. `None` or an empty value searches the whole
    /// domain. The value is a literal path, so `%` and `_` in a folder name are
    /// escaped rather than matched as wildcards.
    pub path_prefix: Option<String>,
    /// Whose rows to search: `None` is the base dimension alone - the rows the
    /// domains' files on disk say exist - and `Some(actor)` is that actor's own
    /// drafts folded in over it.
    ///
    /// The fold is a shadowing one, so an engram is one answer however many
    /// rows carry it. A draft at a path no file holds is a hit of its own; a
    /// draft over a base row replaces it; a draft deletion (a tombstone) takes
    /// the base row away and is never a hit itself. Nobody else's drafts are
    /// ever in range: the value is one actor key, and the predicate reads it
    /// literally.
    ///
    /// `None` is what every unauthenticated reader and every caller on a domain
    /// that reviews nothing gets, and the backends emit the base predicate
    /// verbatim for it, so a search that names no actor is the search that was
    /// there before this dimension existed.
    pub actor: Option<String>,
    /// Page size.
    pub limit: usize,
    /// One-based page number.
    pub page: usize,
}

impl SearchQuery {
    /// A text query over all domains with sane paging defaults.
    pub fn text(query: &str) -> SearchQuery {
        SearchQuery {
            text: Some(query.to_string()),
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        }
    }
}

/// Whether a hit is a whole engram or a single observation within one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum HitKind {
    /// The engram itself matched.
    Engram,
    /// An observation matched; carries its source line.
    Observation {
        /// One-based source line of the observation.
        line: usize,
    },
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
    /// The domain the hit lives in.
    pub domain: String,
    /// The engram permalink.
    pub permalink: String,
    /// The engram title.
    pub title: String,
    /// A snippet cut around the match, or a lead-in for filter-only searches.
    pub snippet: String,
    /// The relevance score; higher is more relevant.
    pub score: f64,
    /// The engram `type`.
    pub engram_type: String,
    /// The engram `status`.
    pub status: String,
    /// The engram's tags, alphabetical and folded to lowercase; empty when the
    /// engram is untagged. Every hit passively teaches the querying agent the
    /// existing vocabulary. Observation-kind hits carry their engram's tags.
    pub tags: Vec<String>,
    /// The engram's `recorded_at` as stored, on a filter-only page (the
    /// listing, which sorts by it); `None` on a ranked search and for an
    /// undated engram, and then left out of the answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    /// Whether the hit is the engram or one of its observations.
    #[serde(flatten)]
    pub kind: HitKind,
}

/// A page of results plus the total match count for the same filters.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page<T> {
    /// The results for this page.
    pub items: Vec<T>,
    /// One-based page number.
    pub page: usize,
    /// Page size.
    pub limit: usize,
    /// Total matches across all pages.
    pub total: usize,
}

/// A node in a graph slice.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphNode {
    /// The engram id.
    pub id: EngramId,
    /// The domain the engram lives in.
    pub domain: String,
    /// The engram permalink.
    pub permalink: String,
    /// The engram title.
    pub title: String,
    /// The engram `type`.
    pub engram_type: String,
    /// The raw `salience` frontmatter value, `None` when absent. A non-numeric
    /// value reads as neutral too, though not byte-identically across backends:
    /// Postgres's `jsonb_typeof` guard yields `None`, Turso's `CAST` yields
    /// `Some(0.0)`. Both are zero lift for ranking, so callers must compare
    /// against neutral (`<= 0.0` or absent), never assert an exact `None`.
    pub salience: Option<f64>,
    /// The engram's exact frontmatter status; feeds the retired-status fade in
    /// context ranking.
    pub status: String,
    /// Whose row this is: empty for the base row the domain's files describe,
    /// and the actor's key for one of their own drafts. A reader only ever
    /// meets their own, so a non-empty value means "this is your draft" and is
    /// what the graph verbs mark a node with.
    pub actor: String,
    /// The engram's `recorded_at` as stored (an ISO date), `None` when it has
    /// none. What `build_context`'s `timeframe` compares.
    pub recorded_at: Option<String>,
}

/// Whether an edge came from a relation bullet or a prose wikilink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// A `- rel_type [[Target]]` relation.
    Relation,
    /// A prose `[[Target]]` wikilink.
    Link,
}

/// A directed, typed edge between two engrams. May cross domains.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphEdge {
    /// The source engram id.
    pub from: EngramId,
    /// The target engram id.
    pub to: EngramId,
    /// The relation type; `links_to` for wikilinks.
    pub rel_type: String,
    /// Whether the edge is a relation or a link.
    pub kind: EdgeKind,
}

/// The neighborhood of a set of seed engrams: nodes plus typed edges.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GraphSlice {
    /// Every engram reachable within the requested depth, seeds included.
    pub nodes: Vec<GraphNode>,
    /// Every edge among those nodes, including cross-domain edges.
    pub edges: Vec<GraphEdge>,
}

/// The frontmatter salience scale: a salience of `SALIENCE_SCALE` maps to the
/// full prior; absent or non-positive salience adds nothing.
const SALIENCE_SCALE: f64 = 10.0;
/// The default salience-prior weight when a query does not override it: the
/// maximum lift a fully-salient engram gets on the normalized `[0,1]` relevance
/// score. Small on purpose so relevance dominates and the prior only reorders
/// within a relevance band, never filters.
pub const DEFAULT_SALIENCE_WEIGHT: f64 = 0.15;

/// The bounded, non-negative salience prior added to a normalized relevance
/// score. `salience` is the raw frontmatter value (`None` or non-positive means
/// no lift); `weight` is the maximum lift. The result is never negative and
/// never exceeds `weight`, so it can reorder within a relevance band but can
/// never exclude a result. build_context ranking is the other consumer: it
/// multiplies a neighbor's propagated mass by `1 + prior`, so the same
/// bounded [0, weight] contract holds there too.
pub fn salience_prior(salience: Option<f64>, weight: f64) -> f64 {
    let s = salience.unwrap_or(0.0);
    if s <= 0.0 || weight <= 0.0 {
        return 0.0;
    }
    weight * (s / SALIENCE_SCALE).clamp(0.0, 1.0)
}

/// How many LIKE candidates the lexical prefilter loads before it ranks them in
/// Rust, in both backends. The cut is deterministic: the candidate query orders
/// by engram id, so the same corpus and the same query always yield the same
/// candidate set. A term that matches more engrams than the cap is ranked within
/// the first `LEXICAL_CANDIDATE_CAP` matches by id rather than over the whole
/// match set, and the reported total is capped with it. The value is generous
/// enough that a realistic corpus never reaches it; it exists so a filter-only
/// or very common term over a huge domain cannot pull the whole matched corpus
/// into memory at once.
pub const LEXICAL_CANDIDATE_CAP: usize = 5000;

/// Statuses that mark knowledge as retired. Exact lowercase match, the same
/// stance as the canonical current filter.
pub const RETIRED_STATUSES: [&str; 4] = ["deprecated", "superseded", "archived", "legacy"];

/// Whether a status belongs to the retired set. Exact lowercase match, the
/// counterpart of [`is_current_status`]. Retirement is terminal: the
/// consolidation sweep never flags a retired engram for being old, unverified
/// or unlinked.
pub fn is_retired_status(status: &str) -> bool {
    RETIRED_STATUSES.contains(&status)
}

/// The statuses that mean "this is what holds now", as one equivalence class.
/// `stable` is the canonical word Crystalline writes and the OKF v0.2 spelling;
/// `current` is what it wrote before the flip and is read as the same state
/// forever. Both backends expand the class in the canonical current filter and
/// in an explicit status filter, in both directions: filtering on either word
/// matches engrams carrying either. Every other status stays an exact match.
pub const CURRENT_STATUS_CLASS: [&str; 2] = ["stable", "current"];

/// Whether a status belongs to the current equivalence class. Exact lowercase
/// match, the same stance as the retirement set.
pub fn is_current_status(status: &str) -> bool {
    CURRENT_STATUS_CLASS.contains(&status)
}

/// The default retired-status fade when a query does not override it.
pub const DEFAULT_RETIRED_WEIGHT: f64 = 0.6;

/// The multiplicative ranking factor for an engram's status: statuses in the
/// retired set fade by `weight` (clamped into the unit range) and every other
/// value keeps full rank. Never a filter - it reorders scores and can never
/// exclude a result.
pub fn retired_factor(status: &str, weight: f64) -> f64 {
    if is_retired_status(status) {
        weight.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// A filter for recent activity.
#[derive(Debug, Clone, Default)]
pub struct RecentFilter {
    /// Restrict to these domain names; `None` covers all domains.
    pub domains: Option<Vec<String>>,
    /// Only engrams with `recorded_at >= after`.
    pub after: Option<String>,
    /// Restrict to these `type` values.
    pub engram_types: Option<Vec<String>>,
    /// Maximum rows to return.
    pub limit: usize,
}

/// A compact engram summary for listings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngramSummary {
    /// The domain the engram lives in.
    pub domain: String,
    /// The engram permalink.
    pub permalink: String,
    /// The engram title.
    pub title: String,
    /// The engram `type`.
    pub engram_type: String,
    /// The engram `status`.
    pub status: String,
    /// `recorded_at` as an ISO date string.
    pub recorded_at: Option<String>,
    /// Tags.
    pub tags: Vec<String>,
}

/// A lightweight engram descriptor, enough to locate its file and address it.
///
/// Returned by the [`Store`] lookup methods that back identifier resolution,
/// browsing, validation and schema inference. It carries the ids so a caller can
/// go straight to a graph traversal or a single-file upsert without a second
/// query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EngramDescriptor {
    /// The engram id.
    pub id: EngramId,
    /// The owning domain id.
    pub domain_id: DomainId,
    /// The domain name.
    pub domain: String,
    /// The domain-relative file path, forward-slashed, with the `.md` suffix.
    pub path: String,
    /// The engram permalink.
    pub permalink: String,
    /// The engram title.
    pub title: String,
    /// The engram `type`.
    pub engram_type: String,
    /// The engram `status`.
    pub status: String,
}

/// One level of a domain's folder tree, as [`ReadStore::browse_level`] reports it:
/// the engrams a navigation view draws at this level, the child folders under
/// it, and how many engrams the level really holds.
///
/// `engrams` is capped by the caller's limit while `folders` and `total` are
/// not, which is the whole point of the shape: a tree that truncates its rows
/// must still name every folder a reader can descend into, and must still be
/// able to say how much it is not showing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowseLevel {
    /// The engrams at this level, ordered by path in byte order, at most the
    /// caller's limit of them.
    pub engrams: Vec<EngramDescriptor>,
    /// Every child folder directly under the browsed path, ordered by name in
    /// byte order. Derived from the distinct first path segment below the
    /// prefix rather than from `engrams`, so the cap never hides a folder.
    pub folders: Vec<String>,
    /// How many engrams each of `folders` holds, recursively: every base row
    /// under the folder at any depth, which is the number the paged listing
    /// scoped to that folder reports. Counted in the same grouped statement
    /// that names the folders, so it costs no query of its own.
    pub folder_counts: BTreeMap<String, usize>,
    /// How many engrams the level holds, counted under exactly the filter that
    /// selected `engrams`. Larger than `engrams.len()` means the cap cut the
    /// listing.
    ///
    /// The level, not the folder: the depth cut applies to this count as it
    /// does to the rows, so it moves with `depth` and never counts an engram
    /// nested deeper than the level drawn. A search filtered by the same folder
    /// counts recursively instead and legitimately reports more. See
    /// [`ReadStore::browse_level`] for why the two differ and what it would take to
    /// return both.
    pub total: usize,
}

/// A folder prefix carrying its trailing slash, whatever the caller passed.
///
/// The slash is the whole difference between a folder and a string: without it
/// `notes` also takes `notes-misc/z.md`, and the first-segment derivation would
/// cut a folder name out of a `rel` that opens with a slash and report a folder
/// with no name. An empty prefix stays empty, since the root is the whole domain
/// rather than a folder.
pub(crate) fn folder_slash(prefix: &str) -> String {
    if prefix.is_empty() || prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{prefix}/")
    }
}

/// The half-open byte range `[low, high)` every path under a folder falls in,
/// for [`ReadStore::folder_holds_other`]: `notes/` up to `notes0`, `0` being the
/// character after `/`. Compared bytewise (SQLite's default collation, and
/// `COLLATE "C"` on Postgres), so the match folds no case and reads no
/// wildcard, and a plain range over `path` is one an index can seek
/// (`idx_engram_path_actor` on Turso, `idx_engram_base_path_c` on Postgres).
/// The root is every path.
pub(crate) fn folder_range(prefix: &str) -> (String, String) {
    let prefix = folder_slash(prefix);
    match prefix.strip_suffix('/') {
        Some(folder) => (prefix.clone(), format!("{folder}0")),
        None => (String::new(), char::MAX.to_string()),
    }
}

/// A stored engram's addressing plus its full markdown content and checksum.
/// Returned by [`ReadStore::all_engram_contents`] to materialize a whole domain for
/// `domain export`, and shaped so an export writes each engram back to disk
/// verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredEngram {
    /// The domain-relative file path, forward-slashed, with the `.md` suffix.
    pub path: String,
    /// The engram permalink.
    pub permalink: String,
    /// The full markdown content (frontmatter plus body) as stored.
    pub content: String,
    /// The lowercase hex SHA-256 of `content`, the CAS token.
    pub sha256: String,
    /// Whose row this is: the empty string for the base row, an actor key for
    /// that actor's draft of the same path.
    pub actor: String,
    /// Whether the row is an actor's draft deletion of the base row rather than
    /// a draft replacement of it.
    ///
    /// A tombstone is a full row like any other and nothing empties the columns
    /// beside this flag, so `content` may well hold whatever the caller wrote
    /// there. A reader decides by this flag, never by finding the content
    /// empty.
    pub tombstone: bool,
    /// The row's own engram id, which is what its chunks, observations and
    /// graph edges key to.
    ///
    /// Here because a draft is a full row: a caller serving one has to be able
    /// to ask the index about that row's edges, and for a draft at a path no
    /// base row holds there is no other id to ask with.
    pub id: EngramId,
}

/// One inbound reference to an engram: a relation or a prose link that resolves
/// to it. Used by the move to find the engrams whose references follow the
/// engram to its new address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboundRef {
    /// The linking engram's domain name.
    pub src_domain: String,
    /// The linking engram's domain id.
    pub src_domain_id: DomainId,
    /// The linking engram's domain-relative file path.
    pub src_path: String,
    /// The exact target text used in the link.
    pub to_target: String,
    /// The domain the reference named, when it named one: `Some("open")` for
    /// `[[open:Thing]]`, `None` for a bare `[[Thing]]`.
    ///
    /// Here because `to_target` alone cannot tell the two apart, and a caller
    /// that rewrites the bracket text has to. The cross-domain move rewrites
    /// bare links only: for a prefixed one the bracket text on disk is not
    /// `[[{to_target}]]`, so a needle built from `to_target` either misses it -
    /// or, when the same file also holds a bare link with that exact text,
    /// matches the wrong one and reports the rewrite as a success. A colon
    /// title (`[[Log: Weekly Notes]]`, which resolves by title) reaches that
    /// loop the same way, since parsing splits it at the colon.
    pub to_domain: Option<String>,
    /// Whether the reference came from a relation bullet or a prose link.
    pub kind: EdgeKind,
}

/// One engram whose stored content holds a given text, from
/// [`ReadStore::engrams_mentioning`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContentMention {
    /// The engram's domain name.
    pub domain: String,
    /// The engram's domain id.
    pub domain_id: DomainId,
    /// The engram's domain-relative file path.
    pub path: String,
}

/// The label a prose wikilink carries wherever inbound references are grouped
/// or filtered by relation type.
///
/// The same word the graph edges and the consolidation sweep already use for a
/// wikilink, rather than a second name for one thing: a reader who has seen
/// `links_to` on an edge meets it again on the chip that counts those edges.
pub const LINKS_TO: &str = "links_to";

/// One row of [`ReadStore::inbound_page`]: an engram that points at the target, and
/// the relation it points with.
///
/// Addressed rather than merely located: [`InboundRef`] carries a path because
/// the cross-domain move rewrites files, while a reader needs a link, so this
/// one carries the permalink and title a client renders and navigates with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboundHit {
    /// The referencing engram's domain name.
    pub domain: String,
    /// The referencing engram's permalink.
    pub permalink: String,
    /// Its title, as indexed. Empty when it carries none.
    pub title: String,
    /// Its domain-relative file path.
    pub path: String,
    /// Its `status` frontmatter, free form. Empty when it carries none. Here so
    /// a reader is told that what points at this engram is itself retired,
    /// which is a fact about the reference worth as much as its existence.
    pub status: String,
    /// The relation type of this reference, or [`LINKS_TO`] for a prose
    /// wikilink.
    pub rel: String,
}

/// What [`ReadStore::inbound_page`] takes: which engram is being pointed at, and
/// how the caller wants the references to it narrowed and paged.
///
/// The target is spelled the way [`ReadStore::inbound_refs`] spells it - id, domain
/// and the permalink and title an unresolved reference is text-matched against -
/// because both walk the same rows and must agree about what points where.
#[derive(Debug, Clone, Copy)]
pub struct InboundQuery<'a> {
    /// The engram being pointed at.
    pub engram_id: EngramId,
    /// Its domain, for the unresolved same-domain text match.
    pub domain_id: DomainId,
    /// Its permalink, for that same match.
    pub permalink: &'a str,
    /// Its title, for that same match.
    pub title: &'a str,
    /// Case-insensitive substring the referencing engram's title or path must
    /// contain. `None` or empty selects every reference.
    pub q: Option<&'a str>,
    /// Keep only references carrying this relation type ([`LINKS_TO`] for prose
    /// wikilinks). `None` selects every relation.
    pub rel: Option<&'a str>,
    /// Domain names whose references are left out entirely: the private
    /// domains the caller asking may not see. Empty for a caller who may see
    /// everything, which is the usual case.
    ///
    /// Subtracted inside the query rather than from its answer, and from all
    /// three of the statements it runs - the page, the total and the
    /// per-relation summary. A caller that filtered the returned page would
    /// hand out short pages and a `total` that counts what it did not show,
    /// and a count that disagrees with its rows says a reference exists
    /// somewhere the reader may not look, which is exactly the fact being
    /// kept.
    pub exclude_domains: &'a [String],
    /// One-based page number.
    pub page: usize,
    /// Page size.
    pub limit: usize,
}

/// The `LIMIT` and `OFFSET` a one-based page asks for, as numbers a database
/// integer can actually hold.
///
/// Every clamp happens before the cast, which is the whole point. A page size
/// arriving from an HTTP query is a `usize`, and `usize::MAX as i64` is `-1`:
/// SQLite reads a negative `LIMIT` as no limit at all and hands back the entire
/// set, while Postgres refuses it outright. One request would then get three
/// different wrong answers - every row, the first page under any page number,
/// or a 500 - all from a number a caller typed. Saturating arithmetic and a
/// ceiling of [`i64::MAX`] turn all three into the honest one: a page past the
/// end is empty.
///
/// No policy ceiling here on purpose. How much of an index one request may
/// materialize is the calling surface's decision (the HTTP layer clamps its own
/// page size), and this is only the arithmetic that keeps that decision from
/// being undone by a cast.
pub(crate) fn page_window(page: usize, limit: usize) -> (i64, i64) {
    let ceiling = i64::MAX as usize;
    let limit = limit.max(1).min(ceiling);
    let offset = page.saturating_sub(1).saturating_mul(limit).min(ceiling);
    (limit as i64, offset as i64)
}

/// One page of [`ReadStore::inbound_page`], with the counts a client draws its
/// chips from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InboundPage {
    /// How many references match under the active filters, exactly.
    pub total: usize,
    /// Every relation type pointing at this engram with its count, most-used
    /// first, ties broken by name. Deliberately NOT narrowed by the filters:
    /// this is the map a client filters *with*, so it must not change shape as
    /// the reader clicks through it.
    pub types: Vec<NamedCount>,
    /// This page of matching references.
    pub hits: Vec<InboundHit>,
}

/// One outbound reference from an engram: a relation bullet or a prose wikilink,
/// with whether it currently resolves to a target in the index. Backs the
/// `read_engram` resolution flags so a reading agent learns which of its links
/// land and which dangle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutboundRef {
    /// One-based source line.
    pub line: usize,
    /// Whether the reference came from a relation bullet or a prose link.
    pub kind: EdgeKind,
    /// The relation type for a relation edge; `None` for a prose link.
    pub rel_type: Option<String>,
    /// The exact target text used in the reference.
    pub to_target: String,
    /// An explicit cross-domain target domain, or `None` for same-domain.
    pub to_domain: Option<String>,
    /// The target with its `#heading` as written, `None` when the parse split
    /// no fragment off: what a message quotes the link with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_whole: Option<String>,
    /// Whether the reference currently resolves to a target in the index.
    pub resolved: bool,
}

/// A chunk awaiting an embedding. Populated by M4; the M3 store returns none.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkJob {
    /// The chunk row id.
    pub chunk_id: i64,
    /// The owning engram id for [`ChunkTable::Engram`]. A reflection chunk
    /// has no owning row id (R51), so for [`ChunkTable::Reflection`] this is
    /// the chunk's own id: the keyset cursor `(engram_id, seq)` is then
    /// `(id, seq)`, and since `id` is unique and increasing, paging never
    /// skips or repeats a job.
    pub engram_id: i64,
    /// The chunk sequence within the engram (within the page for a reflection
    /// chunk).
    pub seq: i64,
    /// The chunk text.
    pub text: String,
    /// The chunk fingerprint `sha256(model_id + text)`.
    pub text_hash: String,
}

/// A computed embedding to store back against a chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingRow {
    /// The chunk row id.
    pub chunk_id: i64,
    /// The embedding vector.
    pub embedding: Vec<f32>,
    /// The vector dimensionality.
    pub dims: usize,
}

/// One engram's lead vector: the embedding of its first chunk (`seq = 0`)
/// for one model, as [`ReadStore::lead_vectors`] returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct LeadVector {
    /// The engram the lead chunk belongs to.
    pub engram_id: EngramId,
    /// The vector dimensionality, as the chunk row declares it. Carried so a
    /// caller can skip a pair of mismatched widths rather than compare them.
    pub dims: usize,
    /// The lead chunk's embedding, exactly as the backend stored it.
    pub vector: Vec<f32>,
}

/// One scored engram pair as `contradiction_pair` records it: the two engrams,
/// ordered `a < b` by id, and the content checksums they were scored at. The
/// daemon's pending set is "candidate pairs minus the ones whose row here still
/// carries both current checksums for the model", so this row is what says
/// "this pair is up to date".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoredPair {
    /// The engram with the lower id.
    pub a: EngramId,
    /// The engram with the higher id.
    pub b: EngramId,
    /// `a`'s scoring checksum at scoring time
    /// ([`crate::nli::scoring_checksum`]): what the model read under the line
    /// rules, not the file's checksum, so a frontmatter-only edit keeps the
    /// pair scored and a change of the rules scores it again.
    pub checksum_a: String,
    /// `b`'s scoring checksum at scoring time.
    pub checksum_b: String,
}

/// An exclusive work claim from [`Store::try_work_claim`], held for as long
/// as the guard lives.
///
/// On Postgres it holds the connection whose session took the advisory lock.
/// [`WorkClaim::release`] unlocks it and hands the connection back to the
/// pool; a guard dropped without a release detaches its connection and drops
/// it, which ends the session and so frees the lock. On a store this process
/// owns alone the guard holds nothing.
pub struct WorkClaim {
    #[cfg(feature = "postgres")]
    session: Option<(sqlx::pool::PoolConnection<sqlx::Postgres>, i64)>,
}

impl WorkClaim {
    /// A claim with nobody to exclude, for a store this process owns alone.
    pub(crate) fn unshared() -> WorkClaim {
        WorkClaim {
            #[cfg(feature = "postgres")]
            session: None,
        }
    }

    /// A claim held by the session of `conn` under the advisory lock `id`.
    #[cfg(feature = "postgres")]
    pub(crate) fn session(conn: sqlx::pool::PoolConnection<sqlx::Postgres>, id: i64) -> WorkClaim {
        WorkClaim {
            session: Some((conn, id)),
        }
    }

    /// Let go of the claim now. A failed unlock closes the session instead,
    /// so a connection that might still hold the lock never goes back to the
    /// pool.
    #[cfg_attr(not(feature = "postgres"), allow(unused_mut))]
    pub async fn release(mut self) {
        #[cfg(feature = "postgres")]
        if let Some((conn, id)) = self.session.take() {
            crate::postgres::release_work_claim(conn, id).await;
        }
    }
}

impl Drop for WorkClaim {
    fn drop(&mut self) {
        #[cfg(feature = "postgres")]
        if let Some((conn, _)) = self.session.take() {
            drop(conn.detach());
        }
    }
}

impl std::fmt::Debug for WorkClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkClaim").finish_non_exhaustive()
    }
}

/// One scored observation-line pair as `contradiction` records it.
///
/// `line_*` are the one-based lines `read_engram` reports; `hash_*` are the
/// sha256 of the observation text with its category and tags stripped and its
/// whitespace folded, so a row survives a renumbering and dies with an edit of
/// the line. Both reading orders are kept, so the order aggregation can change
/// without a rescore.
#[derive(Debug, Clone, PartialEq)]
pub struct ContradictionRow {
    /// The engram with the lower id.
    pub a: EngramId,
    /// The engram with the higher id.
    pub b: EngramId,
    /// `a`'s observation line at scoring time.
    pub line_a: usize,
    /// `b`'s observation line at scoring time.
    pub line_b: usize,
    /// The hash of `a`'s observation text.
    pub hash_a: String,
    /// The hash of `b`'s observation text.
    pub hash_b: String,
    /// The contradiction probability with `a` as premise and `b` as hypothesis.
    pub score_ab: f32,
    /// The same with the order reversed.
    pub score_ba: f32,
    /// The cosine of the two lines' vectors under the embedding model the
    /// pair was scored with: how much the two lines are about the same
    /// subject, which is what let them reach the model at all.
    pub similarity: f32,
    /// Whether either line names a period (a year, a date, a month, since or
    /// until): the finding then points at the validity window.
    pub period: bool,
}

/// One observation line's embedding as `observation_vector` keeps it: the
/// row hash of its folded text ([`crate::nli::observation_hash`]) and the
/// vector the embedding model gave it. Never the text (lessons 5 and 6).
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationVector {
    /// [`crate::nli::observation_hash`] of the line.
    pub hash: String,
    /// The embedding, unit-normalized as every provider returns it.
    pub vector: Vec<f32>,
}

/// A freshly computed chunk to store against an engram. Produced by the chunker
/// and handed to [`Store::replace_chunks`], which reconciles it against the
/// engram's existing chunk rows and carries over any matching embedding.
#[derive(Debug, Clone, PartialEq)]
pub struct NewChunk {
    /// The chunk sequence within the engram, zero-based.
    pub seq: i64,
    /// The chunk text.
    pub text: String,
    /// The chunk fingerprint `sha256(model_id + ":" + text)`.
    pub text_hash: String,
}

/// Embedded-chunk counts for one `(model, dims)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChunkModelCount {
    /// The embedding model that produced these chunks.
    pub model: String,
    /// The vector dimensionality.
    pub dims: usize,
    /// The number of embedded chunks with this model and dimensionality.
    pub count: usize,
}

/// Embedding coverage across the whole index: how many chunks exist and how many
/// are embedded, broken down by model. Callers derive per-model coverage,
/// staleness and the interactive default search mode from this.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct EmbeddingCoverage {
    /// Total chunk rows in the index.
    pub total_chunks: usize,
    /// Chunk rows that carry an embedding, across all models.
    pub embedded_chunks: usize,
    /// Embedded-chunk counts per `(model, dims)`, most chunks first.
    pub models: Vec<ChunkModelCount>,
}

/// One group of the coverage aggregate: the chunk rows that share a model and
/// a width, with how many of them carry a vector. A chunk not embedded yet has
/// no model and no width, so it lands in the one group whose `embedded` is 0.
/// Each backend maps its rows into these and [`EmbeddingCoverage::from_groups`]
/// is the one fold, for every table and for any set of domains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageGroup {
    /// The embedding model, empty for chunks without a vector.
    pub model: String,
    /// The vector width, 0 for chunks without a vector.
    pub dims: usize,
    /// Chunk rows in the group.
    pub chunks: usize,
    /// Chunk rows in the group that carry an embedding.
    pub embedded: usize,
}

impl EmbeddingCoverage {
    /// Fold coverage groups into one snapshot. Every group counts toward the
    /// totals; only a group with vectors and a model becomes a `models` entry,
    /// and groups that share a model and width (the same model in two domains)
    /// merge into one. Entries run most chunks first, then by model and width,
    /// so the order never depends on how the database returned the rows.
    pub fn from_groups(groups: impl IntoIterator<Item = CoverageGroup>) -> Self {
        let mut total_chunks = 0usize;
        let mut embedded_chunks = 0usize;
        let mut by_model: std::collections::BTreeMap<(String, usize), usize> =
            std::collections::BTreeMap::new();
        for g in groups {
            total_chunks += g.chunks;
            embedded_chunks += g.embedded;
            if g.embedded == 0 || g.model.is_empty() {
                continue;
            }
            *by_model.entry((g.model, g.dims)).or_default() += g.embedded;
        }
        let mut models: Vec<ChunkModelCount> = by_model
            .into_iter()
            .map(|((model, dims), count)| ChunkModelCount { model, dims, count })
            .collect();
        models.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.model.cmp(&b.model))
                .then(a.dims.cmp(&b.dims))
        });
        Self {
            total_chunks,
            embedded_chunks,
            models,
        }
    }

    /// Embedded chunks produced by the given model.
    pub fn embedded_for(&self, model: &str) -> usize {
        self.models
            .iter()
            .filter(|m| m.model == model)
            .map(|m| m.count)
            .sum()
    }

    /// Whether the given model has at least one embedded chunk, so a caller may
    /// default interactive search to the hybrid mode.
    pub fn has_active_embeddings(&self, model: &str) -> bool {
        self.embedded_for(model) > 0
    }

    /// Chunks still awaiting embedding by the given model: the total minus what
    /// that model has already embedded.
    pub fn backlog_for(&self, model: &str) -> usize {
        self.total_chunks.saturating_sub(self.embedded_for(model))
    }
}

/// Which table a chunk and its vector live in. Each table keeps its own rows;
/// one body of code chunks, embeds, prunes and counts them all, the table
/// chosen here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkTable {
    /// `chunk`: the engram chunks.
    Engram,
    /// `reflection_chunk`: the Reflection's section chunks, never read by
    /// engram search.
    Reflection,
}

impl ChunkTable {
    /// Every table, in the order the embed passes walk them: engrams first.
    pub const ALL: &'static [ChunkTable] = &[ChunkTable::Engram, ChunkTable::Reflection];
}

/// A freshly computed chunk of one reflection page, handed to
/// [`Store::replace_reflection_chunks`]. Produced by
/// [`crate::embed::chunk_reflection`].
#[derive(Debug, Clone, PartialEq)]
pub struct NewReflectionChunk {
    /// The section's place on the page: 0 is the text before the first `##`.
    pub section_ord: i64,
    /// The section id, `_top` for the top, `None` for a heading with no id yet.
    pub section: Option<String>,
    /// The section heading, empty for the top.
    pub heading: String,
    /// The chunk itself; its `seq` counts through the whole page.
    pub chunk: NewChunk,
}

/// The closest chunk of one section, as cosine similarity: one row of
/// [`ReadStore::reflection_vector_hits`], keyed `(path, section_ord)` the way
/// [`crate::reflection::rank_sections`] merges the two halves (R60).
#[derive(Debug, Clone, PartialEq)]
pub struct SectionVector {
    /// The page's path inside `reflections/`.
    pub path: String,
    /// The section's place on the page: 0 is the text before the first `##`.
    pub section_ord: usize,
    /// `1 - cosine distance` of the section's closest chunk to the query.
    pub similarity: f64,
}

/// Which full-text path the store is using.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FtsMode {
    /// A native FTS5 virtual table is active.
    Native,
    /// The LIKE-candidate scan fallback is active.
    CandidateScan,
}

/// Diagnostics about the open store.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoreInfo {
    /// Which full-text path is active.
    pub fts_mode: FtsMode,
    /// The applied schema version.
    pub schema_version: i64,
    /// The database file path, or `None` for an in-memory store.
    pub db_path: Option<String>,
    /// The database file size in bytes, when known.
    pub db_size: Option<u64>,
}

/// Per-domain counts for `status` and `domain list`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DomainStats {
    /// The domain name.
    pub name: String,
    /// The domain root path as registered.
    pub path: String,
    /// The domain kind, `file` (default) or `virtual`.
    pub kind: DomainKind,
    /// Number of engrams.
    pub engrams: i64,
    /// Number of observations.
    pub observations: i64,
    /// Number of relations.
    pub relations: i64,
    /// Number of relations still awaiting their target (forward refs).
    pub unresolved_relations: i64,
    /// Number of prose wikilinks.
    pub links: i64,
    /// Number of prose wikilinks still awaiting their target (forward refs).
    pub unresolved_links: i64,
    /// Last successful sync, RFC 3339, or `None` if never synced.
    pub last_sync: Option<String>,
    /// The instance id currently hosting this file domain in a shared database,
    /// or `None` when unhosted (every domain in a single-instance deployment, and
    /// every virtual domain, which never take a host lock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_instance_id: Option<String>,
    /// The host's human label, when hosted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_label: Option<String>,
    /// The host's last heartbeat, RFC 3339, when hosted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_heartbeat_at: Option<String>,
    /// When this domain was last seen registered in the configuration, RFC
    /// 3339, or `None` when it has never been stamped. Written by
    /// [`Store::stamp_registered`].
    ///
    /// `None` means never stamped. It is emphatically not an old timestamp: a
    /// caller comparing ages treats `None` as no evidence of staleness, never
    /// as infinitely old. Every domain row written before the column existed
    /// reads `None`, so a fresh upgrade must find nothing collectable on its
    /// first sweep.
    pub last_registered: Option<String>,
    /// When a forced rebuild of this domain was stamped as started, RFC 3339,
    /// or `None` when no rebuild is in flight. Written by
    /// [`Store::begin_rebuild`] and cleared by [`Store::end_rebuild`] inside
    /// the transaction that commits the rebuild.
    ///
    /// A set value reads as history, not as liveness: nothing clears it when
    /// the process that stamped it is killed, which is exactly what makes it
    /// useful. A reader that also sees a live `reindex` activity may say a
    /// rebuild is running, though not that it is this domain's (the activity
    /// record carries no domain); one that does not must say a rebuild did not
    /// finish and that this domain's rows are the ones from before it - they
    /// are complete rows either way, because a rebuild never clears anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild_started: Option<String>,
    /// Which rebuild verb stamped [`DomainStats::rebuild_started`] - `full` or
    /// `wipe` - or `None` when no rebuild is in flight (and for a marker a
    /// binary older than the column stamped).
    ///
    /// The two verbs are opposites and a reader must not describe one as the
    /// other: a forced rebuild destroys nothing, so the domain's rows are the
    /// complete ones from before it; a wipe destroyed every row and every
    /// embedding before it started, so what is there is only what its rebuild
    /// managed before it stopped. `None` is read as neither: say that a rebuild
    /// did not finish and name the command that finishes it, and claim nothing
    /// about what is in the rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild_kind: Option<String>,
    /// The rows a walk kept after a host move on a shared database because
    /// this instance's disk does not have them, while that is unresolved
    /// and the count is not zero ([`HostMove`]). `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_elsewhere: Option<OnlyElsewhere>,
}

/// Which rebuild verb is stamping a domain's marker: `reindex --full`, which
/// destroys nothing, or `reindex --wipe`, which destroyed every row and every
/// embedding before it began. Carried into [`Store::begin_rebuild`] so an
/// interrupted run can be described in the words of the verb that ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildKind {
    /// `reindex --full`: every file re-read, nothing destroyed.
    Full,
    /// `reindex --wipe`: the index emptied first, embeddings and all.
    Wipe,
}

impl RebuildKind {
    /// The word stored in the marker column and printed to a person.
    pub fn as_str(self) -> &'static str {
        match self {
            RebuildKind::Full => "full",
            RebuildKind::Wipe => "wipe",
        }
    }
}

/// The instance currently holding a file domain's host lock in a shared
/// database. Returned by [`ReadStore::domain_host`] and carried by
/// [`HostClaim::HeldByOther`] so a refused sync can name the host and its last
/// heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DomainHost {
    /// The holding instance's stable id.
    pub instance_id: String,
    /// The holding instance's human label.
    pub label: String,
    /// The host's last heartbeat, RFC 3339.
    pub heartbeat_at: String,
    /// The token of the claim that wrote the row (its `acquired_at`), which
    /// a release compares before it deletes. Never serialized: it is the
    /// holder's to keep, not something an answer shows.
    #[serde(skip)]
    pub(crate) token: String,
}

/// The outcome of a [`Store::claim_domain_host`]: this instance acquired (or
/// renewed, or took over) the host lock, or another live instance still holds
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum HostClaim {
    /// This instance holds the host lock now, under `token`: the claim's own
    /// `acquired_at`, new on every claim, a re-claim by the same instance
    /// id included. Only a release that names this token deletes the row
    /// ([`Store::release_domain_host`]), so a process whose claim another
    /// claim under the same id replaced cannot release the newer one.
    Acquired {
        /// The claim's token. Never serialized: it is the holder's to keep.
        #[serde(skip)]
        token: String,
    },
    /// Another instance holds it and the claim did not qualify for takeover
    /// (not stale, no `take_over`).
    HeldByOther(DomainHost),
}

impl HostClaim {
    /// Whether this instance holds the lock after the claim.
    pub fn is_acquired(&self) -> bool {
        matches!(self, HostClaim::Acquired { .. })
    }
}

/// A new claim token: the claim's time, so the column still reads as when
/// the lock was acquired, then a random part, so two claims in the same
/// instant (two processes of one machine) never share a token.
pub(crate) fn claim_token(now: &str) -> String {
    format!("{now} {}", uuid::Uuid::new_v4().simple())
}

/// One unshared file of a domain when an automation lease was taken, with
/// its checksum, so the run shares only the files that are new or changed
/// since then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PathChecksum {
    /// The domain-relative path.
    pub path: String,
    /// The file's checksum at take time.
    pub checksum: String,
}

/// One item a loop had due: its kind, a stable key and a version that moves
/// only when the item really changes. Stored at take time and, for a run that
/// ended `done`, as the items left for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct DueKey {
    /// The item kind, for example a reflect queue kind or `finding`.
    pub kind: String,
    /// The item's key within its kind.
    pub key: String,
    /// The item's version.
    pub version: String,
}

/// Who holds an automation lease, and what the domain looked like when it
/// was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct LeaseHolder {
    /// The holding instance's stable id.
    pub instance_id: String,
    /// The account the runner works as, when it has one.
    pub account: Option<String>,
    /// The runner label: `cli`, `n8n`, or what `--runner` names.
    pub runner: String,
    /// The domain's unshared files with their checksums at take time. Never
    /// serialized, like [`AutomationLease::token`]: page paths stay out of
    /// any answer a lease reaches.
    #[serde(skip)]
    pub unshared_at_start: Vec<PathChecksum>,
    /// The loop's due items at take time.
    pub due_at_start: Vec<DueKey>,
}

/// The lease of one loop on one domain, as stored. A stale lease (not
/// renewed in time) is still returned by [`ReadStore::automation_lease`]; the
/// caller decides what stale means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AutomationLease {
    /// Who holds it.
    pub holder: LeaseHolder,
    /// The lease token, new on every take: renew and release name it. Never
    /// serialized: it is the holder's to keep, so a refused take or a status
    /// answer never hands it to someone else.
    #[serde(skip)]
    pub token: String,
    /// When it was taken, RFC 3339.
    pub taken_at: String,
    /// When it was last renewed, RFC 3339.
    pub renewed_at: String,
    /// The pull request the run shared its changes in, once it shared.
    pub pr_url: Option<String>,
    /// The paths the holder's account wrote over MCP while it held the
    /// lease, each once ([`Store::note_automation_written`]). Never
    /// serialized, like `token`.
    #[serde(skip)]
    pub written: Vec<String>,
}

/// The outcome of a [`Store::take_automation_lease`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum LeaseClaim {
    /// The lease is this caller's now, under `token`.
    Acquired {
        /// The take's token, which renew and release name. Serialized, unlike
        /// [`AutomationLease::token`]: it goes back only to the caller whose
        /// take landed, which may be a runner on the far side of a route.
        token: String,
        /// The stale lease this take replaced, as it was, `None` when the
        /// lease was unheld. Read in the same claim, so two takes never
        /// both report one stale lease.
        replaced: Option<AutomationLease>,
    },
    /// Another runner holds a lease that is not stale.
    Held(AutomationLease),
}

/// One recorded run of a loop on a domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AutomationRun {
    /// When the run started, RFC 3339.
    pub started_at: String,
    /// When the run ended, RFC 3339.
    pub ended_at: String,
    /// The runner label.
    pub runner: String,
    /// The account the run worked as, when it had one.
    pub account: Option<String>,
    /// `done`, `stopped_at_cap`, `failed`, `nothing_due` or `expired`.
    pub outcome: String,
    /// Items done.
    pub done: u64,
    /// Items left for a person.
    pub left: u64,
    /// Judgment findings waiting (evolve).
    pub judgment_waiting: u64,
    /// A short summary text.
    pub summary: String,
    /// The pull request the run shared its changes in, when it shared.
    pub pr_url: Option<String>,
    /// The items left for a person, recorded only for a `done` run.
    pub left_items: Vec<DueKey>,
}

/// The rows of a file domain that the last complete walk did not see on
/// the disk of the instance that hosts it now, kept because the domain was
/// taken over on a shared database and the previous host's disk may have
/// them (see [`HostMove`]). Counts only, never content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct OnlyElsewhere {
    /// The label of the instance that held the domain before, when known.
    pub previous_host: Option<String>,
    /// Engram rows.
    pub engrams: u64,
    /// Engram attachment rows (files under `assets/`).
    pub attachments: u64,
    /// Reflection page rows, the pact included.
    pub pages: u64,
    /// Reflection file rows (files under `reflections/assets/`).
    pub files: u64,
}

impl OnlyElsewhere {
    /// Every row counted, of every kind.
    pub fn total(&self) -> u64 {
        self.engrams + self.attachments + self.pages + self.files
    }

    /// The one line a person reads for `domain` on the instance that hosts
    /// it, in the sync report and in `crystalline status`, with the two ways
    /// to end it.
    pub fn line(&self, domain: &str) -> String {
        format!(
            "{}; copy the domain folder over or restore it, then sync, or drop them with: crystalline sync --domain {domain} --accept-disk",
            self.what()
        )
    }

    /// The line on an instance that does not host the domain: the rows are
    /// the host's to resolve, so it names the host instead of a remedy that
    /// would be refused here.
    pub fn line_elsewhere(&self, host: &str) -> String {
        format!(
            "{}; {host} hosts this domain, so resolve it there",
            self.what()
        )
    }

    /// The line on an instance that does not host the domain while the host
    /// sends no heartbeat: the lock is stale, so this instance can take the
    /// domain over and resolve it here.
    pub fn line_stale_host(&self, host: &str, domain: &str) -> String {
        format!(
            "{}; {host} hosts this domain but sends no heartbeat, so take it over with: crystalline sync --domain {domain} --take-over",
            self.what()
        )
    }

    /// "1 engram and 2 Reflection files exist only on alpha".
    fn what(&self) -> String {
        let count =
            |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let engrams = if self.attachments > 0 {
            format!(
                "{}, {}",
                count(self.engrams, "engram", "engrams"),
                count(self.attachments, "attachment", "attachments")
            )
        } else {
            count(self.engrams, "engram", "engrams")
        };
        let reflection = count(
            self.pages + self.files,
            "Reflection file",
            "Reflection files",
        );
        let host = self.previous_host.as_deref().unwrap_or("another instance");
        format!("{engrams} and {reflection} exist only on {host}")
    }
}

/// The stored text of a [`HostMove`], the JSON both backends keep in
/// `domain.host_move`.
pub(crate) fn host_move_text(host_move: &HostMove) -> String {
    serde_json::to_string(host_move).unwrap_or_default()
}

/// A [`HostMove`] read back from `domain.host_move`. Text that does not
/// read as one (a newer binary's shape) counts as a move with nothing
/// counted yet, so a walk keeps rows rather than dropping them.
pub(crate) fn parse_host_move(text: &str) -> Option<HostMove> {
    Some(serde_json::from_str(text).unwrap_or_else(|_| HostMove {
        at: String::new(),
        claimant: None,
        walked: false,
        only_elsewhere: OnlyElsewhere::default(),
    }))
}

/// What a domain's stats report from its stored host move: the counts once
/// a walk found rows missing, else nothing.
pub(crate) fn reported_elsewhere(text: Option<&str>) -> Option<OnlyElsewhere> {
    let host_move = parse_host_move(text?)?;
    (host_move.walked && host_move.only_elsewhere.total() > 0).then_some(host_move.only_elsewhere)
}

/// An unresolved host move of a file domain on a shared database: this
/// instance took the domain's host lock from another instance, or from no
/// known one, and its disk may lack files the previous host wrote. While a
/// domain carries one, a complete walk keeps every row it does not see and
/// counts them into [`HostMove::only_elsewhere`]; a complete walk that sees
/// every row, or one run with `--accept-disk`, clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct HostMove {
    /// When the move was marked, RFC 3339.
    pub at: String,
    /// The instance whose claim marked the move. Only a walk by that
    /// instance ends it; `None` in a mark from before the field.
    #[serde(default)]
    pub claimant: Option<String>,
    /// Whether a complete walk has counted the rows since.
    pub walked: bool,
    /// What that walk did not see.
    pub only_elsewhere: OnlyElsewhere,
}

/// One binary attachment's metadata: an asset stored under a domain's `assets/`
/// folder, addressed by its domain-relative path.
///
/// Where the bytes themselves live depends on the domain kind, and the row is
/// the same either way: a file domain keeps them on disk under the domain root,
/// a virtual domain has no filesystem and keeps them in the index alongside the
/// row (see [`Store::write_attachment_blob`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentRow {
    /// The domain-relative path, always under the `assets/` prefix.
    pub path: String,
    /// Lowercase hex SHA-256 of the attachment bytes.
    pub sha256: String,
    /// The MIME type resolved from the extension.
    pub mime: String,
    /// Byte length.
    pub size: u64,
    /// Last modification instant, RFC 3339.
    pub modified: String,
}

/// One reflection as the index keeps it: the text Reflection-mode search
/// reads and the title the tree shows. Never an engram row: reflections
/// live in their own table and never reach search_engrams, evolve or the
/// graph. `path` is inside `reflections/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReflectionRow {
    /// The path inside `reflections/`.
    pub path: String,
    /// The reflection's title.
    pub title: String,
    /// The whole file.
    pub content: String,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
    /// Byte length.
    pub size: u64,
    /// Last modification instant, RFC 3339.
    pub modified: String,
}

/// One engram a Reflection section cites, as [`ReadStore::cited_engrams`]
/// answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitedEngram {
    /// The permalink the source names.
    pub permalink: String,
    /// The engram's title.
    pub title: String,
    /// The engram's status, as written.
    pub status: String,
    /// Lowercase hex SHA-256 of the file, the checksum a source records.
    pub sha256: String,
    /// Seconds since the epoch of the file's last change.
    pub mtime: i64,
}

/// A [`ReflectionRow`] without its `content`: [`ReadStore::list_reflection_stamps`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectionStamp {
    /// The path inside `reflections/`.
    pub path: String,
    /// The reflection's title.
    pub title: String,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
    /// Byte length.
    pub size: u64,
    /// Last modification instant, RFC 3339.
    pub modified: String,
}

/// Which table a domain's files are kept in. Each table keeps its own rows;
/// one body of code reads and writes them all, the table chosen here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetTable {
    /// `attachment`: the engram attachments under `assets/`.
    Engram,
    /// `reflection_asset`: a Reflection's files under `reflections/assets/`,
    /// keyed by their path inside `reflections/` (`assets/...`). No engram
    /// path reads this table, and it has no blobs: a Reflection always lives
    /// in a file domain.
    Reflection,
}

/// One tag in use, with how many engrams and how many observations carry it.
/// The two counts are separate scans so a tag applied on the frontmatter and one
/// applied on an observation are both visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagCount {
    /// The tag name, without the leading `#`.
    pub name: String,
    /// Number of engrams tagged with it on their frontmatter.
    pub engrams: i64,
    /// Number of observations tagged with it.
    pub observations: i64,
}

/// One folded tag-alias mapping in effect: an `alias` that folds onto a
/// `canonical` at query time. Derived purely from MANIFEST declarations, so it
/// carries no usage count; the mapping, not a marker on [`TagCount`], is what
/// tells an agent two spellings are one tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagAlias {
    /// The aliased (old) spelling, folded to lowercase.
    pub alias: String,
    /// The canonical spelling it folds onto, folded to lowercase.
    pub canonical: String,
}

/// One named vocabulary term with its usage count: an observation category or a
/// relation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NamedCount {
    /// The term.
    pub name: String,
    /// How many times it is used.
    pub count: i64,
}

/// The vocabulary in use across a domain or the whole index: tags with their
/// engram and observation usage counts, observation categories with counts,
/// relation types with counts and the engram `type` and `status` values in use.
/// Backs the `vocabulary` tool so an agent reuses an existing term instead of
/// coining a near-duplicate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Vocabulary {
    /// Tags, most-used first (engrams + observations), then by name.
    pub tags: Vec<TagCount>,
    /// Observation categories, most-used first, then by name.
    pub categories: Vec<NamedCount>,
    /// Relation types, most-used first, then by name.
    pub relation_types: Vec<NamedCount>,
    /// Engram `type` values in use, most-used first, then by name. Raw as
    /// stored: the vocabulary reports what the engrams are literally written in,
    /// never a recommended set.
    pub types: Vec<NamedCount>,
    /// Engram `status` values in use, most-used first, then by name. Raw as
    /// stored too, so `stable` and `current` stay two entries and a retired
    /// status is listed like any other.
    pub statuses: Vec<NamedCount>,
    /// Tag aliases in effect, sorted by alias then canonical. Derived from the
    /// scanned domains' MANIFEST declarations, so an agent sees which spellings
    /// fold onto which and reuses the canonical.
    pub aliases: Vec<TagAlias>,
}

/// Merge the decoded vocabulary aggregates into a sorted [`Vocabulary`].
/// Shared by both backends so the ordering is identical regardless of SQL's
/// grouping order: engram-tag and observation-tag counts merge by tag name, tags
/// sort by total usage (engrams + observations) descending then name, and
/// categories, relation types, engram types and engram statuses sort by count
/// descending then name.
pub(crate) fn build_vocabulary(
    engram_tags: Vec<(String, i64)>,
    observation_tags: Vec<(String, i64)>,
    categories: Vec<(String, i64)>,
    relation_types: Vec<(String, i64)>,
    types: Vec<(String, i64)>,
    statuses: Vec<(String, i64)>,
    aliases: Vec<(String, String)>,
) -> Vocabulary {
    let mut merged: HashMap<String, (i64, i64)> = HashMap::new();
    for (name, count) in engram_tags {
        merged.entry(name).or_default().0 = count;
    }
    for (name, count) in observation_tags {
        merged.entry(name).or_default().1 = count;
    }
    let mut tags: Vec<TagCount> = merged
        .into_iter()
        .map(|(name, (engrams, observations))| TagCount {
            name,
            engrams,
            observations,
        })
        .collect();
    tags.sort_by(|a, b| {
        (b.engrams + b.observations)
            .cmp(&(a.engrams + a.observations))
            .then_with(|| a.name.cmp(&b.name))
    });

    let to_named = |rows: Vec<(String, i64)>| -> Vec<NamedCount> {
        let mut v: Vec<NamedCount> = rows
            .into_iter()
            .map(|(name, count)| NamedCount { name, count })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        v
    };

    // Aliases arrive deduped from a `SELECT DISTINCT`, but an all-domain sweep
    // can still surface the same pair from two domains; dedupe and sort here so
    // the order never depends on SQL grouping, matching the tag ordering above.
    let mut aliases: Vec<TagAlias> = aliases
        .into_iter()
        .map(|(alias, canonical)| TagAlias { alias, canonical })
        .collect();
    aliases.sort_by(|a, b| {
        a.alias
            .cmp(&b.alias)
            .then_with(|| a.canonical.cmp(&b.canonical))
    });
    aliases.dedup();

    Vocabulary {
        tags,
        categories: to_named(categories),
        relation_types: to_named(relation_types),
        types: to_named(types),
        statuses: to_named(statuses),
        aliases,
    }
}

/// Sum several vocabularies into one, ordered exactly as a single sweep over
/// the same domains would order it.
///
/// This exists for the one caller that cannot ask SQL for what it wants: a
/// vocabulary sweep for a caller who may not read every domain. The store's own
/// sweep is all-domains or one domain, so such a caller reads the domains it may
/// see and merges here - and the merge is `build_vocabulary` itself, fed the
/// summed per-name counts, so the result cannot order or shape itself
/// differently from a sweep the store answered in one query.
pub fn merge_vocabularies(parts: Vec<Vocabulary>) -> Vocabulary {
    let mut engram_tags: HashMap<String, i64> = HashMap::new();
    let mut observation_tags: HashMap<String, i64> = HashMap::new();
    let mut categories: HashMap<String, i64> = HashMap::new();
    let mut relation_types: HashMap<String, i64> = HashMap::new();
    let mut types: HashMap<String, i64> = HashMap::new();
    let mut statuses: HashMap<String, i64> = HashMap::new();
    let mut aliases: Vec<(String, String)> = Vec::new();
    fn sum(into: &mut HashMap<String, i64>, rows: Vec<NamedCount>) {
        for row in rows {
            *into.entry(row.name).or_default() += row.count;
        }
    }
    for part in parts {
        for tag in part.tags {
            // A zero count is a name SQL would never have grouped, so it is not
            // carried into the pair lists `build_vocabulary` expects.
            if tag.engrams != 0 {
                *engram_tags.entry(tag.name.clone()).or_default() += tag.engrams;
            }
            if tag.observations != 0 {
                *observation_tags.entry(tag.name).or_default() += tag.observations;
            }
        }
        sum(&mut categories, part.categories);
        sum(&mut relation_types, part.relation_types);
        sum(&mut types, part.types);
        sum(&mut statuses, part.statuses);
        aliases.extend(
            part.aliases
                .into_iter()
                .map(|alias| (alias.alias, alias.canonical)),
        );
    }
    build_vocabulary(
        engram_tags.into_iter().collect(),
        observation_tags.into_iter().collect(),
        categories.into_iter().collect(),
        relation_types.into_iter().collect(),
        types.into_iter().collect(),
        statuses.into_iter().collect(),
        aliases,
    )
}

/// The reading half of the backend-agnostic storage interface: every method
/// here only reads, so it may run on a read connection beside the writer
/// ([`crate::reader`]). All methods are async so a network backend can
/// implement the same trait.
///
/// A method that fills a cache shared with the writer (the embedding coverage
/// snapshot) keeps its fill only when no write invalidated the cache since its
/// read began, so a reader on an older snapshot never stores a stale value.
#[async_trait]
pub trait ReadStore: Send + Sync {
    /// The id of a domain the index already holds, or `None` for a name it has
    /// never been told about. A pure read: it registers nothing.
    ///
    /// [`Store::upsert_domain`] is the other way to a [`DomainId`] and it
    /// writes, which is fine for a verb that is about to write anyway and wrong
    /// for one that is answering a question. A status call counting somebody's
    /// drafts, or a removal asking who would lose work, has to reach the rows
    /// without registering a domain on the way - and has to stay answerable on
    /// a read-only instance, where a write is refused outright.
    async fn domain_id(&self, name: &str) -> Result<Option<DomainId>>;

    /// Every recorded spelling of every domain with the id it resolves to,
    /// sorted byte-wise by spelling. The resolve pass reads the same table to
    /// find the domain a reference names; each domain row is at least its own
    /// name, which [`Store::upsert_domain`] records with the row.
    #[doc(hidden)]
    async fn domain_spellings(&self) -> Result<Vec<(String, DomainId)>>;

    /// Every `engram_meta_value` row, every actor's included, as
    /// `(engram id, key, value)`, sorted by id and then byte-wise. The
    /// parity tests read it to prove that both backends write and remove
    /// the same rows on every write path.
    ///
    /// For tests only: it reads the whole table, so no product path may call
    /// it.
    #[doc(hidden)]
    async fn meta_values(&self) -> Result<Vec<(EngramId, String, String)>>;

    /// Every `(source domain name, engram path)` that names one of `spellings`
    /// as a target domain: through a relation or link row, or through a
    /// `crystalline://<spelling>` URL in its text, which no edge table records.
    /// Base rows only, each once, sorted by domain then path, byte order.
    ///
    /// The engrams a rename has to respell. A name that merely starts with a
    /// spelling (`eng-other` for `eng`) is not a match.
    async fn engrams_referencing_domains(
        &self,
        spellings: &[String],
    ) -> Result<Vec<(String, String)>>;

    /// One domain's references whose domain prefix is one of `spellings`,
    /// with the line and the bracket text as written. Base rows only, ordered
    /// by source path, then line, then bracket text.
    async fn spelled_references(
        &self,
        domain: DomainId,
        spellings: &[String],
    ) -> Result<Vec<SpelledRef>>;

    /// The recorded file stamps for a domain, keyed by domain-relative path.
    async fn file_stamps(&self, domain: DomainId) -> Result<HashMap<String, FileStamp>>;

    /// Up to `limit` paths of the domain's base engram rows whose first path
    /// segment is the Reflection's folder in any ASCII letter case, in no
    /// particular order. The Reflection gate's fast path (R82): a domain that
    /// records engrams and none under the folder has nothing pending, which
    /// this answers without loading every [`ReadStore::file_stamps`] entry.
    async fn engram_rows_under_reflections(
        &self,
        domain: DomainId,
        limit: usize,
    ) -> Result<Vec<String>>;

    /// Whether the domain has any base engram row. A synced domain always
    /// records its `MANIFEST.md`, so `false` means the index holds no row of
    /// it and the files on disk decide what is pending.
    async fn has_engram_rows(&self, domain: DomainId) -> Result<bool>;

    /// Whether the domain's Reflection has any row: a page, the pact or a
    /// file under `reflections/assets/`. Two `LIMIT 1` seeks, never a
    /// listing, so the check after every change under `reflections/` costs
    /// the same however many pages the Reflection holds.
    async fn has_reflection_rows(&self, domain: DomainId) -> Result<bool>;

    /// Which of `paths` hold a base engram row in `domain`: the existence
    /// probe a browse level's folder counts ask of a reader's drafts and
    /// tombstones. One `LIMIT 1` seek of the unique `(domain_id, path,
    /// actor)` per distinct path, reading no body, so it costs the reader's
    /// overlay, never the domain.
    async fn base_paths(&self, domain: DomainId, paths: &[String]) -> Result<HashSet<String>>;

    /// The base engram rows of `domain` holding one of `permalinks`, with
    /// what a cited source is held against: title, status, checksum and
    /// mtime. A permalink no base row holds has no answer, a permalink named
    /// twice answers once, and the order is unspecified. One index seek per
    /// distinct permalink, so a page citing a few engrams never reads the
    /// whole domain's [`ReadStore::file_stamps`].
    async fn cited_engrams(
        &self,
        domain: DomainId,
        permalinks: &[String],
    ) -> Result<Vec<CitedEngram>>;

    /// The full markdown content stored for the engram at a domain-relative
    /// path, or `None` when no such row exists. Serves the database read path
    /// (virtual domains and non-host reads) and reads current content for a
    /// virtual edit.
    async fn engram_content(&self, domain: DomainId, path: &str) -> Result<Option<String>>;

    /// Every engram in a domain with its path, permalink, content and checksum,
    /// ordered by path. Streams a whole domain for `domain export`.
    async fn all_engram_contents(&self, domain: DomainId) -> Result<Vec<StoredEngram>>;

    // --- lookups -------------------------------------------------------------
    // Repository-level addressing helpers used by the service layer to turn a
    // tool identifier or a `crystalline://` anchor into ids and a file path.
    // They are on the trait (not backend-specific) so the engine can hold a
    // `dyn Store` and a second backend reimplements them in its own dialect.

    /// Resolve a `(domain, permalink)` pair to an engram id. Backs graph anchors
    /// and `crystalline://` resolution.
    async fn lookup_id(&self, domain: &str, permalink: &str) -> Result<Option<EngramId>>;

    /// Resolve an identifier within one domain to a descriptor. Matches the
    /// permalink first, then the title case-insensitively.
    async fn find_engram(&self, domain: &str, key: &str) -> Result<Option<EngramDescriptor>>;

    /// Resolve an identifier across every domain, permalink first then title.
    /// Returns all matches so the caller can detect an ambiguous bare identifier.
    async fn find_engram_any(&self, key: &str) -> Result<Vec<EngramDescriptor>>;

    /// List engrams in a domain, optionally under a domain-relative path prefix
    /// and optionally of a given type, ordered by path. Backs browse, validate
    /// and schema inference.
    async fn list_engrams(
        &self,
        domain: &str,
        path_prefix: Option<&str>,
        engram_type: Option<&str>,
    ) -> Result<Vec<EngramDescriptor>>;

    /// One level of a domain's folder tree: the engrams no more than `depth`
    /// segments below `path_prefix`, capped at `limit` rows and ordered by path,
    /// every child folder under the prefix, and the exact number of engrams the
    /// level holds.
    ///
    /// Three bounded queries rather than one unbounded listing, which is what
    /// keeps a navigation tree affordable on a domain of tens of thousands of
    /// engrams: the row page is `LIMIT`ed, the count runs under exactly the
    /// same filter so the two can never disagree, and the folder names and
    /// their counts come from a `GROUP BY` over the path column alone - no
    /// bodies are read to learn that a folder exists.
    ///
    /// `path_prefix` carries its trailing slash (`notes/`) and is matched as a
    /// literal, so a folder named `50%` or `a_b` is a folder rather than a
    /// wildcard. `None` or an empty prefix browses the domain root. `depth`
    /// counts segments below the prefix, so `1` is the direct children and `2`
    /// reaches one folder further down; it is clamped to at least 1 here, and a
    /// caller taking it from a request bounds it from above too, since the
    /// depth cut is a pattern that grows one term per level.
    ///
    /// **What `total` counts.** The level, not the folder. The count runs under
    /// the depth cut with the page, so it moves with `depth` and excludes
    /// everything nested deeper - a folder of ten engrams holding a subfolder of
    /// a thousand reports ten at depth 1. A [`SearchQuery`] carrying the same
    /// folder as its `path_prefix` counts the other way, recursively, over the
    /// prefix at any depth, and so reports the larger number. Both are right for
    /// what they answer: a level states a fact about the rows it just drew,
    /// while a folder listing promises the folder. Whoever renders "N engrams in
    /// this folder" wants the second one.
    ///
    /// The recursive number per child folder is in `folder_counts`: the folder
    /// names already come from a scan of every row under the prefix, so the
    /// same statement groups them by first segment and counts each group.
    async fn browse_level(
        &self,
        domain: &str,
        path_prefix: Option<&str>,
        depth: usize,
        limit: usize,
    ) -> Result<BrowseLevel>;

    /// Whether a base engram other than `except` stands anywhere under the
    /// folder `prefix` (with or without its trailing slash), at any depth. One
    /// `LIMIT 1` probe that stops at the first row, for a caller that asks
    /// only "is this folder new", which is the close-folder hint on a write
    /// receipt; [`ReadStore::browse_level`] answers a bigger question with a count
    /// and a `DISTINCT` scan.
    ///
    /// The prefix is matched exactly, case included and never as a pattern:
    /// `Plan/` is not `plan/`, the same way the hint compares folder names,
    /// and `a_b/` is not `axb/`. Drafts are not base rows and are not counted,
    /// and neither is a reserved `index.md` or `log.md` at any depth, matched
    /// exactly as [`crystalline_core::is_reserved_path`] matches: sync skips
    /// both names and every write refuses them, but a virtual domain's import
    /// stores any `.md` file it is handed.
    async fn folder_holds_other(
        &self,
        domain: DomainId,
        prefix: &str,
        except: &str,
    ) -> Result<bool>;

    /// Every engram carrying a tag, on its frontmatter or on one of its
    /// observations, optionally scoped to one domain. The bound tag is
    /// case-folded to match the case-folded `tag.name`. Ordered by domain name
    /// then path so a `tags rename` / `tags merge` rewrites files in a stable
    /// order. Backs the CLI tag-hygiene commands.
    async fn engrams_with_tag(
        &self,
        tag: &str,
        domain: Option<&str>,
    ) -> Result<Vec<EngramDescriptor>>;

    /// The distinct folded `(alias, canonical)` tag-alias pairs, optionally
    /// scoped to a set of domain names, ordered by alias then canonical. An
    /// all-domain sweep (no scope) unions every domain's map, so one alias can
    /// come back paired with more than one canonical. Backs query-time alias
    /// expansion and the vocabulary surface.
    async fn tag_aliases(&self, domains: Option<&[String]>) -> Result<Vec<(String, String)>>;

    /// Every relation or prose link that points at the given engram, with the
    /// linking engram's path and the exact target text. Used by the move to
    /// find the engrams whose references it rewrites.
    async fn inbound_refs(
        &self,
        engram_id: EngramId,
        domain_id: DomainId,
        permalink: &str,
        title: &str,
    ) -> Result<Vec<InboundRef>>;

    /// Every base engram whose stored content holds `needle` verbatim, as its
    /// domain and path, ordered by domain name then path.
    ///
    /// The move's second finder, beside [`ReadStore::inbound_refs`]: a
    /// `crystalline://domain/permalink` URL written in prose is a reference a
    /// reader follows, but the parser never turns it into a `link` row, so no
    /// edge table knows about it. This asks the content instead, and the move
    /// passes the old address as the needle. A hit is a candidate, not a
    /// verdict - `old` also matches inside `old-notes`, and inside code - so
    /// the caller's rewrite decides what is actually a reference.
    ///
    /// A plain substring scan over `engram_content`, case-sensitive like the
    /// address it looks for. It projects three narrow columns and never
    /// selects or sorts a body: sorting wide rows is what once made a search
    /// spill gigabytes here. Base rows only, like the inbound query: a draft
    /// is its author's own and is not rewritten by somebody else's move.
    async fn engrams_mentioning(&self, needle: &str) -> Result<Vec<ContentMention>>;

    /// One page of the references that point at an engram, filtered and counted
    /// in SQL, with the per-relation summary of all of them.
    ///
    /// The paged twin of [`ReadStore::inbound_refs`], over exactly the same rows:
    /// resolved references plus the unresolved same-domain ones whose text names
    /// this engram, so a total here and the count `read_engram` reports are the
    /// same number. What differs is what a reader needs - a title and a
    /// permalink to follow, a page rather than the whole set, and a substring
    /// filter - none of which the move's rewrite has any use for.
    ///
    /// `total` is exact under [`InboundQuery::q`] and [`InboundQuery::rel`], and
    /// [`InboundPage::types`] ignores both: an engram with thousands of inbound
    /// references is browsed by picking a relation type and then searching
    /// inside it, and a summary that shrank as it was used would be a map that
    /// redraws itself while it is being read.
    ///
    /// [`InboundQuery::exclude_domains`] is the one narrowing all three of
    /// them honor, summary included: it is not a filter a reader chose but the
    /// set of domains that reader may not see, and a count that named one
    /// would be the disclosure the exclusion is for.
    ///
    /// Ordered by title, then permalink, then domain, then relation, byte-wise
    /// on both backends, so paging is stable and a page boundary never drops or
    /// repeats a row.
    async fn inbound_page(&self, query: InboundQuery<'_>) -> Result<InboundPage>;

    /// Every relation and prose link that points out of the given engram, each
    /// carrying whether it currently resolves to a target in the index. Ordered
    /// by source line. Backs the `read_engram` resolution flags.
    ///
    /// `actor` is who is asking. `None` reads the stored `to_id` and nothing
    /// else, which is the domain's own verdict and the answer this gave before
    /// the dimension existed. `Some(a)` reads it in that actor's view, which
    /// moves exactly two verdicts and no others: a bound reference into a path
    /// they have deleted reads unresolved for them, and an unbound one that
    /// their own draft answers reads resolved. Both are read-time readings of
    /// the same stored row - the row is never rewritten for a reader.
    async fn outbound_refs(
        &self,
        engram_id: EngramId,
        actor: Option<&str>,
    ) -> Result<Vec<OutboundRef>>;

    /// Every relation and prose link written in a domain whose target the index
    /// could not bind, as the consolidation sweep's `V102` input.
    ///
    /// One row per dangling reference, never one per engram: an engram that
    /// names three missing targets yields three. The domain is the *source*
    /// domain, so a bare `[[Ghost]]` and a `[[other:Ghost]]` written in the same
    /// engram both come back here, the second carrying its prefix in
    /// [`UnresolvedRef::target_domain`] so a caller can tell an unregistered
    /// target domain apart from a target that simply does not exist.
    ///
    /// [`UnresolvedRef::target`] is the text exactly as the author wrote it
    /// inside the brackets, case and spacing intact, because a finding quotes it
    /// verbatim for the repair. A prose wikilink reports `links_to` as its
    /// `rel_type`, matching [`GraphEdge`]. A line of `0` (the parser's unknown)
    /// reads back as `None`.
    ///
    /// Ordered by source path then line, then kind and target for rows that tie,
    /// so the queue is stable across runs and identical on both backends. Rides
    /// the `idx_relation_unresolved` and `idx_link_unresolved` partial indexes,
    /// so the cost tracks the number of dangling references rather than the size
    /// of the domain.
    ///
    /// `actor` is who is asking, the same dimension [`ReadStore::outbound_refs`]
    /// names. `None` is the domain's own queue: the rows its files hold, judged
    /// by the stored `to_id`. `Some(a)` is that actor's queue - the base rows
    /// their drafts do not shadow plus their own drafts - judged in their view,
    /// so a base link their draft answers is not reported to them, a base link
    /// into a path they deleted is, and their own drafts' dangling references
    /// stand beside the domain's. Ordered the same way in both arms.
    async fn unresolved_refs(
        &self,
        domain: DomainId,
        actor: Option<&str>,
    ) -> Result<Vec<UnresolvedRef>>;

    /// Run a search and return one page of hits plus the total match count.
    async fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>> {
        self.search_with_candidate_cap(query, LEXICAL_CANDIDATE_CAP)
            .await
    }

    /// [`ReadStore::search`] with an explicit lexical candidate cap, so a test can
    /// exercise the cap boundary over a handful of engrams. `candidate_cap` only
    /// bounds how many LIKE candidates the lexical prefilter loads and ranks; it
    /// never changes the shape of a hit. Production always goes through
    /// [`ReadStore::search`], which passes [`LEXICAL_CANDIDATE_CAP`].
    async fn search_with_candidate_cap(
        &self,
        query: &SearchQuery,
        candidate_cap: usize,
    ) -> Result<Page<SearchHit>>;

    /// Return the neighborhood of a set of seed engrams up to `depth` hops
    /// (`1..=3`), following relations and links across domain boundaries.
    ///
    /// `actor` is the same dimension [`SearchQuery::actor`] names: `None` walks
    /// the base rows alone - the graph the domains' files describe, and the
    /// traversal this ran before the dimension existed - and `Some(a)` walks
    /// that actor's shadowed view. Both ends of every edge are screened, so an
    /// edge into a row the reader may not see is not walked at all rather than
    /// walked and then dropped at the hydrate, which would leave an edge with
    /// no node and pull engrams in through somebody else's private draft.
    async fn neighbors(
        &self,
        ids: &[EngramId],
        depth: u8,
        actor: Option<&str>,
    ) -> Result<GraphSlice>;

    /// Return recent engrams matching a filter, newest first.
    async fn recent(&self, filter: &RecentFilter) -> Result<Vec<EngramSummary>>;

    /// Return chunks in `table` that need an embedding for the given model:
    /// those with no embedding yet, plus those embedded by a different model (a
    /// model swap). `domains`, when `Some`, restricts the scan to those domains:
    /// the daemon scopes each instance's embed pass to the file domains it hosts
    /// plus all virtual domains, so a non-host does not wastefully re-embed a
    /// chunk another instance owns. `None` scans every domain (the
    /// single-instance and standalone default). Duplicate embedding across
    /// instances is always safe (the chunk fingerprint folds in the model id, so
    /// two instances converge on the same vector), just wasteful, and this
    /// filter avoids the waste.
    ///
    /// The result is one page: at most `limit` jobs ordered by
    /// `(owner, seq)`, starting past the `after` key when it is given. A
    /// caller pages the backlog with the last row of the previous page as the
    /// next cursor rather than holding every job's text at once, and stops when
    /// a page comes back short. Rows that gain an embedding mid-pass drop out of
    /// the predicate behind the cursor, so paging never skips or repeats a job.
    /// [`ChunkJob::engram_id`] is the owning row of the table: for
    /// [`ChunkTable::Engram`] the engram id, for [`ChunkTable::Reflection`]
    /// the chunk's own id (a reflection chunk has no owning row id).
    async fn chunks_needing_embedding_in(
        &self,
        table: ChunkTable,
        model: &str,
        domains: Option<&[DomainId]>,
        limit: usize,
        after: Option<(i64, i64)>,
    ) -> Result<Vec<ChunkJob>>;

    /// Embedding coverage of `table`: total chunks, embedded chunks and a
    /// per-model breakdown. With `domains` named, only those domains' chunks
    /// count, read fresh; a domain that does not exist counts nothing. With
    /// `None` it is the table-wide snapshot, served from a cache every
    /// mutator that can change an embedding drops. This is the one place the
    /// count is made: [`ReadStore::embedding_coverage_of`],
    /// [`ReadStore::embedding_coverage`] and [`ReadStore::reflection_coverage`] are
    /// this call with fixed arguments.
    async fn embedding_coverage_in(
        &self,
        table: ChunkTable,
        domains: Option<&[DomainId]>,
    ) -> Result<EmbeddingCoverage>;

    /// [`ReadStore::embedding_coverage_in`] on the engram chunks of the domains
    /// called `names`. A name that is not a registered domain counts nothing.
    /// The one place names become ids for a search: the engine's mode decision
    /// and the store's staleness gate both read it, so they cannot disagree.
    async fn embedding_coverage_of_names(&self, names: &[String]) -> Result<EmbeddingCoverage> {
        let mut ids = Vec::with_capacity(names.len());
        for name in names {
            if let Some(id) = self.domain_id(name).await? {
                ids.push(id);
            }
        }
        self.embedding_coverage_in(ChunkTable::Engram, Some(&ids))
            .await
    }

    /// [`ReadStore::embedding_coverage_in`] over the whole table.
    async fn embedding_coverage_of(&self, table: ChunkTable) -> Result<EmbeddingCoverage> {
        self.embedding_coverage_in(table, None).await
    }

    /// [`ReadStore::chunks_needing_embedding_in`] on the engram chunks.
    async fn chunks_needing_embedding(
        &self,
        model: &str,
        domains: Option<&[DomainId]>,
        limit: usize,
        after: Option<(i64, i64)>,
    ) -> Result<Vec<ChunkJob>> {
        self.chunks_needing_embedding_in(ChunkTable::Engram, model, domains, limit, after)
            .await
    }

    /// [`ReadStore::embedding_coverage_of`] the engram chunks: what `status` and
    /// the backlog read, and what a search that names no domain decides its
    /// mode and staleness on (R55). A search that names domains reads
    /// [`ReadStore::embedding_coverage_in`] of those domains instead (R88).
    async fn embedding_coverage(&self) -> Result<EmbeddingCoverage> {
        self.embedding_coverage_of(ChunkTable::Engram).await
    }

    /// Every engram in `domain` whose first chunk carries an embedding by
    /// `model`, with that vector, ordered by engram id. The maintenance
    /// sweep's `V301` compares these pairwise, so the projection is exactly
    /// the lead chunk - title, description and opening body - and nothing
    /// wider: one row per engram, never one per chunk. A row whose stored
    /// width disagrees with its `dims` column is skipped rather than
    /// returned mis-sized.
    ///
    /// `actor` says whose view of the domain the vectors are, the same
    /// dimension [`SearchQuery::actor`] names: `None` is the base rows alone -
    /// what the domain's files say exists, and byte for byte the statement
    /// this method ran before the dimension existed - and `Some(a)` is that
    /// actor's shadowed view, their own drafts standing in for the base rows
    /// they are drafts of, their deletions taking a path away, and nobody
    /// else's drafts ever in range. The dimension is explicit in what this
    /// takes rather than in what it returns: the caller already knows whose
    /// view it asked for, and a [`LeadVector`] is the widest row the index
    /// hands out, so stamping an actor on each one would pay per vector for a
    /// fact that is constant across the call.
    ///
    /// Unbounded on purpose, and the cost is the caller's to bound: every
    /// matching engram in the domain comes back, so this materializes
    /// `dims * 4` bytes of payload per engram plus per-vector heap overhead,
    /// twice over at the peak (the database rows and the decoded output are
    /// both live inside the call). At the default model's 384 dims that is
    /// roughly 1.5 KB an engram, so a hundred thousand of them is hundreds of
    /// megabytes.
    ///
    /// The one caller, the sweep's per-domain fact assembly, takes the whole
    /// fetch knowingly: it already parses every engram in the domain and holds
    /// each body, so the vectors add a fraction to a cost that was linear in
    /// domain size anyway, and a domain over the sweep's `MAX_TWIN_VECTORS`
    /// pays for vectors it then declines to compare. There is deliberately no
    /// count method to skip on - adding one, or pushing the sweep's rule filter
    /// down so a run that cannot emit `V301` never asks, is the named follow-up
    /// in the backlog. Until it lands, a new caller with a ceiling should
    /// assume this returns everything.
    async fn lead_vectors(
        &self,
        domain: DomainId,
        model: &str,
        actor: Option<&str>,
    ) -> Result<Vec<LeadVector>>;

    /// Every engram pair `model` has scored in `domain`, with the checksums it
    /// was scored at, ordered by `(a, b)`.
    async fn contradiction_pairs_scored(
        &self,
        domain: DomainId,
        model: &str,
    ) -> Result<Vec<ScoredPair>>;

    /// Every stored line pair in `domain` for `model` whose higher reading
    /// order is at or above `min_score`, ordered by `(a, b, line_a, line_b)`.
    /// The sweep's read: it filters by status, window and line hash itself, so
    /// a pair that stopped being a candidate still comes back here.
    async fn contradictions(
        &self,
        domain: DomainId,
        model: &str,
        min_score: f32,
    ) -> Result<Vec<ContradictionRow>>;

    /// How many engram pairs `model` has scored across the whole index. The
    /// status block's figure.
    async fn scored_pair_count(&self, model: &str) -> Result<u64>;

    /// Which of `hashes` carry a line vector for `model`. The contradiction
    /// walk's per-domain check before it embeds what is missing; narrow, the
    /// hash column only. Read on the full `(model, hash)` key: in runs of
    /// [`OBSERVATION_VECTOR_CHUNK`] on postgres, one hash at a time on turso.
    async fn observation_vectors_present(
        &self,
        model: &str,
        hashes: &[String],
    ) -> Result<HashSet<String>>;

    /// The line vectors for `model` of those `hashes` that have one, by hash.
    /// A row whose stored width disagrees with its `dims` is skipped with a
    /// warning. Read like [`ReadStore::observation_vectors_present`].
    async fn observation_vectors(
        &self,
        model: &str,
        hashes: &[String],
    ) -> Result<HashMap<String, Vec<f32>>>;

    /// Every hash with a line vector for `model`, unordered. The pruning read:
    /// narrow, one short column per row.
    async fn observation_vector_hashes(&self, model: &str) -> Result<Vec<String>>;

    /// The parser generation ([`crystalline_core::PARSE_GENERATION`]) the
    /// domain's rows were last derived with: 0 for a domain an upgrade found,
    /// the current generation for a row created since. A sync that reads an
    /// older value than the current one reparses the domain whole once.
    async fn parse_generation(&self, domain: DomainId) -> Result<u32>;

    /// One actor's draft at a domain-relative path, or `None` when that actor
    /// holds none there. A tombstone is a draft like any other and is returned,
    /// carrying its flag; a caller deciding what a reader sees reads the flag
    /// rather than the absence.
    async fn overlay_entry(
        &self,
        domain: DomainId,
        actor: &str,
        path: &str,
    ) -> Result<Option<StoredEngram>>;

    /// Every draft one actor holds in a domain, ordered by path. Tombstones
    /// included, for the same reason.
    async fn overlay_entries(&self, domain: DomainId, actor: &str) -> Result<Vec<StoredEngram>>;

    /// [`ReadStore::overlay_entries`] without the bodies: every draft path one
    /// actor holds in a domain with its tombstone flag, ordered by path. For a
    /// caller that only asks where drafts stand, such as the close-folder hint
    /// on a write receipt, so it never loads a draft's text to learn its path.
    async fn overlay_paths(&self, domain: DomainId, actor: &str) -> Result<Vec<(String, bool)>>;

    /// How many drafts each actor holds in a domain, ordered by actor. Base
    /// rows are not counted and an actor holding none is not listed, so an
    /// empty answer means nobody is drafting here.
    async fn overlay_counts(&self, domain: DomainId) -> Result<Vec<(String, u64)>>;

    /// Diagnostics about the open store.
    async fn store_info(&self) -> Result<StoreInfo>;

    /// Per-domain counts, in registration order.
    async fn domain_stats(&self) -> Result<Vec<DomainStats>>;

    /// Every domain name the index holds, sorted.
    ///
    /// The cheap half of [`ReadStore::domain_stats`], for the caller that needs to
    /// know *which* domains have rows rather than how many rows each has. One
    /// column of a table with a row per domain, against six correlated counting
    /// scans per domain - which matters because the serving screen asks this on
    /// every read, and a domain name is all it wants.
    async fn domain_names(&self) -> Result<Vec<String>>;

    /// The vocabulary in use: tag, observation-category, relation-type, engram
    /// `type` and engram `status` usage counts, for one domain or (when
    /// `domain` is `None`) across every domain.
    /// An unknown domain name yields empty vectors rather than an error, so a
    /// caller can probe a domain that holds no engrams yet. The vectors are
    /// sorted by usage in Rust for cross-backend determinism.
    ///
    /// `actor` is who is asking. `None` - every caller that shows a person the
    /// vocabulary - answers the team's own list, because a tag one author is
    /// trying out in a draft is not yet the domain's agreement. `Some(a)` reads
    /// that actor's view, and the one caller that passes it is the sweep's tag
    /// drift rule, whose finding is about what that author wrote.
    async fn vocabulary(&self, domain: Option<&str>, actor: Option<&str>) -> Result<Vocabulary>;

    /// One file's metadata in `table` by path, or `None`.
    async fn get_asset(
        &self,
        table: AssetTable,
        domain: DomainId,
        path: &str,
    ) -> Result<Option<AttachmentRow>>;

    /// Every file of a domain in `table`, ordered by path in byte order (both
    /// backends pin the collation, so the listing is identical across them).
    async fn list_assets(&self, table: AssetTable, domain: DomainId) -> Result<Vec<AttachmentRow>>;

    /// [`ReadStore::get_asset`] on the engram attachments.
    async fn get_attachment(&self, domain: DomainId, path: &str) -> Result<Option<AttachmentRow>> {
        self.get_asset(AssetTable::Engram, domain, path).await
    }

    /// [`ReadStore::list_assets`] on the engram attachments.
    async fn list_attachments(&self, domain: DomainId) -> Result<Vec<AttachmentRow>> {
        self.list_assets(AssetTable::Engram, domain).await
    }

    /// An attachment's stored bytes, or `None` when the path has no row or the
    /// row has no blob (the normal case for a file domain, whose bytes are on
    /// disk).
    async fn read_attachment_blob(&self, domain: DomainId, path: &str) -> Result<Option<Vec<u8>>>;

    /// One reflection's row, or `None`.
    async fn get_reflection(&self, domain: DomainId, path: &str) -> Result<Option<ReflectionRow>>;

    /// Every reflection row of a domain, ordered by path in byte order.
    async fn list_reflections(&self, domain: DomainId) -> Result<Vec<ReflectionRow>>;

    /// Every reflection row of a domain without its text, ordered by path in
    /// byte order: what the sync's stat prefilter and the move screens read,
    /// which have no use for the page itself.
    async fn list_reflection_stamps(&self, domain: DomainId) -> Result<Vec<ReflectionStamp>>;

    /// Embedding coverage of one domain's reflection chunks: total chunks,
    /// embedded chunks and a per-model breakdown, read fresh (never cached).
    /// A reflection search reads it for its mode decision and its staleness
    /// gate, so a domain whose pages are not embedded yet answers as text
    /// while another domain's are. [`ReadStore::embedding_coverage_of`] stays the
    /// table-wide count that status and the backlog read. This is
    /// [`ReadStore::embedding_coverage_in`] on the reflection chunks of `domain`.
    async fn reflection_coverage(&self, domain: DomainId) -> Result<EmbeddingCoverage> {
        self.embedding_coverage_in(ChunkTable::Reflection, Some(&[domain]))
            .await
    }

    /// The vector half of a reflection search (R60): the closest chunk of
    /// each section of one domain's pages embedded by `model` at
    /// `query.len()` dims, at most [`crate::rank::SEMANTIC_TOPK`] sections,
    /// best first, ties on path then section order. Answers
    /// [`crate::IndexError::StaleEmbeddings`] when the stored reflection vectors
    /// cannot be compared with the query (`crate::rank::check_staleness`
    /// over [`ReadStore::reflection_coverage`] of `domain`), so a caller falls
    /// back to text.
    async fn reflection_vector_hits(
        &self,
        domain: DomainId,
        query: &[f32],
        model: &str,
    ) -> Result<Vec<SectionVector>>;

    /// The labels `instance_id` is recorded under already (on a lock it
    /// holds, or on a domain it last hosted), each once.
    async fn own_labels(&self, instance_id: &str) -> Result<Vec<String>>;

    /// The instance ids holding a lock under `label` whose heartbeat is not
    /// older than `stale_before`.
    async fn live_label_holders(&self, label: &str, stale_before: &str) -> Result<Vec<String>>;

    /// The current holder of a file domain's host lock, or `None` when unheld.
    async fn domain_host(&self, domain: DomainId) -> Result<Option<DomainHost>>;

    /// [`ReadStore::domain_host`] read for a walk about to apply: inside the
    /// caller's write transaction the lock row stays as read until it ends
    /// (`FOR SHARE` on Postgres), so a claim of the domain waits for the
    /// apply to commit instead of landing in the middle of it. A Turso
    /// database belongs to one instance, so the plain read is the same.
    async fn domain_host_held(&self, domain: DomainId) -> Result<Option<DomainHost>>;

    /// The disclosures recorded on `domain`'s Reflection, one entry per
    /// page: its path inside `reflections/` and its records as the JSON text
    /// [`Store::set_reflection_disclosures`] stored. Sorted by path.
    async fn reflection_disclosures(&self, domain: DomainId) -> Result<Vec<(String, String)>>;

    /// The lease of `loop_kind` on `domain` as stored, a stale one included.
    async fn automation_lease(
        &self,
        domain: DomainId,
        loop_kind: &str,
    ) -> Result<Option<AutomationLease>>;

    /// The newest `limit` runs of `loop_kind` on `domain`, newest first.
    async fn automation_runs(
        &self,
        domain: DomainId,
        loop_kind: &str,
        limit: usize,
    ) -> Result<Vec<AutomationRun>>;

    /// The domain's host move as stored, JSON text, or `None`.
    async fn host_move_text(&self, domain: DomainId) -> Result<Option<String>>;

    /// The instance that last acquired a file domain's host lock, as
    /// `(instance id, label)`, or `None` when none was recorded. Kept on the
    /// domain row, so it outlives a release, which deletes the lock row.
    async fn last_domain_host(&self, domain: DomainId) -> Result<Option<(String, String)>>;

    /// Whether another instance than `instance_id` is recorded under
    /// `label`, as a lock's holder or as a domain's last host: two instances
    /// on one machine name, or behind one address, would show the same name,
    /// so the later one adds a short id ([`crate::instance_label`]).
    async fn label_taken(&self, label: &str, instance_id: &str) -> Result<bool>;

    /// The domain's unresolved host move, or `None`.
    async fn host_move(&self, domain: DomainId) -> Result<Option<HostMove>> {
        Ok(self
            .host_move_text(domain)
            .await?
            .and_then(|text| parse_host_move(&text)))
    }
}

/// The backend-agnostic storage interface: the reads of [`ReadStore`] plus
/// every write and the transaction verbs. The engine holds one behind a mutex;
/// code that holds it reads through it too, which is what keeps a read inside
/// an open transaction, or a read whose answer decides a write, on the writer.
#[async_trait]
pub trait Store: ReadStore {
    /// Register or update a domain by name, root path and kind, returning its
    /// id. A file domain passes `Some(path)`; a virtual domain passes `None`,
    /// since its engrams live in the database with no filesystem root. The name
    /// is recorded as one of the domain's spellings too, unless another domain
    /// already holds that spelling.
    async fn upsert_domain(
        &self,
        name: &str,
        path: Option<&str>,
        kind: DomainKind,
    ) -> Result<DomainId>;

    /// Replace the spellings of the domains `spellings` names with exactly
    /// that list, and answer every spelling whose mapping changed - added,
    /// removed or now pointing at a different domain - sorted byte-wise.
    ///
    /// The replace is scoped to the domain ids the list names, so instances
    /// sharing one Postgres index never delete each other's canonical names or
    /// aliases. The engine passes every `(spelling, local name)` pair of its
    /// name table, and every registered domain is in it through its local name,
    /// so the scope is exactly the domains this instance registers.
    ///
    /// Every domain row keeps its own name as a spelling whether or not the
    /// list names it, and that name wins over any other claim on the spelling:
    /// local names always win, which is the name table's first rule. A listed
    /// spelling another domain holds as a canonical name or alias moves to the
    /// listed domain. One spelling listed for two domains, or a domain id with
    /// no row, is [`crate::IndexError::Invalid`] and changes nothing.
    ///
    /// Opens its own transaction: never call it between [`Store::begin`] and
    /// [`Store::commit`].
    async fn replace_domain_spellings(
        &self,
        spellings: &[(String, DomainId)],
    ) -> Result<Vec<String>>;

    /// Unbind every relation and link row, every actor's included, whose
    /// `to_domain` is one of `spellings`, so the next resolve pass binds it
    /// through what the spelling means now. Answers the number of rows that
    /// were bound and are not any more.
    ///
    /// Opens its own transaction: never call it between [`Store::begin`] and
    /// [`Store::commit`]. The resolve passes that follow it are the caller's
    /// to wrap, as [`crate::resolve_forward_refs`] does.
    async fn reset_references_to_spellings(&self, spellings: &[String]) -> Result<u64>;

    /// Bind every pending relation and link row, in every domain and every
    /// actor's included, whose `to_domain` is one of `spellings`. Answers the
    /// number of rows bound. The pass a newly registered domain needs: a
    /// reference elsewhere that already named it waited pending, and only the
    /// rows spelled with its names can bind to it, so nothing else is read.
    ///
    /// Base candidates, as every resolve pass has: right for a domain that was
    /// just registered, which holds no drafts and no tombstones yet.
    ///
    /// Opens its own transaction: never call it between [`Store::begin`] and
    /// [`Store::commit`].
    async fn resolve_references_to_spellings(&self, spellings: &[String]) -> Result<u64>;

    /// Bind the pending relation and link rows in every domain but `domain`
    /// (every actor's rows) whose `to_domain` is one of `spellings` and whose
    /// target, lowercased, is one of `targets` lowercased, by the base rule
    /// every resolve pass uses. Answers the number bound per domain, sorted by
    /// id, domains with none left out. The pass a base write in `domain` needs
    /// when it adds an engram or changes its permalink or title: `targets`
    /// are that permalink and that title, `spellings` every name `domain`
    /// answers to.
    ///
    /// Opens its own transaction: never call it between [`Store::begin`] and
    /// [`Store::commit`].
    async fn resolve_references_to(
        &self,
        domain: DomainId,
        spellings: &[String],
        targets: &[String],
    ) -> Result<Vec<(DomainId, u64)>>;

    /// Rename a domain row in place: `domain.name` becomes `new` and the id
    /// stays, so every engram and every bound reference stays attached. The
    /// row's own spelling follows it in the same transaction (taking `new`
    /// from any domain that held it as a canonical name or alias, since local
    /// names win), and `old` stays behind as an alias spelling of the same
    /// row. `relation.to_domain` and `link.to_domain` keep the text as
    /// written: a reference spelled `old` resolves through that alias, before
    /// and after the rename, so no file reads as unresolved. When `new` was
    /// another domain's spelling, the references bound through it are
    /// unbound; the caller runs the resolve passes afterwards.
    ///
    /// The alias lasts until the next [`Store::replace_domain_spellings`] for
    /// the row, which keeps only what its list names: the caller records `old`
    /// among the domain's aliases before that runs.
    ///
    /// Idempotent, for a rename that is completed again after a crash: a row
    /// already named `new` with none named `old` is fine, and so is no row
    /// under either name (a domain that was never indexed has nothing to
    /// rename). A different row already named `new` is
    /// [`crate::IndexError::Constraint`] naming both.
    ///
    /// Opens its own transaction: never call it between [`Store::begin`] and
    /// [`Store::commit`].
    async fn rename_domain_row(&self, old: &str, new: &str) -> Result<()>;

    /// Drop the domain row named `name` when it holds nothing: no engram row
    /// of any actor (a draft counts) and no attachment. Its spellings, tag
    /// aliases, host lock and any stray relation or link rows go with it.
    /// Answers `true` when a row was dropped, `false` when there is no row
    /// under the name or the row still holds something, which is left exactly
    /// as it was.
    ///
    /// For a domain nobody registers any more: `domain remove` leaves an
    /// empty row after [`Store::clear_domain`], and the row would otherwise
    /// hold the name against a rename or a later adoption. The caller decides
    /// that the domain is unregistered and not another instance's; this only
    /// guarantees that nothing with content is ever dropped.
    ///
    /// Opens its own transaction: never call it between [`Store::begin`] and
    /// [`Store::commit`].
    async fn drop_empty_domain_row(&self, name: &str) -> Result<bool>;

    /// Insert or update one engram, replacing its observations, relations, links
    /// and tags. Returns the engram id. A duplicate permalink (a different path
    /// already owning the permalink) is a [`crate::IndexError::Constraint`].
    async fn upsert_engram(&self, domain: DomainId, record: &EngramRecord) -> Result<EngramId>;

    /// Like [`Store::upsert_engram`], but a compare-and-swap on the stored
    /// content. When `expected_sha` is `Some` and a row already exists at
    /// `record.path` whose stored `sha256` differs, this returns
    /// [`crate::IndexError::StaleEdit`] instead of writing; otherwise it behaves
    /// exactly like `upsert_engram`. The compare and the write are one atomic
    /// unit (the engine wraps the call in a transaction). This is the guard that
    /// makes a stale virtual edit a conflict rather than a silent clobber.
    async fn upsert_engram_checked(
        &self,
        domain: DomainId,
        record: &EngramRecord,
        expected_sha: Option<&str>,
    ) -> Result<EngramId>;

    /// Rewrite the base row at `record.path` only if it still exists and still
    /// carries `expected_sha`, with the same guarded compare as
    /// [`Store::upsert_engram_checked`]: `Ok(Some(id))` when it was rewritten,
    /// `Ok(None)` when no row stands at the path any more (deleted or renamed
    /// since it was read), [`crate::IndexError::StaleEdit`] when one stands
    /// there with other content. Never inserts, which is what the reparse
    /// after a parser change needs: it rewrites what it read, and a row that
    /// vanished meanwhile must stay gone. On Postgres the never-insert
    /// guarantee holds only inside a transaction: the compare's row lock is
    /// what keeps a concurrent delete out until the rewrite, and without a
    /// transaction it is released when the compare statement ends, so a row
    /// deleted between the compare and the rewrite would be inserted again.
    async fn update_engram_checked(
        &self,
        domain: DomainId,
        record: &EngramRecord,
        expected_sha: &str,
    ) -> Result<Option<EngramId>>;

    /// Delete every engram (and its child and chunk rows) in a single domain,
    /// keeping the domain row itself. The scoped clear behind `domain remove`
    /// and the orphaned-row sweep. Contrast [`Store::wipe`], which clears
    /// everything.
    ///
    /// The domain's spellings go too, all but its own name: the row stays and
    /// still answers to that, while a canonical name or former name left behind
    /// would keep resolving references to a domain nobody registers and would
    /// hold the spelling against the next domain that claims it.
    ///
    /// It also clears the domain's contradiction pairs and line rows, and
    /// unbinds (`to_id = NULL`) every link and relation row in another domain
    /// that named one of its engrams, so those references read as pending.
    ///
    /// A reindex does not use this, and deliberately: `--full` re-reads and
    /// re-upserts instead, so rows a reader is using are never absent between
    /// a clear and the rebuild that would have refilled them, and an unchanged
    /// chunk keeps its embedding.
    async fn clear_domain(&self, domain: DomainId) -> Result<()>;

    /// Delete the engram at a domain-relative path and all its child rows.
    ///
    /// Every link and relation row that named the deleted engram, in any
    /// domain and for any actor, is unbound (`to_id = NULL`) in the same
    /// statements, so it reads as pending until a write at that address binds
    /// it again. Opens no transaction of its own: the sync driver calls this
    /// inside one, and the engine's delete wraps its call in one.
    async fn delete_engram(&self, domain: DomainId, path: &str) -> Result<()>;

    /// Move an engram from one path to another without reparsing. The permalink
    /// follows only when it was path-derived.
    async fn rename_engram(&self, domain: DomainId, from: &str, to: &str) -> Result<()>;

    /// Give the base row at `from` a new path and a new permalink in one
    /// statement, without reparsing, keeping its id.
    ///
    /// The move's own rename, beside the sync's [`Store::rename_engram`]: a
    /// move decides the permalink itself - the path's, the one the engram
    /// kept, or one the caller named - and writes it into the frontmatter, so
    /// the row takes exactly that value rather than the path-derived guess.
    /// The id is what every chunk, embedding and bound reference keys to, so
    /// keeping it is what makes a rename cheaper than a delete and a create.
    /// `from` and `to` may be the same path, which is the permalink-only
    /// rename. The caller has already made sure no other row in the domain
    /// holds `permalink`; the unique index refuses it otherwise.
    async fn readdress_engram(
        &self,
        domain: DomainId,
        from: &str,
        to: &str,
        permalink: &str,
    ) -> Result<()>;

    /// Resolve every pending forward reference in a domain in one batch,
    /// matching the target text against permalink then title within the target
    /// domain. Returns the number of relations newly resolved.
    async fn resolve_pending_relations(&self, domain: DomainId) -> Result<u64>;

    /// Resolve every pending prose wikilink in a domain in one batch, the same
    /// permalink-then-title match as [`Store::resolve_pending_relations`] but
    /// over the `link` table. Wikilinks carry no relation type, so a resolved
    /// link becomes a `links_to` edge in graph traversal. Returns the number of
    /// links newly resolved.
    async fn resolve_pending_links(&self, domain: DomainId) -> Result<u64>;

    /// Replace a domain's derived tag-alias rows with the given folded
    /// `(alias, canonical)` pairs. Deletes every existing row for the domain,
    /// then inserts each pair, ignoring a duplicate alias. Called by
    /// [`crate::sync::refresh_tag_aliases`] on every sync, so an empty slice
    /// clears the domain's aliases. Both sides are already lowercased by the
    /// caller; the store never parses markdown. Rows that already say exactly
    /// this are left alone and nothing is written; answers whether it wrote.
    async fn replace_tag_aliases(
        &self,
        domain: DomainId,
        pairs: &[(String, String)],
    ) -> Result<bool>;

    /// Re-resolve one actor's own references in their own view, and answer how
    /// many bound.
    ///
    /// Two statements per reference table, inside whatever transaction the
    /// caller has open. First what dangles is unbound: a `to_id` naming a row
    /// nobody holds any more is the trace of a draft that went away, and
    /// leaving it would be an edge into nothing. Then what is pending is bound,
    /// against `ReferenceCandidates::View` - their own drafts first, then the
    /// base - so the author's links follow their own rows as those rows are
    /// written, dropped and folded.
    ///
    /// Scoped to that actor's rows on purpose: the base resolvers
    /// ([`Store::resolve_pending_relations`] and its twin) stay exactly as they
    /// are and keep answering for the domain, because a base row's edges are a
    /// fact about the domain rather than about a reader.
    ///
    /// **A binding is made once and not revisited, and two cases follow from
    /// that.** A reference bound before its author tombstoned the target stays
    /// bound, since a tombstone deletes nothing - and that one the reader's own
    /// screen settles at every surface that reads an edge, because the deleted
    /// path is where the binding points. The other it does not: a reference
    /// bound to a base row, whose author LATER writes a page of their own
    /// answering to the same name at a different path, goes on naming the base
    /// row. Neither statement here moves it (it does not dangle, and it is not
    /// pending) and no screen can, because the two pages sit at two addresses.
    /// A third statement that unbound every bound reference to see whether a
    /// newer row of the author's would beat it is what that would take, and it
    /// would rewrite an author's settled links on every write they make. The
    /// binding is the reading that held when the reference was written, which
    /// is the honest answer rather than the better one.
    ///
    /// An empty `actor` is a [`crate::IndexError::Constraint`], like
    /// [`Store::upsert_overlay`]: the empty string is the base row's own key.
    async fn reresolve_actor_references(&self, domain: DomainId, actor: &str) -> Result<u64>;

    /// Replace an engram's chunk rows with a freshly computed set, carrying over
    /// the embedding of any chunk whose fingerprint is unchanged so an edit only
    /// re-embeds the paragraphs that actually changed. Called by the sync engine
    /// after each upsert.
    async fn replace_chunks(&self, engram_id: EngramId, chunks: &[NewChunk]) -> Result<()>;

    /// Store a batch of embeddings against their chunks in `table` for the
    /// given model.
    async fn store_embeddings_in(
        &self,
        table: ChunkTable,
        batch: &[EmbeddingRow],
        model: &str,
    ) -> Result<()>;

    /// Clear the vectors in `table` of every model other than `model`,
    /// returning how many chunks were cleared.
    ///
    /// The embedding lives in columns on the chunk table (`embedding`, `dims`,
    /// `model`), so this nulls those three and never removes a chunk row: the
    /// text stays, and every cleared chunk is back in
    /// [`ReadStore::chunks_needing_embedding_in`] for the active model. Called once
    /// per table by an embed pass that finds the active model covering every
    /// chunk of that table, to reclaim the space an install's previous model
    /// left behind. A user who switches back simply re-embeds; nothing is kept
    /// for a return trip.
    async fn prune_embeddings_except_in(&self, table: ChunkTable, model: &str) -> Result<usize>;

    /// [`Store::store_embeddings_in`] on the engram chunks.
    async fn store_embeddings(&self, batch: &[EmbeddingRow], model: &str) -> Result<()> {
        self.store_embeddings_in(ChunkTable::Engram, batch, model)
            .await
    }

    /// [`Store::prune_embeddings_except_in`] on the engram chunks.
    async fn prune_embeddings_except(&self, model: &str) -> Result<usize> {
        self.prune_embeddings_except_in(ChunkTable::Engram, model)
            .await
    }

    /// Record one scoring of `pair` by `model`: the pair row with `cosine` and
    /// `scored_at`, and its line rows, replacing whatever an earlier scoring
    /// left, in one transaction. `rows` may be empty: a pair with nothing above
    /// the store floor is scored with nothing to say and is not asked again
    /// until an edit moves a checksum. Refuses inside an open `begin`.
    async fn replace_contradictions(
        &self,
        domain: DomainId,
        pair: &ScoredPair,
        cosine: f64,
        model: &str,
        scored_at: &str,
        rows: &[ContradictionRow],
    ) -> Result<()>;

    /// Delete what `model` stored for the pair `a`, `b` (lower id first): its
    /// pair row and its line rows, in one transaction, through the primary
    /// keys. The contradiction walk's delete for a pair one of whose engrams is
    /// past its `valid_to`: with the pair row gone, a window opened again later
    /// makes the pair pending, not scored with nothing to say. Refuses inside
    /// an open `begin`.
    async fn delete_contradiction_pair(&self, model: &str, a: EngramId, b: EngramId) -> Result<()>;

    /// Store line vectors for `model`. A (model, hash) already stored keeps
    /// its vector: the same text under the same model is the same vector. One
    /// transaction per call; refuses inside an open `begin`.
    async fn store_observation_vectors(
        &self,
        model: &str,
        rows: &[ObservationVector],
    ) -> Result<()>;

    /// Delete the line vectors of `hashes` under `model` in one transaction,
    /// on the full key like [`ReadStore::observation_vectors_present`]; returns
    /// how many went.
    async fn delete_observation_vectors(&self, model: &str, hashes: &[String]) -> Result<u64>;

    /// Delete every line vector of any embedding model but `model`, once a
    /// daemon knows which model it runs. Not a hot statement: it runs once per
    /// process.
    async fn delete_observation_vectors_except(&self, model: &str) -> Result<u64>;

    /// Delete every line vector. The contradiction check's off switch.
    async fn clear_observation_vectors(&self) -> Result<u64>;

    /// Delete the contradiction pair and line rows of `domains`, or of every
    /// domain when `None`, in one transaction. The contradiction check's off
    /// switch; a domain's own clear stays `clear_domain`.
    async fn clear_contradictions(&self, domains: Option<&[DomainId]>) -> Result<()>;

    /// The read connections this store opened beside itself, as one
    /// [`ReadStore`] that runs every call on a connection of its own, or
    /// `None` when it has none (an in-memory store, read connections turned
    /// off with `CRYSTALLINE_READ_CONNECTIONS=0`). The engine then reads
    /// through the writer, under its lock ([`crate::reader::LockedReader`]).
    fn read_pool(&self) -> Option<std::sync::Arc<dyn ReadStore>> {
        None
    }

    /// Whether this database may be shared by several instances at once, so
    /// per-instance housekeeping must not delete shared rows.
    fn shares_database(&self) -> bool {
        false
    }

    /// Try to take an exclusive work claim named `key` for as long as the
    /// returned guard lives. On Postgres a session-level advisory lock on a
    /// connection the guard holds; the lock ends with the session, so a
    /// crashed holder lets go by itself. On a store this process owns alone
    /// there is nobody to exclude: always `Some`. `None` means another
    /// session holds the claim right now.
    async fn try_work_claim(&self, key: &str) -> Result<Option<WorkClaim>>;

    /// Delete all indexed data, keeping the schema. The corruption-recovery
    /// path behind `crystalline reindex --wipe`, and nothing else: an ordinary
    /// rebuild (`--full`) never comes here, because destroying every embedding
    /// to re-read files that mostly did not change costs hours and buys
    /// nothing. The contradiction tables and the line vectors go with
    /// everything else.
    async fn wipe(&self) -> Result<()>;

    /// Best-effort WAL checkpoint in TRUNCATE mode, shrinking a local WAL file
    /// back down after a sync or reindex, full or incremental, or a wipe. Any
    /// of these may precede a caller shipping the database file as a
    /// single-file snapshot (sidecars deleted), so the WAL must not be left
    /// holding an un-merged delta. The default is a no-op, which is correct
    /// for Postgres: it has no local WAL file to truncate. Turso overrides
    /// this with a real `PRAGMA wal_checkpoint(TRUNCATE)` (confirmed to work
    /// by a runtime probe; see the doc comment on `TursoStore::build`).
    async fn checkpoint_wal(&self) -> Result<()> {
        Ok(())
    }

    /// Record that a sync of a domain finished at the given RFC 3339 instant.
    /// `changed` is whether the sync wrote anything. A sync that changed
    /// nothing stamps the domain only when it carries no stamp yet (so the
    /// first sync of an empty domain still reads as synced) or when its stamp
    /// is older than `refresh_before`, so an idle domain synced on every
    /// watcher flush writes its stamp at most once per refresh interval
    /// ([`crate::SYNC_STAMP_REFRESH`]) and the stamp is never older
    /// than that interval after a sync. An engine that wants the exact time
    /// of its last sync keeps it in memory.
    async fn record_sync(
        &self,
        domain: DomainId,
        when: &str,
        changed: bool,
        refresh_before: &str,
    ) -> Result<()>;

    /// Record that every named domain was seen registered at the given RFC
    /// 3339 instant, writing `last_registered` on each matching row.
    ///
    /// Domains are named rather than identified because the caller is the
    /// configuration, which knows names. A name with no row in the index is a
    /// silent no-op: this stamps rows, it never creates them. An empty set is
    /// a no-op too, so a configuration that registers nothing is not an error.
    ///
    /// The caller must pass the configuration's own registrations, read the
    /// way a named lookup resolves them (including a re-read of the config
    /// file on disk), not a cached or narrower approximation of that set. A
    /// domain that is genuinely registered but missing from the set handed in
    /// here goes unstamped, ages, and is indistinguishable from one that was
    /// removed - which, for a caller that collects on the stamp, is data loss.
    ///
    /// The stamp is how a later reader tells a domain removed a week ago from
    /// one whose configuration was edited an hour ago. Absence of a stamp
    /// (`DomainStats::last_registered` reading `None`) means *never stamped*,
    /// not *stamped infinitely long ago*: a caller aging the stamp must treat
    /// `None` as no evidence of staleness and leave the row alone, so the
    /// first sweep after an upgrade, when every pre-existing row reads `None`,
    /// collects nothing.
    async fn stamp_registered(&self, names: &[&str], when: &str) -> Result<()>;

    /// Stamp a domain as having a forced rebuild in flight, `when` being an RFC
    /// 3339 instant and `kind` the verb that is running. Written in the first
    /// lock window of the domain's rebuild, before anything is read from disk.
    ///
    /// The kind is stamped with the instant because the two verbs leave
    /// opposite states behind and a reader cannot tell them apart afterwards:
    /// an interrupted `--full` left complete rows from before it, an
    /// interrupted `--wipe` left whatever its rebuild had managed and no
    /// embeddings at all.
    ///
    /// The stamp is durable on purpose. The in-memory activity record the
    /// daemon keeps dies with the process, and the incident this exists for was
    /// on the daemonless path, which has no activity record at all: whatever
    /// runs, the fact that a rebuild started has to outlive the process that
    /// started it, so an interrupted run is never read as a normal one.
    async fn begin_rebuild(&self, domain: DomainId, when: &str, kind: RebuildKind) -> Result<()>;

    /// Clear a domain's rebuild stamp. Called from inside the transaction that
    /// commits the rebuild's apply, so the marker is set exactly while that
    /// domain's rebuild is unfinished and is never cleared by a run that did
    /// not finish one.
    async fn end_rebuild(&self, domain: DomainId) -> Result<()>;

    /// Record the parser generation a domain's rows were derived with. Called
    /// inside the transaction that commits a whole-domain reparse, so the
    /// stamp moves exactly when that reparse is complete and a run that died
    /// before it leaves the old value standing for the next one to repeat.
    async fn set_parse_generation(&self, domain: DomainId, generation: u32) -> Result<()>;

    /// The unreadable database this store was opened beside, when it was opened
    /// resiliently and found one. `None` for every ordinary open, and for every
    /// backend but the embedded one: corruption recovery is a local file
    /// concern.
    ///
    /// A rebuild that had to move a damaged database out of the way says where
    /// the bytes went, rather than leaving a person to guess whether anything
    /// was kept. See [`crate::TursoStore::open_resilient`].
    fn set_aside_database(&self) -> Option<PathBuf> {
        None
    }

    // --- the actor dimension -------------------------------------------------
    // An overlay entry is one actor's private draft of a path in a shared
    // domain: a full `engram` row carrying that actor's key, sitting beside the
    // base row - the one the domain's files on disk say exists - rather than
    // replacing it. Because it is a full row, its chunks, embeddings and graph
    // rows key to its id exactly as a base row's do, and nothing downstream has
    // to learn a second shape.
    //
    // Every method above this comment reads the base and only the base. These
    // five are the whole of the other direction, and all of them name their
    // actor. The empty actor never reaches a base row through any of them:
    // `upsert_overlay` refuses it outright, because a write that fell through
    // to the base is the one failure this dimension exists to prevent, while
    // the three readers answer as they would for an actor holding nothing -
    // `None`, empty, `false`.

    /// Write one actor's draft of a path, replacing that actor's previous draft
    /// there. Returns the row's id, which is stable across rewrites so the
    /// chunks already keyed to it survive.
    ///
    /// `actor` is authoritative and `record.actor` is ignored, so a caller
    /// cannot write into one actor's overlay while naming another. An empty
    /// `actor` is a [`crate::IndexError::Constraint`]: the empty string is the
    /// base row's own key, and a draft that silently overwrote the base would
    /// be the one failure this whole dimension exists to prevent.
    ///
    /// A tombstone - this actor's draft deletion of the base row - is written
    /// the same way, with `record.tombstone` set.
    async fn upsert_overlay(
        &self,
        domain: DomainId,
        actor: &str,
        record: &EngramRecord,
    ) -> Result<EngramId>;

    /// Drop one actor's draft at a path. Returns whether a row was there to
    /// drop, so a caller can tell "cleared" from "there was nothing to clear"
    /// without asking twice.
    ///
    /// Opens no transaction of its own: its caller (`clear_row`) holds one, so
    /// the child rows and the unbinding go with the row or not at all.
    async fn clear_overlay_entry(&self, domain: DomainId, actor: &str, path: &str) -> Result<bool>;

    // --- attachments ---------------------------------------------------------
    // Binary assets under a domain's `assets/` folder. The metadata row is the
    // same for both domain kinds; only the bytes differ in where they live, so
    // the blob pair below is exercised by virtual domains and left untouched by
    // file domains, whose bytes stay on disk.

    /// Insert or replace one file's metadata in `table`, keyed by
    /// `(domain, path)`. The sync walker calls this on every scan, so a
    /// rewritten file refreshes its row in place rather than adding a second
    /// one.
    async fn upsert_asset(
        &self,
        table: AssetTable,
        domain: DomainId,
        row: &AttachmentRow,
    ) -> Result<()>;

    /// Remove one file's metadata from `table` and any stored blob. Returns
    /// `false` when no row was there, which lets a scan tell a delete from a
    /// no-op.
    async fn delete_asset(&self, table: AssetTable, domain: DomainId, path: &str) -> Result<bool>;

    /// [`Store::upsert_asset`] on the engram attachments.
    async fn upsert_attachment(&self, domain: DomainId, row: &AttachmentRow) -> Result<()> {
        self.upsert_asset(AssetTable::Engram, domain, row).await
    }

    /// [`Store::delete_asset`] on the engram attachments.
    async fn delete_attachment(&self, domain: DomainId, path: &str) -> Result<bool> {
        self.delete_asset(AssetTable::Engram, domain, path).await
    }

    /// Store an attachment's bytes in the index. The metadata row must already
    /// exist, otherwise this is a [`crate::IndexError::Constraint`]: the row is
    /// what names the mime type and the size, so a blob without one would be an
    /// orphan nothing can serve. Only a virtual domain uses this.
    async fn write_attachment_blob(&self, domain: DomainId, path: &str, bytes: &[u8])
    -> Result<()>;

    // --- reflections ---------------------------------------------------------

    /// Insert or replace one reflection's row, keyed by `(domain, path)`.
    async fn upsert_reflection(&self, domain: DomainId, row: &ReflectionRow) -> Result<()>;

    /// Remove one reflection's row and its chunks. `false` when there was no
    /// row.
    async fn delete_reflection(&self, domain: DomainId, path: &str) -> Result<bool>;

    /// Replace one page's chunk rows with a freshly computed set, carrying over
    /// the vector of any chunk whose fingerprint is unchanged, so an edit
    /// re-embeds only the sections that changed. Called by
    /// [`crate::reflection::index_reflection_row`] only (R53).
    async fn replace_reflection_chunks(
        &self,
        domain: DomainId,
        path: &str,
        chunks: &[NewReflectionChunk],
    ) -> Result<()>;

    // --- host locks ----------------------------------------------------------
    // The single-writer-per-file-domain rule for shared-database collaboration.
    // Only file domains take a host lock; a virtual domain's concurrency is
    // engram-level compare-and-swap, so the daemon never claims one. All four
    // are backend-agnostic: an atomic upsert plus a re-read on Turso, the same
    // with `RETURNING` on Postgres, over a `domain_lock` table.

    /// Claim the host lock for a file domain. Inserts when unheld; takes over
    /// when `take_over` is set or the current holder's `heartbeat_at` is older
    /// than `stale_before`; otherwise leaves the existing holder untouched. Times
    /// (`now`, `stale_before`) are RFC 3339 strings the caller computes from its
    /// stale threshold, so the comparison stays a plain lexical one, matching the
    /// temporal columns. Returns [`HostClaim::Acquired`] when this instance holds
    /// the lock afterward, else [`HostClaim::HeldByOther`] naming the live holder.
    async fn claim_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        label: &str,
        now: &str,
        stale_before: &str,
        take_over: bool,
    ) -> Result<HostClaim>;

    /// Refresh this instance's heartbeat on a lock it holds, and its label
    /// unless `label` is empty, so every lock of one instance shows the one
    /// label it uses now. Returns `true` when
    /// a row was updated (this instance still holds it), `false` when it does not
    /// (another instance took over), which tells the daemon to stop hosting.
    async fn renew_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        label: &str,
        now: &str,
    ) -> Result<bool>;

    /// Release a lock this instance holds (graceful shutdown, or the end of a
    /// one-shot command's walk): a compare-and-delete on `token`, the one
    /// its claim returned ([`HostClaim::Acquired`]). A no-op when the row
    /// names another holder or another claim's token, so a released takeover
    /// never deletes the new host's lock, and a process of this machine
    /// never deletes the lock a later claim under the same id took (this
    /// machine's daemon after a command's claim, or a second command).
    /// `true` when the row was deleted.
    async fn release_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        token: &str,
    ) -> Result<bool>;

    /// Give the lock back to the claim it was taken over from under the same
    /// instance id: when the row still carries `token`, put `previous` back
    /// as its token, so that claim's own release still matches. A no-op when
    /// another claim replaced `token` meanwhile. `true` when it was put back.
    async fn hand_back_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        token: &str,
        previous: &str,
    ) -> Result<bool>;

    /// Take the lease of `loop_kind` (`reflect` or `evolve`) on `domain`: one
    /// atomic claim that lands when the lease is unheld or its last renewal
    /// is older than `stale_before`. Every take that lands writes a new token
    /// ([`LeaseClaim::Acquired`]); a lease that is still fresh is refused,
    /// the same instance's included, and named ([`LeaseClaim::Held`]). The
    /// two loops' leases never block each other.
    async fn take_automation_lease(
        &self,
        domain: DomainId,
        loop_kind: &str,
        holder: &LeaseHolder,
        now: &str,
        stale_before: &str,
    ) -> Result<LeaseClaim>;

    /// Move the lease's renewal time to `now`. `true` when the row still
    /// carries `token`, `false` when it is gone or another take replaced it.
    async fn renew_automation_lease(
        &self,
        domain: DomainId,
        loop_kind: &str,
        token: &str,
        now: &str,
    ) -> Result<bool>;

    /// Release the lease: a compare-and-delete on `token`. Returns the lease
    /// as it was when it was deleted, `None` when the row carries another
    /// token or there is none.
    async fn release_automation_lease(
        &self,
        domain: DomainId,
        loop_kind: &str,
        token: &str,
    ) -> Result<Option<AutomationLease>>;

    /// Record the pull request the run holding `token` shared in. `false`
    /// when the row is gone or carries another token.
    async fn set_automation_pr_url(
        &self,
        domain: DomainId,
        loop_kind: &str,
        token: &str,
        pr_url: &str,
    ) -> Result<bool>;

    /// Add `paths` to the written paths of every lease on `domain` (either
    /// loop) that `account` holds, whatever its age: a lease that went stale
    /// and that nobody took over is still its token's, so what its runner
    /// wrote meanwhile is kept. A path already noted is kept once. One
    /// statement per lease, so two writers on a shared database never lose
    /// each other's paths. Answers how many leases noted them.
    async fn note_automation_written(
        &self,
        domain: DomainId,
        account: &str,
        paths: &[String],
    ) -> Result<u64>;

    /// Replace the disclosures recorded on one page of `domain`'s
    /// Reflection with `records` (JSON text the engine reads), or drop them
    /// when `records` is `None`.
    async fn set_reflection_disclosures(
        &self,
        domain: DomainId,
        path: &str,
        records: Option<&str>,
    ) -> Result<()>;

    /// Record one run of `loop_kind` on `domain`, then keep only the newest
    /// `keep` runs of that domain and loop.
    async fn record_automation_run(
        &self,
        domain: DomainId,
        loop_kind: &str,
        run: &AutomationRun,
        keep: usize,
    ) -> Result<()>;

    /// Write the domain's host move only when its stored text still is
    /// `expected`, the text the caller read ([`ReadStore::host_move_text`]):
    /// `false` and nothing written when another writer changed it in
    /// between. The stored text is compared as stored, so a mark in an older
    /// or newer shape matches the read of it.
    async fn swap_host_move(
        &self,
        domain: DomainId,
        expected: Option<&str>,
        next: Option<&HostMove>,
    ) -> Result<bool>;

    /// Clear the domain's rebuild marker only when it still is the one
    /// stamped at `started` ([`Store::begin_rebuild`]): a marker another
    /// run stamped since stays.
    async fn end_rebuild_stamped(&self, domain: DomainId, started: &str) -> Result<()>;

    /// Record the instance that just acquired a file domain's host lock.
    async fn set_last_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        label: &str,
    ) -> Result<()>;

    /// Set or (with `None`) clear the domain's host move. Inside an open
    /// write transaction it commits or rolls back with it.
    async fn set_host_move(&self, domain: DomainId, host_move: Option<&HostMove>) -> Result<()>;

    // --- batch control -------------------------------------------------------
    // These let the sync engine wrap its whole write phase in one transaction
    // for speed. A backend without explicit transactions may implement them as
    // no-ops.

    /// Begin a write transaction.
    async fn begin(&self) -> Result<()>;

    /// Commit the current write transaction.
    async fn commit(&self) -> Result<()>;

    /// Roll back the current write transaction.
    async fn rollback(&self) -> Result<()>;
}

/// What a domain-scoped identifier names: an engram, or nothing plus the
/// permalink it probably meant.
#[derive(Debug, Clone, PartialEq)]
pub enum DomainLookup {
    /// The engram the identifier names.
    Found(EngramDescriptor),
    /// Nothing; `suggest` is the permalink the identifier's slug is, when an
    /// engram in the domain answers to it, for the caller's not-found message.
    Missing {
        /// The permalink to name in a hint. `None` when nothing matched.
        suggest: Option<String>,
    },
}

/// The one domain-scoped lookup every identifier-taking surface shares: the
/// permalink, then the title, both through [`ReadStore::find_engram`] and nothing
/// else. A file path is no identifier. A miss carries the permalink the
/// identifier's slug ([`crystalline_core::path_permalink`]) is, when the slug
/// differs from the identifier and an engram answers to it as its permalink,
/// so a caller who copied a file path (`guides/Agent Workflow Guide.md`) is
/// told the permalink to pass (`guides/agent-workflow-guide`). The hint is one
/// more indexed lookup, never a listing, and a title hit on the slug is no
/// hint: a title is not what the identifier spelled. A free function rather
/// than a trait method, so neither backend changes. Callers skip it for a
/// domain the reader may not see, which keeps a hidden domain's miss free of
/// hints.
pub async fn lookup_in_domain(
    store: &dyn ReadStore,
    dom: &str,
    identifier: &str,
) -> Result<DomainLookup> {
    if let Some(d) = store.find_engram(dom, identifier).await? {
        return Ok(DomainLookup::Found(d));
    }
    let slug = crystalline_core::path_permalink(identifier);
    let mut suggest = None;
    if !slug.is_empty()
        && slug != identifier
        && let Some(d) = store.find_engram(dom, &slug).await?
        && d.permalink == slug
    {
        suggest = Some(d.permalink);
    }
    Ok(DomainLookup::Missing { suggest })
}

#[cfg(test)]
mod salience_tests {
    use super::{DEFAULT_SALIENCE_WEIGHT, salience_prior};

    #[test]
    fn salience_prior_absent_is_neutral() {
        assert_eq!(salience_prior(None, 0.15), 0.0);
    }

    #[test]
    fn salience_prior_zero_or_negative_is_neutral() {
        assert_eq!(salience_prior(Some(0.0), 0.15), 0.0);
        assert_eq!(salience_prior(Some(-4.0), 0.15), 0.0);
    }

    #[test]
    fn salience_prior_max_is_weight() {
        assert!((salience_prior(Some(10.0), 0.15) - 0.15).abs() < 1e-9);
    }

    #[test]
    fn salience_prior_is_clamped_above_scale() {
        assert!((salience_prior(Some(50.0), 0.15) - 0.15).abs() < 1e-9);
    }

    #[test]
    fn salience_prior_is_monotonic_and_bounded() {
        let low = salience_prior(Some(3.0), 0.15);
        let high = salience_prior(Some(7.0), 0.15);
        assert!(low < high);
        assert!(high <= 0.15 + 1e-9);
    }

    #[test]
    fn salience_prior_zero_weight_disables() {
        assert_eq!(salience_prior(Some(9.0), 0.0), 0.0);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn default_weight_is_conservative() {
        assert!(DEFAULT_SALIENCE_WEIGHT > 0.0 && DEFAULT_SALIENCE_WEIGHT < 0.5);
    }
}

#[cfg(test)]
mod retired_tests {
    use super::{DEFAULT_RETIRED_WEIGHT, retired_factor};

    #[test]
    fn retired_factor_fades_all_four_retired_statuses() {
        for status in ["deprecated", "superseded", "archived", "legacy"] {
            assert_eq!(retired_factor(status, DEFAULT_RETIRED_WEIGHT), 0.6);
        }
    }

    #[test]
    fn retired_factor_keeps_full_rank_for_other_statuses() {
        for status in ["current", "draft", "idea", "proposed", "poc", "", "wip"] {
            assert_eq!(retired_factor(status, DEFAULT_RETIRED_WEIGHT), 1.0);
        }
    }

    #[test]
    fn retired_factor_is_case_sensitive_exact_match() {
        for status in ["Deprecated", "SUPERSEDED", " deprecated"] {
            assert_eq!(retired_factor(status, DEFAULT_RETIRED_WEIGHT), 1.0);
        }
    }

    #[test]
    fn retired_factor_clamps_weight_into_unit_range() {
        assert_eq!(retired_factor("deprecated", 1.5), 1.0);
        assert_eq!(retired_factor("deprecated", -0.5), 0.0);
    }

    #[test]
    fn retired_weight_one_is_a_no_op() {
        assert_eq!(retired_factor("superseded", 1.0), 1.0);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn default_retired_weight_is_a_soft_fade() {
        assert!(DEFAULT_RETIRED_WEIGHT > 0.0 && DEFAULT_RETIRED_WEIGHT < 1.0);
    }
}

#[cfg(test)]
mod meta_value_tests {
    use super::meta_value_rows;
    use crystalline_core::META_KEY_MAX_BYTES;
    use serde_json::json;

    #[test]
    fn rows_are_one_per_list_element_and_one_per_scalar() {
        let mut metadata = json!({
            "sources": ["b", "a", "b"],
            "n": 7,
            "nested": [[1], {"x": 1}, 2],
            "obj": {"x": 1},
        });
        metadata
            .as_object_mut()
            .unwrap()
            .insert("k".repeat(META_KEY_MAX_BYTES + 1), json!("v"));
        assert_eq!(
            meta_value_rows(&metadata),
            vec![
                ("n".to_string(), "7".to_string()),
                ("nested".to_string(), "2".to_string()),
                ("sources".to_string(), "\"a\"".to_string()),
                ("sources".to_string(), "\"b\"".to_string()),
            ]
        );
        assert!(meta_value_rows(&json!(null)).is_empty());
        assert!(meta_value_rows(&json!({})).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::{ChunkModelCount, ChunkTable, CoverageGroup, EmbeddingCoverage};

    /// Whatever holds the writer can hand it to code that only reads: the
    /// split costs no call site a second store.
    #[test]
    fn a_writer_coerces_to_a_reader() {
        fn reads(_: &dyn super::ReadStore) {}
        fn writer(store: &dyn super::Store) {
            reads(store);
        }
        let _ = writer;
        let _: fn(std::sync::Arc<dyn super::Store>) -> std::sync::Arc<dyn super::ReadStore> = |s| s;
    }

    fn group(model: &str, dims: usize, chunks: usize, embedded: usize) -> CoverageGroup {
        CoverageGroup {
            model: model.to_string(),
            dims,
            chunks,
            embedded,
        }
    }

    #[test]
    fn from_groups_orders_most_chunks_first_and_skips_groups_with_no_vectors() {
        let cov = EmbeddingCoverage::from_groups([
            group("", 0, 5, 0),
            group("small", 8, 2, 2),
            group("big", 8, 4, 3),
            group("ghost", 8, 6, 0),
            group("alpha", 4, 2, 2),
        ]);
        assert_eq!(cov.total_chunks, 19);
        assert_eq!(cov.embedded_chunks, 7);
        let models: Vec<(&str, usize, usize)> = cov
            .models
            .iter()
            .map(|m| (m.model.as_str(), m.dims, m.count))
            .collect();
        // Most embedded first, then model, then width; no group without vectors.
        assert_eq!(
            models,
            vec![("big", 8, 3), ("alpha", 4, 2), ("small", 8, 2)]
        );
    }

    #[test]
    fn from_groups_merges_the_same_model_and_width_across_groups() {
        let cov = EmbeddingCoverage::from_groups([group("m", 8, 3, 2), group("m", 8, 2, 1)]);
        assert_eq!(cov.total_chunks, 5);
        assert_eq!(
            cov.models,
            vec![ChunkModelCount {
                model: "m".into(),
                dims: 8,
                count: 3
            }]
        );
    }

    #[test]
    fn the_embed_passes_walk_the_engram_table_first() {
        assert_eq!(ChunkTable::ALL.first(), Some(&ChunkTable::Engram));
        let mut seen = std::collections::HashSet::new();
        assert!(
            ChunkTable::ALL.iter().all(|t| seen.insert(*t)),
            "each table once"
        );
    }
}
