use std::error::Error;
use std::fmt;
use std::sync::Arc;

use scorepeek_core::model::session::RegisteredScreenFieldObservation;
use scorepeek_core::recognition::registered_field::{
    FieldRecognitionInput, RegisteredFieldLoadError,
    RegisteredScreenFieldObserver as CoreFieldObserver,
};
use scorepeek_core::recognition::screen::ScreenFieldObservationError;
use scorepeek_core::recognition::shared::CatalogCandidateDomainError;
use scorepeek_core::recognition::title::OnnxParityError;
use scorepeek_resources::recognition::{
    RegisteredRecognitionResources, RegisteredResourceLoadError,
};

use super::RecognitionExecutionMode;
use super::field_observer::{FieldObserver, FieldObserverAdmission, FieldObserverInput};

/// Runtime adapter for admission and whole-frame worker lifecycle.
pub struct RegisteredScreenFieldObserver {
    core: CoreFieldObserver,
}

#[derive(Debug)]
pub enum RegisteredScreenFieldObserverLoadError {
    Resources(RegisteredResourceLoadError),
    NumericModel(OnnxParityError),
    CandidateDomain(CatalogCandidateDomainError),
    TextRuntime(OnnxParityError),
}

impl fmt::Display for RegisteredScreenFieldObserverLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resources(error) => error.fmt(formatter),
            Self::NumericModel(error) | Self::TextRuntime(error) => error.fmt(formatter),
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

impl RegisteredScreenFieldObserver {
    /// Binds resolved immutable resources to core's shared recognition engine.
    pub fn new(
        resources: RegisteredRecognitionResources,
        execution_mode: RecognitionExecutionMode,
    ) -> Result<Self, RegisteredScreenFieldObserverLoadError> {
        let (catalog, text_bundle) = resources.into_catalog_and_text_bundle();
        let core =
            CoreFieldObserver::new(catalog, &text_bundle, execution_mode).map_err(|error| {
                match error {
                    RegisteredFieldLoadError::Manifest => Self::load_error(),
                    RegisteredFieldLoadError::NumericModel(error) => {
                        RegisteredScreenFieldObserverLoadError::NumericModel(error)
                    }
                    RegisteredFieldLoadError::CandidateDomain(error) => {
                        RegisteredScreenFieldObserverLoadError::CandidateDomain(error)
                    }
                    RegisteredFieldLoadError::TextRuntime(error) => {
                        RegisteredScreenFieldObserverLoadError::TextRuntime(error)
                    }
                }
            })?;
        Ok(Self { core })
    }

    fn load_error() -> RegisteredScreenFieldObserverLoadError {
        RegisteredScreenFieldObserverLoadError::TextRuntime(OnnxParityError::InvalidArtifact)
    }

    #[cfg(test)]
    pub(crate) fn prefetch_fields(
        &self,
        input: &FieldObserverInput,
    ) -> Result<(), ScreenFieldObservationError<OnnxParityError>> {
        self.core.prefetch_fields(&FieldRecognitionInput::new(
            input.sequence(),
            input.crops(),
            input.field_queue_wait_us(),
        ))
    }
}

impl FieldObserver for RegisteredScreenFieldObserver {
    type Output =
        Result<RegisteredScreenFieldObservation, ScreenFieldObservationError<OnnxParityError>>;
    const PIPELINED_PREFETCH: bool = true;

    fn outer_worker_count(&self, maximum_outstanding: usize) -> usize {
        self.core.outer_worker_count(maximum_outstanding)
    }

    fn fork_outer_worker(&self) -> Option<Self> {
        Some(Self {
            core: self.core.fork_worker(),
        })
    }

    fn admission(&self) -> Option<FieldObserverAdmission<Self::Output>> {
        let core = Arc::new(self.core.fork_worker());
        Some(Arc::new(move |input| {
            core.prefetch_fields(&FieldRecognitionInput::new(
                input.sequence(),
                input.crops(),
                input.field_queue_wait_us(),
            ))
            .err()
            .map(Err)
        }))
    }

    fn prefetch(&mut self, input: &FieldObserverInput) -> Option<Self::Output> {
        self.core
            .prefetch_fields(&FieldRecognitionInput::new(
                input.sequence(),
                input.crops(),
                input.field_queue_wait_us(),
            ))
            .err()
            .map(Err)
    }

    fn observe(&mut self, input: &FieldObserverInput) -> Self::Output {
        self.core.observe(&FieldRecognitionInput::new(
            input.sequence(),
            input.crops(),
            input.field_queue_wait_us(),
        ))
    }
}
