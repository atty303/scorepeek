# infinitas

**Responsibility:** This specification owns the Orbital Light world's assets,
expression and recommended dimensions. Reusable information and meaning
remain in the authoring skill's basic design system; package acceptance remains
in `docs/skin-plugin-api-v2.md`.

## Identity and world

- ID: `dev.atty303.infinitas`; directory and display name: `infinitas`.
- Orbital Light uses a continuous
  silver chamfered perimeter, a cyan optical lip, a layered dark glass face,
  orbital paths and angular reflections. Those cues must survive background off.
- Ordinary lettering has angular italic contours and a bright face with a dark
  reflection band. AAA adds gold facets; FULL COMBO has a separate prismatic rim.
- Dynamic titles, artists and play options stay live. Mock headings and incidental
  words do not alter the information contract.

## Series palettes and sources

The canvas `series` property changes decorative colors only. INFINITAS is the
initial value. Frame geometry, type, spacing, motion and data geometry stay fixed.
Difficulty, result semantics, FAST/SLOW and graph colors are independent of series.
Selection never follows the currently detected game version automatically.

The research used the official [INFINITAS play guide](https://p.eagate.573.jp/game/infinitas/2/howto/play.html)
and KONAMI's arcade product pages, visually inspected on 2026-09-27. These are
references, not packaged game artwork. Hex values in `src/palette.rs` are original
interpretations of the themes, not official color specifications.

| Series | Decorative reading | Official source |
| --- | --- | --- |
| INFINITAS | Icy cyan, silver, dark blue | [Play guide](https://p.eagate.573.jp/game/infinitas/2/howto/play.html) |
| IIDX RED | Crimson, black, white | [11](https://www.konami.com/arcadegames/products/am_bmiidx11/) |
| HAPPY SKY | Summer sky, pale blue, white | [12](https://www.konami.com/arcadegames/products/am_bmiidx12/) |
| DistorteD | Black, white, silver | [13](https://www.konami.com/arcadegames/products/am_bmiidx13/) |
| GOLD | Gold, black, ivory | [14](https://www.konami.com/arcadegames/products/am_bmiidx14/) |
| DJ TROOPERS | Olive, dark green, dull silver | [15](https://www.konami.com/arcadegames/products/am_bmiidx15/) |
| EMPRESS | Pearl pink, mulberry, white | [16](https://www.konami.com/arcadegames/products/am_bmiidx16/) |
| SIRIUS | Star silver, navy, icy blue | [17](https://www.konami.com/arcadegames/products/am_bmiidx17/) |
| Resort Anthem | Sunset orange, gold, sea blue | [18](https://www.konami.com/arcadegames/products/am_bmiidx18/) |
| Lincle | Cyan, blue, luminous accents | [19](https://www.konami.com/arcadegames/products/am_bmiidx19/) |
| tricoro | White, blue, dark red | [20](https://www.konami.com/arcadegames/products/am_bmiidx20/) |
| SPADA | Crimson, black, burnt orange | [21](https://www.konami.com/arcadegames/products/am_bmiidx21/) |
| PENDUAL | Silver, magenta, deep purple | [22](https://www.konami.com/arcadegames/products/am_bmiidx22/) |
| copula | Yellow, white, teal | [23](https://www.konami.com/arcadegames/products/am_bmiidx23/) |
| SINOBUZ | Silver, dark teal, black | [24](https://www.konami.com/arcadegames/products/am_bmiidx24/) |
| CANNON BALLERS | Silver, black, racing red | [25](https://www.konami.com/arcadegames/products/am_bmiidx25/) |
| Rootage | Old gold, brown, dark burgundy | [26](https://www.konami.com/arcadegames/products/am_bmiidx26/) |
| HEROIC VERSE | Silver, purple, electric blue | [27](https://www.konami.com/arcadegames/products/am_bmiidx27/) |
| BISTROVER | Cream, orange, teal | [28](https://www.konami.com/arcadegames/products/am_bmiidx28/) |
| CastHour | Orange, navy, cyan | [29](https://www.konami.com/arcadegames/products/am_bmiidx29/) |
| RESIDENT | Black, blue, cyan | [30](https://www.konami.com/arcadegames/products/am_bmiidx30/) |
| EPOLIS | Acid yellow-green, charcoal, white | [31](https://www.konami.com/arcadegames/products/am_bmiidx31/) |
| Pinky Crush | Pink, cyan, bright purple | [32](https://www.konami.com/arcadegames/products/am_bmiidx32/) |
| Sparkle Shower | Lime, yellow, white | [33](https://www.konami.com/arcadegames/products/am_bmiidx33/) |
| ZINRAI | Purple, silver, black | [34](https://www.konami.com/arcadegames/products/am_bmiidx34/) |

PENDUAL and SINOBUZ each use one representative palette. The skin has no clock,
weekday, event or achievement detection.

## Materials and construction

The original frame is an SVG constructed from widget dimensions: fixed-width
silver shell, dark recess, illuminated glass lip, angular joints and an inset
information face. Independent corner geometry preserves thickness when resized.
The face contains a reflection sheet and an orbital curve.

## Typography and information hierarchy

Chakra Petch SemiBold Italic is obtained independently from Google Fonts at
revision `a4c8c2a0f77efa06765d596d64d077af1d7f0dae`, under the bundled
[OFL](resources/OFL-ChakraPetch.txt). It is not copied from another skin.
`mise run //skins/infinitas:assets` exports the font outlines through the existing,
locked `skrifa` 0.44.0 renderer dependency. `src/glyphs.rs` is the deterministic
output. SVG paths use gradient faces and a fine contour; no bitmap lettering is
scaled. Semantic strings remain in the DOM. Titles, artists, dates and options
use live text with the packaged font and Japanese fallback. The package includes
the font and its OFL notice; it needs no runtime download.

## Observability and scope

This is a synchronous presentation plugin. It consumes an immutable v2 snapshot
and emits a DOM tree plus a bounded schedule. It has no network, filesystem,
storage, recognition, Event API or user-input authority. The existing host
manifest/resource/Wasm validation, skin diagnostics and visual runner manifests
are the observation surfaces. Native PNGs, selector layouts and manifests, and
browser resource/DOM/media checks are required; no parallel logging system is
introduced.

## Meaning-specific presentation

- BEGINNER is green, NORMAL cyan, HYPER yellow, ANOTHER red and LEGGENDARIA
  purple. SP/DP, difficulty, level and NOTES stay separate in the selection rail.
- A uses a silver face. AA adds an icy blue face and a bright underline. AAA adds
  a faceted gold face, a gold backing and a travelling highlight. Both Score and
  History retain these cues; the rate bar is not the rank treatment.
- FULL COMBO has a separate silver/prismatic face and double light edge. Its
  highlight is white with a violet halo rather than AAA's gold.
- NO PLAY is inactive gray; FAILED is red; ASSIST purple; EASY green; CLEAR blue;
  HARD gold with an underline; EX HARD rose with a stronger edge. The expanded
  `HARD CLEAR` editor sample retains its text and uses the same HARD treatment.
- PGREAT has a gold face and a warm row reflection. GREAT/GOOD are silver.
  BAD/POOR/COMBO BREAK are red. FAST is cyan and SLOW rose at identical sizes.
- Missing or invalid SCORE/NOTES produce `?` for the distance and a striped,
  unknown rate track. Valid zero produces an empty known track and `F+0`.
  An inconsistent supplied DJ LEVEL remains visible but gets no guessed distance.
- SYSTEM/RESULT/SELECT are distinct lamps: inactive dark, active cyan, error rose.
  The transparent dark-surface logo variant comes from the project's official art.
- The graph's cyan SCORE curve rises with score ratio. Red MISS RATE rises with
  miss ratio: 100% is at the top and 0% at the bottom. Missing samples break a
  line; they never become zero.

## Motion

One 6000 ms phase drives a ±16 px horizontal drift of the canvas glass field.
AAA and FULL COMBO use a separate moving highlight along their own edge, with
maximum displacement/brightness at 3000 ms and minimum at 0/6000 ms. Digits,
labels, graph points and graph gaps stay still. The phase comes from the host's
monotonic clock; it has no achievement detector or one-shot celebration.

`background=none` removes the canvas field; `static` keeps it at its central
position; `animated` moves it. State highlights continue in all three modes.
The plugin requests 50 ms updates only while the canvas or a visible special
state needs them; otherwise it is idle. EMPTY apertures cut holes in the canvas
field, including while the field moves.

The standard PNG and WebM previews use the shared 640×640 scene. The video is
8 seconds at 25 fps, VP9, with no audio.

## Recommended dimensions and states

| Widget / condition | Width × height |
| --- | --- |
| Status | 544×52 |
| Selection, ordinary and standard long text | 544×124 |
| Score, every rank/clear and standard long options | 544×200 |
| History, 5 rows | 600×188 |
| History, 10 / 20 / 50 rows | 600×302 / 600×542 / 600×1262 |
| History Graph, 1 / 3 / 6 / 12 months | 544×208 |
| EMPTY, ordinary | 640×360 |
| EMPTY, additional wide / tall examples | 500×80 / 180×300 |

These are recommended discrete sizes; arbitrary width/height combinations may
not fit all information. Long titles use 18 px lettering and 14 px artist text;
short titles retain their 28/16 px hierarchy.

## Assets and reproduction

`resources/optical-glass.png` is an original generated achromatic optical-glass
field, made for this skin. No official game capture or artwork is packaged.
`resources/scorepeek-logo.png` is the existing project's dark-surface transparent
logo. The font is independently pinned as described above.

The same Wasm, CSS and assets serve native and browser.

- `mise run //skins/infinitas:assets` regenerates vector lettering.
- `mise run overlay:skins:build` builds the self-contained ZIP under `target/skins/`.
- `SCOREPEEK_SKIN_PREVIEW_SLUG=infinitas mise run overlay:skins:preview:generate` regenerates the standard
  catalog PNG and WebM through the production browser renderer.
- Native and browser rendering procedures are described in
  [overlay visual debugging](../../docs/overlay-visual-debugging.md). Render outputs
  belong in temporary directories; they are not skin package inputs.
