# Cyan System Instrument

**Responsibility:** Record this skin's world, original assets and their
provenance, visual expression, motion, final dimensions, baseline adjustments
and verified evidence. Reusable information and layout rules belong to the
authoring skill; API and package acceptance belong to the technical spec.

## Identity and world

- ID: `dev.atty303.scorepeek.skin.cyan-system-instrument`
- Display name: Cyan System Instrument
- World: a precision navy instrument. The selected A concept uses milled
  silver corner brackets, a recessed dark screen and narrow cyan light
  channels. The shape differs from Cyan System's geometric glass rails while
  retaining its technical, high-contrast identity.

## Materials and construction

The selected concept is retained as [concept-a.png](concept-a.png). The
[design master](design.png) puts exact synthetic text, score detail, history
and graph alongside that material study. [design-source.html](design-source.html)
is the editable initial composition source. They predate the later score layout,
logo, status and history refinements recorded below. Concept A remains the
material/world reference; this specification describes the current composition.
The master is not a package preview.

OpenAI imagegen generated original decorative material for this skin. The
concept prompt requested three blank precision instrument directions; the
owner selected A, with milled silver and cyan edge emitters. A second prompt
requested a uniform 16:9 navy brushed aluminum field with fine cyan hairlines,
without panels or text. A third prompt used only selected concept A as a
reference and requested an original square transparent flexible frame, with
machined silver brackets, dark rails, small fasteners and cyan channels. The
frame is assembled from eight edge/corner pieces by this skin's own DOM/CSS.
The center remains live text and data. No old skin art or skin-specific source
is included.

| Asset | SHA-256 |
| --- | --- |
| `concept-a.png` | `93c4e22e2555c791da9e91b601b48902a039f5e561b1c9d8248d1749069c88f7` |
| `resources/background.png` | `5502d4af39655b588d718ec71256b013a1a829f1869a3cf91357e2aa1ad3e710` |
| `resources/frame.png` | `64c69f73268db897f5b1450d42ee87ecb0e651221922967f5acb81f043928e58` |

