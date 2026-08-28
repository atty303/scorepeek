use scorepeek::recognition::{
    CatalogCandidateDomain, CatalogCandidateDomainError, MusicSelectSongResolution,
    OnnxParityError, RegisteredRecognitionResources, RegisteredResourceLoadError,
    ResultSongResolution, ScreenCatalogCandidateObservations, ScreenFieldObservationError,
    ScreenFieldObservations, ScreenSongResolution, observe_screen_fields,
    resolve_music_select_song, resolve_result_song,
};
use std::error::Error;
use std::fmt;

use super::field_observer::{FieldObserver, FieldObserverInput};

/// Production screen-field observer owning the exact resources for one immutable run.
pub struct RegisteredScreenFieldObserver {
    resources: RegisteredRecognitionResources,
    candidate_domain: CatalogCandidateDomain,
}

#[derive(Debug)]
pub enum RegisteredScreenFieldObserverLoadError {
    Resources(RegisteredResourceLoadError),
    CandidateDomain(CatalogCandidateDomainError),
}

impl fmt::Display for RegisteredScreenFieldObserverLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resources(error) => error.fmt(formatter),
            Self::CandidateDomain(error) => error.fmt(formatter),
        }
    }
}

impl Error for RegisteredScreenFieldObserverLoadError {}

impl From<RegisteredResourceLoadError> for RegisteredScreenFieldObserverLoadError {
    fn from(error: RegisteredResourceLoadError) -> Self {
        Self::Resources(error)
    }
}

impl From<CatalogCandidateDomainError> for RegisteredScreenFieldObserverLoadError {
    fn from(error: CatalogCandidateDomainError) -> Self {
        Self::CandidateDomain(error)
    }
}

impl RegisteredScreenFieldObserver {
    /// Builds the immutable full-catalog comparison domain once for this observer lifetime.
    ///
    /// # Errors
    /// Returns the exact catalog-domain error when an active song has no scoreable title.
    pub fn new(
        resources: RegisteredRecognitionResources,
    ) -> Result<Self, CatalogCandidateDomainError> {
        let candidate_domain = CatalogCandidateDomain::from_catalog(resources.catalog())?;
        Ok(Self {
            resources,
            candidate_domain,
        })
    }
}

/// Complete registered field inference and full-catalog evidence for one classified screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredScreenFieldObservation {
    fields: ScreenFieldObservations,
    candidates: ScreenCatalogCandidateObservations,
    song_resolution: ScreenSongResolution,
    clear_type: Option<&'static str>,
}

impl RegisteredScreenFieldObservation {
    pub(crate) fn from_fields(
        candidate_domain: &CatalogCandidateDomain,
        fields: ScreenFieldObservations,
    ) -> Self {
        let candidates = candidate_domain.observe(&fields);
        let song_resolution = match (&fields, &candidates) {
            (
                ScreenFieldObservations::Result(fields),
                ScreenCatalogCandidateObservations::Result { candidates, .. },
            ) => ScreenSongResolution::Result(resolve_result_song(
                &fields.title.open_text,
                &fields.artist.open_text,
                candidates,
            )),
            (
                ScreenFieldObservations::MusicSelect(fields),
                ScreenCatalogCandidateObservations::MusicSelect { candidates, .. },
            ) => ScreenSongResolution::MusicSelect(resolve_music_select_song(
                &fields.central_title.open_text,
                &fields.artist.open_text,
                &fields.active_list_title.open_text,
                candidates,
            )),
            _ => unreachable!("field observations and candidates share one screen"),
        };
        let clear_type = match &fields {
            ScreenFieldObservations::Result(fields) => {
                resolve_clear_type(&fields.clear_type.open_text)
            }
            ScreenFieldObservations::MusicSelect(_) => None,
        };
        Self {
            fields,
            candidates,
            song_resolution,
            clear_type,
        }
    }

    #[must_use]
    pub const fn fields(&self) -> &ScreenFieldObservations {
        &self.fields
    }

    #[must_use]
    pub const fn candidates(&self) -> &ScreenCatalogCandidateObservations {
        &self.candidates
    }

    #[must_use]
    pub const fn result_resolution(&self) -> Option<&ResultSongResolution> {
        match &self.song_resolution {
            ScreenSongResolution::Result(resolution) => Some(resolution),
            ScreenSongResolution::MusicSelect(_) => None,
        }
    }

    #[must_use]
    pub const fn music_select_resolution(&self) -> Option<&MusicSelectSongResolution> {
        match &self.song_resolution {
            ScreenSongResolution::Result(_) => None,
            ScreenSongResolution::MusicSelect(resolution) => Some(resolution),
        }
    }

    #[must_use]
    pub const fn song_resolution(&self) -> &ScreenSongResolution {
        &self.song_resolution
    }

