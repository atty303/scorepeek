use scorepeek::recognition::{
    CatalogCandidateDomain, CatalogCandidateDomainError, OnnxParityError,
    RegisteredRecognitionResources, RegisteredResourceLoadError,
    ScreenCatalogCandidateObservations, ScreenFieldObservationError, ScreenFieldObservations,
    observe_screen_fields,
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
}

impl RegisteredScreenFieldObservation {
    fn from_fields(
        candidate_domain: &CatalogCandidateDomain,
        fields: ScreenFieldObservations,
    ) -> Self {
        let candidates = candidate_domain.observe(&fields);
        Self { fields, candidates }
    }

    #[must_use]
    pub const fn fields(&self) -> &ScreenFieldObservations {
        &self.fields
    }

    #[must_use]
    pub const fn candidates(&self) -> &ScreenCatalogCandidateObservations {
        &self.candidates
    }
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
        ResultScreenFieldObservations,
    };

    use super::*;

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
    }
}
