//! Tachi response and fixture schema decoders.

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::source::common::{
    AdapterError, SourceRevision, hex_digest, parse_bounded_json, snapshot_from_parts,
    validate_record_count, validate_text,
};
#[cfg(test)]
use crate::source::common::{
    Fixture, FixtureChart, parse_fixture, snapshot, validate_charts, validate_source_id,
};
use scorepeek_core::catalog::{
    Chart, ChartKey, Difficulty, DisplayVariantKind, PlayType, SourceChartObservation,
    SourceObservation, SourcePolicy, SourceSnapshot, SourceTitleObservation, TachiObservation,
};

pub(crate) const MAX_TACHI_SONG_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_TACHI_CHART_BYTES: usize = 24 * 1024 * 1024;
const MAX_TACHI_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TACHI_CHART_RECORDS: usize = 50_000;
const MAX_TACHI_VARIANTS: usize = 64;

#[cfg(test)]
pub struct TachiFixtureAdapter;
pub struct TachiLiveAdapter;

#[cfg(test)]
impl TachiFixtureAdapter {
    /// Parses the bounded, synthetic Tachi fixture contract.
    ///
    /// # Errors
    ///
    /// Returns an error when the fixture exceeds a bound, violates its versioned schema, or
    /// contains an invalid or duplicate record.
    pub fn parse(bytes: &[u8], revision: SourceRevision) -> Result<SourceSnapshot, AdapterError> {
        let fixture: TachiFixture = parse_fixture(bytes, "scorepeek-tachi-fixture-v1")?;
        validate_record_count(fixture.records.len())?;
        let mut source_ids = BTreeSet::new();
        let mut observations = Vec::with_capacity(fixture.records.len());
        for record in fixture.records {
            validate_source_id(&record.source_song_id)?;
            if !source_ids.insert(record.source_song_id.clone()) {
                return Err(AdapterError::DuplicateSourceId(record.source_song_id));
            }
            observations.push(SourceObservation::Tachi(TachiObservation {
                source_song_id: record.source_song_id,
                title_variants: BTreeSet::from([SourceTitleObservation {
                    value: validate_text("title", record.title)?,
                    kind: record.title_kind,
                }]),
                artist: validate_text("artist", record.artist)?,
                version: validate_text("version", record.version)?,
                charts: validate_charts(&record.charts)?,
                primary_infinitas: record.primary_infinitas,
            }));
        }
        snapshot(SourcePolicy::tachi(), revision, bytes, observations)
    }
}

impl TachiLiveAdapter {
    /// Parses the three pinned Tachi IIDX seed collections without executing repository code.
    ///
    /// # Errors
    ///
    /// Returns an error when a collection exceeds its bound, violates the strict live schema,
    /// contains invalid or duplicate IDs, has inconsistent primary charts, references an absent
    /// song, or is not pinned to a Git commit.
    pub fn parse(
        songs_bytes: &[u8],
        single_charts_bytes: &[u8],
        double_charts_bytes: &[u8],
        revision: SourceRevision,
    ) -> Result<SourceSnapshot, AdapterError> {
        let byte_size = songs_bytes
            .len()
            .checked_add(single_charts_bytes.len())
            .and_then(|size| size.checked_add(double_charts_bytes.len()))
            .ok_or(AdapterError::SourceTooLarge {
                actual: usize::MAX,
                maximum: MAX_TACHI_BUNDLE_BYTES,
            })?;
        if byte_size > MAX_TACHI_BUNDLE_BYTES {
            return Err(AdapterError::SourceTooLarge {
                actual: byte_size,
                maximum: MAX_TACHI_BUNDLE_BYTES,
            });
        }
        let songs: Vec<TachiLiveSong> = parse_bounded_json(songs_bytes, MAX_TACHI_SONG_BYTES)?;
        validate_record_count(songs.len())?;
        let single_charts: Vec<TachiLiveChart> =
            parse_bounded_json(single_charts_bytes, MAX_TACHI_CHART_BYTES)?;
        let double_charts: Vec<TachiLiveChart> =
            parse_bounded_json(double_charts_bytes, MAX_TACHI_CHART_BYTES)?;
        validate_tachi_chart_count(single_charts.len())?;
        validate_tachi_chart_count(double_charts.len())?;

        let mut source_chart_ids = BTreeSet::new();
        let mut referenced_song_ids = BTreeSet::new();
        let mut chart_keys = BTreeSet::new();
        let mut charts_by_song = BTreeMap::<String, Vec<SourceChartObservation>>::new();
        let mut accepted_chart_count = 0_usize;
        append_tachi_charts(
            single_charts,
            PlayType::Single,
            &mut source_chart_ids,
            &mut referenced_song_ids,
            &mut chart_keys,
            &mut charts_by_song,
            &mut accepted_chart_count,
        )?;
        append_tachi_charts(
            double_charts,
            PlayType::Double,
            &mut source_chart_ids,
            &mut referenced_song_ids,
            &mut chart_keys,
            &mut charts_by_song,
            &mut accepted_chart_count,
        )?;

        let song_count = songs.len();
        let observations = build_tachi_observations(songs, charts_by_song, &referenced_song_ids)?;

        let content_sha256 = tachi_bundle_digest([
            ("db/seeds/songs-iidx.json", songs_bytes),
            ("db/seeds/charts-iidx-sp.json", single_charts_bytes),
            ("db/seeds/charts-iidx-dp.json", double_charts_bytes),
        ]);
        snapshot_from_parts(
            SourcePolicy::tachi(),
            revision,
            content_sha256,
            byte_size,
            song_count + accepted_chart_count,
            observations,
        )
    }
}

