use std::ffi::OsStr;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::stats::{Level, Snapshot};

/// 卡片尺寸（96 DPI 逻辑单位，不含阴影留白）
const CARD_W: i32 = 236;
const CARD_H: i32 = 50;
/// 卡片四周为投影预留的空白
const MARGIN_X: i32 = 12;
const MARGIN_TOP: i32 = 10;
const MARGIN_BOTTOM: i32 = 16;

/// 整个窗口尺寸（卡片 + 阴影留白）
pub const WIN_W: i32 = CARD_W + MARGIN_X * 2;
pub const WIN_H: i32 = CARD_H + MARGIN_TOP + MARGIN_BOTTOM;

const CARD_RADIUS: f32 = 13.0;
const CARD_ALPHA: f32 = 0.95;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_BLUR: f32 = 8.0;
const SHADOW_ALPHA: f32 = 0.45;

// 卡片配色
const CARD_TOP: (u8, u8, u8) = (32, 39, 53);
const CARD_BOTTOM: (u8, u8, u8) = (20, 25, 34);
const LABEL_COLOR: (u8, u8, u8) = (139, 149, 168);
const VALUE_COLOR: (u8, u8, u8) = (238, 242, 248);
const TRACK_COLOR: (u8, u8, u8) = (44, 51, 66);

const COLOR_NORMAL: (u8, u8, u8) = (61, 214, 140);
const COLOR_WARN: (u8, u8, u8) = (245, 176, 59);
const COLOR_CRITICAL: (u8, u8, u8) = (240, 80, 80);

fn color_for(level: Level) -> (u8, u8, u8) {
    match level {
        Level::Normal => COLOR_NORMAL,
        Level::Warn => COLOR_WARN,
        Level::Critical => COLOR_CRITICAL,
    }
}

pub fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
}

fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// 圆角矩形有向距离场（负数在内部）
fn sd_round_box(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - hw + r;
    let qy = (py - cy).abs() - hh + r;
    let mx = qx.max(0.0);
    let my = qy.max(0.0);
    (mx * mx + my * my).sqrt() + qx.max(qy).min(0.0) - r
}

/// 距离转 1px 抗锯齿覆盖率
fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// 可分离盒式模糊，两遍逼近高斯
fn blur(src: Vec<f32>, w: i32, h: i32, r: i32) -> Vec<f32> {
    if r < 1 {
        return src;
    }
    let n = (w * h) as usize;
    let mut a = src;
    let mut b = vec![0f32; n];
    for _ in 0..2 {
        // 水平
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0f32;
                let mut cnt = 0f32;
                for k in -r..=r {
                    let xx = x + k;
                    if xx >= 0 && xx < w {
                        sum += a[(y * w + xx) as usize];
                        cnt += 1.0;
                    }
                }
                b[(y * w + x) as usize] = sum / cnt;
            }
        }
        // 垂直
        for x in 0..w {
            for y in 0..h {
                let mut sum = 0f32;
                let mut cnt = 0f32;
                for k in -r..=r {
                    let yy = y + k;
                    if yy >= 0 && yy < h {
                        sum += b[(yy * w + x) as usize];
                        cnt += 1.0;
                    }
                }
                a[(y * w + x) as usize] = sum / cnt;
            }
        }
    }
    a
}

struct Fonts {
    label: HFONT,
    value: HFONT,
}

impl Fonts {
    fn new(dpi: u32) -> Self {
        Fonts {
            label: make_font(dpi, 11, 400, "Microsoft YaHei UI"),
            value: make_font(dpi, 17, 600, "Segoe UI"),
        }
    }

    fn destroy(&mut self) {
        unsafe {
            if self.label != 0 {
                DeleteObject(self.label);
                self.label = 0;
            }
            if self.value != 0 {
                DeleteObject(self.value);
                self.value = 0;
            }
        }
    }
}

fn make_font(dpi: u32, px: i32, weight: i32, face: &str) -> HFONT {
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
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide(face).as_ptr(),
        )
    }
}

