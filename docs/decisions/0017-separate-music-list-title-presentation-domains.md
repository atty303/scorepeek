# ADR 0017: Separate music-list title presentation domains

## Status

Accepted

## Context

The paced HYPER full-list recording confirms that right-list title geometry is not one uniform
rendering domain. Locked songs are dimmed. Selecting a locked row inserts an unlock-condition bar
in the following geometric slot, so that slot is not a title crop. INFINITAS titles use blue text,
and LEGGENDARIA titles use purple text. These presentation changes are independent of whether the
list is stationary or scrolling.

Treating low brightness or a non-standard hue as motion, clipping, or OCR failure would corrupt
stability calibration and provisional labels. Applying an unmeasured recoloring transform would
also turn a plausible correction into unverified training evidence.

## Decision

- Temporal state remains independent from title presentation. A stationary or scrolling title
  observation records availability (`available` or `locked_dimmed`) and color domain (`standard`,
  `infinitas_blue`, or `leggendaria_purple`) separately.
- An unlock-condition bar occupies a geometric list slot as explicit `non_title` evidence. It never
  receives the selected title, the following title, or an inferred catalog label.
- Locked, INFINITAS-blue, and LEGGENDARIA-purple observations remain useful private evidence, but
  they are not silently treated as standard available-title pixels. A correction or color
  normalization must be versioned and calibrated from artifact-bound observations before those
  domains can contribute equivalent baseline training examples.
- Selected rows, clipped rows, and obscured rows retain the exclusions from ADR 0016. Presentation
  metadata does not make an incomplete or selected crop admissible.
- Motion thresholds are computed from artifact-rehashed RGB bytes within declared presentation
  domains. A threshold must fail closed if the retained calibration does not establish separation
  for a domain.

## Consequences

- Stability calibration cannot mistake intentional dimming or title hue for scrolling.
- The HYPER recording can retain all observed rows without guessing a correction; initially
  unsupported presentation domains remain quarantined rather than discarded.
- Corpus tooling needs artifact-bound verification plus per-observation presentation metadata
  before deduplication or catalog-bound provisional-label generation.