fn build_tachi_observations(
    songs: Vec<TachiLiveSong>,
    mut charts_by_song: BTreeMap<String, Vec<SourceChartObservation>>,
    referenced_song_ids: &BTreeSet<String>,
) -> Result<Vec<SourceObservation>, AdapterError> {
    let mut source_song_ids = BTreeSet::new();
    let mut observations = Vec::with_capacity(songs.len());
    for song in songs {
        validate_tachi_id("source_song_id", &song.id, b'S')?;
        if !source_song_ids.insert(song.id.clone()) {
            return Err(AdapterError::DuplicateSourceId(song.id));
        }
        if song.legacy_song_id == 0 {
            return Err(AdapterError::InvalidField {
                field: "legacySongID",
                detail: "must be positive".to_owned(),
            });
        }
        validate_string_list("altTitles", &song.alt_titles, MAX_TACHI_VARIANTS)?;
        validate_string_list("searchTerms", &song.search_terms, MAX_TACHI_VARIANTS)?;
        let title_variants =
            tachi_title_variants(song.title, song.alt_titles, song.data.eamusement_csv_title)?;
        validate_text("genre", song.data.genre)?;
        let mut charts = charts_by_song.remove(&song.id).unwrap_or_default();
        charts.sort_by(|left, right| left.chart.cmp(&right.chart));
        let primary_infinitas = charts
            .iter()
            .any(|chart| chart.product_versions.contains("inf"));
        observations.push(SourceObservation::Tachi(TachiObservation {
            source_song_id: song.id,
            title_variants,
            artist: validate_text("artist", song.artist)?,
            version: validate_text("displayVersion", song.data.display_version)?,
            charts,
            primary_infinitas,
        }));
    }
    if !charts_by_song.is_empty() || !referenced_song_ids.is_subset(&source_song_ids) {
        return Err(AdapterError::InvalidField {
            field: "chart.songID",
            detail: "references an absent song".to_owned(),
        });
    }
    Ok(observations)
}

fn tachi_title_variants(
    title: String,
    alternate_titles: Vec<String>,
    eamusement_csv_title: Option<String>,
) -> Result<BTreeSet<SourceTitleObservation>, AdapterError> {
    let mut variants = BTreeSet::from([SourceTitleObservation {
        value: validate_text("title", title)?,
        kind: DisplayVariantKind::InGameDisplay,
    }]);
    for title in alternate_titles {
        variants.insert(SourceTitleObservation {
            value: validate_text("altTitles", title)?,
            kind: DisplayVariantKind::AlternateDisplay,
        });
    }
    if let Some(title) = eamusement_csv_title {
        variants.insert(SourceTitleObservation {
            value: validate_text("eamusementCsvTitle", title)?,
            kind: DisplayVariantKind::EamusementCsv,
        });
    }
    Ok(variants)
}

