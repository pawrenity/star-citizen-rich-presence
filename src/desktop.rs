use super::*;
use serde::{Deserialize, Serialize};
use std::{cell::Cell, path::PathBuf, ptr::null_mut, sync::Mutex};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::*, Registry::*, Threading::*},
    UI::{Controls::*, Shell::*, WindowsAndMessaging::*},
};

const TRAY: u32 = WM_APP + 1;
const APP_ICON: Pcwstr = 101usize as Pcwstr;
const STATUS: i32 = 100;
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
const EYEBROW: i32 = 113;
const CARD_LABEL: i32 = 114;
const CUSTOM_LABEL: i32 = 115;
const ACTIVITY_LABEL: i32 = 116;
const GENERAL_LABEL: i32 = 117;
const DETAILS_LABEL: i32 = 118;
const DETAILS: i32 = 119;
const LINKS_LABEL: i32 = 120;
const LINK_FIELDS: [(i32, i32); 2] = [(121, 122), (123, 124)];
const BACKGROUND: u32 = 0x22171b;
const PANEL: u32 = 0x34232c;
const TEXT: u32 = 0xf8edf7;
const MUTED: u32 = 0xcbb5ca;
const ACCENT: u32 = 0xcdb6fa;
const RUN_KEY: Pcwstr = windows_sys::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: Pcwstr = windows_sys::w!("StarCitizenRichPresence");

type Pcwstr = *const u16;

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
    custom: String,
    details: String,
    buttons: Option<Vec<star_citizen_rich_presence::config::CardButton>>,
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
            custom: config.custom_state.clone(),
            details: config.custom_details.clone(),
            buttons: None,
        }
    }

    fn apply(&self, config: &mut Config) {
        config.show_location = self.location;
        config.show_ship = self.ship;
        config.show_artwork = self.artwork;
        config.show_profile = self.profile;
        config.show_contracts = self.contracts;
        config.show_notifications = self.notices;
        config.custom_state.clone_from(&self.custom);
        config.custom_details.clone_from(&self.details);
        if let Some(buttons) = &self.buttons {
            config.buttons.clone_from(buttons);
        }
    }
}

