use serde::{Deserialize, Serialize};

use super::FrameError;

/// Canonical-frame region of interest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Roi {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Roi {
    pub(crate) fn validate(self, width: u32, height: u32) -> Result<(), FrameError> {
        if self.width == 0
            || self.height == 0
            || self
                .x
                .checked_add(self.width)
                .is_none_or(|right| right > width)
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|bottom| bottom > height)
        {
            return Err(FrameError::InvalidCanonicalLayout);
        }
        Ok(())
    }

    pub(crate) fn translated_x(self, origin_x: u32) -> Result<Self, FrameError> {
        Ok(Self {
            x: self
                .x
                .checked_add(origin_x)
                .ok_or(FrameError::InvalidCanonicalLayout)?,
            ..self
        })
    }
}
