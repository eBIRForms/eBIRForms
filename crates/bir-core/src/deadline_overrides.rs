//! Moving one form's deadline for one period, for every taxpayer.
//!
//! A deadline is a rule, and a rule applies to everyone (maintainer
//! decision, 2026-10-26): "adjust the deadline of 2551Q Q3 2026" is one
//! stored global [`DeadlineOverride`] for `2551Q`, the same entry the
//! desktop's admin calendar writes ([`Database::set_deadline_overrides`],
//! which also asks Google Calendar to sync). The Calendar, the Google
//! Calendar sync and every form's penalties read the stored overrides, so
//! a moved deadline moves all three.
//!
//! An override names the deadline it moves by the rule's own date (its
//! `original_deadline`) and the form; [`set`] finds that date from the form
//! and period. As-needed forms are refused: their deadline runs from each
//! return's own trigger date.
//!
//! The arguments and answers are JSON objects, so the agent tools
//! (`bir_deadline_override_*`) and the mini apps' host functions share one
//! shape.

use chrono::{Datelike, NaiveDate};
use serde_json::{Map, Value, json};

use crate::calendar_rules::{
    DeadlineOverride, DeadlinePeriod, DeadlineResolver, Frequency, ResolvedTaxDeadline,
};
use crate::db::Database;
use crate::forms::taxform::{self, TaxPeriod};
use crate::official_holidays::SEED_ID_PREFIX;

/// What an override set without a reason cites as its source. An override
/// with an empty source is ignored by the resolver, so one is always kept.
pub const DEFAULT_REASON: &str = "Set by the user";

fn check_keys(arguments: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match arguments
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        Some(key) => Err(format!(
            "unknown argument `{key}` (allowed: {})",
            allowed.join(", ")
        )),
        None => Ok(()),
    }
}

fn text<'a>(arguments: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim())),
        Some(Value::String(_)) => Ok(None),
        Some(other) => Err(format!("`{key}` must be a string (got {other})")),
    }
}

fn date(arguments: &Map<String, Value>, key: &str) -> Result<Option<NaiveDate>, String> {
    text(arguments, key)?
        .map(|raw| {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .map_err(|_| format!("`{key}` must be a date like 2026-11-03 (got `{raw}`)"))
        })
        .transpose()
}

fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// The calendar's code for `raw` (`1601-C` -> `1601C`), when BIR's calendar
/// has a dated deadline for it.
fn calendar_form(raw: &str) -> Result<String, String> {
    let code: String = raw
        .trim()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_uppercase();
    match DeadlineResolver::frequency_of(&code) {
        None => Err(format!("BIR's calendar has no deadline for form `{raw}`")),
        Some(Frequency::AsNeeded) => Err(format!(
            "{code} is filed as needed: each return is due from its own transaction date, so it has no calendar deadline to move"
        )),
        Some(_) => Ok(code),
    }
}

/// A calendar period as a token names it.
fn tax_period(period: &DeadlinePeriod) -> Option<TaxPeriod> {
    Some(match *period {
        DeadlinePeriod::Monthly {
            taxable_year,
            month,
        } => TaxPeriod::Month {
            year: u16::try_from(taxable_year).ok()?,
            month,
        },
        DeadlinePeriod::Quarterly {
            taxable_year,
            quarter,
        } => TaxPeriod::Quarter {
            year: u16::try_from(taxable_year).ok()?,
            quarter,
        },
        DeadlinePeriod::Annual { taxable_year } => {
            TaxPeriod::Year(u16::try_from(taxable_year).ok()?)
        }
        DeadlinePeriod::EventBased => return None,
    })
}

fn deadline_period(period: TaxPeriod) -> Option<DeadlinePeriod> {
    Some(match period {
        TaxPeriod::Month { year, month } => DeadlinePeriod::Monthly {
            taxable_year: i32::from(year),
            month,
        },
        TaxPeriod::Quarter { year, quarter } => DeadlinePeriod::Quarterly {
            taxable_year: i32::from(year),
            quarter,
        },
        TaxPeriod::Year(year) => DeadlinePeriod::Annual {
            taxable_year: i32::from(year),
        },
        TaxPeriod::Day(_) => return None,
    })
}

/// `code`'s deadline for `period`, resolved with `overrides` (none: the
/// rule's own date moved off weekends).
fn resolved(
    code: &str,
    period: &DeadlinePeriod,
    overrides: &[DeadlineOverride],
) -> Option<ResolvedTaxDeadline> {
    DeadlineResolver::deadlines_for_forms(&[code.to_string()], period.taxable_year()?, overrides)
        .into_iter()
        .find(|deadline| &deadline.period == period)
}

