//! The phone's tab pages, pages and sheets (the approved mobile map):
//!
//! - Home: Due next with Start return, Pick up where you left off, Sending.
//! - Filings: All · Drafts · Sending · Sent, rows that swipe left for Print
//!   and Delete; Delete asks first (the only confirm in the app).
//! - Calendar: a month with dots on due days, then the deadlines by month;
//!   late ones show the penalty so far.
//! - A queued or sent return's page (where it is, in steps) and a failed
//!   one's (the plain reason, the next step in the bar).
//! - The start-a-return sheet: year chips, period chips; a late period says
//!   what starting it now adds in penalties.
//! - Search, pulled down on any tab: forms and the filer's own filings.
//! - Empty screens always offer one way forward.

use bir_app_core::display::period_choices;
use bir_app_core::{BirApp, DeadlineRow, FormCatalogEntry, OpenReturn, ReturnRow};
use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::live::store;
use crate::screens::home::{greeting, late_penalty, open_deadline, open_status};
use crate::screens::{Route, library, preview, welcome};
use crate::store::{Catalog, Db, FilingsFilter, Loaded, SendState, days_until, today};
use crate::theme;
use crate::widgets::{CLEAR, Tone, chip, peso, status_tone};

/// The least height of a list row.
const ROW: f32 = 60.0;
/// How far a swiped row opens for each action.
const ACTION_W: f32 = 84.0;

/// A tab page's content: one scroll, pulled down to search.
pub(crate) fn tab_content(inner: impl IntoElement) -> impl IntoElement {
    let st = store();
    scroll()
        .flex_1()
        .refreshing(false)
        .on_refresh(move |_: &()| st.search_open.set(true))
        .child(view().w_full().px(px(16.0)).pt(px(16.0)).pb(move || px(24.0 + safe_area().bottom.0)).gap(px(14.0)).child(inner))
}

/// A small caps group heading.
fn heading(label: impl IntoReactive<SharedString>) -> Text {
    text(label).pt(px(6.0)).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG2)
}

fn card() -> View {
    view().rounded(px(theme::R_CARD)).bg(theme::SURFACE).border_1().border_color(theme::BORDER).overflow_hidden()
}

/// A code chip: 12/700 accent on accent-soft.
fn code_chip(code: &str) -> impl IntoElement {
    view()
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .bg(theme::ACCENT_SOFT)
        .flex_shrink(0.0)
        .child(text(code.to_owned()).text_size(px(12.0)).font_weight(FontWeight::BOLD).tabular_nums().text_color(theme::ACCENT))
}

/// A list row: code chip, title over a line, a trailing element, chevron.
fn row_view(code: &str, title: String, line: impl IntoReactive<SharedString>, trailing: AnyElement, border: bool) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .px(px(14.0))
        .min_h(px(ROW))
        .py(px(8.0))
        .bg(theme::SURFACE)
        .border_t(px(if border { 1.0 } else { 0.0 }))
        .border_color(theme::BORDER)
        .child(code_chip(code))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(title).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate())
                .child(text(line).text_size(px(12.5)).text_color(theme::FG3).truncate()),
        )
        .child(trailing)
        .child(icon(Icon::ChevronRight, 14.0, || theme::FG3))
}

/// A return's status with its local send state.
fn status_of(r: &ReturnRow) -> String {
    crate::screens::filings::shown_status(r)
}

/// `Edited today`, `Queued · sends when online`, the failure.
fn row_line(r: &ReturnRow) -> String {
    let edited = r.updated_at.get(..10).unwrap_or(&r.updated_at);
    let when = match days_until(edited) {
        0 => "today".to_owned(),
        -1 => "yesterday".to_owned(),
        _ => crate::screens::steps::short_date(edited),
    };
    match store().send_state_of(r.id) {
        Some(SendState::Failed(why)) => format!("Not accepted: {why}"),
        Some(SendState::Sending) => "Sending now…".to_owned(),
        None => match r.status.as_str() {
            "Draft" => format!("Draft · edited {when}"),
            "Queued" => "Waiting for internet".to_owned(),
            _ => format!("Sent {when}"),
        },
    }
}

// ---------------------------------------------------------------- home

/// The selected branch's next open deadline.
fn due_next() -> Option<DeadlineRow> {
    let st = store();
    let sel = st.selected.get()?;
    match st.deadlines.get() {
        Loaded::Ready(rows) => rows.into_iter().filter(|d| d.tin == sel.tin && open_status(&d.status)).min_by(|a, b| a.due_date.cmp(&b.due_date)),
        _ => None,
    }
}

fn when_words(n: i64) -> String {
    match n {
        0 => "today".to_owned(),
        1 => "tomorrow".to_owned(),
        n if n < 0 => format!("{} day{} late", -n, if n == -1 { "" } else { "s" }),
        n => format!("in {n} days"),
    }
}

