/// One frame admitted at the fixed canonical recognition boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalFrame {
    pub(crate) pixels: Box<[u8]>,
    pub(crate) source_pts_ms: i64,
    pub(crate) decode_index: u64,
    pub(crate) capture_profile_id: String,
    pub(crate) normalizer_artifact_sha256: String,
    pub(crate) frame_extraction_sha256: String,
}

impl CanonicalFrame {
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn into_pixels(self) -> Box<[u8]> {
        self.pixels
    }

    #[must_use]
    pub fn capture_profile_id(&self) -> &str {
        &self.capture_profile_id
    }

    #[must_use]
    pub const fn source_pts_ms(&self) -> i64 {
        self.source_pts_ms
    }

    #[must_use]
    pub const fn decode_index(&self) -> u64 {
        self.decode_index
    }

    #[must_use]
    pub fn normalizer_artifact_sha256(&self) -> &str {
        &self.normalizer_artifact_sha256
    }

    #[must_use]
    pub fn frame_extraction_sha256(&self) -> &str {
        &self.frame_extraction_sha256
    }
}
