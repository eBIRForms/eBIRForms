//! The trigger date an as-needed return holds (decision D8c, 2026-10-18).
//!
//! An as-needed return is due relative to an event, not a period: the
//! removal date for the excise returns 2200A, 2200AN, 2200M, 2200P, 2200S
//! and 2200T (Item 1), the withholding date for 1600WP (Item 1), and each
//! other form's own typed date (0605's due date, the transaction date of
//! 1706, 1707, 2000OT and 2552, the donation date of 1800, the date of death
//! of 1801). Once a saved return holds that date it has a computed due date
//! ([`crate::calendar_rules::DeadlineResolver::event_deadline`]); until then
//! it has none.
//!
//! **Only a date the filer entered counts.** Several constructors pre-fill
//! a real date (the excise returns and 1600WP: today or the month's last
//! day; 1606 and 1706: today or January 1; 2552: the date its caller
//! passes), so a stored date alone does not say the event happened then.
//! For those forms ([`PREFILLED_TRIGGER_FORMS`]) a Draft counts only once
//! the filer has typed every part of the date it needs. The record is
//! durable, on the flattened `SubmissionLifecycle`: `trigger_parts_typed`
//! (the date fields typed so far) and `trigger_date_entered` (set once
//! every part needed is typed; parts the return's identity fixes count as
//! given). Both are skipped when empty, so a draft saved before them keeps
//! its bytes until edited. Every save settles both, from the stored row's
//! typed parts that still hold their stored value and the host's own typed
//! writes (an unrelated save writes the stored row's two fields back as
//! they were); a client's value is never kept. A profile import restores
//! a user-chosen backup, so it brings the stored records back unchanged.
//! The desktop views record their own date edits ([`note_trigger_typed`])
//! and the agent's own accepted writes ([`note_trigger_field_written`]). So
//! a date typed over
//! two sessions, or one part edited after a restart, still counts; a
//! cleared part no longer does. A return that left Draft files the date
//! it holds, typed or not, so it is dated by that date; a cancelled queue
//! makes it a Draft again, and without the flag it is undated again. The
//! other as-needed forms start with the date blank (0, empty or `None`), so
//! any date they hold was typed.
//!
//! Penalties (decision P5-7) wait for the typed date only on a Draft that
//! carries `trigger_date_model`, written once when the draft is created by
//! a path that records typing (the shell page, every desktop view of a
//! pre-filled form). A save settles it the same way: a stored row keeps the
//! marker it had, a new row gets one only from a creator — a client's own
//! value is never stored. A row without it (stored before the marker
//! existed) computes its penalties from the date it holds, exactly as
//! before; on Home and the Calendar it stays undated until its date is
//! typed, as it was before.

use chrono::NaiveDate;
use serde_json::Value;

use crate::calendar_rules::{
    DeadlineResolver, EventDueRule, FORM_2552_PRIMARY_OFFERING_RULE, event_due_rule,
};

/// The date an as-needed return holds and the rule its due date follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventTrigger {
    pub date: NaiveDate,
    pub rule: EventDueRule,
}

fn number(draft: &Value, key: &str) -> Option<u32> {
    draft
        .get(key)?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
}

/// A date held as three numeric fields; `None` while any is unset (0) or the
/// day does not exist.
fn date_from(draft: &Value, year: &str, month: &str, day: &str) -> Option<NaiveDate> {
    let year = i32::try_from(number(draft, year)?).ok()?;
    NaiveDate::from_ymd_opt(year, number(draft, month)?, number(draft, day)?)
}

/// 2000OT's Item 1 as stored, dated only when the form's own validator
/// takes it: `MM/DD/YYYY` (its `date_parts`), not before 2018 and not after
/// `today` (`Form2000OTDraft::validate`). Anything else (`M/D/YYYY`, which
/// the desktop view can store as typed, another separator, an impossible
/// day, a year out of range) is no date.
fn date_from_text(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let (month, day, year) = super::form_2000ot::date_parts(text)?;
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    (year >= 2018 && date <= today).then_some(date)
}

/// The as-needed forms whose constructor pre-fills the trigger date.
pub const PREFILLED_TRIGGER_FORMS: &[&str] = &[
    "2200A", "2200AN", "2200M", "2200P", "2200S", "2200T", "1600WP", "1606", "1706", "2552",
];

