//! The typed facade the UI calls: [`BirApp`].
//!
//! Each method is a typed wrapper over one of the ported JSON-shaped
//! functions (`crate::host::HostModule`). Inputs and outputs are serde
//! structs; amounts are decimal strings (the field map's text), never `f64`.
//! A draft is an opaque [`Draft`]: hand back what a call returned.
//!
//! Every method that takes `&self` reads or writes the database: call them
//! from a worker thread, not the UI thread. [`BirApp::form_catalog`] reads
//! only compiled-in data.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bir_core::background_cron::SubmissionTransport;
use bir_core::db::Database;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::database::{self, DatabaseHandle, json, to_json};
use crate::host::{HostArguments, HostError, HostModule};

/// Why a call failed.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The database could not be opened.
    #[error("database: {0}")]
    Database(#[from] bir_core::db::DbError),
    /// The call was refused or failed; the message is for the filer.
    #[error("{0}")]
    Call(String),
    /// A result did not have the shape its typed method expects.
    #[error("unexpected result from `{function}`: {message}")]
    Shape { function: String, message: String },
}

impl From<HostError> for AppError {
    fn from(error: HostError) -> Self {
        AppError::Call(error.message().to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// A return's working draft, exactly as the form engine holds it. Opaque to
/// the UI: read amounts from `values`, write them with [`BirApp::fill`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Draft(pub Value);

/// One form of the library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormCatalogEntry {
    /// The BIR form code, e.g. `1701Q`.
    #[serde(rename = "c")]
    pub code: String,
    #[serde(rename = "t")]
    pub title: String,
    #[serde(rename = "rev", default)]
    pub revision: Option<String>,
    /// `Monthly`, `Quarterly`, `Annual` or `As needed` (as the schema says).
    #[serde(rename = "freq", default)]
    pub frequency: Option<String>,
    #[serde(default)]
    pub pages: Option<u32>,
}

/// One validation message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
    #[serde(default)]
    pub keys: Vec<String>,
    /// A notice the filer may mark checked: the token for
    /// `screen_form_check_notice`, and its button label.
    #[serde(default)]
    pub dismiss: Option<String>,
    #[serde(default)]
    pub dismiss_label: Option<String>,
}

/// One party's Surcharge, Interest and Compromise boxes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PenaltyParty {
    pub keys: Vec<String>,
    pub edited: bool,
    #[serde(default)]
    pub kept: Option<bool>,
    pub applies: bool,
}

impl PenaltyParty {
    /// Surcharge + interest + compromise, from the return's values, as
    /// decimal text (`1,234.50`).
    pub fn total(&self, values: &BTreeMap<String, String>) -> String {
        crate::display::sum_amounts(
            self.keys
                .iter()
                .map(|key| values.get(key).map_or("", String::as_str)),
        )
    }
}

/// A choice of a drop-down box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub selected: Option<bool>,
}

