//! The model: everything the screens show, as signals in one `Store`
//! anchored by [`crate::live::store`]. Views read the signals; the methods
//! here move the data. Every call into `bir-app-core` runs on a blocking
//! thread ([`call`]) and writes its result back here; the main thread never
//! touches the database.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bir_app_core::{AppResult, BirApp, FormCatalogEntry, ReturnFilter, ReturnRow, Taxpayer};
use nsh_core::prelude::*;

use crate::live::store;
use crate::screens::Route;
use crate::session::FormSession;

/// The forms library as loaded so far.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Catalog {
    Loading,
    Ready(Vec<FormCatalogEntry>),
    Failed(String),
}

/// The opened database, compared by identity.
#[derive(Clone)]
pub(crate) struct AppRef(pub Arc<BirApp>);

impl PartialEq for AppRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The database, as opened at startup.
#[derive(Clone, PartialEq)]
pub(crate) enum Db {
    Opening,
    Ready(AppRef),
    Failed(String),
}

/// A list read from the database.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Loaded<T> {
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NoticeTone {
    Info,
    Success,
    Danger,
}

/// A one-line message at the top of the screen; `id` tells a newer one
/// from an older one when its timer ends.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Notice {
    pub id: u64,
    pub tone: NoticeTone,
    pub text: String,
}

/// The appearance the filer picked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ThemePref {
    System,
    Light,
    Dark,
}

/// The selected branch of a taxpayer, as the switcher shows it.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Selected {
    /// The 14-digit branch TIN every form call takes.
    pub tin: String,
    pub tin_display: String,
    pub name: String,
    pub branch_name: String,
    pub branch_count: usize,
}

pub(crate) struct Store {
    /// The owner every background task and timer runs under: the root,
    /// which outlives screens and remounts.
    pub owner: Owner,
    /// The screen shown; change it with [`Store::go`].
    pub route: Signal<Route>,
    /// Places behind and ahead of the current one (back / forward).
    pub history: Signal<Vec<Route>>,
    pub ahead: Signal<Vec<Route>>,
    pub sidebar_open: Signal<bool>,
    /// The tax-profile panel.
    pub panel_open: Signal<bool>,
    /// The tax profile as a sheet (the phone's).
    pub profile_open: Signal<bool>,
    /// The sidebar and panel as they were before a form closed them.
    pub before_form: Signal<Option<(bool, bool)>>,
    pub theme: Signal<ThemePref>,
    /// Every on-screen BIR form, for the Forms screen.
    pub catalog: Signal<Catalog>,
    pub db: Signal<Db>,
    pub taxpayers: Signal<Loaded<Vec<Taxpayer>>>,
    pub selected: Signal<Option<Selected>>,
    pub switcher_open: Signal<bool>,
    pub new_taxpayer_open: Signal<bool>,
    /// Saved and filed returns of every branch, newest first.
    pub returns: Signal<Loaded<Vec<ReturnRow>>>,
    pub library_query: Signal<String>,
    /// The form whose periods the library shows.
    pub library_pick: Signal<Option<String>>,
    pub library_year: Signal<i64>,
    pub notice: Signal<Option<Notice>>,
    /// The open return.
    pub form: FormSession,
}

impl Store {
    pub(crate) fn new() -> Self {
        Self {
            owner: Owner::current(),
            route: signal(Route::Home),
            history: signal(Vec::new()),
            ahead: signal(Vec::new()),
            sidebar_open: signal(true),
            panel_open: signal(false),
            profile_open: signal(false),
            before_form: signal(None),
            theme: signal(ThemePref::System),
            catalog: signal(Catalog::Loading),
            db: signal(Db::Opening),
            taxpayers: signal(Loaded::Loading),
            selected: signal(None),
            switcher_open: signal(false),
            new_taxpayer_open: signal(false),
            returns: signal(Loaded::Loading),
            library_query: signal(String::new()),
            library_pick: signal(None),
            library_year: signal(bir_app_core::display::current_year()),
            notice: signal(None),
            form: FormSession::new(),
        }
    }

    /// Show `to`, remembering where the filer was.
    pub(crate) fn go(&self, to: Route) {
        let from = self.route.get_untracked();
        if from == to {
            return;
        }
        self.history.update(|h| h.push(from));
        self.ahead.set(Vec::new());
        self.enter(from, to);
    }

