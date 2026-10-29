//! The hot area: the whole shell, rebuilt on every patch. Keep patchable
//! state out of statics and thread-locals: the model lives in
//! `live::store()`'s anchor and small ephemeral signals go through
//! `live::keep`. A patch that changes a type the program can see is
//! answered `Restart` by the daemon, not applied.
//!
//! The layout is bir-shell's (DESIGN_SYSTEM.md "Shell layout"): a 40 px
//! title bar with no separators, then the left sidebar (248), the main
//! surface (radius 12, inset 8) and the tax-profile panel (320). Below
//! `BREAK_OVERLAYS` the sidebar and the panel float over the content.
//!
//! A phone gets no columns (D22): a stack of pages ([`phone_shell`]) —
//! Home lists Library, Filings and the queue — each under a nav bar that
//! clears the status bar, the notch or the island; the tax profile is a
//! sheet.

use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::live::store;
use crate::screens::{self, Route, taxpayer};
use crate::store::{Db, Loaded, ThemePref};
use crate::theme;
use crate::widgets::{CLEAR, avatar, group_label, icon_button, nav_row};

/// The room the macOS traffic lights take at the bar's left, as rox reads
/// it: the title bar's inset, else 66 (`SIZE` 12 px lights, 8 apart).
fn traffic_lights() -> f32 {
    #[cfg(target_os = "macos")]
    {
        let bar = nsh_core::element::desktop::title_bar();
        if bar.left.0 > 0.0 {
            return bar.left.0;
        }
    }
    66.0
}

fn wide() -> bool {
    window().width.0 >= theme::BREAK_OVERLAYS
}

pub(crate) fn hot_area(note: Signal<String>) -> AnyElement {
    if crate::mobile::phone() {
        return phone_shell().into_element();
    }
    let st = store();
    view()
        .flex_1()
        .h_full()
        .bg(theme::BG)
        .child(title_bar())
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(when(move || wide() && st.sidebar_open.get(), move || sidebar(note)))
                .child(main_surface())
                .child(when(move || wide() && st.panel_open.get(), taxpayer::panel))
                .child(when(move || !wide() && (st.sidebar_open.get() || st.panel_open.get()), move || overlays(note))),
        )
        .child(toast())
        .child(when(move || st.new_taxpayer_open.get(), taxpayer::new_taxpayer_dialog))
        .into_element()
}

// ---------------------------------------------------------------- phone

fn phone_shell() -> impl IntoElement {
    let st = store();
    view()
        .flex_1()
        .h_full()
        .bg(theme::SURFACE)
        .child(view().flex_1().child(each(|| vec![store().route.get()], |r| *r, |r, _| phone_page(r.get_untracked()))))
        .child(when(move || is_tab(st.route.get()), tab_bar))
        .child(toast())
        .child(when(move || st.profile_open.get(), profile_sheet))
        .child(when(move || st.new_taxpayer_open.get(), taxpayer::new_taxpayer_dialog))
}

fn phone_page(route: Route) -> AnyElement {
    if route == Route::Form {
        return screens::screen(route);
    }
    let st = store();
    let back = !is_tab(route);
    let title = match route {
        Route::Home => "Buwiz",
        Route::Library => "Forms",
        other => other.label(),
    };
    view()
        .flex_1()
        .h_full()
        .child(
            view()
                .pt(move || px(safe_area().top.0))
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .border_b(px(1.0))
                .border_color(theme::BORDER)
                .child(
                    view()
                        .h(px(screens::phone_form::NAV_H))
                        .px(px(10.0))
                        .flex_row()
                        .items_center()
                        .child(if back {
                            screens::phone_form::nav_icon(Icon::ChevronLeft, "Back", move || st.back()).into_element()
                        } else {
                            view().w(px(36.0)).into_element()
                        })
                        .child(text(title).flex_1().text_center().text_size(px(17.0)).font_weight(FontWeight::BOLD).text_color(theme::FG))
                        .child(profile_button()),
                ),
        )
        .child(
            view()
                .flex_1()
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .child(match route {
                    Route::Home => screens::phone_home().into_element(),
                    Route::Filings | Route::Queue => screens::phone_filings(route == Route::Queue).into_element(),
                    Route::Calendar => screens::phone_calendar().into_element(),
                    _ => screens::screen(route),
                }),
        )
        .into_element()
}

/// The phone's four tabs (the approved mobile map): Home, Forms, Filings,
/// Calendar. The send queue is a filter of Filings.
fn is_tab(route: Route) -> bool {
    matches!(route, Route::Home | Route::Library | Route::Filings | Route::Calendar)
}

