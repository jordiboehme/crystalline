//! The hot statements and the plans they are entitled to.
//!
//! One place, eighteen entries, each named by the function that issues it, so a
//! rewrite that drops an index fails with the function's name rather than with
//! a diff. Every entry obtains its SQL the way the code obtains it - a shared
//! builder, a named constant or the same `format!` the method calls - because a
//! registry holding a second copy of a statement is a registry that can be
//! right about SQL nobody runs.
//!
//! The turso leg asserts `EXPLAIN QUERY PLAN`. The postgres leg asserts
//! `EXPLAIN (FORMAT JSON)` with `enable_seqscan` off, behind
//! `CRYSTALLINE_TEST_POSTGRES_URL`, like the parity suite. Neither is a timing
//! test: the scale run stays the timing check.
//!
//! **What the turso spike settled** (step 1 of the task that wrote this file,
//! run against turso_core 0.7.2 on a synced domain; recorded here rather than
//! in a report because two of the four answers are load bearing for the code
//! below).
//!
//! (a) Every shape in the registry produces plan rows. A compound `UNION ALL`
//! select comes back as `COMPOUND QUERY` / `LEFT-MOST SUBQUERY` / `UNION ALL`
//! markers with each arm's own rows between them, and an `UPDATE` comes back as
//! one row per table it reads.
//!
//! (b) The spellings these eighteen statements actually produce are `SEARCH
//! <alias> USING INDEX <name> (<cols>)`, `SEARCH <alias> USING INTEGER PRIMARY
//! KEY (rowid=?)`, `SCAN <table> AS <alias> USING INDEX <name>`, `MULTI-INDEX
//! OR <alias> (<idx>, <idx>)` and, once the indexes are dropped, a bare `SCAN
//! <table>`; plus the markers that read no table - `USE SORTER FOR ORDER BY`,
//! `USE HASH TABLE FOR DISTINCT`, `CORRELATED SCALAR SUBQUERY <n>`, `COMPOUND
//! QUERY`, `LEFT-MOST SUBQUERY`, `UNION ALL` and `SCAN CONSTANT ROW`. `USING
//! COVERING INDEX` is in the crate's vocabulary and none of these produce it.
//!
//! Two of those decided [`read_of`], which is why it is not a substring test on
//! `USING INDEX`. `SCAN <table> USING INDEX <name>` is a FULL pass that happens
//! to walk an index, not a seek into one, so it counts as a scan and needs a
//! reason. And with every engram index dropped an aliased join came back as
//! `SEARCH e USING INDEX ephemeral_engram_t1 (domain_id=? AND actor=?)`: an
//! ephemeral index is one turso BUILT for the query by reading the whole table,
//! so counting it would make the red-first leg green on a schema with no indexes
//! at all.
//!
//! (c) `DROP INDEX` is accepted on an open store, unique indexes included, so
//! the red-first leg takes indexes away for real rather than stopping a
//! migration list early.
//!
//! (d) The output is byte-stable across consecutive runs of the same fixture.
//!
//! The decision, on that evidence: plan assertions, not the index inventory the
//! spec named as a fallback.

use crystalline_index::SearchOrder;
use crystalline_index::{DomainId, DomainKind, EmbeddingRow, Store, TursoStore, sync_domain};

// --- the registry ------------------------------------------------------------

/// One statement this project cannot afford to have planned as a table scan.
pub struct HotStatement {
    /// `module::function`, spelled as the source spells it. This is what a
    /// failure names.
    pub issued_by: &'static str,
    /// The turso text, obtained the same way the turso code obtains it.
    pub turso: fn() -> String,
    /// The postgres text, obtained the same way the postgres code obtains it.
    pub postgres: fn() -> String,
    /// SQL literals for `?1`/`$1` onward, in order.
    ///
    /// The plan asserted is the custom plan for representative literals, which
    /// is the plan postgres builds for the first executions of a prepared
    /// statement and so the plan a real read gets; a generic plan can differ and
    /// this file does not speak about it. The literals match the fixture: the
    /// seeded domain id, a permalink that exists, the model that embedded.
    pub literals: &'static [&'static str],
    /// A postgres override for `literals`, `None` where the two dialects spell
    /// the same value the same way. Exactly one entry needs it: the semantic
    /// query vector is a blob in turso and a `vector` literal in postgres.
    pub literals_pg: Option<&'static [&'static str]>,
    /// Tables whose full pass IS the intended plan on TURSO, each with the
    /// reason. Empty for all but one; a non-empty one is a claim a reviewer has
    /// to read, and the reason is written from an observed plan, never guessed.
    pub scan_expected: &'static [(&'static str, &'static str)],
    /// The same for POSTGRES, and `None` where the two backends are entitled to
    /// the same permissions.
    ///
    /// A permission has to name its backend, because a permission a backend
    /// does not spend is a blanket one. Entry 6 is the case: turso genuinely
    /// cannot do better than a full pass over `chunk`, and postgres reaches the
    /// same rows through `idx_chunk_model`. Sharing one list would mean that a
    /// migration dropping or narrowing that index turned every semantic search
    /// on postgres into a sequential scan of the chunk table with this test
    /// still green - the statement the registry most wants to watch would be
    /// the one it stopped watching.
    pub scan_expected_pg: Option<&'static [(&'static str, &'static str)]>,
    /// Indexes the turso plan must name in full, where the entry's claim is
    /// about WHICH index serves it rather than merely that one does.
    pub turso_must_seek: &'static [&'static str],
    /// The same for postgres, and deliberately not the same list.
    ///
    /// The two planners do not pick the same index for the same statement, and
    /// measured on this fixture they mostly do not: postgres reaches the folder
    /// derivation through `idx_engram_domain` rather than `idx_engram_path_actor`,
    /// and the two dangling arms through `idx_relation_to` / `idx_link_to`
    /// rather than the partial unresolved pair. Those are the turso design's
    /// claims, so they are asserted where they are claims; copying them here
    /// would pin a planner's mood as if it were a design. What IS a claim on
    /// postgres is the index that stands between entry 6 and a sequential scan
    /// of `chunk`, which is the other half of the fix for the same finding.
    pub postgres_must_seek: &'static [&'static str],
}

