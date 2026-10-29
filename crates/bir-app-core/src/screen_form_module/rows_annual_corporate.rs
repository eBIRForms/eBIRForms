//! PR 8: the annual corporate income tax returns (1702RT, 1702MX, 1702EX)
//! and 1707A. Boxes the host writes through the draft because the probed
//! bindings cannot say what the printed sheet says, the locks that say why a
//! box is shut, what a blank box waits for, and the answers that remove what
//! the filer typed.
//!
//! - 1702RT Item 10 and the Part III dates: the draft keeps each as a
//!   `Form1702RTDate` (year, month, day), which the probe cannot bind to one
//!   box. The page sends MM/DD/YYYY; the host keeps it as that date, an empty
//!   box clears it, and anything else is refused with what to type.
//! - 1707A Item 12: the draft keeps the answer as `tax_relief` (none for No,
//!   the Item 12A text for Yes), which the probe cannot bind to the two
//!   radios. Yes keeps what 12A holds (blank until typed, which validation
//!   asks for); No clears 12A, which the page asks first ([`removes`]).
//! - 1702MX's tax rates (Schedule 1 item 4, Schedule 2 item 14): the draft
//!   keeps each as a `PercentInput` (hundredths and the official lexeme),
//!   which the probe cannot bind. The host writes the typed rate with the
//!   lexeme the official field map writes, so a typed rate files as the
//!   official program would file it.
//! - 1702RT item 42 MCIT Due takes an amount only while MCIT applies (the
//!   form's own `subject_to_mcit`); otherwise recompute clears it, so the
//!   host refuses it with why. Before 2020 the form computes it, 2% of item
//!   33 (maintainer decision: pre-2020 MCIT is 2% of gross), and it stays
//!   locked with that reason. Its (Add more...) items read OTHERS and the
//!   subtotal once the table holds two or more rows: they are then the
//!   form's, locked and refused.
//! - Computed boxes the probe left as "derived" say they are computed
//!   (bir-core's `agent::draft_locks::annual_corporate::relock`, like every
//!   lock above); a blank one says what fills it ([`waits`]).
//! - An answer whose recompute clears or replaces boxes the filer typed (an
//!   OSD answer, Amended Return No, MCIT no longer applying, an overpayment
//!   gone) is reported ([`removes`]): the page asks first and offers Undo.
//! - A message naming another item number than the printed one reads the
//!   printed number ([`printed_item_numbers`]; maintainer decision D6,
//!   2026-10-18: wording only, the checks and the filed data are unchanged).
//!
//! Field keys, the official XML and the print map are the forms' own and
//! unchanged.

use std::collections::BTreeMap;

use std::collections::BTreeSet;

// The locks themselves — item 42, items 40 and 57 before 2020, a folded
// (Add more...) item, the derived boxes said to be computed and 1702EX's
// shut rows — are bir-core's (`agent::draft_locks::annual_corporate`,
// decision P4-19), so the page and the assistant lock alike. This module
// reads their keys and reasons from there.
use bir_core::forms::agent::draft_locks::annual_corporate::{
    A, EX, EX_ATTACHED_ROWS, EX_FOLDED_LINES, MX, RT, RT_ITEM_57, RT_MCIT_DUE, RT_RATE,
    rt_before_2020_lock, rt_folded_item, rt_item_42_lock, rt_more_rows,
};
pub(super) use bir_core::forms::agent::draft_locks::annual_corporate::{
    EX_ATTACHED_LOCK, RT_57_BEFORE_2020, RT_FOLDED_LOCK, RT_MCIT_BEFORE_2020, RT_MCIT_NO_NET,
    RT_MCIT_REFUSAL, RT_RATE_BEFORE_2020, ex_attached_keys,
};
use bir_core::forms::agent::{self, AgentFormKind, Binding};
use bir_core::forms::form_1702rt::{
    FORM_1702RT_MODALS, Form1702RTDate, Form1702RTDraft, Form1702RTModal,
};
use serde_json::{Map, Value, json};

use super::{schema_entry, schema_keys, set_draft_path};

/// 1702RT's date boxes and the draft field each keeps: Item 10 and Part III
/// rows 23 to 26 (`payment_details[0..4]`, form_1702rt_xml.rs
/// insert_payment_rows).
const RT_DATES: [(&str, &str); 5] = [
    ("txtPg1Pt1I10", "/incorporation_date"),
    ("txtPg1Pt3I23DebitMemoC3Date", "/payment_details/0/date"),
    ("txtPg1Pt3I24CheckC3Date", "/payment_details/1/date"),
    ("txtPg1Pt3I25TaxDebitDate", "/payment_details/2/date"),
    ("txtPg1Pt3I26OthersC3Date", "/payment_details/3/date"),
];

/// What a 1702RT date box says when it is not a date.
pub(super) const RT_DATE_REFUSAL: &str = "Enter the date as MM/DD/YYYY (a real calendar date).";

/// 1702MX's rate boxes: the draft field each keeps and the decimals the
/// official field map writes it with (form_1702mx_official.rs
/// to_official_field_map: item 4B one, items 14B and 14C two).
const MX_RATES: [(&str, &str, usize); 3] = [
    (
        "txtPg2Pt4I34SpecialTaxRate",
        "/relief_basis/special_tax_rate",
        1,
    ),
    ("txtPg2Sc2It14B", "/schedule_2/item_14_special_rate", 2),
    ("txtPg2Sc2It14C", "/schedule_2/item_14_regular_rate", 2),
];

/// What a 1702MX rate box says when it is not a rate it files as typed
/// (`decimals` is the official field map's).
pub(super) fn mx_rate_refusal(decimals: usize) -> String {
    let places = if decimals == 1 {
        "one decimal"
    } else {
        "two decimals"
    };
    format!("Enter the rate as a percent below 100, with at most {places}.")
}

