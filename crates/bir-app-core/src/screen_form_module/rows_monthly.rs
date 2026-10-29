//! The monthly withholding remittances (PR 3: 0619E, 0619F, 1600PT, 1600VT,
//! 1600WP): answers the host writes through the typed draft, because the
//! probed bindings either have none (0619E and 0619F item 11/12) or set a
//! draft field directly and so skip the form's own setter, which also
//! clears what the answer closes.
//!
//! - 0619E item 12 and 0619F item 11, Category of Withholding Agent: the
//!   filer's choice. The form has no setter for it: the host writes
//!   `withholding_agent_category` itself (the profile's government flag is
//!   only the new draft's default), and the field map, the XML and the
//!   print write both boxes from it.
//! - 1600PT and 1600VT Part II (items 13-17, then the attachment's "Other
//!   Selected ATC" rows): a row's ATC is chosen from the category's popup as
//!   `catalog:<position>` and ticked with `toggle_atc` (which keeps popup
//!   order); a changed row keeps its tax base; blank removes the row from
//!   `atc_rows` (toggle_atc would refuse it under item 3 No). The rate
//!   follows the ATC and is never typed. A tax base has no setter: the host
//!   writes the row's `tax_base` and runs the form's `recompute`, which
//!   sets the row's tax withheld and items 18-27. Item 3
//!   (`set_taxes_withheld`) and item 10 (`set_agent_category`) go through
//!   the form's setters, which clear the rows as the official handlers do.
//! - 1600PT and 1600VT `AtcCode1..n`, the official ATC popup's tick boxes
//!   (not drawn: the page picks a row's ATC in the row): a direct fill
//!   ticks the category's `n`th popup ATC into Part II with `toggle_atc`,
//!   and an untick removes its row, so the box shows what the draft holds
//!   (the probe had tied the slots to item 10, PR 14 review F10).
//! - 1600WP item 4 (`set_taxes_withheld`, `cancelAllCompute`) and item 7
//!   (`set_agent_category`). Schedule 1.1 is closed while item 4 is not Yes
//!   (fields.json `enabled_when` "Item 4 Yes."): a write there is refused
//!   and rule 6 fades it ([`gated`]).
//! - An answer or a cleared ATC that removes rows the filer typed is
//!   reported ([`removes`]) so the page asks first and offers Undo. This is
//!   the same contract as PR 2's `rows_withholding::removes`, kept per form
//!   here so the integration merge can join the two.
//!
//! Field keys, the official XML and the print map are the forms' own and
//! unchanged.

use bir_core::forms::agent::{self, AgentFormKind};
use bir_core::forms::form_0619e::{Form0619EDraft, WithholdingAgentCategory as Category0619E};
use bir_core::forms::form_0619f::{Form0619FDraft, WithholdingAgentCategory as Category0619F};
use bir_core::forms::form_1600wp::{Form1600WpAgentCategory, Form1600WpDraft};
use serde_json::{Map, Value, json};

/// What a row's tax rate waits for while it is blank.
pub(super) const RATE_WAITS: &str = "Shows the ATC's rate once this row's ATC is chosen.";
/// What a Schedule 1.1 nature of payment waits for while it is blank.
pub(super) const NATURE_WAITS: &str =
    "Shows the ATC's nature of payment once this row's ATC is chosen.";
/// What a 1600WP Schedule 1.1 box says when item 4 has it closed.
pub(super) const ANSWER_4_FIRST: &str = "Answer Yes to item 4 first.";
/// What a 1600PT or 1600VT rate says when it is typed.
const RATE_FOLLOWS: &str = "The rate follows the ATC chosen in this row.";

/// Schedule 1.1 rows on the 1600WP page.
const WP_SCHEDULE_ROWS: usize = 10;
/// Schedule 1.1 boxes the filer types (the nature of payment and the rate
/// follow the row's ATC and stay locked, item 4 or not).
const WP_SCHEDULE_STEMS: [&str; 4] = [
    "frm1600WP:dtSched:txtTin",
    "frm1600WP:dtSched:txtFullname",
    "frm1600WP:dtSched:drpAtcCode",
    "frm1600WP:dtSched:amount",
];