/// The top-level draft fields that hold `form_code`'s trigger date (what
/// the page records `trigger_date_entered` for). Empty for a form without one.
pub fn trigger_paths(form_code: &str) -> &'static [&'static str] {
    match form_code {
        "2200A" | "2200AN" | "2200M" | "2200P" | "2200S" | "2200T" | "1600WP" => {
            &["month", "day", "year"]
        }
        "0605" => &["due_date"],
        "1606" | "1706" | "1707" | "2552" => {
            &["transaction_month", "transaction_day", "transaction_year"]
        }
        "2000OT" => &["transaction_date"],
        "1800" => &["donation_month", "donation_day", "donation_year"],
        "1801" => &["death_month", "death_day", "death_year"],
        _ => &[],
    }
}

/// Whether the date a draft holds was entered (see the module docs).
fn date_was_entered(form_code: &str, draft: &Value) -> bool {
    if !PREFILLED_TRIGGER_FORMS.contains(&form_code) {
        return true;
    }
    let filed = draft
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status != "Draft");
    filed
        || draft
            .get("trigger_date_entered")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

/// The trigger date of a saved return of `form_code` from its stored
/// `form_drafts.data_json`. `None` for a form that is not as needed or has
/// no trigger-date rule, a return that does not hold an entered date yet, or
/// JSON that does not parse.
pub fn event_trigger_from_json(form_code: &str, data_json: &str) -> Option<EventTrigger> {
    if !DeadlineResolver::is_as_needed(form_code) {
        return None;
    }
    let draft: Value = serde_json::from_str(data_json).ok()?;
    if !date_was_entered(form_code, &draft) {
        return None;
    }
    let rule = due_rule_for(form_code, &draft)?;
    let date = held_trigger_date(form_code, &draft)?;
    Some(EventTrigger { date, rule })
}

/// The due-date rule a draft of `form_code` follows: the form's own, except
/// 2552, where Item 11 picks it (a primary offering is due from the listing
/// date; the LSE and a secondary offering from collection) and an unpicked
/// Item 11 has none yet. The one choice every reader uses.
pub fn due_rule_for(form_code: &str, draft: &Value) -> Option<EventDueRule> {
    let rule = event_due_rule(form_code)?;
    if form_code != "2552" {
        return Some(rule);
    }
    match draft.get("transaction").and_then(Value::as_str) {
        Some("primary_offering") => Some(FORM_2552_PRIMARY_OFFERING_RULE),
        Some("local_stock_exchange" | "secondary_offering") => Some(rule),
        _ => None,
    }
}

/// The trigger field a draft path writes (its top-level segment, when that
/// is one of the form's trigger fields): the one matcher every trigger
/// check uses, here and in the shell.
pub fn trigger_field_of(form_code: &str, path: &str) -> Option<&'static str> {
    let top = path.trim_start_matches('/').split('/').next()?;
    trigger_paths(form_code)
        .iter()
        .copied()
        .find(|field| *field == top)
}

/// The trigger fields a box of the return's identity holds (the period the
/// return was opened for, locked on the page: the excise returns' and
/// 1600WP's month and year; 2200AN's, 2200S's and 2552's whole date). The
/// filer set them by choosing the return; they are not a constructor's
/// guess, so they need no typing.
pub fn identity_trigger_fields(form_code: &str) -> std::collections::BTreeSet<&'static str> {
    let Some(kind) = super::agent::AgentFormKind::from_code(form_code) else {
        return Default::default();
    };
    kind.definition()
        .bindings
        .values()
        .filter(|bound| bound.lock == Some(super::agent::Lock::Identity))
        .flat_map(|bound| super::agent::binding_paths(&bound.binding))
        .filter_map(|path| trigger_field_of(form_code, path))
        .collect()
}

/// Whether `parts` (the typed trigger fields) date the return: at least one
/// typed, and every field the date needs typed or fixed by the return's
/// identity.
pub fn parts_complete(form_code: &str, parts: &std::collections::BTreeSet<String>) -> bool {
    let fields = trigger_paths(form_code);
    let given = identity_trigger_fields(form_code);
    !fields.is_empty()
        && !parts.is_empty()
        && fields
            .iter()
            .all(|field| given.contains(field) || parts.contains(*field))
}

