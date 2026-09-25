# Cyan Precision Instrument

**Responsibility:** Record this skin's authored visual decisions, inherited
starter constraints, verified dimensions, production media and reproducible
review evidence. This document does not define the common design system or
the v2 package API.

## Identity and world

- ID: `dev.atty303.scorepeek.skin.cyan-system-reconstruction`.
- Direction: a dark titanium measuring instrument with cyan traces and small fastened corners.
- Selected direction was supplied as “precision instrument”; there was no selected reference image. This skin was authored from the basic design system and the authoring skill. No earlier Cyan skin or comparison image was used.

## Materials and construction

The panel uses a deep blue titanium face, a four-pixel stepped edge, a light reflecting top edge, and four nine-pixel square fasteners. Fixed edge pieces preserve their thickness at every tested size. The content rectangle, rather than the edge, absorbs resizing. A procedural 256×256 grid PNG supplies middle-scale canvas texture. It was made with ImageMagick from solid color, 64-pixel ruled lines and a central six-pixel crosshair; it contains no game imagery or baked text. From the repository root, regenerate it with:

```sh
magick -size 256x256 xc:'#0a1720' -stroke '#173947' -strokewidth 1 -draw 'line 0,0 255,0 line 0,64 255,64 line 0,128 255,128 line 0,192 255,192 line 0,255 255,255 line 0,0 0,255 line 64,0 64,255 line 128,0 128,255 line 192,0 192,255 line 255,0 255,255' -stroke '#2b5d69' -draw 'line 125,128 131,128 line 128,125 128,131' skins/cyan-system-reconstruction/resources/metrology-grid.png
```

The approved repository scorepeek logo is the only reused artwork.

Packaged art SHA-256:

| Resource | SHA-256 | Source |
| --- | --- | --- |
| `metrology-grid.png` | `8d02d0049ef580dc948bfc7543baa365d5b62fefb968f2b54b9bdd71eb75f4da` | Procedurally generated for this skin |
| `scorepeek-logo-dark-transparent.png` | `68c7660b4152a3c788bec308b4be3d6bb66b8cbe483cf8f70964320c4f2a65cc` | Repository-approved artwork |

There is no bundled font. Live Japanese and unknown text use the host's Noto Sans JP fallback, Latin text uses DejaVu Sans when available, and prominent numeric values use DejaVu Sans Mono when available. No package distribution license is declared.

The panel content follows the baseline's two-column score, row-aligned history, and double-axis graph. The new material is rendered by this skin's CSS; the Wasm DOM, display-only score metric implementation, widget field mapping, and baseline dimensions came from the skill's generic starter. That starter already encodes layout choices, so this exercise establishes independent visual decisions but cannot isolate the basic design system as the sole cause of the resulting composition.

## Typography and information hierarchy

Titles are 26 px with an illuminated left rule. Artist text is 14 px and muted. Numeric SCORE is 32 px tabular mono, rank and miss are 23 px, labels are 10–11 px with restrained tracking. Required values remain live DOM text. Strong white values and subdued blue-gray labels distinguish data from captions. Long supplied text keeps the starter's single-line ellipsis; the source string remains in the DOM. Unknown fields use its neutral em dash.

The palette is navy `#071923`, panel `#081b26`, cyan `#59d7dc`, value `#e8f7f9`, label `#83aeb8`, warm AAA and FULL COMBO `#ffe6a5`/`#ffe7b1`. Cyan describes measurement surfaces, not a game result.

## Meaning-specific presentation

BEGINNER, NORMAL, HYPER, ANOTHER, LEGGENDARIA use green, blue, yellow, red and purple. AA is cool mint; AAA adds a warm luminous rank plate. FULL COMBO uses an independent bordered amber clear plate, including when rank is AA. The clear progression remains textual, from neutral NO PLAY to negative FAILED and increasingly bright positive clear types. PGREAT is warm; GREAT and GOOD are mint; BAD, POOR and COMBO BREAK are coral. FAST is blue and SLOW is red with equal visual weight.

SYSTEM, RESULT and SELECT each have a separate square lamp. Inactive is dark outlined, active is lit, and error is coral. SELECT active is blue and derives only from `history.recorded`. The score bar uses the supplied score ratio and fixed DJ LEVEL boundaries; the rank distance comes from the starter's display-only metric calculation and disappears for invalid input. Graph lines use cyan for supplied score ratio and orange for MISS RATE. Missing graph values remain broken intervals. Geometry and values never animate. No information or semantic exception was made.

## Motion

Only the canvas light level breathes, on a 9-second alternating cycle, between 0.90 and 1.08 brightness. Text, score bar, graph, frames and widget positions remain still. `background=none` removes the material; `static` fixes it; `animated` enables this light cycle. Reduced-motion preference disables it. The native scenario captured the same state at its initial and 2000 ms motion steps; hashes differ. The 8-second, 25 fps package WebM records the browser route's continuous background motion.

