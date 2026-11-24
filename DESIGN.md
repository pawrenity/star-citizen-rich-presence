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

| Build | Full repaint | Binary |
| --- | ---: | ---: |
| 3.1.4 baseline | 3.01 ms | 4.28 MB |
| 3.2.0 | **1.25 ms** | 5.45 MB |

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
trade for 2.4× faster paints. `codegen-units = 1` in the release profile trims the binary
from 6.25 MB to 5.45 MB; the image decoders dominate the rest.

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

## Second pass (design review with Opus 5.5 and Gemini 4 Neon)

Both advisors reviewed the shipped screenshots and the diff. Fixed from their list:

- **DPI leaks:** footer rule and crease, `CB_SETITEMHEIGHT`, and the Images link field
  (device coordinates were being fed back through `rect()`).
- **Overflow:** Appearance tiles now fit 657 px exactly (156/10 and 102/9 strides).
- **Banner:** actions centred in the 34 px banner; compact hero keeps 12 px bottom padding.
- **Buttons page:** one rhythm for both slots (`header, 6, field, divider`), so the first
  slot's inputs no longer sit on the divider.
- **Focus:** 2 px frame along the inside edge instead of two filled rounded rects that
  erased the control's text when tabbing.
- **Apply:** primary only while an edit differs from the saved preferences
  (`edits_dirty()` compares text controls on `EN_CHANGE`), so the green button means
  something again.
- **Quit:** outlined, left-aligned, muted — it reads as a button instead of floating text.
- **Copy:** "every 30 minutes" everywhere, no triple version display on Updates, short
  status labels on the monitor pill, and the Activity example follows the chosen verb.
- **Card:** timer plus a channel chip instead of repeating the ship, concentric pill dot
  and halo, accent-tinted idle art well with a specular rim, shadow that ramps from dark
  (near the card) to light, section rules that fade out.
- **Images:** the link field follows the last thumbnail row, so short galleries have no
  dead gap.

Deliberate deviations from the advice:

- **Card toggles stay as individual tiles.** Grouping them into two surfaces would look
  tighter, but each tile is exactly the control's hit area; a grouped card would need the
  checkbox window to become borderless and to know its position in the group, for a gain
  that is smaller than the regression risk.
- **Mission badges keep the circular mask** in the picture grid, because that is how
  Discord draws a contract badge in the small-image slot; ships keep the rounded square.
- **No dirty-state "All changes saved" caption.** The Apply button changing weight is the
  signal; a second piece of text next to it adds noise to the footer.

## Verification

- `cargo build --release`, `cargo clippy --all-targets`, `cargo fmt --check`, `cargo test`
  (58 unit + 6 panel layout + 8 content + 4 replay tests) all clean.
- The panel tests pin the layout invariants: no page may extend past `FOOTER_TOP`
  at either card height, the ninth image column must stay inside `RIGHT`, every card
  toggle must have a description, and colour mixing/elapsed formatting stay exact.
  Writing them caught a real defect: the Images helper text was clipped when the
  update banner was showing.
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
