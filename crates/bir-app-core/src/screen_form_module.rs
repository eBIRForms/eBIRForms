//! The redesigned on-screen form page's host functions.
//!
//! The page (JavaScript) renders a form from `app/forms/forms-schema.json`
//! and binds every control by its rules-corpus key `k` (e.g.
//! `frm1601c:txtMonth`). Everything it needs about a draft goes through
//! bir-core's agent seam (`bir_core::forms::agent`): `fill` writes one box,
//! `compute`/`validate` run the form's own formulas and checks, `field_map`
//! reads every box back as official key -> string, and `save_draft` saves
//! the typed draft the way the painted page's "Save draft" does.
//!
//! Drafts open the way each painted page opens them (stored draft for the
//! taxpayer and period, else a new one built from the profile), reproduced
//! here because bir-shell cannot depend on bir-desktop. A Form library
//! opening (`screen_form_sample`) always builds the new one and shows its
//! period and date boxes blank; the page never saves it.
//!
//! A box the profile filled is locked until the filer chooses Edit on it;
//! the edit then stays on this return's draft and never reaches the profile.
//! A box the profile left blank is an ordinary box.
//!
//! `screen_form_open` and `screen_form_validate` are asynchronous: the main
//! thread reads the database and parses arguments, and the returned future
//! runs compute, validation and the field map off the main thread without
//! touching the database (it is thread-local and not `Send`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use bir_core::db::Database;
// A page edit writes a draft path the way the penalty layer's trial does:
// one setter, bir-core's.
use crate::host::{HostArguments, HostError, HostModule, HostObject, HostResult, HostValue};
use bir_core::forms::agent::set_draft_path;
use bir_core::forms::agent::{self, AgentFormDef, AgentFormKind, Binding, BoxDef};
use bir_core::forms::queueable::{QueueableForm, QueueableKind};
use bir_core::forms::registry::FilingFrequency;
use bir_core::forms::{FilingPeriod, FilingStatus};
use bir_core::profile::TaxpayerProfile;
use serde_json::{Map, Value, json};

use crate::database::{json, to_json, with_database};

mod catalogs;
mod combs;
mod conditions;
mod penalties;
mod rows_1604;
mod rows_1700;
mod rows_2550m;
mod rows_2550q;
mod rows_annual_corporate;
mod rows_annual_individual;
mod rows_as_needed;
mod rows_donor_estate;
mod rows_dst_pt_vat;
mod rows_excise;
mod rows_income_q;
mod rows_monthly;
mod rows_withholding;

/// The JSON shapes of the functions below, in TypeScript notation (kept from
/// the old JS UI as the spec for typing them).
#[allow(dead_code)]
pub const DECLARATIONS: &str = r#"
export interface ScreenFormIndexEntry {
    c: string;
    t: string;
    rev: string;
    freq: string;
    pages: number;
    n: number;
    q: number;
}
export interface ScreenFormError {
    field: string;
    message: string;
    keys: string[];
    /** A notice the filer may mark checked without changing anything: the token `screen_form_check_notice` takes, and its button's label. */
    dismiss?: string;
    dismiss_label?: string;
}
export type ScreenFormStatus = "Draft" | "Queued" | "Sent" | "Confirmed" | "Paid";
export interface ScreenFormOpened {
    form_code: string;
    year: number;
    period: number;
    status: ScreenFormStatus;
    editable: boolean;
    /** The period column the draft is stored under: pass it to queue and print. */
    stored_period: number | null;
    draft: object;
    values: Record<string, string>;
    options: Record<string, ScreenFormOption[]>;
    locks: Record<string, string>;
    /** Boxes the taxpayer profile filled (the page tucks and marks them); a box it left blank is not here and not locked. */
    profile: string[];
    /** Filled profile boxes the filer may open with Edit; pass the opened ones to `screen_form_fill` as `unlocked`. */
    profile_edit: string[];
    /** Every profile box, filled by the profile or left blank: an edit to one stays on this return once it is passed as `unlocked`. */
    profile_boxes: string[];
    /** What the profile alone shows in each profile box ("" where it is blank). */
    profile_values: Record<string, string>;
    /** The profile fact a profile box shows (`rdo_code`, `registered_address`, `zip_code`, `phone`, `email`, `line_of_business`), for "Save to profile too". */
    profile_facts: Record<string, string>;
    /** Profile boxes the filer changed on this return: they keep their value when the profile changes. */
    profile_edited: string[];
    /** Opened from the Form library: a blank sample; saved once its period is set, it becomes that period's return. */
    sample: boolean;
    /** A sample's boxes shown blank while its period is not set: its period parts and everything that follows the period. */
    blank: string[];
    /** A sample's held box that cannot be typed: what it waits for (shown in the box). */
    needs: Record<string, string>;
    /** A printed box the filer cannot fill here, while it shows blank: why, in the filer's words (the schema's reviewed `why`; shown in the box). */
    whys: Record<string, string>;
    /** A blank box's reason, shown in the box: what it waits for (a rate its row's ATC, a tick box the amount that ticks it, a date the return it is started with). */
    waits: Record<string, string>;
    /** An untouched date whose message the page holds until it is edited or Check / Save is pressed (1800/1801 item 1). */
    quiet?: { field: string; keys: string[] } | null;
    /** Why each locked box is shut, in the filer's words (drawn under its item while the box shows blank). */
    lock_notes: Record<string, string>;
    /** A sample's period parts: page inputs (never draft writes) the filer sets the period with. */
    period_parts: ScreenFormPeriodPart[];
    /** A sample rebuilt for a period its filer completed. */
    period_set: boolean;
    /** An amended return's filed version, kept in its history (null when never amended): open it with `screen_form_filed_original`. */
    filed_original?: { id: number; status: string; filed_on: string | null } | null;
    /** Set on the read-only view of a filed original. */
    filed_original_view?: { status: string; filed_on: string | null };
    /** When a return was sent to BIR and confirmed (a read-only view says so). */
    submitted_at: string | null;
    confirmed_at: string | null;
    /** A new amended return (`screen_form_amend`): the status of the filed return it amends. */
    amends?: ScreenFormStatus;
    /** Boxes another answer turns off right now, each with its tag ("Needs item 13 = Yes"; the page fades them). */
    off: Record<string, string>;
    /** Each party's Surcharge, Interest and Compromise boxes: locked at the computed amounts until Edit penalties opens them (sent as `unlocked`). */
    penalties: ScreenFormPenalties[];
    /** Each pooled address (by its first line) as the draft holds it: the page shows this, never the field map's cut lines. */
    pools: Record<string, string>;
    /** A stored Draft whose profile boxes the filer did not edit followed a later tax-profile change (saved): what changed ("RDO 050 → 043"). Empty when nothing moved. */
    profile_updates?: string[];
    /** A Draft saved before it recorded its profile baseline (or reverted to Draft): its unedited profile boxes that differ from the profile now ("RDO 039 (profile 043)"), not changed; `screen_form_use_profile` applies them. */
    profile_differs?: string[];
    /** One-time notes this opening offers (`key`, `text`); show the ones `screen_form_notes_seen` records. */
    notes?: { key: string; text: string }[];
    errors: ScreenFormError[];
}
export interface ScreenFormPeriodPart {
    key: string;
    part: "year" | "month" | "year_end_month" | "day" | "quarter" | "date";
    /** For a quarter's tick box: which quarter. */
    quarter: number | null;
}
export interface ScreenFormExisting {
    form_code: string;
    year: number;
    period: number;
    label: string;
    status: ScreenFormStatus;
    updated_at: string;
    total_payable: number | null;
    /** What the page may offer: `load` (a draft), `view` (read-only), `amend` (reached BIR, and the form has an amended-return answer), `keep`. */
    actions: ("load" | "view" | "amend" | "keep")[];
    submitted_at: string | null;
    confirmed_at: string | null;
}
export interface ScreenFormSamplePeriod {
    complete: boolean;
    /** A stored return of the form already has this period. */
    existing?: ScreenFormExisting;
    /** No stored return: a fresh sample built for exactly this period. */
    sample?: ScreenFormOpened;
    /** What `screen_form_open` takes for it: year, period, and the exact stored key (`key`, an existing answer) or the key a new return starts under (`new_key`, with `new_key_from`: the sample it was offered for, its `created_at`, which the open passes back); null: start it with New return. */
    opens?: { year: number; period: number; key: number | null; new_key?: number | null; new_key_from?: string | null } | null;
}
export interface ScreenFormPenalties {
    keys: string[];
    /** The filer entered the amounts: they file instead of the computed ones; blanks go back to the computed. */
    edited: boolean;
    /** The amounts are kept as entered before penalties were computed (1701Q, 1701): "Use the computed amounts" works once Edit opens them. */
    kept?: boolean;
    /** The party is on the return as it stands (a spouse's column with no spouse is not): only then is Edit offered. */
    applies: boolean;
}
export interface ScreenFormFilled {
    draft: object;
    values: Record<string, string>;
    options: Record<string, ScreenFormOption[]>;
    rejected: Record<string, string>;
    penalties: ScreenFormPenalties[];
    /** Each pooled address (by its first line) as the draft holds it: the page shows this, never the field map's cut lines. */
    pools: Record<string, string>;
    /** A sample's boxes still blank (null for a return). */
    blank?: string[] | null;
    /** Rows the filer typed that this edit removed (an answer that closes a section, a cleared ATC): the page asks `message` first and offers Undo with `done`. */
    removes?: { message: string; done: string; rows: number } | null;
    /** Locks this fill opened or shut (a folded item, a box an answer shuts): the reason, or null to open the box; `relock_notes` holds each one's note. */
    relocks?: Record<string, string | null>;
    relock_notes?: Record<string, string | null>;
}
export interface ScreenFormValidated {
    draft: object;
    values: Record<string, string>;
    options: Record<string, ScreenFormOption[]>;
    errors: ScreenFormError[];
    /** Boxes another answer turns off right now, each with its tag. */
    off: Record<string, string>;
    penalties: ScreenFormPenalties[];
    /** Each pooled address (by its first line) as the draft holds it: the page shows this, never the field map's cut lines. */
    pools: Record<string, string>;
    /** A sample's boxes still blank (null for a return). */
    blank?: string[] | null;
}
export interface ScreenFormOption {
    value: string;
    label: string;
    /** Selected by the typed draft when multiple choices share a code. */
    selected?: boolean;
}
export interface ScreenFormSaved {
    /** A Form library sample: false when its period already has a stored return (nothing saved; `existing` and `opens` are the offer). */
    saved?: boolean;
    existing?: ScreenFormExisting;
    /** A saved sample: open the return it became with these (`screen_form_open`'s year, period and key). */
    opens?: { year: number; period: number; key: number | null };
    id?: number;
    status?: ScreenFormStatus;
    stored_period?: number | null;
    draft?: object;
}
export function screen_form_schema(form_code: string): object;
export function screen_forms_index(): ScreenFormIndexEntry[];
/** `key`: the exact stored return (a transaction's MMDD, 2553's quarter; gone, the open fails), else the latest of the period. `new_key`: no stored return, a new one started under that key (the typed date); `new_key_from`: the sample the host offered `new_key` for (its `created_at`). */
export function screen_form_open(tin: string, form_code: string, year: number, period: number, key?: number | null, new_key?: number | null, new_key_from?: string | null): Promise<ScreenFormOpened>;
/** The Form library's blank sample of a form for this taxpayer: reads no stored return and computes no penalty (its draft is marked). Saved once its period is set, it becomes that period's return; queue and print take the saved return. */
export function screen_form_sample(tin: string, form_code: string): Promise<ScreenFormOpened>;
/** A sample's typed period parts (key -> value): incomplete, an existing stored return, or a fresh sample for that period. */
/** A NEW amended return copied from a stored one that reached BIR; nothing is stored until it is saved. The filed version is kept in this return's history. */
export function screen_form_amend(tin: string, form_code: string, year: number, period: number, key?: number | null): Promise<ScreenFormOpened>;
/** An amended return's filed version from its history, read-only. */
export function screen_form_filed_original(form_code: string, snapshot_id: number): Promise<ScreenFormOpened>;
export function screen_form_sample_period(tin: string, form_code: string, parts: Record<string, string>): Promise<ScreenFormSamplePeriod>;
/** `unlocked`: profile boxes the page has open (opened with Edit, or left blank by the profile); their edits stay on this return and never reach the profile. */
export function screen_form_fill(form_code: string, draft: object, edits: Record<string, string | number | boolean | null>, unlocked?: string[]): ScreenFormFilled;
/** "Use profile values" on a draft that differs from the tax profile (`profile_differs`): every profile box the filer did not edit takes the profile's value, and they follow later profile changes. With `keys`, only those boxes (edited on this return or not) take the profile's value and stop being the filer's edits. Nothing is saved; save the draft. */
export function screen_form_use_profile(form_code: string, year: number, period: number, draft: object, keys?: string[]): ScreenFormFilled;
export function screen_form_validate(form_code: string, draft: object): Promise<ScreenFormValidated>;
/** Save a draft. A Form library sample saves only once its period is set, never over a stored return of that period (answering the offer instead); an entry-numbered form takes its next free entry. */
export function screen_form_save(form_code: string, draft: object): ScreenFormSaved;
/** Print preview of a saved draft (opens the filled official sheet in the in-app preview window). */
export function screen_form_print(form_code: string, draft: object): { html: string; title: string };
/** Print preview of a Form library sample as the page holds it, staged edits included and its held boxes blank. Nothing is saved and no period is needed; the preview then follows `screen_form_preview_sync`. */
export function screen_form_print_sample(form_code: string, draft: object, edits: Record<string, string | number | boolean | null>, unlocked?: string[]): { html: string; title: string };
/** Queue a saved draft; 1601C and 2551Q use queue_form_draft instead. */
export function screen_form_queue(form_code: string, draft: object): number;
/** Take a queued draft out of the queue; 1601C and 2551Q use cancel_queued. */
export function screen_form_cancel_queue(form_code: string, draft: object): null;
/** Record one-time notes (`ScreenFormOpened.notes` keys) as seen, each only if not yet recorded; the keys this call recorded, which are the notes to show. */
export function screen_form_notes_seen(keys: string[]): string[];
/** Mark a notice (`ScreenFormError.dismiss`) of a saved draft checked; the return is not changed. */
export function screen_form_check_notice(form_code: string, draft: object, token: string): null;
"#;

pub fn register(module: HostModule) -> HostModule {
    module
        .function("screen_form_schema", screen_form_schema)
        .function("screen_forms_index", |_| forms_index())
        .async_function("screen_form_open", screen_form_open)
        .async_function("screen_form_sample", screen_form_sample)
        .async_function("screen_form_sample_period", screen_form_sample_period)
        .async_function("screen_form_amend", screen_form_amend)
        .async_function("screen_form_filed_original", screen_form_filed_original)
        .function("screen_form_fill", screen_form_fill)
        .function("screen_form_use_profile", screen_form_use_profile)
        .async_function("screen_form_validate", screen_form_validate)
        .function("screen_form_save", screen_form_save)
        .function("screen_form_print", screen_form_print)
        .function("screen_form_print_sample", screen_form_print_sample)
        .function("screen_form_print_preview", |arguments| {
            let (html, title) = print_preview_page(arguments)?;
            Ok(HostObject::new()
                .field("html", html)
                .field("title", title)
                .into())
        })
        .function("screen_form_queue", screen_form_queue)
        .function("screen_form_cancel_queue", screen_form_cancel_queue)
        .function("screen_form_notes_seen", screen_form_notes_seen)
        .function("screen_form_check_notice", screen_form_check_notice)
}

/// Record the one-time notes an opening offered as seen (D3), each by an
/// atomic insert-if-absent, and answer the keys this call recorded: the
/// notes the page keeps. Called by the page once it has drawn the opening,
/// so an opening that fails before then offers the note again. Of two
/// openings that both drew it, the one whose call records nothing takes it
/// off (it shows briefly there). Only one-time note keys are written.
fn screen_form_notes_seen(arguments: &HostArguments) -> HostResult {
    let keys: Vec<String> = match to_json(arguments.value(0)?) {
        Value::Array(keys) => keys
            .iter()
            .filter_map(|key| key.as_str().map(str::to_string))
            .collect(),
        _ => return Err(host_error("keys must be a list")),
    };
    if let Some(key) = keys
        .iter()
        .find(|key| !key.starts_with(rows_income_q::NOTE_SEEN_PREFIX))
    {
        return Err(host_error(format!("`{key}` is not a one-time note")));
    }
    let recorded = with_database(|db| {
        let mut recorded = Vec::new();
        for key in keys {
            if db
                .insert_setting_if_absent(&key, "1")
                .map_err(|error| host_error(error.to_string()))?
            {
                recorded.push(key);
            }
        }
        Ok(recorded)
    })?;
    Ok(json(json!(recorded)))
}

/// Mark a saved draft's notice checked (D5: a 1604E row), as app state;
/// the return is not changed.
fn screen_form_check_notice(arguments: &HostArguments) -> HostResult {
    let kind = kind_of(arguments.string(0)?)?;
    let draft = draft_argument(arguments, 1)?;
    let token = arguments.string(2)?.to_owned();
    with_database(|db| rows_1604::check_notice(db, kind, &draft, &token).map_err(host_error))?;
    Ok(HostValue::Null)
}

fn host_error(message: impl Into<String>) -> HostError {
    HostError::new(message)
}

// ---------------------------------------------------------------- schema

/// The on-screen form schema: one entry per form, parsed once, with the
/// reviewed corrections in `app/forms/schema-overrides.json` applied.
/// 1700's entry is replaced by `app/forms/screen-1700-v2018.json`: the
/// extracted entry follows the 2013 rule package, while the model, its
/// upload and the printed sheet are the January 2018 revision.
fn schema() -> &'static [Value] {
    static SCHEMA: OnceLock<Vec<Value>> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let mut forms: Vec<Value> = serde_json::from_str(&bir_core::packed::inflate(
            bir_core::packed_bytes!("assets/forms/forms-schema.json"),
        ))
        .expect("app/forms/forms-schema.json is a JSON array of forms");
        let form_1700: Value = serde_json::from_str(&bir_core::packed::inflate(
            bir_core::packed_bytes!("assets/forms/screen-1700-v2018.json"),
        ))
        .expect("app/forms/screen-1700-v2018.json is one form");
        for form in forms.iter_mut().filter(|form| form["c"] == "1700") {
            *form = form_1700.clone();
        }
        let overrides: Value = serde_json::from_str(&bir_core::packed::inflate(
            bir_core::packed_bytes!("assets/forms/schema-overrides.json"),
        ))
        .expect("app/forms/schema-overrides.json is a JSON object");
        for form in &mut forms {
            let code = form["c"].as_str().unwrap_or_default().to_string();
            if let Some(fixes) = overrides.get(&code).and_then(Value::as_object) {
                apply_overrides(form, fixes);
                if let Some(layout) = fixes.get("layout") {
                    apply_layout(form, layout);
                }
            }
            hide_repeated_occurrences(form);
            retype_by_binding(form, &code);
            if code == "2551Q" {
                inject_2551q_not_applicable(form);
            }
            mark_radio_groups(form, &code);
            rows_dst_pt_vat::mark_calendar_tables(form, &code);
            rows_income_q::mark_one_date(form, &code);
            rows_annual_corporate::mark_whole_pesos(form, &code);
            rows_as_needed::mark_calendar_tables(form, &code);
            rows_donor_estate::mark_schema(form, &code);
            rows_as_needed::mark_one_date(form, &code);
            combs::apply_combs(form, &code);
            // 1701A's item 9, printed from `derived:` lines (PR 7).
            rows_annual_individual::mark_pools(form, &code);
            // 1701 Part VIII item 6's hint from Part VII item 8 (P4-10).
            rows_annual_individual::mark_from(form, &code);
        }
        forms
    })
}

/// What a number box holds, from the draft field it writes and its label:
/// a count or quantity (`number_of_*`, "No. of …", 2200M and 2200S's
/// volumes) or a tax rate (`tax_rate`,
/// `…_rate_percent`), else an amount. Other rates (2200M and 2200P's
/// specific excise rates, pesos per unit; 1602Q's BSP exchange rate) are
/// pesos and stay amounts.
#[derive(PartialEq)]
enum Quantity {
    Count,
    TaxRate,
    Amount,
}

fn quantity(path: &str, label: &str) -> Quantity {
    let leaf = path
        .rsplit('/')
        .find(|segment| !segment.is_empty() && !segment.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_default();
    let words: Vec<&str> = leaf.split('_').collect();
    let label = label.to_ascii_lowercase();
    if leaf.starts_with("number_of")
        || ["count", "volume", "quantity"]
            .iter()
            .any(|w| words.contains(w))
        || ["no. of", "no of ", "number of", "volume", "quantity"]
            .iter()
            .any(|w| label.contains(w))
    {
        Quantity::Count
    } else if words.contains(&"rate") && (words.contains(&"tax") || words.contains(&"percent")) {
        Quantity::TaxRate
    } else {
        Quantity::Amount
    }
}

/// A plain text box that is one entry of a draft list another entry of
/// which keeps a whole date (1702MX's effectivity dates, columns A and C).
fn dated_list_entry(def: &AgentFormDef, key: &str) -> bool {
    use bir_core::forms::agent::DateFormat;
    // `/a/b/2` -> `/a/b`: only an entry of a list (a numeric last segment).
    let list = |path: &str| {
        path.rsplit_once('/')
            .filter(|(_, index)| !index.is_empty() && index.chars().all(|c| c.is_ascii_digit()))
            .map(|(list, _)| list.to_string())
    };
    let Some(Binding::Text { path, .. }) = def.bindings.get(key).map(|b| &b.binding) else {
        return false;
    };
    let Some(mine) = list(path) else {
        return false;
    };
    def.bindings.values().any(|bound| {
        matches!(&bound.binding, Binding::Text {
            path,
            date: Some(DateFormat::Mdy | DateFormat::Iso),
            part: None,
            ..
        } if list(path).as_deref() == Some(mine.as_str()))
    })
}

/// How a box is typed on the page: money only where its binding is an
/// amount (rule 7), a date where it holds a whole date (rule 4).
///
/// - A text binding is never money (2550Q's contact number, 1701MS's
///   taxpayer name take what the filer types, never "₱ 281,234,567.00").
/// - An amount binding is a peso box (1606's 17B Interest), a whole-number
///   one keeps the schema's type and, as money, shows whole pesos (`w`):
///   its draft field holds no centavos. Either one holding a count is a
///   number box, and a tax rate a percent box ([`quantity`]).
/// - A text binding that keeps a whole date is a date box, and so is a
///   plain text or unbound box the print sheet prints as a whole date
///   (2200T's payment dates, 2550Q's Return Period From) or a list entry
///   whose sibling keeps one: the page shows it as "Dec 10, 2020" and sends
///   MM/DD/YYYY. A box showing one part of a date stays that part's box.
/// - Unbound and computed boxes otherwise keep the schema's type.
fn page_type(
    def: &AgentFormDef,
    code: &str,
    key: &str,
    schema_type: &str,
    label: &str,
) -> (String, bool) {
    use bir_core::forms::agent::DateFormat;
    let binding = def.bindings.get(key).map(|bound| &bound.binding);
    if matches!(schema_type, "text" | "money" | "int" | "date") {
        let whole_date = matches!(
            binding,
            Some(Binding::Text {
                date: Some(DateFormat::Mdy | DateFormat::Iso),
                part: None,
                ..
            })
        );
        let plain = matches!(
            binding,
            None | Some(Binding::Text {
                date: None,
                part: None,
                ..
            })
        );
        if whole_date
            || (plain && (combs::printed_as_date(code, key) || dated_list_entry(def, key)))
        {
            return ("date".into(), false);
        }
    }
    let kept = (schema_type.to_string(), false);
    match binding {
        Some(Binding::Text { .. }) if schema_type == "money" => ("text".into(), false),
        Some(Binding::Amount { path, .. } | Binding::Whole { path, .. })
            if matches!(schema_type, "text" | "int" | "date" | "money" | "pct") =>
        {
            let whole = matches!(binding, Some(Binding::Whole { .. }));
            match quantity(path, label) {
                Quantity::Count => ("int".into(), false),
                Quantity::TaxRate => ("pct".into(), false),
                Quantity::Amount if whole && schema_type == "money" => ("money".into(), true),
                Quantity::Amount if !whole && schema_type != "pct" => ("money".into(), false),
                Quantity::Amount => kept,
            }
        }
        _ => kept,
    }
}

/// The official 2551Q page has two Item 13 radios. The desktop also offers
/// "Not applicable" (`Item13Election::NotApplicable`). Inject that third
/// radio into the on-screen schema so the page can record it. The upload
/// map is unchanged.
fn inject_2551q_not_applicable(form: &mut Value) {
    let Some(parts) = form["parts"].as_array_mut() else {
        return;
    };
    for part in parts {
        let Some(items) = part["items"].as_array_mut() else {
            continue;
        };
        for item in items {
            let Some(fields) = item["f"].as_array_mut() else {
                continue;
            };
            if !fields
                .iter()
                .any(|field| field["k"] == "frm2551Qv2018:taxRate1")
            {
                continue;
            }
            for field in fields.iter_mut() {
                match field["k"].as_str() {
                    Some("frm2551Qv2018:taxRate1") => {
                        field["l"] = Value::from("Graduated income tax rate");
                    }
                    Some("frm2551Qv2018:taxRate2") => {
                        field["l"] = Value::from("8% income tax rate");
                    }
                    _ => {}
                }
            }
            if !fields
                .iter()
                .any(|field| field["k"] == "frm2551Qv2018:taxRateNA")
            {
                fields.push(json!({
                    "l": "Not applicable",
                    "t": "radio",
                    "k": "frm2551Qv2018:taxRateNA",
                }));
            }
        }
    }
}

/// Each box of a one-answer group carries its group's member count (`rg`,
/// from [`one_answer_groups`]), so the page unticks the ticked box of a
/// group of three or more however the group's boxes are split over items or
/// drawn (2200S's manner of payment is items 15 and 16 as radios and item 17
/// as a tick box). A radio without `rg` is in no group: the page never
/// sends its untick.
fn mark_radio_groups(form: &mut Value, code: &str) {
    let Some(kind) = AgentFormKind::from_code(code) else {
        return;
    };
    let groups = one_answer_groups(kind);
    let fields = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten())
        .flat_map(|item| item["f"].as_array_mut().into_iter().flatten());
    for field in fields {
        if let Some((group, members)) = field["k"].as_str().and_then(|key| groups.get(key)) {
            field["rg"] = Value::from(*members);
            // The group's own id too, so the page sends a pick's "false"
            // only to members of the same host group (review F1): an item
            // can draw boxes of other groups beside it (1701MS's item 11
            // draws the taxpayer's and the spouse's).
            field["grp"] = Value::String(group.clone());
        }
    }
}

/// Apply [`page_type`] to every field, and to every schedule column whose
/// bound cells all agree: the page draws a box as its draft field reads (a
/// count as a whole number, a rate as a percent, an amount as money, whole
/// pesos where the binding is whole), whatever type the schema guessed from
/// the official control.
fn retype_by_binding(form: &mut Value, code: &str) {
    let Some(kind) = AgentFormKind::from_code(code) else {
        return;
    };
    let def = kind.definition();
    let binding = |key: &str| def.bindings.get(key).map(|bound| &bound.binding);
    let items = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten());
    for item in items {
        for field in item["f"].as_array_mut().into_iter().flatten() {
            let (Some(key), Some(t)) = (field["k"].as_str(), field["t"].as_str()) else {
                continue;
            };
            let label = field["l"].as_str().unwrap_or_default().to_string();
            let (t, whole) = page_type(def, code, key, t, &label);
            field["t"] = Value::from(t);
            if whole {
                field["w"] = Value::Bool(true);
            }
        }
        let rows: Vec<Vec<String>> = item["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| {
                row.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|key| key.as_str().map(str::to_string))
                    .collect()
            })
            .collect();
        for (c, col) in item["cols"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(t) = col["t"].as_str().map(str::to_string) else {
                continue;
            };
            let label = col["l"].as_str().unwrap_or_default().to_string();
            let bound: Vec<&String> = rows
                .iter()
                .filter_map(|row| row.get(c))
                .filter(|key| binding(key).is_some())
                .collect();
            let mut typed: BTreeSet<(String, bool)> = bound
                .iter()
                .map(|key| page_type(def, code, key, &t, &label))
                .collect();
            // A date column some of whose rows the binding data leaves as
            // plain text is a date column: plain text keeps the MM/DD/YYYY
            // the page sends.
            let plain_text = |key: &&String| {
                matches!(
                    binding(key),
                    Some(Binding::Text {
                        date: None,
                        part: None,
                        ..
                    })
                )
            };
            if typed.contains(&("date".to_string(), false))
                && bound
                    .iter()
                    .all(|key| plain_text(key) || page_type(def, code, key, &t, &label).0 == "date")
            {
                typed = BTreeSet::from([("date".to_string(), false)]);
            }
            if let [(t, whole)] = typed.into_iter().collect::<Vec<_>>().as_slice() {
                col["t"] = Value::from(t.as_str());
                if *whole {
                    col["w"] = Value::Bool(true);
                }
            }
        }
        // A table column whose every cell is bound to text (2200S's
        // `txtsched1TaxBracket{N}`, /others/N/tax_bracket) is a text column.
        let rows: Vec<Vec<String>> = item["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| {
                row.as_array()
                    .into_iter()
                    .flatten()
                    .map(|key| key.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .collect();
        for (c, col) in item["cols"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let cells: Vec<&String> = rows.iter().filter_map(|row| row.get(c)).collect();
            let text = !cells.is_empty()
                && cells.iter().all(|key| {
                    matches!(
                        def.bindings.get(key.as_str()).map(|bound| &bound.binding),
                        Some(Binding::Text { .. })
                    )
                });
            if text && col["t"] == "money" {
                col["t"] = Value::from("text");
            }
        }
    }
}

/// A box the official form repeats (`X#occurrence-2`, `-3`, …: 2551Q's
/// email, 2200A's TIN, branch and name) is shown once: every later
/// occurrence is hidden. The serializer writes the same value to all of
/// them, so nothing is lost on save or print.
fn hide_repeated_occurrences(form: &mut Value) {
    let Some(parts) = form["parts"].as_array_mut() else {
        return;
    };
    for part in parts.iter_mut() {
        let Some(items) = part["items"].as_array_mut() else {
            continue;
        };
        for item in items.iter_mut() {
            let Some(fields) = item["f"].as_array_mut() else {
                continue;
            };
            for field in fields.iter_mut() {
                let later = field["k"]
                    .as_str()
                    .and_then(|k| k.rsplit_once("#occurrence-"))
                    .and_then(|(_, n)| n.parse::<u32>().ok())
                    .is_some_and(|n| n >= 2);
                if later {
                    field["t"] = Value::from("hidden");
                }
            }
        }
    }
}