/// 渲染器：持有分层窗口用的 DIB、文本掩码用的临时 DIB 与字体
pub struct Renderer {
    dpi: u32,
    fonts: Fonts,
    // 最终合成的窗口位图
    dc: HDC,
    dib: HBITMAP,
    bits: *mut u32,
    old_bmp: HGDIOBJ,
    // 文本抗锯齿掩码
    tdc: HDC,
    tdib: HBITMAP,
    tbits: *mut u32,
    told_bmp: HGDIOBJ,
    cw: i32,
    ch: i32,
    ww: i32,
    wh: i32,
}

impl Renderer {
    pub fn new(dpi: u32) -> Self {
        let mut r = Renderer {
            dpi,
            fonts: Fonts::new(dpi),
            dc: 0,
            dib: 0,
            bits: null_mut(),
            old_bmp: 0,
            tdc: 0,
            tdib: 0,
            tbits: null_mut(),
            told_bmp: 0,
            cw: 0,
            ch: 0,
            ww: 0,
            wh: 0,
        };
        r.alloc(dpi);
        r
    }

    fn alloc(&mut self, dpi: u32) {
        unsafe {
            let s = dpi as f32 / 96.0;
            self.cw = (CARD_W as f32 * s).round() as i32;
            self.ch = (CARD_H as f32 * s).round() as i32;
            self.ww = (WIN_W as f32 * s).round() as i32;
            self.wh = (WIN_H as f32 * s).round() as i32;

            let screen = GetDC(0);

            if self.dc != 0 {
                SelectObject(self.dc, self.old_bmp);
                DeleteObject(self.dib);
                DeleteDC(self.dc);
            }
            if self.tdc != 0 {
                SelectObject(self.tdc, self.told_bmp);
                DeleteObject(self.tdib);
                DeleteDC(self.tdc);
            }

            self.dc = CreateCompatibleDC(screen);
            let (bmp, bits) = create_dib(self.dc, self.ww, self.wh);
            self.dib = bmp;
            self.bits = bits;
            self.old_bmp = SelectObject(self.dc, self.dib);

            self.tdc = CreateCompatibleDC(screen);
            let (tbmp, tbits) = create_dib(self.tdc, self.cw, self.ch);
            self.tdib = tbmp;
            self.tbits = tbits;
            self.told_bmp = SelectObject(self.tdc, self.tdib);

            ReleaseDC(0, screen);
        }
    }

    fn sync(&mut self, dpi: u32) {
        if self.dpi != dpi {
            self.fonts.destroy();
            self.fonts = Fonts::new(dpi);
            self.alloc(dpi);
            self.dpi = dpi;
        }
    }

    pub fn destroy(&mut self) {
        unsafe {
            if self.dc != 0 {
                SelectObject(self.dc, self.old_bmp);
                DeleteObject(self.dib);
                DeleteDC(self.dc);
                self.dc = 0;
            }
            if self.tdc != 0 {
                SelectObject(self.tdc, self.told_bmp);
                DeleteObject(self.tdib);
                DeleteDC(self.tdc);
                self.tdc = 0;
            }
        }
        self.fonts.destroy();
    }
}

unsafe fn create_dib(dc: HDC, w: i32, h: i32) -> (HBITMAP, *mut u32) {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
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
    let mut bits: *mut std::ffi::c_void = null_mut();
    let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
    (bmp, bits as *mut u32)
}

/// 把一个颜色按覆盖率混进 RGB 缓冲的某个像素
fn blend_px(buf: &mut [u8], i: usize, color: (u8, u8, u8), amount: f32) {
    for k in 0..3 {
        let c = match k {
            0 => color.0,
            1 => color.1,
            _ => color.2,
        } as f32;
        buf[i + k] = (buf[i + k] as f32 + (c - buf[i + k] as f32) * amount).round() as u8;
    }
}

/// 在卡片缓冲上叠加一个抗锯齿圆角矩形（坐标为卡片内坐标）
fn blend_round_rect(
    buf: &mut [u8],
    cw: i32,
    ch: i32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    radius: f32,
    color: (u8, u8, u8),
    alpha: f32,
) {
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let hw = (x1 - x0) * 0.5;
    let hh = (y1 - y0) * 0.5;
    let y_start = y0.floor() as i32;
    let y_end = (y1.ceil() as i32).min(ch);
    let x_start = x0.floor() as i32;
    let x_end = (x1.ceil() as i32).min(cw);
    for y in y_start.max(0)..y_end {
        for x in x_start.max(0)..x_end {
            let d = sd_round_box(x as f32 + 0.5, y as f32 + 0.5, cx, cy, hw, hh, radius);
            let cov = coverage(d) * alpha;
            if cov <= 0.0 {
                continue;
            }
            let i = ((y * cw + x) * 3) as usize;
            blend_px(buf, i, color, cov);
        }
    }
}

