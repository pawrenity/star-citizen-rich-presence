//! Windows control panel ("Sidebar" design): a navigation rail, a pinned live card
//! preview with real artwork, one settings page at a time, and the in-app updater.
use super::*;
use serde::{Deserialize, Serialize};
use star_citizen_rich_presence::{
    artwork,
    config::CardButton,
    net,
    thumbnail::{self, Mask, Thumbnail},
    update::{self, Release},
};
use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    ptr::null_mut,
    sync::{
        Mutex,
        atomic::{AtomicI32, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::*, Registry::*, Threading::*},
    UI::{
        Controls::{Dialogs::*, *},
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
            SetProcessDpiAwarenessContext,
        },
        Input::KeyboardAndMouse::{
            EnableWindow, IsWindowEnabled, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
        },
        Shell::*,
        WindowsAndMessaging::*,
    },
};

type Pcwstr = *const u16;

thread_local! {
    /// Rasterised rounded rectangles and circles, reused across repaints.
    static SHAPES: std::cell::RefCell<star_citizen_rich_presence::shapes::ShapeCache> =
        std::cell::RefCell::new(star_citizen_rich_presence::shapes::ShapeCache::new(192));
}

const TRAY: u32 = WM_APP + 1;
/// Posted by background threads (artwork, update checks) to request a repaint.
const REFRESH: u32 = WM_APP + 2;
const APP_ICON: Pcwstr = 101usize as Pcwstr;

// ── Controls ────────────────────────────────────────────────────────────────
const ENABLED: i32 = 101;
const STARTUP: i32 = 102;
const LOCATION: i32 = 103;
const SHIP: i32 = 104;
const ART: i32 = 105;
const PROFILE: i32 = 106;
const CONTRACTS: i32 = 107;
const CUSTOM: i32 = 108;
const SAVE: i32 = 109;
const HIDE: i32 = 110;
const EXIT: i32 = 111;
const NOTICES: i32 = 112;
const PARTY: i32 = 113;
const UPDATES: i32 = 114;
const DETAILS: i32 = 119;
const GAME_NAME: i32 = 120;
const LINK_FIELDS: [(i32, i32); 2] = [(121, 122), (123, 124)];
const VERB: i32 = 130;
const MEMBER_LIST: i32 = 131;
const IMAGE_TARGET: i32 = 132;
const IMAGE_LINK: i32 = 133;
const USE_LINK: i32 = 134;
const USE_DEFAULT: i32 = 135;
const UPLOAD_IMAGE: i32 = 136;
const IMG_PREV: i32 = 137;
const IMG_NEXT: i32 = 138;
const UPDATE_NOW: i32 = 140;
const RELEASE_NOTES: i32 = 141;
const UPDATE_ACTION: i32 = 142;
const UPDATE_NOTES: i32 = 143;
const NAV: i32 = 300;
/// Picture tiles on the Images page (up to 27: three rows of nine 1:1 icon squares).
const CHOICE: i32 = 400;
const MAX_CHOICES: usize = 27;
const THEME_BTN: i32 = 500;
const ACCENT_BTN: i32 = 510;
const CORNER_BTN: i32 = 520;

const PAGES: [&str; 6] = [
    "Card",
    "Activity",
    "Images",
    "Buttons",
    "Appearance",
    "Updates",
];
/// Segoe MDL2 Assets glyphs for the rail: card, message, picture, link, colour, sync.
const NAV_GLYPHS: [&str; 6] = [
    "\u{E8A1}", "\u{E8BD}", "\u{E8B9}", "\u{E71B}", "\u{E790}", "\u{E895}",
];
const CARD: usize = 0;
const ACTIVITY: usize = 1;
const IMAGES: usize = 2;
const BUTTONS: usize = 3;
const APPEARANCE: usize = 4;
const UPDATES_PAGE: usize = 5;

// ── Palette & Themes ────────────────────────────────────────────────────────
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// Primary text: 10.2:1 or better on every surface, in every theme.
const TEXT: u32 = rgb(0xf2, 0xf2, 0xf4);
/// Secondary text: 7.4:1 or better.
const SOFT: u32 = rgb(0xc8, 0xc8, 0xce);
/// Tertiary text and captions: 5:1 or better (was `FAINT`, which failed AA).
const MUTED: u32 = rgb(0xa4, 0xa4, 0xae);
/// Disabled text. Never use it for anything actionable.
const DISABLED: u32 = rgb(0x75, 0x75, 0x7e);
/// Ink on an accent fill.
const ON_ACCENT: u32 = rgb(0x0d, 0x0d, 0x0f);
const BANNER_TEXT: u32 = rgb(0xe8, 0xf3, 0xfb);

/// Fixed semantic status colours. Never the user's accent.
const LIVE: u32 = rgb(0x57, 0xd9, 0x87);
const PAUSED: u32 = rgb(0xf5, 0xc5, 0x6b);
const ERROR: u32 = rgb(0xff, 0x8a, 0x80);

/// Linear blend of two `0x00BBGGRR` colours; `weight` 0.0 is `from`, 1.0 is `to`.
fn mix(from: u32, to: u32, weight: f32) -> u32 {
    let w = weight.clamp(0.0, 1.0);
    let channel = |shift: u32| {
        let a = ((from >> shift) & 0xff) as f32;
        let b = ((to >> shift) & 0xff) as f32;
        (a + (b - a) * w).round() as u32
    };
    channel(0) | channel(8) << 8 | channel(16) << 16
}

/// Lifts a colour towards white: used for hover states and top highlights.
fn lighten(color: u32, amount: f32) -> u32 {
    mix(color, 0xff_ff_ff, amount)
}

#[derive(Clone, Copy)]
struct Theme {
    caption: u32,
    canvas: u32,
    rail: u32,
    footer: u32,
    rail_line: u32,
    divider: u32,
    nav_selected: u32,
    nav_hover: u32,
    /// Hero card surface, one step above `tile`.
    card: u32,
    card_border: u32,
    tile: u32,
    tile_hover: u32,
    tile_pressed: u32,
    tile_border: u32,
    field: u32,
    field_border: u32,
    field_bottom: u32,
    chip: u32,
    accent: u32,
    accent_hover: u32,
    accent_pressed: u32,
    banner: u32,
    banner_border: u32,
    tile_radius: i32,
    button_radius: i32,
}

/// Swatch previews for the Appearance page: (key, label, canvas, rail, tile).
const THEMES: [(&str, &str, u32, u32, u32); 4] = [
    (
        "obsidian",
        "Obsidian",
        rgb(0x20, 0x20, 0x20),
        rgb(0x1b, 0x1b, 0x1b),
        rgb(0x2b, 0x2b, 0x2b),
    ),
    (
        "midnight",
        "Midnight",
        rgb(0x14, 0x19, 0x23),
        rgb(0x0e, 0x13, 0x1b),
        rgb(0x1d, 0x24, 0x33),
    ),
    (
        "pitch",
        "Pitch",
        rgb(0x0c, 0x0c, 0x0e),
        rgb(0x07, 0x07, 0x09),
        rgb(0x17, 0x17, 0x1b),
    ),
    (
        "slate",
        "Slate",
        rgb(0x26, 0x27, 0x2c),
        rgb(0x1e, 0x1f, 0x24),
        rgb(0x31, 0x32, 0x39),
    ),
];

/// (key, label, accent, hover, pressed).
const ACCENTS: [(&str, &str, u32, u32, u32); 6] = [
    (
        "sky",
        "Sky",
        rgb(0x4c, 0xc2, 0xff),
        rgb(0x70, 0xd2, 0xff),
        rgb(0x32, 0xa6, 0xe3),
    ),
    (
        "amber",
        "Amber",
        rgb(0xf2, 0xb3, 0x3d),
        rgb(0xf6, 0xbf, 0x54),
        rgb(0xd4, 0x96, 0x24),
    ),
    (
        "emerald",
        "Emerald",
        rgb(0x43, 0xd0, 0x7e),
        rgb(0x6c, 0xe3, 0x96),
        rgb(0x2e, 0xb2, 0x66),
    ),
    (
        "coral",
        "Coral",
        rgb(0xf7, 0x6e, 0x6e),
        rgb(0xff, 0x7a, 0x7a),
        rgb(0xd9, 0x52, 0x52),
    ),
    (
        "violet",
        "Violet",
        rgb(0xa9, 0x7e, 0xff),
        rgb(0xbc, 0x95, 0xff),
        rgb(0x8e, 0x5f, 0xe8),
    ),
    (
        "ice",
        "Ice",
        rgb(0xd6, 0xe2, 0xee),
        rgb(0xdc, 0xe7, 0xf2),
        rgb(0xb4, 0xc6, 0xd8),
    ),
];

impl Theme {
    fn from_prefs(prefs: &Preferences) -> Self {
        let (accent, accent_hover, accent_pressed) =
            ACCENTS.iter().find(|(k, ..)| *k == prefs.accent).map_or(
                (ACCENTS[0].2, ACCENTS[0].3, ACCENTS[0].4),
                |(_, _, a, h, p)| (*a, *h, *p),
            );
        let (tile_radius, button_radius) = if prefs.sharp_corners { (2, 2) } else { (6, 5) };
        // (caption, canvas, rail, tile, tile_hover, tile_pressed, stroke, hair, field, field_border, ctl)
        let palette = match prefs.theme.as_str() {
            "midnight" => [
                rgb(0x0e, 0x13, 0x1b),
                rgb(0x14, 0x19, 0x23),
                rgb(0x0e, 0x13, 0x1b),
                rgb(0x1d, 0x24, 0x33),
                rgb(0x25, 0x2d, 0x3e),
                rgb(0x2d, 0x36, 0x48),
                rgb(0x31, 0x3c, 0x52),
                rgb(0x29, 0x32, 0x4a),
                rgb(0x0f, 0x14, 0x1c),
                rgb(0x2d, 0x3d, 0x59),
                rgb(0x7f, 0x8d, 0xa6),
            ],
            "pitch" => [
                rgb(0x06, 0x06, 0x08),
                rgb(0x0c, 0x0c, 0x0e),
                rgb(0x07, 0x07, 0x09),
                rgb(0x17, 0x17, 0x1b),
                rgb(0x1f, 0x1f, 0x24),
                rgb(0x27, 0x27, 0x2d),
                rgb(0x2b, 0x2b, 0x32),
                rgb(0x23, 0x23, 0x29),
                rgb(0x08, 0x08, 0x0a),
                rgb(0x2c, 0x2c, 0x35),
                rgb(0x72, 0x72, 0x7e),
            ],
            "slate" => [
                rgb(0x1c, 0x1d, 0x22),
                rgb(0x26, 0x27, 0x2c),
                rgb(0x1e, 0x1f, 0x24),
                rgb(0x31, 0x32, 0x39),
                rgb(0x39, 0x3b, 0x43),
                rgb(0x42, 0x44, 0x4d),
                rgb(0x47, 0x4a, 0x54),
                rgb(0x3e, 0x40, 0x48),
                rgb(0x1f, 0x20, 0x25),
                rgb(0x46, 0x49, 0x54),
                rgb(0x90, 0x94, 0xa0),
            ],
            _ => [
                rgb(0x19, 0x19, 0x19),
                rgb(0x20, 0x20, 0x20),
                rgb(0x1b, 0x1b, 0x1b),
                rgb(0x2b, 0x2b, 0x2b),
                rgb(0x33, 0x33, 0x33),
                rgb(0x3b, 0x3b, 0x3b),
                rgb(0x3d, 0x3d, 0x3d),
                rgb(0x36, 0x36, 0x36),
                rgb(0x1c, 0x1c, 0x1c),
                rgb(0x3d, 0x3d, 0x3d),
                rgb(0x8b, 0x8b, 0x8b),
            ],
        };
        let [
            caption,
            canvas,
            rail,
            tile,
            tile_hover,
            tile_pressed,
            tile_border,
            divider,
            field,
            field_border,
            field_bottom,
        ] = palette;
        Self {
            caption,
            canvas,
            rail,
            footer: rail,
            rail_line: mix(rail, 0xff_ff_ff, 0.09),
            divider,
            nav_selected: mix(rail, accent, 0.16),
            nav_hover: mix(rail, tile, 0.55),
            card: mix(tile, 0xff_ff_ff, 0.045),
            card_border: mix(tile_border, 0xff_ff_ff, 0.10),
            tile,
            tile_hover,
            tile_pressed,
            tile_border,
            field,
            field_border,
            field_bottom,
            chip: mix(tile, 0xff_ff_ff, 0.03),
            accent,
            accent_hover,
            accent_pressed,
            banner: mix(canvas, accent, 0.13),
            banner_border: mix(canvas, accent, 0.34),
            tile_radius,
            button_radius,
        }
    }

    /// 1 px top highlight for raised surfaces; brighter on darker themes.
    fn status_fill(&self, tone: u32) -> u32 {
        mix(self.tile, tone, 0.14)
    }

    fn status_border(&self, tone: u32) -> u32 {
        mix(self.tile, tone, 0.34)
    }
}

// ── Geometry (designed at 900 × 588 logical pixels, scaled for DPI) ────────
const CLIENT_WIDTH: i32 = 900;
const CLIENT_HEIGHT: i32 = 588;
const RAIL_WIDTH: i32 = 206;
const LEFT: i32 = 225;
const RIGHT: i32 = 882;
const FOOTER_TOP: i32 = 540;
const COLUMN: i32 = 324;
const COLUMN_GAP: i32 = 9;

/// Display scale as a percentage; 100 is 96 DPI. Set before the window is
/// created and refreshed on `WM_DPICHANGED`.
static SCALE: AtomicI32 = AtomicI32::new(100);

/// Logical → device pixels, rounding away from zero.
fn s(value: i32) -> i32 {
    let scale = SCALE.load(Ordering::Relaxed);
    if scale == 100 || value == 0 {
        value
    } else {
        (value * scale + value.signum() * 50) / 100
    }
}

fn set_scale(percent: i32) {
    SCALE.store(percent.clamp(75, 500), Ordering::Relaxed);
}

/// Reads a window's DPI as a scale percentage (120 = 125 %, 144 = 150 %).
unsafe fn window_scale(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        100
    } else {
        (dpi as i32 * 100 + 48) / 96
    }
}

/// Largest scale whose panel still fits on the primary screen (title bar included).
fn largest_fitting_scale() -> i32 {
    let screen = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if screen <= 0 {
        return 500;
    }
    (((screen - s(56)) * 100) / CLIENT_HEIGHT).clamp(100, 500)
}

/// Window DPI, or `SCRP_UI_SCALE` when set (used to screenshot other scalings),
/// never larger than the screen can show.
unsafe fn preferred_scale(hwnd: HWND) -> i32 {
    let wanted = std::env::var("SCRP_UI_SCALE")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or_else(|| unsafe { window_scale(hwnd) });
    wanted.min(largest_fitting_scale())
}

const RUN_KEY: Pcwstr = windows_sys::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: Pcwstr = windows_sys::w!("StarCitizenRichPresence");

