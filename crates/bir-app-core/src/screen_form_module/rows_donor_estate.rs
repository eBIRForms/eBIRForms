//! PR 10: the donor's and estate tax returns (1800, 1801). Boxes the host
//! writes through the typed draft because the probed bindings have none or
//! skip the form's own setter, the boxes the official page keeps shut until
//! another answer opens them, and the answers that remove what the filer
//! typed.
//!
//! - 1800 item 13 and 1801 item 14 (tax relief): one draft field,
//!   `tax_relief` (`Some` holds 13A / 14A). The probe calls the Yes and No
//!   boxes constants (1800) or derived (1801), so the host writes them: Yes
//!   keeps what 13A / 14A holds, No clears it (asked first: [`removes`]).
//!   13A / 14A take text only while the answer is Yes (rules/forms
//!   fields.json `enabled_when` "Item 13 Yes." / "Item 14 Yes."); a write
//!   while it is No is refused with its reason, and rule 6 fades the box
//!   ([`closed`]).
//! - 1800 items 28-32 (deductions): the draft's `deductions` rows, which the
//!   probe could not bind (its samples have none). Item 33 stays 0.00 as on
//!   the official page (form_1800.rs `recompute`).
//! - 1801 item 1 (date of death): three whole-number boxes the page shows as
//!   one date ([`mark_schema`] `one_date`); a blank clears a part to the
//!   draft's 0 ("not chosen"), which the probed whole binding refuses.
//! - 1801 item 11 (the executor's TIN): `administrator_tin`, nine digits
//!   shown as three boxes; the branch code is its own bound box. The boxes
//!   fill in order (rows_1700.rs `with_tin_part`); a whole TIN pasted into
//!   the first box fills the branch code too. The page draws it as a TIN the
//!   filer types, not the profile's ([`mark_schema`] `typed_tin`).
//! - 1801 item 15D (installment) goes through `set_installment_granted`,
//!   which clears the frequency as the official `installmentGranted` does
//!   (asked first). The frequency takes a choice only while 15D is Yes, and
//!   its Others (specify) box only while the frequency is Others: the
//!   tracked function inventory (rules/forms/1801-v2018/fixtures/
//!   calculation-function-inventory-v796.json `installmentGranted`,
//!   validation-function-inventory-v796.json `freqOtherChecked`) ties both
//!   to `frm1801v2018:instGranted_1`, 15D's Yes (fields.json says "Item 15B"
//!   for the same box; the sheet prints the frequency under 15D).
//!
//! Field keys, the official XML and the print map are the forms' own and
//! unchanged.

use bir_core::forms::agent::{self, AgentFormKind};
use bir_core::forms::form_1800::{FORM_1800_DEDUCTIONS, Form1800Deduction, Form1800Draft};
use bir_core::forms::form_1801::{Form1801Draft, Form1801Frequency};
use bir_core::forms::official_inputs::{cents, within_round_limit};
use serde_json::{Map, Value, json};

use super::rows_1700::with_tin_part;

/// Which part of 1801's item 1 a box is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DatePart {
    Month,
    Day,
    Year,
}

/// A box this module writes or gates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Cell {
    /// 1800 item 13 Yes (`true`) or No.
    Relief1800(bool),
    /// 1800 item 13A.
    ReliefText1800,
    /// 1800 items 28-32: row 0-4, its description (`false`) or amount.
    Deduction1800(usize, bool),
    /// 1801 item 1.
    DeathDate1801(DatePart),
    /// 1801 item 11: TIN box 0-2.
    AdminTin1801(usize),
    /// 1801 item 14 Yes (`true`) or No.
    Relief1801(bool),
    /// 1801 item 14A.
    ReliefText1801,
    /// 1801 item 15D: Yes, No, or the group box (`None`, value 1 or 2).
    Installment1801(Option<bool>),
    /// 1801 item 15D frequency: one choice, or the group box (`None`).
    Frequency1801(Option<Form1801Frequency>),
    /// 1801 item 15D Others (specify).
    FrequencyOther1801,
}

const FREQUENCIES: [(&str, Form1801Frequency); 4] = [
    ("Mon", Form1801Frequency::Monthly),
    ("Qtr", Form1801Frequency::Quarterly),
    ("Semi", Form1801Frequency::SemiAnnual),
    ("Other", Form1801Frequency::Others),
];

pub(super) fn cell(kind: AgentFormKind, key: &str) -> Option<Cell> {
    match kind.form_code() {
        "1800" => cell_1800(key.strip_prefix("frm1800v2018:")?),
        "1801" => cell_1801(key.strip_prefix("frm1801v2018:")?),
        _ => None,
    }
}

