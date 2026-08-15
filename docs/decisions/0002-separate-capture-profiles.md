# ADR 0002: Maintain explicit OBS and Gamescope capture profiles

- Status: Accepted
- Date: 2026-08-15

## Context

Linux rendering and capture paths do not produce the same pixels as the Windows
path used to generate upstream recognition resources. OBS game capture and the
standard Gamescope PipeWire node also observe different rendering stages and
have different performance costs.

## Decision

Support two independent frame sources behind one canonical frame contract:

1. OBS WebSocket v5 requests lossless 1920x1080 PNG screenshots from one exact,
   active FHD `vkcapture-source`, at no more than 4 Hz and one in-flight request.
2. Gamescope direct PipeWire requires a unique 3840x2160 SystemMemory BGRx node
   and applies a fixed 2:1 normalizer to canonical FHD.

Each backend has a distinct capture profile, recognition calibration, fixture
suite, and Bazzite performance gate. Selection is explicit and fixed for a
session; automatic fallback and frame mixing are prohibited.

## Consequences

- OBS can reuse the user's existing streaming setup without a custom OBS plugin,
  but its screenshot path must prove acceptable load on the target machine.
- Gamescope remains available when OBS is closed, at the cost of potentially
  expensive 4K SystemMemory transfer.
- A backend that fails its gate is not advertised as supported. No silent FHD,
  NV12, fuzzy-recognition, or alternate-capture fallback is added.
