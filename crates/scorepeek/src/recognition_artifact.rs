use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{BufWriter, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use scorepeek::catalog::ScorepeekSongId;
use scorepeek::recognition::{
    CatalogCandidateEvidenceTable, ResultSongResolution, ScreenCatalogCandidateObservations,
    ScreenFieldObservations,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

const CATALOG_SCHEMA: &str = "scorepeek-recognition-catalog-evidence-v1";
const OBSERVATION_SCHEMA: &str = "scorepeek-recognition-observation-v1";
const MANIFEST_SCHEMA: &str = "scorepeek-recognition-evidence-manifest-v1";
const MAX_CATALOG_BYTES: u64 = 16 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: u64 = 256 * 1024 * 1024;
const MAX_OBSERVATIONS: usize = 3_600;

pub struct RecognitionArtifactWriter {
    root: PathBuf,
    observations: BufWriter<File>,
    observation_hasher: Sha256,
    observation_bytes: u64,
    observation_count: usize,
    catalog_sha256: Option<String>,
    catalog_entries: usize,
    profile_sha256: String,
    run_id: String,
}

#[derive(Serialize)]
struct StoredCatalog<'a> {
    schema: &'static str,
    profile_sha256: &'a str,
    catalog: &'a CatalogCandidateEvidenceTable,
}

#[derive(Serialize)]
struct StoredObservation<'a> {
    schema: &'static str,
    sequence: u64,
    source_pts_ms: u64,
    fields: StoredFields<'a>,
    candidates: StoredCandidates<'a>,
    decision: StoredDecision<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<RecognitionArtifactExpected<'a>>,
}

#[derive(Clone, Copy, Serialize)]
pub struct RecognitionArtifactExpected<'a> {
    pub episode_id: &'a str,
    pub song_id: Option<ScorepeekSongId>,
    pub clear_type: &'a str,
}

#[derive(Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
enum StoredDecision<'a> {
    Result {
        resolution: &'a ResultSongResolution,
    },
    MusicSelect {
        status: &'static str,
    },
}

#[derive(Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
enum StoredFields<'a> {
    Result {
        title: StoredText<'a>,
        artist: StoredText<'a>,
        clear_type: StoredText<'a>,
        difficulty: &'static str,
        level: &'static str,
        notes: &'static str,
        current_score: &'static str,
    },
    MusicSelect {
        central_title: StoredText<'a>,
        artist: StoredText<'a>,
        selected_chart: &'static str,
        active_list_title: StoredText<'a>,
    },
}

#[derive(Serialize)]
struct StoredText<'a> {
    input_width: usize,
    output_timesteps: usize,
    open_text: &'a str,
}

#[derive(Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
enum StoredCandidates<'a> {
    Result {
        comparison_key_id: &'static str,
        candidates: &'a [scorepeek::recognition::ResultSongCandidateObservation],
    },
    MusicSelect {
        comparison_key_id: &'static str,
        candidates: &'a [scorepeek::recognition::MusicSelectSongCandidateObservation],
    },
}

#[derive(Serialize)]
struct StoredManifest<'a> {
    schema: &'static str,
    run_id: &'a str,
    profile_sha256: &'a str,
    status: &'static str,
    catalog_sha256: &'a str,
    catalog_entries: usize,
    observations_sha256: String,
    observation_count: usize,
    observation_bytes: u64,
}

impl RecognitionArtifactWriter {
    pub fn create(root: &Path, run_id: String, profile_sha256: String) -> Result<Self, String> {
        let parent = root
            .parent()
            .ok_or_else(|| "recognition artifact root has no parent".to_owned())?;
        let parent_metadata = fs::symlink_metadata(parent)
            .map_err(|error| format!("recognition artifact parent is unavailable: {error}"))?;
        if !parent_metadata.file_type().is_dir() || parent_metadata.file_type().is_symlink() {
            return Err("recognition artifact parent must be a directory".to_owned());
        }
        DirBuilder::new()
            .mode(0o700)
            .create(root)
            .map_err(|error| format!("recognition artifact root creation failed: {error}"))?;
        sync_directory(parent)?;
        let observations = open_create_only(&root.join("observations.ndjson"))?;
        sync_directory(root)?;
        Ok(Self {
            root: root.to_owned(),
            observations: BufWriter::new(observations),
            observation_hasher: Sha256::new(),
            observation_bytes: 0,
            observation_count: 0,
            catalog_sha256: None,
            catalog_entries: 0,
            profile_sha256,
            run_id,
        })
    }