/// 1707A Item 12's radios: Yes and No.
const TAX_RELIEF_YES: &str = "frm1707Av2021:rdoI11TaxTreatyYes";
const TAX_RELIEF_NO: &str = "frm1707Av2021:rdoI11TaxTreatyNo";

fn mx_rate(key: &str) -> Option<(&'static str, usize)> {
    let tail = key.strip_prefix(MX)?;
    MX_RATES
        .iter()
        .find(|(stem, _, _)| *stem == tail)
        .map(|(_, path, decimals)| (*path, *decimals))
}

fn rt_date_path(key: &str) -> Option<&'static str> {
    let tail = key.strip_prefix(RT)?;
    RT_DATES
        .iter()
        .find(|(stem, _)| *stem == tail)
        .map(|(_, path)| *path)
}

/// What an [`EX_ATTACHED_ROWS`] box's note says (maintainer decision
/// (1702EX shut rows locked like 1702MX): the rows the official page keeps
/// shut at load are refused here with [`EX_ATTACHED_LOCK`], and a draft
/// stored with values there says to check them, [`notices`]).
pub(super) const EX_ATTACHED_NOTE: &str = "Filled from the attached schedule.";

/// An [`EX_ATTACHED_ROWS`] amount box: it always shows an amount (0.00
/// blank), so its note is drawn whatever it shows (`notes_always`); its
/// description and legal basis boxes say what fills them in place while
/// blank ([`waits`]).
fn ex_attached_amount(key: &str) -> bool {
    key.ends_with("Amt") || key.ends_with("Amount")
}

/// Whether `key` is one of 1702EX's [`EX_ATTACHED_ROWS`] boxes.
fn ex_attached(kind: AgentFormKind, key: &str) -> bool {
    kind.form_code() == "1702EX" && ex_attached_keys().any(|attached| attached == key)
}

/// Whether the host writes `key` itself (the page shows it open).
pub(super) fn host_writes(kind: AgentFormKind, key: &str) -> bool {
    match kind.form_code() {
        "1702RT" => rt_date_path(key).is_some(),
        "1702MX" => mx_rate(key).is_some(),
        "1707A" => key == TAX_RELIEF_YES || key == TAX_RELIEF_NO,
        _ => false,
    }
}

/// Write one of this module's boxes; `None` for any other box.
pub(super) fn fill(
    kind: AgentFormKind,
    draft: &mut Value,
    key: &str,
    value: &Value,
) -> Option<Result<(), String>> {
    match kind.form_code() {
        "1702RT" => {
            if let Some(path) = rt_date_path(key) {
                return Some(fill_1702rt_date(draft, path, value));
            }
            refuse_1702rt(draft, key, value).map(Err)
        }
        "1702MX" => {
            mx_rate(key).map(|(path, decimals)| fill_1702mx_rate(draft, path, decimals, value))
        }
        "1707A" if host_writes(kind, key) => Some(fill_1707a_tax_relief(draft, key, value)),
        "1702EX" if ex_attached(kind, key) => Some(Err(format!("`{key}` is {EX_ATTACHED_LOCK}"))),
        _ => None,
    }
}

/// A 1702RT box the form keeps right now: item 42 while it is shut (see
/// [`rt_item_42_lock`]), items 40 and 57 before 2020 (see
/// [`rt_before_2020_lock`]), and an (Add more...) item while its table is
/// folded. A blank item 42 is taken: it only clears the box.
#[inline(never)]
fn refuse_1702rt(draft: &Value, key: &str, value: &Value) -> Option<String> {
    if let Some(reason) = rt_before_2020_lock(draft, key) {
        return Some(reason.to_string());
    }
    if key == RT_MCIT_DUE {
        let text = text_of(value);
        if text.is_empty() || agent::parse_amount(&text) == Some(0.0) {
            return None;
        }
        return rt_item_42_lock(draft).map(str::to_string);
    }
    let modal = rt_folded_item(key)?;
    rt_more_rows(draft)
        .is_folded(modal)
        .then(|| format!("`{key}` is {RT_FOLDED_LOCK}"))
}

/// A 1702MX rate: blank clears it, a percent below 100 with no more
/// decimals than the official field map writes keeps it (with that
/// lexeme), anything else is refused.
#[inline(never)]
fn fill_1702mx_rate(
    draft: &mut Value,
    path: &str,
    decimals: usize,
    value: &Value,
) -> Result<(), String> {
    let typed = text_of(value);
    let text = typed.strip_suffix('%').unwrap_or(&typed).trim();
    let rate = if typed.is_empty() {
        json!({"hundredths": null, "raw": ""})
    } else {
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
        if whole.is_empty() || !digits(whole) || !digits(fraction) || fraction.len() > decimals {
            return Err(mx_rate_refusal(decimals));
        }
        // Checked: a long number is refused, never wrapped into a small rate.
        let fraction: i32 = format!("{fraction:0<2}").parse().unwrap_or(0);
        let hundredths = whole
            .parse::<i32>()
            .ok()
            .and_then(|whole| whole.checked_mul(100))
            .and_then(|whole| whole.checked_add(fraction))
            .filter(|hundredths| (0..10_000).contains(hundredths))
            .ok_or_else(|| mx_rate_refusal(decimals))?;
        let raw = format!("{:.*}", decimals, f64::from(hundredths) / 100.0);
        json!({"hundredths": hundredths, "raw": raw})
    };
    set_draft_path(draft, path, rate);
    Ok(())
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A 1702RT date box: MM/DD/YYYY keeps that date, blank clears it.
#[inline(never)]
fn fill_1702rt_date(draft: &mut Value, path: &str, value: &Value) -> Result<(), String> {
    let text = text_of(value);
    let date = if text.is_empty() {
        Value::Null
    } else {
        // MM/DD/YYYY exactly: a two-digit year is not this box's date.
        let shape = text.len() == 10
            && text.char_indices().all(|(i, c)| {
                if i == 2 || i == 5 {
                    c == '/'
                } else {
                    c.is_ascii_digit()
                }
            });
        if !shape {
            return Err(RT_DATE_REFUSAL.to_string());
        }
        let date = Form1702RTDate::parse(&text).map_err(|_| RT_DATE_REFUSAL.to_string())?;
        serde_json::to_value(date).map_err(|error| error.to_string())?
    };
    set_draft_path(draft, path, date);
    Ok(())
}

fn ticked(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::String(text) => text.trim() == "true",
        _ => false,
    }
}

