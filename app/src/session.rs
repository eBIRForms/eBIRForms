//! The open return: its layout, its values and messages as signals, and
//! the loop that writes edits back through `bir-app-core`.
//!
//! Typing stages an edit; [`FormSession::edit`] flushes the staged edits
//! after a short pause (or at once for choices and on blur): one blocking
//! call fills the draft and validates it, and the answer replaces values,
//! choices, locks, messages and penalties. Fills never overlap: each takes
//! the draft the last one returned ([`FormSession::flush`] waits for the
//! one in flight).
//!
//! The return saves itself: each written edit schedules a save once typing
//! has paused for [`AUTOSAVE`] ([`FormSession::autosave`]), off the main
//! thread like every other call; [`SaveState`] tells the title what it is
//! doing. Leaving the form saves at once ([`FormSession::save_now`]).
//!
//! A message shows on its item only once the filer has left it
//! ([`FormSession::touch`]) or when the item came prefilled
//! ([`FormSession::shows_error`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use bir_app_core::{
    Draft, FieldError, FieldOption, FieldValue, FormLayout, LayoutItem, OpenReturn, OpenedReturn,
    PenaltyParty,
};
use nsh_core::prelude::*;

use crate::live::store;
use crate::screens::Route;
use crate::store::{NoticeTone, call, task};

/// How long typing pauses before the staged edits are written.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// How long edits pause before the draft is saved.
const AUTOSAVE: Duration = Duration::from_millis(700);

/// What the title says about saving.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SaveState {
    /// Nothing typed since the return opened.
    Idle,
    /// Edits wait to be saved, or the save runs.
    Saving,
    Saved,
    Failed,
}

/// An item's place in the layout: (part, item).
pub(crate) type ItemId = (usize, usize);

/// How a box is drawn, from its layout entry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BoxInfo {
    pub kind: String,
    pub computed: bool,
    pub whole: bool,
    pub item: ItemId,
}

/// A return's layout and where it was opened from; fixed while it is open.
#[derive(Debug)]
pub(crate) struct OpenForm {
    /// Which opening this is: a page built for an older one is stale.
    pub token: u64,
    pub layout: FormLayout,
    pub params: OpenReturn,
    pub boxes: HashMap<String, BoxInfo>,
}

impl OpenForm {
    fn new(token: u64, layout: FormLayout, params: OpenReturn) -> Self {
        let mut boxes = HashMap::new();
        for (pi, part) in layout.parts.iter().enumerate() {
            for (ii, item) in part.items.iter().enumerate() {
                let id = (pi, ii);
                if item.is_schedule() {
                    for row in &item.keys {
                        for (ci, key) in row.iter().enumerate() {
                            if let Some(col) = item.cols.get(ci) {
                                boxes.insert(key.clone(), BoxInfo { kind: col.kind.clone(), computed: col.computed, whole: col.whole, item: id });
                            }
                        }
                    }
                } else {
                    for field in &item.fields {
                        let info = BoxInfo { kind: field.kind.clone(), computed: field.computed, whole: field.whole, item: id };
                        if let Some(pool) = &field.pool {
                            for key in &pool.keys {
                                boxes.insert(key.clone(), info.clone());
                            }
                        }
                        boxes.insert(field.key.clone(), info);
                    }
                }
            }
        }
        Self { token, layout, params, boxes }
    }

    pub(crate) fn item(&self, id: ItemId) -> Option<&LayoutItem> {
        self.layout.parts.get(id.0)?.items.get(id.1)
    }
}

/// What the form screen shows.
#[derive(Clone)]
pub(crate) enum FormDoc {
    Closed,
    Opening { token: u64, title: String },
    Failed { token: u64, message: String },
    Open(Arc<OpenForm>),
}

impl FormDoc {
    /// Tells one opening's page from another's.
    pub(crate) fn key(&self) -> (u64, u8) {
        match self {
            FormDoc::Closed => (0, 0),
            FormDoc::Opening { token, .. } => (*token, 1),
            FormDoc::Failed { token, .. } => (*token, 2),
            FormDoc::Open(form) => (form.token, 3),
        }
    }
}

impl PartialEq for FormDoc {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FormDoc::Open(a), FormDoc::Open(b)) => Arc::ptr_eq(a, b),
            (FormDoc::Opening { token: a, title: x }, FormDoc::Opening { token: b, title: y }) => a == b && x == y,
            (FormDoc::Failed { token: a, message: x }, FormDoc::Failed { token: b, message: y }) => a == b && x == y,
            (FormDoc::Closed, FormDoc::Closed) => true,
            _ => false,
        }
    }
}