/// A return opened for editing or viewing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenedReturn {
    pub form_code: String,
    pub year: i64,
    pub period: i64,
    /// `Draft`, `Queued`, `Sent`, `Confirmed` or `Paid`.
    pub status: String,
    pub editable: bool,
    /// The period column the draft is stored under.
    pub stored_period: Option<i64>,
    pub draft: Draft,
    /// Every box, official key -> text (amounts as decimal text).
    pub values: BTreeMap<String, String>,
    #[serde(default)]
    pub options: BTreeMap<String, Vec<FieldOption>>,
    #[serde(default)]
    pub locks: BTreeMap<String, String>,
    /// A Form library sample (blank until its period is set).
    #[serde(default)]
    pub sample: bool,
    #[serde(default)]
    pub penalties: Vec<PenaltyParty>,
    #[serde(default)]
    pub errors: Vec<FieldError>,
    /// Boxes switched off by another answer, key -> what they need
    /// (`Needs Civil Status = Married`).
    #[serde(default)]
    pub off: BTreeMap<String, String>,
    /// Why a locked box cannot be typed into, in the filer's words.
    #[serde(default)]
    pub lock_notes: BTreeMap<String, String>,
    /// Why a box stays blank (`whys`), or what it waits for (`waits`).
    #[serde(default)]
    pub whys: BTreeMap<String, String>,
    #[serde(default)]
    pub waits: BTreeMap<String, String>,
    /// Boxes filled from the tax profile.
    #[serde(default)]
    pub profile: Vec<String>,
    /// A pooled text (an address spread over boxes), first key -> text.
    #[serde(default)]
    pub pools: BTreeMap<String, String>,
    #[serde(default)]
    pub submitted_at: Option<String>,
    /// Everything else the old page read (`blank`, `needs`, `notes`, ...),
    /// untyped for now.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What one fill changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilledReturn {
    pub draft: Draft,
    pub values: BTreeMap<String, String>,
    #[serde(default)]
    pub options: BTreeMap<String, Vec<FieldOption>>,
    /// Edits the form refused, key -> why.
    #[serde(default)]
    pub rejected: BTreeMap<String, String>,
    /// Profile boxes the profile leaves blank that this edit filled on the
    /// return: the app may write them back to the taxpayer profile.
    #[serde(default)]
    pub profile_blank: Vec<String>,
    #[serde(default)]
    pub penalties: Vec<PenaltyParty>,
    #[serde(default)]
    pub pools: BTreeMap<String, String>,
    /// Boxes this edit locked (`Some(why)`) or opened (`None`).
    #[serde(default)]
    pub relocks: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub relock_notes: BTreeMap<String, Option<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A validated return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidatedReturn {
    pub draft: Draft,
    pub values: BTreeMap<String, String>,
    #[serde(default)]
    pub errors: Vec<FieldError>,
    #[serde(default)]
    pub off: BTreeMap<String, String>,
    #[serde(default)]
    pub options: BTreeMap<String, Vec<FieldOption>>,
    #[serde(default)]
    pub penalties: Vec<PenaltyParty>,
    #[serde(default)]
    pub pools: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What a save did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedReturn {
    /// False when a sample's period already has a stored return (nothing
    /// saved; see `extra.existing` / `extra.opens`).
    #[serde(default)]
    pub saved: Option<bool>,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub stored_period: Option<i64>,
    #[serde(default)]
    pub draft: Option<Draft>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Which stored return to open (or start).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenReturn {
    pub tin: String,
    pub form_code: String,
    pub year: i64,
    /// Month 1-12, quarter 1-4, `0` for an annual return, else the
    /// as-needed key.
    pub period: i64,
    /// The exact stored return (a transaction date's MMDD, ...).
    #[serde(default)]
    pub key: Option<i64>,
}

/// One edit to a box: text, a tick, or a cleared box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    Text(String),
    Tick(bool),
    Clear,
}

impl From<&str> for FieldValue {
    fn from(text: &str) -> Self {
        FieldValue::Text(text.to_string())
    }
}

/// A saved draft row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftSummary {
    pub id: i64,
    pub tin: String,
    pub form_code: String,
    pub taxable_year: i64,
    pub quarter: Option<i64>,
    pub month: Option<i64>,
    pub status: String,
    pub updated_at: String,
}

/// A return due in a date range, per branch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeadlineRow {
    pub tin: String,
    pub taxpayer_name: String,
    pub branch_name: String,
    pub form_code: String,
    pub form_name: String,
    pub frequency: String,
    pub taxable_year: i64,
    pub period: i64,
    pub period_label: String,
    /// `YYYY-MM-DD`.
    pub due_date: String,
    /// `Not started`, `Draft`, `Queued`, `Sent`, `Confirmed` or `Paid`.
    pub status: String,
    pub draft_id: Option<i64>,
    pub return_key: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A saved or filed return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReturnRow {
    pub id: i64,
    pub tin: String,
    pub taxpayer_name: String,
    pub branch_name: String,
    pub form_code: String,
    pub taxable_year: i64,
    pub period: i64,
    pub period_label: String,
    pub status: String,
    pub updated_at: String,
    pub return_key: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Which returns [`BirApp::list_returns`] lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnFilter {
    Saved,
    Filed,
    All,
}

impl ReturnFilter {
    fn as_str(self) -> &'static str {
        match self {
            ReturnFilter::Saved => "saved",
            ReturnFilter::Filed => "filed",
            ReturnFilter::All => "all",
        }
    }
}