fn cell_1800(rest: &str) -> Option<Cell> {
    match rest {
        "TaxTreatyYN_Y" => return Some(Cell::Relief1800(true)),
        "TaxTreatyYN_N" => return Some(Cell::Relief1800(false)),
        "TaxTreaty" => return Some(Cell::ReliefText1800),
        _ => {}
    }
    let (item, amount) = if let Some(n) = rest.strip_prefix("txtDeductionTitle") {
        (n, false)
    } else {
        (rest.strip_prefix("txtDeductionAmount")?, true)
    };
    let item: usize = item.parse().ok()?;
    let row = item.checked_sub(28)?;
    (row < FORM_1800_DEDUCTIONS).then_some(Cell::Deduction1800(row, amount))
}

fn cell_1801(rest: &str) -> Option<Cell> {
    Some(match rest {
        "txtDateMonth" => Cell::DeathDate1801(DatePart::Month),
        "txtDateDay" => Cell::DeathDate1801(DatePart::Day),
        "txtDateYear" => Cell::DeathDate1801(DatePart::Year),
        "txtTINE1" => Cell::AdminTin1801(0),
        "txtTINE2" => Cell::AdminTin1801(1),
        "txtTINE3" => Cell::AdminTin1801(2),
        "optTreaty_1" => Cell::Relief1801(true),
        "optTreaty_2" => Cell::Relief1801(false),
        "treatyY" => Cell::ReliefText1801,
        "instGranted" => Cell::Installment1801(None),
        "instGranted_1" => Cell::Installment1801(Some(true)),
        "instGranted_2" => Cell::Installment1801(Some(false)),
        "freq" => Cell::Frequency1801(None),
        "freqOtherInput" => Cell::FrequencyOther1801,
        _ => {
            let choice = rest.strip_prefix("freq_")?;
            let (_, frequency) = FREQUENCIES.iter().find(|(name, _)| *name == choice)?;
            Cell::Frequency1801(Some(*frequency))
        }
    })
}

/// Whether the host writes `key` itself (its "cannot be filled here" lock
/// does not apply on the page).
pub(super) fn host_writes(kind: AgentFormKind, key: &str) -> bool {
    cell(kind, key).is_some()
}

/// One of 1801 item 15D's four frequency ticks (not its group box).
pub(super) fn is_frequency_tick(kind: AgentFormKind, key: &str) -> bool {
    matches!(cell(kind, key), Some(Cell::Frequency1801(Some(_))))
}

/// An answer before the box it opens, and a TIN's boxes left to right, in
/// one batch.
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    match cell(kind, key)? {
        Cell::Relief1800(_) | Cell::Relief1801(_) | Cell::Installment1801(_) => Some(0),
        Cell::Frequency1801(_) => Some(1),
        Cell::ReliefText1800 | Cell::ReliefText1801 | Cell::FrequencyOther1801 => Some(2),
        Cell::AdminTin1801(part) => Some(1 + part),
        Cell::Deduction1800(..) | Cell::DeathDate1801(_) => None,
    }
}

/// What item 13A says while item 13 is not Yes.
pub(super) const ANSWER_13_FIRST: &str = "Answer Yes to item 13 first.";
/// What item 14A says while item 14 is not Yes.
pub(super) const ANSWER_14_FIRST: &str = "Answer Yes to item 14 first.";
/// What item 15D's frequency says while 15D is not Yes.
pub(super) const ANSWER_15D_FIRST: &str = "Answer Yes to item 15D first.";
/// What 15D's Others (specify) says while the frequency is not Others.
pub(super) const CHOOSE_OTHERS_FIRST: &str =
    "Answer Yes to item 15D and choose Others as the frequency first.";

/// The tag rule 6 shows on each box while it is shut (the page's "Needs"
/// line), keyed like [`closed`].
const TAG_13: &str = "Needs item 13 = Yes";
const TAG_14: &str = "Needs item 14 = Yes";
const TAG_15D: &str = "Needs item 15D = Yes";
const TAG_15D_OTHERS: &str = "Needs item 15D = Yes and frequency Others";

/// The boxes this module shuts while their answer does not hold, each with
/// its tag, whatever they show (a legacy draft's value there is faded, not
/// hidden). Rule 6 (conditions.rs `off_boxes`) shows them off.
pub(super) fn closed(kind: AgentFormKind, draft: &Value) -> Vec<(String, &'static str)> {
    gated_keys(kind)
        .into_iter()
        .filter_map(|key| {
            let tag = shut(kind, draft, &key)?;
            Some((key, tag))
        })
        .collect()
}