/// Apply one form's corrections. Keys are a field key `k` (label, kind,
/// options or computed flag of that field, or of a schedule column when `k`
/// is a table cell)
/// or `item:<part>:<index>` (that item's printed label). `kind: "hidden"`
/// keeps the field for validation placement but the page does not draw it;
/// `why` is what a printed box the filer cannot fill says while blank.
/// The `evidence` text is provenance only.
fn apply_overrides(form: &mut Value, fixes: &serde_json::Map<String, Value>) {
    let set = |target: &mut Value, fix: &Value, kind_key: &str| {
        if let Some(label) = fix.get("label") {
            target["l"] = label.clone();
        }
        if let Some(kind) = fix.get("kind") {
            target[kind_key] = kind.clone();
        }
        if let Some(options) = fix.get("options") {
            target["o"] = options.clone();
        }
        // `why`: what a box the filer cannot fill says while it is blank
        // (a printed box the official program fills elsewhere or this app
        // files blank), so it never looks shut for no reason.
        if let Some(why) = fix.get("why") {
            target["why"] = why.clone();
        }
        // `computed: false` corrects a field the extractor marked computed
        // although the filer chooses it (1606 Item 31).
        if let Some(computed) = fix.get("computed").and_then(Value::as_bool)
            && let Some(object) = target.as_object_mut()
        {
            if computed {
                object.insert("c".into(), Value::from(1));
            } else {
                object.remove("c");
            }
        }
    };
    let Some(parts) = form["parts"].as_array_mut() else {
        return;
    };
    // Tables first, so a cell key's correction (a column's options) finds
    // the table it belongs to.
    for (key, fix) in fixes {
        if let Some(part) = key.strip_prefix("table:") {
            append_table(parts, part, fix);
        }
    }
    // `move:<field key>` with `to: "<part>:<item>"`: a field the extractor
    // grouped under the wrong item moves to the printed item it belongs to.
    // `to: "<part>:new:<no>"` with `item: {l, order}` moves it to printed
    // item `<no>` (one per `<no>` and heading), which the extractor left
    // out, appended at the part's end.
    // The emptied item stays (indexes hold) and the page skips it.
    for (key, fix) in fixes {
        if let Some(field_key) = key.strip_prefix("move:") {
            move_field(parts, field_key, fix);
        }
    }
    // Added items sit after every extracted item, ordered by their `order`
    // (the overrides map's own order is not relied on).
    for part in parts.iter_mut() {
        if let Some(items) = part["items"].as_array_mut() {
            let first = items
                .iter()
                .position(|it| it["added"] == true)
                .unwrap_or(items.len());
            items[first..].sort_by_key(|it| it["order"].as_i64().unwrap_or(i64::MAX));
        }
    }
    for (key, fix) in fixes {
        if key.starts_with("table:") || key.starts_with("move:") {
            continue;
        }
        // `add:<part>:<item>`: a bound control the extracted schema left out,
        // appended to that item (`fields`: `{k, l, t}`).
        if let Some(rest) = key.strip_prefix("add:") {
            let mut at = rest.split(':').filter_map(|n| n.parse::<usize>().ok());
            if let (Some(p), Some(i)) = (at.next(), at.next())
                && let Some(item) = parts.get_mut(p).and_then(|part| part["items"].get_mut(i))
                && let Some(fields) = fix["fields"].as_array()
            {
                match item["f"].as_array_mut() {
                    Some(existing) => existing.extend(fields.iter().cloned()),
                    None => item["f"] = Value::Array(fields.clone()),
                }
            }
            continue;
        }
        if let Some(rest) = key.strip_prefix("item:") {
            let mut indexes = rest.split(':').filter_map(|n| n.parse::<usize>().ok());
            if let (Some(p), Some(i)) = (indexes.next(), indexes.next())
                && let Some(item) = parts.get_mut(p).and_then(|part| part["items"].get_mut(i))
                && let Some(label) = fix.get("label")
            {
                item["l"] = label.clone();
            }
            continue;
        }
        for part in parts.iter_mut() {
            let Some(items) = part["items"].as_array_mut() else {
                continue;
            };
            for item in items.iter_mut() {
                if let Some(fields) = item["f"].as_array_mut() {
                    for field in fields.iter_mut().filter(|f| f["k"] == key.as_str()) {
                        set(field, fix, "t");
                    }
                }
                let column = item["keys"].as_array().and_then(|rows| {
                    rows.iter().find_map(|row| {
                        row.as_array()?
                            .iter()
                            .position(|cell| cell.as_str() == Some(key.as_str()))
                    })
                });
                if let Some(c) = column
                    && let Some(col) = item["cols"].get_mut(c)
                {
                    set(col, fix, "t");
                }
            }
        }
    }
}

/// A `layout` correction: the printed sheet's own structure for a form whose
/// extracted items follow another numbering (the official program's
/// fields.json). `parts: [{n, items: [{no, l, k: [field keys]} |
/// {table: <cell key>}], keep}]` rebuilds the form in that order, after
/// every other correction; a `table` entry takes the whole schedule table
/// holding that cell, and `keep: <part index>` appends that extracted part's
/// items (less what the layout names elsewhere) unchanged, its name too when
/// `n` is absent. Whatever the layout does not name keeps its own label and
/// goes to a last part (`rest`, "Not on the printed sheet") without an item
/// number, so no control is lost. A `k` may name a cell of an extracted
/// table: that cell becomes a field of the item (the table is then taken
/// apart, so a layout naming one of its cells names them all). An item with
/// no control but a `note` is a printed item the official program gives no
/// box; the page shows its caption and the note.
fn apply_layout(form: &mut Value, layout: &Value) {
    let Some(old_parts) = form["parts"].as_array().cloned() else {
        return;
    };
    let specs: Vec<&Value> = layout["parts"].as_array().into_iter().flatten().collect();
    let named_items = || {
        specs
            .iter()
            .flat_map(|p| p["items"].as_array().into_iter().flatten())
    };
    // What the layout places itself: field keys, and every cell of a named table.
    let mut claimed: BTreeSet<String> = named_items()
        .flat_map(|it| {
            it["k"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(grid_keys(it))
        })
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect();
    let table_keys = |item: &Value| -> Vec<String> {
        item["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|row| row.as_array().into_iter().flatten())
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect()
    };
    let mut fields: BTreeMap<String, (Value, Value)> = BTreeMap::new();
    let mut tables: Vec<Value> = Vec::new();
    for part in &old_parts {
        for item in part["items"].as_array().into_iter().flatten() {
            if item.get("rows").is_some() {
                tables.push(item.clone());
            }
            for field in item["f"].as_array().into_iter().flatten() {
                if let Some(key) = field["k"].as_str() {
                    fields.insert(key.to_string(), (field.clone(), item["p"].clone()));
                }
            }
        }
    }
    // A cell the layout names by key leaves its extracted table and takes
    // that column's caption and type as a field (2550Q's Items 42, 47 and
    // 56 "Others (Specify)" lines, which the extractor drew as one table).
    for table in &tables {
        for row in table["keys"].as_array().into_iter().flatten() {
            for (c, cell) in row.as_array().into_iter().flatten().enumerate() {
                let Some(key) = cell.as_str().filter(|key| claimed.contains(*key)) else {
                    continue;
                };
                let col = &table["cols"][c];
                let mut field = json!({"k": key, "l": col["l"].clone(), "t": col["t"].clone()});
                for flag in ["c", "w"] {
                    if col.get(flag).is_some() {
                        field[flag] = col[flag].clone();
                    }
                }
                fields
                    .entry(key.to_string())
                    .or_insert((field, table["p"].clone()));
            }
        }
    }
    // What a `grid` may name: the loose fields, and (PR 6, 1604C/E/F Part
    // II) the cells of an extracted table, each as a field of its column on
    // the table's page. A grid naming no table cell builds what it built.
    let mut grid_fields = fields.clone();
    for table in &tables {
        for row in table["keys"].as_array().into_iter().flatten() {
            for (c, key) in row.as_array().into_iter().flatten().enumerate() {
                let (Some(key), Some(col)) = (key.as_str(), table["cols"].get(c)) else {
                    continue;
                };
                let mut field = json!({ "k": key, "l": col["l"].clone(), "t": col["t"].clone() });
                for flag in ["c", "o", "w"] {
                    if let Some(value) = col.get(flag) {
                        field[flag] = value.clone();
                    }
                }
                grid_fields
                    .entry(key.to_string())
                    .or_insert((field, table["p"].clone()));
            }
        }
    }
    let wanted_tables: Vec<&str> = named_items()
        .filter_map(|it| it["table"].as_str())
        .collect();
    for table in &tables {
        let keys = table_keys(table);
        if keys.iter().any(|k| wanted_tables.contains(&k.as_str())) {
            claimed.extend(keys);
        }
    }
    // An extracted table goes whole: a layout that names only some of its
    // cells would drop the rest from the page, so it is a broken override.
    for table in &tables {
        let keys = table_keys(table);
        let taken = keys.iter().filter(|k| claimed.contains(*k)).count();
        if taken > 0 && taken < keys.len() {
            panic!(
                "{}: schema-overrides.json layout names {taken} of the {} cells of table {:?} (first cell {}); name every cell or none",
                form["c"].as_str().unwrap_or("?"),
                keys.len(),
                table["l"].as_str().unwrap_or(""),
                keys[0]
            );
        }
    }
    let mut kept = BTreeSet::new();
    // An extracted item less what the layout claims; None when nothing is left.
    let remainder = |item: &Value, claimed: &BTreeSet<String>, number: bool| -> Option<Value> {
        if item.get("rows").is_some() {
            let keys = table_keys(item);
            if keys.iter().any(|k| claimed.contains(k)) {
                return None;
            }
            // A table whose every column an override hides (0619F's Part
            // III, which the official program keeps disabled) is no table
            // on the page: its cells stay in the form as hidden boxes.
            let cols = item["cols"].as_array().cloned().unwrap_or_default();
            if !cols.is_empty() && cols.iter().all(|col| col["t"] == "hidden") {
                let mut hidden = item.clone();
                let object = hidden.as_object_mut()?;
                for gone in ["rows", "cols", "keys", "start"] {
                    object.remove(gone);
                }
                hidden["f"] = Value::Array(
                    keys.iter()
                        .map(|key| json!({ "k": key, "l": key, "t": "hidden" }))
                        .collect(),
                );
                if !number {
                    hidden["no"] = Value::from("");
                }
                return Some(hidden);
            }
            return Some(item.clone());
        }
        let left: Vec<Value> = item["f"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| f["k"].as_str().is_some_and(|k| !claimed.contains(k)))
            .cloned()
            .collect();
        if left.is_empty() {
            return None;
        }
        let mut item = item.clone();
        item["f"] = Value::Array(left);
        if !number {
            item["no"] = Value::from("");
        }
        Some(item)
    };
    let mut parts = Vec::new();
    for spec in &specs {
        let mut items = Vec::new();
        for entry in spec["items"].as_array().into_iter().flatten() {
            if let Some(cell) = entry["table"].as_str() {
                if let Some(table) = tables
                    .iter()
                    .find(|t| table_keys(t).iter().any(|k| k == cell))
                {
                    items.push(table.clone());
                }
                continue;
            }
            if entry.get("grid").is_some() {
                // A grid that cannot be built is a broken override, never a
                // silently missing table (its keys are claimed already).
                let table = grid_table(entry, &grid_fields).unwrap_or_else(|error| {
                    panic!(
                        "{}: schema-overrides.json layout grid {}: {error}",
                        form["c"].as_str().unwrap_or("?"),
                        entry["l"]
                    )
                });
                items.push(table);
                continue;
            }
            // A printed item with no box in the official program (1601EQ's
            // item 24): its number and caption, and the note saying so.
            if let Some(note) = entry["note"].as_str() {
                items.push(json!({ "no": entry["no"].clone(), "l": entry["l"].clone(), "p": entry["p"].clone(), "f": [], "note": note }));
                continue;
            }
            let mut page = Value::Null;
            let chosen: Vec<Value> = entry["k"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|key| fields.get(key).cloned())
                .map(|(field, p)| {
                    if page.is_null() {
                        page = p;
                    }
                    field
                })
                .collect();
            if !chosen.is_empty() {
                items.push(json!({ "no": entry["no"].clone(), "l": entry["l"].clone(), "p": page, "f": chosen }));
            } else if let Some(note) = entry["note"].as_str() {
                // A printed item the official program has no control for
                // (2550Q's Part III payment lines): its caption and why it
                // takes no entry here.
                items.push(json!({ "no": entry["no"].clone(), "l": entry["l"].clone(), "p": entry["p"].clone(), "f": [], "note": note }));
            }
        }
        let mut name = spec["n"].clone();
        if let Some(i) = spec["keep"].as_u64().map(|i| i as usize)
            && let Some(old) = old_parts.get(i)
        {
            kept.insert(i);
            if name.is_null() {
                name = old["n"].clone();
            }
            items.extend(
                old["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|it| remainder(it, &claimed, true)),
            );
        }
        parts.push(json!({ "n": name, "items": items }));
    }
    // The rest, in its extracted order and grouping.
    let rest: Vec<Value> = old_parts
        .iter()
        .enumerate()
        .filter(|(i, _)| !kept.contains(i))
        .flat_map(|(_, part)| part["items"].as_array().cloned().unwrap_or_default())
        .filter_map(|it| remainder(&it, &claimed, false))
        .collect();
    if !rest.is_empty() {
        let name = layout["rest"]
            .as_str()
            .unwrap_or("Not on the printed sheet");
        parts.push(json!({ "n": name, "items": rest }));
    }
    form["parts"] = Value::Array(parts);
}

/// The field keys of a layout `grid` entry, row by row.
fn grid_keys(entry: &Value) -> impl Iterator<Item = &Value> {
    entry["grid"]["keys"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row.as_array().into_iter().flatten())
}

/// A layout `grid` entry: `{no, l, grid: {cols: [captions], keys: [[row
/// keys]], start, row_labels: {caption, rows}}}` becomes one schedule
/// table of those boxes, wherever the extractor put them. Each column
/// takes its first row's type and computed flag (the overrides already
/// applied); `row_labels` are the sheet's preprinted row captions (1603Q
/// Schedule I's recipients). An error when it has no rows, an empty or
/// ragged row, or a key that is not a field of the form.
fn grid_table(entry: &Value, fields: &BTreeMap<String, (Value, Value)>) -> Result<Value, String> {
    let grid = &entry["grid"];
    let keys: Vec<Vec<String>> = grid["keys"]
        .as_array()
        .ok_or("has no `keys` rows")?
        .iter()
        .map(|row| {
            row.as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect()
        })
        .collect();
    let first = keys.first().ok_or("has no rows")?;
    if first.is_empty() || keys.iter().any(|row| row.len() != first.len()) {
        return Err("has an empty row or rows of different lengths".to_string());
    }
    if let Some(key) = keys.iter().flatten().find(|key| !fields.contains_key(*key)) {
        return Err(format!("names `{key}`, which is not a field of the form"));
    }
    let captions = grid["cols"].as_array().cloned().unwrap_or_default();
    let cols: Vec<Value> = first
        .iter()
        .enumerate()
        .map(|(c, key)| {
            let (field, _) = &fields[key];
            let mut col = json!({
                "l": captions.get(c).cloned().unwrap_or_else(|| field["l"].clone()),
                "t": field["t"].clone(),
            });
            for flag in ["c", "o"] {
                if let Some(value) = field.get(flag) {
                    col[flag] = value.clone();
                }
            }
            col
        })
        .collect();
    let mut table = json!({
        "no": entry["no"].clone(),
        "l": entry["l"].clone(),
        "p": fields[&first[0]].1.clone(),
        "cols": cols,
        "rows": keys.len(),
        "keys": keys,
    });
    if let Some(start) = grid["start"].as_u64() {
        table["start"] = Value::from(start);
    }
    if grid.get("row_labels").is_some() {
        table["row_labels"] = grid["row_labels"].clone();
    }
    // `numbered: false`: the row captions stand in for the row numbers
    // (PR 6: the 1604 Part II sheets print no row numbers).
    if grid["numbered"] == false {
        table["numbered"] = Value::Bool(false);
    }
    Ok(table)
}

/// Take the field `key` out of its item and append it to the item `fix.to`
/// names (`<part>:<item>`, or `<part>:new:<no>` for a printed item the
/// extractor left out). Nothing changes when the field or target is missing.
fn move_field(parts: &mut [Value], key: &str, fix: &Value) {
    let to: Vec<&str> = fix["to"].as_str().unwrap_or_default().split(':').collect();
    let Some(p) = to.first().and_then(|n| n.parse::<usize>().ok()) else {
        return;
    };
    let new_no = match to.as_slice() {
        [_, "new", no] => Some(*no),
        _ => None,
    };
    let index = to.get(1).and_then(|n| n.parse::<usize>().ok());
    let Some(items) = parts.get(p).and_then(|part| part["items"].as_array()) else {
        return;
    };
    if new_no.is_none() && index.is_none_or(|i| i >= items.len()) {
        return;
    }
    let mut taken = None;
    for part in parts.iter_mut() {
        for item in part["items"].as_array_mut().into_iter().flatten() {
            let page = item["p"].clone();
            if let Some(fields) = item["f"].as_array_mut()
                && let Some(n) = fields.iter().position(|f| f["k"] == key)
            {
                taken = Some((fields.remove(n), page));
            }
        }
    }
    let Some((field, page)) = taken else {
        return;
    };
    let Some(items) = parts[p]["items"].as_array_mut() else {
        return;
    };
    let i = match new_no {
        Some(no) => match items
            .iter()
            .position(|it| it["added"] == true && it["no"] == no && it["l"] == fix["item"]["l"])
        {
            Some(i) => i,
            None => {
                items.push(json!({
                    "no": no,
                    "l": fix["item"]["l"].clone(),
                    "p": page,
                    "f": [],
                    "added": true,
                    "order": fix["item"]["order"].clone(),
                }));
                items.len() - 1
            }
        },
        None => index.unwrap_or_default(),
    };
    match items[i]["f"].as_array_mut() {
        Some(fields) => fields.push(field),
        None => items[i]["f"] = Value::Array(vec![field]),
    }
}

/// A `table:<part>:<name>` correction: the row-template fields `templates`
/// of part `<part>` (keys with `{index}`, in column order) become a schedule
/// table appended to that part, with rows `rows[0]..=rows[1]` (`{index}`
/// replaced by the row number, in the template key or in its `row_keys`
/// pattern when the official row key differs) and `cols` captions; the
/// template fields are hidden. Appending keeps every existing
/// `item:<part>:<index>` correction pointing at the same item. Each row key
/// must be a box the agent definition binds (a test checks).
fn append_table(parts: &mut [Value], spec: &str, fix: &Value) {
    let Some(at) = spec
        .split(':')
        .next()
        .and_then(|p| p.parse::<usize>().ok())
        .filter(|p| *p < parts.len())
    else {
        return;
    };
    let wanted: Vec<&str> = fix["templates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let (first, last) = (
        fix["rows"][0].as_u64().unwrap_or(1),
        fix["rows"][1].as_u64().unwrap_or(0),
    );
    let captions = fix["cols"].as_array().cloned().unwrap_or_default();
    // Find every template first; hide them only when the table can be built.
    let mut found: BTreeMap<&str, Value> = BTreeMap::new();
    let mut page = Value::Null;
    for item in parts[at]["items"].as_array().into_iter().flatten() {
        for field in item["f"].as_array().into_iter().flatten() {
            let key = field["k"].as_str().unwrap_or_default();
            if let Some(name) = wanted.iter().find(|w| **w == key) {
                found.insert(name, field.clone());
                page = item["p"].clone();
            }
        }
        // A row template can also be a cell of an extracted table (2550Q's
        // Items 42, 47 and 56 popups sit in the Item 42 table's columns);
        // it takes that column's type, and an override hides the column.
        for row in item["keys"].as_array().into_iter().flatten() {
            for (c, cell) in row.as_array().into_iter().flatten().enumerate() {
                let key = cell.as_str().unwrap_or_default();
                if let Some(name) = wanted.iter().find(|w| **w == key) {
                    let col = &item["cols"][c];
                    let mut field = json!({"k": key, "l": col["l"].clone(), "t": col["t"].clone()});
                    if col.get("c").is_some() {
                        field["c"] = col["c"].clone();
                    }
                    found.insert(name, field);
                    page = item["p"].clone();
                }
            }
        }
    }
    let templates: Vec<Value> = wanted.iter().filter_map(|k| found.remove(k)).collect();
    if templates.is_empty() || templates.len() != wanted.len() {
        return;
    }
    let cols: Vec<Value> = templates
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let mut col = json!({
                "l": captions.get(i).cloned().unwrap_or_else(|| field["l"].clone()),
                "t": field["t"].clone(),
            });
            if field.get("c").is_some() {
                col["c"] = field["c"].clone();
            }
            col
        })
        .collect();
    let keys: Vec<Value> = (first..=last)
        .map(|row| {
            Value::Array(
                templates
                    .iter()
                    .map(|field| {
                        let template = field["k"].as_str().unwrap_or_default();
                        let pattern = fix["row_keys"][template].as_str().unwrap_or(template);
                        Value::from(pattern.replace("{index}", &row.to_string()))
                    })
                    .collect(),
            )
        })
        .collect();
    // A concrete row field the extractor listed is a table cell now: drop
    // the loose copy, in whichever part it sits (one control, one place;
    // 2550Q splits its first two schedule rows across two parts). Row
    // templates stay, hidden.
    let cells: BTreeSet<&str> = keys
        .iter()
        .flat_map(|row| row.as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    for item in parts
        .iter_mut()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten())
    {
        if let Some(fields) = item["f"].as_array_mut() {
            fields.retain(|field| !cells.contains(field["k"].as_str().unwrap_or_default()));
            for field in fields.iter_mut() {
                if wanted.contains(&field["k"].as_str().unwrap_or_default()) {
                    field["t"] = Value::from("hidden");
                }
            }
        }
    }
    if let Some(items) = parts[at]["items"].as_array_mut() {
        let mut table = json!({
            "no": fix["no"].clone(),
            "l": fix["label"].clone(),
            "p": page,
            "cols": cols,
            "rows": keys.len(),
            "keys": keys,
        });
        // `start`: the number the first row shows, when a continuation
        // table picks up after the printed rows (1707A's row 4 onward).
        if let Some(start) = fix["start"].as_u64() {
            table["start"] = Value::from(start);
        }
        items.push(table);
    }
}

/// `1601-C`, `form 1601c` -> `1601C`.
fn normalize_code(code: &str) -> String {
    let upper = code.trim().to_ascii_uppercase();
    let upper = upper.strip_prefix("FORM").unwrap_or(&upper);
    upper.chars().filter(char::is_ascii_alphanumeric).collect()
}

fn schema_entry(form_code: &str) -> Option<&'static Value> {
    let wanted = normalize_code(form_code);
    schema()
        .iter()
        .find(|form| form["c"].as_str().map(normalize_code).as_deref() == Some(&wanted))
}

