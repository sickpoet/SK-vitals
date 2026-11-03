#![windows_subsystem = "windows"]

mod stats;
mod ui;

use std::mem::{size_of, zeroed};
use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use stats::{Sampler, Snapshot};
use ui::{wide, Fonts};

const WM_TRAY: u32 = WM_APP + 1;
const WM_SHOW_PANEL: u32 = WM_APP + 2;
const TIMER_ID: usize = 1;
const TRAY_UID: u32 = 1;
const MENU_TOGGLE: u32 = 1;
const MENU_QUIT: u32 = 2;
const CLASS_NAME: &str = "SKVitalsPanel";

struct App {
    hwnd: HWND,
    icon: HICON,
    fonts: Fonts,
    sampler: Sampler,
    snapshot: Snapshot,
    panel_visible: bool,
    taskbar_msg: u32,
}

static mut APP: *mut App = std::ptr::null_mut();

unsafe fn app() -> &'static mut App {
    &mut *APP
}

fn main() {
    unsafe {
        // 尽早声明 DPI 感知，否则悬浮窗在缩放屏上会被系统拉伸糊掉
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let class = wide(CLASS_NAME);

        // 单实例：再次启动只是把已运行的悬浮窗显示出来
        let running = FindWindowW(class.as_ptr(), null());
        if running != 0 {
            PostMessageW(running, WM_SHOW_PANEL, 0, 0);
            return;
        }

        let hinst = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            return;
        }

        let (w, h) = ui::panel_size(GetDpiForSystem());
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class.as_ptr(),
            wide("SK-vitals").as_ptr(),
            WS_POPUP,
            0,
            0,
            w,
            h,
            0,
            0,
            hinst,
            null(),
        );
        if hwnd == 0 {
            return;
        }

        let mut sampler = Sampler::new();
        let snapshot = sampler.sample();
        let state = App {
            hwnd,
            icon: 0,
            fonts: Fonts::new(GetDpiForWindow(hwnd)),
            sampler,
            snapshot,
            panel_visible: true,
            taskbar_msg: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        };
        APP = Box::into_raw(Box::new(state));

        let app = app();
        app.icon = ui::make_icon(app.snapshot.level());
        add_tray(app);
        SetTimer(hwnd, TIMER_ID, 1000, None);

        ui::apply_round_corners(hwnd);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        ui::dock(hwnd);
        InvalidateRect(hwnd, null(), 0);

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn fill_nid(app: &App, nid: &mut NOTIFYICONDATAW, flags: u32) {
    *nid = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = app.hwnd;
    nid.uID = TRAY_UID;
    nid.uFlags = flags;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = app.icon;

    let tip = wide(&app.snapshot.tooltip());
    let len = tip.len().min(127);
    nid.szTip[..len].copy_from_slice(&tip[..len]);
}

unsafe fn add_tray(app: &App) {
    let mut nid: NOTIFYICONDATAW = zeroed();
    fill_nid(app, &mut nid, NIF_ICON | NIF_MESSAGE | NIF_TIP);
    Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn update_tray(app: &App) {
    let mut nid: NOTIFYICONDATAW = zeroed();
    fill_nid(app, &mut nid, NIF_ICON | NIF_MESSAGE | NIF_TIP);
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

unsafe fn tick(app: &mut App) {
    let snapshot = app.sampler.sample();
    let level_changed = snapshot.level() != app.snapshot.level();
    app.snapshot = snapshot;

    if level_changed {
        if app.icon != 0 {
            DestroyIcon(app.icon);
        }
        app.icon = ui::make_icon(app.snapshot.level());
    }
    update_tray(app);

    if app.panel_visible {
        InvalidateRect(app.hwnd, null(), 0);
    }
}

unsafe fn toggle_panel(app: &mut App) {
    app.panel_visible = !app.panel_visible;
    if app.panel_visible {
        ui::apply_round_corners(app.hwnd);
        ShowWindow(app.hwnd, SW_SHOWNOACTIVATE);
        ui::dock(app.hwnd);
        InvalidateRect(app.hwnd, null(), 0);
    } else {
        ShowWindow(app.hwnd, SW_HIDE);
    }
}

unsafe fn show_menu(app: &mut App) {
    let mut pt: POINT = zeroed();
    GetCursorPos(&mut pt);

    let menu = CreatePopupMenu();
    if menu == 0 {
        return;
    }
    let toggle_text = if app.panel_visible {
        "隐藏悬浮窗"
    } else {
        "显示悬浮窗"
    };
    AppendMenuW(menu, MF_STRING, MENU_TOGGLE as usize, wide(toggle_text).as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, MENU_QUIT as usize, wide("退出").as_ptr());

    // TrackPopupMenu 需要窗口在前台，否则点击别处菜单不会收起
    SetForegroundWindow(app.hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        app.hwnd,
        null(),
    );
    PostMessageW(app.hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);

    match cmd as u32 {
        MENU_TOGGLE => toggle_panel(app),
        MENU_QUIT => quit(app),
        _ => {}
    }
}

unsafe fn quit(app: &mut App) {
    let mut nid: NOTIFYICONDATAW = zeroed();
    fill_nid(app, &mut nid, NIF_ICON | NIF_MESSAGE | NIF_TIP);
    Shell_NotifyIconW(NIM_DELETE, &nid);

    KillTimer(app.hwnd, TIMER_ID);
    if app.icon != 0 {
        DestroyIcon(app.icon);
        app.icon = 0;
    }
    app.fonts.destroy();
    DestroyWindow(app.hwnd);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if APP.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = app();

    match msg {
        WM_PAINT => {
            ui::paint(hwnd, &app.snapshot, &mut app.fonts);
            0
        }
        WM_ERASEBKGND => 1,
        // 整个面板都可以拖动
        WM_NCHITTEST => HTCAPTION as LRESULT,
        WM_TIMER => {
            if wparam == TIMER_ID {
                tick(app);
            }
            0
        }
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP => toggle_panel(app),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(app),
                _ => {}
            }
            0
        }
        WM_SHOW_PANEL => {
            if !app.panel_visible {
                toggle_panel(app);
            }
            0
        }
        WM_DPICHANGED => {
            let suggested = lparam as *const RECT;
            if !suggested.is_null() {
                let rc = *suggested;
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOACTIVATE,
                );
                ui::apply_round_corners(hwnd);
            }
            0
        }
        WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
            if app.panel_visible {
                ui::dock(hwnd);
            }
            0
        }
        WM_CLOSE => {
            quit(app);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => {
            // 任务栏（explorer）重启后图标会丢失，需要重新挂上
            if msg == app.taskbar_msg {
                add_tray(app);
                0
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
    }
}
