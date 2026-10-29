//! PR 4: the monthly DST, percentage tax and VAT returns (2000, 2553,
//! 2550M). Boxes the host writes through the typed draft because the probed
//! bindings cannot say what the printed sheet says, the boxes that wait for
//! another box of their row, and the answers that remove what the filer
//! typed.
//!
//! - 2000 Schedules 2, 3 and 4 belong to the Item 13 mode that opens them
//!   (`Form2000AffixtureMode::schedules`, the official
//!   `processModeAffixture`); `recompute` clears a schedule its mode does not
//!   use, so a write there before Item 13 opens it is refused with what to
//!   answer, never taken and silently cleared.
//! - 2553 rows 14 to 17: the July 1999 sheet preprints each row's
//!   classification, ATC and rate (html-frozen/2553-1999/index.html: Pagcor
//!   OT010 5%, Clark Development Corporations OT011 5%, Special/Regular
//!   Economic/Free Port Zone Enterprises OT011 5%, Others OT012), and the
//!   print fills only their amounts (writer-cells.json rule: "rows 14-17
//!   expose only some columns"), next to that static text. So a new entry
//!   on rows 14-17 is that row's printed entry: typing a blank row's
//!   taxable amount gives it its printed ATC through `Form2553Draft::set_atc`
//!   (the rate and the tax due follow), unless another row already holds
//!   it, and the row's ATC and preprinted rate are shut. A stored row that
//!   holds another entry (the official popup lets any row hold any entry)
//!   opens unchanged with its ATC open and a notice ([`notices`]) that the
//!   sheet prints the row's own entry; the filer clears it or sets the
//!   printed one. Row 18 is the free row: its ATC is a choice from the
//!   official popup's list (`FORM_2553_ATC_OPTIONS`, `getATCCode`).
//! - 2553 Item 13: Yes opens the list blank (`YesUnspecified`, which
//!   validation asks for), No closes it; while it is No the list takes
//!   nothing (fields.json `enabled_when`: "Item 13 Yes is selected."), so
//!   rule 6 shows it off.
//! - 2553 Item 23's boxes take a choice only while Item 23 is an
//!   overpayment (the official `checkOverpayment`).
//! - An answer that removes what the filer typed (2000 Item 13's schedule
//!   rows, item 10's other party, an Amended Return "No" that clears 2000's
//!   15A, 2553's 20A or 2550M's 23D, 2553 item 13 "No" clearing a chosen
//!   relief, a cleared 2000, 2553 or 2550M ATC with its amount) is
//!   reported ([`removes`]) so the page asks first and offers Undo, the way
//!   PR 2's `rows_withholding::removes` does for the quarterly withholding
//!   returns.
//!
//! Field keys, the official XML and the print map are the forms' own and
//! unchanged.

// The locks themselves — 2000's 15B and 15C, 2553's classification,
// printed-row and quarter boxes — and the 2553 printed rows they follow are
// bir-core's (`agent::draft_locks::dst_pt_vat`, decision P4-19), so the
// page and the assistant lock alike; the cells here write against them.
pub(super) use bir_core::forms::agent::draft_locks::dst_pt_vat::PT_PRINTED_ROWS;
use bir_core::forms::agent::draft_locks::dst_pt_vat::{
    PT_FIRST_ITEM, PT_ROWS, misplaced_entry, printed_option, same_entry,
};
use bir_core::forms::agent::{self, AgentFormKind};
use bir_core::forms::form_2000::Form2000Draft;
use bir_core::forms::form_2553::{FORM_2553_ATC_OPTIONS, Form2553AtcOption, Form2553Draft};
use serde_json::{Map, Value, json};

/// A box this module writes or closes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Cell {
    /// A 2000 Schedule 2, 3 or 4 box (`frm2000:sched<n>:...`).
    DstSchedule(u8),
    /// 2553 row `14 + .0` column C (taxable amount).
    PtAmount(usize),
    /// 2553 row `14 + .0` column D (tax rate).
    PtRate(usize),
    /// 2553 row `14 + .0` column B: the row's ATC from the popup's list.
    PtAtc(usize),
    /// 2553 item 13 Yes (`true`) or No.
    PtItem13(bool),
    /// 2553 item 13's list (`lstTaxTreaty`).
    PtItem13List,
    /// 2553 item 23's overpayment boxes.
    PtOverpayment,
}

