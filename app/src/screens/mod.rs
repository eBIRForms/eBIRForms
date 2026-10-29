//! One module per screen. The sidebar lists Home, Form library, Calendar,
//! Filings and Send queue; `Form` is reached from the library or Recent.

mod form;
pub(crate) use form::focus_nth;
mod library;
pub(crate) mod phone_fields;
pub(crate) mod phone_form;
pub(crate) mod steps;
pub(crate) mod taxpayer;

use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::live::store;
use crate::store::Db;
use crate::theme;
use crate::widgets::{Variant, button};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Route {
    Home,
    Library,
    Form,
    Filings,
    Queue,
    Calendar,
    Settings,
}

impl Route {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Route::Home => "Home",
            Route::Library => "Form library",
            Route::Form => "Return",
            Route::Filings => "Filings",
            Route::Queue => "Send queue",
            Route::Calendar => "Calendar",
            Route::Settings => "Settings",
        }
    }

    /// The sidebar entry that stays current on this screen.
    pub(crate) fn nav(self) -> Route {
        if self == Route::Form { Route::Library } else { self }
    }

    /// What the screen will do, shown until it is ported (P4b).
    fn summary(self) -> &'static str {
        match self {
            Route::Home => "Returns due in the next 30 days, as-needed forms and recently filed returns arrive here in the next pass.",
            Route::Filings => "Returns that left Draft.",
            Route::Queue => "Queued, sending, sent and failed returns, with receipts.",
            Route::Calendar => "Deadlines for the next 12 months, and your overrides.",
            Route::Settings => "Database, print and submission settings.",
            Route::Library | Route::Form => "",
        }
    }
}

/// The selected screen's body.
pub(crate) fn screen(route: Route) -> AnyElement {
    match route {
        Route::Library => library::screen().into_element(),
        Route::Form => form::screen().into_element(),
        Route::Home => home().into_element(),
        _ => content(placeholder(route)).into_element(),
    }
}

/// The content column of non-form pages: at most 880 wide, padding 28 × 32
/// (20 × 16 on a narrow window), scrolling vertically only.
pub(crate) fn content(inner: impl IntoElement) -> impl IntoElement {
    let narrow = || window().width.0 < theme::BREAK_NARROW;
    scroll().flex_1().child(
        view().w_full().items_center().child(
            view()
                .w_full()
                .max_w(px(theme::CONTENT_MAX))
                .px(move || px(if narrow() { 16.0 } else { 32.0 }))
                .pt(move || px(if narrow() { 20.0 } else { 28.0 }))
                // Clear of the home indicator on a phone (0 elsewhere).
                .pb(move || px(if narrow() { 20.0 } else { 28.0 } + safe_area().bottom.0))
                .gap(px(16.0))
                .child(inner),
        ),
    )
}