/// One validation message placed on an item.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Placed {
    pub item: ItemId,
    pub label: String,
    pub message: String,
}

/// What the event loop holds outside the signals.
#[derive(Default)]
struct Inner {
    draft: Draft,
    pending: BTreeMap<String, FieldValue>,
}

pub(crate) struct FormSession {
    pub doc: Signal<FormDoc>,
    /// Every box, key -> text (amounts as decimal text), as last
    /// answered. Not reactive: views read a box through its own signal
    /// ([`FormSession::value`]), and an answer sets only the boxes whose
    /// text changed ([`FormSession::set_values`]).
    values: RefCell<BTreeMap<String, String>>,
    /// One signal per box key, made when a view first reads it.
    cells: RefCell<HashMap<String, Signal<String>>>,
    pub options: Signal<BTreeMap<String, Vec<FieldOption>>>,
    pub locks: Signal<BTreeMap<String, String>>,
    pub lock_notes: Signal<BTreeMap<String, String>>,
    pub off: Signal<BTreeMap<String, String>>,
    /// Why a box stays blank or what it waits for.
    pub reasons: Signal<BTreeMap<String, String>>,
    pub profile: Signal<BTreeSet<String>>,
    pub pools: Signal<BTreeMap<String, String>>,
    pub penalties: Signal<Vec<PenaltyParty>>,
    /// Validation messages, in form order, each on its item.
    pub messages: Signal<Vec<Placed>>,
    /// Messages not tied to an item (a refused edit with no item, a failed
    /// check).
    pub form_errors: Signal<Vec<String>>,
    pub status: Signal<String>,
    pub editable: Signal<bool>,
    pub saved: Signal<bool>,
    pub dirty: Signal<bool>,
    /// What runs now (`Saving…`), for the toolbar.
    pub busy: Signal<Option<&'static str>>,
    /// A fill and check is running (the context bar's "Checking…").
    pub checking: Signal<bool>,
    pub open_parts: Signal<BTreeSet<usize>>,
    pub save_state: Signal<SaveState>,
    /// Items the filer has left (a blur, a choice): their messages show.
    pub touched: Signal<BTreeSet<ItemId>>,
    /// Items that opened with a value: their messages show from the start.
    pub prefilled: Signal<BTreeSet<ItemId>>,
    /// The box being typed into, while one is.
    pub focused: Signal<Option<String>>,
    /// The inputs on screen, by box key, to focus them from the bar.
    pub inputs: RefCell<HashMap<String, ElementRef>>,
    /// Each item's row on screen, to scroll to it.
    pub rows: RefCell<HashMap<ItemId, ElementRef>>,
    /// The scroll view of the form and its content, to scroll to a row.
    pub list: Cell<Option<(ElementRef, ElementRef)>>,
    /// An item to draw attention to for a moment (a jump from the bar).
    pub flash: Signal<Option<ItemId>>,
    /// The return's due date (`YYYY-MM-DD`), once read.
    pub due: Signal<Option<String>>,
    /// The outline sheet (≡ on the bar).
    pub outline_open: Signal<bool>,
    /// The phone's full-screen sheets: the long-list search of an item, the
    /// row editor of a schedule row, how a computed item is worked out.
    pub picker: Signal<Option<ItemId>>,
    pub row_edit: Signal<Option<(ItemId, usize)>>,
    /// The schedule page shown (a schedule item's rows), and how many empty
    /// rows "+ Add" has opened on each schedule.
    pub schedule_page: Signal<Option<ItemId>>,
    pub added_rows: Signal<BTreeMap<ItemId, usize>>,
    pub explain: Signal<Option<ItemId>>,
    /// Boxes whose last edit the form refused, with a count that changes
    /// each time: the box shows the return's value again, never the
    /// refused text.
    pub reverted: Signal<(u64, Vec<String>)>,
    inner: RefCell<Inner>,
    token: Cell<u64>,
    edit_seq: Cell<u64>,
    in_flight: Cell<bool>,
    /// Edits written since the last save.
    unsaved: Cell<bool>,
    save_seq: Cell<u64>,
}

