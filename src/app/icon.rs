//! The app's icons, painted pixel by pixel: the window's icon, and the bar graph that the
//! notification area icon can show. The painting is plain pixels, so that `--tray-icons`
//! can write them out without a window.

use crate::ui::theme::THEME;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The use the bar graph shows, in percent. The GPU is None when no adapter has counters.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Levels {
    pub cpu: f32,
    pub mem: f32,
    pub gpu: Option<f32>,
}

/// How a pixel of a rounded square tile is covered: 1 inside, 0 outside, between at the edge.
/// `inset` is how far inside the edge the measured line lies.
fn tile_cover(x: i32, y: i32, size: i32, radius: f32, inset: f32) -> f32 {
    let f = size as f32;
    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
    let qx = (fx - f / 2.0).abs() - f / 2.0 + radius;
    let qy = (fy - f / 2.0).abs() - f / 2.0 + radius;
    let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
    (0.5 - d - inset).clamp(0.0, 1.0)
}

/// Mixes two colors (0xRRGGBB): `t` 0 gives `a`, 1 gives `b`.
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (a, b) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        ((a + (b - a) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// Premultiplied 0xAARRGGBB from a color and its coverage, as icons take it.
fn premul(color: u32, alpha: f32) -> u32 {
    let ch = |s: u32| ((((color >> s) & 255) as f32 * alpha).round() as u32) << s;
    (((alpha * 255.0).round() as u32) << 24) | ch(16) | ch(8) | ch(0)
}

/// The window's icon: a pulse line on an accent tile.
pub fn app_pixels(size: i32) -> Vec<u32> {
    let accent = THEME.accent;
    let on = 0x0B1730u32;
    let f = size as f32;
    let radius = f * 0.24;
    let points = [(0.16, 0.56), (0.36, 0.56), (0.46, 0.28), (0.58, 0.74), (0.68, 0.48), (0.84, 0.48)];
    let mut px = vec![0u32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let tile = tile_cover(x, y, size, radius, 0.0);
            let mut line_d = f32::MAX;
            for w in points.windows(2) {
                let (ax, ay, bx, by) = (w[0].0 * f, w[0].1 * f, w[1].0 * f, w[1].1 * f);
                let (dx, dy) = (bx - ax, by - ay);
                let t = (((fx - ax) * dx + (fy - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                line_d = line_d.min(((fx - ax - t * dx).powi(2) + (fy - ay - t * dy).powi(2)).sqrt());
            }
            let line = (f * 0.05 + 0.5 - line_d).clamp(0.0, 1.0);
            px[(y * size + x) as usize] = premul(mix(accent, on, line), tile);
        }
    }
    px
}

/// The space the bars take inside a tile of `size`: the margin around them, and the gap between them.
fn graph_layout(size: i32) -> (i32, i32) {
    let pad = ((size as f32 * 0.125).round() as i32).max(2);
    let gap = (size / 16).max(1);
    (pad, gap)
}

/// How tall each bar (CPU, memory, GPU) is, in pixels of a tile of `size`. A bar that shows any
/// use is at least one pixel tall; the GPU's is empty without counters.
pub fn bar_heights(levels: Option<Levels>, size: i32) -> [i32; 3] {
    let (pad, _) = graph_layout(size);
    let inner = size - pad * 2;
    let h = |pct: f32| {
        let v = (pct.clamp(0.0, 100.0) / 100.0 * inner as f32).round() as i32;
        if v == 0 && pct >= 1.0 { 1 } else { v }
    };
    match levels {
        Some(l) => [h(l.cpu), h(l.mem), l.gpu.map_or(0, h)],
        None => [0; 3],
    }
}

/// The bar graph icon: three bars (CPU red, memory yellow, GPU blue, as in the app's graphs)
/// on a dark tile with an accent rim. The tile is dark so that the GPU's blue stands out, and
/// so that it reads on light and dark taskbars alike. Under each bar a dim track shows where
/// it would reach at 100%.
pub fn graph_pixels(size: i32, heights: [i32; 3]) -> Vec<u32> {
    let t = THEME;
    let colors = [t.chart_cpu, t.chart_mem, t.chart_gpu];
    let tile_color = 0x121216u32;
    let radius = size as f32 * 0.2;
    let rim = (size / 16).max(1) as f32;
    let (pad, gap) = graph_layout(size);
    let inner = size - pad * 2;
    let bar_w = (inner - gap * 2) / 3;
    // What the bars leave over goes to both sides, so they sit in the middle.
    let left = pad + (inner - bar_w * 3 - gap * 2) / 2;
    let bottom = size - pad;
    let mut px = vec![0u32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let cover = tile_cover(x, y, size, radius, 0.0);
            if cover == 0.0 {
                continue;
            }
            // The rim is the band between the edge and a line `rim` inside it.
            let inside = tile_cover(x, y, size, radius, rim);
            let mut color = mix(t.accent, tile_color, inside);
            let col = x - left;
            if col >= 0 && y >= pad && y < bottom {
                let (bar, within) = (col / (bar_w + gap), col % (bar_w + gap));
                if bar < 3 && within < bar_w {
                    let top = bottom - heights[bar as usize];
                    color = if y >= top { colors[bar as usize] } else { mix(tile_color, colors[bar as usize], 0.22) };
                }
            }
            px[(y * size + x) as usize] = premul(color, cover);
        }
    }
    px
}

/// Turns premultiplied pixels into an icon. The caller destroys it.
pub fn to_icon(px: &[u32], size: i32) -> HICON {
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size;
        bmi.bmiHeader.biHeight = -size;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits = null_mut();
        let color = CreateDIBSection(null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if color.is_null() || bits.is_null() {
            return null_mut();
        }
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        let mask = CreateBitmap(size, size, 1, 1, null());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

pub fn app_icon(size: i32) -> HICON {
    to_icon(&app_pixels(size), size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_the_inside_at_full_use() {
        for size in [16, 20, 24, 32, 48] {
            let (pad, _) = graph_layout(size);
            let full = Levels { cpu: 100.0, mem: 100.0, gpu: Some(100.0) };
            assert_eq!(bar_heights(Some(full), size), [size - pad * 2; 3]);
        }
    }

    #[test]
    fn small_use_shows_and_missing_gpu_is_empty() {
        let l = Levels { cpu: 1.0, mem: 0.4, gpu: None };
        assert_eq!(bar_heights(Some(l), 16), [1, 0, 0]);
        assert_eq!(bar_heights(None, 16), [0, 0, 0]);
    }

    #[test]
    fn three_bars_fit_inside_the_tile() {
        for size in [16, 20, 24, 32, 48] {
            let (pad, gap) = graph_layout(size);
            let inner = size - pad * 2;
            let bar_w = (inner - gap * 2) / 3;
            assert!(bar_w >= 3, "size {size}: bars {bar_w} px wide");
            assert!(bar_w * 3 + gap * 2 <= inner);
        }
    }

    #[test]
    fn graph_shows_the_bar_colors() {
        let px = graph_pixels(32, [24, 24, 24]);
        let rgb = |p: u32| p & 0xFFFFFF;
        // Somewhere in the lower half each bar's color appears, fully opaque.
        for color in [THEME.chart_cpu, THEME.chart_mem, THEME.chart_gpu] {
            assert!(px.iter().any(|p| rgb(*p) == color && p >> 24 == 255), "{color:06x}");
        }
    }
}
