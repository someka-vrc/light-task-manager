//! Turns one touch or pen contact into taps, press-and-hold, drags and pans.
//!
//! The first event is withheld until the contact shows what it is, so a pan
//! never selects a row and a hold never starts a column drag.

pub const HOLD_MS: u32 = 500;

const FLING_MAX_IDLE_SECONDS: f64 = 0.08;
const FLING_MIN_SLOPS: f32 = 20.0;
const VELOCITY_SMOOTHING: f32 = 0.4;
const RAIL_RATIO: i32 = 2;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Out {
    Move { x: i32, y: i32 },
    Press { x: i32, y: i32, right: bool },
    Release { x: i32 },
    Pan { dx: f32, dy: f32 },
    Fling { vx: f32, vy: f32 },
}

/// How a new contact should be read.
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    /// How far (px) it may move and still count as a tap or a hold.
    pub slop: i32,
    /// Whether dragging it should scroll the content under it.
    pub pannable: bool,
    /// The pen's barrel button is held, so the contact is a right click.
    pub barrel: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Idle,
    Pending,
    Dragging,
    Panning { lock_x: bool, lock_y: bool },
    /// Already resolved; the rest of the contact is ignored.
    Done,
}

pub struct Gesture {
    phase: Phase,
    id: u32,
    start: (i32, i32),
    last: (i32, i32),
    last_t: f64,
    slop: i32,
    pannable: bool,
    velocity: (f32, f32),
}

