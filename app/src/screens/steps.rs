//! What the form shows and what comes next, shared by the desktop and the
//! phone renderers (D22):
//!
//! - Parts and items that do not apply are hidden: a part whose boxes to
//!   fill are all switched off by another answer (Schedule I or II of the
//!   1701Q, picked by item 16), an item whose boxes are all off, the
//!   penalty boxes (worked out, shown in the summary), and the boxes the
//!   app writes itself from the period and the tax profile (shown in the
//!   title and the filer card).
//! - The next step: a message to fix, else a required box left empty, else
//!   queue for sending ([`next_step`]).
//!
//! Every read here is tracked: views call these inside closures.

use bir_app_core::LayoutItem;

use crate::session::{FormSession, ItemId, OpenForm, Placed};

/// A part's mono tag and its name, as a section header shows them:
/// `Part V · Schedule I – For Graduated IT Rate` is tag `Part V · Schedule
/// I`, name `For graduated IT rate`.
pub(crate) fn section_title(name: &str, index: usize) -> (String, String) {
    if index == 0 && name.starts_with("Page ") {
        return (name.to_owned(), "Return details".to_owned());
    }
    let (tag, rest) = if let Some((tag, rest)) = name.split_once(" – ") {
        (tag.to_owned(), rest.to_owned())
    } else if let Some((tag, rest)) = name.rsplit_once(" · ") {
        (tag.to_owned(), rest.to_owned())
    } else {
        (String::new(), name.to_owned())
    };
    (tag, sentence_case(&rest))
}

