//! `V201`'s two-stage near-duplicate clusterer.
//!
//! All-pairs verification is quadratic and a knowledge base is not small, so
//! detection runs in two stages:
//!
//! 1. **Blocking.** Each body is folded with
//!    [`crystalline_core::similarity::normalize`], cut into word 3-grams and
//!    summarized by a MinHash signature. The signature is split into bands and
//!    two bodies become a candidate pair only when a whole band matches. Work
//!    is linear in corpus bytes rather than quadratic in engram count.
//! 2. **Verification.** Every candidate pair is scored with the bigram Dice
//!    coefficient, the same arithmetic verify's `Q004` uses, and a pair only
//!    survives at or above [`super::DUP_THRESHOLD`].
//!
//! Surviving pairs are merged with union-find, so a chain of pairwise
//! duplicates forms one cluster.
//!
//! Two caps keep the pass bounded, both reported so a caller can say the run
//! was incomplete rather than silently under-reporting: a MinHash bucket over
//! [`super::MAX_BUCKET`] members is skipped whole (a bucket that big is a
//! boilerplate block, not a duplicate set) and the global candidate-pair count
//! stops at [`super::MAX_CANDIDATE_PAIRS`].
//!
//! Known limit, deliberate: this is lexical. It finds copy-paste and edited
//! copies, never a pure paraphrase. Rescoring the same candidate pairs with
//! embeddings is the natural follow-up, and those pairs are also exactly where
//! semantic contradiction detection would attach.

use std::collections::{BTreeMap, BTreeSet};

use crystalline_core::similarity::{dice_coefficient, normalize};

use super::SweepOptions;

/// How many words make up one shingle. Three is the usual choice for prose:
/// long enough that a shared 3-gram is evidence rather than coincidence, short
/// enough that a short paragraph still produces many of them.
pub const SHINGLE_WORDS: usize = 3;

/// The FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// The FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The result of one clustering pass over a list of bodies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DuplicateClusters {
    /// Clusters of input indices. Every cluster holds two or more members,
    /// members are sorted ascending and clusters are sorted by first member,
    /// so the output is deterministic for a given input.
    pub clusters: Vec<Vec<usize>>,
    /// How many distinct candidate pairs stage 2 scored. Never above
    /// [`SweepOptions::max_candidate_pairs`].
    pub candidate_pairs: usize,
    /// How many MinHash buckets were skipped for exceeding
    /// [`SweepOptions::max_bucket`].
    pub skipped_buckets: usize,
    /// Whether the global candidate-pair cap stopped blocking early.
    pub capped: bool,
}

