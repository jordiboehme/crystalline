//! Reflection-mode search: text, semantic and hybrid search over a domain's
//! reflection rows.
//!
//! A hit is one section, keyed `(path, section_ord)` across the lexical and
//! the vector half (R60). Lexical: every query term (lowercased, split on
//! whitespace) must occur in the section's text or the reflection's title;
//! the score is the number of term occurrences in the section plus 5 per term
//! the title holds, and at least one occurrence must be in the section. Text
//! mode ranks by that score, then path, then section order. Semantic and
//! hybrid read the closest chunk of each section ([`SectionVector`], R60) and
//! blend through `crate::rank` the way engram hybrid search does, without
//! salience or the retired fade. Id lines never reach a snippet:
//! `split_sections` leaves them out of the text. The snippet is the window
//! the engram search cuts (`crate::snippet`) when the lexical half matched,
//! else the section's lead, both taken from the section without its heading
//! line, or the top section without the page's `# Title` line (the hit
//! carries both on their own).
//!
//! It is also where a reflection row is written: [`index_reflection_row`]
//! puts the row and its chunks in the index together (R53).

use std::collections::HashMap;

use serde::Serialize;

use crate::embed::ChunkParams;
use crate::error::Result;
use crate::snippet::{lead, make_snippet};
use crate::store::{DomainId, Page, ReflectionRow, SearchMode, SectionVector, Store};
use crystalline_core::reflection::{Section, TOP_SECTION, split_sections};

/// One section that matched.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReflectionHit {
    /// The reflection's path inside `reflections/`.
    pub path: String,
    /// Its title.
    pub title: String,
    /// The section id, `_top` for the text before the first `##`, `None` for
    /// a heading with no id yet.
    pub section: Option<String>,
    /// The section heading, empty for the top.
    pub heading: String,
    /// The text around the first matching term, cut the way an engram hit's
    /// snippet is; the section's lead when only the vector half found it.
    pub snippet: String,
    /// The rank score: orders the hits, never part of an answer. Text mode
    /// keeps the raw lexical score; semantic is the similarity; hybrid the
    /// blend.
    #[serde(skip)]
    pub score: f64,
}

/// A section's key across the two halves: its page and its place on the page.
type SectionKey<'a> = (&'a str, usize);

