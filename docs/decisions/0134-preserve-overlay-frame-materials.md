# ADR 0134: Preserve overlay frame materials

- Status: Accepted
- Date: 2026-09-06
- Supersedes: ADR 0133's SVG-only frame geometry and restriction of material images to surfaces.

The original imagegen concepts in Codex task `01a06cdb-d1c0-7621-8759-0cf6b8ed9e18`
define the material quality: layered cyan rails, crystalline silver/gold/violet Aurora trim,
and textured Blackbox faceplates with engraved grooves and fasteners. The five-widget sheets
retain their information architecture and proportions, but their simplified frame rendering
must not remove those material details.

Compose the existing bundled frame PNGs from fixed-aspect corners, stretched edge middles
and a central surface in the shared DOM/CSS. Keep edge thickness independent of normal
widget resizing; scale corners together only when a widget is smaller than their combined size.
Use the same nine-region composition in native and browser, avoiding unsupported border-image.
Keep chart badges and status lamps semantic SVG fittings and all content as live DOM values.
Aurora's light ribbons belong behind the song header, with gold score typography.

This changes no saved geometry, data or event authority, canvas clipping, motion driver,
Japanese system-font policy, dependency, download or deployment boundary.