const VERBS: [(&str, &str); 4] = [
    ("playing", "Playing"),
    ("listening", "Listening to"),
    ("watching", "Watching"),
    ("competing", "Competing in"),
];
const MEMBER_LIST_OPTIONS: [(&str, &str); 3] = [
    ("name", "Game name (Star Citizen)"),
    ("details", "Line 2 — Location / custom"),
    ("state", "Line 3 — Ship & contract"),
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    enabled: bool,
    location: bool,
    ship: bool,
    artwork: bool,
    profile: bool,
    contracts: bool,
    notices: bool,
    party: bool,
    updates: bool,
    custom: String,
    details: String,
    game_name: String,
    buttons: Option<Vec<CardButton>>,
    /// `None` keeps the value from scrp.toml.
    activity_type: Option<String>,
    status_display: Option<String>,
    /// Added on top of scrp.toml's tables; these win on the same key.
    ship_images: BTreeMap<String, String>,
    mission_images: BTreeMap<String, String>,
    /// UI appearance customisation.
    theme: String,
    accent: String,
    sharp_corners: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

impl Preferences {
    fn from_config(config: &Config) -> Self {
        Self {
            enabled: true,
            location: config.show_location,
            ship: config.show_ship,
            artwork: config.show_artwork,
            profile: config.show_profile,
            contracts: config.show_contracts,
            notices: config.show_notifications,
            party: config.show_party,
            updates: config.check_for_updates,
            custom: config.custom_state.clone(),
            details: config.custom_details.clone(),
            game_name: config.custom_name.clone(),
            buttons: None,
            activity_type: None,
            status_display: None,
            ship_images: BTreeMap::new(),
            mission_images: BTreeMap::new(),
            theme: "obsidian".into(),
            accent: "sky".into(),
            sharp_corners: false,
        }
    }

    fn apply(&self, config: &mut Config) {
        config.show_location = self.location;
        config.show_ship = self.ship;
        config.show_artwork = self.artwork;
        config.show_profile = self.profile;
        config.show_contracts = self.contracts;
        config.show_notifications = self.notices;
        config.show_party = self.party;
        config.custom_state.clone_from(&self.custom);
        config.custom_details.clone_from(&self.details);
        config.custom_name.clone_from(&self.game_name);
        if let Some(buttons) = &self.buttons {
            config.buttons.clone_from(buttons);
        }
        if let Some(verb) = &self.activity_type {
            config.activity_type.clone_from(verb);
        }
        if let Some(display) = &self.status_display {
            config.status_display.clone_from(display);
        }
        config.ship_images.extend(self.ship_images.clone());
        config.mission_images.extend(self.mission_images.clone());
    }

    /// Saved image overrides as (name, value), ships first.
    fn overrides(&self) -> Vec<(String, String)> {
        self.ship_images
            .iter()
            .chain(&self.mission_images)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[derive(Clone, PartialEq)]
enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    Failed(String),
}

#[derive(Clone, Copy)]
struct Fonts {
    brand: HFONT,
    page_title: HFONT,
    body: HFONT,
    body_semi: HFONT,
    small: HFONT,
    small_semi: HFONT,
    tiny: HFONT,
    tiny_semi: HFONT,
    details: HFONT,
    kicker: HFONT,
    glyph: HFONT,
    glyph_small: HFONT,
    mono: HFONT,
}

#[derive(Clone, PartialEq, Eq)]
enum UploadState {
    Idle,
    Uploading,
    Done {
        mission: bool,
        key: String,
        url: String,
    },
    Failed(String),
}

struct Desktop {
    prefs: Arc<Mutex<Preferences>>,
    status: Arc<Mutex<String>>,
    preview: Arc<Mutex<Option<Snapshot>>>,
    update: Arc<Mutex<UpdateState>>,
    upload: Arc<Mutex<UploadState>>,
    /// Decoded artwork by URL; `None` while loading or after a failure.
    thumbnails: Arc<Mutex<HashMap<String, Option<Thumbnail>>>>,
    requested: std::cell::RefCell<HashSet<String>>,
    /// Settings from scrp.toml, for the effective verb and update policy.
    base: Config,
    file: PathBuf,
    icon: HICON,
    taskbar_created: u32,
    tray_ready: Cell<bool>,
    page: Cell<usize>,
    banner: Cell<bool>,
    /// What the window last painted, to skip needless repaints.
    painted: std::cell::RefCell<String>,
    fonts: Cell<Option<Fonts>>,
    theme: Cell<Theme>,
    field: Cell<HBRUSH>,
    /// Reused off-screen surface; a repaint draws here once and blits once.
    buffer: Cell<BackBuffer>,
    /// Control currently under the pointer, for hover styling.
    hot: Cell<HWND>,
    /// True when an edit field differs from what is saved, so Apply can stand out.
    dirty: Cell<bool>,
    /// Everything that can have a custom picture: contract types, then ships.
    targets: Vec<Target>,
    /// Index into `targets` shown on the Images page.
    target: Cell<usize>,
    /// 0-based page index into `choices()` on the Images page (27 tiles per page).
    image_page: Cell<usize>,
    /// Last release version we already showed a Windows tray notification for.
    notified_update: std::cell::RefCell<Option<String>>,
}

#[derive(Clone, Copy)]
struct BackBuffer {
    dc: HDC,
    bitmap: HBITMAP,
    width: i32,
    height: i32,
}

impl BackBuffer {
    const EMPTY: Self = Self {
        dc: null_mut(),
        bitmap: null_mut(),
        width: 0,
        height: 0,
    };
}

/// A ship or contract type whose picture can be customised.
struct Target {
    mission: bool,
    key: String,
    label: String,
}

fn targets() -> Vec<Target> {
    let missions = artwork::CATEGORIES.iter().map(|(key, label, _)| Target {
        mission: true,
        key: key.to_string(),
        label: format!("Contract: {label}"),
    });
    let ships = artwork::ship_names().into_iter().map(|name| Target {
        mission: false,
        key: name.to_string(),
        label: name.to_string(),
    });
    missions.chain(ships).collect()
}

fn find_ci<'a>(map: &'a BTreeMap<String, String>, key: &str) -> Option<&'a String> {
    map.iter()
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

impl Desktop {
    fn current_target(&self) -> &Target {
        &self.targets[self.target.get().min(self.targets.len() - 1)]
    }

    /// All wiki pictures for the selected ship or contract type.
    fn choices(&self) -> &'static [String] {
        let target = self.current_target();
        if target.mission {
            artwork::mission_choices(&target.key)
        } else {
            artwork::ship_choices(&target.key)
        }
    }

    fn image_page_count(&self) -> usize {
        self.choices().len().div_ceil(MAX_CHOICES).max(1)
    }

    fn current_image_page(&self) -> usize {
        self.image_page.get().min(self.image_page_count() - 1)
    }

    /// `(offset, slice)` of up to `MAX_CHOICES` pictures for the current page.
    fn visible_choices(&self) -> (usize, &'static [String]) {
        let all = self.choices();
        let offset = self.current_image_page() * MAX_CHOICES;
        let end = (offset + MAX_CHOICES).min(all.len());
        (offset, &all[offset..end])
    }

    /// The saved picture for the selected target: the panel's choice, else scrp.toml's.
    fn saved_picture(&self) -> Option<String> {
        let target = self.current_target();
        let prefs = self.prefs.lock().unwrap();
        let (mine, file) = if target.mission {
            (&prefs.mission_images, &self.base.mission_images)
        } else {
            (&prefs.ship_images, &self.base.ship_images)
        };
        find_ci(mine, &target.key)
            .or_else(|| find_ci(file, &target.key))
            .cloned()
    }

    /// Which wiki picture is in use (0-based), or `None` for a custom link.
    fn selected_choice(&self) -> Option<usize> {
        match self.saved_picture() {
            None => Some(0),
            Some(value) => match value.trim().trim_start_matches('#').parse::<usize>() {
                Ok(n) if n >= 1 && n <= self.choices().len() => Some(n - 1),
                Ok(_) => Some(0),
                Err(_) => None,
            },
        }
    }

    fn fonts(&self) -> Fonts {
        self.fonts.get().expect("fonts are created in WM_CREATE")
    }

    /// The off-screen surface for this window, recreated when the pixel size changes.
    unsafe fn back_buffer(&self, target: HDC, width: i32, height: i32) -> HDC {
        unsafe {
            let current = self.buffer.get();
            if current.width == width && current.height == height && !current.dc.is_null() {
                return current.dc;
            }
            if !current.dc.is_null() {
                DeleteObject(current.bitmap);
                DeleteDC(current.dc);
            }
            let dc = CreateCompatibleDC(target);
            let bitmap = CreateCompatibleBitmap(target, width, height);
            if dc.is_null() || bitmap.is_null() {
                self.buffer.set(BackBuffer::EMPTY);
                return target;
            }
            SelectObject(dc, bitmap);
            self.buffer.set(BackBuffer {
                dc,
                bitmap,
                width,
                height,
            });
            dc
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

pub(super) fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(message).as_ptr(),
            windows_sys::w!("Star Citizen Rich Presence"),
            MB_ICONERROR,
        );
    }
}

fn startup_enabled() -> bool {
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_NAME,
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut 0,
        ) == ERROR_SUCCESS
    }
}

fn set_startup(enabled: bool) -> Result<()> {
    let result = if enabled {
        let exe = std::env::current_exe()?;
        use std::os::windows::ffi::OsStrExt;
        let command: Vec<u16> = [b'"' as u16]
            .into_iter()
            .chain(exe.as_os_str().encode_wide())
            .chain(wide("\" --tray"))
            .collect();
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_NAME,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME) }
    };
    if result != ERROR_SUCCESS && (enabled || result != ERROR_FILE_NOT_FOUND) {
        return Err(std::io::Error::from_raw_os_error(result as i32))
            .context("Could not update Windows startup");
    }
    Ok(())
}

fn save(file: &PathBuf, prefs: &Preferences) -> Result<()> {
    let tmp = file.with_extension("tmp");
    std::fs::create_dir_all(file.parent().context("Missing settings directory")?)?;
    std::fs::write(&tmp, toml::to_string_pretty(prefs)?)?;
    std::fs::rename(tmp, file)?;
    Ok(())
}

fn open(target: &str) {
    unsafe {
        ShellExecuteW(
            null_mut(),
            windows_sys::w!("open"),
            wide(target).as_ptr(),
            null_mut(),
            null_mut(),
            SW_SHOWNORMAL,
        );
    }
}

fn request_refresh(hwnd: isize) {
    unsafe {
        PostMessageW(hwnd as HWND, REFRESH, 0, 0);
    }
}

fn check_for_update(state: Arc<Mutex<UpdateState>>, hwnd: isize) {
    {
        let mut current = state.lock().unwrap();
        if matches!(
            *current,
            UpdateState::Checking | UpdateState::Downloading(_)
        ) {
            return;
        }
        *current = UpdateState::Checking;
    }
    request_refresh(hwnd);
    std::thread::spawn(move || {
        let result = match update::check() {
            Ok(Some(release)) => UpdateState::Available(release),
            Ok(None) => UpdateState::UpToDate,
            Err(error) => UpdateState::Failed(format!("{error:#}")),
        };
        *state.lock().unwrap() = result;
        request_refresh(hwnd);
    });
}

pub(super) fn run(start_hidden: bool) -> Result<()> {
    unsafe {
        // Per-monitor DPI awareness: sharp text and correct layout at any scaling.
        // Must happen before the first window exists; a manifest may already set it.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(null_mut(), 0, windows_sys::w!("Local\\SCRP.Desktop"));
        if mutex.is_null() {
            return Err(std::io::Error::last_os_error()).context("Cannot start desktop app");
        }
        let already_open = GetLastError() == ERROR_ALREADY_EXISTS;
        struct Handle(HANDLE);
        impl Drop for Handle {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let _mutex = Handle(mutex);
        let class = windows_sys::w!("SCRP.Desktop");
        if already_open {
            if !start_hidden {
                let existing = FindWindowW(class, null_mut());
                if !existing.is_null() {
                    ShowWindow(existing, SW_RESTORE);
                    SetForegroundWindow(existing);
                }
            }
            return Ok(());
        }

        let args = Args::parse_from(["scrp"]);
        let config = Config::load(&args)?;
        let locations = Locations::load(config.locations_file.as_deref())?;
        let file =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is missing")?)
                .join("StarCitizenRichPresence/preferences.toml");
        let mut initial = match std::fs::read_to_string(&file) {
            Ok(text) => toml::from_str(&text).context("Invalid desktop preferences")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Preferences::from_config(&config),
            Err(e) => return Err(e).context("Could not read desktop preferences"),
        };
        if initial.buttons.is_none() {
            initial.buttons = Some(config.buttons.clone());
        }
        let instance = GetModuleHandleW(null_mut());
        let icon = LoadIconW(instance, APP_ICON);
        anyhow::ensure!(!icon.is_null(), "Cannot load the companion icon");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: icon,
            hbrBackground: null_mut(),
            ..Default::default()
        };
        anyhow::ensure!(
            RegisterClassW(&wc) != 0,
            "Cannot register the desktop window"
        );
        let initial_theme = Theme::from_prefs(&initial);
        let prefs = Arc::new(Mutex::new(initial));
        let status = Arc::new(Mutex::new(String::from("Waiting for Star Citizen…")));
        let preview = Arc::new(Mutex::new(None));
        let update_state = Arc::new(Mutex::new(UpdateState::Idle));
        let mut desktop = Box::new(Desktop {
            prefs: Arc::clone(&prefs),
            status: Arc::clone(&status),
            preview: Arc::clone(&preview),
            update: Arc::clone(&update_state),
            upload: Arc::new(Mutex::new(UploadState::Idle)),
            thumbnails: Arc::default(),
            requested: Default::default(),
            base: config.clone(),
            file,
            icon,
            taskbar_created: RegisterWindowMessageW(windows_sys::w!("TaskbarCreated")),
            tray_ready: Cell::new(false),
            page: Cell::new(CARD),
            banner: Cell::new(false),
            painted: Default::default(),
            fonts: Cell::new(None),
            theme: Cell::new(initial_theme),
            field: Cell::new(CreateSolidBrush(initial_theme.field)),
            buffer: Cell::new(BackBuffer::EMPTY),
            hot: Cell::new(null_mut()),
            dirty: Cell::new(false),
            targets: targets(),
            target: Cell::new(0),
            image_page: Cell::new(0),
            notified_update: Default::default(),
        });
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: CLIENT_WIDTH,
            bottom: CLIENT_HEIGHT,
        };
        AdjustWindowRectEx(&mut frame, style, 0, 0);
        let hwnd = CreateWindowExW(
            0,
            class,
            windows_sys::w!("Star Citizen Rich Presence"),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            frame.right - frame.left,
            frame.bottom - frame.top,
            null_mut(),
            null_mut(),
            instance,
            (&mut *desktop as *mut Desktop).cast(),
        );
        anyhow::ensure!(!hwnd.is_null(), "Cannot create the desktop window");
        let small_icon = LoadImageW(
            instance,
            APP_ICON,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_SHARED,
        );
        if !small_icon.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small_icon as LPARAM);
        }
        let dark_title = 1u32;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark_title as *const u32).cast(),
            size_of::<u32>() as u32,
        );
        let t = desktop.theme.get();
        for (attribute, color) in [
            (DWMWA_CAPTION_COLOR, t.caption),
            (DWMWA_BORDER_COLOR, t.tile_border),
            (DWMWA_TEXT_COLOR, SOFT),
        ] {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                (&color as *const u32).cast(),
                size_of::<u32>() as u32,
            );
        }
        // Windows 11: round the window corners like every other modern app.
        let round = 2u32;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&round as *const u32).cast(),
            size_of::<u32>() as u32,
        );
        if !start_hidden || !desktop.tray_ready.get() {
            ShowWindow(hwnd, SW_SHOW);
        }
        let running = Arc::new(AtomicBool::new(true));

        // Update checks: shortly after start, then every 30 minutes, while enabled.
        if config.check_for_updates {
            let prefs = Arc::clone(&prefs);
            let state = Arc::clone(&update_state);
            let running = Arc::clone(&running);
            let handle = hwnd as isize;
            std::thread::spawn(move || {
                let mut next = Instant::now() + Duration::from_secs(5);
                while running.load(Ordering::SeqCst) {
                    if Instant::now() >= next {
                        if prefs.lock().unwrap().updates {
                            check_for_update(Arc::clone(&state), handle);
                        }
                        next = Instant::now() + Duration::from_secs(30 * 60);
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
        }

        let demo = std::env::var_os("SCRP_DEMO").is_some();
        if demo {
            *preview.lock().unwrap() = Some(Snapshot {
                name: "Star Citizen".into(),
                details: "Area18 · ArcCorp · Stanton".into(),
                state: "Aegis Gladius · Bounty Hunting".into(),
                large_image: String::new(),
                large_text: "Aegis Gladius".into(),
                small_image: String::new(),
                small_text: "Bounty".into(),
                activity_type: 0,
                status_display: 0,
                profile_url: None,
                buttons: Vec::new(),
                started: now() - 3661,
            });
            if std::env::var_os("SCRP_DEMO_BANNER").is_some() {
                *update_state.lock().unwrap() = UpdateState::Available(Release {
                    version: "3.2.0".into(),
                    notes_url: String::new(),
                    installer_url: None,
                    sums_url: None,
                });
            }
        }
        let signal = Arc::clone(&running);
        let worker = std::thread::spawn(move || {
            monitor(
                args,
                config,
                locations,
                signal,
                |config, update| {
                    if let Some(update) = update {
                        *status.lock().unwrap() = update.to_owned();
                    }
                    let prefs = prefs.lock().unwrap();
                    if update.is_none() {
                        prefs.apply(config);
                    }
                    prefs.enabled
                },
                |card| {
                    if !demo {
                        *preview.lock().unwrap() = card.cloned()
                    }
                },
            )
        });
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        running.store(false, Ordering::SeqCst);
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("Presence monitor stopped unexpectedly"))?
    }
}

unsafe fn tray(hwnd: HWND, icon: HICON, operation: u32) -> bool {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY,
        hIcon: icon,
        ..Default::default()
    };
    let tip = wide("Star Citizen Rich Presence");
    data.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe { Shell_NotifyIconW(operation, &data) != 0 }
}

/// Shows a native Windows tray notification when a new release is detected.
unsafe fn notify_update(hwnd: HWND, desktop: &Desktop, version: &str) {
    if !desktop.tray_ready.get() {
        return;
    }
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };
    let title = wide("Update available");
    let body = wide(&format!(
        "Star Citizen Rich Presence v{version} is ready. Click to open and update."
    ));
    let tlen = title.len().min(data.szInfoTitle.len());
    data.szInfoTitle[..tlen].copy_from_slice(&title[..tlen]);
    let blen = body.len().min(data.szInfo.len());
    data.szInfo[..blen].copy_from_slice(&body[..blen]);
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

unsafe fn hide(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        if desktop.tray_ready.get() {
            ShowWindow(hwnd, SW_HIDE);
        } else {
            ShowWindow(hwnd, SW_SHOWMINIMIZED);
        }
    }
}

unsafe fn font(size: i32, weight: i32, face: Pcwstr) -> HFONT {
    unsafe { CreateFontW(s(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, face) }
}

/// Creates (or recreates, after a DPI change) every font the panel draws with.
unsafe fn create_fonts(desktop: &Desktop) {
    unsafe {
        if let Some(old) = desktop.fonts.replace(None) {
            delete_fonts(old);
        }
        let face = windows_sys::w!("Segoe UI");
        desktop.fonts.set(Some(Fonts {
            brand: font(-15, 600, face),
            page_title: font(-18, 600, face),
            body: font(-13, 400, face),
            body_semi: font(-13, 600, face),
            small: font(-12, 400, face),
            small_semi: font(-12, 600, face),
            tiny: font(-11, 400, face),
            tiny_semi: font(-11, 600, face),
            details: font(-16, 600, face),
            kicker: font(-10, 700, face),
            glyph: font(-16, 400, windows_sys::w!("Segoe MDL2 Assets")),
            glyph_small: font(-11, 400, windows_sys::w!("Segoe MDL2 Assets")),
            mono: font(-12, 400, windows_sys::w!("Consolas")),
        }));
    }
}

unsafe fn delete_fonts(fonts: Fonts) {
    unsafe {
        for handle in [
            fonts.brand,
            fonts.page_title,
            fonts.body,
            fonts.body_semi,
            fonts.small,
            fonts.small_semi,
            fonts.tiny,
            fonts.tiny_semi,
            fonts.details,
            fonts.kicker,
            fonts.glyph,
            fonts.glyph_small,
            fonts.mono,
        ] {
            DeleteObject(handle);
        }
    }
}

unsafe extern "system" fn apply_child_font(child: HWND, font: LPARAM) -> i32 {
    unsafe {
        SendMessageW(child, WM_SETFONT, font as usize, 1);
    }
    1
}

/// Points every child control at the current body font.
unsafe fn apply_fonts(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        EnumChildWindows(
            hwnd,
            Some(apply_child_font),
            desktop.fonts().body as usize as LPARAM,
        );
    }
}