impl FormSession {
    pub(crate) fn new() -> Self {
        Self {
            doc: signal(FormDoc::Closed),
            values: RefCell::new(BTreeMap::new()),
            cells: RefCell::new(HashMap::new()),
            options: signal(BTreeMap::new()),
            locks: signal(BTreeMap::new()),
            lock_notes: signal(BTreeMap::new()),
            off: signal(BTreeMap::new()),
            reasons: signal(BTreeMap::new()),
            profile: signal(BTreeSet::new()),
            pools: signal(BTreeMap::new()),
            penalties: signal(Vec::new()),
            messages: signal(Vec::new()),
            form_errors: signal(Vec::new()),
            status: signal(String::new()),
            editable: signal(false),
            saved: signal(false),
            dirty: signal(false),
            busy: signal(None),
            checking: signal(false),
            open_parts: signal(BTreeSet::from([0])),
            save_state: signal(SaveState::Idle),
            touched: signal(BTreeSet::new()),
            prefilled: signal(BTreeSet::new()),
            focused: signal(None),
            inputs: RefCell::new(HashMap::new()),
            rows: RefCell::new(HashMap::new()),
            list: Cell::new(None),
            flash: signal(None),
            due: signal(None),
            outline_open: signal(false),
            picker: signal(None),
            row_edit: signal(None),
            schedule_page: signal(None),
            added_rows: signal(BTreeMap::new()),
            explain: signal(None),
            reverted: signal((0, Vec::new())),
            inner: RefCell::new(Inner::default()),
            token: Cell::new(0),
            edit_seq: Cell::new(0),
            in_flight: Cell::new(false),
            unsaved: Cell::new(false),
            save_seq: Cell::new(0),
        }
    }

    /// The open form, untracked.
    pub(crate) fn form(&self) -> Option<Arc<OpenForm>> {
        match self.doc.get_untracked() {
            FormDoc::Open(form) => Some(form),
            _ => None,
        }
    }

    // ------------------------------------------------------------ reads

    /// A box's signal, made (under the root, so it outlives views and
    /// remounts) the first time a view reads the box.
    fn cell(&self, key: &str) -> Signal<String> {
        if let Some(cell) = self.cells.borrow().get(key) {
            return *cell;
        }
        let now = self.values.borrow().get(key).cloned().unwrap_or_default();
        let cell = store().owner.run(|| signal(now));
        self.cells.borrow_mut().insert(key.to_owned(), cell);
        cell
    }

    /// A box's text (tracked: only this box's changes rerun the reader).
    pub(crate) fn value(&self, key: &str) -> String {
        self.cell(key).get()
    }

    /// Take `next` as every box's text: only boxes whose text differs
    /// from the last answer notify their readers.
    fn set_values(&self, next: BTreeMap<String, String>) {
        // Collect the changed cells and release every borrow before
        // notifying: a notified view may mount boxes that read cells.
        let changed: Vec<(Signal<String>, String)> = {
            let values = self.values.borrow();
            let cells = self.cells.borrow();
            let cleared = values
                .iter()
                .filter(|(key, old)| !next.contains_key(*key) && !old.is_empty())
                .filter_map(|(key, _)| cells.get(key).map(|cell| (*cell, String::new())));
            let set = next
                .iter()
                .filter(|(key, text)| values.get(*key) != Some(*text))
                .filter_map(|(key, text)| cells.get(key).map(|cell| (*cell, text.clone())));
            cleared.chain(set).collect()
        };
        *self.values.borrow_mut() = next;
        batch(|| {
            for (cell, text) in changed {
                cell.set(text);
            }
        });
    }

    /// Merge `changes` into the boxes' texts (a check's answer).
    fn merge_values(&self, changes: BTreeMap<String, String>) {
        let mut next = self.values.borrow().clone();
        next.extend(changes);
        self.set_values(next);
    }

    pub(crate) fn pool_text(&self, key: &str) -> String {
        self.pools.with(|pools| pools.get(key).cloned().unwrap_or_default())
    }

    pub(crate) fn is_ticked(&self, key: &str) -> bool {
        matches!(self.value(key).as_str(), "true" | "1" | "Y")
    }

    /// Whether the filer can type into the box (tracked).
    pub(crate) fn is_locked(&self, key: &str, computed: bool) -> bool {
        if !self.editable.get() || computed {
            return true;
        }
        if self.off.with(|off| off.contains_key(key)) {
            return true;
        }
        let penalty = self.penalties.with(|parties| {
            parties.iter().find(|p| p.keys.iter().any(|k| k == key)).map(|p| !p.applies || !p.edited)
        });
        if let Some(locked) = penalty {
            return locked;
        }
        self.locks.with(|locks| locks.contains_key(key))
    }