/// 1707A Item 12: Yes keeps (or opens, blank) the relief named in 12A; No
/// clears it. Unticking either radio alone changes nothing: the page ticks
/// the other one.
#[inline(never)]
fn fill_1707a_tax_relief(draft: &mut Value, key: &str, value: &Value) -> Result<(), String> {
    if !ticked(value) {
        return Ok(());
    }
    if key == TAX_RELIEF_NO {
        set_draft_path(draft, "/tax_relief", Value::Null);
    } else if !draft["tax_relief"].is_string() {
        set_draft_path(draft, "/tax_relief", Value::String(String::new()));
    }
    Ok(())
}

/// Forms every amount of whose model is whole pesos: each money box shows
/// whole pesos (`w`), computed ones too (the bound ones already do, from
/// their `Whole` binding). 1702RT: every amount of Form1702RTDraft is a
/// `WholePeso`, and the sheet prints each over a whole-peso comb
/// (html-frozen/1702rt-2018c/writer-cells.json peso_joins). 1702MX: every
/// amount of Form1702MXDraft is a `WholePeso` or `WholePesoInput`, and its
/// sheet says "DO NOT enter Centavos" over Part II and Schedules 2, 4, 7.1
/// and 8.1 (html-frozen/1702mx-2018c/index.html).
/// Display only: a typed fraction still shows (form_page.js formatMoney).
const WHOLE_PESO_FORMS: [&str; 2] = ["1702RT", "1702MX"];

/// Mark the money boxes and columns of a whole-peso form `w` (rule 7): the
/// computed ones, which have no binding to say so. A box bound to an
/// amount (a penalty override) keeps taking centavos, as its binding says.
pub(super) fn mark_whole_pesos(form: &mut Value, code: &str) {
    let Some(kind) = AgentFormKind::from_code(code).filter(|_| WHOLE_PESO_FORMS.contains(&code))
    else {
        return;
    };
    let def = kind.definition();
    let unbound = |key: &Value| {
        key.as_str()
            .is_some_and(|key| !def.bindings.contains_key(key))
    };
    let items = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten());
    for item in items {
        for field in item["f"].as_array_mut().into_iter().flatten() {
            if field["t"] == "money" && unbound(&field["k"]) {
                field["w"] = Value::Bool(true);
            }
        }
        let keys = item["keys"].clone();
        for (c, col) in item["cols"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let cells: Vec<&Value> = keys
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row.get(c))
                .collect();
            if col["t"] == "money" && !cells.is_empty() && cells.iter().all(|key| unbound(key)) {
                col["w"] = Value::Bool(true);
            }
        }
    }
}

/// What a 1702EX pop-up's last fixed line waits for while it is blank.
pub(super) const EX_FOLDED_WAITS: &str =
    "Shows the first (more...) row below, or OTHERS once they hold two or more.";

/// 1702EX items 8 and 9: the box writes the draft's whole field, which the
/// field map cuts into the official lines (loadBGData's 60 and 80
/// characters, upper-cased); the page shows the whole field, as the print
/// shapes it over the sheet's combs.
const EX_WHOLE_FIELDS: [(&str, &str); 2] = [
    ("txtPg1Pt1I8RegisteredName", "registered_name"),
    ("txtPg1Pt1I9RegisteredAddress", "registered_address"),
];

/// Number of Attachments where the new draft holds a default (form_1702rt.rs
/// and form_1702mx.rs new_from_profile: `000`, `00`; form_1702ex.rs: the
/// number 0): the box shows empty while the draft holds exactly that
/// default, and any other count as typed. 1702RT and 1702MX keep a typed 0
/// apart from their default; 1702EX keeps a number, so its typed 0 is the
/// default and shows empty too. `(key, draft field, default)`.
fn attachments() -> [(&'static str, &'static str, Value); 3] {
    [
        (
            "frm1702RT:txtPg1Pt2PagesFilled",
            "number_of_attachments",
            json!("000"),
        ),
        (
            "frm1702MX:txtPg1P2I23NumOfAttachments",
            "number_of_attachments",
            json!("00"),
        ),
        (
            "frm1702EX:txtPg1P2I23NumOfAttachments",
            "number_of_attachments",
            json!(0),
        ),
    ]
}

/// What the page shows where the field map's value is not what the box
/// holds (display only: the field map, the XML and the print are
/// unchanged): 1702EX's whole name and address, and an untyped count of
/// attachments.
pub(super) fn shown(kind: AgentFormKind, draft: &Value, map: &mut BTreeMap<String, String>) {
    let code = kind.form_code();
    if code == "1702EX" {
        for (stem, field) in EX_WHOLE_FIELDS {
            if let Some(text) = draft[field].as_str().map(str::trim)
                && !text.is_empty()
            {
                map.insert(format!("{EX}{stem}"), text.to_string());
            }
        }
    }
    // A 1702MX rate never typed shows empty (the field map writes 0.00).
    if code == "1702MX" {
        for (stem, path, _) in MX_RATES {
            if draft.pointer(path).and_then(|rate| rate.get("hundredths")) == Some(&Value::Null)
                && let Some(shown) = map.get_mut(&format!("{MX}{stem}"))
            {
                shown.clear();
            }
        }
    }
    for (key, field, default) in attachments()
        .into_iter()
        .filter(|(key, _, _)| key.starts_with(&format!("frm{code}:")))
    {
        if same(draft.get(field), Some(&default))
            && let Some(shown) = map.get_mut(key)
        {
            shown.clear();
        }
    }
}

