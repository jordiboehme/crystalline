//! Near-duplicate tag cluster detection.
//!
//! Case folding (the tag interner lowercases every tag) collapses `Foo` and
//! `foo`, but it does not catch the other ways one concept ends up spelled two
//! ways: a separator swap (`multi-word` vs `multi_word` vs `multiword`) or a
//! plural (`deploy` vs `deploys`). This module groups the tags already in use
//! into clusters of likely-the-same-thing so the `vocabulary` tool and
//! `crystalline doctor` can surface them for a `crystalline tags merge`.
//!
//! A one-character edit is deliberately *not* a relation. It reads plausible as
//! a typo catcher and is destructive in practice: against a real archive it
//! joined unrelated concepts (`audio`/`audit`, `preference`/`reference`) and
//! numbered variants (`reactor-0`/`reactor-2`), and the merge each cluster
//! prescribes would have collapsed distinctions the archive relies on. Only
//! relations that are mechanically the same word survive here.
//!
//! Pure Rust with no store dependency: it works off the [`TagCount`] list the
//! vocabulary already computes.

use crate::store::{TagAlias, TagCount};

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
}

impl Reason {
    fn label(self) -> &'static str {
        match self {
            Reason::Separator => "separator variants",
            Reason::Plural => "plural variants",
        }
    }
}

/// Group the tags in use into near-duplicate clusters. Each returned cluster has
/// two or more members; a singleton (a tag with no near-duplicate) is omitted.
///
/// Two relations join a pair of tags, strongest first:
/// 1. **separator variants** - equal once `-`, `_` and spaces are dropped, so
///    `multi-word`, `multi_word`, `multi word` and `multiword` are one cluster;
/// 2. **plural variants** - one is the other plus a trailing `s` or `es`.
///
/// Both are mechanical rewrites of one word. A near-miss spelling is not a
/// relation: two tags one character apart stay two tags.
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

/// Near-duplicate tag clusters with declared aliases folded out first. Each tag
/// name is canonicalized one hop through the alias map (an `alias -> canonical`
/// mapping), the canonicalized names are deduplicated (summing usage) and
/// [`tag_clusters`] runs over the result. A cluster that existed only because two
/// spellings are a declared alias pair collapses onto one canonical name and
/// drops out, so only the near-duplicates an alias does not already explain are
/// surfaced, and every surviving member is reported under its canonical spelling.
/// With no aliases this is byte-identical to [`tag_clusters`].
pub fn tag_clusters_with_aliases(tags: &[TagCount], aliases: &[TagAlias]) -> Vec<TagCluster> {
    // One-hop `alias -> canonical` fold, first-wins on a duplicate alias to match
    // the MANIFEST fold. The canonical is never itself rewritten, so the hop is
    // single: `a -> b` folds `a` to `b` even when `b -> c` also exists.
    let mut canonical_of: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for a in aliases {
        canonical_of
            .entry(a.alias.as_str())
            .or_insert(a.canonical.as_str());
    }

    // Canonicalize each tag's name and merge duplicates, summing the usage so a
    // folded canonical carries the combined weight of its spellings. Insertion
    // order is preserved so an empty alias map reproduces the input exactly.
    let mut merged: std::collections::HashMap<String, TagCount> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for t in tags {
        let name = canonical_of
            .get(t.name.as_str())
            .copied()
            .unwrap_or(t.name.as_str())
            .to_string();
        match merged.get_mut(&name) {
            Some(existing) => {
                existing.engrams += t.engrams;
                existing.observations += t.observations;
            }
            None => {
                order.push(name.clone());
                merged.insert(
                    name.clone(),
                    TagCount {
                        name,
                        engrams: t.engrams,
                        observations: t.observations,
                    },
                );
            }
        }
    }

    let canonicalized: Vec<TagCount> = order
        .into_iter()
        .map(|k| merged.remove(&k).expect("every ordered key was inserted"))
        .collect();
    tag_clusters(&canonicalized)
}

