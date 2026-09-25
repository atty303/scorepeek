//! Display-only progress derived from the supplied EX score and chart note count.

pub struct ScoreMetrics {
    pub percent_hundredths: u64,
    pub delta: Option<String>,
}

const RANKS: [&str; 8] = ["F", "E", "D", "C", "B", "A", "AA", "AAA"];
const BOUNDARY_NUMERATORS: [u64; 8] = [0, 2, 3, 4, 5, 6, 7, 8];

fn boundary(max_score: u64, numerator: u64) -> u64 {
    (max_score * numerator).div_ceil(9)
}

impl ScoreMetrics {
    pub fn new(score_text: &str, notes: Option<u64>, supplied_rank: &str) -> Option<Self> {
        let score = score_text.parse::<u64>().ok()?;
        let notes = notes.filter(|notes| *notes > 0 && u32::try_from(*notes).is_ok())?;
        let max_score = notes * 2;
        if score > max_score {
            return None;
        }

        let percent_hundredths = (score * 10_000 + max_score / 2) / max_score;
        let actual_rank = (1..RANKS.len())
            .rev()
            .find(|index| score >= boundary(max_score, BOUNDARY_NUMERATORS[*index]))
            .unwrap_or(0);
        let delta = RANKS
            .iter()
            .position(|rank| *rank == supplied_rank)
            .filter(|rank| *rank == actual_rank)
            .map(|rank| {
                let lower = boundary(max_score, BOUNDARY_NUMERATORS[rank]);
                let upper = if rank + 1 == RANKS.len() {
                    max_score
                } else {
                    boundary(max_score, BOUNDARY_NUMERATORS[rank + 1])
                };
                let below = score - lower;
                let above = upper - score;
                if rank + 1 == RANKS.len() && above == 0 {
                    "MAX".to_owned()
                } else if below <= above {
                    format!("{}+{below:04}", RANKS[rank])
                } else {
                    let next = RANKS.get(rank + 1).copied().unwrap_or("MAX");
                    format!("{next}-{above:04}")
                }
            });

        Some(Self {
            percent_hundredths,
            delta,
        })
    }

    pub fn css_width(&self) -> String {
        format!(
            "{}.{:02}%",
            self.percent_hundredths / 100,
            self.percent_hundredths % 100
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ScoreMetrics;

    #[test]
    fn rank_delta_uses_current_and_next_thresholds() {
        let near_aa = ScoreMetrics::new("2932", Some(1877), "AA").unwrap();
        assert_eq!(near_aa.delta.as_deref(), Some("AA+0012"));
        assert_eq!(near_aa.css_width(), "78.10%");

        let near_aaa = ScoreMetrics::new("3325", Some(1877), "AA").unwrap();
        assert_eq!(near_aaa.delta.as_deref(), Some("AAA-0012"));

        let near_max = ScoreMetrics::new("3742", Some(1877), "AAA").unwrap();
        assert_eq!(near_max.delta.as_deref(), Some("MAX-0012"));
        assert_eq!(
            ScoreMetrics::new("3754", Some(1877), "AAA")
                .unwrap()
                .delta
                .as_deref(),
            Some("MAX")
        );
    }

    #[test]
    fn invalid_or_inconsistent_inputs_never_claim_progress() {
        assert!(ScoreMetrics::new("—", Some(1877), "AA").is_none());
        assert!(ScoreMetrics::new("2932", None, "AA").is_none());
        assert!(ScoreMetrics::new("2932", Some(0), "AA").is_none());
        assert!(ScoreMetrics::new("4000", Some(1877), "AAA").is_none());
        assert!(
            ScoreMetrics::new("2932", Some(1877), "A")
                .unwrap()
                .delta
                .is_none()
        );
    }
}