pub fn render(hwnd: HWND, snapshot: &Snapshot, r: &mut Renderer) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        r.sync(dpi);
        let s = dpi as f32 / 96.0;

        let cw = r.cw;
        let ch = r.ch;
        let ww = r.ww;
        let wh = r.wh;
        let mx = ((MARGIN_X as f32) * s).round() as i32;
        let mt = ((MARGIN_TOP as f32) * s).round() as i32;

        // ---------- 卡片内容（不透明 RGB） ----------
        let mut card = vec![0u8; (cw * ch * 3) as usize];
        for y in 0..ch {
            let t = if ch > 1 {
                y as f32 / (ch - 1) as f32
            } else {
                0.0
            };
            let c = lerp(CARD_TOP, CARD_BOTTOM, t);
            for x in 0..cw {
                let i = ((y * cw + x) * 3) as usize;
                card[i] = c.0;
                card[i + 1] = c.1;
                card[i + 2] = c.2;
            }
        }

        let pad = (18.0 * s).round() as i32;
        let col_gap = (32.0 * s).round() as i32;
        let col_w = (cw - pad * 2 - col_gap) / 2;
        let col2_x = pad + col_w + col_gap;
        let row_top = (8.0 * s).round() as i32;
        let row_h = (22.0 * s).round() as i32;
        let bar_h = ((4.0 * s).round() as i32).max(3);
        let bar_y = (35.0 * s).round() as i32;

        // 分隔线
        let div_x = pad + col_w + col_gap / 2;
        let div_top = (10.0 * s).round() as i32;
        let div_bottom = (40.0 * s).round() as i32;
        for y in div_top..div_bottom.min(ch) {
            for x in (div_x - 1).max(0)..(div_x + 1).min(cw) {
                let i = ((y * cw + x) * 3) as usize;
                buf_blend_white(&mut card, i, 0.08);
            }
        }

        // 两条进度条
        draw_bar(&mut card, cw, ch, pad, bar_y, col_w, bar_h, snapshot.cpu, color_for(snapshot.cpu_level));
        draw_bar(
            &mut card,
            cw,
            ch,
            col2_x,
            bar_y,
            col_w,
            bar_h,
            snapshot.mem,
            color_for(snapshot.mem_level),
        );

        // 文本
        draw_metric(
            r,
            &mut card,
            cw,
            ch,
            pad,
            row_top,
            row_h,
            "CPU",
            snapshot.cpu,
            snapshot.cpu_level,
        );
        draw_metric(
            r,
            &mut card,
            cw,
            ch,
            col2_x,
            row_top,
            row_h,
            "内存",
            snapshot.mem,
            snapshot.mem_level,
        );

        // ---------- 投影 ----------
        let radius = CARD_RADIUS * s;
        let so = SHADOW_OFFSET * s;
        let mut mask = vec![0f32; (ww * wh) as usize];
        for y in 0..wh {
            for x in 0..ww {
                let d = sd_round_box(
                    x as f32 + 0.5,
                    y as f32 + 0.5 - so,
                    mx as f32 + cw as f32 * 0.5,
                    mt as f32 + ch as f32 * 0.5,
                    cw as f32 * 0.5,
                    ch as f32 * 0.5,
                    radius,
                );
                mask[(y * ww + x) as usize] = coverage(d);
            }
        }
        let blur_r = (SHADOW_BLUR * s * 0.5).round() as i32;
        let mask = blur(mask, ww, wh, blur_r.max(1));

        // ---------- 合成到窗口位图 ----------
        let bits = r.bits;
        for y in 0..wh {
            for x in 0..ww {
                let s_alpha = mask[(y * ww + x) as usize] * SHADOW_ALPHA;
                let (cr, cg, cb, c_alpha) = if x >= mx && x < mx + cw && y >= mt && y < mt + ch {
                    let lx = x - mx;
                    let ly = y - mt;
                    let d = sd_round_box(
                        lx as f32 + 0.5,
                        ly as f32 + 0.5,
                        cw as f32 * 0.5,
                        ch as f32 * 0.5,
                        cw as f32 * 0.5,
                        ch as f32 * 0.5,
                        radius,
                    );
                    let cov = coverage(d);
                    let i = ((ly * cw + lx) * 3) as usize;
                    (
                        card[i] as f32,
                        card[i + 1] as f32,
                        card[i + 2] as f32,
                        cov * CARD_ALPHA,
                    )
                } else {
                    (0.0, 0.0, 0.0, 0.0)
                };

                let a = s_alpha + c_alpha * (1.0 - s_alpha);
                let pr = cr * c_alpha * (1.0 - s_alpha);
                let pg = cg * c_alpha * (1.0 - s_alpha);
                let pb = cb * c_alpha * (1.0 - s_alpha);
                let px = ((a * 255.0) as u32) << 24
                    | ((pr.clamp(0.0, 255.0)) as u32) << 16
                    | ((pg.clamp(0.0, 255.0)) as u32) << 8
                    | (pb.clamp(0.0, 255.0)) as u32;
                *bits.offset((y * ww + x) as isize) = px;
            }
        }

        // ---------- 提交到分层窗口 ----------
        let screen = GetDC(0);
        let size = SIZE {
            cx: ww,
            cy: wh,
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hwnd,
            screen,
            null(),
            &size,
            r.dc,
            &src,
            0,
            &blend,
            ULW_ALPHA,
        );
        ReleaseDC(0, screen);
    }
}

