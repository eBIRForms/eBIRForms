//! The hot area: the whole shell, rebuilt on every patch. Keep patchable
//! state out of statics and thread-locals: the model lives in
//! `live::store()`'s anchor and small ephemeral signals go through
//! `live::keep`. A patch that changes a type the program can see is
//! answered `Restart` by the daemon, not applied.

use nsh_core::prelude::*;

use crate::live::store;
use crate::screens::{self, Route};
use crate::theme;

/// Sidebar · selected screen, plus the live status line at the bottom of
/// the sidebar.
pub(crate) fn hot_area(note: Signal<String>) -> impl IntoElement {
    view()
        .flex_1()
        .h_full()
        .flex_row()
        .child(sidebar(note))
        .child(view().flex_1().h_full().child(each(
            || vec![store().route.get()],
            |r| *r,
            |r, _| screens::screen(r.get_untracked()),
        )))
}

fn sidebar(note: Signal<String>) -> impl IntoElement {
    let mut items = view().gap(px(2.0));
    for route in Route::ALL {
        items = items.child(nav_item(route));
    }
    view()
        .w(px(220.0))
        .h_full()
        .bg(theme::SIDEBAR)
        .border_r(px(1.0))
        .border_color(theme::LINE)
        // Room for the window controls, level with the title bar.
        .pt(px(52.0))
        .pl(px(10.0))
        .pr(px(10.0))
        .pb(px(12.0))
        .window_drag(true)
        .child(items)
        .child(view().flex_1())
        .child(text(move || live_label(&note.get())).text_size(px(11.0)).text_color(theme::MUTED))
}

fn nav_item(route: Route) -> impl IntoElement {
    view()
        .h(px(32.0))
        .pl(px(10.0))
        .justify_center()
        .rounded(px(7.0))
        .bg(move || if store().route.get() == route { theme::SELECTED } else { theme::SIDEBAR })
        .on_press(move || store().route.set(route))
        .child(text(route.label()).text_size(px(14.0)).text_color(theme::TEXT))
}

/// `note` is `<state>:<message>` from `live::install`; the sidebar shows
/// the state only.
fn live_label(note: &str) -> String {
    match note.split(':').next().unwrap_or("") {
        "live" | "applied" => "● live".to_owned(),
        "restart" => "● restart needed".to_owned(),
        "declined" => "● patch declined".to_owned(),
        _ => "○ not live".to_owned(),
    }
}