    pub fn record(
        &mut self,
        sequence: u64,
        source_pts_ms: u64,
        fields: &ScreenFieldObservations,
        candidates: &ScreenCatalogCandidateObservations,
        result_resolution: Option<&ResultSongResolution>,
        expected: Option<RecognitionArtifactExpected<'_>>,
    ) -> Result<(), String> {
        if self.observation_count >= MAX_OBSERVATIONS {
            return Err("recognition artifact observation capacity exceeded".to_owned());
        }
        self.ensure_catalog(candidates.catalog_evidence())?;
        let decision = match (fields, result_resolution) {
            (ScreenFieldObservations::Result(_), Some(resolution)) => {
                StoredDecision::Result { resolution }
            }
            (ScreenFieldObservations::MusicSelect(_), None) => StoredDecision::MusicSelect {
                status: "resolver_not_implemented",
            },
            _ => return Err("recognition artifact decision does not match screen".to_owned()),
        };
        let stored = StoredObservation {
            schema: OBSERVATION_SCHEMA,
            sequence,
            source_pts_ms,
            fields: StoredFields::from(fields),
            candidates: StoredCandidates::from(candidates),
            decision,
            expected,
        };
        let mut bytes = serde_json::to_vec(&stored)
            .map_err(|_| "recognition observation serialization failed".to_owned())?;
        bytes.push(b'\n');
        let next_bytes = self
            .observation_bytes
            .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| "recognition artifact byte count overflow".to_owned())?;
        if next_bytes > MAX_OBSERVATION_BYTES {
            return Err("recognition artifact byte capacity exceeded".to_owned());
        }
        self.observations
            .write_all(&bytes)
            .map_err(|error| format!("recognition observation write failed: {error}"))?;
        self.observation_hasher.update(&bytes);
        self.observation_bytes = next_bytes;
        self.observation_count += 1;
        Ok(())
    }

    pub fn finish(mut self, succeeded: bool) -> Result<String, String> {
        let catalog_sha256 = self
            .catalog_sha256
            .as_deref()
            .ok_or_else(|| "recognition artifact catalog evidence is missing".to_owned())?;
        self.observations
            .flush()
            .map_err(|error| format!("recognition observation flush failed: {error}"))?;
        self.observations
            .get_ref()
            .sync_all()
            .map_err(|error| format!("recognition observation sync failed: {error}"))?;
        let manifest = StoredManifest {
            schema: MANIFEST_SCHEMA,
            run_id: &self.run_id,
            profile_sha256: &self.profile_sha256,
            status: if succeeded { "success" } else { "error" },
            catalog_sha256,
            catalog_entries: self.catalog_entries,
            observations_sha256: hex_digest(self.observation_hasher.finalize()),
            observation_count: self.observation_count,
            observation_bytes: self.observation_bytes,
        };
        let bytes = serde_json::to_vec(&manifest)
            .map_err(|_| "recognition artifact manifest serialization failed".to_owned())?;
        let digest = sha256_bytes(&bytes);
        write_create_only(&self.root.join("manifest.json"), &bytes)?;
        sync_directory(&self.root)?;
        Ok(digest)
    }

    fn ensure_catalog(&mut self, catalog: &CatalogCandidateEvidenceTable) -> Result<(), String> {
        if self.catalog_sha256.is_some() {
            return Ok(());
        }
        let stored = StoredCatalog {
            schema: CATALOG_SCHEMA,
            profile_sha256: &self.profile_sha256,
            catalog,
        };
        let bytes = serde_json::to_vec(&stored)
            .map_err(|_| "recognition catalog serialization failed".to_owned())?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CATALOG_BYTES {
            return Err("recognition catalog artifact byte capacity exceeded".to_owned());
        }
        let digest = sha256_bytes(&bytes);
        write_create_only(&self.root.join("catalog.json"), &bytes)?;
        sync_directory(&self.root)?;
        self.catalog_entries = catalog.songs.len();
        self.catalog_sha256 = Some(digest);
        Ok(())
    }
}