/// The tables a scan of is a performance bug unless `scan_expected` says
/// otherwise.
///
/// `domain` is deliberately not here: it holds one row per registered domain,
/// it is reached through a unique index anyway, and guarding it would turn every
/// fixture into a question about a backend's small-table heuristics rather than
/// about our indexes.
pub const GUARDED_TABLES: &[&str] = &["engram", "chunk", "relation", "link"];

/// Which table each alias in these statements stands for.
///
/// Turso names the ALIAS in a plan line (`SEARCH e USING ...`), not the table,
/// so without this a guarded table would hide behind every one-letter alias in
/// the codebase. One map for all eighteen entries, because the aliases are used
/// consistently across both backends; a name absent here stands for itself.
const ALIASES: &[(&str, &str)] = &[
    ("e", "engram"),
    ("src", "engram"),
    ("dst", "engram"),
    ("tgt", "engram"),
    ("c", "chunk"),
    ("r", "relation"),
    ("l", "link"),
    ("d", "domain"),
];

fn table_of(name: &str) -> &str {
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, table)| *table)
        .unwrap_or(name)
}

/// Substitute an entry's literals for its bind placeholders, both dialects.
///
/// Descending by index, which is the whole trick: ascending, the pass for `?1`
/// rewrites the `?1` inside `?10` and every placeholder above nine is
/// corrupted. The two spellings are replaced in one pass each because a
/// statement is in one dialect or the other, never both, and replacing the
/// absent spelling is a no-op.
fn bind_literals(sql: &str, literals: &[&str]) -> String {
    let mut out = sql.to_string();
    for (i, literal) in literals.iter().enumerate().rev() {
        let n = i + 1;
        out = out.replace(&format!("?{n}"), literal);
        out = out.replace(&format!("${n}"), literal);
    }
    out
}

/// The base screen every entry passes where the code passes a composed one.
///
/// The actor arm of a composed screen is bounded by one actor's own row count,
/// which is what an overlay is; the base arm is the one that has to be a seek.
const BASE_SCREEN: &str = "e.actor = ''";

/// The semantic query vector, one spelling per dialect. Eight components, the
/// width the fixture embeds at: a postgres `vector` literal of any other width
/// fails the statement on a type error rather than on a plan, which would be a
/// green this file has no business giving.
const QVEC_TURSO: &str = "x'00'";
const QVEC_PG: &str = "'[0,0,0,0,0,0,0,0]'::vector";

