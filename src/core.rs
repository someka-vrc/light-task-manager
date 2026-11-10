//! Window-independent application state: chrome, navigation and page routing.

use crate::gfx::{rect, Align, Canvas, Font, Rect};
use crate::model::Snapshot;
use crate::pages::{self, glyph, icons, Cmd, Cursor, Cx, Ev, Page};
use crate::sampler::Shared;
use crate::settings::Settings;
use crate::theme::THEME;
use std::sync::Arc;

pub const TITLE_H: i32 = 40;
const SIDEBAR_COLLAPSED: f32 = 52.0;
const SIDEBAR_EXPANDED: f32 = 208.0;
const SIDEBAR_SECONDS: f32 = 0.12;
const NAV_H: i32 = 40;
const TOAST_SECONDS: f32 = 4.5;
const CAPTION_W: i32 = 46;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Caption {
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    None,
    Menu,
    Nav(usize),
    Search,
    Clear,
}

pub struct Core {
    pub canvas: Canvas,
    pub settings: Settings,
    pub shared: Arc<Shared>,
    pub pages: Vec<Box<dyn Page>>,
    pub active: usize,
    pub dirty: bool,
    pub cmds: Vec<Cmd>,
    pub maximized: bool,
    pub filter_focused: bool,
    pub filter_text: String,
    pub caption_hover: Option<Caption>,
    pub touch: bool,
    sidebar: f32,
    hover: Hover,
    page_captured: bool,
    toast: Option<(String, f32)>,
}

impl Core {
    pub fn new(settings: Settings, shared: Arc<Shared>, scale: f32, w: i32, h: i32) -> Self {
        let mut canvas = Canvas::new(scale);
        canvas.resize(w, h);
        let sidebar = if settings.sidebar_expanded { 1.0 } else { 0.0 };
        Self {
            canvas,
            settings,
            shared,
            pages: pages::all(),
            active: 0,
            dirty: true,
            cmds: Vec::new(),
            maximized: false,
            filter_focused: false,
            filter_text: String::new(),
            caption_hover: None,
            touch: false,
            sidebar,
            hover: Hover::None,
            page_captured: false,
            toast: None,
        }
    }

    fn px(&self, v: i32) -> i32 {
        self.canvas.px(v)
    }

    fn title_h(&self) -> i32 {
        self.px(TITLE_H)
    }

    fn sidebar_w(&self) -> i32 {
        let eased = self.sidebar * self.sidebar * (3.0 - 2.0 * self.sidebar);
        let w = SIDEBAR_COLLAPSED + (SIDEBAR_EXPANDED - SIDEBAR_COLLAPSED) * eased;
        (w * self.canvas.scale).round() as i32
    }

    pub fn content_rect(&self) -> Rect {
        let (x, y) = (self.sidebar_w(), self.title_h());
        rect(x, y, self.canvas.w - x, self.canvas.h - y)
    }

    pub fn search_rect(&self) -> Rect {
        let w = self.px(420).min(self.canvas.w - self.px(420)).max(self.px(160));
        let h = self.px(30);
        rect((self.canvas.w - w) / 2, (self.title_h() - h) / 2, w, h)
    }

    fn clear_rect(&self) -> Rect {
        let s = self.search_rect();
        rect(s.r() - s.h, s.y, s.h, s.h)
    }

    /// Where the native text box goes, in client coordinates.
    pub fn edit_rect(&self) -> Rect {
        let s = self.search_rect();
        let h = self.canvas.font_height(Font::Body) + 2;
        let left = self.px(34);
        rect(s.x + left, s.y + (s.h - h) / 2, s.w - left - s.h, h)
    }

    pub fn wants_filter(&self) -> bool {
        self.pages[self.active].wants_filter()
    }

    fn caption_rect(&self, c: Caption) -> Rect {
        let (w, h) = (self.px(CAPTION_W), self.title_h());
        let index = match c {
            Caption::Minimize => 3,
            Caption::Maximize => 2,
            Caption::Close => 1,
        };
        rect(self.canvas.w - w * index, 0, w, h)
    }

    pub fn caption_at(&self, x: i32, y: i32) -> Option<Caption> {
        [Caption::Minimize, Caption::Maximize, Caption::Close]
            .into_iter()
            .find(|c| self.caption_rect(*c).contains(x, y))
    }

