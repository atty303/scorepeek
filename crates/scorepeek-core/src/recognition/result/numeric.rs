//! Result numeric layout, preprocessing, inference, and confidence contracts.

mod fixed_slot;
mod onnx;

pub use super::super::shared::confidence::*;
pub use super::panel::*;
pub use fixed_slot::*;
pub use onnx::*;
