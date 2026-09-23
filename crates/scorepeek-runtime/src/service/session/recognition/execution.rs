use sha2::{Digest as _, Sha256};

use crate::diagnostics::contract::DiagnosticRunDescriptor;

/// Immutable resource selection and identity for one recognition session.
#[derive(Clone, Debug)]
pub(crate) struct RecognitionExecutionContext {
    pub session_id: String,
    pub identity_sha256: String,
    pub canonical_layout_sha256: String,
    pub catalog_sha256: String,
    pub model_sha256: String,
    pub runtime_sha256: String,
}

impl RecognitionExecutionContext {
    pub(crate) fn from_descriptor(descriptor: &DiagnosticRunDescriptor) -> Option<Self> {
        let binding = &descriptor.binding;
        Self::new(
            descriptor.run_id.clone(),
            binding.canonical_layout_sha256.clone(),
            binding.catalog_sha256.clone(),
            binding.model_sha256.clone(),
            binding.runtime_sha256.clone(),
        )
    }

    pub(crate) fn new(
        session_id: String,
        canonical_layout_sha256: String,
        catalog_sha256: String,
        model_sha256: String,
        runtime_sha256: String,
    ) -> Option<Self> {
        if session_id.is_empty()
            || session_id.len() > 64
            || canonical_layout_sha256 != scorepeek_core::frame::CanonicalLayout::sha256()
            || [&catalog_sha256, &model_sha256, &runtime_sha256]
                .into_iter()
                .any(|value| !valid_sha256(value))
        {
            return None;
        }
        let encoded = serde_json::to_vec(&(
            "scorepeek-recognition-execution-v1",
            &session_id,
            &canonical_layout_sha256,
            &catalog_sha256,
            &model_sha256,
            &runtime_sha256,
        ))
        .ok()?;
        let mut identity_sha256 = String::with_capacity(64);
        for byte in Sha256::digest(encoded) {
            use std::fmt::Write as _;
            write!(&mut identity_sha256, "{byte:02x}").ok()?;
        }
        Some(Self {
            session_id,
            identity_sha256,
            canonical_layout_sha256,
            catalog_sha256,
            model_sha256,
            runtime_sha256,
        })
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