/// Marks the control under the pointer as hot, so it can paint a hover state.
unsafe fn track_hover(hwnd: HWND, msg: u32, desktop: *const Desktop) {
    unsafe {
        if desktop.is_null() {
            return;
        }
        let desktop = &*desktop;
        match msg {
            WM_MOUSEMOVE => {
                desktop.hot.set(hwnd);
                let mut event = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut event);
            }
            WM_MOUSELEAVE => {
                desktop.hot.set(null_mut());
            }
            _ => {}
        }
    }
}

/// Shared subclass: hover feedback and a hand cursor for owner-drawn buttons.
unsafe extern "system" fn hover_subclass(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    unsafe {
        let _ = id;
        if matches!(msg, WM_MOUSEMOVE | WM_MOUSELEAVE) {
            let parent = GetParent(hwnd);
            let ptr = GetWindowLongPtrW(parent, GWLP_USERDATA) as *const Desktop;
            track_hover(hwnd, msg, ptr);
            InvalidateRect(hwnd, null_mut(), 0);
        }
        if msg == WM_SETCURSOR {
            SetCursor(LoadCursorW(null_mut(), IDC_HAND));
            return 1;
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

/// Resizes the window so the client area is exactly the design size at the current scale.
unsafe fn fit_client(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: s(CLIENT_WIDTH),
            bottom: s(CLIENT_HEIGHT),
        };
        AdjustWindowRectEx(&mut frame, style, 0, 0);
        SetWindowPos(
            hwnd,
            null_mut(),
            0,
            0,
            frame.right - frame.left,
            frame.bottom - frame.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

// ── Layout ──────────────────────────────────────────────────────────────────

fn card_top(banner: bool) -> i32 {
    if banner { 56 } else { 14 }
}

/// Hero card height: shorter while the update banner is showing.
fn card_height(banner: bool) -> i32 {
    if banner { 96 } else { 112 }
}

fn page_top(banner: bool) -> i32 {
    card_top(banner) + card_height(banner) + 14
}

// ── Rail geometry ───────────────────────────────────────────────────────────
const NAV_TOP: i32 = 80;
const NAV_HEIGHT: i32 = 34;
const NAV_PITCH: i32 = 40;

fn nav_rect(index: i32) -> RECT {
    rect(8, NAV_TOP + index * NAV_PITCH, 190, NAV_HEIGHT)
}

// ── Footer geometry ─────────────────────────────────────────────────────────
const BUTTON_HEIGHT: i32 = 32;
const FOOTER_Y: i32 = FOOTER_TOP + 8;

fn footer_rects() -> [(i32, RECT); 3] {
    [
        (EXIT, rect(LEFT, FOOTER_Y, 84, BUTTON_HEIGHT)),
        (
            HIDE,
            rect(RIGHT - 12 - 88 - 108, FOOTER_Y, 108, BUTTON_HEIGHT),
        ),
        (SAVE, rect(RIGHT - 88, FOOTER_Y, 88, BUTTON_HEIGHT)),
    ]
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    let (left, top) = (s(x), s(y));
    RECT {
        left,
        top,
        right: left + s(w),
        bottom: top + s(h),
    }
}

/// A child rectangle positioned in logical units relative to a device rect.
fn at(r: RECT, x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: r.left + s(x),
        top: r.top + s(y),
        right: r.left + s(x + w),
        bottom: r.top + s(y + h),
    }
}

/// One settings row: 42 px tall, a 13 px title over an 11 px description.
const ROW_H: i32 = 42;
const ROW_GAP: i32 = 6;
const GROUP_ROW_H: i32 = 46;

/// Toggle tiles per page: (id, label, column, row, full width).
/// Rows 0-3 sit under "Show on card", rows 4-5 under "General".
const CARD_TILES: [(i32, &str, i32, i32, bool); 10] = [
    (LOCATION, "Location", 0, 0, false),
    (SHIP, "Ship", 1, 0, false),
    (CONTRACTS, "Contracts", 0, 1, false),
    (PARTY, "Party status", 1, 1, false),
    (NOTICES, "Game notices", 0, 2, false),
    (PROFILE, "RSI profile button", 1, 2, false),
    (ART, "Wiki artwork", 0, 3, true),
    (ENABLED, "Share on Discord", 0, 4, false),
    (STARTUP, "Start with Windows", 1, 4, false),
    (UPDATES, "Check for updates automatically", 0, 5, true),
];

/// The one-line explanation under each Card-page toggle title.
fn toggle_detail(id: i32) -> &'static str {
    match id {
        LOCATION => "System and landing zone",
        SHIP => "The ship you're flying",
        CONTRACTS => "Your active contract",
        PARTY => "When you're in a group",
        NOTICES => "Hangar and refinery alerts",
        PROFILE => "Link to your citizen page",
        ART => "Ship and location pictures from the wiki",
        ENABLED => "Publish live presence to Discord",
        STARTUP => "Start minimised in the tray",
        UPDATES => "Checked at startup and every 30 minutes",
        _ => "",
    }
}

fn tile_rect(top: i32, column: i32, row: i32, full: bool) -> RECT {
    let y = top + 52 + group_offset(row) + row * (ROW_H + ROW_GAP);
    let x = LEFT + column * (COLUMN + COLUMN_GAP);
    rect(x, y, if full { RIGHT - LEFT } else { COLUMN }, ROW_H)
}

/// Extra space taken by the "General" kicker between rows 3 and 4.
fn group_offset(row: i32) -> i32 {
    if row >= 4 { 26 } else { 0 }
}

/// Frames the layout uses, relative to `top`, for Activity rows.
fn activity_row(top: i32, index: i32) -> RECT {
    let y = if index < 2 {
        top + 52 + index * GROUP_ROW_H
    } else {
        top + 170 + (index - 2) * GROUP_ROW_H
    };
    rect(LEFT, y, RIGHT - LEFT, GROUP_ROW_H)
}

/// Picture tile `index` on the Images page: nine 66×66 1:1 previews per row.
fn choice_rect(top: i32, index: usize) -> RECT {
    let (column, row) = ((index % 9) as i32, (index / 9) as i32);
    rect(LEFT + column * 73, top + 74 + row * 73, 66, 66)
}

/// "Your own link" field below the picture grid; `rows` is the visible tile rows.
fn link_frame(top: i32, rows: i32) -> RECT {
    rect(LEFT, top + 74 + rows * 73 + 14, RIGHT - LEFT - 128, 32)
}

/// Tile rows the Images page needs for the current page of pictures.
fn image_rows(desktop: &Desktop) -> i32 {
    let (_, visible) = desktop.visible_choices();
    (visible.len().div_ceil(9) as i32).max(1)
}

fn buttons_box(top: i32) -> RECT {
    rect(LEFT, top + 52, RIGHT - LEFT, 144)
}

fn updates_hero(top: i32) -> RECT {
    rect(LEFT, top + 34, RIGHT - LEFT, 84)
}

/// Field frames drawn behind EDIT controls: (id, frame).
fn fields(page: usize, top: i32, image_rows: i32) -> Vec<(i32, RECT)> {
    match page {
        ACTIVITY => [(2, GAME_NAME), (3, DETAILS), (4, CUSTOM)]
            .into_iter()
            .map(|(row, id)| {
                let r = activity_row(top, row);
                (
                    id,
                    RECT {
                        left: r.right - s(14 + 252),
                        top: r.top + s(8),
                        right: r.right - s(14),
                        bottom: r.top + s(38),
                    },
                )
            })
            .collect(),
        IMAGES => vec![(IMAGE_LINK, link_frame(top, image_rows))],
        BUTTONS => {
            let b = buttons_box(top);
            LINK_FIELDS
                .into_iter()
                .enumerate()
                .flat_map(|(i, (label, url))| {
                    let y = 32 + i as i32 * 72;
                    [
                        (label, at(b, 14, y, 200, 32)),
                        (
                            url,
                            RECT {
                                left: b.left + s(226),
                                top: b.top + s(y),
                                right: b.right - s(14),
                                bottom: b.top + s(y + 32),
                            },
                        ),
                    ]
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

unsafe fn place(hwnd: HWND, id: i32, visible: bool, r: RECT) {
    unsafe {
        let control = GetDlgItem(hwnd, id);
        if control.is_null() {
            return;
        }
        SetWindowPos(
            control,
            null_mut(),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        ShowWindow(control, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}

/// Positions and shows only the controls of the current page.
unsafe fn layout(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        let page = desktop.page.get();
        let banner = desktop.banner.get();
        let top = page_top(banner);
        let hidden = rect(-500, -500, 10, 10);
        // Hover state does not survive a relayout: controls move or hide.
        desktop.hot.set(null_mut());

        for index in 0..PAGES.len() as i32 {
            place(hwnd, NAV + index, true, nav_rect(index));
        }
        for (id, text, column, row, full) in CARD_TILES {
            let _ = text;
            place(hwnd, id, page == CARD, tile_rect(top, column, row, full));
        }
        // Banner actions.
        place(hwnd, UPDATE_NOW, banner, rect(RIGHT - 12 - 96, 22, 96, 24));
        place(
            hwnd,
            RELEASE_NOTES,
            banner,
            rect(RIGHT - 12 - 96 - 8 - 92, 22, 92, 24),
        );

        // Activity page: the two combos sit at the right of their rows.
        for (row, id) in [(0, VERB), (1, MEMBER_LIST)] {
            let r = activity_row(top, row);
            place(
                hwnd,
                id,
                page == ACTIVITY,
                RECT {
                    left: r.right - s(14 + 252),
                    top: r.top + s(8),
                    right: r.right - s(14),
                    // A dropdown needs room for its list below the closed box.
                    bottom: r.top + s(8 + 220),
                },
            );
        }
        // Images page.
        let paged = desktop.image_page_count() > 1;
        place(
            hwnd,
            IMAGE_TARGET,
            page == IMAGES,
            rect(LEFT, top + 34, if paged { 252 } else { 330 }, 320),
        );
        place(
            hwnd,
            IMG_PREV,
            page == IMAGES && paged,
            rect(LEFT + 262, top + 34, 52, 30),
        );
        place(
            hwnd,
            IMG_NEXT,
            page == IMAGES && paged,
            rect(LEFT + 322, top + 34, 52, 30),
        );
        place(
            hwnd,
            UPLOAD_IMAGE,
            page == IMAGES,
            rect(RIGHT - 130 - 8 - 124, top + 34, 124, 30),
        );
        place(
            hwnd,
            USE_DEFAULT,
            page == IMAGES,
            rect(RIGHT - 130, top + 34, 130, 30),
        );
        let (_, visible) = desktop.visible_choices();
        for index in 0..MAX_CHOICES {
            place(
                hwnd,
                CHOICE + index as i32,
                page == IMAGES && index < visible.len(),
                choice_rect(top, index),
            );
        }
        let link = link_frame(top, image_rows(desktop));
        place(
            hwnd,
            USE_LINK,
            page == IMAGES,
            rect(RIGHT - 120, link.top, 120, 32),
        );
        // Appearance page.
        for index in 0..THEMES.len() {
            let (w, gap) = (156, 10);
            place(
                hwnd,
                THEME_BTN + index as i32,
                page == APPEARANCE,
                rect(LEFT + index as i32 * (w + gap), top + 52, w, 68),
            );
        }
        for index in 0..ACCENTS.len() {
            let (w, gap) = (102, 9);
            place(
                hwnd,
                ACCENT_BTN + index as i32,
                page == APPEARANCE,
                rect(LEFT + index as i32 * (w + gap), top + 146, w, 38),
            );
        }
        for index in 0..2 {
            let (w, gap) = (156, 10);
            place(
                hwnd,
                CORNER_BTN + index,
                page == APPEARANCE,
                rect(LEFT + index * (w + gap), top + 210, w, 36),
            );
        }
        // Updates page.
        let u = updates_hero(top);
        place(
            hwnd,
            UPDATE_ACTION,
            page == UPDATES_PAGE,
            RECT {
                left: u.right - s(14 + 136),
                top: u.top + s(26),
                right: u.right - s(14),
                bottom: u.top + s(26 + BUTTON_HEIGHT),
            },
        );
        place(
            hwnd,
            UPDATE_NOTES,
            page == UPDATES_PAGE,
            RECT {
                left: u.right - s(14 + 136 + 10 + 110),
                top: u.top + s(26),
                right: u.right - s(14 + 136 + 10),
                bottom: u.top + s(26 + BUTTON_HEIGHT),
            },
        );

        // Footer.
        for (id, r) in footer_rects() {
            place(hwnd, id, true, r);
        }

        // EDIT controls sit inside painted field frames.
        for id in [GAME_NAME, DETAILS, CUSTOM, IMAGE_LINK, 121, 122, 123, 124] {
            place(hwnd, id, false, hidden);
        }
        for (id, frame) in fields(page, top, image_rows(desktop)) {
            place(
                hwnd,
                id,
                true,
                RECT {
                    left: frame.left + 9,
                    top: frame.top + 6,
                    right: frame.right - 9,
                    bottom: frame.bottom - 6,
                },
            );
        }
        InvalidateRect(hwnd, null_mut(), 0);
    }
}

// ── Controls ────────────────────────────────────────────────────────────────

unsafe fn build_controls(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        create_fonts(desktop);
        let body = desktop.fonts().body;
        let control = |class: Pcwstr, label: &str, style: u32, id: i32| {
            let handle = CreateWindowExW(
                0,
                class,
                wide(label).as_ptr(),
                WS_CHILD | style,
                -500,
                -500,
                10,
                10,
                hwnd,
                id as usize as HMENU,
                null_mut(),
                null_mut(),
            );
            SendMessageW(handle, WM_SETFONT, body as usize, 1);
            handle
        };
        let button = |id: i32, label: &str| {
            let handle = control(
                windows_sys::w!("BUTTON"),
                label,
                WS_TABSTOP | BS_OWNERDRAW as u32,
                id,
            );
            SetWindowSubclass(handle, Some(hover_subclass), 3, 0);
            handle
        };
        let prefs = desktop.prefs.lock().unwrap().clone();

        for (index, name) in PAGES.iter().enumerate() {
            button(NAV + index as i32, name);
        }
        for (id, label, ..) in CARD_TILES {
            let checked = match id {
                LOCATION => prefs.location,
                SHIP => prefs.ship,
                CONTRACTS => prefs.contracts,
                PARTY => prefs.party,
                NOTICES => prefs.notices,
                PROFILE => prefs.profile,
                ART => prefs.artwork,
                ENABLED => prefs.enabled,
                STARTUP => startup_enabled(),
                UPDATES => prefs.updates && desktop.base.check_for_updates,
                _ => false,
            };
            let check = control(
                windows_sys::w!("BUTTON"),
                label,
                WS_TABSTOP | BS_AUTOCHECKBOX as u32,
                id,
            );
            SendMessageW(check, BM_SETCHECK, checked as usize, 0);
            SetWindowSubclass(check, Some(paint_toggle), 1, body as usize);
        }

        let edit = |id: i32, value: &str, hint: &str, limit: usize| {
            let handle = control(
                windows_sys::w!("EDIT"),
                value,
                WS_TABSTOP | ES_AUTOHSCROLL as u32,
                id,
            );
            SendMessageW(handle, EM_SETLIMITTEXT, limit, 0);
            SendMessageW(handle, EM_SETCUEBANNER, 0, wide(hint).as_ptr() as LPARAM);
        };
        let combo = |id: i32, style: u32| {
            let handle = control(
                windows_sys::w!("COMBOBOX"),
                "",
                WS_TABSTOP | WS_VSCROLL | style | CBS_OWNERDRAWFIXED as u32 | CBS_HASSTRINGS as u32,
                id,
            );
            SetWindowTheme(handle, windows_sys::w!("DarkMode_CFD"), null_mut());
            SendMessageW(handle, CB_SETITEMHEIGHT, usize::MAX, s(24) as isize);
            SendMessageW(handle, CB_SETITEMHEIGHT, 0, s(28) as isize);
            SetWindowSubclass(handle, Some(paint_combo), 2, body as usize);
            handle
        };

        let verb = combo(VERB, CBS_DROPDOWNLIST as u32);
        let current = prefs
            .activity_type
            .clone()
            .unwrap_or_else(|| desktop.base.activity_type.clone());
        for (index, (key, label)) in VERBS.iter().enumerate() {
            SendMessageW(verb, CB_ADDSTRING, 0, wide(label).as_ptr() as LPARAM);
            if *key == current {
                SendMessageW(verb, CB_SETCURSEL, index, 0);
            }
        }
        let member = combo(MEMBER_LIST, CBS_DROPDOWNLIST as u32);
        let current = prefs
            .status_display
            .clone()
            .unwrap_or_else(|| desktop.base.status_display.clone());
        for (index, (key, label)) in MEMBER_LIST_OPTIONS.iter().enumerate() {
            SendMessageW(member, CB_ADDSTRING, 0, wide(label).as_ptr() as LPARAM);
            if *key == current {
                SendMessageW(member, CB_SETCURSEL, index, 0);
            }
        }
        edit(GAME_NAME, &prefs.game_name, "Star Citizen", 128);
        edit(DETAILS, &prefs.details, "Automatic", 128);
        edit(CUSTOM, &prefs.custom, "Automatic", 128);

        let target = combo(IMAGE_TARGET, CBS_DROPDOWNLIST as u32);
        for item in &desktop.targets {
            SendMessageW(
                target,
                CB_ADDSTRING,
                0,
                wide(&item.label).as_ptr() as LPARAM,
            );
        }
        SendMessageW(target, CB_SETCURSEL, desktop.target.get(), 0);
        edit(
            IMAGE_LINK,
            "",
            "Or paste your own image link (https://…)",
            256,
        );
        button(USE_LINK, "Use link");
        button(IMG_PREV, "‹ Prev");
        button(IMG_NEXT, "Next ›");
        button(UPLOAD_IMAGE, "Upload image");
        button(USE_DEFAULT, "Use default");
        for index in 0..MAX_CHOICES {
            button(CHOICE + index as i32, &format!("Picture {}", index + 1));
        }

        let buttons = prefs.buttons.as_deref().unwrap_or_default();
        for (index, (name, url)) in LINK_FIELDS.into_iter().enumerate() {
            let button = buttons.get(index);
            edit(name, button.map_or("", |b| &b.label), "Button label", 32);
            edit(url, button.map_or("", |b| &b.url), "https://…", 512);
        }

        for (index, (_, label, ..)) in THEMES.iter().enumerate() {
            button(THEME_BTN + index as i32, label);
        }
        for (index, (_, label, ..)) in ACCENTS.iter().enumerate() {
            button(ACCENT_BTN + index as i32, label);
        }
        button(CORNER_BTN, "Rounded corners");
        button(CORNER_BTN + 1, "Sharp corners");

        button(UPDATE_NOW, "Update now");
        button(RELEASE_NOTES, "Release notes");
        button(UPDATE_ACTION, "Check now");
        button(UPDATE_NOTES, "Release notes");

        for (id, label, r) in [
            (HIDE, "Hide to tray", rect(LEFT, 549, 110, 30)),
            (SAVE, "Apply", rect(RIGHT - 74 - 8 - 88, 549, 88, 30)),
            (EXIT, "Quit", rect(RIGHT - 74, 549, 74, 30)),
        ] {
            let handle = button(id, label);
            SetWindowPos(
                handle,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_SHOWWINDOW,
            );
        }
        layout(hwnd, desktop);
    }
}

// ── Painting ────────────────────────────────────────────────────────────────

unsafe fn fill(dc: HDC, r: RECT, color: u32) {
    unsafe {
        SetDCBrushColor(dc, color);
        FillRect(dc, &r, GetStockObject(DC_BRUSH) as HBRUSH);
    }
}

/// Anti-aliased rounded rectangle (with optional crisp 1 px inner border) via analytical SDF.
unsafe fn shape(dc: HDC, r: RECT, radius: i32, color: u32, border: Option<u32>) {
    unsafe {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            return;
        }
        if radius <= 0 && border.is_none() {
            fill(dc, r, color);
            return;
        }
        let thumb = SHAPES.with(|cache| {
            cache
                .borrow_mut()
                .get_rounded(w as u32, h as u32, radius, color, border)
        });
        draw_thumbnail(dc, &thumb, r.left, r.top);
    }
}

/// Anti-aliased filled circle via analytical SDF.
unsafe fn circle(dc: HDC, x: i32, y: i32, size: i32, color: u32) {
    unsafe {
        if size <= 0 {
            return;
        }
        let thumb = SHAPES.with(|cache| cache.borrow_mut().get_circle(size as u32, color));
        draw_thumbnail(dc, &thumb, x, y);
    }
}

unsafe fn line(dc: HDC, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
    unsafe {
        fill(
            dc,
            RECT {
                left: x1,
                top: y1,
                right: x2.max(x1 + 1),
                bottom: y2.max(y1 + 1),
            },
            color,
        );
    }
}

unsafe fn text(dc: HDC, value: &str, mut r: RECT, font: HFONT, color: u32, flags: u32) -> RECT {
    unsafe {
        let previous = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        let mut wide: Vec<u16> = value.encode_utf16().collect();
        let len = wide.len() as i32;
        DrawTextW(dc, wide.as_mut_ptr(), len, &mut r, flags | DT_NOPREFIX);
        let mut size = SIZE::default();
        GetTextExtentPoint32W(dc, wide.as_mut_ptr(), len, &mut size);
        SelectObject(dc, previous);
        RECT {
            left: r.left,
            top: r.top,
            right: r.left + size.cx,
            bottom: r.top + size.cy,
        }
    }
}

/// Width of `value` in `font`, without drawing it.
unsafe fn width_of(dc: HDC, value: &str, font: HFONT) -> i32 {
    unsafe {
        let previous = SelectObject(dc, font);
        let mut wide: Vec<u16> = value.encode_utf16().collect();
        let mut size = SIZE::default();
        GetTextExtentPoint32W(dc, wide.as_mut_ptr(), wide.len() as i32, &mut size);
        SelectObject(dc, previous);
        size.cx
    }
}

/// Draws a small caps label with 1 px tracking.
unsafe fn kicker_text(dc: HDC, value: &str, r: RECT, font: HFONT, color: u32, flags: u32) -> RECT {
    unsafe {
        let previous = SetTextCharacterExtra(dc, s(1));
        let measured = text(dc, value, r, font, color, flags);
        SetTextCharacterExtra(dc, previous);
        measured
    }
}

const LEFT_LINE: u32 = DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;

unsafe fn draw_thumbnail(dc: HDC, thumb: &Thumbnail, x: i32, y: i32) {
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: thumb.width as i32,
                biHeight: -(thumb.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(thumb.bgra.as_ptr(), bits.cast(), thumb.bgra.len());
        let memory = CreateCompatibleDC(dc);
        let old = SelectObject(memory, bitmap);
        AlphaBlend(
            dc,
            x,
            y,
            thumb.width as i32,
            thumb.height as i32,
            memory,
            0,
            0,
            thumb.width as i32,
            thumb.height as i32,
            BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            },
        );
        SelectObject(memory, old);
        DeleteDC(memory);
        DeleteObject(bitmap);
    }
}

/// Starts downloading artwork that isn't cached yet.
fn want_thumbnail(
    desktop: &Desktop,
    hwnd: HWND,
    url: &str,
    (width, height): (u32, u32),
    mask: Mask,
) -> Option<Thumbnail> {
    let key = format!("{width}x{height}:{url}");
    if let Some(found) = desktop.thumbnails.lock().unwrap().get(&key) {
        return found.clone();
    }
    if !url.starts_with("https://") || !desktop.requested.borrow_mut().insert(key.clone()) {
        return None;
    }
    let cache = Arc::clone(&desktop.thumbnails);
    let url = url.to_owned();
    let handle = hwnd as isize;
    std::thread::spawn(move || {
        let result = thumbnail::fetch_cached(&url, width, height, mask).ok();
        cache.lock().unwrap().insert(key, result);
        request_refresh(handle);
    });
    None
}

#[derive(Clone, Copy, PartialEq)]
enum Liveness {
    Live,
    Paused,
    Waiting,
}

impl Liveness {
    fn label(self) -> &'static str {
        match self {
            Self::Live => "Live on Discord",
            Self::Paused => "Sharing paused",
            Self::Waiting => "Waiting for game",
        }
    }

    /// Compact form for the rail and the monitor card.
    fn short_label(self) -> &'static str {
        match self {
            Self::Live => "Connected",
            Self::Paused => "Paused",
            Self::Waiting => "Waiting",
        }
    }
}

fn liveness(desktop: &Desktop) -> Liveness {
    if !desktop.prefs.lock().unwrap().enabled {
        Liveness::Paused
    } else if desktop.preview.lock().unwrap().is_some() {
        Liveness::Live
    } else {
        Liveness::Waiting
    }
}

fn elapsed(started: i64) -> String {
    let seconds = (now() - started).max(0);
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

unsafe fn paint(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let target = BeginPaint(hwnd, &mut ps);
        let (width, height) = (s(CLIENT_WIDTH), s(CLIENT_HEIGHT));
        let dc = desktop.back_buffer(target, width, height);
        let full = rect(0, 0, CLIENT_WIDTH, CLIENT_HEIGHT);
        let t = desktop.theme.get();

        fill(dc, full, t.canvas);
        paint_rail(dc, desktop);
        let banner = desktop.banner.get();
        if banner {
            paint_banner(dc, desktop);
        }
        paint_card(dc, hwnd, desktop, card_top(banner));
        let top = page_top(banner);
        paint_page(dc, desktop, top);
        for (_, frame) in fields(desktop.page.get(), top, image_rows(desktop)) {
            shape(dc, frame, t.button_radius, t.field, Some(t.field_border));
            line(
                dc,
                frame.left + 4,
                frame.bottom - 1,
                frame.right - 4,
                frame.bottom,
                t.field_bottom,
            );
        }
        // Footer. A dark crease above the rule makes it read as a physical seam.
        line(
            dc,
            s(RAIL_WIDTH + 1),
            s(FOOTER_TOP - 1),
            s(CLIENT_WIDTH),
            s(FOOTER_TOP),
            mix(t.canvas, rgb(0, 0, 0), 0.55),
        );
        fill(
            dc,
            rect(
                RAIL_WIDTH + 1,
                FOOTER_TOP,
                CLIENT_WIDTH,
                CLIENT_HEIGHT - FOOTER_TOP,
            ),
            t.footer,
        );
        line(
            dc,
            s(RAIL_WIDTH + 1),
            s(FOOTER_TOP),
            s(CLIENT_WIDTH),
            s(FOOTER_TOP) + 1,
            t.rail_line,
        );
        let hint = match desktop.page.get() {
            ACTIVITY => "Dropdowns save instantly · text fields need Apply",
            BUTTONS => "Text fields and links need Apply",
            IMAGES => "Uploaded pictures are resized and public",
            _ => "",
        };
        if !hint.is_empty() {
            let f = desktop.fonts();
            text(
                dc,
                hint,
                rect(LEFT + 98, FOOTER_Y, 340, BUTTON_HEIGHT),
                f.small,
                MUTED,
                LEFT_LINE,
            );
        }

        BitBlt(target, 0, 0, width, height, dc, 0, 0, SRCCOPY);
        EndPaint(hwnd, &ps);
    }
}

unsafe fn paint_rail(dc: HDC, desktop: &Desktop) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        fill(dc, rect(0, 0, RAIL_WIDTH, CLIENT_HEIGHT), t.rail);
        line(
            dc,
            s(RAIL_WIDTH),
            0,
            s(RAIL_WIDTH + 1),
            s(CLIENT_HEIGHT),
            t.rail_line,
        );

        // Brand block.
        shape(dc, rect(16, 16, 32, 32), 8, t.tile, Some(t.tile_border));
        DrawIconEx(
            dc,
            s(22),
            s(22),
            desktop.icon,
            s(20),
            s(20),
            0,
            null_mut(),
            DI_NORMAL,
        );
        text(
            dc,
            "Star Citizen",
            rect(58, 16, 132, 18),
            f.brand,
            TEXT,
            LEFT_LINE,
        );
        text(
            dc,
            "Rich Presence",
            rect(58, 34, 132, 14),
            f.tiny,
            MUTED,
            LEFT_LINE,
        );
        fade_line(dc, s(16), s(190), 64, mix(t.accent, t.rail, 0.30), t.rail);

        // Status footer: halo dot, label, version.
        let live = liveness(desktop);
        let (tone, label) = match live {
            Liveness::Live => (LIVE, live.short_label()),
            Liveness::Paused => (PAUSED, live.short_label()),
            Liveness::Waiting => (MUTED, live.short_label()),
        };
        line(
            dc,
            0,
            s(FOOTER_TOP),
            s(RAIL_WIDTH),
            s(FOOTER_TOP + 1),
            t.rail_line,
        );
        if live != Liveness::Waiting {
            circle(dc, s(15), s(558), s(15), mix(t.rail, tone, 0.16));
            circle(dc, s(17), s(560), s(11), mix(t.rail, tone, 0.36));
        }
        circle(dc, s(19), s(562), s(7), tone);
        text(
            dc,
            label,
            rect(32, 555, 110, 18),
            f.small_semi,
            tone,
            LEFT_LINE,
        );
        text(
            dc,
            &format!("v{}", env!("CARGO_PKG_VERSION")),
            rect(132, 555, 58, 18),
            f.tiny,
            MUTED,
            DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
        );
    }
}

/// One-pixel horizontal line that fades from `from` to `to` left to right.
unsafe fn fade_line(dc: HDC, x1: i32, x2: i32, y: i32, from: u32, to: u32) {
    unsafe {
        const STEPS: i32 = 20;
        let width = x2 - x1;
        if width <= 0 {
            return;
        }
        for step in 0..STEPS {
            let a = x1 + width * step / STEPS;
            let b = x1 + width * (step + 1) / STEPS;
            let color = mix(from, to, step as f32 / (STEPS - 1) as f32);
            line(dc, a, s(y), b.max(a + 1), s(y + 1), color);
        }
    }
}

/// Soft drop shadow: a few offset rounded rectangles under a raised surface.
unsafe fn shadow(dc: HDC, t: Theme, r: RECT, radius: i32) {
    unsafe {
        for (offset, weight) in [(5, 0.14), (3, 0.28), (1, 0.45)] {
            let mut layer = r;
            layer.top += s(offset);
            layer.bottom += s(offset);
            layer.left += s(offset / 2);
            layer.right -= s(offset / 2);
            shape(dc, layer, radius, mix(t.canvas, rgb(0, 0, 0), weight), None);
        }
    }
}

/// A raised settings surface: fill, hairline border and a 1 px top highlight.
unsafe fn surface(dc: HDC, r: RECT, radius: i32, fill_color: u32, border: u32) {
    unsafe {
        shape(dc, r, radius, fill_color, Some(border));
        let inset = s(radius.max(8));
        line(
            dc,
            r.left + inset,
            r.top + 1,
            r.right - inset,
            r.top + 2,
            lighten(fill_color, 0.06),
        );
    }
}

unsafe fn paint_banner(dc: HDC, desktop: &Desktop) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        let r = rect(LEFT, 12, RIGHT - LEFT, 34);
        shape(dc, r, t.tile_radius + 2, t.banner, Some(t.banner_border));
        circle(dc, r.left + s(10), r.top + s(8), s(18), t.accent);
        text(
            dc,
            "\u{E896}",
            RECT {
                left: r.left + s(10),
                top: r.top + s(8),
                right: r.left + s(28),
                bottom: r.top + s(26),
            },
            f.glyph_small,
            ON_ACCENT,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        let message = match &*desktop.update.lock().unwrap() {
            UpdateState::Downloading(release) => {
                format!("Downloading version {}…", release.version)
            }
            UpdateState::Available(release) => {
                format!("Version {} is ready to install", release.version)
            }
            _ => String::new(),
        };
        text(
            dc,
            &message,
            RECT {
                left: r.left + s(38),
                top: r.top,
                right: r.right - s(220),
                bottom: r.bottom,
            },
            f.small_semi,
            BANNER_TEXT,
            LEFT_LINE,
        );
    }
}

unsafe fn paint_card(dc: HDC, hwnd: HWND, desktop: &Desktop, top: i32) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        let card = desktop.preview.lock().unwrap().clone();
        // Copy only the fields this paint needs: `Preferences` owns two image maps.
        let (artwork_on, game_name, activity_type) = {
            let prefs = desktop.prefs.lock().unwrap();
            (
                prefs.artwork,
                prefs.game_name.trim().to_string(),
                prefs.activity_type.clone(),
            )
        };
        let height = card_height(desktop.banner.get());
        let r = rect(LEFT, top, RIGHT - LEFT, height);
        let radius = t.tile_radius + 4;
        let compact = height < 100;
        // Text rows hug the artwork: 82 px art in full height, 74 px when compact.
        let (verb_y, title_y, state_y, meta_y) = if compact {
            (8, 22, 44, 66)
        } else {
            (12, 30, 56, 78)
        };

        shadow(dc, t, r, radius);
        surface(dc, r, radius, t.card, t.card_border);

        // Artwork: 82 px ship (or location) art with a 28 px badge.
        let size = if height < 100 { 74 } else { 82 };
        let ay = r.top + s((height - size) / 2);
        let ax = r.left + s(15);
        let large = card
            .as_ref()
            .map(|c| c.large_image.clone())
            .unwrap_or_default();
        let art = (artwork_on && large.starts_with("https://"))
            .then(|| {
                want_thumbnail(
                    desktop,
                    hwnd,
                    &large,
                    (size as u32, size as u32),
                    Mask::Rounded(8),
                )
            })
            .flatten();
        match art {
            Some(thumb) => draw_thumbnail(dc, &thumb, ax, ay),
            None => {
                let well = RECT {
                    left: ax,
                    top: ay,
                    right: ax + s(size),
                    bottom: ay + s(size),
                };
                shape(
                    dc,
                    well,
                    8,
                    mix(t.card, t.accent, 0.07),
                    Some(lighten(t.card, 0.06)),
                );
                DrawIconEx(
                    dc,
                    ax + s(size / 2 - 16),
                    ay + s(size / 2 - 16),
                    desktop.icon,
                    s(32),
                    s(32),
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
            }
        }
        let small = card
            .as_ref()
            .map(|c| c.small_image.clone())
            .unwrap_or_default();
        if artwork_on
            && small.starts_with("https://")
            && let Some(badge) = want_thumbnail(desktop, hwnd, &small, (22, 22), Mask::Circle)
        {
            circle(dc, ax + s(size - 23), ay + s(size - 23), s(28), t.card);
            draw_thumbnail(dc, &badge, ax + s(size - 20), ay + s(size - 20));
        }

        let live = liveness(desktop);
        let x = ax + s(size) + s(16);
        let text_right = r.right - s(14);
        let verb = activity_type
            .as_deref()
            .unwrap_or(&desktop.base.activity_type);
        let verb = VERBS
            .iter()
            .find(|(key, _)| *key == verb)
            .map_or("Playing", |(_, label)| label);
        let app = if !game_name.is_empty() {
            game_name.as_str()
        } else if desktop.base.discord_app_id == star_citizen_rich_presence::config::DEFAULT_APP_ID
        {
            "Star Citizen"
        } else {
            "your Discord app"
        };

        // Status pill, right-aligned on the overline row.
        let (pill_fill, pill_border, pill_text) = match live {
            Liveness::Live => (t.status_fill(LIVE), t.status_border(LIVE), LIVE),
            Liveness::Paused => (t.status_fill(PAUSED), t.status_border(PAUSED), PAUSED),
            Liveness::Waiting => (t.chip, t.tile_border, MUTED),
        };
        let label = live.label();
        let pill_width = width_of(dc, label, f.tiny_semi) + s(30);
        let pill_top = r.top + s(verb_y - 1);
        let pill = RECT {
            left: text_right - pill_width,
            top: pill_top,
            right: text_right,
            bottom: pill_top + s(22),
        };
        shape(dc, pill, 11, pill_fill, Some(pill_border));
        if live != Liveness::Waiting {
            circle(
                dc,
                pill.left + s(6),
                pill.top + s(5),
                s(12),
                mix(pill_fill, pill_text, 0.22),
            );
        }
        circle(dc, pill.left + s(9), pill.top + s(8), s(6), pill_text);
        text(
            dc,
            label,
            RECT {
                left: pill.left + s(20),
                top: pill.top,
                right: pill.right,
                bottom: pill.bottom,
            },
            f.tiny_semi,
            pill_text,
            LEFT_LINE,
        );

        let verb_area = RECT {
            left: x,
            top: r.top + s(verb_y),
            right: pill.left - s(10),
            bottom: r.top + s(verb_y + 18),
        };
        kicker_text(
            dc,
            &format!("{verb} {app}").to_uppercase(),
            verb_area,
            f.kicker,
            if live == Liveness::Live {
                t.accent
            } else {
                MUTED
            },
            LEFT_LINE,
        );

        let text_width = text_right - x;
        let title_area = RECT {
            left: x,
            top: r.top + s(title_y),
            right: x + text_width,
            bottom: r.top + s(title_y + 24),
        };
        let state_area = RECT {
            left: x,
            top: r.top + s(state_y),
            right: x + text_width,
            bottom: r.top + s(state_y + 18),
        };
        let meta_area = RECT {
            left: x,
            top: r.top + s(meta_y),
            right: x + text_width,
            bottom: r.top + s(meta_y + 18),
        };
        match &card {
            Some(card) => {
                text(dc, &card.details, title_area, f.details, TEXT, LEFT_LINE);
                text(dc, &card.state, state_area, f.body, SOFT, LEFT_LINE);
                let elapsed_text = elapsed(card.started);
                text(
                    dc,
                    &elapsed_text,
                    meta_area,
                    f.mono,
                    if live == Liveness::Live { LIVE } else { MUTED },
                    LEFT_LINE,
                );
                // Channel chip after the timer: context without repeating the ship.
                let channel = desktop.base.channel.to_uppercase();
                let offset = width_of(dc, &elapsed_text, f.mono) + s(12);
                if !channel.is_empty() {
                    let width = width_of(dc, &channel, f.tiny_semi) + s(16);
                    let chip_rect = RECT {
                        left: x + offset,
                        top: r.top + s(meta_y + 1),
                        right: x + offset + width,
                        bottom: r.top + s(meta_y + 17),
                    };
                    shape(dc, chip_rect, 4, t.chip, Some(t.tile_border));
                    text(
                        dc,
                        &channel,
                        chip_rect,
                        f.tiny_semi,
                        MUTED,
                        DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                    );
                }
            }
            None => {
                text(
                    dc,
                    "Nothing on your card yet",
                    title_area,
                    f.details,
                    TEXT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    "Your live card appears here and on Discord once Star Citizen is running.",
                    state_area,
                    f.body,
                    SOFT,
                    LEFT_LINE,
                );
                let status = desktop.status.lock().unwrap().clone();
                let line = if status.starts_with("Waiting for Star Citizen") {
                    "Open Discord, then launch the game — the card fills in by itself.".to_string()
                } else {
                    status
                };
                text(dc, &line, meta_area, f.small, MUTED, LEFT_LINE);
            }
        }
    }
}

