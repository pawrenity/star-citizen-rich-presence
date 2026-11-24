## 3.2.0: redesigned control panel, crisp on every display, twice as fast to draw

- **New control panel design.** The card preview is now a proper Discord "playing" card: ship art with a
  mission badge, the verb as a small caps overline, the game title, your location, ship and contract, and
  the play timer in a mono font. The rail has icons, a hover state and an accent-tinted selection, page
  titles are a real title size with one-line explanations, and settings sit in grouped cards with hairline
  dividers instead of a grid of identical boxes.
- **Sharp at any display scaling.** The panel is now per-monitor DPI aware, so 125 %, 150 %, 175 % and
  200 % displays get crisp text and correctly sized controls instead of a stretched bitmap. Moving the
  window to another monitor re-lays it out on the fly.
- **Twice as fast to paint.** Rounded rectangles and circles are rasterised once and reused, and the panel
  draws into a persistent back buffer instead of allocating a full-window bitmap per frame: a full repaint
  dropped from **3.01 ms to 1.27 ms** (300 repaints timed with `scripts/screenshot.ps1 -BenchPaints`).
- **Artwork is cached on disk.** Ship, location and contract pictures are stored under
  `%LOCALAPPDATA%\StarCitizenRichPresence\thumbnails`, so the panel is instant on later launches instead of
  re-downloading up to 27 images. Downloads are capped at three at a time and the cache is pruned to
  512 files / 128 MB.
- **New pages worth visiting.** Activity shows a live member-list preview of the line Discord will display;
  Buttons shows a Discord-shaped preview of your two buttons with per-slot validation and the rules beside
  it; Updates shows the install channel, how releases are verified, and a live presence monitor; Appearance
  has a control sample strip so theme, accent and corner changes are visible without leaving the page.
- **Smaller fixes.** The Images grid now fits nine square previews per row, selected pictures use an accent
  ring plus a check badge instead of a solid colour slab, pressed picture tiles no longer flash a negative
  image, and keyboard focus draws a rounded accent ring instead of the old dotted rectangle.
- **Accessibility.** Secondary text now clears WCAG AA at 11–13 px on every theme, disabled states are
  distinct from enabled ones, and the status colours (live / paused / error) no longer borrow your accent.

Download the installer or the Windows ZIP under **Assets**, or click **Update now** inside the app.
