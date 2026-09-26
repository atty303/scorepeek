# Basic skin design system

**Responsibility:** Own the reusable information, meaning, priority and
baseline layout contract for all authoring-skill skins. It includes observed
starting measurements, their testable reasons and quality criteria. It does
not own a skin's palette, material, final dimensions or a series identity.

This is the authoring baseline for every skin made with this skill, whether
bundled or external. The versioned `docs/skin-plugin-api-v2.md`
alone defines package acceptance. A skin records its final choices under the
skill's specification template. An optional series
specification may add shared expression for several skins; the skin's own
specification records the final result.

## Shared information contract

For the same widget kind, every authored skin presents the same information,
meaning and priority. A skin may change placement, size ratios, material,
typographic expression and continuous motion within its verified sizes. The
skin does not calculate a new game result, infer a missing value, change
supplied text, or treat a visual accent as a substitute for a label. It may
derive the specified display-only score rate and threshold distance from the
supplied best score and chart note count.

| Widget | Required information | Priority within the widget |
| --- | --- | --- |
| `status` | scorepeek identity; SYSTEM and RESULT signals with their supplied states; SELECT lamp reflecting `history.recorded` | All three signals remain distinct; SELECT means a record exists for the selected chart, not a RESULT confirmation; the wordmark is secondary |
| `selection` | Song title, artist, SP/DP, difficulty kind, level number, notes | Title first; chart identity next; artist remains readable; no lamp occupies the title's leading edge |
| `score` | SCORE (the supplied EX score), DJ LEVEL, nearest threshold distance, MISS COUNT, CLEAR, score rate bar with DJ LEVEL ticks; PGREAT, GREAT, GOOD, BAD, POOR, FAST, SLOW, COMBO BREAK, PLAY OPTIONS | SCORE and DJ LEVEL lead, CLEAR is independently prominent; rate and threshold distance explain progress; every detail row remains readable and comparable. Do not add BEST or RESULT DETAIL group headings or a score-rate label/value |
| `history-list` | HISTORY; DATE, SCORE, DJ LEVEL, MISS, CLEAR headers and the number of supplied rows requested by `history_count`, in supplied order | Comparison across columns; no newest-row celebration |
| `history-graph` | HISTORY GRAPH; DJ LEVEL and MISS RATE legends, F/E/D/C/B/A/AA/AAA and 0/25/50/75/100% axis stages, month/time ticks, supplied points and missing intervals within the requested `graph_months` range | Data lines and legend lead; all stages and units remain legible; surrounding material is subordinate |
| `empty` | Optional user title, requested interior opacity and open aperture | The user's content in the aperture leads; title and frame do not obscure it |
| Canvas | Selectable background off/static, animated when the adopted world calls for motion, and EMPTY apertures | Widgets and user content lead over background material |

An absent field remains visibly unknown or neutral where the widget has a
field. Zero, NO PLAY and FAILED are different values. BEST and RESULT DETAIL
can describe different plays; the skin does not collapse them. EX SCORE and DJ
LEVEL are separate from CLEAR, and MISS COUNT is separate from COMBO BREAK.
FAST and SLOW are equal peers. Unknown options remain visible as supplied.
The SELECT lamp is on only when `history.recorded` is true. It does not infer
the selected chart from song text or interpret the RESULT signal.
History row count and graph time range come from each widget's standard host
settings. A small five-row baseline does not authorize dropping rows when the
user selects a larger count; a graph does not use all supplied months when a
shorter range is selected.
SCORE remains an EX score; renaming its display label does not change the
numeric meaning. The rate is SCORE divided by twice the note count, bounded
to 0–100% for the bar. DJ LEVEL ticks use the same rank thresholds as the
runtime. With a missing or invalid score or note count, derived values remain
unknown; a skin never guesses them from the shown DJ LEVEL.
This applies to the painted score-rate bar or dial as well as numeric text:
an unknown input must not paint the same zero-progress state as a valid SCORE 0.