/// Page title on the left, one-line explanation on the right.
unsafe fn page_header(dc: HDC, desktop: &Desktop, top: i32, title: &str, subtitle: &str) {
    unsafe {
        let f = desktop.fonts();
        text(
            dc,
            title,
            rect(LEFT, top, 320, 26),
            f.page_title,
            TEXT,
            LEFT_LINE,
        );
        if !subtitle.is_empty() {
            text(
                dc,
                subtitle,
                RECT {
                    left: rect(LEFT + 320, top, RIGHT - LEFT - 320, 26).left,
                    top: rect(0, top, 0, 0).top,
                    right: rect(RIGHT, 0, 0, 0).left,
                    bottom: rect(0, top, 0, 0).top + s(26),
                },
                f.small,
                MUTED,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
    }
}

/// Section label with a hairline rule running to the right edge.
unsafe fn section_kicker(dc: HDC, desktop: &Desktop, top: i32, label: &str) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        let measured = kicker_text(
            dc,
            label,
            rect(LEFT, top, 400, 14),
            f.kicker,
            MUTED,
            LEFT_LINE,
        );
        fade_line(
            dc,
            measured.right + s(10),
            s(RIGHT),
            top + 7,
            t.divider,
            t.canvas,
        );
    }
}

/// Painted label + description row inside a grouped card.
unsafe fn settings_row(dc: HDC, desktop: &Desktop, r: RECT, title: &str, detail: &str) {
    unsafe {
        let f = desktop.fonts();
        text(
            dc,
            title,
            at(r, 14, 7, 420, 16),
            f.body_semi,
            TEXT,
            LEFT_LINE,
        );
        if !detail.is_empty() {
            text(dc, detail, at(r, 14, 25, 440, 14), f.tiny, MUTED, LEFT_LINE);
        }
    }
}