/// Whether the schema marks `key` computed (`c`), as a field or as its
/// table column.
fn schema_computed(form_code: &str, key: &str) -> bool {
    // The schema writes the flag as `1`.
    let flagged = |c: &Value| c.as_bool() == Some(true) || c.as_u64() == Some(1);
    let Some(form) = schema_entry(form_code) else {
        return false;
    };
    form["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array().into_iter().flatten())
        .any(|item| {
            let field = item["f"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|f| f["k"] == key && flagged(&f["c"]));
            let cell = item["keys"].as_array().into_iter().flatten().any(|row| {
                row.as_array()
                    .into_iter()
                    .flatten()
                    .position(|k| k == key)
                    .is_some_and(|c| flagged(&item["cols"][c]["c"]))
            });
            field || cell
        })
}

/// Each field the schema gives a `why` (an override's reviewed reason a
/// printed box cannot be filled here), with that reason.
fn schema_whys(form_code: &str) -> Vec<(String, String)> {
    let Some(form) = schema_entry(form_code) else {
        return Vec::new();
    };
    form["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array().into_iter().flatten())
        .flat_map(|item| item["f"].as_array().into_iter().flatten())
        .filter_map(|field| {
            Some((
                field["k"].as_str()?.to_string(),
                field["why"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// Every control key `k` the schema shows for a form (fields and table cells).
fn schema_keys(form_code: &str) -> &'static BTreeSet<String> {
    static KEYS: OnceLock<BTreeMap<String, BTreeSet<String>>> = OnceLock::new();
    static EMPTY: BTreeSet<String> = BTreeSet::new();
    let keys = KEYS.get_or_init(|| {
        schema()
            .iter()
            .map(|form| {
                let mut keys = BTreeSet::new();
                let items = form["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|part| part["items"].as_array().into_iter().flatten());
                for item in items {
                    for field in item["f"].as_array().into_iter().flatten() {
                        if let Some(key) = field["k"].as_str() {
                            keys.insert(key.to_string());
                        }
                    }
                    for row in item["keys"].as_array().into_iter().flatten() {
                        for key in row.as_array().into_iter().flatten() {
                            if let Some(key) = key.as_str() {
                                keys.insert(key.to_string());
                            }
                        }
                    }
                }
                (normalize_code(form["c"].as_str().unwrap_or_default()), keys)
            })
            .collect()
    });
    keys.get(&normalize_code(form_code)).unwrap_or(&EMPTY)
}

fn screen_form_schema(arguments: &HostArguments) -> HostResult {
    let code = arguments.string(0)?;
    schema_entry(code)
        .cloned()
        .map(json)
        .ok_or_else(|| host_error(format!("no on-screen schema for form {code}")))
}

pub(crate) fn forms_index() -> HostResult {
    Ok(HostValue::Array(
        schema()
            .iter()
            .map(|form| {
                json(json!({
                    "c": form["c"],
                    "t": form["t"],
                    "rev": form["rev"],
                    "freq": form["freq"],
                    "pages": form["pages"],
                    "n": form["n"],
                    "q": form["q"],
                }))
            })
            .collect(),
    ))
}

// ----------------------------------------------------------- open drafts

fn kind_of(form_code: &str) -> Result<AgentFormKind, HostError> {
    AgentFormKind::from_code(form_code).ok_or_else(|| {
        host_error(format!(
            "unknown form `{form_code}`; use one of the 43 BIR form codes, e.g. 1601C"
        ))
    })
}

/// The period a form's page opens for: month 1-12, quarter 1-4, `0` for an
/// annual return, the open-ended key (at least 1) otherwise.
fn page_period(code: &str, period: i64) -> Result<u8, String> {
    let frequency = bir_core::forms::find_form(code)
        .map(|form| form.frequency.clone())
        .unwrap_or(FilingFrequency::Annual);
    let period =
        u8::try_from(period).map_err(|_| format!("{code}: period {period} is out of range"))?;
    match frequency {
        FilingFrequency::Monthly if (1..=12).contains(&period) => Ok(period),
        FilingFrequency::Monthly => Err(format!("{code} is monthly: period must be 1-12")),
        FilingFrequency::Quarterly if (1..=4).contains(&period) => Ok(period),
        FilingFrequency::Quarterly => Err(format!("{code} is quarterly: period must be 1-4")),
        FilingFrequency::Annual => Ok(0),
        FilingFrequency::OpenEnded => Ok(period.max(1)),
    }
}

fn read_error(code: &str, error: impl std::fmt::Display) -> String {
    format!("the stored {code} draft could not be read: {error}")
}

/// The stored generic-queue draft in `period`'s column; none without a
/// store (a Form library sample never reads the taxpayer's returns).
fn stored<F: QueueableForm>(
    db: Option<&Database>,
    tin: &str,
    year: u16,
    period: i64,
) -> Result<Option<F>, String> {
    let Some(db) = db else {
        return Ok(None);
    };
    db.get_queueable_draft::<F>(tin, year, period)
        .map_err(|error| read_error(F::FORM_CODE, error))
}

/// The most recently updated stored draft among `periods` that `keep` accepts.
fn latest<F: QueueableForm>(
    db: Option<&Database>,
    tin: &str,
    year: u16,
    periods: impl IntoIterator<Item = i64>,
    keep: impl Fn(&F) -> bool,
) -> Result<Option<F>, String> {
    let mut best: Option<F> = None;
    for period in periods {
        if let Some(draft) = stored::<F>(db, tin, year, period)?
            && keep(&draft)
            && best
                .as_ref()
                .is_none_or(|current| draft.lifecycle().updated_at > current.lifecycle().updated_at)
        {
            best = Some(draft);
        }
    }
    Ok(best)
}

/// An annual return stored before the generic queue (NULL period column,
/// period key `A`): the first save adopts the row.
fn stored_annual<F: QueueableForm>(
    db: Option<&Database>,
    tin: &str,
    year: u16,
) -> Result<Option<F>, String> {
    match (stored::<F>(db, tin, year, 0)?, db) {
        (Some(draft), _) => Ok(Some(draft)),
        (None, Some(db)) => db
            .get_form_draft_v2::<F>(tin, F::FORM_CODE, year, &FilingPeriod::Annual)
            .map_err(|error| read_error(F::FORM_CODE, error)),
        (None, None) => Ok(None),
    }
}

/// Penalties measured from the Calendar's deadline with the profile's EOPT
/// class; an open Draft shows today's values (the painted `prepare_penalties`).
pub(crate) fn finish<F: QueueableForm>(
    db: &Database,
    profile: &TaxpayerProfile,
    mut draft: F,
) -> Result<Value, String> {
    // A stored 1702RT's items 40, 41, 42 and 57, which this compute may
    // change (maintainer decisions: pre-2020 MCIT is 2% of gross; pre-2020
    // 1702RT computed like BIR).
    let stored = (F::FORM_CODE == "1702RT")
        .then(|| serde_json::to_value(&draft).ok())
        .flatten();
    if !F::penalty_items().is_empty() {
        draft.apply_penalty_profile(profile, &db.get_deadline_overrides());
        if draft.lifecycle().is_editable() {
            draft.compute();
        }
    }
    let mut opened = serde_json::to_value(draft).map_err(|error| error.to_string())?;
    rows_annual_corporate::mark_pre_2020_held(&mut opened, stored.as_ref());
    Ok(opened)
}

fn month_or(period: u8, fallback: u8) -> u8 {
    if (1..=12).contains(&period) {
        period
    } else {
        fallback
    }
}

/// An exact-key open or amend whose return is gone.
const NO_LONGER_STORED: &str =
    "That return is no longer stored. Open the form again from its period or New return.";

/// The draft each painted page opens for `(profile, year, period)`: the
/// stored one, else a new one from the profile. `fresh` (a Form library
/// sample) always builds the new one and reads no stored return.
pub(crate) fn load_draft(
    db: &Database,
    kind: AgentFormKind,
    profile: &TaxpayerProfile,
    year: u16,
    period: u8,
    fresh: bool,
    exact: Option<i64>,
) -> Result<Value, String> {
    use bir_core::forms::*;
    let tin = profile.tin.full();
    let tin = tin.as_str();
    let store = (!fresh).then_some(db);
    // Opened by its stored key, the return must still be stored: a miss is
    // an error, never another return of the period or a new one.
    let exact = exact.filter(|_| store.is_some());
    let queueable = match kind {
        AgentFormKind::Form1601C => {
            return out_of_line(|| {
                let month = period.clamp(1, 12);
                let draft = match store
                    .map(|db| db.get_1601c_draft(tin, year, month))
                    .transpose()
                    .map_err(|error| read_error("1601C", error))?
                    .flatten()
                {
                    Some(draft) => draft,
                    None if exact.is_some() => return Err(NO_LONGER_STORED.to_string()),
                    None => form_1601c::Form1601CDraft::new_unanswered_from_profile(
                        profile, year, month,
                    ),
                };
                // Penalties run from the Calendar's deadline, the stored
                // overrides applied, as every other form's (`finish`); a
                // moved deadline recomputes them.
                let mut draft = draft;
                if draft.status == FilingStatus::Draft {
                    let before = draft.penalty.penalty_due_date;
                    draft.apply_penalty_profile(profile, &db.get_deadline_overrides());
                    if draft.penalty.penalty_due_date != before {
                        draft.compute();
                    }
                }
                serde_json::to_value(draft).map_err(|error| error.to_string())
            });
        }
        AgentFormKind::Form2551Q => {
            return out_of_line(|| {
                let quarter = period.clamp(1, 4);
                let mut draft = match store
                    .map(|db| db.get_2551q_draft(tin, year, quarter))
                    .transpose()
                    .map_err(|error| read_error("2551Q", error))?
                    .flatten()
                {
                    Some(draft) => draft,
                    None if exact.is_some() => return Err(NO_LONGER_STORED.to_string()),
                    None => form_2551q::Form2551QDraft::new_from_profile(profile, year, quarter),
                };
                // A later quarter takes the Item 13 answer of its filed Q1
                // in this app, and is locked to it (`draft_locks::item_13`).
                if let Some(db) = store {
                    agent::draft_locks::item_13::refresh(db, &mut draft)
                        .map_err(|error| read_error("2551Q", error))?;
                }
                // Its penalties run from the Calendar's deadline for the
                // quarter, the stored overrides applied (Q3 2026: Oct 26); a
                // moved deadline recomputes them.
                if draft.status == FilingStatus::Draft {
                    let before = draft.penalty_due_date;
                    draft.apply_penalty_profile(profile, &db.get_deadline_overrides());
                    if draft.penalty_due_date != before {
                        draft.recompute(None);
                    }
                }
                serde_json::to_value(draft).map_err(|error| error.to_string())
            });
        }
        AgentFormKind::Queueable(kind) => kind,
    };
    // The one stored return under `exact` (a transaction's `MMDD`, 2553's
    // quarter) rather than the latest of the year; none stored, a new one.
    if let (Some(key), Some(store)) = (exact, store)
        && queueable != QueueableKind::Form2550Q
    {
        let found = bir_core::with_queueable_kind!(queueable, F => out_of_line(|| {
            Ok::<_, String>(match stored::<F>(Some(store), tin, year, key)? {
                Some(found) => Some(finish(db, profile, found)?),
                None => None,
            })
        })?);
        return found.ok_or_else(|| NO_LONGER_STORED.to_string());
    }
    let column = i64::from(period);
    macro_rules! simple {
        ($ty:ty, $new:expr) => {
            out_of_line(|| {
                let draft: $ty = stored::<$ty>(store, tin, year, column)?.unwrap_or_else(|| $new);
                finish(db, profile, draft)
            })
        };
    }
    match queueable {
        QueueableKind::Form2550Q => out_of_line(|| {
            let quarter = period.clamp(1, 4);
            let draft = store
                .map(|db| {
                    db.get_form_draft::<form_2550q::Form2550QDraft>(
                        tin,
                        "2550Q",
                        year,
                        Some(quarter),
                    )
                })
                .transpose()
                .map_err(|error| read_error("2550Q", error))?
                .flatten();
            if draft.is_none() && exact.is_some() {
                return Err(NO_LONGER_STORED.to_string());
            }
            let mut draft = draft.unwrap_or_else(|| {
                form_2550q::Form2550QDraft::new_from_profile(profile, year, quarter)
            });
            bir_core::form_rules::prepare_form_2550q_live_row_identities(&mut draft)
                .map_err(|error| format!("the 2550Q draft's row identities are unsafe: {error}"))?;
            serde_json::to_value(draft).map_err(|error| error.to_string())
        }),
        QueueableKind::Form0605 => out_of_line(|| {
            type F = form_0605::Form0605Draft;
            let slot = period.max(1);
            let draft = match (stored::<F>(store, tin, year, i64::from(slot))?, store) {
                (Some(draft), _) => Some(draft),
                (None, Some(db)) => db
                    .get_form_draft_v2::<F>(
                        tin,
                        "0605",
                        year,
                        &FilingPeriod::OpenEnded(u32::from(slot)),
                    )
                    .map_err(|error| read_error("0605", error))?,
                (None, None) => None,
            }
            .unwrap_or_else(|| F::new_from_profile(profile, year, slot));
            finish(db, profile, draft)
        }),
        QueueableKind::Form0619E => simple!(
            form_0619e::Form0619EDraft,
            form_0619e::Form0619EDraft::new_from_profile(profile, year, period)
        ),
        QueueableKind::Form0619F => simple!(
            form_0619f::Form0619FDraft,
            form_0619f::Form0619FDraft::new_from_profile(profile, year, period)
        ),
        QueueableKind::Form1600PT => simple!(
            form_1600pt::Form1600PtDraft,
            form_1600pt::Form1600PtDraft::new_from_profile(profile, year, period.clamp(1, 12))
        ),
        QueueableKind::Form1600VT => simple!(
            form_1600vt::Form1600VtDraft,
            form_1600vt::Form1600VtDraft::new_from_profile(profile, year, period.clamp(1, 12))
        ),
        QueueableKind::Form1600WP => simple!(
            form_1600wp::Form1600WpDraft,
            form_1600wp::Form1600WpDraft::new_from_profile(profile, year, period.clamp(1, 12))
        ),
        QueueableKind::Form1601EQ => simple!(
            form_1601eq::Form1601EqDraft,
            form_1601eq::Form1601EqDraft::new_from_profile(profile, year, period.clamp(1, 4))
        ),
        QueueableKind::Form1601FQ => simple!(
            form_1601fq::Form1601FqDraft,
            form_1601fq::Form1601FqDraft::new_from_profile(profile, year, period.clamp(1, 4))
        ),
        QueueableKind::Form1602Q => simple!(
            form_1602q::Form1602QDraft,
            form_1602q::Form1602QDraft::new_from_profile(profile, year, period.clamp(1, 4))
        ),
        QueueableKind::Form1603Q => simple!(
            form_1603q::Form1603QDraft,
            form_1603q::Form1603QDraft::new_from_profile(profile, year, period.clamp(1, 4))
        ),
        QueueableKind::Form1604C => simple!(
            form_1604c::Form1604cDraft,
            form_1604c::Form1604cDraft::new_from_profile(profile, year)
        ),
        QueueableKind::Form1604E => simple!(
            form_1604e::Form1604eDraft,
            form_1604e::Form1604eDraft::new_from_profile(profile, year)
        ),
        QueueableKind::Form1604F => simple!(
            form_1604f::Form1604fDraft,
            form_1604f::Form1604fDraft::new_from_profile(profile, year)
        ),
        QueueableKind::Form1606 => simple!(
            form_1606::Form1606Draft,
            form_1606::Form1606Draft::new_from_profile(profile, year, u32::from(period.max(1)))
        ),
        QueueableKind::Form1700 => simple!(
            form_1700::Form1700Draft,
            form_1700::Form1700Draft::new_from_profile(profile, year)
        ),
        QueueableKind::Form1701 => out_of_line(|| {
            type F = form_1701::Form1701Draft;
            let draft = stored_annual::<F>(store, tin, year)?.unwrap_or_else(|| {
                let mut draft = F::new_from_profile(profile, year, 12);
                draft.recompute();
                draft
            });
            finish(db, profile, draft)
        }),
        QueueableKind::Form1701A => simple!(
            form_1701a::Form1701ADraft,
            form_1701a::Form1701ADraft::new_from_profile(profile, year)
        ),
        QueueableKind::Form1701MS => simple!(
            form_1701ms::Form1701MsDraft,
            form_1701ms::Form1701MsDraft::new_from_profile(profile, year)
        ),
        QueueableKind::Form1701Q => simple!(form_1701q::Form1701QDraft, {
            let mut draft =
                form_1701q::Form1701QDraft::new_from_profile(profile, year, period.clamp(1, 3));
            draft.recompute();
            draft
        }),
        QueueableKind::Form1702EX => simple!(
            form_1702ex::Form1702ExDraft,
            form_1702ex::Form1702ExDraft::new_from_profile(profile, year)
        ),
        QueueableKind::Form1702MX => out_of_line(|| {
            type F = form_1702mx::Form1702MXDraft;
            let draft = stored_annual::<F>(store, tin, year)?.unwrap_or_else(|| {
                let mut draft = F::new_from_profile(profile, year);
                draft.recompute();
                draft
            });
            finish(db, profile, draft)
        }),
        QueueableKind::Form1702Q => simple!(form_1702q::Form1702qDraft, {
            let quarter = if (1..=3).contains(&period) { period } else { 1 };
            form_1702q::Form1702qDraft::new_from_profile(profile, year, quarter)
        }),
        QueueableKind::Form1702RT => out_of_line(|| {
            type F = form_1702rt::Form1702RTDraft;
            let draft = stored_annual::<F>(store, tin, year)?.unwrap_or_else(|| {
                let mut draft = F::new_from_profile(profile, year, 12);
                draft.part_iv.item_40_income_tax_rate_percent = 30;
                draft.deduction_method = form_1702rt::Form1702RTDeductionMethod::Itemized;
                draft.recompute();
                draft
            });
            finish(db, profile, draft)
        }),
        QueueableKind::Form1706 => simple!(
            form_1706::Form1706Draft,
            form_1706::Form1706Draft::new_from_profile(profile, year, u32::from(period.max(1)))
        ),
        QueueableKind::Form1707 => simple!(
            form_1707::Form1707Draft,
            form_1707::Form1707Draft::new_from_profile(profile, year, u32::from(period.max(1)))
        ),
        QueueableKind::Form1707A => simple!(
            form_1707a::Form1707ADraft,
            form_1707a::Form1707ADraft::new_from_profile(profile, year, u32::from(period.max(1)))
        ),
        QueueableKind::Form1800 => out_of_line(|| {
            type F = form_1800::Form1800Draft;
            let mut draft = stored::<F>(store, tin, year, column)?
                .unwrap_or_else(|| F::new_from_profile(profile, year, u32::from(period.max(1))));
            // A TIN fixed in the profile reaches a still-editable draft.
            if draft.lifecycle().is_editable() {
                draft.tin = tin.to_string();
            }
            finish(db, profile, draft)
        }),
        QueueableKind::Form1801 => out_of_line(|| {
            type F = form_1801::Form1801Draft;
            let mut draft = stored::<F>(store, tin, year, column)?
                .unwrap_or_else(|| F::new_from_profile(profile, year, u32::from(period.max(1))));
            if draft.lifecycle().is_editable() {
                draft.tin = tin.to_string();
            }
            finish(db, profile, draft)
        }),
        QueueableKind::Form2000 => simple!(
            form_2000::Form2000Draft,
            form_2000::Form2000Draft::new_from_profile(profile, year, period.clamp(1, 12))
        ),
        QueueableKind::Form2000OT => out_of_line(|| {
            // One return per transaction date (`MMDD`): this year's latest
            // editable draft, else a new transaction.
            type F = form_2000ot::Form2000OTDraft;
            let draft = latest::<F>(store, tin, year, mmdd_keys(0..=12, 0..=31), |draft| {
                draft.lifecycle().is_editable()
            })?
            .unwrap_or_else(|| F::new_from_profile(profile, year));
            finish(db, profile, draft)
        }),
        QueueableKind::Form2200A => simple!(
            form_2200a::Form2200ADraft,
            form_2200a::Form2200ADraft::new_from_profile(profile, year, period)
        ),
        QueueableKind::Form2200AN => out_of_line(|| {
            type F = form_2200an::Form2200ANDraft;
            let month = i64::from(period.clamp(1, 12));
            let draft = latest::<F>(store, tin, year, mmdd_keys(month..=month, 0..=31), |_| true)?
                .unwrap_or_else(|| F::new_from_profile(profile, year, period));
            finish(db, profile, draft)
        }),
        QueueableKind::Form2200C => out_of_line(|| {
            type F = form_2200c::Form2200CDraft;
            let months = if (1..=12).contains(&period) {
                i64::from(period)..=i64::from(period)
            } else {
                1..=12
            };
            let draft = latest::<F>(store, tin, year, mmdd_keys(months, 0..=31), |_| true)?
                .unwrap_or_else(|| F::new_for_month(profile, year, period));
            finish(db, profile, draft)
        }),
        QueueableKind::Form2200M => simple!(
            form_2200m::Form2200MDraft,
            form_2200m::Form2200MDraft::new_from_profile(profile, year, period)
        ),
        QueueableKind::Form2200P => simple!(
            form_2200p::Form2200PDraft,
            form_2200p::Form2200PDraft::new_from_profile(profile, year, period)
        ),
        QueueableKind::Form2200S => out_of_line(|| {
            type F = form_2200s::Form2200SDraft;
            let month = i64::from(period.clamp(1, 12));
            let draft = latest::<F>(store, tin, year, mmdd_keys(month..=month, 0..=31), |_| true)?
                .unwrap_or_else(|| F::new_from_profile(profile, year, period));
            finish(db, profile, draft)
        }),
        QueueableKind::Form2200T => simple!(
            form_2200t::Form2200TDraft,
            form_2200t::Form2200TDraft::new_from_profile(profile, year, period)
        ),
        QueueableKind::Form2550M => simple!(
            form_2550m::Form2550MDraft,
            form_2550m::Form2550MDraft::new_from_profile(profile, year, month_or(period, 1))
        ),
        QueueableKind::Form2552 => out_of_line(|| {
            // Keyed by its Item 1 date: this year's latest return still in
            // progress (month/day 0 is a blank component), else a new one.
            type F = form_2552::Form2552Draft;
            let draft = latest::<F>(store, tin, year, mmdd_keys(0..=12, 0..=31), |draft| {
                !matches!(
                    draft.lifecycle().status,
                    FilingStatus::Confirmed | FilingStatus::Paid
                )
            })?
            .unwrap_or_else(|| F::new_from_profile(profile, year, 0, 0));
            finish(db, profile, draft)
        }),
        QueueableKind::Form2553 => out_of_line(|| {
            // The return names its own quarter (Item 3): this year's latest.
            type F = form_2553::Form2553Draft;
            let quarter = if (1..=4).contains(&period) { period } else { 0 };
            let draft = latest::<F>(store, tin, year, 1..=4, |_| true)?
                .unwrap_or_else(|| F::new_from_profile(profile, year, quarter));
            finish(db, profile, draft)
        }),
    }
}

/// Run `run` in a frame of its own. A dispatcher over every form calls
/// each form's arm through this, so in a debug build (no stack colouring)
/// its own frame holds only the arm's result, not every form's typed draft
/// at once; each arm (each closure type) gets its own out-of-line copy.
#[inline(never)]
pub(crate) fn out_of_line<T>(run: impl FnOnce() -> T) -> T {
    run()
}

/// `MMDD` period keys for every month and day in the ranges.
fn mmdd_keys(
    months: std::ops::RangeInclusive<i64>,
    days: std::ops::RangeInclusive<i64>,
) -> impl Iterator<Item = i64> {
    months.flat_map(move |month| days.clone().map(move |day| month * 100 + day))
}

// ------------------------------------------------------- draft evaluation

// The effective lock map and its profile-box machinery moved core-side in
// PR 15 (decision P4-19): `agent::locks`, `agent::FromProfile`,
// `agent::is_profile_box`, `agent::profile_blank`, `agent::profile_keys`
// and `agent::profile_edit_keys` are the one computation `agent::fill`
// also reads, so the assistant refuses exactly the keys this page locks.
// PR 17 moved the draft-dependent layer over it too
// (`agent::draft_locks`, `agent::live_locks`: the row families' relocks
// and the penalty group locks, with the page's penalty groups pinned from
// this page's schema). What stays here is the page's UX around them: the
// penalties Edit (`unlocked`), the lock notes, notices and waits; the
// page's own writes never consult the draft layer.

/// Where a draft keeps `field` (`profile_edits`, `profile_baseline`): at
/// its top level. Every generic draft flattens its lifecycle there, and
/// 1601C and 2551Q keep the field at the top level themselves; only a draft
/// that nests its lifecycle under `lifecycle` (none today) keeps it there.
fn lifecycle_pointer(draft: &Value, field: &str) -> String {
    if draft["lifecycle"].is_object() {
        format!("/lifecycle/{field}")
    } else {
        format!("/{field}")
    }
}

fn profile_edits_pointer(draft: &Value) -> String {
    lifecycle_pointer(draft, "profile_edits")
}

/// The profile boxes the filer edited on this return.
fn profile_edits(draft: &Value) -> BTreeSet<String> {
    draft
        .pointer(&profile_edits_pointer(draft))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|key| key.as_str().map(str::to_string))
        .collect()
}

/// Record that the filer edited profile box `key` on this return.
fn mark_profile_edit(draft: &mut Value, key: &str) {
    let mut keys = profile_edits(draft);
    if keys.insert(key.to_string()) {
        let pointer = profile_edits_pointer(draft);
        set_draft_path(draft, &pointer, json!(keys));
    }
}

/// Which draft the host is talking about: form, TIN and the draft's
/// `created_at`. Every draft the host builds gets its own `created_at`, and
/// every save keeps it, so a never-saved sample and the real return it
/// resembles are two drafts. `None` for a draft without `created_at`: it
/// gets no typed state at all and never shares a key with another.
///
/// Known limit: `created_at` arrives in the client's draft JSON like every
/// other field, so a crafted payload could reuse another draft's. A
/// host-owned draft id would close that; it is future work.
type DraftKey = (String, String, String);

fn draft_key(kind: AgentFormKind, draft: &Value) -> Option<DraftKey> {
    let created = created_at(draft).filter(|created| !created.is_empty())?;
    Some((
        kind.form_code().to_string(),
        draft["tin"].as_str().unwrap_or_default().to_string(),
        created.to_string(),
    ))
}

thread_local! {
    /// The trigger fields this session's typed writes wrote, per draft
    /// (`mark_trigger_date`).
    static TYPED_PARTS: std::cell::RefCell<BTreeMap<DraftKey, BTreeSet<String>>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
    /// The trigger fields a stored draft had typed when it was opened
    /// (its `trigger_parts_typed`; every field if it was dated), each with
    /// the value it held: such a field still counts while it holds that
    /// value (`seed_typed_parts`).
    static SEEDED_PARTS: std::cell::RefCell<BTreeMap<DraftKey, BTreeMap<String, Value>>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

/// An offer: the sample's [`DraftKey`], the year and the `MMDD` key.
type Offer = (DraftKey, u16, i64);

/// The `MMDD` keys `screen_form_sample_period` offered from a day the filer
/// typed, per sample draft. `open_screen_form` spends one once its whole
/// open (the page's JSON included) has succeeded, to date the new return it
/// starts. Process-wide, not per thread: the open's last step may run on
/// another thread. Every key names its sample's own `created_at`, so two
/// samples never share an offer.
fn issued_offers() -> std::sync::MutexGuard<'static, BTreeSet<Offer>> {
    static OFFERS: std::sync::Mutex<BTreeSet<Offer>> = std::sync::Mutex::new(BTreeSet::new());
    OFFERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The trigger fields box `key` writes (one matcher:
/// `event_trigger::trigger_field_of`).
fn trigger_fields_of_box(kind: AgentFormKind, key: &str) -> BTreeSet<&'static str> {
    bound_paths(kind.definition(), key)
        .iter()
        .filter_map(|path| bir_core::forms::event_trigger::trigger_field_of(kind.form_code(), path))
        .collect()
}

/// The trigger fields the return's identity fixes
/// (`event_trigger::identity_trigger_fields`).
fn identity_trigger_fields(kind: AgentFormKind) -> BTreeSet<&'static str> {
    bir_core::forms::event_trigger::identity_trigger_fields(kind.form_code())
}

/// A new draft the page builds for a pre-filled form follows the
/// typed-trigger model (decision P5-7): mark it at creation. A stored row is
/// never marked here.
fn mark_typed_trigger_model(kind: AgentFormKind, draft: &mut Value) {
    if bir_core::forms::event_trigger::PREFILLED_TRIGGER_FORMS.contains(&kind.form_code())
        && draft["id"].is_null()
    {
        let pointer = lifecycle_pointer(draft, "trigger_date_model");
        set_draft_path(draft, &pointer, json!(true));
    }
}

/// A trigger field holds no value (0, empty, null): cleared.
fn is_blank_trigger(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Number(number)) => number.as_f64() == Some(0.0),
        Some(Value::String(text)) => text.trim().is_empty(),
        Some(Value::Object(object)) => object.values().all(|part| is_blank_trigger(Some(part))),
        Some(_) => false,
    }
}

/// The parts a stored row had typed: its `trigger_parts_typed`, and every
/// trigger field if it was dated.
fn stored_typed_parts(kind: AgentFormKind, stored: &Value) -> BTreeSet<String> {
    let fields = bir_core::forms::event_trigger::trigger_paths(kind.form_code());
    let mut parts: BTreeSet<String> = stored["trigger_parts_typed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|part| fields.contains(part))
        .map(str::to_string)
        .collect();
    if stored["trigger_date_entered"] == true {
        parts.extend(fields.iter().map(|field| field.to_string()));
    }
    parts
}

/// The parts of `draft` that count as typed: this session's typed writes,
/// and the parts a stored row had typed (`earlier`, each with its stored
/// value) that still hold that value. A blank field never counts.
fn counted_parts(
    kind: AgentFormKind,
    draft: &Value,
    session: &BTreeSet<String>,
    earlier: &BTreeMap<String, Value>,
) -> BTreeSet<String> {
    bir_core::forms::event_trigger::trigger_paths(kind.form_code())
        .iter()
        .filter(|field| !is_blank_trigger(draft.get(**field)))
        .filter(|field| {
            session.contains(**field)
                || earlier
                    .get(**field)
                    .is_some_and(|value| draft.get(**field) == Some(value))
        })
        .map(|field| field.to_string())
        .collect()
}

/// Set (`Some`) or remove (`None`) one lifecycle field of `draft`.
fn set_lifecycle_field(draft: &mut Value, field: &str, value: Option<Value>) {
    let pointer = lifecycle_pointer(draft, field);
    match value {
        Some(value) => set_draft_path(draft, &pointer, value),
        None => {
            if let Some((parent, leaf)) = pointer.rsplit_once('/')
                && let Some(Value::Object(object)) = draft.pointer_mut(parent)
            {
                object.remove(leaf);
            }
        }
    }
}

/// Write the host's record of `draft`'s typed parts: `trigger_parts_typed`
/// (removed when empty) and `trigger_date_entered`, set only once the parts
/// date the return (`event_trigger::parts_complete`).
fn write_typed_state(kind: AgentFormKind, draft: &mut Value, parts: &BTreeSet<String>) {
    let entered = bir_core::forms::event_trigger::parts_complete(kind.form_code(), parts);
    set_lifecycle_field(
        draft,
        "trigger_parts_typed",
        (!parts.is_empty()).then(|| json!(parts.iter().collect::<Vec<_>>())),
    );
    set_lifecycle_field(draft, "trigger_date_entered", entered.then(|| json!(true)));
}

/// Remember, for this session, the parts a stored draft had typed (see
/// SEEDED_PARTS), so a part edited after a restart completes the date. A
/// stored row with none clears any earlier seed, so no stale one survives.
fn seed_typed_parts(kind: AgentFormKind, stored: &Value) {
    let Some(id) = draft_key(kind, stored) else {
        return;
    };
    let seeded: BTreeMap<String, Value> = stored_typed_parts(kind, stored)
        .into_iter()
        .filter_map(|part| stored.get(&part).cloned().map(|value| (part, value)))
        .collect();
    SEEDED_PARTS.with(|map| {
        let mut map = map.borrow_mut();
        if seeded.is_empty() {
            map.remove(&id);
        } else {
            map.insert(id, seeded);
        }
    });
}

/// Seed an amendment with its filed return's typed parts: every trigger
/// field when the filed return left Draft (it filed that date), else its
/// recorded parts, each with the filed value; then record the amendment's
/// own state from them.
fn seed_amendment_parts(kind: AgentFormKind, filed: &Value, amended: &mut Value) {
    let Some(id) = draft_key(kind, amended) else {
        return;
    };
    let fields = bir_core::forms::event_trigger::trigger_paths(kind.form_code());
    let parts: BTreeSet<String> = if filed["status"] != "Draft" {
        fields.iter().map(|field| field.to_string()).collect()
    } else {
        stored_typed_parts(kind, filed)
    };
    let seeded: BTreeMap<String, Value> = parts
        .into_iter()
        .filter_map(|part| filed.get(&part).cloned().map(|value| (part, value)))
        .collect();
    if seeded.is_empty() {
        // As `seed_typed_parts` does for a stored row: the filed return
        // typed nothing, so no seed from an earlier open of this key may
        // survive, and the amendment carries no typed state of its own.
        SEEDED_PARTS.with(|map| map.borrow_mut().remove(&id));
        write_typed_state(kind, amended, &BTreeSet::new());
        return;
    }
    let session = TYPED_PARTS.with(|map| map.borrow().get(&id).cloned().unwrap_or_default());
    let counted = counted_parts(kind, amended, &session, &seeded);
    SEEDED_PARTS.with(|map| map.borrow_mut().insert(id, seeded));
    write_typed_state(kind, amended, &counted);
}

/// Record that the filer wrote part of this as-needed return's trigger
/// date (its removal, withholding, sale or transaction date, decision D8c)
/// when box `key` writes one of the form's trigger fields; a write that
/// clears the field takes it out again. The return is dated
/// (`trigger_date_entered`) only once the parts that count date it
/// ([`counted_parts`], [`write_typed_state`]): a part left as the
/// constructor pre-filled it (today, or January 1) is not the filer's date.
fn mark_trigger_date(kind: AgentFormKind, draft: &mut Value, key: &str) {
    let written = trigger_fields_of_box(kind, key);
    if written.is_empty() {
        return;
    }
    let Some(id) = draft_key(kind, draft) else {
        return;
    };
    let session = TYPED_PARTS.with(|parts| {
        let mut parts = parts.borrow_mut();
        let typed = parts.entry(id.clone()).or_default();
        for field in &written {
            if is_blank_trigger(draft.get(*field)) {
                typed.remove(*field);
            } else {
                typed.insert(field.to_string());
            }
        }
        typed.clone()
    });
    let earlier = SEEDED_PARTS.with(|map| map.borrow().get(&id).cloned().unwrap_or_default());
    let counted = counted_parts(kind, draft, &session, &earlier);
    write_typed_state(kind, draft, &counted);
}

/// Before any write of a client's draft (save, queue, cancel, a followed
/// save): `trigger_date_model`, `trigger_parts_typed` and
/// `trigger_date_entered` are what the host knows, never what the client
/// sent.
/// - `trigger_date_model` is the stored row's marker; only a creator (a
///   new row of a pre-filled form) sets it. A client can neither add it to
///   a row that has none nor clear it from a row that has it.
/// - No trigger write this session (an unrelated save): the stored row's
///   two typed fields are written back exactly as stored (a legacy row
///   gains no `trigger_parts_typed`), while the draft is that row and its
///   trigger fields hold their stored values.
/// - Otherwise the parts that count are the stored row's typed parts (read
///   from the database, kept while they hold their stored value) and this
///   session's typed writes; the flag follows from them.
///
/// A form without a trigger date, or a draft without `created_at`, stores
/// neither field, so no client value is ever stored.
fn settle_trigger_flag(
    db: &Database,
    kind: AgentFormKind,
    draft: &mut Value,
) -> Result<(), String> {
    let fields = bir_core::forms::event_trigger::trigger_paths(kind.form_code());
    let Some(id) = draft_key(kind, draft).filter(|_| !fields.is_empty()) else {
        write_typed_state(kind, draft, &BTreeSet::new());
        set_lifecycle_field(draft, "trigger_date_model", None);
        return Ok(());
    };
    let stored = stored_return(db, kind, draft)?
        .filter(|stored| draft_key(kind, stored).as_ref() == Some(&id));
    // The marker is host state like the typed record beside it: the stored
    // row's when there is one, a creator's `true` on a new pre-filled row.
    let marker = match &stored {
        Some(stored) => stored.get("trigger_date_model").cloned(),
        None => bir_core::forms::event_trigger::PREFILLED_TRIGGER_FORMS
            .contains(&kind.form_code())
            .then(|| json!(true)),
    };
    set_lifecycle_field(draft, "trigger_date_model", marker);
    let session = TYPED_PARTS.with(|parts| parts.borrow().get(&id).cloned());
    if session.is_none()
        && let Some(stored) = &stored
        && fields
            .iter()
            .all(|field| stored.get(*field) == draft.get(*field))
    {
        for field in ["trigger_parts_typed", "trigger_date_entered"] {
            set_lifecycle_field(draft, field, stored.get(field).cloned());
        }
        return Ok(());
    }
    // The host's own seeds count too (an amendment seeded from its filed
    // return, `screen_form_amend`), then the stored row's.
    let mut earlier: BTreeMap<String, Value> =
        SEEDED_PARTS.with(|map| map.borrow().get(&id).cloned().unwrap_or_default());
    if let Some(stored) = &stored {
        earlier.extend(
            stored_typed_parts(kind, stored)
                .into_iter()
                .filter_map(|part| stored.get(&part).cloned().map(|value| (part, value))),
        );
    }
    let counted = counted_parts(kind, draft, &session.unwrap_or_default(), &earlier);
    write_typed_state(kind, draft, &counted);
    Ok(())
}

/// The record `draft`'s penalty resolution stands on: its typed-state
/// fields and the Calendar period each penalty item resolves from (a typed
/// transaction date moves it). `None` for a form without penalty items.
fn penalty_signature(kind: AgentFormKind, draft: &Value) -> Option<Value> {
    let AgentFormKind::Queueable(queueable) = kind else {
        return None;
    };
    bir_core::with_queueable_kind!(queueable, F => {
        if F::penalty_items().is_empty() {
            return None;
        }
        let typed: F = serde_json::from_value(draft.clone()).ok()?;
        Some(json!({
            "entered": typed.lifecycle().trigger_date_entered,
            "parts": typed.lifecycle().trigger_parts_typed,
            "periods": F::penalty_items()
                .iter()
                .map(|items| (items.deadline_period)(&typed))
                .collect::<Vec<_>>(),
        }))
    })
}

/// Resolve `draft`'s penalty inputs from the profile and the Calendar the
/// way its page's open does (`finish` → `apply_penalty_profile`), so a
/// fill that moved its trigger record — or a draft a save or queue cannot
/// trust — never keeps the due date an older state resolved (decision
/// P5-9). A failed resolution leaves the draft as it is.
fn reapply_penalty_context(
    db: &Database,
    kind: AgentFormKind,
    draft: &mut Value,
) -> Result<(), String> {
    let Some(tin) = draft["tin"].as_str().map(str::to_string) else {
        return Ok(());
    };
    let Some(profile) = db
        .get_profile_by_tin(&tin)
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let overrides = db.get_deadline_overrides();
    bir_core::forms::agent::apply_penalty_profile(kind, draft, &profile, &overrides)
}

/// How a profile note names a box's two values.
#[derive(Clone, Copy)]
enum NoteStyle {
    /// It followed the profile: "RDO 050 → 043".
    Followed,
    /// It differs from the profile, which was not applied: "RDO 039
    /// (profile 043)".
    Differs,
}

/// How a profile box reads in a note at the top of a draft: its item's
/// caption without the parenthetical or a trailing "code" ("RDO Code"
/// reads "RDO"), and what the page shows (`before`) and what the profile
/// gives (`after`).
fn profile_change_note(
    code: &str,
    key: &str,
    before: &Value,
    after: &Value,
    style: NoteStyle,
) -> Option<String> {
    let form = schema_entry(code)?;
    let (item, field) = form["parts"]
        .as_array()?
        .iter()
        .flat_map(|part| part["items"].as_array().into_iter().flatten())
        .find_map(|item| {
            item["f"]
                .as_array()?
                .iter()
                .find(|field| field["k"] == key)
                .map(|field| (item, field))
        })?;
    let caption = item["l"].as_str().unwrap_or_default();
    let caption = caption.split(" (").next().unwrap_or(caption).trim();
    let caption = caption
        .strip_suffix(" Code")
        .or_else(|| caption.strip_suffix(" code"))
        .unwrap_or(caption);
    let text = |values: &Value| values[key].as_str().unwrap_or_default().trim().to_string();
    let (was, now) = (text(before), text(after));
    if matches!(field["t"].as_str(), Some("radio" | "chk")) {
        // A radio reads as the choice it moved to.
        if now != "true" {
            return None;
        }
        let picked = field["l"].as_str().unwrap_or(key);
        return Some(match style {
            NoteStyle::Followed => format!("{caption} → {picked}"),
            NoteStyle::Differs => format!("{caption} (profile {picked})"),
        });
    }
    let shown = |text: &str| {
        if text.is_empty() {
            "blank".to_string()
        } else {
            text.to_string()
        }
    };
    Some(match style {
        NoteStyle::Followed => format!("{caption} {} → {}", shown(&was), shown(&now)),
        NoteStyle::Differs => format!("{caption} {} (profile {})", shown(&was), shown(&now)),
    })
}

/// The draft fields the form's profile boxes write (period fields aside).
fn profile_paths(kind: AgentFormKind) -> BTreeSet<&'static str> {
    let def = kind.definition();
    def.boxes
        .iter()
        .filter(|entry| agent::is_profile_box(def, entry))
        .flat_map(|entry| bound_paths(def, &entry.key))
        .filter(|path| !is_period_path(path))
        .collect()
}

/// What the profile makes in each of those fields (`from_profile`: the
/// draft the profile alone builds): a draft's `profile_baseline`.
fn profile_values(kind: AgentFormKind, from_profile: &Value) -> Map<String, Value> {
    profile_paths(kind)
        .into_iter()
        .filter_map(|path| Some((path.to_string(), from_profile.pointer(path)?.clone())))
        .collect()
}

/// Whether a Draft records the profile values its boxes were built from.
fn has_profile_baseline(draft: &Value) -> bool {
    draft
        .pointer(&lifecycle_pointer(draft, "profile_baseline"))
        .and_then(Value::as_object)
        .is_some_and(|baseline| !baseline.is_empty())
}

/// Record what the profile makes now as the draft's baseline (in the draft
/// only: the next save keeps it).
fn set_profile_baseline(kind: AgentFormKind, draft: &mut Value, from_profile: &Value) {
    let values = profile_values(kind, from_profile);
    if !values.is_empty() {
        let pointer = lifecycle_pointer(draft, "profile_baseline");
        set_draft_path(draft, &pointer, Value::Object(values));
    }
}

/// The form's profile boxes (their keys).
fn profile_box_keys(kind: AgentFormKind) -> Vec<&'static String> {
    let def = kind.definition();
    def.boxes
        .iter()
        .filter(|entry| agent::is_profile_box(def, entry))
        .map(|entry| &entry.key)
        .collect()
}

/// The stored profile fact profile box `key` shows, by the draft field it
/// writes: a branch fact (`rdo_code`, `registered_address`, `zip_code`,
/// `phone`, `email`) or the taxpayer's `line_of_business`. `None` for the
/// rest (names, a line of a split address): those change in the profile.
fn profile_fact(kind: AgentFormKind, key: &str) -> Option<&'static str> {
    let def = kind.definition();
    match def.bindings.get(key).map(|bound| &bound.binding)? {
        Binding::Text { path, .. } => match path.as_str() {
            "/rdo_code" => Some("rdo_code"),
            "/registered_address" => Some("registered_address"),
            "/zip_code" => Some("zip_code"),
            "/contact_number" => Some("phone"),
            "/email" | "/email_address" => Some("email"),
            "/line_of_business" => Some("line_of_business"),
            _ => None,
        },
        _ => None,
    }
}

/// The draft fields of the profile boxes the filer edited on this return
/// (`profile_edits`; a pooled address and a radio group count as one box).
fn edited_profile_paths(kind: AgentFormKind, draft: &Value) -> BTreeSet<&'static str> {
    let def = kind.definition();
    let edits = profile_edits(draft);
    let pools = combs::pools(kind.form_code());
    let edited = |key: &str| {
        edits.contains(key)
            || pools.values().any(|pool| {
                pool.keys.iter().any(|k| k == key) && pool.keys.iter().any(|k| edits.contains(k))
            })
            || def.box_def(key).is_some_and(|entry| {
                entry.group.as_deref().is_some_and(|group| {
                    edits.contains(group)
                        || def
                            .box_def(group)
                            .is_some_and(|group| group.members.iter().any(|m| edits.contains(m)))
                })
            })
    };
    profile_box_keys(kind)
        .into_iter()
        .filter(|key| edited(key))
        .flat_map(|key| bound_paths(def, key))
        .collect()
}

/// The profile notes for `before` -> `after`, one per box whose shown
/// value differs, in `style`.
fn profile_notes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    style: NoteStyle,
) -> Result<Vec<String>, String> {
    let code = kind.form_code();
    let mut before = before.clone();
    agent::compute(kind, &mut before)?;
    let before = values_json(kind, &before)?;
    let mut after = after.clone();
    agent::compute(kind, &mut after)?;
    let after = values_json(kind, &after)?;
    let mut notes: Vec<String> = Vec::new();
    for key in profile_box_keys(kind) {
        if before[key.as_str()] == after[key.as_str()] {
            continue;
        }
        if let Some(note) = profile_change_note(code, key, &before, &after, style)
            && !notes.contains(&note)
        {
            notes.push(note);
        }
    }
    Ok(notes)
}

/// The draft with every profile box the filer did not edit set to what the
/// profile makes now (`from_profile`), and whether any value changed.
fn with_profile_values(kind: AgentFormKind, draft: &Value, from_profile: &Value) -> (Value, bool) {
    let kept = edited_profile_paths(kind, draft);
    let now = profile_values(kind, from_profile);
    let mut next = draft.clone();
    let mut moved = false;
    for (path, want) in now.iter().filter(|(path, _)| !kept.contains(path.as_str())) {
        if next.pointer(path) != Some(want) {
            set_draft_path(&mut next, path, want.clone());
            moved = true;
        }
    }
    (next, moved)
}

/// A Draft with no baseline (saved before baselines existed, or reverted
/// to Draft: an amended copy, a cancelled queue) is never changed on open.
/// Its profile boxes that differ from the profile now, and that the filer
/// did not edit on it, are only listed ("RDO 039 (profile 043)"), for the
/// page's "Use profile values"; the profile as it is now becomes its
/// baseline (in the draft: the next save keeps it, so an open writes
/// nothing). Nothing for a draft that has a baseline or left Draft.
fn profile_differs(
    kind: AgentFormKind,
    draft: &mut Value,
    from_profile: &Value,
) -> Result<Vec<String>, String> {
    if agent::identity(kind, draft)?.status != FilingStatus::Draft || has_profile_baseline(draft) {
        return Ok(Vec::new());
    }
    let (applied, moved) = with_profile_values(kind, draft, from_profile);
    let notes = if moved {
        profile_notes(kind, draft, &applied, NoteStyle::Differs)?
    } else {
        Vec::new()
    };
    set_profile_baseline(kind, draft, from_profile);
    Ok(notes)
}

/// "Use profile values": every profile box the filer did not edit takes
/// what the profile makes now, and the profile becomes the baseline, so
/// those boxes follow later changes too.
fn use_profile_values(
    kind: AgentFormKind,
    draft: &mut Value,
    from_profile: &Value,
) -> Result<(), String> {
    let (applied, _) = with_profile_values(kind, draft, from_profile);
    *draft = applied;
    set_profile_baseline(kind, draft, from_profile);
    Ok(())
}

/// "Use profile value" on profile boxes the filer changed on this return
/// (`keys`, with the pooled address lines and radios that go with them):
/// they are no longer the filer's edits, so they take what the profile
/// makes now and follow later profile changes again. Other boxes are left
/// as they are.
fn use_profile_values_for(
    kind: AgentFormKind,
    draft: &mut Value,
    from_profile: &Value,
    keys: &BTreeSet<String>,
) {
    let def = kind.definition();
    let pools = combs::pools(kind.form_code());
    let related = |key: &str| {
        keys.contains(key)
            || pools.values().any(|pool| {
                pool.keys.iter().any(|k| k == key) && pool.keys.iter().any(|k| keys.contains(k))
            })
            || def.box_def(key).is_some_and(|entry| {
                entry.group.as_deref().is_some_and(|group| {
                    keys.contains(group)
                        || def
                            .box_def(group)
                            .is_some_and(|group| group.members.iter().any(|m| keys.contains(m)))
                })
            })
    };
    let kept: BTreeSet<String> = profile_edits(draft).into_iter().filter(|key| !related(key)).collect();
    let pointer = profile_edits_pointer(draft);
    set_draft_path(draft, &pointer, json!(kept));
    let now = profile_values(kind, from_profile);
    let paths: BTreeSet<&str> = profile_box_keys(kind)
        .into_iter()
        .filter(|key| related(key))
        .flat_map(|key| bound_paths(def, key))
        .filter(|path| !is_period_path(path))
        .collect();
    let baseline_pointer = lifecycle_pointer(draft, "profile_baseline");
    let mut baseline = draft
        .pointer(&baseline_pointer)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for path in paths {
        if let Some(want) = now.get(path) {
            set_draft_path(draft, path, want.clone());
            baseline.insert(path.to_string(), want.clone());
        }
    }
    if !baseline.is_empty() {
        set_draft_path(draft, &baseline_pointer, Value::Object(baseline));
    }
}

/// A Draft follows a later change of the tax profile in every profile box
/// the filer did not edit on it (`profile_edits`). The draft keeps the
/// profile values its boxes were built from (`profile_baseline`); a box
/// takes the profile's new value (`from_profile`, what the profile alone
/// makes now) only when the profile changed since that baseline AND the
/// box still holds the baseline value, so a value typed or written by any
/// writer that differs from the profile is never replaced. A draft with no
/// baseline is left to [`profile_differs`]. Returns what changed, in the
/// filer's words, for the note at the top of the page ("RDO 050 → 043"),
/// with the baseline moved to the profile as it is now; `None` when no
/// box moved (the baseline is then only moved in the draft, kept by its
/// next save), always on a return that left Draft. The profile is only
/// read, never written.
fn follow_profile(
    kind: AgentFormKind,
    draft: &mut Value,
    from_profile: &Value,
) -> Result<Option<Vec<String>>, String> {
    if agent::identity(kind, draft)?.status != FilingStatus::Draft || !has_profile_baseline(draft) {
        return Ok(None);
    }
    let kept = edited_profile_paths(kind, draft);
    let pointer = lifecycle_pointer(draft, "profile_baseline");
    let baseline = draft
        .pointer(&pointer)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let now = profile_values(kind, from_profile);
    let mut next = draft.clone();
    let mut moved = false;
    for (path, want) in now.iter().filter(|(path, _)| !kept.contains(path.as_str())) {
        let Some(was) = baseline.get(path) else {
            continue;
        };
        if want != was && next.pointer(path) == Some(was) {
            set_draft_path(&mut next, path, want.clone());
            moved = true;
        }
    }
    set_draft_path(&mut next, &pointer, Value::Object(now));
    if !moved {
        *draft = next;
        return Ok(None);
    }
    let notes = profile_notes(kind, draft, &next, NoteStyle::Followed)?;
    *draft = next;
    Ok(Some(notes))
}

/// Every on-screen key that repeats an official control id, bound or not,
/// with that official key. A bound one (2551Q's Item 11 email, keyed
/// `txtEmail#occurrence-1`) writes its draft field like any box; the field
/// map still writes the value once, under the official key, so the page
/// shows it from there.
fn occurrence_keys(kind: AgentFormKind) -> Vec<(&'static String, &'static str)> {
    schema_keys(kind.form_code())
        .iter()
        .filter_map(|key| {
            key.split_once("#occurrence-")
                .map(|(official, _)| (key, official))
        })
        .collect()
}

/// A validation error with the schema keys it names: a radio item's group
/// box becomes its radios, since the page binds each radio by its own key.
/// The page keys a message's field names, in the bindings' order.
fn message_keys(kind: AgentFormKind, field: &str, on_screen: &BTreeSet<String>) -> Vec<String> {
    let def = kind.definition();
    let mut keys: Vec<String> = Vec::new();
    for key in agent::error_keys(kind, field) {
        match def.box_def(&key) {
            Some(entry) if entry.radio_group => keys.extend(entry.members.iter().cloned()),
            _ => keys.push(key),
        }
    }
    // A 2550M schedule row has no draft binding of its own.
    keys.extend(rows_2550m::error_keys(kind, field));
    // 1800/1801 boxes the host writes (PR 10, rows_donor_estate.rs).
    keys.extend(rows_donor_estate::error_keys(kind, field));
    // A penalty item's message names the model's field.
    if keys.is_empty() {
        keys.extend(penalties::error_keys(kind, field));
    }
    // Keys the page cannot bind (not in the schema) are dropped;
    // the error still carries its field and message.
    let mut seen = BTreeSet::new();
    keys.retain(|key| on_screen.contains(key) && seen.insert(key.clone()));
    keys
}

fn errors_json(kind: AgentFormKind, errors: &[(String, String)]) -> Value {
    let on_screen = schema_keys(kind.form_code());
    Value::Array(
        errors
            .iter()
            .map(|(field, message)| {
                let mut keys = message_keys(kind, field, on_screen);
                rows_as_needed::order_error_keys(kind, &mut keys);
                // The printed item numbers where BIR's message names its
                // program's own (decision D6; PR 9's, PR 10's and PR 11's forms).
                let message = rows_as_needed::printed_message(kind, message)
                    .or_else(|| rows_donor_estate::printed_message(kind, message))
                    .or_else(|| rows_excise::printed_message(kind, message))
                    .unwrap_or(message);
                // The annual individual returns draw a message under the
                // item it cites (PR 7, rows_annual_individual.rs).
                rows_annual_individual::cited_keys(kind, message, &mut keys);
                json!({"field": field, "message": message, "keys": keys})
            })
            .collect(),
    )
}

/// Every box as official key -> string, plus each duplicate occurrence the
/// page shows (`txtEmail#occurrence-1`) with its official key's value.
fn values_json(kind: AgentFormKind, draft: &Value) -> Result<Value, String> {
    let mut map = agent::field_map(kind, draft)?;
    // PR 9's forms: what the draft keeps where the field map cuts it.
    rows_as_needed::values(kind, draft, &mut map);
    // A row family's row the upload leaves out still shows what the draft
    // holds: 1702RT writes its "(Add more...)" rows only once two or more
    // are saved, but a single row is already the filer's entry.
    let def = kind.definition();
    for entry in def.boxes.iter().filter(|entry| entry.family.is_some()) {
        if map.contains_key(&entry.key) || catalogs::inactive_schedule(kind, &entry.key, draft) {
            continue;
        }
        let path = match def.bindings.get(&entry.key).map(|bound| &bound.binding) {
            Some(
                Binding::Text { path, .. }
                | Binding::Amount { path, .. }
                | Binding::Whole { path, .. },
            ) => path,
            _ => continue,
        };
        let shown = match draft.pointer(path) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            _ => continue,
        };
        map.insert(entry.key.clone(), shown);
    }
    // A profile box the field map leaves empty (2200S's Schedule 1 name
    // header, written only once that sheet prints) shows the draft field it
    // writes, so a locked box never reads blank; the map, XML and print are
    // unchanged.
    for entry in def
        .boxes
        .iter()
        .filter(|entry| agent::is_profile_box(def, entry))
    {
        if map
            .get(&entry.key)
            .is_some_and(|shown| !shown.trim().is_empty())
        {
            continue;
        }
        if let Some(Binding::Text { path, .. }) = def.bindings.get(&entry.key).map(|b| &b.binding)
            && let Some(Value::String(text)) = draft.pointer(path)
            && !text.trim().is_empty()
        {
            map.insert(entry.key.clone(), text.trim().to_string());
        }
    }
    // A box the official runtime stores escape()d (1601EQ's line of
    // business: `Wholesale%20trading`) shows the text the filer typed; the
    // field map, the XML and the print keep the escaped value.
    for (key, bound) in &def.bindings {
        let Binding::Text {
            path,
            date: None,
            part: None,
            ..
        } = &bound.binding
        else {
            continue;
        };
        let Some(Value::String(text)) = draft.pointer(path) else {
            continue;
        };
        let text = text.trim();
        let escaped = bir_core::official_xml::js_escape(text);
        if escaped != text && map.get(key) == Some(&escaped) {
            map.insert(key.clone(), text.to_string());
        }
    }
    // What the page shows where the field map's value is not what the box
    // holds (PR 8: 1702EX's whole name and address, an untyped count).
    rows_annual_corporate::shown(kind, draft, &mut map);
    // 1702RT Item 5: the official select keeps IC010 even while "Other" is
    // not selected (the upload keeps that raw default); the page shows the
    // election, as the print does.
    if kind == AgentFormKind::Queueable(QueueableKind::Form1702RT)
        && let Ok(atc) = serde_json::from_value::<
            bir_core::forms::form_1702rt::Form1702RTAtcSelection,
        >(draft["atc"].clone())
    {
        map.insert(
            "frm1702RT:drpPg1I5AtcOther".into(),
            atc.elected_other_code(),
        );
    }
    // PR 10's forms: what the draft keeps where the field map shows it
    // otherwise (display only).
    rows_donor_estate::values(kind, draft, &mut map);
    for (key, official) in occurrence_keys(kind) {
        if !map.contains_key(key)
            && let Some(value) = map.get(official).cloned()
        {
            map.insert(key.clone(), value);
        }
    }
    // 1701Q's Civil Status, a page box with no official key (PR 5).
    rows_income_q::values(kind, draft, &mut map);
    // 1700, 1701, 1701A and 1701MS: the spouse's TIN boxes and 1700's
    // display fixes (PR 7, rows_annual_individual.rs).
    rows_annual_individual::values(kind, draft, &mut map);
    Ok(Value::Object(
        map.into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    ))
}