/// What a 2000 Schedule 1 rate waits for while it is blank (PR 2's words
/// for the same box on 1601EQ and 1601FQ).
pub(super) const RATE_WAITS: &str = "Shows the ATC's rate once this row's ATC is chosen.";
/// What 2553's item 18A waits for.
pub(super) const ATC_18_WAITS: &str = "Shows the classification of the ATC chosen in 18B.";
/// The 2553 rows' short printed names, rows 14-17.
const PT_SHORT_NAMES: [&str; 4] = [
    "Pagcor",
    "Clark Development Corporations",
    "Special/Regular Economic/Free Port Zone Enterprises",
    "Others",
];

/// What a row whose entry is held on another row says (the amount path and
/// the ATC list alike).
fn held_elsewhere(form: &Form2553Draft, row: usize, option: &Form2553AtcOption) -> Option<String> {
    form.schedule
        .iter()
        .enumerate()
        .find(|(other, entry)| *other != row && same_entry(entry, option))
        .map(|(other, _)| {
            format!(
                "Choose this row's ATC; {} is already on row {}.",
                option.code,
                PT_FIRST_ITEM + other
            )
        })
}

/// A stored row 14-17 holding another entry than the one the sheet prints
/// there (bir-core's `misplaced_entry`, which keeps its ATC and rate open):
/// what it says.
fn misplaced(form: &Form2553Draft, row: usize) -> Option<String> {
    let entry = misplaced_entry(form, row)?;
    let printed = printed_option(row)?;
    Some(format!(
        "Row {} prints {} ({}); your {} entry won't print on this row. Clear it or move it to row 18.",
        PT_FIRST_ITEM + row,
        printed.code,
        PT_SHORT_NAMES[row],
        entry.atc_code
    ))
}

/// The page's notices for a draft, shown with its messages: each stored
/// 2553 row 14-17 whose entry is not the one the sheet prints there (its
/// amount would print under another caption). Validation is unchanged.
pub(super) fn notices(kind: AgentFormKind, draft: &Value) -> Vec<Value> {
    if kind.form_code() != "2553" {
        return Vec::new();
    }
    let Ok(form) = typed::<Form2553Draft>(draft) else {
        return Vec::new();
    };
    (0..PT_PRINTED_ROWS.len())
        .filter_map(|row| {
            misplaced(&form, row).map(|message| {
                json!({
                    "field": format!("schedule[{row}].atc_code"),
                    "message": message,
                    "keys": [format!("frm2553:txt{}B", PT_FIRST_ITEM + row)],
                })
            })
        })
        .collect()
}

/// What a schedule the 2000 Item 13 answer has not opened says when it is
/// written to.
pub(super) fn dst_schedule_closed(schedule: u8) -> &'static str {
    match schedule {
        2 => "Schedule 2 is for Constructive Affixture: choose it at item 13 first.",
        3 => "Schedule 3 is for the eDST System: choose it at item 13 first.",
        _ => {
            "Schedule 4 is for Constructive Affixture or Loose Stamps: choose one at item 13 first."
        }
    }
}

/// What 2553's item 23 boxes say while item 23 is not an overpayment.
pub(super) const NOT_OVERPAID: &str =
    "Item 23 is not an overpayment: mark a box here only when it is below zero.";

fn pt_item(key: &str, column: char) -> Option<usize> {
    let rest = key.strip_prefix("frm2553:txt")?;
    let n: usize = rest.strip_suffix(column)?.parse().ok()?;
    (PT_FIRST_ITEM..PT_FIRST_ITEM + PT_ROWS)
        .contains(&n)
        .then(|| n - PT_FIRST_ITEM)
}