    /// True when (x, y) in the title bar should drag the window.
    pub fn is_drag_area(&self, x: i32, y: i32) -> bool {
        y < self.title_h()
            && self.caption_at(x, y).is_none()
            && !(self.wants_filter() && self.search_rect().contains(x, y))
    }

    fn nav_rects(&self) -> Vec<Rect> {
        let top = self.title_h() + self.px(NAV_H) + self.px(4);
        let (mut y, h) = (top, self.px(NAV_H));
        let mut bottom = self.canvas.h - self.px(6);
        let w = self.sidebar_w();
        let inset = self.px(6);
        let mut out = vec![Rect::default(); self.pages.len()];
        for (i, page) in self.pages.iter().enumerate().rev() {
            if page.nav().bottom {
                bottom -= h;
                out[i] = rect(inset, bottom, w - inset * 2, h);
            }
        }
        for (i, page) in self.pages.iter().enumerate() {
            if !page.nav().bottom {
                out[i] = rect(inset, y, w - inset * 2, h);
                y += h + self.px(2);
            }
        }
        out
    }

    fn menu_rect(&self) -> Rect {
        rect(self.px(6), self.title_h(), self.px(40), self.px(NAV_H))
    }

    fn hover_at(&self, x: i32, y: i32) -> Hover {
        if self.wants_filter() && self.search_rect().contains(x, y) {
            if !self.filter_text.is_empty() && self.clear_rect().contains(x, y) {
                return Hover::Clear;
            }
            return Hover::Search;
        }
        if self.menu_rect().contains(x, y) {
            return Hover::Menu;
        }
        if let Some(i) = self.nav_rects().iter().position(|r| r.contains(x, y)) {
            return Hover::Nav(i);
        }
        Hover::None
    }

    fn with_page(&mut self, f: impl FnOnce(&mut Box<dyn Page>, &mut Cx)) {
        let mut cx = Cx {
            settings: &mut self.settings,
            shared: &self.shared,
            cmds: &mut self.cmds,
            dirty: &mut self.dirty,
            touch: self.touch,
        };
        f(&mut self.pages[self.active], &mut cx);
    }

    pub fn pannable(&self, x: i32, y: i32) -> bool {
        self.pages[self.active].pannable(x, y)
    }

    pub fn on_snapshot(&mut self, snap: &Arc<Snapshot>) {
        let mut cx = Cx {
            settings: &mut self.settings,
            shared: &self.shared,
            cmds: &mut self.cmds,
            dirty: &mut self.dirty,
            touch: self.touch,
        };
        for page in &mut self.pages {
            page.on_snapshot(snap, &mut cx);
        }
    }

    pub fn navigate(&mut self, i: usize) {
        if i == self.active || i >= self.pages.len() {
            return;
        }
        self.with_page(|p, cx| {
            p.event(&Ev::Leave, cx);
            p.on_hide(cx);
        });
        self.active = i;
        self.with_page(|p, cx| p.on_show(cx));
        let text = self.filter_text.clone();
        self.with_page(|p, cx| p.set_filter(&text, cx));
        self.dirty = true;
    }

    pub fn on_enriched(&mut self) {
        self.with_page(|p, cx| p.on_enriched(cx));
    }

    pub fn set_filter(&mut self, text: String) {
        self.filter_text = text.clone();
        self.with_page(|p, cx| p.set_filter(&text, cx));
    }

    pub fn toast(&mut self, message: String) {
        self.toast = Some((message, TOAST_SECONDS));
        self.dirty = true;
    }

    pub fn animating(&self) -> bool {
        let target = if self.settings.sidebar_expanded { 1.0 } else { 0.0 };
        self.sidebar != target || self.toast.is_some() || self.pages[self.active].animating()
    }