    pub(crate) fn reason(&self, key: &str) -> String {
        self.reasons.with(|r| r.get(key).cloned().unwrap_or_default())
    }

    pub(crate) fn off_reason(&self, key: &str) -> Option<String> {
        self.off.with(|off| off.get(key).cloned())
    }

    // ------------------------------------------------------------ open

    /// Open (or start) the return for `params` and show it.
    pub(crate) fn open(&'static self, params: OpenReturn, title: String) {
        let token = self.token.get() + 1;
        self.token.set(token);
        let same_form = self.form().is_some_and(|f| f.params.form_code == params.form_code);
        let leaving = self.form();
        self.doc.set(FormDoc::Opening { token, title });
        store().go(Route::Form);
        task(async move {
            // What was open keeps its last edits.
            if let Some(old) = leaving {
                self.save_pending(&old).await;
            }
            // A fill still in flight writes the draft of what was open.
            self.wait_idle().await;
            let p = params.clone();
            let opened = call(move |app| {
                let layout = app.form_layout(&p.form_code)?;
                let opened = app.open_return(&p)?;
                let due = due_date(app, &p);
                Ok((layout, opened, due))
            })
            .await;
            if self.token.get() != token {
                return;
            }
            match opened {
                Ok((layout, opened, due)) => {
                    let form = Arc::new(OpenForm::new(token, layout, params));
                    if !same_form {
                        self.open_parts.set(BTreeSet::from([0]));
                    }
                    self.take_opened(&form, opened);
                    self.due.set(due);
                    self.doc.set(FormDoc::Open(form));
                }
                Err(message) => self.doc.set(FormDoc::Failed { token, message }),
            }
        });
    }

    /// Open the same return again (after a queue or a cancel).
    fn reopen(&'static self) {
        if let Some(form) = self.form() {
            let title = form.layout.title.clone();
            self.open(form.params.clone(), title);
        }
    }

    fn take_opened(&self, form: &OpenForm, opened: OpenedReturn) {
        batch(|| {
            {
                let mut inner = self.inner.borrow_mut();
                inner.draft = opened.draft.clone();
                inner.pending.clear();
            }
            self.edit_seq.set(self.edit_seq.get() + 1);
            self.save_seq.set(self.save_seq.get() + 1);
            self.unsaved.set(false);
            self.saved.set(opened.draft.0.get("id").is_some_and(|id| !id.is_null()));
            self.save_state.set(SaveState::Idle);
            self.touched.set(BTreeSet::new());
            self.prefilled.set(prefilled_items(form, &opened.values));
            self.focused.set(None);
            self.flash.set(None);
            self.outline_open.set(false);
            self.picker.set(None);
            self.row_edit.set(None);
            self.schedule_page.set(None);
            self.added_rows.set(BTreeMap::new());
            self.explain.set(None);
            self.inputs.borrow_mut().clear();
            self.rows.borrow_mut().clear();
            self.set_values(opened.values);
            self.options.set(opened.options);
            self.locks.set(opened.locks);
            self.lock_notes.set(opened.lock_notes);
            self.off.set(opened.off);
            let mut reasons = opened.waits;
            reasons.extend(opened.whys);
            self.reasons.set(reasons);
            self.profile.set(opened.profile.into_iter().collect());
            self.pools.set(opened.pools);
            self.penalties.set(opened.penalties);
            self.status.set(opened.status);
            self.editable.set(opened.editable);
            self.dirty.set(false);
            self.busy.set(None);
            self.form_errors.set(Vec::new());
            self.messages.set(place_errors(form, &opened.errors));
        });
    }

    // ------------------------------------------------------------ edits

    /// Stage an edit. `now`: write it at once (choices, a blur), else after
    /// typing pauses.
    pub(crate) fn edit(&'static self, key: String, value: FieldValue, now: bool) {
        if !self.editable.get_untracked() {
            return;
        }
        self.inner.borrow_mut().pending.insert(key, value);
        self.dirty.set(true);
        self.unsaved.set(true);
        self.save_state.set(SaveState::Saving);
        let seq = self.edit_seq.get() + 1;
        self.edit_seq.set(seq);
        if now {
            task(async move { self.flush().await });
            return;
        }
        store().owner.run(|| {
            set_timeout(DEBOUNCE, move || {
                if self.edit_seq.get() == seq {
                    task(async move { self.flush().await });
                }
            })
        });
    }