    #[must_use]
    pub const fn clear_type(&self) -> Option<&'static str> {
        self.clear_type
    }
}

const CLEAR_TYPES: [&str; 7] = [
    "FAILED",
    "ASSIST CLEAR",
    "EASY CLEAR",
    "CLEAR",
    "HARD CLEAR",
    "EXH-CLEAR",
    "F-COMBO",
];

/// Resolves one OCR value through the registered fail-closed clear-type vocabulary.
#[must_use]
pub fn resolve_clear_type(observed: &str) -> Option<&'static str> {
    let mut matches = CLEAR_TYPES
        .into_iter()
        .filter(|candidate| ascii_edit_distance_at_most_one(observed, candidate));
    let selected = matches.next()?;
    matches.next().is_none().then_some(selected)
}

fn ascii_edit_distance_at_most_one(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    let (shorter, longer) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    let mut short = 0;
    let mut long = 0;
    let mut edits = 0;
    while short < shorter.len() && long < longer.len() {
        if shorter[short] == longer[long] {
            short += 1;
            long += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            if shorter.len() == longer.len() {
                short += 1;
            }
            long += 1;
        }
    }
    edits + usize::from(long < longer.len()) <= 1
}

impl FieldObserver for RegisteredScreenFieldObserver {
    type Output =
        Result<RegisteredScreenFieldObservation, ScreenFieldObservationError<OnnxParityError>>;

    fn observe(&mut self, input: &FieldObserverInput) -> Self::Output {
        let fields = observe_screen_fields(input.crops(), |crop| {
            self.resources.title_runtime().observe_open_text(crop)
        })?;
        Ok(RegisteredScreenFieldObservation::from_fields(
            &self.candidate_domain,
            fields,
        ))
    }
}

#[cfg(test)]
mod tests {
    use scorepeek::catalog::Catalog;
    use scorepeek::recognition::{
        DynamicTextObservation, FieldNotObserved, FieldNotObservedReason,
        MusicSelectScreenFieldObservations, MusicSelectSongResolution,
        MusicSelectSongUnknownReason, ResultScreenFieldObservations, ResultSongResolution,
        ResultSongUnknownReason,
    };

    use super::*;

    #[test]
    fn clear_type_resolution_accepts_only_a_unique_one_edit_registered_value() {
        assert_eq!(resolve_clear_type("EXH-CLEAR"), Some("EXH-CLEAR"));
        assert_eq!(resolve_clear_type("XH-CLEAR"), Some("EXH-CLEAR"));
        assert_eq!(resolve_clear_type("F-COMBO"), Some("F-COMBO"));
        assert_eq!(resolve_clear_type(""), None);
        assert_eq!(resolve_clear_type("UNRELATED"), None);
    }

    #[test]
    fn registered_output_keeps_fields_and_full_catalog_evidence_together() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let fields = ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "title".to_owned(),
            },
            artist: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "artist".to_owned(),
            },
            clear_type: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "FAILED".to_owned(),
            },
            difficulty: FieldNotObserved {
                reason: FieldNotObservedReason::ObserverNotImplemented,
            },
            level: FieldNotObserved {
                reason: FieldNotObservedReason::ObserverNotImplemented,
            },
            notes: FieldNotObserved {
                reason: FieldNotObservedReason::ObserverNotImplemented,
            },
            current_score: FieldNotObserved {
                reason: FieldNotObservedReason::ObserverNotImplemented,
            },
        });
        let output = RegisteredScreenFieldObservation::from_fields(&domain, fields.clone());

        assert_eq!(output.fields(), &fields);
        assert_eq!(output.candidates().candidate_count(), 0);
        assert!(matches!(
            output.result_resolution(),
            Some(ResultSongResolution::Unknown {
                reason: ResultSongUnknownReason::NoCatalogCandidates,
                ..
            })
        ));
    }

    #[test]
    fn registered_output_resolves_the_matching_music_select_screen_shape() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let text = |value: &str| DynamicTextObservation {
            input_width: 1,
            output_timesteps: 1,
            open_text: value.to_owned(),
        };
        let fields = ScreenFieldObservations::MusicSelect(MusicSelectScreenFieldObservations {
            central_title: text("texture"),
            artist: text("artist"),
            selected_chart: FieldNotObserved {
                reason: FieldNotObservedReason::ObserverNotImplemented,
            },
            active_list_title: text("TITLE"),
        });
        let output = RegisteredScreenFieldObservation::from_fields(&domain, fields.clone());

        assert_eq!(output.fields(), &fields);
        assert!(output.result_resolution().is_none());
        assert!(matches!(
            output.music_select_resolution(),
            Some(MusicSelectSongResolution::Unknown {
                reason: MusicSelectSongUnknownReason::NoCatalogCandidates,
                ..
            })
        ));
    }
}
