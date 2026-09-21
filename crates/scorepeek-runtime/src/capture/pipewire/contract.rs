//! Negotiated `PipeWire` video contract and uncalibrated frame evidence.

use std::fmt;

use serde::{Deserialize, Serialize};

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

/// Raw `BGRx` evidence retained only for calibration and receiver diagnostics.
///
/// This type is not an `ObservedFrame`: it has no capture-profile or normalizer binding and must
/// not enter recognition. Its debug representation deliberately omits pixel bytes.
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
