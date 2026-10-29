//! 2550Q Schedules 1, 3 and 4 rows. The official upload writes a schedule as
//! its modal rows (`addlistSched1/3/4`) with empty rows left out, so a row
//! the filer leaves empty ahead of a filled one would move that row up on
//! the way out. The host keeps the rows gap-free instead: the next row
//! starts on its first box, a row past it is refused, and clearing a row
//! removes it, as the modal's Delete does. A schedule keeps the two rows the
//! form opens with.

use bir_core::forms::agent::{self, AgentFormKind};
use bir_core::forms::form_2550q::Form2550QDate;
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Date,
    Text,
    Money,
    Whole,
}

/// `(key stem, draft list, field, kind)` in row order; a longer stem first
/// where one starts another (`txtDateCovered3To`, `txtDateCovered3`).
const CELLS: &[(&str, &str, &str, Kind)] = &[
    (
        "txtDatePurchase1",
        "/schedule_1",
        "purchase_or_import_date",
        Kind::Date,
    ),
    ("txtSourceCode1", "/schedule_1", "source_code", Kind::Text),
    ("txtDescription1", "/schedule_1", "description", Kind::Text),
    (
        "txtAmountPurchase1",
        "/schedule_1",
        "purchase_or_import_amount",
        Kind::Money,
    ),
    ("txtInputTax1", "/schedule_1", "input_tax", Kind::Money),
    (
        "txtEstimatedLife1",
        "/schedule_1",
        "estimated_life_months",
        Kind::Whole,
    ),
    (
        "txtRecognizedLife1",
        "/schedule_1",
        "recognized_life_months",
        Kind::Whole,
    ),
    ("txtDateCovered3To", "/schedule_3", "period_to", Kind::Date),
    ("txtDateCovered3", "/schedule_3", "period_from", Kind::Date),
    (
        "txtNameWithHoldingAgent3",
        "/schedule_3",
        "withholding_agent_name",
        Kind::Text,
    ),
    (
        "txtIncomePayment3",
        "/schedule_3",
        "income_payment",
        Kind::Money,
    ),
    (
        "txtTotalTaxWithHeld3",
        "/schedule_3",
        "tax_withheld",
        Kind::Money,
    ),
    ("txtDate4To", "/schedule_4", "period_to", Kind::Date),
    ("txtDate4", "/schedule_4", "period_from", Kind::Date),
    ("txtNameOfMiller4", "/schedule_4", "miller_name", Kind::Text),
    (
        "txtNameOfTaxpayer4",
        "/schedule_4",
        "taxpayer_name",
        Kind::Text,
    ),
    (
        "txtOfficialReceiptNumber4",
        "/schedule_4",
        "official_receipt_number",
        Kind::Text,
    ),
    ("txtAmountPaid4", "/schedule_4", "amount_paid", Kind::Money),
];

/// Rows a schedule keeps (`new_from_profile` opens each with two).
const MIN_ROWS: usize = 2;

/// `(index into CELLS, row)` of a 2550Q schedule box.
pub(super) fn cell(kind: AgentFormKind, key: &str) -> Option<(usize, usize)> {
    if kind.form_code() != "2550Q" {
        return None;
    }
    CELLS.iter().enumerate().find_map(|(at, (stem, ..))| {
        let rest = key.strip_prefix(stem)?;
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((at, rest.parse().ok()?))
    })
}

/// Earlier rows first in a batch, then the row's own order.
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    cell(kind, key).map(|(at, row)| 2 + row.saturating_mul(CELLS.len()).saturating_add(at))
}

fn blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => matches!(text.trim(), "" | "0.00"),
        Value::Number(number) => number.as_f64() == Some(0.0),
        Value::Bool(flag) => !flag,
        Value::Array(items) => items.is_empty(),
        Value::Object(_) => false,
    }
}

/// Every box of the row is empty (its stable row ID aside).
fn empty(row: &Value) -> bool {
    row.as_object().is_none_or(|fields| {
        fields
            .iter()
            .filter(|(name, _)| name.as_str() != "instance_id")
            .all(|(_, value)| blank(value))
    })
}

fn parse(kind: Kind, text: &str) -> Result<Value, String> {
    let text = text.trim();
    Ok(match kind {
        Kind::Date if text.is_empty() => Value::Null,
        Kind::Date => serde_json::to_value(Form2550QDate::parse_return_period(text)?)
            .map_err(|e| e.to_string())?,
        Kind::Text => Value::from(text),
        Kind::Money if text.is_empty() => Value::from(0.0),
        Kind::Money => match agent::parse_amount(text) {
            Some(amount) if amount >= 0.0 => Value::from(amount),
            _ => return Err(format!("`{text}` is not an amount.")),
        },
        Kind::Whole if text.is_empty() => Value::from(0),
        Kind::Whole => Value::from(
            text.parse::<u16>()
                .map_err(|_| format!("`{text}` is not a number of months."))?,
        ),
    })
}

pub(super) fn fill(
    kind: AgentFormKind,
    draft: &mut Value,
    (at, row): (usize, usize),
    value: &Value,
) -> Result<(), String> {
    let (_, list, field, cell_kind) = CELLS[at];
    let text = match value {
        Value::Number(n) => n.to_string(),
        other => other.as_str().unwrap_or_default().to_string(),
    };
    let parsed = parse(cell_kind, &text)?;
    let template = kind
        .definition()
        .rows
        .get(list)
        .cloned()
        .ok_or_else(|| format!("rows of `{list}` cannot be added here"))?;
    let rows = draft
        .pointer_mut(list)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("draft has no `{list}`"))?;
    let filling = !blank(&parsed);
    // The first row nothing is typed in yet: rows fill in order.
    let open = rows.iter().position(empty).unwrap_or(rows.len());
    let starts_row = rows.get(row).is_none_or(empty);
    if filling && starts_row && row > open {
        return Err(format!("Fill row {} of this schedule first.", open + 1));
    }
    if row >= rows.len() {
        if !filling {
            return Ok(());
        }
        rows.resize(row + 1, template.clone());
    }
    rows[row][field] = parsed;
    if empty(&rows[row]) {
        rows.remove(row);
        while rows.len() < MIN_ROWS {
            rows.push(template.clone());
        }
    }
    Ok(())
}
