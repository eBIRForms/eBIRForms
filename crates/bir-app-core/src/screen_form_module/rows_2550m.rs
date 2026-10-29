//! 2550M Schedules 2, 3A, 3B, 6, 7 and 8. The official page builds their
//! rows in script (`addlistSched2()` … `addlistSched8()`, ids `<stem><row>`
//! from row 0), so the probed agent bindings carry only the `{N>=0}` row
//! families. The host writes each row into the typed draft instead, the way
//! `schedule_row_controls` reads it back.

use bir_core::forms::agent::{self, AgentFormKind};
use bir_core::forms::form_2550m::{
    Form2550MAdvancePaymentRow, Form2550MAmortizedRow, Form2550MCapitalGoodsRow, Form2550MDraft,
    Form2550MWithholdingRow,
};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Schedule {
    S2,
    S3A,
    S3B,
    S6,
    S7,
    S8,
}

/// The boxes the filer types, per schedule, in the official row order (an
/// amount precedes the input tax it fills). Computed cells (allowable input
/// tax, balance) are not listed: the page greys them out.
const SCHEDULES: &[(Schedule, &str, &[&str])] = &[
    (
        Schedule::S2,
        "schedule_2",
        &[
            "txtDatePurchased",
            "txtDescription",
            "txtAmt",
            "txtInputTax",
        ],
    ),
    (
        Schedule::S3A,
        "schedule_3a",
        &[
            "txtDatePurchased3A",
            "txtDescription3A",
            "txtAmt3A",
            "txtInputTax3A",
            "txtEstLife3A",
            "txtRecogLife3A",
        ],
    ),
    (
        Schedule::S3B,
        "schedule_3b",
        &[
            "txtDatePurchased3B",
            "txtDescription3B",
            "txtAmt3B",
            "txtBalInputTaxPrevious3B",
            "txtEstLife3B",
            "txtRecogLife3B",
        ],
    ),
    (
        Schedule::S6,
        "schedule_6",
        &[
            "txtPeriodCovered",
            "txtNameAgent",
            "txtIncomePayment",
            "txtTotalWithheld",
            "txtAppliedCurr",
        ],
    ),
    (
        Schedule::S7,
        "schedule_7",
        &[
            "txtPeriodCoveredSch7",
            "txtNameMillerSch7",
            "txtNameTaxPayerSch7",
            "txtORNumSch7",
            "txtAmountPaidSch7",
            "txtAppliedCurrSch7",
        ],
    ),
    (
        Schedule::S8,
        "schedule_8",
        &[
            "txtPeriodCoveredSch8",
            "txtNameAgentSch8",
            "txtIncomePaymentSch8",
            "txtTotalWithheldSch8",
            "txtAppliedCurrSch8",
        ],
    ),
];

/// A schedule row box: `(schedule, column, row)`. The remainder after the
/// stem must be digits only, so `txtAmt3A0` is never Schedule 2's `txtAmt`.
pub(super) fn cell(kind: AgentFormKind, key: &str) -> Option<(Schedule, usize, usize)> {
    if kind.form_code() != "2550M" {
        return None;
    }
    SCHEDULES.iter().find_map(|(schedule, _, stems)| {
        stems.iter().enumerate().find_map(|(col, stem)| {
            let rest = key.strip_prefix(stem)?;
            if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((*schedule, col, rest.parse().ok()?))
        })
    })
}

/// Earlier rows first within a batch, and a row's amount before its input
/// tax (the amount resets the input tax to 12%, as `getInputTaxCompute` does).
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    cell(kind, key).map(|(_, col, row)| 2 + row.saturating_mul(8).saturating_add(col))
}

/// The row boxes a validation message about `schedule_2[i]` (one row) or
/// `schedule_2` (the whole schedule, shown at its first row) belongs to.
pub(super) fn error_keys(kind: AgentFormKind, field: &str) -> Vec<String> {
    if kind.form_code() != "2550M" {
        return Vec::new();
    }
    let (name, row) = match field.split_once('[') {
        Some((name, rest)) => match rest.strip_suffix(']').and_then(|r| r.parse().ok()) {
            Some(row) => (name, row),
            None => return Vec::new(),
        },
        None => (field, 0usize),
    };
    SCHEDULES
        .iter()
        .find(|(_, n, _)| *n == name)
        .map(|(_, _, stems)| stems.iter().map(|s| format!("{s}{row}")).collect())
        .unwrap_or_default()
}

fn text_of(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        other => other.as_str().unwrap_or_default().to_string(),
    }
}

/// The page's `numbersonly` keypress filter: no sign, no letters.
fn amount(text: &str) -> Result<f64, String> {
    if text.trim().is_empty() {
        return Ok(0.0);
    }
    match agent::parse_amount(text) {
        Some(value) if value >= 0.0 => Ok(value),
        _ => Err(format!("`{}` is not an amount.", text.trim())),
    }
}

/// A text box with the official `maxlength`.
fn limited(text: &str, max: usize) -> Result<String, String> {
    let text = text.trim();
    if text.chars().count() > max {
        return Err(format!("This box holds at most {max} characters."));
    }
    Ok(text.to_string())
}

