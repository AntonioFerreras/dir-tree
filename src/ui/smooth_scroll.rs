//! Row-level smooth scroll with exponential ease-out.
//!
//! When the logical scroll target changes, a row displacement is injected
//! proportional to the number of cards scrolled × average card height.
//! Each tick the displacement decays toward zero, causing cards to slide
//! by a few terminal rows per frame — visible deceleration.

/// Row-offset smooth scroll animator.
#[derive(Debug, Clone)]
pub struct SmoothScroll {
    /// Current row displacement.  Positive = cards shifted down from
    /// their target (scroll-down); negative = shifted up (scroll-up).
    row_offset: f64,
    /// Previous scroll target index (to detect changes).
    prev_target: usize,
    /// Damping: `offset *= (1 - speed)` each tick.
    /// Higher speed = faster settle.  Good range: 0.25–0.45 at 20 fps.
    speed: f64,
}

impl SmoothScroll {
    pub fn new(speed: f64) -> Self {
        Self {
            row_offset: 0.0,
            prev_target: 0,
            speed: speed.clamp(0.05, 0.95),
        }
    }

    /// Feed the current target and approximate card height.
    /// Detects when the target changed and injects displacement.
    pub fn set_target(&mut self, target: usize, approx_card_h: f64) {
        if target != self.prev_target {
            let delta = target as f64 - self.prev_target as f64;
            // Positive delta → scrolling down → cards need to slide UP
            // from below their target → start offset positive.
            self.row_offset += delta * approx_card_h;
            self.prev_target = target;
        }
    }

    /// Decay the offset toward zero.  Call once per frame.
    pub fn tick(&mut self) {
        self.row_offset *= 1.0 - self.speed;
        if self.row_offset.abs() < 0.4 {
            self.row_offset = 0.0;
        }
    }

    /// Current row displacement (integer rows).
    pub fn row_offset(&self) -> i16 {
        self.row_offset.round() as i16
    }

    /// True when the animation has fully settled (no visible motion).
    pub fn is_animating(&self) -> bool {
        self.row_offset != 0.0
    }
}