fn buf_blend_white(buf: &mut [u8], i: usize, amount: f32) {
    for k in 0..3 {
        buf[i + k] = (buf[i + k] as f32 + (255.0 - buf[i + k] as f32) * amount).round() as u8;
    }
}

fn draw_bar(
    buf: &mut [u8],
    cw: i32,
    ch: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    pct: u8,
    color: (u8, u8, u8),
) {
    let radius = h as f32 * 0.5;
    blend_round_rect(buf, cw, ch, x as f32, y as f32, (x + w) as f32, (y + h) as f32, radius, TRACK_COLOR, 1.0);
    let fill = ((w as f32) * (pct as f32 / 100.0)).round() as i32;
    if fill >= 1 {
        let fw = fill.min(w);
        let r = radius.min(fw as f32 * 0.5);
        blend_round_rect(
            buf,
            cw,
            ch,
            x as f32,
            y as f32,
            (x + fw) as f32,
            (y + h) as f32,
            r,
            color,
            1.0,
        );
    }
}

/// 量一段文字的像素宽度（DrawText + DT_CALCRECT）
unsafe fn text_width(r: &mut Renderer, font: HFONT, text: &str) -> i32 {
    SelectObject(r.tdc, font);
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let _ = DrawTextW(
        r.tdc,
        wide(text).as_ptr(),
        -1,
        &mut rc,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    rc.right - rc.left
}

/// 画一列指标：标签 + 大号数值（数值紧跟标签，整体靠左）
unsafe fn draw_metric(
    r: &mut Renderer,
    card: &mut [u8],
    cw: i32,
    ch: i32,
    x: i32,
    top: i32,
    height: i32,
    label: &str,
    value: u8,
    level: Level,
) {
    let f_label = r.fonts.label;
    let f_value = r.fonts.value;

    let text = format!("{}%", value);
    let label_w = text_width(r, f_label, label);
    let value_w = text_width(r, f_value, &text);
    let gap = (9.0 * r.dpi as f32 / 96.0).round() as i32;

    // 标签
    draw_text_mask(
        r,
        card,
        cw,
        ch,
        f_label,
        label,
        RECT {
            left: x,
            top,
            right: x + label_w + 2,
            bottom: top + height,
        },
        LABEL_COLOR,
        DT_LEFT,
    );

    // 数值：大号数字（带百分号）
    let value_color = if level == Level::Normal {
        VALUE_COLOR
    } else {
        color_for(level)
    };
    let vx = x + label_w + gap;
    draw_text_mask(
        r,
        card,
        cw,
        ch,
        f_value,
        &text,
        RECT {
            left: vx,
            top,
            right: vx + value_w + 2,
            bottom: top + height,
        },
        value_color,
        DT_LEFT,
    );
}

/// 用 GDI 把文字画成掩码，再按覆盖率混色，得到带 alpha 的抗锯齿文字
unsafe fn draw_text_mask(
    r: &mut Renderer,
    card: &mut [u8],
    cw: i32,
    ch: i32,
    font: HFONT,
    text: &str,
    rect: RECT,
    color: (u8, u8, u8),
    align: u32,
) {
    let x0 = rect.left.max(0);
    let y0 = rect.top.max(0);
    let x1 = rect.right.min(cw);
    let y1 = rect.bottom.min(ch);
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    // 清空掩码区域
    for y in y0..y1 {
        for x in x0..x1 {
            *r.tbits.offset((y * cw + x) as isize) = 0xFF00_0000;
        }
    }

    SelectObject(r.tdc, font);
    SetBkMode(r.tdc, OPAQUE as i32);
    SetBkColor(r.tdc, 0x0000_0000);
    SetTextColor(r.tdc, 0x00FF_FFFF);
    let mut rc = rect;
    let flags = DT_SINGLELINE | DT_BOTTOM | DT_NOPREFIX | align;
    DrawTextW(r.tdc, wide(text).as_ptr(), -1, &mut rc, flags);

    // 掩码是白色字 + 黑底，取红通道当覆盖率
    for y in y0..y1 {
        for x in x0..x1 {
            let px = *r.tbits.offset((y * cw + x) as isize);
            let red = ((px >> 16) & 0xFF) as f32 / 255.0;
            if red <= 0.0 {
                continue;
            }
            let i = ((y * cw + x) * 3) as usize;
            blend_px(card, i, color, red);
        }
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

        let mut bits: *mut std::ffi::c_void = null_mut();
        let color_bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
        if color_bmp == 0 {
            DeleteDC(dc);
            ReleaseDC(0, screen);
            return 0;
        }
        let old_bmp = SelectObject(dc, color_bmp);

        let bg = CreateSolidBrush(rgb_of(CARD_TOP));
        let bg_pen = CreatePen(PS_SOLID, 1, rgb_of(CARD_BOTTOM));
        SelectObject(dc, bg);
        SelectObject(dc, bg_pen);
        RoundRect(dc, 3, 3, 29, 29, 8, 8);

        let bar_brush = CreateSolidBrush(rgb_of(accent));
        let bar_pen = CreatePen(PS_SOLID, 1, rgb_of(accent));
        SelectObject(dc, bar_brush);
        SelectObject(dc, bar_pen);
        RoundRect(dc, 9, 17, 13, 25, 3, 3);
        RoundRect(dc, 15, 12, 19, 25, 3, 3);
        RoundRect(dc, 21, 7, 25, 25, 3, 3);

        DeleteObject(bg);
        DeleteObject(bg_pen);
        DeleteObject(bar_brush);
        DeleteObject(bar_pen);

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

        let mask = CreateBitmap(S, S, 1, 1, null());
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

fn rgb_of(c: (u8, u8, u8)) -> COLORREF {
    (c.0 as u32) | ((c.1 as u32) << 8) | ((c.2 as u32) << 16)
}

/// 贴靠任务栏：优先放到托盘通知区正上方，取不到再退回工作区右下角
pub fn dock(hwnd: HWND) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        let s = dpi as f32 / 96.0;
        let cw = (CARD_W as f32 * s).round() as i32;
        let ch = (CARD_H as f32 * s).round() as i32;
        let mw = (MARGIN_X as f32 * s).round() as i32;
        let mt = (MARGIN_TOP as f32 * s).round() as i32;
        let ww = (WIN_W as f32 * s).round() as i32;
        let wh = (WIN_H as f32 * s).round() as i32;

        let (card_x, card_y) = anchor(cw, ch);
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            card_x - mw,
            card_y - mt,
            ww,
            wh,
            SWP_NOACTIVATE,
        );
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
