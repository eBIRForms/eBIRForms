//! Home, Calendar and Filings feeds for the redesigned shell: deadlines for
//! the forms each branch files, and the saved and filed returns across every
//! taxpayer. Read-only; nothing here writes.
//!
//! Deadlines come from `calendar_rules::DeadlineResolver::deadlines_for_forms`
//! over each branch's yearly forms (the per-year Forms Set), with the user's
//! deadline overrides applied, exactly as the painted dashboard resolves them.
//!
//! As-needed returns (decision D8c, 2026-10-18) have no period deadline. A
//! saved one that holds its trigger date (a removal, a withholding, a
//! transaction) is a dated row too, due per its form's rule
//! (`calendar_rules::EVENT_DUE_RULES`); the as-needed forms a branch files
//! in a year, with their saved returns that hold no date yet, are listed by
//! `as_needed_forms` for Home's "As needed" list.

use crate::host::{HostArguments, HostError, HostModule, HostResult};
use bir_core::calendar_rules::{
    BusinessDayCalendar, DeadlineOverride, DeadlinePeriod, DeadlineResolver, DeadlineStatus,
    ResolvedTaxDeadline, event_due_rule,
};
use bir_core::db::Database;
use bir_core::forms::{FilingStatus, FormDraftSummary};
use chrono::NaiveDate;
use serde_json::{Value, json};

use crate::database::{serialize, with_database};

/// The JSON shapes of the functions below, in TypeScript notation (kept from
/// the old JS UI as the spec for typing them).
#[allow(dead_code)]
pub const DECLARATIONS: &str = r#"
export interface DeadlineRow {
    tin: string;
    tin_root: string;
    branch_code: string;
    branch_name: string;
    taxpayer_name: string;
    branch_count: number;
    form_code: string;
    form_name: string;
    frequency: "Monthly" | "Quarterly" | "Annual" | "As needed";
    taxable_year: number;
    /** Month 1-12, quarter 1-4, 0 for an annual return, or an as-needed return's stored period. */
    period: number;
    period_label: string;
    due_date: string;
    /** "Not started" | "Draft" | "Queued" | "Sent" | "Confirmed" | "Paid" */
    status: string;
    draft_id: number | null;
    /** The saved return's stored key (`screen_form_open`'s `key`), null when none is saved. */
    return_key: number | null;
    /** The rule's own date, before weekends, holidays and deadline overrides moved it. */
    original_due_date: string;
    /** Why `due_date` differs from `original_due_date`: "moved" is a deadline override (a person's or BIR's). */
    deadline_status: "normal" | "weekend" | "holiday" | "non_working" | "moved";
    /** The source a moved deadline cites (the reason given), else null. */
    moved_reason: string | null;
    /** The period as a deadline override names it ("2026-Q3", "2026-09", "2025"); null for an as-needed row. */
    period_key: string | null;
}
export function deadlines_between(from: string, to: string): DeadlineRow[];
export interface DeadlineOverrideEntry {
    id: string;
    /** "holiday": an official holiday the app ships; "moved": a deadline a person (or BIR) moved. */
    kind: "holiday" | "moved";
    title: string;
    reason: string;
    forms: string[];
    original_deadline: string;
    new_deadline: string;
    /** The deadlines it moves (none listed for a holiday). */
    periods: { form: string; period: string; label: string }[];
}
export interface DeadlineOverrideList {
    year: number | null;
    overrides: DeadlineOverrideEntry[];
    damaged: number;
    unreadable: boolean;
}
/** Every stored deadline override; they apply to every taxpayer. `args`: `{year?}`. */
export function deadline_overrides(args: { year?: number }): DeadlineOverrideList;
/** Move one form's deadline for one period, for every taxpayer (`bir_deadline_override_set`). */
export function deadline_override_set(args: { form: string; period?: string; original_deadline?: string; new_deadline: string; reason?: string }): {
    form: string; period: string; label: string; rule_deadline: string; was: string | null; due: string | null; replaced: boolean; override: DeadlineOverrideEntry; applies_to: string;
};
/** Put a deadline back: `{id}`, or `{form, period}` / `{form, original_deadline}` (`bir_deadline_override_remove`). */
export function deadline_override_remove(args: { id?: string; form?: string; period?: string; original_deadline?: string }): {
    form?: string; period?: string; label?: string; due?: string | null; removed: DeadlineOverrideEntry[];
};
export interface AsNeededReturn {
    draft_id: number;
    period: number;
    return_key: number | null;
    status: string;
    updated_at: string;
}
export interface AsNeededForm {
    tin: string;
    tin_root: string;
    branch_code: string;
    branch_name: string;
    taxpayer_name: string;
    branch_count: number;
    form_code: string;
    form_name: string;
    taxable_year: number;
    /** What dates a return ("Removal"), null when the form has no trigger-date rule. */
    trigger: string | null;
    /** When it is due, as the form's guide says. */
    due_rule: string;
    /** Saved returns of the form that hold no trigger date yet. */
    undated: AsNeededReturn[];
}
export function as_needed_forms(year: number): AsNeededForm[];
export interface ReturnRow {
    id: number;
    tin: string;
    tin_root: string;
    branch_code: string;
    branch_name: string;
    taxpayer_name: string;
    branch_count: number;
    form_code: string;
    taxable_year: number;
    period: number;
    period_label: string;
    status: string;
    updated_at: string;
    /** The return's stored key (`screen_form_open`'s `key`): opens exactly this return. */
    return_key: number | null;
}
export function list_returns(which: "filed" | "saved" | "all"): ReturnRow[];
"#;

