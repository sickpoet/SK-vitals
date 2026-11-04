#![windows_subsystem = "windows"]

mod settings;
mod stats;
mod temp;
mod ui;

use std::mem::{size_of, zeroed};
use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, EndPaint};
use windows_sys::Win32::System::Environment::GetCommandLineW;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::Ime::ImmDisableIME;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use settings::Settings;
use stats::{Sampler, Snapshot};
use temp::TempReader;
use ui::{wide, Renderer};

const WM_TRAY: u32 = WM_APP + 1;
const WM_SHOW_PANEL: u32 = WM_APP + 2;
const TIMER_ID: usize = 1;
const TRAY_UID: u32 = 1;
const MENU_TOGGLE: u32 = 1;
const MENU_QUIT: u32 = 2;
const MENU_AUTOSTART: u32 = 3;
const MENU_PANEL_ON_START: u32 = 4;
const MENU_SHOW_TEMP: u32 = 5;
const CLASS_NAME: &str = "SKVitalsPanel";

struct App {
    hwnd: HWND,
    icon: HICON,
    renderer: Renderer,
    sampler: Sampler,
    temp: TempReader,
    snapshot: Snapshot,
    panel_visible: bool,
    taskbar_msg: u32,
    settings: Settings,
}

static mut APP: *mut App = std::ptr::null_mut();

unsafe fn app() -> &'static mut App {
    &mut *APP
}

