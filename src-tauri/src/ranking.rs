//! Deterministic, explainable digest ranking driven only by explicit user feedback.
//!
//! Per `plan.md` M3 ("trends and learning"): ranking must be explicit-feedback-only
//! (there are no passive engagement signals in this app, and this module never reads
//! any), gated by a minimum amount of accumulated per-source feedback, bounded so it
//! can never fully override chronological order (a 25% chronological/diversity
//! reserve), and must produce a why-shown reason that mechanically reflects the
//! actual ranking decision made for that item — never templated filler.
//!
//! This module is pure and DB-free by design so every invariant above is unit
//! tested without a SQLite fixture. `db.rs` is responsible for selecting the
//! candidate posts (unchanged source-balanced/chronological cap query) and for
//! aggregating `feedback` rows into [`SourceFeedbackStats`] before calling
//! [`rank_candidates`].

use std::collections::HashMap;

/// A post already selected for an edition, supplied in strict chronological
/// order (most recent first), as produced by the existing source-balanced
/// selection query. This module only reorders and labels this fixed set; it
/// never changes which posts were selected.
#[derive(Debug, Clone)]
pub struct CandidatePost {
    pub post_id: String,
    pub source_id: String,
}

/// Aggregated explicit feedback for one source, counting only active
/// (non-retracted) `more_like_this` / `less_like_this` signals. `not_relevant`
/// and `mute_source` are handled upstream by excluding posts/sources from the
/// candidate set entirely (see the `NOT EXISTS` filters in `run_digest_fenced`)
/// and never feed this ranking bias — this app has no passive/behavioral
/// signal of any kind.
#[derive(Debug, Clone, Copy, Default)]
pub struct SourceFeedbackStats {
    pub more_count: i64,
    pub less_count: i64,
}

impl SourceFeedbackStats {
    fn total(&self) -> i64 {
        self.more_count + self.less_count
    }
}

/// The mechanically-derived reason a particular digest item is where it is.
/// Every variant maps to exactly one branch of the ranking decision in
/// [`rank_candidates`] — none of this text is templated filler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankingReason {
    /// Learned bias applied because this source cleared the minimum-data gate
    /// and has a net positive More/Less balance.
    LearnedHigher,
    /// Learned bias applied because this source cleared the minimum-data gate
    /// and has a net negative More/Less balance.
    LearnedLower,
    /// Gate cleared but More/Less feedback for this source is exactly
    /// balanced, so no directional bias is applied.
    LearnedMixed,
    /// This source has not yet accumulated the minimum number of explicit
    /// feedback signals; ranking stays neutral/chronological for it.
    InsufficientData,
    /// This slot is part of the fixed chronological/diversity reserve and is
    /// never affected by learned ranking, regardless of feedback.
    ChronologicalReserve,
    /// The user has paused learned ranking entirely in settings.
    Paused,
}

impl RankingReason {
    pub fn text(self) -> &'static str {
        match self {
            RankingReason::LearnedHigher => {
                "Placed higher: you often mark items from this source More."
            }
            RankingReason::LearnedLower => {
                "Placed lower: you often mark items from this source Less."
            }
            RankingReason::LearnedMixed => {
                "Chronological: feedback for this source is mixed, no clear preference yet."
            }
            RankingReason::InsufficientData => {
                "Chronological: not enough feedback yet to personalize this source."
            }
            RankingReason::ChronologicalReserve => {
                "Chronological reserve slot: kept in publish order regardless of ranking."
            }
            RankingReason::Paused => "Chronological: learned ranking is paused in settings.",
        }
    }
}

/// A fully-decided digest slot: final position, bounded importance score, and
/// the exact reason text for that decision.
#[derive(Debug, Clone)]
pub struct RankedItem {
    pub post_id: String,
    pub rank: i64,
    pub importance: f64,
    pub reason: &'static str,
}

/// Minimum number of active More/Less signals a source must accumulate before
/// its learned bias applies to ranking.
///
/// Web is a single-user desktop app with modest, slow-accumulating feedback
/// volume (plan.md explicitly forbids passive signals, so this is the *only*
/// source of ranking data). A single click (n=1) could be an accidental tap,
/// and a single opposing pair (n=2) is inherently a coin flip with no way to
/// tell a real preference from noise. 3 is the smallest threshold at which a
/// source's feedback must show a real majority (at least 2-to-1) before it is
/// allowed to bias ranking, while still being reachable within the first few
/// editions of ordinary use of a source, per the "reasonable small threshold"
/// called for in plan.md's M3 spec.
pub const MIN_FEEDBACK_FOR_BIAS: i64 = 3;