pub fn register(module: HostModule) -> HostModule {
    module
        .function("deadlines_between", deadlines_between)
        .function("as_needed_forms", as_needed_forms)
        .function("list_returns", list_returns)
        .function("deadline_overrides", deadline_overrides)
        .function("deadline_override_set", deadline_override_set)
        .function("deadline_override_remove", deadline_override_remove)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn period_label(period: &DeadlinePeriod) -> (u8, String) {
    match period {
        DeadlinePeriod::Monthly {
            taxable_year,
            month,
        } => (
            *month,
            format!(
                "{} {taxable_year}",
                MONTHS[(*month as usize).clamp(1, 12) - 1]
            ),
        ),
        DeadlinePeriod::Quarterly {
            taxable_year,
            quarter,
        } => (*quarter, format!("Q{quarter} {taxable_year}")),
        DeadlinePeriod::Annual { taxable_year } => (0, format!("TY {taxable_year}")),
        DeadlinePeriod::EventBased => (0, "As needed".to_string()),
    }
}

/// What moved a deadline off its rule's date, as `DeadlineRow` names it.
fn deadline_status(status: &DeadlineStatus) -> &'static str {
    match status {
        DeadlineStatus::WeekendAdjusted => "weekend",
        DeadlineStatus::HolidayAdjusted => "holiday",
        DeadlineStatus::NonWorkingDayAdjusted => "non_working",
        DeadlineStatus::Extended => "moved",
        DeadlineStatus::Normal | DeadlineStatus::EventBased => "normal",
    }
}

/// The original date, status and override source of a dated row.
fn deadline_fields(row: &mut serde_json::Map<String, Value>, deadline: &ResolvedTaxDeadline) {
    row.insert(
        "original_due_date".into(),
        json!(
            deadline
                .original_deadline_date()
                .map(|date| date.format("%Y-%m-%d").to_string())
        ),
    );
    row.insert(
        "deadline_status".into(),
        json!(deadline_status(&deadline.status)),
    );
    row.insert(
        "moved_reason".into(),
        json!(
            (deadline.status == DeadlineStatus::Extended)
                .then(|| deadline.source_reference.clone())
                .flatten()
        ),
    );
}

