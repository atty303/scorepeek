# Overlay canvas design masters

These independently generated mockups define the worlds and information design of the three bundled overlay
skins. ADR 0133 makes these sheets the reference for faithful panel proportions, contours,
materials, typography hierarchy and information density. Runtime values remain semantic and charts retain their data meaning.

- `cyan-system.png`: technical navy, silver and cyan
- `result-aurora.png`: black-violet glass, silver, gold and aurora light
- `dj-blackbox.png`: charcoal hardware, engraved divisions, lime and amber lamps

The normal canvas is transparent outside individual widgets. The enlarged selection panel in each
sheet demonstrates edit-only grid and resize chrome; it is not a second display layout.

The shared editor sample uses NEON CIRCUIT, SP HYPER and the sheet's BEST/DETAIL values so visual
comparison does not depend on different text lengths. History dates and graph values remain synthetic.
Compare panels at the same width against these sheets; verify typography, badge/rail/lamp shape,
material edges and graph axes in addition to the five-widget arrangement.

## Material reference

The initial imagegen concepts in Codex task `01a06cdb-d1c0-7621-8759-0cf6b8ed9e18`
are the reference for material detail. The five-widget sheets above retain authority over
information structure; their flatter rendering does not replace the initial material treatment.

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
