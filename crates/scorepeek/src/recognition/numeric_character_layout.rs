use serde::Deserialize;

use super::{CanonicalLayout, RecognitionError, Roi};

const LAYOUT_BYTES: &[u8] = include_bytes!("../result-numeric-character-layout-v1.json");
const LAYOUT_SCHEMA: &str = "scorepeek-result-numeric-character-layout-v1";

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericCharacterFieldLayout {
    pub digit_cells: Vec<Roi>,
    pub not_displayed_marker: Option<Roi>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericCharacterLayoutVariant {
    pub difficulty: String,
    pub displayed_digits: usize,
    pub digit_cells: Vec<Roi>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultNumericCharacterLayout {
    schema: String,
    canonical_frame_contract_id: String,
    canonical_layout_sha256: String,
    pub level: Vec<NumericCharacterLayoutVariant>,
    pub notes: NumericCharacterFieldLayout,
    pub current_score: NumericCharacterFieldLayout,
    pub previous_score: NumericCharacterFieldLayout,
    pub previous_miss_count: NumericCharacterFieldLayout,
    pub miss_count: NumericCharacterFieldLayout,
    pub pgreat: NumericCharacterFieldLayout,
    pub great: NumericCharacterFieldLayout,
    pub good: NumericCharacterFieldLayout,
    pub bad: NumericCharacterFieldLayout,
    pub poor: NumericCharacterFieldLayout,
    pub fast: NumericCharacterFieldLayout,
    pub slow: NumericCharacterFieldLayout,
    pub combo_break: NumericCharacterFieldLayout,
}

impl ResultNumericCharacterLayout {
    /// Loads the fixed character cells measured in canonical-frame coordinates.
    ///
    /// Cells are ordered from the most-significant slot to the least-significant slot. A later
    /// recognizer may accept only leading blank cells followed by a contiguous digit sequence; it
    /// must not locate or shift cells from image content.
    ///
    /// # Errors
    /// Returns an error if the artifact does not bind the active canonical layout or if a cell
    /// leaves, overlaps, or reorders its owning field ROI.
    pub fn load() -> Result<Self, RecognitionError> {
        let layout: Self = serde_json::from_slice(LAYOUT_BYTES)?;
        let canonical = CanonicalLayout::load()?;
        if layout.schema != LAYOUT_SCHEMA
            || layout.canonical_frame_contract_id != super::CANONICAL_FRAME_CONTRACT_ID
            || layout.canonical_layout_sha256 != CanonicalLayout::sha256()
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        validate_level_variants(&layout.level, canonical.result.level)?;
        for (field, source, expected_cells, marker) in [
            (&layout.notes, canonical.result.notes, 4, false),
            (
                &layout.current_score,
                canonical.result.current_score,
                4,
                false,
            ),
            (
                &layout.previous_score,
                canonical.result.previous_score,
                4,
                true,
            ),
            (
                &layout.previous_miss_count,
                canonical.result.previous_miss_count,
                3,
                true,
            ),
            (&layout.miss_count, canonical.result.miss_count, 3, true),
            (&layout.pgreat, canonical.result.pgreat, 4, false),
            (&layout.great, canonical.result.great, 4, false),
            (&layout.good, canonical.result.good, 4, false),
            (&layout.bad, canonical.result.bad, 4, false),
            (&layout.poor, canonical.result.poor, 4, false),
            (&layout.fast, canonical.result.fast, 3, false),
            (&layout.slow, canonical.result.slow, 3, false),
            (&layout.combo_break, canonical.result.combo_break, 3, false),
        ] {
            validate_field(field, source, expected_cells, marker)?;
        }
        Ok(layout)
    }
}

fn validate_level_variants(
    variants: &[NumericCharacterLayoutVariant],
    source: Roi,
) -> Result<(), RecognitionError> {
    let expected = [("another", 1), ("beginner", 1), ("hyper", 1), ("hyper", 2)];
    if variants.len() != expected.len() {
        return Err(RecognitionError::InvalidCanonicalLayout);
    }
    for (variant, (difficulty, digits)) in variants.iter().zip(expected) {
        if variant.difficulty != difficulty
            || variant.displayed_digits != digits
            || variant.digit_cells.len() != digits
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        validate_cells(&variant.digit_cells, source)?;
    }
    Ok(())
}

fn validate_field(
    field: &NumericCharacterFieldLayout,
    source: Roi,
    expected_cells: usize,
    marker: bool,
) -> Result<(), RecognitionError> {
    if field.digit_cells.len() != expected_cells
        || field.not_displayed_marker.is_some() != marker
        || field
            .not_displayed_marker
            .is_some_and(|marker_roi| marker_roi != source)
    {
        return Err(RecognitionError::InvalidCanonicalLayout);
    }
    validate_cells(&field.digit_cells, source)
}

fn validate_cells(cells: &[Roi], source: Roi) -> Result<(), RecognitionError> {
    let mut previous_right = None;
    for cell in cells {
        cell.validate(super::CANONICAL_WIDTH, super::CANONICAL_HEIGHT)?;
        if !contains(source, *cell) || previous_right.is_some_and(|right| cell.x < right) {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        previous_right = cell.x.checked_add(cell.width);
    }
    Ok(())
}

fn contains(outer: Roi, inner: Roi) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner
            .x
            .checked_add(inner.width)
            .is_some_and(|right| right <= outer.x + outer.width)
        && inner
            .y
            .checked_add(inner.height)
            .is_some_and(|bottom| bottom <= outer.y + outer.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_numeric_character_layout_binds_the_canonical_result_rois() {
        let parsed: ResultNumericCharacterLayout = serde_json::from_slice(LAYOUT_BYTES).unwrap();
        assert_eq!(parsed.schema, LAYOUT_SCHEMA);
        assert_eq!(
            parsed.canonical_frame_contract_id,
            super::super::CANONICAL_FRAME_CONTRACT_ID
        );
        assert_eq!(parsed.canonical_layout_sha256, CanonicalLayout::sha256());
        let canonical = CanonicalLayout::load().unwrap();
        validate_level_variants(&parsed.level, canonical.result.level).unwrap();
        for (name, field, source, expected_cells, marker) in [
            ("notes", &parsed.notes, canonical.result.notes, 4, false),
            (
                "current_score",
                &parsed.current_score,
                canonical.result.current_score,
                4,
                false,
            ),
            (
                "previous_score",
                &parsed.previous_score,
                canonical.result.previous_score,
                4,
                true,
            ),
            (
                "previous_miss_count",
                &parsed.previous_miss_count,
                canonical.result.previous_miss_count,
                3,
                true,
            ),
            (
                "miss_count",
                &parsed.miss_count,
                canonical.result.miss_count,
                3,
                true,
            ),
            ("pgreat", &parsed.pgreat, canonical.result.pgreat, 4, false),
            ("great", &parsed.great, canonical.result.great, 4, false),
            ("good", &parsed.good, canonical.result.good, 4, false),
            ("bad", &parsed.bad, canonical.result.bad, 4, false),
            ("poor", &parsed.poor, canonical.result.poor, 4, false),
            ("fast", &parsed.fast, canonical.result.fast, 3, false),
            ("slow", &parsed.slow, canonical.result.slow, 3, false),
            (
                "combo_break",
                &parsed.combo_break,
                canonical.result.combo_break,
                3,
                false,
            ),
        ] {
            validate_field(field, source, expected_cells, marker)
                .unwrap_or_else(|_| panic!("invalid fixed cells for {name}"));
        }
        let layout = ResultNumericCharacterLayout::load().unwrap();
        assert_eq!(layout.level.len(), 4);
        assert_eq!(layout.notes.digit_cells.len(), 4);
        assert_eq!(layout.current_score.digit_cells.len(), 4);
        assert_eq!(layout.pgreat.digit_cells.len(), 4);
        assert_eq!(layout.combo_break.digit_cells.len(), 3);
        assert_eq!(
            layout.previous_miss_count.not_displayed_marker,
            Some(CanonicalLayout::load().unwrap().result.previous_miss_count)
        );
    }
}
