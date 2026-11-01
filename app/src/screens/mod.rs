//! One module per screen. The sidebar lists Home, Form library, Calendar,
//! Filings and Send queue; `Form` is reached from the library or Recent.

mod calendar;
pub(crate) use calendar::calendar_rows;
pub(crate) mod deadline_sheet;
mod filings;
pub(crate) use filings::delete_dialog as filings_delete_dialog;
mod form;
pub(crate) use form::focus_nth;
pub(crate) use form::mono as form_mono;
mod home;
mod library;
pub(crate) mod palette;
pub(crate) mod phone;
pub(crate) mod phone_fields;
pub(crate) mod phone_form;
pub(crate) mod preview;
mod settings;
pub(crate) mod steps;
pub(crate) mod taxpayer;
mod taxpayers;

use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::live::store;
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
    /// The selected taxpayer: profile, branches, every taxpayer.
    Taxpayer,
    /// A queued, sent or failed return's page (phone).
    Status,
    /// The print preview.
    Preview,
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
            Route::Taxpayer => "Taxpayers",
            Route::Status => "Return",
            Route::Preview => "Print preview",
        }
    }

    /// The sidebar entry that stays current on this screen.
    pub(crate) fn nav(self) -> Route {
        match self {
            Route::Form | Route::Preview => Route::Library,
            other => other,
        }
    }
}

/// The selected screen's body.
pub(crate) fn screen(route: Route) -> AnyElement {
    match route {
        Route::Library => library::screen().into_element(),
        Route::Form => form::screen().into_element(),
        Route::Home => home::screen().into_element(),
        Route::Filings => filings::screen(false).into_element(),
        Route::Queue => filings::screen(true).into_element(),
        Route::Calendar => calendar::screen().into_element(),
        Route::Settings => settings::screen().into_element(),
        Route::Taxpayer => taxpayers::screen().into_element(),
        Route::Preview => preview::screen().into_element(),
        Route::Status => phone::status_page().into_element(),
    }
}

/// The content column of non-form pages: at most 880 wide, padding 28 × 32
/// (20 × 16 on a narrow window), scrolling vertically only.
pub(crate) fn content(inner: impl IntoElement) -> impl IntoElement {
    content_in(element_ref(), element_ref(), inner)
}

/// [`content`] with refs on its scroll view (`list`) and the content it
/// scrolls (`body`), for a page that scrolls something into view
/// ([`reveal_in`]).
pub(crate) fn content_in(list: ElementRef, body: ElementRef, inner: impl IntoElement) -> impl IntoElement {
    let narrow = || window().width.0 < theme::BREAK_NARROW;
    scroll().element_ref(&list).flex_1().child(
        view().element_ref(&body).w_full().items_center().child(
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

/// Scroll [`content_in`]'s `list` so `at` (in its `body`) is in full view
/// once its place settles, and again while the page and the window settle
/// (for two seconds); nothing while it is in view.
pub(crate) fn reveal_in(list: ElementRef, body: ElementRef, at: ElementRef) {
    crate::store::task(async move {
        let start = std::time::Instant::now();
        let (mut last, mut still, mut done) = (None, 0, None);
        while start.elapsed() < std::time::Duration::from_secs(4) {
            if let (Some(a), Some(b), Some(l)) = (at.measure().await, body.measure().await, list.measure().await) {
                let top = a.page.y.0 - b.page.y.0;
                let bottom = top + a.frame.size.height.0;
                let view_h = l.frame.size.height.0;
                let place = Some((top, bottom, view_h));
                still = if place == last { still + 1 } else { 0 };
                last = place;
                if still >= 3 && done != place {
                    done = place;
                    let pad = 16.0;
                    let shown = l.page.y.0 - b.page.y.0;
                    if bottom + pad > shown + view_h || top < shown {
                        let y = (bottom + pad - view_h).min(top - pad).max(0.0);
                        list.scroll_to(Point { x: px(0.0), y: px(y) }, false);
                    }
                }
                if done.is_some() && start.elapsed() > std::time::Duration::from_secs(2) {
                    return;
                }
            }
            timer(std::time::Duration::from_millis(50)).await;
        }
    });
}

/// The page title, 20/600, one per view.
pub(crate) fn page_title(title: impl IntoReactive<SharedString>) -> Text {
    text(title).text_size(px(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG)
}

pub(crate) fn welcome() -> impl IntoElement {
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
        .child(page_title("Welcome to eBIRForms"))
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

/// `BUWIZ_UNLOCK` (a launch hook): press Edit penalties (`penalties`) or
/// Edit on this return (`profile`), then bring the first opened box into
/// view (the desktop's penalties panel is at the top already); for
/// `profile`, the first box changed on this return when there is one.
pub(crate) fn unlock_on_launch(which: &str) {
    let st = &store().form;
    let Some(form) = st.form() else { return };
    let keys: Vec<String> = if which == "penalties" {
        form::unlock_penalties();
        untracked(|| st.penalties.get().into_iter().filter(|p| p.applies).flat_map(|p| p.keys).collect())
    } else {
        form::unlock_profile();
        let all: Vec<String> = untracked(|| st.profile.get().into_iter().collect());
        let changed: Vec<String> = untracked(|| all.iter().filter(|k| st.profile_changed(k)).cloned().collect());
        if changed.is_empty() { all } else { changed }
    };
    if which == "penalties" && !crate::mobile::phone() {
        return;
    }
    let ids = untracked(|| steps::shown_items(st, &form));
    match ids.into_iter().find(|id| form.item(*id).is_some_and(|item| item.box_keys().iter().any(|k| keys.contains(k)))) {
        Some(id) => form::jump_to(id, false),
        None => log::warn!("BUWIZ_UNLOCK={which}: nothing opened on {}", form.layout.code),
    }
}

/// `BUWIZ_PICK` (a launch hook): open the open form's first short-choice
/// sheet (`choice`), long-list search (`list`), schedule (`schedule`) or
/// first schedule row (`row`).
pub(crate) fn pick_on_launch(pick: &str) {
    use phone_fields::{Kind, kind_of};
    // The more menu opens with the form (phone_form), no item to pick.
    if pick == "menu" {
        return;
    }
    let st = &store().form;
    let Some(form) = st.form() else { return };
    let ids = untracked(|| steps::shown_items(st, &form));
    let want = |k: Kind| match pick {
        "choice" => k == Kind::ShortChoice,
        "list" => matches!(k, Kind::LongList | Kind::Place),
        "how" => k == Kind::Computed,
        "payee" => k == Kind::Tin,
        _ => k == Kind::Schedule,
    };
    let Some(id) = ids.into_iter().find(|id| form.item(*id).is_some_and(|item| want(kind_of(item, &item.fields)))) else {
        log::warn!("BUWIZ_PICK={pick}: no such item on {}", form.layout.code);
        return;
    };
    if !crate::mobile::phone() {
        form::jump_to(id, false);
        return;
    }
    match pick {
        "schedule" => st.schedule_page.set(Some(id)),
        "row" => {
            st.schedule_page.set(Some(id));
            st.row_edit.set(Some((id, 0)));
        }
        "how" => st.explain.set(Some(id)),
        "payee" => st.payee.set(Some((id, None))),
        _ => st.picker.set(Some(id)),
    }
}
