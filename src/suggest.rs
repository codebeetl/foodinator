use chrono::NaiveDate;

/// A meal's history/preference data, fetched once and reused for scoring
/// every day of a week rather than requeried per day.
#[derive(Debug, Clone)]
pub struct MealCandidate {
    pub id: i64,
    pub all_time_occurrences: i64,
    pub planned_dates: Vec<NaiveDate>,
    pub liked_by: Vec<i64>,
    pub disliked_by: Vec<i64>,
}

/// Picks a meal for `target_date` from `candidates`, preferring meals not
/// planned recently, planned often overall, and liked by more of
/// `attendee_ids`. `exclude_meal_id` removes the day's current meal (reroll).
/// `jitter` perturbs the score by meal id so repeated calls can differ.
pub fn score_candidates(
    candidates: &[MealCandidate],
    target_date: NaiveDate,
    attendee_ids: &[i64],
    exclude_meal_id: Option<i64>,
    jitter: impl Fn(i64) -> f64,
) -> Option<i64> {
    let pool: Vec<&MealCandidate> = candidates
        .iter()
        .filter(|c| Some(c.id) != exclude_meal_id)
        .collect();

    let not_disliked: Vec<&MealCandidate> = pool
        .iter()
        .copied()
        .filter(|c| !any_attendee_dislikes(c, attendee_ids))
        .collect();
    let usable = if not_disliked.is_empty() {
        pool
    } else {
        not_disliked
    };

    usable
        .iter()
        .max_by(|a, b| {
            score(a, target_date, attendee_ids, &jitter)
                .partial_cmp(&score(b, target_date, attendee_ids, &jitter))
                .unwrap()
                .then_with(|| a.id.cmp(&b.id))
        })
        .map(|c| c.id)
}

fn score(
    candidate: &MealCandidate,
    target_date: NaiveDate,
    attendee_ids: &[i64],
    jitter: &impl Fn(i64) -> f64,
) -> f64 {
    let staleness = staleness_days(candidate, target_date) as f64;
    let favourite = ((1 + candidate.all_time_occurrences) as f64).ln();
    let liked_share = liked_share(candidate, attendee_ids);
    let dislike_penalty = if any_attendee_dislikes(candidate, attendee_ids) {
        DISLIKE_PENALTY
    } else {
        0.0
    };

    staleness + favourite * 2.0 + liked_share * 5.0 - dislike_penalty + jitter(candidate.id)
}

const DISLIKE_PENALTY: f64 = 1000.0;

fn any_attendee_dislikes(candidate: &MealCandidate, attendee_ids: &[i64]) -> bool {
    attendee_ids
        .iter()
        .any(|id| candidate.disliked_by.contains(id))
}

const STALENESS_CAP_DAYS: i64 = 28;

fn staleness_days(candidate: &MealCandidate, target_date: NaiveDate) -> i64 {
    candidate
        .planned_dates
        .iter()
        .map(|d| (*d - target_date).num_days().abs())
        .min()
        .unwrap_or(STALENESS_CAP_DAYS)
        .min(STALENESS_CAP_DAYS)
}