    pub(crate) fn back(&self) {
        let Some(to) = self.history.get_untracked().last().copied() else { return };
        let from = self.route.get_untracked();
        self.history.update(|h| {
            h.pop();
        });
        self.ahead.update(|a| a.push(from));
        self.enter(from, to);
    }

    pub(crate) fn forward(&self) {
        let Some(to) = self.ahead.get_untracked().last().copied() else { return };
        let from = self.route.get_untracked();
        self.ahead.update(|a| {
            a.pop();
        });
        self.history.update(|h| h.push(from));
        self.enter(from, to);
    }

    /// A form takes the full width: entering one closes the sidebar and
    /// the panel, leaving it puts them back as they were.
    fn enter(&self, from: Route, to: Route) {
        batch(|| {
            if to == Route::Form && from != Route::Form {
                self.before_form.set(Some((self.sidebar_open.get_untracked(), self.panel_open.get_untracked())));
                self.sidebar_open.set(false);
                self.panel_open.set(false);
            } else if from == Route::Form && to != Route::Form {
                // Back keeps the draft: the last edits are saved now.
                store().form.save_now();
                if let Some((sidebar, panel)) = self.before_form.get_untracked() {
                    self.sidebar_open.set(sidebar);
                    self.panel_open.set(panel);
                }
                self.before_form.set(None);
            }
            self.route.set(to);
        });
    }

    /// Load the forms catalog on a blocking thread (the first read parses
    /// the compiled-in schema), then publish it. Call once, under the root.
    pub(crate) fn load_catalog(&'static self) {
        let catalog = self.catalog;
        task(async move {
            let loaded = blocking(|| BirApp::form_catalog().map_err(|error| error.to_string())).await;
            catalog.set(match loaded {
                Ok(Ok(forms)) => Catalog::Ready(forms),
                Ok(Err(error)) => Catalog::Failed(error),
                Err(error) => Catalog::Failed(error.to_string()),
            });
        });
    }

    /// Open the live database off the main thread, then load what the
    /// screens list. Call once, under the root.
    pub(crate) fn open_database(&'static self) {
        task(async move {
            let kept = crate::live::database().lock().ok().and_then(|db| db.clone());
            let opened = match kept {
                Some(app) => Ok(Ok(app)),
                None => blocking(|| BirApp::open_live().map(Arc::new).map_err(|error| error.to_string())).await,
            };
            match opened {
                Ok(Ok(app)) => {
                    if let Ok(mut db) = crate::live::database().lock() {
                        *db = Some(app.clone());
                    }
                    self.db.set(Db::Ready(AppRef(app)));
                    self.reload_taxpayers(None);
                    self.reload_returns();
                }
                Ok(Err(error)) => self.db.set(Db::Failed(error)),
                Err(error) => self.db.set(Db::Failed(error.to_string())),
            }
        });
    }

    pub(crate) fn app(&self) -> Option<Arc<BirApp>> {
        match self.db.get_untracked() {
            Db::Ready(app) => Some(app.0),
            _ => None,
        }
    }

    /// Read the taxpayers again; `select` (a branch TIN) becomes the
    /// selection, else the current one stays, else the first branch.
    pub(crate) fn reload_taxpayers(&'static self, select: Option<String>) {
        task(async move {
            match call(|app| app.list_taxpayers()).await {
                Ok(list) => {
                    let want = select.or_else(|| self.selected.get_untracked().map(|s| s.tin));
                    let pick = want
                        .and_then(|tin| select_branch(&list, &tin))
                        .or_else(|| list.first().and_then(|t| t.branches.first()).and_then(|b| select_branch(&list, &b.tin)));
                    self.selected.set(pick);
                    self.taxpayers.set(Loaded::Ready(list));
                    self.open_from_launch();
                }
                Err(error) => self.taxpayers.set(Loaded::Failed(error)),
            }
        });
    }