/// Estimated life: `maxlength='3'`, digits only; blank is the row's 0.
fn estimated_life(text: &str) -> Result<u16, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    text.parse::<u16>()
        .ok()
        .filter(|n| *n <= 999)
        .ok_or_else(|| "Enter a value 1 to 999 for Estimated Life.".to_string())
}

/// Recognized life: 1 to 60 months, since allowable input tax divides by
/// it. Blank is the row's starting 1 (`Schedule3()`).
fn recognized_life(text: &str) -> Result<u16, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(1);
    }
    text.parse::<u16>()
        .ok()
        .filter(|n| (1..=60).contains(n))
        .ok_or_else(|| "Enter a value 1 to 60 for Recognized Life.".to_string())
}

/// The row being written: the next row starts one (`addlistSched…`), a
/// row past it would leave a gap the page cannot have.
fn row_mut<T: Default>(rows: &mut Vec<T>, row: usize) -> Result<&mut T, String> {
    if row == rows.len() {
        rows.push(T::default());
    }
    let next = rows.len();
    rows.get_mut(row)
        .ok_or_else(|| format!("Fill row {next} of this schedule first."))
}

/// Drop the row when every box is blank again, as ticking it and pressing
/// Delete does on the page; later rows move up.
fn prune<T: Default + PartialEq>(rows: &mut Vec<T>, row: usize) {
    if rows.get(row).is_some_and(|r| *r == T::default()) {
        rows.remove(row);
    }
}

pub(super) fn fill(
    draft: &mut Value,
    (schedule, col, row): (Schedule, usize, usize),
    value: &Value,
) -> Result<(), String> {
    let text = text_of(value);
    let mut typed: Form2550MDraft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    match schedule {
        Schedule::S2 => {
            let entry: &mut Form2550MCapitalGoodsRow = row_mut(&mut typed.schedule_2, row)?;
            match col {
                0 => entry.date_purchased = limited(&text, 10)?,
                1 => entry.description = limited(&text, 50)?,
                2 => {
                    let value = amount(&text)?;
                    typed.set_schedule_2_amount(row, value);
                }
                _ => entry.input_tax = amount(&text)?,
            }
            typed.recompute();
            prune(&mut typed.schedule_2, row);
        }
        Schedule::S3A | Schedule::S3B => {
            let rows = if schedule == Schedule::S3A {
                &mut typed.schedule_3a
            } else {
                &mut typed.schedule_3b
            };
            let entry: &mut Form2550MAmortizedRow = row_mut(rows, row)?;
            match col {
                0 => entry.date_purchased = limited(&text, 10)?,
                1 => entry.description = limited(&text, 50)?,
                2 if schedule == Schedule::S3A => {
                    let value = amount(&text)?;
                    typed.set_schedule_3a_amount(row, value);
                }
                2 => entry.amount = amount(&text)?,
                3 => entry.input_tax = amount(&text)?,
                4 => entry.estimated_life = estimated_life(&text)?,
                _ => entry.recognized_life = recognized_life(&text)?,
            }
            typed.recompute();
            let rows = if schedule == Schedule::S3A {
                &mut typed.schedule_3a
            } else {
                &mut typed.schedule_3b
            };
            prune(rows, row);
        }
        Schedule::S6 | Schedule::S8 => {
            // The page enables Schedule 8 only once Item 13A has a value.
            if schedule == Schedule::S8
                && row >= typed.schedule_8.len()
                && typed.sales_to_government <= 0.0
                && !text.trim().is_empty()
            {
                return Err(
                    "Enter Item 13A (Sales to Government) to be able to load Schedule 8."
                        .to_string(),
                );
            }
            let rows = if schedule == Schedule::S6 {
                &mut typed.schedule_6
            } else {
                &mut typed.schedule_8
            };
            let entry: &mut Form2550MWithholdingRow = row_mut(rows, row)?;
            match col {
                0 => entry.period_covered = limited(&text, 10)?,
                1 => entry.withholding_agent = limited(&text, 50)?,
                2 => entry.income_payment = amount(&text)?,
                3 => entry.total_withheld = amount(&text)?,
                _ => entry.applied_current_month = amount(&text)?,
            }
            typed.recompute();
            let rows = if schedule == Schedule::S6 {
                &mut typed.schedule_6
            } else {
                &mut typed.schedule_8
            };
            prune(rows, row);
        }
        Schedule::S7 => {
            let entry: &mut Form2550MAdvancePaymentRow = row_mut(&mut typed.schedule_7, row)?;
            match col {
                0 => entry.period_covered = limited(&text, 10)?,
                1 => entry.miller = limited(&text, 50)?,
                2 => entry.taxpayer_name = limited(&text, 17)?,
                3 => entry.or_number = limited(&text, 20)?,
                4 => entry.amount_paid = amount(&text)?,
                _ => entry.applied_current_month = amount(&text)?,
            }
            typed.recompute();
            prune(&mut typed.schedule_7, row);
        }
    }
    typed.recompute();
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}