/// Which of the two Part II popup forms.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Popup {
    Pt,
    Vt,
}

impl Popup {
    fn of(kind: AgentFormKind) -> Option<Self> {
        match kind.form_code() {
            "1600PT" => Some(Self::Pt),
            "1600VT" => Some(Self::Vt),
            _ => None,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Pt => "frm1600PT:",
            Self::Vt => "frm1600VT:",
        }
    }
}

/// A box this module writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Cell {
    /// 0619E item 12 / 0619F item 11: `true` is Government.
    ECategory(bool),
    FCategory(bool),
    /// 1600PT/1600VT Part II row's ATC, tax base and (never typed) rate.
    Atc(Popup, usize),
    Base(Popup, usize),
    Rate(Popup, usize),
    /// 1600PT/1600VT `AtcCode<n>`: popup slot `n - 1` of item 10's list.
    Slot(Popup, usize),
    /// 1600PT/1600VT item 3 and item 10 (`true` is Government).
    Withheld(Popup, bool),
    Category(Popup, bool),
    /// 1600WP item 4 and item 7 (`true` is Government).
    WpWithheld(bool),
    WpCategory(bool),
    /// 1600WP Schedule 1.1 box `(stem index, row)`: closed until item 4 Yes.
    WpSchedule(usize, usize),
}

fn row_of(key: &str, stem: &str, rows: usize) -> Option<usize> {
    let n: usize = key.strip_prefix(stem)?.parse().ok()?;
    (1..=rows).contains(&n).then(|| n - 1)
}

/// Part II rows each popup form's page shows (the printed five, then the
/// attachment's rows): one per official popup slot.
pub(super) fn popup_rows(popup: Popup) -> usize {
    match popup {
        Popup::Pt => 31,
        Popup::Vt => 13,
    }
}