/// The tab bar, above the home indicator or the navigation bar.
fn tab_bar() -> impl IntoElement {
    let st = store();
    let mut row = view().flex_row().h(px(56.0));
    for (route, i, label) in [
        (Route::Home, Icon::Home, "Home"),
        (Route::Library, Icon::Library, "Forms"),
        (Route::Filings, Icon::FileText, "Filings"),
        (Route::Calendar, Icon::Calendar, "Calendar"),
    ] {
        let on = move || st.route.get() == route;
        let on2 = on.clone();
        row = row.child(
            view()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(2.0))
                .cursor_pointer()
                .label(label)
                .on_press(move || {
                    // A tab starts its own stack.
                    st.history.set(Vec::new());
                    st.go(route);
                    st.history.set(Vec::new());
                })
                .child(each(move || vec![on()], |o| *o, move |o, _| {
                    let o = o.get_untracked();
                    icon(i, 22.0, move || if o { theme::ACCENT } else { theme::FG3 })
                }))
                .child(text(label).text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(move || if on2() { theme::ACCENT } else { theme::FG3 })),
        );
    }
    view()
        .pb(move || px(safe_area().bottom.0))
        .pl(move || px(safe_area().left.0))
        .pr(move || px(safe_area().right.0))
        .border_t(px(1.0))
        .border_color(theme::BORDER)
        .bg(theme::SURFACE)
        .child(row)
}

/// The taxpayer's avatar at the nav bar's end: opens the tax profile.
fn profile_button() -> impl IntoElement {
    let st = store();
    view()
        .size(px(36.0))
        .items_center()
        .justify_center()
        .cursor_pointer()
        .label("Tax profile")
        .active(|s| s.opacity(0.5))
        .on_press(move || {
            if st.selected.get_untracked().is_some() {
                st.profile_open.set(true);
            } else if matches!(st.db.get_untracked(), Db::Ready(_)) {
                st.new_taxpayer_open.set(true);
            }
        })
        .child(each(
            move || vec![st.selected.get()],
            |s| s.as_ref().map(|s| s.tin.clone()),
            |sel, _| match sel.get_untracked() {
                Some(sel) => avatar(&sel.name, &sel.tin[..9.min(sel.tin.len())], 28.0).into_element(),
                None => icon(Icon::User, 20.0, || theme::ACCENT),
            },
        ))
}

/// The tax profile as a bottom sheet, clear of the home indicator.
fn profile_sheet() -> impl IntoElement {
    let st = store();
    view()
        .absolute()
        .inset(px(0.0))
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(move || st.profile_open.set(false)))
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .h(move || px(window().height.0 * 0.8))
                .rounded_t(px(22.0))
                .bg(theme::BG)
                .shadow_lg()
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .pb(move || px(safe_area().bottom.0))
                .child(view().w(px(36.0)).h(px(5.0)).rounded(px(99.0)).bg(theme::BORDER_STRONG).mt(px(8.0)).self_center().on_press(move || st.profile_open.set(false)))
                .child(view().flex_1().child(taxpayer::panel()))
                .child(
                    view()
                        .mx(px(16.0))
                        .mt(px(8.0))
                        .min_h(px(48.0))
                        .px(px(14.0))
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .rounded(px(12.0))
                        .bg(theme::SURFACE)
                        .cursor_pointer()
                        .label("Settings")
                        .on_press(move || {
                            st.profile_open.set(false);
                            st.go(Route::Settings);
                        })
                        .child(icon(Icon::Settings, 18.0, || theme::FG2))
                        .child(text("Settings").flex_1().text_size(px(15.0)).text_color(theme::FG))
                        .child(icon(Icon::ChevronRight, 16.0, || theme::FG3)),
                ),
        )
}

// ---------------------------------------------------------------- title bar

