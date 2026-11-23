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
    sync::Mutex,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::*, Registry::*, Threading::*},
    UI::{
        Controls::{Dialogs::*, *},
        Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
        Shell::*,
        WindowsAndMessaging::*,
    },
};

type Pcwstr = *const u16;

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
/// Picture tiles on the Images page (up to 18: three rows of six).
const CHOICE: i32 = 400;
const MAX_CHOICES: usize = 18;
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
const TEXT: u32 = rgb(0xf3, 0xf3, 0xf3);
const HEADING: u32 = rgb(0xe6, 0xe6, 0xe6);
const SOFT: u32 = rgb(0xc8, 0xc8, 0xc8);
const STATE_TEXT: u32 = rgb(0xb4, 0xb4, 0xb4);
const OFF_TEXT: u32 = rgb(0xa4, 0xa4, 0xa4);
const MUTED: u32 = rgb(0x96, 0x96, 0x96);
const FAINT: u32 = rgb(0x7c, 0x7c, 0x7c);
const ON_ACCENT: u32 = rgb(0x11, 0x11, 0x11);
const GREEN: u32 = rgb(0x6c, 0xcb, 0x5f);
const BANNER_TEXT: u32 = rgb(0xe8, 0xf3, 0xfb);

#[derive(Clone, Copy)]
struct Theme {
    caption: u32,
    canvas: u32,
    rail: u32,
    rail_line: u32,
    divider: u32,
    nav_selected: u32,
    tile: u32,
    tile_border: u32,
    card_border: u32,
    field: u32,
    field_border: u32,
    field_bottom: u32,
    footer: u32,
    accent: u32,
    accent_pressed: u32,
    banner: u32,
    banner_border: u32,
    tile_radius: i32,
    button_radius: i32,
}

const THEMES: [(&str, &str, u32, u32, u32); 4] = [
    (
        "obsidian",
        "Obsidian",
        rgb(0x20, 0x20, 0x20),
        rgb(0x1b, 0x1b, 0x1b),
        rgb(0x27, 0x27, 0x27),
    ),
    (
        "midnight",
        "Midnight",
        rgb(0x14, 0x19, 0x23),
        rgb(0x0f, 0x14, 0x1d),
        rgb(0x1c, 0x23, 0x31),
    ),
    (
        "pitch",
        "Pitch",
        rgb(0x0c, 0x0c, 0x0e),
        rgb(0x07, 0x07, 0x09),
        rgb(0x15, 0x15, 0x19),
    ),
    (
        "slate",
        "Slate",
        rgb(0x25, 0x26, 0x2b),
        rgb(0x1e, 0x1f, 0x24),
        rgb(0x2e, 0x30, 0x37),
    ),
];

const ACCENTS: [(&str, &str, u32, u32); 6] = [
    ("sky", "Sky", rgb(0x60, 0xcd, 0xff), rgb(0x4c, 0xb0, 0xe0)),
    (
        "amber",
        "Amber",
        rgb(0xf5, 0xb8, 0x41),
        rgb(0xd4, 0x9b, 0x2b),
    ),
    (
        "emerald",
        "Emerald",
        rgb(0x5c, 0xe0, 0x8a),
        rgb(0x42, 0xbe, 0x6e),
    ),
    (
        "coral",
        "Coral",
        rgb(0xff, 0x6b, 0x6b),
        rgb(0xde, 0x52, 0x52),
    ),
    (
        "violet",
        "Violet",
        rgb(0xb5, 0x89, 0xff),
        rgb(0x97, 0x6c, 0xe0),
    ),
    ("ice", "Ice", rgb(0xd8, 0xe4, 0xf0), rgb(0xb4, 0xc4, 0xd4)),
];

impl Theme {
    fn from_prefs(prefs: &Preferences) -> Self {
        let (accent, accent_pressed) = ACCENTS
            .iter()
            .find(|(k, ..)| *k == prefs.accent)
            .map_or((ACCENTS[0].2, ACCENTS[0].3), |(_, _, a, p)| (*a, *p));
        let (tile_radius, button_radius) = if prefs.sharp_corners { (2, 2) } else { (6, 5) };
        match prefs.theme.as_str() {
            "midnight" => Self {
                caption: rgb(0x0d, 0x12, 0x1a),
                canvas: rgb(0x14, 0x19, 0x23),
                rail: rgb(0x0f, 0x14, 0x1d),
                rail_line: rgb(0x22, 0x2c, 0x3d),
                divider: rgb(0x1f, 0x28, 0x38),
                nav_selected: rgb(0x21, 0x2a, 0x3b),
                tile: rgb(0x1c, 0x23, 0x31),
                tile_border: rgb(0x2a, 0x35, 0x48),
                card_border: rgb(0x2d, 0x39, 0x4e),
                field: rgb(0x11, 0x16, 0x20),
                field_border: rgb(0x31, 0x3e, 0x54),
                field_bottom: rgb(0x4a, 0x5d, 0x7a),
                footer: rgb(0x10, 0x15, 0x1e),
                accent,
                accent_pressed,
                banner: rgb(0x17, 0x25, 0x38),
                banner_border: rgb(0x29, 0x44, 0x64),
                tile_radius,
                button_radius,
            },
            "pitch" => Self {
                caption: rgb(0x06, 0x06, 0x08),
                canvas: rgb(0x0c, 0x0c, 0x0e),
                rail: rgb(0x07, 0x07, 0x09),
                rail_line: rgb(0x1e, 0x1e, 0x24),
                divider: rgb(0x1b, 0x1b, 0x20),
                nav_selected: rgb(0x1c, 0x1c, 0x22),
                tile: rgb(0x15, 0x15, 0x19),
                tile_border: rgb(0x25, 0x25, 0x2c),
                card_border: rgb(0x28, 0x28, 0x30),
                field: rgb(0x09, 0x09, 0x0c),
                field_border: rgb(0x2c, 0x2c, 0x35),
                field_bottom: rgb(0x46, 0x46, 0x54),
                footer: rgb(0x08, 0x08, 0x0a),
                accent,
                accent_pressed,
                banner: rgb(0x13, 0x1b, 0x24),
                banner_border: rgb(0x24, 0x38, 0x4c),
                tile_radius,
                button_radius,
            },
            "slate" => Self {
                caption: rgb(0x1c, 0x1d, 0x22),
                canvas: rgb(0x25, 0x26, 0x2b),
                rail: rgb(0x1e, 0x1f, 0x24),
                rail_line: rgb(0x34, 0x36, 0x3e),
                divider: rgb(0x30, 0x32, 0x3a),
                nav_selected: rgb(0x33, 0x35, 0x3e),
                tile: rgb(0x2e, 0x30, 0x37),
                tile_border: rgb(0x3d, 0x40, 0x49),
                card_border: rgb(0x41, 0x44, 0x4e),
                field: rgb(0x1f, 0x20, 0x25),
                field_border: rgb(0x46, 0x49, 0x54),
                field_bottom: rgb(0x63, 0x67, 0x75),
                footer: rgb(0x20, 0x21, 0x26),
                accent,
                accent_pressed,
                banner: rgb(0x24, 0x31, 0x40),
                banner_border: rgb(0x37, 0x52, 0x6e),
                tile_radius,
                button_radius,
            },
            _ => Self {
                caption: rgb(0x19, 0x19, 0x19),
                canvas: rgb(0x20, 0x20, 0x20),
                rail: rgb(0x1b, 0x1b, 0x1b),
                rail_line: rgb(0x2d, 0x2d, 0x2d),
                divider: rgb(0x2a, 0x2a, 0x2a),
                nav_selected: rgb(0x2b, 0x2b, 0x2b),
                tile: rgb(0x27, 0x27, 0x27),
                tile_border: rgb(0x33, 0x33, 0x33),
                card_border: rgb(0x36, 0x36, 0x36),
                field: rgb(0x1c, 0x1c, 0x1c),
                field_border: rgb(0x3d, 0x3d, 0x3d),
                field_bottom: rgb(0x59, 0x59, 0x59),
                footer: rgb(0x1c, 0x1c, 0x1c),
                accent,
                accent_pressed,
                banner: rgb(0x1d, 0x2a, 0x36),
                banner_border: rgb(0x2d, 0x48, 0x60),
                tile_radius,
                button_radius,
            },
        }
    }
}