/// The page title, 20/600, one per view.
pub(crate) fn page_title(title: impl IntoReactive<SharedString>) -> Text {
    text(title).text_size(px(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG)
}

fn placeholder(route: Route) -> impl IntoElement {
    view()
        .gap(px(8.0))
        .child(page_title(route.label()))
        .child(text(route.summary()).text_size(px(13.0)).text_color(theme::FG2))
}

/// The phone's Home: where to go (Library, Filings, the queue) and the
/// selected taxpayer's recent returns, as lists.
pub(crate) fn phone_home() -> impl IntoElement {
    let st = store();
    let row = move |i: Icon, label: &'static str, line: &'static str, to: Route| {
        view()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .px(px(14.0))
            .min_h(px(56.0))
            .py(px(8.0))
            .cursor_pointer()
            .active(|s| s.bg(theme::HOVER))
            .on_press(move || st.go(to))
            .child(view().size(px(32.0)).rounded(px(10.0)).bg(theme::ACCENT_SOFT).items_center().justify_center().child(icon(i, 17.0, || theme::ACCENT)))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(text(label).text_size(px(16.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
                    .child(text(line).text_size(px(12.5)).text_color(theme::FG2)),
            )
            .child(icon(Icon::ChevronRight, 16.0, || theme::FG3))
    };
    let card = || view().rounded(px(theme::R_CARD)).border_1().border_color(theme::BORDER).bg(theme::SURFACE).overflow_hidden();
    let recent = move || match (st.returns.get(), st.selected.get()) {
        (crate::store::Loaded::Ready(rows), Some(sel)) => rows.into_iter().filter(|r| r.tin == sel.tin).take(6).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    content(
        view()
            .gap(px(16.0))
            .child(when(move || matches!(st.db.get(), Db::Failed(_)), library::db_error))
            .child(when(move || matches!(st.db.get(), Db::Ready(_)) && st.selected.get().is_none(), welcome))
            .child(when(move || st.selected.get().is_some(), move || page_title(move || st.selected.get().map(|s| s.name).unwrap_or_default())))
            .child(when(move || st.selected.get().is_some() && recent().is_empty(), move || {
                card().child(row(Icon::Library, "Start a return", "Pick a form and its period", Route::Library))
            }))
            .child(when(move || !recent().is_empty(), move || {
                view()
                    .gap(px(6.0))
                    .child(text("Recent").text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG3))
                    .child(card().child(each(recent, |r| r.id, move |r, index| {
                        let r = r.get_untracked();
                        let open = bir_app_core::OpenReturn { tin: r.tin.clone(), form_code: r.form_code.clone(), year: r.taxable_year, period: r.period, key: r.return_key };
                        let title = format!("{} · {}", r.form_code, r.period_label);
                        let t2 = title.clone();
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .px(px(14.0))
                            .min_h(px(52.0))
                            .border_t(move || px(if index.get() == 0 { 0.0 } else { 1.0_f32 }))
                            .border_color(theme::BORDER)
                            .cursor_pointer()
                            .active(|s| s.bg(theme::HOVER))
                            .on_press(move || st.form.open(open.clone(), t2.clone()))
                            .child(text(title).flex_1().min_w(px(0.0)).text_size(px(15.0)).text_color(theme::FG).truncate())
                            .child(crate::widgets::chip(r.status.clone(), crate::widgets::status_tone(&r.status)))
                            .child(icon(Icon::ChevronRight, 16.0, || theme::FG3))
                    })))
            })),
    )
}

/// Which returns the phone's Filings shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FilingsFilter {
    All,
    Drafts,
    Sending,
    Sent,
}

/// The phone's Filings: every return of the selected taxpayer, filtered
/// (All, Drafts, Sending, Sent — the send queue is "Sending").
pub(crate) fn phone_filings(sending: bool) -> impl IntoElement {
    let st = store();
    let filter = signal(if sending { FilingsFilter::Sending } else { FilingsFilter::All });
    let rows = move || match (st.returns.get(), st.selected.get()) {
        (crate::store::Loaded::Ready(rows), Some(sel)) => rows
            .into_iter()
            .filter(|r| r.tin == sel.tin)
            .filter(|r| match filter.get() {
                FilingsFilter::All => true,
                FilingsFilter::Drafts => r.status == "Draft",
                FilingsFilter::Sending => r.status == "Queued" || r.status == "Sending",
                FilingsFilter::Sent => !matches!(r.status.as_str(), "Draft" | "Queued" | "Sending"),
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let rows2 = rows.clone();
    let mut chips = view().flex_row().gap(px(8.0));
    for (f, label) in [(FilingsFilter::All, "All"), (FilingsFilter::Drafts, "Drafts"), (FilingsFilter::Sending, "Sending"), (FilingsFilter::Sent, "Sent")] {
        chips = chips.child(
            view()
                .h(px(44.0))
                .px(px(16.0))
                .rounded(px(22.0))
                .items_center()
                .justify_center()
                .bg(move || if filter.get() == f { theme::ACCENT_SOFT } else { theme::HOVER })
                .cursor_pointer()
                .label(label)
                .on_press(move || filter.set(f))
                .child(text(label).text_size(px(14.0)).font_weight(FontWeight::MEDIUM).text_color(move || if filter.get() == f { theme::ACCENT } else { theme::FG })),
        );
    }
    content(
        view()
            .gap(px(14.0))
            .child(chips)
            .child(when(move || rows2().is_empty(), move || {
                // An empty state always offers one way forward.
                view()
                    .gap(px(10.0))
                    .py(px(24.0))
                    .items_center()
                    .child(text("Nothing here yet").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                    .child(text("Returns you open save themselves and show here.").text_size(px(13.0)).text_color(theme::FG2))
                    .child(button("Start a return", Variant::Primary, || false, move || st.go(Route::Library)))
            }))
            .child(
                view().rounded(px(theme::R_CARD)).border_1().border_color(theme::BORDER).overflow_hidden().child(each(rows, |r| r.id, move |r, index| {
                    let r = r.get_untracked();
                    let open = bir_app_core::OpenReturn { tin: r.tin.clone(), form_code: r.form_code.clone(), year: r.taxable_year, period: r.period, key: r.return_key };
                    let title = format!("{} · {}", r.form_code, r.period_label);
                    let t2 = title.clone();
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(14.0))
                        .min_h(px(56.0))
                        .border_t(move || px(if index.get() == 0 { 0.0 } else { 1.0_f32 }))
                        .border_color(theme::BORDER)
                        .cursor_pointer()
                        .active(|s| s.bg(theme::HOVER))
                        .on_press(move || st.form.open(open.clone(), t2.clone()))
                        .child(text(title).flex_1().min_w(px(0.0)).text_size(px(15.0)).text_color(theme::FG).truncate())
                        .child(crate::widgets::chip(r.status.clone(), crate::widgets::status_tone(&r.status)))
                        .child(icon(Icon::ChevronRight, 16.0, || theme::FG3))
                })),
            ),
    )
}

/// The phone's Calendar: what is due, arriving with P4b's calendar; until
/// then a way forward.
pub(crate) fn phone_calendar() -> impl IntoElement {
    let st = store();
    content(
        view()
            .gap(px(10.0))
            .py(px(24.0))
            .items_center()
            .child(text("Deadlines arrive here").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
            .child(text(Route::Calendar.summary()).text_size(px(13.0)).text_color(theme::FG2))
            .child(button("Start a return", Variant::Primary, || false, move || st.go(Route::Library))),
    )
}

/// Home: the fresh-install steps until there is a taxpayer.
fn home() -> impl IntoElement {
    let st = store();
    content(
        view()
            .gap(px(16.0))
            .child(when(move || matches!(st.db.get(), Db::Failed(_)), library::db_error))
            .child(when(move || matches!(st.db.get(), Db::Ready(_)) && st.selected.get().is_none(), welcome))
            .child(when(move || st.selected.get().is_some(), move || {
                view()
                    .gap(px(8.0))
                    .child(page_title(move || st.selected.get().map(|s| s.name).unwrap_or_default()))
                    .child(text(Route::Home.summary()).text_size(px(13.0)).text_color(theme::FG2))
                    .child(view().flex_row().pt(px(8.0)).child(button("Open the form library", Variant::Secondary, || false, move || st.go(Route::Library))))
            })),
    )
}

fn welcome() -> impl IntoElement {
    let st = store();
    let step = |n: u32, i: Icon, title: &'static str, line: &'static str| {
        view()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .min_h(px(52.0))
            .py(px(8.0))
            .border_t(px(if n == 1 { 0.0 } else { 1.0 }))
            .border_color(theme::BORDER)
            .child(view().size(px(28.0)).rounded(px(theme::R_FULL)).bg(theme::HOVER).items_center().justify_center().child(icon(i, 16.0, || theme::FG2)))
            .child(
                view()
                    .flex_1()
                    .child(text(title).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
                    .child(text(line).text_size(px(12.0)).text_color(theme::FG3)),
            )
    };
    view()
        .gap(px(16.0))
        .child(page_title("Welcome to Buwiz"))
        .child(text("File your BIR returns offline: fill a form, check it, see its penalties, print it and queue it for sending.").text_size(px(13.0)).text_color(theme::FG2))
        .child(
            view()
                .rounded(px(theme::R_CARD))
                .border_1()
                .border_color(theme::BORDER)
                .overflow_hidden()
                .child(step(1, Icon::Users, "Add a taxpayer", "TIN, registered name and head-office address."))
                .child(step(2, Icon::Calendar, "Pick a period", "Any month, quarter or year a form is due for."))
                .child(step(3, Icon::FileText, "Open its form", "All 43 BIR forms, checked as you type."))
                .child(
                    view()
                        .px(px(16.0))
                        .py(px(12.0))
                        .border_t(px(1.0))
                        .border_color(theme::BORDER)
                        .flex_row()
                        .child(button("Add your first taxpayer", Variant::Primary, || false, move || st.new_taxpayer_open.set(true))),
                ),
        )
}