/// Every box [`closed`] may report: the conditions' own trial leaves them
/// to it.
pub(super) fn gated_keys(kind: AgentFormKind) -> Vec<String> {
    match kind.form_code() {
        "1800" => vec!["frm1800v2018:TaxTreaty".into()],
        "1801" => [
            "treatyY",
            "freq_Mon",
            "freq_Qtr",
            "freq_Semi",
            "freq_Other",
            "freqOtherInput",
        ]
        .iter()
        .map(|rest| format!("frm1801v2018:{rest}"))
        .collect(),
        _ => Vec::new(),
    }
}

/// `Some(tag)` while `key`'s answer does not hold on `draft`.
fn shut(kind: AgentFormKind, draft: &Value, key: &str) -> Option<&'static str> {
    let relief = !draft["tax_relief"].is_null();
    let installment = draft["installment_granted"] == json!(true);
    let others = draft["installment_frequency"] == json!("others");
    match cell(kind, key)? {
        Cell::ReliefText1800 => (!relief).then_some(TAG_13),
        Cell::ReliefText1801 => (!relief).then_some(TAG_14),
        Cell::Frequency1801(Some(_)) => (!installment).then_some(TAG_15D),
        Cell::FrequencyOther1801 => (!(installment && others)).then_some(TAG_15D_OTHERS),
        _ => None,
    }
}

fn ticked(value: &Value) -> bool {
    matches!(value, Value::Bool(true)) || value.as_str().is_some_and(|s| s.trim() == "true")
}

