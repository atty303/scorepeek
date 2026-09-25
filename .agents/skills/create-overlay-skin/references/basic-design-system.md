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
| Canvas | Requested background off/static/animated and EMPTY apertures | Widgets and user content lead over background material |

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
  in score and history. FULL COMBO has an independent treatment; either can
  appear without the other.
- A clear state is written as text. NO PLAY is inactive, FAILED is negative,
  ASSIST and EASY are weak positive, CLEAR is positive, HARD is somewhat
  stronger, EX HARD is strong positive, and FULL COMBO is the most positive.
  Color, light and motion reinforce the text consistently in score and history.
- PGREAT is very positive; GREAT and GOOD are positive; BAD, POOR and COMBO
  BREAK are negative. These meaning groups remain distinguishable by color
  and text without changing the supplied counts.
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
personality, spacing and motion rhythm. Material must retain deliberate
thickness, edges, joints, reflection or engraving at actual widget size. A
flat border that merely hints at an approved material is incomplete. Background
art gives canvas gaps texture without baked-in text, widgets or game imagery.
Background off is a complete visual state.
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
