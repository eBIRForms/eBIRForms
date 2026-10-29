//! Design tokens (bir-shell DESIGN_SYSTEM.md, `ui.js` PALETTES): every
//! color has a light and a dark value and follows the system appearance
//! unless the filer picks one. Plain `Hsla` literals only: const eval of
//! `to_hsla()` or `clamp` is not in ra-jit's evaluator.

use nsh_core::prelude::*;

pub const fn tone(light: Hsla, dark: Hsla) -> AdaptiveColor {
    AdaptiveColor { light, dark }
}
pub const fn hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h, s, l, a }
}

pub const BG: AdaptiveColor = tone(hsla(0.6667, 0.1000, 0.9608, 1.000), hsla(0.6667, 0.0667, 0.0588, 1.000));
pub const SURFACE: AdaptiveColor = tone(hsla(0.0000, 0.0000, 1.0000, 1.000), hsla(0.6667, 0.0638, 0.0922, 1.000));
pub const RAISED: AdaptiveColor = tone(hsla(0.0000, 0.0000, 1.0000, 1.000), hsla(0.6667, 0.0667, 0.1471, 1.000));
pub const HOVER: AdaptiveColor = tone(hsla(0.6667, 0.0811, 0.9275, 1.000), hsla(0.6667, 0.0606, 0.1294, 1.000));
pub const SELECTED: AdaptiveColor = tone(hsla(0.6667, 0.0943, 0.8961, 1.000), hsla(0.6667, 0.0682, 0.1725, 1.000));
pub const BORDER: AdaptiveColor = tone(hsla(0.6667, 0.0741, 0.8941, 1.000), hsla(0.6667, 0.0633, 0.1549, 1.000));
pub const BORDER_STRONG: AdaptiveColor = tone(hsla(0.6667, 0.0638, 0.8157, 1.000), hsla(0.6667, 0.0588, 0.2333, 1.000));
pub const FG: AdaptiveColor = tone(hsla(0.6667, 0.0588, 0.1000, 1.000), hsla(0.6667, 0.0909, 0.9569, 1.000));
pub const FG2: AdaptiveColor = tone(hsla(0.6667, 0.0508, 0.3471, 1.000), hsla(0.6667, 0.0602, 0.6745, 1.000));
pub const FG3: AdaptiveColor = tone(hsla(0.6667, 0.0383, 0.4608, 1.000), hsla(0.6667, 0.0450, 0.5647, 1.000));
pub const PRIMARY: AdaptiveColor = tone(hsla(0.6667, 0.0588, 0.1000, 1.000), hsla(0.6667, 0.0909, 0.9569, 1.000));
pub const PRIMARY_FG: AdaptiveColor = tone(hsla(0.0000, 0.0000, 1.0000, 1.000), hsla(0.6667, 0.0556, 0.0706, 1.000));
pub const ACCENT: AdaptiveColor = tone(hsla(0.6196, 0.7052, 0.4922, 1.000), hsla(0.6202, 1.0000, 0.7471, 1.000));
pub const SUCCESS: AdaptiveColor = tone(hsla(0.3977, 0.7063, 0.2804, 1.000), hsla(0.3970, 0.6000, 0.5980, 1.000));
pub const WARNING: AdaptiveColor = tone(hsla(0.0774, 0.8857, 0.3431, 1.000), hsla(0.1045, 0.8756, 0.6216, 1.000));
pub const DANGER: AdaptiveColor = tone(hsla(0.9924, 0.6696, 0.4510, 1.000), hsla(0.0000, 1.0000, 0.7412, 1.000));
pub const ACCENT_SOFT: AdaptiveColor = tone(hsla(0.6196, 0.7052, 0.4922, 0.102), hsla(0.6202, 1.0000, 0.7471, 0.141));
pub const SUCCESS_SOFT: AdaptiveColor = tone(hsla(0.3977, 0.7063, 0.2804, 0.102), hsla(0.3970, 0.6000, 0.5980, 0.122));
pub const WARNING_SOFT: AdaptiveColor = tone(hsla(0.0896, 0.9029, 0.4039, 0.129), hsla(0.1045, 0.8756, 0.6216, 0.129));
pub const DANGER_SOFT: AdaptiveColor = tone(hsla(0.9924, 0.6696, 0.4510, 0.102), hsla(0.0000, 1.0000, 0.7412, 0.122));
pub const SCRIM: AdaptiveColor = tone(hsla(0.6667, 0.0588, 0.1000, 0.302), hsla(0.0000, 0.0000, 0.0000, 0.549));

// Sizes (`SIZE`, `RADIUS`, `BREAK`).
pub const TITLE_BAR: f32 = 40.0;
pub const SIDEBAR_W: f32 = 248.0;
pub const PANEL_W: f32 = 320.0;
pub const INSET: f32 = 8.0;
pub const CONTENT_MAX: f32 = 880.0;
pub const CONTROL: f32 = 32.0;
pub const CONTROL_SM: f32 = 28.0;
pub const R_CTL: f32 = 6.0;
pub const R_ROW: f32 = 8.0;
pub const R_CARD: f32 = 12.0;
pub const R_DIALOG: f32 = 14.0;
pub const R_FULL: f32 = 9999.0;
/// Below this width the sidebar and the panel are overlays.
pub const BREAK_OVERLAYS: f32 = 860.0;
/// Below this the pill shows the avatar and branch code only.
pub const BREAK_COMPACT_PILL: f32 = 600.0;
/// Below this content padding is 20 × 16.
pub const BREAK_NARROW: f32 = 640.0;
/// Below this the form's part navigator is a chip row.
pub const BREAK_FORM_NAV: f32 = 760.0;
