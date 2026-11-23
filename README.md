# Star Citizen Rich Presence

A small Windows companion that shows what you're doing in Star Citizen on your Discord
profile. It only reads the game's `Game.log`. It doesn't read game memory, take
screenshots, use OCR, or edit `user.cfg`.

## What your card shows

- **Status and play time:** starting up, main menu, loading, or in game. The timer keeps
  running through Discord reconnects and server changes.
- **Where you are:** named places in Stanton, Pyro, and Nyx, taken from events the game
  logs about you (inventory requests, ATC contact, quantum travel).
- **Your ship:** its proper name, including variants and special editions. While you're
  aboard, the big image becomes the ship's Star Citizen Wiki artwork (282 ships) and the
  location image moves to the small badge.
- **Your contract:** the title of your active contract, or a count if you have several.
  Its type (bounty, mercenary, hauling, delivery, mining, salvage, rescue, investigation,
  racing, XenoThreat, Wikelo, Arena Commander) puts a guild logo or wiki artwork in the
  small badge. If you have contracts of different types, the badge shows your location
  instead.
- **Your party:** "In a party" appears while you're in one.
- **Short notices:** blueprint acquired, refinery order done, hangar ready (up to 60
  seconds each), and when you're incapacitated.
- **Buttons:** your public RSI profile, plus up to two links of your own.

Every part can be turned off.

### How location works

The game doesn't log every movement, so this isn't live tracking. The card shows the last
location the game confirmed and hides it after 15 minutes by default. Selecting a quantum
target alone never changes it. Respawning clears your old ship and location but keeps the
timer.

## Quick start

1. From **Releases**, download either `star-citizen-rich-presence-setup-x64.exe` (installer:
   Start menu shortcuts, optional start with Windows, uninstall from Windows Settings, no
   admin rights needed) or the portable ZIP (extract it anywhere).
2. Open Discord (the desktop app).
3. Run Star Citizen Rich Presence.

The control panel opens and waits for Star Citizen. Your status clears when the game
closes. You normally don't need administrator rights.

In the control panel you can:

- pause or resume sharing,
- start the app hidden in the tray when you sign in to Windows (keep the exe in one
  folder; if you move it, turn this off and on again),
- turn location, ship, artwork, RSI profile, contracts, and notices on or off,
- replace the card's second and third lines with your own text (clear a field and click
  **Apply** to go back to automatic text),
- add up to two Discord buttons.

Closing or minimising the window sends it to the tray. Left-click the tray icon to reopen
it; right-click for pause and quit. Panel preferences are saved in
`%LOCALAPPDATA%\StarCitizenRichPresence\preferences.toml`.

### Game installed somewhere unusual?

The app looks for the running game first, then the usual install folders. If it can't find
yours, set `log_path` in `scrp.toml` (see below) or start it with `--game-dir`:

```powershell
.\star-citizen-rich-presence.exe --game-dir 'D:\Games\StarCitizen\LIVE'
```

> Coming from the old Go version? This version never touches `user.cfg`. If the old tool
> turned on `r_DisplayInfo`, remove that line yourself to hide the overlay.

## Settings

Copy `scrp.example.toml` to `scrp.toml` next to the exe, or pass `--config <path>`. Every
setting is optional. Command-line flags win over the file. Relative paths are resolved
from the settings file's folder. Put Windows paths in single quotes so backslashes stay
as they are.

| Setting | Default | What it does |
| --- | --- | --- |
| `channel` | `"LIVE"` | `LIVE`, `PTU`, `EPTU`, or `TECH-PREVIEW` |
| `discord_app_id` | original app | Your own Discord application ID (17–20 digits) |
| `activity_type` | `"playing"` | `playing`, `listening`, `watching`, or `competing` |
| `status_display` | `"name"` | What the member list shows after that word: `name`, `details`, or `state` |
| `log_path` | found automatically | Full path to `Game.log` |
| `locations_file` | none | JSON file with extra location names (see below) |
| `poll_seconds` | `2` | How often to check the log (1–60) |
| `update_seconds` | `15` | Shortest gap between Discord updates (15–300) |
| `location_ttl_seconds` | `900` | Hide a location this long after it was last confirmed (30–86400) |
| `show_location` | `true` | Show where you are |
| `show_ship` | `true` | Show your ship |
| `show_party` | `true` | Show "In a party" |
| `show_artwork` | `true` | Use wiki artwork for locations, ships, and contract types |
| `show_profile` | `true` | Add the RSI profile button |
| `show_contracts` | `true` | Show contract titles and badges |
| `show_notifications` | `true` | Show the short notices |
| `custom_details` | empty | Your own text for line 2 while in game |
| `custom_state` | empty | Your own text for line 3 while in game |
| `[[buttons]]` | none | Up to two buttons, each with a `label` (1–32 bytes) and an `https://` `url` |
| `[image_assets]` | none | Your own image per location |
| `[ship_images]` | none | Your own image per ship |
| `[mission_images]` | none | Your own image per contract type or title |