/// `period` as a deadline override names it (`2026-Q3`, `2026-09`, `2025`).
fn period_key(period: &DeadlinePeriod) -> Option<String> {
    match period {
        DeadlinePeriod::Monthly {
            taxable_year,
            month,
        } => Some(format!("{taxable_year}-{month:02}")),
        DeadlinePeriod::Quarterly {
            taxable_year,
            quarter,
        } => Some(format!("{taxable_year}-Q{quarter}")),
        DeadlinePeriod::Annual { taxable_year } => Some(taxable_year.to_string()),
        DeadlinePeriod::EventBased => None,
    }
}

fn summary_period(summary: &FormDraftSummary) -> u8 {
    summary.month.or(summary.quarter).unwrap_or(0)
}

fn summary_label(summary: &FormDraftSummary) -> String {
    match (summary.month, summary.quarter) {
        (Some(month), _) if (1..=12).contains(&month) => {
            format!("{} {}", MONTHS[month as usize - 1], summary.taxable_year)
        }
        (_, Some(quarter)) if (1..=4).contains(&quarter) => {
            format!("Q{quarter} {}", summary.taxable_year)
        }
        _ => format!("TY {}", summary.taxable_year),
    }
}

fn status_name(status: &FilingStatus) -> String {
    format!("{status:?}")
}

fn parse_date(text: &str) -> Result<NaiveDate, HostError> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| HostError::new(format!("dates are YYYY-MM-DD, got {text:?}")))
}

fn error(error: impl std::fmt::Display) -> HostError {
    HostError::new(error.to_string())
}

/// One branch of one taxpayer, flattened for the feeds.
struct BranchRef {
    tin: String,
    tin_root: String,
    branch_code: String,
    branch_name: String,
    taxpayer_name: String,
    branch_count: usize,
}

fn branches(db: &Database) -> Result<Vec<BranchRef>, HostError> {
    let mut out = Vec::new();
    for taxpayer in db.list_taxpayers().map_err(error)? {
        let count = taxpayer.branches.len();
        for branch in &taxpayer.branches {
            out.push(BranchRef {
                tin: branch.tin.clone(),
                tin_root: taxpayer.taxpayer.tin_root.clone(),
                branch_code: branch.branch_code.clone(),
                branch_name: branch.name.clone(),
                taxpayer_name: taxpayer.taxpayer.name.clone(),
                branch_count: count,
            });
        }
    }
    Ok(out)
}

fn branch_fields(branch: &BranchRef) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    map.insert("tin".into(), json!(branch.tin));
    map.insert("tin_root".into(), json!(branch.tin_root));
    map.insert("branch_code".into(), json!(branch.branch_code));
    map.insert("branch_name".into(), json!(branch.branch_name));
    map.insert("taxpayer_name".into(), json!(branch.taxpayer_name));
    map.insert("branch_count".into(), json!(branch.branch_count));
    map
}

