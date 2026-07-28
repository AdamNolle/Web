//! Deterministic lexical clustering for trend detection.
//!
//! Per `plan.md` M3 ("trends and learning"): "Deterministic lexical clustering
//! with cross-source/origin/actor/dedup gates ... LLMs may label validated
//! clusters but never decide membership." This module decides membership
//! entirely: it tokenizes significant terms out of each post's title and
//! body, collapses near-duplicate reposts of the same story to a single
//! representative (the dedup gate), then groups the remaining posts that
//! share enough overlapping significant terms into a candidate cluster. A
//! cluster is only real if its representative members span more than one
//! distinct source (the cross-source gate) -- today every connector is RSS,
//! so "source" is also the finest-grained origin/actor distinction the app
//! has; when actor-level connectors (Mastodon/Bluesky) land, the same gate
//! generalizes to "more than one distinct actor across more than one
//! distinct source". The dedup gate only collapses a *source's own* repost
//! of its own story (an edited re-publish of the same headline): two
//! different sources independently writing a near-identical headline about
//! the same event is exactly the cross-source signal a trend exists to
//! capture, so it is never collapsed away.
//!
//! This module is pure and DB-free by design, exactly like `ranking.rs`, so
//! every gate is unit tested without a SQLite fixture. `db.rs` is
//! responsible for selecting the candidate pool of new/recent posts (already
//! excluding hidden/deleted posts, same filters as digest selection) and for
//! persisting the resulting clusters. No model is ever consulted here or by
//! any caller of [`build_clusters`] to decide membership -- a model may only
//! be asked afterwards to phrase a nicer label for an already-decided
//! cluster, and this module always computes a deterministic fallback label
//! (the most frequent significant shared term(s)) so that path never depends
//! on one being available, mirroring the existing extractive-fallback
//! pattern for item summaries.

use std::collections::BTreeSet;

/// Two posts need at least this many overlapping significant terms to be
/// considered part of the same lexical cluster. 1 shared term is too weak
/// (one common noun like "election" or "report" proves nothing); 2 requires
/// real thematic overlap while staying reachable for short titles.
const MIN_SHARED_TERMS: usize = 2;

/// Jaccard similarity over title-only significant terms at or above which
/// two posts are treated as the same underlying story reposted, not two
/// distinct members of a trend. Expressed as a fraction (numerator/denominator)
/// to stay in exact integer arithmetic. 0.7 tolerates minor rewrites ("Senate
/// passes the bill" vs. "Senate passes bill") without merging stories that
/// merely share a subject.
const DEDUP_TITLE_JACCARD_NUM: usize = 7;
const DEDUP_TITLE_JACCARD_DEN: usize = 10;

/// A cluster needs posts from at least this many distinct sources to be a
/// trend at all: a single source repeating itself is not cross-source
/// evidence, regardless of how many times it says it.
const MIN_SOURCES_FOR_TREND: usize = 2;

/// How many of a cluster's most-frequent shared significant terms feed the
/// deterministic fallback label.
const LABEL_TERM_COUNT: usize = 3;

/// A post considered for lexical clustering. `db.rs` supplies these already
/// filtered to non-deleted, non-hidden (no active `not_relevant` /
/// `mute_source`) posts -- the same privacy contract as digest selection.
#[derive(Debug, Clone)]
pub struct ClusterCandidate {
    pub post_id: String,
    pub source_id: String,
    pub title: String,
    pub body: String,
    pub published_at: i64,
}

/// A validated, cross-source, dedup-collapsed trend cluster with a
/// deterministic label. Membership (`member_post_ids`) is never decided by a
/// model; `label` is a deterministic fallback that a caller may optionally
/// replace with a model-phrased version of the same underlying
/// `shared_terms`, without changing membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    /// Representative member post ids, one per underlying story, sorted by
    /// (most recent first, then post id) for deterministic output regardless
    /// of input order.
    pub member_post_ids: Vec<String>,
    /// Count of distinct sources among the representative members. Always
    /// >= [`MIN_SOURCES_FOR_TREND`] for any cluster this module returns.
    pub source_count: usize,
    pub confidence: &'static str,
    /// Deterministic fallback label: the most frequent significant terms
    /// shared by more than one member, title-cased and space-joined.
    pub label: String,
    /// The terms behind `label`, most-frequent first, for a caller that
    /// wants to hand them to a model as grounding for a nicer label.
    pub shared_terms: Vec<String>,
}