/// `period` as `code` is filed: a month of a quarterly form is its quarter;
/// anything else must name the form's own kind of period.
fn period_for(code: &str, raw: &str, today: NaiveDate) -> Result<DeadlinePeriod, String> {
    let parsed = taxform::parse(raw, today);
    let period = parsed
        .period
        .filter(|_| parsed.unknown.is_empty() && parsed.tin.is_none() && parsed.form.is_none())
        .ok_or_else(|| {
            format!("`period` must be a period like 2026-Q3, 2026-09 or 2025 (got `{raw}`)")
        })?;
    let frequency = DeadlineResolver::frequency_of(code);
    let period = match (frequency, period) {
        (Some(Frequency::Monthly), TaxPeriod::Month { .. })
        | (Some(Frequency::Quarterly), TaxPeriod::Quarter { .. })
        | (Some(Frequency::Annual), TaxPeriod::Year(_)) => period,
        (Some(Frequency::Quarterly), TaxPeriod::Month { year, month }) => TaxPeriod::Quarter {
            year,
            quarter: (month - 1) / 3 + 1,
        },
        (Some(Frequency::Monthly), _) => {
            return Err(format!("{code} is filed monthly: name the month, like 2026-09"));
        }
        (Some(Frequency::Quarterly), _) => {
            return Err(format!("{code} is filed quarterly: name the quarter, like 2026-Q3"));
        }
        _ => return Err(format!("{code} is filed yearly: name the tax year, like 2025")),
    };
    deadline_period(period).ok_or_else(|| format!("`{raw}` is not a period of {code}"))
}

/// Whether `entry` moves `deadline` (the resolver's own test: the rule's
/// date, or that date moved off a weekend).
fn moves(entry: &DeadlineOverride, deadline: &ResolvedTaxDeadline) -> bool {
    entry
        .affected_form_codes
        .iter()
        .any(|code| code == &deadline.form_code)
        && (deadline.original_deadline_date() == Some(entry.original_deadline)
            || deadline.final_deadline_date() == Some(entry.original_deadline))
}

/// A seeded official holiday: one bir seeds now (`official-holiday-…`), or
/// one the desktop's former 2026 seed stored under its own id (a random
/// UUID) for many forms on a date bir ships as a holiday. Neither is the
/// user's to move; the user's own (one form, set here) never matches.
fn is_holiday(entry: &DeadlineOverride) -> bool {
    entry.id.starts_with(SEED_ID_PREFIX)
        || entry.affected_form_codes.len() > 1
            && crate::official_holidays::OFFICIAL_HOLIDAY_YEARS
            .iter()
            .flat_map(|year| year.dates())
            .any(|(date, _)| date == entry.original_deadline)
}

/// The deadlines (form and period) a stored override moves, read off the
/// rules for the years its date can belong to. Holidays move every form
/// that falls on them and are not listed form by form.
fn periods_moved(entry: &DeadlineOverride) -> Vec<Value> {
    if is_holiday(entry) {
        return Vec::new();
    }
    let year = entry.original_deadline.year();
    let mut out = Vec::new();
    for code in &entry.affected_form_codes {
        for taxable_year in (year - 1)..=year {
            for deadline in DeadlineResolver::deadlines_for_forms(&[code.clone()], taxable_year, &[])
            {
                if !moves(entry, &deadline) {
                    continue;
                }
                if let Some(period) = tax_period(&deadline.period) {
                    out.push(json!({
                        "form": code,
                        "period": period.canonical(),
                        "label": period.label(),
                    }));
                }
            }
        }
    }
    out
}

fn entry_json(entry: &DeadlineOverride) -> Value {
    json!({
        "id": entry.id,
        "kind": if is_holiday(entry) { "holiday" } else { "moved" },
        "title": entry.title,
        "reason": entry.source_reference,
        "forms": entry.affected_form_codes,
        "original_deadline": iso(entry.original_deadline),
        "new_deadline": iso(entry.adjusted_deadline),
        "periods": periods_moved(entry),
    })
}

fn stored(db: &Database) -> Result<Vec<DeadlineOverride>, String> {
    if db.deadline_overrides_unreadable() {
        return Err(
            "the stored deadline overrides cannot be read, so none can be changed here; repair them in the desktop app's calendar settings"
                .into(),
        );
    }
    Ok(db.get_deadline_overrides())
}