/// One filter-only page, once per order a reader can ask for: the listing the
/// domain page and every folder view page through, scoped to one domain. Every
/// order sorts, bounded by the `LIMIT` in the same statement, and reaches
/// `engram` through the domain filter rather than by walking the table.
///
/// A macro rather than a table of `(label, order)` read by one constructor
/// because `HotStatement` holds plain `fn() -> String` pointers: a closure that
/// read its order out of a table would capture it and stop coercing to one,
/// while an order pasted in here is a path expression and the closure stays
/// non-capturing. So this is that one table, with the four rows at the call
/// site and the single shape here.
macro_rules! filter_only_entry {
    ($label:expr, $order:expr) => {
        HotStatement {
            issued_by: $label,
            turso: || {
                crystalline_index::turso::filter_only_sql(
                    BASE_SCREEN,
                    "AND d.name IN (?1)",
                    $order,
                    50,
                    0,
                )
            },
            postgres: || {
                crystalline_index::postgres::filter_only_sql(
                    BASE_SCREEN,
                    "AND d.name IN ($1)",
                    $order,
                    50,
                    0,
                )
            },
            literals: &["'d'"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        }
    };
}

pub fn registry() -> Vec<HotStatement> {
    vec![
        HotStatement {
            issued_by: "Store::file_stamps",
            turso: || crystalline_index::turso::FILE_STAMPS_SQL.to_string(),
            postgres: || crystalline_index::postgres::FILE_STAMPS_SQL.to_string(),
            literals: &["1"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        HotStatement {
            // The one entry whose tie-break differs from `reference_match`'s:
            // this ties by path (`COLLATE "C"` on postgres) where the resolve
            // pass ties by `e.id`, so where two engrams in a domain share a
            // title the oldest answers a reference and the byte-first answers a
            // lookup. Deliberate, and left as it is: making them one key would
            // cost this lookup the index its path ordering is served from.
            issued_by: "Store::find_engram",
            turso: || crystalline_index::turso::FIND_ENGRAM_SQL.to_string(),
            postgres: || crystalline_index::postgres::FIND_ENGRAM_SQL.to_string(),
            literals: &["'d'", "'p0'"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "Store::list_engrams",
            turso: || crystalline_index::turso::list_engrams_sql("d.name=?1"),
            postgres: || crystalline_index::postgres::list_engrams_sql("d.name=$1"),
            literals: &["'d'"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        HotStatement {
            // The folder derivation, which used to be guarded by a hand-copied
            // literal in `turso_only.rs`. `idx_engram_path_actor` is named in
            // full and not as a substring: `idx_engram_path` was the v1 index
            // this one replaced, a substring test passes on either, and the
            // distinction between them is the whole claim. The claim itself is
            // on `browse_level`: no body is read to learn a folder exists,
            // which holds exactly while that index covers this on its
            // `(domain_id, path)` prefix.
            issued_by: "Store::browse_level (folder derivation)",
            turso: || crystalline_index::turso::folder_level_sql("substr(e.path, 1)", "d.name=?1"),
            postgres: || {
                crystalline_index::postgres::folder_level_sql("substr(e.path, 1)", "d.name=$1")
            },
            literals: &["'d'"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &["idx_engram_path_actor"],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "search::scored_lexical",
            turso: || {
                crystalline_index::turso::lexical_candidate_sql(
                    BASE_SCREEN,
                    "AND d.name IN (?1)",
                    500,
                )
            },
            postgres: || {
                crystalline_index::postgres::lexical_candidate_sql(
                    BASE_SCREEN,
                    "AND d.name IN ($1)",
                    500,
                )
            },
            literals: &["'d'"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        filter_only_entry!(
            "search::filter_only (recorded, newest first)",
            SearchOrder::RecordedDesc
        ),
        filter_only_entry!(
            "search::filter_only (recorded, oldest first)",
            SearchOrder::RecordedAsc
        ),
        filter_only_entry!("search::filter_only (path, A to Z)", SearchOrder::PathAsc),
        filter_only_entry!("search::filter_only (path, Z to A)", SearchOrder::PathDesc),
        HotStatement {
            issued_by: "search::semantic_phase1_sql",
            turso: || {
                crystalline_index::turso::semantic_phase1_sql(BASE_SCREEN, "AND c.model = ?2")
            },
            postgres: || {
                crystalline_index::postgres::semantic_phase1_sql(BASE_SCREEN, "AND c.model = $2")
            },
            literals: &[QVEC_TURSO, "'fake'"],
            literals_pg: Some(&[QVEC_PG, "'fake'"]),
            // The only permission in the registry, written from the plans as
            // observed rather than guessed, and granted on turso ALONE. This is
            // the exact distance scan - one row per engram by min(distance)
            // over every chunk - and an exact GROUP BY cannot read a
            // nearest-neighbour index: the HNSW index postgres carries is
            // present for a future top-k rewrite, and turso has no vector index
            // at all. So turso spends it (`SCAN chunk AS c USING INDEX
            // idx_chunk_engram`, a full pass).
            //
            // Postgres spends its permission on the OTHER table, which is
            // exactly why the two lists cannot be one. With the model predicate
            // to work from it reaches `chunk` through a bitmap scan of
            // `idx_chunk_model` - the only thing standing between every
            // semantic search and a sequential pass over the whole chunk table,
            // so `chunk` is NOT permitted there and that index is named in full
            // below. What postgres does pass over is `engram`: an unscoped
            // semantic query filters `engram` on nothing at all, so there is
            // nothing there to seek and every chunk of the model has to meet
            // its parent row. Postgres walks `idx_engram_domain` and applies
            // the actor screen as a filter; turso reaches the same rows from
            // the other end, one primary-key seek per chunk. A full pass by
            // construction rather than by a missing index, and bounded by the
            // engram count rather than the chunk count.
            //
            // The top-k rewrite the first paragraph anticipates would likely
            // replace `idx_chunk_model` with a composite - and then this entry
            // is where that is said out loud, which is the point of naming an
            // index rather than settling for "some index".
            scan_expected: &[(
                "chunk",
                "the exact distance scan, which no nearest-neighbour index can serve",
            )],
            scan_expected_pg: Some(&[(
                "engram",
                "an unscoped semantic query filters `engram` on nothing, so every chunk \
                 of the model meets its parent row; bounded by the engram count",
            )]),
            turso_must_seek: &[],
            postgres_must_seek: &["idx_chunk_model"],
        },
        HotStatement {
            issued_by: "search::semantic_hydrate_sql",
            turso: || crystalline_index::turso::semantic_hydrate_sql(BASE_SCREEN, "1,2,3"),
            postgres: || crystalline_index::postgres::semantic_hydrate_sql(BASE_SCREEN, "1,2,3"),
            literals: &[],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "Store::lead_vectors",
            turso: || crystalline_index::turso::lead_vectors_sql(BASE_SCREEN),
            postgres: || crystalline_index::postgres::lead_vectors_sql(BASE_SCREEN, 0),
            literals: &["1", "'fake'"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "Store::resolve_pending_relations",
            turso: || crystalline_index::resolve_pending_sql("relation", "?1"),
            postgres: || crystalline_index::resolve_pending_sql("relation", "$1"),
            literals: &["1"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            // The driving index only: this pins that the pass itself stays
            // bounded by the partial unresolved index. Which index each of the
            // four COALESCE arms seeks is asked by name in
            // `the_reference_resolve_pass_seeks_the_title_index`
            // (`src/turso/mod.rs`), which reads this same builder: a statement
            // can lose the title index to a rewrite that still has it seeking
            // something, and only that guard notices.
            turso_must_seek: &["idx_relation_unresolved"],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "Store::resolve_pending_links",
            turso: || crystalline_index::resolve_pending_sql("link", "?1"),
            postgres: || crystalline_index::resolve_pending_sql("link", "$1"),
            literals: &["1"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &["idx_link_unresolved"],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "Store::unresolved_refs",
            turso: || {
                crystalline_index::turso::unresolved_refs_sql(
                    BASE_SCREEN,
                    "r.to_id IS NULL",
                    "l.to_id IS NULL",
                )
            },
            postgres: || {
                crystalline_index::postgres::unresolved_refs_sql(
                    BASE_SCREEN,
                    "r.to_id IS NULL",
                    "l.to_id IS NULL",
                )
            },
            literals: &["1"],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            // The pair of partial indexes exists for exactly these two arms, so
            // this entry names both rather than settling for any index at all.
            turso_must_seek: &["idx_relation_unresolved", "idx_link_unresolved"],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "search::neighbors (relation frontier)",
            turso: || {
                crystalline_index::relation_frontier_sql(
                    "1,2",
                    "src.actor = ''",
                    "dst.actor = ''",
                    "",
                )
            },
            postgres: || {
                crystalline_index::relation_frontier_sql(
                    "1,2",
                    "src.actor = ''",
                    "dst.actor = ''",
                    "",
                )
            },
            literals: &[],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "search::neighbors (link frontier)",
            turso: || {
                crystalline_index::link_frontier_sql("1,2", "src.actor = ''", "dst.actor = ''", "")
            },
            postgres: || {
                crystalline_index::link_frontier_sql("1,2", "src.actor = ''", "dst.actor = ''", "")
            },
            literals: &[],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        HotStatement {
            issued_by: "search::neighbors (node hydrate)",
            turso: || crystalline_index::turso::node_hydrate_sql(BASE_SCREEN, "1,2"),
            postgres: || crystalline_index::postgres::node_hydrate_sql(BASE_SCREEN, "1,2"),
            literals: &[],
            literals_pg: None,
            scan_expected: &[],
            scan_expected_pg: None,
            turso_must_seek: &[],
            postgres_must_seek: &[],
        },
        // The contradiction scorer wave (plans/2026-09-14-contradiction-scorer-plan.md)
        // adds two per-domain reads, and both belong here the day they land:
        //
        //   Store::contradiction_pairs_scored
        //     SELECT ... FROM contradiction_pair WHERE domain_id=?1 AND model=?2
        //   Store::contradictions
        //     SELECT ... FROM contradiction c JOIN contradiction_pair p
        //     ON p.engram_a=c.engram_a AND p.engram_b=c.engram_b AND p.model=c.model
        //     WHERE p.domain_id=?1 AND c.model=?2 AND c.score>=?3
        //
        // That wave's Task 1 writes both statements and their migrations. Its
        // last step adds two entries here, named by those two functions, adds
        // `contradiction` and `contradiction_pair` to GUARDED_TABLES, and
        // extends `seed` with scored pairs so the planner has statistics for
        // them. Nothing else about this file changes.
    ]
}

// --- the fixture -------------------------------------------------------------

/// How wide the fixture's embeddings are. See [`QVEC_PG`].
const FIXTURE_DIMS: usize = 8;

/// How many engrams each fixture domain holds.
///
/// A planner that has never seen a row plans everything as a scan, so a
/// statistics-free fixture would make the whole guard vacuous. A few hundred is
/// enough for that: with `enable_seqscan = off` a usable index wins at any table
/// size, so this number buys real statistics rather than a cost threshold, and
/// the postgres leg is run three times.
const FIXTURE_ENGRAMS: usize = 200;

fn engram_file(i: usize, target: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: Engram {i}\npermalink: p{i}\ntags:\n  - t\nstatus: stable\n\
         recorded_at: 2026-01-01\nsalience: 3\n---\n\n# Engram {i}\n\n\
         body of engram {i}, which points at [[{target}]]\n\n- relates_to [[{target}]]\n"
    )
}

/// Seed two domains of engrams with relations, links and one embedded chunk
/// each, and return the first domain's id.
///
/// Two domains so `d.name` is selective, relations and links so the frontier and
/// the resolve pass have rows, one dangling reference per domain so the
/// unresolved read has rows, and one embedded chunk per engram at `seq=0` so
/// `lead_vectors` and the semantic phase-1 scan have rows. Written through
/// `sync_domain` and the `Store` trait, so one body seeds both backends.
async fn seed(store: &dyn Store) -> DomainId {
    let mut first = None;
    for domain_name in ["d", "other"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        for i in 0..FIXTURE_ENGRAMS {
            // Every engram points at the next one, so every relation and link
            // row resolves and the graph frontier has edges to walk. The last
            // one points at nothing, so the dangling read has a row too.
            let target = if i + 1 == FIXTURE_ENGRAMS {
                "nowhere".to_string()
            } else {
                format!("p{}", i + 1)
            };
            std::fs::write(
                dir.path().join(format!("notes/e{i}.md")),
                engram_file(i, &target),
            )
            .unwrap();
        }
        sync_domain(store, domain_name, dir.path()).await.unwrap();
        let domain = store
            .upsert_domain(
                domain_name,
                Some(&dir.path().to_string_lossy()),
                DomainKind::File,
            )
            .await
            .unwrap();
        if first.is_none() {
            first = Some(domain);
        }
    }

    // One embedding per chunk, at the fixture's width, under the model the
    // registry's literals name. `sync_domain` wrote the chunk rows.
    let jobs = store
        .chunks_needing_embedding("fake", None, FIXTURE_ENGRAMS * 8, None)
        .await
        .unwrap();
    assert!(
        !jobs.is_empty(),
        "the fixture seeded no chunks, so the vector reads would be planned over nothing"
    );
    let batch: Vec<EmbeddingRow> = jobs
        .iter()
        .map(|job| EmbeddingRow {
            chunk_id: job.chunk_id,
            embedding: vec![0.5f32; FIXTURE_DIMS],
            dims: FIXTURE_DIMS,
        })
        .collect();
    store.store_embeddings(&batch, "fake").await.unwrap();

    let domain = first.expect("two domains were seeded");
    // The literals say `1`, and a statement planned against a domain with no
    // rows is exactly the vacuous green this file exists to prevent, so the
    // assumption about the id sequence fails here rather than passing in a plan.
    assert_eq!(
        domain.0, 1,
        "the registry's literals name domain 1; this fixture seeded {}",
        domain.0
    );
    domain
}

// --- the turso leg -----------------------------------------------------------

/// What a turso plan line reads, as `(the table or alias, is it a seek)`, and
/// `None` for a line that reads no table at all.
///
/// The three cases the eighteen statements produce, and the reasoning the module
/// doc records the measurement for:
///
/// - `SEARCH <name> USING ...` is a seek, unless the index it names is an
///   `ephemeral_` one turso built for this query by reading the whole table.
/// - `MULTI-INDEX OR <name> (<idx>, <idx>)` is a seek through two indexes at
///   once, and carries neither `SEARCH` nor the substring `USING INDEX`, which
///   is why it is a case of its own.
/// - `SCAN <name> ...` is a full pass, and `SCAN <name> USING INDEX <idx>` is
///   still a full pass: it walks every entry of the index rather than seeking
///   into it, so it needs a `scan_expected` reason exactly as a bare scan does.
fn read_of(line: &str) -> Option<(&str, bool)> {
    if let Some(rest) = line.strip_prefix("SEARCH ") {
        let seek = !line.contains("USING INDEX ephemeral_")
            && (line.contains("USING INDEX") || line.contains("USING INTEGER PRIMARY KEY"));
        return Some((rest.split_whitespace().next()?, seek));
    }
    if let Some(rest) = line.strip_prefix("MULTI-INDEX OR ") {
        return Some((rest.split_whitespace().next()?, true));
    }
    if let Some(rest) = line.strip_prefix("SCAN ") {
        return Some((rest.split_whitespace().next()?, false));
    }
    None
}

async fn turso_fixture() -> TursoStore {
    let store = TursoStore::open_in_memory().await.unwrap();
    seed(&store).await;
    store
}

/// Every hot statement reaches its rows through an index, on turso.
///
/// Any read of a guarded table that is not an index seek - see [`read_of`] - is
/// the failure. See the registry's `scan_expected` for the one entry where the
/// full pass is the design, and `turso_must_seek` for the entries whose claim is
/// about WHICH index serves them.
#[tokio::test]
async fn every_hot_statement_is_index_served_on_turso() {
    let store = turso_fixture().await;

    for entry in registry() {
        let sql = bind_literals(&(entry.turso)(), entry.literals);
        let plan = store.explain_query_plan(&sql).await.unwrap_or_else(|e| {
            panic!("{} did not explain: {e}. Statement: {sql}", entry.issued_by)
        });
        assert!(
            !plan.is_empty(),
            "{} produced no plan rows at all: {sql}",
            entry.issued_by
        );
        for line in &plan {
            let Some((name, seek)) = read_of(line) else {
                continue;
            };
            let table = table_of(name);
            if seek || !GUARDED_TABLES.contains(&table) {
                continue;
            }
            assert!(
                entry.scan_expected.iter().any(|(t, _)| *t == table),
                "{} reads `{table}` without seeking an index: {line}. Whole plan: {plan:?}",
                entry.issued_by
            );
        }
        let joined = plan.join(" | ");
        for index in entry.turso_must_seek {
            assert!(
                joined.contains(index),
                "{} must be served by {index} by name, plan was: {joined}",
                entry.issued_by
            );
        }
    }
}

/// The lexical candidate scan stays bounded, whatever it is ordered from.
///
/// Its own builder documents the property: unscoped it is answered from rowid
/// order and opens no sorter, and scoped to a domain it does sort, held down by
/// the `LIMIT` in the same statement. So the claim is not "never sorts" - that
/// would be false, and pinning it as true would mean pinning a lie - it is
/// "never sorts without a bound", which is what stands between this projection
/// and the bodies of a whole domain.
#[tokio::test]
async fn the_lexical_candidate_scan_is_never_sorted_unbounded() {
    let store = turso_fixture().await;
    let entry = registry()
        .into_iter()
        .find(|e| e.issued_by == "search::scored_lexical")
        .expect("scored_lexical is in the registry");
    let sql = bind_literals(&(entry.turso)(), entry.literals);
    let plan = store.explain_query_plan(&sql).await.unwrap().join(" | ");
    assert!(
        !plan.contains("SORTER") || sql.contains(" LIMIT "),
        "the candidate scan sorted with no bound: {plan}"
    );
    assert!(
        !plan.contains("GROUP BY"),
        "the candidate scan grew a GROUP BY, which unbounds the sorter: {plan}"
    );
}

/// The filter-only page sorts by design, in every order, and is bounded by
/// the LIMIT in the same statement: the property that keeps one page of
/// bodies in the sorter rather than a domain's worth.
///
/// What is asserted here is the keys, one spelling per order, because the
/// bound itself is pinned from the source by
/// `a_body_projection_never_reaches_an_unbounded_sorter` in
/// `tests/turso_only.rs`, which reads the `LIMIT` off the same line as the
/// `ORDER BY`. Asserting it again on a string this builder always emits would
/// pin the builder's own `format!` rather than a property. What is left for
/// this test is what turso does with those keys: no `GROUP BY`, which is the
/// one thing that would take the bounded-sorter optimization away, and rows
/// reached through an index rather than by walking the table.
#[tokio::test]
async fn the_filter_only_page_is_sorted_and_bounded_in_every_order() {
    let store = turso_fixture().await;
    let expected: &[(&str, &str)] = &[
        (
            "search::filter_only (recorded, newest first)",
            "ORDER BY e.recorded_at IS NULL, e.recorded_at DESC, e.path ASC",
        ),
        (
            "search::filter_only (recorded, oldest first)",
            "ORDER BY e.recorded_at IS NULL, e.recorded_at ASC, e.path ASC",
        ),
        ("search::filter_only (path, A to Z)", "ORDER BY e.path ASC"),
        ("search::filter_only (path, Z to A)", "ORDER BY e.path DESC"),
    ];
    let entries: Vec<HotStatement> = registry()
        .into_iter()
        .filter(|e| e.issued_by.starts_with("search::filter_only"))
        .collect();
    assert_eq!(entries.len(), expected.len(), "one entry per order");
    for entry in entries {
        let keys = expected
            .iter()
            .find(|(label, _)| *label == entry.issued_by)
            .unwrap_or_else(|| panic!("{} is an order with no expected keys", entry.issued_by))
            .1;
        let sql = bind_literals(&(entry.turso)(), entry.literals);
        assert!(
            sql.contains(keys),
            "{} orders by `{keys}`: {sql}",
            entry.issued_by
        );
        assert!(
            !sql.contains("GROUP BY"),
            "{} groups, which unbounds the sorter: {sql}",
            entry.issued_by
        );
        let plan = store.explain_query_plan(&sql).await.unwrap().join(" | ");
        assert!(
            plan.contains("SEARCH"),
            "{} reaches its rows through an index: {plan}",
            entry.issued_by
        );
    }
}

/// The turso guard fails when an index it depends on is gone.
///
/// Without this, a plan test can only ever prove that today's schema is fine; it
/// cannot prove that it would notice tomorrow's schema not being. A scratch
/// in-memory store, every engram index dropped, one statement, and the assertion
/// the green run makes has to turn over.
#[tokio::test]
async fn a_dropped_index_is_caught_by_the_turso_plan_guard() {
    let store = turso_fixture().await;
    for index in [
        "idx_engram_domain",
        "idx_engram_path_actor",
        "idx_engram_permalink_actor",
        "idx_engram_title_lower",
    ] {
        store.drop_index(index).await.unwrap();
    }

    let entry = registry()
        .into_iter()
        .find(|e| e.issued_by == "Store::file_stamps")
        .expect("file_stamps is in the registry");
    let sql = bind_literals(&(entry.turso)(), entry.literals);
    let plan = store.explain_query_plan(&sql).await.unwrap();
    let scanned = plan
        .iter()
        .filter_map(|line| read_of(line))
        .any(|(name, seek)| !seek && table_of(name) == "engram");
    assert!(
        scanned,
        "with every engram index dropped the plan must be a bare scan, so the green run \
         means something: {plan:?}"
    );
}

// --- the postgres leg --------------------------------------------------------

#[cfg(feature = "postgres")]
mod postgres_plans {
    use super::*;
    use crystalline_index::PostgresStore;
    use serde_json::Value;

    /// Mirrored from `tests/store.rs`, like every other standalone suite here:
    /// lifting the helpers into a shared module would touch five suites for one
    /// test.
    fn pg_url() -> Option<String> {
        use std::sync::Once;
        static NOTE: Once = Once::new();
        match std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") {
            Ok(u) if !u.is_empty() => Some(u),
            _ => {
                NOTE.call_once(|| {
                    eprintln!(
                        "note: skipping the postgres plan leg (CRYSTALLINE_TEST_POSTGRES_URL is unset)"
                    )
                });
                None
            }
        }
    }

    /// A distinct schema name per test invocation. The pid keeps runs apart, the
    /// counter keeps tests within a run apart, and the hash salt keeps a
    /// recycled pid from adopting a schema a panicking run left behind.
    fn unique_schema() -> String {
        use std::hash::{BuildHasher, RandomState};
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            "cp_{}_{}_{:x}",
            std::process::id(),
            n,
            RandomState::new().hash_one(n)
        )
    }

    /// Every node in the plan tree, flattened.
    fn nodes(plan: &Value, out: &mut Vec<Value>) {
        match plan {
            Value::Array(items) => {
                for item in items {
                    nodes(item, out);
                }
            }
            Value::Object(map) => {
                if map.contains_key("Node Type") {
                    out.push(plan.clone());
                }
                for value in map.values() {
                    nodes(value, out);
                }
            }
            _ => {}
        }
    }

    /// Every read of a guarded table that is NOT an index seek, as
    /// `(table, the node)`.
    ///
    /// Two shapes, and the second is the postgres twin of the one `read_of`
    /// rejects on turso.
    ///
    /// `Seq Scan` is the obvious one. The other is an `Index Scan` or `Index
    /// Only Scan` that carries a `Filter` and no `Index Cond`: postgres walks
    /// every entry of the index and tests the predicate on each row, which is a
    /// full pass that happens to be spelled as an index read - exactly what
    /// `SCAN t USING INDEX i` is on turso. `enable_seqscan = off` does not make
    /// that shape rarer, it makes it commoner, because a penalised sequential
    /// scan is what the planner trades it against, and a statement carrying
    /// `ORDER BY e.id` over an indexed column is the shape it reaches for. That
    /// is `lexical_candidate_sql`, which is the statement whose bound is its
    /// whole claim.
    ///
    /// `Bitmap Heap Scan` is deliberately not here: it always carries a
    /// `Recheck Cond` and is driven by a `Bitmap Index Scan` beneath it, so it
    /// can never be the shape in question.
    fn unseeked_reads(plan: &Value) -> Vec<(String, String)> {
        let mut all = Vec::new();
        nodes(plan, &mut all);
        all.iter()
            .filter(|node| {
                let kind = node["Node Type"].as_str().unwrap_or_default();
                match kind {
                    "Seq Scan" => true,
                    "Index Scan" | "Index Only Scan" => {
                        node.get("Index Cond").is_none() && node.get("Filter").is_some()
                    }
                    _ => false,
                }
            })
            .filter_map(|node| {
                node["Relation Name"]
                    .as_str()
                    .map(|r| (r.to_string(), node.to_string()))
            })
            .filter(|(table, _)| GUARDED_TABLES.contains(&table.as_str()))
            .collect()
    }

    /// Every index this plan reads by name.
    fn index_names(plan: &Value) -> Vec<String> {
        let mut all = Vec::new();
        nodes(plan, &mut all);
        all.iter()
            .filter_map(|node| node["Index Name"].as_str().map(str::to_string))
            .collect()
    }

    fn has_node(plan: &Value, node_type: &str) -> bool {
        let mut all = Vec::new();
        nodes(plan, &mut all);
        all.iter().any(|node| node["Node Type"] == node_type)
    }

    async fn open_seeded(url: &str, schema: &str) -> PostgresStore {
        let store = PostgresStore::open_in_schema(url, schema).await.unwrap();
        seed(&store).await;
        store.analyze().await.unwrap();
        store
    }

    /// Every hot statement reaches its rows through an index, on postgres.
    ///
    /// `enable_seqscan = off` (inside `explain_json`) is what makes this
    /// meaningful on a fixture: without it a statement with no usable index is
    /// planned as a scan that happens to be cheap over a few hundred rows, and
    /// the test passes on exactly the corpus size nobody has a problem with.
    /// With it, a statement no index can serve is planned as a scan anyway - the
    /// setting is a cost penalty, not a prohibition - and that is the signal.
    #[tokio::test]
    async fn every_hot_statement_is_index_served_on_postgres() {
        let Some(url) = pg_url() else { return };
        let schema = unique_schema();
        let store = open_seeded(&url, &schema).await;

        for entry in registry() {
            let sql = bind_literals(
                &(entry.postgres)(),
                entry.literals_pg.unwrap_or(entry.literals),
            );
            let plan = store.explain_json(&sql).await.unwrap_or_else(|e| {
                panic!("{} did not explain: {e}. Statement: {sql}", entry.issued_by)
            });
            let permitted = entry.scan_expected_pg.unwrap_or(entry.scan_expected);
            for (table, node) in unseeked_reads(&plan) {
                assert!(
                    permitted.iter().any(|(t, _)| *t == table),
                    "{} reads `{table}` without seeking an index, with enable_seqscan \
                     off, so no index covers its predicate. Node: {node}. \
                     Statement: {sql}",
                    entry.issued_by
                );
            }
            let seen = index_names(&plan);
            for index in entry.postgres_must_seek {
                assert!(
                    seen.iter().any(|name| name == index),
                    "{} must be served by {index} by name; the plan read {seen:?}. \
                     Statement: {sql}",
                    entry.issued_by
                );
            }
            // The lexical prefilter's real claim, the same one the turso leg
            // makes: bounded whenever it is sorted.
            if entry.issued_by == "search::scored_lexical"
                || entry.issued_by.starts_with("search::filter_only")
            {
                assert!(
                    !has_node(&plan, "Sort") || has_node(&plan, "Limit"),
                    "{} sorts a body projection with no bound: {plan}",
                    entry.issued_by
                );
            }
        }
        store.drop_schema().await.unwrap();
    }

    /// The postgres guard fails when an index it depends on is gone.
    ///
    /// The twin of the turso red, kept for the same reason: a plan test nobody
    /// has seen fail is a plan test nobody knows the teeth of.
    #[tokio::test]
    async fn a_dropped_index_is_caught_by_the_postgres_plan_guard() {
        let Some(url) = pg_url() else { return };
        let schema = unique_schema();
        let store = open_seeded(&url, &schema).await;
        // All four, not three: `idx_engram_title_lower` is
        // `(domain_id, lower(title))`, so its leading column serves
        // `domain_id=$1` on its own and leaving it in place left the plan a
        // bitmap scan through it rather than the sequential scan this red is
        // about. Measured, not assumed.
        for index in [
            "idx_engram_domain",
            "idx_engram_path_actor",
            "idx_engram_permalink_actor",
            "idx_engram_title_lower",
        ] {
            store.drop_index(index).await.unwrap();
        }
        store.analyze().await.unwrap();

        let entry = registry()
            .into_iter()
            .find(|e| e.issued_by == "Store::file_stamps")
            .expect("file_stamps is in the registry");
        let sql = bind_literals(
            &(entry.postgres)(),
            entry.literals_pg.unwrap_or(entry.literals),
        );
        let plan = store.explain_json(&sql).await.unwrap();
        assert!(
            unseeked_reads(&plan).iter().any(|(t, _)| t == "engram"),
            "with every engram index dropped the plan must be a scan, so the green run \
             means something: {plan}"
        );
        store.drop_schema().await.unwrap();
    }
}
