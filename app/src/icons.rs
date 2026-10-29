//! Lucide icons (24 × 24 viewBox, stroke 2, round caps and joins) drawn
//! through `canvas()` + `svg_path`. Circles and rounded rects are written
//! as path data. Chrome icons are 16 px; 12 inside chips, 14 for the pill
//! chevron (DESIGN_SYSTEM.md "Hard rules").

use nsh_core::canvas::{LineCap, LineJoin, svg_path};
use nsh_core::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Icon {
    PanelLeft,
    PanelRight,
    ArrowLeft,
    ArrowRight,
    Search,
    Plus,
    Sun,
    Moon,
    Home,
    Library,
    Calendar,
    FileText,
    Send,
    Settings,
    ChevronRight,
    ChevronLeft,
    ChevronDown,
    Printer,
    CircleCheck,
    Building,
    Users,
    Clock,
    List,
    Alert,
    Check,
    ChevronUp,
    More,
    User,
}

impl Icon {
    fn paths(self) -> &'static [&'static str] {
        match self {
            Icon::PanelLeft => &["M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z", "M9 3v18"],
            Icon::PanelRight => &["M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z", "M15 3v18"],
            Icon::ArrowLeft => &["m12 19-7-7 7-7", "M19 12H5"],
            Icon::List => &["M3 6h.01", "M3 12h.01", "M3 18h.01", "M8 6h13", "M8 12h13", "M8 18h13"],
            Icon::Alert => &["M12 5v9", "M12 19h.01"],
            Icon::Check => &["M20 6 9 17l-5-5"],
            Icon::ChevronUp => &["m18 15-6-6-6 6"],
            Icon::More => &["M5 12h.01", "M12 12h.01", "M19 12h.01"],
            Icon::User => &["M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2", "M8 7a4 4 0 1 0 8 0a4 4 0 1 0-8 0"],
            Icon::ArrowRight => &["M5 12h14", "m12 5 7 7-7 7"],
            Icon::Search => &["M3 11a8 8 0 1 0 16 0a8 8 0 1 0-16 0", "m21 21-4.3-4.3"],
            Icon::Plus => &["M5 12h14", "M12 5v14"],
            Icon::Sun => &[
                "M8 12a4 4 0 1 0 8 0a4 4 0 1 0-8 0",
                "M12 2v2", "M12 20v2", "m4.93 4.93 1.41 1.41", "m17.66 17.66 1.41 1.41",
                "M2 12h2", "M20 12h2", "m6.34 17.66-1.41 1.41", "m19.07 4.93-1.41 1.41",
            ],
            Icon::Moon => &["M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"],
            Icon::Home => &[
                "M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8",
                "M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
            ],
            Icon::Library => &["m16 6 4 14", "M12 6v14", "M8 8v12", "M4 4v16"],
            Icon::Calendar => &["M8 2v4", "M16 2v4", "M5 4h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z", "M3 10h18"],
            Icon::FileText => &[
                "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z",
                "M14 2v4a2 2 0 0 0 2 2h4", "M10 9H8", "M16 13H8", "M16 17H8",
            ],
            Icon::Send => &["m22 2-7 20-4-9-9-4Z", "M22 2 11 13"],
            Icon::Settings => &[
                "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z",
                "M9 12a3 3 0 1 0 6 0a3 3 0 1 0-6 0",
            ],
            Icon::ChevronRight => &["m9 18 6-6-6-6"],
            Icon::ChevronLeft => &["m15 18-6-6 6-6"],
            Icon::ChevronDown => &["m6 9 6 6 6-6"],
            Icon::Printer => &[
                "M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2",
                "M6 9V3a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v6",
                "M7 14h10a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1v-6a1 1 0 0 1 1-1z",
            ],
            Icon::CircleCheck => &["M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0", "m9 12 2 2 4-4"],
            Icon::Building => &[
                "M8 2h8a2 2 0 0 1 2 2v18H6V4a2 2 0 0 1 2-2z",
                "M6 12H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h2", "M18 9h2a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-2",
                "M10 6h4", "M10 10h4", "M10 14h4", "M10 18h4",
            ],
            Icon::Users => &[
                "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2", "M5 7a4 4 0 1 0 8 0a4 4 0 1 0-8 0",
                "M22 21v-2a4 4 0 0 0-3-3.87", "M16 3.13a4 4 0 0 1 0 7.75",
            ],
            Icon::Clock => &["M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0", "M12 6v6l4 2"],
        }
    }
}

/// `icon` at `size` points in `ink` (reactive: the ink follows the theme).
pub(crate) fn icon(icon: Icon, size: f32, ink: impl Fn() -> AdaptiveColor + 'static) -> AnyElement {
    let paths: Vec<_> = icon.paths().iter().map(|d| svg_path(d)).collect();
    canvas(move || {
        let ink = color(ink());
        let mut p = Painter::new()
            .scale(size / 24.0, size / 24.0)
            .line_cap(LineCap::Round)
            .line_join(LineJoin::Round)
            .stroke_style(ink)
            .line_width(2.0);
        for path in &paths {
            p = p.stroke_path(path);
        }
        p
    })
    .size(px(size))
    .flex_shrink(0.0)
    .into_element()
}
