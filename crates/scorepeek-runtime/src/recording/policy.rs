//! Canonical recording admission policy.

const MIB: usize = 1024 * 1024;

pub const DEFAULT_RECORDING_MEMORY_MIB: usize = 1024;
pub const MIN_RECORDING_MEMORY_MIB: usize = 128;
pub const MAX_RECORDING_MEMORY_MIB: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingMemoryLimit {
    bytes: u64,
}

impl RecordingMemoryLimit {
    /// Converts the configured MiB value into the bounded byte policy.
    ///
    /// # Errors
    /// Returns an error when the value is outside the registered range or overflows bytes.
    pub fn from_mib(mib: usize) -> Result<Self, String> {
        if !(MIN_RECORDING_MEMORY_MIB..=MAX_RECORDING_MEMORY_MIB).contains(&mib) {
            return Err(format!(
                "recording memory must be between {MIN_RECORDING_MEMORY_MIB} and {MAX_RECORDING_MEMORY_MIB} MiB"
            ));
        }
        let bytes = u64::try_from(mib)
            .ok()
            .and_then(|value| value.checked_mul(MIB as u64))
            .ok_or_else(|| "recording memory byte count overflows".to_owned())?;
        Ok(Self { bytes })
    }

    #[cfg(test)]
    #[must_use]
    pub fn default_limit() -> Self {
        Self::from_mib(DEFAULT_RECORDING_MEMORY_MIB)
            .expect("the registered recording memory default is valid")
    }

    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }

    #[cfg(test)]
    pub(super) const fn from_bytes(bytes: u64) -> Self {
        Self { bytes }
    }
}
