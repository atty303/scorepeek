# ADR 0131: Rich semantic overlay presentation

- Status: Accepted
- Date: 2026-09-06
- Supersedes: ADR 0123/0124's settled-animation requirement, single Latin font and restricted decorative treatment; ADR 0125's widget-level decoration clipping.

## Decision

Keep the five widget roles, their information grouping and relative emphasis. The three original
masters define worlds, not pixel fidelity: technical cyan/silver, violet aurora, and charcoal DJ
hardware. Use embedded original artwork, layered materials and role-specific typography generously.
Visible canvases may animate continuously; visual richness takes precedence over rendering cost.

Native and OBS share the Dioxus DOM, CSS, artwork, font bytes and semantic motion settings. A fixed
embedded motion specification defines selectors, properties, periods, phase offsets and waveforms.
Rust applies it through Blitz's style-property API on native frame callbacks. JavaScript applies the
same specification through requestAnimationFrame. Frame-exact cross-backend synchronization is not
required. Native hidden surfaces retain their transparent/idle behavior. Browser motion suspends
when the document is hidden. No independent drawing engine or runtime asset download is introduced.

Meaning selects presentation: clear type, rank, difficulty, lamps and primary versus supporting
numbers have distinct roles. FULL COMBO and EX HARD may keep glowing/pulsing while present; those
motions do not claim a new achievement or persistence event. Actual values change immediately,
without count-up or invented intermediate values. Text remains semantic and selectable; numeric
atlases are permitted but are not needed by this implementation.

Decorations may overflow widgets. Canvas boundaries clip them, and the operator adjusts canvas
size and widget positions visually. Decorative nodes do not receive pointer events. Content keeps
its existing clipping behavior; no automatic canvas padding or position adjustment is added.

Embed Orbitron and Rajdhani alongside Oxanium, plus one Noto Sans JP variable font for Japanese.
Keep their immutable source, hashes and OFL licenses with the assets. System fonts remain a fallback
for characters outside the bundled coverage. No host font installation is required.

## Verification boundary

The native visual scenario supports a skin override and explicit `motion` timestamps. Its PNG,
selector-layout JSON and complete/partial manifest remain the evidence for actual Blitz/Vello paint.
The browser uses the production full-screen stage, iframe, editor and motion driver. Compare both
visually; layout rectangles or accepted CSS alone do not prove native paint support. Native time
samples establish deterministic frame changes, while Wayland presentation and OBS composition stay
separate live gates.