pub(crate) fn home() -> impl IntoElement {
    let st = store();
    let rows = move || crate::screens::filings::branch_rows();
    let drafts = move || rows().into_iter().filter(|r| r.status == "Draft").take(3).collect::<Vec<_>>();
    let sending = move || rows().into_iter().filter(|r| r.status == "Queued").collect::<Vec<_>>();
    let (drafts2, sending2) = (drafts.clone(), sending.clone());
    tab_content(
        view()
            .gap(px(14.0))
            .child(when(move || matches!(st.db.get(), Db::Failed(_)), library::db_error))
            .child(when(move || matches!(st.db.get(), Db::Ready(_)) && st.selected.get().is_none(), welcome))
            .child(when(move || st.selected.get().is_some(), move || text(greeting()).text_size(px(28.0)).font_weight(FontWeight::BOLD).text_color(theme::FG)))
            .child(when(move || st.selected.get().is_some(), due_card))
            .child(when(move || !drafts2().is_empty(), move || {
                let drafts = drafts.clone();
                view().gap(px(6.0)).child(heading("Pick up where you left off")).child(card().child(each(drafts, |r| r.id, |r, i| list_row(r.get_untracked(), i.get_untracked() > 0))))
            }))
            .child(when(move || !sending2().is_empty(), move || {
                let sending = sending.clone();
                view().gap(px(6.0)).child(heading("Sending")).child(card().child(each(sending, |r| r.id, |r, i| list_row(r.get_untracked(), i.get_untracked() > 0))))
            })),
    )
}

/// The big card: what is due next, and Start return.
fn due_card() -> impl IntoElement {
    let st = store();
    each(
        move || vec![due_next()],
        |d| d.as_ref().map(|d| (d.form_code.clone(), d.period, d.taxable_year, d.status.clone())),
        move |d, _| match d.get_untracked() {
            None => card()
                .p(px(18.0))
                .gap(px(10.0))
                .child(text("Nothing due soon").text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                .child(text("Deadlines show here once your profile lists the forms you file. You can start any return from Forms.").text_size(px(13.0)).text_color(theme::FG2))
                .child(primary_button("Start a return", move || st.go(Route::Library)))
                .into_element(),
            Some(d) => {
                let n = days_until(&d.due_date);
                let late = late_penalty(&d);
                let row = d.clone();
                view()
                    .p(px(18.0))
                    .gap(px(6.0))
                    .rounded(px(18.0))
                    .bg(if n < 0 { theme::DANGER_SOFT } else { theme::ACCENT_SOFT })
                    .child(text(format!("Due next · {}", when_words(n))).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(if n < 0 { theme::DANGER } else { theme::ACCENT }))
                    .child(text(format!("{} · {}", d.form_code, d.period_label)).text_size(px(24.0)).font_weight(FontWeight::BOLD).text_color(theme::FG))
                    .child(text(format!("{} · {}", crate::screens::steps::short_date(&d.due_date), d.form_name)).text_size(px(13.0)).text_color(theme::FG2).number_of_lines(2u32))
                    .child(when(late.is_some(), move || text(format!("Filing it today adds about {} in penalties.", peso(late.unwrap_or(0.0)))).text_size(px(13.0)).text_color(theme::DANGER)))
                    .child(view().h(px(6.0)))
                    .child(primary_button(if d.status == "Draft" { "Continue return" } else { "Start return" }, move || open_deadline(&row)))
                    .into_element()
            }
        },
    )
}

/// A 48 tall full-width primary button with an arrow.
fn primary_button(label: &'static str, act: impl Fn() + 'static) -> impl IntoElement {
    view()
        .h(px(48.0))
        .px(px(18.0))
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .rounded(px(12.0))
        .bg(theme::PRIMARY)
        .cursor_pointer()
        .label(label)
        .active(|s| s.opacity(0.7))
        .on_press(act)
        .child(text(label).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::PRIMARY_FG))
        .child(icon(Icon::ArrowRight, 16.0, || theme::PRIMARY_FG))
}

/// A return as a list row: a tap opens it (a draft's form, else its page).
fn list_row(r: ReturnRow, border: bool) -> impl IntoElement {
    let st = store();
    let status = status_of(&r);
    let title = format!("{} · {}", r.form_code, r.period_label);
    let open = r.clone();
    row_view(&r.form_code, title, row_line(&r), chip(status.clone(), status_tone(&status)).into_element(), border)
        .cursor_pointer()
        .active(|s| s.bg(theme::HOVER))
        .on_press(move || st.open_any(&open))
}

// ---------------------------------------------------------------- filings

/// A row that swipes left to show its actions (one open row at a time).
pub(crate) fn swipe_row(id: i64, content: View, actions: Vec<(&'static str, AdaptiveColor, std::rc::Rc<dyn Fn()>)>) -> impl IntoElement {
    let st = store();
    let width = ACTION_W * actions.len() as f32;
    let drag = signal(0.0f32);
    let open = move || st.swiped.get() == Some(id);
    let offset = move || {
        let base = if open() { -width } else { 0.0 };
        (base + drag.get()).clamp(-width - 24.0, 0.0)
    };
    let mut tray = view().absolute().top(px(0.0)).bottom(px(0.0)).right(px(0.0)).flex_row();
    for (label, bg, act) in actions {
        tray = tray.child(
            view()
                .w(px(ACTION_W))
                .h_full()
                .items_center()
                .justify_center()
                .bg(bg)
                .cursor_pointer()
                .label(label)
                .on_press(move || {
                    st.swiped.set(None);
                    act();
                })
                .child(text(label).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.08, 1.0)))),
        );
    }
    view()
        .overflow_hidden()
        .child(tray)
        .child(
            content
                .translate_x(move || DefiniteLength::from(px(offset())))
                .claim_touches(TouchClaim::on_pan(PanActivation::horizontal(px(10.0), px(10.0))))
                .on_responder_move(move |e: &ResponderEvent| drag.set(e.gesture.delta.x.0))
                .on_responder_release(move |e: &ResponderEvent| {
                    let was = if open() { -width } else { 0.0 };
                    let at = was + e.gesture.delta.x.0;
                    drag.set(0.0);
                    st.swiped.set(if at < -width / 2.0 { Some(id) } else { None });
                })
                .on_responder_terminate(move |_: &ResponderEvent| drag.set(0.0)),
        )
}

