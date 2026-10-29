//! The model: everything the screens show, as signals in one `Store`
//! anchored by [`crate::live::store`]. Views read the signals; the methods
//! here move the data. Every call into `bir-app-core` runs on a blocking
//! thread ([`call`]) and writes its result back here; the main thread never
//! touches the database.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bir_app_core::{AppResult, AsNeededForm, BirApp, DeadlineRow, FormCatalogEntry, ReturnFilter, ReturnRow, Taxpayer};
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

impl ThemePref {
    pub(crate) fn key(self) -> &'static str {
        match self {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "light" => ThemePref::Light,
            "dark" => ThemePref::Dark,
            _ => ThemePref::System,
        }
    }
}

/// Signing in to send (the device flow of bir-submit).
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum SignIn {
    /// Not read yet (the client starts with the database).
    Unknown,
    SignedOut,
    /// Asking the server for a code.
    Starting,
    /// Waiting for the person to approve `user_code` in a browser.
    Pending { user_code: String, uri: String, uri_complete: String },
    SignedIn { email: Option<String> },
}

/// The submission client, made once at launch (it is shared with the
/// queue worker's thread).
static SUBMISSION: std::sync::OnceLock<bir_app_core::bir_submit::Submission> = std::sync::OnceLock::new();

pub(crate) fn submission() -> Option<&'static bir_app_core::bir_submit::Submission> {
    SUBMISSION.get()
}

/// Which returns Filings lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FilingsFilter {
    All,
    Drafts,
    Sending,
    Sent,
}

impl FilingsFilter {
    pub(crate) fn keeps(self, status: &str) -> bool {
        match self {
            FilingsFilter::All => true,
            FilingsFilter::Drafts => status == "Draft",
            FilingsFilter::Sending => status == "Queued" || status == "Sending",
            FilingsFilter::Sent => !matches!(status, "Draft" | "Queued" | "Sending"),
        }
    }
}

