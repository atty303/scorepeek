//! Typed score fact identities.

use serde::{Deserialize, Serialize};

use super::error::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaySide {
    OnePlayer,
    TwoPlayer,
}

impl PlaySide {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::OnePlayer => "one_player",
            Self::TwoPlayer => "two_player",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "one_player" => Ok(Self::OnePlayer),
            "two_player" => Ok(Self::TwoPlayer),
            _ => Err(Error::UnsupportedContract),
        }
    }
}

use serde_json::Value;

use super::event::{
    Chart, Event, Field, ResultChange, ResultData, SelectClear, SongPresentation, result_clear,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Origin {
    pub(super) event_id: String,
    pub(super) invocation_id: String,
    pub(super) sequence: u64,
    pub(super) emitted_unix_ms: i64,
    pub(super) received_unix_ms: u64,
    pub(super) revision: Option<u64>,
    pub(super) observation_id: Option<String>,
    pub(super) capture: Value,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Source {
    Result,
    PreviousBest,
    Select,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Fact {
    // None is explicit no_record, distinct from an absent observation.
    pub(super) value: Option<i64>,
    pub(super) source: Source,
    pub(super) origin: Origin,
}
pub(super) type Fields = [Option<Fact>; 3];
pub(super) const COLUMNS: [&str; 12] = [
    "result_score",
    "result_miss",
    "result_clear",
    "previous_score",
    "previous_miss",
    "previous_clear",
    "select_score",
    "select_miss",
    "select_clear",
    "score_origin",
    "miss_origin",
    "clear_origin",
];
pub(super) fn better(field: usize, left: i64, right: i64) -> bool {
    if field == 1 {
        left < right
    } else {
        left > right
    }
}
pub(super) fn cumulative(existing: &mut [Option<Fact>], incoming: Fields) {
    for (field, fact) in incoming.into_iter().enumerate() {
        if let Some(fact) = fact
            && let Some(value) = fact.value
            && existing[field]
                .as_ref()
                .and_then(|f| f.value)
                .is_none_or(|old| better(field, value, old))
        {
            existing[field] = Some(fact);
        }
    }
}
pub(super) fn integrate(facts: &mut [Option<Fact>; 12]) {
    for field in 0..3 {
        let mut best: Option<Fact> = None;
        for source in 0..3 {
            if let Some(candidate) = &facts[source * 3 + field]
                && let Some(value) = candidate.value
                && best
                    .as_ref()
                    .and_then(|b| b.value)
                    .is_none_or(|old| better(field, value, old))
            {
                best = Some(candidate.clone());
            }
        }
        if let Some(old) = &facts[9 + field]
            && old.value == best.as_ref().and_then(|f| f.value)
            && (0..3).any(|source| facts[source * 3 + field].as_ref() == Some(old))
        {
            continue;
        }
        facts[9 + field] = best;
    }
}
pub(super) fn known<T>(
    field: &Field<T>,
    convert: impl FnOnce(&T) -> Result<i64, Error>,
) -> Result<Option<i64>, Error> {
    match field {
        Field::Known(value) => convert(value).map(Some),
        _ => Ok(None),
    }
}
pub(super) fn fact(value: Option<i64>, source: Source, origin: &Origin) -> Option<Fact> {
    value.map(|value| Fact {
        value: Some(value),
        source,
        origin: origin.clone(),
    })
}
pub(super) fn select_fact<T>(
    field: &Field<T>,
    convert: impl FnOnce(&T) -> i64,
    origin: &Origin,
) -> Option<Fact> {
    match field {
        Field::Known(value) => fact(Some(convert(value)), Source::Select, origin),
        Field::NoRecord => Some(Fact {
            value: None,
            source: Source::Select,
            origin: origin.clone(),
        }),
        _ => None,
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResultMutation {
    Upsert(&'static str),
    Retract,
}
pub(super) type Prepared<'a> = (
    &'a Chart,
    Option<Value>,
    [Fields; 2],
    Option<ResultMutation>,
    Option<u64>,
    Option<PlaySide>,
);

pub(super) fn prepare_result<'a>(
    result: &'a ResultData,
    song: Option<&SongPresentation>,
    origin: &Origin,
    mutation: ResultMutation,
) -> Result<Prepared<'a>, Error> {
    if result.contract != "scorepeek-result-detected-v4" {
        return Err(Error::UnsupportedContract);
    }
    if let Some(song) = song
        && (song.scorepeek_song_id != result.chart.scorepeek_song_id
            || song.display_titles.is_empty()
            || song.display_titles.iter().any(String::is_empty))
    {
        return Err(Error::UnsupportedContract);
    }
    let current = [
        Some(i64::from(result.current_score)),
        known(&result.miss_count, |v| Ok(i64::from(*v)))?,
        Some(result_clear(&result.clear_type)?),
    ];
    let previous = [
        known(&result.previous_best.score, |v| Ok(i64::from(*v)))?,
        known(&result.previous_best.miss_count, |v| Ok(i64::from(*v)))?,
        known(&result.previous_best.clear_type, |v| result_clear(v))?,
    ];
    Ok((
        &result.chart,
        song.map(serde_json::to_value).transpose()?,
        [
            current.map(|v| fact(v, Source::Result, origin)),
            previous.map(|v| fact(v, Source::PreviousBest, origin)),
        ],
        Some(mutation),
        Some(result.attempt_id),
        Some(result.play_side),
    ))
}
pub(super) fn prepare<'a>(
    event: &'a Event,
    origin: &mut Origin,
) -> Result<Option<Prepared<'a>>, Error> {
    let prepared = match event {
        Event::ResultChanged {
            state: ResultChange::Provisional { result, song },
            ..
        } => prepare_result(
            result,
            Some(song),
            origin,
            ResultMutation::Upsert("provisional"),
        )?,
        Event::ResultChanged {
            state: ResultChange::Confirmed { result, song },
            ..
        } => prepare_result(
            result,
            Some(song),
            origin,
            ResultMutation::Upsert("confirmed"),
        )?,
        Event::ResultChanged {
            state:
                ResultChange::Retracted {
                    result,
                    song,
                    reason,
                },
            ..
        } => {
            let _ = reason;
            prepare_result(result, Some(song), origin, ResultMutation::Retract)?
        }
        Event::MusicSelectBestObserved {
            snapshot: Some(snapshot),
        } => {
            if snapshot.contract != "scorepeek-music-select-best-snapshot-v3" {
                return Err(Error::UnsupportedContract);
            }
            let _ = snapshot.chart.play_side;
            origin.revision = Some(snapshot.revision);
            origin.observation_id = Some(snapshot.observation_id.clone());
            let values = &snapshot.values;
            let fields = [
                select_fact(&values.score, |v| i64::from(*v), origin),
                select_fact(&values.miss_count, |v| i64::from(*v), origin),
                select_fact(&values.clear_type, SelectClear::rank, origin),
            ];
            (
                &snapshot.chart.chart,
                Some(snapshot.chart.presentation.clone()),
                [fields, [None, None, None]],
                None,
                None,
                None,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(prepared))
}