/// `BACKGROUND INFORMATION ON SPOUSE (if applicable)` reads `Background
/// information on spouse (if applicable)`; mixed case stays as it is.
pub(crate) fn sentence_case(text: &str) -> String {
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let upper = text.chars().filter(|c| c.is_uppercase()).count();
    if letters == 0 || upper * 10 < letters * 7 {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut first = true;
    for word in text.split(' ') {
        if !out.is_empty() {
            out.push(' ');
        }
        // Acronyms keep their case.
        let bare: String = word.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let keep = ["TIN", "RDO", "ATC", "IT", "BIR", "VAT", "OSD", "GPP", "ID", "IAF", "CWT"].contains(&bare.as_str());
        if first {
            let mut chars = word.chars();
            if let Some(c) = chars.next() {
                out.extend(c.to_uppercase());
                if keep { out.push_str(chars.as_str()) } else { out.push_str(&chars.as_str().to_lowercase()) }
            }
            first = word.is_empty();
        } else if keep {
            out.push_str(word);
        } else {
            out.push_str(&word.to_lowercase());
        }
    }
    out
}

/// An item's label as the filer reads it.
pub(crate) fn item_title(item: &LayoutItem) -> String {
    sentence_case(&item.label)
}

/// Every box of `item` with whether it is computed.
fn boxes(item: &LayoutItem) -> Vec<(String, bool)> {
    if item.is_schedule() {
        return item
            .keys
            .iter()
            .flat_map(|row| row.iter().enumerate().map(|(ci, key)| (key.clone(), item.cols.get(ci).is_some_and(|c| c.computed))))
            .collect();
    }
    item.fields.iter().map(|f| (f.key.clone(), f.computed)).collect()
}

/// A part whose boxes to fill are all switched off by another answer.
pub(crate) fn part_hidden(st: &FormSession, form: &OpenForm, part: usize) -> bool {
    let Some(p) = form.layout.parts.get(part) else { return true };
    let editable: Vec<String> = p.items.iter().flat_map(boxes).filter(|(_, computed)| !computed).map(|(k, _)| k).collect();
    !editable.is_empty() && st.off.with(|off| editable.iter().all(|k| off.contains_key(k)))
}

/// Why a hidden part is hidden: the reason its first box to fill gives
/// (the filer's own column; a spouse's column gives its own reason).
pub(crate) fn hidden_reason(st: &FormSession, form: &OpenForm, part: usize) -> Option<String> {
    let p = form.layout.parts.get(part)?;
    st.off.with(|off| p.items.iter().flat_map(boxes).filter(|(_, c)| !c).find_map(|(key, _)| off.get(&key).cloned()))
}

/// Locks that mean "the app writes this from the period or the tax
/// profile": the title and the filer card say it instead.
fn written_by_app(why: &str) -> bool {
    why.starts_with("picks which return") || why.starts_with("cannot be filled here") || why.starts_with("comes from the taxpayer")
}

/// Whether `item` is left out of the form (tracked).
pub(crate) fn item_hidden(st: &FormSession, form: &OpenForm, id: ItemId) -> bool {
    if part_hidden(st, form, id.0) {
        return true;
    }
    let Some(item) = form.item(id) else { return true };
    let keys = item.box_keys();
    if keys.is_empty() {
        return false;
    }
    if st.off.with(|off| keys.iter().all(|k| off.contains_key(k))) {
        return true;
    }
    if st.penalties.with(|parties| keys.iter().all(|k| parties.iter().any(|p| p.keys.contains(k)))) {
        return true;
    }
    !item.is_schedule()
        && st.locks.with(|locks| {
            item.fields.iter().all(|f| {
                !f.computed && (st.profile.with(|p| p.contains(&f.key)) || locks.get(&f.key).is_some_and(|why| written_by_app(why)))
            })
        })
}

/// Whether a computed column (`B) Spouse`) applies: some box of the part
/// with that label is drawn, or the part has none to fill under it.
pub(crate) fn column_shown(st: &FormSession, form: &OpenForm, part: usize, label: &str) -> bool {
    let Some(p) = form.layout.parts.get(part) else { return true };
    let same: Vec<&str> = p.items.iter().flat_map(|i| i.fields.iter()).filter(|f| !f.computed && f.label == label).map(|f| f.key.as_str()).collect();
    same.is_empty() || same.iter().any(|k| field_shown(st, k))
}

/// Whether box `key` is drawn (not switched off).
pub(crate) fn field_shown(st: &FormSession, key: &str) -> bool {
    !st.off.with(|off| off.contains_key(key))
}

/// A required box of `item` the filer can fill is still empty (tracked).
pub(crate) fn item_left(st: &FormSession, item: &LayoutItem) -> bool {
    if item.is_schedule() {
        return false;
    }
    let open: Vec<_> = item
        .fields
        .iter()
        .filter(|f| f.required && f.kind != "chk" && field_shown(st, &f.key) && !st.is_locked(&f.key, f.computed))
        .collect();
    if open.is_empty() {
        return false;
    }
    let radios: Vec<_> = open.iter().filter(|f| f.kind == "radio").collect();
    if !radios.is_empty() {
        let any = item.fields.iter().filter(|f| f.kind == "radio").any(|f| st.is_ticked(&f.key));
        if !any {
            return true;
        }
    }
    open.iter().filter(|f| f.kind != "radio").any(|f| {
        let v = st.value(&f.key);
        let v = v.trim();
        v.is_empty() || (f.kind == "sel" && v.chars().all(|c| c == '0' || c == '-'))
    })
}

/// The items shown, in form order.
pub(crate) fn shown_items(st: &FormSession, form: &OpenForm) -> Vec<ItemId> {
    let mut out = Vec::new();
    for (pi, part) in form.layout.parts.iter().enumerate() {
        if part_hidden(st, form, pi) {
            continue;
        }
        for ii in 0..part.items.len() {
            if !item_hidden(st, form, (pi, ii)) {
                out.push((pi, ii));
            }
        }
    }
    out
}

/// The items with a required box left empty, in form order.
pub(crate) fn left_items(st: &FormSession, form: &OpenForm) -> Vec<ItemId> {
    shown_items(st, form).into_iter().filter(|id| form.item(*id).is_some_and(|item| item_left(st, item))).collect()
}

/// The items that ask for a box to be filled at all (the progress line's
/// whole): those with a required box the filer can fill.
fn required_items(st: &FormSession, form: &OpenForm) -> Vec<ItemId> {
    shown_items(st, form)
        .into_iter()
        .filter(|id| {
            form.item(*id).is_some_and(|item| {
                !item.is_schedule()
                    && item.fields.iter().any(|f| f.required && f.kind != "chk" && field_shown(st, &f.key) && !st.is_locked(&f.key, f.computed))
            })
        })
        .collect()
}

/// Whether `item`'s messages show on its row: the filer left it (or it
/// came prefilled), and it is not just a required box still empty (that is
/// the bar's "left to fill", not an error).
pub(crate) fn error_shown(st: &FormSession, form: &OpenForm, id: ItemId) -> bool {
    st.shows_error(id) && !form.item(id).is_some_and(|item| item_left(st, item))
}

/// The first message on a shown item that is not a box left empty.
pub(crate) fn first_fix(st: &FormSession, form: &OpenForm) -> Option<Placed> {
    let messages = st.messages.get();
    messages.into_iter().find(|m| {
        !item_hidden(st, form, m.item) && !form.item(m.item).is_some_and(|item| item_left(st, item))
    })
}

/// The one next step the bar shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Step {
    Fix { item: ItemId, label: String, message: String },
    Fill { item: ItemId, label: String, left: usize },
    Queue,
    /// Read-only: queued, sent and so on (the status).
    Done(String),
}