For the display-only threshold distance, let `N` be the supplied note count,
`M = 2N`, and `S` the supplied SCORE. The lower score boundaries for
E/D/C/B/A/AA/AAA are `ceil(M × k / 9)` for `k = 2/3/4/5/6/7/8`; F begins at
zero and the upper boundary after AAA is `MAX = M`. The bar's tick positions
follow these same boundaries, including F's two-ninths span. Within the
supplied rank, show the distance from its lower boundary as `RANK+n` if that
boundary is nearer (including a tie), or the distance to the next boundary as
`NEXT-n` if it is nearer. At `S = M`, show `MAX`. A skin may choose the digit
padding and typography. If `N` or `S` is invalid, `S > M`, or the supplied
rank disagrees with the score-derived rank, do not show a fabricated distance.
The supplied SCORE and rank remain separate visible values.

## Meaning-specific presentation

These semantic palette and motion rules are authoritative. In particular:

- BEGINNER, NORMAL, HYPER, ANOTHER and LEGGENDARIA retain green, blue, yellow,
  red and purple identity, respectively. SP/DP and level remain separate.
- AAA has a material treatment beyond simply increasing the intensity of AA.
  AA is positive and AAA is very positive. The same meaning treatment appears
  in score and history. A smaller history version may simplify geometry, but
  must retain a recognizable shape or material cue from the score treatment;
  hue alone is insufficient. Compare A, AA and AAA with other state conditions
  held constant: AA needs a visible positive cue beyond the letters that A
  lacks, and AAA needs a further material cue beyond AA. FULL COMBO has an independent treatment with the
  same cross-widget relation; either can appear without the other.
- A clear state is written as text. NO PLAY is inactive, FAILED is negative,
  ASSIST and EASY are weak positive, CLEAR is positive, HARD is somewhat
  stronger, EX HARD is strong positive, and FULL COMBO is the most positive.
  Color, light and motion reinforce the text consistently in score and history.
- PGREAT is very positive; GREAT and GOOD are positive; BAD, POOR and COMBO
  BREAK are negative. PGREAT has a visible stronger cue than GREAT and GOOD
  beyond its different letters; assigning all three the same label class,
  color and surface treatment does not express the required tier. These
  meaning groups remain distinguishable without changing the supplied counts.
- FAST uses a blue hue and SLOW a red hue as timing directions of equal
  weight. A red SLOW count is not a negative judgment. Missing values are not
  invented from other fields.
- Graph data geometry, values, axes and missing intervals never animate. Only
  frame or surrounding material may move. Continuous motion describes the
  current state and never claims a newly achieved result.
- On the graph, a larger MISS RATE is plotted higher on its own right-hand
  axis. Its 0–100% labels and red data line must use the same direction;
  missing samples do not become a zero-rate line.
- Mixed Japanese/Latin song titles and dynamic values stay live, semantic text.
  Decoration does not replace accessible text or intercept input.
- Labels and their values occupy visibly different color and luminance roles.
  Values retain the stronger contrast; label color must not impersonate a
  semantic state color. Each skin chooses the actual palette and material.
- SYSTEM and RESULT receive only `inactive`, `active` and `error` lamp states.
  The runtime maps provisional and confirmed results to `active`, and a
  retracted result to `error` before the skin renders. A skin must not claim to
  distinguish the two active source events from this lamp input. Its
  specification says how the three supplied lamp states appear.

## Material and typography freedom

A skin owns its contours, surfaces, palette beyond semantic colors, type
personality, spacing and motion rhythm. Where the adopted concept calls for
physical material, its thickness, edges, joints, reflection or engraving must
remain legible at actual widget size. A flat border that merely hints at that
material is incomplete. A deliberately flat concept instead needs equally
deliberate contour, type and spacing at actual size. Background art gives
canvas gaps texture without baked-in text, widgets or game imagery.
Background off is a complete visual state.
Every authored skin exposes no-background and static canvas modes. The
no-background value may be named `none` or `off` in its manifest. When the
selected concept includes canvas motion, it also exposes animated mode with a
visible continuous material change while leaving widget data still. A
concept with a deliberately still canvas omits the ineffective animated control
and records that decision in its skin specification. Each declared mode must be rendered
in native and browser; a manifest value alone does not establish support.
The status identity uses the repository's approved light/dark scorepeek logo
artwork, chosen for its surface. The logo may be framed or cropped but its
artwork is not redrawn as a skin-specific wordmark.
Canvas background art needs visible middle-scale material in gaps, without
baked-in text, widgets or game images. Decoration may extend beyond a widget
but is cropped by the canvas. It does not rewrite saved placement or add
automatic padding. An EMPTY aperture is reserved for the user's camera,
game or other content; optional title and fill must not obscure the opening.
Fixed frame corners and edges retain their thickness as the widget resizes;
the interior, rather than the material edge, absorbs the change in size.
Decoration remains outside accessibility and hit testing while each real
label and value stays live DOM text. Latin font atlases may carry material,
including label art, if text remains present; unknown text and Japanese use
normal text with platform fallback. Verify glyph baselines and transparent
atlas padding at actual size.