/// One grouped card holding `rows` painted settings rows, with hairline dividers.
unsafe fn settings_group(
    dc: HDC,
    desktop: &Desktop,
    r: RECT,
    rows: &[(i32, &str, &str)],
    row_height: i32,
) {
    unsafe {
        let t = desktop.theme.get();
        surface(dc, r, t.tile_radius + 2, t.tile, t.tile_border);
        for (index, (_, title, detail)) in rows.iter().enumerate() {
            let row = at(r, 0, index as i32 * row_height, RIGHT - LEFT, row_height);
            settings_row(dc, desktop, row, title, detail);
            if index > 0 {
                line(
                    dc,
                    row.left + s(14),
                    row.top,
                    row.right - s(14),
                    row.top + 1,
                    t.divider,
                );
            }
        }
    }
}

/// Small rounded chip with centred text; returns its width.
unsafe fn chip(
    dc: HDC,
    desktop: &Desktop,
    r: RECT,
    label: &str,
    fill: u32,
    border: u32,
    color: u32,
) -> i32 {
    unsafe {
        let f = desktop.fonts();
        let width = width_of(dc, label, f.tiny_semi) + s(18);
        let pill = RECT {
            left: r.left,
            top: r.top,
            right: r.left + width,
            bottom: r.bottom,
        };
        shape(dc, pill, (r.bottom - r.top) / 2, fill, Some(border));
        text(
            dc,
            label,
            RECT {
                left: pill.left + s(9),
                top: pill.top,
                right: pill.right,
                bottom: pill.bottom,
            },
            f.tiny_semi,
            color,
            LEFT_LINE,
        );
        width
    }
}

/// Fluent-style switch track and knob at `(x, y)`, 40 × 20 logical pixels.
unsafe fn switch(dc: HDC, t: Theme, x: i32, y: i32, on: bool, enabled: bool) {
    unsafe {
        let track = RECT {
            left: x,
            top: y,
            right: x + s(40),
            bottom: y + s(20),
        };
        if on {
            shape(
                dc,
                track,
                10,
                if enabled {
                    t.accent
                } else {
                    mix(t.tile, t.accent, 0.30)
                },
                None,
            );
            circle(
                dc,
                track.right - s(16),
                track.top + s(4),
                s(12),
                if enabled { ON_ACCENT } else { MUTED },
            );
        } else {
            shape(
                dc,
                track,
                10,
                t.canvas,
                Some(if enabled { t.field_bottom } else { t.divider }),
            );
            circle(
                dc,
                track.left + s(4),
                track.top + s(4),
                s(12),
                if enabled { SOFT } else { DISABLED },
            );
        }
    }
}

/// Discord-style member-list row showing what the selected line looks like.
unsafe fn preview_member_list(dc: HDC, desktop: &Desktop, r: RECT) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        surface(dc, r, t.tile_radius + 2, t.field, t.field_border);
        let avatar = at(r, 14, 14, 36, 36);
        shape(dc, avatar, 18, t.tile, Some(t.tile_border));
        DrawIconEx(
            dc,
            avatar.left + s(10),
            avatar.top + s(10),
            desktop.icon,
            s(16),
            s(16),
            0,
            null_mut(),
            DI_NORMAL,
        );
        circle(
            dc,
            avatar.right - s(10),
            avatar.bottom - s(10),
            s(9),
            t.field,
        );
        circle(dc, avatar.right - s(9), avatar.bottom - s(9), s(7), LIVE);

        let card = desktop.preview.lock().unwrap().clone();
        let prefs = desktop.prefs.lock().unwrap();
        let choice = prefs
            .status_display
            .clone()
            .unwrap_or_else(|| desktop.base.status_display.clone());
        drop(prefs);
        let line = match choice.as_str() {
            "details" => card
                .as_ref()
                .map_or_else(|| "In the 'verse".to_string(), |c| c.details.clone()),
            "state" => card.as_ref().map_or_else(
                || "Waiting for Star Citizen".to_string(),
                |c| c.state.clone(),
            ),
            _ => "Star Citizen".to_string(),
        };
        text(
            dc,
            "You",
            at(r, 62, 12, 200, 18),
            f.body_semi,
            TEXT,
            LEFT_LINE,
        );
        text(dc, &line, at(r, 62, 32, 320, 18), f.small, SOFT, LEFT_LINE);
        text(
            dc,
            "MEMBER LIST",
            RECT {
                left: r.right - s(110),
                top: r.top + s(22),
                right: r.right - s(14),
                bottom: r.top + s(40),
            },
            f.kicker,
            MUTED,
            DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
        );
    }
}

/// A Discord-shaped preview of the buttons that will appear on the card.
unsafe fn preview_buttons(dc: HDC, desktop: &Desktop, r: RECT, buttons: &[CardButton]) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        surface(dc, r, t.tile_radius + 2, t.field, t.field_border);
        text(
            dc,
            "PREVIEW",
            at(r, 16, 14, 240, 14),
            f.kicker,
            MUTED,
            LEFT_LINE,
        );
        let mut labels: Vec<&str> = buttons
            .iter()
            .filter(|b| !b.label.trim().is_empty())
            .map(|b| b.label.trim())
            .take(2)
            .collect();
        if labels.len() < 2 && desktop.prefs.lock().unwrap().profile {
            labels.push("RSI Profile");
        }
        for index in 0..2 {
            let pill = at(r, 14, 40 + index as i32 * 42, RIGHT - LEFT - 28, 32);
            let pill = RECT {
                left: pill.left,
                top: pill.top,
                right: r.right - s(14),
                bottom: pill.bottom,
            };
            match labels.get(index) {
                Some(label) => {
                    shape(dc, pill, 4, t.chip, Some(t.tile_border));
                    text(
                        dc,
                        label,
                        pill,
                        f.small_semi,
                        SOFT,
                        DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                    );
                }
                None => {
                    shape(dc, pill, 4, t.field, Some(t.divider));
                    text(
                        dc,
                        "Empty slot",
                        pill,
                        f.small,
                        DISABLED,
                        DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                    );
                }
            }
        }
    }
}

/// Bullet list explaining how the two button slots behave.
unsafe fn rules_card(dc: HDC, desktop: &Desktop, r: RECT) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        surface(dc, r, t.tile_radius + 2, t.tile, t.tile_border);
        text(
            dc,
            "HOW SLOTS WORK",
            at(r, 16, 14, 240, 14),
            f.kicker,
            MUTED,
            LEFT_LINE,
        );
        for (index, rule) in [
            "Labels are 1 to 32 characters.",
            "Links must start with https://.",
            "The RSI profile button fills the next free slot.",
        ]
        .into_iter()
        .enumerate()
        {
            let y = 40 + index as i32 * 24;
            circle(dc, r.left + s(16), r.top + s(y + 6), s(6), t.accent);
            text(
                dc,
                rule,
                at(r, 30, y, RIGHT - LEFT - 44, 18),
                f.small,
                SOFT,
                LEFT_LINE,
            );
        }
        text(
            dc,
            "Click Apply to publish changes.",
            at(r, 16, 116, 280, 14),
            f.tiny,
            MUTED,
            LEFT_LINE,
        );
    }
}

/// Mini control sample so theme, accent and corner changes are visible in place.
unsafe fn sample_strip(dc: HDC, desktop: &Desktop, r: RECT) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        surface(dc, r, t.tile_radius + 2, t.tile, t.tile_border);
        text(
            dc,
            "PREVIEW",
            at(r, 16, 20, 120, 14),
            f.kicker,
            MUTED,
            LEFT_LINE,
        );
        let primary = at(r, 92, 12, 88, 32);
        shape(dc, primary, t.button_radius, t.accent, None);
        text(
            dc,
            "Apply",
            primary,
            f.body_semi,
            ON_ACCENT,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        let secondary = at(r, 188, 12, 108, 32);
        shape(dc, secondary, t.button_radius, t.tile, Some(t.tile_border));
        text(
            dc,
            "Hide to tray",
            secondary,
            f.body_semi,
            TEXT,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        switch(dc, t, r.left + s(312), r.top + s(18), true, true);
        let pill = at(r, 366, 18, 0, 20);
        let width = chip(
            dc,
            desktop,
            RECT {
                left: pill.left,
                top: pill.top,
                right: pill.left,
                bottom: pill.bottom,
            },
            "Live on Discord",
            t.status_fill(LIVE),
            t.status_border(LIVE),
            LIVE,
        );
        let _ = width;
    }
}

/// Two-column key/value card used on the Updates page.
unsafe fn diagnostics(dc: HDC, desktop: &Desktop, r: RECT, title: &str, rows: &[(&str, &str)]) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        surface(dc, r, t.tile_radius + 2, t.tile, t.tile_border);
        text(
            dc,
            title,
            at(r, 16, 12, 240, 14),
            f.kicker,
            MUTED,
            LEFT_LINE,
        );
        for (index, (key, value)) in rows.iter().enumerate() {
            let y = 32 + index as i32 * 28;
            if index > 0 {
                line(
                    dc,
                    r.left + s(16),
                    r.top + s(y),
                    r.right - s(16),
                    r.top + s(y + 1),
                    t.divider,
                );
            }
            text(
                dc,
                key,
                at(r, 16, y + 6, 140, 18),
                f.small,
                MUTED,
                LEFT_LINE,
            );
            text(
                dc,
                value,
                RECT {
                    left: r.left + s(160),
                    top: r.top + s(y + 6),
                    right: r.right - s(16),
                    bottom: r.top + s(y + 24),
                },
                f.small_semi,
                SOFT,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
            );
        }
    }
}