    /// A choice shows at once; the fill confirms it.
    pub(crate) fn choose(&'static self, edits: Vec<(String, String)>) {
        if !self.editable.get_untracked() {
            return;
        }
        self.merge_values(edits.iter().cloned().collect());
        let mut inner = self.inner.borrow_mut();
        for (key, value) in edits {
            inner.pending.insert(key, FieldValue::Text(value));
        }
        drop(inner);
        self.dirty.set(true);
        self.unsaved.set(true);
        self.save_state.set(SaveState::Saving);
        task(async move { self.flush().await });
    }

    /// Write the staged edits now (if any), after the fill in flight.
    pub(crate) async fn flush(&'static self) {
        self.wait_idle().await;
        let Some(form) = self.form() else { return };
        let (edits, draft) = {
            let mut inner = self.inner.borrow_mut();
            if inner.pending.is_empty() {
                return;
            }
            (std::mem::take(&mut inner.pending), inner.draft.clone())
        };
        self.in_flight.set(true);
        self.checking.set(true);
        let code = form.params.form_code.clone();
        let sent = edits.clone();
        let to_fill = edits.clone();
        let answer = call(move |app| {
            let filled = app.fill(&code, &draft, &to_fill, &[])?;
            let checked = app.validate(&code, &filled.draft)?;
            Ok((filled, checked))
        })
        .await;
        self.in_flight.set(false);
        self.checking.set(false);
        if self.token.get() != form.token {
            return;
        }
        match answer {
            Ok((filled, checked)) => batch(|| {
                self.inner.borrow_mut().draft = checked.draft;
                let mut values = filled.values;
                values.extend(checked.values);
                self.set_values(values);
                self.options.set(if checked.options.is_empty() { filled.options } else { checked.options });
                self.off.set(checked.off);
                self.penalties.set(checked.penalties);
                if !checked.pools.is_empty() {
                    self.pools.set(checked.pools);
                } else if !filled.pools.is_empty() {
                    self.pools.set(filled.pools);
                }
                if !filled.relocks.is_empty() {
                    self.locks.update(|locks| {
                        for (key, lock) in &filled.relocks {
                            match lock {
                                Some(why) => {
                                    locks.insert(key.clone(), why.clone());
                                }
                                None => {
                                    locks.remove(key);
                                }
                            }
                        }
                    });
                    self.lock_notes.update(|notes| notes.extend(filled.relock_notes.iter().filter_map(|(k, v)| v.clone().map(|v| (k.clone(), v)))));
                }
                let mut placed = place_errors(&form, &checked.errors);
                let mut loose = Vec::new();
                if !filled.rejected.is_empty() {
                    let keys: Vec<String> = filled.rejected.keys().cloned().collect();
                    for key in &keys {
                        if let Some(info) = form.boxes.get(key) {
                            self.touch(info.item);
                        }
                    }
                    self.reverted.update(|(n, list)| {
                        *n += 1;
                        *list = keys;
                    });
                }
                if !filled.profile_blank.is_empty() {
                    self.write_back_to_profile(&form, &filled.profile_blank, &sent);
                }
                for (key, why) in filled.rejected {
                    match form.boxes.get(&key) {
                        Some(info) => placed.push(Placed { item: info.item, label: item_label(&form, info.item), message: why }),
                        None => loose.push(why),
                    }
                }
                placed.sort_by_key(|p| p.item);
                self.messages.set(placed);
                self.form_errors.set(loose);
                self.autosave();
            }),
            Err(error) => {
                let mut inner = self.inner.borrow_mut();
                let newer = std::mem::take(&mut inner.pending);
                inner.pending = edits;
                inner.pending.extend(newer);
                drop(inner);
                store().notify(NoticeTone::Danger, format!("Could not apply the edit: {error}"));
            }
        }
    }

    async fn wait_idle(&self) {
        while self.in_flight.get() {
            timer(Duration::from_millis(15)).await;
        }
    }

    /// Run `f` with the current draft once every edit is written; `None`
    /// when the form changed meanwhile or something else is running.
    async fn settled(&'static self, label: &'static str) -> Option<(Arc<OpenForm>, Draft)> {
        if self.busy.get_untracked().is_some() {
            return None;
        }
        self.busy.set(Some(label));
        self.flush().await;
        self.wait_idle().await;
        let Some(form) = self.form() else {
            self.busy.set(None);
            return None;
        };
        let draft = self.inner.borrow().draft.clone();
        Some((form, draft))
    }

    // ------------------------------------------------------------ actions