    /// Returns true when the filter should be cleared.
    pub fn event(&mut self, ev: Ev) -> bool {
        let mut clear_filter = false;
        match ev {
            Ev::Tick { dt } => {
                let target: f32 = if self.settings.sidebar_expanded { 1.0 } else { 0.0 };
                if self.sidebar != target {
                    let step = dt / SIDEBAR_SECONDS;
                    self.sidebar = if target > self.sidebar {
                        (self.sidebar + step).min(1.0)
                    } else {
                        (self.sidebar - step).max(0.0)
                    };
                    self.dirty = true;
                }
                if let Some((_, left)) = &mut self.toast {
                    *left -= dt;
                    if *left <= 0.0 {
                        self.toast = None;
                        self.dirty = true;
                    }
                }
                self.with_page(|p, cx| p.event(&ev, cx));
            }
            Ev::Move { x, y } => {
                let hover = if self.page_captured { Hover::None } else { self.hover_at(x, y) };
                if hover != self.hover {
                    self.hover = hover;
                    self.dirty = true;
                }
                self.with_page(|p, cx| p.event(&ev, cx));
            }
            Ev::Leave => {
                self.hover = Hover::None;
                self.dirty = true;
                self.with_page(|p, cx| p.event(&ev, cx));
            }
            Ev::Down { x, y, right } => match self.hover_at(x, y) {
                Hover::Menu if !right => {
                    self.settings.sidebar_expanded = !self.settings.sidebar_expanded;
                    self.dirty = true;
                }
                Hover::Nav(i) if !right => self.navigate(i),
                Hover::Clear => clear_filter = true,
                Hover::Search => self.cmds.push(Cmd::FocusFilter),
                _ => {
                    if self.content_rect().contains(x, y) {
                        self.page_captured = true;
                        self.with_page(|p, cx| p.event(&ev, cx));
                    }
                }
            },
            Ev::Up { .. } => {
                if std::mem::take(&mut self.page_captured) {
                    self.with_page(|p, cx| p.event(&ev, cx));
                }
            }
            Ev::Wheel { .. } | Ev::Pan { .. } | Ev::Fling { .. } | Ev::Key { .. } => {
                self.with_page(|p, cx| p.event(&ev, cx))
            }
        }
        clear_filter
    }

    pub fn cursor(&self, x: i32, y: i32) -> Cursor {
        if self.wants_filter() && self.edit_rect().contains(x, y) {
            return Cursor::IBeam;
        }
        self.pages[self.active].cursor()
    }

    pub fn page_is_details(&mut self) -> Option<&mut crate::pages::details::DetailsPage> {
        // Page 0 is always the details page; see `pages::all`.
        let page = self.pages.get_mut(0)?;
        Some(unsafe { &mut *(page.as_mut() as *mut dyn Page as *mut crate::pages::details::DetailsPage) })
    }

    pub fn render(&mut self) {
        let t = THEME;
        let (w, h) = (self.canvas.w, self.canvas.h);
        self.canvas.reset_clip();
        self.canvas.fill(rect(0, 0, w, h), t.window);
        self.paint_title();
        let content = self.content_rect();
        self.paint_content_surface(content);
        self.paint_sidebar();

        let inner = rect(content.x + 1, content.y + 1, content.w - 1, content.h - 1);
        self.canvas.set_clip(inner);
        let (pages, active, settings) = (&mut self.pages, self.active, &self.settings);
        pages[active].paint(&mut self.canvas, inner, settings);
        self.canvas.reset_clip();

        self.paint_tooltip();
        self.paint_toast(content);
        self.dirty = false;
    }

    fn paint_title(&mut self) {
        let t = THEME;
        let title_h = self.title_h();
        let c = &mut self.canvas;
        let tile = c.px(18);
        let tile_rect = rect(c.px(17), (title_h - tile) / 2, tile, tile);
        c.round(tile_rect, c.px(5) as f32, t.accent, 255);
        c.text(&glyph(icons::CHART), Font::IconSmall, tile_rect, t.on_accent, Align::Center);
        let x = tile_rect.r() + c.px(26);
        c.text("Light Task Manager", Font::Small, rect(x, 0, c.px(160), title_h), t.text_dim, Align::Left);

        if self.wants_filter() {
            let s = self.search_rect();
            let (focused, hovered) = (self.filter_focused, self.hover == Hover::Search);
            let c = &mut self.canvas;
            c.round(s, c.px(6) as f32, t.field, 255);
            let border = if focused { t.accent } else if hovered { t.text_faint } else { t.border };
            c.stroke(s, c.px(6) as f32, 1.0, border, 255);
            c.text(&glyph(icons::SEARCH), Font::IconSmall, rect(s.x + c.px(11), s.y, c.px(14), s.h), t.text_dim, Align::Left);
            if !self.filter_text.is_empty() {
                let clear = self.clear_rect();
                let hot = self.hover == Hover::Clear;
                let c = &mut self.canvas;
                if hot {
                    c.round(clear.inset(c.px(4), c.px(4)), c.px(4) as f32, t.hover, 255);
                }
                c.text(&glyph(icons::CLEAR), Font::IconSmall, clear, t.text_dim, Align::Center);
            }
        }

        for cap in [Caption::Minimize, Caption::Maximize, Caption::Close] {
            let r = self.caption_rect(cap);
            let hot = self.caption_hover == Some(cap);
            let ch = match cap {
                Caption::Minimize => icons::MINIMIZE,
                Caption::Maximize if self.maximized => icons::RESTORE,
                Caption::Maximize => icons::MAXIMIZE,
                Caption::Close => icons::CLOSE,
            };
            let c = &mut self.canvas;
            let mut fg = t.text;
            if hot {
                if cap == Caption::Close {
                    c.fill(r, t.danger);
                    fg = 0xFFFFFF;
                } else {
                    c.fill(r, t.hover);
                }
            }
            c.text(&glyph(ch), Font::IconSmall, r, fg, Align::Center);
        }
    }