struct Desktop {
    prefs: Arc<Mutex<Preferences>>,
    status: Arc<Mutex<String>>,
    file: PathBuf,
    icon: HICON,
    taskbar_created: u32,
    tray_ready: Cell<bool>,
    heading: Cell<HFONT>,
    body: Cell<HFONT>,
    background: HBRUSH,
    panel: HBRUSH,
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
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            ..Default::default()
        };
        anyhow::ensure!(
            RegisterClassW(&wc) != 0,
            "Cannot register the desktop window"
        );
        let prefs = Arc::new(Mutex::new(initial));
        let status = Arc::new(Mutex::new(String::from("Waiting for Star Citizen…")));
        let mut desktop = Box::new(Desktop {
            prefs: Arc::clone(&prefs),
            status: Arc::clone(&status),
            file,
            icon,
            taskbar_created: RegisterWindowMessageW(windows_sys::w!("TaskbarCreated")),
            tray_ready: Cell::new(false),
            heading: Cell::new(null_mut()),
            body: Cell::new(null_mut()),
            background: CreateSolidBrush(BACKGROUND),
            panel: CreateSolidBrush(PANEL),
        });
        let hwnd = CreateWindowExW(
            0,
            class,
            windows_sys::w!("Star Citizen Rich Presence"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            536,
            698,
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
        for (attribute, color) in [
            (DWMWA_CAPTION_COLOR, BACKGROUND),
            (DWMWA_BORDER_COLOR, PANEL),
            (DWMWA_TEXT_COLOR, TEXT),
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
        let signal = Arc::clone(&running);
        let worker = std::thread::spawn(move || {
            monitor(args, config, locations, signal, |config, update| {
                if let Some(update) = update {
                    *status.lock().unwrap() = update.to_owned();
                }
                let prefs = prefs.lock().unwrap();
                if update.is_none() {
                    prefs.apply(config);
                }
                prefs.enabled
            })
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

unsafe fn build_controls(hwnd: HWND, desktop: &Desktop) {
    unsafe {
        let font = CreateFontW(
            -16,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            windows_sys::w!("Segoe UI"),
        );
        desktop.body.set(font);
        let control = |class: Pcwstr, label: &str, style, id, (x, y, width, height)| {
            let handle = CreateWindowExW(
                0,
                class,
                wide(label).as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                x,
                y,
                width,
                height,
                hwnd,
                id as usize as HMENU,
                null_mut(),
                null_mut(),
            );
            SendMessageW(handle, WM_SETFONT, font as usize, 1);
            handle
        };
        control(
            windows_sys::w!("STATIC"),
            "Discord Rich Presence",
            0,
            EYEBROW,
            (27, 56, 460, 20),
        );
        let title = control(
            windows_sys::w!("STATIC"),
            "Star Citizen",
            0,
            0,
            (24, 17, 460, 41),
        );
        desktop.heading.set(CreateFontW(
            -31,
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            windows_sys::w!("Segoe UI"),
        ));
        SendMessageW(title, WM_SETFONT, desktop.heading.get() as usize, 1);
        control(
            windows_sys::w!("STATIC"),
            "CURRENT ACTIVITY",
            0,
            ACTIVITY_LABEL,
            (40, 101, 440, 18),
        );
        control(
            windows_sys::w!("STATIC"),
            "Waiting for Star Citizen…",
            0,
            STATUS,
            (40, 124, 440, 36),
        );
        control(
            windows_sys::w!("STATIC"),
            "APP SETTINGS",
            0,
            GENERAL_LABEL,
            (36, 184, 220, 20),
        );
        let prefs = desktop.prefs.lock().unwrap();
        for (id, label, checked, x, y) in [
            (ENABLED, "Share activity on Discord", prefs.enabled, 36, 210),
            (STARTUP, "Start with Windows", startup_enabled(), 264, 210),
            (LOCATION, "Show location", prefs.location, 36, 288),
            (SHIP, "Show ship", prefs.ship, 264, 288),
            (ART, "Use location artwork", prefs.artwork, 36, 320),
            (PROFILE, "Link RSI profile", prefs.profile, 264, 320),
            (CONTRACTS, "Show contracts", prefs.contracts, 36, 352),
            (NOTICES, "Show game notifications", prefs.notices, 264, 352),
        ] {
            let check = control(
                windows_sys::w!("BUTTON"),
                label,
                WS_TABSTOP | BS_AUTOCHECKBOX as u32,
                id,
                (x, y, 216, 26),
            );
            SendMessageW(check, BM_SETCHECK, checked as usize, 0);
            SetWindowSubclass(check, Some(paint_checkbox), 1, font as usize);
        }
        control(
            windows_sys::w!("STATIC"),
            "DISCORD CARD",
            0,
            CARD_LABEL,
            (36, 262, 220, 20),
        );
        let label = |id, text, x, y| {
            control(windows_sys::w!("STATIC"), text, 0, id, (x, y, 210, 20));
        };
        let edit = |id, value: &str, hint: &str, limit, rect| {
            let handle = control(
                windows_sys::w!("EDIT"),
                value,
                WS_TABSTOP | ES_AUTOHSCROLL as u32,
                id,
                rect,
            );
            SendMessageW(handle, EM_SETLIMITTEXT, limit, 0);
            SendMessageW(
                handle,
                EM_SETMARGINS,
                (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
                0x0008_0008,
            );
            SendMessageW(handle, EM_SETCUEBANNER, 0, wide(hint).as_ptr() as LPARAM);
        };
        label(DETAILS_LABEL, "Line 2", 36, 388);
        edit(
            DETAILS,
            &prefs.details,
            "Automatic activity",
            128,
            (36, 410, 214, 28),
        );
        label(CUSTOM_LABEL, "Line 3", 264, 388);
        edit(
            CUSTOM,
            &prefs.custom,
            "Automatic ship or status",
            128,
            (264, 410, 220, 28),
        );
        label(LINKS_LABEL, "DISCORD BUTTONS", 36, 474);
        let buttons = prefs.buttons.as_deref().unwrap_or_default();
        for (index, (name, url)) in LINK_FIELDS.into_iter().enumerate() {
            let button = buttons.get(index);
            let y = 501 + index as i32 * 34;
            edit(
                name,
                button.map_or("", |b| &b.label),
                "Button label",
                32,
                (36, y, 137, 28),
            );
            edit(
                url,
                button.map_or("", |b| &b.url),
                "https://…",
                512,
                (185, y, 299, 28),
            );
        }
        for (id, label, x) in [
            (SAVE, "Apply", 306),
            (HIDE, "Hide to tray", 20),
            (EXIT, "Quit", 416),
        ] {
            control(
                windows_sys::w!("BUTTON"),
                label,
                WS_TABSTOP | BS_OWNERDRAW as u32,
                id,
                (x, 621, if id == HIDE { 110 } else { 85 }, 29),
            );
        }
    }
}

unsafe fn fill(dc: HDC, rect: RECT, color: u32) {
    unsafe {
        SetDCBrushColor(dc, color);
        FillRect(dc, &rect, GetStockObject(DC_BRUSH) as HBRUSH);
    }
}

unsafe fn rounded(dc: HDC, rect: RECT, diameter: i32, color: u32) {
    unsafe {
        let brush = SelectObject(dc, GetStockObject(DC_BRUSH));
        let pen = SelectObject(dc, GetStockObject(NULL_PEN));
        SetDCBrushColor(dc, color);
        RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            diameter,
            diameter,
        );
        SelectObject(dc, pen);
        SelectObject(dc, brush);
    }
}

unsafe fn draw_label(dc: HDC, hwnd: HWND, font: HFONT, color: u32, mut rect: RECT, flags: u32) {
    unsafe {
        let mut label = [0u16; 128];
        let len = GetWindowTextW(hwnd, label.as_mut_ptr(), label.len() as i32);
        let previous = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        DrawTextW(dc, label.as_mut_ptr(), len, &mut rect, flags);
        SelectObject(dc, previous);
    }
}

unsafe extern "system" fn paint_checkbox(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    font: usize,
) -> LRESULT {
    unsafe {
        if msg == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(paint_checkbox), id);
        }
        if msg != WM_PAINT {
            let result = DefSubclassProc(hwnd, msg, w, l);
            if matches!(msg, BM_SETCHECK | BM_SETSTATE | WM_SETFOCUS | WM_KILLFOCUS) {
                InvalidateRect(hwnd, null_mut(), 0);
            }
            return result;
        }
        let mut paint = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect);
        fill(dc, rect, PANEL);
        let active = SendMessageW(hwnd, BM_GETCHECK, 0, 0) == 1;
        let y = (rect.bottom - 18) / 2;
        rounded(
            dc,
            RECT {
                left: 0,
                top: y,
                right: 34,
                bottom: y + 18,
            },
            18,
            if active { ACCENT } else { 0x63495f },
        );
        let knob = if active { 18 } else { 3 };
        rounded(
            dc,
            RECT {
                left: knob,
                top: y + 3,
                right: knob + 13,
                bottom: y + 15,
            },
            12,
            if active { BACKGROUND } else { TEXT },
        );
        rect.left = 42;
        draw_label(
            dc,
            hwnd,
            font as HFONT,
            TEXT,
            rect,
            DT_SINGLELINE | DT_VCENTER,
        );
        if SendMessageW(hwnd, BM_GETSTATE, 0, 0) & BST_FOCUS as isize != 0 {
            DrawFocusRect(dc, &rect);
        }
        EndPaint(hwnd, &paint);
        0
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
            WM_ERASEBKGND => {
                let dc = w as HDC;
                let mut rect = RECT::default();
                GetClientRect(hwnd, &mut rect);
                FillRect(dc, &rect, desktop.background);
                for (top, bottom) in [(86, 168), (178, 244), (254, 452), (464, 578)] {
                    rounded(
                        dc,
                        RECT {
                            left: 20,
                            top,
                            right: 501,
                            bottom,
                        },
                        18,
                        PANEL,
                    );
                }
                return 1;
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
                let id = GetDlgCtrlID(l as HWND);
                let dc = w as HDC;
                let panel = matches!(
                    id,
                    STATUS
                        | ACTIVITY_LABEL
                        | GENERAL_LABEL
                        | CARD_LABEL
                        | CUSTOM_LABEL
                        | DETAILS_LABEL
                        | LINKS_LABEL
                );
                SetBkColor(dc, if panel { PANEL } else { BACKGROUND });
                SetTextColor(
                    dc,
                    match id {
                        ACTIVITY_LABEL => ACCENT,
                        EYEBROW | GENERAL_LABEL | CARD_LABEL | CUSTOM_LABEL | DETAILS_LABEL
                        | LINKS_LABEL => MUTED,
                        _ => TEXT,
                    },
                );
                return if panel {
                    desktop.panel
                } else {
                    desktop.background
                } as LRESULT;
            }
            WM_DRAWITEM => {
                let item = &*(l as *const DRAWITEMSTRUCT);
                let primary = item.CtlID == SAVE as u32;
                let pressed = item.itemState & ODS_SELECTED != 0;
                let background = match (primary, pressed) {
                    (true, false) => ACCENT,
                    (true, true) => 0xb799e1,
                    (false, true) => 0x543951,
                    (false, false) => PANEL,
                };
                fill(
                    item.hDC,
                    item.rcItem,
                    if primary { PANEL } else { BACKGROUND },
                );
                rounded(item.hDC, item.rcItem, 11, background);
                draw_label(
                    item.hDC,
                    item.hwndItem,
                    desktop.body.get(),
                    if primary { BACKGROUND } else { TEXT },
                    item.rcItem,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
                if item.itemState & ODS_FOCUS != 0 {
                    let mut focus = item.rcItem;
                    InflateRect(&mut focus, -3, -3);
                    DrawFocusRect(item.hDC, &focus);
                }
                return 1;
            }
            WM_CREATE => {
                build_controls(hwnd, desktop);
                desktop.tray_ready.set(tray(hwnd, desktop.icon, NIM_ADD));
                SetTimer(hwnd, 1, 500, None);
            }
            WM_TIMER => {
                let status = desktop.status.lock().unwrap().clone();
                SetDlgItemTextW(hwnd, STATUS, wide(&status).as_ptr());
            }
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
                match id {
                    EXIT => {
                        DestroyWindow(hwnd);
                    }
                    HIDE => hide(hwnd, desktop),
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
                    ENABLED | LOCATION | SHIP | ART | PROFILE | CONTRACTS | NOTICES | SAVE => {
                        update_prefs(hwnd, desktop, id)
                    }
                    _ => {}
                }
            }
            WM_DESTROY => {
                tray(hwnd, desktop.icon, NIM_DELETE);
                DeleteObject(desktop.heading.get());
                DeleteObject(desktop.body.get());
                DeleteObject(desktop.background);
                DeleteObject(desktop.panel);
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

unsafe fn checked(hwnd: HWND, id: i32) -> bool {
    unsafe { SendDlgItemMessageW(hwnd, id, BM_GETCHECK, 0, 0) == 1 }
}

unsafe fn update_prefs(hwnd: HWND, desktop: &Desktop, id: i32) {
    unsafe {
        let old = desktop.prefs.lock().unwrap().clone();
        let read = |field| {
            let mut text = [0; 513];
            let len = GetDlgItemTextW(hwnd, field, text.as_mut_ptr(), text.len() as i32);
            String::from_utf16_lossy(&text[..len as usize])
                .trim()
                .to_owned()
        };
        let mut buttons = old.buttons.clone();
        if id == SAVE {
            let links: Vec<_> = LINK_FIELDS
                .into_iter()
                .map(
                    |(label, url)| star_citizen_rich_presence::config::CardButton {
                        label: read(label),
                        url: read(url),
                    },
                )
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
            custom: if id == SAVE {
                read(CUSTOM)
            } else {
                old.custom.clone()
            },
            details: if id == SAVE {
                read(DETAILS)
            } else {
                old.details.clone()
            },
            buttons,
        };
        if let Err(error) = save(&desktop.file, &next) {
            let previous = match id {
                ENABLED => old.enabled,
                LOCATION => old.location,
                SHIP => old.ship,
                ART => old.artwork,
                PROFILE => old.profile,
                CONTRACTS => old.contracts,
                NOTICES => old.notices,
                _ => false,
            };
            if id != SAVE {
                SendDlgItemMessageW(hwnd, id, BM_SETCHECK, previous as usize, 0);
            }
            show_error(&format!("Could not save preferences: {error}"));
        } else {
            *desktop.prefs.lock().unwrap() = next;
        }
    }
}