    pub(crate) fn validate(&'static self) {
        task(async move {
            let Some((form, draft)) = self.settled("Checking…").await else { return };
            let code = form.params.form_code.clone();
            let answer = call(move |app| app.validate(&code, &draft)).await;
            self.busy.set(None);
            if self.token.get() != form.token {
                return;
            }
            match answer {
                Ok(checked) => {
                    let count = checked.errors.len();
                    self.inner.borrow_mut().draft = checked.draft;
                    batch(|| {
                        self.merge_values(checked.values);
                        self.off.set(checked.off);
                        self.penalties.set(checked.penalties);
                        self.messages.set(place_errors(&form, &checked.errors));
                        self.form_errors.set(Vec::new());
                    });
                    if count == 0 {
                        store().notify(NoticeTone::Success, "No problems found.");
                    } else {
                        store().notify(NoticeTone::Info, format!("{count} item{} need{} attention.", plural(count), if count == 1 { "s" } else { "" }));
                    }
                }
                Err(error) => store().notify(NoticeTone::Danger, format!("Could not check the return: {error}")),
            }
        });
    }

    /// A profile box the profile left blank was filled on this return: write
    /// it back to the branch's profile so later forms prefill it.
    fn write_back_to_profile(&'static self, form: &OpenForm, keys: &[String], sent: &BTreeMap<String, FieldValue>) {
        let fact_of = |key: &str| -> Option<&'static str> {
            let item = form.boxes.get(key).and_then(|b| form.item(b.item))?;
            let field = item.fields.iter().find(|f| f.key == key)?;
            let label = format!("{} {}", item.label, field.label).to_lowercase();
            if field.kind == "email" || label.contains("email") {
                Some("email")
            } else if label.contains("rdo") {
                Some("rdo_code")
            } else if label.contains("zip") {
                Some("zip_code")
            } else if label.contains("address") {
                Some("registered_address")
            } else if label.contains("contact") || label.contains("telephone") {
                Some("phone")
            } else {
                None
            }
        };
        let facts: Vec<(&'static str, String)> = keys
            .iter()
            .filter_map(|key| {
                let fact = fact_of(key)?;
                let value = match sent.get(key)? {
                    FieldValue::Text(text) => text.clone(),
                    _ => return None,
                };
                Some((fact, value))
            })
            .collect();
        if facts.is_empty() {
            return;
        }
        let tin = form.params.tin.clone();
        task(async move {
            let saved = call(move |app| {
                for (fact, value) in &facts {
                    app.set_branch_fact(&tin, fact, value)?;
                }
                Ok(())
            })
            .await;
            match saved {
                Ok(()) => {
                    let name = store().selected.get_untracked().map(|s| s.name).unwrap_or_else(|| "the taxpayer".to_owned());
                    store().notify(NoticeTone::Success, format!("Saved to {name}'s profile too."));
                    store().reload_taxpayers(None);
                }
                Err(error) => store().notify(NoticeTone::Danger, format!("Not saved to the profile: {error}")),
            }
        });
    }

    /// Save once edits have paused for [`AUTOSAVE`].
    fn autosave(&'static self) {
        if !self.unsaved.get() {
            return;
        }
        let seq = self.save_seq.get() + 1;
        self.save_seq.set(seq);
        store().owner.run(|| {
            set_timeout(AUTOSAVE, move || {
                if self.save_seq.get() == seq {
                    task(async move {
                        if let Some(form) = self.form() {
                            self.save_pending(&form).await;
                        }
                    });
                }
            })
        });
    }

    /// Save now (leaving the form): the staged edits are written first.
    pub(crate) fn save_now(&'static self) {
        let Some(form) = self.form() else { return };
        task(async move {
            self.flush().await;
            self.save_pending(&form).await;
        });
    }

    /// Save the draft of `form` if edits were written since the last save
    /// and it is still the open one; saves never overlap a fill.
    async fn save_pending(&'static self, form: &Arc<OpenForm>) {
        self.wait_idle().await;
        if !self.unsaved.get() || self.token.get() != form.token || !self.editable.get_untracked() {
            return;
        }
        // An action (queue, print) saves on its own.
        if self.busy.get_untracked().is_some() {
            return;
        }
        self.unsaved.set(false);
        self.in_flight.set(true);
        let draft = self.inner.borrow().draft.clone();
        let saved = self.save_draft(form, draft).await;
        self.in_flight.set(false);
        if self.token.get() != form.token {
            return;
        }
        if !saved {
            self.unsaved.set(true);
            self.save_state.set(SaveState::Failed);
        } else if !self.unsaved.get() && self.inner.borrow().pending.is_empty() {
            self.save_state.set(SaveState::Saved);
        }
    }