The package uses independently fetched [Google Fonts Rajdhani](https://github.com/google/fonts/tree/23e54b51ddffbc7713c583748e3bd86f62b1fa4a/ofl/rajdhani)
SemiBold and [Orbitron](https://github.com/google/fonts/tree/23e54b51ddffbc7713c583748e3bd86f62b1fa4a/ofl/orbitron)
variable TTF at commit `23e54b51ddffbc7713c583748e3bd86f62b1fa4a`.
They were downloaded directly from `raw.githubusercontent.com/google/fonts`
at that commit on 2026-09-25, not copied from another skin. Font SHA-256s are
`94bbd25a18ca665999feb05a537de9fd2b860dcfb78bbe9ca00270825bf235da`
and `f42db2dd16e642258e35782916eceb1dcdbea06fb958d77ad71dc5963587e8fd`.
The upstream SIL Open Font License 1.1 files are packaged alongside them.
The original PNGs and fonts are checked in, so normal use does not invoke
imagegen or download a resource.

| Surface | Construction and proportion |
| --- | --- |
| Canvas | Navy brushed field with sparse cyan hairlines; cover the canvas only when static/animated background is requested. With background off, the canvas stays transparent. |
| All widget frames | Eight separate corner/edge pieces from the square frame asset surround a recessed navy screen. The nominal M corner/sheet sizes are 27/112 px, S 21/88 px and L 34/142 px; status uses the slimmer 19/79 px pair at M. The content begins 8 px vertically and 12 px horizontally inside the frame. Silver edges, dark joints and narrow cyan light channels remain distinct at widget size. |
| Status | A 156×34 px unboxed logo sits at the left. SYSTEM, RESULT and SELECT are separate 12 px lamps and labels at the right, with 14 px between signals. |
| Selection | A live song title and quieter artist occupy the upper region. A recessed 31 px rail below them has SP/DP, difficulty, LV and NOTES in that order; the first and third cells are about 70 and 80 px, and the other two share the remaining width. The title and artist never join the rail. |
| Score | The recessed face divides about 54%/46% between best and result detail. The left side has three rows: SCORE/DJ LEVEL at 54%/46%, MISS COUNT/CLEAR TYPE at 40%/60%, then the rank-ticked rate bar. The right side keeps compact judgment rows and a two-line PLAY OPTIONS ending at the same lower edge. Only the left/right boundary is ruled; the left rows have no extra separator. |
| History list | A restrained heading and 23 px table rows use DATE/SCORE/DJ LEVEL/MISS/CLEAR widths of 29/18/16/10/27%. Alternate dark row tones aid scanning; CLEAR is colored text without a per-cell plate or border. |
| History graph | A title and two color-coded legends sit above a plot with 32 px left rank axis and 36 px right miss-rate axis. Fine nine-part horizontal divisions and month ticks frame cyan DJ LEVEL and coral MISS RATE lines. The graph is a comparison surface, not a celebratory panel. |
| Empty | The frame encloses a cut-corner aperture with a transparent or requested-fill interior. The optional title is a small plate at the upper left; it does not expand the empty opening in the review board. |

## Typography and information hierarchy

Rajdhani sets status, labels, song and detail. Orbitron sets large numbers;
the approved transparent dark scorepeek logo from `docs/assets` sits directly in the status surface without
an opaque background or a secondary badge border. Japanese and mixed titles
retain system font fallback and live text.
Cool blue-gray labels (`#84aebd`) sit below near-white values (`#f6fbff`) in
contrast. Headings use bright cyan (`#5bdcf2`), which does not replace a
semantic state color. Primary SCORE is 30 px Orbitron; rank and miss values
are 22 px. Detail labels are 13 px Rajdhani and their values 14 px, aligned
on compact 16 px rows. Selection title is 28 px, artist 16 px. Long dynamic
text remains live and truncates with an ellipsis at the panel edge; an absent
text value appears as `—` rather than zero. The truncated cases have not yet
been accepted as readable across a size range. The chart difficulty keeps
its own semantic hue. Selection gives the title the largest weight, then
chart identity; artist and chart information have a distinct gap. SCORE is
the largest numeric treatment, with AAA in gold lettering and FULL COMBO on
a separate warm plate. The score bar has no separate rate caption, number or
MAX label. A signed nearest-threshold label shows progress from the current
rank lower bound or toward the next bound; its leading zeroes use
the inactive ink color (`#607c89`). The displayed distance uses a four-digit
minimum, for example `A+0012`, `AAA-0012` or `MAX-0008`; exact MAX has no
distance digits. The unfilled bar is still a visible track, and an invalid
distance is shown as `—`.

## Meaning-specific presentation

Difficulty uses green `#6be47a`, blue `#61bfff`, yellow `#f6d94c`, red
`#ff7580` and purple `#c89bff` for BEGINNER/NORMAL/HYPER/ANOTHER/LEGGENDARIA.
The colored difficulty word is set against a dark rail. SP/DP, level and notes
are separate fields. A and lower ranks use the neutral value color; AA uses
positive mint `#83dfba`; AAA uses very positive gold `#ffe396` with a small
warm reflection and glow. History keeps the same meaning hues at table scale.
NO PLAY is inactive, FAILED negative, ASSIST/EASY weak positive, CLEAR
positive, HARD somewhat stronger, EX HARD strong positive, and FULL COMBO the
most positive with a distinct gold inset plate and restrained glow. Rank and
clear meanings repeat in the history list as unboxed ink; the score badge
retains its material treatment. PGREAT is very positive; GREAT
and GOOD are positive; BAD, POOR and COMBO BREAK are negative. These meanings
color the judgment labels while their numbers share one high-contrast value
color.
FAST and SLOW have equally strong blue and red labels for timing direction.
The rate bar fills from navy through cyan at SCORE/(2×NOTES). F occupies the
first two-ninths; E through AAA occupy the remaining one-ninth intervals. It
has rank letters and ticks, without a separate rate label, number or MAX
caption. The result-detail labels carry judgment meaning hues; their numbers
remain the shared near-white value color. Unknown fields remain neutral.
The status lamps are cyan for active SYSTEM or RESULT, blue-violet
for recorded SELECT, coral for SYSTEM or RESULT error and dark/unlit for inactive.
RESULT provisional and confirmed both arrive as `active`; retracted arrives as
`error`. The skin has no separate raw-event distinction to render.
The graph's SVG viewBox follows the plot's actual aspect ratio so the
score-level and miss-rate steps have the same vertical extent in native and
browser rendering. A missing `miss_ratio` sample breaks its line rather than
inventing a value across that interval. Each graph point supplies a
`score_ratio`, so its line has no nullable interval. The SELECT lamp in
status mirrors `history.recorded`: on means this chart has a recorded play.
It remains separate from SYSTEM and RESULT; selection has no leading lamp.
No expressive exception to the [basic system](../../.agents/skills/create-overlay-skin/references/basic-design-system.md) is claimed.

## Motion

Animated canvas background varies opacity between 0.96 and 1 over an 8 s
alternate cycle. FULL COMBO's score plate has a 3.5 s alternate inset/glow
cycle. Status lamps and graph lines are static; motion does not change
numbers, graph geometry or state. Reduced-motion disables both cycles.
Native captures at initial and two seconds differ, as do browser
captures 0.85 seconds apart. Motion-disabled behavior still requires the full
matrix before this skin is accepted.

## Baseline adjustments and reasons

The basic system's Cyan-derived rows and ratios are realized directly here.
The 54%/46% score split leaves sufficient horizontal room for compact detail
labels at the reviewed 440 px width. The 40%/60% MISS/CLEAR split gives the
longer clear phrase room; the 66/68/24 px left rows align with the fixed right
detail density. The 29/18/16/10/27% history columns reserve room for date and
clear text. At the 440 px manifest width, the date cell uses 11 px type
instead of the 13 px general cell size to preserve the full timestamp, and
the list default height is 200 px rather than 170 px to show five 23 px rows
inside the frame and padding.
No other numeric baseline deviation is claimed yet. The frame's exact corner sizes,
type sizes and graph axis widths remain provisional until boundary renders.

## Verified dimensions and states

The following is the verified matrix for the synthetic review content,
including SP/DP, five difficulties, A/AA/AAA, MAX-, three-digit notes and
scores, all eight clear types, inactive/error status, missing MISS RATE
intervals and 0 values. S/M/L frame widths were rendered natively and in the
browser, as were background off/static/animated and EMPTY with/without title
at 0/20/100% fill. Ranges are bounded by the sampled pairs; arbitrary
width/height cross-products are not claimed.

| Widget | Native sampled size pairs (px) | Verified envelope for this content |
| --- | --- | --- |
| Status | 440×52, 544×52, 1840×52 | width 440–1840 at height 52 |
| Selection | 440×126, 544×124 | width 440–544, height 124–126 in those pairings |
| Score | 440×194, 544×200 | width 440–544, height 194–200 in those pairings |
| History list | 440×200, 600×200 | width 440–600 at height 200; five supplied rows visible |
| History graph | 440×214, 600×200, 600×208 | width 440–600, height 200–214 in sampled pairings |
| Empty | 300×80, 512×360 | small title aperture and large open aperture |

The 440×180 history trial clipped its fifth row, so the default and minimum
claimed height were raised to 200. The 440 px history date uses 11 px type
to display the full representative timestamp. Unbounded title, artist and
option strings may ellipsize; no full-string claim is made for arbitrary
length. EMPTY aspect presets, motion-disabled behavior
and sizes between the sampled pairs remain unverified. The package
`preview.png` was unchanged during the review-board work.

## Objective checks and visual comparison

The unchanged Cyan System was drawn with the same 1920×1440 eight-case native
scene as this skin. At 440 px, its SCORE/DJ LEVEL values overlap and its
selection title truncates, while this skin separates those fields. Both
preserve their technical identity; this skin has a machined silver frame,
recessed navy face and readable score hierarchy at the compared width. The
comparison is one-time production-flow evidence, not an authoring rule.

`mise run overlay:skins:build` produced a self-contained ZIP without importing
legacy shared CSS or fonts. The skill's 8-case native checker verified scene
coverage, complete captures, positive layout rectangles and one 1920×1440
review image. The browser verifier rendered the same eight states and sizes,
checking every widget, required labels and supplied values, resource requests,
and capturing each composed image. The normal preview generator produced and
installed 640×640 PNG and 8-second VP9 WebM from the production browser
renderer after review-board work. Native and browser material match in the
reviewed case after making the canvas background opaque. Actual editor
interaction, all variant motion frames and the remaining boundary cases above
are still open. Passing DOM text does not establish unclipped pixels; the
native and browser images were also inspected at their actual sizes.
