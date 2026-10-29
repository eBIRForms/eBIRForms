//! Dropdown values come from the typed form's own catalogs. A generic ATC
//! list would offer codes a particular return cannot serialize or validate.

use bir_core::forms::agent::{self, AgentFormKind, Binding};
use bir_core::forms::queueable::QueueableKind;
use bir_core::forms::{
    excise_places, form_1600wp, form_1601c, form_1601eq, form_1601fq, form_1701a,
    form_1702mx_official, form_1702q, form_2000, form_2200s, form_2550m, form_2551q,
};
use bir_core::reference;
use serde_json::{Map, Value, json};

use super::{
    rows_1700, rows_2550m, rows_2550q, rows_annual_corporate, rows_annual_individual,
    rows_as_needed, rows_donor_estate, rows_dst_pt_vat, rows_excise, rows_income_q, rows_monthly,
    rows_withholding, schema_entry,
};

fn option(value: &str, label: &str) -> Value {
    json!({"value": value, "label": label})
}

fn code_option(code: &str, description: &str) -> Value {
    option(code, &format!("{code} — {description}"))
}

/// Only the three location structures used by the excise forms. Matching
/// typed paths avoids guessing from opaque or inconsistent official IDs.
fn location(kind: AgentFormKind, key: &str) -> Option<(&str, usize)> {
    if !matches!(
        kind.form_code(),
        "2200A" | "2200AN" | "2200C" | "2200M" | "2200P" | "2200S"
    ) {
        return None;
    }
    let Binding::Text { path, .. } = &kind.definition().bindings.get(key)?.binding else {
        return None;
    };
    let (base, member) = path.rsplit_once('/')?;
    if !matches!(
        base,
        "/place" | "/place_of_production" | "/place_of_removal"
    ) {
        return None;
    }
    let level = ["region", "province", "city"]
        .iter()
        .position(|s| *s == member)?;
    Some((base, level))
}

pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> usize {
    // A 2550M Schedule 1 amount needs its row, which choosing the ATC makes.
    if sales_2550m_row(kind, key).is_some() {
        return 1;
    }
    // A 1600WP Part II tax base needs its row, which choosing the ATC makes.
    if kind.form_code() == "1600WP" && key.starts_with("frm1600WP:txtTaxBase") {
        return 1;
    }
    if kind.form_code() == "1601EQ" && key.starts_with("frm1601EQ:txtTaxBase") {
        return 1;
    }
    if let Some(order) = rows_2550m::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_2550q::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_dst_pt_vat::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_1700::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_withholding::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_monthly::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_income_q::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_annual_individual::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_as_needed::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_donor_estate::edit_order(kind, key) {
        return order;
    }
    if let Some(order) = rows_excise::edit_order(kind, key) {
        return order;
    }
    location(kind, key).map_or(0, |(_, level)| level + 1)
}

/// A lock the page has opened for one box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Lift {
    Nothing,
    /// The filer chose Edit on this profile box: the edit stays on this
    /// return and the profile itself is never written.
    Profile,
}

