# ADR 0133: Restore overlay design fidelity

- Status: Accepted
- Date: 2026-09-06
- Supersedes: ADR 0131's permission to depart from the design sheets and ADR 0132's frame treatment.

Use the original three sheets under `docs/design/overlay-canvas/` as the visual reference for
panel proportions, chamfered contours, dark surfaces, typography hierarchy and information density.
Keep the five-widget information architecture. Restore cyan fine rules, Aurora silver/violet trim,
and Blackbox charcoal hardware with lime accents. The enlarged selection panel on each sheet is an
editor illustration, not a second runtime layout.

Share explicit SVG frame geometry between native and browser so resizing preserves the corner and
border scale. Keep semantic content in DOM/CSS and existing material images as surface treatments.
Remove ambient particles and broad frame effects; retain subtle moving highlights and semantic
clear-type/lamp motion. Canvas clipping, actual-value updates and the separate Rust/JS motion drivers
remain unchanged. Japanese continues to use system fonts without an embedded Noto bundle.

New widgets and newly created configurations use the compact reference proportions. Existing saved
canvas and widget geometry remains operator-controlled and is not automatically rearranged.
