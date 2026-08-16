# Research evidence and open verification boundaries

This file records the evidence behind the accepted design. It does not claim
that capture or recognition works on the target Bazzite machine.

## Upstream boundary

The Windows implementation was inspected once to understand the problem shape:
it assumes a fixed FHD coordinate system and couples capture, screen-state
handling, hard-coded layout, exact visual resources, and application state.
Those observations explain why a Linux renderer cannot safely reuse its visual
database.

scorepeek does not record or import the inspected coordinates, code, `.res`
files, pickle structures, catalog, templates, or generated output. Layout
coordinates are independently measured from the private scorepeek capture
corpus. No upstream update workflow exists.

## Catalog evidence

The accepted source matrix and rights boundaries are maintained in
[`sources.md`](sources.md). The important design consequences are:

- [Tachi](https://github.com/zkldi/Tachi/tree/main/db/seeds) offers broad IIDX
  song/chart records and source-scoped IDs, but its [MDB import
  process](https://github.com/zkldi/Tachi/blob/main/docs/src/contributing/cookbook/iidx-mdb.md)
  makes lineage explicit.
- [Textage](https://textage.cc/score/index.html) is independently maintained and
  includes title, chart, BPM, and product information. Its data uses the
  Windows-31J/CP932 web encoding, including bytes outside strict JIS Shift-JIS,
  and JavaScript assignments with comments, HTML fragments, and source-local
  identifiers. It therefore needs replacement-free decoding and a constrained
  parser, and cannot be executed or treated as universal ID data.
  [Textage use policy](https://textage.cc/score/readme.html)
- [dqn/iidxapi](https://github.com/dqn/iidxapi) follows the official INFINITAS
  page and is useful as a positive roster signal, but its [JSON
  rows](https://dqn.github.io/iidxapi/infinitas/music.json) contain no stable
  identity or full chart contract.
- Multiple websites often derive from the same MDB, Textage, or official-page
  lineage. Source count is therefore not evidence count.
- Similar title normalization is unsafe for identity. Punctuation, width,
  symbol, HTML, alternate-display, and source transcription differences must be
  preserved as exact variants or quarantined instead of fuzzy-merged.
- Adding a catalog candidate changes the runner-up for old OCR inputs even when
  model weights are unchanged. Catalog activation therefore needs replay of
  saved CTC logits, not only source-schema tests.

RemyWiki's [robots policy](https://remywiki.com/robots.txt) distinguishes
search/reference use from AI training, and no standard content reuse license
was found. The project treats it as manual reference only. OCR inference is not
model training, but fine-tuning a neural OCR model is; changing the model's
purpose from LLM to OCR does not remove that boundary. [Cloudflare Content
Signals](https://developers.cloudflare.com/bots/additional-configurations/managed-robots-txt/)

## OCR evidence

A song database contains labels and an inference lexicon, not the image/text
pairs needed to train a recognizer. Real game title crops remain necessary to
measure the renderer/font domain gap. Synthetic rendering supplies known labels
and broad controlled variation but cannot, by itself, prove accuracy on the
game's decorated titles.

The intended model is recognition-only sequence OCR:

- [PaddleOCR recognition training](https://www.paddleocr.ai/main/en/version2.x/ppocr/model_train/recognition.html)
  consumes image paths paired with text labels.
- [PP-OCRv6 recognition models](https://www.paddleocr.ai/main/en/version3.x/module_usage/text_recognition.html)
  provide a pretrained starting point rather than a song-class model.
- Paddle's [ONNX conversion
  path](https://paddlepaddle.github.io/PaddleOCR/main/en/version3.x/deployment/obtaining_onnx_models.html)
  makes a Python training/Rust inference split possible, but model-version
  compatibility must be proven by a parity spike.
- ONNX Runtime lists Rust as a [community
  API](https://onnxruntime.ai/docs/get-started/community-projects.html), so the
  exact Rust wrapper, runtime library, model, dictionary, and preprocessing
  contract must be pinned together.

The decoder scores CTC logits against exact catalog variants. A generic OCR
confidence or edit-distance nearest title is not sufficient evidence. New songs
can be recognized after a catalog-only update only when their tokens are already
supported and their rendering remains within the trained domain; unknown glyphs
or styles correctly remain unknown until the private corpus and model change.

## Capture evidence

### OBS vkcapture

`obs-vkcapture` intercepts a Vulkan application's presentation and copies its
selected swapchain image. Capturing the game process therefore observes its
native FHD swapchain before Gamescope output scaling, which violates the
post-scale frame contract.

- [Vulkan swapchain copy path](https://github.com/nowrep/obs-vkcapture/blob/671886721d4f9f561d26fd4dceb006528b0c379a/src/vklayer.c#L1004-L1262)
- [OBS DMA-BUF import](https://github.com/nowrep/obs-vkcapture/blob/671886721d4f9f561d26fd4dceb006528b0c379a/src/vkcapture.c#L481-L513)

OBS remains a candidate only if the existing streaming setup can share a source
whose native pixels are proven to be the 4K post-scale Gamescope output. Standard
Wayland Gamescope does not expose that as an ordinary Vulkan swapchain; forcing
an SDL backend changes the complete rendering/performance profile and must be
tested as such. [Gamescope Wayland backend](https://github.com/ValveSoftware/gamescope/blob/df25cc1db980a1f545675763607faa0749bd6cac/src/Backends/WaylandBackend.cpp#L2292-L2299)

OBS WebSocket `GetSourceScreenshot` creates a render/readback/PNG/Base64 request
path. It is useful for diagnostics but is neither a subscription nor the planned
production capture transport. [Implementation](https://github.com/obsproject/obs-websocket/blob/1ef34bf48110c2a18184e50e41cd0b1a855e2147/src/requesthandler/RequestHandler_Sources.cpp#L28-L103)

### Gamescope direct PipeWire

Gamescope can publish an output-sized PipeWire stream, but the capture painting
path and normal display composition are not identical. Direct capture must be
compared with outer Portal capture rather than assumed to be final scanout.

- [PipeWire stream setup](https://github.com/ValveSoftware/gamescope/blob/df25cc1db980a1f545675763607faa0749bd6cac/src/pipewire.cpp#L72-L170)
- [PipeWire painting path](https://github.com/ValveSoftware/gamescope/blob/df25cc1db980a1f545675763607faa0749bd6cac/src/steamcompmgr.cpp#L2320-L2447)
- [Normal composite path](https://github.com/ValveSoftware/gamescope/blob/df25cc1db980a1f545675763607faa0749bd6cac/src/rendervulkan.cpp#L4030-L4145)

A 3840x2160 BGRx frame is 33,177,600 bytes. At 60 changing frames per second,
the nominal payload approaches 2 GB/s before normalization. Consumer-side frame
dropping does not prove that Gamescope avoids producer work.

### Portal reference

Wayland ScreenCast Portal observes the compositor-managed window/monitor output
and is the correctness reference for post-scale pixels. Portal implementations,
negotiated formats, color management, picker persistence, and crop geometry vary,
so the target profile must record and verify them instead of assuming desktop
names imply capabilities.

## Not yet verified

- Target Bazzite image, GPU/driver, compositor, portal backend, Gamescope,
  PipeWire/GStreamer, Flatpak OBS, and obs-vkcapture versions
- Portal-selected surface geometry and negotiated pixel/color contract
- Whether the target OBS setup has any reusable 4K post-scale source
- Geometry and semantic equivalence among Portal, Gamescope direct, and OBS
- CPU/GPU/power/frame-time impact while simultaneously playing and streaming
- Layout coordinates, preprocessing, OCR export parity, field thresholds,
  acceptance rates, and false-positive rates
- Source adapter behavior against future live schema changes

Development-host builds, synthetic fixtures, and source parsing cannot satisfy
these target-machine or private-corpus gates. No backend or recognizer is stable
until the plan's Bazzite and holdout criteria pass.