/// Apply an ordinary user edit, then clear the location's descendants when
/// a parent actually changes. The caller commits this copy only on success.
/// `lift`: which lock the page has opened for this box.
pub(super) fn fill(
    kind: AgentFormKind,
    draft: &mut Value,
    key: &str,
    value: &Value,
    lift: Lift,
) -> Result<(), String> {
    if inactive_schedule(kind, key, draft) {
        return Err(
            "Item 11 (Kind of Transaction) uses another Part V schedule; choose it there first."
                .to_string(),
        );
    }
    if let Some(done) = rows_withholding::fill(kind, draft, key, value, |draft| {
        agent::fill_as_user(kind, draft, key, value)
    }) {
        return done;
    }
    if let Some(done) = rows_monthly::fill(kind, draft, key, value, |draft| {
        agent::fill_as_user(kind, draft, key, value)
    }) {
        return done;
    }
    if let Some(row) = atc_2550m_row(kind, key) {
        return fill_2550m_atc(draft, row, value);
    }
    if let Some(row) = sales_2550m_row(kind, key) {
        return fill_2550m_amount(draft, row, value);
    }
    if let Some(cell) = rows_2550m::cell(kind, key) {
        return rows_2550m::fill(draft, cell, value);
    }
    if let Some(row) = month_year_1601c_row(kind, key) {
        return fill_1601c_month_year(draft, row, value);
    }
    if let Some(row) = rate_2200s_row(kind, key) {
        return fill_2200s_rate(draft, row, value);
    }
    if let Some(row) = atc_1600wp_row(kind, key) {
        return fill_1600wp_atc(draft, row, value);
    }
    if let Some(row) = atc_1601eq_row(kind, key) {
        return fill_1601eq_atc(draft, row, value);
    }
    if let Some(row) = base_1601eq_row(kind, key) {
        return fill_1601eq_base(draft, row, value);
    }
    if let Some((part_b, row)) = spouse_1701a_more_row(kind, key) {
        return fill_1701a_more_spouse(draft, part_b, row, value);
    }
    if let Some(cell) = rows_2550q::cell(kind, key) {
        return rows_2550q::fill(kind, draft, cell, value);
    }
    if let Some(cell) = rows_1700::cell(kind, key) {
        return rows_1700::fill(draft, cell, value);
    }
    if let Some(done) = rows_dst_pt_vat::fill(kind, draft, key, value, |draft| {
        agent::fill_as_user(kind, draft, key, value)
    }) {
        return done;
    }
    if let Some(done) = rows_income_q::fill(kind, draft, key, value, |draft| {
        agent::fill_as_user(kind, draft, key, value)
    }) {
        return done;
    }
    if let Some(done) = rows_annual_corporate::fill(kind, draft, key, value) {
        return done;
    }
    if let Some(done) = rows_annual_individual::fill(kind, draft, key, value) {
        return done;
    }
    if let Some(done) = rows_as_needed::fill(kind, draft, key, value, |draft| {
        agent::fill_as_user(kind, draft, key, value)
    }) {
        return done;
    }
    if let Some(done) = rows_donor_estate::fill(kind, draft, key, value, |draft, value| {
        agent::fill_as_user(kind, draft, key, value)
    }) {
        return done;
    }
    if let Some(row) = atc_2000_row(kind, key)
        && let Some(choice) = value
            .as_str()
            .filter(|s| s.is_empty() || s.starts_with("catalog:"))
    {
        let selected = if choice.is_empty() {
            None
        } else {
            Some(
                choice
                    .strip_prefix("catalog:")
                    .and_then(|s| s.parse::<usize>().ok())
                    .and_then(|i| form_2000::FORM_2000_ATC_OPTIONS.get(i))
                    .ok_or_else(|| "Choose an ATC from this return's list.".to_string())?,
            )
        };
        // Honor the agent's locks, then use the typed setter: DS112 has
        // two descriptions with different calculations for the same code.
        agent::fill_as_user(
            kind,
            draft,
            key,
            &Value::from(selected.map_or("", |a| a.code)),
        )?;
        let mut typed: form_2000::Form2000Draft =
            serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
        if let Some(selected) = selected {
            typed.set_atc(row, selected)?;
        } else {
            typed.clear_atc(row);
        }
        *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
        return Ok(());
    }
    // 2551Q Schedule 1: a choice is the official option number (PT010 has
    // a 3% and a 1% entry); the row takes that ATC and keeps its amount, and
    // the rate follows the period on recompute. A blank choice clears it.
    if kind == AgentFormKind::Form2551Q
        && let Some(row) = atc_2551q_row(key)
        && let Some(choice) = value.as_str()
    {
        let mut typed: form_2551q::Form2551QDraft =
            serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
        if typed.item_13_election == form_2551q::Item13Election::EightPercent {
            return Err("Schedule 1 is disabled while Item 13 is the 8% option.".to_string());
        }
        while typed.schedule_1.len() <= row {
            typed.schedule_1.push(form_2551q::Schedule1Row {
                atc: String::new(),
                atc_description: String::new(),
                taxable_amount: 0.0,
                tax_rate: 0.0,
                tax_due: 0.0,
            });
        }
        let current = &mut typed.schedule_1[row];
        if choice.trim().is_empty() {
            current.atc.clear();
            current.atc_description.clear();
            current.tax_rate = 0.0;
        } else {
            let option = atc_2551q_options()["options"]
                .as_array()
                .and_then(|options| options.iter().find(|o| o["value"] == choice))
                .ok_or_else(|| "Choose an ATC from this return's list.".to_string())?;
            let code = option["code"].as_str().unwrap_or_default();
            let mut chosen = form_2551q::Schedule1Row::new(code)
                .ok_or_else(|| format!("ATC {code} is not in this return's list."))?;
            chosen.taxable_amount = current.taxable_amount;
            *current = chosen;
        }
        current.recompute();
        *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let loc = location(kind, key);
    let before = loc.and_then(|(base, _)| draft.pointer(base).cloned());
    match lift {
        Lift::Profile => agent::fill_profile_box_as_user(kind, draft, key, value)?,
        Lift::Nothing => agent::fill_as_user(kind, draft, key, value)?,
    }
    // 1702RT Item 5: the official select is enabled only once "Other" is
    // chosen, so picking a code chooses "Other".
    if key == "frm1702RT:drpPg1I5AtcOther"
        && kind == AgentFormKind::Queueable(QueueableKind::Form1702RT)
        && value.as_str().is_some_and(|code| !code.trim().is_empty())
    {
        agent::fill_as_user(
            kind,
            draft,
            "frm1702RT:rdoPg1I5AtcOther",
            &Value::from("true"),
        )?;
    }
    if let Some((base, level)) = loc {
        let member = ["region", "province", "city"][level];
        if before.as_ref().and_then(|v| v.get(member)) != draft.pointer(&format!("{base}/{member}"))
        {
            for child_key in kind.definition().bindings.keys() {
                if location(kind, child_key).is_some_and(|(b, l)| b == base && l > level) {
                    agent::fill_as_user(kind, draft, child_key, &Value::from(""))?;
                }
            }
        }
    }
    Ok(())
}

/// A 2552 Part V box of a schedule Item 11 does not select. All three
/// schedules keep their rows in the one draft list (form_2552.rs
/// `schedule`), so a box of another schedule would write over the
/// selected schedule's rows; the page disables it (`enableSchedule`).
pub(super) fn inactive_schedule(kind: AgentFormKind, key: &str, draft: &Value) -> bool {
    if kind.form_code() != "2552" {
        return false;
    }
    let Some(schedule) = key
        .strip_prefix("frm2552:txtPg2Pt5Sch")
        .and_then(|rest| rest.chars().next())
        .and_then(|c| c.to_digit(10))
    else {
        return false;
    };
    let selected = serde_json::from_value::<bir_core::forms::form_2552::Form2552Transaction>(
        draft["transaction"].clone(),
    )
    .ok()
    .and_then(|transaction| transaction.schedule());
    selected != u8::try_from(schedule).ok()
}

/// 2551Q's official Schedule 1 ATC options, numbered as the XML holds them.
fn atc_2551q_options() -> &'static Value {
    static ATCS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    ATCS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../bir-core/data/official-xml/2551q-v2018-atc-options.json"
        ))
        .expect("pinned 2551Q ATC options")
    })
}