/// Deterministically groups `candidates` into validated trend clusters.
/// Calling this twice on the same (possibly reordered) input always produces
/// the same clusters: internal ordering is derived from content (published
/// time, post id, term text), never from input position or hashing order.
pub fn build_clusters(candidates: &[ClusterCandidate]) -> Vec<Cluster> {
    if candidates.len() < 2 {
        return Vec::new();
    }
    let title_terms: Vec<BTreeSet<String>> = candidates
        .iter()
        .map(|candidate| significant_terms(&candidate.title))
        .collect();
    let topic_terms: Vec<BTreeSet<String>> = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            significant_terms(&candidate.body)
                .union(&title_terms[index])
                .cloned()
                .collect()
        })
        .collect();

    let representatives = dedup_representatives(candidates, &title_terms);

    // Union-find over candidate indices, joining any pair of representatives
    // that clears MIN_SHARED_TERMS overlap in topic terms. `parent` is a
    // full-length array indexed by candidate index (not just representative
    // slots) so find/union stay simply indexed. Iterating `representatives`
    // (already in stable published_at/post_id order) and comparing every
    // pair keeps the result independent of input order.
    fn root(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let (root_a, root_b) = (root(parent, a), root(parent, b));
        if root_a != root_b {
            // Always attach the numerically larger root under the smaller
            // one so the resulting root id is deterministic regardless of
            // union order.
            if root_a < root_b {
                parent[root_b] = root_a;
            } else {
                parent[root_a] = root_b;
            }
        }
    }
    let mut parent: Vec<usize> = (0..candidates.len()).collect();
    for (position, &i) in representatives.iter().enumerate() {
        for &j in &representatives[position + 1..] {
            let shared = topic_terms[i].intersection(&topic_terms[j]).count();
            if shared >= MIN_SHARED_TERMS {
                union(&mut parent, i, j);
            }
        }
    }

    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for &representative in &representatives {
        let root_index = root(&mut parent, representative);
        groups.entry(root_index).or_default().push(representative);
    }

    let mut clusters: Vec<Cluster> = Vec::new();
    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        let distinct_sources: BTreeSet<&str> = members
            .iter()
            .map(|&index| candidates[index].source_id.as_str())
            .collect();
        if distinct_sources.len() < MIN_SOURCES_FOR_TREND {
            continue; // cross-source gate: a single source repeating itself is not a trend
        }
        let mut ordered_members = members.clone();
        ordered_members.sort_by(|&a, &b| {
            candidates[b]
                .published_at
                .cmp(&candidates[a].published_at)
                .then_with(|| candidates[a].post_id.cmp(&candidates[b].post_id))
        });
        let shared_terms = frequent_shared_terms(&topic_terms, members);
        let label = if shared_terms.is_empty() {
            "Related coverage".to_owned()
        } else {
            shared_terms
                .iter()
                .take(LABEL_TERM_COUNT)
                .map(|term| title_case(term))
                .collect::<Vec<_>>()
                .join(" ")
        };
        clusters.push(Cluster {
            member_post_ids: ordered_members
                .into_iter()
                .map(|index| candidates[index].post_id.clone())
                .collect(),
            source_count: distinct_sources.len(),
            confidence: if distinct_sources.len() >= 3 {
                "supported"
            } else {
                "emerging"
            },
            label,
            shared_terms: shared_terms.into_iter().take(LABEL_TERM_COUNT).collect(),
        });
    }
    // Deterministic output order independent of BTreeMap key (which is a
    // root candidate index and otherwise meaningless to a caller): most
    // cross-source-validated first, then by earliest member post id.
    clusters.sort_by(|a, b| {
        b.source_count
            .cmp(&a.source_count)
            .then_with(|| a.member_post_ids.first().cmp(&b.member_post_ids.first()))
    });
    clusters
}

/// For each candidate index, finds the representative (lowest
/// (published_at, post_id) member of its near-duplicate-title group) that it
/// collapses to. A post that starts no group of its own is always its own
/// representative. Returns the sorted, de-duplicated list of representative
/// indices in stable (published_at, post_id) order.
fn dedup_representatives(
    candidates: &[ClusterCandidate],
    title_terms: &[BTreeSet<String>],
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by(|&a, &b| {
        candidates[a]
            .published_at
            .cmp(&candidates[b].published_at)
            .then_with(|| candidates[a].post_id.cmp(&candidates[b].post_id))
    });
    let mut representatives: Vec<usize> = Vec::new();
    for &index in &order {
        // The dedup gate only ever collapses a source's own repost of its
        // own story (e.g. an edited re-publish of the same headline). Two
        // different sources independently writing a near-identical headline
        // about the same event is exactly the cross-source signal a trend
        // is supposed to capture, so it must never be collapsed away here.
        let is_duplicate_of_existing = representatives.iter().any(|&representative| {
            candidates[index].source_id == candidates[representative].source_id
                && is_near_duplicate_title(
                    &candidates[index].title,
                    &candidates[representative].title,
                    &title_terms[index],
                    &title_terms[representative],
                )
        });
        if !is_duplicate_of_existing {
            representatives.push(index);
        }
    }
    representatives
}