/// Record a trigger-date box the filer typed in outside the shell page (a
/// desktop view's edit event, so retyping the pre-filled value counts too):
/// `field` with the value it now holds is typed, or taken out when cleared
/// to 0, and the flag follows [`parts_complete`]. A field that is not one of
/// the form's trigger fields changes nothing.
pub fn note_trigger_typed(
    form_code: &str,
    lifecycle: &mut super::queueable::SubmissionLifecycle,
    field: &str,
    value: u32,
) {
    if !trigger_paths(form_code).contains(&field) {
        return;
    }
    let mut parts: std::collections::BTreeSet<String> =
        lifecycle.trigger_parts_typed.iter().cloned().collect();
    if value == 0 {
        parts.remove(field);
    } else {
        parts.insert(field.to_string());
    }
    lifecycle.trigger_date_entered = parts_complete(form_code, &parts);
    lifecycle.trigger_parts_typed = parts.into_iter().collect();
}

/// A trigger field holds no value (0, empty, null, every part of an object
/// empty): a write that cleared it.
fn blank_trigger_value(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Number(number)) => number.as_f64() == Some(0.0),
        Some(Value::String(text)) => text.trim().is_empty(),
        Some(Value::Object(object)) => object.values().all(|part| blank_trigger_value(Some(part))),
        Some(_) => false,
    }
}

/// [`note_trigger_typed`] on a draft's JSON, for a host that writes box
/// values rather than a lifecycle (the desktop agent's `agent::fill`, an
/// accepted write being the filer's instruction): `field` as `draft` now
/// holds it is typed, or taken out when the write left it blank, and
/// `trigger_date_entered` follows [`parts_complete`]. Both fields are
/// removed rather than stored empty, as the lifecycle's own serde does.
/// A field that is not one of the form's trigger fields changes nothing.
pub fn note_trigger_field_written(form_code: &str, draft: &mut Value, field: &str) {
    if !trigger_paths(form_code).contains(&field) {
        return;
    }
    let Some(object) = draft.as_object_mut() else {
        return;
    };
    let mut parts: std::collections::BTreeSet<String> = object
        .get("trigger_parts_typed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if blank_trigger_value(object.get(field)) {
        parts.remove(field);
    } else {
        parts.insert(field.to_string());
    }
    match parts.is_empty() {
        true => {
            object.remove("trigger_parts_typed");
        }
        false => {
            object.insert(
                "trigger_parts_typed".to_string(),
                Value::Array(parts.iter().cloned().map(Value::String).collect()),
            );
        }
    }
    if parts_complete(form_code, &parts) {
        object.insert("trigger_date_entered".to_string(), Value::Bool(true));
    } else {
        object.remove("trigger_date_entered");
    }
}

/// The trigger date a draft of `form_code` holds in its fields, entered or
/// pre-filled (what [`event_trigger_from_json`] reads once it counts).
pub fn held_trigger_date(form_code: &str, draft: &Value) -> Option<NaiveDate> {
    match form_code {
        // Item 1: the removal date (the excise returns), the withholding
        // date (1600WP).
        "2200A" | "2200AN" | "2200M" | "2200P" | "2200S" | "2200T" | "1600WP" => {
            date_from(draft, "year", "month", "day")
        }
        "0605" => date_from(draft.get("due_date")?, "year", "month", "day"),
        "1606" | "1706" | "1707" | "2552" => date_from(
            draft,
            "transaction_year",
            "transaction_month",
            "transaction_day",
        ),
        "2000OT" => date_from_text(
            draft.get("transaction_date")?.as_str()?,
            super::penalty_items::today(),
        ),
        "1800" => date_from(draft, "donation_year", "donation_month", "donation_day"),
        "1801" => date_from(draft, "death_year", "death_month", "death_day"),
        _ => None,
    }
}