/// The Schedule 1 row (0-based) a 2551Q `drpATC1..6` box edits.
fn atc_2551q_row(key: &str) -> Option<usize> {
    key.strip_prefix("drpATC")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=form_2551q::FORM_2551Q_XML_SCHEDULE_ROW_CAPACITY).contains(n))
        .map(|n| n - 1)
}

fn atc_2000_row(kind: AgentFormKind, key: &str) -> Option<usize> {
    if kind.form_code() != "2000" {
        return None;
    }
    let Binding::Text { path, .. } = &kind.definition().bindings.get(key)?.binding else {
        return None;
    };
    path.strip_prefix("/schedule1/")?
        .strip_suffix("/atc_code")?
        .parse()
        .ok()
}

/// 2550M Schedule 1 row of `frm2550m:txtAtcCde<n>` (row `n - 1`, the way
/// `schedule_row_controls` writes it). The official page fills these from
/// its ATC popup, one row per ticked ATC in popup order; the probed
/// bindings cannot say that (they tie rows to the samples' codes), so the
/// host edits them through the typed draft.
fn atc_2550m_row(kind: AgentFormKind, key: &str) -> Option<usize> {
    if kind.form_code() != "2550M" {
        return None;
    }
    let n: usize = key.strip_prefix("frm2550m:txtAtcCde")?.parse().ok()?;
    (1..=form_2550m::FORM_2550M_ATCS.len())
        .contains(&n)
        .then(|| n - 1)
}