/// `{year?}`: every stored deadline override (`year`: those whose original
/// or new date falls in it), the moved deadlines first, then the official
/// holidays the app ships. `damaged` counts stored entries that cannot be
/// read (kept as stored, never listed).
pub fn list(db: &Database, arguments: &Map<String, Value>) -> Result<Value, String> {
    check_keys(arguments, &["year"])?;
    let year = match arguments.get("year") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|raw| raw.trim().parse().ok()))
                .and_then(|year| i32::try_from(year).ok())
                .ok_or_else(|| format!("`year` must be a year like 2026 (got {value})"))?,
        ),
    };
    let mut entries: Vec<DeadlineOverride> = db
        .get_deadline_overrides()
        .into_iter()
        .filter(|entry| {
            year.is_none_or(|year| {
                entry.original_deadline.year() == year || entry.adjusted_deadline.year() == year
            })
        })
        .collect();
    entries.sort_by_key(|entry| (is_holiday(entry), entry.original_deadline));
    Ok(json!({
        "year": year,
        "overrides": entries.iter().map(entry_json).collect::<Vec<_>>(),
        "damaged": db.damaged_deadline_overrides().len(),
        "unreadable": db.deadline_overrides_unreadable(),
    }))
}

/// The deadline an override is about: from `period`, or the one of `code`
/// whose rule date (or that date moved off a weekend) is
/// `original_deadline`; both must agree when both are given.
fn target(
    code: &str,
    arguments: &Map<String, Value>,
    today: NaiveDate,
) -> Result<ResolvedTaxDeadline, String> {
    let original = date(arguments, "original_deadline")?;
    let by_period = match text(arguments, "period")? {
        Some(raw) => {
            let period = period_for(code, raw, today)?;
            let label = tax_period(&period).map_or_else(|| raw.to_string(), TaxPeriod::label);
            Some(resolved(code, &period, &[]).ok_or_else(|| {
                format!("BIR's calendar has no {code} deadline for {label}")
            })?)
        }
        None => None,
    };
    match (by_period, original) {
        (Some(deadline), None) => Ok(deadline),
        (Some(deadline), Some(original)) => {
            if deadline.original_deadline_date() == Some(original)
                || deadline.final_deadline_date() == Some(original)
            {
                Ok(deadline)
            } else {
                Err(format!(
                    "{code} for {} is due {} by BIR's calendar, not {}",
                    tax_period(&deadline.period).map_or_else(String::new, TaxPeriod::label),
                    deadline.final_deadline_date().map_or_else(String::new, iso),
                    iso(original)
                ))
            }
        }
        (None, Some(original)) => ((original.year() - 2)..=original.year())
            .flat_map(|year| DeadlineResolver::deadlines_for_forms(&[code.to_string()], year, &[]))
            .find(|deadline| {
                deadline.original_deadline_date() == Some(original)
                    || deadline.final_deadline_date() == Some(original)
            })
            .ok_or_else(|| format!("no {code} deadline falls on {} in BIR's calendar", iso(original))),
        (None, None) => Err(
            "name the deadline: `period` (like 2026-Q3 or 2026-09) or `original_deadline` (YYYY-MM-DD)"
                .into(),
        ),
    }
}

/// A user's own override of exactly `code` on `original` (not a holiday,
/// not a BIR extension that names other forms too).
fn own_override(entry: &DeadlineOverride, code: &str, deadline: &ResolvedTaxDeadline) -> bool {
    !is_holiday(entry) && entry.affected_form_codes == [code] && moves(entry, deadline)
}