/// An as-needed form of a branch for a year.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AsNeededForm {
    pub tin: String,
    pub taxpayer_name: String,
    pub form_code: String,
    pub form_name: String,
    pub taxable_year: i64,
    pub trigger: Option<String>,
    pub due_rule: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The home screen: what is due soon, the as-needed forms, recent returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HomeData {
    pub due: Vec<DeadlineRow>,
    pub as_needed: Vec<AsNeededForm>,
    pub recent: Vec<ReturnRow>,
}

/// A branch's facts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BranchFacts {
    #[serde(default)]
    pub rdo_code: String,
    #[serde(default)]
    pub registered_address: String,
    #[serde(default)]
    pub zip_code: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
}

/// One branch of a taxpayer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub tin: String,
    pub tin_display: String,
    pub tin_root: String,
    pub code: String,
    pub name: String,
    pub is_head_office: bool,
    pub facts: BranchFacts,
    #[serde(default)]
    pub inherited: Vec<String>,
}

/// A taxpayer and its branches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Taxpayer {
    pub tin_root: String,
    pub tin_display: String,
    pub name: String,
    /// `Individual`, `Corporation`, `Partnership`, `Cooperative`, `Estate`
    /// or `Trust`.
    pub taxpayer_type: String,
    pub birth_date: Option<String>,
    #[serde(default)]
    pub needs_grouping_review: bool,
    #[serde(default)]
    pub archived: bool,
    pub branches: Vec<Branch>,
}

/// A new taxpayer with its head office.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewTaxpayer {
    /// The 9-digit TIN root.
    pub tin_root: String,
    pub name: String,
    pub taxpayer_type: String,
    #[serde(default)]
    pub birth_date: Option<String>,
    pub head_office: BranchFacts,
}

/// Changes to a taxpayer; `None` keeps the stored value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TaxpayerUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taxpayer_type: Option<String>,
}

/// A place in [`BirApp::places`]: a ZIP area and its RDO, or an RDO alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub label: String,
    pub city: String,
    pub zip: String,
    pub rdo_code: String,
    pub rdo_name: String,
}

/// One return in the submission queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuedReturn {
    pub id: i64,
    pub tin: String,
    pub form_code: String,
    pub taxable_year: i64,
    pub quarter: Option<i64>,
    pub month: Option<i64>,
    pub status: String,
    pub updated_at: String,
}

/// A filled print sheet, as static HTML for the print preview.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrintedReturn {
    pub html: String,
    pub title: String,
}

/// The app's door into bir-core: owns the database handle and the table of
/// ported functions.
pub struct BirApp {
    db: DatabaseHandle,
    path: PathBuf,
    module: HostModule,
}

impl BirApp {
    /// Open the app's live database (`BIR_DATABASE_PATH`, else the platform
    /// data dir), holding its owner lock. The key comes from `BIR_DB_KEY` /
    /// `BIR_DB_KEY_FILE`, else the OS keychain.
    pub fn open_live() -> AppResult<Self> {
        let (database, path, _recovered) = bir_core::db::open_live_database()?;
        Ok(Self::with_database(database, path))
    }

