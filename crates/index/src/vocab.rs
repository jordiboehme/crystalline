//! Near-duplicate tag cluster detection.
//!
//! Case folding (the tag interner lowercases every tag) collapses `Foo` and
//! `foo`, but it does not catch the other ways one concept ends up spelled two
//! ways: a separator swap (`multi-word` vs `multi_word`), a plural (`deploy` vs
//! `deploys`) or a one-character typo (`database` vs `databse`). This module
//! groups the tags already in use into clusters of likely-the-same-thing so the
//! `vocabulary` tool and `crystalline doctor` can surface them for a
//! `crystalline tags merge`.
//!
//! Pure Rust with no store dependency: it works off the [`TagCount`] list the
//! vocabulary already computes.

use crate::store::TagCount;

/// A group of tags that look like variants of one another, with a short reason.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TagCluster {
    /// The clustered tag names, sorted.
    pub tags: Vec<String>,
    /// Why they clustered: the strongest relation that joined them.
    pub reason: String,
}

/// The relation kinds, strongest first. A lower value wins when one cluster is
/// joined by edges of more than one kind.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Reason {
    Separator = 0,
    Plural = 1,
    Edit = 2,
}

impl Reason {
    fn label(self) -> &'static str {
        match self {
            Reason::Separator => "separator variants",
            Reason::Plural => "plural variants",
            Reason::Edit => "one-character edit",
        }
    }
}

/// Group the tags in use into near-duplicate clusters. Each returned cluster has
/// two or more members; a singleton (a tag with no near-duplicate) is omitted.
///
/// Three relations join a pair of tags, strongest first:
/// 1. **separator variants** - equal once `_` and spaces are folded to `-`, so
///    `multi-word`, `multi_word` and `multi word` are one cluster;
/// 2. **plural variants** - one is the other plus a trailing `s` or `es`;
/// 3. **one-character edit** - Levenshtein distance 1, only when the longer name
///    is at least five characters so short tags are not over-clustered.
///
/// Pairs are unioned, so a chain (`a`-`b`, `b`-`c`) forms one cluster, and each
/// cluster reports the strongest relation that joined any of its members.
pub fn tag_clusters(tags: &[TagCount]) -> Vec<TagCluster> {
    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    let n = names.len();
    let mut uf = UnionFind::new(n);
    // The strongest reason seen on any edge inside each component, keyed by the
    // representative at merge time and reconciled after all unions.
    let mut edges: Vec<(usize, usize, Reason)> = Vec::new();

    for i in 0..n {
        for j in (i + 1)..n {
            if let Some(reason) = relation(names[i], names[j]) {
                uf.union(i, j);
                edges.push((i, j, reason));
            }
        }
    }

    // Fold every edge's reason into its final component root.
    let mut best: std::collections::HashMap<usize, Reason> = std::collections::HashMap::new();
    for (i, _, reason) in &edges {
        let root = uf.find(*i);
        best.entry(root)
            .and_modify(|r| {
                if reason < r {
                    *r = *reason;
                }
            })
            .or_insert(*reason);
    }

    // Collect the members of each multi-member component.
    let mut groups: std::collections::HashMap<usize, Vec<String>> =
        std::collections::HashMap::new();
    for (idx, name) in names.iter().enumerate() {
        let root = uf.find(idx);
        if best.contains_key(&root) {
            groups.entry(root).or_default().push(name.to_string());
        }
    }

    let mut clusters: Vec<TagCluster> = groups
        .into_iter()
        .map(|(root, mut members)| {
            members.sort();
            TagCluster {
                tags: members,
                reason: best[&root].label().to_string(),
            }
        })
        .collect();
    // Deterministic order: by first (sorted) member.
    clusters.sort_by(|a, b| a.tags.cmp(&b.tags));
    clusters
}

/// The strongest relation joining two distinct tag names, or `None`.
fn relation(a: &str, b: &str) -> Option<Reason> {
    if separator_fold(a) == separator_fold(b) {
        return Some(Reason::Separator);
    }
    if is_plural_pair(a, b) {
        return Some(Reason::Plural);
    }
    if a.chars().count().max(b.chars().count()) >= 5 && levenshtein_is_one(a, b) {
        return Some(Reason::Edit);
    }
    None
}

