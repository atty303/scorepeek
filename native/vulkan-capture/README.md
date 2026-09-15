# Scorepeek Vulkan capture layer

This explicit Vulkan layer captures an eligible application's newest swapchain before an outer
compositor such as Gamescope scales it. Scorepeek is the consumer and requests at most one frame
every 100 ms through the fixed per-user socket
`$XDG_RUNTIME_DIR/scorepeek/vulkan-capture.sock`.

The disconnected present hook is a pointer check. Capture resources are created only after a
Scorepeek consumer connects. A request records one GPU-local image copy, a producer-local fence
guards `READY`, and a consumer-local fence guards CPU readback. Scorepeek sends `ACK` only after
the asynchronous worker has taken ownership of the copied bytes; the single shared image is not
reused before that ACK. The layer enables `VK_EXT_swapchain_maintenance1` and attaches a
per-present fence, so disconnect cleanup does not destroy the local present semaphore until the
presentation engine has released it. If that feature cannot be enabled, capture stays disabled
without changing the application's device-creation result. There is no external semaphore and no
frame ring or catch-up queue.

`mise run build` builds both Scorepeek and the layer with pinned Zig. Development artifacts live
under `target/vulkan-capture`; release installation layout is intentionally deferred until the
first release.

For a development run, start `scorepeek run --capture vulkan-layer` and scope the explicit layer
to the game process after Gamescope's `--`:

```text
env VK_LAYER_PATH="$PWD/target/vulkan-capture/share/vulkan/explicit_layer.d" VK_INSTANCE_LAYERS=VK_LAYER_SCOREPEEK_capture APPLICATION
```

The layer is compatible with `obs-vkcapture`; set its normal activation variable alongside these
values when OBS capture is also required. Scorepeek does not start or stop Gamescope, Steam,
Proton, OBS, or the game.
