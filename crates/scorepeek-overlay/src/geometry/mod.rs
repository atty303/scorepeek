//! Canvas and widget geometry.

pub mod canvas;
pub mod widget;

/// Upper bound for one logical canvas or widget dimension. Matches the default
/// two-dimensional texture limit used by the native Vello/WGPU renderer.
pub const MAX_DIMENSION: u32 = 8192;

pub use canvas::{CanvasPresentation, canvas_visible};
pub use widget::{AspectRatio, WidgetKind, WidgetLayout, WidgetSettings, next_widget_id};
