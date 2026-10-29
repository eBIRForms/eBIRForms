//! One module per screen. The sidebar lists them in [`Route::ALL`] order;
//! each is a placeholder until phase P4 ports it.

use nsh_core::prelude::*;

use crate::live::store;
use crate::store::Catalog;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Route {
    Home,
    Library,
    Filings,
    Queue,
    Calendar,
    Taxpayers,
    Settings,
}

impl Route {
    pub(crate) const ALL: [Route; 7] = [
        Route::Home,
        Route::Library,
        Route::Filings,
        Route::Queue,
        Route::Calendar,
        Route::Taxpayers,
        Route::Settings,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Route::Home => "Home",
            Route::Library => "Forms",
            Route::Filings => "Filings",
            Route::Queue => "Send queue",
            Route::Calendar => "Calendar",
            Route::Taxpayers => "Taxpayers",
            Route::Settings => "Settings",
        }
    }

    /// What the screen will do, shown until it is ported.
    fn summary(self) -> &'static str {
        match self {
            Route::Home => "Returns due in the next 30 days, as-needed forms, recently filed.",
            Route::Library => "All 43 BIR forms. Open one to start a return.",
            Route::Filings => "Returns that left Draft.",
            Route::Queue => "Queued, sending, sent and failed returns, with receipts.",
            Route::Calendar => "Deadlines for the next 12 months, and your overrides.",
            Route::Taxpayers => "Profiles, branches, tax years and yearly forms.",
            Route::Settings => "Database, print and submission settings.",
        }
    }
}

/// The selected screen's body.
pub(crate) fn screen(route: Route) -> impl IntoElement {
    let body = view()
        .flex_1()
        .pt(px(56.0))
        .pl(px(40.0))
        .pr(px(40.0))
        .gap(px(10.0))
        .child(text(route.label()).text_size(px(28.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
        .child(text(route.summary()).text_size(px(15.0)).text_color(theme::MUTED));
    if route == Route::Library { body.child(library()) } else { body }
}

/// The Forms screen's list: every form's code and title from the catalog.
/// A plain list until P4 ports the real library.
fn library() -> impl IntoElement {
    let status = move || match store().catalog.get() {
        Catalog::Loading => "Loading forms…".to_owned(),
        Catalog::Ready(forms) => format!("{} forms", forms.len()),
        Catalog::Failed(error) => format!("Could not load the forms: {error}"),
    };
    let rows = move || match store().catalog.get() {
        Catalog::Ready(forms) => forms,
        _ => Vec::new(),
    };
    view()
        .flex_1()
        .gap(px(8.0))
        .pb(px(24.0))
        .child(text(status).text_size(px(12.0)).text_color(theme::MUTED))
        .child(scroll().flex_1().child(view().gap(px(2.0)).child(each(
            rows,
            |form| form.code.clone(),
            |form, _| {
                let form = form.get_untracked();
                view()
                    .flex_row()
                    .gap(px(14.0))
                    .py(px(7.0))
                    .border_b(px(1.0))
                    .border_color(theme::LINE)
                    .child(text(form.code).w(px(72.0)).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
                    .child(text(form.title).flex_1().text_size(px(14.0)).text_color(theme::TEXT))
            },
        ))))
}