impl Gesture {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            id: 0,
            start: (0, 0),
            last: (0, 0),
            last_t: 0.0,
            slop: 1,
            pannable: false,
            velocity: (0.0, 0.0),
        }
    }

    pub fn idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    pub fn pending(&self) -> bool {
        self.phase == Phase::Pending
    }

    pub fn down(&mut self, id: u32, at: (i32, i32), t: f64, contact: Contact, out: &mut Vec<Out>) {
        self.id = id;
        self.start = at;
        self.last = at;
        self.last_t = t;
        self.slop = contact.slop.max(1);
        self.pannable = contact.pannable;
        self.velocity = (0.0, 0.0);
        self.phase = Phase::Pending;
        if contact.barrel {
            self.click(true, out);
        }
    }

    fn click(&mut self, right: bool, out: &mut Vec<Out>) {
        let (x, y) = self.start;
        out.push(Out::Move { x, y });
        out.push(Out::Press { x, y, right });
        out.push(Out::Release { x });
        self.phase = Phase::Done;
    }

    pub fn update(&mut self, id: u32, at: (i32, i32), t: f64, out: &mut Vec<Out>) {
        if id != self.id {
            return;
        }
        match self.phase {
            Phase::Pending => {
                let (dx, dy) = ((at.0 - self.start.0).abs(), (at.1 - self.start.1).abs());
                if dx.max(dy) < self.slop {
                    return;
                }
                if self.pannable {
                    self.phase = Phase::Panning {
                        lock_x: dy > dx * RAIL_RATIO,
                        lock_y: dx > dy * RAIL_RATIO,
                    };
                    self.last = at;
                    self.last_t = t;
                } else {
                    let (x, y) = self.start;
                    out.push(Out::Move { x, y });
                    out.push(Out::Press { x, y, right: false });
                    out.push(Out::Move { x: at.0, y: at.1 });
                    self.phase = Phase::Dragging;
                    self.last = at;
                }
            }
            Phase::Dragging => {
                if at != self.last {
                    out.push(Out::Move { x: at.0, y: at.1 });
                    self.last = at;
                }
            }
            Phase::Panning { lock_x, lock_y } => {
                let dx = if lock_x { 0.0 } else { (at.0 - self.last.0) as f32 };
                let dy = if lock_y { 0.0 } else { (at.1 - self.last.1) as f32 };
                let dt = (t - self.last_t) as f32;
                if dt > 0.0 {
                    let k = VELOCITY_SMOOTHING;
                    self.velocity.0 += (dx / dt - self.velocity.0) * k;
                    self.velocity.1 += (dy / dt - self.velocity.1) * k;
                }
                if dx != 0.0 || dy != 0.0 {
                    out.push(Out::Pan { dx, dy });
                }
                self.last = at;
                self.last_t = t;
            }
            Phase::Idle | Phase::Done => {}
        }
    }

    pub fn hold(&mut self, out: &mut Vec<Out>) {
        if self.phase == Phase::Pending {
            self.click(true, out);
        }
    }

    pub fn up(&mut self, id: u32, t: f64, canceled: bool, out: &mut Vec<Out>) {
        if id != self.id {
            return;
        }
        match self.phase {
            Phase::Pending if !canceled => self.click(false, out),
            Phase::Dragging => out.push(Out::Release { x: self.last.0 }),
            Phase::Panning { .. } if !canceled && t - self.last_t <= FLING_MAX_IDLE_SECONDS => {
                let (vx, vy) = self.velocity;
                if vx.hypot(vy) >= self.slop as f32 * FLING_MIN_SLOPS {
                    out.push(Out::Fling { vx, vy });
                }
            }
            _ => {}
        }
        self.phase = Phase::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOP: i32 = 10;

    fn start(pannable: bool) -> Gesture {
        let mut g = Gesture::new();
        g.down(7, (100, 100), 0.0, Contact { slop: SLOP, pannable, barrel: false }, &mut Vec::new());
        g
    }

    fn click(right: bool) -> Vec<Out> {
        vec![
            Out::Move { x: 100, y: 100 },
            Out::Press { x: 100, y: 100, right },
            Out::Release { x: 100 },
        ]
    }

    #[test]
    fn tap_clicks_at_the_start_point() {
        let mut g = start(true);
        let mut out = Vec::new();
        g.update(7, (103, 102), 0.05, &mut out);
        assert!(out.is_empty());
        g.up(7, 0.1, false, &mut out);
        assert_eq!(out, click(false));
        assert!(g.idle());
    }

    #[test]
    fn hold_right_clicks_once() {
        let mut g = start(true);
        let mut out = Vec::new();
        g.hold(&mut out);
        assert_eq!(out, click(true));
        out.clear();
        g.update(7, (200, 300), 0.7, &mut out);
        g.up(7, 0.8, false, &mut out);
        assert!(out.is_empty());
        assert!(g.idle());
    }

    #[test]
    fn barrel_button_right_clicks_at_contact() {
        let mut g = Gesture::new();
        let mut out = Vec::new();
        g.down(7, (100, 100), 0.0, Contact { slop: SLOP, pannable: true, barrel: true }, &mut out);
        assert_eq!(out, click(true));
        assert!(!g.pending());
    }

    #[test]
    fn pan_skips_the_slop_and_never_presses() {
        let mut g = start(true);
        let mut out = Vec::new();
        g.update(7, (100, 120), 0.01, &mut out);
        assert!(out.is_empty());
        g.update(7, (100, 150), 0.02, &mut out);
        assert_eq!(out, [Out::Pan { dx: 0.0, dy: 30.0 }]);
    }

    #[test]
    fn vertical_pan_ignores_sideways_wobble() {
        let mut g = start(true);
        let mut out = Vec::new();
        g.update(7, (100, 130), 0.01, &mut out);
        g.update(7, (106, 150), 0.02, &mut out);
        assert_eq!(out, [Out::Pan { dx: 0.0, dy: 20.0 }]);
    }

    #[test]
    fn quick_release_flings_and_paused_release_does_not() {
        let mut g = start(true);
        let mut out = Vec::new();
        g.update(7, (100, 130), 0.01, &mut out);
        g.update(7, (100, 180), 0.02, &mut out);
        out.clear();
        g.up(7, 0.03, false, &mut out);
        assert!(matches!(out[..], [Out::Fling { vy, .. }] if vy > 0.0));

        let mut g = start(true);
        out.clear();
        g.update(7, (100, 130), 0.01, &mut out);
        g.update(7, (100, 180), 0.02, &mut out);
        out.clear();
        g.up(7, 0.5, false, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn drag_outside_a_pannable_area_presses_at_the_start() {
        let mut g = start(false);
        let mut out = Vec::new();
        g.update(7, (140, 100), 0.05, &mut out);
        assert_eq!(
            out,
            [
                Out::Move { x: 100, y: 100 },
                Out::Press { x: 100, y: 100, right: false },
                Out::Move { x: 140, y: 100 },
            ]
        );
        out.clear();
        g.up(7, 0.1, false, &mut out);
        assert_eq!(out, [Out::Release { x: 140 }]);
    }

    #[test]
    fn canceled_contact_does_not_click() {
        let mut g = start(true);
        let mut out = Vec::new();
        g.up(7, 0.1, true, &mut out);
        assert!(out.is_empty());
        assert!(g.idle());
    }

    #[test]
    fn other_contacts_are_ignored() {
        let mut g = start(true);
        let mut out = Vec::new();
        g.update(8, (300, 300), 0.05, &mut out);
        g.up(8, 0.1, false, &mut out);
        assert!(out.is_empty());
        assert!(g.pending());
    }
}