// ── Geometry (client area 900 × 588) ───────────────────────────────────────
const CLIENT_WIDTH: i32 = 900;
const CLIENT_HEIGHT: i32 = 588;
const RAIL_WIDTH: i32 = 206;
const LEFT: i32 = 225;
const RIGHT: i32 = 882;
const FOOTER_TOP: i32 = 540;
const COLUMN: i32 = 324;
const COLUMN_GAP: i32 = 9;

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

struct Fonts {
    brand: HFONT,
    body: HFONT,
    body_semi: HFONT,
    small: HFONT,
    small_semi: HFONT,
    tiny: HFONT,
    tiny_semi: HFONT,
    details: HFONT,
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
    fonts: std::cell::OnceCell<Fonts>,
    theme: Cell<Theme>,
    field: Cell<HBRUSH>,
    /// Everything that can have a custom picture: contract types, then ships.
    targets: Vec<Target>,
    /// Index into `targets` shown on the Images page.
    target: Cell<usize>,
    /// 0-based page index into `choices()` on the Images page (18 tiles per page).
    image_page: Cell<usize>,
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

    fn fonts(&self) -> &Fonts {
        self.fonts.get().expect("fonts are created in WM_CREATE")
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
            fonts: Default::default(),
            theme: Cell::new(initial_theme),
            field: Cell::new(CreateSolidBrush(initial_theme.field)),
            targets: targets(),
            target: Cell::new(0),
            image_page: Cell::new(0),
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
            (DWMWA_BORDER_COLOR, t.card_border),
            (DWMWA_TEXT_COLOR, rgb(0xcf, 0xcf, 0xcf)),
        ] {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                (&color as *const u32).cast(),
                size_of::<u32>() as u32,
            );
        }
        if !start_hidden || !desktop.tray_ready.get() {
            ShowWindow(hwnd, SW_SHOW);
        }
        let running = Arc::new(AtomicBool::new(true));

        // Update checks: shortly after start, then daily, while enabled.
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
                        next = Instant::now() + Duration::from_secs(24 * 60 * 60);
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
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
                |card| *preview.lock().unwrap() = card.cloned(),
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
    unsafe { CreateFontW(size, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, face) }
}

// ── Layout ──────────────────────────────────────────────────────────────────

fn card_top(banner: bool) -> i32 {
    if banner { 62 } else { 14 }
}

