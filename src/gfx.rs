//! CPU-only drawing: a 32-bit DIB filled by hand, with GDI used for text.

use crate::model::Icon;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

impl Rect {
    pub fn r(&self) -> i32 {
        self.x + self.w
    }
    pub fn b(&self) -> i32 {
        self.y + self.h
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.r() && y < self.b()
    }
    pub fn inset(&self, dx: i32, dy: i32) -> Rect {
        rect(self.x + dx, self.y + dy, self.w - dx * 2, self.h - dy * 2)
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        rect(x, y, (self.r().min(o.r()) - x).max(0), (self.b().min(o.b()) - y).max(0))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Body,
    Semi,
    Small,
    Title,
    Icon,
    IconSmall,
    #[allow(dead_code)]
    IconLarge,
    /// Large numbers on the performance page.
    Value,
    /// Large page heading on the performance page.
    Display,
}

const FONT_COUNT: usize = 9;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy)]
struct Face {
    latin: HFONT,
    cjk: HFONT,
    /// Distance from the vertical center to the baseline.
    baseline: i32,
    height: i32,
    icon: bool,
}

pub struct Canvas {
    hdc: HDC,
    bmp: HBITMAP,
    old_bmp: HGDIOBJ,
    bits: *mut u32,
    stride: i32,
    cap_h: i32,
    pub w: i32,
    pub h: i32,
    clip: Rect,
    faces: [Face; FONT_COUNT],
    current_font: HFONT,
    current_color: u32,
    wide: Vec<u16>,
    pub scale: f32,
}

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn font_exists(hdc: HDC, name: &str) -> bool {
    unsafe extern "system" fn found(
        _: *const LOGFONTW,
        _: *const TEXTMETRICW,
        _: u32,
        out: LPARAM,
    ) -> i32 {
        *(out as *mut bool) = true;
        0
    }
    let mut lf: LOGFONTW = unsafe { std::mem::zeroed() };
    for (i, c) in name.encode_utf16().take(31).enumerate() {
        lf.lfFaceName[i] = c;
    }
    lf.lfCharSet = DEFAULT_CHARSET as u8;
    let mut exists = false;
    unsafe { EnumFontFamiliesExW(hdc, &lf, Some(found), &mut exists as *mut bool as LPARAM, 0) };
    exists
}

fn make_font(name: &str, px: i32) -> HFONT {
    let wide = wide_z(name);
    unsafe {
        CreateFontW(
            -px,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_NATURAL_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE as u8) as u32,
            wide.as_ptr(),
        )
    }
}

impl Canvas {
    pub fn new(scale: f32) -> Self {
        let hdc = unsafe { CreateCompatibleDC(null_mut()) };
        unsafe {
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextAlign(hdc, TA_BASELINE as u32);
        }
        let empty = Face {
            latin: null_mut(),
            cjk: null_mut(),
            baseline: 0,
            height: 0,
            icon: false,
        };
        let mut c = Self {
            hdc,
            bmp: null_mut(),
            old_bmp: null_mut(),
            bits: null_mut(),
            stride: 0,
            cap_h: 0,
            w: 0,
            h: 0,
            clip: Rect::default(),
            faces: [empty; FONT_COUNT],
            current_font: null_mut(),
            current_color: u32::MAX,
            wide: Vec::with_capacity(512),
            scale,
        };
        c.set_scale(scale);
        c
    }