/// Every dated deadline in `[from, to]` for the forms each branch files in
/// the taxable years that can fall in that window, with the return's status.
fn deadline_rows(db: &Database, from: NaiveDate, to: NaiveDate) -> Result<Vec<Value>, HostError> {
    use chrono::Datelike;
    let global = db.get_deadline_overrides();
    let working = bir_core::official_holidays::business_day_calendar();
    // An annual return for year Y is due in Y+1, so the window's first year
    // minus one is the earliest taxable year that can have a deadline in it.
    let first_year = from.year() - 1;
    let last_year = to.year();
    let mut rows = Vec::new();
    for branch in branches(db)? {
        // The profile's own deadline overrides apply on top of the stored
        // global ones, as the Google Calendar sync and the desktop
        // dashboard apply them. A profile that cannot be read costs that
        // branch its profile overrides only, never the other branches' rows.
        let profile = match db.get_profile(&branch.tin) {
            Ok(profile) => profile,
            Err(read_error) => {
                tracing::warn!(
                    tin = %branch.tin,
                    error = %read_error,
                    "deadline feed: profile unreadable; its own deadline overrides are skipped"
                );
                None
            }
        };
        // The as-needed rows' overrides: global, then the profile's for
        // every Forms Set year (one definition with the Google Calendar
        // sync and the penalty clock, decision P5-9).
        let event_overrides = match &profile {
            Some(profile) => bir_core::forms::event_trigger::as_needed_overrides(&global, profile),
            None => global.clone(),
        };
        // As-needed returns of any taxable year whose due date falls in the
        // window (an estate's year-later deadline, a return filed under
        // another year).
        rows.extend(as_needed_rows(
            db,
            &branch,
            &event_overrides,
            &working,
            from,
            to,
        )?);
        for year in first_year..=last_year {
            let Ok(year16) = u16::try_from(year) else {
                continue;
            };
            let codes = db
                .active_form_codes_for_year(&branch.tin, year16)
                .map_err(error)?;
            if codes.is_empty() {
                continue;
            }
            let drafts = db
                .list_draft_summaries(&branch.tin, year16)
                .map_err(error)?;
            let overrides = match &profile {
                Some(profile) => bir_core::forms::event_trigger::periodic_overrides(
                    &global,
                    profile,
                    Some(year16),
                ),
                None => global.clone(),
            };
            for deadline in DeadlineResolver::deadlines_for_forms(&codes, year, &overrides) {
                let Some(due) = deadline.final_deadline_date() else {
                    continue;
                };
                if due < from || due > to {
                    continue;
                }
                let (period, label) = period_label(&deadline.period);
                let draft = drafts.iter().find(|summary| {
                    summary.form_code == deadline.form_code && summary_period(summary) == period
                });
                let frequency = match deadline.period {
                    DeadlinePeriod::Monthly { .. } => "Monthly",
                    DeadlinePeriod::Quarterly { .. } => "Quarterly",
                    _ => "Annual",
                };
                let mut row = branch_fields(&branch);
                row.insert("form_code".into(), json!(deadline.form_code));
                row.insert("form_name".into(), json!(deadline.form_name));
                row.insert("frequency".into(), json!(frequency));
                row.insert("taxable_year".into(), json!(year));
                row.insert("period".into(), json!(period));
                row.insert("period_label".into(), json!(label));
                row.insert("due_date".into(), json!(due.format("%Y-%m-%d").to_string()));
                row.insert(
                    "status".into(),
                    json!(
                        draft.map_or_else(|| "Not started".to_string(), |d| status_name(&d.status))
                    ),
                );
                row.insert("draft_id".into(), json!(draft.map(|d| d.id)));
                let key = match draft {
                    Some(draft) => db.draft_period_column(draft.id).map_err(error)?,
                    None => None,
                };
                row.insert("return_key".into(), json!(key));
                deadline_fields(&mut row, &deadline);
                row.insert("period_key".into(), json!(period_key(&deadline.period)));
                rows.push(Value::Object(row));
            }
        }
    }
    rows.sort_by(|a, b| {
        a["due_date"]
            .as_str()
            .cmp(&b["due_date"].as_str())
            .then(
                a["taxpayer_name"]
                    .as_str()
                    .cmp(&b["taxpayer_name"].as_str()),
            )
            .then(a["form_code"].as_str().cmp(&b["form_code"].as_str()))
    });
    Ok(rows)
}

