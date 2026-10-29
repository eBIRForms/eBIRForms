//! Golden: every queueable form but 1800, 1801 and 2200C, plus 1601C,
//! keeps the penalty due dates and amounts it had before PR 13 (e86b1424),
//! for a fixed set of trigger dates and filing dates around the 2026
//! official holidays. `penalty_due_dates_golden.txt` was written by this same module
//! run on the e86b1424 tree; this test recomputes it on the current code.
//!
//! For each form's sample drafts (`super::agent_seeds`), each penalty
//! declaration: the due date and the computed surcharge, interest and
//! compromise if filed on each filing date. A form whose penalties run from
//! a date the filer types (the excise returns, maintainer decision (excise
//! penalties run from the typed date)) is joined to that model
//! and tried with each trigger date written into its `year` / `month` /
//! `day`; a calendar form's trigger is its period, so its filing dates are
//! what vary.
//!
//! Every agent seed must resolve to a form this covers: the one
//! exception is 2551Q, whose penalties come from its own engine call
//! (form_2551q.rs `recompute`), not from the penalty-items model, so it
//! cannot be pinned here. Its due date is the Calendar's, as every other
//! calendar form's (`apply_penalty_profile` → `penalty_items::
//! resolve_due_date` with the stored and profile overrides); a test checks
//! its source for that.

use bir_core::forms::penalty_items::PenaltyItems;
use bir_core::forms::queueable::{QueueableForm, QueueableKind};
use chrono::{Datelike, NaiveDate};
use serde_json::Value;

/// The forms maintainer decision (1800/1801 penalties run from the due date)
/// and round 2 change on purpose.
const CHANGED: [&str; 3] = ["1800", "1801", "2200C"];

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Trigger dates on and around the 2026 seeded holidays (Maundy Thursday,
/// Good Friday, Araw ng Kagitingan, Labor Day, Independence Day, National
/// Heroes Day, Bonifacio Day, Christmas and Rizal Day) and one in 2027.
fn triggers() -> Vec<NaiveDate> {
    vec![
        day(2026, 1, 1),
        day(2026, 4, 2),
        day(2026, 4, 3),
        day(2026, 4, 9),
        day(2026, 5, 1),
        day(2026, 6, 12),
        day(2026, 8, 31),
        day(2026, 11, 30),
        day(2026, 12, 25),
        day(2026, 12, 30),
        day(2027, 4, 9),
    ]
}

/// Filing dates: each trigger, the working days after it, and a late one.
fn filings(trigger: NaiveDate) -> Vec<NaiveDate> {
    [0, 1, 2, 3, 4, 40]
        .into_iter()
        .map(|days| trigger + chrono::Duration::days(days))
        .collect()
}

fn lines_for<F: QueueableForm>(package: &str, index: usize, seed: &Value, out: &mut Vec<String>) {
    lines_for_items::<F>(package, index, seed, &F::penalty_items(), out);
}

fn lines_for_items<F>(
    package: &str,
    index: usize,
    seed: &Value,
    declarations: &[PenaltyItems<F>],
    out: &mut Vec<String>,
) where
    F: Clone + serde::Serialize + serde::de::DeserializeOwned,
{
    let Ok(form) = serde_json::from_value::<F>(seed.clone()) else {
        out.push(format!("{package}#{index} unreadable"));
        return;
    };
    for (n, items) in declarations.iter().enumerate() {
        let typed = items.typed_due_date.is_some();
        let cases: Vec<(String, F)> = if typed {
            triggers()
                .into_iter()
                .filter_map(|trigger| {
                    let mut json = serde_json::to_value(&form).ok()?;
                    for (field, value) in [
                        ("year", i64::from(trigger.year())),
                        ("month", i64::from(trigger.month())),
                        ("day", i64::from(trigger.day())),
                    ] {
                        json.get(field)?;
                        json[field] = Value::from(value);
                    }
                    let mut tried: F = serde_json::from_value(json).ok()?;
                    items.join_typed_due_date(&mut tried);
                    Some((trigger.to_string(), tried))
                })
                .collect()
        } else {
            vec![("period".to_string(), form.clone())]
        };
        for (trigger, case) in cases {
            let due = items.due_date(&case);
            let dates = match due {
                Some(due) => filings(due),
                None => triggers(),
            };
            for filed in dates {
                let amounts = items
                    .if_filed_on(&case, filed)
                    .map(|c| format!("{:.2}/{:.2}/{:.2}", c.surcharge, c.interest, c.compromise))
                    .unwrap_or_else(|| "none".to_string());
                out.push(format!(
                    "{package}#{index} items{n} trigger={trigger} due={} filed={filed} {amounts}",
                    due.map_or("none".to_string(), |d| d.to_string()),
                ));
            }
        }
    }
}

/// Every line of the golden, in a fixed order.
pub(crate) fn golden_lines() -> Vec<String> {
    let mut out = Vec::new();
    for (package, seeds) in super::agent_seeds() {
        if package == "1601c" {
            let items = [bir_core::forms::form_1601c::Form1601CDraft::penalty_items()];
            for (index, seed) in seeds.iter().enumerate() {
                lines_for_items(package, index, seed, &items, &mut out);
            }
            continue;
        }
        if package == "2551q" {
            continue;
        }
        let kind = QueueableKind::ALL
            .iter()
            .copied()
            .find(|kind| kind.form_code().eq_ignore_ascii_case(package))
            .unwrap_or_else(|| panic!("agent seed {package} has no form kind the golden covers"));
        if CHANGED.contains(&kind.form_code()) {
            continue;
        }
        for (index, seed) in seeds.iter().enumerate() {
            bir_core::with_queueable_kind!(kind, F => lines_for::<F>(package, index, seed, &mut out));
        }
    }
    out
}

fn golden_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sample_payloads/penalty_due_dates_golden.txt")
}

