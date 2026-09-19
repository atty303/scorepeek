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

Three examples of how your overlay can look. Choose a skin, arrange your widgets,
or create a look of your own.

## What you get

### Automatic score tracking

Keep a local record of your play results without entering scores by hand. See
your personal bests, recent results, history, and progress graphs in the overlay.

### Screen recognition that keeps up

Scorepeek reads your game screen using image recognition, without analyzing the
game's internal data. It matches OCR readings against song catalogs fetched
online. New songs do not need their own set of training images, reducing the
upkeep needed as the catalogs grow.

The official catalog is distributed from the project's GitHub Pages site and
updated automatically. `scorepeek run` downloads it on first use, then checks
in the background after 24 hours while continuing to use the catalog selected
at invocation start. A custom ZIP URL can be set in
`$XDG_CONFIG_HOME/scorepeek/config.toml`:

```toml
[catalog]
url = "https://example.invalid/catalog/v1/catalog.zip"
```

`SCOREPEEK_CATALOG_URL` is a temporary higher-priority override. HTTPS,
loopback HTTP for development, and `file://` artifacts are supported.

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

## Project status

Scorepeek is in active development and is not yet ready for distribution or
general use. The current application targets Linux.

Development is private. No public license or redistribution permission has been
granted. Third-party data and assets remain subject to their own terms.
