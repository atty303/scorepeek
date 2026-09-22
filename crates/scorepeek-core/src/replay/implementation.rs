//! Stable identity for the portable production semantics exercised by replay.

use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

const PRODUCTION_SEMANTIC_SOURCES: &[&[u8]] = &[
    include_bytes!("../event/domain.rs"),
    include_bytes!("../event/projection.rs"),
    include_bytes!("../event/reducer.rs"),
    include_bytes!("../event/reducer/hypothesis.rs"),
    include_bytes!("../event/reducer/lifecycle.rs"),
    include_bytes!("../event/reducer/music_select_flow.rs"),
    include_bytes!("../event/reducer/result.rs"),
    include_bytes!("../event/reducer/result_flow.rs"),
    include_bytes!("../event/reducer/selection.rs"),
    include_bytes!("../event/run.rs"),
    include_bytes!("../event/schema.rs"),
    include_bytes!("../model/session/mod.rs"),
    include_bytes!("../model/session/screen_observation.rs"),
    include_bytes!("../model/session/text_observer_pool.rs"),
    include_bytes!("../recognition/screen.rs"),
    include_bytes!("../recognition/screen/export.rs"),
    include_bytes!("../recognition/screen/predicate.rs"),
    include_bytes!("../recognition/screen_reference.rs"),
    include_bytes!("../recognition/title/observe.rs"),
    include_bytes!("../recognition/title/preprocess.rs"),
    include_bytes!("../recognition/title/decode.rs"),
    include_bytes!("../recognition/title/resolve.rs"),
    include_bytes!("../recognition/music_select/observe.rs"),
    include_bytes!("../recognition/music_select/play_type.rs"),
    include_bytes!("../recognition/music_select/best.rs"),
    include_bytes!("../recognition/music_select/resolve.rs"),
    include_bytes!("../recognition/result/observe.rs"),
    include_bytes!("../recognition/result/panel.rs"),
    include_bytes!("../recognition/result/numeric.rs"),
    include_bytes!("../recognition/result/numeric/fixed_slot.rs"),
    include_bytes!("../recognition/result/numeric/onnx.rs"),
    include_bytes!("../recognition/result/play_options.rs"),
    include_bytes!("../recognition/result/resolve.rs"),
    include_bytes!("../recognition/shared/ctc.rs"),
    include_bytes!("../recognition/shared/candidates.rs"),
    include_bytes!("../recognition/shared/confidence.rs"),
    include_bytes!("../session/attempt.rs"),
    include_bytes!("../session/episode.rs"),
    include_bytes!("../session/reducer.rs"),
    include_bytes!("../session/result.rs"),
    include_bytes!("../session/selection.rs"),
    include_bytes!("../session/timeline.rs"),
];

/// Returns a length-delimited digest of the portable production semantics used by replay.
#[must_use]
pub fn production_semantics_sha256() -> String {
    let mut digest = Sha256::new();
    for source in PRODUCTION_SEMANTIC_SOURCES {
        digest.update(
            u64::try_from(source.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(source);
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_semantics_digest_is_sha256() {
        let digest = production_semantics_sha256();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
