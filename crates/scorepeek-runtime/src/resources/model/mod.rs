//! Registered model acquisition and cache.

pub mod acquire;
pub mod cache;

use std::process::ExitCode;

fn operation_main(operation: &'static str) -> ExitCode {
    crate::service::dispatch::development_operation_main(operation)
}

/// Runs the standalone registered-resource load gate.
#[must_use]
pub fn field_resource_load_gate_main() -> ExitCode {
    operation_main("field-resource-load-gate")
}

/// Runs the standalone title dictionary audit.
#[must_use]
pub fn title_dictionary_audit_main() -> ExitCode {
    operation_main("title-dictionary-audit")
}

/// Runs the standalone title model export-requirements generator.
#[must_use]
pub fn title_model_export_requirements_main() -> ExitCode {
    operation_main("title-model-export-requirements")
}

/// Runs the standalone title model contract parity gate.
#[must_use]
pub fn title_model_contract_parity_main() -> ExitCode {
    operation_main("title-model-contract-parity")
}

/// Runs the standalone registered dynamic ONNX decoder.
#[must_use]
pub fn title_official_dynamic_onnx_decode_main() -> ExitCode {
    operation_main("title-official-dynamic-onnx-decode")
}

/// Runs the standalone title ONNX parity gate.
#[must_use]
pub fn title_onnx_parity_main() -> ExitCode {
    operation_main("title-onnx-parity")
}