    /// Save `draft` and take the answer; whether it saved.
    async fn save_draft(&'static self, form: &Arc<OpenForm>, draft: Draft) -> bool {
        let code = form.params.form_code.clone();
        let answer = call(move |app| {
            let saved = app.save(&code, &draft)?;
            let draft = saved.draft.clone().unwrap_or(draft);
            let checked = app.validate(&code, &draft)?;
            Ok((saved, checked))
        })
        .await;
        if self.token.get() != form.token {
            return false;
        }
        match answer {
            Ok((saved, checked)) => {
                self.inner.borrow_mut().draft = checked.draft;
                batch(|| {
                    if let Some(status) = saved.status {
                        self.status.set(status);
                    }
                    self.saved.set(true);
                    self.dirty.set(!self.inner.borrow().pending.is_empty());
                    self.merge_values(checked.values);
                    self.messages.set(place_errors(form, &checked.errors));
                });
                store().reload_returns();
                true
            }
            Err(error) => {
                store().notify(NoticeTone::Danger, format!("Not saved: {error}"));
                false
            }
        }
    }

    /// Saved and unchanged, saving first when it is not.
    async fn ensure_saved(&'static self, form: &Arc<OpenForm>, draft: Draft) -> Option<Draft> {
        if self.saved.get_untracked() && !self.dirty.get_untracked() {
            return Some(draft);
        }
        self.unsaved.set(false);
        if !self.save_draft(form, draft).await {
            self.unsaved.set(true);
            return None;
        }
        self.save_state.set(SaveState::Saved);
        Some(self.inner.borrow().draft.clone())
    }

    pub(crate) fn queue(&'static self) {
        task(async move {
            let Some((form, draft)) = self.settled("Queueing…").await else { return };
            let Some(draft) = self.ensure_saved(&form, draft).await else {
                self.busy.set(None);
                return;
            };
            let code = form.params.form_code.clone();
            let answer = call(move |app| app.queue_return(&code, &draft)).await;
            self.busy.set(None);
            match answer {
                Ok(_) => {
                    store().notify(NoticeTone::Success, "Queued for sending.");
                    store().reload_returns();
                    self.reopen();
                }
                Err(why) => store().notify(NoticeTone::Danger, format!("Not queued: {why}")),
            }
        });
    }

    pub(crate) fn cancel_queue(&'static self) {
        task(async move {
            let Some((form, draft)) = self.settled("Cancelling…").await else { return };
            let code = form.params.form_code.clone();
            let answer = call(move |app| app.cancel_queued(&code, &draft)).await;
            self.busy.set(None);
            match answer {
                Ok(()) => {
                    store().notify(NoticeTone::Info, "Taken off the queue: an editable draft again.");
                    store().reload_returns();
                    self.reopen();
                }
                Err(why) => store().notify(NoticeTone::Danger, format!("Still queued: {why}")),
            }
        });
    }

    /// Fill the print sheet, write it to a temp file and open it in the
    /// system browser (P4b replaces this with an in-app preview).
    pub(crate) fn print(&'static self) {
        task(async move {
            let Some((form, draft)) = self.settled("Printing…").await else { return };
            let draft = if self.editable.get_untracked() {
                match self.ensure_saved(&form, draft).await {
                    Some(draft) => draft,
                    None => {
                        self.busy.set(None);
                        return;
                    }
                }
            } else {
                draft
            };
            let code = form.params.form_code.clone();
            let answer = call(move |app| {
                let printed = app.print_html(&code, &draft)?;
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let path = std::env::temp_dir().join(format!("buwiz-{code}-{stamp}.html"));
                std::fs::write(&path, printed.html)
                    .map_err(|error| bir_app_core::AppError::Call(format!("could not write {}: {error}", path.display())))?;
                open_in_browser(&path)
                    .map_err(|error| bir_app_core::AppError::Call(format!("could not open {}: {error}", path.display())))?;
                Ok(path)
            })
            .await;
            self.busy.set(None);
            match answer {
                Ok(_) => store().notify(NoticeTone::Info, "The print sheet opened in your browser."),
                Err(error) => store().notify(NoticeTone::Danger, format!("Could not print: {error}")),
            }
        });
    }

    /// The filer left `item`: its messages show from now on.
    pub(crate) fn touch(&self, item: ItemId) {
        if !self.touched.with_untracked(|t| t.contains(&item)) {
            self.touched.update(|t| {
                t.insert(item);
            });
        }
    }