pub(crate) fn filings(sending: bool) -> impl IntoElement {
    let st = store();
    if sending {
        st.filings_filter.set(FilingsFilter::Sending);
    }
    let rows = move || {
        let f = st.filings_filter.get();
        crate::screens::filings::branch_rows().into_iter().filter(|r| f.keeps(&status_of(r))).collect::<Vec<_>>()
    };
    let rows2 = rows.clone();
    let mut chips = view().flex_row().gap(px(8.0));
    for (f, label) in [(FilingsFilter::All, "All"), (FilingsFilter::Drafts, "Drafts"), (FilingsFilter::Sending, "Sending"), (FilingsFilter::Sent, "Sent")] {
        chips = chips.child(
            view()
                .h(px(44.0))
                .px(px(16.0))
                .flex_shrink(0.0)
                .rounded(px(22.0))
                .items_center()
                .justify_center()
                .bg(move || if st.filings_filter.get() == f { theme::ACCENT_SOFT } else { theme::HOVER })
                .cursor_pointer()
                .label(label)
                .on_press(move || {
                    st.swiped.set(None);
                    st.filings_filter.set(f);
                })
                .child(text(label).text_size(px(14.0)).font_weight(FontWeight::MEDIUM).text_color(move || if st.filings_filter.get() == f { theme::ACCENT } else { theme::FG })),
        );
    }
    tab_content(
        view()
            .gap(px(14.0))
            .child(chips)
            .child(when(move || rows2().is_empty(), empty_filings))
            .child(card().child(each(rows, |r| (r.id, r.status.clone()), move |r, index| {
                let r = r.get_untracked();
                let mut actions: Vec<(&'static str, AdaptiveColor, std::rc::Rc<dyn Fn()>)> = Vec::new();
                let print = r.clone();
                actions.push(("Print", theme::ACCENT, std::rc::Rc::new(move || preview::print_row(print.clone()))));
                if r.status == "Draft" {
                    let del = r.clone();
                    actions.push(("Delete", theme::DANGER, std::rc::Rc::new(move || st.confirm_delete.set(Some(del.clone())))));
                }
                let id = r.id;
                let status = status_of(&r);
                let open = r.clone();
                let content = row_view(&r.form_code, format!("{} · {}", r.form_code, r.period_label), row_line(&r), chip(status.clone(), status_tone(&status)).into_element(), false)
                    .cursor_pointer()
                    .active(|s| s.bg(theme::HOVER))
                    .on_press(move || {
                        if st.swiped.get_untracked().is_some() {
                            st.swiped.set(None);
                        } else {
                            st.open_any(&open);
                        }
                    });
                view()
                    .border_t(move || px(if index.get() == 0 { 0.0 } else { 1.0_f32 }))
                    .border_color(theme::BORDER)
                    .child(swipe_row(id, content, actions))
            })))
            .child(when(move || crate::screens::filings::branch_rows().iter().any(|r| r.status == "Draft"), || {
                text("Swipe a row left to print or delete it.").self_center().text_size(px(12.0)).text_color(theme::FG3)
            })),
    )
}

/// No returns: start with what is due next.
fn empty_filings() -> impl IntoElement {
    let st = store();
    view()
        .gap(px(10.0))
        .py(px(24.0))
        .items_center()
        .child(text(move || match st.filings_filter.get() {
            FilingsFilter::Drafts => "No drafts",
            FilingsFilter::Sending => "Nothing waiting to send",
            FilingsFilter::Sent => "Nothing sent yet",
            FilingsFilter::All => "No returns yet",
        }).text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
        .child(text("Start with what's due next.").text_size(px(13.0)).text_color(theme::FG2))
        .child(each(move || vec![due_next()], |d| d.as_ref().map(|d| (d.form_code.clone(), d.period)), move |d, _| match d.get_untracked() {
            Some(d) => {
                let row = d.clone();
                view()
                    .w_full()
                    .child(card().child(row_view(&d.form_code, format!("{} · {}", d.form_code, d.period_label), format!("Due {}", crate::screens::steps::short_date(&d.due_date)), view().into_element(), false).cursor_pointer().on_press(move || open_deadline(&row))))
                    .into_element()
            }
            None => primary_button("Start a return", move || st.go(Route::Library)).into_element(),
        }))
}

/// "Delete this draft?": the only confirm in the app.
pub(crate) fn delete_alert() -> impl IntoElement {
    let st = store();
    let what = st.confirm_delete.get_untracked().map(|r| format!("{} · {}", r.form_code, r.period_label)).unwrap_or_default();
    let close = move || st.confirm_delete.set(None);
    let button = move |label: &'static str, danger: bool, act: Box<dyn Fn()>| {
        view()
            .h(px(48.0))
            .items_center()
            .justify_center()
            .border_t(px(1.0))
            .border_color(theme::BORDER)
            .cursor_pointer()
            .label(label)
            .active(|s| s.bg(theme::HOVER))
            .on_press(act)
            .child(text(label).text_size(px(17.0)).font_weight(if danger { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).text_color(if danger { theme::DANGER } else { theme::ACCENT }))
    };
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(60)
        .items_center()
        .justify_center()
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .w(px(290.0))
                .rounded(px(16.0))
                .bg(theme::RAISED)
                .overflow_hidden()
                .child(
                    view()
                        .p(px(18.0))
                        .gap(px(4.0))
                        .items_center()
                        .child(text("Delete this draft?").text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                        .child(text(format!("{what}. This can't be undone.")).text_center().text_size(px(13.0)).text_color(theme::FG2)),
                )
                .child(button("Delete draft", true, Box::new(move || {
                    if let Some(r) = st.confirm_delete.get_untracked() {
                        st.delete_draft(r);
                    }
                    st.confirm_delete.set(None);
                })))
                .child(button("Keep it", false, Box::new(close))),
        )
}

// ---------------------------------------------------------------- calendar

pub(crate) fn calendar() -> impl IntoElement {
    let rows = move || crate::screens::calendar::calendar_rows(false);
    let rows2 = rows.clone();
    let groups = move || {
        let mut out: Vec<(String, Vec<DeadlineRow>)> = Vec::new();
        for d in rows() {
            let key = crate::screens::calendar::month_of(&d);
            match out.last_mut() {
                Some((k, list)) if *k == key => list.push(d),
                _ => out.push((key, vec![d])),
            }
        }
        out
    };
    let groups2 = groups.clone();
    tab_content(
        view()
            .gap(px(14.0))
            .child(month_grid(rows2))
            .child(when(move || groups2().is_empty(), || {
                view()
                    .gap(px(8.0))
                    .py(px(20.0))
                    .items_center()
                    .child(text("No deadlines yet").text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                    .child(text("They show once your tax profile lists the forms you file.").text_center().text_size(px(13.0)).text_color(theme::FG2))
                    .child(primary_button("Start a return", || store().go(Route::Library)))
            }))
            .child(each(groups, |g| g.0.clone(), |g, _| {
                let (label, list) = g.get_untracked();
                let late = label == "Late";
                let mut group = card();
                for (i, d) in list.into_iter().enumerate() {
                    let n = days_until(&d.due_date);
                    let pen = late_penalty(&d);
                    let line = format!("{} · {}", crate::screens::steps::short_date(&d.due_date), when_words(n));
                    let trailing: AnyElement = match pen {
                        Some(p) => chip(peso(p), Tone::Danger).into_element(),
                        None if open_status(&d.status) => view().into_element(),
                        None => chip(d.status.clone(), status_tone(&d.status)).into_element(),
                    };
                    let row = d.clone();
                    group = group.child(row_view(&d.form_code, format!("{} · {}", d.form_code, d.period_label), line, trailing, i > 0).cursor_pointer().active(|s| s.bg(theme::HOVER)).on_press(move || open_deadline(&row)));
                }
                view().gap(px(6.0)).child(heading(label).text_color(if late { theme::DANGER } else { theme::FG2 })).child(group).into_element()
            })),
    )
}

/// This month, Monday first, a dot under each day something is due.
fn month_grid(rows: impl Fn() -> Vec<DeadlineRow> + 'static) -> impl IntoElement {
    use chrono::Datelike;
    let now = today();
    let first = now.with_day(1).unwrap_or(now);
    let lead = first.weekday().num_days_from_monday() as usize;
    let days_in = (first + chrono::Months::new(1) - chrono::Duration::days(1)).day() as usize;
    let rows = std::rc::Rc::new(rows);
    let mut grid = view().flex_row().flex_wrap();
    for d in ["M", "T", "W", "T", "F", "S", "S"] {
        grid = grid.child(view().w(relative(0.1428)).h(px(24.0)).items_center().child(text(d).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG3)));
    }
    for _ in 0..lead {
        grid = grid.child(view().w(relative(0.1428)).h(px(40.0)));
    }
    for day in 1..=days_in {
        let iso = format!("{}-{:02}-{day:02}", now.year(), now.month());
        let is_today = day as u32 == now.day();
        let rows = rows.clone();
        let iso2 = iso.clone();
        let due = move || rows().iter().any(|d| d.due_date == iso2);
        let late = day < now.day() as usize;
        grid = grid.child(
            view()
                .w(relative(0.1428))
                .h(px(40.0))
                .items_center()
                .justify_center()
                .gap(px(2.0))
                .child(
                    view()
                        .size(px(28.0))
                        .rounded(px(theme::R_FULL))
                        .items_center()
                        .justify_center()
                        .bg(if is_today { theme::ACCENT } else { CLEAR })
                        .child(text(day.to_string()).text_size(px(14.0)).font_weight(if is_today { FontWeight::BOLD } else { FontWeight::NORMAL }).text_color(if is_today { theme::PRIMARY_FG } else { theme::FG })),
                )
                .child(view().size(px(5.0)).rounded(px(3.0)).bg(move || if due() { if late { theme::DANGER } else { theme::ACCENT } } else { CLEAR })),
        );
    }
    card()
        .p(px(12.0))
        .gap(px(4.0))
        .child(text(now.format("%B %Y").to_string()).px(px(4.0)).pb(px(4.0)).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
        .child(grid)
}

// ---------------------------------------------------------------- a return's page

/// A queued, sent or failed return: where it is, the one thing to do.
pub(crate) fn status_page() -> impl IntoElement {
    let st = store();
    each(move || vec![(st.status_of.get().map(|r| (r.id, r.status.clone())), st.status_of.get().and_then(|r| st.send_state_of(r.id)).map(|s| format!("{s:?}")))], |k| k.clone(), move |_, _| {
        let Some(r) = st.status_of.get_untracked() else {
            return text("Nothing to show.").m(px(24.0)).into_element();
        };
        match st.send_state_of(r.id) {
            Some(SendState::Failed(why)) => failed_page(r, why).into_element(),
            _ => timeline_page(r).into_element(),
        }
    })
}

/// The bar at a page's foot: an icon, a small line over the action.
fn page_bar(small: String, bold: &'static str, glyph: Icon, tone: u8, act: impl Fn() + 'static) -> impl IntoElement {
    let (bg, ink) = match tone {
        2 => (theme::ACCENT, theme::PRIMARY_FG),
        _ => (theme::FG, theme::SURFACE),
    };
    view()
        .absolute()
        .left(move || px(12.0 + safe_area().left.0))
        .right(move || px(12.0 + safe_area().right.0))
        .bottom(move || px(if safe_area().bottom.0 > 0.0 { safe_area().bottom.0 + 4.0 } else { 16.0 }))
        .h(px(56.0))
        .px(px(8.0))
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .rounded(px(22.0))
        .bg(bg)
        .shadow_lg()
        .cursor_pointer()
        .label(bold)
        .active(|s| s.opacity(0.7))
        .on_press(act)
        .child(view().size(px(40.0)).rounded(px(14.0)).items_center().justify_center().child(icon(glyph, 18.0, move || ink)))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(small).text_size(px(11.0)).text_color(ink).opacity(0.7).truncate())
                .child(text(bold).text_size(px(15.0)).font_weight(FontWeight::BOLD).text_color(ink).truncate()),
        )
}

fn timeline_page(r: ReturnRow) -> impl IntoElement {
    let st = store();
    let queued = r.status == "Queued";
    let sending = matches!(st.send_state_of(r.id), Some(SendState::Sending));
    let edited = crate::screens::steps::short_date(r.updated_at.get(..10).unwrap_or(&r.updated_at));
    // (title, line, done)
    let steps: Vec<(&str, String, bool)> = vec![
        ("Checked", "No problems found".to_owned(), true),
        ("Queued", edited.clone(), true),
        ("Sent to BIR", if sending { "Sending now…".to_owned() } else if queued { "When you're online".to_owned() } else { format!("Sent {edited}") }, !queued),
        ("BIR email", "Arrives in your inbox. Print it for your records.".to_owned(), !queued && r.status != "Sent"),
    ];
    let mut list = view().gap(px(0.0));
    let n = steps.len();
    for (i, (title, line, done)) in steps.into_iter().enumerate() {
        list = list.child(
            view()
                .flex_row()
                .gap(px(14.0))
                .child(
                    view()
                        .items_center()
                        .child(
                            view()
                                .size(px(26.0))
                                .rounded(px(theme::R_FULL))
                                .items_center()
                                .justify_center()
                                .bg(if done { theme::SUCCESS } else { theme::HOVER })
                                .child(if done { icon(Icon::Check, 14.0, || theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.08, 1.0))) } else { text((i + 1).to_string()).text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(theme::FG2).into_element() }),
                        )
                        .child(when(i + 1 < n, move || view().w(px(2.0)).h(px(34.0)).bg(if done { theme::SUCCESS } else { theme::BORDER }))),
                )
                .child(
                    view()
                        .flex_1()
                        .pb(px(14.0))
                        .child(text(title).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(if done { theme::FG } else { theme::FG2 }))
                        .child(text(line).text_size(px(13.0)).text_color(theme::FG3)),
                ),
        );
    }
    let open = r.clone();
    let print = r.clone();
    let cancel = r.clone();
    view()
        .flex_1()
        .child(
            scroll().flex_1().child(
                view()
                    .px(px(18.0))
                    .pt(px(18.0))
                    .pb(move || px(120.0 + safe_area().bottom.0))
                    .gap(px(16.0))
                    .child(
                        view()
                            .gap(px(4.0))
                            .child(chip(status_of(&r), status_tone(&status_of(&r))).self_start())
                            .child(text(format!("{} · {}", r.form_code, r.period_label)).text_size(px(24.0)).font_weight(FontWeight::BOLD).text_color(theme::FG))
                            .child(text(r.taxpayer_name.clone()).text_size(px(13.0)).text_color(theme::FG2)),
                    )
                    .child(heading("Where it is"))
                    .child(list)
                    .child(
                        view()
                            .flex_row()
                            .gap(px(10.0))
                            .child(secondary("Open the return", move || st.open_row(&open)))
                            .child(secondary("Print", move || preview::print_row(print.clone()))),
                    ),
            ),
        )
        .child(if queued && !sending {
            page_bar("Changed your mind?".to_owned(), "Take back to drafts", Icon::ArrowLeft, 0, move || {
                st.cancel_queued(cancel.clone());
                st.back();
            })
            .into_element()
        } else {
            let print = r.clone();
            page_bar(format!("BIR Form {} · {}", r.form_code, r.period_label), "Print or save as PDF", Icon::Printer, 2, move || preview::print_row(print.clone())).into_element()
        })
}