/// Every section of every page, once, by its key.
type Sections<'a> = HashMap<SectionKey<'a>, (&'a ReflectionRow, Section)>;

/// Search `rows` for `query`, best first, at most `limit` hits: the text mode
/// of [`rank_sections`].
pub fn search_reflections(rows: &[ReflectionRow], query: &str, limit: usize) -> Vec<ReflectionHit> {
    rank_sections(rows, query, SearchMode::Text, &[], 0.0, limit, 1).items
}

/// Rank one domain's sections for `query` (R60). A hit is a section, keyed
/// `(path, section_ord)` across the two halves. `mode` is `Text`, `Semantic`
/// or `Hybrid`; any other mode ranks as `Text`. `vectors` is the vector half
/// ([`ReadStore::reflection_vector_hits`](crate::ReadStore::reflection_vector_hits)); a vector under `min_similarity` or for
/// a section no row holds any more is dropped. Ties break on path, then
/// section order.
pub fn rank_sections(
    rows: &[ReflectionRow],
    query: &str,
    mode: SearchMode,
    vectors: &[SectionVector],
    min_similarity: f64,
    limit: usize,
    page: usize,
) -> Page<ReflectionHit> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let limit = limit.max(1);
    let page = page.max(1);
    let mut sections: Sections = HashMap::new();
    for row in rows {
        for (ord, section) in split_sections(&row.content).into_iter().enumerate() {
            sections.insert((row.path.as_str(), ord), (row, section));
        }
    }
    let lexical = match mode {
        SearchMode::Semantic => HashMap::new(),
        _ => lexical_scores(&sections, &terms),
    };
    let semantic = match mode {
        SearchMode::Semantic | SearchMode::Hybrid => {
            semantic_scores(&sections, vectors, min_similarity)
        }
        _ => HashMap::new(),
    };
    let best = lexical.values().copied().fold(0.0_f64, f64::max);
    let mut keys: Vec<SectionKey> = lexical.keys().chain(semantic.keys()).copied().collect();
    keys.sort();
    keys.dedup();
    let mut ranked: Vec<(f64, SectionKey)> = keys
        .into_iter()
        .map(|k| {
            let text = lexical.get(&k).copied();
            let sem = semantic.get(&k).copied();
            let score = match mode {
                SearchMode::Hybrid => {
                    crate::rank::blend(text.map(|t| crate::rank::normalized(t, best)), sem)
                }
                SearchMode::Semantic => sem.unwrap_or(0.0),
                _ => text.unwrap_or(0.0),
            };
            (score, k)
        })
        .collect();
    ranked.sort_by(|(a, ak), (b, bk)| {
        b.partial_cmp(a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(ak.cmp(bk))
    });
    let total = ranked.len();
    let items = ranked
        .into_iter()
        .skip((page - 1).saturating_mul(limit))
        .take(limit)
        .map(|(score, k)| {
            let (row, section) = &sections[&k];
            let body = snippet_text(section);
            let snippet = if lexical.contains_key(&k) {
                make_snippet(body, &terms)
            } else {
                lead(body)
            };
            hit(row, section, snippet, score)
        })
        .collect();
    Page {
        items,
        page,
        limit,
        total,
    }
}

/// The lexical half: Task 8's rule and score per section. Every term in the
/// section text or the title, at least one occurrence in the section, scored
/// `body + 5 * title terms`. No terms, no lexical hit.
fn lexical_scores<'a>(sections: &Sections<'a>, terms: &[String]) -> HashMap<SectionKey<'a>, f64> {
    let mut scores = HashMap::new();
    if terms.is_empty() {
        return scores;
    }
    for (key, (row, section)) in sections {
        let title = row.title.to_lowercase();
        let text = section.text.to_lowercase();
        if !terms
            .iter()
            .all(|t| text.contains(t.as_str()) || title.contains(t.as_str()))
        {
            continue;
        }
        let body: usize = terms.iter().map(|t| text.matches(t.as_str()).count()).sum();
        if body == 0 {
            continue;
        }
        let bonus = terms.iter().filter(|t| title.contains(t.as_str())).count() * 5;
        scores.insert(*key, (body + bonus) as f64);
    }
    scores
}

/// The vector half: the similarity of each section a vector names that is
/// still on its page, at or above the floor, clamped to `[0, 1]` as the engram
/// hybrid blend reads it.
fn semantic_scores<'a>(
    sections: &Sections<'a>,
    vectors: &'a [SectionVector],
    min_similarity: f64,
) -> HashMap<SectionKey<'a>, f64> {
    vectors
        .iter()
        .filter(|v| v.similarity >= min_similarity)
        .filter_map(|v| {
            sections
                .get_key_value(&(v.path.as_str(), v.section_ord))
                .map(|(k, _)| (*k, v.similarity.clamp(0.0, 1.0)))
        })
        .collect()
}

/// The text a section's snippet is cut from: the section without the lines
/// the hit already carries. A `##` section drops its heading line, empty
/// heading or not (the hit carries `heading`); the top section drops the
/// page's `# Title` line when that is its first line with text (the hit
/// carries `title`). The lexical score reads the whole text, so heading and
/// title words still count.
fn snippet_text(section: &Section) -> &str {
    if section.id.as_deref() != Some(TOP_SECTION) {
        return section.text.split_once('\n').map_or("", |(_, body)| body);
    }
    let mut text = section.text.as_str();
    loop {
        let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
        if first.trim().is_empty() && !rest.is_empty() {
            text = rest;
            continue;
        }
        return match crystalline_core::parse::parse_heading(first) {
            Some((1, _)) => rest,
            _ => &section.text,
        };
    }
}

/// One ranked section as an answer row.
fn hit(row: &ReflectionRow, section: &Section, snippet: String, score: f64) -> ReflectionHit {
    ReflectionHit {
        path: row.path.clone(),
        title: row.title.clone(),
        section: section.id.clone(),
        heading: section.heading.clone(),
        snippet,
        score,
    }
}

