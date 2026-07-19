//! Query-time tag alias expansion.
//!
//! A domain's MANIFEST declares `old -> canonical` tag aliases; sync folds each
//! declaration to a lowercase `(alias, canonical)` pair and stores it in the
//! derived `tag_alias` table (see [`crate::sync::refresh_tag_aliases`]). At
//! search time those pairs are loaded into an [`AliasMap`], which expands one
//! requested tag into its equivalence class so a filter on any spelling matches
//! every engram tagged with a sibling spelling.
//!
//! The expansion is a single hop, never a transitive chain. For `a -> b` and
//! `b -> c`, expanding `a` reaches `a` and `b` (and `b`'s other aliases) but
//! never `c`: a chain is a MANIFEST authoring problem the M1 lint flags, and the
//! index folds each declared target exactly once rather than following it on.
//!
//! Pure Rust with no store dependency: it works off the folded pairs the store
//! hands back.

use std::collections::{BTreeSet, HashMap};

use crate::store::SearchQuery;

/// An equivalence-class expander over a domain's folded `(alias, canonical)`
/// tag pairs. Both directions are indexed so a filter on either the alias or the
/// canonical name expands to the whole class.
#[derive(Debug, Default)]
pub struct AliasMap {
    /// alias -> the canonicals it maps to. Multi-valued because an all-domain
    /// sweep can union one alias onto two different canonicals.
    forward: HashMap<String, Vec<String>>,
    /// canonical -> the aliases that map onto it.
    reverse: HashMap<String, Vec<String>>,
}

impl AliasMap {
    /// Build the map from folded `(alias, canonical)` pairs. Duplicate pairs are
    /// absorbed; every pair is indexed in both directions.
    pub fn from_pairs(pairs: &[(String, String)]) -> AliasMap {
        let mut forward: HashMap<String, Vec<String>> = HashMap::new();
        let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
        for (alias, canonical) in pairs {
            let f = forward.entry(alias.clone()).or_default();
            if !f.contains(canonical) {
                f.push(canonical.clone());
            }
            let r = reverse.entry(canonical.clone()).or_default();
            if !r.contains(alias) {
                r.push(alias.clone());
            }
        }
        AliasMap { forward, reverse }
    }

    /// Whether the map carries no mappings, so expansion is a no-op.
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// The equivalence class of a folded tag, deduped and sorted: the tag itself,
    /// its own aliases, and for each canonical it maps to, that canonical plus
    /// the canonical's other aliases. A single hop only: the canonicals' own
    /// canonicals are never followed. When the map is empty (or the tag has no
    /// declared relation) the class is the tag alone.
    pub fn class_of(&self, folded: &str) -> Vec<String> {
        let mut set: BTreeSet<String> = BTreeSet::new();
        set.insert(folded.to_string());
        // The tag's own aliases (things declared to fold onto it).
        if let Some(aliases) = self.reverse.get(folded) {
            for a in aliases {
                set.insert(a.clone());
            }
        }
        // Each canonical the tag folds onto, and that canonical's siblings.
        if let Some(canonicals) = self.forward.get(folded) {
            for c in canonicals {
                set.insert(c.clone());
                if let Some(siblings) = self.reverse.get(c) {
                    for a in siblings {
                        set.insert(a.clone());
                    }
                }
            }
        }
        set.into_iter().collect()
    }
}

/// Whether a query filters on tags at all, so the alias map is loaded only when a
/// tag filter is actually present and every other search pays nothing for it. A
/// `tags` metadata filter counts the same as the dedicated `tags` field.
pub(crate) fn query_uses_tags(query: &SearchQuery) -> bool {
    query.tags.as_ref().is_some_and(|t| !t.is_empty())
        || query.metadata_filters.iter().any(|f| f.key == "tags")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(raw: &[(&str, &str)]) -> Vec<(String, String)> {
        raw.iter()
            .map(|(a, c)| (a.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn empty_map_expands_to_the_tag_alone() {
        let map = AliasMap::from_pairs(&[]);
        assert!(map.is_empty());
        assert_eq!(map.class_of("anything"), vec!["anything".to_string()]);
    }

    #[test]
    fn expands_both_directions() {
        // a -> b: searching either the alias or the canonical yields {a, b}.
        let map = AliasMap::from_pairs(&pairs(&[("a", "b")]));
        assert!(!map.is_empty());
        assert_eq!(map.class_of("a"), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(map.class_of("b"), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn sibling_aliases_share_a_class() {
        // a -> c and b -> c: expanding any one reaches all three.
        let map = AliasMap::from_pairs(&pairs(&[("a", "c"), ("b", "c")]));
        let want = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(map.class_of("a"), want);
        assert_eq!(map.class_of("b"), want);
        assert_eq!(map.class_of("c"), want);
    }

    #[test]
    fn single_hop_never_chains() {
        // a -> b, b -> c: expanding a stops at b (and b's aliases), never c.
        let map = AliasMap::from_pairs(&pairs(&[("a", "b"), ("b", "c")]));
        assert_eq!(map.class_of("a"), vec!["a".to_string(), "b".to_string()]);
        // b sees its alias a and its canonical c, but not a's canonical chain.
        assert_eq!(
            map.class_of("b"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn all_domain_sweep_unions_multiple_canonicals() {
        // One alias mapped onto two canonicals by different domains: the class is
        // the union of both targets.
        let map = AliasMap::from_pairs(&pairs(&[("x", "p"), ("x", "q")]));
        assert_eq!(
            map.class_of("x"),
            vec!["p".to_string(), "q".to_string(), "x".to_string()]
        );
    }
}