    /// The filer left the box `key` (its item's messages show).
    pub(crate) fn touch_key(&self, key: &str) {
        if let Some(item) = self.form().and_then(|f| f.boxes.get(key).map(|b| b.item)) {
            self.touch(item);
        }
    }

    /// Whether `item`'s messages show (tracked): left once, or prefilled.
    pub(crate) fn shows_error(&self, item: ItemId) -> bool {
        self.touched.with(|t| t.contains(&item)) || self.prefilled.with(|p| p.contains(&item))
    }

    pub(crate) fn toggle_part(&self, part: usize) {
        self.open_parts.update(|open| {
            if !open.remove(&part) {
                open.insert(part);
            }
        });
    }
}

#[cfg(target_os = "macos")]
fn open_in_browser(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("open").arg(path).status().map(drop)
}

#[cfg(not(target_os = "macos"))]
fn open_in_browser(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(path).status().map(drop)
}

/// The items that opened with something typed in (not a zero, a blank or
/// an unticked box).
fn prefilled_items(form: &OpenForm, values: &BTreeMap<String, String>) -> BTreeSet<ItemId> {
    form.boxes
        .iter()
        .filter(|(_, info)| !info.computed)
        .filter(|(key, _)| values.get(*key).is_some_and(|v| !is_blank_value(v)))
        .map(|(_, info)| info.item)
        .collect()
}

/// A value that is nothing typed: blank, a zero amount, an unticked box.
pub(crate) fn is_blank_value(value: &str) -> bool {
    let v = value.trim();
    v.is_empty() || matches!(v, "false" | "0" | "0.00" | "0.0" | "N")
}

/// The due date of the return `open` names (`YYYY-MM-DD`), if the
/// calendar has it.
fn due_date(app: &bir_app_core::BirApp, open: &OpenReturn) -> Option<String> {
    let from = format!("{}-01-01", open.year);
    let to = format!("{}-12-31", open.year + 1);
    app.deadlines_between(&from, &to)
        .ok()?
        .into_iter()
        .find(|d| d.tin == open.tin && d.form_code == open.form_code && d.taxable_year == open.year && d.period == open.period)
        .map(|d| d.due_date)
}

pub(crate) fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub(crate) fn item_label(form: &OpenForm, id: ItemId) -> String {
    match form.item(id) {
        Some(item) if !item.no.is_empty() => format!("Item {} · {}", item.no, item.label),
        Some(item) => item.label.clone(),
        None => String::new(),
    }
}

/// Put each message on an item: by its keys, else by an `Item N` in its
/// text, else by the item whose label shares most words with it, else the
/// first item.
fn place_errors(form: &OpenForm, errors: &[FieldError]) -> Vec<Placed> {
    let items: Vec<(ItemId, &LayoutItem)> = form
        .layout
        .parts
        .iter()
        .enumerate()
        .flat_map(|(pi, part)| part.items.iter().enumerate().map(move |(ii, item)| ((pi, ii), item)))
        .collect();
    let words = |text: &str| -> BTreeSet<String> {
        text.split(|c: char| !c.is_ascii_alphabetic())
            .filter(|w| w.len() >= 4)
            .map(str::to_ascii_lowercase)
            .collect()
    };
    let mut placed: Vec<Placed> = errors
        .iter()
        .filter_map(|error| {
            let by_key = error.keys.iter().find_map(|k| form.boxes.get(k).map(|b| b.item));
            let by_number = || {
                let lower = error.message.to_ascii_lowercase();
                let at = lower.find("item ")?;
                let no: String = error.message[at + 5..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                let no = no.trim_start_matches('0').to_ascii_uppercase();
                items
                    .iter()
                    .find(|(_, item)| item.no.trim_start_matches('0').eq_ignore_ascii_case(&no))
                    .map(|(id, _)| *id)
            };
            let by_words = || {
                let want = words(&format!("{} {}", error.field, error.message));
                items
                    .iter()
                    .map(|(id, item)| (words(&item.label).intersection(&want).count(), *id))
                    .filter(|(n, _)| *n > 0)
                    .max_by_key(|(n, id)| (*n, std::cmp::Reverse(*id)))
                    .map(|(_, id)| id)
            };
            let id = by_key.or_else(by_number).or_else(by_words).or_else(|| items.first().map(|(id, _)| *id))?;
            Some(Placed { item: id, label: item_label(form, id), message: error.message.clone() })
        })
        .collect();
    placed.sort_by_key(|p| p.item);
    placed
}
