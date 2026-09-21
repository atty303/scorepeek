//! Canvas and widget geometry.

pub mod canvas;
pub mod widget;

pub use canvas::{CanvasPresentation, canvas_visible};
pub use widget::{AspectRatio, WidgetKind, WidgetLayout, WidgetSettings, next_widget_id};