fn append_tachi_charts(
    records: Vec<TachiLiveChart>,
    play_type: PlayType,
    source_chart_ids: &mut BTreeSet<String>,
    referenced_song_ids: &mut BTreeSet<String>,
    chart_keys: &mut BTreeSet<(String, ChartKey)>,
    charts_by_song: &mut BTreeMap<String, Vec<SourceChartObservation>>,
    accepted_chart_count: &mut usize,
) -> Result<(), AdapterError> {
    for record in records {
        validate_tachi_id("chart.id", &record.id, b'C')?;
        validate_tachi_id("chart.songID", &record.song_id, b'S')?;
        if !source_chart_ids.insert(record.id.clone()) {
            return Err(AdapterError::DuplicateSourceId(record.id));
        }
        referenced_song_ids.insert(record.song_id.clone());
        validate_text("chart.legacyChartID", record.legacy_chart_id)?;
        validate_tachi_level(&record.level, record.level_num)?;
        if record.data.notecount == 0 {
            return Err(AdapterError::InvalidField {
                field: "chart.data.notecount",
                detail: "must be positive".to_owned(),
            });
        }
        if record.versions.is_empty() || record.versions.len() > MAX_TACHI_VARIANTS {
            return Err(AdapterError::InvalidField {
                field: "chart.versions",
                detail: "must contain between 1 and 64 values".to_owned(),
            });
        }
        let product_versions: BTreeSet<_> = record
            .versions
            .into_iter()
            .map(|version| validate_text("chart.version", version))
            .collect::<Result<_, _>>()?;
        if product_versions.len() > MAX_TACHI_VARIANTS {
            return Err(AdapterError::InvalidField {
                field: "chart.versions",
                detail: "contains too many unique values".to_owned(),
            });
        }
        let Some(difficulty) = record.difficulty.standard() else {
            continue;
        };
        if !record.is_primary {
            continue;
        }
        let key = ChartKey {
            play_type,
            difficulty,
        };
        if !chart_keys.insert((record.song_id.clone(), key)) {
            return Err(AdapterError::DuplicateChart {
                source_id: record.song_id,
                chart: key,
            });
        }
        if !(1..=12).contains(&record.level_num) {
            return Err(AdapterError::InvalidField {
                field: "chart.levelNum",
                detail: "primary standard chart level must be between 1 and 12".to_owned(),
            });
        }
        charts_by_song
            .entry(record.song_id)
            .or_default()
            .push(SourceChartObservation {
                chart: Chart {
                    key,
                    level: record.level_num,
                    notes: record.data.notecount,
                },
                source_chart_id: record.id,
                product_versions,
                primary: true,
            });
        *accepted_chart_count = accepted_chart_count.saturating_add(1);
    }
    Ok(())
}

fn validate_tachi_chart_count(actual: usize) -> Result<(), AdapterError> {
    if actual > MAX_TACHI_CHART_RECORDS {
        return Err(AdapterError::TooManyRecords {
            actual,
            maximum: MAX_TACHI_CHART_RECORDS,
        });
    }
    Ok(())
}

fn validate_tachi_level(level: &str, level_num: u8) -> Result<(), AdapterError> {
    if (level == "?" && level_num == 0) || (level_num > 0 && level.parse::<u8>() == Ok(level_num)) {
        return Ok(());
    }
    Err(AdapterError::InvalidField {
        field: "chart.level",
        detail: "must agree with levelNum or use ? with zero".to_owned(),
    })
}

fn validate_tachi_id(field: &'static str, value: &str, prefix: u8) -> Result<(), AdapterError> {
    if value.len() == 20
        && value.as_bytes().first() == Some(&prefix)
        && value.as_bytes()[1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Ok(());
    }
    Err(AdapterError::InvalidField {
        field,
        detail: "must be a Tachi prefixed 19-digit lowercase hexadecimal ID".to_owned(),
    })
}

fn validate_string_list(
    field: &'static str,
    values: &[String],
    maximum: usize,
) -> Result<(), AdapterError> {
    if values.len() > maximum {
        return Err(AdapterError::InvalidField {
            field,
            detail: format!("must contain at most {maximum} values"),
        });
    }
    let mut unique = BTreeSet::new();
    for value in values {
        validate_text(field, value.clone())?;
        if !unique.insert(value) {
            return Err(AdapterError::InvalidField {
                field,
                detail: "must not contain duplicates".to_owned(),
            });
        }
    }
    Ok(())
}

fn tachi_bundle_digest<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"scorepeek-tachi-live-bundle-v1\0");
    for (path, bytes) in files {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    hex_digest(&digest.finalize())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(test)]
struct TachiFixture {
    schema: String,
    records: Vec<TachiRecord>,
}

