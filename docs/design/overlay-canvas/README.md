# Overlay canvas design masters

For new skins and requested redesigns, use the repository skill
[`create-overlay-skin`](../../../.agents/skills/create-overlay-skin/SKILL.md).
It owns the approved per-field design principles, sourced IIDX knowledge, concept/motion
selection gates and shared native/browser verification workflow. Existing masters below remain
the references for their respective skins; adding the skill does not redesign them retroactively.

These independently generated mockups define the worlds and information design of the three bundled overlay
skins. They are the current reference for faithful panel proportions, contours,
materials, typography hierarchy and information density. Runtime values remain semantic and charts retain their data meaning.
They are design masters, not package previews. The bundled package previews live under
`skins/<name>/preview.png` and `preview.webm`; regenerate them from the production browser renderer with
`mise run overlay:skins:preview:generate` after a skin's rendered appearance changes.

- `cyan-system.png`: technical navy, silver and cyan
- `result-aurora.png`: black-violet glass, silver, gold and aurora light
- `dj-blackbox.png`: charcoal hardware, engraved divisions, lime and amber lamps

With background disabled, the canvas is transparent outside individual widgets. The enlarged selection panel in each
sheet demonstrates edit-only grid and resize chrome; it is not a second display layout.

The shared editor sample uses NEON CIRCUIT, SP HYPER and the sheet's BEST/DETAIL values so visual
comparison does not depend on different text lengths. History dates and graph values remain synthetic.
Compare panels at the same width against these sheets; verify typography, badge/rail/lamp shape,
material edges and graph axes in addition to the five-widget arrangement.

## Material reference

The five-widget sheets above own information structure. The checked-in frame artwork owns the
material treatment:

- CYAN SYSTEM: layered luminous rails, dark geometric glass at edges, silver-white type.
- RESULT AURORA: crystalline silver bevels, purple corner joints, a fine gold inner rim,
  gold score, and purple/blue ribbons behind the song header only.
- DJ BLACKBOX: charcoal anodized metal, recessed faceplates, engraved grooves,
  corner fasteners and lime indicator bars.

The existing original PNGs under `crates/scorepeek-overlay-ui/assets/skins/` retain these
materials. `frame.rs` maps each into nine regions: fixed-aspect corners, independently
stretched edge middles and a central surface. It preserves edge thickness when widgets resize;
very small widgets scale the corners together. Raster art owns material detail, while shared
DOM/SVG retains live text, chart badges and signal semantics. Both native and browser use
this composition; neither uses CSS border-image or replaces the material with outline paths.

## Typography material

Large EX SCORE and DJ LEVEL use skin-specific `type-*.png` glyph atlases. Cyan System uses
luminous Orbitron silver, Result Aurora uses reflected Oxanium gold with an outlined bevel,
and DJ Blackbox uses matte Rajdhani silver. Cyan's rank retains the design sheet's gold accent.
Static headings, chart metadata, judgment names, clear types, history columns and graph legends/axes
use proportional whole-word `labels-*.png` atlases with semantic role colors. Small labels have a
thinner material edge to preserve readability. Actual text remains in the DOM for accessibility
and layout; decorative layers are hidden from accessibility. Unsupported strings remain ordinary text.
Mixed-language song titles and dynamic detail/history values use ordinary text with Japanese
system-font fallback. Clear-state opacity motion remains semantic; numeric values never count up.

All six atlases are generated at 3× resolution from bundled OFL fonts through the pinned native
renderer. They are checked in alongside the frame artwork; normal builds require no generation,
font download or game images. Regenerate into a new directory, inspect at actual display sizes,
then replace the six matching files under `crates/scorepeek-overlay-ui/assets/skins/`:

```text
mise run overlay:type:generate -- /tmp/scorepeek-type-atlas
```

`examples/generate_type_atlas.rs` owns font styling, face reflection, groove and bevel lighting;
`scorepeek-overlay-ui/src/typography.rs` owns semantic label roles, glyph order, geometry and shared
native/browser composition. Preserve proportional label shaping when changing the generation font.

Label boxes expose the atlas font baseline through their bottom margin, including the transparent
area below the glyphs. Detail and history rows align text baselines; history headings remain centered.
Keep the below-baseline metrics in `typography.rs` synchronized when changing the atlas fonts.

## Stream composition materials

The `*-background.png` assets fill canvas gaps without game imagery or baked-in widgets.
They are 16:9 textures with no text, logo, border, foreground object, or UI. Runtime composition
uses the checked-in PNGs in `crates/scorepeek-overlay-ui/assets/skins/`, with a separate slow
light layer for optional motion. Aperture geometry and frame width remain semantic code.