/// Boxes whose lock note shows whatever they hold (screen_form_module
/// `notes_always`): 1702RT item 42 while it is shut, which always shows an
/// amount (0 while MCIT does not apply, 2% of item 33 before 2020), so its
/// note says why (maintainer decision (pre-2020 MCIT is 2% of gross)).
///
/// 1702EX's [`EX_ATTACHED_ROWS`] say what fills them over their 0.00 too
/// (maintainer decision (1702EX shut rows locked like 1702MX)).
pub(super) fn notes_always(kind: AgentFormKind, draft: &Value) -> Vec<String> {
    match kind.form_code() {
        "1702RT" => [RT_MCIT_DUE, RT_RATE, RT_ITEM_57]
            .into_iter()
            .filter(|key| {
                if *key == RT_MCIT_DUE {
                    rt_item_42_lock(draft).is_some()
                } else {
                    rt_before_2020_lock(draft, key).is_some()
                }
            })
            .map(str::to_string)
            .collect(),
        "1702EX" => ex_attached_keys()
            .filter(|key| ex_attached_amount(key))
            .collect(),
        _ => Vec::new(),
    }
}

/// The note a live lock shows under its item, in the filer's words: item
/// 42 says why it is shut.
pub(super) fn live_lock_note(reason: &str) -> Option<&'static str> {
    [
        (RT_MCIT_BEFORE_2020, RT_MCIT_BEFORE_2020_NOTE),
        (RT_MCIT_REFUSAL, RT_MCIT_NOT_YET_NOTE),
        (RT_MCIT_NO_NET, RT_MCIT_NO_NET_NOTE),
        (EX_ATTACHED_LOCK, EX_ATTACHED_NOTE),
        (RT_RATE_BEFORE_2020, RT_RATE_BEFORE_2020_NOTE),
        (RT_57_BEFORE_2020, RT_57_BEFORE_2020_NOTE),
    ]
    .into_iter()
    .find(|(known, _)| *known == reason)
    .map(|(_, note)| note)
}

/// Item 42's notes (see [`live_lock_note`]).
pub(super) const RT_MCIT_BEFORE_2020_NOTE: &str =
    "Computed for this year: MCIT before 2020 is 2% of gross income (item 33).";
pub(super) const RT_RATE_BEFORE_2020_NOTE: &str =
    "Computed for this year: the income tax rate before 2020 is 30%.";
pub(super) const RT_57_BEFORE_2020_NOTE: &str =
    "Computed for this year: item 57 before 2020 is item 35 times 30%.";
pub(super) const RT_MCIT_NOT_YET_NOTE: &str =
    "Shut until MCIT applies, from the fourth year after incorporation.";
pub(super) const RT_MCIT_NO_NET_NOTE: &str =
    "Shut while net taxable income is zero or less: MCIT is then zero.";

/// What 1707A's printed row 4 waits for while it is blank.
pub(super) const A_ROW4_WAITS: &str =
    "Shows row 4 of the continuation below, or OTHERS once that has more rows.";
/// What 1702RT's Schedule IIIA item 4 year waits for.
pub(super) const RT_NOLCO_YEAR_WAITS: &str =
    "Shows this year once Schedule III item 3 is a net operating loss.";

/// What 1702MX's Schedule 7.1 and 8.1 item 7 years wait for: this year's
/// loss (form_1702mx_official.rs recompute fills them from Schedules 7 and 8
/// item 3).
pub(super) const MX_NOLCO_7_YEAR_WAITS: &str =
    "Shows this year once Schedule 7 item 3 is a net operating loss.";
pub(super) const MX_NOLCO_8_YEAR_WAITS: &str =
    "Shows this year once Schedule 8 item 3 is a net operating loss.";

/// What a blank box this module locks waits for (shown in the box).
pub(super) fn waits(kind: AgentFormKind) -> Map<String, Value> {
    let mut out = Map::new();
    match kind.form_code() {
        "1702RT" => {
            out.insert(format!("{RT}txtPg4Sc3AI4C1"), json!(RT_NOLCO_YEAR_WAITS));
        }
        "1702EX" => {
            for (description, _) in EX_FOLDED_LINES {
                out.insert(format!("{EX}{description}"), json!(EX_FOLDED_WAITS));
            }
            // maintainer decision (1702EX shut rows locked like 1702MX): a shut
            // row's text boxes say what fills them.
            for key in ex_attached_keys().filter(|key| !ex_attached_amount(key)) {
                out.insert(key, json!(EX_ATTACHED_NOTE));
            }
        }
        "1702MX" => {
            out.insert(
                "frm1702MX:txtPg3IShed7_7Year".into(),
                json!(MX_NOLCO_7_YEAR_WAITS),
            );
            out.insert(
                "frm1702MX:txtPg4IShed8_7Year".into(),
                json!(MX_NOLCO_8_YEAR_WAITS),
            );
        }
        "1707A" => {
            for stem in [
                "txtS1C1DateOfTransactionI4",
                "txtS1C2NameOfCorporateStockI4",
                "txtS2C1DateOfTransactionI4",
                "txtS2C2NameOfCorporateStockI4",
            ] {
                out.insert(format!("{A}{stem}"), json!(A_ROW4_WAITS));
            }
        }
        _ => {}
    }
    out
}

