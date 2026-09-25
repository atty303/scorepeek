//! Language-neutral scorepeek skin ABI v2 types and Rust guest helpers.

mod contract;
mod guest;
mod metrics;
mod tree;

pub use contract::{Canvas, Input, Widget};
pub use guest::{allocate, deallocate, decode, encode};
pub use metrics::{
    BoundaryDirection, RankDistance, ScoreProgress, rank_thresholds, score_progress,
};
pub use tree::{Node, Output, Schedule};

pub const MAX_AFTER_MS: u64 = 2_147_483_647;