/// Fold the separator class: `_` and spaces become `-`, so the three separators
/// are interchangeable but a name with no separator is left untouched.
fn separator_fold(s: &str) -> String {
    s.chars()
        .map(|c| if c == '_' || c == ' ' { '-' } else { c })
        .collect()
}

/// Whether one name is the other plus a trailing `s` or `es`.
fn is_plural_pair(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short == long {
        return false;
    }
    long == format!("{short}s") || long == format!("{short}es")
}

/// Whether the Levenshtein distance between two strings is exactly one. Cheap:
/// a length gap above one already rules it out, and the single shared scan stops
/// at the second difference.
fn levenshtein_is_one(a: &str, b: &str) -> bool {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let (la, lb) = (ac.len(), bc.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    if la == lb {
        // Exactly one substitution.
        let diffs = ac.iter().zip(&bc).filter(|(x, y)| x != y).count();
        return diffs == 1;
    }
    // Lengths differ by one: exactly one insertion/deletion. Walk both, allowing
    // a single skip in the longer string.
    let (short, long) = if la < lb { (&ac, &bc) } else { (&bc, &ac) };
    let mut i = 0;
    let mut j = 0;
    let mut skipped = false;
    while i < short.len() && j < long.len() {
        if short[i] == long[j] {
            i += 1;
            j += 1;
        } else if skipped {
            return false;
        } else {
            skipped = true;
            j += 1;
        }
    }
    true
}

/// A minimal union-find over tag indices.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> UnionFind {
        UnionFind {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            // Keep the lower index as root for deterministic behavior.
            let (root, child) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[child] = root;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(name: &str) -> TagCount {
        TagCount {
            name: name.to_string(),
            engrams: 1,
            observations: 0,
        }
    }

    fn clusters(names: &[&str]) -> Vec<TagCluster> {
        let tags: Vec<TagCount> = names.iter().map(|n| tc(n)).collect();
        tag_clusters(&tags)
    }

    #[test]
    fn separator_variants_cluster() {
        let out = clusters(&["multi-word", "multi_word", "multi word", "unrelated"]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].tags,
            vec![
                "multi word".to_string(),
                "multi-word".to_string(),
                "multi_word".to_string()
            ]
        );
        assert_eq!(out[0].reason, "separator variants");
    }

    #[test]
    fn plural_variants_cluster() {
        let out = clusters(&["deploy", "deploys", "box", "boxes"]);
        assert_eq!(out.len(), 2);
        // Sorted by first member: box/boxes then deploy/deploys.
        assert_eq!(out[0].tags, vec!["box".to_string(), "boxes".to_string()]);
        assert_eq!(out[0].reason, "plural variants");
        assert_eq!(
            out[1].tags,
            vec!["deploy".to_string(), "deploys".to_string()]
        );
        assert_eq!(out[1].reason, "plural variants");
    }

    #[test]
    fn one_edit_clusters_only_when_long_enough() {
        // database/databse differ by one and are long: they cluster.
        let long = clusters(&["database", "databse"]);
        assert_eq!(long.len(), 1);
        assert_eq!(long[0].reason, "one-character edit");

        // api/apo differ by one but are short (< 5): no cluster.
        let short = clusters(&["api", "apo"]);
        assert!(short.is_empty(), "short one-edit pairs are not clustered");
    }

    #[test]
    fn union_find_chains_three_into_one_cluster() {
        // databases -> database (plural), database -> databse (edit): all three
        // form one cluster, reported with the strongest reason (plural).
        let out = clusters(&["database", "databases", "databse"]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].tags,
            vec![
                "database".to_string(),
                "databases".to_string(),
                "databse".to_string()
            ]
        );
        assert_eq!(
            out[0].reason, "plural variants",
            "the strongest joining reason wins"
        );
    }

    #[test]
    fn strongest_reason_wins_separator_over_edit() {
        // multi-word/multi_word are both separator variants (strongest) and one
        // edit apart; separator wins.
        let out = clusters(&["multi-word", "multi_word"]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reason, "separator variants");
    }

    #[test]
    fn unrelated_tags_do_not_cluster() {
        let out = clusters(&["alpha", "bravo", "charlie"]);
        assert!(out.is_empty());
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(tag_clusters(&[]).is_empty());
    }
}