/// Whether `draft` is still a Draft. A notice that asks the filer to check
/// something before filing (D5's 1604E rows, PR 8's 1702RT Schedule IV)
/// shows only then: on a return that left Draft it could never clear.
fn is_draft_return(kind: AgentFormKind, draft: &Value) -> bool {
    agent::identity(kind, draft).is_ok_and(|identity| identity.status == FilingStatus::Draft)
}

fn status_name(status: &FilingStatus) -> String {
    format!("{status:?}")
}

/// Stack for form evaluation. The async halves run on the executor's worker
/// threads (512 KB on macOS); a form's validators compile regexes on first
/// use, and in debug builds that alone overflowed the worker stack (2551Q's
/// phone check took the app down with SIGILL).
const EVALUATE_STACK: usize = 16 * 1024 * 1024;

/// Run `work` on a thread of its own with [`EVALUATE_STACK`]. The async
/// halves call it for everything that reads the typed draft: deserializing a
/// large form (1702MX) in a debug build overflows a 512 KB worker stack too,
/// and GPUI Fast runs host futures on those workers.
fn on_large_stack<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    // The worker runs on this thread's penalty clock (a pinned one in
    // tests), so its compute sees the same today.
    let clock = bir_core::forms::penalty_items::capture_clock();
    std::thread::Builder::new()
        .name("screen-form-evaluate".into())
        .stack_size(EVALUATE_STACK)
        .spawn(move || clock.run(work))
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "validation stopped unexpectedly".to_string())
}

/// Whether `draft` is a sent return that is paid: its status says so, or a
/// payment is recorded for it (bir_record_payment, bir_mark_paid).
fn sent_and_paid(db: &bir_core::db::Database, kind: AgentFormKind, draft: &Value) -> bool {
    let Ok(identity) = agent::identity(kind, draft) else {
        return false;
    };
    let year = identity.taxable_year;
    let period = match identity.period {
        FilingPeriod::Monthly(month) => format!("{year}-{month:02}"),
        FilingPeriod::Quarterly(quarter) => format!("{year}-Q{quarter}"),
        FilingPeriod::Annual => year.to_string(),
        FilingPeriod::OpenEnded(_) => return identity.status == FilingStatus::Paid,
    };
    match identity.status {
        FilingStatus::Draft | FilingStatus::Queued => false,
        FilingStatus::Paid => true,
        // Payments recorded outside the return (the old mini app's) are not
        // carried over: only the Paid status counts here.
        _ => {
            let _ = (db, period);
            false
        }
    }
}

/// Recompute and validate an editable draft in place; a filed return is
/// validated on a copy so it keeps the values it was filed with. `checked`:
/// the 1604E rows whose D5 notice the filer marked checked
/// (`rows_1604::checked_rows`, read on the host thread beforehand).
fn evaluate_here(
    kind: AgentFormKind,
    draft: &mut Value,
    checked: &BTreeSet<String>,
    paid: bool,
) -> Result<(Value, Value, FilingStatus), String> {
    let status = agent::identity(kind, draft)?.status;
    let mut errors = if status == FilingStatus::Draft {
        agent::validate(kind, draft)?
    } else {
        agent::validate(kind, &mut draft.clone())?
    };
    // A message naming another item number than the printed one reads the
    // printed number (D6; PR 8's forms).
    rows_annual_corporate::printed_item_numbers(kind, &mut errors);
    // The same for 1604F and 1604C (D6, rows_1604.rs).
    rows_1604::printed_item_numbers(kind, &mut errors);
    let mut errors = errors_json(kind, &errors);
    // A paid return's penalties are settled ([`sent_and_paid`]): what it was
    // filed and paid with stays, and today's date does not make them stale.
    if paid && let Value::Array(list) = &mut errors {
        list.retain(|error| {
            !error["message"]
                .as_str()
                .is_some_and(|message| message.contains("is stale or inconsistent"))
        });
    }
    // A page notice beside the form's own messages (PR 4: a stored 2553 row
    // whose entry is not the one the sheet prints there).
    if let Value::Array(list) = &mut errors {
        list.extend(rows_dst_pt_vat::notices(kind, draft));
        list.extend(rows_annual_corporate::notices(kind, draft));
        list.extend(rows_annual_individual::notices(kind, draft));
        // PR 9's notices: a stored partial date the page shows blank.
        list.extend(rows_as_needed::notices(kind, draft));
        list.extend(rows_1604::notices(kind, draft, checked));
        list.extend(undated_sale_notice(kind, draft));
    }
    Ok((values_json(kind, draft)?, errors, status))
}

/// What an undated 1606 Draft says (decision P4-3: 1606 is as needed,
/// dated by its sale). A 1606 Draft saved before P4-3 holds no
/// `trigger_date_entered`, so it is off Home and the Calendar until the
/// filer enters its sale date; its stored bytes are left as they are.
const UNDATED_SALE_NOTICE: &str = "Enter the sale date to put this return on the calendar.";

/// What an undated pre-filled Draft's penalty boxes say (it computes none:
/// `PenaltyItems::awaits_trigger_date`): "Enter the sale date to compute
/// penalties." on 1606, the form's own trigger on the others. `None` once
/// it is dated or has left Draft.
fn undated_penalty_note(
    kind: AgentFormKind,
    draft: &Value,
    status: &FilingStatus,
) -> Option<String> {
    let code = kind.form_code();
    if !bir_core::forms::event_trigger::PREFILLED_TRIGGER_FORMS.contains(&code)
        || *status != FilingStatus::Draft
        || draft["trigger_date_model"] != true
        || draft["trigger_date_entered"] == true
    {
        return None;
    }
    // The rule for this return's own kind (2552: a primary offering runs from
    // listing, otherwise from collection), shared with the Calendar.
    let trigger = bir_core::forms::event_trigger::due_rule_for(code, draft)
        .or_else(|| bir_core::calendar_rules::event_due_rule(code))?
        .trigger;
    Some(format!(
        "Enter the {} date to compute penalties.",
        trigger.to_lowercase()
    ))
}

fn undated_sale_notice(kind: AgentFormKind, draft: &Value) -> Vec<Value> {
    if kind.form_code() != "1606" || draft["status"] != "Draft" {
        return Vec::new();
    }
    let dated = serde_json::to_string(draft)
        .ok()
        .and_then(|json| bir_core::forms::event_trigger::event_trigger_from_json("1606", &json))
        .is_some();
    if dated {
        return Vec::new();
    }
    let keys: Vec<&String> = kind
        .definition()
        .bindings
        .keys()
        .filter(|key| !trigger_fields_of_box(kind, key).is_empty())
        .collect();
    vec![json!({
        "field": "transaction_date",
        "message": UNDATED_SALE_NOTICE,
        "keys": keys,
    })]
}

// --------------------------------------------------------- host functions

fn draft_argument(arguments: &HostArguments, index: usize) -> Result<Value, HostError> {
    let draft = to_json(arguments.value(index)?);
    if !draft.is_object() {
        return Err(host_error("draft must be an object"));
    }
    Ok(draft)
}

/// The optional list of profile boxes the page has open (see `apply_screen_edits`).
fn unlocked_argument(arguments: &HostArguments, index: usize) -> BTreeSet<String> {
    match arguments.get(index).map(to_json) {
        Some(Value::Array(keys)) => keys
            .iter()
            .filter_map(|key| key.as_str().map(str::to_string))
            .collect(),
        _ => BTreeSet::new(),
    }
}

fn screen_form_open(
    arguments: &HostArguments,
) -> Result<impl Future<Output = HostResult> + Send + use<>, HostError> {
    open_screen_form(arguments, false)
}

/// A Form library opening: the form as a blank sample for the taxpayer. It
/// never reads or replaces a stored return, shows its period and date boxes
/// blank (`blank`) until the filer types one, and is built for next year so
/// no deadline has passed and no penalty is computed. Its draft carries
/// [`SAMPLE_MARK`]: save makes it a return once its period is set
/// ([`save_sample`]); queue and print refuse it until then.
fn screen_form_sample(
    arguments: &HostArguments,
) -> Result<impl Future<Output = HostResult> + Send + use<>, HostError> {
    open_screen_form(arguments, true)
}

/// Marks a Form library sample's draft, with the boxes it holds blank while
/// its period is not set (`{"blank": [...], "set": false}`; `blank` empty
/// and `set` true once the sample is rebuilt for a complete period). The
/// typed draft has no such field, so the host takes it off before any agent
/// call and puts it back on what it returns (`take_sample_mark`,
/// `put_sample_mark`). A sample's period parts are page inputs, never draft
/// writes: no fill lifts a period lock.
const SAMPLE_MARK: &str = "screen_form_sample";

struct SampleMark {
    blank: Vec<String>,
    /// Rebuilt for a complete period: Save makes it that period's return.
    set: bool,
}

/// The sample's mark, or `None` for a return.
fn take_sample_mark(draft: &mut Value) -> Option<SampleMark> {
    let Value::Object(object) = draft else {
        return None;
    };
    match object.remove(SAMPLE_MARK)? {
        Value::Null | Value::Bool(false) => None,
        mark => Some(SampleMark {
            blank: mark["blank"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|key| key.as_str().map(str::to_string))
                .collect(),
            set: mark["set"] == true,
        }),
    }
}

fn put_sample_mark(draft: &mut Value, mark: Option<&SampleMark>) {
    if let (Some(mark), Value::Object(object)) = (mark, draft) {
        object.insert(
            SAMPLE_MARK.into(),
            json!({ "blank": mark.blank, "set": mark.set }),
        );
    }
}