unsafe fn paint_page(dc: HDC, desktop: &Desktop, top: i32) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        match desktop.page.get() {
            CARD => {
                page_header(dc, desktop, top, "Card", "Choose what appears on Discord");
                section_kicker(dc, desktop, top + 34, "SHOW ON CARD");
                section_kicker(dc, desktop, top + 244, "GENERAL");
            }
            ACTIVITY => {
                page_header(
                    dc,
                    desktop,
                    top,
                    "Activity & text",
                    "How Discord words your status",
                );
                section_kicker(dc, desktop, top + 34, "STATUS FORMAT");
                let chosen = desktop
                    .prefs
                    .lock()
                    .unwrap()
                    .activity_type
                    .clone()
                    .unwrap_or_else(|| desktop.base.activity_type.clone());
                let verb_label = VERBS
                    .iter()
                    .find(|(key, _)| *key == chosen)
                    .map_or("Playing", |(_, label)| label);
                let example =
                    format!("The word before your status (e.g. {verb_label} Star Citizen)");
                settings_group(
                    dc,
                    desktop,
                    rect(LEFT, top + 52, RIGHT - LEFT, 2 * GROUP_ROW_H),
                    &[
                        (VERB, "Discord says", example.as_str()),
                        (
                            MEMBER_LIST,
                            "Member list shows",
                            "The line next to your name in server member lists",
                        ),
                    ],
                    GROUP_ROW_H,
                );
                section_kicker(dc, desktop, top + 152, "TEXT OVERRIDES");
                settings_group(
                    dc,
                    desktop,
                    rect(LEFT, top + 170, RIGHT - LEFT, 3 * GROUP_ROW_H),
                    &[
                        (
                            GAME_NAME,
                            "Game name",
                            "Replaces Star Citizen where Discord allows it",
                        ),
                        (
                            DETAILS,
                            "Line 2",
                            "Overrides the location line (blank for auto)",
                        ),
                        (
                            CUSTOM,
                            "Line 3",
                            "Overrides the ship and contract line (blank for auto)",
                        ),
                    ],
                    GROUP_ROW_H,
                );
                if !desktop.banner.get() {
                    preview_member_list(dc, desktop, rect(LEFT, FOOTER_TOP - 78, RIGHT - LEFT, 64));
                }
            }
            IMAGES => {
                let uploading = matches!(*desktop.upload.lock().unwrap(), UploadState::Uploading);
                let total = desktop.choices().len();
                let pages = desktop.image_page_count();
                let page_num = desktop.current_image_page() + 1;
                let base_status = if uploading {
                    "Uploading image…".to_string()
                } else {
                    match desktop.selected_choice() {
                        Some(0) => "Using the default picture".to_string(),
                        Some(n) => format!("Using picture {}", n + 1),
                        None => "Using your own image".to_string(),
                    }
                };
                let status = if pages > 1 {
                    format!("{base_status} · Page {page_num} of {pages} · {total} pictures")
                } else {
                    format!("{base_status} · {total} pictures")
                };
                let custom = desktop.prefs.lock().unwrap().overrides().len();
                let status = if custom > 0 {
                    format!("{status} · {custom} custom")
                } else {
                    status
                };
                page_header(dc, desktop, top, "Custom images", &status);
                if total == 0 {
                    text(
                        dc,
                        "No wiki pictures for this one yet. Upload an image or paste an https:// link below.",
                        rect(LEFT, top + 74, RIGHT - LEFT, 20),
                        f.small,
                        MUTED,
                        LEFT_LINE,
                    );
                }
                let link = link_frame(top, image_rows(desktop));
                text(
                    dc,
                    "Click a picture to use it. Uploads are resized and hosted publicly so Discord can show them.",
                    RECT {
                        left: s(LEFT),
                        top: link.bottom + s(10),
                        right: s(RIGHT),
                        bottom: link.bottom + s(42),
                    },
                    f.small,
                    MUTED,
                    DT_LEFT | DT_WORDBREAK,
                );
            }
            BUTTONS => {
                page_header(
                    dc,
                    desktop,
                    top,
                    "Discord buttons",
                    "Up to two links on your card",
                );
                section_kicker(dc, desktop, top + 34, "CUSTOM LINKS");
                let b = buttons_box(top);
                surface(dc, b, t.tile_radius + 2, t.tile, t.tile_border);
                let buttons = desktop
                    .prefs
                    .lock()
                    .unwrap()
                    .buttons
                    .clone()
                    .unwrap_or_default();
                for index in 0..2 {
                    let slot = at(b, 0, index as i32 * 72, RIGHT - LEFT, 72);
                    if index > 0 {
                        line(
                            dc,
                            slot.left + s(14),
                            slot.top,
                            slot.right - s(14),
                            slot.top + 1,
                            t.divider,
                        );
                    }
                    text(
                        dc,
                        &format!("Button {}", index + 1),
                        at(slot, 14, 10, 200, 16),
                        f.small_semi,
                        SOFT,
                        LEFT_LINE,
                    );
                    let entry = buttons.get(index);
                    let ready = entry.is_some_and(|b| {
                        !b.label.trim().is_empty() && b.url.trim().starts_with("https://")
                    });
                    let (label, fill, border, ink) = if ready {
                        ("Ready", t.status_fill(LIVE), t.status_border(LIVE), LIVE)
                    } else if entry
                        .is_some_and(|b| !b.label.trim().is_empty() || !b.url.trim().is_empty())
                    {
                        (
                            "Needs https://",
                            t.status_fill(PAUSED),
                            t.status_border(PAUSED),
                            PAUSED,
                        )
                    } else if index == 1 && desktop.prefs.lock().unwrap().profile {
                        (
                            "RSI profile fills this",
                            t.status_fill(t.accent),
                            t.status_border(t.accent),
                            t.accent,
                        )
                    } else {
                        ("Empty", t.chip, t.tile_border, MUTED)
                    };
                    let width = width_of(dc, label, f.tiny_semi) + s(18);
                    chip(
                        dc,
                        desktop,
                        RECT {
                            left: slot.right - s(14) - width,
                            top: slot.top + s(9),
                            right: slot.right - s(14),
                            bottom: slot.top + s(25),
                        },
                        label,
                        fill,
                        border,
                        ink,
                    );
                }
                section_kicker(dc, desktop, top + 206, "ON YOUR CARD");
                let column = (RIGHT - LEFT - COLUMN_GAP) / 2;
                preview_buttons(dc, desktop, rect(LEFT, top + 224, column, 140), &buttons);
                rules_card(
                    dc,
                    desktop,
                    rect(LEFT + column + COLUMN_GAP, top + 224, column, 140),
                );
            }
            APPEARANCE => {
                page_header(
                    dc,
                    desktop,
                    top,
                    "Appearance",
                    "Themes, accent colour and corners",
                );
                section_kicker(dc, desktop, top + 34, "COLOUR SCHEME");
                section_kicker(dc, desktop, top + 128, "ACCENT COLOUR");
                section_kicker(dc, desktop, top + 192, "CORNER STYLE");
                text(
                    dc,
                    "Saved automatically. Every change applies immediately.",
                    rect(LEFT, top + 256, RIGHT - LEFT, 18),
                    f.small,
                    MUTED,
                    LEFT_LINE,
                );
                sample_strip(dc, desktop, rect(LEFT, top + 280, RIGHT - LEFT, 56));
            }
            _ => {
                page_header(dc, desktop, top, "Updates", "");
                let state = desktop.update.lock().unwrap().clone();
                let hero = updates_hero(top);
                surface(dc, hero, t.tile_radius + 2, t.tile, t.tile_border);
                let (tone, glyph, title, description) = match &state {
                    UpdateState::Idle => (
                        MUTED,
                        "\u{E895}",
                        "Not checked yet".to_string(),
                        if desktop.base.check_for_updates {
                            "Checks run at startup and every 30 minutes."
                        } else {
                            "Update checks are turned off."
                        }
                        .to_string(),
                    ),
                    UpdateState::Checking => (
                        t.accent,
                        "\u{E895}",
                        "Checking for updates…".to_string(),
                        "Asking GitHub for the latest release.".to_string(),
                    ),
                    UpdateState::UpToDate => (
                        LIVE,
                        "\u{E73E}",
                        "You're up to date".to_string(),
                        "This is the latest release.".to_string(),
                    ),
                    UpdateState::Available(release) => (
                        t.accent,
                        "\u{E896}",
                        format!("Version {} is available", release.version),
                        "Downloads, verifies and installs, then reopens.".to_string(),
                    ),
                    UpdateState::Downloading(release) => (
                        t.accent,
                        "\u{E896}",
                        format!("Downloading version {}…", release.version),
                        "The app closes and reopens when the installer finishes.".to_string(),
                    ),
                    UpdateState::Failed(error) => (
                        ERROR,
                        "\u{E7BA}",
                        "Couldn't update".to_string(),
                        error.clone(),
                    ),
                };
                let emblem = at(hero, 16, 20, 44, 44);
                shape(
                    dc,
                    emblem,
                    t.tile_radius + 4,
                    mix(t.tile, tone, 0.18),
                    Some(mix(t.tile, tone, 0.4)),
                );
                text(
                    dc,
                    glyph,
                    RECT {
                        left: emblem.left,
                        top: emblem.top,
                        right: emblem.right,
                        bottom: emblem.bottom,
                    },
                    f.glyph,
                    tone,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );
                text(
                    dc,
                    &title,
                    at(hero, 74, 18, RIGHT - LEFT - 340, 22),
                    f.details,
                    TEXT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    &description,
                    at(hero, 74, 44, RIGHT - LEFT - 340, 18),
                    f.small,
                    MUTED,
                    LEFT_LINE,
                );

                section_kicker(dc, desktop, top + 126, "ENVIRONMENT & VERIFICATION");
                let column = (RIGHT - LEFT - COLUMN_GAP) / 2;
                diagnostics(
                    dc,
                    desktop,
                    rect(LEFT, top + 144, column, 120),
                    "INSTALLATION",
                    &[
                        ("Installed", env!("CARGO_PKG_VERSION")),
                        ("Game channel", &desktop.base.channel),
                        (
                            "Update checks",
                            if desktop.base.check_for_updates {
                                "Startup · every 30 min"
                            } else {
                                "Off in scrp.toml"
                            },
                        ),
                    ],
                );
                diagnostics(
                    dc,
                    desktop,
                    rect(LEFT + column + COLUMN_GAP, top + 144, column, 120),
                    "PRIVACY",
                    &[
                        ("Release host", "GitHub (HTTPS)"),
                        ("Installer check", "SHA-256 verified"),
                        ("Data sent", "None · local log"),
                    ],
                );
                let monitor = rect(LEFT, top + 276, RIGHT - LEFT, 96);
                surface(dc, monitor, t.tile_radius + 2, t.tile, t.tile_border);
                let live = liveness(desktop);
                let (tone, label) = match live {
                    Liveness::Live => (LIVE, "Connected"),
                    Liveness::Paused => (PAUSED, "Paused"),
                    Liveness::Waiting => (MUTED, "Waiting"),
                };
                circle(dc, monitor.left + s(16), monitor.top + s(16), s(10), tone);
                text(
                    dc,
                    "PRESENCE MONITOR",
                    at(monitor, 34, 12, 200, 16),
                    f.kicker,
                    MUTED,
                    LEFT_LINE,
                );
                let width = width_of(dc, label, f.tiny_semi) + s(30);
                let pill = RECT {
                    left: monitor.right - s(16) - width,
                    top: monitor.top + s(10),
                    right: monitor.right - s(16),
                    bottom: monitor.top + s(30),
                };
                shape(
                    dc,
                    pill,
                    s(10),
                    t.status_fill(tone),
                    Some(t.status_border(tone)),
                );
                circle(dc, pill.left + s(9), pill.top + s(7), s(6), tone);
                text(
                    dc,
                    label,
                    RECT {
                        left: pill.left + s(19),
                        top: pill.top,
                        right: pill.right,
                        bottom: pill.bottom,
                    },
                    f.tiny_semi,
                    tone,
                    LEFT_LINE,
                );
                let status = desktop.status.lock().unwrap().clone();
                text(
                    dc,
                    &status,
                    at(monitor, 16, 36, RIGHT - LEFT - 32, 22),
                    f.body_semi,
                    TEXT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    &format!(
                        "Reading Game.log · polling every {}s · pushing every {}s",
                        desktop.base.poll_seconds, desktop.base.update_seconds
                    ),
                    at(monitor, 16, 62, RIGHT - LEFT - 32, 16),
                    f.tiny,
                    MUTED,
                    LEFT_LINE,
                );
            }
        }
    }
}
/// Background behind an owner-drawn button, so its rounded corners blend in.
fn behind(id: i32, t: Theme, in_banner: bool) -> u32 {
    match id {
        HIDE | SAVE | EXIT => t.footer,
        n if (NAV..NAV + PAGES.len() as i32).contains(&n) => t.rail,
        UPDATE_NOW | RELEASE_NOTES if in_banner => t.banner,
        _ => t.canvas,
    }
}

unsafe fn draw_combo_item(desktop: &Desktop, item: &DRAWITEMSTRUCT) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        let dc = item.hDC;
        let r = item.rcItem;
        let selected = item.itemState & ODS_SELECTED != 0;
        let in_edit = item.itemState & ODS_COMBOBOXEDIT != 0;
        fill(dc, r, t.field);
        if item.itemID == u32::MAX {
            return;
        }
        let len = SendMessageW(item.hwndItem, CB_GETLBTEXTLEN, item.itemID as usize, 0);
        if len <= 0 {
            return;
        }
        let mut buf = vec![0u16; len as usize + 1];
        SendMessageW(
            item.hwndItem,
            CB_GETLBTEXT,
            item.itemID as usize,
            buf.as_mut_ptr() as LPARAM,
        );
        let label = String::from_utf16_lossy(&buf[..len as usize]);
        if !in_edit && selected {
            let pill = RECT {
                left: r.left + s(4),
                top: r.top + s(3),
                right: r.right - s(4),
                bottom: r.bottom - s(3),
            };
            shape(dc, pill, t.button_radius, t.nav_selected, None);
            shape(
                dc,
                RECT {
                    left: pill.left + s(4),
                    top: pill.top + s(4),
                    right: pill.left + s(7),
                    bottom: pill.bottom - s(4),
                },
                1,
                t.accent,
                None,
            );
        }
        let pad = if in_edit { 8 } else { 14 };
        text(
            dc,
            &label,
            RECT {
                left: r.left + s(pad),
                top: r.top,
                right: r.right - s(8),
                bottom: r.bottom,
            },
            if !in_edit && selected {
                f.body_semi
            } else {
                f.body
            },
            if in_edit || selected { TEXT } else { SOFT },
            LEFT_LINE,
        );
    }
}

/// Custom-painted closed combobox so dropdowns match the dark Fluent theme instead of default Win32 chrome.
unsafe extern "system" fn paint_combo(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    font: usize,
) -> LRESULT {
    unsafe {
        if msg == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(paint_combo), id);
        }
        if msg != WM_PAINT {
            let result = DefSubclassProc(hwnd, msg, w, l);
            if matches!(msg, WM_SETFOCUS | WM_KILLFOCUS | CB_SETCURSEL) {
                InvalidateRect(hwnd, null_mut(), 0);
            }
            return result;
        }
        let parent = GetParent(hwnd);
        let ptr = GetWindowLongPtrW(parent, GWLP_USERDATA) as *const Desktop;
        let t = if ptr.is_null() {
            Theme::from_prefs(&Preferences::default())
        } else {
            (*ptr).theme.get()
        };
        let mut paint = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r);
        let ctrl_id = GetDlgCtrlID(hwnd);
        let bg = if matches!(ctrl_id, VERB | MEMBER_LIST) {
            t.tile
        } else {
            t.canvas
        };
        fill(dc, r, bg);
        let focused = windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd;
        let border = if focused { t.accent } else { t.field_border };
        shape(dc, r, t.button_radius, t.field, Some(border));
        if !focused {
            line(
                dc,
                r.left + s(4),
                r.bottom - 1,
                r.right - s(4),
                r.bottom,
                t.field_bottom,
            );
        }
        let sel = SendMessageW(hwnd, CB_GETCURSEL, 0, 0);
        if sel >= 0 {
            let len = SendMessageW(hwnd, CB_GETLBTEXTLEN, sel as usize, 0);
            if len > 0 {
                let mut buf = vec![0u16; len as usize + 1];
                SendMessageW(hwnd, CB_GETLBTEXT, sel as usize, buf.as_mut_ptr() as LPARAM);
                let label = String::from_utf16_lossy(&buf[..len as usize]);
                text(
                    dc,
                    &label,
                    RECT {
                        left: r.left + s(12),
                        top: r.top,
                        right: r.right - s(34),
                        bottom: r.bottom,
                    },
                    font as HFONT,
                    TEXT,
                    LEFT_LINE,
                );
            }
        }
        text(
            dc,
            "\u{E70D}",
            RECT {
                left: r.right - s(28),
                top: r.top,
                right: r.right - s(8),
                bottom: r.bottom,
            },
            if let Some(ptr) = Some(ptr).filter(|p| !p.is_null()) {
                (*ptr).fonts().glyph_small
            } else {
                font as HFONT
            },
            if focused { t.accent } else { MUTED },
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        EndPaint(hwnd, &paint);
        0
    }
}

