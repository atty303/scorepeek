mod config;
mod diagnostic;
mod doctor;
mod parser;
mod run;
mod skin;
mod vulkan_layer;

pub(crate) use doctor::OutputFormat;
pub use parser::{Action, generate_completion, parse, print_help};