/// A filled print sheet shown in the preview.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Preview {
    pub title: String,
    pub form_code: String,
    pub html: String,
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
    /// Deadlines from 90 days back to a year ahead, every branch.
    pub deadlines: Signal<Loaded<Vec<DeadlineRow>>>,
    /// This year's as-needed forms, every branch.
    pub as_needed: Signal<Loaded<Vec<AsNeededForm>>>,
    /// Due-date reminders at launch (Settings).
    pub reminders: Signal<bool>,
    /// Where each return is on its way to BIR, from the database.
    pub send_stages: Signal<std::collections::BTreeMap<i64, bir_app_core::SendStage>>,
    pub sign_in: Signal<SignIn>,
    pub filings_filter: Signal<FilingsFilter>,
    /// The calendar lists every taxpayer, not only the selected branch.
    pub calendar_all: Signal<bool>,
    /// The command palette (⌘K).
    pub palette_open: Signal<bool>,
    /// The phone's start-a-return sheet, for a form code.
    pub start_sheet: Signal<Option<String>>,
    /// The phone's search over forms and filings.
    pub search_open: Signal<bool>,
    /// The draft the delete confirm asks about.
    pub confirm_delete: Signal<Option<ReturnRow>>,
    /// The return a status page shows (queued, sent, failed).
    pub status_of: Signal<Option<ReturnRow>>,
    /// The print preview.
    pub preview: Signal<Option<Preview>>,
    /// The swiped-open row of a list (one at a time).
    pub swiped: Signal<Option<i64>>,
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
            deadlines: signal(Loaded::Loading),
            as_needed: signal(Loaded::Loading),
            reminders: signal(true),
            send_stages: signal(std::collections::BTreeMap::new()),
            sign_in: signal(SignIn::Unknown),
            filings_filter: signal(FilingsFilter::All),
            calendar_all: signal(false),
            palette_open: signal(false),
            start_sheet: signal(None),
            search_open: signal(false),
            confirm_delete: signal(None),
            status_of: signal(None),
            preview: signal(None),
            swiped: signal(None),
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
                    self.load_settings();
                    self.reload_taxpayers(None);
                    self.reload_returns();
                    self.reload_deadlines();
                    self.start_sending();
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
        launch_hooks(self);
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
            let pick = std::env::var("BUWIZ_PICK").ok().filter(|v| !v.is_empty());
            if focus.is_some() || flag("BUWIZ_OUTLINE") || pick.is_some() {
                self.owner.run(|| {
                    // A loaded machine can take seconds to load the form:
                    // wait for it, then let its rows mount.
                    task(async move {
                        for _ in 0..120 {
                            if untracked(|| store().form.form().is_some()) {
                                break;
                            }
                            timer(Duration::from_millis(250)).await;
                        }
                        timer(Duration::from_millis(1500)).await;
                        let st = store();
                        if flag("BUWIZ_OUTLINE") {
                            st.form.outline_open.set(true);
                        } else if let Some(n) = focus {
                            crate::screens::focus_nth(n);
                        } else if let Some(pick) = pick.as_deref() {
                            crate::screens::pick_on_launch(pick);
                        }
                    })
                });
            }
        });
    }

    pub(crate) fn reload_returns(&'static self) {
        self.reload_send_stages();
        task(async move {
            self.returns.set(match call(|app| app.list_returns(ReturnFilter::All)).await {
                Ok(rows) => Loaded::Ready(rows),
                Err(error) => Loaded::Failed(error),
            });
        });
    }

    /// Deadlines (90 days back to a year ahead) and this year's as-needed
    /// forms, every branch.
    pub(crate) fn reload_deadlines(&'static self) {
        task(async move {
            let today = today();
            let (from, to) = ((today - chrono::Duration::days(90)).to_string(), (today + chrono::Duration::days(366)).to_string());
            let year = bir_app_core::display::current_year();
            match call(move |app| Ok((app.deadlines_between(&from, &to)?, app.as_needed_forms(year)?))).await {
                Ok((due, needed)) => {
                    self.deadlines.set(Loaded::Ready(due));
                    self.as_needed.set(Loaded::Ready(needed));
                    self.remind_once();
                }
                Err(error) => {
                    self.deadlines.set(Loaded::Failed(error.clone()));
                    self.as_needed.set(Loaded::Failed(error));
                }
            }
        });
    }

    /// The returns list and the deadlines, after a return changed.
    pub(crate) fn reload_lists(&'static self) {
        self.reload_returns();
        self.reload_deadlines();
    }

    /// The appearance and the reminders setting, as last saved.
    fn load_settings(&'static self) {
        task(async move {
            if let Ok((theme, reminders)) = call(|app| Ok((app.get_setting("ui.theme")?, app.get_setting("reminders.due")?))).await {
                if let Some(theme) = theme {
                    self.theme.set(ThemePref::from_key(&theme));
                }
                self.reminders.set(reminders.as_deref() != Some("off"));
            }
            send_hooks(self);
        });
    }

    /// Pick an appearance and keep it.
    pub(crate) fn set_theme(&'static self, pref: ThemePref) {
        self.theme.set(pref);
        task(async move {
            let _ = call(move |app| app.set_setting("ui.theme", pref.key())).await;
        });
    }

    pub(crate) fn set_reminders(&'static self, on: bool) {
        self.reminders.set(on);
        task(async move {
            let _ = call(move |app| app.set_setting("reminders.due", if on { "on" } else { "off" })).await;
        });
    }

    /// At launch, with reminders on: one note naming what is due within
    /// three days (or late) for the selected branch.
    fn remind_once(&'static self) {
        static DONE: std::sync::Once = std::sync::Once::new();
        if !self.reminders.get_untracked() {
            return;
        }
        let (Loaded::Ready(rows), Some(sel)) = (self.deadlines.get_untracked(), self.selected.get_untracked()) else { return };
        let soon: Vec<DeadlineRow> = rows
            .into_iter()
            .filter(|d| d.tin == sel.tin && matches!(d.status.as_str(), "Not started" | "Draft") && days_until(&d.due_date) <= 3)
            .collect();
        let Some(first) = soon.iter().min_by_key(|d| d.due_date.clone()).cloned() else { return };
        DONE.call_once(|| {
            let n = days_until(&first.due_date);
            let when = match n {
                0 => "today".to_owned(),
                1 => "tomorrow".to_owned(),
                n if n < 0 => format!("{} days late", -n),
                n => format!("in {n} days"),
            };
            let more = if soon.len() > 1 { format!(" (and {} more)", soon.len() - 1) } else { String::new() };
            self.notify(NoticeTone::Info, format!("{} · {} is due {when}{more}.", first.form_code, first.period_label));
        });
    }

    /// Where return `id` is on its way to BIR (tracked).
    pub(crate) fn send_stage_of(&self, id: i64) -> Option<bir_app_core::SendStage> {
        self.send_stages.with(|m| m.get(&id).cloned())
    }

    /// Read every return's send stage again.
    pub(crate) fn reload_send_stages(&'static self) {
        task(async move {
            if let Ok(stages) = call(|app| app.send_states()).await
                && self.send_stages.with_untracked(|now| *now != stages)
            {
                let left_queue = self.send_stages.with_untracked(|old| {
                    old.iter().any(|(id, s)| matches!(s, bir_app_core::SendStage::Sending) && !matches!(stages.get(id), Some(bir_app_core::SendStage::Sending)))
                });
                self.send_stages.set(stages);
                if left_queue {
                    self.reload_lists();
                }
            }
        });
    }

    /// At launch, off the main thread: the submission client and the queue
    /// worker, then the sign-in state; the send stages are read again every
    /// few seconds so Sending, Sent and Failed show as the worker moves.
    fn start_sending(&'static self) {
        task(async move {
            match call(|app| app.launch_sending()).await {
                Ok(client) => {
                    let _ = SUBMISSION.set(client);
                    self.read_sign_in();
                }
                Err(why) => {
                    log::warn!("sending not started: {why}");
                    self.sign_in.set(SignIn::SignedOut);
                }
            }
        });
        self.owner.run(|| {
            set_interval(Duration::from_secs(4), move || self.reload_send_stages());
        });
    }

    fn read_sign_in(&'static self) {
        let Some(client) = submission() else { return };
        self.sign_in.set(match client.sign_in_status() {
            bir_app_core::bir_submit::SignInStatus::SignedIn { email } => SignIn::SignedIn { email },
            _ => SignIn::SignedOut,
        });
    }

    /// Start signing in: show the code, then poll until it is approved,
    /// denied or expired.
    pub(crate) fn sign_in_start(&'static self) {
        let Some(client) = submission().cloned() else {
            self.notify(NoticeTone::Danger, "Sending isn't ready yet. Try again in a moment.");
            return;
        };
        self.sign_in.set(SignIn::Starting);
        task(async move {
            let c = client.clone();
            let started = blocking(move || bir_app_core::block_on(c.start_sign_in())).await;
            let code = match started {
                Ok(Ok(code)) => code,
                Ok(Err(error)) => {
                    self.sign_in.set(SignIn::SignedOut);
                    self.notify(NoticeTone::Danger, bir_app_core::plain_send_error(&error.to_string()).text);
                    return;
                }
                Err(error) => {
                    self.sign_in.set(SignIn::SignedOut);
                    self.notify(NoticeTone::Danger, error.to_string());
                    return;
                }
            };
            self.sign_in.set(SignIn::Pending { user_code: code.user_code.clone(), uri: code.verification_uri.clone(), uri_complete: code.verification_uri_complete.clone() });
            let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in.max(30) as u64);
            let mut interval = code.interval.max(2) as u64;
            loop {
                timer(Duration::from_secs(interval)).await;
                if !matches!(self.sign_in.get_untracked(), SignIn::Pending { .. }) {
                    return;
                }
                if std::time::Instant::now() > deadline {
                    self.sign_in.set(SignIn::SignedOut);
                    self.notify(NoticeTone::Info, "The sign-in code expired. Start again.");
                    return;
                }
                let c = client.clone();
                match blocking(move || bir_app_core::block_on(c.poll_sign_in())).await {
                    Ok(Ok(bir_app_core::bir_submit::PollOutcome::Pending { interval: next })) => interval = next.max(2) as u64,
                    Ok(Ok(bir_app_core::bir_submit::PollOutcome::SignedIn)) => {
                        self.read_sign_in();
                        self.notify(NoticeTone::Success, "Signed in. Queued returns send now.");
                        if let Some(c) = submission() {
                            c.send_now();
                        }
                        return;
                    }
                    Ok(Ok(bir_app_core::bir_submit::PollOutcome::Denied)) => {
                        self.sign_in.set(SignIn::SignedOut);
                        self.notify(NoticeTone::Info, "Sign-in was declined.");
                        return;
                    }
                    Ok(Ok(bir_app_core::bir_submit::PollOutcome::Expired)) => {
                        self.sign_in.set(SignIn::SignedOut);
                        self.notify(NoticeTone::Info, "The sign-in code expired. Start again.");
                        return;
                    }
                    // A network hiccup: keep polling until the code expires.
                    Ok(Err(_)) | Err(_) => {}
                }
            }
        });
    }

    pub(crate) fn sign_in_cancel(&'static self) {
        self.sign_in.set(SignIn::SignedOut);
    }

    pub(crate) fn sign_out(&'static self) {
        let Some(client) = submission().cloned() else { return };
        task(async move {
            let _ = blocking(move || bir_app_core::block_on(client.sign_out())).await;
            bir_app_core::set_sending_paused(true);
            self.read_sign_in();
            self.notify(NoticeTone::Info, "Signed out. Queued returns wait until you sign in again.");
        });
    }

    /// Send a failed return again now (not after its backoff).
    pub(crate) fn retry(&'static self, id: i64) {
        task(async move {
            match call(move |app| app.retry_now(id)).await {
                Ok(()) => {
                    if let Some(c) = submission() {
                        c.send_now();
                    }
                    self.notify(NoticeTone::Success, "Sending again now.");
                    self.reload_send_stages();
                }
                Err(why) => self.notify(NoticeTone::Danger, why),
            }
        });
    }

    /// Take a queued return back to Draft.
    pub(crate) fn cancel_queued(&'static self, row: ReturnRow) {
        task(async move {
            let open = open_of(&row);
            let code = row.form_code.clone();
            let answer = call(move |app| {
                let opened = app.open_return(&open)?;
                app.cancel_queued(&code, &opened.draft)
            })
            .await;
            match answer {
                Ok(()) => {
                    self.notify(NoticeTone::Info, "Taken off the queue: an editable draft again.");
                    self.reload_lists();
                }
                Err(why) => self.notify(NoticeTone::Danger, format!("Still queued: {why}")),
            }
        });
    }

    /// Delete a draft (after the confirm).
    pub(crate) fn delete_draft(&'static self, row: ReturnRow) {
        task(async move {
            let id = row.id;
            match call(move |app| app.delete_draft(id)).await {
                Ok(()) => {
                    self.notify(NoticeTone::Info, format!("{} · {} deleted.", row.form_code, row.period_label));
                    self.reload_lists();
                }
                Err(why) => self.notify(NoticeTone::Danger, format!("Not deleted: {why}")),
            }
        });
    }

    /// Open a saved or filed return.
    pub(crate) fn open_row(&'static self, row: &ReturnRow) {
        self.form.open(open_of(row), format!("{} · {}", row.form_code, row.period_label));
    }

    /// Open a row of a list: a draft opens its form; a queued, sent or
    /// failed return opens its status page on a phone.
    pub(crate) fn open_any(&'static self, row: &ReturnRow) {
        if crate::mobile::phone() && row.status != "Draft" {
            self.status_of.set(Some(row.clone()));
            self.go(Route::Status);
        } else {
            self.open_row(row);
        }
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

/// Today where the app runs.
pub(crate) fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

/// Days from today to `iso` (`YYYY-MM-DD`); negative when it is past.
pub(crate) fn days_until(iso: &str) -> i64 {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d").map_or(0, |d| (d - today()).num_days())
}

pub(crate) fn open_of(row: &ReturnRow) -> bir_app_core::OpenReturn {
    bir_app_core::OpenReturn { tin: row.tin.clone(), form_code: row.form_code.clone(), year: row.taxable_year, period: row.period, key: row.return_key }
}

/// Launch hooks for screenshots and scripted checks where nothing can tap
/// (the iOS simulator; Android through `launch.env`), run once the
/// taxpayers are read:
///
/// - `BUWIZ_ROUTE=home|forms|filings|queue|calendar|settings|taxpayers`
/// - `BUWIZ_THEME=light|dark`
/// - `BUWIZ_SHEET=start:<form>|search|delete|profile|new-taxpayer|palette`
///   (delete: the first draft's confirm; profile: the panel, or the phone's
///   taxpayer sheet)
/// - `BUWIZ_STATUS=<form>|sent|failed|sending|queued|attention`: the page of
///   that return (a form's first that left Draft, or the first in that
///   send stage)
/// - with `BUWIZ_OPEN`: `BUWIZ_PICK=choice|list|schedule|row` opens the
///   form's first short-choice sheet, long-list search, schedule (the
///   phone's schedule page; the desktop scrolls to the table) or the first
///   schedule row's page
/// - `BUWIZ_SWIPE=1`: the first Filings row swiped open
/// - `BUWIZ_PRINT=<form>`: print preview of that form's first return;
///   `BUWIZ_PRINT_DIALOG=1|share` then opens the print dialog or share sheet
fn launch_hooks(st: &'static Store) {
    static DONE: std::sync::Once = std::sync::Once::new();
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    DONE.call_once(|| {
        if let Some(theme) = var("BUWIZ_THEME") {
            st.theme.set(ThemePref::from_key(&theme));
        }
        let route = var("BUWIZ_ROUTE");
        let sheet = var("BUWIZ_SHEET");
        let status = var("BUWIZ_STATUS");
        let swipe = var("BUWIZ_SWIPE").is_some();
        let print = var("BUWIZ_PRINT");
        let dialog = var("BUWIZ_PRINT_DIALOG");
        if route.is_none() && sheet.is_none() && status.is_none() && print.is_none() && !swipe {
            return;
        }
        task(async move {
            // The lists first.
            for _ in 0..100 {
                if matches!(st.returns.get_untracked(), Loaded::Ready(_)) && matches!(st.deadlines.get_untracked(), Loaded::Ready(_) | Loaded::Failed(_)) {
                    break;
                }
                timer(Duration::from_millis(100)).await;
            }
            timer(Duration::from_millis(600)).await;
            let rows = match st.returns.get_untracked() {
                Loaded::Ready(rows) => rows,
                _ => Vec::new(),
            };
            if let Some(route) = route {
                let to = match route.as_str() {
                    "forms" | "library" => Route::Library,
                    "filings" => Route::Filings,
                    "queue" => Route::Queue,
                    "calendar" => Route::Calendar,
                    "settings" => Route::Settings,
                    "taxpayers" => Route::Taxpayer,
                    _ => Route::Home,
                };
                st.go(to);
            }
            if swipe {
                st.go(Route::Filings);
                if let Some(r) = rows.iter().find(|r| r.status == "Draft") {
                    timer(Duration::from_millis(400)).await;
                    st.swiped.set(Some(r.id));
                }
            }
            // A form code, or the stage of the return to show.
            let stages = st.send_stages.get_untracked();
            let stage_is = |r: &ReturnRow, want: &str| match (want, stages.get(&r.id)) {
                ("sent", Some(bir_app_core::SendStage::Sent { .. })) => true,
                ("failed", Some(bir_app_core::SendStage::Failed { .. })) => true,
                ("sending", Some(bir_app_core::SendStage::Sending)) => true,
                ("attention", Some(bir_app_core::SendStage::NeedsAttention { .. })) => true,
                ("queued", Some(bir_app_core::SendStage::Queued { .. })) => true,
                _ => false,
            };
            if let Some(code) = status
                && let Some(r) = rows.iter().find(|r| (r.form_code == code && r.status != "Draft") || stage_is(r, &code))
            {
                st.status_of.set(Some(r.clone()));
                st.go(Route::Status);
            }
            match sheet.as_deref() {
                Some("search") => st.search_open.set(true),
                Some("profile") => st.open_profile(),
                Some("new-taxpayer") => st.new_taxpayer_open.set(true),
                Some("palette") => st.palette_open.set(true),
                Some("delete") => st.confirm_delete.set(rows.iter().find(|r| r.status == "Draft").cloned()),
                Some(s) if s.starts_with("start:") => st.start_sheet.set(Some(s[6..].to_owned())),
                _ => {}
            }
            if let Some(code) = print
                && let Some(r) = rows.iter().find(|r| r.form_code == code)
            {
                crate::screens::preview::print_row(r.clone());
                if let Some(how) = dialog {
                    for _ in 0..100 {
                        if st.preview.get_untracked().is_some() {
                            break;
                        }
                        timer(Duration::from_millis(100)).await;
                    }
                    timer(Duration::from_millis(1500)).await;
                    let act = if how == "share" { crate::screens::preview::Act::Share } else { crate::screens::preview::Act::Print };
                    crate::screens::preview::run(act, st.owner.run(|| signal(false)));
                }
            }
        });
    });
}

/// Launch hooks that put real send states in the database (demo data,
/// where no relay runs): `BUWIZ_SENDING=<form>`, `BUWIZ_SEND_FAIL=<form>`
/// (the relay's `tin_not_active`), `BUWIZ_SENT=<form>` (with a demo
/// receipt) and `BUWIZ_SEND_UNKNOWN=<form>` move that form's queued
/// returns there.
fn send_hooks(st: &'static Store) {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let wants: Vec<(String, bir_app_core::SendStage)> = [
        ("BUWIZ_SENDING", bir_app_core::SendStage::Sending),
        (
            "BUWIZ_SEND_FAIL",
            bir_app_core::SendStage::Failed {
                reason: "Not sent (nothing reached BIR): TIN 000123453 is not active on this account (tin_not_active)".to_owned(),
                attempts: 1,
                next_retry_at: Some((chrono::Utc::now() + chrono::Duration::minutes(2)).to_rfc3339()),
            },
        ),
        ("BUWIZ_SENT", bir_app_core::SendStage::Sent { at: None, receipt: None }),
        ("BUWIZ_SEND_UNKNOWN", bir_app_core::SendStage::NeedsAttention { reason: "Needs attention: the connection ended during the send".to_owned() }),
    ]
    .into_iter()
    .filter_map(|(name, stage)| var(name).map(|form| (form, stage)))
    .collect();
    if wants.is_empty() {
        return;
    }
    task(async move {
        for _ in 0..100 {
            if let Loaded::Ready(rows) = st.returns.get_untracked() {
                let mut jobs: Vec<(i64, bir_app_core::SendStage)> = Vec::new();
                for (form, stage) in &wants {
                    if let Some(r) = rows.iter().find(|r| r.status == "Queued" && &r.form_code == form && !jobs.iter().any(|(id, _)| *id == r.id)) {
                        jobs.push((r.id, stage.clone()));
                    }
                }
                let _ = call(move |app| {
                    for (id, stage) in &jobs {
                        app.demo_set_send_stage(*id, stage)?;
                    }
                    Ok(())
                })
                .await;
                st.reload_lists();
                st.reload_send_stages();
                return;
            }
            timer(Duration::from_millis(100)).await;
        }
    });
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