unsafe fn draw_button(desktop: &Desktop, item: &DRAWITEMSTRUCT) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        let id = item.CtlID as i32;
        let dc = item.hDC;
        let r = item.rcItem;
        let pressed = item.itemState & ODS_SELECTED != 0;
        fill(dc, r, behind(id, t, desktop.banner.get()));
        let mut label = [0u16; 64];
        let len = GetWindowTextW(item.hwndItem, label.as_mut_ptr(), label.len() as i32);
        let label = String::from_utf16_lossy(&label[..len as usize]);
        let center = DT_CENTER | DT_SINGLELINE | DT_VCENTER;

        if (THEME_BTN..THEME_BTN + THEMES.len() as i32).contains(&id) {
            let index = (id - THEME_BTN) as usize;
            let (key, name, canvas_c, rail_c, tile_c) = THEMES[index];
            let selected = desktop.prefs.lock().unwrap().theme == key;
            let hot = desktop.hot.get() == item.hwndItem;
            shape(
                dc,
                r,
                t.tile_radius + 2,
                if selected {
                    mix(t.tile, t.accent, 0.10)
                } else if hot {
                    t.tile_hover
                } else {
                    t.tile
                },
                Some(if selected {
                    t.accent
                } else if hot {
                    t.field_bottom
                } else {
                    t.tile_border
                }),
            );
            // Mini window swatch: canvas, rail, a tile and the accent.
            let sw = at(r, 12, 14, 40, 40);
            shape(dc, sw, 4, canvas_c, Some(t.tile_border));
            shape(dc, at(sw, 1, 1, 11, 38), 3, rail_c, None);
            shape(dc, at(sw, 16, 8, 20, 10), 2, tile_c, None);
            shape(dc, at(sw, 16, 22, 12, 8), 2, t.accent, None);
            text(
                dc,
                name,
                at(r, 62, 0, RIGHT - LEFT - 200, 68),
                if selected { f.body_semi } else { f.body },
                if selected || hot { TEXT } else { SOFT },
                LEFT_LINE,
            );
            if selected {
                let check = at(r, 122, 26, 18, 18);
                shape(dc, check, 9, t.accent, None);
                text(
                    dc,
                    "\u{E73E}",
                    check,
                    f.glyph_small,
                    ON_ACCENT,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );
            }
        } else if (ACCENT_BTN..ACCENT_BTN + ACCENTS.len() as i32).contains(&id) {
            let index = (id - ACCENT_BTN) as usize;
            let (key, name, color, _, _) = ACCENTS[index];
            let selected = desktop.prefs.lock().unwrap().accent == key;
            let hot = desktop.hot.get() == item.hwndItem;
            shape(
                dc,
                r,
                t.button_radius + 1,
                if selected {
                    mix(t.tile, color, 0.12)
                } else if hot {
                    t.tile_hover
                } else {
                    t.tile
                },
                Some(if selected { color } else { t.tile_border }),
            );
            circle(
                dc,
                at(r, 12, 12, 14, 14).left,
                at(r, 12, 12, 14, 14).top,
                s(14),
                color,
            );
            text(
                dc,
                name,
                at(r, 32, 0, RIGHT - LEFT - 44, 38),
                if selected { f.small_semi } else { f.small },
                if selected || hot { TEXT } else { SOFT },
                LEFT_LINE,
            );
        } else if (CORNER_BTN..CORNER_BTN + 2).contains(&id) {
            let sharp = id == CORNER_BTN + 1;
            let selected = desktop.prefs.lock().unwrap().sharp_corners == sharp;
            let hot = desktop.hot.get() == item.hwndItem;
            shape(
                dc,
                r,
                if sharp { 2 } else { 8 },
                if selected {
                    mix(t.tile, t.accent, 0.12)
                } else if hot {
                    t.tile_hover
                } else {
                    t.tile
                },
                Some(if selected { t.accent } else { t.tile_border }),
            );
            text(
                dc,
                &label,
                r,
                if selected { f.body_semi } else { f.body },
                if selected { TEXT } else { SOFT },
                center,
            );
        } else if (CHOICE..CHOICE + MAX_CHOICES as i32).contains(&id) {
            // Picture tile: the wiki image, a number chip, and a highlight when in use.
            let (offset, _) = desktop.visible_choices();
            let index = offset + (id - CHOICE) as usize;
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let mask = if desktop.current_target().mission {
                Mask::Circle
            } else {
                Mask::Rounded(t.tile_radius.max(2) as u32)
            };
            let picture = desktop.choices().get(index).and_then(|url| {
                let inset = if desktop.selected_choice() == Some(index) {
                    3
                } else {
                    2
                };
                want_thumbnail(
                    desktop,
                    GetParent(item.hwndItem),
                    url,
                    (w as u32 - inset * 2, h as u32 - inset * 2),
                    mask,
                )
            });
            let selected = desktop.selected_choice() == Some(index);
            let hot = desktop.hot.get() == item.hwndItem;
            shape(
                dc,
                r,
                t.tile_radius,
                t.tile,
                Some(if selected {
                    t.accent
                } else if hot {
                    t.field_bottom
                } else {
                    t.tile_border
                }),
            );
            let inset = if selected { s(3) } else { s(2) };
            let inner = RECT {
                left: r.left + inset,
                top: r.top + inset,
                right: r.right - inset,
                bottom: r.bottom - inset,
            };
            match picture {
                Some(thumb) => {
                    let _ = &inner;
                    draw_thumbnail(dc, &thumb, r.left + inset, r.top + inset)
                }
                None => {
                    shape(dc, inner, t.tile_radius - 1, t.nav_hover, None);
                }
            }
            if index == 0 {
                let width = width_of(dc, "Default", f.tiny_semi) + s(12);
                let badge = RECT {
                    left: r.left + s(5),
                    top: r.bottom - s(21),
                    right: r.left + s(5) + width,
                    bottom: r.bottom - s(5),
                };
                shape(dc, badge, 4, rgb(0x12, 0x12, 0x14), None);
                text(
                    dc,
                    "Default",
                    badge,
                    f.tiny_semi,
                    TEXT,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );
            }
            if selected {
                let check = RECT {
                    left: r.right - s(22),
                    top: r.top + s(4),
                    right: r.right - s(4),
                    bottom: r.top + s(22),
                };
                shape(dc, check, 9, t.accent, None);
                text(
                    dc,
                    "\u{E73E}",
                    check,
                    f.glyph_small,
                    ON_ACCENT,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );
            }
        } else if (NAV..NAV + PAGES.len() as i32).contains(&id) {
            let page = (id - NAV) as usize;
            let selected = page == desktop.page.get();
            let hot = desktop.hot.get() == item.hwndItem;
            if selected || hot {
                shape(
                    dc,
                    r,
                    t.button_radius,
                    if selected {
                        t.nav_selected
                    } else {
                        t.nav_hover
                    },
                    selected.then(|| mix(t.rail, t.accent, 0.22)),
                );
            }
            if selected {
                shape(dc, at(r, 0, 8, 3, 18), 2, t.accent, None);
            }
            text(
                dc,
                NAV_GLYPHS[page],
                RECT {
                    left: r.left + s(14),
                    top: r.top,
                    right: r.left + s(36),
                    bottom: r.bottom,
                },
                f.glyph,
                if selected {
                    t.accent
                } else if hot {
                    SOFT
                } else {
                    MUTED
                },
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );
            text(
                dc,
                &label,
                RECT {
                    left: r.left + s(42),
                    top: r.top,
                    right: r.right - s(10),
                    bottom: r.bottom,
                },
                if selected { f.body_semi } else { f.body },
                if selected || hot { TEXT } else { SOFT },
                LEFT_LINE,
            );
            if page == UPDATES_PAGE
                && matches!(*desktop.update.lock().unwrap(), UpdateState::Available(_))
            {
                circle(dc, r.right - s(18), r.top + s(13), s(7), t.accent);
            }
        } else if matches!(id, RELEASE_NOTES | UPDATE_NOTES) {
            let ink = if desktop.hot.get() == item.hwndItem {
                t.accent_hover
            } else {
                t.accent
            };
            let measured = text(dc, &label, r, f.small_semi, ink, center);
            let y = measured.bottom + (r.bottom - r.top - (measured.bottom - measured.top)) / 2 - 1;
            let width = measured.right - measured.left;
            let x = r.left + (r.right - r.left - width) / 2;
            line(
                dc,
                x,
                y.min(r.bottom - 3),
                x + width,
                y.min(r.bottom - 3) + 1,
                ink,
            );
        } else {
            let disabled = item.itemState & ODS_DISABLED != 0;
            let hot = desktop.hot.get() == item.hwndItem && !disabled;
            let available = matches!(*desktop.update.lock().unwrap(), UpdateState::Available(_));
            let primary = (id == SAVE && desktop.dirty.get())
                || id == UPDATE_NOW
                || (id == UPDATE_ACTION && available);
            if primary {
                let fill = if disabled {
                    mix(t.tile, t.accent, 0.28)
                } else if pressed {
                    t.accent_pressed
                } else if hot {
                    t.accent_hover
                } else {
                    t.accent
                };
                shape(dc, r, t.button_radius, fill, None);
                text(
                    dc,
                    &label,
                    r,
                    f.body_semi,
                    if disabled { DISABLED } else { ON_ACCENT },
                    center,
                );
            } else if id == EXIT {
                shape(
                    dc,
                    r,
                    t.button_radius,
                    if pressed {
                        t.tile_pressed
                    } else if hot {
                        t.tile_hover
                    } else {
                        t.footer
                    },
                    Some(if hot { t.field_bottom } else { t.tile_border }),
                );
                text(
                    dc,
                    &label,
                    RECT {
                        left: r.left + s(12),
                        top: r.top,
                        right: r.right,
                        bottom: r.bottom,
                    },
                    f.body_semi,
                    if hot { TEXT } else { MUTED },
                    LEFT_LINE,
                );
            } else {
                let fill = if disabled {
                    t.canvas
                } else if pressed {
                    t.tile_pressed
                } else if hot {
                    t.tile_hover
                } else {
                    t.tile
                };
                shape(
                    dc,
                    r,
                    t.button_radius,
                    fill,
                    Some(if disabled {
                        t.divider
                    } else if hot {
                        t.field_bottom
                    } else {
                        t.tile_border
                    }),
                );
                text(
                    dc,
                    &label,
                    r,
                    f.body_semi,
                    if disabled { DISABLED } else { TEXT },
                    center,
                );
            }
        }
        if item.itemState & ODS_FOCUS != 0
            && item.itemState & ODS_NOFOCUSRECT == 0
            && desktop.hot.get() != item.hwndItem
            && !(CHOICE..CHOICE + MAX_CHOICES as i32).contains(&id)
        {
            focus_frame(dc, r, t.accent);
        }
    }
}

/// Toggle tile: label on the left, Fluent switch on the right.
/// Two-pixel focus frame drawn along the inside edge; never paints over the content.
unsafe fn focus_frame(dc: HDC, r: RECT, color: u32) {
    unsafe {
        let edge = s(2);
        let inset = s(2);
        let (left, top) = (r.left + inset, r.top + inset);
        let (right, bottom) = (r.right - inset, r.bottom - inset);
        for frame in [
            RECT {
                left,
                top,
                right,
                bottom: top + edge,
            },
            RECT {
                left,
                top: bottom - edge,
                right,
                bottom,
            },
            RECT {
                left,
                top,
                right: left + edge,
                bottom,
            },
            RECT {
                left: right - edge,
                top,
                right,
                bottom,
            },
        ] {
            fill(dc, frame, color);
        }
    }
}

unsafe extern "system" fn paint_toggle(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    font: usize,
) -> LRESULT {
    unsafe {
        if msg == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(paint_toggle), id);
        }
        if msg != WM_PAINT {
            let result = DefSubclassProc(hwnd, msg, w, l);
            if matches!(
                msg,
                BM_SETCHECK
                    | BM_SETSTATE
                    | WM_SETFOCUS
                    | WM_KILLFOCUS
                    | WM_MOUSEMOVE
                    | WM_MOUSELEAVE
            ) {
                let parent = GetParent(hwnd);
                let ptr = GetWindowLongPtrW(parent, GWLP_USERDATA) as *const Desktop;
                track_hover(hwnd, msg, ptr);
                InvalidateRect(hwnd, null_mut(), 0);
            }
            if msg == WM_SETCURSOR {
                SetCursor(LoadCursorW(null_mut(), IDC_HAND));
                return 1;
            }
            return result;
        }
        let parent = GetParent(hwnd);
        let ptr = GetWindowLongPtrW(parent, GWLP_USERDATA) as *const Desktop;
        let t = if ptr.is_null() {
            Theme::from_prefs(&Preferences::default())
        } else {
            (*ptr).theme.get()
        };
        let mut paint = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r);
        let enabled = IsWindowEnabled(hwnd) != 0;
        let on = SendMessageW(hwnd, BM_GETCHECK, 0, 0) == 1;
        let hovered = !ptr.is_null() && (*ptr).hot.get() == hwnd;

        fill(dc, r, t.canvas);
        let fill_color = if !enabled {
            t.canvas
        } else if hovered {
            t.tile_hover
        } else {
            t.tile
        };
        shape(
            dc,
            r,
            t.tile_radius,
            fill_color,
            Some(if hovered {
                t.field_bottom
            } else {
                t.tile_border
            }),
        );
        line(
            dc,
            r.left + s(t.tile_radius.max(8)),
            r.top + 1,
            r.right - s(t.tile_radius.max(8)),
            r.top + 2,
            mix(fill_color, 0xff_ff_ff, 0.05),
        );

        let mut label = [0u16; 64];
        let len = GetWindowTextW(hwnd, label.as_mut_ptr(), label.len() as i32);
        let label = String::from_utf16_lossy(&label[..len as usize]);
        let title_font = if ptr.is_null() {
            font as HFONT
        } else {
            (*ptr).fonts().body_semi
        };
        let detail_font = if ptr.is_null() {
            font as HFONT
        } else {
            (*ptr).fonts().tiny
        };
        text(
            dc,
            &label,
            at(r, 14, 6, 400, 16),
            title_font,
            if enabled { TEXT } else { DISABLED },
            LEFT_LINE,
        );
        let detail = if ptr.is_null() {
            ""
        } else {
            toggle_detail(GetDlgCtrlID(hwnd))
        };
        if !detail.is_empty() {
            text(
                dc,
                detail,
                at(r, 14, 23, 460, 14),
                detail_font,
                MUTED,
                LEFT_LINE,
            );
        }
        switch(dc, t, r.right - s(54), r.top + s(11), on, enabled);

        if SendMessageW(hwnd, BM_GETSTATE, 0, 0) & BST_FOCUS as isize != 0 && !hovered {
            focus_frame(dc, r, t.accent);
        }
        EndPaint(hwnd, &paint);
        0
    }
}

/// Repaints only when something visible changed; relayouts when the banner toggles.
unsafe fn refresh(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        let upload_result = {
            let mut slot = desktop.upload.lock().unwrap();
            match &*slot {
                UploadState::Done { .. } | UploadState::Failed(_) => {
                    Some(std::mem::replace(&mut *slot, UploadState::Idle))
                }
                _ => None,
            }
        };
        if let Some(result) = upload_result {
            SetDlgItemTextW(hwnd, UPLOAD_IMAGE, wide("Upload image").as_ptr());
            EnableWindow(GetDlgItem(hwnd, UPLOAD_IMAGE), 1);
            InvalidateRect(GetDlgItem(hwnd, UPLOAD_IMAGE), null_mut(), 0);
            match result {
                UploadState::Done { mission, key, url } => {
                    set_picture_for(hwnd, desktop, mission, &key, Some(url));
                }
                UploadState::Failed(error) => {
                    desktop.painted.borrow_mut().clear();
                    InvalidateRect(hwnd, null_mut(), 0);
                    show_error(&error);
                }
                _ => {}
            }
        }
        let uploading = matches!(*desktop.upload.lock().unwrap(), UploadState::Uploading);
        let state = desktop.update.lock().unwrap().clone();
        if let UpdateState::Available(ref release) = state {
            let should_notify =
                desktop.notified_update.borrow().as_deref() != Some(&release.version);
            if should_notify {
                *desktop.notified_update.borrow_mut() = Some(release.version.clone());
                notify_update(hwnd, desktop, &release.version);
            }
        }
        let banner = matches!(
            state,
            UpdateState::Available(_) | UpdateState::Downloading(_)
        );
        if banner != desktop.banner.get() {
            desktop.banner.set(banner);
            layout(hwnd, desktop);
        }
        let card = desktop.preview.lock().unwrap().clone();
        let signature = format!(
            "{:?}|{}|{:?}|{}|{}|{}",
            card.as_ref().map(|c| (
                &c.details,
                &c.state,
                &c.large_image,
                &c.small_image,
                elapsed(c.started)
            )),
            desktop.status.lock().unwrap(),
            liveness(desktop) == Liveness::Live,
            desktop.thumbnails.lock().unwrap().len(),
            uploading,
            match &state {
                UpdateState::Idle => "idle".to_string(),
                UpdateState::Checking => "checking".into(),
                UpdateState::UpToDate => "current".into(),
                UpdateState::Available(r) => format!("available {}", r.version),
                UpdateState::Downloading(r) => format!("downloading {}", r.version),
                UpdateState::Failed(e) => e.clone(),
            },
        );
        if *desktop.painted.borrow() != signature {
            *desktop.painted.borrow_mut() = signature;
            let label = match &state {
                UpdateState::Available(r) if r.installer_url.is_some() && r.sums_url.is_some() => {
                    "Update now"
                }
                UpdateState::Available(_) => "Open release page",
                UpdateState::Downloading(_) | UpdateState::Checking => "Working…",
                _ => "Check now",
            };
            SetDlgItemTextW(hwnd, UPDATE_ACTION, wide(label).as_ptr());
            let busy = matches!(state, UpdateState::Downloading(_) | UpdateState::Checking);
            EnableWindow(GetDlgItem(hwnd, UPDATE_ACTION), (!busy) as i32);
            EnableWindow(GetDlgItem(hwnd, UPDATE_NOW), (!busy) as i32);
            let has_notes = matches!(
                state,
                UpdateState::Available(_) | UpdateState::Downloading(_)
            );
            ShowWindow(
                GetDlgItem(hwnd, UPDATE_NOTES),
                if has_notes && desktop.page.get() == UPDATES_PAGE {
                    SW_SHOWNA
                } else {
                    SW_HIDE
                },
            );
            InvalidateRect(hwnd, null_mut(), 0);
            for index in 0..PAGES.len() as i32 {
                InvalidateRect(GetDlgItem(hwnd, NAV + index), null_mut(), 0);
            }
            for id in [UPDATE_ACTION, UPDATE_NOW] {
                InvalidateRect(GetDlgItem(hwnd, id), null_mut(), 0);
            }
        }
    }
}