/// Bounded weight applied to the source's net More/Less ratio (which is itself
/// bounded to `[-1.0, 1.0]`).
///
/// The recency component below spans `0.8` down to `~0.52` across an 8-item
/// edition (the current digest cap). `0.15` is large enough to visibly move an
/// item's position once the minimum-data gate is cleared, but small enough
/// that: (a) learned bias alone can never invert two items whose recency gap
/// exceeds it, keeping ranking bounded rather than dominant, and (b) combined
/// with the recency component it can never leave the `digest_items.importance`
/// `[0, 1]` check-constraint range even before the final `clamp`.
const BIAS_WEIGHT: f64 = 0.15;

/// The recency-only baseline importance for the item at `index` (0-based) in
/// the chronological candidate list. This is the same decaying baseline the
/// former placeholder ranking used; it is now the neutral component that
/// learned bias is added on top of, never a replacement for it.
fn recency_component(index: usize) -> f64 {
    (0.8 - (index as f64 * 0.04)).max(0.0)
}

/// Number of an edition's `total` slots that must be filled by pure
/// chronological order, immune to learned ranking: `ceil(total * 0.25)`, per
/// plan.md's 25% chronological/diversity reserve requirement.
pub fn reserve_count(total: usize) -> usize {
    ((total as f64) * 0.25).ceil() as usize
}

