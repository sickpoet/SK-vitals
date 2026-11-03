use std::ffi::OsStr;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::ptr::null;

use windows_sys::Win32::Foundation::{COLORREF, HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::stats::{Level, Snapshot};

/// 96 DPI 下的逻辑尺寸，实际按屏幕 DPI 缩放
pub const PANEL_W: i32 = 176;
pub const PANEL_H: i32 = 58;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

const BG: COLORREF = rgb(18, 21, 28);
const BORDER: COLORREF = rgb(42, 48, 64);
const TRACK: COLORREF = rgb(28, 33, 48);
const LABEL: COLORREF = rgb(148, 158, 180);
const VALUE: COLORREF = rgb(232, 238, 248);

const COLOR_NORMAL: COLORREF = rgb(56, 199, 132);
const COLOR_WARN: COLORREF = rgb(241, 176, 58);
const COLOR_CRITICAL: COLORREF = rgb(239, 80, 80);

fn color_for(level: Level) -> COLORREF {
    match level {
        Level::Normal => COLOR_NORMAL,
        Level::Warn => COLOR_WARN,
        Level::Critical => COLOR_CRITICAL,
    }
}

pub fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
}

pub fn panel_size(dpi: u32) -> (i32, i32) {
    let scale = dpi as f32 / 96.0;
    (
        (PANEL_W as f32 * scale).round() as i32,
        (PANEL_H as f32 * scale).round() as i32,
    )
}

pub struct Fonts {
    label: HFONT,
    value: HFONT,
    dpi: u32,
}

impl Fonts {
    pub fn new(dpi: u32) -> Self {
        Fonts {
            label: make_font(dpi, 10, 500),
            value: make_font(dpi, 12, 600),
            dpi,
        }
    }

    pub fn sync(&mut self, dpi: u32) {
        if self.dpi != dpi {
            self.destroy();
            *self = Fonts::new(dpi);
        }
    }

    pub fn destroy(&mut self) {
        unsafe {
            if self.label != 0 {
                DeleteObject(self.label as HGDIOBJ);
                self.label = 0;
            }
            if self.value != 0 {
                DeleteObject(self.value as HGDIOBJ);
                self.value = 0;
            }
        }
    }
}

fn make_font(dpi: u32, px: i32, weight: i32) -> HFONT {
    let height = -(px * dpi as i32) / 96;
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide("Microsoft YaHei UI").as_ptr(),
        )
    }
}

pub fn paint(hwnd: HWND, snapshot: &Snapshot, fonts: &mut Fonts) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        fonts.sync(dpi);
        let scale = dpi as f32 / 96.0;

        let mut ps: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = zeroed();
        GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        if w <= 0 || h <= 0 {
            EndPaint(hwnd, &ps);
            return;
        }

        // 双缓冲，避免每秒重绘时闪烁
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, bmp);

        let bg = CreateSolidBrush(BG);
        let full = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        FillRect(mem, &full, bg);

        // 1px 内描边
        let border = CreateSolidBrush(BORDER);
        let edges = [
            RECT { left: 0, top: 0, right: w, bottom: 1 },
            RECT { left: 0, top: h - 1, right: w, bottom: h },
            RECT { left: 0, top: 0, right: 1, bottom: h },
            RECT { left: w - 1, top: 0, right: w, bottom: h },
        ];
        for edge in &edges {
            FillRect(mem, edge, border);
        }

        SetBkMode(mem, TRANSPARENT as i32);

        let pad = (10.0 * scale).round() as i32;
        let row_h = (14.0 * scale).round() as i32;
        let bar_h = ((4.0 * scale).round() as i32).max(3);
        let y1 = (7.0 * scale).round() as i32;
        let bar1_y = y1 + row_h + (2.0 * scale).round() as i32;
        let y2 = bar1_y + bar_h + (5.0 * scale).round() as i32;
        let bar2_y = y2 + row_h + (2.0 * scale).round() as i32;

        draw_row(mem, fonts, pad, w - pad, y1, row_h, "CPU", snapshot.cpu, snapshot.cpu_level);
        draw_row(mem, fonts, pad, w - pad, y2, row_h, "内存", snapshot.mem, snapshot.mem_level);
        draw_bar(mem, pad, bar1_y, w - pad, bar_h, snapshot.cpu, color_for(snapshot.cpu_level));
        draw_bar(mem, pad, bar2_y, w - pad, bar_h, snapshot.mem, color_for(snapshot.mem_level));

        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);

        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        DeleteObject(bg);
        DeleteObject(border);
        EndPaint(hwnd, &ps);
    }
}