unsafe fn start_update(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        let state = desktop.update.lock().unwrap().clone();
        match state {
            UpdateState::Available(release)
                if release.installer_url.is_some() && release.sums_url.is_some() =>
            {
                *desktop.update.lock().unwrap() = UpdateState::Downloading(release.clone());
                let shared = Arc::clone(&desktop.update);
                let handle = hwnd as isize;
                std::thread::spawn(move || {
                    match update::download(&release).and_then(|path| update::launch(&path)) {
                        // The installer closes this app; quitting first clears presence cleanly.
                        Ok(()) => {
                            PostMessageW(handle as HWND, WM_COMMAND, EXIT as usize, 0);
                        }
                        Err(error) => {
                            *shared.lock().unwrap() = UpdateState::Failed(format!("{error:#}"));
                            request_refresh(handle);
                        }
                    }
                });
                refresh(hwnd, desktop);
            }
            UpdateState::Available(release) => open(&release.notes_url),
            UpdateState::Downloading(_) | UpdateState::Checking => {}
            _ => check_for_update(Arc::clone(&desktop.update), hwnd as isize),
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Desktop;
        if pointer.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let desktop = &*pointer;
        match msg {
            WM_ERASEBKGND => return 1,
            WM_PAINT => paint(hwnd, desktop),
            WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let dc = w as HDC;
                SetBkColor(dc, desktop.theme.get().field);
                SetTextColor(dc, TEXT);
                return desktop.field.get() as LRESULT;
            }
            WM_DRAWITEM => {
                let item = &*(l as *const DRAWITEMSTRUCT);
                if item.CtlType == ODT_COMBOBOX {
                    draw_combo_item(desktop, item);
                } else {
                    draw_button(desktop, item);
                }
                return 1;
            }
            WM_CREATE => {
                set_scale(preferred_scale(hwnd));
                fit_client(hwnd);
                build_controls(hwnd, desktop);
                desktop.tray_ready.set(tray(hwnd, desktop.icon, NIM_ADD));
                SetTimer(hwnd, 1, 500, None);
            }
            WM_DPICHANGED => {
                set_scale(preferred_scale(hwnd));
                let suggested = &*(l as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                create_fonts(desktop);
                apply_fonts(hwnd, desktop);
                desktop.painted.borrow_mut().clear();
                desktop.buffer.set(BackBuffer::EMPTY);
                layout(hwnd, desktop);
                InvalidateRect(hwnd, null_mut(), 0);
            }
            WM_TIMER | REFRESH => refresh(hwnd, desktop),
            WM_CLOSE => hide(hwnd, desktop),
            WM_SIZE if w == SIZE_MINIMIZED as usize && desktop.tray_ready.get() => {
                ShowWindow(hwnd, SW_HIDE);
            }
            TRAY if l as u32 == WM_LBUTTONUP => {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
            TRAY if l as u32 == NIN_BALLOONUSERCLICK => {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                desktop.page.set(UPDATES_PAGE);
                desktop.painted.borrow_mut().clear();
                layout(hwnd, desktop);
                refresh(hwnd, desktop);
                for index in 0..PAGES.len() as i32 {
                    InvalidateRect(GetDlgItem(hwnd, NAV + index), null_mut(), 0);
                }
            }
            TRAY if l as u32 == WM_RBUTTONUP => {
                let menu = CreatePopupMenu();
                let enabled = desktop.prefs.lock().unwrap().enabled;
                AppendMenuW(menu, MF_STRING, HIDE as usize, windows_sys::w!("Open"));
                AppendMenuW(
                    menu,
                    MF_STRING,
                    ENABLED as usize,
                    wide(if enabled {
                        "Pause sharing"
                    } else {
                        "Resume sharing"
                    })
                    .as_ptr(),
                );
                AppendMenuW(menu, MF_STRING, EXIT as usize, windows_sys::w!("Quit"));
                let mut point = POINT::default();
                GetCursorPos(&mut point);
                SetForegroundWindow(hwnd);
                let selection = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    point.x,
                    point.y,
                    0,
                    hwnd,
                    null_mut(),
                );
                DestroyMenu(menu);
                match selection {
                    HIDE => {
                        ShowWindow(hwnd, SW_RESTORE);
                    }
                    ENABLED => {
                        SendDlgItemMessageW(hwnd, ENABLED, BM_SETCHECK, (!enabled) as usize, 0);
                        update_prefs(hwnd, desktop, ENABLED);
                    }
                    EXIT => {
                        PostMessageW(hwnd, WM_COMMAND, EXIT as usize, 0);
                    }
                    _ => {}
                }
                PostMessageW(hwnd, WM_NULL, 0, 0);
            }
            WM_COMMAND => {
                let id = (w & 0xffff) as i32;
                let code = ((w >> 16) & 0xffff) as u32;
                if code == EN_CHANGE
                    && (matches!(id, GAME_NAME | DETAILS | CUSTOM)
                        || LINK_FIELDS
                            .iter()
                            .any(|&(label, url)| id == label || id == url))
                {
                    let dirty = edits_dirty(hwnd, desktop);
                    if dirty != desktop.dirty.get() {
                        desktop.dirty.set(dirty);
                        InvalidateRect(GetDlgItem(hwnd, SAVE), null_mut(), 0);
                    }
                    return 0;
                }
                match id {
                    EXIT => {
                        DestroyWindow(hwnd);
                    }
                    HIDE => hide(hwnd, desktop),
                    n if (NAV..NAV + PAGES.len() as i32).contains(&n) => {
                        desktop.page.set((n - NAV) as usize);
                        // Images opens on the ship you're flying, when it has artwork.
                        if desktop.page.get() == IMAGES {
                            let flying = desktop
                                .preview
                                .lock()
                                .unwrap()
                                .as_ref()
                                .map(|card| card.large_text.clone());
                            if let Some(index) = flying.and_then(|name| {
                                desktop
                                    .targets
                                    .iter()
                                    .position(|t| !t.mission && t.key == name)
                            }) {
                                desktop.target.set(index);
                                SendDlgItemMessageW(hwnd, IMAGE_TARGET, CB_SETCURSEL, index, 0);
                            }
                        }
                        desktop.painted.borrow_mut().clear();
                        layout(hwnd, desktop);
                        refresh(hwnd, desktop);
                        for index in 0..PAGES.len() as i32 {
                            InvalidateRect(GetDlgItem(hwnd, NAV + index), null_mut(), 0);
                        }
                    }
                    STARTUP => {
                        if let Err(error) = set_startup(checked(hwnd, STARTUP)) {
                            SendDlgItemMessageW(
                                hwnd,
                                STARTUP,
                                BM_SETCHECK,
                                startup_enabled() as usize,
                                0,
                            );
                            show_error(&error.to_string());
                        }
                    }
                    UPDATES => {
                        update_prefs(hwnd, desktop, id);
                        if checked(hwnd, UPDATES)
                            && desktop.base.check_for_updates
                            && *desktop.update.lock().unwrap() == UpdateState::Idle
                        {
                            check_for_update(Arc::clone(&desktop.update), hwnd as isize);
                        }
                    }
                    ENABLED | LOCATION | SHIP | ART | PROFILE | CONTRACTS | NOTICES | PARTY
                    | SAVE => update_prefs(hwnd, desktop, id),
                    VERB | MEMBER_LIST if code == CBN_SELCHANGE => update_prefs(hwnd, desktop, id),
                    IMAGE_TARGET if code == CBN_SELCHANGE => {
                        if let Some(index) = selected(hwnd, IMAGE_TARGET) {
                            show_target(hwnd, desktop, index);
                        }
                    }
                    n if (CHOICE..CHOICE + MAX_CHOICES as i32).contains(&n) => {
                        let (offset, _) = desktop.visible_choices();
                        let index = offset + (n - CHOICE) as usize;
                        set_picture(hwnd, desktop, (index > 0).then(|| (index + 1).to_string()));
                    }
                    IMG_PREV => {
                        let page = desktop.current_image_page();
                        if page > 0 {
                            desktop.image_page.set(page - 1);
                            desktop.painted.borrow_mut().clear();
                            layout(hwnd, desktop);
                            invalidate_pictures(hwnd);
                        }
                    }
                    IMG_NEXT => {
                        let page = desktop.current_image_page();
                        if page + 1 < desktop.image_page_count() {
                            desktop.image_page.set(page + 1);
                            desktop.painted.borrow_mut().clear();
                            layout(hwnd, desktop);
                            invalidate_pictures(hwnd);
                        }
                    }
                    USE_DEFAULT => set_picture(hwnd, desktop, None),
                    UPLOAD_IMAGE => start_upload(hwnd, desktop),
                    USE_LINK => {
                        let link = read_text(hwnd, IMAGE_LINK);
                        if !link.starts_with("https://")
                            || link.len() > 256
                            || link.contains(char::is_whitespace)
                        {
                            show_error(
                                "Paste a picture link that starts with https:// (Discord can't show files on your PC).",
                            );
                        } else {
                            set_picture(hwnd, desktop, Some(link));
                            SetDlgItemTextW(hwnd, IMAGE_LINK, wide("").as_ptr());
                        }
                    }
                    n if (THEME_BTN..THEME_BTN + THEMES.len() as i32).contains(&n) => {
                        let key = THEMES[(n - THEME_BTN) as usize].0;
                        let mut next = desktop.prefs.lock().unwrap().clone();
                        next.theme = key.to_string();
                        if commit(desktop, next) {
                            apply_theme(hwnd, desktop);
                        }
                    }
                    n if (ACCENT_BTN..ACCENT_BTN + ACCENTS.len() as i32).contains(&n) => {
                        let key = ACCENTS[(n - ACCENT_BTN) as usize].0;
                        let mut next = desktop.prefs.lock().unwrap().clone();
                        next.accent = key.to_string();
                        if commit(desktop, next) {
                            apply_theme(hwnd, desktop);
                        }
                    }
                    n if (CORNER_BTN..CORNER_BTN + 2).contains(&n) => {
                        let mut next = desktop.prefs.lock().unwrap().clone();
                        next.sharp_corners = n == CORNER_BTN + 1;
                        if commit(desktop, next) {
                            apply_theme(hwnd, desktop);
                        }
                    }
                    UPDATE_NOW | UPDATE_ACTION => start_update(hwnd, desktop),
                    RELEASE_NOTES | UPDATE_NOTES => {
                        let state = desktop.update.lock().unwrap().clone();
                        if let UpdateState::Available(release) | UpdateState::Downloading(release) =
                            state
                        {
                            open(&release.notes_url);
                        }
                    }
                    _ => {}
                }
            }
            WM_DESTROY => {
                tray(hwnd, desktop.icon, NIM_DELETE);
                if let Some(fonts) = desktop.fonts.replace(None) {
                    delete_fonts(fonts);
                }
                DeleteObject(desktop.field.get());
                let buffer = desktop.buffer.get();
                if !buffer.dc.is_null() {
                    DeleteObject(buffer.bitmap);
                    DeleteDC(buffer.dc);
                }
                desktop.buffer.set(BackBuffer::EMPTY);
                PostQuitMessage(0);
            }
            _ if msg == desktop.taskbar_created => {
                desktop.tray_ready.set(tray(hwnd, desktop.icon, NIM_ADD));
            }
            _ => return DefWindowProcW(hwnd, msg, w, l),
        }
        0
    }
}

// ── Preferences ─────────────────────────────────────────────────────────────

unsafe fn checked(hwnd: HWND, id: i32) -> bool {
    unsafe { SendDlgItemMessageW(hwnd, id, BM_GETCHECK, 0, 0) == 1 }
}

unsafe fn read_text(hwnd: HWND, id: i32) -> String {
    unsafe {
        let mut text = [0; 513];
        let len = GetDlgItemTextW(hwnd, id, text.as_mut_ptr(), text.len() as i32);
        String::from_utf16_lossy(&text[..len as usize])
            .trim()
            .to_owned()
    }
}

/// True when any editable text field differs from the saved preferences.
unsafe fn edits_dirty(hwnd: HWND, desktop: &Desktop) -> bool {
    unsafe {
        let prefs = desktop.prefs.lock().unwrap();
        if read_text(hwnd, GAME_NAME) != prefs.game_name.trim()
            || read_text(hwnd, DETAILS) != prefs.details.trim()
            || read_text(hwnd, CUSTOM) != prefs.custom.trim()
        {
            return true;
        }
        let buttons = prefs.buttons.as_deref().unwrap_or(&[]);
        for (index, (label, url)) in LINK_FIELDS.into_iter().enumerate() {
            let saved = buttons.get(index);
            let (saved_label, saved_url) =
                saved.map_or(("", ""), |b| (b.label.trim(), b.url.trim()));
            if read_text(hwnd, label) != saved_label || read_text(hwnd, url) != saved_url {
                return true;
            }
        }
        false
    }
}

unsafe fn selected(hwnd: HWND, id: i32) -> Option<usize> {
    unsafe { usize::try_from(SendDlgItemMessageW(hwnd, id, CB_GETCURSEL, 0, 0)).ok() }
}

/// Applies the saved theme/accent/corners live to the window, title bar, and all child controls.
unsafe fn apply_theme(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        let t = Theme::from_prefs(&desktop.prefs.lock().unwrap());
        desktop.theme.set(t);
        let old_brush = desktop.field.replace(CreateSolidBrush(t.field));
        DeleteObject(old_brush);
        for (attribute, color) in [
            (DWMWA_CAPTION_COLOR, t.caption),
            (DWMWA_BORDER_COLOR, t.tile_border),
            (DWMWA_TEXT_COLOR, SOFT),
        ] {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                (&color as *const u32).cast(),
                size_of::<u32>() as u32,
            );
        }
        desktop.painted.borrow_mut().clear();
        RedrawWindow(
            hwnd,
            std::ptr::null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
    }
}

/// Saves `next` and makes it current, or reports the error and returns false.
fn commit(desktop: &Desktop, next: Preferences) -> bool {
    match save(&desktop.file, &next) {
        Ok(()) => {
            *desktop.prefs.lock().unwrap() = next;
            true
        }
        Err(error) => {
            show_error(&format!("Could not save preferences: {error}"));
            false
        }
    }
}

/// Shows the pictures for target `index` (a contract type or ship).
unsafe fn show_target(hwnd: HWND, desktop: &Desktop, index: usize) {
    unsafe {
        if index >= desktop.targets.len() {
            return;
        }
        desktop.target.set(index);
        desktop.image_page.set(0);
        SendDlgItemMessageW(hwnd, IMAGE_TARGET, CB_SETCURSEL, index, 0);
        desktop.painted.borrow_mut().clear();
        layout(hwnd, desktop);
        invalidate_pictures(hwnd);
    }
}

unsafe fn invalidate_pictures(hwnd: HWND) {
    unsafe {
        for index in 0..MAX_CHOICES {
            InvalidateRect(GetDlgItem(hwnd, CHOICE + index as i32), null_mut(), 0);
        }
        InvalidateRect(GetDlgItem(hwnd, NAV + IMAGES as i32), null_mut(), 0);
        InvalidateRect(hwnd, null_mut(), 0);
    }
}

/// Opens a native file dialog to pick a PNG, JPEG, or WebP image.
unsafe fn pick_image_file(hwnd: HWND) -> Option<PathBuf> {
    unsafe {
        let mut buffer = vec![0u16; 32_768];
        let filter: Vec<u16> = "Images (*.png;*.jpg;*.jpeg;*.webp)\0*.png;*.jpg;*.jpeg;*.webp\0All files (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = hwnd;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buffer.as_mut_ptr();
        ofn.nMaxFile = buffer.len() as u32;
        ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        if GetOpenFileNameW(&mut ofn) == 0 {
            let err = CommDlgExtendedError();
            if err != 0 {
                show_error(&format!("Could not open file picker (error {err:#x})."));
            }
            return None;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
    }
}

/// Picks a local image, resizes it, uploads it to catbox.moe on a worker thread, and saves its URL.
unsafe fn start_upload(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        if matches!(*desktop.upload.lock().unwrap(), UploadState::Uploading) {
            return;
        }
        let Some(path) = pick_image_file(hwnd) else {
            return;
        };
        let target = desktop.current_target();
        let mission = target.mission;
        let key = target.key.clone();
        *desktop.upload.lock().unwrap() = UploadState::Uploading;
        SetDlgItemTextW(hwnd, UPLOAD_IMAGE, wide("Uploading…").as_ptr());
        EnableWindow(GetDlgItem(hwnd, UPLOAD_IMAGE), 0);
        InvalidateRect(GetDlgItem(hwnd, UPLOAD_IMAGE), null_mut(), 0);
        let shared = Arc::clone(&desktop.upload);
        let handle = hwnd as isize;
        std::thread::spawn(move || {
            let outcome = (|| -> anyhow::Result<String> {
                let meta = std::fs::metadata(&path)
                    .with_context(|| format!("Could not read {}", path.display()))?;
                anyhow::ensure!(meta.len() <= 20 << 20, "Image file is larger than 20 MB");
                let raw = std::fs::read(&path)
                    .with_context(|| format!("Could not read {}", path.display()))?;
                let png = thumbnail::prepare_upload(&raw)?;
                net::upload_image(&png)
            })();
            *shared.lock().unwrap() = match outcome {
                Ok(url) => UploadState::Done { mission, key, url },
                Err(error) => UploadState::Failed(format!("Could not upload image: {error:#}")),
            };
            request_refresh(handle);
        });
        refresh(hwnd, desktop);
    }
}

/// Saves the picture for the selected target: a wiki choice number, a link, or the default.
unsafe fn set_picture(hwnd: HWND, desktop: &Desktop, value: Option<String>) {
    unsafe {
        let target = desktop.current_target();
        set_picture_for(hwnd, desktop, target.mission, &target.key.clone(), value);
    }
}

unsafe fn set_picture_for(
    hwnd: HWND,
    desktop: &Desktop,
    mission: bool,
    key: &str,
    value: Option<String>,
) {
    unsafe {
        let mut next = desktop.prefs.lock().unwrap().clone();
        let (mine, file) = if mission {
            (&mut next.mission_images, &desktop.base.mission_images)
        } else {
            (&mut next.ship_images, &desktop.base.ship_images)
        };
        mine.retain(|k, _| !k.trim().eq_ignore_ascii_case(key));
        // "Default" must still win over a picture chosen in scrp.toml.
        let value = value.or_else(|| find_ci(file, key).map(|_| "1".to_string()));
        if let Some(value) = value {
            mine.insert(key.to_string(), value);
        }
        if commit(desktop, next) {
            desktop.painted.borrow_mut().clear();
            invalidate_pictures(hwnd);
        }
    }
}

unsafe fn update_prefs(hwnd: HWND, desktop: &Desktop, id: i32) {
    unsafe {
        let old = desktop.prefs.lock().unwrap().clone();
        let mut buttons = old.buttons.clone();
        if id == SAVE {
            let links: Vec<_> = LINK_FIELDS
                .into_iter()
                .map(|(label, url)| CardButton {
                    label: read_text(hwnd, label),
                    url: read_text(hwnd, url),
                })
                .filter(|b| !b.label.is_empty() || !b.url.is_empty())
                .collect();
            if let Err(error) = (Config {
                buttons: links.clone(),
                ..Config::default()
            })
            .validate()
            {
                show_error(&error.to_string());
                return;
            }
            buttons = Some(links);
        }
        let next = Preferences {
            enabled: checked(hwnd, ENABLED),
            location: checked(hwnd, LOCATION),
            ship: checked(hwnd, SHIP),
            artwork: checked(hwnd, ART),
            profile: checked(hwnd, PROFILE),
            contracts: checked(hwnd, CONTRACTS),
            notices: checked(hwnd, NOTICES),
            party: checked(hwnd, PARTY),
            updates: checked(hwnd, UPDATES),
            custom: if id == SAVE {
                read_text(hwnd, CUSTOM)
            } else {
                old.custom.clone()
            },
            details: if id == SAVE {
                read_text(hwnd, DETAILS)
            } else {
                old.details.clone()
            },
            game_name: if id == SAVE {
                read_text(hwnd, GAME_NAME)
            } else {
                old.game_name.clone()
            },
            buttons,
            activity_type: selected(hwnd, VERB)
                .and_then(|i| VERBS.get(i))
                .map(|(key, _)| key.to_string())
                .or(old.activity_type.clone()),
            status_display: selected(hwnd, MEMBER_LIST)
                .and_then(|i| MEMBER_LIST_OPTIONS.get(i))
                .map(|(key, _)| key.to_string())
                .or(old.status_display.clone()),
            ship_images: old.ship_images.clone(),
            mission_images: old.mission_images.clone(),
            theme: old.theme.clone(),
            accent: old.accent.clone(),
            sharp_corners: old.sharp_corners,
        };
        let saved = commit(desktop, next);
        if id == SAVE && saved {
            desktop.dirty.set(false);
            InvalidateRect(GetDlgItem(hwnd, SAVE), null_mut(), 0);
        }
        if !saved && id != SAVE {
            let previous = match id {
                ENABLED => old.enabled,
                LOCATION => old.location,
                SHIP => old.ship,
                ART => old.artwork,
                PROFILE => old.profile,
                CONTRACTS => old.contracts,
                NOTICES => old.notices,
                PARTY => old.party,
                UPDATES => old.updates,
                _ => return,
            };
            SendDlgItemMessageW(hwnd, id, BM_SETCHECK, previous as usize, 0);
        }
        desktop.painted.borrow_mut().clear();
        refresh(hwnd, desktop);
    }
}