pub(super) fn cell(kind: AgentFormKind, key: &str) -> Option<Cell> {
    match kind.form_code() {
        "2000" => {
            let rest = key.strip_prefix("frm2000:sched")?;
            let n = rest.chars().next()?.to_digit(10)?;
            (rest[1..].starts_with(':') && (2..=4).contains(&n))
                .then_some(Cell::DstSchedule(n as u8))
        }
        "2553" => match key {
            "frm2553:optTreaty_1" => Some(Cell::PtItem13(true)),
            "frm2553:optTreaty_2" => Some(Cell::PtItem13(false)),
            "frm2553:lstTaxTreaty" => Some(Cell::PtItem13List),
            "frm2553:ifoverpay_1" | "frm2553:ifoverpay_2" => Some(Cell::PtOverpayment),
            _ => pt_item(key, 'B')
                .map(Cell::PtAtc)
                .or_else(|| pt_item(key, 'C').map(Cell::PtAmount))
                .or_else(|| pt_item(key, 'D').map(Cell::PtRate)),
        },
        _ => None,
    }
}

/// Whether the host writes `key` itself (its "cannot be filled here" lock
/// does not apply on the page). A 2000 schedule box and a 2553 item 23 box
/// are written by the ordinary fill once their answer holds.
pub(super) fn host_writes(kind: AgentFormKind, key: &str) -> bool {
    cell(kind, key).is_some_and(|cell| !matches!(cell, Cell::DstSchedule(_) | Cell::PtOverpayment))
}

/// A row's ATC is chosen, or its amount typed, before its rate.
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    match cell(kind, key)? {
        Cell::PtAmount(_) => Some(1),
        Cell::PtRate(_) => Some(2),
        _ => None,
    }
}

fn ticked(value: &Value) -> bool {
    matches!(value, Value::Bool(true)) || value.as_str().is_some_and(|s| s.trim() == "true")
}

fn text(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        other => other.as_str().unwrap_or_default().trim().to_string(),
    }
}

fn typed<T: serde::de::DeserializeOwned>(draft: &Value) -> Result<T, String> {
    serde_json::from_value(draft.clone()).map_err(|e| e.to_string())
}

fn store<T: serde::Serialize>(draft: &mut Value, form: &T) -> Result<(), String> {
    *draft = serde_json::to_value(form).map_err(|e| e.to_string())?;
    Ok(())
}

/// A typed amount: blank is zero.
fn amount(value: &Value) -> Result<f64, String> {
    let typed_text = text(value);
    if typed_text.is_empty() {
        return Ok(0.0);
    }
    agent::parse_amount(&typed_text)
        .filter(|amount| *amount >= 0.0)
        .ok_or_else(|| format!("`{typed_text}` is not an amount."))
}