fn text(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
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

/// The answer a Yes / No box gives: ticking it gives its own answer, and
/// unticking it the other one (the probed radio bindings' own reading:
/// `AmendedRtn_1` false sets No).
fn answer(yes_box: bool, value: &Value) -> bool {
    if ticked(value) { yes_box } else { !yes_box }
}

/// Write `key` when this module owns it: `None` leaves it to the ordinary
/// fill. `ordinary` is that fill, for a box this module only gates or
/// checks.
pub(super) fn fill(
    kind: AgentFormKind,
    draft: &mut Value,
    key: &str,
    value: &Value,
    ordinary: impl FnOnce(&mut Value, &Value) -> Result<(), String>,
) -> Option<Result<(), String>> {
    let cell = cell(kind, key)?;
    Some(match cell {
        Cell::Relief1800(yes) => fill_1800_relief(draft, answer(yes, value)),
        Cell::ReliefText1800 => fill_1800_relief_text(draft, value, ordinary),
        Cell::Deduction1800(row, amount) => fill_1800_deduction(draft, row, amount, value),
        Cell::DeathDate1801(part) => fill_1801_date(part, draft, value, ordinary),
        Cell::AdminTin1801(part) => fill_1801_admin_tin(draft, part, value),
        Cell::Relief1801(yes) => fill_1801_relief(draft, answer(yes, value)),
        Cell::ReliefText1801 => fill_1801_relief_text(draft, value),
        Cell::Installment1801(which) => {
            let granted = match which {
                Some(yes) => Ok(answer(yes, value)),
                None => match text(value).as_str() {
                    "1" => Ok(true),
                    "2" => Ok(false),
                    other => Err(format!("Item 15D is 1 (Yes) or 2 (No), not `{other}`.")),
                },
            };
            granted.and_then(|granted| fill_1801_installment(draft, granted))
        }
        Cell::Frequency1801(which) => fill_1801_frequency(draft, which, value),
        Cell::FrequencyOther1801 => fill_1801_frequency_other(draft, value),
    })
}

#[inline(never)]
fn fill_1800_relief(draft: &mut Value, yes: bool) -> Result<(), String> {
    let mut form: Form1800Draft = typed(draft)?;
    form.tax_relief = if yes {
        Some(form.tax_relief.take().unwrap_or_default())
    } else {
        None
    };
    form.recompute();
    store(draft, &form)
}

/// 13A: the ordinary text fill while item 13 is Yes (a blank keeps Yes:
/// the binding writes `Some("")`); refused while it is No.
#[inline(never)]
fn fill_1800_relief_text(
    draft: &mut Value,
    value: &Value,
    ordinary: impl FnOnce(&mut Value, &Value) -> Result<(), String>,
) -> Result<(), String> {
    if draft["tax_relief"].is_null() {
        // Blank is what the shut box shows: nothing to write or refuse.
        return if text(value).is_empty() {
            Ok(())
        } else {
            Err(ANSWER_13_FIRST.to_string())
        };
    }
    ordinary(draft, value)
}

/// The amount a deduction box takes: blank is zero; never negative and
/// within the official round limit (below 1,000,000,000,000).
fn deduction_amount(value: &Value) -> Result<f64, String> {
    let shown = text(value);
    if shown.is_empty() {
        return Ok(0.0);
    }
    let refused =
        || format!("`{shown}` is not an amount: enter pesos and centavos below 1,000,000,000,000.");
    // The typed text decides the centavos: pesos (digits, thousands
    // commas, a peso sign) and at most two digits after one point. More is
    // refused, never rounded away; a float check (`has_cent_precision`)
    // would refuse valid amounts of about 2,000,000 and up.
    let plain: String = shown
        .chars()
        .filter(|c| !matches!(c, ',' | '\u{20b1}' | ' '))
        .collect();
    let (pesos, centavos) = plain.split_once('.').unwrap_or((&plain, ""));
    let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    if !digits(pesos)
        || !digits(centavos)
        || centavos.len() > 2
        || pesos.len() + centavos.len() == 0
    {
        return Err(refused());
    }
    let amount = agent::parse_amount(&plain).ok_or_else(refused)?;
    // The bound applies to what is stored: the amount at centavos.
    let stored = cents(amount);
    if !amount.is_finite() || !within_round_limit(amount) || !within_round_limit(stored) {
        return Err(refused());
    }
    Ok(stored)
}

#[inline(never)]
fn fill_1800_deduction(
    draft: &mut Value,
    row: usize,
    amount: bool,
    value: &Value,
) -> Result<(), String> {
    let mut form: Form1800Draft = typed(draft)?;
    let (title, figure) = if amount {
        (None, Some(deduction_amount(value)?))
    } else {
        (Some(text(value)), None)
    };
    let filling = title.as_ref().is_some_and(|t| !t.is_empty()) || figure.is_some_and(|a| a != 0.0);
    if form.deductions.len() <= row {
        if !filling {
            return Ok(());
        }
        form.deductions
            .resize(row + 1, Form1800Deduction::default());
    }
    if let Some(title) = title {
        form.deductions[row].title = title;
    }
    if let Some(figure) = figure {
        form.deductions[row].amount = figure;
    }
    // A cleared last row goes, as the page shows it: blank.
    while form
        .deductions
        .last()
        .is_some_and(|last| last.title.is_empty() && last.amount == 0.0)
    {
        form.deductions.pop();
    }
    form.recompute();
    store(draft, &form)
}

/// One part of item 1: blank clears it (the draft's 0, "not chosen"); a
/// month 01-12, a day 01-31 or a four-digit year goes through the ordinary
/// fill, which keeps the box's own locks.
#[inline(never)]
fn fill_1801_date(
    part: DatePart,
    draft: &mut Value,
    value: &Value,
    ordinary: impl FnOnce(&mut Value, &Value) -> Result<(), String>,
) -> Result<(), String> {
    let shown = text(value);
    if shown.is_empty() {
        return ordinary(draft, &json!("0"));
    }
    let digits = shown.chars().all(|c| c.is_ascii_digit());
    let number: Option<u32> = digits.then(|| shown.parse().ok()).flatten();
    let fits = match part {
        DatePart::Month => number.is_some_and(|n| (1..=12).contains(&n)) && shown.len() <= 2,
        DatePart::Day => number.is_some_and(|n| (1..=31).contains(&n)) && shown.len() <= 2,
        DatePart::Year => number.is_some_and(|n| n >= 1000) && shown.len() == 4,
    };
    if !fits {
        return Err(match part {
            DatePart::Month => format!("Item 1's month is 01 to 12, not `{shown}`."),
            DatePart::Day => format!("Item 1's day is 01 to 31, not `{shown}`."),
            DatePart::Year => format!("Item 1's year has four digits (YYYY), not `{shown}`."),
        });
    }
    ordinary(draft, &json!(shown))
}

#[inline(never)]
fn fill_1801_admin_tin(draft: &mut Value, part: usize, value: &Value) -> Result<(), String> {
    let mut form: Form1801Draft = typed(draft)?;
    let digits = with_tin_part(&form.administrator_tin, part, &text(value))?;
    if digits.len() > 9 {
        form.administrator_branch_code = digits[9..].to_string();
    }
    form.administrator_tin = digits.chars().take(9).collect();
    form.recompute();
    store(draft, &form)
}

#[inline(never)]
fn fill_1801_relief(draft: &mut Value, yes: bool) -> Result<(), String> {
    let mut form: Form1801Draft = typed(draft)?;
    form.tax_relief = if yes {
        Some(form.tax_relief.take().unwrap_or_default())
    } else {
        None
    };
    form.recompute();
    store(draft, &form)
}

/// 14A: while item 14 is Yes the text is 14A's (a blank keeps Yes, where
/// the probed nullable binding would answer No); refused while it is No.
#[inline(never)]
fn fill_1801_relief_text(draft: &mut Value, value: &Value) -> Result<(), String> {
    let mut form: Form1801Draft = typed(draft)?;
    let shown = text(value);
    match form.tax_relief.as_mut() {
        Some(relief) => *relief = shown,
        None if shown.is_empty() => return Ok(()),
        None => return Err(ANSWER_14_FIRST.to_string()),
    }
    form.recompute();
    store(draft, &form)
}

#[inline(never)]
fn fill_1801_installment(draft: &mut Value, granted: bool) -> Result<(), String> {
    let mut form: Form1801Draft = typed(draft)?;
    form.set_installment_granted(granted);
    store(draft, &form)
}

/// 15D's frequency set to `frequency`. A frequency other than Others
/// clears the Others (specify) text: the official serializer never files it
/// then (this port's `to_bir_field_map` writes `freqOtherInput` only for
/// Others, and `set_installment_granted`, the port of the official
/// `installmentGranted`, clears it with the frequency), and the tracked
/// function inventory's `freqOtherChecked` (rules/forms/1801-v2018/
/// fixtures/validation-function-inventory-v796.json, source lines
/// 4942-4945) reads the box with `freq_Other` and `instGranted_1` (15D's
/// Yes): the box belongs to Others under 15D Yes only. Kept, a hidden text
/// would come back unseen with Others. The page asks first ([`removes`]).
fn set_frequency(form: &mut Form1801Draft, frequency: Option<Form1801Frequency>) {
    form.installment_frequency = frequency;
    if frequency != Some(Form1801Frequency::Others) {
        form.installment_frequency_other.clear();
    }
    form.recompute();
}

/// 15D's frequency: ticking a choice (or the group box's value) picks it
/// while 15D is Yes; unticking the chosen one clears the group (another
/// box's untick says nothing: the ticked box carries the answer).
#[inline(never)]
fn fill_1801_frequency(
    draft: &mut Value,
    which: Option<Form1801Frequency>,
    value: &Value,
) -> Result<(), String> {
    let mut form: Form1801Draft = typed(draft)?;
    let picked = match which {
        Some(frequency) if ticked(value) => Some(frequency),
        Some(_) => {
            // The page sends the untick of the box it leaves; the answer
            // is the ticked one's. A lone untick clears the group.
            if form.installment_frequency == which {
                set_frequency(&mut form, None);
                return store(draft, &form);
            }
            return Ok(());
        }
        None => {
            let choice = text(value);
            if choice.is_empty() {
                set_frequency(&mut form, None);
                return store(draft, &form);
            }
            Some(
                FREQUENCIES
                    .iter()
                    .find(|(name, _)| *name == choice)
                    .map(|(_, frequency)| *frequency)
                    .ok_or_else(|| {
                        format!("Item 15D's frequency is Mon, Qtr, Semi or Other, not `{choice}`.")
                    })?,
            )
        }
    };
    if !form.installment_granted {
        return Err(ANSWER_15D_FIRST.to_string());
    }
    set_frequency(&mut form, picked);
    store(draft, &form)
}

/// 15D Others (specify): taken while 15D is Yes and the frequency Others.
/// While shut the box shows blank (the field map writes it only for
/// Others), so a blank is no write; text is refused with its reason.
#[inline(never)]
fn fill_1801_frequency_other(draft: &mut Value, value: &Value) -> Result<(), String> {
    let mut form: Form1801Draft = typed(draft)?;
    let shown = text(value);
    let open =
        form.installment_granted && form.installment_frequency == Some(Form1801Frequency::Others);
    if !open {
        return if shown.is_empty() {
            Ok(())
        } else {
            Err(CHOOSE_OTHERS_FIRST.to_string())
        };
    }
    form.installment_frequency_other = shown;
    form.recompute();
    store(draft, &form)
}

/// Draft fields whose errors point at boxes without a binding of their own.
pub(super) fn error_keys(kind: AgentFormKind, field: &str) -> Vec<String> {
    match (kind.form_code(), field) {
        ("1800", "deductions") => (28..28 + FORM_1800_DEDUCTIONS)
            .flat_map(|item| {
                [
                    format!("frm1800v2018:txtDeductionTitle{item}"),
                    format!("frm1800v2018:txtDeductionAmount{item}"),
                ]
            })
            .collect(),
        ("1801", "administrator_tin") => (1..=3)
            .map(|n| format!("frm1801v2018:txtTINE{n}"))
            .collect(),
        ("1801", "installment_frequency_other") => vec!["frm1801v2018:freqOtherInput".into()],
        _ => Vec::new(),
    }
}

/// Marks on the on-screen schema: 1801 item 1 is one date box
/// (`one_date`, form_page.js `oneDate`, the draft and upload keep the three
/// boxes), and item 11 is a TIN the filer types for the executor
/// (`typed_tin`: form_page.js draws it without the profile mark). 1800's
/// extracted Part III table, whose boxes the official program keeps
/// disabled (the layout shows items 21-24 as notes), stays in the schema as
/// hidden boxes, so the form loses no control and the page draws none.
pub(super) fn mark_schema(form: &mut Value, code: &str) {
    if code == "1800" {
        hide_part_iii_1800(form);
        return;
    }
    if code != "1801" {
        return;
    }
    let items = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten());
    for item in items {
        let first = item["f"][0]["k"].as_str().unwrap_or_default().to_string();
        match first.as_str() {
            "frm1801v2018:txtDateMonth" => item["one_date"] = Value::Bool(true),
            "frm1801v2018:txtTINE1" => item["typed_tin"] = Value::Bool(true),
            _ => {}
        }
    }
}

