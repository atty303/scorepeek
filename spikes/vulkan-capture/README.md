# Vulkan capture spike

This active spike tests whether a Vulkan layer can copy a game's swapchain
before Gamescope scales it, without consuming the Gamescope PipeWire source.
It is deliberately separate from Scorepeek's supported capture profiles and
does not feed recognition.

The explicit layer adds transfer-source usage to an eligible swapchain and
owns one exportable GPU image. A consumer requests frames at 10 Hz. On a
request, the present thread records and submits one copy without waiting for
the consumer. A layer worker waits for the producer fence and sends `READY`;
the consumer imports the DMA-BUF, copies it to CPU-visible memory, waits on its
local fence, passes the bytes to an asynchronous digest/artifact worker, and
then sends `ACK`. The layer does not reuse the image before `ACK`. Requests
while the image is owned are dropped rather than queued or caught up.

There is intentionally no external semaphore in this spike. Queue submission
order plus the producer's local fence protects producer completion, and the
consumer's local fence plus `ACK` protects reuse. The Unix socket carries
control, lifecycle, and timing records but not frame bytes.

## Build and test

Zig 0.16.0 is the only toolchain managed by mise for this spike. Zig compiles
and statically links the C++20 layer and backend. `vkroots` and
`Vulkan-Headers` are content-pinned Zig package dependencies.

```text
mise run capture:vulkan:spike:build
mise run capture:vulkan:spike:test
```

Installed spike artifacts are written below
`target/vulkan-capture-spike`. The layer library exports only
`vkNegotiateLoaderLayerInterfaceVersion` and neither binary has a dynamic C++
runtime dependency.

## Bounded local run

Start the consumer first. The following example asks for ten frames and keeps
all temporary evidence outside the repository:

```text
./target/vulkan-capture-spike/bin/scorepeek-vulkan-capture-consumer --socket /tmp/scorepeek-vulkan-capture.sock --diagnostics /tmp/scorepeek-vulkan-capture.ndjson --artifact /tmp/scorepeek-vulkan-capture.ppm --frames 10
```

Put the layer variables after Gamescope's `--`. This scopes both capture layers
to the game process instead of injecting the Scorepeek layer into Gamescope:

```text
gamescope -W 1280 -H 720 -w 640 -h 360 -r 60 --expose-wayland -- env OBS_VKCAPTURE=1 VK_LAYER_PATH="$PWD/target/vulkan-capture-spike/share/vulkan/explicit_layer.d" VK_INSTANCE_LAYERS=VK_LAYER_SCOREPEEK_capture_spike SCOREPEEK_VK_CAPTURE_SOCKET=/tmp/scorepeek-vulkan-capture.sock vkcube --wsi wayland --width 640 --height 360 --c 600
```

Replace only the command after `env ...` for the eventual Proton/game trial.
Keep `OBS_VKCAPTURE=1`; its installed implicit manifest enables
`VK_LAYER_OBS_vkcapture_64` alongside this explicit layer.

The consumer prints one `scorepeek-vulkan-capture-result-v1` JSON record. Its
diagnostic file is bounded to 8 MiB and starts incomplete; only a successful
`run_end` makes the run complete. The first consumed frame is saved as raw PPM
with adjacent JSON metadata. Frame bytes never enter diagnostics. Delete the
PPM and metadata after visual inspection.

The layer tolerates startup swapchain replacement by handing capture to the
new swapchain, while the consumer reimports the replacement DMA-BUF under the
same run. Supported formats are currently 8-bit RGBA/BGRA swapchains only.

## Promotion boundary

The spike answers only feasibility and game-session cost. Promotion requires
the operator's `bm2dx.exe` play assessment with the actual 1920x1080, 120 Hz
launch profile and obs-vkcapture enabled. If that assessment is acceptable,
the protocol and capture path can be integrated behind a versioned Scorepeek
capture profile with its own semantic, lifecycle, and performance gates. If
not, remove the spike rather than treating its vkcube result as game support.
