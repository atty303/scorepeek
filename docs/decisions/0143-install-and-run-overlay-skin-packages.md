# ADR 0143: Install and run overlay skin packages

- Status: Accepted
- Date: 2026-09-08
- Supersedes: ADR 0123 and ADR 0124's binary-owned skin set; ADR 0131 through ADR 0134's shared fixed skin renderer; ADR 0138's fixed appearance controls.
- Complements: ADR 0122's independent overlay consumers, ADR 0125's host-owned widget geometry, ADR 0127's semantic visibility, and ADR 0140's Wayland paint cadence.

## Context

The executable owns a closed `Skin` enum, all three styles and artwork, and their Dioxus rendering
branches. Adding an appearance therefore requires rebuilding scorepeek. Native cannot execute a
browser script, while loading a skin-specific `.so` would make skin builds platform-specific.

Skins are operator-selected local inputs. This feature does not add an adversarial sandbox, a
registry, validation for every CSS feature, or a special rendering primitive.

## Decision

One skin is one ZIP with root entries `skin.toml`, `skin.wasm`, `skin.css`, and `preview.png`;
`preview.webm` and arbitrary relative resources are optional. The manifest declares a lowercase
reverse-domain `id`, opaque non-empty `release`, display name, integer `api_version`, and
canvas/widget property schemas. Unknown manifest fields are ignored. The initial API version is 1.

`scorepeek skin install ZIP`, `scorepeek skin uninstall ID`, and `scorepeek skin list` manage
original ZIPs below the XDG data store. Install validates structure, UTF-8 CSS, preview PNG, the core
Wasm ABI, and native/OBS initialization with synthetic input before an atomic rename. The same ID
and release is a no-op; a different release atomically replaces it after property scope/type
compatibility checks. Release is identity, not SemVer. No registry, signature, digest, quota,
pruning, source-location record, or running-instance replacement protection is added.

The module exports linear memory plus `scorepeek_alloc`, `scorepeek_dealloc`, `scorepeek_init`, and
`scorepeek_render`. Calls exchange UTF-8 JSON according to the documented v1 ABI. Native uses
Wasmtime without WASI or host imports and interrupts calls with a generous wall-clock epoch
deadline. OBS uses an independent Web Worker instance per canvas and terminates timed-out workers.
Plugins cannot create host timers, threads, filesystem/network operations, or editor input.

Each render returns a complete keyed JSON tree and an idle, next-frame, or after-ms schedule.
After-ms is bounded to `2147483647` milliseconds so native and browser timers share one contract.
Keys are non-empty and unique. The host reconciles arbitrary ordinary HTML/SVG elements, attributes,
text, and CSS into browser DOM or Blitz DOM. The canvas is the final clip; decoration may overflow
widget geometry. Relative URLs resolve only inside the ZIP. OBS applies a self/package-only CSP. No
tag, attribute, CSS-property, tree-size, memory-size, package-size, or store-size policy is imposed.

The host owns canvas/widget geometry, semantic snapshots, prepared score/history data, editor UI,
validation, and persistence. Manifests may declare boolean, bounded integer/number, color, enum, or
bounded string properties. Updates may add/remove properties and enum values but may not change a
surviving key's scope or type. Invalid persisted values resolve silently to defaults until normal
SAVE. Switching skins copies only same-key, same-type values valid for the target and otherwise uses
target defaults; the switch is one undo action.

Both editors display the mandatory PNG for the selected skin; the OBS editor may also play its
optional WebM. Editor controls come only from the selected manifest. Native editing renders the
selected draft through the same Wasm/tree/CSS implementation as normal display, but in a separate
Wasm instance so sample-state preview cannot mutate live guest state. It retains host-owned geometry
hit regions for every visible canvas.

Configuration advances to schema v6. Skin values are arbitrary IDs. The v5 migration maps the
three legacy names to formal IDs and preserves background/frame/fill choices as properties even if
a package is absent. Missing packages are not replaced and uninstall does not rewrite config. With
no config, the lexicographically first installed ID creates one empty Wayland and one empty OBS
canvas. With no installed skin, configuration creation fails. There is no built-in fallback.

The repository retains source for the three designs and a shared Rust guest SDK/core.
`mise run overlay:skins:build` creates uncommitted ZIPs; normal builds do not install them. Visual
and E2E tasks install packages into isolated XDG roots and clean owned state.

Private diagnostics reuse the existing bounded child-to-parent path. They may identify skin ID and
release, canvas ID, backend, phase, duration, next tick, tree-apply outcome, trap, and timeout. They
may distinguish compile, instantiate, init and render failures without retaining error text. They
never record DOM/CSS, render input, score/player data, or public Event API records.

Before the first distributed scorepeek build, API v1 may break in place. Distribution freezes v1
to additive evolution; later breaking changes require a new integer version.

## Consequences

- Skin authors build one portable Wasm artifact and verify browser and native CSS behavior.
- Wasmtime increases native build time and binary size; ZIP support is limited to ordinary
  stored/deflate packages.
- A running process may observe replacement or uninstall inconsistently until restart. This is an
  unsupported case, not a lock, lease, hot reload, or generation-pinning contract.
- The host API stays narrow while DOM and CSS expression remain unrestricted.