fn secondary(label: &'static str, act: impl Fn() + 'static) -> impl IntoElement {
    view()
        .flex_1()
        .h(px(48.0))
        .rounded(px(12.0))
        .items_center()
        .justify_center()
        .bg(theme::HOVER)
        .cursor_pointer()
        .label(label)
        .active(|s| s.opacity(0.6))
        .on_press(act)
        .child(text(label).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
}

fn failed_page(r: ReturnRow, why: String) -> impl IntoElement {
    let st = store();
    let tin_problem = why.to_lowercase().contains("tin");
    let next = if tin_problem {
        "Check the TIN in your tax profile, then send it again. Your answers are kept."
    } else {
        "Open the return, fix what it says, then send it again. Your answers are kept."
    };
    let id = r.id;
    let open = r.clone();
    view()
        .flex_1()
        .child(
            scroll().flex_1().child(
                view()
                    .px(px(18.0))
                    .pt(px(18.0))
                    .pb(move || px(120.0 + safe_area().bottom.0))
                    .gap(px(14.0))
                    .child(
                        view()
                            .p(px(16.0))
                            .gap(px(6.0))
                            .rounded(px(16.0))
                            .bg(theme::DANGER_SOFT)
                            .child(text("BIR didn't accept this return").text_size(px(17.0)).font_weight(FontWeight::BOLD).text_color(theme::DANGER))
                            .child(text(why).text_size(px(14.0)).text_color(theme::FG)),
                    )
                    .child(heading("What to do"))
                    .child(text(next).text_size(px(15.0)).text_color(theme::FG))
                    .child(view().flex_row().gap(px(10.0)).child(secondary("Send again", move || {
                        st.retry(id);
                        st.back();
                    })).child(secondary("Open the return", move || st.open_row(&open)))),
            ),
        )
        .child(if tin_problem {
            page_bar("Fix · Filer".to_owned(), "Open tax profile", Icon::ArrowRight, 0, move || st.profile_open.set(true)).into_element()
        } else {
            let open = r.clone();
            page_bar("Fix · the return".to_owned(), "Open the return", Icon::ArrowRight, 0, move || st.open_row(&open)).into_element()
        })
}

// ---------------------------------------------------------------- start a return

/// A bottom sheet over the page.
fn sheet(close: impl Fn() + Clone + 'static, body: impl IntoElement) -> impl IntoElement {
    let c2 = close.clone();
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(40)
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .max_h(move || px(window().height.0 * 0.86))
                .rounded_t(px(22.0))
                .bg(theme::SURFACE)
                .shadow_lg()
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .pb(move || px(safe_area().bottom.0 + 16.0))
                .child(view().w(px(36.0)).h(px(5.0)).rounded(px(99.0)).bg(theme::BORDER_STRONG).mt(px(8.0)).mb(px(6.0)).self_center().on_press(c2))
                .child(body),
        )
}