pub(super) fn cell(kind: AgentFormKind, key: &str) -> Option<Cell> {
    match kind.form_code() {
        "0619E" => match key {
            "frm0619E:optCategory:P" => Some(Cell::ECategory(false)),
            "frm0619E:optCategory:G" => Some(Cell::ECategory(true)),
            _ => None,
        },
        "0619F" => match key {
            "frm0619F:optCategory:P" => Some(Cell::FCategory(false)),
            "frm0619F:optCategory:G" => Some(Cell::FCategory(true)),
            _ => None,
        },
        "1600PT" | "1600VT" => {
            let popup = Popup::of(kind)?;
            let rows = popup_rows(popup);
            // The popup's boxes carry no form prefix (`AtcCode1`).
            if let Some(slot) = row_of(key, "AtcCode", rows) {
                return Some(Cell::Slot(popup, slot));
            }
            let rest = key.strip_prefix(popup.prefix())?;
            if let Some(row) = row_of(rest, "txtAtcCd", rows) {
                return Some(Cell::Atc(popup, row));
            }
            if let Some(row) = row_of(rest, "txtPg1P2TaxBase", rows) {
                return Some(Cell::Base(popup, row));
            }
            if let Some(row) = row_of(rest, "txtPg1P2TaxRate", rows) {
                return Some(Cell::Rate(popup, row));
            }
            match rest {
                "rdoPg1I3TaxWithheldYes" => Some(Cell::Withheld(popup, true)),
                "rdoPg1I3TaxWithheldNo" => Some(Cell::Withheld(popup, false)),
                "rdoPg1I10CategoryofWthholdAgentPriv" => Some(Cell::Category(popup, false)),
                "rdoPg1I10CategoryofWthholdAgentGovt" => Some(Cell::Category(popup, true)),
                _ => None,
            }
        }
        "1600WP" => {
            for (stem_index, stem) in WP_SCHEDULE_STEMS.iter().enumerate() {
                if let Some(row) = row_of(key, stem, WP_SCHEDULE_ROWS) {
                    return Some(Cell::WpSchedule(stem_index, row));
                }
            }
            match key {
                "frm1600WP:AnyTaxHeld_1" => Some(Cell::WpWithheld(true)),
                "frm1600WP:AnyTaxHeld_2" => Some(Cell::WpWithheld(false)),
                "frm1600WP:CategoryAgent_P" => Some(Cell::WpCategory(false)),
                "frm1600WP:CategoryAgent_G" => Some(Cell::WpCategory(true)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether the host writes `key` itself (its "cannot be filled here" lock
/// does not apply on the page). The 1600WP Schedule 1.1 boxes and the
/// 1600PT/1600VT rates keep the ordinary binding and lock.
pub(super) fn host_writes(kind: AgentFormKind, key: &str) -> bool {
    cell(kind, key).is_some_and(|cell| {
        !matches!(
            cell,
            Cell::WpSchedule(..) | Cell::Rate(..) | Cell::Base(..) | Cell::Atc(..)
        )
    })
}

/// An answer before what it opens, and an ATC before its row's tax base.
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    match cell(kind, key)? {
        Cell::Atc(..) | Cell::Slot(..) | Cell::WpSchedule(..) => Some(1),
        Cell::Base(..) => Some(2),
        _ => None,
    }
}

/// `Some(closed)` for a 1600WP Schedule 1.1 box: closed while item 4 is not
/// Yes. `None` for every other box.
pub(super) fn gated(kind: AgentFormKind, draft: &Value, key: &str) -> Option<bool> {
    match cell(kind, key)? {
        Cell::WpSchedule(..) => Some(draft["taxes_withheld"] != json!(true)),
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

/// `catalog:<popup position>` -> that position.
fn popup_position(value: &Value) -> Result<Option<usize>, String> {
    let choice = text(value);
    if choice.is_empty() {
        return Ok(None);
    }
    choice
        .strip_prefix("catalog:")
        .and_then(|i| i.parse().ok())
        .map(Some)
        .ok_or_else(|| "Choose an ATC from this return's list.".to_string())
}

fn amount(value: &Value) -> Result<f64, String> {
    let typed_text = text(value);
    if typed_text.is_empty() {
        return Ok(0.0);
    }
    agent::parse_amount(&typed_text)
        .filter(|amount| *amount >= 0.0)
        .ok_or_else(|| format!("`{typed_text}` is not an amount."))
}

/// The 1600PT and 1600VT drafts share one shape (`atc_rows` of code, rate
/// text and tax base; `toggle_atc` by position in the official list).
macro_rules! popup_form {
    ($name:ident, $draft:ty, $category:ty, $options:path, $index:path, $popup:path) => {
        mod $name {
            use super::*;

            fn category(government: bool) -> $category {
                if government {
                    <$category>::Government
                } else {
                    <$category>::Private
                }
            }

            pub(super) fn set_withheld(draft: &mut Value, withheld: bool) -> Result<(), String> {
                let mut form: $draft = typed(draft)?;
                form.set_taxes_withheld(withheld);
                store(draft, &form)
            }

            pub(super) fn set_category(draft: &mut Value, government: bool) -> Result<(), String> {
                let mut form: $draft = typed(draft)?;
                form.set_agent_category(category(government));
                store(draft, &form)
            }

            /// Choose row `row`'s ATC: a new row ticks it in the popup, a
            /// changed row keeps its tax base, blank unticks it. Rows
            /// re-sort into popup order (`getATCCode`).
            pub(super) fn fill_atc(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
                let mut form: $draft = typed(draft)?;
                let wanted = popup_position(value)?;
                let current = form.atc_rows.get(row).map(|r| (
                    $index(&r.atc_code, &r.rate_text),
                    r.tax_base,
                ));
                match (wanted, current) {
                    (None, None) => return Ok(()),
                    // Blank unticks the row's ATC (the popup's untick
                    // removes its row), a code the list does not have (a
                    // legacy typed row) included.
                    (None, Some(_)) => {
                        form.atc_rows.remove(row);
                        form.recompute();
                    }
                    (Some(at), Some((Some(now), _))) if at == now => return Ok(()),
                    (Some(at), current) => {
                        let Some(option) = $options.get(at) else {
                            return Err("Not an ATC on this return's list.".to_string());
                        };
                        if form
                            .atc_rows
                            .iter()
                            .any(|r| $index(&r.atc_code, &r.rate_text) == Some(at))
                        {
                            return Err(format!("{} ({}%) is already chosen.", option.code, option.rate_text));
                        }
                        let base = current.map(|(_, base)| base);
                        if current.is_some() {
                            form.atc_rows.remove(row);
                        }
                        form.toggle_atc(at)?;
                        if let Some(base) = base
                            && let Some(entry) = form
                                .atc_rows
                                .iter_mut()
                                .find(|r| $index(&r.atc_code, &r.rate_text) == Some(at))
                        {
                            entry.tax_base = base;
                        }
                        form.recompute();
                    }
                }
                store(draft, &form)
            }

            /// Tick or untick the ATC in popup slot `slot` of item 10's
            /// list, as the official popup's box does: a tick adds its Part
            /// II row (`toggle_atc`), an untick removes it.
            pub(super) fn fill_slot(draft: &mut Value, slot: usize, value: &Value) -> Result<(), String> {
                let mut form: $draft = typed(draft)?;
                let wanted = ticked(value);
                let Some(category) = form.agent_category else {
                    return if wanted {
                        Err("Please select an option for Item 10.".to_string())
                    } else {
                        Ok(())
                    };
                };
                let Some((at, _)) = $popup(category).nth(slot) else {
                    return if wanted {
                        Err(format!(
                            "Item 10's category lists {} ATCs; this slot is not one of them.",
                            $popup(category).count()
                        ))
                    } else {
                        Ok(())
                    };
                };
                let row = form
                    .atc_rows
                    .iter()
                    .position(|r| $index(&r.atc_code, &r.rate_text) == Some(at));
                match (wanted, row) {
                    (true, None) => form.toggle_atc(at)?,
                    (false, Some(row)) => {
                        form.atc_rows.remove(row);
                        form.recompute();
                    }
                    _ => return Ok(()),
                }
                store(draft, &form)
            }

            pub(super) fn fill_base(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
                let base = amount(value)?;
                let mut form: $draft = typed(draft)?;
                let Some(entry) = form.atc_rows.get_mut(row) else {
                    if base == 0.0 {
                        return Ok(());
                    }
                    return Err("Choose this row's ATC first.".to_string());
                };
                entry.tax_base = base;
                form.recompute();
                store(draft, &form)
            }

            /// The popup for the draft's category (both lists' union before
            /// item 10 is answered), each entry by its position, the row's
            /// own entry marked.
            pub(super) fn options(row: usize, draft: &Value) -> Vec<Value> {
                let form: Option<$draft> = typed(draft).ok();
                let chosen = form
                    .as_ref()
                    .and_then(|form| form.atc_rows.get(row))
                    .and_then(|r| $index(&r.atc_code, &r.rate_text));
                let category = form.as_ref().and_then(|form| form.agent_category);
                $options
                    .iter()
                    .enumerate()
                    .filter(|(_, option)| category.is_none_or(|c| option.offered_to(c)))
                    .map(|(i, option)| {
                        json!({
                            "value": format!("catalog:{i}"),
                            "label": format!("{} — {} ({}%)", option.code, option.description, option.rate_text),
                            "selected": chosen == Some(i),
                        })
                    })
                    .collect()
            }
        }
    };
}

popup_form!(
    pt,
    bir_core::forms::form_1600pt::Form1600PtDraft,
    bir_core::forms::form_1600pt::Form1600PtAgentCategory,
    bir_core::forms::form_1600pt::FORM_1600PT_ATC_OPTIONS,
    bir_core::forms::form_1600pt::form_1600pt_atc_index,
    bir_core::forms::form_1600pt::form_1600pt_popup
);
popup_form!(
    vt,
    bir_core::forms::form_1600vt::Form1600VtDraft,
    bir_core::forms::form_1600vt::Form1600VtAgentCategory,
    bir_core::forms::form_1600vt::FORM_1600VT_ATC_OPTIONS,
    bir_core::forms::form_1600vt::form_1600vt_atc_index,
    bir_core::forms::form_1600vt::form_1600vt_popup
);

/// Write `key` when this module owns it: `None` leaves it to the ordinary
/// fill. `ordinary` is that fill, for a 1600WP Schedule 1.1 box item 4 has
/// open.
pub(super) fn fill(
    kind: AgentFormKind,
    draft: &mut Value,
    key: &str,
    value: &Value,
    ordinary: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Option<Result<(), String>> {
    let cell = cell(kind, key)?;
    Some(write(cell, draft, value, ordinary))
}

fn write(
    cell: Cell,
    draft: &mut Value,
    value: &Value,
    ordinary: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    // A one-answer pair: the ticked box carries the answer, the other
    // box's untick in the same batch says nothing more.
    let answer = matches!(
        cell,
        Cell::ECategory(_)
            | Cell::FCategory(_)
            | Cell::Withheld(..)
            | Cell::Category(..)
            | Cell::WpWithheld(_)
            | Cell::WpCategory(_)
    );
    if answer && !ticked(value) {
        return Ok(());
    }
    match cell {
        Cell::ECategory(government) => {
            let mut form: Form0619EDraft = typed(draft)?;
            form.withholding_agent_category = if government {
                Category0619E::Government
            } else {
                Category0619E::Private
            };
            store(draft, &form)
        }
        Cell::FCategory(government) => {
            let mut form: Form0619FDraft = typed(draft)?;
            form.withholding_agent_category = if government {
                Category0619F::Government
            } else {
                Category0619F::Private
            };
            store(draft, &form)
        }
        Cell::Withheld(Popup::Pt, yes) => pt::set_withheld(draft, yes),
        Cell::Withheld(Popup::Vt, yes) => vt::set_withheld(draft, yes),
        Cell::Category(Popup::Pt, government) => pt::set_category(draft, government),
        Cell::Category(Popup::Vt, government) => vt::set_category(draft, government),
        Cell::Atc(Popup::Pt, row) => pt::fill_atc(draft, row, value),
        Cell::Atc(Popup::Vt, row) => vt::fill_atc(draft, row, value),
        Cell::Slot(Popup::Pt, slot) => pt::fill_slot(draft, slot, value),
        Cell::Slot(Popup::Vt, slot) => vt::fill_slot(draft, slot, value),
        Cell::Base(Popup::Pt, row) => pt::fill_base(draft, row, value),
        Cell::Base(Popup::Vt, row) => vt::fill_base(draft, row, value),
        Cell::Rate(..) => {
            // The page never sends it (the box follows the ATC); blank is
            // no write, anything typed is refused.
            if text(value).is_empty() {
                Ok(())
            } else {
                Err(RATE_FOLLOWS.to_string())
            }
        }
        Cell::WpWithheld(yes) => {
            let mut form: Form1600WpDraft = typed(draft)?;
            form.set_taxes_withheld(yes);
            store(draft, &form)
        }
        Cell::WpCategory(government) => {
            let mut form: Form1600WpDraft = typed(draft)?;
            form.set_agent_category(if government {
                Form1600WpAgentCategory::Government
            } else {
                Form1600WpAgentCategory::Private
            });
            store(draft, &form)
        }
        Cell::WpSchedule(..) => {
            if draft["taxes_withheld"] != json!(true) {
                // Blank is no write at all: nothing to refuse.
                if text(value).is_empty() {
                    return Ok(());
                }
                return Err(ANSWER_4_FIRST.to_string());
            }
            ordinary(draft)
        }
    }
}

/// The 1600PT and 1600VT Part II ATC lists.
pub(super) fn options(kind: AgentFormKind, key: &str, draft: &Value) -> Option<Vec<Value>> {
    match cell(kind, key)? {
        Cell::Atc(Popup::Pt, row) => Some(pt::options(row, draft)),
        Cell::Atc(Popup::Vt, row) => Some(vt::options(row, draft)),
        _ => None,
    }
}

/// What a box the form fills from another box of its row waits for while
/// it shows blank: 1600PT, 1600VT and 1600WP Part II rates, and 1600WP
/// Schedule 1.1's nature of payment and rate.
pub(super) fn waits(kind: AgentFormKind) -> Map<String, Value> {
    let code = kind.form_code();
    let shown = super::schema_keys(code);
    let mut keys: Vec<(String, &str)> = Vec::new();
    if let Some(popup) = Popup::of(kind) {
        for n in 1..=popup_rows(popup) {
            keys.push((format!("{}txtPg1P2TaxRate{n}", popup.prefix()), RATE_WAITS));
        }
    }
    if code == "1600WP" {
        for n in 1..=2 {
            keys.push((format!("frm1600WP:txtTaxRate{n}"), RATE_WAITS));
        }
        for n in 1..=WP_SCHEDULE_ROWS {
            keys.push((format!("frm1600WP:dtSched:naturePayment{n}"), NATURE_WAITS));
            keys.push((format!("frm1600WP:dtSched:txtRatePercent{n}"), RATE_WAITS));
        }
    }
    keys.into_iter()
        .filter(|(key, _)| shown.contains(key))
        .map(|(key, text)| (key, json!(text)))
        .collect()
}

fn rows<'a>(draft: &'a Value, list: &str) -> Vec<&'a Value> {
    draft[list]
        .as_array()
        .map(|rows| rows.iter().collect())
        .unwrap_or_default()
}

fn amount_of(row: &Value, field: &str) -> f64 {
    row[field].as_f64().unwrap_or(0.0)
}

fn has_text(row: &Value, field: &str) -> bool {
    row[field].as_str().is_some_and(|s| !s.trim().is_empty())
}

/// 1600WP Schedule 1.1 rows the filer typed something into.
fn typed_schedule(draft: &Value) -> usize {
    rows(draft, "schedule")
        .into_iter()
        .filter(|row| {
            ["tin", "payee_name", "atc_code"]
                .iter()
                .any(|f| has_text(row, f))
                || amount_of(row, "amount") != 0.0
        })
        .count()
}

/// 1600WP Schedule 1.1 rows whose ATC an answer cleared.
fn cleared_schedule_atcs(before: &Value, after: &Value) -> usize {
    let after = rows(after, "schedule");
    rows(before, "schedule")
        .into_iter()
        .enumerate()
        .filter(|(i, row)| {
            has_text(row, "atc_code") && after.get(*i).is_some_and(|r| !has_text(r, "atc_code"))
        })
        .count()
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// When `edits` turned `before` into `after` by removing what the filer
/// typed, what the page asks first (`message`), what the Undo toast says
/// (`done`) and how many rows went; `None` when nothing typed went. The
/// host applies the edit either way; the page keeps its old draft until
/// the filer chooses Continue.
pub(super) fn removes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    let trigger = edits.iter().find_map(|(key, value)| {
        let cell = cell(kind, key)?;
        match cell {
            Cell::Withheld(_, false) if ticked(value) => Some("Changing item 3 to No"),
            Cell::Category(..) if ticked(value) => Some("Changing item 10"),
            Cell::WpWithheld(false) if ticked(value) => Some("Changing item 4 to No"),
            Cell::WpCategory(_) if ticked(value) => Some("Changing item 7"),
            _ => None,
        }
    });
    let lost = |b: usize, a: usize| b.saturating_sub(a);
    let part_ii = lost(
        rows(before, "atc_rows").len(),
        rows(after, "atc_rows").len(),
    );
    if let Some(trigger) = trigger {
        let wp = kind.form_code() == "1600WP";
        let schedule = if wp {
            lost(typed_schedule(before), typed_schedule(after))
        } else {
            0
        };
        // Item 7 keeps Schedule 1.1's rows but clears their ATCs.
        let atcs = if wp && schedule == 0 {
            cleared_schedule_atcs(before, after)
        } else {
            0
        };
        let mut removed = Vec::new();
        if part_ii > 0 {
            removed.push(format!(
                "the {} you chose",
                count(part_ii, "Part II ATC row")
            ));
        }
        if schedule > 0 {
            removed.push(format!(
                "the {} you typed",
                count(schedule, "Schedule 1.1 row")
            ));
        }
        if removed.is_empty() && atcs == 0 {
            return None;
        }
        let mut message = trigger.to_string();
        if !removed.is_empty() {
            message.push_str(&format!(" removes {}", removed.join(" and ")));
        }
        if atcs > 0 {
            message.push_str(&format!(
                "{} clears the ATC of {}",
                if removed.is_empty() { "" } else { " and" },
                count(atcs, "Schedule 1.1 row")
            ));
        }
        let rows = part_ii + schedule;
        let done = if rows > 0 {
            format!("Removed {}", count(rows, "row"))
        } else {
            format!("Cleared {}", count(atcs, "ATC"))
        };
        return Some(json!({
            "message": format!("{message}. Continue?"),
            "done": done,
            "rows": rows.max(atcs),
        }));
    }
    // A cleared Part II ATC goes with its tax base: ask only when it had one.
    let clears_atc = edits.iter().any(|(key, value)| {
        (!ticked(value) && matches!(cell(kind, key), Some(Cell::Slot(..))))
            || text(value).is_empty()
                && (matches!(cell(kind, key), Some(Cell::Atc(..)))
                    || (kind.form_code() == "1600WP" && key.starts_with("frm1600WP:txtAtcCode")))
    });
    if !clears_atc || part_ii == 0 {
        return None;
    }
    let kept = |row: &Value| {
        rows(after, "atc_rows").iter().any(|other| {
            other["atc_code"] == row["atc_code"] && other["rate_text"] == row["rate_text"]
        })
    };
    let base: f64 = rows(before, "atc_rows")
        .into_iter()
        .filter(|row| !kept(row))
        .map(|row| amount_of(row, "tax_base"))
        .sum();
    if base == 0.0 {
        return None;
    }
    Some(json!({
        "message": format!(
            "Clearing this ATC removes its row and its tax base of ₱{}. Continue?",
            pesos(base)
        ),
        "done": "Removed the row",
        "rows": 1,
    }))
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

/// 1600PT's and 1600VT's Part I text fields that a stored draft may lack
/// (P2-4, `KeptText` in bir-core).
pub(super) const KEPT_PART_I: [&str; 6] = [
    "rdo_code",
    "agent_name",
    "registered_address",
    "zip_code",
    "contact_number",
    "email",
];

/// Whether `kind` keeps a Part I field a stored draft lacks out while it is
/// blank: 1600PT and 1600VT.
pub(super) fn keeps_part_i(kind: AgentFormKind) -> bool {
    matches!(kind.form_code(), "1600PT" | "1600VT")
}

/// The [`KEPT_PART_I`] fields `draft` lacks (none on other forms).
pub(super) fn absent_part_i(kind: AgentFormKind, draft: &Value) -> Vec<&'static str> {
    if !keeps_part_i(kind) {
        return Vec::new();
    }
    KEPT_PART_I
        .into_iter()
        .filter(|field| draft.get(*field).is_none())
        .collect()
}

/// After the page's edits: a field the draft lacked (`absent`) that the
/// filer left blank (blank or spaces) is taken out again, so the page's fill,
/// which writes the JSON key, keeps a missing box missing as the desktop
/// editor does; a typed value stays.
pub(super) fn keep_absent_blank(absent: &[&str], draft: &mut Value) {
    let Some(object) = draft.as_object_mut() else {
        return;
    };
    for field in absent {
        if object
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|text| text.trim().is_empty())
        {
            object.remove(*field);
        }
    }
}