impl<'a> From<&'a ScreenFieldObservations> for StoredFields<'a> {
    fn from(fields: &'a ScreenFieldObservations) -> Self {
        match fields {
            ScreenFieldObservations::Result(fields) => Self::Result {
                title: StoredText::from(&fields.title),
                artist: StoredText::from(&fields.artist),
                clear_type: StoredText::from(&fields.clear_type),
                difficulty: "observer_not_implemented",
                level: "observer_not_implemented",
                notes: "observer_not_implemented",
                current_score: "observer_not_implemented",
            },
            ScreenFieldObservations::MusicSelect(fields) => Self::MusicSelect {
                central_title: StoredText::from(&fields.central_title),
                artist: StoredText::from(&fields.artist),
                selected_chart: "observer_not_implemented",
                active_list_title: StoredText::from(&fields.active_list_title),
            },
        }
    }
}

impl<'a> From<&'a scorepeek::recognition::DynamicTextObservation> for StoredText<'a> {
    fn from(observation: &'a scorepeek::recognition::DynamicTextObservation) -> Self {
        Self {
            input_width: observation.input_width,
            output_timesteps: observation.output_timesteps,
            open_text: &observation.open_text,
        }
    }
}

impl<'a> From<&'a ScreenCatalogCandidateObservations> for StoredCandidates<'a> {
    fn from(candidates: &'a ScreenCatalogCandidateObservations) -> Self {
        match candidates {
            ScreenCatalogCandidateObservations::Result {
                comparison_key_id,
                candidates,
                ..
            } => Self::Result {
                comparison_key_id,
                candidates,
            },
            ScreenCatalogCandidateObservations::MusicSelect {
                comparison_key_id,
                candidates,
                ..
            } => Self::MusicSelect {
                comparison_key_id,
                candidates,
            },
        }
    }
}

fn open_create_only(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("recognition artifact file creation failed: {error}"))
}