/// What a period of `code` is for the selected branch: its deadline, its
/// saved return.
fn period_state(code: &str, year: i64, period: i64) -> (Option<DeadlineRow>, Option<ReturnRow>) {
    let st = store();
    let tin = st.selected.get_untracked().map(|s| s.tin).unwrap_or_default();
    let due = match st.deadlines.get_untracked() {
        Loaded::Ready(rows) => rows.into_iter().find(|d| d.tin == tin && d.form_code == code && d.taxable_year == year && d.period == period),
        _ => None,
    };
    let saved = match st.returns.get_untracked() {
        Loaded::Ready(rows) => rows.into_iter().find(|r| r.tin == tin && r.form_code == code && r.taxable_year == year && r.period == period),
        _ => None,
    };
    (due, saved)
}

pub(crate) fn start_sheet() -> impl IntoElement {
    // The catalog (each form's frequency) may still be loading at launch.
    let st = store();
    each(move || vec![matches!(st.catalog.get(), Catalog::Ready(_))], |r| *r, move |ready, _| {
        if ready.get_untracked() {
            start_sheet_ready().into_element()
        } else {
            sheet(move || st.start_sheet.set(None), text("Loading forms…").m(px(20.0)).text_size(px(15.0)).text_color(theme::FG2)).into_element()
        }
    })
}

