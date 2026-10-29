//! PR 6: the annual information returns (1604C, 1604F, 1604E). An answer
//! that closes a section clears what the filer typed there (the official
//! handlers, kept by each form's `recompute`): 1604C Item 11 No clears
//! Items 11A to 13 (`changeRefund`), 1604F Item 11 No clears Item 11A
//! (`TaxReliefEnable`). The page asks first and offers Undo. A Part II
//! Date of Remittance the filer clears is cleared.

use bir_core::forms::agent::{AgentFormKind, Binding, DateFormat};
use serde_json::{Map, Value, json};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn text(draft: &Value, field: &str) -> String {
    draft[field].as_str().unwrap_or("").trim().to_string()
}

fn amount(draft: &Value, field: &str) -> f64 {
    draft[field].as_f64().unwrap_or(0.0)
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

/// When `edits` turned `before` into `after` by clearing what the filer
/// typed, what the page asks first (`message`), what the Undo toast says
/// (`done`: the items actually cleared) and how many rows went (none: these
/// are items); `None` when nothing typed went. The host returns the edited
/// draft; the page holds it, still showing the old one, and takes it only
/// when the filer chooses Continue (Cancel and Esc drop it).
pub(super) fn removes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    let edited = |prefix: &str| edits.keys().any(|key| key.starts_with(prefix));
    let closed = |flag: &str| before[flag] == true && after[flag] == false;
    match kind.form_code() {
        "1604C" if edited("frm1604c:releasedOfFunds") && closed("refunds_released") => {
            let (mut lost, mut items) = (Vec::new(), Vec::new());
            let date = text(before, "refund_date");
            if !date.is_empty() && text(after, "refund_date").is_empty() {
                lost.push(format!("item 11A (date of refund {date})"));
                items.push("11A".to_string());
            }
            let over = amount(before, "overremittance");
            if over != 0.0 && amount(after, "overremittance") == 0.0 {
                lost.push(format!("item 12 (₱{})", pesos(over)));
                items.push("12".to_string());
            }
            let month = before["first_crediting_month"].as_u64().unwrap_or(0);
            if (1..=12).contains(&month)
                && after["first_crediting_month"].as_u64().unwrap_or(0) == 0
            {
                lost.push(format!("item 13 ({})", MONTHS[month as usize - 1]));
                items.push("13".to_string());
            }
            if lost.is_empty() {
                return None;
            }
            let done = match items.as_slice() {
                [one] => format!("Cleared item {one}"),
                _ => format!("Cleared items {}", join(&items)),
            };
            Some(json!({
                "message": format!("Changing item 11 to No clears {}. Continue?", join(&lost)),
                "done": done,
                "rows": 0,
            }))
        }
        "1604F" if edited("frm1604f:taxRelief") && closed("tax_relief") => {
            let relief = text(before, "tax_relief_details");
            if relief.is_empty() || !text(after, "tax_relief_details").is_empty() {
                return None;
            }
            Some(json!({
                "message": format!("Changing item 11 to No clears item 11A ({relief}). Continue?"),
                "done": "Cleared item 11A",
                "rows": 0,
            }))
        }
        _ => None,
    }
}

/// The draft path a cleared Part II Date of Remittance empties: the key is
/// one of the 1604C/E/F schedule date boxes (a whole MM/DD/YYYY date) and
/// the page sent blank. The binding writes only a whole date, so without
/// this a filer could not empty a date they typed; other text is still
/// judged by the binding (MM/DD/YYYY or refused), as on every date box.
pub(super) fn cleared_date(kind: AgentFormKind, key: &str, value: &Value) -> Option<String> {
    if !matches!(kind.form_code(), "1604C" | "1604E" | "1604F") || !is_part_ii_date(key) {
        return None;
    }
    let blank = value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty());
    if !blank {
        return None;
    }
    match kind
        .definition()
        .bindings
        .get(key)
        .map(|bound| &bound.binding)
    {
        Some(Binding::Text {
            path,
            date: Some(DateFormat::Mdy),
            part: None,
            ..
        }) => Some(path.clone()),
        _ => None,
    }
}

/// A Part II Date of Remittance box of 1604C, 1604E or 1604F.
pub(super) fn is_part_ii_date(key: &str) -> bool {
    [
        "frm1604c:txtSched1Date",
        "frm1604e:txtSched1RemDate",
        "frm1604e:txtSched2RemDate",
        "frm1604f:txtSched1Date",
        "frm1604f:txtSched2Date",
        "frm1604f:txtSched3Date",
    ]
    .iter()
    .any(|stem| {
        key.strip_prefix(stem)
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    })
}

