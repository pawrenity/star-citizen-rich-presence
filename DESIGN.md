# Control panel design and performance plan

Oversight notes for the 3.2 visual/performance pass, with the results of each workstream.

Baseline captured from `target/release/scrp-before-redesign.exe` (v3.1.4) with
`scripts/screenshot.ps1`, all six pages, 916×627 window at 100 % DPI.

## What the baseline showed

| Area | Observation | Status |
| --- | --- | --- |
| DPI | No DPI awareness at all, so every window was bitmap-stretched and blurry above 96 DPI. | Fixed: per-monitor v2, `WM_DPICHANGED`, every coordinate scaled at the `rect()` boundary. |
| Card preview | 106 px strip, 78 px art, three text lines that repeat the rail status ("Waiting for Star Citizen" three times). | Fixed: 112 px Discord-shaped card, overline verb, title, location, ship/contract, mono timer, one status pill. |
| Empty space | 120–250 px of dead canvas on Card, Activity, Buttons and Updates. | Fixed: grouped cards, live previews and diagnostics fill the page; nothing is filler. |
| Hierarchy | Page title, row title and section label were all 13/600 or 11/600 grey. | Fixed: 18/600 title, 13/600 row, 11/400 detail, 10/700 tracked kicker with rules. |
| Navigation | Text-only rail, no icons, no hover, flat grey selection. | Fixed: MDL2 glyphs, hover pill, accent-tinted selection with a 3 px indicator. |
| Depth | Everything flat: no shadows, no highlights, no separators inside cards. | Fixed: card shadow, 1 px top sheen on raised surfaces, hairline dividers in groups. |
| Buttons | No hover/pressed distinction, dotted Win95 focus rectangle. | Fixed: primary/secondary/ghost states, hover + pressed, rounded accent focus ring. |
| Images | Grid was 8 across while the layout assumed 9, so the last tile collided with the margin; selection was a solid accent slab; pressed tiles inverted to a photo negative. | Fixed: exact 9-column grid, accent ring + check badge, no inversion. |
| Activity | Rows were stacked as five separate boxes; no indication of what "member list shows" changes. | Fixed: two grouped cards plus a live member-list preview row. |

## Performance

Measured with `scripts/screenshot.ps1 -BenchPaints 300`, which forces synchronous full
repaints through `InvalidateRect` + `UpdateWindow`:

| Build | Full repaint | Working set |
| --- | ---: | ---: |
| 3.1.4 baseline | 3.01 ms | 25.3 MB |
| 3.2.0 | **1.27 ms** | 33.8 MB |

What changed:

1. `shape()`/`circle()` no longer rasterise per call. `src/shapes.rs` (agent: GLM-5.3-Flash)
   caches anti-aliased rounded rectangles and circles in an LRU keyed by size, radius and
   colours; the panel keeps one in a thread-local.
2. `paint()` draws into a persistent back buffer (`BackBuffer`) instead of creating and
   deleting a full-window DIB every paint.
3. `text()` measures with `GetTextExtentPoint32W` after a single `DrawTextW` instead of
   drawing every string twice.
4. `paint_card()` copies four fields out of `Preferences` rather than deep-cloning both
   artwork maps on every paint.
5. Artwork thumbnails are cached on disk (`src/thumbnail.rs`, agent: DeepSeek V4.1 Flash):
   `%LOCALAPPDATA%\StarCitizenRichPresence\thumbnails\<hash>.bin`, three downloads at a
   time, pruned to 512 files / 128 MB. Later launches paint instantly.

The extra 8 MB of working set is the shape cache (up to 192 entries) and is a deliberate
trade for 2.4× faster paints.

## Design system

- **Surfaces:** `rail` < `canvas` < `tile` (`s1`) < `tile_hover` (`s2`) < `tile_pressed` (`s3`),
  with `field` sunk below the canvas. Separation comes from `tile_border` plus a 1 px top
  sheen, not from heavy outlines.
- **Theme tokens:** four themes with their own canvas, rail, tile, hover, pressed, border,
  divider, field and control-underline colours, plus a shared accent ramp
  (`accent` / `accent_hover` / `accent_pressed`) and semantic status colours
  (live `#57D987`, paused `#F5C56B`, error `#FF8A80`) that never borrow the accent.
- **Type scale:** 18/600 page title, 16/600 card title, 15/600 brand, 13/600 row title,
  13/400 body, 12/400 caption, 11/600 chip, 10/700 tracked kicker, 12 mono timer.
- **Accent discipline:** accent is only used for the primary button, the toggle-on track,
  the selected nav item and indicator, selected tile rings, focus rings and links.
- **Contrast:** secondary text is `#A4A4AE`-class on every theme (≥ 5:1 at 11–13 px),
  control underlines carry the boundary (`#8B8B8B`-class, ≥ 3:1 non-text), and disabled
  text is a separate token so "off" no longer looks "disabled".

## Verification

- `cargo build --release`, `cargo clippy --all-targets`, `cargo fmt --check`, `cargo test`
  (58 unit + 8 content + 4 replay tests) all clean.
- Screenshots of all six pages at 100 % and 150 %, plus the update-banner state and a
  populated card, in `scripts/screenshot.ps1` output.
- Paint benchmark above.

## Developer tools

`scripts/screenshot.ps1` (PowerShell) starts the panel, switches page, optionally hovers a
control (`-HoverId`), opens a dropdown (`-DropId`) and captures the window with
`PrintWindow`. `-BenchPaints N` times N synchronous repaints instead of capturing.

`SCRP_UI_SCALE=125` forces a scale factor so other DPI settings can be screenshotted.
`SCRP_DEMO=1` seeds a fake live presence and `SCRP_DEMO_BANNER=1` shows the update banner,
so the populated states can be reviewed without launching Star Citizen.

## Known local-only warning

The GNU (mingw) toolchain prints `.rsrc merge failure: multiple non-default manifests`
when linking. It is pre-existing, comes from ld merging our manifest with the CRT's, and
does not occur on the MSVC toolchain used by CI. The runtime
`SetProcessDpiAwarenessContext` call makes DPI awareness independent of manifest merging.