/// Queue and print take a return, never a Form library sample: the page
/// saves a sample first (`screen_form_save`), which makes it a return.
fn refuse_sample(draft: &mut Value, doing: &str) -> Result<(), HostError> {
    if take_sample_mark(draft).is_some() {
        return Err(host_error(format!(
            "a sample from the Form library cannot be {doing}; save it first, which makes it a return"
        )));
    }
    Ok(())
}

fn open_screen_form(
    arguments: &HostArguments,
    sample: bool,
) -> Result<impl Future<Output = HostResult> + Send + use<>, HostError> {
    use chrono::Datelike;
    let tin: String = arguments
        .string(0)?
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let kind = kind_of(arguments.string(1)?)?;
    let code = kind.form_code();
    let (year, period) = if sample {
        // A placeholder period: the sample is rebuilt for the one its filer
        // completes, and holds every period-derived box until then.
        let next = u16::try_from(chrono::Local::now().year() + 1)
            .map_err(|_| host_error("year is out of range"))?;
        (next, page_period(code, 1).map_err(host_error)?)
    } else {
        let year =
            u16::try_from(arguments.integer(2)?).map_err(|_| host_error("year is out of range"))?;
        (
            year,
            page_period(code, arguments.integer(3)?).map_err(host_error)?,
        )
    };
    // The exact stored return to open (an offer's `opens.key`): a
    // transaction's `MMDD`, 2553's quarter; gone, the open fails. Or the key
    // a new return is started under (`opens.new_key`: the typed date of a
    // sample with no stored return).
    let (exact, new_key, new_key_from) = if sample {
        (None, None, None)
    } else {
        (
            arguments.get(4).and_then(|key| to_json(key).as_i64()),
            arguments.get(5).and_then(|key| to_json(key).as_i64()),
            arguments
                .get(6)
                .and_then(|from| to_json(from).as_str().map(str::to_string)),
        )
    };
    // The draft to show; the one the profile alone makes for this period
    // (what decides whether the profile filled a box); and for a sample, one
    // for another year and period (what tells which boxes follow the period).
    let (draft, from_profile, build, filed_original, followed, notes, checked, paid, consumed) =
        with_database(|db| {
            let profile = db
                .get_profile_by_tin(&tin)
                .map_err(|error| host_error(error.to_string()))?
                .ok_or_else(|| host_error(format!("no taxpayer profile with TIN {tin}")))?;
            let mut draft =
                load_draft(db, kind, &profile, year, period, sample, exact).map_err(host_error)?;
            let mut from_profile = if sample {
                draft.clone()
            } else {
                load_draft(db, kind, &profile, year, period, true, None).map_err(host_error)?
            };
            // A stored return the page opens brings the parts it had typed (a
            // date typed over two sessions, a part edited after a restart).
            if !sample && !draft["id"].is_null() {
                seed_typed_parts(kind, &draft);
            }
            // A new draft built here follows the typed-trigger model.
            mark_typed_trigger_model(kind, &mut draft);
            let mut consumed = None;
            // A new return started for the date its key names.
            if let Some(key) = new_key
                && exact.is_none()
                && draft["id"].is_null()
            {
                let parts = sample_period_parts(kind);
                let typed = typed_from_key(year, key);
                // On a form with a trigger date, a key that names a day (`MMDD`)
                // is written, and dates the return, only when this host offered
                // it for the sample this open names (`screen_form_sample_period`;
                // the page passes the sample's `created_at` as `new_key_from`);
                // the offer is consumed once the open has succeeded. A key the
                // client sends alone writes no date at all (D8c). Other forms
                // take the key as before.
                let has_trigger = !bir_core::forms::event_trigger::trigger_paths(code).is_empty();
                let offer = new_key_from
                    .as_ref()
                    .filter(|_| has_trigger && typed.day.is_some())
                    .map(|from| ((code.to_string(), tin.clone(), from.clone()), year, key))
                    .filter(|offer| issued_offers().contains(offer));
                let issued = offer.is_some();
                consumed = offer;
                if !has_trigger || typed.day.is_none() || issued {
                    write_typed_period(kind, &parts, &typed, &mut draft).map_err(host_error)?;
                    write_typed_period(kind, &parts, &typed, &mut from_profile)
                        .map_err(host_error)?;
                }
                if issued {
                    for (part_key, _) in &parts {
                        mark_trigger_date(kind, &mut draft, part_key);
                    }
                }
            }
            // A stored Draft follows the tax profile in every box the filer did
            // not edit on it, saved quietly; the page says what changed.
            // Never in the way of opening it: a follow or save that fails leaves
            // the draft open as it is (or as followed, in memory).
            let followed = if sample {
                None
            } else {
                follow_profile(kind, &mut draft, &from_profile).unwrap_or_else(|error| {
                    tracing::warn!("{code}: following the tax profile failed: {error}");
                    None
                })
            };
            let stored = match followed.as_ref().map(|_| stored_return(db, kind, &draft)) {
                Some(Ok(stored)) => stored.is_some(),
                Some(Err(error)) => {
                    tracing::warn!("{code}: reading the stored return to save it failed: {error}");
                    false
                }
                None => false,
            };
            if stored {
                let mut saved = draft.clone();
                match refuse_replacing_a_stored_return(db, kind, &saved)
                    .and_then(|()| settle_trigger_flag(db, kind, &mut saved))
                    .and_then(|()| agent::save_draft(kind, db, &mut saved))
                {
                    Ok(_) => draft = saved,
                    Err(error) => {
                        tracing::warn!("{code}: saving the followed draft failed: {error}")
                    }
                }
            }
            let followed = followed.unwrap_or_default();
            let build = if sample {
                let period = page_period(code, 2).map_err(host_error)?;
                Some(SampleBuild {
                    other: Some(
                        load_draft(db, kind, &profile, year + 1, period, true, None)
                            .map_err(host_error)?,
                    ),
                })
            } else {
                None
            };
            let filed_original = if sample {
                Value::Null
            } else {
                filed_original(db, kind, &draft).map_err(host_error)?
            };
            // A note a stored return shows once (D3: a filed third-quarter
            // 1701Q charged under October 15), offered until the page records it
            // seen (`screen_form_notes_seen`).
            let notes = if sample {
                Vec::new()
            } else {
                rows_income_q::once_notes(db, kind, &draft)
            };
            // The 1604E rows the filer marked checked (D5).
            let checked = rows_1604::checked_rows(db, kind, &draft);
            let paid = sent_and_paid(db, kind, &draft);
            Ok((
                draft,
                from_profile,
                build,
                filed_original,
                followed,
                notes,
                checked,
                paid,
                consumed,
            ))
        })?;
    Ok(async move {
        let mut opened = on_large_stack(move || {
            opened_json(
                kind,
                year,
                period,
                draft,
                from_profile,
                build,
                checked,
                paid,
            )
        })
        .and_then(|result| result)
        .map_err(host_error)?;
        // The whole open has succeeded: the offer it used is spent.
        if let Some(offer) = consumed {
            issued_offers().remove(&offer);
        }
        opened["filed_original"] = filed_original;
        opened["profile_updates"] = json!(followed);
        opened["notes"] = json!(notes);
        Ok(json(opened))
    })
}

/// How a sample is built: `other`, a draft for another year and period
/// while the filer has not set one (`None` once the sample is rebuilt for a
/// complete period).
struct SampleBuild {
    other: Option<Value>,
}

/// Under a locked box the page shows blank (`lock_notes`), why it is shut,
/// in the filer's words: never a key name or the app's internal reason.
const NOTE_COMPUTED: &str = "Computed by the form.";
const NOTE_PROFILE: &str = "Comes from the tax profile.";
const NOTE_IDENTITY: &str = "Set by the return you opened (its taxpayer and period).";
const NOTE_REPEAT: &str = "Repeats another box on this form; fill it there.";
const NOTE_NOT_YET: &str = "Can't be filled on this page yet.";
/// A box the official page keeps shut from load (PR 7: 1701 Part VIII
/// column B, probe.rs `SHUT_ON_OFFICIAL_PAGE`).
const NOTE_SHUT: &str = "The official form keeps this box shut; it files as 0.00.";
/// 1701Q's (PR 5), 1701's (PR 7), the excise returns' (PR 11) and 1800's and
/// 1801's (P4-4) penalties kept as the filer entered them.
const NOTE_KEPT: &str = "Kept as entered before penalties were computed.";
/// Only for a box a form group is assigned to open (the pinned list).
const NOTE_ASSIGNED: &str = "Can't be filled on this page yet. It will be added for this form.";

/// Shut boxes each assigned to the form group that opens them
/// (`CODE key…: reason | group`), the same list the locked-and-blank
/// invariant pins.
const ASSIGNED_SHUT_BOXES: &str = include_str!("screen_form_module/locked_and_blank.txt");

/// Whether a form group is assigned to open `code`'s box `key`.
fn assigned_to_open(code: &str, key: &str) -> bool {
    ASSIGNED_SHUT_BOXES
        .lines()
        .filter(|line| !line.starts_with('#') && line.contains(" | "))
        .any(|line| {
            let mut words = line.split(' ');
            words.next() == Some(code)
                && words.next().map(|word| word.trim_end_matches(':')) == Some(key)
        })
}

/// A lock's reason as a filer reads it (see the NOTE_* texts), for
/// `code`'s box `key`.
fn lock_note_for(code: &str, key: &str, reason: &str) -> &'static str {
    if rows_withholding::preprinted(code, key) {
        return rows_withholding::PREPRINTED;
    }
    if let Some(note) = rows_annual_corporate::live_lock_note(reason) {
        return note;
    }
    match lock_note(reason) {
        NOTE_NOT_YET if assigned_to_open(code, key) => NOTE_ASSIGNED,
        note => note,
    }
}

/// Each module's "kept as entered" lock reason and the note it shows, one
/// entry per module constant: `lock_note` looks a reason up
/// here by the constant itself, so a module rewording its constant keeps
/// its note. A test fails when a module declares a `KEPT_AS_ENTERED` the
/// table lacks.
const KEPT_NOTES: [(&str, &str, &str); 3] = [
    ("rows_income_q", rows_income_q::KEPT_AS_ENTERED, NOTE_KEPT),
    (
        "rows_annual_individual",
        rows_annual_individual::KEPT_AS_ENTERED,
        NOTE_KEPT,
    ),
    ("rows_excise", rows_excise::KEPT_AS_ENTERED, NOTE_KEPT),
];

/// A lock's reason as a filer reads it (see the NOTE_* texts).
fn lock_note(reason: &str) -> &'static str {
    use bir_core::forms::agent::Lock;
    if let Some((_, _, note)) = KEPT_NOTES.iter().find(|(_, kept, _)| *kept == reason) {
        return note;
    }
    // 2551Q's 8% penalty reasons are already the filer's words.
    if reason == penalties::KEPT_ON_EIGHT_PERCENT {
        return penalties::KEPT_ON_EIGHT_PERCENT;
    }
    if reason == penalties::EIGHT_PERCENT {
        return penalties::EIGHT_PERCENT;
    }
    if reason.starts_with("computed") {
        NOTE_COMPUTED
    } else if reason == Lock::Profile.reason() || reason.contains("from the tax profile") {
        NOTE_PROFILE
    } else if reason == Lock::Identity.reason() {
        NOTE_IDENTITY
    } else if reason.starts_with("repeats `") && reason.ends_with("; fill it there") {
        NOTE_REPEAT
    } else if reason.ends_with(bir_core::forms::agent::probe::SHUT_REASON) {
        NOTE_SHUT
    } else {
        // Unmapped, derived, written by the app, or a field this page
        // cannot write: shut until the form's own work opens it.
        NOTE_NOT_YET
    }
}

/// Boxes whose lock note the page draws whatever the box shows; it draws
/// every other lock note only while its box is blank. A kept penalty
/// group's boxes say "Kept as entered before penalties were computed." over
/// 0.00 too (maintainer decision (kept note over held amounts)), and 1702RT
/// item 42, which always shows an
/// amount, says why it is shut (maintainer decision (pre-2020 MCIT is 2% of gross)).
fn notes_always(kind: AgentFormKind, draft: &Value) -> Vec<String> {
    // A kept group off the return as it stands is locked as not on the
    // return, not as kept: its note keeps the blank-only rule.
    let mut keys: Vec<String> = penalties::computed_groups(kind)
        .into_iter()
        .filter(|group| {
            penalties::kept(kind, group, draft) && penalties::applies(kind, group, draft)
        })
        .flat_map(|group| group.keys)
        .collect();
    keys.extend(rows_annual_corporate::notes_always(kind, draft));
    keys
}

/// What `screen_form_open`, `screen_form_sample` and a rebuilt sample answer.
fn opened_json(
    kind: AgentFormKind,
    year: u16,
    period: u8,
    mut draft: Value,
    from_profile: Value,
    sample: Option<SampleBuild>,
    checked: BTreeSet<String>,
    paid: bool,
) -> Result<Value, String> {
    let code = kind.form_code();
    // What a stored 1702RT draft held in items 40, 41, 42 and 57 before
    // opening recomputed them (maintainer decisions (pre-2020 MCIT is 2% of
    // gross; pre-2020 1702RT computed like BIR); `finish` marks
    // it), for the notices below.
    let stored_pre_2020 = rows_annual_corporate::take_pre_2020_held(&mut draft);
    let (mut values, mut errors, status) = evaluate_here(kind, &mut draft, &checked, paid)?;
    if let Value::Array(list) = &mut errors {
        list.extend(rows_annual_corporate::rt_pre_2020_notices(
            stored_pre_2020,
            &draft,
        ));
    }
    let mut from_profile_draft = from_profile;
    agent::compute(kind, &mut from_profile_draft)?;
    let from_profile = agent::FromProfile {
        values: values_json(kind, &from_profile_draft)?,
        draft: from_profile_draft,
    };
    // A Draft with no baseline: what differs from the profile now, never
    // changed here (the page offers "Use profile values").
    let differs = profile_differs(kind, &mut draft, &from_profile.draft).unwrap_or_else(|error| {
        tracing::warn!("{code}: comparing with the tax profile failed: {error}");
        Vec::new()
    });
    // The base map, then the draft-dependent layer over it (PR 4's 2000 and
    // 2553 boxes, PR 8's annual corporate boxes, PR 9's 2552 cells, each
    // computed penalty group not edited, and 1701Q's spouse penalties taken
    // off again): both bir-core's, the one computation `agent::fill` refuses.
    let mut locks = agent::locks(kind, &from_profile);
    agent::draft_locks(kind, &draft, &mut locks);
    let profile = agent::profile_keys(kind, &from_profile);
    let profile_edit = agent::profile_edit_keys(kind, &from_profile);
    // Every profile box (filled by the profile or left blank), what the
    // profile alone shows in it, the profile fact it shows (for "Save to
    // profile too"), and the ones the filer changed on this return.
    let profile_boxes: Vec<&String> = profile_box_keys(kind);
    let profile_values: Map<String, Value> = profile_boxes
        .iter()
        .map(|key| {
            let shown = from_profile.values[key.as_str()].as_str().unwrap_or("").to_string();
            ((*key).clone(), Value::String(shown))
        })
        .collect();
    let profile_facts: Map<String, Value> = profile_boxes
        .iter()
        .filter_map(|key| Some(((*key).clone(), json!(profile_fact(kind, key)?))))
        .collect();
    let profile_edited: Vec<String> = profile_edits(&draft)
        .into_iter()
        .filter(|key| profile_boxes.contains(&key))
        .collect();
    let parts = if sample.is_some() {
        sample_period_parts(kind)
    } else {
        Vec::new()
    };
    let mut blank: Vec<String> = Vec::new();
    let mut needs = Map::new();
    if let Some(build) = &sample {
        if let Some(other) = &build.other {
            let mut other = other.clone();
            agent::compute(kind, &mut other)?;
            let other = values_json(kind, &other)?;
            // The period parts start blank, and so does everything that
            // follows the period until the filer sets it.
            blank = parts.iter().map(|(key, _)| key.clone()).collect();
            for key in sample_blank_keys(kind, &values, &other) {
                if !blank.contains(&key) {
                    blank.push(key);
                }
            }
        }
        // Period parts are page inputs in a sample, however a return locks
        // them; a period box the form computes stays shut.
        for (key, _) in &parts {
            locks.remove(key);
        }
        for key in DERIVED_PERIOD_BOXES {
            if schema_keys(code).contains(key) {
                locks.insert(key.to_string(), json!("computed from the year-end month"));
            }
        }
        for key in &blank {
            if locks.contains_key(key) {
                needs.insert(key.clone(), json!(SAMPLE_NEEDS));
            }
        }
    }
    // An undated pre-filled Draft computes no penalties (decision P4-3):
    // its penalty boxes say what they wait for.
    if let Some(note) = undated_penalty_note(kind, &draft, &status) {
        for group in penalties::computed_groups(kind) {
            for key in group.keys {
                needs.entry(key).or_insert_with(|| json!(note));
            }
        }
    }
    blank_sample(&blank, &mut values, &mut errors);
    // A printed box the filer cannot fill says why while it is blank (the
    // schema's reviewed `why`).
    let mut whys = Map::new();
    for (key, why) in schema_whys(code) {
        let blank = values[key.as_str()]
            .as_str()
            .is_none_or(|shown| shown.trim().is_empty());
        if blank && locks.contains_key(&key) && !needs.contains_key(&key) {
            whys.insert(key, json!(why));
        }
    }
    // 1800/1801 (PR 10): an untouched item 1 date says nothing on open,
    // and the page keeps it quiet until the date is edited or checked.
    rows_donor_estate::hold_untouched_date(kind, &draft, &mut errors);
    let quiet = rows_donor_estate::quiet(kind, &draft);
    let lock_notes: Map<String, Value> = locks
        .iter()
        .map(|(key, reason)| {
            let reason = reason.as_str().unwrap_or("");
            // 2551Q's Item 13 locked to its filed Q1 is already the filer's
            // words, with the year.
            let note = if reason.starts_with(agent::draft_locks::item_13::REASON_START) {
                reason
            } else {
                lock_note_for(code, key, reason)
            };
            (key.clone(), json!(note))
        })
        .collect();
    let stored_period = if sample.is_some() {
        None
    } else {
        stored_period(kind, &draft)
    };
    let options = catalogs::options(kind, &draft);
    let off = if status == FilingStatus::Draft {
        conditions::off_boxes(kind, &draft, &values)
    } else {
        Map::new()
    };
    let penalty_groups = penalties::json(kind, &draft);
    let noted = notes_always(kind, &draft);
    let pool_texts = combs::pool_texts(kind, &draft);
    let mark = sample.as_ref().map(|build| SampleMark {
        blank: blank.clone(),
        set: build.other.is_none(),
    });
    put_sample_mark(&mut draft, mark.as_ref());
    let period_parts: Vec<Value> = parts
        .iter()
        .map(|(key, part)| {
            let quarter = match part {
                PeriodPart::Quarter(quarter) => Some(*quarter),
                _ => None,
            };
            json!({ "key": key, "part": part.name(), "quarter": quarter })
        })
        .collect();
    // What a box waits for, from every form group's rows (each returns
    // nothing for a form it does not own).
    let mut waits = rows_withholding::waits(kind);
    waits.extend(rows_monthly::waits(kind));
    waits.extend(rows_dst_pt_vat::waits(kind));
    waits.extend(rows_annual_corporate::waits(kind));
    waits.extend(rows_annual_individual::waits(kind));
    waits.extend(rows_as_needed::waits(kind, &draft));
    Ok(json!({
        "form_code": code,
        "year": year,
        "period": period,
        "status": status_name(&status),
        "editable": status == FilingStatus::Draft,
        "stored_period": stored_period,
        "draft": draft,
        "values": values,
        "options": options,
        "locks": locks,
        "lock_notes": lock_notes,
        "notes_always": noted,
        "profile": profile,
        "profile_edit": profile_edit,
        "profile_boxes": profile_boxes,
        "profile_values": profile_values,
        "profile_facts": profile_facts,
        "profile_edited": profile_edited,
        "sample": sample.is_some(),
        "blank": blank,
        "needs": needs,
        "whys": whys,
        "waits": waits,
        "off": off,
        "penalties": penalty_groups,
        "pools": pool_texts,
        "period_parts": period_parts,
        "period_set": sample.as_ref().is_some_and(|build| build.other.is_none()),
        "profile_differs": differs,
        "submitted_at": draft["submitted_at"].as_str().or_else(|| draft["lifecycle"]["submitted_at"].as_str()),
        "confirmed_at": draft["confirmed_at"].as_str().or_else(|| draft["lifecycle"]["confirmed_at"].as_str()),
        "errors": errors,
        "quiet": quiet,
    }))
}

/// A sample's filer typed period parts (key -> value). While the period is
/// incomplete the answer is `{complete: false}`. Complete, it is either
/// `existing` (this taxpayer already has a stored return of the form for
/// that period: its period, label, status, last save and total payable),
/// or `sample`, a fresh sample built from the profile for exactly that
/// period, with nothing held and no trace of the placeholder period. Both
/// carry `opens`, the year and period `screen_form_open` takes for it.
fn screen_form_sample_period(
    arguments: &HostArguments,
) -> Result<impl Future<Output = HostResult> + Send + use<>, HostError> {
    let tin: String = arguments
        .string(0)?
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let kind = kind_of(arguments.string(1)?)?;
    let code = kind.form_code();
    let typed: Map<String, Value> = match to_json(arguments.value(2)?) {
        Value::Object(map) => map,
        _ => return Err(host_error("parts must be an object of key -> value")),
    };
    let parts = sample_period_parts(kind);
    let raw_parts = typed.clone();
    let typed = read_typed_period(code, &parts, &typed);
    enum Answer {
        Incomplete,
        Existing(Value, Value),
        Fresh(Value, u16, u8, Value),
    }
    let answer = match complete_period(kind, &parts, &typed) {
        None => Answer::Incomplete,
        Some((year, period)) => with_database(|db| {
            let profile = db
                .get_profile_by_tin(&tin)
                .map_err(|error| host_error(error.to_string()))?
                .ok_or_else(|| host_error(format!("no taxpayer profile with TIN {tin}")))?;
            let mut draft =
                load_draft(db, kind, &profile, year, period, true, None).map_err(host_error)?;
            mark_typed_trigger_model(kind, &mut draft);
            write_typed_period(kind, &parts, &typed, &mut draft).map_err(host_error)?;
            // The filer typed this period. Only a typed day makes it the
            // trigger date (a removal's Item 1): a monthly form's period is
            // complete with month and year alone, and its day is then still
            // the constructor's (D8c), as in `open_screen_form`.
            if typed.day.is_some() {
                for (key, _) in &parts {
                    mark_trigger_date(kind, &mut draft, key);
                }
            }
            agent::compute(kind, &mut draft).map_err(host_error)?;
            // A per-transaction form numbered by entry, not by date, cannot
            // match a typed date to a stored return.
            let numbered = NUMBERED_FORMS.contains(&code);
            let stored = if numbered {
                stored_for_date(db, kind, &draft).map_err(host_error)?
            } else {
                stored_for_period(db, kind, &draft).map_err(host_error)?
            };
            Ok(match stored {
                // Opened by its own stored key: exactly the return matched.
                Some(stored) => Answer::Existing(
                    existing_return(db, kind, &stored).map_err(host_error)?,
                    json!({ "year": year, "period": period, "key": stored_period(kind, &stored) }),
                ),
                // Start a return opens a new return for that period; for an
                // entry-numbered form, New return.
                None => {
                    // A stored key that names a day (`MMDD`, 101 or more)
                    // is offered only for a day the filer typed: an untyped
                    // day is the constructor's, and the return it would
                    // start must stay undated (D8c). On a form with a
                    // trigger date the day counts only when the host itself
                    // dated this sample from its typed parts just above
                    // (`mark_trigger_date`; the sample is built here, so its
                    // flag is the host's), and the key is recorded against
                    // this sample draft, which only that open consumes. A
                    // form whose answer opens nothing (entry-numbered) gets
                    // no offer.
                    let has_trigger =
                        !bir_core::forms::event_trigger::trigger_paths(code).is_empty();
                    let day_typed = typed.day.is_some()
                        && (!has_trigger || draft["trigger_date_entered"] == true);
                    let new_key = stored_period(kind, &draft).filter(|key| *key < 101 || day_typed);
                    let opens = if numbered {
                        Value::Null
                    } else {
                        let issued_from = new_key
                            .filter(|key| has_trigger && *key >= 101)
                            .and_then(|key| {
                                let sample = draft_key(kind, &draft)?;
                                issued_offers().insert((sample.clone(), year, key));
                                Some(sample.2)
                            });
                        json!({
                            "year": year,
                            "period": period,
                            "key": null,
                            "new_key": new_key,
                            "new_key_from": issued_from,
                        })
                    };
                    Answer::Fresh(draft, year, period, opens)
                }
            })
        })?,
    };
    Ok(async move {
        let answer = match answer {
            Answer::Incomplete => json!({ "complete": false }),
            Answer::Existing(existing, opens) => json!({
                "complete": true,
                "existing": existing,
                "opens": opens,
            }),
            Answer::Fresh(draft, year, period, opens) => {
                let build = SampleBuild { other: None };
                let from_profile = draft.clone();
                let mut sample = on_large_stack(move || {
                    opened_json(
                        kind,
                        year,
                        period,
                        draft,
                        from_profile,
                        Some(build),
                        BTreeSet::new(),
                        false,
                    )
                })
                .and_then(|result| result)
                .map_err(host_error)?;
                // A period part with no box in the field map (0605's quarter
                // ticks) still shows what the filer set.
                let def = kind.definition();
                for (key, _) in sample_period_parts(kind) {
                    let empty = sample["values"][&key].as_str().is_none_or(str::is_empty);
                    let unbound = !def.bindings.contains_key(&key);
                    if let (true, Some(value)) = (empty || unbound, raw_parts.get(&key)) {
                        sample["values"][&key] = value.clone();
                    }
                }
                json!({
                    "complete": true,
                    "sample": sample,
                    "opens": opens,
                })
            }
        };
        Ok(json(answer))
    })
}

/// Words of a period or date box's key or draft path.
const PERIOD_WORDS: [&str; 7] = ["year", "month", "quarter", "qtr", "date", "day", "period"];
/// Dates that are facts about the taxpayer, kept in a sample: a birth date,
/// a date of incorporation, the start of the business.
const PROFILE_DATE_WORDS: [&str; 3] = ["birth", "incorporat", "start"];

/// The draft paths a box writes (its own binding, else its item's group box).
fn bound_paths<'a>(def: &'a AgentFormDef, key: &str) -> Vec<&'a str> {
    let bound = def.bindings.get(key).or_else(|| {
        def.box_def(key)
            .and_then(|entry| entry.group.as_deref())
            .and_then(|group| def.bindings.get(group))
    });
    match bound.map(|bound| &bound.binding) {
        Some(
            Binding::Text { path, .. } | Binding::Amount { path, .. } | Binding::Whole { path, .. },
        ) => vec![path.as_str()],
        Some(Binding::Choice { options }) => options
            .iter()
            .flat_map(|option| option.set.keys().map(String::as_str))
            .collect(),
        None => Vec::new(),
    }
}

/// Whether a draft path is one of [`agent::PERIOD_FIELDS`] (`/taxable_year`,
/// `/year_end_month`, `/is_short_period`, `/quarter`).
fn is_period_path(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|leaf| agent::PERIOD_FIELDS.contains(&leaf))
}

/// The period and date boxes a Form library sample shows blank: a box bound
/// to a period path ([`is_period_path`]), a box whose value moves with the
/// year and period the draft is built for (`here` against `other`, such as a
/// due date), and a period-named box with no binding. Amounts, boxes the
/// profile fills and the taxpayer's own dates (birth, incorporation) keep
/// their values.
fn sample_blank_keys(kind: AgentFormKind, here: &Value, other: &Value) -> Vec<String> {
    let def = kind.definition();
    let Some(form) = schema_entry(kind.form_code()) else {
        return Vec::new();
    };
    let profile: BTreeSet<String> = def
        .boxes
        .iter()
        .filter(|entry| agent::is_profile_box(def, entry))
        .map(|entry| entry.key.clone())
        .chain(
            agent::occurrence_aliases(kind)
                .into_iter()
                .filter(|(_, official)| agent::has_profile_lock(def, official))
                .map(|(key, _)| key.clone()),
        )
        .collect();
    let has_word = |text: &str, list: &[&str]| {
        let text = text.to_ascii_lowercase();
        list.iter().any(|word| text.contains(word))
    };
    let blank = |key: &str, t: &str| {
        let name = key.split_once(':').map_or(key, |(_, name)| name);
        let paths = bound_paths(def, key);
        let amount = t == "money"
            || matches!(
                def.bindings.get(key).map(|bound| &bound.binding),
                Some(Binding::Amount { .. })
            );
        if amount
            || profile.contains(key)
            || has_word(name, &PROFILE_DATE_WORDS)
            || paths.iter().any(|path| has_word(path, &PROFILE_DATE_WORDS))
        {
            return false;
        }
        let shown = |values: &Value| values[key].as_str().unwrap_or("").to_string();
        let moves = !shown(here).is_empty() && shown(here) != shown(other);
        let named = paths.is_empty()
            && has_word(name, &PERIOD_WORDS)
            && ["sel", "int", "radio", "text", "date"].contains(&t);
        paths.iter().any(|path| is_period_path(path)) || moves || named
    };
    let mut keys = Vec::new();
    let items = form["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array().into_iter().flatten());
    for item in items {
        for field in item["f"].as_array().into_iter().flatten() {
            if let (Some(key), Some(t)) = (field["k"].as_str(), field["t"].as_str())
                && blank(key, t)
            {
                keys.push(key.to_string());
            }
        }
        let cols = item["cols"].as_array();
        for row in item["keys"].as_array().into_iter().flatten() {
            for (c, key) in row.as_array().into_iter().flatten().enumerate() {
                let t = cols
                    .and_then(|cols| cols.get(c))
                    .and_then(|col| col["t"].as_str());
                if let (Some(key), Some(t)) = (key.as_str(), t)
                    && blank(key, t)
                {
                    keys.push(key.to_string());
                }
            }
        }
    }
    keys
}

/// Show a sample's held boxes blank, and hold back the messages that judge
/// the period the sample was only built for: any message naming a held box
/// (held by key, not by item). Once the filer completes the period the
/// sample is rebuilt for it and nothing is held.
fn blank_sample(blank: &[String], values: &mut Value, errors: &mut Value) {
    if blank.is_empty() {
        return;
    }
    if let Value::Object(map) = values {
        for key in blank {
            if let Some(value) = map.get_mut(key) {
                *value = Value::String(String::new());
            }
        }
    }
    let held: BTreeSet<&str> = blank.iter().map(String::as_str).collect();
    if let Value::Array(list) = errors {
        list.retain(|error| {
            !error["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|key| held.contains(key))
        });
    }
}

/// What a sample's held box (a due date, a period-derived line) says while
/// the period is not set.
const SAMPLE_NEEDS: &str = "Shows once the period is set";

/// Per-transaction forms whose returns are numbered by entry (1, 2, ...)
/// rather than keyed by their date: a typed date cannot match a stored one,
/// and Start a return opens New return.
const NUMBERED_FORMS: [&str; 7] = ["0605", "1606", "1706", "1707", "1707A", "1800", "1801"];

/// Period boxes the form computes although nothing marks them computed:
/// 1707A's year-end day is the last day of its year-end month. Never a part
/// a sample's filer sets; shut in a sample with its reason.
const DERIVED_PERIOD_BOXES: [&str; 1] = ["frm1707Av2021:txtI1YearEndDay"];

/// One part of a return's period, as a sample's filer types it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeriodPart {
    Year,
    /// The month the return is for (a monthly or per-transaction return).
    Month,
    /// The month a fiscal year ends (an annual or quarterly return).
    YearEndMonth,
    Day,
    /// One quarter's tick box or radio.
    Quarter(u8),
    /// One box holding a whole date (MM/DD/YYYY).
    Date,
}

impl PeriodPart {
    fn name(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Month => "month",
            Self::YearEndMonth => "year_end_month",
            Self::Day => "day",
            Self::Quarter(_) => "quarter",
            Self::Date => "date",
        }
    }
}

