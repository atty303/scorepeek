//! DQN response schema decoder.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer};

use crate::source::common::{
    AdapterError, SourceRevision, snapshot, validate_record_count, validate_source_size,
    validate_text,
};
use scorepeek_core::catalog::{DqnObservation, SourceObservation, SourcePolicy, SourceSnapshot};

pub struct DqnLiveAdapter;

impl DqnLiveAdapter {
    /// Parses pinned bytes from the public dqn/iidxapi INFINITAS music endpoint.
    ///
    /// This boundary is deliberately independent of HTTP clients, credentials, response headers,
    /// and caches. Acquisition code must supply the exact response body and its expected SHA-256
    /// revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the source exceeds a bound, violates the strict live schema, has a
    /// duplicate row, contains an invalid record, or does not match its pinned content revision.
    pub fn parse(bytes: &[u8], revision: SourceRevision) -> Result<SourceSnapshot, AdapterError> {
        validate_source_size(bytes)?;
        let records: Vec<DqnLiveRecord> =
            serde_json::from_slice(bytes).map_err(AdapterError::InvalidJson)?;
        validate_record_count(records.len())?;
        let mut unique_records = BTreeSet::new();
        let mut observations = Vec::with_capacity(records.len());
        for record in records {
            let title = validate_text("title", record.title)?;
            let artist = validate_text("artist", record.artist)?;
            let pack = record
                .pack_name
                .map(|pack| validate_text("packName", pack))
                .transpose()?;
            if !unique_records.insert((title.clone(), artist.clone(), pack.clone())) {
                return Err(AdapterError::DuplicateRecord(format!(
                    "({title:?}, {artist:?}, {pack:?})"
                )));
            }
            observations.push(SourceObservation::Dqn(DqnObservation {
                title,
                artist,
                pack,
            }));
        }
        snapshot(SourcePolicy::dqn(), revision, bytes, observations)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DqnLiveRecord {
    title: String,
    artist: String,
    #[serde(
        rename = "packName",
        deserialize_with = "deserialize_nullable_pack_name"
    )]
    pack_name: Option<String>,
}

fn deserialize_nullable_pack_name<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::deserialize(deserializer)
}