/// Whether the host writes `key` itself, so the agent's "cannot be filled
/// here" verdict for it does not apply on the page.
pub(super) fn host_writes(kind: AgentFormKind, key: &str) -> bool {
    atc_2550m_row(kind, key).is_some()
        || rows_dst_pt_vat::host_writes(kind, key)
        || rows_income_q::host_writes(kind, key)
        || rows_annual_corporate::host_writes(kind, key)
        || sales_2550m_row(kind, key).is_some()
        || rows_2550m::cell(kind, key).is_some()
        || month_year_1601c_row(kind, key).is_some()
        || rate_2200s_row(kind, key).is_some()
        || atc_1600wp_row(kind, key).is_some()
        || atc_1601eq_row(kind, key).is_some()
        || base_1601eq_row(kind, key).is_some()
        || spouse_1701a_more_row(kind, key).is_some()
        || rows_2550q::cell(kind, key).is_some()
        || rows_1700::cell(kind, key).is_some()
        || rows_withholding::host_writes(kind, key)
        || rows_monthly::host_writes(kind, key)
        || rows_annual_individual::host_writes(kind, key)
        || rows_as_needed::host_writes(kind, key)
        || rows_donor_estate::host_writes(kind, key)
}

/// 1701A "(add more...)" spouse cell `frm1701A:txtmodalPartIV<A|B>_<n>C3`:
/// `(Part IV.B, row n - 1)`. The probe bound only some of these: the spouse
/// ATC's disable handlers moved the rest, and row 1 shows the Item 42/51
/// text the link transfers. The cell is the filer's (`Form1701AMoreRow`).
fn spouse_1701a_more_row(kind: AgentFormKind, key: &str) -> Option<(bool, usize)> {
    if kind.form_code() != "1701A" {
        return None;
    }
    let rest = key.strip_prefix("frm1701A:txtmodalPartIV")?;
    let (part_b, rest) = match rest.split_at_checked(1)? {
        ("A", rest) => (false, rest),
        ("B", rest) => (true, rest),
        _ => return None,
    };
    let n: usize = rest.strip_prefix('_')?.strip_suffix("C3")?.parse().ok()?;
    n.checked_sub(1).map(|row| (part_b, row))
}