/// The dated rows of a branch's saved as-needed returns, any taxable year,
/// that hold an entered trigger date due in `[from, to]`
/// (`event_trigger::dated_event_returns`). A saved return is its own
/// evidence the form is filed, so these do not depend on the year's forms.
fn as_needed_rows(
    db: &Database,
    branch: &BranchRef,
    overrides: &[DeadlineOverride],
    working: &BusinessDayCalendar,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<Value>, HostError> {
    let mut rows = Vec::new();
    for dated in
        bir_core::forms::event_trigger::dated_event_returns(db, &branch.tin, overrides, working)
            .map_err(error)?
    {
        let (summary, trigger, deadline) = (&dated.summary, dated.trigger, &dated.deadline);
        let Some(due) = deadline.final_deadline_date() else {
            continue;
        };
        if due < from || due > to {
            continue;
        }
        let mut row = branch_fields(branch);
        row.insert("form_code".into(), json!(summary.form_code));
        row.insert("form_name".into(), json!(deadline.form_name));
        row.insert("frequency".into(), json!("As needed"));
        row.insert("taxable_year".into(), json!(summary.taxable_year));
        row.insert("period".into(), json!(summary_period(summary)));
        row.insert(
            "period_label".into(),
            json!(format!(
                "{} {}",
                trigger.rule.trigger,
                trigger.date.format("%b %-d, %Y")
            )),
        );
        row.insert("due_date".into(), json!(due.format("%Y-%m-%d").to_string()));
        row.insert("status".into(), json!(status_name(&summary.status)));
        row.insert("draft_id".into(), json!(summary.id));
        row.insert(
            "return_key".into(),
            json!(db.draft_period_column(summary.id).map_err(error)?),
        );
        deadline_fields(&mut row, deadline);
        row.insert("period_key".into(), Value::Null);
        rows.push(Value::Object(row));
    }
    Ok(rows)
}

/// The as-needed forms each branch files in `year` (its yearly forms the
/// calendar files as needed), each with its saved returns that hold no
/// trigger date yet. Branches in `list_taxpayers` order, forms by code.
fn as_needed_form_rows(db: &Database, year: u16) -> Result<Vec<Value>, HostError> {
    let mut rows = Vec::new();
    for branch in branches(db)? {
        let mut codes: Vec<String> = db
            .active_form_codes_for_year(&branch.tin, year)
            .map_err(error)?
            .into_iter()
            .filter(|code| DeadlineResolver::is_as_needed(code))
            .collect();
        if codes.is_empty() {
            continue;
        }
        codes.sort();
        let drafts = db.list_draft_summaries(&branch.tin, year).map_err(error)?;
        for code in codes {
            let rule = event_due_rule(&code);
            let mut undated = Vec::new();
            for summary in drafts.iter().filter(|summary| summary.form_code == code) {
                if db.draft_event_trigger(summary.id).map_err(error)?.is_some() {
                    continue;
                }
                undated.push(json!({
                    "draft_id": summary.id,
                    "period": summary_period(summary),
                    "return_key": db.draft_period_column(summary.id).map_err(error)?,
                    "status": status_name(&summary.status),
                    "updated_at": summary.updated_at,
                }));
            }
            let form_name = bir_core::forms::FORM_REGISTRY
                .iter()
                .find(|form| form.code == code)
                .map_or(code.as_str(), |form| form.title);
            let mut row = branch_fields(&branch);
            row.insert("form_code".into(), json!(code));
            row.insert("form_name".into(), json!(form_name));
            row.insert("taxable_year".into(), json!(year));
            row.insert("trigger".into(), json!(rule.map(|rule| rule.trigger)));
            row.insert(
                "due_rule".into(),
                json!(rule.map_or("Upon the transaction or event", |rule| rule.description)),
            );
            row.insert("undated".into(), Value::Array(undated));
            rows.push(Value::Object(row));
        }
    }
    Ok(rows)
}

fn as_needed_forms(arguments: &HostArguments) -> HostResult {
    let year = arguments.integer(0)?;
    let year =
        u16::try_from(year).map_err(|_| HostError::new(format!("a tax year, got {year}")))?;
    with_database(|db| serialize(&as_needed_form_rows(db, year)?))
}

/// The one object argument of the deadline override functions (none: `{}`).
fn object_argument(arguments: &HostArguments) -> Result<serde_json::Map<String, Value>, HostError> {
    if arguments.is_empty() {
        return Ok(serde_json::Map::new());
    }
    match crate::database::to_json(arguments.value(0)?) {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(serde_json::Map::new()),
        other => Err(HostError::new(format!(
            "expected an object argument, got {other}"
        ))),
    }
}

/// `bir_core::deadline_overrides::list`: the overrides apply to every taxpayer.
fn deadline_overrides(arguments: &HostArguments) -> HostResult {
    let args = object_argument(arguments)?;
    with_database(|db| {
        serialize(&bir_core::deadline_overrides::list(db, &args).map_err(HostError::new)?)
    })
}

fn deadline_override_set(arguments: &HostArguments) -> HostResult {
    let args = object_argument(arguments)?;
    let today = chrono::Local::now().date_naive();
    with_database(|db| {
        serialize(&bir_core::deadline_overrides::set(db, &args, today).map_err(HostError::new)?)
    })
}

fn deadline_override_remove(arguments: &HostArguments) -> HostResult {
    let args = object_argument(arguments)?;
    let today = chrono::Local::now().date_naive();
    with_database(|db| {
        serialize(&bir_core::deadline_overrides::remove(db, &args, today).map_err(HostError::new)?)
    })
}

fn deadlines_between(arguments: &HostArguments) -> HostResult {
    let from = parse_date(arguments.string(0)?)?;
    let to = parse_date(arguments.string(1)?)?;
    with_database(|db| serialize(&deadline_rows(db, from, to)?))
}

/// Saved (Draft) and/or filed (anything past Draft) returns across every
/// branch, newest first.
fn return_rows(db: &Database, which: &str) -> Result<Vec<Value>, HostError> {
    let want_filed = matches!(which, "filed" | "all");
    let want_saved = matches!(which, "saved" | "all");
    if !want_filed && !want_saved {
        return Err(HostError::new(format!(
            "list_returns takes \"filed\", \"saved\" or \"all\", got {which:?}"
        )));
    }
    let branches = branches(db)?;
    // Filed returns come from one query (the notifications poll reads them
    // every 30 s); saved drafts are listed per branch and year.
    let mut summaries: Vec<FormDraftSummary> = Vec::new();
    if want_filed {
        summaries.extend(db.list_all_filed_submissions().map_err(error)?);
    }
    if want_saved {
        for branch in &branches {
            for year in FIRST_TAX_YEAR..=current_year() + 1 {
                summaries.extend(
                    db.list_draft_summaries(&branch.tin, year)
                        .map_err(error)?
                        .into_iter()
                        .filter(|summary| summary.status == FilingStatus::Draft),
                );
            }
        }
    }
    let mut rows = Vec::new();
    for summary in summaries {
        // A return whose TIN no longer matches a branch (a removed profile)
        // has nobody to show it under.
        let Some(branch) = branches.iter().find(|branch| branch.tin == summary.tin) else {
            continue;
        };
        let mut row = branch_fields(branch);
        row.insert("id".into(), json!(summary.id));
        row.insert("form_code".into(), json!(summary.form_code));
        row.insert("taxable_year".into(), json!(summary.taxable_year));
        row.insert("period".into(), json!(summary_period(&summary)));
        row.insert("period_label".into(), json!(summary_label(&summary)));
        row.insert("status".into(), json!(status_name(&summary.status)));
        row.insert("updated_at".into(), json!(summary.updated_at));
        row.insert(
            "return_key".into(),
            json!(db.draft_period_column(summary.id).map_err(error)?),
        );
        rows.push(Value::Object(row));
    }
    rows.sort_by(|a, b| b["updated_at"].as_str().cmp(&a["updated_at"].as_str()));
    Ok(rows)
}

/// The design's earliest tax year; the bound itself is enforced in bir-core.
const FIRST_TAX_YEAR: u16 = 2010;

fn current_year() -> u16 {
    use chrono::Datelike;
    u16::try_from(chrono::Local::now().year()).unwrap_or(2026)
}

fn list_returns(arguments: &HostArguments) -> HostResult {
    let which = arguments.string(0)?.to_owned();
    with_database(|db| serialize(&return_rows(db, &which)?))
}