### Choosing images

The app comes with Star Citizen Wiki images for 39 locations, 282 ships, and 12 contract
types. Most ships and types have several images to choose from.

To browse them, open `artwork-gallery.html` (next to the exe; `assets/` in the source)
in your browser, search for a ship or
contract type, and click a picture. That copies a line such as `"Aegis Gladius" = "3"`.
Paste it into `scrp.toml`:

```toml
[ship_images]
"Aegis Gladius" = "3"                                  # 3rd wiki image
"RSI Perseus" = "https://example.com/my-perseus.png"   # your own image

[mission_images]
bounty = "2"
"Tactical Strike" = "https://example.com/strike.png"   # words from a contract title

[image_assets]
"Levski" = "https://example.com/my-levski.png"
```

- **Ship keys** are the ship name exactly as the card shows it (capitals don't matter).
- **Contract keys** can be a type (`bounty`, `mercenary`, `hauling`, `delivery`, `mining`,
  `salvage`, `rescue`, `investigation`, `racing`, `xenothreat`, `wikelo`, `arena`), its
  label such as `Bounty hunting`, or any words from a contract title. When several keys
  match, the longest title match wins, and title matches beat types.
- **Values** can be a number (pick that wiki image; `1` is the default), a public
  `https://` image link, or the name of an image uploaded to your own Discord application.

Discord can only show images from the internet or from a Discord application, so files on
your PC won't work. Upload them somewhere public first.

### "Listening to Scam Citizen"

`activity_type` changes the word Discord shows: Playing, Listening to, Watching, or
Competing in. The name after it comes from the Discord application, not from this app. To
change the name:

1. Create an application at the [Discord Developer Portal](https://discord.com/developers)
   and give it the name you want, for example *Scam Citizen*.
2. Put its application ID in `discord_app_id`, or pass `--app-id`.
3. Set `activity_type = "listening"`.

If you don't want to make an application, set `status_display = "details"` and write your
text in `custom_details` (line 2 in the control panel). The member list then shows that
text after the word instead of the game name.

### Adding locations

Location names are built into the app. To add one or fix one without rebuilding, point
`locations_file` at a JSON file like this:

```json
[
  { "name": "My outpost", "system": "Pyro", "aliases": ["EXACT_LOG_LOCATION_ID"] }
]
```

Your entries replace built-in ones with the same alias. Matching ignores capitals and
picks the most specific alias. Unknown sub-locations can fall back to a known parent area.

## Command line

Run the exe with no options (or `--tray`) for the control panel. Any other option runs it
in the console. In PowerShell, use `Start-Process -Wait -NoNewWindow` to see its output.

```powershell
.\star-citizen-rich-presence.exe --dry-run              # print updates, don't send them to Discord
.\star-citizen-rich-presence.exe --channel EPTU         # LIVE, PTU, EPTU, TECH-PREVIEW
.\star-citizen-rich-presence.exe --discover             # list the game processes and logs it finds
.\star-citizen-rich-presence.exe --check-discord        # send a test status to Discord, then clear it
.\star-citizen-rich-presence.exe --replay 'D:\Games\StarCitizen\LIVE\Game.log'   # print the card for a saved log as JSON
.\star-citizen-rich-presence.exe --no-profile --no-contracts --no-notifications
.\star-citizen-rich-presence.exe --log 'D:\Games\StarCitizen\LIVE\Game.log'      # or set SCRP_LOG
.\star-citizen-rich-presence.exe --config '.\custom.toml' --app-id 123456789012345678
```

`--replay` prints `"presence": null` for a log that ends with the game closing.

## Privacy

- **Read-only.** No memory reading, code injection, screenshots, or config changes.
- **Nothing leaves your PC except the card.** The app talks only to Discord on your own
  computer. Game data and image links are built in, so it doesn't download anything while
  you play. Discord itself loads the card images from the Star Citizen Wiki.
- **Your logs stay with you.** The card can reveal your location, ship, contract, party
  status, and RSI profile. Each can be turned off.
- **Only your own events count.** Locations are matched to your logged-in handle, so other
  players' events can't move you.
- **Safe with big or changing logs.** Handles log rotation, truncation, half-written lines,
  and missing files, with limits on memory use.
- **Gentle on Discord.** Updates are deduplicated and sent at most every 15 seconds, with
  retries and reconnects when Discord restarts.
- **No guessing.** If a patch stops logging something, that part of the card falls back to
  a general status. A ship or contract can stay on the card until the game logs its end.

---

## For developers

### Built-in game data

The data is pinned to the **4.10.2-LIVE.12881860** snapshot from the
[Star Citizen Wiki API](https://api.star-citizen.wiki/api/locations). Sources and counts
are in `data/content-source.json`.

| Content | Records |
| --- | ---: |
| Stanton locations | 923 |
| Pyro locations | 582 |
| Nyx locations | 448 |
| All named locations | 1,953 |
| Vehicle classes and variants | 310 |

This includes outposts, stations, distribution centres, caves, contested zones, and other
starmap points, plus current Mk, Alliance, Wikelo, and newer vehicle variants. Contracts
are recognised by name, so no mission list is needed.

Records use game UUIDs, unambiguous names, and aliases scoped to their parent. Shared
tags and duplicate names can't pick the wrong place. Log-only identifiers that have been
checked live in `data/location-overrides.json`.

Extracted game data can include hidden or future content, so a recognised name doesn't
mean the place is currently reachable. New patches may need new aliases.

### Refreshing data and artwork

You don't need either script to run the app.

```text
node scripts/refresh-content.mjs --version 4.10.2-LIVE.12881860
node scripts/refresh-artwork.mjs        # or: bun scripts/refresh-artwork.mjs
cargo test --locked
```

- `refresh-content.mjs` pins every API page to one game version, checks the download is
  complete, rejects clashing aliases, and leaves out descriptions, images, and player
  data. `--input-dir` rebuilds from saved API responses. After a patch update, check the
  version assertion in `tests/content.rs`.
- `refresh-artwork.mjs` collects every usable Star Citizen Wiki image for each ship and
  contract type (up to 16 each; the page's main image comes first). It writes
  `data/ship-artwork.json` and `data/mission-artwork.json` and rebuilds
  `assets/artwork-gallery.html` from `scripts/artwork-gallery.template.html`. Only image
  links are bundled; Discord can't display local files.

### Build and test

You need Rust 1.95+ and a Windows linker. The easiest setup is the MSVC toolchain with
Visual Studio Build Tools (*Desktop development with C++*). The GNU toolchain also works
with a full MinGW-w64 install (for example WinLibs), which provides `windres` and
`dlltool`.

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
```

The exe ends up at `target/release/star-citizen-rich-presence.exe`.

Parser, session, and IPC tests also run on Linux. Finding the game automatically only
works on Windows. On Linux or Wine, pass `--log` or `--game-dir`. Live process detection
there hasn't been tested.

GitHub Actions checks formatting, Clippy, tests, and the build. Pushing a `v*` tag
publishes a Windows ZIP with a SHA-256 checksum. A manual run produces downloadable
builds.

### Validation

The parser was checked against local **4.10.197.2878** logs and recent 4.10 backups. A
**4.10.2 LIVE** desktop test confirmed log discovery, the main-menu card, Lorville, the
Discord profile card, and clean-up on exit. `tests/fixtures/session-4.10.log` is a
made-up, anonymised session built from real 4.10 event shapes. Details and limits are in
[VALIDATION.md](VALIDATION.md).

### Code map

| Module | Does |
| --- | --- |
| `artwork` | Ship and contract artwork, contract types, image overrides |
| `config` | Command-line flags, `scrp.toml`, validation |
| `desktop` | Control panel, tray, start-with-Windows, saved preferences |
| `discord` | Local Discord connection and activity messages |
| `discovery` | Finding the running game and its `Game.log` |
| `locations` | Built-in and custom location aliases |
| `parser` | Turning your own log lines into events; removing in-game text markup |
| `presence` | Building the card: privacy settings, images, rate limits, retries |
| `session` | Who you are, game phase, ship, contracts, party, notices, location |
| `tail` | Reading the log safely as it grows and rotates |
| `vehicles` | Vehicle class names to display names |

## References

- [VALIDATION.md](VALIDATION.md): what was tested and the limits
- [RSI: game log files](https://support.robertsspaceindustries.com/hc/en-us/articles/360000065688-Send-In-Game-Files-for-RSI-Support)
- [Discord: local RPC and SET_ACTIVITY](https://docs.discord.com/developers/topics/rpc)
- [SC Bridge: documented log event patterns](https://github.com/SC-Bridge/sc-companion/blob/main/docs/parser-patterns.csv)
- [Star Citizen Wiki: location API](https://skmtc.dev/starcitizenwiki/apis/star-citizen-wiki-api/docs/api/locations/get)
- [Star Citizen Wiki: vehicle API](https://skmtc.dev/starcitizenwiki/apis/star-citizen-wiki-api/docs/api/vehicles/get)
- [RSI: March 2026 monthly report (Stanton, Pyro, Nyx)](https://robertsspaceindustries.com/en/comm-link/transmission/21094-Star-Citizen-Monthly-Report-March-2026)

MIT licensed, keeping the original project's licence and credit. Unofficial fan project,
not affiliated with Cloud Imperium Games or Discord. Artwork belongs to its owners and is
linked from the Star Citizen Wiki.
