use scorepeek::recognition::{
    OnnxParityError, RegisteredRecognitionResources, ScreenFieldObservationError,
    ScreenFieldObservations, observe_screen_fields,
};

use super::field_observer::{FieldObserver, FieldObserverInput};

/// Production screen-field observer owning the exact resources for one immutable run.
pub struct RegisteredScreenFieldObserver {
    resources: RegisteredRecognitionResources,
}

impl RegisteredScreenFieldObserver {
    #[must_use]
    pub const fn new(resources: RegisteredRecognitionResources) -> Self {
        Self { resources }
    }
}

impl FieldObserver for RegisteredScreenFieldObserver {
    type Output = Result<ScreenFieldObservations, ScreenFieldObservationError<OnnxParityError>>;

    fn observe(&mut self, input: &FieldObserverInput) -> Self::Output {
        observe_screen_fields(input.crops(), |crop| {
            self.resources.title_runtime().observe_open_text(crop)
        })
    }
}
