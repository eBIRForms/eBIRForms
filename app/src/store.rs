//! The model: everything the screens show, as signals in one `Store`
//! anchored by [`crate::live::store`]. Views read the signals; the methods
//! here move the data. Calls into `bir-app-core` that touch the database
//! run off the main thread and write their results back here.

use nsh_core::prelude::*;

use crate::screens::Route;

pub(crate) struct Store {
    /// The screen the sidebar has selected.
    pub route: Signal<Route>,
}

impl Store {
    pub(crate) fn new() -> Self {
        Self { route: signal(Route::Home) }
    }
}