fn title_bar() -> impl IntoElement {
    let st = store();
    let compact = move || window().width.0 < theme::BREAK_COMPACT_PILL;
    let left = view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(view().w(move || px(traffic_lights())))
        .child(icon_button(Icon::PanelLeft, "Toggle sidebar", move || st.sidebar_open.get(), move || st.sidebar_open.update(|o| *o = !*o)))
        .child(when(move || !compact(), move || {
            view()
                .flex_row()
                .gap(px(4.0))
                .child(icon_button(Icon::ArrowLeft, "Back", || false, move || st.back()).opacity(move || if st.history.with(Vec::is_empty) { 0.4 } else { 1.0_f32 }))
                .child(icon_button(Icon::ArrowRight, "Forward", || false, move || st.forward()).opacity(move || if st.ahead.with(Vec::is_empty) { 0.4 } else { 1.0_f32 }))
        }))
        .child(when(move || !st.sidebar_open.get(), move || {
            view()
                .flex_row()
                .gap(px(4.0))
                .child(icon_button(Icon::Search, "Find a form", || false, move || st.go(Route::Library)))
                .child(icon_button(Icon::Plus, "New return", || false, move || st.go(Route::Library)))
        }));
    let right = view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .pr(px(8.0))
        .child(each(
            || vec![dark()],
            |d| *d,
            move |d, _| {
                icon_button(
                    if d.get_untracked() { Icon::Sun } else { Icon::Moon },
                    "Switch light or dark",
                    || false,
                    move || st.theme.set(if dark() { ThemePref::Light } else { ThemePref::Dark }),
                )
            },
        ))
        .child(icon_button(Icon::PanelRight, "Toggle tax profile", move || st.panel_open.get(), move || st.panel_open.update(|o| *o = !*o)));
    view()
        .h(px(theme::TITLE_BAR))
        .flex_row()
        .items_center()
        .window_drag(true)
        .child(left)
        .child(view().flex_1())
        .child(right)
        // The pill is centred on the window, not between the groups.
        .child(
            view()
                .absolute()
                .inset(px(0.0))
                .items_center()
                .justify_center()
                .pointer_events(PointerEvents::BoxNone)
                .child(context_pill()),
        )
}

/// `ContextPill`: the taxpayer's avatar, name and full TIN; a press toggles
/// the tax-profile panel. "Add a taxpayer" while there is none.
fn context_pill() -> impl IntoElement {
    let st = store();
    let compact = move || window().width.0 < theme::BREAK_COMPACT_PILL;
    view()
        .h(px(30.0))
        .pl(px(5.0))
        .pr(px(10.0))
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .rounded(px(theme::R_FULL))
        .cursor_pointer()
        .label("Tax profile")
        .hover(|s| s.bg(theme::HOVER))
        .bg(move || if st.panel_open.get() { theme::SELECTED } else { CLEAR })
        .on_press(move || {
            if st.selected.get_untracked().is_some() {
                st.panel_open.update(|o| *o = !*o);
            } else if matches!(st.db.get_untracked(), Db::Ready(_)) {
                st.new_taxpayer_open.set(true);
            }
        })
        .child(each(
            move || vec![st.selected.get()],
            |s| s.as_ref().map(|s| s.tin.clone()),
            move |sel, _| match sel.get_untracked() {
                Some(sel) => {
                    let branch = sel.tin.get(9..).unwrap_or("00000").to_owned();
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(avatar(&sel.name, &sel.tin[..9.min(sel.tin.len())], 20.0))
                        .child(when(move || !compact(), {
                            let sel = sel.clone();
                            move || {
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(text(sel.name.clone()).max_w(px(260.0)).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate())
                                    .child(text(sel.tin_display.clone()).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).tabular_nums().text_color(theme::FG2))
                            }
                        }))
                        .child(when(compact, move || text(branch.clone()).text_size(px(12.0)).tabular_nums().text_color(theme::FG2)))
                        .into_element()
                }
                None => text(move || match st.db.get() {
                    Db::Opening => "Opening database…".to_owned(),
                    Db::Failed(_) => "Database unavailable".to_owned(),
                    Db::Ready(_) => "Add a taxpayer".to_owned(),
                })
                .pl(px(5.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::FG)
                .into_element(),
            },
        ))
        .child(each(
            move || vec![st.panel_open.get()],
            |o| *o,
            |open, _| icon(if open.get_untracked() { Icon::ChevronLeft } else { Icon::ChevronRight }, 14.0, || theme::FG3),
        ))
}

// ---------------------------------------------------------------- sidebar

