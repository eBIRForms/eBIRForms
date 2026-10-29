//! 1700 Schedule 1 employer rows and the spouse's TIN (Item 18). The probe
//! cannot bind these boxes: each TIN box shows only a part of one draft
//! field, and once a fifth employer is added the official page folds the
//! schedule (Item 4 reads OTHERS with the popup subtotal and its employer
//! moves to the "(add more...)" row 4_1), so one box shows different
//! employers. The host writes them through the typed draft instead.
//!
//! Rows: Items 1-3 are employers 1-3; Item 4 is employer 4 until the
//! schedule folds; popup row 4_k is employer k + 3 (4_1 is Item 4's own).
//! Popup rows fill in order, and clearing one removes it, as the popup's
//! Delete does.

use bir_core::forms::agent::{self, AgentFormKind};
use bir_core::forms::form_1700::{
    FORM_1700_EMPLOYER_ROWS, FORM_1700_MAX_EMPLOYERS, Form1700Draft, Form1700Employer,
};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Row {
    /// Items 1-4 on the page.
    Page(usize),
    /// "(add more...)" row 4_k.
    Popup(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Field {
    Taxpayer,
    Spouse,
    Name,
    Name2,
    /// TIN box 0-2 (three digits each) or 3 (branch code).
    Tin(usize),
    Regular,
    Flat,
    Withheld,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Cell {
    Employer(Row, Field),
    SpouseTin(usize),
}

fn row(text: &str) -> Option<Row> {
    if let Some(k) = text.strip_prefix("4_") {
        let k: usize = k.parse().ok()?;
        return (k >= 1 && k + FORM_1700_EMPLOYER_ROWS - 1 <= FORM_1700_MAX_EMPLOYERS)
            .then_some(Row::Popup(k));
    }
    let n: usize = text.parse().ok()?;
    (1..=FORM_1700_EMPLOYER_ROWS)
        .contains(&n)
        .then_some(Row::Page(n))
}

fn tin_part(suffix: &str) -> Option<usize> {
    match suffix {
        "TIN1" => Some(0),
        "TIN2" => Some(1),
        "TIN3" => Some(2),
        "BranchCode" => Some(3),
        _ => None,
    }
}

pub(super) fn cell(kind: AgentFormKind, key: &str) -> Option<Cell> {
    if kind.form_code() != "1700" {
        return None;
    }
    let key = key.strip_prefix("frm1700:")?;
    if let Some(rest) = key.strip_prefix("txtPg1I18S") {
        return tin_part(rest).map(Cell::SpouseTin);
    }
    if let Some(rest) = key.strip_prefix("rdoPg2I") {
        let (r, side) = rest.split_once("PartVIEmployee")?;
        let field = match side {
            "T" => Field::Taxpayer,
            "S" => Field::Spouse,
            _ => return None,
        };
        return Some(Cell::Employer(row(r)?, field));
    }
    if let Some(rest) = key.strip_prefix("txtPg2ISched1") {
        let (field, rest, suffix) = match rest.split_at_checked(2)? {
            ("c_", rest) => (Field::Regular, rest, "REG"),
            ("d_", rest) => (Field::Flat, rest, "CIFR"),
            ("e_", rest) => (Field::Withheld, rest, "TW"),
            _ => return None,
        };
        return Some(Cell::Employer(row(rest.strip_suffix(suffix)?)?, field));
    }
    let rest = key.strip_prefix("txtPg2I")?;
    let (r, box_name) = rest.split_once("PartVIEmployer")?;
    let field = match box_name {
        "Name1" => Field::Name,
        "Name2" => Field::Name2,
        other => Field::Tin(tin_part(other)?),
    };
    Some(Cell::Employer(row(r)?, field))
}

/// A batch fills a TIN's boxes left to right and the rows top to bottom.
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    let field_order = |field: Field| match field {
        Field::Taxpayer | Field::Spouse => 0,
        Field::Name => 1,
        Field::Name2 => 2,
        Field::Tin(part) => 3 + part,
        Field::Regular => 7,
        Field::Flat => 8,
        Field::Withheld => 9,
    };
    Some(match cell(kind, key)? {
        Cell::SpouseTin(part) => 1 + part,
        Cell::Employer(row, field) => 10 + index(row) * 10 + field_order(field),
    })
}

/// The draft's employer index of a row.
fn index(row: Row) -> usize {
    match row {
        Row::Page(n) => n - 1,
        Row::Popup(k) => k + FORM_1700_EMPLOYER_ROWS - 2,
    }
}

fn row_name(row: Row) -> String {
    match row {
        Row::Page(n) => format!("Item {n}"),
        Row::Popup(k) => format!("row 4_{k}"),
    }
}

/// The TIN with one box replaced. The draft keeps one digit string, so the
/// boxes fill in order (three digits, three, three, then the branch code);
/// a whole TIN typed or pasted into the first box fills them all.
pub(super) fn with_tin_part(tin: &str, part: usize, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text
        .chars()
        .any(|c| !c.is_ascii_digit() && c != '-' && c != ' ')
    {
        return Err(format!("`{text}` is not a TIN: use digits only."));
    }
    let typed: String = text.chars().filter(char::is_ascii_digit).collect();
    if part == 0 && typed.len() > 3 {
        if !(9..=14).contains(&typed.len()) {
            return Err("A TIN has 9 digits, then a branch code of up to 5 digits.".to_string());
        }
        return Ok(typed);
    }
    let width = if part == 3 { 5 } else { 3 };
    if typed.len() > width {
        return Err(format!("This box takes at most {width} digits."));
    }
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let slice = |from: usize, to: usize| -> String {
        digits
            .get(from.min(digits.len())..to.min(digits.len()))
            .unwrap_or("")
            .to_string()
    };
    let mut boxes = [
        slice(0, 3),
        slice(3, 6),
        slice(6, 9),
        slice(9, digits.len()),
    ];
    boxes[part] = typed;
    // Every TIN box before the last filled one is complete.
    if let Some(last) = boxes.iter().rposition(|b| !b.is_empty())
        && boxes[..last].iter().any(|b| b.len() != 3)
    {
        return Err(
            "Fill the TIN boxes in order: 3 digits, 3 digits, 3 digits, then the branch code."
                .to_string(),
        );
    }
    Ok(boxes.concat())
}

fn text_of(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.as_str().unwrap_or_default().to_string(),
    }
}

pub(super) fn fill(draft: &mut Value, cell: Cell, value: &Value) -> Result<(), String> {
    let mut typed: Form1700Draft =
        serde_json::from_value(draft.clone()).map_err(|e| e.to_string())?;
    let text = text_of(value);
    match cell {
        Cell::SpouseTin(part) => {
            if !typed.is_joint() && !text.trim().is_empty() {
                return Err(
                    "Item 18 is for a joint return: choose Married, spouse has income \
                     and Joint Filing (Items 15 to 17) first."
                        .to_string(),
                );
            }
            typed.spouse.tin = with_tin_part(&typed.spouse.tin, part, &text)?;
        }
        Cell::Employer(row, field) => fill_employer(&mut typed, row, field, &text)?,
    }
    *draft = serde_json::to_value(typed).map_err(|e| e.to_string())?;
    Ok(())
}

fn fill_employer(
    typed: &mut Form1700Draft,
    row: Row,
    field: Field,
    text: &str,
) -> Result<(), String> {
    if row == Row::Page(FORM_1700_EMPLOYER_ROWS) && typed.schedule_is_folded() {
        return Err(
            "Item 4 reads OTHERS with the (add more...) subtotal; change its employer in row 4_1."
                .to_string(),
        );
    }
    let at = index(row);
    let blank = text.trim().is_empty();
    let filling = match field {
        Field::Taxpayer | Field::Spouse => text == "true",
        Field::Regular | Field::Flat | Field::Withheld => {
            !blank && agent::parse_amount(text) != Some(0.0)
        }
        _ => !blank,
    };
    let len = typed.employers.len();
    if at >= len {
        if !filling {
            return Ok(());
        }
        if matches!(row, Row::Popup(_)) {
            if len + 1 < FORM_1700_EMPLOYER_ROWS {
                return Err("Fill Items 1 to 3 first; row 4_1 is Item 4.".to_string());
            }
            if at > len {
                return Err(format!(
                    "Fill row 4_{} first.",
                    len + 2 - FORM_1700_EMPLOYER_ROWS
                ));
            }
        }
        typed.employers.resize(at + 1, Form1700Employer::default());
    }
    let employer = &mut typed.employers[at];
    match field {
        Field::Taxpayer => {
            if text == "true" {
                employer.for_spouse = false;
            }
        }
        Field::Spouse => match text {
            "true" => employer.for_spouse = true,
            "false" => employer.for_spouse = false,
            _ => {}
        },
        Field::Name => employer.name = text.trim().to_string(),
        Field::Name2 => employer.name2 = text.trim().to_string(),
        Field::Tin(part) => employer.tin = with_tin_part(&employer.tin, part, text)?,
        Field::Regular | Field::Flat | Field::Withheld => {
            let amount = if blank {
                0.0
            } else {
                match agent::parse_amount(text) {
                    Some(amount) if amount >= 0.0 => amount,
                    _ => {
                        return Err(format!(
                            "`{}` is not an amount for {}.",
                            text.trim(),
                            row_name(row)
                        ));
                    }
                }
            };
            match field {
                Field::Regular => employer.regular = amount,
                Field::Flat => employer.flat = amount,
                _ => employer.withheld = amount,
            }
        }
    }
    // A cleared popup row goes, as the popup's Delete does; trailing empty
    // rows never stay.
    if matches!(row, Row::Popup(_)) && typed.employers[at] == Form1700Employer::default() {
        typed.employers.remove(at);
    }
    while typed
        .employers
        .last()
        .is_some_and(|last| *last == Form1700Employer::default())
    {
        typed.employers.pop();
    }
    Ok(())
}