fn start_sheet_ready() -> impl IntoElement {
    let st = store();
    let code = st.start_sheet.get_untracked().unwrap_or_default();
    let form: Option<FormCatalogEntry> = match st.catalog.get_untracked() {
        Catalog::Ready(forms) => forms.into_iter().find(|f| f.code == code),
        _ => None,
    };
    let freq = form.as_ref().and_then(|f| f.frequency.clone()).unwrap_or_default();
    let this_year = bir_app_core::display::current_year();
    let year = signal(this_year);
    let freq2 = freq.clone();
    let periods = move || period_choices(&freq2, year.get());
    let first = untracked(|| periods().first().map(|p| p.period).unwrap_or(0));
    let picked = signal(first);
    let close = move || st.start_sheet.set(None);
    let what = match freq.as_str() {
        "Monthly" => "which month?",
        "Quarterly" => "which quarter?",
        "Annual" | "Annually" => "which year?",
        _ => "which period?",
    };
    let chip_view = move |label: String, on: Box<dyn Fn() -> bool>, act: Box<dyn Fn()>| {
        let on = std::rc::Rc::new(on);
        let (on1, on2, on3) = (on.clone(), on.clone(), on.clone());
        view()
            .min_h(px(44.0))
            .px(px(14.0))
            .py(px(6.0))
            .rounded(px(12.0))
            .justify_center()
            .border(move || px(if on1() { 1.5 } else { 1.0_f32 }))
            .border_color(move || if on2() { theme::ACCENT } else { theme::BORDER_STRONG })
            .bg(move || if on3() { theme::ACCENT_SOFT } else { theme::SURFACE })
            .cursor_pointer()
            .label(label.clone())
            .on_press(act)
            .child(text(label).text_size(px(14.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
    };
    let mut years = view().flex_row().gap(px(8.0));
    for y in [this_year - 1, this_year] {
        let freq = freq.clone();
        years = years.child(chip_view(y.to_string(), Box::new(move || year.get() == y), Box::new(move || {
            year.set(y);
            picked.set(untracked(|| period_choices(&freq, y).first().map(|p| p.period).unwrap_or(0)));
        })));
    }
    let code2 = code.clone();
    let code3 = code.clone();
    let code4 = code.clone();
    let periods2 = periods.clone();
    let note = move || {
        let (due, saved) = period_state(&code3, year.get(), picked.get());
        match (due, saved) {
            (_, Some(r)) if r.status != "Draft" => format!("This period is {}. Opening it shows what was sent.", r.status.to_lowercase()),
            (Some(d), _) if days_until(&d.due_date) < 0 && open_status(&d.status) => {
                let late = -days_until(&d.due_date);
                let pen = chrono::NaiveDate::parse_from_str(&d.due_date, "%Y-%m-%d").ok().and_then(|due| BirApp::penalty_estimate(&d.form_code, due, today(), 0.0)).map(|e| e.total()).unwrap_or(0.0);
                format!("{} is {late} days late. Starting it now adds about {} in penalties, more if tax is due.", d.period_label, peso(pen))
            }
            (Some(d), _) => format!("Due {}.", crate::screens::steps::short_date(&d.due_date)),
            (None, Some(_)) => "You have a draft for this period. It opens where you left off.".to_owned(),
            (None, None) => String::new(),
        }
    };
    let note2 = note.clone();
    let late_note = {
        let code = code.clone();
        move || {
            let (due, _) = period_state(&code, year.get(), picked.get());
            due.is_some_and(|d| days_until(&d.due_date) < 0 && open_status(&d.status))
        }
    };
    let late2 = late_note.clone();
    let start = move || {
        let Some(sel) = st.selected.get_untracked() else { return };
        let (y, p) = (year.get_untracked(), picked.get_untracked());
        let (_, saved) = period_state(&code4, y, p);
        st.start_sheet.set(None);
        let label = period_choices(&freq_for(&code4), y).into_iter().find(|c| c.period == p).map(|c| c.label).unwrap_or_default();
        st.form.open(OpenReturn { tin: sel.tin, form_code: code4.clone(), year: y, period: p, key: saved.and_then(|r| r.return_key) }, format!("{code4} · {label}"));
    };
    let who = move || st.selected.get().map(|s| s.name).unwrap_or_default();
    sheet(
        close,
        view()
            .px(px(20.0))
            .gap(px(14.0))
            .child(text(format!("{code} · {what}")).text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
            .child(years)
            .child(view().flex_row().flex_wrap().gap(px(8.0)).child(each(periods2, |p| p.label.clone(), move |p, _| {
                let p = p.get_untracked();
                let (due, saved) = untracked(|| period_state(&code2, year.get(), p.period));
                let tag = match (&due, &saved) {
                    (_, Some(r)) if r.status != "Draft" => format!(" · {}", r.status.to_lowercase()),
                    (Some(d), _) if days_until(&d.due_date) < 0 && open_status(&d.status) => " · late".to_owned(),
                    (Some(d), _) if open_status(&d.status) => format!(" · due {}", crate::screens::steps::short_date(&d.due_date)),
                    (_, Some(_)) => " · draft".to_owned(),
                    _ => String::new(),
                };
                let short = p.label.split(' ').next().unwrap_or(&p.label).to_owned();
                let period = p.period;
                chip_view(format!("{short}{tag}"), Box::new(move || picked.get() == period), Box::new(move || picked.set(period)))
            })))
            .child(when(move || !note().is_empty(), move || {
                let note2 = note2.clone();
                let late2 = late2.clone();
                text(note2).text_size(px(13.0)).text_color(move || if late2() { theme::DANGER } else { theme::FG2 })
            }))
            .child(
                view()
                    .h(px(56.0))
                    .px(px(16.0))
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(px(22.0))
                    .bg(theme::ACCENT)
                    .cursor_pointer()
                    .label("Start return")
                    .active(|s| s.opacity(0.7))
                    .on_press(start)
                    .child(icon(Icon::ArrowRight, 18.0, || theme::PRIMARY_FG))
                    .child(
                        view()
                            .flex_1()
                            .child(text(move || format!("{} {} · {}", periods().into_iter().find(|c| c.period == picked.get()).map(|c| c.label.split(' ').next().unwrap_or("").to_owned()).unwrap_or_default(), year.get(), who())).text_size(px(11.0)).text_color(theme::PRIMARY_FG).opacity(0.75))
                            .child(text("Start return").text_size(px(15.0)).font_weight(FontWeight::BOLD).text_color(theme::PRIMARY_FG)),
                    ),
            )
    )
}

/// A form's frequency, from the catalog.
fn freq_for(code: &str) -> String {
    match store().catalog.get_untracked() {
        Catalog::Ready(forms) => forms.into_iter().find(|f| f.code == code).and_then(|f| f.frequency).unwrap_or_default(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- search

/// Pulled down on any tab: one search over the forms and the filer's own
/// filings.
pub(crate) fn search_sheet() -> impl IntoElement {
    let st = store();
    let query = signal(String::new());
    let close = move || crate::screens::taxpayer::close_after_blur(move || st.search_open.set(false));
    let forms = move || {
        let q = query.get().trim().to_lowercase();
        let qn: String = q.chars().filter(|c| !matches!(c, '-' | ' ')).collect();
        match st.catalog.get() {
            Catalog::Ready(all) if !q.is_empty() => all.into_iter().filter(|f| f.code.to_lowercase().replace('-', "").contains(&qn) || f.title.to_lowercase().contains(&q)).take(8).collect::<Vec<_>>(),
            _ => Vec::new(),
        }
    };
    let filings = move || {
        let q = query.get().trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        crate::screens::filings::branch_rows().into_iter().filter(|r| format!("{} {} {}", r.form_code, r.period_label, r.status).to_lowercase().contains(&q)).take(8).collect::<Vec<_>>()
    };
    let (forms2, filings2) = (forms.clone(), filings.clone());
    let (forms3, filings3) = (forms.clone(), filings.clone());
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(45)
        .bg(theme::SURFACE)
        .pt(move || px(safe_area().top.0))
        .pl(move || px(safe_area().left.0))
        .pr(move || px(safe_area().right.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .px(px(14.0))
                .py(px(8.0))
                .child(
                    view()
                        .flex_1()
                        .h(px(40.0))
                        .px(px(12.0))
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(10.0))
                        .bg(theme::HOVER)
                        .child(icon(Icon::Search, 15.0, || theme::FG3))
                        .child(
                            nsh_base::input()
                                .value(query)
                                .on_change(move |v: &str| query.set(v.to_owned()))
                                .placeholder("Search forms and your filings")
                                .placeholder_color(|| color(theme::FG3))
                                .auto_focus(true)
                                .auto_correct(false)
                                .flex_1()
                                .h(px(40.0))
                                .text_size(px(16.0))
                                .text_color(theme::FG)
                                .bg(CLEAR),
                        ),
                )
                .child(view().h(px(44.0)).justify_center().cursor_pointer().label("Cancel").on_press(close).child(text("Cancel").text_size(px(16.0)).text_color(theme::ACCENT))),
        )
        .child(
            scroll().flex_1().keyboard_dismiss_mode(KeyboardDismissMode::OnDrag).keyboard_taps(KeyboardTaps::Handled).child(
                view()
                    .px(px(16.0))
                    .pb(move || px(keyboard_height().0.max(safe_area().bottom.0) + 16.0))
                    .gap(px(8.0))
                    .child(when(move || query.get().trim().is_empty(), || text("Type a form code like 1701Q, a word like VAT, or a period like Q3.").pt(px(12.0)).text_size(px(13.0)).text_color(theme::FG3)))
                    .child(when(move || !query.get().trim().is_empty() && forms3().is_empty() && filings3().is_empty(), move || text(move || format!("Nothing matches “{}”. Try a form code like 1701Q.", query.get().trim())).pt(px(12.0)).text_size(px(13.0)).text_color(theme::FG3)))
                    .child(when(move || !forms2().is_empty(), move || {
                        let forms = forms.clone();
                        view().gap(px(6.0)).child(heading("Forms")).child(card().child(each(forms, |f| f.code.clone(), move |f, i| {
                            let f = f.get_untracked();
                            let code = f.code.clone();
                            row_view(&f.code, f.title.clone(), "Start a return", view().into_element(), i.get_untracked() > 0).cursor_pointer().active(|s| s.bg(theme::HOVER)).on_press(move || {
                                let code = code.clone();
                                crate::screens::taxpayer::close_after_blur(move || {
                                    st.search_open.set(false);
                                    st.start_sheet.set(Some(code));
                                });
                            })
                        })))
                    }))
                    .child(when(move || !filings2().is_empty(), move || {
                        let filings = filings.clone();
                        view().gap(px(6.0)).child(heading("Your filings")).child(card().child(each(filings, |r| r.id, move |r, i| {
                            let r = r.get_untracked();
                            let open = r.clone();
                            let status = status_of(&r);
                            row_view(&r.form_code, format!("{} · {}", r.form_code, r.period_label), row_line(&r), chip(status.clone(), status_tone(&status)).into_element(), i.get_untracked() > 0).cursor_pointer().active(|s| s.bg(theme::HOVER)).on_press(move || {
                                let open = open.clone();
                                crate::screens::taxpayer::close_after_blur(move || {
                                    st.search_open.set(false);
                                    st.open_any(&open);
                                });
                            })
                        })))
                    })),
            ),
        )
}