/// What this edit removed that the filer typed: the page asks `message`
/// first and offers Undo with `done`.
pub(super) fn removes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    if kind.form_code() == "1707A"
        && edits.contains_key(TAX_RELIEF_NO)
        && let Some(relief) = before["tax_relief"]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        && after["tax_relief"].is_null()
    {
        return Some(json!({
            "message": format!(
                "Changing item 12 to No clears item 12A (the relief you specified: {relief}). Continue?"
            ),
            "done": "Cleared item 12A",
            "rows": 0,
        }));
    }
    if ASKS_FIRST.contains(&kind.form_code()) {
        return cleared_entries(kind, before, after, edits);
    }
    None
}

/// The forms whose recompute clears or replaces entries an answer turns
/// off: 1702RT (OSD clears Schedules I, II and IIIA, the pop-up rows and
/// item 57; MCIT no longer applying clears Schedule IV and items 42 and 45;
/// Amended No zeroes item 51; an overpayment gone clears item 21's choice),
/// 1702MX (OSD clears column C of Schedules 5 and 6 and Schedule 7.1 rows
/// 1 to 3, their year incurred and every amount column; Amended No zeroes
/// item 27; MCIT no longer applying clears Schedule 9; an overpayment gone
/// clears item 21's choice) and 1702EX (OSD clears every Schedule 1 item,
/// the other-deduction rows 17d to 17i and their pop-up, and the Schedule 2
/// special rows), in form_1702rt_official.rs, form_1702mx_official.rs and
/// form_1702ex.rs recompute.
const ASKS_FIRST: [&str; 3] = ["1702RT", "1702MX", "1702EX"];

/// The draft fields a binding writes.
fn binding_paths(binding: &Binding) -> Vec<String> {
    match binding {
        Binding::Text { path, .. } | Binding::Amount { path, .. } | Binding::Whole { path, .. } => {
            vec![path.clone()]
        }
        Binding::Choice { options } => {
            let paths: BTreeSet<String> = options
                .iter()
                .flat_map(|option| option.set.keys().cloned())
                .collect();
            paths.into_iter().collect()
        }
    }
}

/// A draft value that holds nothing the filer typed: missing, null, blank,
/// zero or false.
fn holds_nothing(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => {
            let text = text.trim();
            text.is_empty() || text.chars().all(|c| matches!(c, '0' | '.' | ','))
        }
        Some(Value::Number(number)) => number.as_f64() == Some(0.0),
        Some(Value::Bool(flag)) => !flag,
        Some(_) => false,
    }
}

/// The same draft value: numbers by value (the page's transport turns
/// `5000.0` into `5000`).
fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        (Some(Value::Array(a)), Some(Value::Array(b))) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(Some(a), Some(b)))
        }
        (Some(Value::Object(a)), Some(Value::Object(b))) => {
            a.len() == b.len() && a.iter().all(|(k, v)| same(Some(v), b.get(k)))
        }
        _ => a == b,
    }
}