    pub fn px(&self, v: i32) -> i32 {
        (v as f32 * self.scale).round() as i32
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
        let (hdc, old) = (self.hdc, self.faces);
        let body = if font_exists(hdc, "Segoe UI Variable Text") {
            ("Segoe UI Variable Text", "Segoe UI Variable Text Semibold", "Segoe UI Variable Display Semibold")
        } else {
            ("Segoe UI", "Segoe UI Semibold", "Segoe UI Semibold")
        };
        let specs: [(&str, &str, i32, bool); FONT_COUNT] = [
            (body.0, "Yu Gothic UI", 13, false),
            (body.1, "Yu Gothic UI Semibold", 13, false),
            (body.0, "Yu Gothic UI", 12, false),
            (body.2, "Yu Gothic UI Semibold", 20, false),
            ("Segoe Fluent Icons", "Segoe Fluent Icons", 16, true),
            ("Segoe Fluent Icons", "Segoe Fluent Icons", 11, true),
            ("Segoe Fluent Icons", "Segoe Fluent Icons", 44, true),
            (body.0, "Yu Gothic UI", 20, false),
            (body.2, "Yu Gothic UI Semibold", 28, false),
        ];
        for (i, (latin, cjk, size, icon)) in specs.iter().enumerate() {
            let px = self.px(*size);
            let (latin, cjk) = (make_font(latin, px), make_font(cjk, px));
            let mut tm: TEXTMETRICW = unsafe { std::mem::zeroed() };
            unsafe {
                let previous = SelectObject(hdc, latin);
                GetTextMetricsW(hdc, &mut tm);
                SelectObject(hdc, previous);
            }
            self.faces[i] = Face {
                latin,
                cjk,
                baseline: if *icon {
                    tm.tmAscent - tm.tmHeight / 2
                } else {
                    (px as f32 * 0.36).round() as i32
                },
                height: tm.tmHeight,
                icon: *icon,
            };
        }
        self.current_font = null_mut();
        unsafe { SelectObject(hdc, self.faces[0].latin) };
        for f in old {
            if !f.latin.is_null() {
                unsafe {
                    DeleteObject(f.latin);
                    DeleteObject(f.cjk);
                }
            }
        }
    }

