# Scorepeek Vulkan capture layer

This explicit Vulkan layer captures an eligible application's newest swapchain before an outer
compositor such as Gamescope scales it. Scorepeek is the consumer and requests at most one frame
every 100 ms through the fixed per-user socket
`$XDG_RUNTIME_DIR/scorepeek/vulkan-capture.sock`.

The disconnected present hook is a pointer check. Capture resources are created only after a
Scorepeek consumer connects. A request records one GPU-local image copy, a producer-local fence
guards `READY`, and a consumer-local fence guards CPU readback. Scorepeek sends `ACK` only after
the asynchronous worker has taken ownership of the copied bytes; the single shared image is not
reused before that ACK. The layer enables `VK_EXT_swapchain_maintenance1` and uses an existing
application present fence when supplied, or attaches its own otherwise, so disconnect cleanup does
not destroy the local present semaphore until the presentation engine has released it. Reset and
destruction interception synchronize a borrowed fence with cleanup without taking ownership of it.
Application fences created for external sharing or given an imported payload fail capture because
another handle can replace the payload that proves presentation completion.
If that feature cannot be enabled, capture stays disabled
without changing the application's device-creation result. There is no external semaphore and no
frame ring or catch-up queue.

Every Scorepeek binary build invokes pinned Zig for a ReleaseFast, stripped layer and embeds that
library and the manifest as a deflate ZIP. A distributed binary installs its embedded copy
only when `scorepeek vulkan-layer install` is run; `scorepeek run` never installs or updates it.

After installation, Vulkan Loader discovers the layer from the user's XDG data directory. Scope
activation to the game process after Gamescope's `--`:

```text
env VK_INSTANCE_LAYERS=VK_LAYER_SCOREPEEK_capture APPLICATION
```

When `APPLICATION` is started by `umu-run`, expose the socket directory to Pressure Vessel:

```text
env PRESSURE_VESSEL_FILESYSTEMS_RW="$XDG_RUNTIME_DIR/scorepeek" VK_INSTANCE_LAYERS=VK_LAYER_SCOREPEEK_capture umu-run APPLICATION
```

Activation still requires
`VK_INSTANCE_LAYERS=VK_LAYER_SCOREPEEK_capture`, and Pressure Vessel still requires the socket
exposure setting shown above.

The layer is compatible with `obs-vkcapture`; set its normal activation variable alongside these
values when OBS capture is also required. Scorepeek does not start or stop Gamescope, Steam,
Proton, OBS, or the game.