/// `{form, period?, original_deadline?, new_deadline, reason?}`: move
/// `form`'s deadline for `period` to `new_deadline`, for every taxpayer.
/// A deadline this form already had moved by the user is moved again (the
/// same entry, its id kept); a holiday or a BIR extension that names it
/// stays stored and this one, saved after it, wins. The new date still
/// moves off a weekend, as every deadline does.
pub fn set(db: &Database, arguments: &Map<String, Value>, today: NaiveDate) -> Result<Value, String> {
    check_keys(
        arguments,
        &["form", "period", "original_deadline", "new_deadline", "reason"],
    )?;
    let raw = text(arguments, "form")?.ok_or("`form` is required: a form code such as 2551Q")?;
    let code = calendar_form(raw)?;
    let new_deadline = date(arguments, "new_deadline")?
        .ok_or("`new_deadline` is required: the new due date, YYYY-MM-DD")?;
    let reason = text(arguments, "reason")?.unwrap_or(DEFAULT_REASON).to_string();
    let deadline = target(&code, arguments, today)?;
    let original = deadline
        .original_deadline_date()
        .ok_or_else(|| format!("{code} has no dated deadline for that period"))?;
    let period = tax_period(&deadline.period).ok_or("not a period of a calendar form")?;
    let mut overrides = stored(db)?;
    let was = resolved(&code, &deadline.period, &overrides).and_then(|d| d.final_deadline_date());
    let replaced = overrides
        .iter()
        .position(|entry| own_override(entry, &code, &deadline))
        .map(|index| overrides.remove(index));
    let entry = DeadlineOverride {
        id: replaced.as_ref().map_or_else(
            || format!("override-{}", chrono::Utc::now().timestamp_millis()),
            |old| old.id.clone(),
        ),
        title: format!("{code} {} deadline", period.label()),
        source_reference: reason,
        affected_form_codes: vec![code.clone()],
        original_deadline: original,
        adjusted_deadline: new_deadline,
        affected_regions: Vec::new(),
        affected_taxpayer_types: Vec::new(),
        effective_from: None,
        effective_until: None,
        expires_at: None,
    };
    // Any other own override of the same deadline goes too (one per deadline).
    overrides.retain(|other| !own_override(other, &code, &deadline));
    overrides.push(entry.clone());
    db.set_deadline_overrides(&overrides)
        .map_err(|error| error.to_string())?;
    let due = resolved(&code, &deadline.period, &overrides).and_then(|d| d.final_deadline_date());
    Ok(json!({
        "form": code,
        "period": period.canonical(),
        "label": period.label(),
        "rule_deadline": iso(original),
        "was": was.map(iso),
        "due": due.map(iso),
        "replaced": replaced.is_some(),
        "override": entry_json(&entry),
        "applies_to": "every taxpayer: the Calendar, Google Calendar and the penalties of every return of this form and period",
    }))
}