/// Cluster near-duplicate bodies. The returned clusters index into `bodies`.
///
/// A body whose normalized form holds fewer than
/// [`SweepOptions::min_dup_body_chars`] characters, or fewer than
/// [`SHINGLE_WORDS`] words, is skipped: below that length the Dice coefficient
/// is noise and two short stubs would cluster on nothing.
pub fn cluster_near_duplicates(bodies: &[&str], options: &SweepOptions) -> DuplicateClusters {
    let normalized: Vec<Option<String>> = bodies
        .iter()
        .map(|b| {
            let text = normalize(b);
            if text.chars().count() < options.min_dup_body_chars {
                None
            } else {
                Some(text)
            }
        })
        .collect();

    let signatures: Vec<Option<Vec<u64>>> = normalized
        .iter()
        .map(|t| {
            t.as_deref()
                .and_then(|t| minhash(t, options.minhash_hashes))
        })
        .collect();

    let mut out = DuplicateClusters::default();
    let buckets = band_buckets(&signatures, options);
    let pairs = candidate_pairs(&buckets, options, &mut out);
    out.candidate_pairs = pairs.len();

    let mut uf = UnionFind::new(bodies.len());
    for (a, b) in &pairs {
        let (Some(ta), Some(tb)) = (&normalized[*a], &normalized[*b]) else {
            continue;
        };
        if dice_coefficient(ta, tb) >= options.dup_threshold {
            uf.union(*a, *b);
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (idx, text) in normalized.iter().enumerate() {
        if text.is_none() {
            continue;
        }
        groups.entry(uf.find(idx)).or_default().push(idx);
    }
    out.clusters = groups
        .into_values()
        .filter(|members| members.len() >= 2)
        .collect();
    out.clusters.sort();
    out
}

/// Group signatures into `(band, band hash)` buckets. A `BTreeMap` rather than
/// a `HashMap` so the later iteration order, and with it the candidate-pair cap
/// cut, is identical on every run.
fn band_buckets(
    signatures: &[Option<Vec<u64>>],
    options: &SweepOptions,
) -> BTreeMap<(usize, u64), Vec<usize>> {
    let mut buckets: BTreeMap<(usize, u64), Vec<usize>> = BTreeMap::new();
    for (i, sig) in signatures.iter().enumerate() {
        let Some(sig) = sig else { continue };
        for band in 0..options.minhash_bands {
            let start = band * options.minhash_rows;
            if start >= sig.len() {
                break;
            }
            let end = (start + options.minhash_rows).min(sig.len());
            buckets
                .entry((band, band_key(band, &sig[start..end])))
                .or_default()
                .push(i);
        }
    }
    buckets
}

/// Expand the buckets into distinct candidate pairs, honoring both caps and
/// recording what they cut.
fn candidate_pairs(
    buckets: &BTreeMap<(usize, u64), Vec<usize>>,
    options: &SweepOptions,
    out: &mut DuplicateClusters,
) -> BTreeSet<(usize, usize)> {
    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    'buckets: for members in buckets.values() {
        if members.len() < 2 {
            continue;
        }
        if members.len() > options.max_bucket {
            out.skipped_buckets += 1;
            continue;
        }
        for a in 0..members.len() {
            for b in (a + 1)..members.len() {
                let (lo, hi) = if members[a] < members[b] {
                    (members[a], members[b])
                } else {
                    (members[b], members[a])
                };
                pairs.insert((lo, hi));
                if pairs.len() >= options.max_candidate_pairs {
                    out.capped = true;
                    break 'buckets;
                }
            }
        }
    }
    pairs
}

/// The MinHash signature of a normalized body: the minimum hash of every word
/// 3-gram under `hashes` independent mixings. `None` when the body holds fewer
/// than [`SHINGLE_WORDS`] words, which leaves it out of blocking entirely.
fn minhash(text: &str, hashes: usize) -> Option<Vec<u64>> {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    if words.len() < SHINGLE_WORDS || hashes == 0 {
        return None;
    }
    let mut signature = vec![u64::MAX; hashes];
    for shingle in words.windows(SHINGLE_WORDS) {
        let base = shingle_hash(shingle);
        for (i, slot) in signature.iter_mut().enumerate() {
            let h = mix(base ^ mix(i as u64 + 1));
            if h < *slot {
                *slot = h;
            }
        }
    }
    Some(signature)
}

/// FNV-1a over the words of one shingle joined by a single space, computed
/// without allocating the joined string.
fn shingle_hash(words: &[&str]) -> u64 {
    let mut h = FNV_OFFSET;
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            h ^= u64::from(b' ');
            h = h.wrapping_mul(FNV_PRIME);
        }
        for b in w.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    h
}

/// Collapse one band of signature values into a bucket key. The band index
/// seeds the fold so the same values in different bands never collide.
fn band_key(band: usize, values: &[u64]) -> u64 {
    let mut h = mix(band as u64 + 1);
    for v in values {
        h = mix(h ^ v);
    }
    h
}