/// `a`, `a and b`, `a, b and c`.
fn join(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// 1604E Part II rows whose Total Amount Remitted box was bound to the
/// row's penalties before PR 6: `(schedule, 0-based rows)`. The pin
/// before a7b9e47e (crates/bir-core/data/agent-forms/1604e-v2018.json)
/// bound txtSched1TotRemAmt3 to /schedule1/2/penalties and
/// txtSched2TotRemAmt2-5 and 7-11 to /schedule2/{1-4,6-10}/penalties
/// (PR 6's ledger, 1604E manual-0: "a legacy 1604E draft whose penalties
/// were typed through a Total cell ... still files those penalties"); the
/// other rows' totals were never bound.
pub(super) const LEGACY_TOTAL_ROWS: [(u8, &[usize]); 2] =
    [(1, &[2]), (2, &[1, 2, 3, 4, 6, 7, 8, 9, 10])];

/// What a 1604E row that may hold a total typed as its penalties says
/// (maintainer decision D5, 2026-10-18), naming its schedule (P4-11,
/// 2026-10-19).
pub(super) fn legacy_total_notice(schedule: u8, row: usize) -> String {
    format!(
        "Schedule {schedule}, row {row} may hold an amount typed as the total; check it before you file."
    )
}

/// What a D5 notice adds on a draft not saved yet, which cannot be marked
/// checked until it is.
pub(super) const SAVE_TO_CHECK: &str = "Save the return to mark it checked.";

/// When PR 6 (a7b9e47e, which unbound the Total boxes) reached integration:
/// the merge a2beb40b, 2026-10-18T06:58:15+08:00. A 1604E draft created
/// after it was never typed through a bound Total box.
pub(super) const PR6_REACHED_INTEGRATION: &str = "2026-10-17T22:58:15+00:00";

/// Whether `draft` was created before [`PR6_REACHED_INTEGRATION`]. The
/// stored draft carries no trace of the box an amount was typed in (the
/// old binding wrote the very path the Penalties box writes, and the form
/// recomputes the Total from it either way), so its creation stamp is the
/// only sound discriminator: a draft without a readable `created_at` counts
/// as created before. Residual: a build from before the merge, still used
/// after the cut-off, stamps its new drafts after it while still binding
/// the Totals, and such a draft is not flagged.
fn created_before_pr6(draft: &Value) -> bool {
    let Ok(cutoff) = chrono::DateTime::parse_from_rfc3339(PR6_REACHED_INTEGRATION) else {
        return true;
    };
    draft["created_at"]
        .as_str()
        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
        .is_none_or(|created| created < cutoff)
}

/// The settings key that records the filer marked a 1604E row's D5 notice
/// checked, for the stored return `id`. App state, never the return: the
/// return's bytes do not change. The mark is permanent for that return and
/// row: it stays even if the row's penalties change later.
pub(super) fn checked_key(id: i64, token: &str) -> String {
    format!("notice_checked:1604e_total_row:{id}:{token}")
}

/// A row's notice token: `"<schedule>:<0-based row>"`.
fn row_token(schedule: u8, index: usize) -> String {
    format!("{schedule}:{index}")
}

/// The tokens of the rows of stored return `draft` whose notice the filer
/// marked checked (read on the host's own thread, before an evaluation).
pub(super) fn checked_rows(
    db: &bir_core::db::Database,
    kind: AgentFormKind,
    draft: &Value,
) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let Some(id) = draft["id"].as_i64().filter(|_| kind.form_code() == "1604E") else {
        return out;
    };
    for (schedule, rows) in LEGACY_TOTAL_ROWS {
        for &index in rows {
            let token = row_token(schedule, index);
            if db
                .get_setting(&checked_key(id, &token))
                .ok()
                .flatten()
                .is_some()
            {
                out.insert(token);
            }
        }
    }
    out
}

