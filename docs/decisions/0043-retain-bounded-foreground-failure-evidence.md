# ADR 0043: Retain bounded foreground failure evidence

## Status

Accepted

## Context

A normal foreground session may last several hours. Recording every canonical frame and every
full-catalog candidate row would make ordinary use impractical, while retaining only counters or a
post-recognition trigger would make a missed result impossible to repair without playing again.
Canonical QOI evidence can replay recognition but cannot test whether the selected binding and
normalizer reproduced those pixels correctly.

## Decision

Foreground diagnostic retention uses a fixed failure-window policy:

- sample unknown canonical frames once per second in a 12-frame rolling tail;
- flush that tail on a warm partial-result predicate or an unknown-to-known transition;
- retain known result frames once per second and ordinary known-screen baselines once per five
  minutes;
- retain exact raw BGRx source bytes only for the first retained partial-result frame in one
  interval and for the transition frame into a known screen; a transient cool predicate while the
  screen remains unknown does not close that interval;
- bind a raw source file and its complete video, memory, stride, timing, byte-count, and digest
  contract to the same manifest frame entry as its canonical QOI;
- keep all writes behind the existing non-blocking capacity-two worker and the existing per-run and
  aggregate byte limits, and account for every selected frame if a batched failure-window offer is
  rejected;
- write the retention-bearing run-start document as v2 while continuing to accept the exact
  canonical v1 document, whose absent retention field means the former complete-cadence policy.

The foreground recognition artifact retains one representative result observation per result
interval, prefers an accepted resolution, and starts a new interval after more than 30 seconds even
when no music-select observation separates consecutive result scenes. It retains the following
music-select observation and then samples music-select once per five minutes. Candidate metrics are
arrays in the exact catalog-table order, so song IDs and field names are not repeated for every
candidate. Bounded gates and offline simulation retain complete observations.

## Consequences

Recognition changes can be tested from canonical QOI without another game session. Transform
changes can be investigated from a paired raw/canonical frame and the run-bound profile/normalizer
identity. Raw evidence is not continuously recorded and repeated frames in the same partial-result
interval do not create repeated raw copies. Exact offline transform comparison tooling remains a
separate executable slice; until it exists, paired evidence is sufficient input but not a completed
transform conformance claim.
