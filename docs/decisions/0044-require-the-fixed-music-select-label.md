# ADR 0044: Require the fixed music-select label

## Status

Accepted

## Context

An ordinary Wayland foreground session classified 19 startup frames as `music_select` before the
operator had entered music selection. The retained canonical frame at sequence 76 is the INFINITAS
hexagonal loading screen. It passed the existing aggregate color predicates with 12,125 cyan header
pixels against a 7,000 minimum and 41,572 colored level-column pixels against a 1,000 minimum.
Those predicates describe common INFINITAS palette regions, not a music-select-specific structure.

The fixed `MUSIC SELECT` label occupies canonical ROI `x=20, y=25, width=500, height=90`.
Independently retained canonical evidence measured pixels with all RGB channels above 178 in this
ROI. Forty-five retained frames from the false-positive startup run had at most 814 such pixels.
Stable music-select frames in the 2026-08-17 recording had 4,660 through 5,962, including a menu
overlay over the music-select screen.

## Decision

Keep both existing aggregate color predicates and additionally require at least 4,000 bright label
pixels in the fixed label ROI. Bind the ROI and threshold in the canonical layout, include both
observed and minimum counts in typed screen-predicate diagnostics, and fail closed when any of the
three music-select conditions is absent.

The canonical layout digest changes with this contract. Every dependent layout artifact must bind
the new digest. The retained startup frame must become `unknown`; the complete recording simulation
must continue to pass all reviewed result episodes through the production post-canonical path.

## Consequences

Startup animations no longer acquire field-observer or catalog-scoring authority merely because
they share INFINITAS's cyan palette. Brief entering or leaving animations whose fixed label has not
fully appeared remain `unknown`; this is intentional fail-closed behavior, not a coverage failure.
This decision establishes a measured development profile predicate. It does not establish release
accuracy, other capture-profile support, or music-select song acceptance.