/// Writes the golden (run on the e86b1424 tree with `--ignored`); it fails
/// after writing, so a recording never passes as a check.
#[test]
#[ignore = "records the golden; run on the e86b1424 tree"]
fn record_penalty_golden() {
    let lines = golden_lines().join("\n") + "\n";
    std::fs::write(golden_path(), &lines).unwrap();
    panic!(
        "golden written to {}; this run compared nothing",
        golden_path().display()
    );
}

/// 2551Q, the one seed the golden cannot pin, never takes a typed due
/// date. Its penalties run from the deadline the Calendar resolves for the
/// quarter, the stored (global) and profile deadline overrides applied,
/// through the same `penalty_items::resolve_due_date` and
/// `event_trigger::calendar_overrides` every other calendar form's
/// `apply_profile` uses (2026-10-26: Q3 2026's Sunday, Oct 25, charged
/// penalties on returns paid on Monday, Oct 26, the real deadline). A
/// fiscal quarter, which the Calendar has no row for, keeps its own 25th
/// after the quarter, moved off weekends and official holidays with
/// `next_business_day`. form_2551q's `a_weekend_deadline_moves_to_the_next_working_day`
/// and `penalties_run_from_the_calendars_deadline_with_its_overrides` pin
/// the dates. The guard is textual: it catches a typed due date or a
/// dropped Calendar resolution in form_2551q.rs, but not one reached
/// through another module.
#[test]
fn form_2551q_penalties_do_not_read_the_typed_date_calendar() {
    let source = include_str!("../../src/forms/form_2551q.rs");
    assert!(!source.contains("typed_due_date"), "typed_due_date");
    for name in ["penalty_items::resolve_due_date", "event_trigger::calendar_overrides"] {
        assert!(source.contains(name), "{name}");
    }
}

#[test]
fn every_other_form_keeps_its_penalty_due_dates_and_amounts() {
    let lines = golden_lines().join("\n") + "\n";
    let golden = std::fs::read_to_string(golden_path()).unwrap();
    assert!(
        golden.lines().count() > 1_000,
        "the golden covers every form"
    );
    assert_eq!(lines, golden);
}