Each skin records any exception to an expressive rule and its reason in its
specification. Information items, meanings and priority, and legibility within
the claimed size range have no per-skin exceptions.

## Layout and dimensional baseline

Implement this structure first in a working skin. Then change dimensions to
fit the chosen type, material and verified size range. The numbers are
**starting values**, observed in the accepted Cyan composition at its review
size, not guaranteed minimum sizes. Record every final value and adjustment
in the skin specification. Preserve the reason in the last column when
adjusting; prove readability at the new dimensions by rendering.

| Region | Starting structure and value | Reason to preserve when adjusting |
| --- | --- | --- |
| Status | identity at left, SYSTEM/RESULT/SELECT at right; 156×34 identity, 12 px lamps, 14 px signal gaps at a 1840×52 review size | Three signals need separate readable names and states; identity cannot compete with them. The 156, 12 and 14 px values are observed, while their exact historical choice has no independent evidence. |
| Selection | title over artist, then a 31 px rail for SP/DP, difficulty, LV and NOTES; rail columns 70 px / flexible / 80 px / 1.1 flexible; title 28 px and artist 16 px at 440×126 | Title leads, artist has visible separation, and mode/level have predictable width while difficulty and notes may need more room. Widths are observed; validate longest representative strings before changing them. |
| Score overall | left/right 54%/46% at 440×194; right judgment rows 16 px and final two-line options 30 px | The left contains the large score and state, while the right retains a compact but readable full detail set. Match the two columns' lower edges without adding empty space below detail. |
| Score left | three rows of 66, 68 and 24 px: SCORE/DJ LEVEL at 54%/46%, MISS COUNT/CLEAR TYPE at 40%/60%, then a rank-ticked bar | Four-digit MISS COUNT needs less horizontal room than a long CLEAR TYPE; SCORE stays dominant; the bar occupies its own row without extra rate text or divider. These row heights sum to 158 px, matching the observed right detail density at the reviewed size. |
| History list | DATE/SCORE/DJ LEVEL/MISS/CLEAR at 29/18/16/10/27%; header and rows 23 px | Date and clear text are the longest comparison fields; MISS can be narrow; fixed row height keeps columns aligned. The exact percentages remain provisional until boundary-size verification. |
| History graph | title and two legends above plot; 32 px rank axis and 36 px MISS RATE axis, with a 22 px time axis | Distinct left/right scales and time labels need room without shrinking the central plot excessively. F through AAA and 0% through 100% in 25% steps make the levels explicit. Both axes and plot must remain readable at the claimed minimum size. |
| Empty | optional title over an open aperture; 300×80 in the review composition | A small empty widget proves aperture and title behavior without consuming the full review board. This review size is not a universal default or verified range. |
| Frame | S/M/L corner sizes 21/27/34 px, edge sheets 88/112/142 px; 8 px vertical and 12 px horizontal content inset at M | A fixed-thickness frame lets the content area grow independently of material edges; every chosen frame must retain a usable content rectangle. These values describe the source composition's construction, not a mandatory frame shape. |

Do not copy the Cyan frame art, font, color values or CSS when realizing this
baseline. A different frame can satisfy the same content and spacing reasons.
If an observed number has no demonstrated causal reason, treat it as a
provisional starting point and decide its replacement through actual renders.

## Verified size contract

