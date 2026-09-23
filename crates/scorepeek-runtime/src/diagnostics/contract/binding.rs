//! Immutable resource identity for one diagnostic run.

use serde::Serialize;
use sha2::{Digest as _, Sha256};

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticResource {
    pub program: &'static str,
    pub version: &'static str,
    pub build_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticBinding {
    pub canonical_layout_sha256: String,
    pub catalog_sha256: String,
    pub model_sha256: String,
    pub runtime_sha256: String,
    pub replay: Option<DiagnosticReplayBinding>,
}

impl DiagnosticBinding {
    /// Returns the stable identity of the immutable inputs owned by one diagnostic run.
    #[must_use]
    pub fn identity_sha256(&self) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        let mut bytes = serde_json::to_vec(&DiagnosticBindingIdentity {
            schema: super::BINDING_IDENTITY_SCHEMA,
            binding: self,
        })
        .ok()?;
        bytes.push(b'\n');
        Some(encode_sha256(&bytes))
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        [
            &self.canonical_layout_sha256,
            &self.catalog_sha256,
            &self.model_sha256,
            &self.runtime_sha256,
        ]
        .into_iter()
        .all(|value| valid_sha256(value))
            && self.replay.as_ref().is_none_or(|replay| {
                valid_sha256(&replay.request_sha256) && valid_sha256(&replay.extraction_sha256)
            })
    }
}

#[derive(Serialize)]
struct DiagnosticBindingIdentity<'a> {
    schema: &'static str,
    binding: &'a DiagnosticBinding,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticReplayBinding {
    pub request_sha256: String,
    pub extraction_sha256: String,
}

#[derive(Clone, Debug)]
pub struct DiagnosticRunDescriptor {
    pub run_id: String,
    pub monotonic_start_ms: u64,
    pub resource: DiagnosticResource,
    pub binding: DiagnosticBinding,
}

impl DiagnosticRunDescriptor {
    #[must_use]
    pub fn is_valid_for_version(&self, product_version: &str) -> bool {
        !self.run_id.is_empty()
            && self.run_id.len() <= 64
            && self
                .run_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && self.resource.program == "scorepeek"
            && self.resource.version == product_version
            && valid_sha256(&self.resource.build_sha256)
            && self.binding.is_valid()
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn encode_sha256(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> DiagnosticBinding {
        DiagnosticBinding {
            canonical_layout_sha256: "3".repeat(64),
            catalog_sha256: "4".repeat(64),
            model_sha256: "5".repeat(64),
            runtime_sha256: "6".repeat(64),
            replay: None,
        }
    }

    #[test]
    fn binding_identity_changes_with_resource_revision() {
        let first = binding().identity_sha256().unwrap();
        let mut changed = binding();
        changed.catalog_sha256 = "7".repeat(64);
        assert_eq!(first.len(), 64);
        assert_ne!(first, changed.identity_sha256().unwrap());
    }

    #[test]
    fn descriptor_rejects_invalid_resource_or_binding_identity() {
        let mut descriptor = DiagnosticRunDescriptor {
            run_id: "run-1".to_owned(),
            monotonic_start_ms: 0,
            resource: DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "7".repeat(64),
            },
            binding: binding(),
        };
        assert!(descriptor.is_valid_for_version(env!("CARGO_PKG_VERSION")));
        assert!(!descriptor.is_valid_for_version("different-product-version"));
        descriptor.binding.catalog_sha256.clear();
        assert!(!descriptor.is_valid_for_version(env!("CARGO_PKG_VERSION")));
        assert!(descriptor.binding.identity_sha256().is_none());
    }
}