/// 1800's Part III table (rows 21-23, `txtAgency20` onward) as one item of
/// hidden boxes.
fn hide_part_iii_1800(form: &mut Value) {
    let items = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten());
    for item in items {
        if item["keys"][0][0] != "frm1800v2018:txtAgency20" {
            continue;
        }
        let cols = item["cols"].as_array().cloned().unwrap_or_default();
        let fields: Vec<Value> = item["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|row| row.as_array().cloned().unwrap_or_default())
            .enumerate()
            .map(|(i, key)| {
                let label = cols
                    .get(i % cols.len().max(1))
                    .map_or(Value::Null, |col| col["l"].clone());
                json!({ "k": key, "t": "hidden", "l": label })
            })
            .collect();
        *item = json!({
            "no": "",
            "l": "Details of Payment (disabled in the official program)",
            "p": item["p"].clone(),
            "f": fields,
        });
    }
}

/// What the page shows where the field map's value is not what the draft
/// keeps (display only; the field map, upload and print are unchanged):
/// 1801 item 1's parts show blank while the draft holds 0 ("not chosen",
/// which the upload writes as `00`), and 1800 item 11 shows the email as
/// typed (the upload keeps it in capitals, form_1800.rs; the print shows it
/// as typed, `to_print_field_map`).
pub(super) fn values(
    kind: AgentFormKind,
    draft: &Value,
    map: &mut std::collections::BTreeMap<String, String>,
) {
    match kind.form_code() {
        "1800" => {
            if let Some(email) = draft["email"].as_str()
                && map.contains_key("frm1800v2018:txtEmail")
            {
                map.insert("frm1800v2018:txtEmail".into(), email.trim().to_string());
            }
            for (key, field) in [
                ("frm1800v2018:txtMonth", "donation_month"),
                ("frm1800v2018:txtDate", "donation_day"),
                ("frm1800v2018:txtYear", "donation_year"),
            ] {
                if draft[field].as_u64() == Some(0) && map.contains_key(key) {
                    map.insert(key.into(), String::new());
                }
            }
        }
        "1801" => {
            for (key, field) in [
                ("frm1801v2018:txtDateMonth", "death_month"),
                ("frm1801v2018:txtDateDay", "death_day"),
                ("frm1801v2018:txtDateYear", "death_year"),
            ] {
                if draft[field].as_u64() == Some(0) && map.contains_key(key) {
                    map.insert(key.into(), String::new());
                }
            }
        }
        _ => {}
    }
}