/// Where a key is drawn: its part's name and its item number.
fn placement(code: &str, key: &str) -> Option<(String, String)> {
    let form = schema_entry(code)?;
    for part in form["parts"].as_array()? {
        for item in part["items"].as_array()? {
            let fields = item["f"].as_array().into_iter().flatten().map(|f| &f["k"]);
            let cells = item["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|row| row.as_array().into_iter().flatten());
            if fields.chain(cells).any(|k| k == key) {
                return Some((
                    part["n"].as_str()?.to_string(),
                    item["no"].as_str().unwrap_or("").to_string(),
                ));
            }
        }
    }
    None
}

/// A part's short name: "Schedule I", "Part II".
fn short_part(part: &str) -> &str {
    part.split(" · ").next().unwrap_or(part)
}

/// Every entry box (bound, on the page, not one of this edit's) whose
/// draft field held something before this edit and holds something else
/// after it: cleared or replaced by the form's own recompute.
#[inline(never)]
fn cleared_entries(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    let code = kind.form_code();
    let def = kind.definition();
    let mut edited: BTreeSet<String> = BTreeSet::new();
    for key in edits.keys() {
        if let Some(bound) = def.bindings.get(key) {
            edited.extend(binding_paths(&bound.binding));
        }
        let group = def.box_def(key).and_then(|entry| entry.group.as_deref());
        if let Some(bound) = group.and_then(|group| def.bindings.get(group)) {
            edited.extend(binding_paths(&bound.binding));
        }
        edited.extend(rt_date_path(key).map(str::to_string));
        edited.extend(mx_rate(key).map(|(path, _)| path.to_string()));
    }
    let on_page = schema_keys(code);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut lost: Vec<String> = Vec::new();
    for (key, bound) in &def.bindings {
        if !on_page.contains(key) {
            continue;
        }
        // A radio or checkbox is named only if it was the ticked one.
        if let Binding::Choice { options } = &bound.binding
            && let Some(ticked) = options.iter().find(|option| option.value == "true")
            && !ticked
                .set
                .iter()
                .all(|(path, value)| before.pointer(path) == Some(value))
        {
            continue;
        }
        for path in binding_paths(&bound.binding) {
            if edited.contains(&path) || seen.contains(&path) {
                continue;
            }
            let was = before.pointer(&path);
            if !holds_nothing(was) && !same(was, after.pointer(&path)) {
                seen.insert(path);
                lost.push(key.clone());
                break;
            }
        }
    }
    if lost.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for key in &lost {
        if let Some((part, _)) = placement(code, key) {
            let part = short_part(&part).to_string();
            if !parts.contains(&part) {
                parts.push(part);
            }
        }
    }
    let trigger = edits
        .keys()
        .find_map(|key| placement(code, key))
        .map(|(part, no)| {
            if no.is_empty() {
                short_part(&part).to_string()
            } else if part.starts_with("Schedule") {
                format!("item {no} of {}", short_part(&part))
            } else {
                format!("item {no}")
            }
        })
        .unwrap_or_else(|| "this answer".to_string());
    let n = lost.len();
    let boxes = if n == 1 { "box" } else { "boxes" };
    Some(json!({
        "message": format!(
            "Changing {trigger} clears {n} {boxes} you typed ({}). Continue?",
            parts.join(", ")
        ),
        "done": format!("Cleared {n} {boxes}"),
        "rows": n,
        "keys": lost,
    }))
}

/// Messages that name another item number than the one the printed sheet
/// gives the thing they name, reworded to the printed number (maintainer
/// decision D6, 2026-10-18; an approved exception to rule 0 for wording
/// only). `(form, field prefix, official words, printed words)`. The
/// printed numbers are the frozen sheets' (html-frozen/<slug>/index.html),
/// as the page draws them:
/// - 1707A: the year ended is item 2 (item 1 is Calendar/Fiscal), the
///   number of sheets attached item 4, the tax paid on a previous return
///   item 18B.
/// - 1702RT: Part III's payment rows are items 23 to 26 (the official
///   labels run three ahead, form_1702rt_official.rs validate), Schedule I's
///   sixth OTHERS row is 17h (the official alert reuses 17g), and the date
///   of incorporation is item 10. Its (Add more...) alerts ("Item 54.2",
///   "Item i.2") read the printed item and the row ([`rt_popup_row`]).
/// - 1702MX: the registered name is item 8 and the registered address item
///   9 (the date of incorporation is item 10).
/// - 1702EX: its "(more...)" pop-up alerts name rows the sheet does not
///   print ("item 17.2"); they read the printed item and the row
///   ([`ex_popup_row`]). Its other messages name their printed item.
const PRINTED_ITEM_NUMBERS: [(&str, &str, &str, &str); 11] = [
    ("1707A", "year_end", "on Item 1", "on Item 2"),
    (
        "1707A",
        "number_of_attached_sheets",
        "Item 3 holds",
        "Item 4 holds",
    ),
    (
        "1707A",
        "tax_paid_previous",
        "Item 16B applies",
        "Item 18B applies",
    ),
    (
        "1702RT",
        "payment_details[0]",
        "Part III Item 26.",
        "Part III Item 23.",
    ),
    (
        "1702RT",
        "payment_details[1]",
        "Part III Item 27.",
        "Part III Item 24.",
    ),
    (
        "1702RT",
        "payment_details[2]",
        "Part III Item 28.",
        "Part III Item 25.",
    ),
    (
        "1702RT",
        "payment_details[3]",
        "Part III Item 29.",
        "Part III Item 26.",
    ),
    (
        "1702RT",
        "schedule_1.other[4]",
        "Schedule 1 Item 17g.",
        "Schedule 1 Item 17h.",
    ),
    (
        "1702RT",
        "schedule_4.rows[",
        "(Page 1 Item 8).",
        "(Page 1 Item 10).",
    ),
    (
        "1702MX",
        "taxpayer_name",
        "(Part I Item 9).",
        "(Part I Item 8).",
    ),
    (
        "1702MX",
        "registered_address",
        "(Part I Item 10).",
        "(Part I Item 9).",
    ),
];

/// 1702EX's "(more...)" pop-ups: the official alerts number their rows
/// "17.1", "17.2", ..., which the printed sheet does not print; the sheet
/// prints the item the pop-up belongs to (Schedule 1 item 17i, Schedule 2
/// item 4, Schedule 3 items 3, 6 and 8; html-frozen/1702ex-2018/index.html)
/// and the page lists the pop-up's rows under it. `(field prefix, official
/// number, printed item)`, from form_1702ex.rs POPUP_*.
const EX_POPUP_ITEMS: [(&str, &str, &str); 5] = [
    ("other_deductions[", "17.", "17i"),
    ("special_rows[", "4.", "4"),
    ("non_deductible[", "3.", "3"),
    ("non_taxable_income[", "6.", "6"),
    ("special_deductions_s3[", "8.", "8"),
];

/// A 1702EX pop-up alert ("... on item 17.2") read with the printed item
/// and the pop-up row: "... on item 17i (row 2)".
fn ex_popup_row(field: &str, message: &str) -> Option<String> {
    let (_, official, printed) = EX_POPUP_ITEMS
        .iter()
        .find(|(prefix, _, _)| field.starts_with(prefix))?;
    let marker = format!("on item {official}");
    let at = message.rfind(&marker)?;
    let row = &message[at + marker.len()..];
    if row.is_empty() || !row.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}on item {printed} (row {row})", &message[..at]))
}

/// The printed item of a 1702RT (Add more...) table: Part IV item 54,
/// Schedule I item 17i, Schedule II item 4 and Schedule V items 3, 6 and 8
/// (html-frozen/1702rt-2018c/index.html). The official alerts number the
/// table's rows from the item's own number ("Item 54.2"), and Schedule I's
/// from its letter alone ("Item i.2").
fn rt_printed_item(modal: Form1702RTModal) -> &'static str {
    match modal {
        Form1702RTModal::Schedule1Item17i => "17i",
        other => other.item(),
    }
}