/// A form's period parts, the boxes a sample's filer sets its period with:
/// on-screen boxes bound (themselves or through their item's group box) to
/// a period field ([`agent::PERIOD_FIELDS`]), or, with no binding, named for
/// one (`txtPg1I1Month`, `qtr_1`). Due dates, "from/to" ranges and the
/// short-period answer are not parts: they follow the period.
fn sample_period_parts(kind: AgentFormKind) -> Vec<(String, PeriodPart)> {
    let def = kind.definition();
    let Some(form) = schema_entry(kind.form_code()) else {
        return Vec::new();
    };
    let frequency = bir_core::forms::find_form(kind.form_code())
        .map(|form| form.frequency.clone())
        .unwrap_or(FilingFrequency::Annual);
    let year_end = matches!(
        frequency,
        FilingFrequency::Annual | FilingFrequency::Quarterly
    );
    let digit = |name: &str| {
        name.chars()
            .rev()
            .find(|c| c.is_ascii_digit())
            .and_then(|c| c.to_digit(10))
            .and_then(|d| u8::try_from(d).ok())
            .filter(|d| (1..=4).contains(d))
    };
    let classify = |key: &str, t: &str, first_part: bool| -> Option<PeriodPart> {
        let name = key
            .split_once(':')
            .map_or(key, |(_, name)| name)
            .to_ascii_lowercase();
        if name.contains("due")
            || name.contains("from")
            || name.contains("to")
                && ["tomonth", "today", "toyear", "tono", "to_"]
                    .iter()
                    .any(|w| name.contains(w))
            || name.contains("period")
            || PROFILE_DATE_WORDS.iter().any(|w| name.contains(w))
        {
            return None;
        }
        // A computed box (1707A's year-end day) shows its value; it is not
        // something the filer sets.
        if DERIVED_PERIOD_BOXES.contains(&key)
            || def
                .box_def(key)
                .is_some_and(|entry| entry.computed || entry.required == "computed")
        {
            return None;
        }
        // One part of the transaction's own date, kept whole in the draft.
        // Any other date (1604C's refund date) is an answer, not the period.
        if let Some(Binding::Text {
            path,
            part: Some(part),
            ..
        }) = def.bindings.get(key).map(|bound| &bound.binding)
        {
            if path != "/transaction_date" {
                return None;
            }
            return Some(match part {
                agent::DatePart::Month => PeriodPart::Month,
                agent::DatePart::Day => PeriodPart::Day,
                agent::DatePart::Year => PeriodPart::Year,
            });
        }
        let paths = bound_paths(def, key);
        let leaf = |path: &str| path.rsplit('/').next().unwrap_or("").to_string();
        let leaves: Vec<String> = paths.iter().map(|path| leaf(path)).collect();
        if !paths.is_empty() {
            // The return's own period is a top-level field; a schedule row's
            // year (`/nolco/0/year`) is an entry, not the period.
            if paths.iter().any(|path| path.matches('/').count() > 1) {
                return None;
            }
            let one = |names: &[&str]| leaves.iter().all(|l| names.contains(&l.as_str()));
            return if one(&["quarter"]) {
                digit(&name).map(PeriodPart::Quarter)
            } else if one(&[
                "year",
                "taxable_year",
                "transaction_year",
                "year_end_year",
                "donation_year",
                "death_year",
            ]) {
                Some(PeriodPart::Year)
            } else if one(&["year_end_month"]) {
                Some(PeriodPart::YearEndMonth)
            } else if one(&[
                "month",
                "transaction_month",
                "donation_month",
                "death_month",
            ]) {
                Some(if year_end {
                    PeriodPart::YearEndMonth
                } else {
                    PeriodPart::Month
                })
            } else if one(&[
                "day",
                "transaction_day",
                "year_end_day",
                "donation_day",
                "death_day",
            ]) {
                Some(PeriodPart::Day)
            } else if one(&["transaction_date"]) {
                Some(PeriodPart::Date)
            } else {
                None
            };
        }
        // A box with no binding is a period part only where the return names
        // its period, in the form's first part (not a schedule's year column).
        if !first_part {
            return None;
        }
        if name.contains("quarter") || name.contains("qtr") {
            return digit(&name).map(PeriodPart::Quarter);
        }
        if name.contains("month") {
            return Some(
                if year_end || name.contains("yearend") || name.contains("yrend") {
                    PeriodPart::YearEndMonth
                } else {
                    PeriodPart::Month
                },
            );
        }
        if name.contains("year") {
            return Some(PeriodPart::Year);
        }
        if name.contains("day") && t != "date" {
            return Some(PeriodPart::Day);
        }
        None
    };
    form["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .flat_map(|(index, part)| {
            part["items"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|item| item["f"].as_array().into_iter().flatten())
                .map(move |field| (index == 0, field))
        })
        .filter_map(|(first_part, field)| {
            let key = field["k"].as_str()?;
            let t = field["t"].as_str().unwrap_or("");
            if t == "hidden" || field["c"].as_bool() == Some(true) {
                return None;
            }
            classify(key, t, first_part).map(|part| (key.to_string(), part))
        })
        .collect()
}

/// The period a sample's filer typed, read from its period parts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TypedPeriod {
    year: Option<u16>,
    month: Option<u8>,
    year_end_month: Option<u8>,
    day: Option<u8>,
    quarter: Option<u8>,
}

/// A schema field's `m` (its length), when it has one.
fn field_length(code: &str, key: &str) -> Option<u64> {
    schema_entry(code)?["parts"]
        .as_array()?
        .iter()
        .flat_map(|part| part["items"].as_array().into_iter().flatten())
        .flat_map(|item| item["f"].as_array().into_iter().flatten())
        .find(|field| field["k"] == key)
        .and_then(|field| field["m"].as_u64())
}

fn read_typed_period(
    code: &str,
    parts: &[(String, PeriodPart)],
    typed: &Map<String, Value>,
) -> TypedPeriod {
    let text = |key: &str| match typed.get(key) {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(true)) => "true".into(),
        _ => String::new(),
    };
    // `03 - March` and `3` both read as 3.
    let number = |text: &str| -> Option<u32> {
        let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let mut period = TypedPeriod::default();
    for (key, part) in parts {
        let value = text(key);
        if value.is_empty() {
            continue;
        }
        match part {
            PeriodPart::Year => {
                // All four digits, or both of a two-digit box: "2", "20" and
                // "202" are a year still being typed, never 2002 or 2020.
                // Only a two-digit box's year is in this century ("24" is
                // 2024); four digits are the year as typed, so "0020" is
                // year 20, out of range, never 2020.
                let digits = value.chars().filter(char::is_ascii_digit).count();
                let short = field_length(code, key) == Some(2) && digits == 2;
                let whole = digits == 4 || short;
                period.year = number(&value)
                    .filter(|_| whole && value.chars().all(|c| c.is_ascii_digit()))
                    .map(|year| if short { year + 2000 } else { year })
                    .filter(|year| (1900..=2100).contains(year))
                    .and_then(|year| u16::try_from(year).ok());
            }
            PeriodPart::Month => {
                period.month = number(&value)
                    .filter(|m| (1..=12).contains(m))
                    .and_then(|m| u8::try_from(m).ok());
            }
            PeriodPart::YearEndMonth => {
                period.year_end_month = number(&value)
                    .filter(|m| (1..=12).contains(m))
                    .and_then(|m| u8::try_from(m).ok());
            }
            PeriodPart::Day => {
                period.day = number(&value)
                    .filter(|d| (1..=31).contains(d))
                    .and_then(|d| u8::try_from(d).ok());
            }
            PeriodPart::Quarter(quarter) => {
                if value == "true" {
                    period.quarter = Some(*quarter);
                }
            }
            PeriodPart::Date => {
                let pieces: Vec<u32> = value
                    .split(['/', '-'])
                    .filter_map(|piece| piece.trim().parse().ok())
                    .collect();
                if let [month, day, year] = pieces[..] {
                    period.month = u8::try_from(month).ok().filter(|m| (1..=12).contains(m));
                    period.day = u8::try_from(day).ok().filter(|d| (1..=31).contains(d));
                    period.year = u16::try_from(year)
                        .ok()
                        .filter(|y| (1900..=2100).contains(y));
                }
            }
        }
    }
    period
}

/// The year and page period (as `screen_form_open` takes them) a typed
/// period completes, or `None` while a part the form's frequency needs is
/// missing: monthly, month and year; quarterly, the quarter (or a month
/// inside it) and year; annual, the year; per transaction, its year and,
/// where the form has them, month and day.
fn complete_period(
    kind: AgentFormKind,
    parts: &[(String, PeriodPart)],
    typed: &TypedPeriod,
) -> Option<(u16, u8)> {
    let code = kind.form_code();
    let year = typed.year?;
    let has = |want: fn(&PeriodPart) -> bool| parts.iter().any(|(_, part)| want(part));
    let frequency = bir_core::forms::find_form(code)
        .map(|form| form.frequency.clone())
        .unwrap_or(FilingFrequency::Annual);
    let period = match frequency {
        FilingFrequency::Monthly => typed.month?,
        FilingFrequency::Quarterly => match (typed.quarter, typed.month) {
            (Some(quarter), _) => quarter,
            (None, Some(month)) if !has(|p| matches!(p, PeriodPart::Quarter(_))) => {
                (month - 1) / 3 + 1
            }
            _ => return None,
        },
        FilingFrequency::Annual => 0,
        FilingFrequency::OpenEnded => {
            if has(|p| matches!(p, PeriodPart::Month | PeriodPart::Date)) && typed.month.is_none() {
                return None;
            }
            if has(|p| matches!(p, PeriodPart::Day | PeriodPart::Date)) && typed.day.is_none() {
                return None;
            }
            if has(|p| matches!(p, PeriodPart::Quarter(_))) && typed.quarter.is_none() {
                return None;
            }
            if NUMBERED_FORMS.contains(&code) {
                1
            } else {
                typed.month.or(typed.quarter).unwrap_or(1)
            }
        }
    };
    // A form whose trigger date is its identity (2200AN, 2200S: the stored
    // key is the date) needs the day too: a period without it would start
    // a return on the constructor's day.
    let identity = identity_trigger_fields(kind);
    let identity_day = parts.iter().any(|(key, part)| {
        matches!(part, PeriodPart::Day | PeriodPart::Date)
            && trigger_fields_of_box(kind, key)
                .iter()
                .any(|field| identity.contains(field))
    });
    if identity_day && typed.day.is_none() {
        return None;
    }
    page_period(code, i64::from(period))
        .ok()
        .map(|period| (year, period))
}

/// Write every typed period part into a draft the host builds for that
/// period (a library sample's rebuild, a new return for a date), through
/// [`agent::write_period_box`]: the host building its own draft, never a
/// page's edit. A quarter with no box of its own goes into the draft's
/// `/quarter` (0605). A refusal is an error, never skipped.
fn write_typed_period(
    kind: AgentFormKind,
    parts: &[(String, PeriodPart)],
    typed: &TypedPeriod,
    draft: &mut Value,
) -> Result<(), String> {
    let def = kind.definition();
    for (key, part) in parts {
        let value = match part {
            PeriodPart::Year => typed.year.map(|year| year.to_string()),
            PeriodPart::Month => typed.month.map(|month| format!("{month:02}")),
            PeriodPart::YearEndMonth => typed.year_end_month.map(|month| format!("{month:02}")),
            PeriodPart::Day => typed.day.map(|day| format!("{day:02}")),
            PeriodPart::Date => match (typed.month, typed.day, typed.year) {
                (Some(month), Some(day), Some(year)) => Some(format!("{month:02}/{day:02}/{year}")),
                _ => None,
            },
            PeriodPart::Quarter(quarter) => match typed.quarter {
                Some(ticked) if ticked == *quarter => Some("true".to_string()),
                _ => None,
            },
        };
        let Some(value) = value else {
            continue;
        };
        if !def.bindings.contains_key(key) {
            if let (PeriodPart::Quarter(quarter), Some(Value::Number(_))) =
                (part, draft.pointer("/quarter"))
            {
                draft["quarter"] = Value::from(*quarter);
            }
            continue;
        }
        agent::write_period_box(kind, draft, key, &Value::from(value.clone()))
            .map_err(|error| format!("{key} = {value}: {error}"))?;
    }
    Ok(())
}

/// The period a stored return's key names: a transaction's `MMDD`, else a
/// quarter (2553), for a new return opened under that key.
fn typed_from_key(year: u16, key: i64) -> TypedPeriod {
    let mut typed = TypedPeriod {
        year: Some(year),
        ..TypedPeriod::default()
    };
    if key >= 101 {
        typed.month = u8::try_from(key / 100).ok();
        typed.day = u8::try_from(key % 100).ok();
    } else {
        typed.quarter = u8::try_from(key).ok();
    }
    typed
}

/// A period's words for the page: `March 2025`, `Q1 2025`, `2025`, or a
/// transaction's `03/16/2025`.
fn period_label(period: &FilingPeriod, year: u16) -> String {
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
    match period {
        FilingPeriod::Monthly(month) => match MONTHS.get(usize::from(*month).saturating_sub(1)) {
            Some(name) => format!("{name} {year}"),
            None => year.to_string(),
        },
        FilingPeriod::Quarterly(quarter) => format!("Q{quarter} {year}"),
        FilingPeriod::Annual => year.to_string(),
        FilingPeriod::OpenEnded(key) if *key >= 101 => {
            format!("{:02}/{:02}/{year}", key / 100, key % 100)
        }
        FilingPeriod::OpenEnded(key) => format!("No. {key}, {year}"),
    }
}

/// What a page needs to offer a stored return: its period and label, status,
/// when it was last saved and its total amount payable.
fn existing_return(db: &Database, kind: AgentFormKind, stored: &Value) -> Result<Value, String> {
    let identity = agent::identity(kind, stored)?;
    let total_payable = stored["id"]
        .as_i64()
        .and_then(|id| db.draft_amount_payable(id).ok().flatten());
    let updated_at = stored["updated_at"]
        .as_str()
        .or_else(|| stored["lifecycle"]["updated_at"].as_str())
        .unwrap_or("");
    // Draft: load it. Queued: view it while it waits. Reached BIR: view it,
    // or amend it where the form has an amended-return answer (never a
    // Paid return, which the app cannot revert).
    let amend = agent::amended_copy(kind, stored).is_ok();
    let mut actions = match identity.status {
        FilingStatus::Draft => vec!["load"],
        _ => vec!["view"],
    };
    if amend {
        actions.push("amend");
    }
    actions.push("keep");
    let stamp = |name: &str| {
        stored[name]
            .as_str()
            .or_else(|| stored["lifecycle"][name].as_str())
            .map(str::to_string)
    };
    Ok(json!({
        "actions": actions,
        "submitted_at": stamp("submitted_at"),
        "confirmed_at": stamp("confirmed_at"),
        "form_code": kind.form_code(),
        "year": identity.taxable_year,
        "period": bir_core::forms::queueable::period_column(&identity.period),
        "label": period_label(&identity.period, identity.taxable_year),
        "status": status_name(&identity.status),
        "updated_at": updated_at,
        "total_payable": total_payable,
    }))
}

/// "Use profile values" (see [`use_profile_values`]): the draft the page
/// holds with every unedited profile box at the profile's value, answered
/// like a fill. Nothing is saved: the page saves it.
fn screen_form_use_profile(arguments: &HostArguments) -> HostResult {
    let kind = kind_of(arguments.string(0)?)?;
    let year =
        u16::try_from(arguments.integer(1)?).map_err(|_| host_error("year is out of range"))?;
    let period = page_period(kind.form_code(), arguments.integer(2)?).map_err(host_error)?;
    let mut draft = draft_argument(arguments, 3)?;
    let sample = take_sample_mark(&mut draft);
    let amendment = take_amendment_mark(&mut draft);
    if agent::identity(kind, &draft).map_err(host_error)?.status != FilingStatus::Draft {
        return Err(host_error("this return is no longer a draft"));
    }
    let tin = agent::identity(kind, &draft).map_err(host_error)?.tin;
    let from_profile = with_database(|db| {
        let profile = db
            .get_profile_by_tin(&tin)
            .map_err(|error| host_error(error.to_string()))?
            .ok_or_else(|| host_error(format!("no taxpayer profile with TIN {tin}")))?;
        load_draft(db, kind, &profile, year, period, true, None).map_err(host_error)
    })?;
    // `keys`: only those boxes (each one's "Use profile value"); without
    // them, every box the filer did not edit (the page's "Use profile
    // values").
    let keys = unlocked_argument(arguments, 4);
    if keys.is_empty() {
        use_profile_values(kind, &mut draft, &from_profile).map_err(host_error)?;
    } else {
        use_profile_values_for(kind, &mut draft, &from_profile, &keys);
    }
    agent::compute(kind, &mut draft).map_err(host_error)?;
    let values = values_json(kind, &draft).map_err(host_error)?;
    let options = catalogs::options(kind, &draft);
    let penalty_groups = penalties::json(kind, &draft);
    let pool_texts = combs::pool_texts(kind, &draft);
    put_sample_mark(&mut draft, sample.as_ref());
    put_amendment_mark(&mut draft, amendment);
    Ok(json(json!({
        "draft": draft,
        "values": values,
        "options": options,
        "rejected": {},
        "penalties": penalty_groups,
        "pools": pool_texts,
    })))
}

fn screen_form_fill(arguments: &HostArguments) -> HostResult {
    let kind = kind_of(arguments.string(0)?)?;
    let mut draft = draft_argument(arguments, 1)?;
    let sample = take_sample_mark(&mut draft);
    let amendment = take_amendment_mark(&mut draft);
    let edits = arguments
        .value(2)?
        .as_object()
        .ok_or_else(|| host_error("edits must be an object of key -> value"))?;
    let editable = agent::identity(kind, &draft).map_err(host_error)?.status == FilingStatus::Draft;
    let edits: Map<String, Value> = edits.iter().map(|(k, v)| (k.clone(), to_json(v))).collect();
    let mut unlocked = unlocked_argument(arguments, 3);
    // A profile box the profile leaves blank is the filer's to fill on the
    // return, as the open shows it (unlocked, empty): its edit is kept, and
    // reported in `profile_blank` so the app can write it back to the
    // profile. A profile box with a value stays refused.
    let profile_blank = if editable && sample.is_none() { blank_profile_edits(kind, &draft, &edits, &unlocked) } else { BTreeSet::new() };
    unlocked.extend(profile_blank.iter().cloned());
    // A sample's period is set with `screen_form_sample_period`, never by a
    // fill: every writer here keeps the period locked.
    let before = draft.clone();
    let signature = penalty_signature(kind, &draft);
    let rejected = apply_screen_edits(kind, &mut draft, &edits, editable, &unlocked)?;
    let profile_blank: Vec<String> = profile_blank
        .into_iter()
        .filter(|key| !rejected.contains_key(key))
        .filter(|key| edits.get(key).and_then(Value::as_str).is_some_and(|v| !v.trim().is_empty()))
        .collect();
    if editable {
        // An added 2550Q row needs a stable row ID before it is saved; the
        // same step opening a draft runs.
        if kind == AgentFormKind::Queueable(QueueableKind::Form2550Q) {
            let mut typed: bir_core::forms::form_2550q::Form2550QDraft =
                serde_json::from_value(draft.clone()).map_err(|e| host_error(e.to_string()))?;
            bir_core::form_rules::prepare_form_2550q_live_row_identities(&mut typed).map_err(
                |e| host_error(format!("the 2550Q draft's row identities are unsafe: {e}")),
            )?;
            draft = serde_json::to_value(typed).map_err(|e| host_error(e.to_string()))?;
        }
        // A write that moved the record its penalties resolve from (a typed
        // transaction date) resolves them again as the open did — never the
        // due date the earlier state gave (decision P5-9).
        if penalty_signature(kind, &draft) != signature {
            with_database(|db| {
                reapply_penalty_context(db, kind, &mut draft).map_err(HostError::new)
            })?;
        }
        agent::compute(kind, &mut draft).map_err(host_error)?;
    }
    let mut values = values_json(kind, &draft).map_err(host_error)?;
    let options = catalogs::options(kind, &draft);
    // A held box the filer typed into (a schedule's month) shows what they
    // typed; the rest stays held until the period is set.
    let sample = sample.map(|mut mark| {
        mark.blank
            .retain(|key| !edits.contains_key(key) || rejected.contains_key(key));
        blank_sample(&mark.blank, &mut values, &mut Value::Null);
        mark
    });
    let penalty_groups = penalties::json(kind, &draft);
    let noted = notes_always(kind, &draft);
    let pool_texts = combs::pool_texts(kind, &draft);
    // What the filer typed that this edit removed: the page asks first (PR
    // 4's, PR 5's, PR 6's, PR 7's, PR 8's, PR 9's, PR 10's and PR 11's forms;
    // the integration merge chains PR 2's and
    // PR 3's below).
    let removes = rows_dst_pt_vat::removes(kind, &before, &draft, &edits)
        .or_else(|| rows_income_q::removes(kind, &before, &draft, &edits))
        .or_else(|| rows_1604::removes(kind, &before, &draft, &edits))
        .or_else(|| rows_annual_corporate::removes(kind, &before, &draft, &edits))
        .or_else(|| rows_annual_individual::removes(kind, &before, &draft, &edits))
        .or_else(|| rows_as_needed::removes(kind, &before, &draft, &edits))
        .or_else(|| rows_donor_estate::removes(kind, &before, &draft, &edits))
        .or_else(|| rows_excise::removes(kind, &before, &draft, &edits));
    // Locks this fill opened or shut (PR 8: a folded 1702RT item, item 42;
    // PR 11: an excise return's penalties kept as entered, then computed),
    // each with its note; `null` opens the box. bir-core's, as the open's.
    let relocks = agent::live_locks(kind, &draft);
    let relock_notes: Map<String, Value> = relocks
        .iter()
        .map(|(key, lock)| {
            let note = lock.as_str().map_or(Value::Null, |reason| {
                json!(lock_note_for(kind.form_code(), key, reason))
            });
            (key.clone(), note)
        })
        .collect();
    put_sample_mark(&mut draft, sample.as_ref());
    put_amendment_mark(&mut draft, amendment);
    Ok(json(json!({
        "draft": draft,
        "values": values,
        "options": options,
        "rejected": rejected,
        "profile_blank": profile_blank,
        "penalties": penalty_groups,
        "pools": pool_texts,
        "blank": sample.as_ref().map(|mark| mark.blank.clone()),
        // Rows the filer typed that this edit removed: the page asks first.
        "removes": rows_withholding::removes(kind, &before, &draft, &edits)
            .or_else(|| rows_monthly::removes(kind, &before, &draft, &edits))
            .or(removes),
        "relocks": relocks,
        "relock_notes": relock_notes,
        "notes_always": noted,
    })))
}

/// Apply the page's edits to `draft` the one way both the draft fill and the
/// live print preview use, so the preview never prints a different answer
/// than the page shows. Returns the rejected keys with their reasons.
/// The profile boxes among `edits` (not already `unlocked`) that the
/// taxpayer's stored profile leaves blank: the return's profile values are
/// read fresh, as "Use profile values" reads them.
fn blank_profile_edits(kind: AgentFormKind, draft: &Value, edits: &Map<String, Value>, unlocked: &BTreeSet<String>) -> BTreeSet<String> {
    let def = kind.definition();
    let wanted: Vec<&String> = edits
        .keys()
        .filter(|key| !unlocked.contains(*key))
        .filter(|key| def.box_def(key).is_some_and(|entry| agent::is_profile_box(&def, entry)))
        .collect();
    if wanted.is_empty() {
        return BTreeSet::new();
    }
    let Ok(identity) = agent::identity(kind, draft) else { return BTreeSet::new() };
    let profile = with_database(|db| {
        let profile = db
            .get_profile_by_tin(&identity.tin)
            .map_err(|error| host_error(error.to_string()))?
            .ok_or_else(|| host_error(format!("no taxpayer profile with TIN {}", identity.tin)))?;
        // The profile's boxes do not depend on the period.
        load_draft(db, kind, &profile, identity.taxable_year, 1, true, None).map_err(host_error)
    });
    let Ok(mut from_profile) = profile else { return BTreeSet::new() };
    if agent::compute(kind, &mut from_profile).is_err() {
        return BTreeSet::new();
    }
    let Ok(values) = values_json(kind, &from_profile) else { return BTreeSet::new() };
    wanted
        .into_iter()
        .filter(|key| values.get(key.as_str()).and_then(Value::as_str).is_none_or(|v| v.trim().is_empty()))
        .cloned()
        .collect()
}

/// `unlocked`: profile boxes the page has open, opened with Edit or left
/// blank by the profile (a radio by its own key or its item's group box);
/// their edits stay on this return.
fn apply_screen_edits(
    kind: AgentFormKind,
    draft: &mut Value,
    edits: &Map<String, Value>,
    editable: bool,
    unlocked: &BTreeSet<String>,
) -> Result<Map<String, Value>, HostError> {
    // 1600PT/1600VT Part I fields an older draft lacks stay out while blank (P2-4).
    let absent_part_i = rows_monthly::absent_part_i(kind, draft);
    let rejected = apply_screen_edits_here(kind, draft, edits, editable, unlocked)?;
    rows_monthly::keep_absent_blank(&absent_part_i, draft);
    Ok(rejected)
}