/// The draft fields of each form's item 1 date, and its error's field.
fn item_1_date(kind: AgentFormKind) -> Option<([&'static str; 3], &'static str)> {
    match kind.form_code() {
        "1800" => Some((
            ["donation_month", "donation_day", "donation_year"],
            "donation_date",
        )),
        "1801" => Some((["death_month", "death_day", "death_year"], "death_date")),
        _ => None,
    }
}

/// What the page keeps quiet on a return whose item 1 date nobody has
/// typed: the date's error field and its boxes (form_page.js `quiet`: the
/// message waits until the filer edits a box of it, or presses Check or
/// Save). `null` otherwise.
pub(super) fn quiet(kind: AgentFormKind, draft: &Value) -> Value {
    let Some((parts, field)) = item_1_date(kind) else {
        return Value::Null;
    };
    if !parts.iter().all(|part| draft[*part].as_u64() == Some(0)) {
        return Value::Null;
    }
    let keys: &[&str] = if kind.form_code() == "1800" {
        &[
            "frm1800v2018:txtMonth",
            "frm1800v2018:txtDate",
            "frm1800v2018:txtYear",
        ]
    } else {
        &[
            "frm1801v2018:txtDateMonth",
            "frm1801v2018:txtDateDay",
            "frm1801v2018:txtDateYear",
        ]
    };
    json!({ "field": field, "keys": keys })
}

/// On opening a return whose item 1 date nobody has typed (all three parts
/// 0, as a new return starts), the page shows no message for it yet; Check,
/// which validates, and save still report it (display only: the form's
/// validation is unchanged).
pub(super) fn hold_untouched_date(kind: AgentFormKind, draft: &Value, errors: &mut Value) {
    let Some((parts, field)) = item_1_date(kind) else {
        return;
    };
    if !parts.iter().all(|part| draft[*part].as_u64() == Some(0)) {
        return;
    }
    if let Value::Array(list) = errors {
        list.retain(|error| error["field"] != field);
    }
}

/// Validation messages that name the official program's item numbers,
/// rewritten to the January 2018 sheet's printed ones (maintainer decision
/// D6: the wording only; the checks and the filed data stay the same).
/// `(form, official message, printed message)`. 1801's messages already
/// cite the printed numbers.
pub(super) const PRINTED_MESSAGES: &[(&str, &str, &str)] = &[
    // 1800: the official program's own numbering is off by the sheet's
    // items 4 (ATC) and 8A/9A; the sheet prints 13A, 5, 6 and 7.
    (
        "1800",
        "Please specify the Special Treaty or International Law the payee is availing in item 11A.",
        "Please specify the Special Treaty or International Law the payee is availing in item 13A.",
    ),
    (
        "1800",
        "Please enter a valid TIN number on Item 4.",
        "Please enter a valid TIN number on Item 5.",
    ),
    (
        "1800",
        "Please enter a valid RDO Code on Item 5.",
        "Please enter a valid RDO Code on Item 6.",
    ),
    (
        "1800",
        "Please enter a valid Taxpayer's Name on Item 8.",
        "Please enter a valid Taxpayer's Name on Item 7.",
    ),
];

/// The page's wording of `message`: the printed item numbers where the
/// official program names its own (D6), else `None`.
pub(super) fn printed_message(kind: AgentFormKind, message: &str) -> Option<&'static str> {
    let code = kind.form_code();
    PRINTED_MESSAGES
        .iter()
        .find(|(form, official, _)| *form == code && *official == message)
        .map(|(_, _, printed)| *printed)
}

