//! One module per screen. The sidebar lists them in [`Route::ALL`] order;
//! each is a placeholder until phase P4 ports it.

use nsh_core::prelude::*;

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
            Route::Library => "All 44 BIR forms. Open one to start a return.",
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
    view()
        .flex_1()
        .pt(px(56.0))
        .pl(px(40.0))
        .pr(px(40.0))
        .gap(px(10.0))
        .child(text(route.label()).text_size(px(28.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
        .child(text(route.summary()).text_size(px(15.0)).text_color(theme::MUTED))
}