fn apply_screen_edits_here(
    kind: AgentFormKind,
    draft: &mut Value,
    edits: &Map<String, Value>,
    editable: bool,
    unlocked: &BTreeSet<String>,
) -> Result<Map<String, Value>, HostError> {
    // An open radio opens its item's group box, and an open group box its radios.
    let def = kind.definition();
    let expand = |keys: &BTreeSet<String>| {
        let mut open: BTreeSet<String> = keys.clone();
        for entry in keys.iter().filter_map(|key| def.box_def(key)) {
            open.extend(entry.group.clone());
            open.extend(entry.members.iter().cloned());
        }
        open
    };
    let profile_open = expand(unlocked);
    let lift = |key: &str| {
        if profile_open.contains(key) {
            catalogs::Lift::Profile
        } else {
            catalogs::Lift::Nothing
        }
    };
    // A stored draft is brought to the current model first, so no write
    // below is undone by a migration in the compute (2551Q's manual
    // penalties).
    agent::prepare_for_writes(kind, draft).map_err(host_error)?;
    let mut rejected = Map::new();
    // The boxes this batch blanks (an excise return joins decision D8 only
    // when a whole penalty group is blanked, PR 11).
    let blanked: Vec<String> = edits
        .iter()
        .filter(|(_, v)| v.is_null() || v.as_str().is_some_and(|text| text.trim().is_empty()))
        .map(|(k, _)| k.clone())
        .collect();
    let mut edits: Vec<_> = edits.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    edits.sort_by_key(|(key, _)| catalogs::edit_order(kind, key));
    // 2551Q's penalty boxes come last, so they are judged on the batch's
    // own Item 13 answer (an 8% answer in the same batch refuses them).
    if kind == AgentFormKind::Form2551Q {
        let groups = penalties::groups(kind);
        edits.sort_by_key(|(key, _)| groups.iter().any(|group| group.keys.contains(key)));
    }
    // What each box was sent, for a gate that reads the write itself.
    let sent: BTreeMap<String, Value> = edits.iter().cloned().collect();
    let shown = values_json(kind, draft).map_err(host_error)?;
    let pools = combs::pools(kind.form_code());
    // Whether each penalty group was edited before this batch: an amount in
    // one box of a batch does not open the others, and blanking the group
    // from a partly edited state clears all of it. 0605's group is not
    // computed and is open: it is always "edited".
    let computes = penalties::computes(kind);
    let penalty_groups: Vec<(penalties::Group, bool)> = penalties::groups(kind)
        .into_iter()
        .map(|group| {
            let edited = !computes || penalties::edited(&group, draft);
            (group, edited)
        })
        .collect();
    // A one-date box's parts are judged whole, as the page reads a date
    // (PR 9's forms, rows_as_needed.rs).
    let date_refusals = rows_as_needed::refuse_dates(kind, &shown, &edits);
    let planned = plan_choice_edits(kind, draft, &shown, edits);
    // The draft paths this batch's picks write — a member's pick, a group
    // box's option, a filer-type step. The value a choice box was sent, as
    // its binding spells it ("true"/"false"/an option value).
    let sent_option = |value: &Value| -> String {
        value
            .as_str()
            .map(str::to_string)
            .or_else(|| value.as_bool().map(|b| b.to_string()))
            .unwrap_or_default()
    };
    let picked_paths: BTreeSet<String> = planned
        .iter()
        .flat_map(|(key, step)| match step {
            ChoiceStep::Fill(value) if !is_unticked(value) => {
                option_paths(def, key, &sent_option(value))
            }
            ChoiceStep::Set(sets) => sets.iter().map(|(path, _)| path.clone()).collect(),
            _ => Vec::new(),
        })
        .collect();
    for (key, step) in planned {
        if let Some(why) = date_refusals.get(&key) {
            rejected.insert(key.clone(), why.clone());
            continue;
        }
        if !editable {
            rejected.insert(key.clone(), json!("this return is no longer a draft"));
            continue;
        }
        // 2551Q's Item 13 on a later quarter whose filed Q1 is in this app
        // is that Q1's answer (`draft_locks::item_13`).
        if let Some(lock) = agent::draft_locks::item_13::lock(kind, draft, &key) {
            rejected.insert(key.clone(), Value::String(format!("`{key}`: {lock}")));
            continue;
        }
        // A box the page shows as the profile's (core-locked, or the filer's
        // own name matched by key) takes an edit only once opened: the host
        // keeps the lock it reports.
        if def
            .box_def(&key)
            .is_some_and(|entry| agent::is_profile_box(def, entry))
            && !profile_open.contains(&key)
        {
            let reason = bir_core::forms::agent::Lock::Profile.reason();
            rejected.insert(key.clone(), Value::String(format!("`{key}` {reason}")));
            continue;
        }
        // A radio written by direct draft steps keeps the locks a fill keeps:
        // the return's period and computed boxes stay as they are.
        if matches!(step, ChoiceStep::Set(_) | ChoiceStep::Untick)
            && let Some(reason) = set_refusal(def, &key)
        {
            rejected.insert(key.clone(), Value::String(reason));
            continue;
        }
        // 1701Q's spouse part while it is closed says what opens it, its
        // penalty boxes too (PR 5, rows_income_q.rs).
        if let ChoiceStep::Fill(value) = &step
            && let Some(why) = rows_income_q::closed_refusal(kind, draft, &key, value)
        {
            rejected.insert(key.clone(), json!(why));
            continue;
        }
        // The annual individual returns' spouse part while it is closed says
        // what opens it, for a radio or its group box alike, read on the
        // draft as this batch has left it (PR 7, rows_annual_individual.rs).
        if let Some(why) = sent
            .get(&key)
            .and_then(|value| rows_annual_individual::closed_refusal(kind, draft, &key, value))
        {
            rejected.insert(key.clone(), json!(why));
            continue;
        }
        // A "false" that writes a path this batch's picks write is a pick's
        // sibling noise — on some forms a member's "false" is another
        // answer (1701MS's Profession false is Mixed) — never the pick's
        // undo. Dropped as silently as the group siblings
        // plan_choice_edits drops (review F1). Only the sibling's own
        // "false" is noise: "" is a blank the refused-arm judges, and "0"
        // is a real option (1606's treaty list), neither a pick's undo.
        if let ChoiceStep::Fill(value) = &step
            && (value.as_str() == Some("false") || value.as_bool() == Some(false))
            && option_paths(def, &key, &sent_option(value))
                .iter()
                .any(|path| picked_paths.contains(path))
        {
            continue;
        }
        // A SHOWN_WIDER box the page showed ticked while the draft holds a
        // wider answer than its tick writes: the tick is a re-ask of the
        // shown answer, so it writes nothing unless the draft — as this
        // batch has left it — already holds the tick (review F3, judged
        // when the step runs as decision P4-18's unticks are).
        if let ChoiceStep::Fill(value) = &step
            && !is_unticked(value)
            && shown_wider(kind, &key)
            && shown.get(&key).and_then(Value::as_str) == Some("true")
            && !holds_tick(def, draft, &key)
        {
            continue;
        }
        let mut next = draft.clone();
        // A computed penalty takes an amount only once its group is opened
        // with Edit (sent as unlocked) or already edited; a blank clears the
        // override, so the computed amount files again (rule 8).
        if let Some((group, edited)) = penalty_groups
            .iter()
            .find(|(group, _)| group.keys.contains(&key))
        {
            // 2551Q's Items 20-22 on an 8% return take no amount (the
            // official page zeroes and disables them): refused with the
            // reason, judged on the Item 13 answer this batch leaves, rather
            // than dropped by the compute. A blank only clears an amount an
            // older manual draft kept there.
            if penalties::eight_percent(kind, draft) {
                let blank = matches!(&step, ChoiceStep::Fill(value)
                    if value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()));
                match penalties::kept_amount_path(kind, draft, &key) {
                    Some(path) if blank => {
                        set_draft_path(&mut next, &path, json!(0.0));
                        *draft = next;
                    }
                    Some(_) => {
                        rejected.insert(
                            key.clone(),
                            json!(format!("`{key}`: {}", penalties::KEPT_ON_EIGHT_PERCENT)),
                        );
                    }
                    None => {
                        rejected.insert(
                            key.clone(),
                            json!(format!("`{key}`: {}", penalties::EIGHT_PERCENT)),
                        );
                    }
                }
                continue;
            }
            // A group not on the return opens with nothing (PR 11's forms).
            if let ChoiceStep::Fill(value) = &step
                && let Some(lock) = rows_excise::closed_penalty(kind, draft, group, value)
            {
                rejected.insert(key.clone(), json!(format!("`{key}` is {lock}")));
                continue;
            }
            if !unlocked.contains(&key) && !edited {
                let lock = rows_income_q::penalty_lock(kind, draft, &key)
                    .or_else(|| rows_annual_individual::penalty_lock(kind, draft, &key))
                    .or_else(|| rows_excise::penalty_lock(kind, draft, &key))
                    .unwrap_or(penalties::LOCK);
                rejected.insert(key.clone(), json!(format!("`{key}` is {lock}")));
                continue;
            }
            if let (ChoiceStep::Fill(value), Some(path)) = (&step, penalties::path_of(kind, &key))
                && (value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()))
            {
                set_draft_path(&mut next, &path, Value::Null);
                // "Use the computed amounts" on a 1701Q column still on its
                // typed amounts joins the model (PR 5).
                rows_income_q::join_model(kind, &mut next, &key);
                // "Use the computed amounts" on a 1701 column still on its
                // typed amounts joins the model (PR 7).
                rows_annual_individual::join_model(kind, &mut next, &key);
                *draft = next;
                continue;
            }
            // A 1701 column with no penalty state yet takes the amount as
            // its override (PR 7).
            rows_annual_individual::make_room(kind, &mut next, &key);
        }
        // A Part II date cleared on the page clears the draft's date (PR 6;
        // the whole-date binding takes only MM/DD/YYYY text).
        if let ChoiceStep::Fill(value) = &step
            && let Some(path) = rows_1604::cleared_date(kind, &key, value)
        {
            set_draft_path(&mut next, &path, Value::String(String::new()));
            *draft = next;
            continue;
        }
        // What the batch wrote, for PR 11's host refusals below.
        let written = match &step {
            ChoiceStep::Fill(value) => Some(value.clone()),
            ChoiceStep::Set(_) | ChoiceStep::Untick => None,
        };
        let result = match step {
            // A pooled address (rule 5) comes as one text on its first line.
            ChoiceStep::Fill(value) if pools.contains_key(&key) => {
                combs::write_pool(kind, &pools[&key], &mut next, &value, |line: &str| {
                    lift(line)
                })
            }
            ChoiceStep::Fill(value) => catalogs::fill(kind, &mut next, &key, &value, lift(&key)),
            ChoiceStep::Set(sets) => {
                for (path, value) in sets {
                    set_draft_path(&mut next, &path, value);
                }
                Ok(())
            }
            ChoiceStep::Untick => {
                untick(kind, &mut next, &key);
                Ok(())
            }
        };
        // A box the form would clear (its answer does not hold) or past its
        // official length is refused with its reason (PR 11's forms).
        let result = result.and_then(|()| {
            written
                .and_then(|value| rows_excise::refusal(kind, &next, &key, &value))
                .map_or(Ok(()), Err)
        });
        match result {
            Ok(()) => {
                // A profile box edited here keeps the edit when the profile
                // changes later (`follow_profile`).
                if def
                    .box_def(&key)
                    .is_some_and(|entry| agent::is_profile_box(def, entry))
                {
                    mark_profile_edit(&mut next, &key);
                }
                mark_trigger_date(kind, &mut next, &key);
                *draft = next;
            }
            Err(message) => {
                rejected.insert(key.clone(), Value::String(message));
            }
        }
    }
    // "Use the computed amounts" on an excise return stored before decision
    // maintainer decision (excise penalties run from the typed date) (PR 11),
    // or a 1800 or 1801 return stored before maintainer decision (1800/1801
    // penalties run from the due date),
    // puts it on the typed due date.
    let groups: Vec<penalties::Group> =
        penalty_groups.into_iter().map(|(group, _)| group).collect();
    rows_excise::join_if_group_blanked(kind, draft, &groups, &blanked, &rejected);
    Ok(rejected)
}

/// Why a box's direct draft steps (`ChoiceStep::Set`) are refused: it is
/// computed, or it (or its item's group box) picks the return's identity.
fn set_refusal(def: &AgentFormDef, key: &str) -> Option<String> {
    let entry = def.box_def(key)?;
    if entry.computed || entry.required == "computed" {
        return Some(format!("`{key}` is computed by BIR's own formulas"));
    }
    let lock = def
        .bindings
        .get(key)
        .or_else(|| {
            entry
                .group
                .as_deref()
                .and_then(|group| def.bindings.get(group))
        })
        .and_then(|bound| bound.lock);
    (lock == Some(agent::Lock::Identity))
        .then(|| format!("`{key}` {}", agent::Lock::Identity.reason()))
}

/// One planned write of `screen_form_fill`.
enum ChoiceStep {
    /// The ordinary box fill.
    Fill(Value),
    /// Direct draft writes (JSON pointer -> value).
    Set(Vec<(String, Value)>),
    /// Untick the ticked member of a group of three or more ([`untick`]).
    Untick,
}

/// Whether `draft` holds what ticking `key` writes (its `"true"` option).
fn holds_tick(def: &AgentFormDef, draft: &Value, key: &str) -> bool {
    match def.bindings.get(key).map(|bound| &bound.binding) {
        Some(Binding::Choice { options }) => options
            .iter()
            .filter(|option| option.value == "true")
            .flat_map(|option| option.set.iter())
            .all(|(path, value)| draft.pointer(path) == Some(value)),
        _ => false,
    }
}

/// Decision P4-18: untick `key`, the ticked member of a group of three or
/// more, on the draft as the batch has left it when this step runs. A
/// member an earlier step of the batch already moved off (the group box
/// answered another way) is no longer ticked: nothing is written. Otherwise
/// each path its tick writes ends as [`untick_blanks`] says.
fn untick(kind: AgentFormKind, draft: &mut Value, key: &str) {
    let def = kind.definition();
    if !holds_tick(def, draft, key) {
        return;
    }
    for (path, write) in untick_blanks(kind, draft, &true_option_paths(def, key)) {
        if let UntickWrite::Blank(value) = write {
            set_draft_path(draft, &path, value);
        }
    }
}

fn is_unticked(value: &Value) -> bool {
    match value {
        Value::Bool(b) => !b,
        Value::Null => true,
        Value::String(s) => matches!(s.trim(), "" | "false" | "0" | "N"),
        _ => false,
    }
}

/// Paths a choice box's `value` option writes (`"false"` for a sibling
/// untick, `"true"` for its pick, the option values of a group box).
fn option_paths(def: &AgentFormDef, key: &str, value: &str) -> Vec<String> {
    match def.bindings.get(key).map(|bound| &bound.binding) {
        Some(Binding::Choice { options }) => options
            .iter()
            .filter(|option| option.value == value)
            .flat_map(|option| option.set.keys().cloned())
            .collect(),
        _ => Vec::new(),
    }
}

/// Paths a choice box's `"true"` option writes.
fn true_option_paths(def: &AgentFormDef, key: &str) -> Vec<String> {
    option_paths(def, key, "true")
}

/// The unanswered variants an answer-list field may have, tried in this
/// order once `null` does not parse (2000OT's ATC writes `Unanswered`).
pub(crate) const UNTICK_UNANSWERED: [&str; 4] = ["unanswered", "none", "Unanswered", "None"];

/// What unticking the ticked member of a group of three or more does at one
/// of its draft paths (decision P4-18).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum UntickWrite {
    /// Writes this value: `null` where the field is nullable (the item
    /// unanswered, as before), else the field's own unanswered variant
    /// ([`UNTICK_UNANSWERED`]) where it is an answer list that has one:
    /// PR 11's excise manner items, and the groups whose `null` failed the
    /// whole fill on 1601EQ, 1602Q, 1700, 1701, 1701A, 1701MS, 1702EX, 2000
    /// and 2000OT.
    Blank(Value),
    /// Keeps the answer: the field takes any text, so it is not an answer
    /// list and only `null` would blank it.
    KeptText,
    /// Keeps the answer: the field has no `null` and none of
    /// [`UNTICK_UNANSWERED`]. 2552 item 11 is the one case: its
    /// `Form2552Transaction::Unspecified` default is not one of those
    /// variants, the group box refuses a blank ("must be one of LSE,
    /// Primary, Secondary"), and PR 9 decided an untick keeps the answer.
    KeptNoBlank,
}

/// [`UntickWrite`] for each of `paths`, judged per path by writing the
/// candidate on a copy of `draft` (with the paths before it written) and
/// checking the copy still parses as the form's draft. A path that keeps
/// its answer never stops the other paths' writes, nor the rest of the
/// edit.
pub(crate) fn untick_blanks(
    kind: AgentFormKind,
    draft: &Value,
    paths: &[String],
) -> Vec<(String, UntickWrite)> {
    let parses = |candidate: &Value| agent::identity(kind, candidate).is_ok();
    let mut next = draft.clone();
    let mut writes = Vec::new();
    for path in paths {
        let fits = |value: &Value| {
            let mut copy = next.clone();
            set_draft_path(&mut copy, path, value.clone());
            parses(&copy)
        };
        let write = if fits(&Value::Null) {
            UntickWrite::Blank(Value::Null)
        } else if fits(&json!("\u{1}not an answer")) {
            UntickWrite::KeptText
        } else {
            UNTICK_UNANSWERED
                .iter()
                .map(|variant| json!(variant))
                .find(|variant| fits(variant))
                .map_or(UntickWrite::KeptNoBlank, UntickWrite::Blank)
        };
        if let UntickWrite::Blank(value) = &write {
            set_draft_path(&mut next, path, value.clone());
        }
        writes.push((path.clone(), write));
    }
    writes
}

/// 1701 Item 6 and Part IV Item 3: the filer type and its "also a
/// compensation earner" flag. Compensation Earner alone is its own filer
/// type (ATC II011); beside Single Proprietor or Professional it marks mixed
/// income, as the official form's TPFilerType does.
fn form_1701_type_paths(kind: AgentFormKind, key: &str) -> Option<(&'static str, &'static str)> {
    if kind.form_code() != "1701" {
        return None;
    }
    let tail = key.rsplit(':').next().unwrap_or(key);
    if tail.starts_with("rdoPg1I6TaxpayerType") {
        Some(("/taxpayer_type", "/taxpayer_also_compensation_earner"))
    } else if tail.starts_with("rdoPg2I3SpouseType") {
        Some(("/spouse/filer_type", "/spouse_also_compensation_earner"))
    } else {
        None
    }
}

/// The official radio name groups of an agent-form package (`name` ->
/// control ids), from each form's runtime-control inventory
/// (rules/forms/<package>/fixtures/runtime-control-inventory-v796.json),
/// generated into app/forms/radio-name-groups.json. `None` for a form the
/// rules corpus has no inventory for (1701MS, 1701Q).
fn official_radio_groups(package: &str) -> Option<&'static BTreeMap<String, Vec<String>>> {
    static GROUPS: OnceLock<BTreeMap<String, BTreeMap<String, Vec<String>>>> = OnceLock::new();
    GROUPS
        .get_or_init(|| {
            serde_json::from_str(&bir_core::packed::inflate(bir_core::packed_bytes!(
                "assets/forms/radio-name-groups.json"
            )))
            .expect("app/forms/radio-name-groups.json is package -> name -> ids")
        })
        .get(package)
}

/// Boxes that answer an official radio name group they are not drawn in,
/// with evidence: `(form code, key, group name, evidence)`.
pub(crate) const JOINED_RADIO_GROUPS: &[(&str, &str, &str, &str)] = &[
    (
        "2200S",
        "frm2200S:chkPymntManner_3",
        "frm2200S:chkPymntManner",
        "item 17 Other Similar Schemes is the third answer of the manner of payment beside items 15 \
     and 16 (PR 11; decision P4-16: unticking the manner leaves it unanswered); the official \
     page draws it as a tick box of its own name (chkPymntMannerOther)",
    ),
    (
        "2551Q",
        "frm2551Qv2018:taxRateNA",
        "frm2551Qv2018:taxRate",
        "Item 13 Not applicable is the third answer beside Graduated and 8%. The official HTML \
     draws only taxRate1 and taxRate2; Not applicable is this app's explicit election \
     (Item13Election::NotApplicable) and is not an official upload key",
    ),
];

/// Boxes the page shows ticked for a wider answer than their binding's
/// tick writes: `(form, key)`. Only these — 1701's and 1701A's tax-rate
/// pairs — may have a tick dropped: Graduated shows for every graduated
/// ATC but its tick writes II013's (1701) and i_i012's (1701A), so a
/// re-pick of the shown answer is a re-ask, not a move. Everywhere else a
/// shown tick is exactly the binding's answer and its re-pick writes it
/// again (review F3: the rule used to swallow the tick of any grouped box
/// judged on the pre-batch draft).
const SHOWN_WIDER: &[(&str, &str)] = &[
    ("1701", "frm1701:rdoPg1I21TaxRateG"),
    ("1701", "frm1701:rdoPg1I21TaxRateP"),
    ("1701", "frm1701:rdoPg2I12TaxRateG"),
    ("1701", "frm1701:rdoPg2I12TaxRateP"),
    ("1701A", "frm1701A:optTaxRate_1"),
    ("1701A", "frm1701A:optTaxRate_2"),
    ("1701A", "frm1701A:optSpouseTaxRate_1"),
    ("1701A", "frm1701A:optSpouseTaxRate_2"),
];

fn shown_wider(kind: AgentFormKind, key: &str) -> bool {
    SHOWN_WIDER
        .iter()
        .any(|(form, pinned)| *form == kind.form_code() && *pinned == key)
}

/// Each one-answer box's (group id, member count).
///
/// Where the form's official runtime inventory names radio groups, those are
/// the groups: the official name group's boxes this app can fill (bound, or
/// written by the host), plus [`JOINED_RADIO_GROUPS`]. 1701's and 1701A's
/// tax-rate radios are then the two-member pairs they are officially, not
/// part of the ATC.
///
/// Every other box keeps the grouping the agent definition gives it: a
/// radio of a group box (`radio_group`), or radio boxes without a group box
/// that write one draft field (`path:<pointer>`, 1701MS's and 1701Q's
/// items), counting only the boxes no official group claims. A box the host
/// writes itself (`catalogs::host_writes`) joins no such field group: its
/// probed binding is not what its tick does (1700's Schedule 1 row 2 Spouse
/// box was bound to item 17's filing status).
fn one_answer_groups(kind: AgentFormKind) -> BTreeMap<String, (String, usize)> {
    let def = kind.definition();
    let derived = derived_groups(kind);
    let Some(official) = official_radio_groups(kind.package()) else {
        return derived;
    };
    let fillable = |key: &str| {
        def.box_def(key).is_some_and(BoxDef::is_user_box)
            && (def.bindings.contains_key(key) || catalogs::host_writes(kind, key))
    };
    let mut groups = BTreeMap::new();
    for (name, ids) in official {
        let mut members: Vec<&str> = ids
            .iter()
            .map(String::as_str)
            .filter(|key| fillable(key))
            .collect();
        members.extend(
            JOINED_RADIO_GROUPS
                .iter()
                .filter(|(form, key, group, _)| {
                    *form == kind.form_code() && group == name && fillable(key)
                })
                .map(|(_, key, ..)| *key),
        );
        if members.len() >= 2 {
            for key in &members {
                groups.insert(key.to_string(), (name.clone(), members.len()));
            }
        }
    }
    let mut rest: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, (group, _)) in derived {
        if !groups.contains_key(&key) {
            rest.entry(group).or_default().push(key);
        }
    }
    for (group, keys) in rest {
        if keys.len() >= 2 {
            for key in &keys {
                groups.insert(key.clone(), (group.clone(), keys.len()));
            }
        }
    }
    groups
}

/// The grouping the agent definition gives: a radio of a group box
/// (`radio_group`) is in that group; radio boxes without a group box (1701
/// Item 7's ATCs) form one group per draft field their tick writes
/// (`path:<pointer>`), when two or more write it, leaving out the boxes the
/// host writes itself.
fn derived_groups(kind: AgentFormKind) -> BTreeMap<String, (String, usize)> {
    let def = kind.definition();
    let implicit_path = |entry: &BoxDef| -> Option<String> {
        if entry.group.is_some()
            || !entry.kind.contains("radio")
            || catalogs::host_writes(kind, &entry.key)
        {
            return None;
        }
        let paths = true_option_paths(def, &entry.key);
        (paths.len() == 1).then(|| paths[0].clone())
    };
    let mut implicit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in &def.boxes {
        if let Some(path) = implicit_path(entry) {
            implicit.entry(path).or_default().push(entry.key.clone());
        }
    }
    let mut groups = BTreeMap::new();
    for entry in &def.boxes {
        if let Some(group) = entry
            .group
            .as_deref()
            .and_then(|g| def.box_def(g))
            .filter(|g| g.radio_group)
        {
            groups.insert(entry.key.clone(), (group.key.clone(), group.members.len()));
        } else if let Some(path) = implicit_path(entry) {
            let members = implicit.get(&path).map_or(0, Vec::len);
            if members >= 2 {
                groups.insert(entry.key.clone(), (format!("path:{path}"), members));
            }
        }
    }
    groups
}

/// Turn the page's checkbox edits into writes that keep a radio group to one
/// answer. The probe binds each member's `"false"` to some other answer
/// (unticking Single Proprietor wrote Professional), and a batch that ticks
/// one member and unticks the rest could end on an unticked member's
/// "other" answer. So:
/// - a group with a ticked member in the batch ignores its unticked members;
/// - unticking a member that is not ticked does nothing;
/// - a sibling "false" beside a batch's pick never overwrites the pick
///   (judged when the step runs, `apply_screen_edits`);
/// - ticking a [`SHOWN_WIDER`] box the page already shows ticked writes
///   nothing when its tick would write another answer than the draft, as the
///   batch has left it, holds: the box shows a wider answer than its binding
///   writes (1701's Graduated shows for every graduated ATC but its tick
///   writes II013), so a pair's re-pick must not move the answer — judged
///   when the step runs, in `apply_screen_edits`;
/// - unticking the ticked member of a group of three or more leaves the
///   item unanswered where its field can say so, and otherwise keeps the
///   answer (`untick_blanks`); a yes/no pair keeps its official flip.
///
/// Edits keep their dependency order otherwise.
fn plan_choice_edits(
    kind: AgentFormKind,
    draft: &Value,
    shown: &Value,
    edits: Vec<(String, Value)>,
) -> Vec<(String, ChoiceStep)> {
    let def = kind.definition();
    let groups = one_answer_groups(kind);
    // (group id, member count) of a one-answer box.
    let group_info = |key: &str| -> Option<(String, usize)> { groups.get(key).cloned() };
    let group_of = |key: &str| -> Option<&BoxDef> {
        let group = def.box_def(key)?.group.as_deref()?;
        def.box_def(group).filter(|entry| entry.radio_group)
    };
    let ticked_groups: BTreeSet<String> = edits
        .iter()
        .filter(|(_, value)| !is_unticked(value))
        .filter_map(|(key, _)| group_info(key).map(|g| g.0))
        .collect();
    let mut plain = Vec::new();
    for (key, value) in edits {
        let unticked = is_unticked(&value);
        // 1801 item 15D's four frequency ticks (PR 10): a lone untick of the
        // chosen one would be a direct draft step here (three members or
        // more), which skips the host's own write (it clears the Others
        // text and asks first: rows_donor_estate.rs). Every other untick
        // keeps the rule below: one of a group with a box ticked in the
        // batch, or of a box not shown ticked, writes nothing.
        if rows_donor_estate::is_frequency_tick(kind, &key) {
            let group_ticked = group_info(&key).is_some_and(|(g, _)| ticked_groups.contains(&g));
            let shown_on = shown.get(&key).and_then(Value::as_str) == Some("true");
            if unticked && (group_ticked || !shown_on) {
                continue;
            }
            plain.push((key, ChoiceStep::Fill(value)));
            continue;
        }
        if let Some((type_path, flag_path)) = form_1701_type_paths(kind, &key) {
            let current = draft
                .pointer(type_path)
                .and_then(Value::as_str)
                .unwrap_or("");
            let mixed = draft
                .pointer(flag_path)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let business = matches!(current, "SingleProprietor" | "Professional");
            if key.ends_with('C') && group_of(&key).is_none() {
                let sets = match (unticked, business) {
                    (false, true) => vec![(flag_path.to_string(), json!(true))],
                    (false, false) => vec![
                        (type_path.to_string(), json!("CompensationEarner")),
                        (flag_path.to_string(), json!(false)),
                    ],
                    (true, _) if current == "CompensationEarner" => vec![
                        (type_path.to_string(), Value::Null),
                        (flag_path.to_string(), json!(false)),
                    ],
                    (true, _) => vec![(flag_path.to_string(), json!(false))],
                };
                plain.push((key, ChoiceStep::Set(sets)));
                continue;
            }
            if group_of(&key).is_some() {
                let shown_on = shown.get(&key).and_then(Value::as_str) == Some("true");
                if unticked {
                    if ticked_groups.contains(&group_of(&key).unwrap().key) || !shown_on {
                        continue;
                    }
                    // Unticking the business type of a mixed-income filer
                    // leaves Compensation Earner ticked.
                    let rest = if mixed && business {
                        json!("CompensationEarner")
                    } else {
                        Value::Null
                    };
                    plain.push((
                        key,
                        ChoiceStep::Set(vec![
                            (type_path.to_string(), rest),
                            (flag_path.to_string(), json!(false)),
                        ]),
                    ));
                } else {
                    let pure_comp = current == "CompensationEarner";
                    let mut sets: Vec<(String, Value)> = Vec::new();
                    let option = match def.bindings.get(&key).map(|b| &b.binding) {
                        Some(Binding::Choice { options }) => {
                            options.iter().find(|o| o.value == "true")
                        }
                        _ => None,
                    };
                    let picked = option
                        .and_then(|o| o.set.get(type_path))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let picked_business =
                        matches!(picked.as_str(), "SingleProprietor" | "Professional");
                    sets.push((type_path.to_string(), json!(picked)));
                    sets.push((
                        flag_path.to_string(),
                        json!(picked_business && (mixed || pure_comp)),
                    ));
                    plain.push((key, ChoiceStep::Set(sets)));
                }
                continue;
            }
        }
        match group_info(&key) {
            Some((group, members)) if unticked => {
                if ticked_groups.contains(&group)
                    || shown.get(&key).and_then(Value::as_str) != Some("true")
                {
                    continue;
                }
                // 2200S item 17 is a tick box of its own (PR 11): unticking
                // it writes its false option, not the group's blank.
                if rows_excise::untick_writes_its_option(kind, &key) {
                    plain.push((key, ChoiceStep::Fill(value)));
                    continue;
                }
                // Decision P4-18, judged when the step runs. A box with no
                // tick of its own to undo (0605's quarter, written by the
                // host) takes its "false" as the host writes it.
                if members >= 3 && !true_option_paths(def, &key).is_empty() {
                    plain.push((key, ChoiceStep::Untick));
                } else {
                    plain.push((key, ChoiceStep::Fill(value)));
                }
            }
            Some(_) => plain.push((key, ChoiceStep::Fill(value))),
            None => plain.push((key, ChoiceStep::Fill(value))),
        }
    }
    plain
}

fn screen_form_validate(
    arguments: &HostArguments,
) -> Result<impl Future<Output = HostResult> + Send + use<>, HostError> {
    let form_code = arguments.string(0)?.to_string();
    let kind = kind_of(&form_code)?;
    let mut draft = draft_argument(arguments, 1)?;
    let sample = take_sample_mark(&mut draft);
    let amendment = take_amendment_mark(&mut draft);
    // Read here, on the host's thread: the evaluation runs on its own.
    let rows_checked = with_database(|db| Ok(rows_1604::checked_rows(db, kind, &draft)))?;
    let paid = sample.is_none() && with_database(|db| Ok(sent_and_paid(db, kind, &draft)))?;
    Ok(async move {
        let checked = on_large_stack(move || -> Result<Value, String> {
            let mut draft = draft;
            let (mut values, mut errors, status) =
                evaluate_here(kind, &mut draft, &rows_checked, paid)?;
            if let Some(mark) = &sample {
                blank_sample(&mark.blank, &mut values, &mut errors);
            }
            let options = catalogs::options(kind, &draft);
            let off = if status == FilingStatus::Draft {
                conditions::off_boxes(kind, &draft, &values)
            } else {
                Map::new()
            };
            let blank = sample.as_ref().map(|mark| mark.blank.clone());
            let penalty_groups = penalties::json(kind, &draft);
            let pool_texts = combs::pool_texts(kind, &draft);
            put_sample_mark(&mut draft, sample.as_ref());
            put_amendment_mark(&mut draft, amendment);
            Ok(json!({
                "draft": draft,
                "values": values,
                "options": options,
                "errors": errors,
                "off": off,
                "penalties": penalty_groups,
                "pools": pool_texts,
                "blank": blank,
            }))
        })
        .and_then(|result| result)
        .map_err(host_error)?;
        Ok(json(checked))
    })
}

