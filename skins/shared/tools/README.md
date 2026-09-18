# Shared skin authoring tools

`compose-css.bash` assembles a skin's package CSS from its own `theme.css` and its
`skin.build.toml`. Setting `shared = true` opts that skin into the reusable styles and
fonts in this directory. The tool receives the skin directory and does not contain a
skin registry, skin identifiers, or theme-specific values.