A specification lists a tested width and height range for **each** widget,
including frame width, title/opacity and aspect settings where applicable.
The lower bound is established by the smallest tested size at which all
required information is present and legible. It is not inferred from the
editor's 16-pixel minimum. Outside a listed range, the editor and saved layout
continue to work under the existing API, but this design system makes no
readability claim. Document any aspect-ratio limitation as a pair of width
and height bounds or an explicit tested matrix; a single area number is not
enough.
For the stressed title, artist and PLAY OPTIONS values used to certify a size,
the full supplied string must be visibly available at that size. Keeping it
in DOM text while the paint clips, overlays an ellipsis or otherwise hides the
tail does not establish full-information display. Reflow, wrap, change the
layout or narrow the claimed content/size condition and test again. State an
explicit content limit if the skin cannot fit arbitrary-length text.
For an interval claim, inspect its minimum, maximum and interior width/height
combinations with stressed text and states, plus both sides of every responsive
breakpoint inside the interval. A pair of successful endpoints alone does not
prove the interval between them. If only isolated sizes were checked, report
those sizes as discrete evidence and leave the range unverified.

## Quality criteria

Mechanical checks establish that a package has a unique ID, complete
manifest and resources, required fields and states, correct supplied values,
valid preview media and a buildable ZIP. They check each claimed size and
state case for missing content. They do not certify readability or craft.

Visual review establishes that all required information is legible and
correctly ordered throughout the claimed size range; contours, material,
typography and motion have a coherent finish; and state changes preserve the
meaning rules above. Native and browser rendering must both meet these
criteria. A per-skin specification records the concrete evidence and any
expression-only exception.

The skill defines the production and verification procedure. This baseline is
a starting layout to implement before adjusting
for a chosen world's type and material. Numerical choices below are measured
from the accepted Cyan composition. Where the original decision rationale
cannot be established from code or rendering, the stated reason is an
engineering constraint to test, not a claim about the original designer's intent.

## Finished material at display size

Judge material with information rendered on it at the widget's actual size.
Frames, surfaces, highlights and small lettering must share a deliberate
construction: a richly rendered background cannot compensate for flat labels
or numbers. Low- and high-cardinality text may use different techniques, but
their weight, alignment, edge treatment and contrast must belong to the same
world. Review the full state and size matrix before claiming this finish;
material that only works in a concept mock is incomplete.

Compare against the chosen concept for its intended world and against the
quality criteria here for the finished skin. The concept sets a direction, not
fixed coordinates, colors or a stopping point. Observe letterforms, surface
depth, joint details, intentional light, information hierarchy and motion in
native and browser renders. Record specific shortcomings and revise the
material or typography before treating the skin as complete.

### Self-assessed finish gates

The creator judges these gates from **unscaled, information-filled renders** at
every lower-bound size pair and the declared default size. For a discrete
matrix, this includes incomparable narrow and short cases. Selection, Score and
History Graph also use their package-preview sizes. A concept image,
isolated texture, CSS declaration, DOM rectangle or enlarged crop cannot prove
one of them. All applicable gates must pass independently; strong background
art cannot compensate for weak lettering or an unfinished panel. Record the
observed evidence and the correction for any failed gate in the skin's
specification. The creator performs this judgment before asking another person
to review the skin.