fn liked_share(candidate: &MealCandidate, attendee_ids: &[i64]) -> f64 {
    if attendee_ids.is_empty() {
        return 0.0;
    }
    let liked = attendee_ids
        .iter()
        .filter(|id| candidate.liked_by.contains(id))
        .count();
    liked as f64 / attendee_ids.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: i64, occurrences: i64, planned_dates: Vec<NaiveDate>) -> MealCandidate {
        MealCandidate {
            id,
            all_time_occurrences: occurrences,
            planned_dates,
            liked_by: vec![],
            disliked_by: vec![],
        }
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, day).unwrap()
    }

    #[test]
    fn picks_the_meal_planned_longest_ago_when_otherwise_equal() {
        let stale = candidate(1, 5, vec![date(1)]);
        let recent = candidate(2, 5, vec![date(24)]);
        let candidates = vec![stale, recent];

        let winner = score_candidates(&candidates, date(25), &[], None, |_| 0.0);

        assert_eq!(
            winner,
            Some(1),
            "the meal planned 24 days ago beats one planned yesterday"
        );
    }

    #[test]
    fn staleness_is_capped_so_a_never_planned_meal_does_not_dominate_forever() {
        let never_planned = candidate(1, 5, vec![]);
        let planned_a_month_ago = candidate(2, 5, vec![date(1)]);
        let candidates = vec![never_planned, planned_a_month_ago];

        // date(29) is 28 days after date(1), exactly at the staleness cap, so
        // both candidates should tie on staleness and the higher id wins the
        // deterministic tie-break.
        let winner = score_candidates(&candidates, date(29), &[], None, |_| 0.0);

        assert_eq!(winner, Some(2));
    }

    #[test]
    fn higher_all_time_occurrence_count_wins_when_recency_is_equal() {
        let rarely_made = candidate(1, 1, vec![]);
        let household_favourite = candidate(2, 50, vec![]);
        let candidates = vec![rarely_made, household_favourite];

        let winner = score_candidates(&candidates, date(1), &[], None, |_| 0.0);

        assert_eq!(winner, Some(2));
    }

    #[test]
    fn prefers_the_meal_liked_by_more_of_todays_attendees() {
        let mut liked_by_nobody = candidate(1, 5, vec![]);
        liked_by_nobody.liked_by = vec![];
        let mut liked_by_both = candidate(2, 5, vec![]);
        liked_by_both.liked_by = vec![10, 20];
        let candidates = vec![liked_by_nobody, liked_by_both];

        let winner = score_candidates(&candidates, date(1), &[10, 20], None, |_| 0.0);

        assert_eq!(winner, Some(2));
    }

    #[test]
    fn excludes_a_meal_disliked_by_an_attendee_even_when_it_would_otherwise_win() {
        let mut disliked = candidate(1, 100, vec![]);
        disliked.disliked_by = vec![10];
        let fallback = candidate(2, 1, vec![]);
        let candidates = vec![disliked, fallback];

        let winner = score_candidates(&candidates, date(1), &[10], None, |_| 0.0);

        assert_eq!(
            winner,
            Some(2),
            "the household favourite is skipped because attendee 10 dislikes it"
        );
    }

    #[test]
    fn falls_back_to_a_disliked_meal_when_it_is_the_only_candidate() {
        let mut disliked = candidate(1, 5, vec![]);
        disliked.disliked_by = vec![10];
        let candidates = vec![disliked];

        let winner = score_candidates(&candidates, date(1), &[10], None, |_| 0.0);

        assert_eq!(
            winner,
            Some(1),
            "no other candidate exists, so the dislike is only a penalty"
        );
    }

    #[test]
    fn zero_attendees_does_not_panic_and_ignores_liked_share() {
        let candidates = vec![candidate(1, 5, vec![])];

        let winner = score_candidates(&candidates, date(1), &[], None, |_| 0.0);

        assert_eq!(winner, Some(1));
    }

    #[test]
    fn excluded_meal_id_is_never_picked_even_if_it_would_otherwise_win() {
        let current = candidate(1, 100, vec![]);
        let alternative = candidate(2, 1, vec![]);
        let candidates = vec![current, alternative];

        let winner = score_candidates(&candidates, date(1), &[], Some(1), |_| 0.0);

        assert_eq!(
            winner,
            Some(2),
            "meal 1 is excluded (it's the current day's pick, for reroll)"
        );
    }

    #[test]
    fn jitter_can_flip_the_winner_between_two_close_candidates() {
        let a = candidate(1, 5, vec![]);
        let b = candidate(2, 5, vec![]);
        let candidates = vec![a, b];

        let winner = score_candidates(&candidates, date(1), &[], None, |id| {
            if id == 2 {
                100.0
            } else {
                0.0
            }
        });

        assert_eq!(
            winner,
            Some(2),
            "a large jitter on meal 2 should overcome an otherwise exact tie"
        );
    }

    #[test]
    fn feeding_a_pick_back_into_planned_dates_prevents_repeating_it_later_the_same_week() {
        let mut favourite = candidate(1, 50, vec![]);
        let alternative = candidate(2, 5, vec![]);
        let mut candidates = vec![favourite.clone(), alternative];

        let tuesday_pick = score_candidates(&candidates, date(2), &[], None, |_| 0.0);
        assert_eq!(
            tuesday_pick,
            Some(1),
            "the favourite wins Tuesday with no history yet"
        );

        favourite.planned_dates.push(date(2));
        candidates[0] = favourite;

        let wednesday_pick = score_candidates(&candidates, date(3), &[], None, |_| 0.0);
        assert_eq!(
            wednesday_pick,
            Some(2),
            "having just been placed on Tuesday, the favourite loses its staleness edge for Wednesday"
        );
    }
}