/// Put one reflection's row and its chunks in the index (R53): the one
/// writer of reflection rows, called by the sync and by the engine, so a page
/// never has a row without the chunks of that same text.
///
/// Writes the row and its chunks in two statements and opens no transaction
/// of its own (turso's `Conn` takes no savepoint). Every production caller holds one:
/// `Engine::index_reflection` begins and commits around it, and the sync's
/// `reconcile_rows` runs inside the apply's transaction. A row written
/// without its chunks would never be re-chunked, because the prefilter trusts
/// its stamp.
pub async fn index_reflection_row<S: Store + ?Sized>(
    store: &S,
    domain: DomainId,
    row: &ReflectionRow,
    params: &ChunkParams,
) -> Result<()> {
    store.upsert_reflection(domain, row).await?;
    let chunks = crate::embed::chunk_reflection(&row.title, &row.content, params);
    store
        .replace_reflection_chunks(domain, &row.path, &chunks)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, title: &str, content: &str) -> ReflectionRow {
        ReflectionRow {
            path: path.into(),
            title: title.into(),
            content: content.into(),
            sha256: String::new(),
            size: 0,
            modified: String::new(),
        }
    }

    #[test]
    fn hits_are_sections_ranked_by_matches_and_title() {
        let rows = vec![
            row(
                "guides/deploying.md",
                "Deploying",
                "# Deploying\n\n## Night deploys\n<!-- r:aaaa1111 -->\n\nDeploys run at night. Night is quiet.\n\n## Rollback\n<!-- r:bbbb2222 -->\n\nRoll back with the script.\n",
            ),
            row(
                "howto/night.md",
                "Night shifts",
                "# Night shifts\n\nWho is on call at night.\n",
            ),
        ];
        let hits = search_reflections(&rows, "Night", 10);
        let got: Vec<(&str, Option<&str>)> = hits
            .iter()
            .map(|h| (h.path.as_str(), h.section.as_deref()))
            .collect();
        // howto/night.md: two body matches plus the title bonus (7); the
        // deploying section: three body matches and no title bonus (3).
        assert_eq!(
            got,
            [
                ("howto/night.md", Some("_top")),
                ("guides/deploying.md", Some("aaaa1111"))
            ]
        );
        assert!(hits[1].snippet.contains("Deploys run at night"));
        assert!(!hits[1].snippet.contains("<!-- r:"), "ids never show");
        assert!(
            search_reflections(&rows, "night script", 10).is_empty(),
            "every term in one section"
        );
        assert_eq!(
            search_reflections(&rows, "roll back", 10)[0].heading,
            "Rollback"
        );
        let best = search_reflections(&rows, "night", 1);
        assert_eq!(best.len(), 1);
        assert_eq!(best[0].path, "howto/night.md", "the cut keeps the best hit");
        assert!(search_reflections(&rows, "   ", 10).is_empty());
    }

    #[test]
    fn a_title_alone_makes_no_hit() {
        let rows = vec![row(
            "oncall.md",
            "Night shifts",
            "Who is on call.\n\n## Pager\n<!-- r:cccc3333 -->\n\nThe pager rings.\n",
        )];
        assert!(
            search_reflections(&rows, "night", 10).is_empty(),
            "a section the title alone matches is no hit"
        );
        let hits = search_reflections(&rows, "night pager", 10);
        let got: Vec<(Option<&str>, f64)> = hits
            .iter()
            .map(|h| (h.section.as_deref(), h.score))
            .collect();
        // The title carries `night` (5); `pager` is in the heading line and
        // the body (2), the heading being part of the section text.
        assert_eq!(got, [(Some("cccc3333"), 7.0)]);
    }

    fn deploy_rows() -> Vec<ReflectionRow> {
        vec![
            row(
                "guides/deploying.md",
                "Deploying",
                "# Deploying\n\n## Night deploys\n<!-- r:aaaa1111 -->\n\nDeploys run at night. Night after night, the night shift watches.\n\n## Rollback\n<!-- r:bbbb2222 -->\n\nRoll back with the script.\n",
            ),
            row(
                "howto/oncall.md",
                "On call",
                "# On call\n\n## Pager\n<!-- r:cccc3333 -->\n\nThe pager rings at night. Night calls are loud, night after night.\n\n## Handover\n<!-- r:dddd4444 -->\n\nHand over before night, and again at night.\n",
            ),
        ]
    }

    fn v(path: &str, ord: usize, similarity: f64) -> SectionVector {
        SectionVector {
            path: path.into(),
            section_ord: ord,
            similarity,
        }
    }

    #[test]
    fn semantic_ranks_sections_by_their_closest_chunk_and_cuts_at_the_floor() {
        let rows = deploy_rows();
        let vectors = [
            v("howto/oncall.md", 1, 0.91),
            v("guides/deploying.md", 2, 0.83),
            v("guides/deploying.md", 1, 0.70),
        ];
        let page = rank_sections(&rows, "pager", SearchMode::Semantic, &vectors, 0.78, 10, 1);
        let got: Vec<(&str, Option<&str>)> = page
            .items
            .iter()
            .map(|h| (h.path.as_str(), h.section.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("howto/oncall.md", Some("cccc3333")),
                ("guides/deploying.md", Some("bbbb2222"))
            ]
        );
        assert_eq!(page.total, 2, "the 0.70 section is under the floor");
        assert_eq!(
            page.items[1].snippet, "Roll back with the script.",
            "no lexical match: the section's lead, without its heading line"
        );
    }

    /// "night" occurs 5 times in the night deploys section, 4 times in the
    /// pager section and twice in the handover section; the vector half saw
    /// the pager section (0.95) and the handover section (0.80). The blend
    /// lifts the corroborated pager section (0.4 * 0.8 + 0.6 * 0.95 = 0.89)
    /// over the stronger text-only one (5/5 * 0.85 = 0.85), and keeps the weak
    /// corroborated handover section (0.4 * 0.4 + 0.6 * 0.8 = 0.64) under it.
    /// Adding the raw scores, adding the normalized ones, taking the larger
    /// one or a penalty of 1.0 each reorders these three.
    #[test]
    fn hybrid_lifts_a_section_both_halves_found() {
        let rows = deploy_rows();
        let vectors = [v("howto/oncall.md", 1, 0.95), v("howto/oncall.md", 2, 0.80)];
        let page = rank_sections(&rows, "night", SearchMode::Hybrid, &vectors, 0.78, 10, 1);
        let got: Vec<(&str, Option<&str>)> = page
            .items
            .iter()
            .map(|h| (h.path.as_str(), h.section.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("howto/oncall.md", Some("cccc3333")),
                ("guides/deploying.md", Some("aaaa1111")),
                ("howto/oncall.md", Some("dddd4444")),
            ]
        );
        // The scores, not just the order: swapping the two blend weights
        // (0.6 text, 0.4 semantic) gives the pager section 0.86, which still
        // ranks first.
        let scores: Vec<f64> = page.items.iter().map(|h| h.score).collect();
        for (got, want) in scores.iter().zip([0.89, 0.85, 0.64]) {
            assert!((got - want).abs() < 1e-9, "{scores:?}");
        }
        assert!(
            page.items[0].snippet.contains("night"),
            "the lexical half matched: the snippet is cut there"
        );
        let paged = rank_sections(&rows, "night", SearchMode::Hybrid, &vectors, 0.78, 1, 2);
        assert_eq!(
            (paged.total, paged.items.len(), paged.items[0].path.as_str()),
            (3, 1, "guides/deploying.md")
        );
    }

    /// The hit carries the heading, so no snippet repeats the `## heading`
    /// line, in any mode; a term only the heading holds still finds the
    /// section, and its snippet is then the section's lead.
    #[test]
    fn no_snippet_repeats_the_heading_line() {
        let rows = deploy_rows();
        for mode in [SearchMode::Text, SearchMode::Hybrid, SearchMode::Semantic] {
            let vectors = [v("guides/deploying.md", 2, 0.9)];
            let page = rank_sections(&rows, "rollback", mode, &vectors, 0.78, 10, 1);
            let hit = &page.items[0];
            assert_eq!(hit.heading, "Rollback", "{mode:?}");
            assert_eq!(hit.snippet, "Roll back with the script.", "{mode:?}");
        }
        let page = rank_sections(&rows, "night deploys", SearchMode::Text, &[], 0.0, 10, 1);
        assert!(
            page.items.iter().all(|h| !h.snippet.contains("##")),
            "{:?}",
            page.items
        );
    }

    /// The top section's snippet does not start with the page's `# Title`
    /// line, which the hit carries as `title`; the title words still count
    /// for the score. A top section that is only the title line has an empty
    /// snippet.
    #[test]
    fn the_top_snippet_leaves_out_the_title_line() {
        let rows = vec![
            row(
                "night.md",
                "Night shifts",
                "\n# Night shifts\n\nWho is on call at night.\n\n## Pager\n<!-- r:cccc3333 -->\n\nThe pager.\n",
            ),
            row("bare.md", "Bare night", "# Bare night\n"),
        ];
        for mode in [SearchMode::Text, SearchMode::Hybrid, SearchMode::Semantic] {
            let vectors = [v("night.md", 0, 0.9), v("bare.md", 0, 0.85)];
            let page = rank_sections(&rows, "night", mode, &vectors, 0.78, 10, 1);
            let snippets: Vec<(&str, &str)> = page
                .items
                .iter()
                .map(|h| (h.path.as_str(), h.snippet.as_str()))
                .collect();
            assert_eq!(
                snippets,
                [("night.md", "Who is on call at night."), ("bare.md", "")],
                "{mode:?}"
            );
        }
        // Night shifts: one body match in the title line, one below it, and
        // the title bonus (7): the title line still counts.
        assert_eq!(search_reflections(&rows, "night", 10)[0].score, 7.0);
    }

    /// An empty `## ` heading is a section heading all the same: its line
    /// never reaches the snippet, though the heading text is empty.
    #[test]
    fn an_empty_heading_line_never_reaches_the_snippet() {
        let rows = vec![row(
            "a.md",
            "A",
            "# A\n\n## \n<!-- r:dddd4444 -->\n\nThe pager rings.\n",
        )];
        for mode in [SearchMode::Text, SearchMode::Hybrid, SearchMode::Semantic] {
            let page = rank_sections(&rows, "pager", mode, &[v("a.md", 1, 0.9)], 0.78, 10, 1);
            let hit = &page.items[0];
            assert_eq!(
                (
                    hit.section.as_deref(),
                    hit.heading.as_str(),
                    hit.snippet.as_str()
                ),
                (Some("dddd4444"), "", "The pager rings."),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_vector_for_a_section_that_is_gone_is_skipped() {
        let rows = deploy_rows();
        let page = rank_sections(
            &rows,
            "x",
            SearchMode::Semantic,
            &[v("guides/deploying.md", 9, 0.99), v("gone.md", 0, 0.99)],
            0.78,
            10,
            1,
        );
        assert!(page.items.is_empty());
    }

    /// Two id-less sections may share a heading (R60): the key is the
    /// section's place on the page, so each is its own hit, once.
    #[test]
    fn two_sections_under_one_heading_are_two_hits() {
        let rows = vec![row(
            "notes.md",
            "Notes",
            "# Notes\n\n## Notes\n\nfirst pager note\n\n## Notes\n\nsecond pager note\n",
        )];
        for mode in [SearchMode::Text, SearchMode::Hybrid] {
            let page = rank_sections(&rows, "pager", mode, &[v("notes.md", 2, 0.9)], 0.78, 10, 1);
            let mut snippets: Vec<&str> = page.items.iter().map(|h| h.snippet.as_str()).collect();
            snippets.sort();
            assert_eq!(page.total, 2, "{mode:?}");
            assert!(
                snippets[0].contains("first") && snippets[1].contains("second"),
                "{mode:?}: {snippets:?}"
            );
        }
    }

    #[test]
    fn the_vector_statement_keeps_the_engram_top_k() {
        let limit = format!("LIMIT {}", crate::rank::SEMANTIC_TOPK);
        assert!(crate::turso::REFLECTION_VECTOR_SQL.ends_with(&limit));
        #[cfg(feature = "postgres")]
        assert!(crate::postgres::REFLECTION_VECTOR_SQL.ends_with(&limit));
    }

    #[test]
    fn equal_scores_rank_by_path_then_page_order() {
        let rows = vec![
            row(
                "b.md",
                "B",
                "Alpha.\n\n## One\n<!-- r:11111111 -->\n\nalpha\n\n## Two\n<!-- r:22222222 -->\n\nalpha\n",
            ),
            row("a.md", "A", "## Zed\n<!-- r:33333333 -->\n\nalpha\n"),
        ];
        let got: Vec<(String, Option<String>)> = search_reflections(&rows, "alpha", 10)
            .into_iter()
            .map(|h| (h.path, h.section))
            .collect();
        let want: Vec<(String, Option<String>)> = [
            ("a.md", "33333333"),
            ("b.md", "_top"),
            ("b.md", "11111111"),
            ("b.md", "22222222"),
        ]
        .iter()
        .map(|(p, s)| (p.to_string(), Some(s.to_string())))
        .collect();
        assert_eq!(got, want);
    }
}