/// Write `key` when this module owns it: `None` leaves it to the ordinary
/// fill. `ordinary` is that fill, for a box this module only gates.
pub(super) fn fill(
    kind: AgentFormKind,
    draft: &mut Value,
    key: &str,
    value: &Value,
    ordinary: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Option<Result<(), String>> {
    let cell = cell(kind, key)?;
    Some(match cell {
        Cell::DstSchedule(schedule) => fill_dst_schedule(draft, schedule, value, ordinary),
        Cell::PtOverpayment => {
            let overpaid = draft["total_amount_payable"].as_f64().unwrap_or(0.0) < 0.0;
            if ticked(value) && !overpaid {
                Err(NOT_OVERPAID.to_string())
            } else {
                ordinary(draft)
            }
        }
        Cell::PtAmount(row) => fill_pt_amount(draft, row, value),
        Cell::PtRate(row) => fill_pt_rate(draft, row, value),
        Cell::PtAtc(row) => fill_pt_atc(draft, row, value),
        Cell::PtItem13(yes) => fill_pt_item_13(draft, yes, value),
        Cell::PtItem13List => fill_pt_item_13_list(draft, value),
    })
}

fn fill_dst_schedule(
    draft: &mut Value,
    schedule: u8,
    value: &Value,
    ordinary: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    let form: Form2000Draft = typed(draft)?;
    let (two, three, four) = form.affixture_mode.schedules();
    let open = match schedule {
        2 => two,
        3 => three,
        _ => four,
    };
    let blank = {
        let shown = text(value);
        shown.is_empty() || agent::parse_amount(&shown) == Some(0.0)
    };
    if !open && !blank {
        return Err(dst_schedule_closed(schedule).to_string());
    }
    ordinary(draft)
}

/// A 2553 row 14-17's taxable amount (or row 18's once its ATC is chosen).
fn fill_pt_amount(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let amount = amount(value)?;
    let mut form: Form2553Draft = typed(draft)?;
    let held = form
        .schedule
        .get(row)
        .filter(|entry| !entry.is_empty())
        .cloned();
    match (held, printed_option(row)) {
        // Cleared: a row holding its printed ATC goes back to blank.
        (Some(entry), Some(printed))
            if amount == 0.0
                && entry.atc_code == printed.code
                && entry.description == printed.description =>
        {
            form.clear_atc(row);
        }
        (Some(_), _) => {
            form.schedule[row].taxable_amount = amount;
            form.recompute();
        }
        (None, _) if amount == 0.0 => return Ok(()),
        (None, Some(printed)) => {
            // A new entry is the row's printed one, unless another row
            // already holds it (the official popup's duplicate check);
            // refused, nothing is stored.
            if let Some(why) = held_elsewhere(&form, row, printed) {
                return Err(why);
            }
            form.set_atc(row, printed)?;
            form.schedule[row].taxable_amount = amount;
            form.recompute();
        }
        (None, None) => {
            return Err(format!("Choose the ATC in {}B first.", PT_FIRST_ITEM + row));
        }
    }
    store(draft, &form)
}

/// A 2553 row's rate: typed only for an ATC whose rate the filer enters
/// (OT012 Others, `rate_is_editable`); the others keep the official rate.
fn fill_pt_rate(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let rate = amount(value)?;
    let mut form: Form2553Draft = typed(draft)?;
    let item = PT_FIRST_ITEM + row;
    let option = match form.schedule.get(row).filter(|entry| !entry.is_empty()) {
        // Clearing the rate of a row holding its printed entry clears the
        // row, as clearing its amount does.
        Some(entry)
            if rate == 0.0
                && printed_option(row).is_some_and(|printed| same_entry(entry, printed)) =>
        {
            form.clear_atc(row);
            return store(draft, &form);
        }
        Some(entry) => FORM_2553_ATC_OPTIONS
            .iter()
            .find(|option| same_entry(entry, option)),
        None if rate == 0.0 => return Ok(()),
        // A rate never makes a row: the amount does.
        None => return Err(format!("Type the taxable amount in {item}C first.")),
    };
    match option {
        Some(option) if option.rate_is_editable => {
            form.schedule[row].tax_rate = rate;
            form.recompute();
            store(draft, &form)
        }
        Some(option) => Err(format!(
            "Item {item}D is the official rate of {}% for {}.",
            option.rate_text, option.code
        )),
        None => Err(format!(
            "Item {item}B is not an ATC the official form offers."
        )),
    }
}

/// `catalog:<index>` -> that entry of the official popup.
fn popup_entry(value: &Value) -> Result<Option<&'static Form2553AtcOption>, String> {
    let choice = text(value);
    if choice.is_empty() {
        return Ok(None);
    }
    choice
        .strip_prefix("catalog:")
        .and_then(|i| i.parse::<usize>().ok())
        .and_then(|i| FORM_2553_ATC_OPTIONS.get(i))
        .map(Some)
        .ok_or_else(|| "Choose an ATC from the official list.".to_string())
}

/// A row's ATC from the popup's list (any row may hold any entry, as the
/// official popup allows); blank clears the row.
fn fill_pt_atc(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let mut form: Form2553Draft = typed(draft)?;
    match popup_entry(value)? {
        None => form.clear_atc(row),
        Some(option) => {
            // Rows 14-17 take only their printed entry (a stored row holding
            // another can be cleared or set to it); row 18 takes any.
            if let Some(printed) = printed_option(row)
                && !same_entry_option(option, printed)
            {
                return Err(format!(
                    "Row {} prints {} ({}); use row 18 for {}.",
                    PT_FIRST_ITEM + row,
                    printed.code,
                    PT_SHORT_NAMES[row],
                    option.code
                ));
            }
            if let Some(why) = held_elsewhere(&form, row, option) {
                return Err(why);
            }
            form.set_atc(row, option)?;
        }
    }
    store(draft, &form)
}

fn same_entry_option(a: &Form2553AtcOption, b: &Form2553AtcOption) -> bool {
    a.code == b.code && a.description == b.description
}