fn sidebar(note: Signal<String>) -> impl IntoElement {
    let st = store();
    let current = move |r: Route| move || st.route.get().nav() == r;
    let mut nav = view().gap(px(2.0));
    for (route, i) in [
        (Route::Home, Icon::Home),
        (Route::Library, Icon::Library),
        (Route::Calendar, Icon::Calendar),
        (Route::Filings, Icon::FileText),
        (Route::Queue, Icon::Send),
    ] {
        nav = nav.child(nav_row(Some(i), route.label(), current(route), move || st.go(route)));
    }
    let recent = move || match (st.returns.get(), st.selected.get()) {
        (Loaded::Ready(rows), Some(sel)) => rows.into_iter().filter(|r| r.tin == sel.tin).take(5).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    view()
        .w(px(theme::SIDEBAR_W))
        .h_full()
        .flex_shrink(0.0)
        .pt(px(4.0))
        .px(px(8.0))
        .pb(px(8.0))
        .bg(theme::BG)
        .child(
            view()
                .flex_row()
                .gap(px(4.0))
                .mb(px(8.0))
                .child(
                    nsh_base::input()
                        .value(st.library_query)
                        .on_change(move |v: &str| {
                            st.library_query.set(v.to_owned());
                            st.go(Route::Library);
                        })
                        .placeholder("Search forms")
                        .placeholder_color(|| color(theme::FG3))
                        .auto_correct(false)
                        .flex_1()
                        .h(px(theme::CONTROL_SM))
                        .px(px(10.0))
                        .text_size(px(13.0))
                        .text_color(theme::FG)
                        .bg(theme::SURFACE)
                        .border_1()
                        .border_color(theme::BORDER)
                        .rounded(px(theme::R_ROW)),
                )
                .child(icon_button(Icon::Plus, "New return", || false, move || st.go(Route::Library))),
        )
        .child(nav)
        .child(when(move || !recent().is_empty(), move || {
            view().child(group_label("Recent")).child(view().gap(px(2.0)).child(each(
                recent,
                |r| r.id,
                move |row, _| {
                    let row = row.get_untracked();
                    let label = format!("{} · {}", row.form_code, row.period_label);
                    let open = bir_app_core::OpenReturn {
                        tin: row.tin.clone(),
                        form_code: row.form_code.clone(),
                        year: row.taxable_year,
                        period: row.period,
                        key: row.return_key,
                    };
                    let title = label.clone();
                    nav_row(Some(Icon::Clock), label, || false, move || st.form.open(open.clone(), title.clone()))
                },
            )))
        }))
        .child(view().flex_1())
        .child(nav_row(Some(Icon::Settings), "Settings", current(Route::Settings), move || st.go(Route::Settings)))
        .child(
            view()
                .h(px(44.0))
                .mt(px(4.0))
                .px(px(8.0))
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(avatar("Buwiz", "buwiz", 28.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(text("Buwiz").text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
                        .child(text(move || live_label(&note.get())).text_size(px(12.0)).text_color(theme::FG3).truncate()),
                ),
        )
}

/// `note` is `<state>:<message>` from `live::install`.
fn live_label(note: &str) -> String {
    match note.split(':').next().unwrap_or("") {
        "live" | "applied" => "Offline filer · live".to_owned(),
        "restart" => "Offline filer · restart needed".to_owned(),
        "declined" => "Offline filer · patch declined".to_owned(),
        _ => "Offline filer".to_owned(),
    }
}

// ---------------------------------------------------------------- main

fn main_surface() -> impl IntoElement {
    let st = store();
    view()
        .flex_1()
        .min_w(px(0.0))
        .mb(px(theme::INSET))
        .ml(move || px(if wide() && st.sidebar_open.get() { 0.0 } else { theme::INSET }))
        .mr(move || px(if wide() && st.panel_open.get() { 0.0 } else { theme::INSET }))
        .rounded(px(theme::R_CARD))
        .border_1()
        .border_color(theme::BORDER)
        .bg(theme::SURFACE)
        .overflow_hidden()
        .child(each(
            || vec![store().route.get()],
            |r| *r,
            |r, _| screens::screen(r.get_untracked()),
        ))
}

/// Below `BREAK_OVERLAYS`: the sidebar and the panel over the content, a
/// scrim behind them that closes them.
fn overlays(note: Signal<String>) -> impl IntoElement {
    let st = store();
    view()
        .absolute()
        .inset(px(0.0))
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(move || {
            st.sidebar_open.set(false);
            st.panel_open.set(false);
        }))
        .child(when(move || st.sidebar_open.get(), move || {
            view().absolute().top(px(0.0)).bottom(px(0.0)).left(px(0.0)).shadow_lg().child(sidebar(note))
        }))
        .child(when(move || st.panel_open.get(), || {
            view().absolute().top(px(0.0)).bottom(px(0.0)).right(px(0.0)).shadow_lg().child(taxpayer::panel())
        }))
}

/// Toasts sit bottom-centre on `primary`, 20 px above the edge; on a phone
/// at the top, under the status bar (the approved mobile map).
fn toast() -> impl IntoElement {
    let st = store();
    when(move || st.notice.get().is_some(), move || {
        let place = view().absolute().left(px(16.0)).right(px(16.0));
        let place = if crate::mobile::phone() { place.top(move || px(safe_area().top.0 + 56.0)) } else { place.bottom(px(20.0)) };
        place
            .items_center()
            .pointer_events(PointerEvents::BoxNone)
            .child(
                view()
                    .max_w(px(560.0))
                    .px(px(16.0))
                    .py(px(10.0))
                    .rounded(px(theme::R_ROW))
                    .bg(theme::PRIMARY)
                    .shadow_lg()
                    .cursor_pointer()
                    .on_press(move || st.notice.set(None))
                    .child(text(move || st.notice.get().map(|n| n.text).unwrap_or_default()).text_size(px(13.0)).text_color(theme::PRIMARY_FG)),
            )
    })
}
