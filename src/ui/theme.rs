//! Colors in one place, so a light theme only needs a second constant.

pub struct Theme {
    pub window: u32,
    pub content: u32,
    pub border: u32,
    pub hover: u32,
    pub pressed: u32,
    pub text: u32,
    pub text_dim: u32,
    pub text_faint: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub selection: u32,
    pub field: u32,
    pub popup: u32,
    pub danger: u32,
    pub warn: u32,
    /// Processes that just started, and their row tint.
    pub born: u32,
    pub born_bg: u32,
    /// Processes that just ended, and their row tint.
    pub died: u32,
    pub died_bg: u32,
    pub chart_cpu: u32,
    pub chart_mem: u32,
    pub chart_disk: u32,
    pub chart_net: u32,
    pub chart_gpu: u32,
    pub chart_grid: u32,
}

pub static DARK: Theme = Theme {
    window: 0x1B1B1F,
    content: 0x232328,
    border: 0x34343B,
    hover: 0x2E2E35,
    pressed: 0x3A3A42,
    text: 0xEDEDF2,
    text_dim: 0xA4A4AF,
    text_faint: 0x70707C,
    accent: 0x5AA2FF,
    on_accent: 0x0B1730,
    selection: 0x2A3C5C,
    field: 0x2B2B31,
    popup: 0x2C2C33,
    danger: 0xC42B3A,
    warn: 0xFFB454,
    born: 0x5FD38A,
    born_bg: 0x1E3A2B,
    died: 0xFF6B70,
    died_bg: 0x45252B,
    // CPU and memory get the warm, eye-catching colors; the rest stay apart in hue.
    chart_cpu: 0xFF5A4E,
    chart_mem: 0xFFC53D,
    chart_disk: 0x5CCB6A,
    chart_net: 0xA07CFF,
    chart_gpu: 0x4F9DFF,
    chart_grid: 0x303038,
};

pub static THEME: &Theme = &DARK;