fn is_near_duplicate_title(
    title_a: &str,
    title_b: &str,
    terms_a: &BTreeSet<String>,
    terms_b: &BTreeSet<String>,
) -> bool {
    if title_a.trim().eq_ignore_ascii_case(title_b.trim()) {
        return true;
    }
    if terms_a.is_empty() || terms_b.is_empty() {
        return false;
    }
    let intersection = terms_a.intersection(terms_b).count();
    let union = terms_a.union(terms_b).count();
    union > 0 && intersection * DEDUP_TITLE_JACCARD_DEN >= DEDUP_TITLE_JACCARD_NUM * union
}

/// Terms shared by at least two of `members`' topic-term sets, most
/// frequent first, ties broken alphabetically for determinism.
fn frequent_shared_terms(topic_terms: &[BTreeSet<String>], members: &[usize]) -> Vec<String> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for &member in members {
        for term in &topic_terms[member] {
            *counts.entry(term.as_str()).or_insert(0) += 1;
        }
    }
    let mut frequent: Vec<(&str, usize)> = counts
        .into_iter()
        .filter(|&(_, count)| count >= 2)
        .collect();
    frequent.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    frequent
        .into_iter()
        .map(|(term, _)| term.to_owned())
        .collect()
}

fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Extracts the lowercase, de-duplicated set of significant (non-stopword,
/// length >= 4) alphanumeric terms from free text.
fn significant_terms(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|word| word.chars().count() >= 4 && !is_stopword(word))
        .collect()
}

const STOPWORDS: &[&str] = &[
    "that",
    "this",
    "with",
    "from",
    "have",
    "has",
    "had",
    "were",
    "been",
    "being",
    "they",
    "them",
    "their",
    "there",
    "here",
    "when",
    "where",
    "why",
    "how",
    "what",
    "which",
    "who",
    "whom",
    "will",
    "would",
    "shall",
    "should",
    "could",
    "cannot",
    "into",
    "onto",
    "over",
    "under",
    "after",
    "before",
    "between",
    "again",
    "further",
    "once",
    "about",
    "against",
    "during",
    "without",
    "within",
    "throughout",
    "than",
    "then",
    "also",
    "just",
    "only",
    "such",
    "same",
    "some",
    "more",
    "most",
    "other",
    "each",
    "both",
    "very",
    "does",
    "did",
    "doing",
    "done",
    "you",
    "your",
    "yours",
    "our",
    "ours",
    "his",
    "her",
    "hers",
    "its",
    "itself",
    "themselves",
    "were",
    "says",
    "said",
    "according",
    "report",
    "reports",
    "reported",
    "breaking",
    "update",
    "updates",
    "today",
    "news",
    "live",
    "watch",
    "video",
    "photos",
    "read",
    "more",
    "click",
    "here",
];

fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        post_id: &str,
        source_id: &str,
        title: &str,
        body: &str,
        published_at: i64,
    ) -> ClusterCandidate {
        ClusterCandidate {
            post_id: post_id.to_owned(),
            source_id: source_id.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            published_at,
        }
    }

    #[test]
    fn single_source_repetition_is_rejected_by_the_cross_source_gate() {
        let candidates = vec![
            candidate(
                "a1",
                "source-a",
                "Senate advances election reform bill",
                "Lawmakers debated election reform provisions extensively today.",
                100,
            ),
            candidate(
                "a2",
                "source-a",
                "Senate committee reviews election reform details",
                "The committee reviewed election reform provisions again this afternoon.",
                90,
            ),
        ];
        assert!(
            build_clusters(&candidates).is_empty(),
            "two posts from the same source must never form a trend on their own"
        );
    }

    #[test]
    fn cross_source_overlap_forms_an_emerging_cluster_with_two_sources() {
        let candidates = vec![
            candidate(
                "a1",
                "source-a",
                "Senate advances election reform bill",
                "Lawmakers debated election reform provisions extensively today.",
                100,
            ),
            candidate(
                "b1",
                "source-b",
                "Election reform bill clears Senate committee",
                "The election reform legislation advanced after a lengthy committee session.",
                90,
            ),
        ];
        let clusters = build_clusters(&candidates);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].source_count, 2);
        assert_eq!(clusters[0].confidence, "emerging");
        assert_eq!(clusters[0].member_post_ids, vec!["a1", "b1"]);
    }

    #[test]
    fn three_distinct_sources_are_supported_confidence() {
        let candidates = vec![
            candidate(
                "a1",
                "source-a",
                "Senate advances election reform bill",
                "Lawmakers debated election reform provisions extensively today.",
                100,
            ),
            candidate(
                "b1",
                "source-b",
                "Election reform bill clears Senate committee",
                "The election reform legislation advanced after a lengthy committee session.",
                90,
            ),
            candidate(
                "c1",
                "source-c",
                "Election reform advances in the Senate",
                "Senators moved election reform forward in a committee vote.",
                80,
            ),
        ];
        let clusters = build_clusters(&candidates);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].source_count, 3);
        assert_eq!(clusters[0].confidence, "supported");
    }

    #[test]
    fn insufficient_term_overlap_never_clusters() {
        let candidates = vec![
            candidate(
                "a1",
                "source-a",
                "Local bakery wins regional award",
                "A small bakery earned regional recognition this week.",
                100,
            ),
            candidate(
                "b1",
                "source-b",
                "City council approves new budget",
                "The council voted to approve next year's municipal budget.",
                90,
            ),
        ];
        assert!(build_clusters(&candidates).is_empty());
    }

    #[test]
    fn near_duplicate_repost_collapses_to_one_representative_member() {
        // source-a posts the same story twice with a near-identical title
        // (a repost); source-b covers the same underlying story once. Without
        // the dedup gate this would wrongly look like a 3-post cluster.
        let candidates = vec![
            candidate(
                "a1",
                "source-a",
                "Senate advances election reform bill",
                "Lawmakers debated election reform provisions extensively today.",
                100,
            ),
            candidate(
                "a2",
                "source-a",
                "Senate advances the election reform bill",
                "Lawmakers debated election reform provisions extensively today, updated.",
                101,
            ),
            candidate(
                "b1",
                "source-b",
                "Election reform bill clears Senate committee",
                "The election reform legislation advanced after a lengthy committee session.",
                90,
            ),
        ];
        let clusters = build_clusters(&candidates);
        assert_eq!(clusters.len(), 1);
        assert_eq!(
            clusters[0].member_post_ids.len(),
            2,
            "the near-duplicate repost must collapse to a single representative, not inflate the cluster"
        );
        assert_eq!(clusters[0].source_count, 2);
        // The earlier-published of the two near-duplicates ("a1") is kept as
        // the representative for source-a.
        assert!(clusters[0].member_post_ids.contains(&"a1".to_owned()));
        assert!(!clusters[0].member_post_ids.contains(&"a2".to_owned()));
    }

    #[test]
    fn membership_is_deterministic_regardless_of_input_order() {
        let candidates = vec![
            candidate(
                "a1",
                "source-a",
                "Senate advances election reform bill",
                "Lawmakers debated election reform provisions extensively today.",
                100,
            ),
            candidate(
                "b1",
                "source-b",
                "Election reform bill clears Senate committee",
                "The election reform legislation advanced after a lengthy committee session.",
                90,
            ),
            candidate(
                "c1",
                "source-c",
                "Local bakery wins regional award",
                "A small bakery earned regional recognition this week.",
                80,
            ),
        ];
        let forward = build_clusters(&candidates);
        let mut reversed = candidates.clone();
        reversed.reverse();
        let backward = build_clusters(&reversed);
        assert_eq!(
            forward, backward,
            "same input, reordered, must yield identical clusters"
        );
    }

    #[test]
    fn fallback_label_uses_the_most_frequent_shared_terms_when_no_model_is_available() {
        // Deliberately share only "election" and "reform" between the two
        // posts (everything else in their titles/bodies differs) so the
        // fallback label is unambiguous: it must be built from exactly the
        // terms genuinely common to both members, nothing else.
        let candidates = vec![
            candidate(
                "a1",
                "source-a",
                "Senate advances election reform bill",
                "Lawmakers debated election reform provisions extensively.",
                100,
            ),
            candidate(
                "b1",
                "source-b",
                "City council approves election reform measure",
                "Officials finalized election reform details after lengthy negotiation.",
                90,
            ),
        ];
        let clusters = build_clusters(&candidates);
        assert_eq!(clusters.len(), 1);
        assert_eq!(
            clusters[0].shared_terms,
            vec!["election".to_owned(), "reform".to_owned()]
        );
        assert_eq!(clusters[0].label, "Election Reform");
    }

    #[test]
    fn significant_terms_excludes_stopwords_and_short_words() {
        let terms = significant_terms("The bill was with them and it has a report today");
        // Stopwords, even long ones, are excluded.
        assert!(!terms.contains("with"));
        assert!(!terms.contains("report"));
        assert!(!terms.contains("today"));
        // Words shorter than 4 characters are excluded regardless of stopword status.
        assert!(!terms.contains("was"));
        assert!(!terms.contains("and"));
        // A genuine significant term survives.
        assert!(terms.contains("bill"));
    }
}
