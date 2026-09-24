//! Shared editor and stage styles.

pub const EDITOR_CSS: &str = concat!(
    include_str!("../styles/editor.css"),
    include_str!("../styles/editor-button.css")
);
pub const HOST_CSS: &str = concat!(
    include_str!("../styles/stage.css"),
    include_str!("../styles/editor.css"),
    include_str!("../styles/editor-button.css")
);