/// The strongest relation joining two distinct tag names, or `None`.
fn relation(a: &str, b: &str) -> Option<Reason> {
    if separator_fold(a) == separator_fold(b) {
        return Some(Reason::Separator);
    }
    if is_plural_pair(a, b) {
        return Some(Reason::Plural);
    }
    None
}

/// Fold the separator class: `-`, `_` and spaces are dropped, so every way of
/// separating one name's words compares equal, including writing it with no
/// separator at all - `multi-word`, `multi_word`, `multi word` and `multiword`
/// are one name.
fn separator_fold(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
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

    fn ta(alias: &str, canonical: &str) -> TagAlias {
        TagAlias {
            alias: alias.to_string(),
            canonical: canonical.to_string(),
        }
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
    fn dropping_a_separator_is_a_separator_variant() {
        // Writing one name with no separator at all is the same drift as
        // swapping one separator for another, so `e-bike` and `ebike` cluster.
        // `bike` is a different name and stays out of it.
        let out = clusters(&["e-bike", "ebike", "bike"]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tags, vec!["e-bike".to_string(), "ebike".to_string()]);
        assert_eq!(out[0].reason, "separator variants");
    }

    #[test]
    fn one_edit_apart_is_not_a_relation() {
        // Regression pin: a one-character edit used to cluster, and against a
        // real archive every such cluster was wrong. These pairs are different
        // words, or numbered variants of one name, and merging either way
        // destroys a distinction the archive keeps on purpose.
        for pair in [
            ["audio", "audit"],
            ["preference", "reference"],
            ["reactor-0", "reactor-2"],
            ["database", "databse"],
        ] {
            let out = clusters(&pair);
            assert!(
                out.is_empty(),
                "{pair:?} are one edit apart and must not cluster: {out:?}"
            );
        }
    }

    #[test]
    fn union_find_chains_three_into_one_cluster_with_the_strongest_reason() {
        // multi_word -> multi-word (separator), multi-word -> multi-words
        // (plural): all three form one cluster, reported with the strongest
        // reason (separator).
        let out = clusters(&["multi-word", "multi_word", "multi-words"]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].tags,
            vec![
                "multi-word".to_string(),
                "multi-words".to_string(),
                "multi_word".to_string()
            ]
        );
        assert_eq!(
            out[0].reason, "separator variants",
            "the strongest joining reason wins"
        );
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

    #[test]
    fn aliases_suppress_the_pairs_they_explain() {
        let tags: Vec<TagCount> = ["deploy", "deploys", "multi-word", "multi_word"]
            .iter()
            .map(|n| tc(n))
            .collect();

        // Empty aliases: byte-identical to tag_clusters, both near-dup pairs
        // cluster (a plural and a separator swap).
        let plain = tag_clusters(&tags);
        assert_eq!(plain.len(), 2);
        assert_eq!(tag_clusters_with_aliases(&tags, &[]), plain);

        // deploys -> deploy is a declared alias: that pair canonicalizes onto the
        // single name `deploy`, collapses to a singleton and drops out. The
        // unrelated separator cluster survives untouched.
        let with = tag_clusters_with_aliases(&tags, &[ta("deploys", "deploy")]);
        assert_eq!(with.len(), 1);
        assert_eq!(
            with[0].tags,
            vec!["multi-word".to_string(), "multi_word".to_string()]
        );
        assert_eq!(with[0].reason, "separator variants");
    }

    #[test]
    fn surviving_members_are_reported_under_their_canonical_name() {
        // multi_word -> multi-word folds the alias out; `multi-word` and
        // `multi-words` then cluster as a plural pair, reported under the
        // canonical `multi-word`, never the aliased `multi_word`.
        let tags: Vec<TagCount> = ["multi_word", "multi-words"]
            .iter()
            .map(|n| tc(n))
            .collect();
        let with = tag_clusters_with_aliases(&tags, &[ta("multi_word", "multi-word")]);
        assert_eq!(with.len(), 1);
        assert_eq!(
            with[0].tags,
            vec!["multi-word".to_string(), "multi-words".to_string()]
        );
        assert_eq!(with[0].reason, "plural variants");
    }
}