## Baseline adjustments and reasons

| Region | Baseline | Final choice and reason |
| --- | --- | --- |
| Status | 156×34 logo, 12 px lamps, 14 px signal gaps | Retained these dimensions because all three names and distinct states fit at tested 900×52 and 1840×52; square instrument lamps and thicker rail are skin choices. |
| Selection | 440×126; title 28 px, artist 16 px, 31 px four-column rail | 26/14 px title/artist make room for the illuminated title rule and 4 px frame while keeping the title dominant. Rail geometry remains 70/flexible/80/1.1 flexible. |
| Score | 440×194, 54/46 columns; rows 66/68/24 px | Retained the functional ratios; increased SCORE to 32 px and rank/miss to 23 px so values lead over the instrument material. The browser MISS COUNT label is 10 px with 0.04em tracking to remain on one line. |
| History list | 29/18/16/10/27 percent columns, 23 px rows | Retained comparison alignment; color and panel construction are skin-specific. Tested at 440×200 and 600×200. |
| History graph | 32/36 px axes, 22 px time axis | Retained axis room and supplied point geometry; tested at 440×200, 544×208 and 600×200. |
| Empty | 300×80 review opening | Retained a small unobscured aperture, with 7 px inner setback and optional title; tested at 300×80. |
| Frame | S/M/L baseline corner and edge sheets | Replaced sheet art with 4 px graduated rails and 9 px fasteners. The actual metal rail is constant in CSS rather than scaled by the widget dimensions. |

The inherited baseline dimensions and column ratios are provisional rather than independently optimized. The tested sizes below support the listed layouts, not a general minimum-size guarantee.

## Verified dimensions and states

| Widget | Both hosts: all eight cases | Additional browser cases | Additional native case 01 | Evidence and bound |
| --- | --- | --- | --- | --- |
| status | 1840×52 | 900×52 | 900×52 | Both widths retain logo and three distinct lamps; only 1840×52 has eight-case native coverage. |
| selection | 440×126 | 400×126, 544×124 preview scene | 400×126, 544×124 | At 440×126 all five difficulty colors, SP/DP, level 1/12 and three/four-digit notes were seen in both hosts. The other widths have case-01 native coverage only. |
| score | 440×194 | 544×200 preview scene | 544×200 | Eight clears, A/AA/AAA, score and judgment variation are covered at 440×194. The wide case is one state. |
| history-list | 600×200 | 440×200 | 440×200 | Five supplied rows align at both widths; the narrow native case is case 01. |
| history-graph | 600×200 | 440×200, 544×208 preview scene | 440×200, 544×208 | Axes, legends and broken MISS RATE intervals are visible; wide and narrow native sizes use case 01. |
| empty | 300×80 | 300×80 | 300×80 | Title and 20% fill leave the aperture open in both hosts. |

All review cases used frame width M and animated canvas background. S/L frame widths, background off/static, very long option strings, arbitrary editor geometry, and live OBS/Wayland composition remain unverified. This is a deliberately narrow verified size claim, not a claim that the editor rejects other sizes.

## Objective checks and visual review

- `cargo test -p scorepeek-skin-cyan-system-reconstruction`: 2 score-metric tests passed.
- `mise run overlay:skins:build`: produced `target/skins/cyan-system-reconstruction.zip`.
- `generate-skin-review-scenes.ts`, eight `overlay:visual:native` runs, `compose-skin-review.ts`, and `check-skin-review-scenes.ts`: a 1920×1440 native board with eight selection/score pairs plus status, history list, history graph, EMPTY and canvas background; all manifests and geometry checks passed.
- `verify-skin-review-browser.bash`: eight production browser cases passed at the same state and dimensions; eight smaller-size browser cases also passed. Additional native captures checked the smaller-size case and the 544 px preview widths. The browser's initially translucent background and wrapped MISS COUNT were corrected and visually reinspected.
- `SCOREPEEK_SKIN_PREVIEW_SLUG=cyan-system-reconstruction mise run overlay:skins:preview:generate`: production PNG 640×640 and WebM 640×640, VP9, 25 fps, 8 seconds, without audio; resource, Wasm, DOM, media and hash checks passed. The starter placeholder PNG was kept while review images were made and replaced only at this final preview step.

The review board, its `native-review-board.png.json` checker sidecar, cases table, all review scenes, native PNG/layout/manifest sets and all browser screenshots are retained under `/home/atty/.codex/visualizations/2026/09/25/01a0d66a-3031-7e50-8e3b-9e63e1b8aa15/cyan-system-reconstruction/`. The board is `native-review-board.png`; the scene sources are `review-scenes/`; the size variants are `size-scenes/`, `size-native/`, `size-browser/`, `wide-scene.json` and `wide-native/`. The checker passes directly against this retained directory. Development-host native and browser renderings establish these claims; target Wayland composition, OBS CEF, saved editor interactions and other sizes were not exercised.