    /// Open (creating if missing) the database file at `path`, without the
    /// live owner lock. Same key lookup as [`Self::open_live`].
    pub fn open_at(path: impl AsRef<Path>) -> AppResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let database = Database::open(&path)?;
        if let Err(error) = database.seed_official_holidays() {
            tracing::warn!(%error, "official holidays not seeded");
        }
        Ok(Self::with_database(database, path))
    }

    fn with_database(database: Database, path: PathBuf) -> Self {
        let db = Arc::new(Mutex::new(database));
        database::install(db.clone(), path.clone());
        Self {
            db,
            path,
            module: crate::bir_module::module(),
        }
    }

    /// The database file this app opened.
    pub fn database_path(&self) -> &Path {
        &self.path
    }

    /// Start the submission worker on its own thread: queued returns go to
    /// `transport` (plaintext IAF XML), with retries, until the process ends.
    pub fn start_queue_worker<T: SubmissionTransport + 'static>(
        &self,
        transport: Arc<T>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        let db = self.db.clone();
        std::thread::Builder::new()
            .name("bir-submission-worker".into())
            .spawn(move || match tokio::runtime::Runtime::new() {
                Ok(runtime) => {
                    runtime.block_on(bir_core::background_cron::start_cron_jobs(db, transport))
                }
                Err(error) => tracing::error!("submission worker not started: {error}"),
            })
    }

    // ------------------------------------------------------- untyped door

    /// Every ported function's name.
    pub fn function_names(&self) -> Vec<&str> {
        self.module.function_names().collect()
    }

    /// Call a ported function by name with JSON arguments; its JSON result.
    /// The typed methods below cover the common ones.
    pub fn call(&self, function: &str, arguments: Vec<Value>) -> AppResult<Value> {
        let arguments = HostArguments::new(arguments.into_iter().map(json));
        Ok(to_json(&self.module.call(function, &arguments)?))
    }

    fn call_as<T: DeserializeOwned>(&self, function: &str, arguments: Vec<Value>) -> AppResult<T> {
        let value = self.call(function, arguments)?;
        serde_json::from_value(value).map_err(|error| AppError::Shape {
            function: function.to_string(),
            message: error.to_string(),
        })
    }

    // ------------------------------------------------------------- forms

    /// The forms library: every on-screen form, in schema order. Compiled-in
    /// data, no database; the first call parses the schema (slow in debug
    /// builds), so call it off the UI thread.
    pub fn form_catalog() -> AppResult<Vec<FormCatalogEntry>> {
        let index = crate::screen_form_module::forms_index()?;
        serde_json::from_value(to_json(&index)).map_err(|error| AppError::Shape {
            function: "screen_forms_index".into(),
            message: error.to_string(),
        })
    }

    /// One form's on-screen schema (parts, items, boxes), as JSON.
    pub fn form_schema(&self, form_code: &str) -> AppResult<Value> {
        self.call("screen_form_schema", vec![form_code.into()])
    }

    /// One form's on-screen layout, typed, without its hidden boxes.
    /// Compiled-in data; the first read parses the schema, so call it off
    /// the UI thread.
    pub fn form_layout(&self, form_code: &str) -> AppResult<crate::FormLayout> {
        let schema = self.form_schema(form_code)?;
        serde_json::from_value::<crate::FormLayout>(schema)
            .map(crate::FormLayout::without_hidden)
            .map_err(|error| AppError::Shape {
                function: "screen_form_schema".into(),
                message: error.to_string(),
            })
    }

    /// A blank Form library sample of `form_code` for `tin`; it becomes a
    /// return once its period is set and it is saved.
    pub fn sample_return(&self, tin: &str, form_code: &str) -> AppResult<OpenedReturn> {
        self.call_as("screen_form_sample", vec![tin.into(), form_code.into()])
    }

    /// Open the stored return for the period, or start a new one from the
    /// taxpayer's profile when none is stored.
    pub fn open_return(&self, open: &OpenReturn) -> AppResult<OpenedReturn> {
        self.call_as(
            "screen_form_open",
            vec![
                open.tin.as_str().into(),
                open.form_code.as_str().into(),
                open.year.into(),
                open.period.into(),
                open.key.map_or(Value::Null, Value::from),
            ],
        )
    }

    /// Write boxes; answers the recomputed values, refused edits and
    /// penalties. Nothing is saved. `unlocked`: profile boxes opened with
    /// Edit.
    pub fn fill(
        &self,
        form_code: &str,
        draft: &Draft,
        edits: &BTreeMap<String, FieldValue>,
        unlocked: &[String],
    ) -> AppResult<FilledReturn> {
        self.call_as(
            "screen_form_fill",
            vec![
                form_code.into(),
                draft.0.clone(),
                serde_json::to_value(edits).unwrap_or_default(),
                serde_json::to_value(unlocked).unwrap_or_default(),
            ],
        )
    }

    /// Run the form's computations and checks.
    pub fn validate(&self, form_code: &str, draft: &Draft) -> AppResult<ValidatedReturn> {
        self.call_as(
            "screen_form_validate",
            vec![form_code.into(), draft.0.clone()],
        )
    }

    /// Save the draft.
    pub fn save(&self, form_code: &str, draft: &Draft) -> AppResult<SavedReturn> {
        self.call_as("screen_form_save", vec![form_code.into(), draft.0.clone()])
    }

    /// Saved drafts of a branch for a year.
    pub fn list_drafts(&self, tin: &str, year: i64) -> AppResult<Vec<DraftSummary>> {
        self.call_as("list_drafts", vec![tin.into(), year.into()])
    }

    // --------------------------------------------------- calendar & home

    /// Every branch's returns due between two dates (`YYYY-MM-DD`,
    /// inclusive).
    pub fn deadlines_between(&self, from: &str, to: &str) -> AppResult<Vec<DeadlineRow>> {
        self.call_as("deadlines_between", vec![from.into(), to.into()])
    }

    /// Saved and/or filed returns across every branch, newest first.
    pub fn list_returns(&self, filter: ReturnFilter) -> AppResult<Vec<ReturnRow>> {
        self.call_as("list_returns", vec![filter.as_str().into()])
    }

    /// Every branch's as-needed forms for `year`.
    pub fn as_needed_forms(&self, year: i64) -> AppResult<Vec<AsNeededForm>> {
        self.call_as("as_needed_forms", vec![year.into()])
    }

    /// The home screen for `today` (`YYYY-MM-DD`): returns due in the next
    /// 30 days (and overdue ones of the past 90), this year's as-needed
    /// forms, and the 10 most recent returns.
    pub fn home(&self, today: chrono::NaiveDate) -> AppResult<HomeData> {
        use chrono::Datelike as _;
        let from = today - chrono::Duration::days(90);
        let to = today + chrono::Duration::days(30);
        let mut recent = self.list_returns(ReturnFilter::All)?;
        recent.truncate(10);
        Ok(HomeData {
            due: self.deadlines_between(&from.to_string(), &to.to_string())?,
            as_needed: self.as_needed_forms(i64::from(today.year()))?,
            recent,
        })
    }

    // ---------------------------------------------------------- taxpayers

    pub fn list_taxpayers(&self) -> AppResult<Vec<Taxpayer>> {
        self.call_as("list_taxpayers", vec![])
    }

    pub fn get_taxpayer(&self, tin_root: &str) -> AppResult<Option<Taxpayer>> {
        self.call_as("get_taxpayer", vec![tin_root.into()])
    }

    pub fn create_taxpayer(&self, taxpayer: &NewTaxpayer) -> AppResult<Taxpayer> {
        let input = serde_json::to_value(taxpayer).unwrap_or_default();
        self.call_as("create_taxpayer", vec![input])
    }

    pub fn update_taxpayer(&self, tin_root: &str, update: &TaxpayerUpdate) -> AppResult<Taxpayer> {
        let input = serde_json::to_value(update).unwrap_or_default();
        self.call_as("update_taxpayer", vec![tin_root.into(), input])
    }

    /// Every place a taxpayer registers in, for one search: each ZIP code
    /// with its area and the RDO whose name matches it (when one does),
    /// then each RDO on its own.
    pub fn places() -> Vec<Place> {
        let rdos = bir_core::reference::get_all_rdos();
        let mut out: Vec<Place> = Vec::new();
        for entry in bir_core::reference::get_all_zipcodes() {
            let Some((zip, area)) = entry.split_once(" - ") else { continue };
            // The RDO whose name shares a place word with the ZIP area
            // (`Pasig CPO` and `East Pasig`): a word of four letters or more
            // that is not "city" or a post-office suffix.
            let words = |text: &str| -> Vec<String> {
                text.to_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| w.len() >= 4 && !["city", "town", "south", "north", "east", "west", "proper"].contains(w))
                    .map(str::to_owned)
                    .collect()
            };
            let area_words = words(area);
            let rdo = rdos.iter().find(|r| words(&r.description).iter().any(|w| area_words.contains(w)));
            out.push(Place {
                label: format!("{area} · {zip}"),
                city: area.trim().to_owned(),
                zip: zip.trim().to_owned(),
                rdo_code: rdo.map(|r| r.code.clone()).unwrap_or_default(),
                rdo_name: rdo.map(|r| r.description.clone()).unwrap_or_default(),
            });
        }
        for r in rdos {
            out.push(Place {
                label: format!("RDO {} · {}", r.code, r.description),
                city: r.description.split(',').next().unwrap_or("").trim().to_owned(),
                zip: String::new(),
                rdo_code: r.code,
                rdo_name: r.description,
            });
        }
        out
    }

    /// Write `value` into one fact of a branch (`email`, `rdo_code`,
    /// `registered_address`, `zip_code`, `phone`): a profile box the filer
    /// filled on a return ([`FilledReturn::profile_blank`]) goes back to the
    /// profile so later forms prefill it.
    pub fn set_branch_fact(&self, tin: &str, fact: &str, value: &str) -> AppResult<()> {
        let input = serde_json::json!({ "fields": { fact: value } });
        self.call("update_branch", vec![tin.into(), input]).map(drop)
    }

    // -------------------------------------------------------------- queue

    /// Queue a saved, valid return for submission; answers its draft id.
    pub fn queue_return(&self, form_code: &str, draft: &Draft) -> AppResult<i64> {
        match monthly_or_quarterly_slot(form_code, draft) {
            Some((tin, year, slot)) => self.call_as(
                "queue_form_draft",
                vec![tin.into(), form_code.into(), year.into(), slot.into()],
            ),
            None => self.call_as("screen_form_queue", vec![form_code.into(), draft.0.clone()]),
        }
    }

    /// Take a queued return back to Draft (unless the worker already took it).
    pub fn cancel_queued(&self, form_code: &str, draft: &Draft) -> AppResult<()> {
        match monthly_or_quarterly_slot(form_code, draft) {
            Some((tin, year, slot)) => self.call(
                "cancel_queued",
                vec![tin.into(), form_code.into(), year.into(), slot.into()],
            ),
            None => self.call(
                "screen_form_cancel_queue",
                vec![form_code.into(), draft.0.clone()],
            ),
        }
        .map(drop)
    }

    /// Every queued return across taxpayers.
    pub fn list_queue(&self) -> AppResult<Vec<QueuedReturn>> {
        self.call_as("list_queued", vec![])
    }

    // -------------------------------------------------------------- print

    /// The return's filled print sheet as static HTML (save it first; a
    /// Form library sample prints with `screen_form_print_sample`).
    pub fn print_html(&self, form_code: &str, draft: &Draft) -> AppResult<PrintedReturn> {
        self.call_as("screen_form_print", vec![form_code.into(), draft.0.clone()])
    }
}

/// 1601C and 2551Q queue through their own typed path, keyed by TIN, year
/// and month or quarter; every other form queues from its draft.
fn monthly_or_quarterly_slot(form_code: &str, draft: &Draft) -> Option<(String, i64, i64)> {
    let slot_key = match form_code {
        "1601C" => "month",
        "2551Q" => "quarter",
        _ => return None,
    };
    let tin = draft
        .0
        .get("tin")
        .and_then(Value::as_str)
        .map(str::to_string)?;
    let year = draft.0.get("taxable_year").and_then(Value::as_i64)?;
    let slot = draft.0.get(slot_key).and_then(Value::as_i64)?;
    Some((tin, year, slot))
}