    /// `BUWIZ_OPEN=<form>:<year>:<period>` at launch (`ios.sh` passes it
    /// through): open that return of the selected taxpayer once the
    /// taxpayers are read; `BUWIZ_FOCUS=1` then focuses its first box (the
    /// keyboard and its row) — `BUWIZ_FOCUS=<n>` the n-th — `BUWIZ_OUTLINE=1` opens the outline. For
    /// screenshots and a scripted check of the simulator, where nothing
    /// else can tap.
    fn open_from_launch(&'static self) {
        static DONE: std::sync::Once = std::sync::Once::new();
        let Some(spec) = std::env::var("BUWIZ_OPEN").ok().filter(|v| !v.is_empty()) else { return };
        let Some(sel) = self.selected.get_untracked() else { return };
        DONE.call_once(|| {
            let mut parts = spec.split(':');
            let form = parts.next().unwrap_or("1701Q").to_owned();
            let year = parts.next().and_then(|y| y.parse().ok()).unwrap_or_else(bir_app_core::display::current_year);
            let period = parts.next().and_then(|p| p.parse().ok()).unwrap_or(1);
            let open = bir_app_core::OpenReturn { tin: sel.tin, form_code: form.clone(), year, period, key: None };
            self.form.open(open, format!("{form} · {year}"));
            let flag = |name: &str| std::env::var(name).is_ok_and(|v| v == "1");
            let focus: Option<usize> = std::env::var("BUWIZ_FOCUS").ok().and_then(|v| v.parse().ok());
            if focus.is_some() || flag("BUWIZ_OUTLINE") {
                self.owner.run(|| {
                    set_timeout(Duration::from_millis(2500), move || {
                        let st = store();
                        if flag("BUWIZ_OUTLINE") {
                            st.form.outline_open.set(true);
                        } else if let Some(n) = focus {
                            crate::screens::focus_nth(n);
                        }
                    })
                });
            }
        });
    }

    pub(crate) fn reload_returns(&'static self) {
        task(async move {
            self.returns.set(match call(|app| app.list_returns(ReturnFilter::All)).await {
                Ok(rows) => Loaded::Ready(rows),
                Err(error) => Loaded::Failed(error),
            });
        });
    }

    pub(crate) fn select(&self, tin: &str) {
        if let Loaded::Ready(list) = self.taxpayers.get_untracked() {
            self.selected.set(select_branch(&list, tin));
        }
        self.switcher_open.set(false);
    }

    /// Show the tax profile: a sheet on a phone, the side panel otherwise.
    pub(crate) fn open_profile(&self) {
        if crate::mobile::phone() {
            self.profile_open.set(true);
        } else {
            self.panel_open.set(true);
        }
    }

    /// Show `text` for a few seconds.
    pub(crate) fn notify(&'static self, tone: NoticeTone, text: impl Into<String>) {
        let id = self.notice.get_untracked().map_or(1, |n| n.id + 1);
        self.notice.set(Some(Notice { id, tone, text: text.into() }));
        let hold = if tone == NoticeTone::Danger { 8000 } else { 2600 };
        self.owner.run(|| {
            set_timeout(Duration::from_millis(hold), move || {
                if self.notice.get_untracked().is_some_and(|n| n.id == id) {
                    self.notice.set(None);
                }
            })
        });
    }
}

fn select_branch(list: &[Taxpayer], tin: &str) -> Option<Selected> {
    list.iter().find_map(|t| {
        t.branches.iter().find(|b| b.tin == tin).map(|b| Selected {
            tin: b.tin.clone(),
            tin_display: b.tin_display.clone(),
            name: t.name.clone(),
            branch_name: b.name.clone(),
            branch_count: t.branches.len(),
        })
    })
}

/// Run `future` under the root owner: it outlives the screen that
/// started it.
pub(crate) fn task(future: impl Future<Output = ()> + 'static) {
    store().owner.run(|| {
        spawn(future);
    });
}

/// Run `f` against the open database on a blocking thread; its error as
/// the filer's sentence.
pub(crate) async fn call<R: Send + 'static>(
    f: impl FnOnce(&BirApp) -> AppResult<R> + Send + 'static,
) -> Result<R, String> {
    let Some(app) = store().app() else {
        return Err("The database is not open.".to_owned());
    };
    match blocking(move || f(&app).map_err(|error| error.to_string())).await {
        Ok(result) => result,
        Err(error) => Err(format!("the worker stopped: {error}")),
    }
}