unsafe fn draw_row(
    hdc: HDC,
    fonts: &Fonts,
    left: i32,
    right: i32,
    top: i32,
    height: i32,
    label: &str,
    value: u8,
    level: Level,
) {
    let label_text = wide(label);
    let mut rc = RECT {
        left,
        top,
        right,
        bottom: top + height,
    };
    SetTextColor(hdc, LABEL);
    SelectObject(hdc, fonts.label);
    DrawTextW(hdc, label_text.as_ptr(), -1, &mut rc, DT_LEFT | DT_SINGLELINE | DT_VCENTER);

    let value_text = wide(&format!("{}%", value));
    let mut rc = RECT {
        left,
        top,
        right,
        bottom: top + height,
    };
    SetTextColor(hdc, if level == Level::Normal { VALUE } else { color_for(level) });
    SelectObject(hdc, fonts.value);
    DrawTextW(hdc, value_text.as_ptr(), -1, &mut rc, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
}

unsafe fn draw_bar(hdc: HDC, left: i32, top: i32, right: i32, height: i32, pct: u8, color: COLORREF) {
    if right <= left {
        return;
    }
    let radius = (height * 2) as i32;

    let track_brush = CreateSolidBrush(TRACK);
    let track_pen = CreatePen(PS_SOLID, 1, TRACK);
    let old_brush = SelectObject(hdc, track_brush);
    let old_pen = SelectObject(hdc, track_pen);
    RoundRect(hdc, left, top, right, top + height, radius, radius);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    DeleteObject(track_brush);
    DeleteObject(track_pen);

    let filled = (right - left) as f32 * (pct as f32 / 100.0);
    if filled >= 2.0 {
        let end = left + (filled.max(radius as f32)) as i32;
        let fill_brush = CreateSolidBrush(color);
        let fill_pen = CreatePen(PS_SOLID, 1, color);
        let old_brush = SelectObject(hdc, fill_brush);
        let old_pen = SelectObject(hdc, fill_pen);
        RoundRect(hdc, left, top, end.min(right), top + height, radius, radius);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(fill_brush);
        DeleteObject(fill_pen);
    }
}

/// 托盘图标：深色圆角底 + 三根柱子，颜色随告警等级变化
pub fn make_icon(level: Level) -> HICON {
    const S: i32 = 32;
    let accent = color_for(level);

    unsafe {
        let screen = GetDC(0);
        let dc = CreateCompatibleDC(screen);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: S,
                biHeight: -S,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let color_bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
        if color_bmp == 0 {
            DeleteDC(dc);
            ReleaseDC(0, screen);
            return 0;
        }
        let old_bmp = SelectObject(dc, color_bmp);

        // 底板
        let bg_brush = CreateSolidBrush(BG);
        let bg_pen = CreatePen(PS_SOLID, 1, BORDER);
        SelectObject(dc, bg_brush);
        SelectObject(dc, bg_pen);
        RoundRect(dc, 3, 3, 29, 29, 8, 8);

        // 三根柱子，模拟 CPU / 内存的读数条
        let bar_brush = CreateSolidBrush(accent);
        let bar_pen = CreatePen(PS_SOLID, 1, accent);
        SelectObject(dc, bar_brush);
        SelectObject(dc, bar_pen);
        RoundRect(dc, 9, 17, 13, 25, 3, 3);
        RoundRect(dc, 15, 12, 19, 25, 3, 3);
        RoundRect(dc, 21, 7, 25, 25, 3, 3);

        DeleteObject(bg_brush);
        DeleteObject(bg_pen);
        DeleteObject(bar_brush);
        DeleteObject(bar_pen);

        // GDI 画 32bpp DIB 时 alpha 全为 0，按“有颜色即不透明”补回
        if !bits.is_null() {
            let pixels = bits as *mut u32;
            for i in 0..(S * S) {
                let px = *pixels.offset(i as isize);
                if px & 0x00FF_FFFF != 0 {
                    *pixels.offset(i as isize) = px | 0xFF00_0000;
                }
            }
        }

        SelectObject(dc, old_bmp);

        let mask = CreateBitmap(S, S, 1, 1, std::ptr::null());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color_bmp,
        };
        let icon = CreateIconIndirect(&info);

        DeleteObject(mask);
        DeleteObject(color_bmp);
        DeleteDC(dc);
        ReleaseDC(0, screen);
        icon
    }
}

/// 贴靠任务栏：优先放到托盘通知区正上方，取不到再退回工作区右下角
pub fn dock(hwnd: HWND) {
    unsafe {
        let mut rc: RECT = zeroed();
        GetWindowRect(hwnd, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        let (x, y) = anchor(w, h);
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
    }
}

fn anchor(w: i32, h: i32) -> (i32, i32) {
    unsafe {
        // TrayNotifyWnd 是 Shell_TrayWnd 的子窗口，FindWindow 找不到它
        let shell = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
        if shell != 0 {
            let tray = FindWindowExW(shell, 0, wide("TrayNotifyWnd").as_ptr(), null());
            if tray != 0 {
                let mut rc: RECT = zeroed();
                if GetWindowRect(tray, &mut rc) != 0 && rc.right > 0 {
                    let y = if rc.top - h - 6 > 0 {
                        rc.top - h - 6
                    } else {
                        rc.bottom + 6
                    };
                    return ((rc.right - w - 4).max(0), y);
                }
            }
        }

        let mut work: RECT = zeroed();
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            &mut work as *mut RECT as *mut std::ffi::c_void,
            0,
        );
        if work.right <= work.left || work.bottom <= work.top {
            return (16, 16);
        }
        (
            (work.right - w - 12).max(work.left),
            (work.bottom - h - 12).max(work.top),
        )
    }
}

pub fn apply_round_corners(hwnd: HWND) {
    unsafe {
        let mut rc: RECT = zeroed();
        GetClientRect(hwnd, &mut rc);
        if rc.right <= 0 || rc.bottom <= 0 {
            return;
        }
        let radius = ((12.0 * GetDpiForWindow(hwnd) as f32 / 96.0).round() as i32).max(6);
        let rgn = CreateRoundRectRgn(0, 0, rc.right + 1, rc.bottom + 1, radius, radius);
        if rgn != 0 {
            SetWindowRgn(hwnd, rgn, 1);
        }
    }
}
