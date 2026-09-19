<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/scorepeek-logo-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/scorepeek-logo-light.png">
    <img src="docs/assets/scorepeek-logo-light.png" alt="scorepeek" width="760">
  </picture>
</p>

<p align="center">
  An IIDX companion that automatically records your results and brings your progress into view.
</p>

| Cyan System | Result Aurora | DJ Blackbox |
| :---: | :---: | :---: |
| ![Cyan System overlay preview](skins/cyan-system/preview.png) | ![Result Aurora overlay preview](skins/result-aurora/preview.png) | ![DJ Blackbox overlay preview](skins/dj-blackbox/preview.png) |

> **NOTICE**
> Scorepeek is in active development. Published builds are early releases and
are not yet ready for general use.

## What you get

### Automatic score tracking

Keep a local record of your play results without entering scores by hand. See
your personal bests, recent results, history, and progress graphs in the overlay.

### Screen recognition that keeps up

Scorepeek reads your game screen using image recognition, without analyzing the
game's internal data. It matches OCR readings against song catalogs fetched
online. New songs do not need their own set of training images, reducing the
upkeep needed as the catalogs grow.

### Local processing, local records

Recognition runs on your machine, and your scores are saved locally. No cloud
OCR or screen uploads are needed to recognize and record your results.

### Build your own tools

The recognition core is separated from capture and presentation for portability
and reuse. Use the Event API for live recognition events and SQLite for recorded
scores and history to build your own dashboards, analysis tools, or integrations.
The included overlays are optional.

### For your screen and your stream

Show an overlay on your own screen, in an OBS broadcast, or both. It is just as
useful for everyday play when you are not streaming.

### Make the overlay yours

Choose the information you want to see, move and resize widgets in the visual
editor, and decide which game screens show them. Install skins or create your
own; the three previews above are examples, not a fixed set of styles.

## Releases

GitHub Releases use `YYYY.M.COUNTER` CalVer, starting at counter `0` each month.
Each release contains the Linux x86-64 archive produced by the repository's
verified cargo-dist build. Release automation verifies its locally computed
SHA-256 against the digest recorded by GitHub after upload.

## Third-party notices

Third-party source acknowledgements and terms are documented in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