fn write_create_only(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = open_create_only(path)?;
    file.write_all(bytes)
        .map_err(|error| format!("recognition artifact write failed: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("recognition artifact sync failed: {error}"))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("recognition artifact directory sync failed: {error}"))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::Arc;

    use scorepeek::recognition::{
        CatalogCandidateEvidenceTable, DynamicTextObservation, FieldNotObserved,
        FieldNotObservedReason, RESULT_SONG_RESOLVER_ID, ResultScreenFieldObservations,
        ResultSongResolution, ResultSongUnknownReason,
    };

    use super::*;

    fn result_fields() -> ScreenFieldObservations {
        let text = |value: &str| DynamicTextObservation {
            input_width: 64,
            output_timesteps: 12,
            open_text: value.to_owned(),
        };
        let missing = FieldNotObserved {
            reason: FieldNotObservedReason::ObserverNotImplemented,
        };
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text("ABSOLUTE EVIL"),
            artist: text("Yuta Imai"),
            clear_type: text("FAILED"),
            difficulty: missing,
            level: missing,
            notes: missing,
            current_score: missing,
        })
    }

    fn empty_candidates() -> ScreenCatalogCandidateObservations {
        let song_id = serde_json::from_str("\"6ef33da9-090a-500c-844a-8bffd14de63f\"")
            .expect("fixture song ID is valid");
        ScreenCatalogCandidateObservations::Result {
            comparison_key_id: "test-comparison-v1",
            catalog: Arc::new(CatalogCandidateEvidenceTable {
                comparison_key_id: "test-comparison-v1",
                songs: vec![scorepeek::recognition::CatalogCandidateSongEvidence {
                    song_id,
                    title: scorepeek::recognition::CatalogCandidateTextEvidence {
                        display: vec!["ABSOLUTE EVIL".to_owned()],
                        exact: vec!["ABSOLUTEEVIL".to_owned()],
                        folded: vec!["ABSOLUTEEVIL".to_owned()],
                    },
                    artist: scorepeek::recognition::CatalogCandidateTextEvidence {
                        display: vec!["Yuta Imai".to_owned()],
                        exact: vec!["YutaImai".to_owned()],
                        folded: Vec::new(),
                    },
                }],
            }),
            candidates: Vec::new(),
        }
    }

    fn empty_catalog_candidates() -> ScreenCatalogCandidateObservations {
        ScreenCatalogCandidateObservations::Result {
            comparison_key_id: "test-comparison-v1",
            catalog: Arc::new(CatalogCandidateEvidenceTable {
                comparison_key_id: "test-comparison-v1",
                songs: Vec::new(),
            }),
            candidates: Vec::new(),
        }
    }

    #[test]
    fn artifact_retains_exact_values_and_finalizes_last() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("run");
        let mut writer =
            RecognitionArtifactWriter::create(&root, "simulation-001".to_owned(), "a".repeat(64))
                .unwrap();
        writer
            .record(
                7,
                140_000,
                &result_fields(),
                &empty_candidates(),
                Some(&ResultSongResolution::Unknown {
                    resolver_id: RESULT_SONG_RESOLVER_ID,
                    reason: ResultSongUnknownReason::EmptyTitle,
                    selected: None,
                    runner_up: None,
                    title_edit_margin: None,
                }),
                None,
            )
            .unwrap();
        let digest = writer.finish(true).unwrap();

        assert_eq!(digest.len(), 64);
        let observation = fs::read_to_string(root.join("observations.ndjson")).unwrap();
        assert!(observation.contains("ABSOLUTE EVIL"));
        assert!(observation.contains("Yuta Imai"));
        assert!(observation.contains("FAILED"));
        let catalog = fs::read_to_string(root.join("catalog.json")).unwrap();
        assert!(catalog.contains("test-comparison-v1"));
        assert!(catalog.contains("ABSOLUTE EVIL"));
        assert!(catalog.contains("ABSOLUTEEVIL"));
        assert!(catalog.contains("Yuta Imai"));
        let manifest = fs::read_to_string(root.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"observation_count\":1"));
        assert_eq!(
            fs::metadata(root.join("catalog.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn artifact_root_is_create_only() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("run");
        fs::create_dir(&root).unwrap();
        assert!(
            RecognitionArtifactWriter::create(&root, "simulation-001".to_owned(), "a".repeat(64),)
                .is_err()
        );
    }

    #[test]
    fn failed_empty_catalog_run_retains_observation_and_expected_values() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("run");
        let expected_song_id = serde_json::from_str("\"6ef33da9-090a-500c-844a-8bffd14de63f\"")
            .expect("fixture song ID is valid");
        let mut writer =
            RecognitionArtifactWriter::create(&root, "simulation-002".to_owned(), "b".repeat(64))
                .unwrap();
        writer
            .record(
                8,
                141_000,
                &result_fields(),
                &empty_catalog_candidates(),
                Some(&ResultSongResolution::Unknown {
                    resolver_id: RESULT_SONG_RESOLVER_ID,
                    reason: ResultSongUnknownReason::NoCatalogCandidates,
                    selected: None,
                    runner_up: None,
                    title_edit_margin: None,
                }),
                Some(RecognitionArtifactExpected {
                    episode_id: "failed-result-1",
                    song_id: Some(expected_song_id),
                    clear_type: "FAILED",
                }),
            )
            .unwrap();
        writer.finish(false).unwrap();

        let catalog: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("catalog.json")).unwrap()).unwrap();
        assert_eq!(catalog["catalog"]["songs"], serde_json::json!([]));
        let observation = fs::read_to_string(root.join("observations.ndjson")).unwrap();
        assert!(observation.contains("ABSOLUTE EVIL"));
        assert!(observation.contains("no_catalog_candidates"));
        assert!(observation.contains("failed-result-1"));
        assert!(observation.contains("6ef33da9-090a-500c-844a-8bffd14de63f"));
        let manifest = fs::read_to_string(root.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"status\":\"error\""));
        assert!(manifest.contains("\"catalog_entries\":0"));
        assert!(manifest.contains("\"observation_count\":1"));
    }
}