/// The deadline overrides a return's Calendar row is resolved with
/// (decision P5-9: one definition for the Home and Calendar feed, the
/// Google Calendar sync and the penalty clock): the stored global ones,
/// then the profile's own, for every year of its Forms Set when the form is
/// as needed (its event can fall in any of them), else for the return's
/// taxable year.
pub fn calendar_overrides(
    global: &[crate::calendar_rules::DeadlineOverride],
    profile: &crate::profile::TaxpayerProfile,
    form_code: &str,
    taxable_year: Option<u16>,
) -> Vec<crate::calendar_rules::DeadlineOverride> {
    if DeadlineResolver::is_as_needed(form_code) {
        return as_needed_overrides(global, profile);
    }
    periodic_overrides(global, profile, taxable_year)
}

/// [`calendar_overrides`] for a periodic return of `taxable_year`.
pub fn periodic_overrides(
    global: &[crate::calendar_rules::DeadlineOverride],
    profile: &crate::profile::TaxpayerProfile,
    taxable_year: Option<u16>,
) -> Vec<crate::calendar_rules::DeadlineOverride> {
    let mut overrides = global.to_vec();
    if let Some(year) = taxable_year {
        overrides.extend(crate::integration::profile_deadline_overrides_for_year(
            profile, year,
        ));
    }
    overrides
}

/// [`calendar_overrides`] for every as-needed form of a profile.
pub fn as_needed_overrides(
    global: &[crate::calendar_rules::DeadlineOverride],
    profile: &crate::profile::TaxpayerProfile,
) -> Vec<crate::calendar_rules::DeadlineOverride> {
    let mut overrides = global.to_vec();
    for year in profile.per_year_forms.keys() {
        overrides.extend(crate::integration::profile_deadline_overrides_for_year(
            profile, *year,
        ));
    }
    overrides
}

/// A saved as-needed return that holds its entered trigger date, with the
/// due date that date gives.
#[derive(Debug, Clone)]
pub struct DatedEventReturn {
    pub summary: super::FormDraftSummary,
    pub trigger: EventTrigger,
    pub deadline: crate::calendar_rules::ResolvedTaxDeadline,
}