/// Ranks `candidates` (already chronologically ordered, most recent first,
/// already source-balanced/capped by the caller) into final digest order.
///
/// Returns the same set of posts, reordered, with `rank` starting at 1 and a
/// mechanically-derived `reason` for every item. The last `reserve_count`
/// positions of `candidates` (the oldest items in the set — the ones learned
/// bias would most likely bury further) always keep their original relative
/// chronological order and are never touched by the bias computation, so
/// learned ranking can never crowd out that quarter of the edition.
pub fn rank_candidates(
    candidates: &[CandidatePost],
    feedback: &HashMap<String, SourceFeedbackStats>,
    ranking_paused: bool,
) -> Vec<RankedItem> {
    let total = candidates.len();

    if ranking_paused {
        return candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| RankedItem {
                post_id: candidate.post_id.clone(),
                rank: (index + 1) as i64,
                importance: recency_component(index),
                reason: RankingReason::Paused.text(),
            })
            .collect();
    }

    let reserve = reserve_count(total);
    let learned_len = total - reserve;

    struct Scored<'a> {
        candidate: &'a CandidatePost,
        importance: f64,
        reason: RankingReason,
    }

    let mut learned: Vec<Scored> = candidates[..learned_len]
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let recency = recency_component(index);
            let stats = feedback
                .get(&candidate.source_id)
                .copied()
                .unwrap_or_default();
            if stats.total() < MIN_FEEDBACK_FOR_BIAS {
                Scored {
                    candidate,
                    importance: recency,
                    reason: RankingReason::InsufficientData,
                }
            } else {
                let ratio = (stats.more_count - stats.less_count) as f64 / stats.total() as f64;
                let importance = (recency + ratio * BIAS_WEIGHT).clamp(0.0, 1.0);
                let reason = if ratio > 0.0 {
                    RankingReason::LearnedHigher
                } else if ratio < 0.0 {
                    RankingReason::LearnedLower
                } else {
                    RankingReason::LearnedMixed
                };
                Scored {
                    candidate,
                    importance,
                    reason,
                }
            }
        })
        .collect();

    // Stable sort: every tie (including every item whose gate was not met,
    // since those all keep importance == recency) preserves original
    // chronological order. `total_cmp` is used instead of `partial_cmp` so
    // this can never panic regardless of the bounded arithmetic above.
    learned.sort_by(|a, b| b.importance.total_cmp(&a.importance));

    let mut ranked: Vec<RankedItem> = learned
        .into_iter()
        .map(|scored| RankedItem {
            post_id: scored.candidate.post_id.clone(),
            rank: 0,
            importance: scored.importance,
            reason: scored.reason.text(),
        })
        .collect();

    for (offset, candidate) in candidates[learned_len..].iter().enumerate() {
        let index = learned_len + offset;
        ranked.push(RankedItem {
            post_id: candidate.post_id.clone(),
            rank: 0,
            importance: recency_component(index),
            reason: RankingReason::ChronologicalReserve.text(),
        });
    }

    for (position, item) in ranked.iter_mut().enumerate() {
        item.rank = (position + 1) as i64;
    }
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(post_id: &str, source_id: &str) -> CandidatePost {
        CandidatePost {
            post_id: post_id.to_owned(),
            source_id: source_id.to_owned(),
        }
    }

    fn ids(items: &[RankedItem]) -> Vec<&str> {
        items.iter().map(|item| item.post_id.as_str()).collect()
    }

    #[test]
    fn reserve_count_is_exact_ceiling_of_one_quarter() {
        assert_eq!(reserve_count(0), 0);
        assert_eq!(reserve_count(1), 1);
        assert_eq!(reserve_count(2), 1);
        assert_eq!(reserve_count(3), 1);
        assert_eq!(reserve_count(4), 1);
        assert_eq!(reserve_count(5), 2);
        assert_eq!(reserve_count(8), 2);
        assert_eq!(reserve_count(20), 5);
    }

    #[test]
    fn below_threshold_source_stays_neutral_chronological() {
        // Source "b" has only 2 active signals (below MIN_FEEDBACK_FOR_BIAS=3),
        // all More, which would otherwise strongly favor it.
        let candidates = vec![
            candidate("p0", "a"),
            candidate("p1", "b"),
            candidate("p2", "a"),
            candidate("p3", "a"),
        ];
        let mut feedback = HashMap::new();
        feedback.insert(
            "b".to_owned(),
            SourceFeedbackStats {
                more_count: 2,
                less_count: 0,
            },
        );
        let ranked = rank_candidates(&candidates, &feedback, false);
        // Reserve = ceil(4*0.25) = 1 -> learned zone is p0,p1,p2 (indices 0..3).
        // With no gate cleared, learned order matches chronological order.
        assert_eq!(ids(&ranked), vec!["p0", "p1", "p2", "p3"]);
        let p1 = ranked.iter().find(|item| item.post_id == "p1").unwrap();
        assert_eq!(p1.reason, RankingReason::InsufficientData.text());
        assert_eq!(p1.importance, recency_component(1));
    }

    #[test]
    fn at_threshold_source_gets_bounded_bias_and_reorders() {
        // Source "b" clears MIN_FEEDBACK_FOR_BIAS with a unanimous More signal.
        let candidates = vec![
            candidate("p0", "a"),
            candidate("p1", "a"),
            candidate("p2", "b"),
            candidate("p3", "a"),
        ];
        let mut feedback = HashMap::new();
        feedback.insert(
            "b".to_owned(),
            SourceFeedbackStats {
                more_count: 3,
                less_count: 0,
            },
        );
        let ranked = rank_candidates(&candidates, &feedback, false);
        // Reserve = 1 -> learned zone is p0,p1,p2 (indices 0..3); p3 is reserve.
        let p2 = ranked.iter().find(|item| item.post_id == "p2").unwrap();
        assert_eq!(p2.reason, RankingReason::LearnedHigher.text());
        let expected = (recency_component(2) + BIAS_WEIGHT).clamp(0.0, 1.0);
        assert!((p2.importance - expected).abs() < f64::EPSILON);
        // p2's recency alone (0.72) trails p0 (0.80) and p1 (0.76); the bounded
        // +0.15 bias (-> 0.87) must move it ahead of at least one
        // chronologically-earlier item, proving bias is actually applied to
        // ordering and not just computed and discarded.
        let position = |id: &str| ranked.iter().position(|item| item.post_id == id).unwrap();
        assert!(position("p2") < position("p1"));
    }

    #[test]
    fn exactly_balanced_feedback_at_gate_is_labeled_mixed_not_biased() {
        // 4 candidates -> reserve = 1, so index 1 (p1) is inside the learned zone.
        let candidates = vec![
            candidate("p0", "a"),
            candidate("p1", "b"),
            candidate("p2", "a"),
            candidate("p3", "a"),
        ];
        let mut feedback = HashMap::new();
        feedback.insert(
            "b".to_owned(),
            SourceFeedbackStats {
                more_count: 2,
                less_count: 2,
            },
        );
        let ranked = rank_candidates(&candidates, &feedback, false);
        let p1 = ranked.iter().find(|item| item.post_id == "p1").unwrap();
        assert_eq!(p1.reason, RankingReason::LearnedMixed.text());
        assert_eq!(p1.importance, recency_component(1));
    }

    #[test]
    fn negative_feedback_ranks_lower_and_is_labeled() {
        // 4 candidates -> reserve = 1, so index 1 (p1) is inside the learned zone.
        let candidates = vec![
            candidate("p0", "a"),
            candidate("p1", "b"),
            candidate("p2", "a"),
            candidate("p3", "a"),
        ];
        let mut feedback = HashMap::new();
        feedback.insert(
            "b".to_owned(),
            SourceFeedbackStats {
                more_count: 0,
                less_count: 4,
            },
        );
        let ranked = rank_candidates(&candidates, &feedback, false);
        let p1 = ranked.iter().find(|item| item.post_id == "p1").unwrap();
        assert_eq!(p1.reason, RankingReason::LearnedLower.text());
        assert!(p1.importance < recency_component(1));
    }

    #[test]
    fn reserve_slots_are_immune_to_extreme_ranking_bias() {
        // 8 candidates -> reserve = ceil(8*0.25) = 2. The last two chronological
        // items (p6, p7) must remain last, in original order, with the reserve
        // reason, no matter how extreme the feedback bias is.
        let candidates: Vec<CandidatePost> = (0..8)
            .map(|index| candidate(&format!("p{index}"), "biased-source"))
            .collect();
        let mut feedback = HashMap::new();
        feedback.insert(
            "biased-source".to_owned(),
            SourceFeedbackStats {
                more_count: 100,
                less_count: 0,
            },
        );
        let ranked = rank_candidates(&candidates, &feedback, false);
        let last_two: Vec<&str> = ranked[6..8]
            .iter()
            .map(|item| item.post_id.as_str())
            .collect();
        assert_eq!(last_two, vec!["p6", "p7"]);
        for item in &ranked[6..8] {
            assert_eq!(item.reason, RankingReason::ChronologicalReserve.text());
        }

        // Flip the bias to maximally negative for the same source and confirm
        // the reserve slots still hold the same two posts in the same order.
        let mut negative_feedback = HashMap::new();
        negative_feedback.insert(
            "biased-source".to_owned(),
            SourceFeedbackStats {
                more_count: 0,
                less_count: 100,
            },
        );
        let ranked_negative = rank_candidates(&candidates, &negative_feedback, false);
        let last_two_negative: Vec<&str> = ranked_negative[6..8]
            .iter()
            .map(|item| item.post_id.as_str())
            .collect();
        assert_eq!(last_two_negative, vec!["p6", "p7"]);
    }

    #[test]
    fn paused_ranking_forces_pure_chronological_order_and_reason() {
        let candidates = vec![
            candidate("p0", "a"),
            candidate("p1", "b"),
            candidate("p2", "a"),
            candidate("p3", "b"),
        ];
        let mut feedback = HashMap::new();
        feedback.insert(
            "b".to_owned(),
            SourceFeedbackStats {
                more_count: 50,
                less_count: 0,
            },
        );
        let ranked = rank_candidates(&candidates, &feedback, true);
        assert_eq!(ids(&ranked), vec!["p0", "p1", "p2", "p3"]);
        assert!(
            ranked
                .iter()
                .all(|item| item.reason == RankingReason::Paused.text())
        );
    }

    #[test]
    fn rank_is_sequential_and_one_indexed_for_any_edition_size() {
        for total in [0usize, 1, 3, 4, 8] {
            let candidates: Vec<CandidatePost> = (0..total)
                .map(|index| candidate(&format!("p{index}"), "a"))
                .collect();
            let ranked = rank_candidates(&candidates, &HashMap::new(), false);
            assert_eq!(ranked.len(), total);
            for (index, item) in ranked.iter().enumerate() {
                assert_eq!(item.rank, (index + 1) as i64);
            }
        }
    }

    #[test]
    fn importance_never_leaves_the_zero_to_one_range() {
        let candidates: Vec<CandidatePost> = (0..8)
            .map(|index| candidate(&format!("p{index}"), "a"))
            .collect();
        let mut feedback = HashMap::new();
        feedback.insert(
            "a".to_owned(),
            SourceFeedbackStats {
                more_count: 1000,
                less_count: 0,
            },
        );
        for item in rank_candidates(&candidates, &feedback, false) {
            assert!((0.0..=1.0).contains(&item.importance));
        }
    }
}
