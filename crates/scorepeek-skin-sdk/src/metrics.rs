//! Display-only arithmetic for score progress. Presentation stays with each skin.

const RANK_COUNT: usize = 8;
const BOUNDARY_NUMERATORS: [u64; 8] = [0, 2, 3, 4, 5, 6, 7, 8];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreProgress {
    pub score: u64,
    pub max_score: u64,
    /// Percentage in hundredths: 10000 means 100%.
    pub rate_hundredths: u64,
    pub rank_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankDistance {
    /// Boundary index from 0 to 7, or 8 for the maximum score.
    pub boundary_index: usize,
    pub difference: u64,
    pub direction: BoundaryDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryDirection {
    Above,
    Below,
    ExactMax,
}

fn boundary(max_score: u64, numerator: u64) -> u64 {
    (max_score * numerator).div_ceil(9)
}

#[must_use]
pub fn rank_thresholds(notes: u64) -> Option<[u64; 9]> {
    let max_score = notes.checked_mul(2)?;
    if notes == 0 || notes > u64::from(u32::MAX) {
        return None;
    }
    let mut thresholds = [0; 9];
    for (index, numerator) in BOUNDARY_NUMERATORS.iter().enumerate() {
        thresholds[index] = boundary(max_score, *numerator);
    }
    thresholds[8] = max_score;
    Some(thresholds)
}

#[must_use]
pub fn score_progress(score: u64, notes: u64) -> Option<ScoreProgress> {
    let thresholds = rank_thresholds(notes)?;
    let max_score = thresholds[8];
    if score > max_score {
        return None;
    }
    let rank_index = (1..RANK_COUNT)
        .rev()
        .find(|index| score >= thresholds[*index])
        .unwrap_or(0);
    Some(ScoreProgress {
        score,
        max_score,
        rate_hundredths: (score * 10_000 + max_score / 2) / max_score,
        rank_index,
    })
}

impl ScoreProgress {
    /// Returns no distance if the supplied rank index disagrees with the numeric score.
    #[must_use]
    pub fn nearest_boundary(self, supplied_rank_index: usize) -> Option<RankDistance> {
        if self.rank_index != supplied_rank_index {
            return None;
        }
        let notes = self.max_score / 2;
        let thresholds = rank_thresholds(notes)?;
        let lower = self.score - thresholds[self.rank_index];
        let upper = thresholds[self.rank_index + 1] - self.score;
        if self.rank_index == RANK_COUNT - 1 && upper == 0 {
            Some(RankDistance {
                boundary_index: RANK_COUNT,
                difference: 0,
                direction: BoundaryDirection::ExactMax,
            })
        } else if lower <= upper {
            Some(RankDistance {
                boundary_index: self.rank_index,
                difference: lower,
                direction: BoundaryDirection::Above,
            })
        } else {
            Some(RankDistance {
                boundary_index: self.rank_index + 1,
                difference: upper,
                direction: BoundaryDirection::Below,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundaryDirection, rank_thresholds, score_progress};

    #[test]
    fn all_rank_boundaries_and_ties_follow_the_score_scale() {
        assert_eq!(
            rank_thresholds(90).unwrap(),
            [0, 40, 60, 80, 100, 120, 140, 160, 180]
        );
        for (index, score) in [0, 40, 60, 80, 100, 120, 140, 160].into_iter().enumerate() {
            let progress = score_progress(score, 90).unwrap();
            assert_eq!(progress.rank_index, index);
            let distance = progress.nearest_boundary(index).unwrap();
            assert_eq!(distance.boundary_index, index);
            assert_eq!(distance.difference, 0);
            assert_eq!(distance.direction, BoundaryDirection::Above);
        }
        let tie = score_progress(150, 90)
            .unwrap()
            .nearest_boundary(6)
            .unwrap();
        assert_eq!(tie.boundary_index, 6);
        assert_eq!(tie.difference, 10);
        assert_eq!(tie.direction, BoundaryDirection::Above);
    }

    #[test]
    fn derives_rate_rank_and_nearest_boundary_without_formatting() {
        let progress = score_progress(2932, 1877).unwrap();
        assert_eq!(progress.rank_index, 6);
        assert_eq!(progress.rate_hundredths, 7810);
        let distance = progress.nearest_boundary(6).unwrap();
        assert_eq!(distance.boundary_index, 6);
        assert_eq!(distance.difference, 12);
        assert_eq!(distance.direction, BoundaryDirection::Above);

        let near_max = score_progress(3742, 1877).unwrap();
        let distance = near_max.nearest_boundary(7).unwrap();
        assert_eq!(distance.boundary_index, 8);
        assert_eq!(distance.difference, 12);
        assert_eq!(distance.direction, BoundaryDirection::Below);
        assert_eq!(
            score_progress(3754, 1877)
                .unwrap()
                .nearest_boundary(7)
                .unwrap()
                .direction,
            BoundaryDirection::ExactMax
        );
    }

    #[test]
    fn invalid_or_conflicting_input_has_no_progress_claim() {
        assert!(score_progress(0, 0).is_none());
        assert!(score_progress(4000, 1877).is_none());
        assert!(
            score_progress(2932, 1877)
                .unwrap()
                .nearest_boundary(5)
                .is_none()
        );
    }
}