/// A 1702RT (Add more...) alert ("Page 2 Item 54.2 Description should not
/// be blank.", form_1702rt_official.rs validate, field
/// `more_rows.<row name>`) read with the printed item and the table row:
/// "Page 2 Item 54 (row 2) Description should not be blank.".
fn rt_popup_row(field: &str, message: &str) -> Option<String> {
    let row_name = field.strip_prefix("more_rows.")?;
    let modal = FORM_1702RT_MODALS
        .into_iter()
        .find(|modal| modal.row_name() == row_name)?;
    let marker = format!("Page {} Item {}.", modal.page(), modal.item());
    let rest = message.strip_prefix(&marker)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    Some(format!(
        "Page {} Item {} (row {digits}){}",
        modal.page(),
        rt_printed_item(modal),
        &rest[digits.len()..]
    ))
}

/// Each message the form's validation gives, with an item number the
/// printed sheet does not give that item reworded to the printed one.
pub(super) fn printed_item_numbers(kind: AgentFormKind, errors: &mut [(String, String)]) {
    let code = kind.form_code();
    for (field, message) in errors.iter_mut() {
        let popup = match code {
            "1702EX" => ex_popup_row(field, message),
            "1702RT" => rt_popup_row(field, message),
            _ => None,
        };
        if let Some(reworded) = popup {
            *message = reworded;
            continue;
        }
        if let Some((_, _, official, printed)) =
            PRINTED_ITEM_NUMBERS
                .iter()
                .find(|(form, prefix, official, _)| {
                    *form == code && field.starts_with(prefix) && message.contains(official)
                })
        {
            *message = message.replace(official, printed);
        }
    }
}

/// Where `finish` keeps, for one open, what a stored 1702RT held in items
/// 40, 41, 42 and 57 before opening recomputed them (decisions maintainer
/// decision (pre-2020 MCIT is 2% of gross) and
/// maintainer decision (pre-2020 1702RT computed like BIR)), each item that
/// changed by its number; `opened_json` takes it off
/// before anything else reads the draft.
const PRE_2020_HELD: &str = "screen_form_pre_2020_held";