fn page_top(banner: bool) -> i32 {
    card_top(banner) + 106 + 16
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

/// Toggle tiles per page: (id, label, column, row, full width).
const CARD_TILES: [(i32, &str, i32, i32, bool); 10] = [
    (LOCATION, "Location", 0, 0, false),
    (SHIP, "Ship", 1, 0, false),
    (CONTRACTS, "Contracts", 0, 1, false),
    (PARTY, "Party status", 1, 1, false),
    (NOTICES, "Game notices", 0, 2, false),
    (PROFILE, "RSI profile button", 1, 2, false),
    (ART, "Wiki artwork", 0, 3, true),
    (ENABLED, "Share on Discord", 0, 5, false),
    (STARTUP, "Start with Windows", 1, 5, false),
    (UPDATES, "Check for updates automatically", 0, 6, true),
];

fn tile_rect(top: i32, column: i32, row: i32, full: bool) -> RECT {
    // Rows 5+ belong to the "General" group, below a second heading.
    let y = top + 26 + row * 42 + if row >= 5 { 34 - 42 } else { 0 };
    let x = LEFT + column * (COLUMN + COLUMN_GAP);
    rect(x, y, if full { RIGHT - LEFT } else { COLUMN }, 36)
}

/// Activity page rows: the control sits at the right of each 50 px row.
fn activity_row(top: i32, index: i32) -> RECT {
    rect(LEFT, top + 26 + index * 58, RIGHT - LEFT, 50)
}

/// Picture tile `index` on the Images page: six 104×60 tiles per row.
fn choice_rect(top: i32, index: usize) -> RECT {
    let (column, row) = ((index % 6) as i32, (index / 6) as i32);
    rect(LEFT + column * 110, top + 66 + row * 66, 104, 60)
}

/// "Your own link" field below the picture grid.
fn link_frame(top: i32) -> RECT {
    rect(LEFT, top + 66 + 3 * 66 + 4, RIGHT - LEFT - 128, 30)
}

fn buttons_box(top: i32) -> RECT {
    rect(LEFT, top + 26, RIGHT - LEFT, 128)
}

fn updates_row(top: i32) -> RECT {
    rect(LEFT, top + 26, RIGHT - LEFT, 64)
}

/// Field frames drawn behind EDIT controls: (id, frame).
fn fields(page: usize, top: i32) -> Vec<(i32, RECT)> {
    match page {
        ACTIVITY => [(2, DETAILS), (3, CUSTOM)]
            .into_iter()
            .map(|(row, id)| {
                let r = activity_row(top, row);
                (id, rect(r.right - 14 - 240, r.top + 10, 240, 30))
            })
            .collect(),
        IMAGES => vec![(IMAGE_LINK, link_frame(top))],
        BUTTONS => {
            let b = buttons_box(top);
            LINK_FIELDS
                .into_iter()
                .enumerate()
                .flat_map(|(i, (label, url))| {
                    let y = b.top + 28 + i as i32 * 58;
                    [
                        (label, rect(b.left + 12, y, 180, 30)),
                        (url, rect(b.left + 200, y, b.right - b.left - 212, 30)),
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

        for (index, _) in PAGES.iter().enumerate() {
            place(
                hwnd,
                NAV + index as i32,
                true,
                rect(10, 92 + index as i32 * 39, 186, 36),
            );
        }
        for (id, _, column, row, full) in CARD_TILES {
            place(hwnd, id, page == CARD, tile_rect(top, column, row, full));
        }
        // Banner actions.
        place(
            hwnd,
            UPDATE_NOW,
            banner,
            rect(RIGHT - 10 - 104, 20, 104, 24),
        );
        place(
            hwnd,
            RELEASE_NOTES,
            banner,
            rect(RIGHT - 10 - 104 - 12 - 96, 20, 96, 24),
        );

        // Activity page.
        for (row, id) in [(0, VERB), (1, MEMBER_LIST)] {
            let r = activity_row(top, row);
            place(
                hwnd,
                id,
                page == ACTIVITY,
                rect(r.right - 14 - 260, r.top + 10, 260, 220),
            );
        }
        // Images page.
        let paged = desktop.image_page_count() > 1;
        place(
            hwnd,
            IMAGE_TARGET,
            page == IMAGES,
            rect(LEFT, top + 26, if paged { 260 } else { 330 }, 320),
        );
        place(
            hwnd,
            IMG_PREV,
            page == IMAGES && paged,
            rect(LEFT + 268, top + 26, 58, 30),
        );
        place(
            hwnd,
            IMG_NEXT,
            page == IMAGES && paged,
            rect(LEFT + 332, top + 26, 58, 30),
        );
        place(
            hwnd,
            UPLOAD_IMAGE,
            page == IMAGES,
            rect(RIGHT - 130 - 8 - 124, top + 26, 124, 30),
        );
        place(
            hwnd,
            USE_DEFAULT,
            page == IMAGES,
            rect(RIGHT - 130, top + 26, 130, 30),
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
        let link = link_frame(top);
        place(
            hwnd,
            USE_LINK,
            page == IMAGES,
            rect(RIGHT - 120, link.top, 120, 30),
        );
        // Appearance page.
        for index in 0..THEMES.len() {
            let w = 156;
            let gap = 11;
            place(
                hwnd,
                THEME_BTN + index as i32,
                page == APPEARANCE,
                rect(LEFT + index as i32 * (w + gap), top + 46, w, 64),
            );
        }
        for index in 0..ACCENTS.len() {
            let w = 102;
            let gap = 9;
            place(
                hwnd,
                ACCENT_BTN + index as i32,
                page == APPEARANCE,
                rect(LEFT + index as i32 * (w + gap), top + 144, w, 38),
            );
        }
        for index in 0..2 {
            let w = 156;
            let gap = 11;
            place(
                hwnd,
                CORNER_BTN + index,
                page == APPEARANCE,
                rect(LEFT + index * (w + gap), top + 216, w, 36),
            );
        }
        // Updates page.
        let u = updates_row(top);
        place(
            hwnd,
            UPDATE_ACTION,
            page == UPDATES_PAGE,
            rect(u.right - 14 - 160, u.top + 17, 160, 30),
        );
        place(
            hwnd,
            UPDATE_NOTES,
            page == UPDATES_PAGE,
            rect(u.right - 14 - 160 - 12 - 110, u.top + 17, 110, 30),
        );

        // EDIT controls sit inside painted field frames.
        for id in [DETAILS, CUSTOM, IMAGE_LINK, 121, 122, 123, 124] {
            place(hwnd, id, false, hidden);
        }
        for (id, frame) in fields(page, top) {
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
        let face = windows_sys::w!("Segoe UI");
        let _ = desktop.fonts.set(Fonts {
            brand: font(-15, 600, face),
            body: font(-13, 400, face),
            body_semi: font(-13, 600, face),
            small: font(-12, 400, face),
            small_semi: font(-12, 600, face),
            tiny: font(-11, 400, face),
            tiny_semi: font(-11, 600, face),
            details: font(-16, 600, face),
            mono: font(-12, 400, windows_sys::w!("Consolas")),
        });
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
            control(
                windows_sys::w!("BUTTON"),
                label,
                WS_TABSTOP | BS_OWNERDRAW as u32,
                id,
            )
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
            SendMessageW(handle, CB_SETITEMHEIGHT, usize::MAX, 24);
            SendMessageW(handle, CB_SETITEMHEIGHT, 0, 28);
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

/// Rounded rectangle with an optional 1 px border (`radius` is the corner radius).
fn color_channels(color: u32) -> (f32, f32, f32) {
    (
        (color & 0xff) as f32,
        ((color >> 8) & 0xff) as f32,
        ((color >> 16) & 0xff) as f32,
    )
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
        let (wf, hf) = (w as f32, h as f32);
        let (hx, hy) = (wf * 0.5, hf * 0.5);
        let rad = (radius.max(0) as f32).min(hx).min(hy);
        let (fr, fg, fb) = color_channels(color);
        let (br, bg, bb) = border.map_or((fr, fg, fb), color_channels);
        let has_border = border.is_some();
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        for y in 0..h {
            let dy = (y as f32 + 0.5 - hy).abs() - (hy - rad);
            let row = (y as usize) * (w as usize) * 4;
            for x in 0..w {
                let dx = (x as f32 + 0.5 - hx).abs() - (hx - rad);
                let dist = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - rad;
                let outer = (0.5 - dist).clamp(0.0, 1.0);
                if outer <= 0.0 {
                    continue;
                }
                let (fill_w, border_w) = if has_border {
                    let inner = (-0.5 - dist).clamp(0.0, 1.0);
                    (inner, (outer - inner).max(0.0))
                } else {
                    (outer, 0.0)
                };
                let idx = row + (x as usize) * 4;
                bgra[idx] = (fb * fill_w + bb * border_w).round() as u8;
                bgra[idx + 1] = (fg * fill_w + bg * border_w).round() as u8;
                bgra[idx + 2] = (fr * fill_w + br * border_w).round() as u8;
                bgra[idx + 3] = (outer * 255.0).round() as u8;
            }
        }
        let thumb = Thumbnail {
            width: w as u32,
            height: h as u32,
            bgra,
        };
        draw_thumbnail(dc, &thumb, r.left, r.top);
    }
}

/// Anti-aliased filled circle via analytical SDF.
unsafe fn circle(dc: HDC, x: i32, y: i32, size: i32, color: u32) {
    unsafe {
        if size <= 0 {
            return;
        }
        let (fr, fg, fb) = color_channels(color);
        let radius = size as f32 * 0.5;
        let mut bgra = vec![0u8; (size as usize) * (size as usize) * 4];
        for py in 0..size {
            let dy = py as f32 + 0.5 - radius;
            let row = (py as usize) * (size as usize) * 4;
            for px in 0..size {
                let dx = px as f32 + 0.5 - radius;
                let dist = dx.hypot(dy) - radius;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let idx = row + (px as usize) * 4;
                bgra[idx] = (fb * cov).round() as u8;
                bgra[idx + 1] = (fg * cov).round() as u8;
                bgra[idx + 2] = (fr * cov).round() as u8;
                bgra[idx + 3] = (cov * 255.0).round() as u8;
            }
        }
        let thumb = Thumbnail {
            width: size as u32,
            height: size as u32,
            bgra,
        };
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
        let mut value: Vec<u16> = value.encode_utf16().collect();
        let len = value.len() as i32;
        DrawTextW(dc, value.as_mut_ptr(), len, &mut r, flags | DT_NOPREFIX);
        let mut measured = r;
        DrawTextW(
            dc,
            value.as_mut_ptr(),
            len,
            &mut measured,
            flags | DT_NOPREFIX | DT_CALCRECT,
        );
        SelectObject(dc, previous);
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
        let result = thumbnail::fetch(&url, width, height, mask).ok();
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
    /// (fill, border, text)
    fn colors(self) -> (u32, u32, u32) {
        match self {
            Self::Live => (rgb(0x1d, 0x2e, 0x20), rgb(0x2e, 0x52, 0x34), GREEN),
            Self::Paused => (
                rgb(0x33, 0x29, 0x1b),
                rgb(0x5e, 0x4b, 0x2e),
                rgb(0xf5, 0xcf, 0x87),
            ),
            Self::Waiting => (rgb(0x2a, 0x2a, 0x2a), rgb(0x3d, 0x3d, 0x3d), STATE_TEXT),
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
        format!("{h}:{m:02}:{s:02} elapsed")
    } else {
        format!("{m}:{s:02} elapsed")
    }
}

unsafe fn paint(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let target = BeginPaint(hwnd, &mut ps);
        let full = rect(0, 0, CLIENT_WIDTH, CLIENT_HEIGHT);
        let dc = CreateCompatibleDC(target);
        let bitmap = CreateCompatibleBitmap(target, CLIENT_WIDTH, CLIENT_HEIGHT);
        let old = SelectObject(dc, bitmap);
        let f = desktop.fonts();
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
        for (_, frame) in fields(desktop.page.get(), top) {
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
        // Footer.
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
            RAIL_WIDTH + 1,
            FOOTER_TOP,
            CLIENT_WIDTH,
            FOOTER_TOP + 1,
            t.rail_line,
        );
        let _ = f;

        BitBlt(target, 0, 0, CLIENT_WIDTH, CLIENT_HEIGHT, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
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
            RAIL_WIDTH,
            0,
            RAIL_WIDTH + 1,
            CLIENT_HEIGHT,
            t.rail_line,
        );
        shape(
            dc,
            rect(16, 14, 32, 32),
            t.tile_radius,
            t.tile,
            Some(t.tile_border),
        );
        DrawIconEx(dc, 22, 20, desktop.icon, 20, 20, 0, null_mut(), DI_NORMAL);
        text(
            dc,
            "Star Citizen",
            rect(58, 13, 140, 20),
            f.brand,
            TEXT,
            LEFT_LINE,
        );
        text(
            dc,
            "Discord Rich Presence",
            rect(58, 32, 140, 16),
            f.tiny,
            rgb(0x9b, 0x9b, 0x9b),
            LEFT_LINE,
        );
        line(dc, 16, 60, 190, 61, t.divider);
        text(
            dc,
            "SETTINGS",
            rect(20, 70, 160, 16),
            f.tiny_semi,
            FAINT,
            LEFT_LINE,
        );

        line(dc, 16, 546, 190, 547, t.divider);
        let live = liveness(desktop);
        let (_, _, color) = live.colors();
        circle(dc, 18, 562, 7, color);
        let label = match live {
            Liveness::Live => "Connected",
            Liveness::Paused => "Paused",
            Liveness::Waiting => "Waiting",
        };
        text(
            dc,
            label,
            rect(31, 556, 100, 20),
            f.small_semi,
            color,
            LEFT_LINE,
        );
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        let chip = rect(190 - 52, 555, 52, 21);
        shape(dc, chip, 4, t.tile, Some(t.tile_border));
        text(
            dc,
            &version,
            chip,
            f.small_semi,
            rgb(0xa8, 0xa8, 0xa8),
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
    }
}

unsafe fn paint_banner(dc: HDC, desktop: &Desktop) {
    unsafe {
        let f = desktop.fonts();
        let t = desktop.theme.get();
        let r = rect(LEFT, 14, RIGHT - LEFT, 36);
        shape(dc, r, t.tile_radius, t.banner, Some(t.banner_border));
        circle(dc, LEFT + 12, 23, 18, t.accent);
        text(
            dc,
            "↑",
            rect(LEFT + 12, 23, 19, 18),
            f.small_semi,
            ON_ACCENT,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        let message = match &*desktop.update.lock().unwrap() {
            UpdateState::Downloading(release) => {
                format!("Downloading version {}…", release.version)
            }
            UpdateState::Available(release) => format!("Version {} is available", release.version),
            _ => String::new(),
        };
        text(
            dc,
            &message,
            rect(LEFT + 40, 14, 360, 36),
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
        let prefs = desktop.prefs.lock().unwrap().clone();
        shape(
            dc,
            rect(LEFT, top, RIGHT - LEFT, 106),
            t.tile_radius + 2,
            t.tile,
            Some(t.card_border),
        );

        // Artwork: 78 px ship (or location) art with a 22 px mission badge.
        let (ax, ay) = (LEFT + 14, top + 14);
        let large = card
            .as_ref()
            .map(|c| c.large_image.clone())
            .unwrap_or_default();
        let art = (prefs.artwork && large.starts_with("https://"))
            .then(|| want_thumbnail(desktop, hwnd, &large, (78, 78), Mask::Rounded(6)))
            .flatten();
        match art {
            Some(thumb) => draw_thumbnail(dc, &thumb, ax, ay),
            None => {
                shape(
                    dc,
                    rect(ax, ay, 78, 78),
                    t.tile_radius,
                    t.field,
                    Some(t.field_border),
                );
                DrawIconEx(
                    dc,
                    ax + 23,
                    ay + 23,
                    desktop.icon,
                    32,
                    32,
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
        if prefs.artwork
            && small.starts_with("https://")
            && let Some(badge) = want_thumbnail(desktop, hwnd, &small, (22, 22), Mask::Circle)
        {
            circle(dc, ax + 78 - 23, ay + 78 - 23, 28, t.tile);
            draw_thumbnail(dc, &badge, ax + 78 - 20, ay + 78 - 20);
        }

        let x = ax + 78 + 14;
        let width = RIGHT - 14 - x;
        let live = liveness(desktop);
        let verb = prefs
            .activity_type
            .as_deref()
            .unwrap_or(&desktop.base.activity_type);
        let verb = VERBS
            .iter()
            .find(|(key, _)| *key == verb)
            .map_or("Playing", |(_, label)| label);
        let app =
            if desktop.base.discord_app_id == star_citizen_rich_presence::config::DEFAULT_APP_ID {
                "Star Citizen"
            } else {
                "your Discord app"
            };
        text(
            dc,
            &format!("{verb} {app}"),
            rect(x, top + 12, width - 140, 18),
            f.small_semi,
            t.accent,
            LEFT_LINE,
        );

        // Status pill, right-aligned.
        let (pill_fill, pill_border, pill_text) = live.colors();
        let label = live.label();
        let measured = text(
            dc,
            label,
            rect(0, 0, 400, 20),
            f.tiny_semi,
            pill_text,
            DT_SINGLELINE | DT_CALCRECT,
        );
        let pill_width = measured.right - measured.left + 30;
        let pill = rect(RIGHT - 14 - pill_width, top + 11, pill_width, 20);
        shape(dc, pill, 10, pill_fill, Some(pill_border));
        circle(dc, pill.left + 9, pill.top + 7, 6, pill_text);
        text(
            dc,
            label,
            rect(pill.left + 20, pill.top, pill_width - 20, 20),
            f.tiny_semi,
            pill_text,
            LEFT_LINE,
        );

        match &card {
            Some(card) => {
                text(
                    dc,
                    &card.details,
                    rect(x, top + 31, width, 22),
                    f.details,
                    TEXT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    &card.state,
                    rect(x, top + 54, width, 18),
                    f.small,
                    STATE_TEXT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    &elapsed(card.started),
                    rect(x, top + 75, width, 18),
                    f.small_semi,
                    GREEN,
                    LEFT_LINE,
                );
            }
            None => {
                text(
                    dc,
                    "Nothing on your card yet",
                    rect(x, top + 31, width, 22),
                    f.details,
                    TEXT,
                    LEFT_LINE,
                );
                let status = desktop.status.lock().unwrap().clone();
                text(
                    dc,
                    "It appears here and on Discord once Star Citizen is running.",
                    rect(x, top + 54, width, 18),
                    f.small,
                    STATE_TEXT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    &status,
                    rect(x, top + 75, width, 18),
                    f.small,
                    MUTED,
                    LEFT_LINE,
                );
            }
        }
    }
}

unsafe fn heading(dc: HDC, desktop: &Desktop, top: i32, title: &str, subtitle: &str) {
    unsafe {
        let f = desktop.fonts();
        text(
            dc,
            title,
            rect(LEFT, top, 300, 20),
            f.body_semi,
            HEADING,
            LEFT_LINE,
        );
        if !subtitle.is_empty() {
            text(
                dc,
                subtitle,
                rect(LEFT + 300, top, RIGHT - LEFT - 300, 20),
                f.small,
                rgb(0x8f, 0x8f, 0x8f),
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
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
                heading(
                    dc,
                    desktop,
                    top,
                    "Show on card",
                    "Choose what appears on Discord",
                );
                heading(dc, desktop, top + 26 + 4 * 42 + 6, "General", "");
            }
            ACTIVITY => {
                heading(
                    dc,
                    desktop,
                    top,
                    "Activity and text",
                    "How Discord words your status",
                );
                for (index, (title, description)) in [
                    (
                        "Discord says",
                        "The verb before your status (e.g. Playing Star Citizen)",
                    ),
                    (
                        "Member list shows",
                        "Which line appears next to your name in server member lists",
                    ),
                    (
                        "Line 2",
                        "Overrides the automatic location line (leave blank for auto)",
                    ),
                    (
                        "Line 3",
                        "Overrides the automatic ship & contract line (blank for auto)",
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    let r = activity_row(top, index as i32);
                    shape(dc, r, t.tile_radius, t.tile, Some(t.tile_border));
                    text(
                        dc,
                        title,
                        rect(r.left + 14, r.top + 8, 340, 18),
                        f.body_semi,
                        TEXT,
                        LEFT_LINE,
                    );
                    text(
                        dc,
                        description,
                        rect(r.left + 14, r.top + 26, 360, 16),
                        f.small,
                        MUTED,
                        LEFT_LINE,
                    );
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
                    format!("{base_status} · Page {page_num} of {pages} ({total} pictures)")
                } else {
                    base_status
                };
                heading(dc, desktop, top, "Custom images", &status);
                let count = total;
                if count == 0 {
                    text(
                        dc,
                        "No wiki pictures for this one yet. Upload an image or paste an https:// link below.",
                        rect(LEFT, top + 70, RIGHT - LEFT, 20),
                        f.small,
                        FAINT,
                        LEFT_LINE,
                    );
                }
                let link = link_frame(top);
                text(
                    dc,
                    "Click a picture to use it, click Upload image to pick a file from your PC, \
                     or paste an https:// link. Uploaded images are resized and hosted publicly so Discord can show them.",
                    rect(LEFT, link.bottom + 10, RIGHT - LEFT, 34),
                    f.small,
                    MUTED,
                    DT_LEFT | DT_WORDBREAK,
                );
            }
            BUTTONS => {
                heading(
                    dc,
                    desktop,
                    top,
                    "Discord buttons",
                    "Up to two links on your card",
                );
                let b = buttons_box(top);
                shape(dc, b, t.tile_radius, t.tile, Some(t.tile_border));
                for (index, caption) in ["Button 1", "Button 2"].into_iter().enumerate() {
                    let y = b.top + 8 + index as i32 * 58;
                    text(
                        dc,
                        caption,
                        rect(b.left + 12, y, 200, 18),
                        f.small_semi,
                        MUTED,
                        LEFT_LINE,
                    );
                }
                text(
                    dc,
                    "Labels are 1–32 characters; links must start with https://. With one button, \
                     the RSI profile button can use the free slot. Click Apply to save.",
                    rect(LEFT, b.bottom + 10, RIGHT - LEFT, 34),
                    f.small,
                    MUTED,
                    DT_LEFT | DT_WORDBREAK,
                );
            }
            APPEARANCE => {
                heading(
                    dc,
                    desktop,
                    top,
                    "Appearance",
                    "Customise the control panel theme",
                );
                text(
                    dc,
                    "COLOUR SCHEME",
                    rect(LEFT, top + 26, 200, 16),
                    f.tiny_semi,
                    FAINT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    "ACCENT COLOUR",
                    rect(LEFT, top + 124, 200, 16),
                    f.tiny_semi,
                    FAINT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    "CORNER STYLE",
                    rect(LEFT, top + 196, 200, 16),
                    f.tiny_semi,
                    FAINT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    "Changes apply immediately and are saved to your local preferences.",
                    rect(LEFT, top + 264, RIGHT - LEFT, 20),
                    f.small,
                    MUTED,
                    LEFT_LINE,
                );
            }
            _ => {
                heading(
                    dc,
                    desktop,
                    top,
                    "Updates",
                    &format!("Installed: v{}", env!("CARGO_PKG_VERSION")),
                );
                let r = updates_row(top);
                shape(dc, r, t.tile_radius, t.tile, Some(t.tile_border));
                let state = desktop.update.lock().unwrap().clone();
                let (title, description) = match &state {
                    UpdateState::Idle => (
                        "Updates".to_string(),
                        if desktop.base.check_for_updates {
                            "Not checked yet.".to_string()
                        } else {
                            "Turned off in scrp.toml (check_for_updates = false).".to_string()
                        },
                    ),
                    UpdateState::Checking => (
                        "Checking for updates…".into(),
                        "Asking GitHub for the latest release.".into(),
                    ),
                    UpdateState::UpToDate => (
                        "You're up to date".into(),
                        "This is the latest release.".into(),
                    ),
                    UpdateState::Available(release) => (
                        format!("Version {} is available", release.version),
                        if release.installer_url.is_some() && release.sums_url.is_some() {
                            "Downloads, verifies, and installs the update, then reopens.".into()
                        } else {
                            "Download the new version from the GitHub release page.".into()
                        },
                    ),
                    UpdateState::Downloading(release) => (
                        format!("Downloading version {}…", release.version),
                        "The app closes and reopens when the installer finishes.".into(),
                    ),
                    UpdateState::Failed(error) => ("Couldn't update".into(), error.clone()),
                };
                text(
                    dc,
                    &title,
                    rect(r.left + 14, r.top + 12, 340, 20),
                    f.body_semi,
                    TEXT,
                    LEFT_LINE,
                );
                text(
                    dc,
                    &description,
                    rect(r.left + 14, r.top + 34, 340, 18),
                    f.small,
                    MUTED,
                    LEFT_LINE,
                );
                text(
                    dc,
                    "Checks ask GitHub for the latest release when the app starts and once a day. \
                     Nothing about you or your game is sent. Installers are verified against the \
                     release's checksums before they run.",
                    rect(LEFT, r.bottom + 12, RIGHT - LEFT, 34),
                    f.small,
                    MUTED,
                    DT_LEFT | DT_WORDBREAK,
                );
            }
        }
    }
}

/// Background behind an owner-drawn button, so its rounded corners blend in.
fn behind(id: i32, t: Theme, banner_bottom: i32, y: i32) -> u32 {
    match id {
        HIDE | SAVE | EXIT => t.footer,
        n if (NAV..NAV + PAGES.len() as i32).contains(&n) => t.rail,
        UPDATE_NOW | RELEASE_NOTES if y < banner_bottom => t.banner,
        UPDATE_ACTION | UPDATE_NOTES => t.tile,
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
            let pill = rect(
                r.left + 4,
                r.top + 2,
                (r.right - r.left - 8).max(4),
                (r.bottom - r.top - 4).max(4),
            );
            shape(dc, pill, t.button_radius, t.nav_selected, None);
            shape(
                dc,
                rect(
                    pill.left,
                    pill.top + 4,
                    3,
                    (pill.bottom - pill.top - 8).max(4),
                ),
                1,
                t.accent,
                None,
            );
        }
        let pad = if in_edit { 8 } else { 14 };
        text(
            dc,
            &label,
            rect(
                r.left + pad,
                r.top,
                (r.right - r.left - pad - 8).max(0),
                r.bottom - r.top,
            ),
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
                r.left + 4,
                r.bottom - 1,
                r.right - 4,
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
                    rect(10, 0, (r.right - 34).max(0), r.bottom),
                    font as HFONT,
                    TEXT,
                    LEFT_LINE,
                );
            }
        }
        text(
            dc,
            "▾",
            rect(r.right - 26, 0, 22, r.bottom),
            font as HFONT,
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
        fill(
            dc,
            r,
            behind(id, t, 50, if desktop.banner.get() { 20 } else { 999 }),
        );
        let mut label = [0u16; 64];
        let len = GetWindowTextW(item.hwndItem, label.as_mut_ptr(), label.len() as i32);
        let label = String::from_utf16_lossy(&label[..len as usize]);
        let center = DT_CENTER | DT_SINGLELINE | DT_VCENTER;

        if (THEME_BTN..THEME_BTN + THEMES.len() as i32).contains(&id) {
            let index = (id - THEME_BTN) as usize;
            let (key, name, canvas_c, rail_c, tile_c) = THEMES[index];
            let selected = desktop.prefs.lock().unwrap().theme == key;
            shape(
                dc,
                r,
                t.tile_radius,
                t.tile,
                Some(if selected { t.accent } else { t.tile_border }),
            );
            // Mini theme swatch preview on the left.
            let sw = rect(r.left + 10, r.top + 12, 40, 40);
            shape(dc, sw, 4, canvas_c, Some(t.tile_border));
            shape(dc, rect(sw.left + 2, sw.top + 2, 11, 36), 2, rail_c, None);
            shape(dc, rect(sw.left + 16, sw.top + 6, 20, 12), 2, tile_c, None);
            shape(
                dc,
                rect(sw.left + 16, sw.top + 22, 14, 10),
                2,
                t.accent,
                None,
            );
            text(
                dc,
                name,
                rect(r.left + 58, r.top + 14, r.right - r.left - 78, 18),
                if selected { f.body_semi } else { f.body },
                TEXT,
                LEFT_LINE,
            );
            text(
                dc,
                if selected { "Active" } else { "Select" },
                rect(r.left + 58, r.top + 33, r.right - r.left - 78, 16),
                f.tiny,
                if selected { t.accent } else { MUTED },
                LEFT_LINE,
            );
            if selected {
                circle(dc, r.right - 22, r.top + 8, 14, t.accent);
                text(
                    dc,
                    "✓",
                    rect(r.right - 22, r.top + 8, 15, 15),
                    f.tiny_semi,
                    ON_ACCENT,
                    center,
                );
            }
        } else if (ACCENT_BTN..ACCENT_BTN + ACCENTS.len() as i32).contains(&id) {
            let index = (id - ACCENT_BTN) as usize;
            let (key, name, color, _) = ACCENTS[index];
            let selected = desktop.prefs.lock().unwrap().accent == key;
            shape(
                dc,
                r,
                t.button_radius,
                if selected { t.nav_selected } else { t.tile },
                Some(if selected { color } else { t.tile_border }),
            );
            circle(
                dc,
                r.left + 12,
                r.top + (r.bottom - r.top - 14) / 2,
                14,
                color,
            );
            text(
                dc,
                name,
                rect(r.left + 32, r.top, r.right - r.left - 38, r.bottom - r.top),
                if selected { f.small_semi } else { f.small },
                if selected { TEXT } else { SOFT },
                LEFT_LINE,
            );
        } else if (CORNER_BTN..CORNER_BTN + 2).contains(&id) {
            let sharp = id == CORNER_BTN + 1;
            let selected = desktop.prefs.lock().unwrap().sharp_corners == sharp;
            shape(
                dc,
                r,
                if sharp { 2 } else { 6 },
                if selected { t.nav_selected } else { t.tile },
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
            let picture = desktop.choices().get(index).and_then(|url| {
                want_thumbnail(
                    desktop,
                    GetParent(item.hwndItem),
                    url,
                    (w as u32 - 4, h as u32 - 4),
                    Mask::Rounded(4),
                )
            });
            let selected = desktop.selected_choice() == Some(index);
            shape(
                dc,
                r,
                t.tile_radius,
                if selected { t.accent } else { t.tile },
                Some(if selected { t.accent } else { t.tile_border }),
            );
            match picture {
                Some(thumb) => draw_thumbnail(dc, &thumb, r.left + 2, r.top + 2),
                None => {
                    text(dc, "Loading…", r, f.tiny, FAINT, center);
                }
            }
            let chip_label = if index == 0 {
                "Default".to_string()
            } else {
                (index + 1).to_string()
            };
            let measured = text(
                dc,
                &chip_label,
                rect(0, 0, 200, 16),
                f.tiny_semi,
                TEXT,
                DT_SINGLELINE | DT_CALCRECT,
            );
            let chip = rect(
                r.left + 6,
                r.bottom - 22,
                measured.right - measured.left + 12,
                16,
            );
            shape(
                dc,
                chip,
                4,
                if selected {
                    t.accent
                } else {
                    rgb(0x14, 0x14, 0x14)
                },
                None,
            );
            text(
                dc,
                &chip_label,
                chip,
                f.tiny_semi,
                if selected { ON_ACCENT } else { TEXT },
                center,
            );
            if selected {
                circle(dc, r.right - 22, r.top + 6, 16, t.accent);
                text(
                    dc,
                    "✓",
                    rect(r.right - 22, r.top + 6, 17, 17),
                    f.small_semi,
                    ON_ACCENT,
                    center,
                );
            }
            if pressed {
                InvertRect(dc, &rect(r.left + 2, r.top + 2, w - 4, h - 4));
            }
        } else if (NAV..NAV + PAGES.len() as i32).contains(&id) {
            let page = (id - NAV) as usize;
            let selected = page == desktop.page.get();
            if selected || pressed {
                shape(dc, r, t.button_radius, t.nav_selected, None);
            }
            if selected {
                shape(dc, rect(r.left, r.top + 9, 3, 18), 1, t.accent, None);
            }
            text(
                dc,
                &label,
                rect(r.left + 14, r.top, 120, r.bottom - r.top),
                if selected { f.body_semi } else { f.body },
                if selected {
                    rgb(0xff, 0xff, 0xff)
                } else {
                    SOFT
                },
                LEFT_LINE,
            );
            if page == IMAGES {
                let count = desktop.prefs.lock().unwrap().overrides().len();
                if count > 0 {
                    let chip = rect(r.right - 36, r.top + 9, 26, 18);
                    shape(dc, chip, 9, t.tile, Some(t.tile_border));
                    text(
                        dc,
                        &count.to_string(),
                        chip,
                        f.tiny,
                        rgb(0x9e, 0x9e, 0x9e),
                        center,
                    );
                }
            }
            if page == UPDATES_PAGE
                && matches!(*desktop.update.lock().unwrap(), UpdateState::Available(_))
            {
                circle(dc, r.right - 20, r.top + 15, 7, t.accent);
            }
        } else if matches!(id, RELEASE_NOTES | UPDATE_NOTES) {
            let measured = text(dc, &label, r, f.small_semi, t.accent, center);
            let y = measured.bottom + (r.bottom - r.top - (measured.bottom - measured.top)) / 2 - 1;
            let width = measured.right - measured.left;
            let x = r.left + (r.right - r.left - width) / 2;
            line(
                dc,
                x,
                y.min(r.bottom - 3),
                x + width,
                y.min(r.bottom - 3) + 1,
                t.accent,
            );
        } else {
            let primary = id == SAVE
                || id == UPDATE_NOW
                || (id == UPDATE_ACTION
                    && matches!(*desktop.update.lock().unwrap(), UpdateState::Available(_)));
            let (face, border, color, font) = if primary {
                (
                    if pressed { t.accent_pressed } else { t.accent },
                    None,
                    ON_ACCENT,
                    if id == UPDATE_NOW {
                        f.small_semi
                    } else {
                        f.body_semi
                    },
                )
            } else if id == EXIT {
                (
                    t.tile,
                    Some(t.tile_border),
                    rgb(0xbc, 0xbc, 0xbc),
                    f.body_semi,
                )
            } else if id == HIDE {
                (
                    if pressed { t.nav_selected } else { t.tile },
                    Some(t.field_border),
                    rgb(0xec, 0xec, 0xec),
                    f.body_semi,
                )
            } else {
                (
                    if pressed { t.nav_selected } else { t.tile },
                    Some(t.field_border),
                    TEXT,
                    f.body_semi,
                )
            };
            shape(
                dc,
                r,
                if id == UPDATE_NOW { 4 } else { t.button_radius },
                face,
                border,
            );
            text(dc, &label, r, font, color, center);
        }
        if item.itemState & ODS_FOCUS != 0 {
            let mut focus = r;
            InflateRect(&mut focus, -3, -3);
            DrawFocusRect(dc, &focus);
        }
    }
}

/// Toggle tile: label on the left, Fluent switch on the right.
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
            if matches!(msg, BM_SETCHECK | BM_SETSTATE | WM_SETFOCUS | WM_KILLFOCUS) {
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
        fill(dc, r, t.canvas);
        shape(dc, r, t.tile_radius, t.tile, Some(t.tile_border));
        let on = SendMessageW(hwnd, BM_GETCHECK, 0, 0) == 1;
        let enabled = IsWindowEnabled(hwnd) != 0;
        let mut label = [0u16; 64];
        let len = GetWindowTextW(hwnd, label.as_mut_ptr(), label.len() as i32);
        let label = String::from_utf16_lossy(&label[..len as usize]);
        text(
            dc,
            &label,
            rect(12, 0, r.right - 70, r.bottom),
            font as HFONT,
            if on && enabled {
                rgb(0xed, 0xed, 0xed)
            } else {
                OFF_TEXT
            },
            LEFT_LINE,
        );
        let sx = r.right - 12 - 36;
        let sy = (r.bottom - 18) / 2;
        if on {
            shape(
                dc,
                rect(sx, sy, 36, 18),
                9,
                if enabled {
                    t.accent
                } else {
                    rgb(0x4a, 0x6a, 0x7a)
                },
                None,
            );
            circle(dc, sx + 22, sy + 4, 10, rgb(0x14, 0x14, 0x14));
        } else {
            shape(
                dc,
                rect(sx, sy, 36, 18),
                9,
                t.canvas,
                Some(rgb(0x8c, 0x8c, 0x8c)),
            );
            circle(dc, sx + 4, sy + 4, 10, rgb(0xcf, 0xcf, 0xcf));
        }
        if SendMessageW(hwnd, BM_GETSTATE, 0, 0) & BST_FOCUS as isize != 0 {
            let mut focus = r;
            InflateRect(&mut focus, -3, -3);
            DrawFocusRect(dc, &focus);
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
                build_controls(hwnd, desktop);
                desktop.tray_ready.set(tray(hwnd, desktop.icon, NIM_ADD));
                SetTimer(hwnd, 1, 500, None);
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
                if let Some(f) = desktop.fonts.get() {
                    for font in [
                        f.brand,
                        f.body,
                        f.body_semi,
                        f.small,
                        f.small_semi,
                        f.tiny,
                        f.tiny_semi,
                        f.details,
                        f.mono,
                    ] {
                        DeleteObject(font);
                    }
                }
                DeleteObject(desktop.field.get());
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
            (DWMWA_BORDER_COLOR, t.card_border),
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
        if !commit(desktop, next) && id != SAVE {
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
