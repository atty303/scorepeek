# ADR 0022: Select PP-OCRv6 small for contextual song recognition

- Status: Accepted
- Date: 2026-08-21

## Context

The official-model census measured title-only uniqueness on the same 3,061
stationary crops, 1,119 labelled songs, and full competing catalog. Corrected
native-dynamic PP-OCRv6 small recognized 1,110 songs completely, with one wrong
unique crop decision and 17 unknown or tied decisions. PP-OCRv6 medium
recognized 1,111 songs, with 14 wrong unique decisions and six unknown or tied
decisions. Their registered ONNX graphs are 21,159,378 and 76,554,979 bytes,
respectively. Small also read the result-screen artist `Yuta Imai` with
confidence 0.9686627984046936 in the existing result probe.

Song identity is not a title-only product decision. Artist and chart context,
plus a linked selection-to-result transition, can resolve an abstention. A
wrong unique title is more dangerous because it can appear compatible with a
different catalog song. The one-song title-only coverage advantage of medium
does not justify its larger graph and higher false-unique count.

## Decision

Use the registered official **PP-OCRv6 small native-dynamic** model and
`paddleocr-3.7.0-bgr-dynamic-rec-resize-3x48x320-3200-v1` preprocessor as the v1
text observer. Do not use the historical fixed-width small contract as the
runtime baseline. The same immutable model may observe title and artist, while
each field retains its own layout ROI and versioned preprocessing contract.
Decoded text and confidence are observations, never authoritative field values.
Song resolution continues to search the full catalog and fail closed on ties,
conflicts, or insufficient evidence.

Stop the active exhaustive phase-two comparison of medium and the other
official models. Also stop custom training/export, mapped-initializer work,
one-character specialists, per-song aliases, and further OCR-only optimization
for the current milestone. Preserve every existing observation and comparison
artifact as reproducible diagnostic and reopening evidence; do not delete or
reinterpret it as release validation.

Reopen another model or OCR-specific work only after the integrated small-model
path has been measured with artist, chart context, and play-attempt transitions,
and a residual failure is attributable to missing OCR signal. A challenger must
then resolve frozen evidence without increasing unsafe unique decisions and
must justify its runtime and implementation cost.

The 1,110-of-1,119 title-only result remains a diagnostic baseline, not the
product objective or release gate.

## Consequences

ADR 0006 is superseded only for its mandatory custom-fine-tune and direct-CTC
single-title runtime sequence. ADR 0020 is superseded for its requirement to keep every runnable official
model in phase two and for its no-selection state. ADR 0021 is superseded only
for its requirement to compare each decoder policy across every model; its
full-catalog search and imperfect-observation rules remain authoritative. ADR
0018 is refined: stationary music-list evidence remains the low-cost transfer
surrogate, but title-only model improvement is no longer the next task. ADR
0019 comparison keys remain part of candidate generation.

This selects an implementation baseline, not a supported capture profile,
threshold, result recognizer, or release-ready model bundle.