/// `1234.5` -> `1,234.50`.
fn pesos(amount: f64) -> String {
    let cents = (amount * 100.0).round() as i64;
    let (whole, cents) = ((cents / 100).abs(), (cents % 100).abs());
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!(
        "{}{grouped}.{cents:02}",
        if amount < 0.0 { "-" } else { "" }
    )
}

/// The `(sentence, done)` of a batch that blanks a whole penalty group whose
/// typed amounts (non-zero overrides) the computed ones then replace; `None`
/// when it replaces nothing typed.
fn replaced_penalties(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &serde_json::Map<String, Value>,
) -> Option<(String, String)> {
    let mut replaced: Vec<String> = Vec::new();
    for group in super::penalties::groups(kind) {
        let blanked = group
            .keys
            .iter()
            .all(|key| edits.get(key).is_some_and(|value| text(value).is_empty()));
        if !blanked {
            continue;
        }
        for key in &group.keys {
            let Some(path) = super::penalties::path_of(kind, key) else {
                continue;
            };
            let Some(typed) = before
                .pointer(&path)
                .and_then(Value::as_f64)
                .filter(|amount| *amount != 0.0)
            else {
                continue;
            };
            if after.pointer(&path).is_none_or(Value::is_null) {
                replaced.push(format!(
                    "item {} (\u{20b1}{})",
                    penalty_item(kind, key),
                    pesos(typed)
                ));
            }
        }
    }
    if replaced.is_empty() {
        return None;
    }
    replaced.sort();
    Some((
        format!(
            "Using the computed amounts replaces the penalties you typed in {}.",
            replaced.join(", ")
        ),
        "Replaced the typed penalties".to_string(),
    ))
}

/// The printed item of a penalty box: 1800's 19A-19C, 1801's 23A-23C.
fn penalty_item(kind: AgentFormKind, key: &str) -> String {
    let first = if kind.form_code() == "1800" { 19 } else { 23 };
    let letter = if key.contains("Surcharge") {
        'A'
    } else if key.contains("Interest") {
        'B'
    } else {
        'C'
    };
    format!("{first}{letter}")
}

/// The Amended Return boxes of each form (its group box too).
fn amended_key(kind: AgentFormKind, key: &str) -> bool {
    match kind.form_code() {
        "1800" => key.starts_with("frm1800v2018:AmendedRtn"),
        "1801" => key.starts_with("frm1801v2018:amendedRtn"),
        _ => false,
    }
}