fn fill_pt_item_13(draft: &mut Value, yes: bool, value: &Value) -> Result<(), String> {
    use bir_core::forms::form_2553::Form2553TaxTreaty;
    // A one-answer pair: the ticked box carries the answer.
    if !ticked(value) {
        return Ok(());
    }
    let mut form: Form2553Draft = typed(draft)?;
    form.tax_treaty = match (yes, form.tax_treaty) {
        (true, held) if held.is_yes() => held,
        // `enableSelTreaty`: Yes opens the list with nothing chosen.
        (true, _) => Form2553TaxTreaty::YesUnspecified,
        (false, _) => Form2553TaxTreaty::No,
    };
    form.recompute();
    store(draft, &form)
}

fn fill_pt_item_13_list(draft: &mut Value, value: &Value) -> Result<(), String> {
    use bir_core::forms::form_2553::Form2553TaxTreaty;
    let mut form: Form2553Draft = typed(draft)?;
    // Closed while item 13 is No: the list takes nothing.
    if !form.tax_treaty.is_yes() {
        return Ok(());
    }
    form.tax_treaty = match text(value).as_str() {
        "" | "0" => Form2553TaxTreaty::YesUnspecified,
        "1" => Form2553TaxTreaty::SpecialRate,
        "2" => Form2553TaxTreaty::InternationalTaxTreaty,
        other => return Err(format!("`{other}` is not an item 13 choice.")),
    };
    form.recompute();
    store(draft, &form)
}

/// A 2553 row's ATC list: the official popup's entries, by position, the
/// row's own entry marked.
pub(super) fn options(kind: AgentFormKind, key: &str, draft: &Value) -> Option<Vec<Value>> {
    let Cell::PtAtc(row) = cell(kind, key)? else {
        return None;
    };
    let form: Option<Form2553Draft> = typed(draft).ok();
    let held = form
        .as_ref()
        .and_then(|form| form.schedule.get(row).cloned())
        .filter(|row| !row.is_empty());
    Some(
        FORM_2553_ATC_OPTIONS
            .iter()
            .enumerate()
            .map(|(i, option)| {
                let rate = if option.rate_is_editable {
                    "rate you enter".to_string()
                } else {
                    format!("{}%", option.rate_text)
                };
                json!({
                    "value": format!("catalog:{i}"),
                    "label": format!("{} — {} ({rate})", option.code, option.description),
                    "selected": held.as_ref().is_some_and(|row| row.atc_code == option.code && row.description == option.description),
                })
            })
            .collect(),
    )
}

/// Rule 4 for a schedule's date boxes: 2000's Schedules 2-4 tables (2000
/// R4.0) carry `calendar: true`, so each of their date boxes gets its own
/// calendar on the page (form_page.js `scheduleTable`). Other forms' tables
/// are their own groups' and keep typing only.
pub(super) fn mark_calendar_tables(form: &mut Value, code: &str) {
    if code != "2000" {
        return;
    }
    let items = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten());
    for item in items {
        let dated = item["cols"]
            .as_array()
            .is_some_and(|cols| cols.iter().any(|col| col["t"] == "date"));
        if dated {
            item["calendar"] = Value::Bool(true);
        }
    }
}

/// What each box the form fills from another box of its row waits for while
/// it shows blank: 2000's Schedule 1 rates follow the ATC; 2553's preprinted
/// rows 14-17 show their printed classification, ATC and rate, and 18A the
/// ATC chosen in 18B.
pub(super) fn waits(kind: AgentFormKind) -> Map<String, Value> {
    let mut out = Map::new();
    match kind.form_code() {
        "2000" => {
            for row in 0..bir_core::forms::form_2000::FORM_2000_ROWS {
                out.insert(format!("frm2000:sched1:txtTaxRate{row}"), json!(RATE_WAITS));
            }
        }
        "2553" => {
            // A blank row 14-17 shows the sheet's printed default.
            for (row, (_, caption, atc, rate)) in PT_PRINTED_ROWS.iter().enumerate() {
                let item = PT_FIRST_ITEM + row;
                let rate = if rate.is_empty() {
                    "rate you enter"
                } else {
                    rate
                };
                out.insert(
                    format!("frm2553:txt{item}A"),
                    json!(format!("Printed: {caption}, {atc}, {rate}")),
                );
                out.insert(
                    format!("frm2553:txt{item}B"),
                    json!(format!("Printed: {atc}")),
                );
                if rate != "rate you enter" {
                    out.insert(
                        format!("frm2553:txt{item}D"),
                        json!(format!("Printed: {rate}")),
                    );
                }
            }
            out.insert("frm2553:txt18A".into(), json!(ATC_18_WAITS));
        }
        _ => {}
    }
    out
}

