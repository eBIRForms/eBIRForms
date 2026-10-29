//! Colors. Plain `Hsla` literals only: const eval of `to_hsla()` or
//! `clamp` is not in ra-jit's evaluator, and a function whose consts it
//! cannot evaluate goes to cargo instead of a patch.

use nsh_core::prelude::*;

pub const fn tone(light: Hsla, dark: Hsla) -> AdaptiveColor {
    AdaptiveColor { light, dark }
}
pub const fn hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h, s, l, a }
}

pub const MAIN: AdaptiveColor = tone(hsla(0.61, 0.25, 0.97, 1.0), hsla(0.61, 0.30, 0.07, 1.0));
pub const SIDEBAR: AdaptiveColor = tone(hsla(0.61, 0.22, 0.93, 1.0), hsla(0.61, 0.28, 0.05, 1.0));
pub const TEXT: AdaptiveColor = tone(hsla(0.61, 0.45, 0.16, 1.0), hsla(0.61, 0.30, 0.93, 1.0));
pub const MUTED: AdaptiveColor = tone(hsla(0.61, 0.15, 0.43, 1.0), hsla(0.61, 0.18, 0.70, 1.0));
pub const SELECTED: AdaptiveColor = tone(hsla(0.62, 0.68, 0.51, 0.14), hsla(0.62, 0.90, 0.75, 0.16));
pub const LINE: AdaptiveColor = tone(hsla(0.61, 0.25, 0.89, 1.0), hsla(0.61, 0.30, 0.24, 1.0));