/// When `edits` turned `before` into `after` by removing what the filer
/// typed, what the page asks first (`message`), what the Undo toast says
/// (`done`) and how many things went; `None` when nothing typed went. Every
/// removal of one batch is in the one question (an answer to item 14 and
/// one to 15D together name both).
pub(super) fn removes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    if !matches!(kind.form_code(), "1800" | "1801") {
        return None;
    }
    let touched =
        |cells: &dyn Fn(Cell) -> bool| edits.keys().any(|key| cell(kind, key).is_some_and(cells));
    let typed_text = |draft: &Value, field: &str| {
        draft[field]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    // `(sentence, done)` for each removal.
    let mut removed: Vec<(String, String)> = Vec::new();
    // Amended Return No: the form's recompute zeroes 1800's 17C and 1801's
    // 19B (`tax_paid_previous`, form_1800.rs / form_1801.rs `recompute`).
    if edits.keys().any(|key| amended_key(kind, key)) {
        let was = before["tax_paid_previous"].as_f64().unwrap_or(0.0);
        let now = after["tax_paid_previous"].as_f64().unwrap_or(0.0);
        if was != 0.0 && now == 0.0 {
            let item = if kind.form_code() == "1800" {
                "17C"
            } else {
                "19B"
            };
            removed.push((
                format!(
                    "Answering No at Amended Return clears item {item} (\u{20b1}{}).",
                    pesos(was)
                ),
                format!("Cleared item {item}"),
            ));
        }
    }
    if touched(&|c| matches!(c, Cell::Relief1800(_) | Cell::Relief1801(_)))
        && let Some(held) = typed_text(before, "tax_relief")
        && after["tax_relief"].is_null()
    {
        let (item, sub) = if kind.form_code() == "1800" {
            ("13", "13A")
        } else {
            ("14", "14A")
        };
        removed.push((
            format!("Answering No at item {item} clears item {sub} (\"{held}\")."),
            format!("Cleared item {sub}"),
        ));
    }
    let frequency =
        !before["installment_frequency"].is_null() && after["installment_frequency"].is_null();
    let other = typed_text(before, "installment_frequency_other");
    let other_gone = other.is_some() && typed_text(after, "installment_frequency_other").is_none();
    let others = |held: &Option<String>| {
        held.as_deref()
            .map(|text| format!("the Others (specify) text (\"{text}\")"))
            .unwrap_or_default()
    };
    // 15D No is what cleared them only when this batch turned 15D from Yes
    // to No; a 15D Yes in the same batch as a frequency change leaves the
    // frequency's own sentence.
    let answered_no =
        before["installment_granted"] == json!(true) && after["installment_granted"] != json!(true);
    if touched(&|c| matches!(c, Cell::Installment1801(_)))
        && answered_no
        && (frequency || other_gone)
    {
        let what = match (frequency, other_gone) {
            (true, true) => format!("its frequency of payment and {}", others(&other)),
            (true, false) => "its frequency of payment".to_string(),
            _ => others(&other),
        };
        removed.push((
            format!("Answering No at item 15D clears {what}."),
            "Cleared item 15D's frequency".to_string(),
        ));
    } else if touched(&|c| matches!(c, Cell::Frequency1801(_))) && (frequency || other_gone) {
        // A frequency unticked, or Others changed for another choice.
        let sentence = match (frequency, other_gone) {
            (true, true) => format!(
                "Unticking the frequency of payment at item 15D clears it and {}.",
                others(&other)
            ),
            (true, false) => {
                "Unticking the frequency of payment at item 15D clears it.".to_string()
            }
            _ => format!(
                "Changing item 15D's frequency from Others clears {}.",
                others(&other)
            ),
        };
        removed.push((sentence, "Cleared item 15D's frequency".to_string()));
    }
    // "Use the computed amounts" (maintainer decision (1800/1801 penalties run
    // from the due date)): blanking a penalty group
    // replaces the amounts the filer typed there, so it asks first, as the
    // excise returns, 1701 and 1701Q do; a legacy draft's typed amounts are
    // never replaced silently.
    if let Some(sentence) = replaced_penalties(kind, before, after, edits) {
        removed.push(sentence);
    }
    if removed.is_empty() {
        return None;
    }
    let rows = removed.len();
    let (sentences, done): (Vec<String>, Vec<String>) = removed.into_iter().unzip();
    Some(json!({
        "message": format!("{} Continue?", sentences.join(" ")),
        "done": done.join("; "),
        "rows": rows,
    }))
}