fn amount_of(row: &Value, field: &str) -> f64 {
    row[field].as_f64().unwrap_or(0.0)
}

fn rows<'a>(draft: &'a Value, list: &str) -> Vec<&'a Value> {
    draft[list]
        .as_array()
        .map(|rows| rows.iter().collect())
        .unwrap_or_default()
}

/// Rows of a 2000 payment or remittance schedule the filer typed into.
fn typed_rows(draft: &Value, list: &str) -> usize {
    rows(draft, list)
        .into_iter()
        .filter(|row| {
            row.as_object().is_some_and(|fields| {
                fields.values().any(|value| match value {
                    Value::String(text) => !text.trim().is_empty(),
                    Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
                    _ => false,
                })
            })
        })
        .count()
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what} row{}", if n == 1 { "" } else { "s" })
}

/// `1234.5` -> `1,234.50`.
fn pesos(amount: f64) -> String {
    let cents = (amount * 100.0).round() as i64;
    let negative = cents < 0;
    let (whole, cents) = ((cents / 100).abs(), (cents % 100).abs());
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}{grouped}.{cents:02}", if negative { "-" } else { "" })
}

/// When `edits` turned `before` into `after` by removing what the filer
/// typed, what the page asks first (`message`), what the Undo toast says
/// (`done`) and how many rows went; `None` when nothing typed went. The
/// host applies the edit either way; the page keeps its old draft until the
/// filer chooses Continue.
pub(super) fn removes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    let code = kind.form_code();
    let edited = |prefix: &str| edits.keys().any(|key| key.starts_with(prefix));
    if code == "2000" && edited("frm2000:optMode_") {
        let lost: Vec<(usize, &str)> = [
            ("schedule2", "Schedule 2"),
            ("schedule3", "Schedule 3"),
            ("schedule4", "Schedule 4"),
        ]
        .into_iter()
        .map(|(list, name)| {
            (
                typed_rows(before, list).saturating_sub(typed_rows(after, list)),
                name,
            )
        })
        .filter(|(n, _)| *n > 0)
        .collect();
        let total: usize = lost.iter().map(|(n, _)| n).sum();
        if total > 0 {
            let parts: Vec<String> = lost.iter().map(|(n, what)| count(*n, what)).collect();
            return Some(json!({
                "message": format!("Changing item 13 removes the {} you typed. Continue?", parts.join(" and ")),
                "done": format!("Removed {total} row{}", if total == 1 { "" } else { "s" }),
                "rows": total,
            }));
        }
    }
    if code == "2000" && edited("frm2000:optParty_") {
        let typed = |draft: &Value| {
            ["other_party_name", "other_party_tin"]
                .iter()
                .any(|field| draft[*field].as_str().is_some_and(|s| !s.trim().is_empty()))
        };
        if typed(before) && !typed(after) {
            return Some(json!({
                "message": "Changing item 10 clears items 11 and 12 (the other party's name and TIN). Continue?",
                "done": "Cleared items 11 and 12",
                "rows": 0,
            }));
        }
    }
    // 2553 item 13 No clears the relief named under "If yes, specify".
    if code == "2553" && edited("frm2553:optTreaty_") {
        let relief = |draft: &Value| match draft["tax_treaty"].as_str() {
            Some("special_rate") => Some("Special Rate"),
            Some("international_tax_treaty") => Some("International Tax Treaty"),
            _ => None,
        };
        if let Some(chosen) = relief(before)
            && after["tax_treaty"] == "no"
        {
            return Some(json!({
                "message": format!("Changing item 13 to No clears the relief chosen under If yes, specify ({chosen}). Continue?"),
                "done": "Cleared item 13's relief",
                "rows": 0,
            }));
        }
    }
    // Amended Return Yes -> No clears the previous return's payment.
    let amended = match code {
        "2000" => Some(("frm2000:AmendedRtn_", "balance_carried_over", "15A")),
        "2553" => Some(("frm2553:optAmended_", "tax_paid_previous", "20A")),
        "2550M" => Some(("frm2550m:OptAmendedYN", "vat_paid_previous", "23D")),
        _ => None,
    };
    if let Some((prefix, field, item)) = amended
        && edited(prefix)
    {
        let previous = amount_of(before, field);
        if previous != 0.0 && amount_of(after, field) == 0.0 {
            return Some(json!({
                "message": format!(
                    "Changing Amended Return to No clears item {item} (₱{}). Continue?",
                    pesos(previous)
                ),
                "done": format!("Cleared item {item}"),
                "rows": 0,
            }));
        }
    }
    // Rows the edit removed, each with what it held (counted per row, so
    // one batch over two rows states both amounts).
    let atc = |row: &Value| row["atc_code"].as_str().unwrap_or("").trim().to_string();
    match code {
        "2000" | "2550M" => {
            let (list, field, stem) = if code == "2000" {
                ("schedule1", "tax_base", "drpATCCode")
            } else {
                ("sales_schedule", "amount", "frm2550m:txtAtcCde")
            };
            if !edits
                .iter()
                .any(|(key, value)| key.starts_with(stem) && text(value).is_empty())
            {
                return None;
            }
            let after_rows = rows(after, list);
            let lost: Vec<f64> = rows(before, list)
                .into_iter()
                .enumerate()
                .filter(|(i, row)| {
                    let code_before = atc(row);
                    !code_before.is_empty()
                        && if code == "2000" {
                            // 2000 rows keep their place.
                            after_rows.get(*i).is_none_or(|r| atc(r).is_empty())
                        } else {
                            // 2550M rows re-sort by the popup's order.
                            !after_rows.iter().any(|r| atc(r) == code_before)
                        }
                })
                .map(|(_, row)| amount_of(row, field))
                .filter(|amount| *amount > 0.0)
                .collect();
            match lost.as_slice() {
                [] => None,
                [one] => Some(json!({
                    "message": format!(
                        "Clearing this ATC removes its row and its amount of ₱{}. Continue?",
                        pesos(*one)
                    ),
                    "done": "Removed the row",
                    "rows": 1,
                })),
                many => Some(json!({
                    "message": format!(
                        "Clearing these ATCs removes their rows and amounts of {}. Continue?",
                        many.iter().map(|a| format!("₱{}", pesos(*a))).collect::<Vec<_>>().join(" and ")
                    ),
                    "done": format!("Removed {} rows", many.len()),
                    "rows": many.len(),
                })),
            }
        }
        // 2553: a row cleared through its ATC, amount or rate.
        "2553" => {
            let after_rows = rows(after, "schedule");
            let lost: Vec<(usize, String, f64)> = rows(before, "schedule")
                .into_iter()
                .enumerate()
                .filter(|(i, row)| {
                    !atc(row).is_empty() && after_rows.get(*i).is_none_or(|r| atc(r).is_empty())
                })
                .map(|(i, row)| {
                    (
                        PT_FIRST_ITEM + i,
                        atc(row),
                        amount_of(row, "taxable_amount"),
                    )
                })
                .filter(|(_, _, amount)| *amount > 0.0)
                .collect();
            let entry =
                |(_, code, amount): &(usize, String, f64)| format!("{code}, ₱{}", pesos(*amount));
            match lost.as_slice() {
                [] => None,
                [one] => Some(json!({
                    "message": format!("Clearing row {} removes its entry ({}). Continue?", one.0, entry(one)),
                    "done": format!("Removed row {}", one.0),
                    "rows": 1,
                })),
                many => Some(json!({
                    "message": format!(
                        "Clearing rows {} removes their entries ({}). Continue?",
                        many.iter().map(|r| r.0.to_string()).collect::<Vec<_>>().join(" and "),
                        many.iter().map(entry).collect::<Vec<_>>().join("; ")
                    ),
                    "done": format!("Removed {} rows", many.len()),
                    "rows": many.len(),
                })),
            }
        }
        _ => None,
    }
}
