//! The model: everything the screens show, as signals in one `Store`
//! anchored by [`crate::live::store`]. Views read the signals; the methods
//! here move the data. Calls into `bir-app-core` that touch the database
//! (or parse its compiled-in schema) run off the main thread and write
//! their results back here.

use bir_app_core::{BirApp, FormCatalogEntry};
use nsh_core::prelude::*;

use crate::screens::Route;

/// The forms library as loaded so far.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Catalog {
    Loading,
    Ready(Vec<FormCatalogEntry>),
    Failed(String),
}

pub(crate) struct Store {
    /// The screen the sidebar has selected.
    pub route: Signal<Route>,
    /// Every on-screen BIR form, for the Forms screen.
    pub catalog: Signal<Catalog>,
}

impl Store {
    pub(crate) fn new() -> Self {
        Self {
            route: signal(Route::Home),
            catalog: signal(Catalog::Loading),
        }
    }

    /// Load the forms catalog on a blocking thread (the first read parses
    /// the compiled-in schema), then publish it. Call once, under the root.
    pub(crate) fn load_catalog(&'static self) {
        let catalog = self.catalog;
        spawn(async move {
            let loaded = blocking(|| BirApp::form_catalog().map_err(|error| error.to_string())).await;
            catalog.set(match loaded {
                Ok(Ok(forms)) => Catalog::Ready(forms),
                Ok(Err(error)) => Catalog::Failed(error),
                Err(error) => Catalog::Failed(error.to_string()),
            });
        });
    }
}
