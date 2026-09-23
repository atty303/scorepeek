//! Source evidence and the common `BGRx` normalization input.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::normalization::{
    FractionalLinearGeometry, FractionalRectangle, RationalCoordinate, UnboundNormalizationError,
};
use scorepeek_core::frame::CANONICAL_FRAME_CONTRACT_ID;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EdgeCrop {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeCaptureEvidence {
    pub backend: &'static str,
    pub source_contract: Value,
    pub normalization_input_format: &'static str,
    pub memory_type: UncalibratedMemoryType,
    pub stride: u32,
    pub crop: EdgeCrop,
    pub normalization: &'static str,
    pub canonical_output: &'static str,
}

impl RuntimeCaptureEvidence {
    /// # Errors
    /// Returns an invalid-geometry error for unsupported dimensions, stride, or crop.
    pub fn new(
        backend: &'static str,
        width: u32,
        height: u32,
        source_contract: Value,
        memory_type: UncalibratedMemoryType,
        stride: u32,
        crop: EdgeCrop,
    ) -> Result<(Self, FractionalLinearGeometry), UnboundNormalizationError> {
        let minimum_stride = width
            .checked_mul(4)
            .ok_or(UnboundNormalizationError::InvalidGeometry)?;
        if width == 0 || height == 0 || stride < minimum_stride {
            return Err(UnboundNormalizationError::InvalidGeometry);
        }
        let retained_width = width
            .checked_sub(crop.left)
            .and_then(|value| value.checked_sub(crop.right))
            .filter(|value| *value > 0)
            .ok_or(UnboundNormalizationError::InvalidGeometry)?;
        let retained_height = height
            .checked_sub(crop.top)
            .and_then(|value| value.checked_sub(crop.bottom))
            .filter(|value| *value > 0)
            .ok_or(UnboundNormalizationError::InvalidGeometry)?;
        let coordinate = |value| RationalCoordinate::new(i64::from(value), 1);
        let rectangle = FractionalRectangle::new(
            coordinate(crop.left)?,
            coordinate(crop.top)?,
            coordinate(retained_width)?,
            coordinate(retained_height)?,
        );
        let geometry = FractionalLinearGeometry::new_edge_crop(width, height, rectangle)?;
        Ok((
            Self {
                backend,
                source_contract,
                normalization_input_format: "BGRx",
                memory_type,
                stride,
                crop,
                normalization: "edge_crop_linear_bgrx_to_rgb8_v1",
                canonical_output: CANONICAL_FRAME_CONTRACT_ID,
            },
            geometry,
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncalibratedVideoContract {
    pub width: u32,
    pub height: u32,
    pub framerate_num: u32,
    pub framerate_denom: u32,
    pub maximum_framerate_num: u32,
    pub maximum_framerate_denom: u32,
    pub pixel_aspect_num: u32,
    pub pixel_aspect_denom: u32,
    pub chroma_site: u32,
    pub color_range: u32,
    pub color_matrix: u32,
    pub transfer_function: u32,
    pub color_primaries: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UncalibratedMemoryType {
    MemoryPointer,
    MemoryFileDescriptor,
    DmaBuf,
}

impl UncalibratedMemoryType {
    pub(super) const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::MemoryPointer => "memory_pointer",
            Self::MemoryFileDescriptor => "memory_file_descriptor",
            Self::DmaBuf => "dma_buf",
        }
    }
}

/// Raw `BGRx` frame owned by a backend receiver.
///
/// Only its `BgrxFrame` view enters the shared normalizer. Source contract and memory
/// type stay available to backend diagnostics. Debug output omits pixel bytes.
pub struct UncalibratedFrame {
    pub(super) contract: UncalibratedVideoContract,
    pub(super) memory_type: UncalibratedMemoryType,
    pub(super) stride: u32,
    pub(super) sequence: u64,
    pub(super) received_monotonic_ns: u64,
    pub(super) bytes: Vec<u8>,
}

impl fmt::Debug for UncalibratedFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UncalibratedFrame")
            .field("contract", &self.contract)
            .field("memory_type", &self.memory_type)
            .field("stride", &self.stride)
            .field("sequence", &self.sequence)
            .field("received_monotonic_ns", &self.received_monotonic_ns)
            .field("byte_count", &self.bytes.len())
            .finish()
    }
}

impl UncalibratedFrame {
    #[must_use]
    pub fn bgrx(&self) -> BgrxFrame<'_> {
        BgrxFrame {
            width: self.contract.width,
            height: self.contract.height,
            stride: self.stride,
            pixels: &self.bytes,
            source_sequence: self.sequence,
            received_monotonic_ns: self.received_monotonic_ns,
        }
    }

    #[must_use]
    pub const fn contract(&self) -> UncalibratedVideoContract {
        self.contract
    }

    #[must_use]
    pub const fn memory_type(&self) -> UncalibratedMemoryType {
        self.memory_type
    }

    #[must_use]
    pub const fn stride(&self) -> u32 {
        self.stride
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn received_monotonic_ns(&self) -> u64 {
        self.received_monotonic_ns
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[cfg(test)]
    pub(crate) fn for_normalizer_test(
        contract: UncalibratedVideoContract,
        stride: u32,
        sequence: u64,
        received_monotonic_ns: u64,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            contract,
            memory_type: UncalibratedMemoryType::MemoryFileDescriptor,
            stride,
            sequence,
            received_monotonic_ns,
            bytes,
        }
    }
}

/// The source-independent data required for canonical normalization.
#[derive(Clone, Copy, Debug)]
pub struct BgrxFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: &'a [u8],
    pub source_sequence: u64,
    pub received_monotonic_ns: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(width: u32, height: u32) -> UncalibratedVideoContract {
        UncalibratedVideoContract {
            width,
            height,
            framerate_num: 60,
            framerate_denom: 1,
            maximum_framerate_num: 60,
            maximum_framerate_denom: 1,
            pixel_aspect_num: 1,
            pixel_aspect_denom: 1,
            chroma_site: 0,
            color_range: 0,
            color_matrix: 0,
            transfer_function: 0,
            color_primaries: 0,
        }
    }

    #[test]
    fn runtime_evidence_rejects_invalid_stride_and_crop_before_admission() {
        let create = |width, height, stride, crop| {
            RuntimeCaptureEvidence::new(
                "pipewire",
                width,
                height,
                serde_json::to_value(video(width, height)).unwrap(),
                UncalibratedMemoryType::MemoryFileDescriptor,
                stride,
                crop,
            )
        };
        assert!(create(1920, 1080, 7679, EdgeCrop::default()).is_err());
        assert!(create(u32::MAX, 1080, u32::MAX, EdgeCrop::default()).is_err());
        assert!(
            create(
                1920,
                1080,
                7680,
                EdgeCrop {
                    left: 1920,
                    ..EdgeCrop::default()
                }
            )
            .is_err()
        );
        let (evidence, _) = create(1920, 1080, 7680, EdgeCrop::default()).unwrap();
        assert_eq!(evidence.normalization_input_format, "BGRx");
        assert_eq!(evidence.canonical_output, CANONICAL_FRAME_CONTRACT_ID);
    }
}