#[cfg(test)]
impl Fixture for TachiFixture {
    fn schema(&self) -> &str {
        &self.schema
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(test)]
struct TachiRecord {
    source_song_id: String,
    title: String,
    title_kind: DisplayVariantKind,
    artist: String,
    version: String,
    charts: Vec<FixtureChart>,
    primary_infinitas: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiLiveSong {
    #[serde(rename = "altTitles")]
    alt_titles: Vec<String>,
    artist: String,
    data: TachiLiveSongData,
    id: String,
    #[serde(rename = "legacySongID")]
    legacy_song_id: u64,
    #[serde(rename = "searchTerms")]
    search_terms: Vec<String>,
    title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiLiveSongData {
    #[serde(rename = "displayVersion")]
    display_version: String,
    #[serde(
        default,
        rename = "eamusementCsvTitle",
        deserialize_with = "deserialize_optional_nonnull_string"
    )]
    eamusement_csv_title: Option<String>,
    genre: String,
}

fn deserialize_optional_nonnull_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiLiveChart {
    data: TachiLiveChartData,
    difficulty: TachiDifficulty,
    id: String,
    #[serde(rename = "isPrimary")]
    is_primary: bool,
    #[serde(rename = "legacyChartID")]
    legacy_chart_id: String,
    level: String,
    #[serde(rename = "levelNum")]
    level_num: u8,
    #[serde(rename = "songID")]
    song_id: String,
    versions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiLiveChartData {
    #[serde(default, rename = "2dxtraSet")]
    _two_dxtra_set: Option<IgnoredAny>,
    #[serde(default, rename = "bpiCoefficient")]
    _bpi_coefficient: Option<IgnoredAny>,
    #[serde(default, rename = "dpTier")]
    _dp_tier: Option<IgnoredAny>,
    #[serde(default, rename = "exhcTier")]
    _exhc_tier: Option<IgnoredAny>,
    #[serde(default, rename = "hashSHA256")]
    _hash_sha256: Option<IgnoredAny>,
    #[serde(default, rename = "hcTier")]
    _hc_tier: Option<IgnoredAny>,
    #[serde(default, rename = "inGameID")]
    _in_game_id: Option<IgnoredAny>,
    #[serde(default, rename = "kaidenAverage")]
    _kaiden_average: Option<IgnoredAny>,
    #[serde(default, rename = "ncTier")]
    _nc_tier: Option<IgnoredAny>,
    notecount: u32,
    #[serde(default, rename = "worldRecord")]
    _world_record: Option<IgnoredAny>,
}

#[derive(Deserialize)]
enum TachiDifficulty {
    #[serde(rename = "NORMAL")]
    Normal,
    #[serde(rename = "HYPER")]
    Hyper,
    #[serde(rename = "ANOTHER")]
    Another,
    #[serde(rename = "LEGGENDARIA")]
    Leggendaria,
    #[serde(rename = "All Scratch NORMAL")]
    AllScratchNormal,
    #[serde(rename = "All Scratch HYPER")]
    AllScratchHyper,
    #[serde(rename = "All Scratch ANOTHER")]
    AllScratchAnother,
    #[serde(rename = "All Scratch LEGGENDARIA")]
    AllScratchLeggendaria,
    #[serde(rename = "Kichiku NORMAL")]
    KichikuNormal,
    #[serde(rename = "Kichiku HYPER")]
    KichikuHyper,
    #[serde(rename = "Kichiku ANOTHER")]
    KichikuAnother,
    #[serde(rename = "Kichiku LEGGENDARIA")]
    KichikuLeggendaria,
    #[serde(rename = "Kiraku NORMAL")]
    KirakuNormal,
    #[serde(rename = "Kiraku HYPER")]
    KirakuHyper,
    #[serde(rename = "Kiraku ANOTHER")]
    KirakuAnother,
    #[serde(rename = "Kiraku LEGGENDARIA")]
    KirakuLeggendaria,
}

impl TachiDifficulty {
    const fn standard(&self) -> Option<Difficulty> {
        match self {
            Self::Normal => Some(Difficulty::Normal),
            Self::Hyper => Some(Difficulty::Hyper),
            Self::Another => Some(Difficulty::Another),
            Self::Leggendaria => Some(Difficulty::Leggendaria),
            Self::AllScratchNormal
            | Self::AllScratchHyper
            | Self::AllScratchAnother
            | Self::AllScratchLeggendaria
            | Self::KichikuNormal
            | Self::KichikuHyper
            | Self::KichikuAnother
            | Self::KichikuLeggendaria
            | Self::KirakuNormal
            | Self::KirakuHyper
            | Self::KirakuAnother
            | Self::KirakuLeggendaria => None,
        }
    }
}