/// The SplitMix64 finalizer: a cheap, dependency-free avalanche over a u64.
/// Good enough to turn an FNV hash into independent-looking hash families,
/// which is all MinHash blocking needs.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A minimal union-find over body indices, the same shape as the one the tag
/// clusterer uses.
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

    /// A body long enough to clear the 200 normalized character floor.
    fn long_body(subject: &str) -> String {
        format!(
            "The {subject} pipeline runs on every push to the main branch. \
             It builds the workspace then runs the full test suite before it \
             uploads the artifacts. A failure anywhere in the chain stops the \
             release and pages the on-call engineer immediately."
        )
    }

    fn cluster(bodies: &[String]) -> DuplicateClusters {
        let refs: Vec<&str> = bodies.iter().map(|b| b.as_str()).collect();
        cluster_near_duplicates(&refs, &SweepOptions::default())
    }

    #[test]
    fn identical_bodies_cluster() {
        let bodies = vec![long_body("deploy"), long_body("deploy")];
        let out = cluster(&bodies);
        assert_eq!(out.clusters, vec![vec![0, 1]]);
        assert!(!out.capped);
        assert_eq!(out.skipped_buckets, 0);
    }

    #[test]
    fn reordered_paragraphs_still_cluster() {
        let first = "The index is a derived layer and can always be rebuilt \
                     from the files on disk without any loss of knowledge.";
        let second = "A wipe followed by a resync recreates every row, so a \
                      corrupt index is never a data loss risk for anyone.";
        let a = format!("{first}\n\n{second}");
        let b = format!("{second}\n\n{first}");
        let out = cluster(&[a, b]);
        assert_eq!(
            out.clusters,
            vec![vec![0, 1]],
            "moving a paragraph must not break the cluster"
        );
    }

    #[test]
    fn unrelated_bodies_never_cluster() {
        let a = "Tag aliases collapse two spellings of one concept onto a \
                 single canonical name so the vocabulary stays small and the \
                 search results stay coherent for everyone reading them."
            .to_string();
        let b = "Embedding coverage is often partial right after a sync or a \
                 model swap which is why the duplicate detector never leans \
                 on vectors as its only source of candidate pairs at all."
            .to_string();
        let out = cluster(&[a, b]);
        assert!(out.clusters.is_empty(), "{:?}", out.clusters);
    }

    #[test]
    fn edited_copies_cluster_but_rewrites_do_not() {
        let original = long_body("deploy");
        let edited = original.replace("pages the on-call engineer", "wakes the duty engineer");
        let rewrite = "Nightly jobs are scheduled from a separate workflow \
                       file and never touch the release artifacts at all. \
                       They only refresh the cached model downloads used by \
                       the embedding tests in continuous integration runs."
            .to_string();
        let out = cluster(&[original, edited, rewrite]);
        assert_eq!(out.clusters, vec![vec![0, 1]]);
    }

    #[test]
    fn short_bodies_are_skipped() {
        let short = "Too short to score reliably.".to_string();
        let out = cluster(&[short.clone(), short]);
        assert!(
            out.clusters.is_empty(),
            "a body under the 200 character floor must never cluster"
        );
        assert_eq!(out.candidate_pairs, 0);
    }

    #[test]
    fn a_body_with_fewer_than_three_words_is_skipped() {
        // Long enough in characters, but one single word: no shingle exists.
        let one_word = "a".repeat(250);
        let out = cluster(&[one_word.clone(), one_word]);
        assert!(out.clusters.is_empty());
        assert_eq!(out.candidate_pairs, 0);
    }

    #[test]
    fn oversized_buckets_are_skipped_and_reported() {
        let options = SweepOptions {
            max_bucket: 4,
            ..SweepOptions::default()
        };
        let bodies: Vec<String> = (0..6).map(|_| long_body("deploy")).collect();
        let refs: Vec<&str> = bodies.iter().map(|b| b.as_str()).collect();
        let out = cluster_near_duplicates(&refs, &options);
        assert!(
            out.clusters.is_empty(),
            "every bucket held 6 identical bodies over the cap of 4"
        );
        assert_eq!(out.candidate_pairs, 0);
        assert_eq!(
            out.skipped_buckets, options.minhash_bands,
            "one skipped bucket per band"
        );
    }

    #[test]
    fn the_candidate_pair_cap_is_respected() {
        let options = SweepOptions {
            max_candidate_pairs: 3,
            ..SweepOptions::default()
        };
        let bodies: Vec<String> = (0..8).map(|_| long_body("deploy")).collect();
        let refs: Vec<&str> = bodies.iter().map(|b| b.as_str()).collect();
        let out = cluster_near_duplicates(&refs, &options);
        assert!(out.capped, "the cap must be reported");
        assert_eq!(out.candidate_pairs, 3);
    }

    #[test]
    fn clustering_is_deterministic_across_runs() {
        let bodies: Vec<String> = ["deploy", "deploy", "release", "release", "backup"]
            .iter()
            .map(|s| long_body(s))
            .collect();
        let first = cluster(&bodies);
        for _ in 0..5 {
            assert_eq!(cluster(&bodies), first);
        }
    }

    #[test]
    fn a_chain_of_pairs_merges_into_one_cluster() {
        let base = long_body("deploy");
        let middle = base.replace("pages the on-call engineer", "wakes the duty engineer");
        let far = middle.replace("uploads the artifacts", "publishes the artifacts");
        let out = cluster(&[base, middle, far]);
        assert_eq!(
            out.clusters,
            vec![vec![0, 1, 2]],
            "union-find must chain pairwise duplicates into one cluster"
        );
    }

    #[test]
    fn empty_input_is_empty() {
        let out = cluster_near_duplicates(&[], &SweepOptions::default());
        assert_eq!(out, DuplicateClusters::default());
    }
}