| Gate | Evidence needed for a pass | Fail and revise when |
| --- | --- | --- |
| World in the widgets | Render an actual final-source background-off scene containing information-filled Select, Score and History in both hosts. Their panel contour, surface, typography and state accents still communicate the adopted world. | The off-state proof shows only EMPTY or omits an information widget; only background art or accent hue carries the concept; the panels could belong to an unrelated skin. |
| Material construction | The edges, joints, face and highlights used by the adopted world form a coherent construction at actual size. Record the selected concept's defining material cue on the information face and show its painted counterpart in Select, Score and History. A world that calls for glass, metal, print or fabric shows that material in the information-bearing surface and its edges. A deliberately flat world instead shows equally intentional shape, spacing and type. | A material-rich concept becomes a thin outline around a uniform fill; a face scan, reflection, grain or depth cue disappears during readability fixes without an equally finished replacement on the face; decoration floats without a plausible relation to the panel. |
| Lettering as material | Primary values, fixed labels, judgment labels, History numbers/headers, DJ LEVEL and CLEAR TYPE share the world's deliberate weight, alignment and surface treatment. In a material-rich world, inspect the ordinary glyph faces or contours at actual size: their deliberate glyph geometry, engraving, inset/raised edge, reflection, texture or other chosen treatment must remain visible in native and browser, including small labels. A lit or textured panel behind plain glyphs does not establish this gate. A font-family or weight change alone does not establish it either: point to visible features in the glyphs that belong to this world. Special badges may be richer, but cannot be the only materially finished letters. Dynamic Japanese/Latin titles, artists and options remain live and visually related. Labels are visibly quieter than values without becoming faint. Each bitmap glyph or sprite cell has enough source pixels for its effective displayed size in every verified state and package preview, unless an intentionally pixelated treatment is visibly finished at actual size. The whole atlas dimensions cannot stand in for its individual cells. | Rich art surrounds default-looking or blurry text; ordinary labels and numbers remain flat beside exceptional badges; the material cue exists only in the backing or adjacent glow; upscaled raster lettering has softened or doubled contours; atlas cells have visible padding/baseline jumps; labels and values collapse into one color or luminance role. |
| Hierarchy and density | At the real widget width, SCORE/DJ LEVEL and title lead, supporting rows can be read without zoom, and adjacent groups have deliberate spacing. Status, History and Graph retain their own clear reading order. Available panel area is used to make small information readable before text is reduced. | Texture, glow or ornament competes with values; compact labels disappear; rows are compressed while usable space remains; empty space or crowded groups make the intended order ambiguous. |
| Meaning and exceptional states | Difficulty, judgment, timing, rank and clear meanings stay distinct. At actual size, A, AA and AAA form a readable progression in both score and history: AA has a positive cue absent from A, and AAA adds a very positive material cue. AAA and FULL COMBO have individually recognizable treatments, including when they appear separately. Each history treatment retains a recognizable cue from its score counterpart. In Score, PGREAT has a visibly stronger positive treatment than GREAT/GOOD; FAST and SLOW remain equal-weight directions. Clear states preserve their own tiers. | A and AA differ only in the text string; PGREAT/GREAT/GOOD share one identical treatment; a generic brighter color stands in for every positive state; a special badge becomes only colored text in history or works only in the one preview state; a clear or timing state borrows the wrong meaning. |
| Motion in the material | At normal size, each intended continuous effect has a visible start, change and loop in both native and browser without moving data or obscuring text. Compare pixels inside the claimed effect region at chosen low/high phases in each host; the region must visibly differ, while static data regions remain stable. A deliberately still concept passes with a complete static appearance. | Animation exists only in CSS/code or only one host, all captured phases have identical effect pixels, a change is too small to perceive at display size, or motion supplies material quality missing from the still frame. |

Inspect both a full composition and each affected widget at its actual pixel
size. Use the latter to diagnose an issue, then recheck the full composition:
one impressive crop does not establish cohesion. If a gate fails, change the
underlying asset, glyph treatment, layout or motion and render again. Reducing
the claimed size range does not repair a failure at the skin's own default or
package-preview size. Do not average the gates into a score.
For small type, read the unscaled image without consulting the scene JSON or DOM
and transcribe each distinct label plus representative short, long and numeric
values. Compare the transcription to the supplied scene afterward. If a
required item is ambiguous until enlarged or revealed from source data, its
lettering and hierarchy gates fail even when the DOM value is correct.
For a material-rich world, compare the ordinary SCORE digits, fixed labels,
judgment labels and History lettering in a live-font version and a textured
atlas or glyph-mask version on the same panel at the same actual size. Match
the weight, width and brightness of the neutral control when assessing a font.
The candidate must show a material cue in the glyph itself, not only in its
backing. Count pixel differences only as proof that a technique rendered; they
do not establish craftsmanship. If the textured candidate is blurrier and the
live-text candidate lacks that cue, neither passes: revise the mask resolution,
font, layered treatment or production technique and rerender. Choosing the
sharper of two failing candidates is not a finish-gate pass. A clearly flat
world may omit the material candidate with its reason and actual-size evidence.
