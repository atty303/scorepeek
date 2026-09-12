# ADR 0151: Unify canvas background authority and native resource resolution

## Status

Accepted

## Context

ADR 0150 retained each mounted skin runtime and package, but a canvas background could still
disappear on Wayland while the same skin rendered correctly in a browser. Two independent values
represented the background: `CanvasPresentation.background`, written by the shared Appearance
control, and the manifest property map passed to skin Wasm. The browser and native hosts could
therefore render the manifest default after the editor had selected another background.

Skin Wasm also emits package-relative resource URLs in inline DOM styles. A browser canvas iframe
resolves them through its skin-specific base URL. Native canvases share one Blitz document and have
no equivalent document base, so an unqualified URL resolved outside the package resource namespace.
An opacity-only pixel assertion could not distinguish loaded artwork from an opaque fallback color.

## Decision

`CanvasPresentation.background` is the only writable canvas-background authority. Both browser and
native skin-input builders materialize that value as the effective `background` Wasm property.
Manifest `background` metadata remains the skin ABI declaration and default/migration input, but the
shared editor does not expose it as a second property control.

The native renderer-conformance adapter recursively qualifies only package-relative resource URLs
emitted by skin Wasm. It applies the immutable `/skin/{skin-id}/...` namespace to inline `url(...)`,
`src`, and `poster` values while preserving root-relative, fragment, `data:`, `blob:` and absolute
scheme URLs. The transformation occurs immediately after every native Wasm initialization or render
and before the retained tree is reconciled. It does not interpret editor actions or add native
editor state; it supplies the per-iframe resource-base behavior that browsers already provide.

The routine regression oracle must prove more than DOM presence or nonzero alpha. Browser
integration checks the full canvas background box, computed fallback color and package-qualified
artwork URL. Fake-Wayland integration runs the production Dioxus, skin Wasm/tree, resource resolver,
Blitz layout, scene and Vello raster path and asserts that a background pixel is both opaque and
different from the fallback color. The checked-in native composition scenario selects Static and
Animated backgrounds on a retained renderer.

## Consequences

Changing Background updates the same effective skin input on the browser and Wayland routes. Native
skins can use ordinary package-relative URLs without embedding a renderer-specific prefix, and
unrelated absolute resources are unchanged. Background behavior remains covered by ordinary browser
and fake-Wayland gates; neither an OBS-specific state path nor a native editor workaround is added.