    pub fn resize(&mut self, w: i32, h: i32) {
        let (w, h) = (w.max(1), h.max(1));
        self.w = w;
        self.h = h;
        self.clip = rect(0, 0, w, h);
        if w <= self.stride && h <= self.cap_h {
            return;
        }
        // Growing in steps avoids reallocating on every pixel of a window drag.
        let (cap_w, cap_h) = ((w + 63) & !63, (h + 63) & !63);
        let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = cap_w;
        info.bmiHeader.biHeight = -cap_h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        unsafe {
            let mut bits = null_mut();
            let bmp = CreateDIBSection(self.hdc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            let previous = SelectObject(self.hdc, bmp);
            if self.bmp.is_null() {
                self.old_bmp = previous;
            } else {
                DeleteObject(self.bmp);
            }
            self.bmp = bmp;
            self.bits = bits.cast();
        }
        self.stride = cap_w;
        self.cap_h = cap_h;
    }

    pub fn present(&self, target: HDC, area: Rect) {
        unsafe { BitBlt(target, area.x, area.y, area.w, area.h, self.hdc, area.x, area.y, SRCCOPY) };
    }

    pub fn set_clip(&mut self, c: Rect) {
        self.clip = c.intersect(&rect(0, 0, self.w, self.h));
    }

    pub fn reset_clip(&mut self) {
        self.clip = rect(0, 0, self.w, self.h);
    }

    #[inline]
    fn row(&mut self, y: i32, x0: i32, x1: i32) -> &mut [u32] {
        unsafe {
            let start = self.bits.add((y * self.stride + x0) as usize);
            std::slice::from_raw_parts_mut(start, (x1 - x0) as usize)
        }
    }

    pub fn fill(&mut self, r: Rect, color: u32) {
        let c = r.intersect(&self.clip);
        let value = color | 0xFF000000;
        for y in c.y..c.b() {
            self.row(y, c.x, c.r()).fill(value);
        }
    }

    pub fn fill_alpha(&mut self, r: Rect, color: u32, alpha: u8) {
        let c = r.intersect(&self.clip);
        for y in c.y..c.b() {
            for p in self.row(y, c.x, c.r()) {
                *p = blend(*p, color, alpha as u32);
            }
        }
    }

    pub fn round(&mut self, r: Rect, radius: f32, color: u32, alpha: u8) {
        self.round_impl(r, radius, color, alpha, None);
    }

    pub fn stroke(&mut self, r: Rect, radius: f32, width: f32, color: u32, alpha: u8) {
        self.round_impl(r, radius, color, alpha, Some(width));
    }

    fn round_impl(&mut self, r: Rect, radius: f32, color: u32, alpha: u8, stroke: Option<f32>) {
        let c = r.intersect(&self.clip);
        if c.w == 0 || c.h == 0 {
            return;
        }
        let radius = radius.min(r.w as f32 / 2.0).min(r.h as f32 / 2.0);
        let corner = radius.ceil() as i32 + 1;
        let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
        let (hx, hy) = (r.w as f32 / 2.0, r.h as f32 / 2.0);
        let edge = stroke.map_or(0, |w| w.ceil() as i32);
        let opaque = color | 0xFF000000;
        let a = alpha as u32;
        for y in c.y..c.b() {
            let straight_row = y >= r.y + corner && y < r.b() - corner;
            if straight_row {
                match stroke {
                    None if alpha == 255 => self.row(y, c.x, c.r()).fill(opaque),
                    None => {
                        for p in self.row(y, c.x, c.r()) {
                            *p = blend(*p, color, a);
                        }
                    }
                    Some(_) => {
                        for x in [r.x, r.r() - edge] {
                            let (x0, x1) = (x.max(c.x), (x + edge).min(c.r()));
                            if x0 < x1 {
                                for p in self.row(y, x0, x1) {
                                    *p = blend(*p, color, a);
                                }
                            }
                        }
                    }
                }
                continue;
            }
            let py = y as f32 + 0.5 - cy;
            for x in c.x..c.r() {
                let px = x as f32 + 0.5 - cx;
                let qx = px.abs() - hx + radius;
                let qy = py.abs() - hy + radius;
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt()
                    + qx.max(qy).min(0.0)
                    - radius;
                let mut coverage = (0.5 - d).clamp(0.0, 1.0);
                if let Some(w) = stroke {
                    coverage -= (0.5 - (d + w)).clamp(0.0, 1.0);
                }
                if coverage > 0.0 {
                    let p = &mut self.row(y, x, x + 1)[0];
                    *p = blend(*p, color, (coverage * a as f32) as u32);
                }
            }
        }
    }

    pub fn icon(&mut self, icon: &Icon, x: i32, y: i32, size: i32) {
        let c = rect(x, y, size, size).intersect(&self.clip);
        for py in c.y..c.b() {
            let sy = (py - y) * icon.size / size;
            for px in c.x..c.r() {
                let sx = (px - x) * icon.size / size;
                let src = icon.px[(sy * icon.size + sx) as usize];
                let dst = &mut self.row(py, px, px + 1)[0];
                *dst = over_premultiplied(*dst, src);
            }
        }
    }

    fn prepare_text(&mut self, s: &str, font: Font, color: u32) -> Face {
        let face = self.faces[font as usize];
        let h = if face.icon || s.is_ascii() { face.latin } else { face.cjk };
        if h != self.current_font {
            unsafe { SelectObject(self.hdc, h) };
            self.current_font = h;
        }
        if color != self.current_color {
            let bgr = ((color & 0xFF) << 16) | (color & 0xFF00) | ((color >> 16) & 0xFF);
            unsafe { SetTextColor(self.hdc, bgr) };
            self.current_color = color;
        }
        self.wide.clear();
        self.wide.extend(s.encode_utf16());
        face
    }

    pub fn measure(&mut self, s: &str, font: Font) -> i32 {
        self.prepare_text(s, font, self.current_color);
        let mut size = SIZE { cx: 0, cy: 0 };
        unsafe {
            GetTextExtentPoint32W(self.hdc, self.wide.as_ptr(), self.wide.len() as i32, &mut size)
        };
        size.cx
    }

    /// Draws one line, vertically centered in `area`, ellipsized to fit.
    pub fn text(&mut self, s: &str, font: Font, area: Rect, color: u32, align: Align) {
        if s.is_empty() || area.w <= 0 {
            return;
        }
        let visible = area.intersect(&self.clip);
        if visible.w == 0 || visible.h == 0 {
            return;
        }
        let face = self.prepare_text(s, font, color);
        let mut size = SIZE { cx: 0, cy: 0 };
        let mut fit = 0;
        unsafe {
            GetTextExtentExPointW(
                self.hdc,
                self.wide.as_ptr(),
                self.wide.len() as i32,
                area.w,
                &mut fit,
                null_mut(),
                &mut size,
            )
        };
        let mut width = size.cx;
        if fit < self.wide.len() as i32 {
            let mut dots = SIZE { cx: 0, cy: 0 };
            let ellipsis = [0x2026u16];
            unsafe {
                GetTextExtentPoint32W(self.hdc, ellipsis.as_ptr(), 1, &mut dots);
                GetTextExtentExPointW(
                    self.hdc,
                    self.wide.as_ptr(),
                    self.wide.len() as i32,
                    (area.w - dots.cx).max(0),
                    &mut fit,
                    null_mut(),
                    &mut size,
                );
            }
            let mut keep = fit.max(0) as usize;
            if keep > 0 && (0xD800..0xDC00).contains(&self.wide[keep - 1]) {
                keep -= 1;
            }
            self.wide.truncate(keep);
            self.wide.push(0x2026);
            width = area.w;
        }
        let x = match align {
            Align::Left => area.x,
            Align::Center => area.x + (area.w - width) / 2,
            Align::Right => area.r() - width,
        };
        let y = area.y + area.h / 2 + face.baseline;
        let bounds = RECT {
            left: visible.x,
            top: visible.y,
            right: visible.r(),
            bottom: visible.b(),
        };
        unsafe {
            ExtTextOutW(
                self.hdc,
                x,
                y,
                ETO_CLIPPED,
                &bounds,
                self.wide.as_ptr(),
                self.wide.len() as u32,
                std::ptr::null(),
            )
        };
    }

    pub fn font_height(&self, font: Font) -> i32 {
        self.faces[font as usize].height
    }

    /// Writes a top-down 32-bit BMP. Used by `--shot`.
    pub fn save_bmp(&self, path: &str) -> std::io::Result<()> {
        let bytes = (self.w * self.h * 4) as u32;
        let mut out = Vec::with_capacity(bytes as usize + 54);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(bytes + 54).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&self.w.to_le_bytes());
        out.extend_from_slice(&(-self.h).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&[0; 24]);
        for y in 0..self.h {
            let row = unsafe {
                std::slice::from_raw_parts(self.bits.add((y * self.stride) as usize), self.w as usize)
            };
            for p in row {
                out.extend_from_slice(&(*p | 0xFF000000).to_le_bytes());
            }
        }
        std::fs::write(path, out)
    }
}

#[inline]
fn blend(dst: u32, src: u32, a: u32) -> u32 {
    let inv = 255 - a;
    let rb = ((dst & 0xFF00FF) * inv + (src & 0xFF00FF) * a + 0x800080) >> 8 & 0xFF00FF;
    let g = ((dst & 0xFF00) * inv + (src & 0xFF00) * a + 0x8000) >> 8 & 0xFF00;
    0xFF000000 | rb | g
}

#[inline]
fn over_premultiplied(dst: u32, src: u32) -> u32 {
    let inv = 255 - (src >> 24);
    let rb = (((dst & 0xFF00FF) * inv + 0x800080) >> 8) & 0xFF00FF;
    let g = (((dst & 0xFF00) * inv + 0x8000) >> 8) & 0xFF00;
    0xFF000000 | ((src & 0xFFFFFF) + rb + g)
}