/// Notices beside the form's own messages; nothing is changed and the
/// stored bytes stay (D5). 1604E: an amount typed into one of the
/// [`LEGACY_TOTAL_ROWS`]' Total Amount Remitted boxes was saved as the
/// row's penalties, so it files as penalties and the form adds the tax
/// withheld to it. A stored draft cannot tell which box it was typed in;
/// the binding wrote nothing else, so a row whose penalties are zero holds
/// none. Each of those rows holding penalties says so under its Penalties
/// and Total boxes, while the return is a Draft created before PR 6
/// reached integration and the filer has not marked the row checked
/// (`checked`, [`checked_rows`]); the notice carries the token the page's
/// "Checked" sends back (`screen_form_check_notice`). A draft not saved
/// yet (an amended copy in memory) has no return to record the mark on:
/// its notice says to save first ([`SAVE_TO_CHECK`]), and offers Checked
/// once it is saved.
pub(super) fn notices(
    kind: AgentFormKind,
    draft: &Value,
    checked: &std::collections::BTreeSet<String>,
) -> Vec<Value> {
    if kind.form_code() != "1604E"
        || !super::is_draft_return(kind, draft)
        || !created_before_pr6(draft)
    {
        return Vec::new();
    }
    let dismissible = draft["id"].as_i64().is_some();
    let mut out = Vec::new();
    for (schedule, rows) in LEGACY_TOTAL_ROWS {
        for &index in rows {
            let penalties = &draft[format!("schedule{schedule}")][index]["penalties"];
            let token = row_token(schedule, index);
            if penalties.as_f64().is_none_or(|amount| amount == 0.0) || checked.contains(&token) {
                continue;
            }
            let row = index + 1;
            let message = if dismissible {
                legacy_total_notice(schedule, row)
            } else {
                format!("{} {SAVE_TO_CHECK}", legacy_total_notice(schedule, row))
            };
            let mut notice = json!({
                "field": format!("schedule{schedule}[{index}].penalties"),
                "message": message,
                "keys": [
                    format!("frm1604e:txtSched{schedule}Penalties{row}"),
                    format!("frm1604e:txtSched{schedule}TotRemAmt{row}"),
                ],
            });
            if dismissible {
                notice["dismiss"] = json!(token);
                notice["dismiss_label"] = json!(format!("Checked: Schedule {schedule} row {row}"));
            }
            out.push(notice);
        }
    }
    out
}

/// Record that the filer checked the row `token` of stored 1604E return
/// `draft`: only a row whose notice `draft` shows now. Nothing on the
/// return changes.
pub(super) fn check_notice(
    db: &bir_core::db::Database,
    kind: AgentFormKind,
    draft: &Value,
    token: &str,
) -> Result<(), String> {
    let id = draft["id"]
        .as_i64()
        .ok_or("save the return before marking a row checked")?;
    let shown = notices(kind, draft, &checked_rows(db, kind, draft))
        .iter()
        .any(|notice| notice["dismiss"] == token);
    if !shown {
        return Err(format!(
            "no notice `{token}` to mark checked on this return"
        ));
    }
    db.set_setting(&checked_key(id, token), "1")
        .map_err(|error| error.to_string())
}

/// Messages that name another item number than the one the printed sheet
/// gives the box they are about, reworded to the printed number
/// (maintainer decision D6, 2026-10-18; wording only, as PR 8 does for its
/// forms in `rows_annual_corporate::printed_item_numbers`).
/// `(form, field, official words, printed words)`. The printed numbers are
/// the frozen sheets' captions (html-frozen/<slug>/writer-cells.json):
/// - 1604F: the withholding agent's name is item 6 ("6 Withholding Agent's
///   Name"), the registered address item 7, its ZIP Code 7A ("7 Registered
///   Address (second line, ...) and 7A ZIP Code") and the contact number
///   item 9; the official alerts number them 7, 9, 10 and 8.
/// - 1604C: the ZIP Code is 7A ("7A ZIP Code"); the official alert says
///   item 10, the email address. Its other messages name their printed
///   item.
const PRINTED_ITEM_NUMBERS: [(&str, &str, &str, &str); 5] = [
    ("1604F", "taxpayer_name", "on Item 7.", "on Item 6."),
    ("1604F", "registered_address", "on Item 9.", "on Item 7."),
    ("1604F", "zip_code", "on Item 10.", "on Item 7A."),
    ("1604F", "contact_number", "on Item 8.", "on Item 9."),
    ("1604C", "zip_code", "on Item 10.", "on Item 7A."),
];

/// Each message the form's validation gives, with an item number the
/// printed sheet does not give that box reworded to the printed one.
pub(super) fn printed_item_numbers(kind: AgentFormKind, errors: &mut [(String, String)]) {
    let code = kind.form_code();
    for (field, message) in errors.iter_mut() {
        if let Some((_, _, official, printed)) =
            PRINTED_ITEM_NUMBERS
                .iter()
                .find(|(form, name, official, _)| {
                    *form == code && field == name && message.ends_with(official)
                })
        {
            *message = format!("{}{printed}", &message[..message.len() - official.len()]);
        }
    }
}