fn main() {
    unsafe {
        // 尽早声明 DPI 感知，否则悬浮窗在缩放屏上会被系统拉伸糊掉
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // 程序没有任何文本输入，禁用输入法：避免打开托盘菜单时把输入法组件加载进进程
        ImmDisableIME(0);

        let class = wide(CLASS_NAME);

        // --startup 表示是开机自启拉起的；手动启动一律显示悬浮窗
        let startup = launched_by_startup();

        // 单实例：再次启动只是把已运行的悬浮窗显示出来
        let running = FindWindowW(class.as_ptr(), null());
        if running != 0 {
            PostMessageW(running, WM_SHOW_PANEL, 0, 0);
            return;
        }

        let hinst = GetModuleHandleW(null());
        // exe 资源里的应用图标（资源 ID 1），由 build.rs 用 winresource 嵌入
        let app_icon = LoadIconW(hinst, 1usize as *const u16);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: app_icon,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            return;
        }

        let dpi = GetDpiForSystem();
        let s = dpi as f32 / 96.0;
        let settings = Settings::load();
        let w = (ui::win_width(settings.show_temp) as f32 * s).round() as i32;
        let h = (ui::WIN_H as f32 * s).round() as i32;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
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
        let temp = TempReader::start();
        temp.set_enabled(settings.show_temp);
        let snapshot = sampler.sample().with_temp(temp.read(), settings.show_temp);
        let state = App {
            hwnd,
            icon: 0,
            renderer: Renderer::new(GetDpiForWindow(hwnd), settings.show_temp),
            sampler,
            temp,
            snapshot,
            panel_visible: !startup || settings.panel_on_start,
            taskbar_msg: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            settings,
        };
        APP = Box::into_raw(Box::new(state));

        let app = app();
        app.icon = ui::make_icon(app.snapshot.level());
        add_tray(app);
        SetTimer(hwnd, TIMER_ID, 1000, None);

        ui::dock(hwnd, app.settings.show_temp);
        if app.panel_visible {
            ui::render(hwnd, &app.snapshot, &mut app.renderer);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

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

fn exe_path() -> Option<String> {
    unsafe {
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(0, buf.as_mut_ptr(), buf.len() as u32);
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// 命令行里带 --startup 说明是开机自启拉起的
fn launched_by_startup() -> bool {
    unsafe {
        let cmd = GetCommandLineW();
        if cmd.is_null() {
            return false;
        }
        let mut len = 0usize;
        while *cmd.add(len) != 0 {
            len += 1;
        }
        let line = String::from_utf16_lossy(std::slice::from_raw_parts(cmd, len));
        line.split_whitespace().any(|a| a == "--startup")
    }
}

unsafe fn tick(app: &mut App) {
    let snapshot = app
        .sampler
        .sample()
        .with_temp(app.temp.read(), app.settings.show_temp);
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
        ui::render(app.hwnd, &app.snapshot, &mut app.renderer);
    }
}

unsafe fn toggle_panel(app: &mut App) {
    app.panel_visible = !app.panel_visible;
    if app.panel_visible {
        ShowWindow(app.hwnd, SW_SHOWNOACTIVATE);
        ui::dock(app.hwnd, app.settings.show_temp);
        ui::render(app.hwnd, &app.snapshot, &mut app.renderer);
    } else {
        ShowWindow(app.hwnd, SW_HIDE);
    }
}

/// 切换温度栏：卡片宽度会变，要重新分配位图并重排
unsafe fn toggle_show_temp(app: &mut App) {
    app.settings.show_temp = !app.settings.show_temp;
    app.settings.save();
    app.temp.set_enabled(app.settings.show_temp);
    app.renderer.set_show_temp(app.settings.show_temp);

    let dpi = GetDpiForWindow(app.hwnd);
    let s = dpi as f32 / 96.0;
    let w = (ui::win_width(app.settings.show_temp) as f32 * s).round() as i32;
    let h = (ui::WIN_H as f32 * s).round() as i32;
    SetWindowPos(app.hwnd, HWND_TOPMOST, 0, 0, w, h, SWP_NOMOVE | SWP_NOACTIVATE);

    ui::dock(app.hwnd, app.settings.show_temp);
    if app.panel_visible {
        ui::render(app.hwnd, &app.snapshot, &mut app.renderer);
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

    let autostart_flags = if settings::is_autostart_enabled() {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    AppendMenuW(
        menu,
        autostart_flags,
        MENU_AUTOSTART as usize,
        wide("开机自启").as_ptr(),
    );

    let panel_flags = if app.settings.panel_on_start {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    AppendMenuW(
        menu,
        panel_flags,
        MENU_PANEL_ON_START as usize,
        wide("启动时显示悬浮窗").as_ptr(),
    );

    let temp_flags = if app.settings.show_temp {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    AppendMenuW(
        menu,
        temp_flags,
        MENU_SHOW_TEMP as usize,
        wide("显示温度").as_ptr(),
    );

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
        MENU_AUTOSTART => {
            let enable = !settings::is_autostart_enabled();
            let path = exe_path().unwrap_or_default();
            if !settings::set_autostart(enable, &path) {
                MessageBoxW(
                    app.hwnd,
                    wide("写入开机启动项失败。").as_ptr(),
                    wide("SK-vitals").as_ptr(),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
        MENU_PANEL_ON_START => {
            app.settings.panel_on_start = !app.settings.panel_on_start;
            app.settings.save();
            // 取消勾选时顺手把当前小窗也收起来，让效果立刻可见
            if !app.settings.panel_on_start && app.panel_visible {
                app.panel_visible = false;
                ShowWindow(app.hwnd, SW_HIDE);
            }
        }
        MENU_SHOW_TEMP => toggle_show_temp(app),
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
    app.renderer.destroy();
    DestroyWindow(app.hwnd);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if APP.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = app();

    match msg {
        WM_PAINT => {
            // 分层窗口不使用 GDI 绘制，这里只补一次空擦除
            let mut ps = zeroed();
            BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &mut ps);
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
            }
            ui::dock(hwnd, app.settings.show_temp);
            if app.panel_visible {
                ui::render(hwnd, &app.snapshot, &mut app.renderer);
            }
            0
        }
        WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
            if app.panel_visible {
                ui::dock(hwnd, app.settings.show_temp);
                ui::render(hwnd, &app.snapshot, &mut app.renderer);
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