    fn paint_content_surface(&mut self, content: Rect) {
        let t = THEME;
        let c = &mut self.canvas;
        let radius = c.px(8) as f32;
        let oversize = rect(content.x, content.y, content.w + c.px(20), content.h + c.px(20));
        c.set_clip(content);
        c.round(oversize, radius, t.content, 255);
        c.stroke(oversize, radius, 1.0, t.border, 255);
        c.reset_clip();
    }

    fn paint_sidebar(&mut self) {
        let t = THEME;
        let hot_menu = self.hover == Hover::Menu;
        let menu = self.menu_rect();
        let side = self.sidebar_w();
        let nav = self.nav_rects();
        let c = &mut self.canvas;
        let radius = c.px(5) as f32;
        c.set_clip(rect(0, 0, side - c.px(6), c.h));
        if hot_menu {
            c.round(menu, radius, t.hover, 255);
        }
        c.text(&glyph(icons::MENU), Font::Icon, menu, t.text, Align::Center);

        let icon_w = c.px(40);
        for (i, r) in nav.iter().enumerate() {
            let n = self.pages[i].nav();
            let selected = i == self.active;
            let hot = self.hover == Hover::Nav(i);
            if selected {
                c.round(*r, radius, t.hover, 255);
                c.round(rect(r.x, r.y + c.px(11), c.px(3), r.h - c.px(22)), 1.5, t.accent, 255);
            } else if hot {
                c.round(*r, radius, t.hover, 170);
            }
            let color = if selected { t.accent } else { t.text };
            c.text(&glyph(n.icon), Font::Icon, rect(r.x, r.y, icon_w, r.h), color, Align::Center);
            let label = rect(r.x + icon_w + c.px(4), r.y, c.px(150), r.h);
            c.text(n.label, Font::Body, label, t.text, Align::Left);
        }
        c.reset_clip();
    }

    fn paint_tooltip(&mut self) {
        let Hover::Nav(i) = self.hover else { return };
        if self.settings.sidebar_expanded || self.sidebar != 0.0 {
            return;
        }
        let t = THEME;
        let anchor = self.nav_rects()[i];
        let label = self.pages[i].nav().label;
        let c = &mut self.canvas;
        let w = c.measure(label, Font::Small) + c.px(20);
        let h = c.px(28);
        let r = rect(anchor.r() + c.px(10), anchor.y + (anchor.h - h) / 2, w, h);
        c.round(r, c.px(5) as f32, t.popup, 255);
        c.stroke(r, c.px(5) as f32, 1.0, t.border, 255);
        c.text(label, Font::Small, r, t.text, Align::Center);
    }

    fn paint_toast(&mut self, content: Rect) {
        let Some((message, _)) = &self.toast else { return };
        let t = THEME;
        let c = &mut self.canvas;
        let w = (c.measure(message, Font::Body) + c.px(40)).min(content.w - c.px(40));
        let h = c.px(42);
        let r = rect(content.x + (content.w - w) / 2, content.b() - h - c.px(20), w, h);
        c.round(r, c.px(8) as f32, t.popup, 255);
        c.stroke(r, c.px(8) as f32, 1.0, t.warn, 200);
        c.text(message, Font::Body, r.inset(c.px(20), 0), t.text, Align::Center);
    }
}
