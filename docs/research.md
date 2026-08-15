# Research evidence and open verification boundaries

This file preserves the evidence that shaped the accepted plan. It is not a
substitute for live target-machine validation.

## Upstream snapshot

The local upstream checkout was inspected at commit
`d5ee7a887dc2d7bf37d0f747268ffcb6e42ea0f3` on 2026-08-15.

Relevant observations:

- Windows capture selection and recognition orchestration are coupled through
  `main.pyw`, `capture_winapi.py`, `capture_dxcam.py`, and `recog.py`.
- The effective recognition coordinate space is RGB 1920x1080 with hard-coded
  layout geometry and many exact color/template comparisons.
- Upstream currently has no committed golden capture/recognition suite. Raw and
  collection corpora are ignored, so shipped visual resources cannot be fully
  regenerated or validated from the checkout alone.
- `resources.py` imports Windows-only functionality and performs resource
  loading/update work at import time; it is not a suitable Linux runtime API.
- Resource versions declared by `define.py` include musictable 1.2,
  screenrecognition 1.0, informations 4.1, details 3.2, resultothers 1.0,
  musicselect 2.3, notesradar 1.2, unofficialdifficulty 1.0, and deeper 1.0.
  The inspected checkout contains `resources/deeper0.3.res`, demonstrating why
  adoption must validate actual files and schemas rather than trust declarations
  alone.
- Top-level crop coordinates and selected-difficulty coordinates are not fully
  encoded in `.res`; scorepeek therefore owns a versioned FHD layout profile.

These observations guide resource import and field parity only. They do not make
the upstream repository a source-layout template for scorepeek.

## Capture evidence

### OBS vkcapture

`obs-vkcapture` intercepts a Vulkan application's present path and copies the
selected swapchain image into exportable memory. If the selected client is the
game, it therefore observes the game swapchain before Gamescope output scaling.

- [obs-vkcapture project](https://github.com/nowrep/obs-vkcapture)
- [Vulkan swapchain copy path](https://github.com/nowrep/obs-vkcapture/blob/671886721d4f9f561d26fd4dceb006528b0c379a/src/vklayer.c#L1004-L1262)
- [OBS DMA-BUF import](https://github.com/nowrep/obs-vkcapture/blob/671886721d4f9f561d26fd4dceb006528b0c379a/src/vkcapture.c#L481-L513)

The accepted OBS profile deliberately treats this FHD OBS-rendered source as its
own calibrated input rather than claiming it is post-Gamescope output.

### OBS WebSocket screenshots

OBS Studio 28 and later bundles obs-websocket. The server recommends password
authentication and exposes `GetSourceScreenshot` for a selected source.

- [obs-websocket project and authentication guidance](https://github.com/obsproject/obs-websocket)
- [GetSourceScreenshot implementation](https://github.com/obsproject/obs-websocket/blob/1ef34bf48110c2a18184e50e41cd0b1a855e2147/src/requesthandler/RequestHandler_Sources.cpp#L28-L103)
- [Screenshot encoding and Base64 response](https://github.com/obsproject/obs-websocket/blob/1ef34bf48110c2a18184e50e41cd0b1a855e2147/src/requesthandler/RequestHandler_Sources.cpp#L148-L234)

The implementation creates rendering/staging resources, maps pixels to CPU
memory, encodes an image, and Base64-encodes each request. It is not a free frame
subscription; the 4 Hz, single-flight limit and target performance gate are
mandatory.

### Standard Gamescope PipeWire

The standard node can provide an output-sized capture, but its capture painting
and normal display composition are not identical paths. It must be treated as a
versioned Gamescope capture profile rather than a guaranteed copy of physical
scanout.

- [Gamescope PipeWire stream setup](https://github.com/ValveSoftware/gamescope/blob/df25cc1db980a1f545675763607faa0749bd6cac/src/pipewire.cpp#L72-L170)
- [PipeWire painting path](https://github.com/ValveSoftware/gamescope/blob/df25cc1db980a1f545675763607faa0749bd6cac/src/steamcompmgr.cpp#L2320-L2447)
- [Normal composite path](https://github.com/ValveSoftware/gamescope/blob/df25cc1db980a1f545675763607faa0749bd6cac/src/rendervulkan.cpp#L4030-L4145)
- [GStreamer device monitor](https://gstreamer.freedesktop.org/documentation/gstreamer/gstdevicemonitor.html)

A 3840x2160 BGRx frame is 33,177,600 bytes. At 60 changing frames per second,
the nominal payload approaches 2 GB/s before downstream normalization. A
consumer-side framerate limiter does not prove lower producer cost.

## Recognition evidence

- Upstream semantic resources, especially the music catalog, can carry new game
  content independently of Linux rendering calibration.
- Visual resources can often be decoded into field-specific templates, masks,
  glyphs, colors, and closed-set lookup tables.
- Exact matching remains the first decision. Approximate matching requires an
  absolute bound and a runner-up margin calibrated per field; ties reject.
- Song OCR is useful only as a second candidate generator for the closed catalog.
  It must never publish arbitrary text or override disagreement with the
  resource matcher.

Candidate model/runtime references:

- [OAR OCR models](https://github.com/GreatV/oar-ocr/blob/main/docs/models.md)
- [OAR OCR recognition-only example](https://github.com/GreatV/oar-ocr/blob/main/examples/text_recognition.rs)
- [PP-OCRv6 official model documentation](https://github.com/PaddlePaddle/PaddleOCR/blob/main/docs/version3.x/pipeline_usage/OCR.en.md)
- [ONNX Runtime installation documentation](https://onnxruntime.ai/docs/get-started/with-python.html)

## Not yet verified

The following are target-machine gates, not established facts:

- Exact Bazzite image, GPU/driver, Gamescope, PipeWire/GStreamer, Flatpak OBS,
  obs-vkcapture, and obs-websocket versions.
- The target OBS source UUID/settings and its native screenshot geometry.
- Exact Gamescope node caps and the FHD-to-4K pixel phase on the target.
- CPU/GPU/power impact of 4 Hz PNG screenshots and 4K PipeWire transfer while
  playing and streaming.
- All recognition thresholds, field acceptance rates, and false-positive rates;
  no private Linux fixture corpus exists yet.

No backend or recognizer should be called stable until these boundaries pass the
gates in the implementation plan.