/// `{id}` or `{form, period?, original_deadline?}`: put a deadline back.
/// By `id`, that stored override goes (a holiday too). By form and period,
/// every override that moves that deadline, except the holidays, stops
/// moving it: one of this form only goes, one that names other forms too
/// (a BIR extension) drops this form and keeps the others.
pub fn remove(
    db: &Database,
    arguments: &Map<String, Value>,
    today: NaiveDate,
) -> Result<Value, String> {
    check_keys(arguments, &["id", "form", "period", "original_deadline"])?;
    let mut overrides = stored(db)?;
    if let Some(id) = text(arguments, "id")? {
        if arguments.keys().any(|key| key != "id") {
            return Err("pass `id` alone, or `form` with `period` or `original_deadline`".into());
        }
        let index = overrides
            .iter()
            .position(|entry| entry.id == id)
            .ok_or_else(|| format!("no deadline override `{id}`; list them with bir_deadline_overrides_list"))?;
        let entry = overrides.remove(index);
        db.set_deadline_overrides(&overrides)
            .map_err(|error| error.to_string())?;
        return Ok(json!({ "removed": [entry_json(&entry)] }));
    }
    let raw = text(arguments, "form")?
        .ok_or("name the override: `id`, or `form` with `period` or `original_deadline`")?;
    let code = calendar_form(raw)?;
    let deadline = target(&code, arguments, today)?;
    let period = tax_period(&deadline.period).ok_or("not a period of a calendar form")?;
    let mut removed = Vec::new();
    overrides.retain_mut(|entry| {
        if is_holiday(entry) || !moves(entry, &deadline) {
            return true;
        }
        removed.push(entry_json(entry));
        if entry.affected_form_codes.len() > 1 {
            entry.affected_form_codes.retain(|other| other != &code);
            return true;
        }
        false
    });
    if removed.is_empty() {
        return Err(format!(
            "{code} {} is not moved: it is due {} by BIR's calendar",
            period.label(),
            deadline.final_deadline_date().map_or_else(String::new, iso)
        ));
    }
    db.set_deadline_overrides(&overrides)
        .map_err(|error| error.to_string())?;
    let due = resolved(&code, &deadline.period, &overrides).and_then(|d| d.final_deadline_date());
    Ok(json!({
        "form": code,
        "period": period.canonical(),
        "label": period.label(),
        "due": due.map(iso),
        "removed": removed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, month, day).unwrap()
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_moved_deadline_is_one_global_override_set_again_and_removed() {
        let db = Database::open_ephemeral().unwrap();
        let today = day(10, 26);
        // Q3 2026's Oct 25 is a Sunday: due Oct 26 until moved.
        let moved = set(
            &db,
            &args(json!({"form": "2551Q", "period": "2026-Q3", "new_deadline": "2026-11-03", "reason": "RMC 99-2026"})),
            today,
        )
        .unwrap();
        assert_eq!(moved["was"], "2026-10-26");
        assert_eq!(moved["due"], "2026-11-03");
        assert_eq!(moved["rule_deadline"], "2026-10-25");
        let stored = db.get_deadline_overrides();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].affected_form_codes, ["2551Q"]);
        assert_eq!(stored[0].original_deadline, day(10, 25));
        assert_eq!(stored[0].source_reference, "RMC 99-2026");
        let id = stored[0].id.clone();

        // Moved again by its date (the weekend-moved one): the same entry.
        let again = set(
            &db,
            &args(json!({"form": "2551-Q", "original_deadline": "2026-10-26", "new_deadline": "2026-11-07"})),
            today,
        )
        .unwrap();
        assert_eq!(again["replaced"], true);
        assert_eq!(again["due"], "2026-11-09", "Nov 7 is a Saturday");
        let stored = db.get_deadline_overrides();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, id);
        assert_eq!(stored[0].source_reference, DEFAULT_REASON);

        let listed = list(&db, &args(json!({"year": 2026}))).unwrap();
        let entry = &listed["overrides"][0];
        assert_eq!(entry["kind"], "moved");
        assert_eq!(entry["periods"], json!([{"form": "2551Q", "period": "2026-Q3", "label": "Q3 2026"}]));
        assert!(list(&db, &args(json!({"year": 2025}))).unwrap()["overrides"].as_array().unwrap().is_empty());

        let removed = remove(&db, &args(json!({"form": "2551Q", "period": "2026-Q3"})), today).unwrap();
        assert_eq!(removed["due"], "2026-10-26");
        assert!(db.get_deadline_overrides().is_empty());
        assert!(remove(&db, &args(json!({"form": "2551Q", "period": "2026-Q3"})), today).is_err());
    }

    #[test]
    fn a_shared_extension_keeps_its_other_forms_and_holidays_stay() {
        let db = Database::open_ephemeral().unwrap();
        db.seed_official_holidays().unwrap();
        let holidays = db.get_deadline_overrides().len();
        let mut overrides = db.get_deadline_overrides();
        overrides.push(DeadlineOverride {
            id: "override-1".into(),
            title: "Extension".into(),
            source_reference: "RMC".into(),
            affected_form_codes: vec!["1601C".into(), "0619E".into()],
            original_deadline: day(10, 10),
            adjusted_deadline: day(10, 20),
            affected_regions: Vec::new(),
            affected_taxpayer_types: Vec::new(),
            effective_from: None,
            effective_until: None,
            expires_at: None,
        });
        db.set_deadline_overrides(&overrides).unwrap();
        let removed =
            remove(&db, &args(json!({"form": "1601C", "period": "2026-09"})), day(10, 1)).unwrap();
        assert_eq!(removed["due"], "2026-10-12");
        let after = db.get_deadline_overrides();
        assert_eq!(after.len(), holidays + 1, "the holidays and the extension stay");
        assert_eq!(after.last().unwrap().affected_form_codes, ["0619E"]);
        let listed = list(&db, &Map::new()).unwrap();
        assert_eq!(listed["overrides"][0]["kind"], "moved", "moved deadlines first");
    }

    #[test]
    fn what_cannot_be_moved_is_refused() {
        let db = Database::open_ephemeral().unwrap();
        let today = day(10, 26);
        let refused = |value: Value| set(&db, &args(value), today).unwrap_err();
        assert!(refused(json!({"form": "0605", "period": "2026-09", "new_deadline": "2026-11-03"})).contains("as needed"));
        assert!(refused(json!({"form": "9999", "period": "2026-09", "new_deadline": "2026-11-03"})).contains("no deadline"));
        assert!(refused(json!({"form": "1601C", "period": "2026-Q3", "new_deadline": "2026-11-03"})).contains("monthly"));
        assert!(refused(json!({"form": "1701Q", "period": "2026-Q4", "new_deadline": "2027-05-03"})).contains("no 1701Q deadline"));
        assert!(refused(json!({"form": "2551Q", "new_deadline": "2026-11-03"})).contains("name the deadline"));
        assert!(refused(json!({"form": "2551Q", "period": "2026-Q3", "original_deadline": "2026-10-10", "new_deadline": "2026-11-03"})).contains("not 2026-10-10"));
        assert!(refused(json!({"form": "2551Q", "period": "2026-Q3"})).contains("new_deadline"));
        assert!(refused(json!({"form": "2551Q", "period": "2026-Q3", "new_deadline": "11/03/2026"})).contains("must be a date"));
        assert!(refused(json!({"form": "2551Q", "period": "Q3 of 2026x", "new_deadline": "2026-11-03"})).contains("`period`"));
        assert!(db.get_deadline_overrides().is_empty());
    }
}
