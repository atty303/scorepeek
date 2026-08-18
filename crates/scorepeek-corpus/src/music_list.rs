use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{CorpusError, validate_opaque_id, validate_sha256, validate_token};

const OBSERVATION_SCHEMA: &str = "scorepeek-private-music-list-row-observation-draft-v1";
const MAX_DOCUMENT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_OBSERVATIONS: usize = 250_000;
const MUSIC_LIST_SLOTS: u8 = 20;
const MUSIC_LIST_ROW_RGB_VALUES: u64 = 475 * 45 * 3;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListRowObservationDocument {
    schema: String,
    catalog_sha256: String,
    source_manifest_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_layout_sha256: String,
    observations: Vec<MusicListRowObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListRowObservation {
    observation_id: String,
    slot: u8,
    frame: MusicListRowFrame,
    annotation: MusicListRowAnnotation,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListRowFrame {
    frame_extraction_sha256: String,
    crop_manifest_sha256: String,
    frame_id: String,
    source_pts: i64,
    decode_index: u64,
    crop_file_sha256: String,
    crop_pixel_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum MusicListRowAnnotation {
    Stationary {
        adjacent_frame: MusicListRowFrame,
        reported_rgb_l1_sum: u64,
        reported_compared_rgb_values: u64,
    },
    Scrolling {
        adjacent_frame: MusicListRowFrame,
        reported_rgb_l1_sum: u64,
        reported_compared_rgb_values: u64,
    },
    Selected,
    Clipped {
        edge: ClippedEdge,
    },
    NonTitle {
        kind: NonTitleKind,
    },
    Unknown {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClippedEdge {
    Left,
    Right,
    Both,
    Obscured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum NonTitleKind {
    Empty,
    Separator,
    Overlay,
    OtherUi,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicListRowObservationSummary {
    pub schema: &'static str,
    pub evidence_verified: bool,
    pub catalog_sha256: String,
    pub observation_count: usize,
    pub stationary_count: usize,
    pub scrolling_count: usize,
    pub selected_count: usize,
    pub clipped_count: usize,
    pub non_title_count: usize,
    pub unknown_count: usize,
}

/// Inspects the shape of a private music-list row observation draft.
///
/// This boundary validates only canonical JSON, identifiers, state shape, and reported measurement
/// ranges. It does not read the referenced canonical extraction or crop artifacts and therefore
/// never promotes the draft into verified calibration or label evidence.
///
/// # Errors
/// Returns an error for a non-canonical document, an invalid binding, duplicate observation IDs,
/// or temporal evidence that does not compare adjacent decoded frames.
pub fn inspect_music_list_row_observation_draft(
    path: impl AsRef<Path>,
) -> Result<MusicListRowObservationSummary, CorpusError> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return Err(invalid("observation document path must be absolute"));
    }
    let bytes = read_bounded_regular(path)?;
    let document: MusicListRowObservationDocument = serde_json::from_slice(&bytes)?;
    let mut canonical = serde_json::to_vec(&document)?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err(invalid("observation document must be canonical JSON"));
    }
    document.validate()
}

fn read_bounded_regular(path: &Path) -> Result<Vec<u8>, CorpusError> {
    read_bounded_regular_after_metadata(path, || Ok(()))
}

fn read_bounded_regular_after_metadata(
    path: &Path,
    after_metadata: impl FnOnce() -> Result<(), CorpusError>,
) -> Result<Vec<u8>, CorpusError> {
    let path_metadata = path.symlink_metadata()?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(invalid("observation document is not a regular file"));
    }
    after_metadata()?;
    let mut file = File::open(path)?;
    let opened_metadata = file.metadata()?;
    if !opened_metadata.is_file()
        || opened_metadata.dev() != path_metadata.dev()
        || opened_metadata.ino() != path_metadata.ino()
        || opened_metadata.len() == 0
        || opened_metadata.len() > MAX_DOCUMENT_BYTES
    {
        return Err(invalid(
            "observation document is not a bounded stable regular file",
        ));
    }
    let capacity = usize::try_from(opened_metadata.len())
        .map_err(|_| invalid("observation document length is not addressable"))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let final_metadata = file.metadata()?;
    if bytes.len() as u64 != opened_metadata.len()
        || bytes.len() as u64 != final_metadata.len()
        || bytes.len() as u64 > MAX_DOCUMENT_BYTES
        || final_metadata.dev() != opened_metadata.dev()
        || final_metadata.ino() != opened_metadata.ino()
    {
        return Err(invalid("observation document changed while reading"));
    }
    Ok(bytes)
}

impl MusicListRowObservationDocument {
    fn validate(self) -> Result<MusicListRowObservationSummary, CorpusError> {
        if self.schema != OBSERVATION_SCHEMA {
            return Err(invalid("unsupported observation schema"));
        }
        for (value, field) in [
            (&self.catalog_sha256, "catalog_sha256"),
            (&self.source_manifest_sha256, "source_manifest_sha256"),
            (
                &self.normalizer_artifact_sha256,
                "normalizer_artifact_sha256",
            ),
            (&self.canonical_layout_sha256, "canonical_layout_sha256"),
        ] {
            validate_sha256(value, field, crate::ErrorContext::Replay)?;
        }
        validate_token(
            &self.capture_profile_id,
            "capture_profile_id",
            crate::ErrorContext::Replay,
        )?;
        if self.observations.is_empty() || self.observations.len() > MAX_OBSERVATIONS {
            return Err(invalid("observation count is outside bounds"));
        }
        let mut ids = BTreeSet::new();
        let mut rows = BTreeSet::new();
        let mut counts = [0_usize; 6];
        for observation in self.observations {
            validate_opaque_id(
                &observation.observation_id,
                "observation_id",
                crate::ErrorContext::Replay,
            )?;
            if !ids.insert(observation.observation_id) {
                return Err(invalid("observation IDs must be unique"));
            }
            if !rows.insert((
                observation.frame.frame_extraction_sha256.clone(),
                observation.frame.frame_id.clone(),
                observation.slot,
            )) {
                return Err(invalid("each geometric row may be annotated only once"));
            }
            if observation.slot >= MUSIC_LIST_SLOTS {
                return Err(invalid("music-list slot is outside the shared layout"));
            }
            observation.frame.validate()?;
            match observation.annotation {
                MusicListRowAnnotation::Stationary {
                    adjacent_frame,
                    reported_rgb_l1_sum,
                    reported_compared_rgb_values,
                } => {
                    validate_motion(
                        &observation.frame,
                        &adjacent_frame,
                        reported_rgb_l1_sum,
                        reported_compared_rgb_values,
                    )?;
                    counts[0] += 1;
                }
                MusicListRowAnnotation::Scrolling {
                    adjacent_frame,
                    reported_rgb_l1_sum,
                    reported_compared_rgb_values,
                } => {
                    validate_motion(
                        &observation.frame,
                        &adjacent_frame,
                        reported_rgb_l1_sum,
                        reported_compared_rgb_values,
                    )?;
                    counts[1] += 1;
                }
                MusicListRowAnnotation::Selected => counts[2] += 1,
                MusicListRowAnnotation::Clipped { .. } => counts[3] += 1,
                MusicListRowAnnotation::NonTitle { .. } => counts[4] += 1,
                MusicListRowAnnotation::Unknown { reason } => {
                    validate_token(&reason, "unknown reason", crate::ErrorContext::Replay)?;
                    counts[5] += 1;
                }
            }
        }
        Ok(MusicListRowObservationSummary {
            schema: "scorepeek-music-list-row-observation-draft-inspection-v1",
            evidence_verified: false,
            catalog_sha256: self.catalog_sha256,
            observation_count: counts.iter().sum(),
            stationary_count: counts[0],
            scrolling_count: counts[1],
            selected_count: counts[2],
            clipped_count: counts[3],
            non_title_count: counts[4],
            unknown_count: counts[5],
        })
    }
}

impl MusicListRowFrame {
    fn validate(&self) -> Result<(), CorpusError> {
        for (value, field) in [
            (&self.frame_extraction_sha256, "frame_extraction_sha256"),
            (&self.crop_manifest_sha256, "crop_manifest_sha256"),
            (&self.crop_file_sha256, "crop_file_sha256"),
            (&self.crop_pixel_sha256, "crop_pixel_sha256"),
        ] {
            validate_sha256(value, field, crate::ErrorContext::Replay)?;
        }
        validate_opaque_id(&self.frame_id, "frame_id", crate::ErrorContext::Replay)
    }
}

fn validate_motion(
    frame: &MusicListRowFrame,
    adjacent: &MusicListRowFrame,
    reported_rgb_l1_sum: u64,
    reported_compared_rgb_values: u64,
) -> Result<(), CorpusError> {
    adjacent.validate()?;
    if frame.frame_extraction_sha256 != adjacent.frame_extraction_sha256
        || frame.frame_id == adjacent.frame_id
        || frame.decode_index.abs_diff(adjacent.decode_index) != 1
        || frame.source_pts == adjacent.source_pts
        || reported_compared_rgb_values != MUSIC_LIST_ROW_RGB_VALUES
        || reported_rgb_l1_sum > MUSIC_LIST_ROW_RGB_VALUES * 255
    {
        return Err(invalid(
            "stationary and scrolling states require a valid adjacent-frame RGB comparison",
        ));
    }
    Ok(())
}

fn invalid(detail: &str) -> CorpusError {
    CorpusError::InvalidReplay(detail.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{Seek as _, SeekFrom, Write as _};

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn canonical(value: &serde_json::Value) -> Vec<u8> {
        let document: MusicListRowObservationDocument =
            serde_json::from_value(value.clone()).unwrap();
        let mut bytes = serde_json::to_vec(&document).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn frame(index: u64) -> serde_json::Value {
        json!({
            "frame_extraction_sha256": "1".repeat(64),
            "crop_manifest_sha256": format!("{index:064x}"),
            "frame_id": format!("frame-{index}"),
            "source_pts": i64::try_from(index).unwrap() * 1_000,
            "decode_index": index,
            "crop_file_sha256": "a".repeat(64),
            "crop_pixel_sha256": "b".repeat(64)
        })
    }

    fn document(annotation: &serde_json::Value) -> serde_json::Value {
        json!({
            "schema": OBSERVATION_SCHEMA,
            "catalog_sha256": "c".repeat(64),
            "source_manifest_sha256": "d".repeat(64),
            "capture_profile_id": "profile-v1",
            "normalizer_artifact_sha256": "e".repeat(64),
            "canonical_layout_sha256": "f".repeat(64),
            "observations": [{
                "observation_id": "observation-1",
                "slot": 3,
                "frame": frame(10),
                "annotation": annotation
            }]
        })
    }

    #[test]
    fn stationary_and_scrolling_bind_adjacent_frame_measurements() {
        for state in ["stationary", "scrolling"] {
            let value = document(&json!({
                "state": state,
                "adjacent_frame": frame(11),
                "reported_rgb_l1_sum": 1234,
                "reported_compared_rgb_values": MUSIC_LIST_ROW_RGB_VALUES
            }));
            let directory = tempdir().unwrap();
            let path = directory.path().join(format!("{state}.json"));
            fs::write(&path, canonical(&value)).unwrap();
            let summary = inspect_music_list_row_observation_draft(&path).unwrap();
            assert!(!summary.evidence_verified);
            assert_eq!(summary.observation_count, 1);
            assert_eq!(summary.stationary_count + summary.scrolling_count, 1);
        }
    }

    #[test]
    fn every_non_training_state_is_explicit_and_value_free() {
        for classification in [
            json!({"state": "selected"}),
            json!({"state": "clipped", "edge": "left"}),
            json!({"state": "non_title", "kind": "separator"}),
            json!({"state": "unknown", "reason": "unobservable"}),
        ] {
            let value = document(&classification);
            let directory = tempdir().unwrap();
            let path = directory.path().join("observations.json");
            fs::write(&path, canonical(&value)).unwrap();
            assert_eq!(
                inspect_music_list_row_observation_draft(&path)
                    .unwrap()
                    .observation_count,
                1
            );
        }
    }

    #[test]
    fn temporal_states_reject_non_adjacent_or_noncanonical_evidence() {
        let value = document(&json!({
            "state": "stationary",
            "adjacent_frame": frame(12),
            "reported_rgb_l1_sum": 1,
            "reported_compared_rgb_values": MUSIC_LIST_ROW_RGB_VALUES
        }));
        let directory = tempdir().unwrap();
        let path = directory.path().join("observations.json");
        fs::write(&path, canonical(&value)).unwrap();
        assert!(inspect_music_list_row_observation_draft(&path).is_err());

        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        assert!(inspect_music_list_row_observation_draft(&path).is_err());
    }

    #[test]
    fn one_geometric_row_cannot_have_conflicting_annotations() {
        let mut value = document(&json!({"state": "selected"}));
        let duplicate = json!({
            "observation_id": "observation-2",
            "slot": 3,
            "frame": frame(10),
            "annotation": {"state": "clipped", "edge": "left"}
        });
        value["observations"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        let directory = tempdir().unwrap();
        let path = directory.path().join("observations.json");
        fs::write(&path, canonical(&value)).unwrap();
        assert!(inspect_music_list_row_observation_draft(&path).is_err());
    }

    #[test]
    fn bounded_reader_rejects_growth_beyond_the_document_limit() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("oversized.json");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.set_len(MAX_DOCUMENT_BYTES + 1).unwrap();
        file.seek(SeekFrom::Start(MAX_DOCUMENT_BYTES)).unwrap();
        file.write_all(b"x").unwrap();
        drop(file);
        assert!(read_bounded_regular(&path).is_err());
    }

    #[test]
    fn bounded_reader_rejects_path_replacement_after_metadata() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("draft.json");
        let replacement = directory.path().join("replacement.json");
        fs::write(&path, b"{}\n").unwrap();
        let file = File::create(&replacement).unwrap();
        file.set_len(MAX_DOCUMENT_BYTES + 1).unwrap();
        drop(file);
        let result = read_bounded_regular_after_metadata(&path, || {
            fs::rename(&replacement, &path)?;
            Ok(())
        });
        assert!(result.is_err());
    }
}