/// Items 40, 41, 42 and 57 of a 1702RT draft (its JSON), as `(item, value)`.
fn pre_2020_items(draft: &Value) -> [(&'static str, Option<i64>); 4] {
    let p4 = &draft["part_iv"];
    [
        ("40", p4["item_40_income_tax_rate_percent"].as_i64()),
        ("41", p4["item_41_normal_income_tax_due"].as_i64()),
        ("42", p4["item_42_mcit_due"].as_i64()),
        (
            "57",
            draft["part_v"]["item_57_special_allowable_deductions_tax_effect"].as_i64(),
        ),
    ]
}

/// Mark `opened` with what items 40, 41, 42 and 57 held when stored
/// (`stored`, the draft before its compute), for each one the compute
/// changed.
pub(super) fn mark_pre_2020_held(opened: &mut Value, stored: Option<&Value>) {
    let Some(stored) = stored else {
        return;
    };
    let now = pre_2020_items(opened);
    let held: Map<String, Value> = pre_2020_items(stored)
        .into_iter()
        .zip(now)
        .filter_map(|((item, was), (_, now))| {
            let was = was?;
            (Some(was) != now).then(|| (item.to_string(), json!(was)))
        })
        .collect();
    if !held.is_empty()
        && let Value::Object(fields) = opened
    {
        fields.insert(PRE_2020_HELD.into(), Value::Object(held));
    }
}

/// Take the mark [`mark_pre_2020_held`] put on a draft, if any.
pub(super) fn take_pre_2020_held(draft: &mut Value) -> Option<Map<String, Value>> {
    let Value::Object(fields) = draft else {
        return None;
    };
    match fields.remove(PRE_2020_HELD)? {
        Value::Object(held) => Some(held),
        _ => None,
    }
}

/// Maintainer decisions (pre-2020 MCIT is 2% of gross; pre-2020 1702RT computed
/// like BIR) on a draft stored before them: a return for a
/// year before 2020 whose items 40, 41, 42 or 57 the form now computes to
/// another amount than they held says so on open, item by item with the
/// old and new amounts, beside the form's own messages, so the change is
/// never silent. Nothing else changes; once saved, the draft holds the
/// computed amounts.
pub(super) fn rt_pre_2020_notices(held: Option<Map<String, Value>>, draft: &Value) -> Vec<Value> {
    let Some(held) = held else {
        return Vec::new();
    };
    let Ok(typed) = serde_json::from_value::<Form1702RTDraft>(draft.clone()) else {
        return Vec::new();
    };
    if typed.taxable_year >= bir_core::forms::form_1702rt_official::MCIT_ENTERED_FROM {
        return Vec::new();
    }
    let now = pre_2020_items(draft);
    let mut out = Vec::new();
    for (item, now) in now {
        let (Some(was), Some(now)) = (held.get(item).and_then(Value::as_i64), now) else {
            continue;
        };
        let (field, key, message) = match item {
            "40" => (
                "part_iv.item_40_income_tax_rate_percent",
                RT_RATE,
                rt_rate_notice_text(now, was),
            ),
            "41" => (
                "part_iv.item_41_normal_income_tax_due",
                "frm1702RT:txtPg2Pt4I41IncomeTaxDue",
                rt_item_41_notice_text(now, was),
            ),
            "42" => (
                "part_iv.item_42_mcit_due",
                RT_MCIT_DUE,
                if typed.subject_to_mcit() {
                    rt_item_42_notice_text(now, was)
                } else if typed.incorporation_date.is_none() {
                    rt_item_42_no_date_notice_text(was)
                } else {
                    rt_item_42_not_yet_notice_text(was)
                },
            ),
            _ => (
                "part_v.item_57_special_allowable_deductions_tax_effect",
                RT_ITEM_57,
                rt_item_57_notice_text(now, was),
            ),
        };
        out.push(json!({ "field": field, "message": message, "keys": [key] }));
    }
    out
}

/// The maintainer decision (pre-2020 MCIT is 2% of gross) notice's words: item
/// 42 is now 2% of item 33.
pub(super) fn rt_item_42_notice_text(computed: i64, held: i64) -> String {
    format!(
        "Item 42 is now computed for years before 2020: 2% of gross income (item 33) is {}; this return held {}. Check the tax due before you file.",
        whole_pesos(computed),
        whole_pesos(held)
    )
}

/// The maintainer decision (pre-2020 MCIT is 2% of gross) notice's words for a
/// return whose incorporation date (item
/// 10) is fewer than four years before its taxable year, whose item 42 the
/// form clears.
pub(super) fn rt_item_42_not_yet_notice_text(held: i64) -> String {
    format!(
        "Not yet subject to MCIT (fewer than four years after incorporation); this return held {}.",
        whole_pesos(held)
    )
}

/// The maintainer decision (pre-2020 MCIT is 2% of gross) notice's words for a
/// return with no incorporation date
/// recorded, so MCIT cannot be applied and the form clears item 42 (round
/// 3, F6: no four-year reason).
pub(super) fn rt_item_42_no_date_notice_text(held: i64) -> String {
    format!(
        "Item 42 is cleared: no incorporation date is recorded, so MCIT can't be applied; this return held {}.",
        whole_pesos(held)
    )
}

/// The maintainer decision (pre-2020 1702RT computed like BIR) notice's words for item 40.
pub(super) fn rt_rate_notice_text(rate: i64, held: i64) -> String {
    format!(
        "Item 40 is now {rate}% for years before 2020; this return held {held}%. Check the tax due before you file."
    )
}

/// The maintainer decision (pre-2020 1702RT computed like BIR) notice's words for item 41.
pub(super) fn rt_item_41_notice_text(computed: i64, held: i64) -> String {
    format!(
        "Item 41 is now computed at 30% for years before 2020: {}; this return held {}. Check the tax due before you file.",
        whole_pesos(computed),
        whole_pesos(held)
    )
}

/// The maintainer decision (pre-2020 1702RT computed like BIR) notice's words for item 57.
pub(super) fn rt_item_57_notice_text(computed: i64, held: i64) -> String {
    format!(
        "Item 57 is now item 35 times 30% for years before 2020: {}; this return held {}. Check it before you file.",
        whole_pesos(computed),
        whole_pesos(held)
    )
}

/// `-1234567` -> `-1,234,567`.
fn whole_pesos(amount: i64) -> String {
    let digits = amount.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if amount < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// Maintainer decision (1702EX shut rows locked like 1702MX) on a draft stored
/// before it: each [`EX_ATTACHED_ROWS`] row
/// holding a description or an amount still files it as stored, and says
/// so beside the form's own messages, so the filer checks it.
fn ex_attached_notices(draft: &Value) -> Vec<Value> {
    let rows: [(&str, &str); 7] = [
        ("Item 49", "/other_credits/1"),
        ("Schedule 1 item 17e", "/other_deductions/1"),
        ("Schedule 1 item 17f", "/other_deductions/2"),
        ("Schedule 1 item 17g", "/other_deductions/3"),
        ("Schedule 1 item 17h", "/other_deductions/4"),
        ("Schedule 2 row 2", "/special_rows/1"),
        ("Schedule 2 row 3", "/special_rows/2"),
    ];
    let mut out = Vec::new();
    for ((item, path), (_, stems)) in rows.iter().zip(EX_ATTACHED_ROWS.iter()) {
        let row = draft.pointer(path);
        let holds = row
            .and_then(Value::as_object)
            .is_some_and(|fields| fields.values().any(|value| !holds_nothing(Some(value))));
        if !holds {
            continue;
        }
        out.push(json!({
            "field": path.trim_start_matches('/').replace('/', "."),
            "message": ex_attached_notice(item),
            "keys": stems.iter().map(|stem| format!("{EX}{stem}")).collect::<Vec<_>>(),
        }));
    }
    out
}

/// The maintainer decision (1702EX shut rows locked like 1702MX) notice's words for `item`.
pub(super) fn ex_attached_notice(item: &str) -> String {
    format!(
        "{item} holds an entry BIR's program keeps shut (it is filled from the attached schedule). It still files as stored; check it before you file."
    )
}

/// What a 1702RT Schedule IV row says while it holds an MCIT that an older
/// version may have saved from column C (see [`notices`]).
pub(super) fn rt_column_c_notice(row: usize) -> String {
    format!(
        "Row {row} holds an MCIT amount that may have been typed in column C; check it before you file."
    )
}

/// Notices beside the form's own messages: nothing is changed, the filer
/// checks. 1702RT Schedule IV rows 2 and 3: before PR 8's review, column C
/// (Excess MCIT, which the form computes) was bound to the row's MCIT, so
/// a value typed there was saved as column B and files as MCIT. A stored
/// draft cannot tell which column it was typed in, so each row 2 or 3
/// holding an MCIT says so (as the maintainer's D5 does for 1604E's
/// changed rows); its stored bytes stay as they are.
pub(super) fn notices(kind: AgentFormKind, draft: &Value) -> Vec<Value> {
    // Only on a Draft: a return that left Draft could never clear it.
    if !super::is_draft_return(kind, draft) {
        return Vec::new();
    }
    if kind.form_code() == "1702EX" {
        return ex_attached_notices(draft);
    }
    if kind.form_code() != "1702RT" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for index in [1usize, 2] {
        let mcit = &draft["schedule_4"]["rows"][index]["mcit"];
        if holds_nothing(Some(mcit)) {
            continue;
        }
        let row = index + 1;
        out.push(json!({
            "field": format!("schedule_4.rows[{index}].mcit"),
            "message": rt_column_c_notice(row),
            "keys": [format!("{RT}txtPg4Sc4I{row}C3")],
        }));
    }
    out
}