fn screen_form_save(arguments: &HostArguments) -> HostResult {
    let kind = kind_of(arguments.string(0)?)?;
    let mut draft = draft_argument(arguments, 1)?;
    if let Some(mark) = take_sample_mark(&mut draft) {
        return save_sample(kind, draft, &mark);
    }
    let amendment = take_amendment_mark(&mut draft);
    let id = with_database(|db| {
        // 1600PT/1600VT: a Part I key the stored return lacks stays out while
        // blank, whatever the page sent (P2-4; the fill does the same,
        // rows_monthly::keep_absent_blank). An amendment's stored return is
        // the filed original it amends, until its first save.
        if rows_monthly::keeps_part_i(kind)
            && let Some(stored) = stored_return(db, kind, &draft).map_err(host_error)?
        {
            rows_monthly::keep_absent_blank(
                &rows_monthly::absent_part_i(kind, &stored),
                &mut draft,
            );
        }
        match &amendment {
            Some(mark) => {
                settle_trigger_flag(db, kind, &mut draft).map_err(host_error)?;
                reapply_penalty_context(db, kind, &mut draft).map_err(host_error)?;
                save_amendment(db, kind, &mut draft, mark).map_err(host_error)
            }
            None => {
                settle_trigger_flag(db, kind, &mut draft).map_err(host_error)?;
                reapply_penalty_context(db, kind, &mut draft).map_err(host_error)?;
                refuse_replacing_a_stored_return(db, kind, &draft).map_err(host_error)?;
                agent::save_draft(kind, db, &mut draft).map_err(host_error)
            }
        }
    })?;
    let status = agent::identity(kind, &draft).map_err(host_error)?.status;
    Ok(json(json!({
        "id": id,
        "status": status_name(&status),
        "stored_period": stored_period(kind, &draft),
        "draft": draft,
    })))
}

/// Save a Form library sample (decided 2026-10-17): once its period is set
/// it becomes that period's return, with everything typed into it. The
/// matching rules come first: when the period already has a stored return
/// nothing is saved and the answer is `{saved: false, existing, opens}`, the
/// offer the page shows; a sample never replaces a stored return (the save
/// guard stays). An entry-numbered form, which never matches, takes the
/// year's next free entry. Saved: `{saved: true, id, status,
/// stored_period, draft}`, the draft no longer a sample.
fn save_sample(kind: AgentFormKind, mut draft: Value, mark: &SampleMark) -> HostResult {
    let code = kind.form_code();
    if !mark.set {
        return Err(host_error(format!(
            "set this {code} sample's period first: Save makes it the return for that period"
        )));
    }
    with_database(|db| {
        let numbered = NUMBERED_FORMS.contains(&code);
        let stored = if numbered {
            stored_for_date(db, kind, &draft).map_err(host_error)?
        } else {
            stored_for_period(db, kind, &draft).map_err(host_error)?
        };
        if let Some(stored) = stored {
            return Ok(json(json!({
                "saved": false,
                "existing": existing_return(db, kind, &stored).map_err(host_error)?,
                "opens": opens_for(kind, &stored).map_err(host_error)?,
            })));
        }
        if numbered {
            next_free_entry(db, kind, &mut draft).map_err(host_error)?;
        }
        refuse_replacing_a_stored_return(db, kind, &draft).map_err(host_error)?;
        settle_trigger_flag(db, kind, &mut draft).map_err(host_error)?;
        reapply_penalty_context(db, kind, &mut draft).map_err(host_error)?;
        let id = agent::save_draft(kind, db, &mut draft).map_err(host_error)?;
        let status = agent::identity(kind, &draft).map_err(host_error)?.status;
        Ok(json(json!({
            "saved": true,
            "id": id,
            "status": status_name(&status),
            "stored_period": stored_period(kind, &draft),
            "opens": opens_for(kind, &draft).map_err(host_error)?,
            "draft": draft,
        })))
    })
}

/// What `screen_form_open` takes to open exactly this stored return: its
/// year, the page period, and its stored key. An entry-numbered form's
/// key is its entry (the page period too); any other open-ended key of
/// 101 or more is a transaction's `MMDD`, whose page period is the month.
fn opens_for(kind: AgentFormKind, stored: &Value) -> Result<Value, String> {
    let identity = agent::identity(kind, stored)?;
    let numbered = NUMBERED_FORMS.contains(&kind.form_code());
    let period = match identity.period {
        FilingPeriod::Monthly(month) => u32::from(month),
        FilingPeriod::Quarterly(quarter) => u32::from(quarter),
        FilingPeriod::Annual => 0,
        FilingPeriod::OpenEnded(key) if key >= 101 && !numbered => key / 100,
        FilingPeriod::OpenEnded(key) => key,
    };
    Ok(json!({
        "year": identity.taxable_year,
        "period": period,
        "key": stored_period(kind, stored),
    }))
}

/// The stored return of an entry-numbered form (1606, 1706, 1707, 1707A,
/// 1800, 1801) for exactly the transaction date its draft's period parts
/// hold: the same taxpayer, year and date. `None` without a typed day
/// (0605), or when no entry of the year has that date: a new transaction.
fn stored_for_date(
    db: &Database,
    kind: AgentFormKind,
    draft: &Value,
) -> Result<Option<Value>, String> {
    let code = kind.form_code();
    let parts = sample_period_parts(kind);
    let typed_of = |draft: &Value| -> Result<TypedPeriod, String> {
        let values = values_json(kind, draft)?;
        let values = values.as_object().cloned().unwrap_or_default();
        Ok(read_typed_period(code, &parts, &values))
    };
    let typed = typed_of(draft)?;
    if typed.day.is_none() {
        return Ok(None);
    }
    let identity = agent::identity(kind, draft)?;
    let AgentFormKind::Queueable(queueable) = kind else {
        return Ok(None);
    };
    // Every stored entry of the year, read once, matched in memory: an
    // entry past MAX_ENTRY (stored by another writer) counts too, so a
    // same-date return is never stored twice.
    let rows: Vec<(i64, Value)> = bir_core::with_queueable_kind!(queueable, F => out_of_line(|| {
        db.list_queueable_drafts::<F>(&identity.tin, identity.taxable_year)
            .map_err(|error| read_error(code, error))?
            .into_iter()
            .map(|(entry, row)| {
                serde_json::to_value(row)
                    .map(|row| (entry, row))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, String>>()
    })?);
    for (entry, row) in rows {
        if typed_of(&row)? != typed {
            continue;
        }
        if entry > i64::from(MAX_ENTRY) {
            return Err(format!(
                "a {code} return for this date is already stored as entry {entry}, past entry {MAX_ENTRY}, which this app cannot open; it is not stored again"
            ));
        }
        return Ok(Some(row));
    }
    Ok(None)
}

/// The highest entry number a new entry takes: a page period (`u8`), so
/// every entry opens with its own number. Stored entries past it are still
/// matched by date (`stored_for_date`), never stored again.
const MAX_ENTRY: u32 = 255;

/// Where an entry-numbered draft keeps its entry number.
fn entry_pointer(draft: &Value) -> &'static str {
    if draft.pointer("/open_ended_key").is_some() {
        "/open_ended_key"
    } else {
        // 0605 keeps its entry number in `month`.
        "/month"
    }
}

/// An entry-numbered form's sample (0605, 1606, 1706, 1707, 1707A, 1800,
/// 1801) that matched no stored transaction takes the first entry of its
/// year with no stored return.
fn next_free_entry(db: &Database, kind: AgentFormKind, draft: &mut Value) -> Result<(), String> {
    let code = kind.form_code();
    let pointer = entry_pointer(draft);
    for entry in 1..=MAX_ENTRY {
        let mut candidate = draft.clone();
        set_draft_path(&mut candidate, pointer, json!(entry));
        let identity = agent::identity(kind, &candidate)?;
        if identity.period != FilingPeriod::OpenEnded(entry) {
            return Err(format!("{code}: no entry number to save the sample under"));
        }
        if stored_return(db, kind, &candidate)?.is_none() {
            *draft = candidate;
            return Ok(());
        }
    }
    Err(format!("{code}: every entry number of the year is taken"))
}

/// When the draft was first made (`created_at`, top level or in its
/// lifecycle).
fn created_at(draft: &Value) -> Option<&str> {
    draft["created_at"]
        .as_str()
        .or_else(|| draft["lifecycle"]["created_at"].as_str())
}

/// The stored return for a draft's TIN, form, year and period, if any.
fn stored_return(
    db: &Database,
    kind: AgentFormKind,
    draft: &Value,
) -> Result<Option<Value>, String> {
    use bir_core::forms::*;
    let identity = agent::identity(kind, draft)?;
    let (tin, year) = (identity.tin.as_str(), identity.taxable_year);
    let column = bir_core::forms::queueable::period_column(&identity.period);
    let slot = u8::try_from(column).unwrap_or(0);
    fn to_value<T: serde::Serialize>(draft: Option<T>) -> Result<Option<Value>, String> {
        draft
            .map(|draft| serde_json::to_value(draft).map_err(|error| error.to_string()))
            .transpose()
    }
    match kind {
        AgentFormKind::Form1601C => out_of_line(|| {
            to_value(
                db.get_1601c_draft(tin, year, slot)
                    .map_err(|error| read_error("1601C", error))?,
            )
        }),
        AgentFormKind::Form2551Q => out_of_line(|| {
            to_value(
                db.get_2551q_draft(tin, year, slot)
                    .map_err(|error| read_error("2551Q", error))?,
            )
        }),
        AgentFormKind::Queueable(QueueableKind::Form2550Q) => out_of_line(|| {
            to_value(
                db.get_form_draft::<form_2550q::Form2550QDraft>(tin, "2550Q", year, Some(slot))
                    .map_err(|error| read_error("2550Q", error))?,
            )
        }),
        AgentFormKind::Queueable(queueable) => {
            bir_core::with_queueable_kind!(queueable, F => out_of_line(|| {
                let found = match stored::<F>(Some(db), tin, year, column)? {
                    Some(found) => Some(found),
                    None => db
                        .get_form_draft_v2::<F>(tin, F::FORM_CODE, year, &identity.period)
                        .map_err(|error| read_error(F::FORM_CODE, error))?,
                };
                to_value(found)
            }))
        }
    }
}

/// The stored return a sample's completed period matches: the return of the
/// same period (monthly: month and year; quarterly: quarter and year;
/// annual: year; per transaction: its date). How often the form is filed
/// is the screen index's (`freq`): a monthly form whose rows are keyed by
/// date (`MMDD`: 2200AN, 2200C, 2200S) matches any day of the month, the
/// day does not matter; an "As needed" form keyed by its transaction date
/// (2000OT, 2552) matches only that exact date, so a second transaction in
/// the same month is a return of its own.
fn stored_for_period(
    db: &Database,
    kind: AgentFormKind,
    draft: &Value,
) -> Result<Option<Value>, String> {
    let identity = agent::identity(kind, draft)?;
    let monthly =
        schema_entry(kind.form_code()).and_then(|form| form["freq"].as_str()) == Some("Monthly");
    match (kind, identity.period) {
        (AgentFormKind::Queueable(queueable), FilingPeriod::OpenEnded(key))
            if monthly && key >= 101 =>
        {
            let month = i64::from(key / 100);
            bir_core::with_queueable_kind!(queueable, F => out_of_line(|| {
                let found = latest::<F>(
                    Some(db),
                    &identity.tin,
                    identity.taxable_year,
                    mmdd_keys(month..=month, 0..=31),
                    |_| true,
                )?;
                found
                    .map(|found| serde_json::to_value(found).map_err(|error| error.to_string()))
                    .transpose()
            }))
        }
        _ => stored_return(db, kind, draft),
    }
}

/// What a save may write. A draft saves over the stored return of its TIN,
/// form, year and period only when that row is still a Draft and is this
/// very draft: the same row id; for 2550Q, whose rows keep no id, the same
/// `created_at`. A draft with an id saves only to that row. A filed or
/// queued return never changes here (an amendment does, through its own
/// save).
fn refuse_replacing_a_stored_return(
    db: &Database,
    kind: AgentFormKind,
    draft: &Value,
) -> Result<(), String> {
    let code = kind.form_code();
    let id = draft["id"].as_i64();
    let Some(stored) = stored_return(db, kind, draft)? else {
        return match id {
            Some(id) => Err(format!(
                "this {code} draft names row {id}, which is not stored for its period; open the return again"
            )),
            None => Ok(()),
        };
    };
    let status = agent::identity(kind, &stored)?.status;
    if status != FilingStatus::Draft {
        return Err(format!(
            "the {code} return for this period is {}; it can no longer change here (amend it instead)",
            status_name(&status)
        ));
    }
    let same_2550q = kind == AgentFormKind::Queueable(QueueableKind::Form2550Q)
        && created_at(&stored).is_some()
        && created_at(&stored) == created_at(draft);
    match (id, stored["id"].as_i64()) {
        (Some(id), Some(row)) if id == row => Ok(()),
        (Some(id), Some(row)) => Err(format!(
            "this {code} draft names row {id}, but its period is stored as row {row}; open the return again"
        )),
        (_, None) if same_2550q => Ok(()),
        _ => Err(format!(
            "a {code} return for this period is already stored; open it from the taxpayer's returns to change it"
        )),
    }
}

fn filing_status_named(name: &str) -> Option<FilingStatus> {
    Some(match name {
        "Draft" => FilingStatus::Draft,
        "Queued" => FilingStatus::Queued,
        "Submitted" => FilingStatus::Sent,
        "Confirmed" => FilingStatus::Confirmed,
        "Paid" => FilingStatus::Paid,
        _ => return None,
    })
}

/// An amendment's first save: in one transaction, the user's revert of the
/// filed return (kept byte for byte as its amendment snapshot) and this
/// draft written in its row. Refused, with nothing changed, if the filed row
/// is no longer the one the amendment was copied from (the mark's status and
/// `updated_at`).
fn save_amendment(
    db: &Database,
    kind: AgentFormKind,
    draft: &mut Value,
    mark: &Value,
) -> Result<i64, String> {
    use bir_core::db::AmendmentBase;
    let base = AmendmentBase {
        status: mark["status"]
            .as_str()
            .and_then(filing_status_named)
            .ok_or("this amendment does not say which filed return it amends; amend it again")?,
        updated_at: mark["updated_at"].as_str().unwrap_or("").to_string(),
    };
    agent::compute(kind, draft)?;
    let error = |error: bir_core::db::DbError| error.to_string();
    let draft_ref = &*draft;
    let id = match kind {
        AgentFormKind::Form1601C => out_of_line(|| {
            db.save_1601c_amendment(
                &serde_json::from_value(draft_ref.clone()).map_err(|e| e.to_string())?,
                &base,
            )
            .map_err(error)
        })?,
        AgentFormKind::Form2551Q => out_of_line(|| {
            db.save_2551q_amendment(
                &serde_json::from_value(draft_ref.clone()).map_err(|e| e.to_string())?,
                &base,
            )
            .map_err(error)
        })?,
        AgentFormKind::Queueable(queueable) => {
            bir_core::with_queueable_kind!(queueable, F => out_of_line(|| {
            let typed: F = serde_json::from_value(draft_ref.clone()).map_err(|e| e.to_string())?;
            db.save_queueable_amendment(&typed, &base).map_err(error)
        })?)
        }
    };
    draft["id"] = Value::from(id);
    Ok(id)
}

/// Marks a new amended return (`screen_form_amend`) until its first save:
/// the filed return it amends, as it was (`status`, `updated_at`).
const AMENDMENT_MARK: &str = "screen_form_amendment";

fn take_amendment_mark(draft: &mut Value) -> Option<Value> {
    match draft {
        Value::Object(object) => object.remove(AMENDMENT_MARK).filter(|mark| !mark.is_null()),
        _ => None,
    }
}

fn put_amendment_mark(draft: &mut Value, mark: Option<Value>) {
    if let (Some(mark), Value::Object(object)) = (mark, draft) {
        object.insert(AMENDMENT_MARK.into(), mark);
    }
}

/// Open a NEW amended return for a stored return that reached BIR, copied
/// from it (`agent::amended_copy`). Nothing is stored and the filed return
/// is not touched until the amendment is first saved.
fn screen_form_amend(
    arguments: &HostArguments,
) -> Result<impl Future<Output = HostResult> + Send + use<>, HostError> {
    let tin: String = arguments
        .string(0)?
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let kind = kind_of(arguments.string(1)?)?;
    let code = kind.form_code();
    let year =
        u16::try_from(arguments.integer(2)?).map_err(|_| host_error("year is out of range"))?;
    let period = page_period(code, arguments.integer(3)?).map_err(host_error)?;
    let exact = arguments.get(4).and_then(|key| to_json(key).as_i64());
    let (amended, from_profile, status, updated_at) = with_database(|db| {
        let profile = db
            .get_profile_by_tin(&tin)
            .map_err(|error| host_error(error.to_string()))?
            .ok_or_else(|| host_error(format!("no taxpayer profile with TIN {tin}")))?;
        let filed =
            load_draft(db, kind, &profile, year, period, false, exact).map_err(host_error)?;
        let status = agent::identity(kind, &filed).map_err(host_error)?.status;
        let updated_at = filed["updated_at"]
            .as_str()
            .or_else(|| filed["lifecycle"]["updated_at"].as_str())
            .unwrap_or("")
            .to_string();
        let mut amended = agent::amended_copy(kind, &filed).map_err(host_error)?;
        // The amendment carries its filed return's typed parts, as opening
        // a stored return does, whether or not the filed return was opened
        // in this process.
        seed_amendment_parts(kind, &filed, &mut amended);
        let from_profile =
            load_draft(db, kind, &profile, year, period, true, None).map_err(host_error)?;
        Ok((amended, from_profile, status, updated_at))
    })?;
    Ok(async move {
        let mut opened = on_large_stack(move || {
            opened_json(
                kind,
                year,
                period,
                amended,
                from_profile,
                None,
                BTreeSet::new(),
                false,
            )
        })
        .and_then(|result| result)
        .map_err(host_error)?;
        put_amendment_mark(
            &mut opened["draft"],
            Some(json!({ "status": status_name(&status), "updated_at": updated_at })),
        );
        opened["amends"] = json!(status_name(&status));
        Ok(json(opened))
    })
}

/// The filed version an amended return keeps in its history (its newest
/// amendment snapshot): its id, status and when it was filed. Null for a
/// return that was never amended.
fn filed_original(db: &Database, kind: AgentFormKind, draft: &Value) -> Result<Value, String> {
    let identity = agent::identity(kind, draft)?;
    let snapshots = db
        .list_amendment_snapshots(
            kind.form_code(),
            &identity.tin,
            identity.taxable_year,
            &identity.period.to_period_key(),
        )
        .map_err(|error| error.to_string())?;
    Ok(match snapshots.last() {
        Some(snapshot) => json!({
            "id": snapshot.id,
            "status": snapshot.status_at_revert,
            "filed_on": snapshot.confirmed_at.clone().or_else(|| snapshot.submitted_at.clone()),
        }),
        None => Value::Null,
    })
}

/// "View the filed original": an amended return's filed version, read-only,
/// from its history (the amendment snapshot, exactly as it was stored).
fn screen_form_filed_original(
    arguments: &HostArguments,
) -> Result<impl Future<Output = HostResult> + Send + use<>, HostError> {
    let kind = kind_of(arguments.string(0)?)?;
    let id = arguments.integer(1)?;
    let (draft, snapshot) = with_database(|db| {
        let snapshot = db
            .get_amendment_snapshot(id)
            .map_err(|error| host_error(error.to_string()))?
            .filter(|snapshot| snapshot.form_code == kind.form_code())
            .ok_or_else(|| host_error(format!("no filed {} original #{id}", kind.form_code())))?;
        let draft: Value = serde_json::from_str(&snapshot.draft_json)
            .map_err(|error| host_error(error.to_string()))?;
        Ok((draft, snapshot))
    })?;
    let identity = agent::identity(kind, &draft).map_err(host_error)?;
    let year = identity.taxable_year;
    let period =
        u8::try_from(bir_core::forms::queueable::period_column(&identity.period)).unwrap_or(0);
    Ok(async move {
        let from_profile = draft.clone();
        let mut opened = on_large_stack(move || {
            opened_json(
                kind,
                year,
                period,
                draft,
                from_profile,
                None,
                BTreeSet::new(),
                false,
            )
        })
        .and_then(|result| result)
        .map_err(host_error)?;
        // Never a draft: what was filed, kept as it was.
        opened["editable"] = Value::Bool(false);
        opened["filed_original_view"] = json!({
            "status": snapshot.status_at_revert,
            "filed_on": snapshot.confirmed_at.or(snapshot.submitted_at),
        });
        Ok(json(opened))
    })
}

/// The print sheet's field map and receipt page for a draft as saved.
pub(crate) fn print_map(
    db: &Database,
    kind: AgentFormKind,
    draft: &Value,
) -> Result<
    (
        BTreeMap<String, String>,
        Option<bir_print::frozen_html::ReceiptPage>,
    ),
    HostError,
> {
    fn typed<F: serde::de::DeserializeOwned>(draft: &Value) -> Result<F, HostError> {
        serde_json::from_value(draft.clone()).map_err(|error| host_error(error.to_string()))
    }
    match kind {
        AgentFormKind::Form1601C => out_of_line(|| {
            let d: bir_core::forms::Form1601CDraft = typed(draft)?;
            let page = crate::bir_module::receipt_page(db, d.receipt_id, &d.email_address);
            Ok((d.to_print_field_map(), page))
        }),
        AgentFormKind::Form2551Q => out_of_line(|| {
            let d: bir_core::forms::Form2551QDraft = typed(draft)?;
            let page = crate::bir_module::receipt_page(db, d.receipt_id, &d.email);
            Ok((d.to_print_field_map(), page))
        }),
        AgentFormKind::Queueable(queueable) => {
            bir_core::with_queueable_kind!(queueable, F => out_of_line(|| {
                let d: F = typed(draft)?;
                let page = crate::bir_module::receipt_page(
                    db,
                    d.lifecycle().receipt_id,
                    d.submission_email(),
                );
                Ok((d.print_field_map(), page))
            }))
        }
    }
}

/// Print preview of the draft the page holds (save it first). Works from the
/// draft itself, so forms stored under a transaction-date key print too.
fn screen_form_print(arguments: &HostArguments) -> HostResult {
    let code = arguments.string(0)?.to_owned();
    let kind = kind_of(&code)?;
    let mut draft = draft_argument(arguments, 1)?;
    refuse_sample(&mut draft, "printed")?;
    take_amendment_mark(&mut draft);
    let slug = crate::bir_module::print_slug_for(kind.form_code())
        .ok_or_else(|| host_error(format!("Form {code} has no print sheet")))?;
    let identity = agent::identity(kind, &draft).map_err(host_error)?;
    let year = identity.taxable_year;
    with_database(|db| {
        let (fields, receipt) = print_map(db, kind, &draft)?;
        let profile = db
            .get_profile_by_tin(&identity.tin)
            .ok()
            .flatten()
            .map(|p| p.full_name.clone());
        let (html, title) = crate::bir_module::print_preview_page(
            slug,
            kind.form_code(),
            &fields,
            receipt.as_ref(),
            profile,
        )?;
        let _ = year;
        Ok(HostObject::new()
            .field("html", html)
            .field("title", title)
            .into())
    })
}

/// The page's draft (argument 1) with its staged edits (argument 2, the
/// boxes opened with Edit as argument 3) applied to a copy, recomputed, in
/// memory. A Form library sample's mark comes back with the held boxes the
/// edits did not fill, which print blank as the page shows them.
fn draft_with_staged_edits(
    kind: AgentFormKind,
    arguments: &HostArguments,
) -> Result<(Value, Option<SampleMark>), HostError> {
    let mut draft = draft_argument(arguments, 1)?;
    let mut sample = take_sample_mark(&mut draft);
    take_amendment_mark(&mut draft);
    if let Some(edits) = arguments.value(2).ok().and_then(|v| v.as_object()) {
        let edits: Map<String, Value> =
            edits.iter().map(|(k, v)| (k.clone(), to_json(v))).collect();
        if !edits.is_empty() {
            let unlocked = unlocked_argument(arguments, 3);
            let rejected =
                apply_screen_edits(kind, &mut draft, &edits, true, &unlocked).unwrap_or_default();
            let _ = agent::compute(kind, &mut draft);
            if let Some(mark) = &mut sample {
                mark.blank
                    .retain(|key| !edits.contains_key(key) || rejected.contains_key(key));
            }
        }
    }
    Ok((draft, sample))
}

/// The print sheet's fields for `draft`; a sample's held boxes print blank.
fn page_print_map(
    db: &Database,
    kind: AgentFormKind,
    draft: &Value,
    sample: Option<&SampleMark>,
) -> Result<
    (
        BTreeMap<String, String>,
        Option<bir_print::frozen_html::ReceiptPage>,
    ),
    HostError,
> {
    let (mut fields, receipt) = print_map(db, kind, draft)?;
    if let Some(mark) = sample {
        for key in &mark.blank {
            if let Some(value) = fields.get_mut(key) {
                value.clear();
            }
        }
    }
    Ok((fields, receipt))
}

/// Print preview of a Form library sample as the page holds it (decided
/// 2026-10-22): its draft with the staged edits, in memory, its held boxes
/// blank. It needs no period and writes nothing to the database: no return
/// is stored. Edits then reach it through `screen_form_preview_sync`.
fn screen_form_print_sample(arguments: &HostArguments) -> HostResult {
    let code = arguments.string(0)?.to_owned();
    let kind = kind_of(&code)?;
    let (draft, sample) = draft_with_staged_edits(kind, arguments)?;
    if sample.is_none() {
        return Err(host_error(format!(
            "Form {code}: only a Form library sample previews unsaved; print a return with screen_form_print"
        )));
    }
    let slug = crate::bir_module::print_slug_for(kind.form_code())
        .ok_or_else(|| host_error(format!("Form {code} has no print sheet")))?;
    let identity = agent::identity(kind, &draft).map_err(host_error)?;
    with_database(|db| {
        let (fields, receipt) = page_print_map(db, kind, &draft, sample.as_ref())?;
        let profile = db
            .get_profile_by_tin(&identity.tin)
            .ok()
            .flatten()
            .map(|p| p.full_name.clone());
        let (html, title) = crate::bir_module::print_preview_page(
            slug,
            kind.form_code(),
            &fields,
            receipt.as_ref(),
            profile,
        )?;
        Ok(HostObject::new()
            .field("html", html)
            .field("title", title)
            .into())
    })
}

/// Rox's print preview (`bir_app_print_preview`): the page and title that
/// `screen_form_print` (a return as saved) or `screen_form_print_sample`
/// (a Form library sample with its staged edits, held boxes blank) shows,
/// made in memory: no window opens and no file is written.
pub(crate) fn print_preview_page(arguments: &HostArguments) -> Result<(String, String), HostError> {
    let code = arguments.string(0)?.to_owned();
    let kind = kind_of(&code)?;
    let slug = crate::bir_module::print_slug_for(kind.form_code())
        .ok_or_else(|| host_error(format!("Form {code} has no print sheet")))?;
    let mut draft = draft_argument(arguments, 1)?;
    let (draft, sample) = if take_sample_mark(&mut draft.clone()).is_some() {
        draft_with_staged_edits(kind, arguments)?
    } else {
        take_amendment_mark(&mut draft);
        (draft, None)
    };
    let identity = agent::identity(kind, &draft).map_err(host_error)?;
    with_database(|db| {
        let (fields, receipt) = page_print_map(db, kind, &draft, sample.as_ref())?;
        let profile = db
            .get_profile_by_tin(&identity.tin)
            .ok()
            .flatten()
            .map(|p| p.full_name.clone());
        crate::bir_module::print_preview_page(
            slug,
            kind.form_code(),
            &fields,
            receipt.as_ref(),
            profile,
        )
    })
}

/// Queue the saved draft the page holds for background submission. 1601C and
/// 2551Q use their own queue path (`queue_form_draft`); every other form is
/// queued from the draft itself, after it validates.
fn screen_form_queue(arguments: &HostArguments) -> HostResult {
    let code = arguments.string(0)?.to_owned();
    let kind = kind_of(&code)?;
    let mut draft = draft_argument(arguments, 1)?;
    refuse_sample(&mut draft, "queued")?;
    take_amendment_mark(&mut draft);
    let AgentFormKind::Queueable(queueable) = kind else {
        return Err(host_error(format!(
            "{code} is queued with queue_form_draft (month or quarter)"
        )));
    };
    let queued = with_database(|db| {
        settle_trigger_flag(db, kind, &mut draft).map_err(host_error)?;
        reapply_penalty_context(db, kind, &mut draft).map_err(host_error)?;
        bir_core::with_queueable_kind!(queueable, F => out_of_line(|| {
            let mut typed: F = serde_json::from_value(draft.clone())
                .map_err(|error| host_error(error.to_string()))?;
            if typed.draft_id().is_none() {
                return Err(host_error("Save the draft before queueing it."));
            }
            typed
                .queue(bir_core::filing_queue::QueueAuthSource::Gui)
                .map_err(|errors| {
                    host_error(
                        errors
                            .iter()
                            .map(|(field, message)| format!("{field}: {message}"))
                            .collect::<Vec<_>>()
                            .join("; "),
                    )
                })?;
            db.save_queued_queueable(&typed)
                .map(|id| HostValue::Number(id as f64))
                .map_err(|error| host_error(error.to_string()))
        }))
    })?;
    crate::database::wake_submission_worker();
    Ok(queued)
}

/// Take a queued return out of the queue so it is an editable draft again.
/// 1601C and 2551Q use `cancel_queued`.
fn screen_form_cancel_queue(arguments: &HostArguments) -> HostResult {
    let code = arguments.string(0)?.to_owned();
    let kind = kind_of(&code)?;
    let mut draft = draft_argument(arguments, 1)?;
    refuse_sample(&mut draft, "taken out of the queue")?;
    let AgentFormKind::Queueable(queueable) = kind else {
        return Err(host_error(format!(
            "{code} is cancelled with cancel_queued"
        )));
    };
    let cancelled = with_database(|db| {
        settle_trigger_flag(db, kind, &mut draft).map_err(host_error)?;
        reapply_penalty_context(db, kind, &mut draft).map_err(host_error)?;
        bir_core::with_queueable_kind!(queueable, F => out_of_line(|| {
            let typed: F = serde_json::from_value(draft.clone())
                .map_err(|error| host_error(error.to_string()))?;
            db.cancel_queued_queueable(&typed)
                .map(|_| HostValue::Null)
                .map_err(|error| host_error(error.to_string()))
        }))
    })?;
    crate::database::wake_submission_worker();
    Ok(cancelled)
}

/// The period column the draft is stored under, which is what the queue and
/// print host functions look it up by. It differs from the page's period on
/// the forms keyed by a transaction date (`MMDD`: 2000OT, 2200AN, 2200C,
/// 2200S, 2552) and by a quarter the return names itself.
fn stored_period(kind: AgentFormKind, draft: &Value) -> Option<i64> {
    agent::identity(kind, draft)
        .ok()
        .map(|identity| bir_core::forms::queueable::period_column(&identity.period))
}