pub(crate) fn next_step(st: &FormSession, form: &OpenForm) -> Step {
    if !st.editable.get() {
        return Step::Done(st.status.get());
    }
    if let Some(fix) = first_fix(st, form) {
        let label = form.item(fix.item).map(item_title).unwrap_or_default();
        return Step::Fix { item: fix.item, label, message: fix.message };
    }
    let left = left_items(st, form);
    if let Some(first) = left.first() {
        let label = form.item(*first).map(item_title).unwrap_or_default();
        return Step::Fill { item: *first, label, left: left.len() };
    }
    Step::Queue
}

/// How much of what is asked is filled, 0 to 1.
pub(crate) fn progress(st: &FormSession, form: &OpenForm) -> f32 {
    let all = required_items(st, form).len();
    if all == 0 {
        return 1.0;
    }
    let left = left_items(st, form).len();
    (all - left.min(all)) as f32 / all as f32
}

/// A section's mark in the outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Done,
    Left(usize),
    Fix,
}

pub(crate) fn part_mark(st: &FormSession, form: &OpenForm, part: usize) -> Mark {
    let items: Vec<ItemId> = shown_items(st, form).into_iter().filter(|id| id.0 == part).collect();
    let fixes = st.messages.with(|m| m.iter().any(|p| items.contains(&p.item)));
    let left = items.iter().filter(|id| form.item(**id).is_some_and(|item| item_left(st, item))).count();
    let fixes = fixes
        && st.messages.with(|m| {
            m.iter().any(|p| items.contains(&p.item) && !form.item(p.item).is_some_and(|item| item_left(st, item)))
        });
    if fixes {
        Mark::Fix
    } else if left > 0 {
        Mark::Left(left)
    } else {
        Mark::Done
    }
}

/// The box holding the amount to pay (`Aggregate Amount Payable`, else
/// `Total Amount Payable`, else `Tax Payable`), first in the form.
pub(crate) fn amount_due_key(form: &OpenForm) -> Option<String> {
    for want in ["aggregate amount payable", "total amount payable", "tax payable", "tax still due", "tax due"] {
        for part in &form.layout.parts {
            for item in &part.items {
                if item.label.to_lowercase().contains(want)
                    && let Some(field) = item.fields.iter().find(|f| f.computed || f.kind == "money")
                {
                    return Some(field.key.clone());
                }
            }
        }
    }
    None
}

/// An amount as a number (`"1,234.50"`, `"(12.00)"`), if it is one.
pub(crate) fn amount(text: &str) -> Option<f64> {
    let t: String = text.chars().filter(|c| !matches!(c, ',' | '₱' | ' ')).collect();
    if t.is_empty() {
        return None;
    }
    if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        return inner.parse::<f64>().ok().map(|n| -n);
    }
    t.parse().ok()
}

/// `2026-11-15` as `Nov 15`.
pub(crate) fn short_date(iso: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = iso.split('-');
    let (_, m, d) = (parts.next(), parts.next(), parts.next());
    match (m.and_then(|m| m.parse::<usize>().ok()), d.and_then(|d| d.parse::<u32>().ok())) {
        (Some(m), Some(d)) if (1..=12).contains(&m) => format!("{} {d}", MONTHS[m - 1]),
        _ => iso.to_owned(),
    }
}