/// Every saved as-needed return of `tin`, whatever its taxable year, that
/// holds an entered trigger date, with its due date
/// ([`DeadlineResolver::event_deadline`]: `overrides` applied, banking days
/// counted on `working`). What Home, the Calendar and the Google Calendar
/// sync list as dated as-needed rows.
pub fn dated_event_returns(
    db: &crate::db::Database,
    tin: &str,
    overrides: &[crate::calendar_rules::DeadlineOverride],
    working: &crate::calendar_rules::BusinessDayCalendar,
) -> Result<Vec<DatedEventReturn>, crate::db::DbError> {
    let mut out = Vec::new();
    for summary in db.list_draft_summaries_all_years(tin)? {
        if !DeadlineResolver::is_as_needed(&summary.form_code) {
            continue;
        }
        let Some(trigger) = db.draft_event_trigger(summary.id)? else {
            continue;
        };
        let Some(deadline) =
            DeadlineResolver::event_deadline(&trigger.rule, trigger.date, overrides, working)
        else {
            continue;
        };
        out.push(DatedEventReturn {
            summary,
            trigger,
            deadline,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_rules::DueFromTrigger;
    use crate::profile::TaxpayerProfile;
    use serde_json::json;

    fn profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Example Trading Corp",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "040",
            "line_of_business": "Trading",
            "registered_address": "1 Example Street, Pasig",
            "zip_code": "1600",
            "phone": "0280000000",
            "email": "office@example.com",
            "default_form_type": "2551Qv2018",
            "taxpayer_type": "Corporation"
        }))
        .unwrap()
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn trigger_of(code: &str, draft: &impl serde::Serialize) -> Option<EventTrigger> {
        event_trigger_from_json(code, &serde_json::to_string(draft).unwrap())
    }

    /// Each form's trigger is read from the field its draft keeps it in:
    /// the real draft types, serialized as they are stored.
    #[test]
    fn each_as_needed_draft_yields_its_typed_trigger_date() {
        let p = profile();
        macro_rules! item1 {
            ($ty:ty, $code:literal) => {{
                // As the constructor builds it: a pre-filled date, not entered.
                let mut draft = <$ty>::new_from_profile(&p, 2026, 10);
                assert_eq!(trigger_of($code, &draft), None, "{} untouched", $code);
                assert!(
                    !serde_json::to_string(&draft)
                        .unwrap()
                        .contains("trigger_date_entered"),
                    "an unmarked draft stores no marker"
                );
                draft.month = 10;
                draft.day = 7;
                draft.year = 2026;
                draft.lifecycle.trigger_date_entered = true;
                let trigger = trigger_of($code, &draft).expect($code);
                assert_eq!(trigger.date, ymd(2026, 10, 7), $code);
                assert_eq!(trigger.rule.form_code, $code);
                // A return that left Draft was filed with its date.
                draft.lifecycle.trigger_date_entered = false;
                draft.lifecycle.status = crate::forms::FilingStatus::Queued;
                assert!(trigger_of($code, &draft).is_some(), "{} queued", $code);
            }};
        }
        item1!(super::super::form_2200a::Form2200ADraft, "2200A");
        item1!(super::super::form_2200an::Form2200ANDraft, "2200AN");
        item1!(super::super::form_2200m::Form2200MDraft, "2200M");
        item1!(super::super::form_2200p::Form2200PDraft, "2200P");
        item1!(super::super::form_2200s::Form2200SDraft, "2200S");
        item1!(super::super::form_2200t::Form2200TDraft, "2200T");
        item1!(super::super::form_1600wp::Form1600WpDraft, "1600WP");

        let mut wp = super::super::form_1600wp::Form1600WpDraft::new_from_profile(&p, 2026, 10);
        wp.day = 5;
        wp.lifecycle.trigger_date_entered = true;
        assert_eq!(
            trigger_of("1600WP", &wp).unwrap().rule.due,
            DueFromTrigger::DaysAfter(20)
        );

        let mut f0605 = super::super::form_0605::Form0605Draft::new_from_profile(&p, 2026, 1);
        f0605.due_date = None;
        assert_eq!(trigger_of("0605", &f0605), None, "no due date typed yet");
        f0605.due_date = Some(super::super::form_0605::Form0605Date::new(2026, 10, 15).unwrap());
        assert_eq!(trigger_of("0605", &f0605).unwrap().date, ymd(2026, 10, 15));

        // 1606 (decision P4-3): the sale date, pre-filled with today or
        // January 1, so it counts only once entered.
        let mut sale = super::super::form_1606::Form1606Draft::new_from_profile(&p, 2026, 1);
        assert_eq!(trigger_of("1606", &sale), None, "1606 untouched");
        (
            sale.transaction_month,
            sale.transaction_day,
            sale.transaction_year,
        ) = (9, 18, 2026);
        sale.lifecycle.trigger_date_entered = true;
        let trigger = trigger_of("1606", &sale).unwrap();
        assert_eq!(trigger.date, ymd(2026, 9, 18));
        assert_eq!(trigger.rule.due, DueFromTrigger::DayOfFollowingMonth(10));

        let mut f1706 = super::super::form_1706::Form1706Draft::new_from_profile(&p, 2026, 1);
        assert_eq!(trigger_of("1706", &f1706), None, "1706 untouched");
        f1706.lifecycle.trigger_date_entered = true;
        (
            f1706.transaction_month,
            f1706.transaction_day,
            f1706.transaction_year,
        ) = (9, 1, 2026);
        assert_eq!(trigger_of("1706", &f1706).unwrap().date, ymd(2026, 9, 1));

        let mut f1707 = super::super::form_1707::Form1707Draft::new_from_profile(&p, 2026, 1);
        (
            f1707.transaction_month,
            f1707.transaction_day,
            f1707.transaction_year,
        ) = (9, 2, 2026);
        assert_eq!(trigger_of("1707", &f1707).unwrap().date, ymd(2026, 9, 2));

        let mut dst = super::super::form_2000ot::Form2000OTDraft::new_from_profile(&p, 2026);
        assert_eq!(trigger_of("2000OT", &dst), None, "no transaction date yet");
        dst.transaction_date = "09/03/2026".to_string();
        assert_eq!(trigger_of("2000OT", &dst).unwrap().date, ymd(2026, 9, 3));

        let mut stock = super::super::form_2552::Form2552Draft::new_from_profile(&p, 2026, 10, 8);
        stock.set_transaction(super::super::form_2552::Form2552Transaction::LocalStockExchange);
        assert_eq!(trigger_of("2552", &stock), None, "2552 untouched");
        stock.lifecycle.trigger_date_entered = true;
        stock.set_transaction(super::super::form_2552::Form2552Transaction::Unspecified);
        assert_eq!(
            trigger_of("2552", &stock),
            None,
            "Item 11 not picked: no rule yet"
        );
        stock.set_transaction(super::super::form_2552::Form2552Transaction::LocalStockExchange);
        let lse = trigger_of("2552", &stock).unwrap();
        assert_eq!(lse.date, ymd(2026, 10, 8));
        assert_eq!(lse.rule.due, DueFromTrigger::BankingDaysAfter(5));
        stock.set_transaction(super::super::form_2552::Form2552Transaction::PrimaryOffering);
        assert_eq!(
            trigger_of("2552", &stock).unwrap().rule,
            FORM_2552_PRIMARY_OFFERING_RULE
        );

        let mut gift = super::super::form_1800::Form1800Draft::new_from_profile(&p, 2026, 1);
        (gift.donation_month, gift.donation_day, gift.donation_year) = (9, 15, 2026);
        assert_eq!(trigger_of("1800", &gift).unwrap().date, ymd(2026, 9, 15));

        let mut estate = super::super::form_1801::Form1801Draft::new_from_profile(&p, 2026, 1);
        (estate.death_month, estate.death_day, estate.death_year) = (2, 28, 2026);
        assert_eq!(trigger_of("1801", &estate).unwrap().date, ymd(2026, 2, 28));
    }

    /// 2000OT's Item 1 is dated exactly when the form's own validator takes
    /// it (no Item 1 error), so the two cannot drift. One clock reading:
    /// the reader runs on it (`with_clock`); the validator reads the real
    /// clock, so a run that crosses midnight is repeated on the new day.
    #[test]
    fn form_2000ot_dates_only_what_its_validator_accepts() {
        use super::super::FormValidator as _;
        let as_text = |date: NaiveDate| date.format("%m/%d/%Y").to_string();
        let run = |today: NaiveDate| -> Vec<(String, bool, Option<NaiveDate>)> {
            super::super::penalty_items::with_clock(today, || {
                [
                    "09/03/2025".to_string(),
                    "  09/03/2025 ".to_string(),
                    as_text(today),
                    as_text(today + chrono::Duration::days(1)),
                    "01/01/2018".to_string(),
                    "12/31/2017".to_string(),
                    "01/01/0001".to_string(),
                    "1/2/2026".to_string(),
                    "6/5/2025".to_string(),
                    "2025-09-03".to_string(),
                    "09-03-2025".to_string(),
                    "09/03/25".to_string(),
                    "02/30/2025".to_string(),
                    "09/03/2025 x".to_string(),
                    String::new(),
                ]
                .into_iter()
                .map(|text| {
                    let mut draft = super::super::form_2000ot::Form2000OTDraft::new_from_profile(
                        &profile(),
                        2025,
                    );
                    draft.transaction_date = text.clone();
                    let validator_takes_it = !draft
                        .validate()
                        .iter()
                        .any(|(field, _)| field == "transaction_date");
                    let dated = trigger_of("2000OT", &draft).map(|trigger| trigger.date);
                    (text, validator_takes_it, dated)
                })
                .collect()
            })
        };
        let mut today = chrono::Local::now().date_naive();
        let mut results = run(today);
        let after = chrono::Local::now().date_naive();
        if after != today {
            today = after;
            results = run(today);
        }
        let mut accepted = 0;
        for (text, validator_takes_it, dated) in results {
            assert_eq!(dated.is_some(), validator_takes_it, "{text:?}");
            if let Some(date) = dated {
                assert_eq!(as_text(date), text.trim(), "{text:?}");
                accepted += 1;
            }
        }
        assert_eq!(accepted, 4, "the four valid dates (today {today})");
    }

    /// An accepted write to a trigger box (the desktop agent's `fill`) is
    /// the filer's instruction on a typed-date-model draft: the part joins
    /// `trigger_parts_typed`, a cleared box takes it out, and
    /// `trigger_date_entered` follows the whole set — both keys dropped
    /// rather than stored empty, as the lifecycle's own serde does.
    #[test]
    fn an_agent_write_to_a_trigger_box_records_and_unrecords_the_part() {
        // 2552: sale date is month/day/year; day is the box an agent fills.
        let mut draft = json!({
            "trigger_date_model": true,
            "transaction_month": 3,
            "transaction_day": 0,
            "transaction_year": 2026
        });
        note_trigger_field_written("2552", &mut draft, "transaction_day");
        assert!(
            draft.get("trigger_parts_typed").is_none(),
            "a blank write records nothing"
        );
        assert!(draft.get("trigger_date_entered").is_none());
        draft["transaction_day"] = json!(14);
        note_trigger_field_written("2552", &mut draft, "transaction_day");
        assert_eq!(draft["trigger_parts_typed"], json!(["transaction_day"]));
        assert_eq!(draft["trigger_date_entered"], json!(true));
        // Clearing the box takes the part out again.
        draft["transaction_day"] = json!("");
        note_trigger_field_written("2552", &mut draft, "transaction_day");
        assert!(
            draft.get("trigger_parts_typed").is_none(),
            "no parts left: the key is dropped, not stored empty"
        );
        assert!(draft.get("trigger_date_entered").is_none());
        // A field that is no trigger field of the form changes nothing.
        draft["transaction_month"] = json!(4);
        note_trigger_field_written("2552", &mut draft, "taxpayer_name");
        assert!(draft.get("trigger_parts_typed").is_none());
    }

    /// The lifecycle twin of the JSON write: every edit event on a trigger
    /// box records its part (retyping the pre-filled value counts too), a
    /// box cleared to 0 takes its part out, and the flag follows the whole
    /// set. The desktop views' `note_*_date_typed` hooks call this.
    #[test]
    fn a_typed_trigger_part_is_recorded_and_a_cleared_one_removed() {
        let mut lifecycle = super::super::queueable::SubmissionLifecycle::default();
        // 1606's sale date is three boxes, none fixed by the identity.
        note_trigger_typed("1606", &mut lifecycle, "transaction_day", 14);
        assert_eq!(lifecycle.trigger_parts_typed, ["transaction_day"]);
        assert!(!lifecycle.trigger_date_entered);
        note_trigger_typed("1606", &mut lifecycle, "transaction_month", 3);
        note_trigger_typed("1606", &mut lifecycle, "transaction_year", 2026);
        assert!(lifecycle.trigger_date_entered);
        // Clearing it back to 0 takes it out and undates the return.
        note_trigger_typed("1606", &mut lifecycle, "transaction_day", 0);
        assert!(!lifecycle.trigger_date_entered);
        assert!(
            !lifecycle
                .trigger_parts_typed
                .iter()
                .any(|part| part == "transaction_day")
        );
        // A field that is not a trigger field changes nothing.
        note_trigger_typed("1606", &mut lifecycle, "taxpayer_name", 5);
        assert_eq!(lifecycle.trigger_parts_typed.len(), 2);
    }

    #[test]
    fn periodic_forms_and_unknown_json_have_no_trigger() {
        assert_eq!(
            event_trigger_from_json("1601C", r#"{"month":1,"day":1,"year":2026}"#),
            None
        );
        assert_eq!(
            event_trigger_from_json("1606", r#"{"transaction_month":1}"#),
            None,
            "1606 with no full sale date"
        );
        assert_eq!(event_trigger_from_json("2200A", "not json"), None);
        assert_eq!(
            event_trigger_from_json(
                "2200A",
                r#"{"month":0,"day":0,"year":0,"trigger_date_entered":true}"#
            ),
            None,
            "an unset date is no trigger"
        );
        assert_eq!(
            event_trigger_from_json(
                "2200A",
                r#"{"month":2,"day":30,"year":2026,"trigger_date_entered":true}"#
            ),
            None
        );
        assert_eq!(
            event_trigger_from_json("2200A", r#"{"month":2,"day":3,"year":2026}"#),
            None,
            "a pre-filled date nobody entered"
        );
        assert_eq!(
            event_trigger_from_json("2553", r#"{"month":2,"day":3,"year":2026}"#),
            None,
            "an as-needed form with no trigger-date rule"
        );
    }
}