/// A "(add more...)" row's spouse amount. Like the row's other cells it
/// starts the row (and any before it).
fn fill_1701a_more_spouse(
    draft: &mut Value,
    part_b: bool,
    row: usize,
    value: &Value,
) -> Result<(), String> {
    let text = match value {
        Value::Number(n) => n.to_string(),
        other => other.as_str().unwrap_or_default().trim().to_string(),
    };
    let amount = if text.is_empty() {
        0.0
    } else {
        agent::parse_amount(&text).ok_or_else(|| format!("`{text}` is not an amount."))?
    };
    let mut typed: form_1701a::Form1701ADraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    let rows = if part_b {
        &mut typed.eight_other_income_more
    } else {
        &mut typed.other_income_more
    };
    if row >= rows.len() {
        if text.is_empty() {
            return Ok(());
        }
        rows.resize(row + 1, Default::default());
    }
    rows[row].spouse = amount;
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

/// 1601EQ Part II row of `frm1601EQ:txtAtcCd<n>` (row `n - 1`). The page
/// writes one row per ATC ticked in its popup, in popup order
/// (`getATCCode`), so the probe saw codes move between rows and could not
/// bind them; the host ticks and unticks them through the typed draft.
fn atc_1601eq_row(kind: AgentFormKind, key: &str) -> Option<usize> {
    if kind.form_code() != "1601EQ" {
        return None;
    }
    let n: usize = key.strip_prefix("frm1601EQ:txtAtcCd")?.parse().ok()?;
    (1..=form_1601eq::FORM_1601EQ_ATC_OPTIONS.len())
        .contains(&n)
        .then(|| n - 1)
}

/// 1601EQ Part II tax base of `frm1601EQ:txtTaxBase<n>` (row `n - 1`):
/// rows re-sort with their ATC, so the probe tied row 1's box to the ATC
/// as well and left it unbound; the host writes every row's base.
fn base_1601eq_row(kind: AgentFormKind, key: &str) -> Option<usize> {
    if kind.form_code() != "1601EQ" {
        return None;
    }
    let n: usize = key.strip_prefix("frm1601EQ:txtTaxBase")?.parse().ok()?;
    (1..=form_1601eq::FORM_1601EQ_ATC_OPTIONS.len())
        .contains(&n)
        .then(|| n - 1)
}

fn fill_1601eq_base(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let text = match value {
        Value::Number(n) => n.to_string(),
        other => other.as_str().unwrap_or_default().trim().to_string(),
    };
    let amount = if text.is_empty() {
        0.0
    } else {
        agent::parse_amount(&text)
            .filter(|amount| *amount >= 0.0)
            .ok_or_else(|| format!("`{text}` is not an amount."))?
    };
    let mut typed: form_1601eq::Form1601EqDraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    let Some(entry) = typed.schedule.get_mut(row) else {
        if text.is_empty() {
            return Ok(());
        }
        return Err("Choose this row's ATC first.".to_string());
    };
    entry.tax_base = amount;
    typed.recompute();
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

/// Choose a row's ATC: a new row ticks it, a changed row keeps its tax
/// base, blank unticks it. Rows re-sort into popup order.
fn fill_1601eq_atc(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let code = value.as_str().unwrap_or_default().trim();
    let mut typed: form_1601eq::Form1601EqDraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    let current = typed.schedule.get(row).map(|r| r.atc_code.clone());
    if code.is_empty() {
        if let Some(current) = current {
            typed.remove_atc(&current);
        }
    } else if current.as_deref() != Some(code) {
        if typed.schedule.iter().any(|r| r.atc_code == code) {
            return Err(format!("{code} is already selected."));
        }
        match typed.schedule.get_mut(row) {
            Some(existing) => {
                if form_1601eq::FORM_1601EQ_ATC_OPTIONS
                    .iter()
                    .all(|option| option.code != code)
                {
                    return Err(format!(
                        "{code} is not an ATC on the official 1601-EQ list."
                    ));
                }
                existing.atc_code = code.to_string();
                typed.recompute();
            }
            None => typed.add_atc(code)?,
        }
    }
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

/// 1600WP Part II row of `frm1600WP:txtAtcCode<n>` (row `n - 1`). The page
/// writes one row per ATC ticked in its popup (`getATCCode`), two ATCs per
/// category of withholding agent, so there are at most two rows; the host
/// ticks and unticks them through the typed draft (`toggle_atc`).
fn atc_1600wp_row(kind: AgentFormKind, key: &str) -> Option<usize> {
    if kind.form_code() != "1600WP" {
        return None;
    }
    let n: usize = key.strip_prefix("frm1600WP:txtAtcCode")?.parse().ok()?;
    (1..=2).contains(&n).then(|| n - 1)
}

/// Choose a Part II row's ATC: a new row ticks it, a changed row unticks
/// the old ATC and ticks the new one (its tax base starts again, as on the
/// page), blank unticks it. Rows keep the popup's order.
fn fill_1600wp_atc(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let code = value.as_str().unwrap_or_default().trim();
    let mut typed: form_1600wp::Form1600WpDraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    let current = typed.atc_rows.get(row).map(|r| r.atc_code.clone());
    if current.as_deref() == Some(code) {
        return Ok(());
    }
    if !code.is_empty() && typed.atc_rows.iter().any(|r| r.atc_code == code) {
        return Err(format!("{code} is already in Part II."));
    }
    if let Some(current) = current {
        typed.toggle_atc(&current)?;
    }
    if !code.is_empty() {
        typed.toggle_atc(code)?;
    }
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

/// 2200S Schedule 1 "Others" row of `frm2200S:txtsched1AppTaxRate<i>`. The
/// page shows the rate it keeps reformatted (`others_rate_text`), so the
/// probe could not tie the box to `others[i].rate` and left it unbound.
fn rate_2200s_row(kind: AgentFormKind, key: &str) -> Option<usize> {
    if kind.form_code() != "2200S" {
        return None;
    }
    key.strip_prefix("frm2200S:txtsched1AppTaxRate")?
        .parse()
        .ok()
}

/// The row's applicable tax rate (the official box takes 0 to 999.99).
/// Like the row's other boxes it starts the row and any before it.
fn fill_2200s_rate(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let text = match value {
        Value::Number(n) => n.to_string(),
        other => other.as_str().unwrap_or_default().trim().to_string(),
    };
    let rate = if text.is_empty() {
        0.0
    } else {
        agent::parse_amount(text.trim_end_matches('%'))
            .filter(|rate| (0.0..=999.99).contains(rate))
            .ok_or_else(|| "Enter a rate from 0 to 999.99.".to_string())?
    };
    let mut typed: form_2200s::Form2200SDraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    if row >= typed.others.len() {
        if text.is_empty() {
            return Ok(());
        }
        typed.others.resize(row + 1, Default::default());
    }
    typed.others[row].rate = rate;
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

/// 1601C Schedule I row of `frm1601c:sched1:txtMonthYear<i>` (row `i`).
/// The probe saw the page reformat what it typed (`dateMaskingMonthYear`)
/// and left the box unbound; the draft keeps it as `previous_month`, which
/// `to_bir_field_map` writes back to the same box.
fn month_year_1601c_row(kind: AgentFormKind, key: &str) -> Option<usize> {
    if kind.form_code() != "1601C" {
        return None;
    }
    let row: usize = key
        .strip_prefix("frm1601c:sched1:txtMonthYear")?
        .parse()
        .ok()?;
    (row < form_1601c::MAX_SCHEDULE_1_ROWS).then_some(row)
}

/// Previous Month/s (`MM/YYYY`, `maxlength="7"`). Like the row's other
/// boxes it starts the row (and any before it); validation checks the
/// month the way `validateMonthYear` does.
fn fill_1601c_month_year(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let text = value.as_str().unwrap_or_default().trim().to_string();
    if text.chars().count() > 7 {
        return Err("Enter the month as MM/YYYY.".to_string());
    }
    let mut typed: form_1601c::Form1601CDraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    if row >= typed.schedule_1.len() {
        if text.is_empty() {
            return Ok(());
        }
        typed.schedule_1.resize(row + 1, Default::default());
    }
    typed.schedule_1[row].previous_month = text;
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

/// 2550M Schedule 1 row of `frm2550m:txtAmountSales<n>`. Rows re-sort by
/// ATC, so the probe saw these move with another row's code and left four
/// of them unbound; the host writes the row's amount directly.
fn sales_2550m_row(kind: AgentFormKind, key: &str) -> Option<usize> {
    if kind.form_code() != "2550M" {
        return None;
    }
    let n: usize = key.strip_prefix("frm2550m:txtAmountSales")?.parse().ok()?;
    (1..=form_2550m::FORM_2550M_ATCS.len())
        .contains(&n)
        .then(|| n - 1)
}

fn fill_2550m_amount(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let text = match value {
        Value::Number(n) => n.to_string(),
        other => other.as_str().unwrap_or_default().to_string(),
    };
    let amount = if text.trim().is_empty() {
        0.0
    } else {
        agent::parse_amount(&text).ok_or_else(|| format!("`{text}` is not an amount"))?
    };
    let mut typed: form_2550m::Form2550MDraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    let entry = typed
        .sales_schedule
        .get_mut(row)
        .ok_or_else(|| "Choose this row's ATC first.".to_string())?;
    entry.amount = amount;
    typed.recompute();
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

/// Choose an ATC for a Schedule 1 row: a new row ticks it in the popup, an
/// existing row changes its code and keeps its amount, blank unticks it.
/// Rows re-sort into popup order and an ATC appears once, as on the page.
fn fill_2550m_atc(draft: &mut Value, row: usize, value: &Value) -> Result<(), String> {
    let code = value.as_str().unwrap_or_default().trim();
    let mut typed: form_2550m::Form2550MDraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    let current = typed.sales_schedule.get(row).map(|r| r.atc_code.clone());
    if code.is_empty() {
        if let Some(current) = current {
            typed.remove_sales_atc(&current);
        }
    } else if current.as_deref() != Some(code) {
        if form_2550m::form_2550m_atc_index(code).is_none() {
            return Err(format!("{code} is not a 2550M ATC."));
        }
        if typed.sales_schedule.iter().any(|r| r.atc_code == code) {
            return Err(format!("{code} is already in Schedule 1."));
        }
        match typed.sales_schedule.get_mut(row) {
            Some(existing) => {
                existing.atc_code = code.to_string();
                typed.recompute();
            }
            None => typed.add_sales_atc(code)?,
        }
    }
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

fn field_options(kind: AgentFormKind, key: &str, draft: &Value) -> Option<Vec<Value>> {
    if let Some(rows) = rows_withholding::options(kind, key, draft) {
        return Some(rows);
    }
    if let Some(rows) = rows_monthly::options(kind, key, draft) {
        return Some(rows);
    }
    let code = kind.form_code();
    if let Some((base, level)) = location(kind, key) {
        let place = &draft[base.trim_start_matches('/')];
        let region = place["region"].as_str().unwrap_or_default();
        let province = place["province"].as_str().unwrap_or_default();
        let rows = match level {
            0 => excise_places::regions(code),
            1 => excise_places::provinces(code, region),
            _ => excise_places::cities(code, region, province),
        };
        return Some(rows.into_iter().map(|(c, n)| code_option(c, n)).collect());
    }
    if matches!(kind.definition().bindings.get(key).map(|b| &b.binding), Some(Binding::Text { path, .. }) if path.rsplit('/').next().is_some_and(|leaf| leaf == "rdo_code" || leaf.ends_with("_rdo_code")))
    {
        return Some(
            reference::get_all_rdos()
                .iter()
                .map(|r| code_option(&r.code, &r.description))
                .collect(),
        );
    }
    if let Some(rows) = rows_dst_pt_vat::options(kind, key, draft) {
        return Some(rows);
    }
    match code {
        "1601EQ" if atc_1601eq_row(kind, key).is_some() => Some(
            form_1601eq::FORM_1601EQ_ATC_OPTIONS
                .iter()
                .map(|option| code_option(option.code, option.description))
                .collect(),
        ),
        "1600WP" if atc_1600wp_row(kind, key).is_some() => {
            let category = serde_json::from_value::<form_1600wp::Form1600WpAgentCategory>(
                draft["agent_category"].clone(),
            )
            .ok();
            Some(
                form_1600wp::FORM_1600WP_ATC_OPTIONS
                    .iter()
                    .filter(|option| category.is_none_or(|c| option.category == c))
                    .map(|option| {
                        code_option(
                            option.code,
                            &format!("{} ({}%)", option.description, option.rate_text),
                        )
                    })
                    .collect(),
            )
        }
        "2550M" if atc_2550m_row(kind, key).is_some() => Some(
            form_2550m::FORM_2550M_ATCS
                .iter()
                .map(|(c, n)| code_option(c, n))
                .collect(),
        ),
        "1601FQ" if key.starts_with("drpTreatyCode") => Some(
            std::iter::once(option("NA", "Not applicable"))
                .chain(
                    form_1601fq::FORM_1601FQ_TREATIES
                        .iter()
                        .map(|(c, n)| code_option(c, n)),
                )
                .collect(),
        ),
        "1601FQ" if key.starts_with("drpATCCode") => {
            // Schedule 1 accepts the union of both categories (the main
            // ATC popup is category-specific). Keep the first label per code.
            let mut seen = std::collections::BTreeSet::new();
            Some(
                std::iter::once(option("NA", "Not applicable"))
                    .chain(
                        form_1601fq::FORM_1601FQ_PRIVATE_ATCS
                            .iter()
                            .chain(form_1601fq::FORM_1601FQ_GOVERNMENT_ATCS)
                            .filter(|a| seen.insert(a.code))
                            .map(|a| code_option(a.code, a.description)),
                    )
                    .collect(),
            )
        }
        "1702Q" if key == "frm1702q:cbATC_2" => Some(
            form_1702q::FORM_1702Q_ATC_OPTIONS
                .iter()
                .map(|a| option(a.value, a.label))
                .collect(),
        ),
        "1702MX" if key == "frm1702MX:drpPg1I5ATCR2" => Some(
            form_1702mx_official::FORM_1702MX_ATC_OPTIONS
                .iter()
                .map(|(c, n)| option(c, n))
                .collect(),
        ),
        "1702RT" if key == "frm1702RT:drpPg1I5AtcOther" => {
            // Same eight actual codes as the painted page's OTHER_ATCS
            // (form_1702rt_view.rs). Its "none" is a UI sentinel, not a code.
            Some(
                [
                    ("IC010", "Corporation in general"),
                    ("IC020", "Taxable partnership"),
                    ("IC040", "Government entity"),
                    ("IC041", "National government and LGUs"),
                    ("IC070", "Resident foreign corporation"),
                    ("IC101_25%", "Regional operating headquarters 25%"),
                    ("IC190_25%", "Offshore banking units 25%"),
                    ("IC191_25%", "Foreign currency deposit units 25%"),
                ]
                .into_iter()
                .map(|(c, n)| code_option(c, n))
                .collect(),
            )
        }
        "2000" if atc_2000_row(kind, key).is_some() => Some(
            form_2000::FORM_2000_ATC_OPTIONS
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let row = &draft["schedule1"][atc_2000_row(kind, key).unwrap_or_default()];
                    json!({
                        "value": format!("catalog:{i}"),
                        "label": format!("{} — {}", a.code, a.description),
                        "selected": row["atc_code"] == a.code && row["description"] == a.description,
                    })
                })
                .collect(),
        ),
        // Each choice writes its row's ATC and description (the agent
        // form's `drpATC1..6` choice bindings); the rate follows the period.
        "2551Q" if key.starts_with("drpATC") => {
            // The field map holds a 1-based option value, not PT010 etc.
            // In particular PT010 has separate 3% and 1% entries.
            let atcs = atc_2551q_options();
            Some(
                atcs["options"]
                    .as_array()
                    .expect("ATC options array")
                    .iter()
                    .map(|a| {
                        option(
                            a["value"].as_str().expect("option value"),
                            &format!(
                                "{} — {} ({}%)",
                                a["code"].as_str().expect("ATC code"),
                                a["description"].as_str().expect("ATC description"),
                                a["rate"].as_str().expect("ATC rate")
                            ),
                        )
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// Only supply missing lists. Reviewed schema options remain authoritative.
/// Empty province/city lists are intentional until their parent is chosen.
pub(super) fn options(kind: AgentFormKind, draft: &Value) -> Value {
    let mut options = Map::new();
    let Some(form) = schema_entry(kind.form_code()) else {
        return Value::Object(options);
    };
    let mut add = |field: &Value, key: &str| {
        if field["t"] == "sel"
            && field["o"].as_array().is_none_or(Vec::is_empty)
            && let Some(rows) = field_options(kind, key, draft)
        {
            options.insert(key.to_string(), Value::Array(rows));
        }
    };
    for part in form["parts"].as_array().into_iter().flatten() {
        for item in part["items"].as_array().into_iter().flatten() {
            for field in item["f"].as_array().into_iter().flatten() {
                if let Some(key) = field["k"].as_str() {
                    add(field, key);
                }
            }
            for row in item["keys"].as_array().into_iter().flatten() {
                for (col, key) in row.as_array().into_iter().flatten().enumerate() {
                    if let Some(key) = key.as_str() {
                        add(&item["cols"][col], key);
                    }
                }
            }
        }
    }
    Value::Object(options)
}
