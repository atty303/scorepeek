# Vello image atlas investigation

This document preserves the evidence for a possible future upstream report. It describes a
renderer-level failure isolated while investigating disappearing raster backgrounds in the native
scorepeek overlay. It is not an upstream blame assignment or a confirmed upstream fix.

Evidence was collected on 2026-09-06 with these locked dependencies:

- `anyrender` 0.13.0
- `anyrender_vello` 0.14.0
- `vello` and `vello_encoding` 0.10.0
- Blitz 0.3.0-beta.2 at commit `64eb27853aa2672486b7edf825fb044be78c9db3`

The headless reproduction used the WGPU Vello buffer-rendering path. The exact WGPU backend and
adapter were not recorded and must be included when preparing an upstream report.

## User-visible symptom

The Wayland overlay maps one canvas per output and keeps its layer surface mapped while the
semantic game screen changes. A canvas that is not applicable to the current screen is painted
transparent rather than unmapped, because remapping a layer surface causes a conspicuous compositor
animation.

After the following sequence, raster backgrounds disappear while vector and text content may remain:

1. render a canvas containing a raster background;
2. change to a screen for which that canvas is transparent;
3. change back to the original screen.

The same failure is reproducible without Wayland, so surface configuration and compositor behavior
are not required causes.

## Layer-by-layer isolation

All probes retained one renderer for the complete three-frame sequence.

| Probe | Frame sequence | Observation |
| --- | --- | --- |
| Production Dioxus Native DOM, Blitz paint, and AnyRender Vello | image, no image, same image | background missing on the third frame |
| Minimal Dioxus Native DOM and Blitz | image, no image, same image | image command counts were `1, 0, 1`; the decoded DOM image remained retained, but the third frame was transparent |
| Direct AnyRender Vello with the same `ImageBrush` | image, no image, same image | RGBA byte sums were `5_100_000, 0, 0` |
| Direct Vello public API with the same `ImageBrush` | image, no image, same image | RGBA byte sums were `5_100_000, 0, 0` |

The test image was an opaque red 100 by 100 RGBA image. Its expected byte sum is
`100 * 100 * (255 + 0 + 0 + 255) = 5_100_000`. The third frame should therefore equal the first
frame, rather than zero.

The direct Vello probe reset and rebuilt the `Scene` for each frame, matching the documented
immediate-mode usage. Its essential sequence was:

```rust
let mut renderer = Renderer::new(&device, RendererOptions::default())?;
let mut scene = Scene::new();
let image = ImageBrush::new(red_100_by_100_rgba_image());

scene.fill(Fill::NonZero, Affine::IDENTITY, &image, None, &bounds);
render_and_read_back(&mut renderer, &scene)?; // 5_100_000

scene.reset();
render_and_read_back(&mut renderer, &scene)?; // 0

scene.reset();
scene.fill(Fill::NonZero, Affine::IDENTITY, &image, None, &bounds);
render_and_read_back(&mut renderer, &scene)?; // 0; expected 5_100_000
```

This isolation rules out scorepeek canvas selection, Dioxus reactivity, Blitz DOM image lifecycle,
and AnyRender scene conversion as necessary causes. It does not prove that every Vello backend or
adapter has the same failure.

## Suspected invariant violation

The observed result is consistent with the resolver's image residency metadata outliving the
physical atlas that contains the resident image:

1. Frame A resolves the image, records it as resident, allocates an atlas, and uploads the image.
2. Frame B has no resource patches. [`Resolver::resolve`](https://github.com/linebender/vello/blob/v0.10.0/vello_encoding/src/resolve.rs#L183-L195)
   returns default empty image data before calling the image cache's `begin_resolve` path.
3. [`Renderer::render_to_texture`](https://github.com/linebender/vello/blob/v0.10.0/vello/src/render.rs#L148-L180)
   derives a 1 by 1 image texture from the empty dimensions and replaces the previous persistent
   atlas.
4. Frame C uses the same `Blob` identity. The
   [`ImageCache` resident path](https://github.com/linebender/vello/blob/v0.10.0/vello_encoding/src/image_cache.rs#L113-L125)
   still considers the entry clean, so resolution reuses its atlas coordinates without queueing
   another upload.
5. Rendering samples stale coordinates from the new empty atlas and produces transparent pixels.

This is more precise than ordinary cache eviction: the physical atlas is replaced without the
corresponding resolver residency state being invalidated or rematerialized. The older Vello image
resource design also describes atlas replacement as requiring resources to be rematerialized; see
[Vello issue 176](https://github.com/linebender/vello/issues/176).

## Candidate upstream repair boundaries

The evidence does not yet select one implementation. An upstream repair could enforce the invariant
at one of these boundaries:

- preserve the current atlas dimensions and contents across a resource-free frame;
- invalidate image residency whenever the renderer replaces the physical atlas; or
- force resident images to be uploaded again after atlas replacement.

The preferred fix should be chosen by Vello maintainers according to the intended lifetime contract
between `vello_encoding::Resolver` and `vello::Renderer`.

## Regression test contract

An upstream regression test should:

1. create one renderer and one image with a stable `Blob` identity;
2. render an image frame and verify its pixels;
3. render an empty or solid-only frame with no resource patches;
4. render the same image again;
5. assert that the first and third image regions are equal.

Testing both a reset-and-reused `Scene` and a newly created `Scene` would distinguish scene lifetime
from renderer/resource lifetime. A renderer-level GPU test is the strongest oracle; an additional
encoding-level test may directly assert atlas actions and upload queues.

## scorepeek workaround

Scorepeek appends a transparent 1 by 1 image to every native frame. This prevents a frame from having
no image resources and therefore avoids the atlas replacement/residency mismatch described above.
It is intentionally treated as a workaround, not as evidence that keeping a dummy image alive is
required Vello usage.

Before filing upstream, replace the temporary probe with a standalone repository or upstream test,
record the WGPU adapter and backend, run it against the current Vello release and main branch, and
attach validation results for the proposed fix. Also verify whether the issue reproduces when the
middle frame contains vector paint but no image, as well as when it is completely empty.
