//! PR 5: the quarterly income tax returns (1701Q, 1702Q). What the host
//! writes or closes on these pages, beyond the probed bindings:
//!
//! - 1701Q Civil Status (maintainer decision 11). The January 2018 sheet has
//!   no such item; the page asks it at the head of Part II
//!   ([`CIVIL_STATUS`], a page box with no official key) and
//!   `Form1701QDraft::set_civil_status` keeps the answer: Married opens
//!   Part II and the spouse's column B, another answer closes them and
//!   clears what was entered there. While they are closed (not Married, or
//!   an estate or trust at item 7, the official `disableSpouse`) a write
//!   there is refused in the filer's words and rule 6 fades the boxes with
//!   what opens them ([`gated`]). A draft stored before the answer existed
//!   shows Married when it reports a spouse and is never changed on open.
//! - 1701Q item 17, the spouse's TIN: one draft field shown over four boxes,
//!   written box by box the way 1700's item 18 is (`rows_1700`).
//! - 1701Q items 43 and 48's "(specify)" boxes, shared by both columns,
//!   close while no party files their schedule (the compute keeps the text).
//! - 1702Q's schedules need item 5's ATC: the form's own `recompute` clears
//!   them otherwise, so rule 6 fades them through the conditions the corpus
//!   leaves out (`conditions.rs` CORPUS_GAPS_PR5); nothing here writes them.
//! - An edit that removes what the filer typed elsewhere (Civil Status off
//!   Married, an item 7 estate or trust, an ATC, rate or deduction method
//!   that closes a schedule, Amended Return No, 1702Q's item 5 cleared) is
//!   reported ([`removes`]) so the page asks first and offers Undo.
//!
//! Field keys, the official XML and the print map are the forms' own and
//! unchanged; the civil status is not filed.

use std::collections::BTreeMap;

use bir_core::forms::agent::draft_locks::income_q as draft_locks;
use bir_core::forms::agent::{self, AgentFormKind, Binding};
use bir_core::forms::form_1701q::{
    Form1701QAtc, Form1701QCivilStatus, Form1701QDraft, Form1701QFilerType, Form1701QParty,
    Form1701QTaxRate, USER_ENTERED_AMOUNT_ITEMS,
};
use bir_core::forms::form_1702q::{Form1702qDeduction, Form1702qDraft};
use serde_json::{Map, Value, json};

/// The page box of 1701Q's Civil Status. Not an official control: the
/// upload, the print and the field map never see it.
pub(crate) const CIVIL_STATUS: &str = "page:1701Q:civilStatus";

/// What a closed spouse part says it needs (decision 11's words).
pub(crate) const NEEDS_MARRIED: &str = "Needs Civil Status = Married";
/// What it needs on an estate's or trust's return.
pub(crate) const NEEDS_INDIVIDUAL: &str = "Needs item 7 = Single Proprietor or Professional";

/// The refusal of a write to a closed spouse part, in the filer's words.
pub(crate) const MARRIED_FIRST: &str =
    "Part II and column B are for a married filer: choose Married at Civil Status first.";
/// The refusal on an estate's or trust's return.
pub(crate) const INDIVIDUAL_FIRST: &str = "An estate or trust reports no spouse: Part II and column B are for a Single Proprietor or Professional (item 7).";

/// What item 43's and item 48's "(specify)" boxes say they need: the tags
/// of their conditions (app/forms/condition-labels.json).
pub(crate) const NEEDS_GRADUATED: &str = "Needs the graduated income tax rate";
pub(crate) const NEEDS_EIGHT: &str = "Needs the 8% income tax rate";
/// Their refusals.
pub(crate) const GRADUATED_FIRST: &str =
    "Item 43 is Schedule I's: choose Graduated Rates at item 16 (or item 25 for the spouse) first.";
pub(crate) const EIGHT_FIRST: &str =
    "Item 48 is Schedule II's: choose the 8% rate at item 16 (or item 25 for the spouse) first.";

/// A compensation earner only (item 20 II011) files no Schedule IV: what
/// the spouse's penalty boxes say then, and their refusal.
pub(crate) const NEEDS_NOT_II011: &str = "Needs item 20 other than II011 Compensation Income";
pub(crate) const NOT_II011_FIRST: &str = "A spouse who is a compensation earner only (item 20 II011) files no penalties here: choose the spouse's other ATC at item 20 first.";

/// What a 1702Q schedule entry waits for. `recompute` (form_1702q.rs)
/// clears or overwrites the entry while it does not hold, so the host
/// refuses a write there with the tag (app/forms/condition-labels.json),
/// rather than taking it and losing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Gate {
    /// Item 5 has an ATC (Schedules 1, 2 and 4).
    Atc,
    /// And the return is for the 2nd or 3rd quarter.
    AtcLaterQuarter,
    /// And item 12 is not OSD (Schedule 2 item 6, which OSD overwrites).
    AtcItemized,
    /// And Amended Return is Yes (Schedule 4 item 6).
    AtcAmended,
    /// Both item 5 boxes are ticked (Schedule 3).
    Mcit,
    McitLaterQuarter,
    McitThirdQuarter,
    /// Item 13 Yes (13A).
    Relief,
    /// The regular income tax is above the MCIT (item 15).
    PriorMcit,
}

impl Gate {
    /// The tag rule 6 shows, and the refusal of a write while closed.
    pub(super) const fn tag(self) -> &'static str {
        match self {
            Self::Atc => "Needs item 5 = an ATC",
            Self::AtcLaterQuarter => "Needs item 5 = an ATC, on a 2nd or 3rd quarter return",
            Self::AtcItemized => "Needs item 5 = an ATC and item 12 = Itemized Deductions",
            Self::AtcAmended => "Needs item 5 = an ATC and item 4 = Yes",
            Self::Mcit => "Needs both item 5 boxes ticked (MCIT and an ATC)",
            Self::McitLaterQuarter => {
                "Needs both item 5 boxes ticked, on a 2nd or 3rd quarter return"
            }
            Self::McitThirdQuarter => "Needs both item 5 boxes ticked, on a 3rd quarter return",
            Self::Relief => "Needs item 13 = Yes",
            Self::PriorMcit => "Needs the regular income tax above the MCIT (Schedule 2)",
        }
    }

    /// The gate of a 1702Q box, if any.
    pub(super) fn of(key: &str) -> Option<Self> {
        let tail = key.strip_prefix("frm1702q:")?;
        Some(match tail {
            "Sched1:txtTax8A" | "Sched1:txtTax8B" | "Sched2:txtTax8" | "Sched4:txtTax2"
            | "Sched4:txtTax3" | "Sched4:txtTax4" => Self::AtcLaterQuarter,
            "Sched1:txtTax1A"
            | "Sched1:txtTax1B"
            | "Sched1:txtTax2A"
            | "Sched1:txtTax2B"
            | "Sched1:txtTax4A"
            | "Sched1:txtTax4B"
            | "Sched1:txtTax6A"
            | "Sched1:txtTax6B"
            | "Sched1:txtTax10B"
            | "Sched1:txtTax12B"
            | "Sched2:txtTax1"
            | "Sched2:txtTax2"
            | "Sched2:txtTax4"
            | "Sched4:txtTax1"
            | "Sched4:txtTax5"
            | "Sched4:txtOthrTxCrdts0"
            | "Sched4:txtOthrTxCrdtAmnt0"
            | "Sched4:txtOthrTxCrdts1"
            | "Sched4:txtOthrTxCrdtAmnt1" => Self::Atc,
            "Sched2:txtTax6" => Self::AtcItemized,
            "Sched4:txtTax6" => Self::AtcAmended,
            "Sched3:txtTax1" => Self::Mcit,
            "Sched3:txtTax2" => Self::McitLaterQuarter,
            "Sched3:txtTax3" => Self::McitThirdQuarter,
            "txtTxRlfSpcfy" => Self::Relief,
            "txtTax15" => Self::PriorMcit,
            _ => return None,
        })
    }

    /// Whether the gate holds on this (computed) draft, with the port's own
    /// test for each (form_1702q.rs `recompute`).
    fn holds(self, form: &Form1702qDraft) -> bool {
        let atc = form.atc_option().is_some();
        match self {
            Self::Atc => atc,
            Self::AtcLaterQuarter => atc && form.previous_quarters_open(),
            Self::AtcItemized => atc && form.deduction != Form1702qDeduction::Osd,
            Self::AtcAmended => atc && form.previously_filed_open(),
            Self::Mcit => form.mcit_open(),
            Self::McitLaterQuarter => form.mcit_open() && form.quarter >= 2,
            Self::McitThirdQuarter => form.mcit_open() && form.quarter >= 3,
            Self::Relief => form.tax_relief,
            Self::PriorMcit => form.prior_mcit_open(),
        }
    }
}

/// Part II's boxes (items 17-25A).
pub(crate) const SPOUSE_PART: [&str; 24] = [
    "frm1701q:txtSpouseTIN1",
    "frm1701q:txtSpouseTIN2",
    "frm1701q:txtSpouseTIN3",
    "frm1701q:txtSpouseBranchCode",
    "frm1701q:txtSpouseRDOCode",
    "frm1701q:optSpouseType_1",
    "frm1701q:optSpouseType_2",
    "frm1701q:optSpouseType_3",
    "frm1701q:optSpouseATC_1",
    "frm1701q:optSpouseATC_2",
    "frm1701q:optSpouseATC_3",
    "frm1701q:optSpouseATC_4",
    "frm1701q:optSpouseATC_5",
    "frm1701q:optSpouseATC_6",
    "frm1701q:optSpouseATC_7",
    "frm1701q:txtSpouseName",
    "frm1701q:txtSpouseCitizenship",
    "frm1701q:txtSpouseForeignTaxNum",
    "frm1701q:optSpouseForeignTaxCred_1",
    "frm1701q:optSpouseForeignTaxCred_2",
    "frm1701q:optSpouseTaxRate_1",
    "frm1701q:optSpouseTaxRate_2",
    "frm1701q:optSpouseMethod:_1",
    "frm1701q:optSpouseMethod:_2",
];

/// The spouse's column B items the filer types (Schedules I-IV).
pub(crate) const SPOUSE_COLUMN: [u8; 20] = [
    36, 37, 39, 42, 43, 44, 47, 48, 50, 52, 55, 56, 57, 58, 59, 60, 61, 64, 65, 66,
];

/// A box this module writes or closes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Cell {
    /// 1701Q's Civil Status (a page box).
    CivilStatus,
    /// 1701Q item 17: TIN box 0-2 or 3 (the branch code).
    SpouseTin(usize),
    /// Any other box of Part II or column B: written by the ordinary fill
    /// while the spouse part is open.
    SpousePart,
    /// Items 64B-66B: as `SpousePart`, and closed too for a compensation
    /// earner only, whose column files no penalties (the shared model's
    /// spouse items do not apply), so the page's penalty lock never offers
    /// an Edit that cannot file.
    SpousePenalty,
    /// Item 43's (`true`) or item 48's "(specify)" box, shared by both
    /// columns: open while either party files that schedule (the official
    /// enableSchedule1/2; the compute keeps the text, so the host closes it).
    Specify(bool),
    /// A 1702Q box that waits for its gate.
    Gated1702Q(Gate),
}

/// Part II's radio groups' own boxes: a batch (the assistant, a preview
/// sync) may name the group rather than its radios.
pub(crate) const SPOUSE_GROUPS: [&str; 5] = [
    "frm1701q:optSpouseType",
    "frm1701q:optSpouseATC",
    "frm1701q:optSpouseTaxRate",
    "frm1701q:optSpouseMethod",
    "frm1701q:optSpouseForeignTaxCred",
];

fn is_1701q(kind: AgentFormKind) -> bool {
    kind.form_code() == "1701Q"
}

pub(super) fn cell(kind: AgentFormKind, key: &str) -> Option<Cell> {
    if kind.form_code() == "1702Q" {
        return Gate::of(key).map(Cell::Gated1702Q);
    }
    if !is_1701q(kind) {
        return None;
    }
    if key == CIVIL_STATUS {
        return Some(Cell::CivilStatus);
    }
    match key {
        "frm1701q:txtSpouseTIN1" => return Some(Cell::SpouseTin(0)),
        "frm1701q:txtSpouseTIN2" => return Some(Cell::SpouseTin(1)),
        "frm1701q:txtSpouseTIN3" => return Some(Cell::SpouseTin(2)),
        "frm1701q:txtSpouseBranchCode" => return Some(Cell::SpouseTin(3)),
        _ => {}
    }
    if SPOUSE_PART.contains(&key) || SPOUSE_GROUPS.contains(&key) {
        return Some(Cell::SpousePart);
    }
    match key {
        "frm1701q:txt43Desc" => return Some(Cell::Specify(true)),
        "frm1701q:txt48Desc" => return Some(Cell::Specify(false)),
        _ => {}
    }
    let item: u8 = key
        .strip_prefix("frm1701q:txt")?
        .strip_suffix('B')?
        .parse()
        .ok()?;
    if (64..=66).contains(&item) {
        return Some(Cell::SpousePenalty);
    }
    SPOUSE_COLUMN.contains(&item).then_some(Cell::SpousePart)
}

/// Whether the host writes `key` itself (its "cannot be filled here" lock
/// does not apply on the page): the Civil Status and the spouse's TIN.
pub(super) fn host_writes(kind: AgentFormKind, key: &str) -> bool {
    matches!(
        cell(kind, key),
        Some(Cell::CivilStatus | Cell::SpouseTin(_))
    )
}

/// Civil Status first, then the spouse's TIN boxes in order, then the rest
/// of the spouse part, so one batch can answer Married and fill Part II.
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    match cell(kind, key)? {
        Cell::CivilStatus | Cell::Specify(_) => None,
        Cell::SpouseTin(part) => Some(1 + part),
        Cell::SpousePart | Cell::SpousePenalty | Cell::Gated1702Q(_) => Some(5),
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.as_str().unwrap_or_default().trim().to_string(),
    }
}

fn typed(draft: &Value) -> Result<Form1701QDraft, String> {
    serde_json::from_value(draft.clone()).map_err(|e| e.to_string())
}

fn store(draft: &mut Value, form: &Form1701QDraft) -> Result<(), String> {
    *draft = serde_json::to_value(form).map_err(|e| e.to_string())?;
    Ok(())
}

/// A write that says nothing (blank, zero, unticked), which a closed box
/// takes: the page clears what it shows.
fn says_nothing(value: &Value) -> bool {
    let shown = text(value);
    shown.is_empty()
        || shown == "false"
        || shown.chars().all(|c| matches!(c, '0' | '.' | ',' | '-'))
}

/// Why the spouse part is closed on this draft, or `None` while it is open.
fn spouse_closed(form: &Form1701QDraft) -> Option<(&'static str, &'static str)> {
    if matches!(
        form.filer_type,
        Some(Form1701QFilerType::Estate | Form1701QFilerType::Trust)
    ) {
        Some((INDIVIDUAL_FIRST, NEEDS_INDIVIDUAL))
    } else if !form.has_spouse {
        Some((MARRIED_FIRST, NEEDS_MARRIED))
    } else {
        None
    }
}

/// Why item 43's (`graduated`) or item 48's "(specify)" box is closed: no
/// party files that schedule (the taxpayer at item 16, a married spouse who
/// is not a compensation earner only at item 25), or `None` while one does.
fn specify_closed(form: &Form1701QDraft, graduated: bool) -> Option<(&'static str, &'static str)> {
    let rate = if graduated {
        Form1701QTaxRate::Graduated
    } else {
        Form1701QTaxRate::EightPercent
    };
    let spouse = spouse_closed(form).is_none()
        && form.spouse_atc != Some(Form1701QAtc::Ii011)
        && form.spouse_tax_rate == Some(rate);
    if form.tax_rate == Some(rate) || spouse {
        None
    } else if graduated {
        Some((GRADUATED_FIRST, NEEDS_GRADUATED))
    } else {
        Some((EIGHT_FIRST, NEEDS_EIGHT))
    }
}

/// Why a box this module closes is closed on this draft: `(refusal, tag)`,
/// or `None` while it is open (and for every other box).
fn closed(cell: Cell, form: &Form1701QDraft) -> Option<(&'static str, &'static str)> {
    match cell {
        Cell::SpouseTin(_) | Cell::SpousePart => spouse_closed(form),
        Cell::SpousePenalty => spouse_closed(form).or_else(|| {
            (form.spouse_atc == Some(Form1701QAtc::Ii011))
                .then_some((NOT_II011_FIRST, NEEDS_NOT_II011))
        }),
        Cell::Specify(graduated) => specify_closed(form, graduated),
        Cell::CivilStatus | Cell::Gated1702Q(_) => None,
    }
}

/// [`closed`] on a draft of either form.
fn closed_on(cell: Cell, draft: &Value) -> Option<(&'static str, &'static str)> {
    if let Cell::Gated1702Q(gate) = cell {
        // Read on the recomputed draft: within one batch the stored totals
        // may lag what the batch typed (item 15 reads the regular tax and
        // the MCIT).
        let mut form: Form1702qDraft = serde_json::from_value(draft.clone()).ok()?;
        form.recompute();
        return (!gate.holds(&form)).then_some((gate.tag(), gate.tag()));
    }
    closed(cell, &typed(draft).ok()?)
}

/// `Some(tag)` for a box the host closes while its answer does not hold
/// (1701Q's spouse part while the filer is not married, or files for an
/// estate or trust; items 43 and 48's "(specify)" while no party files
/// their schedule), `None` for every other box. Rule 6 fades it with the
/// tag whatever it shows: a legacy draft's value there is faded, not hidden.
pub(super) fn gated(kind: AgentFormKind, draft: &Value, key: &str) -> Option<&'static str> {
    closed_on(cell(kind, key)?, draft).map(|(_, tag)| tag)
}

/// The refusal of a write to a closed spouse part (`None` for every other
/// write): the page's fill asks it before any other lock of the box, so a
/// spouse penalty says what opens it rather than "Edit penalties".
pub(super) fn closed_refusal(
    kind: AgentFormKind,
    draft: &Value,
    key: &str,
    value: &Value,
) -> Option<&'static str> {
    let cell = cell(kind, key)?;
    if says_nothing(value) {
        return None;
    }
    closed_on(cell, draft).map(|(refusal, _)| refusal)
}

/// Write `key` when this module owns it: `None` leaves it to the ordinary
/// fill. `ordinary` is that fill, for a box this module only gates.
pub(super) fn fill(
    kind: AgentFormKind,
    draft: &mut Value,
    key: &str,
    value: &Value,
    ordinary: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Option<Result<(), String>> {
    let cell = cell(kind, key)?;
    Some(fill_cell(draft, cell, value, ordinary))
}

fn fill_cell(
    draft: &mut Value,
    cell: Cell,
    value: &Value,
    ordinary: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    if let Cell::Gated1702Q(_) = cell {
        if let Some((refusal, _)) = closed_on(cell, draft)
            && !says_nothing(value)
        {
            return Err(refusal.to_string());
        }
        return ordinary(draft);
    }
    let mut form = typed(draft)?;
    if cell == Cell::CivilStatus {
        let answer = text(value);
        let status = if answer.is_empty() {
            None
        } else {
            Some(Form1701QCivilStatus::from_label(&answer).ok_or_else(|| {
                format!(
                    "`{answer}` is not a civil status: choose Single, Married, Legally Separated or Widow/er."
                )
            })?)
        };
        form.set_civil_status(status);
        return store(draft, &form);
    }
    if let Some((refusal, _)) = closed(cell, &form) {
        // Clearing a closed box is taken (it files nothing either way).
        if !says_nothing(value) {
            return Err(refusal.to_string());
        }
    }
    match cell {
        Cell::SpouseTin(part) => {
            form.spouse_tin =
                super::rows_1700::with_tin_part(&form.spouse_tin, part, &text(value))?;
            store(draft, &form)
        }
        _ => ordinary(draft),
    }
}

/// The one-time note on a filed third-quarter 1701Q charged under the old
/// deadline (maintainer decision D3, 2026-10-18). The app's deadline moves
/// off a weekend or holiday (2025's was November 17).
pub(crate) const Q3_DEADLINE_NOTE: &str = "The third-quarter 1701Q is now due November 15, or the next business day when that falls on a weekend or holiday; it was October 15. This return's penalties were counted from October 15: any charged for the days from October 16 to that due date may need checking.";

/// Whether `draft` is a third-quarter 1701Q that left Draft (queued,
/// submitted, confirmed or paid) with penalties the app charged under the
/// October 15 deadline it resolved before PR 5. A column counts when it
/// files a surcharge, interest or compromise and a stored due date says
/// October of the taxable year: the engine's own input
/// (`computed_penalties.inputs.due_date`), else the deadline the return
/// kept (`penalty_due_date`). With no stored date it does not count: before
/// PR 5 the 1701Q penalties were typed by the filer, not charged by the app.
pub(super) fn q3_charged_under_october(kind: AgentFormKind, draft: &Value) -> bool {
    use chrono::Datelike;
    if !is_1701q(kind) {
        return false;
    }
    let Ok(form) = typed(draft) else {
        return false;
    };
    if form.quarter != 3 || form.lifecycle.is_editable() {
        return false;
    }
    Form1701QDraft::penalty_declarations().iter().any(|items| {
        let charged = items.filed(&form).iter().any(|amount| *amount != 0.0);
        let state = (items.state)(&form);
        let due = state
            .computed_penalties
            .as_ref()
            .map(|computed| computed.inputs.due_date)
            .or(state.penalty_due_date);
        charged
            && (items.applies)(&form)
            && due
                .is_some_and(|due| due.year() == i32::from(form.taxable_year) && due.month() == 10)
    })
}

/// The settings key that records [`Q3_DEADLINE_NOTE`] was shown for the
/// stored return `id`. App state, never the return: a filed return's bytes
/// do not change.
pub(super) fn q3_note_seen_key(id: i64) -> String {
    format!("{NOTE_SEEN_PREFIX}1701q_q3_due_nov15:{id}")
}

/// The prefix of every one-time note's seen key: the only settings keys
/// `screen_form_notes_seen` writes.
pub(super) const NOTE_SEEN_PREFIX: &str = "notice_seen:";

/// The one-time notes `draft` offers on opening, `{key, text}`:
/// [`Q3_DEADLINE_NOTE`] for a filed third-quarter 1701Q charged under
/// October 15 (D3), until it is recorded seen. Nothing is written here: the
/// page records a note seen once it has drawn the opening it came with
/// (`screen_form_notes_seen`, insert-if-absent), so an opening that fails
/// before it is drawn offers it again. Two openings offered it before
/// either recorded it both draw it; the one whose record call does not
/// record it then takes it off, so it stays on one page. A note
/// that cannot be read is left out; it never stops the return opening, and
/// the filed amounts are never changed.
pub(super) fn once_notes(
    db: &bir_core::db::Database,
    kind: AgentFormKind,
    draft: &Value,
) -> Vec<Value> {
    let Some(id) = draft["id"].as_i64() else {
        return Vec::new();
    };
    if !q3_charged_under_october(kind, draft) {
        return Vec::new();
    }
    let key = q3_note_seen_key(id);
    match db.get_setting(&key) {
        Ok(None) => vec![json!({"key": key, "text": Q3_DEADLINE_NOTE})],
        Ok(Some(_)) => Vec::new(),
        Err(error) => {
            tracing::warn!("1701Q: reading {key} failed: {error}");
            Vec::new()
        }
    }
}

/// 1702Q's `resultOtherSpecify`: BIR's internal total of the Schedule 4
/// other credits, not a box of the printed sheet.
pub(crate) const INTERNAL_TOTAL: &str = "resultOtherSpecify";

/// What 1702Q's upload carries for [`INTERNAL_TOTAL`] (maintainer decision
/// D9, 2026-10-18: the page shows it, read-only, as BIR's internal value).
/// This app never writes it, so the official layout's default 0.00 goes
/// out, which is what BIR's own save writes
/// (tests/sample-payloads/1702Q-2025Q2.official.xml: `resultOtherSpecify=
/// 0.00` beside an other credit of 1,234.00). Read back from the official
/// plaintext of the draft's field map, so the box shows exactly what files.
fn internal_total(kind: AgentFormKind, draft: &Value) -> Option<String> {
    use bir_core::official_xml;
    let layout = official_xml::layout(bir_core::forms::form_1702q::FORM_1702Q_FORM_ID).ok()?;
    let fields = agent::field_map(kind, draft).ok()?;
    let plaintext = official_xml::write(layout, &fields).ok()?;
    official_xml::read(layout, &plaintext)
        .ok()?
        .remove(INTERNAL_TOTAL)
}

/// What each page box of this module shows: the Civil Status answer, and
/// item 17's TIN boxes as the draft holds the digits (the field map writes
/// a box only once it is whole, so a box typed digit by digit would show
/// nothing until then). Display only: the field map, upload and print are
/// unchanged.
pub(super) fn values(kind: AgentFormKind, draft: &Value, map: &mut BTreeMap<String, String>) {
    if kind.form_code() == "1702Q" {
        if let Some(value) = internal_total(kind, draft) {
            map.insert(INTERNAL_TOTAL.to_string(), value);
        }
        return;
    }
    if !is_1701q(kind) {
        return;
    }
    let Ok(form) = typed(draft) else {
        return;
    };
    let shown = form
        .shown_civil_status()
        .map_or("", Form1701QCivilStatus::label);
    map.insert(CIVIL_STATUS.to_string(), shown.to_string());
    if spouse_closed(&form).is_none() {
        let digits: String = form
            .spouse_tin
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        let slice = |from: usize, to: usize| {
            digits
                .get(from.min(digits.len())..to.min(digits.len()))
                .unwrap_or("")
                .to_string()
        };
        for (key, (from, to)) in [
            ("frm1701q:txtSpouseTIN1", (0, 3)),
            ("frm1701q:txtSpouseTIN2", (3, 6)),
            ("frm1701q:txtSpouseTIN3", (6, 9)),
            ("frm1701q:txtSpouseBranchCode", (9, digits.len())),
        ] {
            map.insert(key.to_string(), slice(from, to));
        }
    }
}

/// The lock of a 1701Q penalty column that still holds the filer's own
/// amounts (a draft stored, or a save file imported, before the penalty
/// model: penalty_model_version 0); "computed from the due date" would be
/// wrong there. bir-core's (`agent::draft_locks::income_q`), which locks
/// the column on the page and refuses it to the assistant alike.
pub(crate) const KEPT_AS_ENTERED: &str = draft_locks::KEPT_AS_ENTERED;

// [`KEPT_AS_ENTERED`] for a penalty box of a column still on its typed
// amounts, `None` for every other box (bir-core's).
pub(super) use draft_locks::penalty_lock;

/// Whether a penalty group's column holds the filer's own amounts: the page
/// offers "Use the computed amounts" as soon as Edit opens it.
pub(super) fn kept_as_entered(kind: AgentFormKind, keys: &[String], draft: &Value) -> bool {
    keys.first()
        .is_some_and(|key| penalty_lock(kind, draft, key).is_some())
}

/// A blank written to a column still on its typed amounts ("Use the
/// computed amounts") joins the model, so the computed amounts file.
pub(super) fn join_model(kind: AgentFormKind, draft: &mut Value, key: &str) {
    let Some((n, version)) = draft_locks::penalty_column(kind, draft, key) else {
        return;
    };
    if version >= bir_core::forms::penalty_items::PENALTY_MODEL_VERSION {
        return;
    }
    let Ok(mut form) = typed(draft) else {
        return;
    };
    let items = &Form1701QDraft::penalty_declarations()[n];
    (items.state_mut)(&mut form).penalty_model_version =
        bir_core::forms::penalty_items::PENALTY_MODEL_VERSION;
    let _ = store(draft, &form);
}

/// A typed entry's printed item for a message: `36A`, `21`, or the
/// corpus's item (`Schedule 2 Item 1` -> `Schedule 2 item 1`).
fn item_name(kind: AgentFormKind, key: &str) -> String {
    let tail = key.rsplit(':').next().unwrap_or(key);
    if is_1701q(kind)
        && let Some(item) = tail
            .strip_prefix("txt")
            .filter(|rest| rest.ends_with(['A', 'B']))
            .filter(|rest| rest[..rest.len() - 1].chars().all(|c| c.is_ascii_digit()))
    {
        return item.to_string();
    }
    kind.definition()
        .box_def(key)
        .map(|entry| entry.item.replace(" Item ", " item "))
        .filter(|item| !item.is_empty())
        .unwrap_or_else(|| tail.to_string())
}

/// The amounts and texts the filer typed, as the page shows them: bound
/// boxes the form does not compute, plus the spouse's TIN boxes.
fn typed_entries(kind: AgentFormKind, draft: &Value) -> BTreeMap<String, String> {
    let def = kind.definition();
    let Ok(map) = agent::field_map(kind, draft) else {
        return BTreeMap::new();
    };
    let typed_box = |key: &str| {
        let binding = def.bindings.get(key).map(|bound| &bound.binding);
        // A penalty box shows the computed amount until the filer types
        // one (its override, rule 8): only that is typed.
        if let Some(Binding::Amount { path, .. }) = binding
            && path.contains("/penalty_overrides/")
        {
            return draft.pointer(path).is_some_and(|value| !value.is_null());
        }
        let bound = matches!(
            binding,
            Some(Binding::Amount { .. } | Binding::Whole { .. } | Binding::Text { .. })
        ) && def.box_def(key).is_some_and(|entry| !entry.computed);
        // A box its gate closes holds nothing the filer typed: it is
        // cleared, or derived (1702Q's Schedule 2 item 6 under OSD).
        let gated_closed = cell(kind, key).is_some_and(|cell| {
            matches!(cell, Cell::Gated1702Q(_)) && closed_on(cell, draft).is_some()
        });
        !gated_closed && (bound || matches!(cell(kind, key), Some(Cell::SpouseTin(_))))
    };
    map.into_iter()
        .filter(|(key, _)| typed_box(key))
        .filter(|(_, shown)| !says_nothing(&json!(shown)))
        .collect()
}

/// A choice: a radio, tick box or list (its binding or its control), or the
/// Civil Status.
fn is_answer(kind: AgentFormKind, key: &str) -> bool {
    let def = kind.definition();
    key == CIVIL_STATUS
        || matches!(
            def.bindings.get(key).map(|bound| &bound.binding),
            Some(Binding::Choice { .. })
        )
        || def.box_def(key).is_some_and(|entry| {
            ["radio", "checkbox", "select"]
                .iter()
                .any(|kind| entry.kind.contains(kind))
        })
}

/// How many entries of 1701Q's spouse part `before` held that `after` does
/// not, read from the drafts (so a name a stored draft kept with no spouse,
/// which the page does not show, counts too): items 17-25A's texts and
/// answers, column B's typed amounts and the spouse's penalty overrides.
fn spouse_entries_removed(before: &Value, after: &Value) -> usize {
    let (Ok(was), Ok(now)) = (typed(before), typed(after)) else {
        return 0;
    };
    let text = |form: &Form1701QDraft| {
        [
            form.spouse_tin.clone(),
            form.spouse_rdo_code.clone(),
            form.spouse_name.clone(),
            form.spouse_citizenship.clone(),
            form.spouse_foreign_tax_number.clone(),
        ]
        .map(|value| !says_nothing(&json!(value)))
    };
    // What the filer chose: item 20 is the recompute's own for a
    // compensation earner only (II011), and item 25 whenever the ATC sets it.
    let choices = |form: &Form1701QDraft| {
        let atc_chosen = form.spouse_atc.is_some()
            && !(form.spouse_type
                == Some(bir_core::forms::form_1701q::Form1701QSpouseType::CompensationEarner)
                && form.spouse_atc == Some(Form1701QAtc::Ii011));
        let rate_chosen = form.spouse_tax_rate.is_some()
            && form.spouse_atc.and_then(Form1701QAtc::tax_rate) != form.spouse_tax_rate;
        [
            form.spouse_type.is_some(),
            atc_chosen,
            form.spouse_claims_foreign_tax_credits.is_some(),
            rate_chosen,
            form.spouse_deduction_method.is_some(),
        ]
    };
    // Typed amounts; items 64B-66B are typed while the spouse's column is
    // still on the typed boxes (penalty_model_version 0: a stored or
    // imported draft), computed (not counted) once on the model.
    let amounts = |form: &Form1701QDraft| -> Vec<bool> {
        let typed_penalties = form.spouse_penalty.penalty_model_version
            < bir_core::forms::penalty_items::PENALTY_MODEL_VERSION;
        USER_ENTERED_AMOUNT_ITEMS
            .iter()
            .filter(|item| typed_penalties || !(64..=66).contains(*item))
            .map(|item| {
                form.amount(*item, Form1701QParty::Spouse)
                    .is_some_and(|value| value != 0.0)
            })
            .collect()
    };
    // An override is what the filer typed, a zero too.
    let overrides = |form: &Form1701QDraft| {
        bir_core::penalties::PenaltyComponent::ALL.map(|component| {
            form.spouse_penalty
                .penalty_overrides
                .get(component)
                .is_some()
        })
    };
    let gone =
        |a: &[bool], b: &[bool]| a.iter().zip(b).filter(|(was, now)| **was && !**now).count();
    gone(&text(&was), &text(&now))
        + gone(&choices(&was), &choices(&now))
        + gone(&amounts(&was), &amounts(&now))
        + gone(&overrides(&was), &overrides(&now))
}

/// When `edits` turned `before` into `after` by removing what the filer
/// typed in other boxes, what the page asks first (`message`), what the
/// Undo toast says (`done`) and how many boxes went; `None` when nothing
/// typed went. The host applies the edit either way; the page keeps its
/// old draft until the filer chooses Continue.
pub(super) fn removes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    if !matches!(kind.form_code(), "1701Q" | "1702Q") {
        return None;
    }
    if is_1701q(kind) && edits.contains_key(CIVIL_STATUS) {
        let n = spouse_entries_removed(before, after);
        if n == 0 {
            return None;
        }
        return Some(json!({
            "message": format!(
                "Without Married at Civil Status, the spouse's part goes: this removes {n} entr{} you made in Part II and column B. Continue?",
                if n == 1 { "y" } else { "ies" }
            ),
            "done": "Cleared the spouse's part",
            "rows": n,
        }));
    }
    let had = typed_entries(kind, before);
    let has = typed_entries(kind, after);
    // What each box shows after the edit, typed or not: an answer (a
    // choice) can replace a typed amount with a computed one (1702Q's item
    // 12 OSD over Schedule 2 item 6), which is lost too.
    let shown = agent::field_map(kind, after).unwrap_or_default();
    let answered = edits.keys().any(|key| is_answer(kind, key));
    let replaced: Vec<&String> = had
        .iter()
        .filter(|(key, was)| {
            answered
                && !edits.contains_key(*key)
                && !has.contains_key(*key)
                && shown
                    .get(*key)
                    .is_some_and(|now| !says_nothing(&json!(now)) && now != *was)
        })
        .map(|(key, _)| key)
        .collect();
    let cleared: Vec<&String> = had
        .keys()
        .filter(|key| {
            !edits.contains_key(*key) && !has.contains_key(*key) && !replaced.contains(key)
        })
        .collect();
    let replaced: Vec<&String> = replaced
        .into_iter()
        .chain(had.iter().filter_map(|(key, was)| {
            (answered && !edits.contains_key(key) && has.get(key).is_some_and(|now| now != was))
                .then_some(key)
        }))
        .collect();
    let lost: Vec<&String> = had
        .keys()
        .filter(|key| cleared.contains(key) || replaced.contains(key))
        .collect();
    if lost.is_empty() {
        return None;
    }
    let n = lost.len();
    let done = format!("Removed {n} entr{}", if n == 1 { "y" } else { "ies" });
    let verb = match (cleared.is_empty(), replaced.is_empty()) {
        (false, true) => "clears",
        (true, false) => "replaces",
        _ => "clears or replaces",
    };
    let mut items: Vec<String> = Vec::new();
    for key in &lost {
        let item = item_name(kind, key);
        if !items.contains(&item) {
            items.push(item);
        }
    }
    let shown = if items.len() > 4 {
        format!("{} and {} more", items[..3].join(", "), items.len() - 3)
    } else if items.len() > 1 {
        format!(
            "{} and {}",
            items[..items.len() - 1].join(", "),
            items[items.len() - 1]
        )
    } else {
        items[0].clone()
    };
    // Page 1's items are numbers ("items 36A and 37A"); 1702Q's schedule
    // items carry their schedule ("Schedule 2 item 6").
    let numbered = items
        .iter()
        .all(|item| item.starts_with(|c: char| c.is_ascii_digit()));
    let what = if !numbered {
        shown
    } else if items.len() == 1 {
        format!("item {shown}")
    } else {
        format!("items {shown}")
    };
    Some(json!({
        "message": format!("This answer {verb} what you typed in {what}. Continue?"),
        "done": done,
        "rows": n,
    }))
}

/// 1701Q item 11's month, day and year boxes are one date on the page
/// (`one_date`: the page shows "May 17, 1980" and writes the three boxes
/// together); the draft and the upload keep the three.
pub(super) fn mark_one_date(form: &mut Value, code: &str) {
    if code != "1701Q" {
        return;
    }
    let items = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten());
    for item in items {
        let keys: Vec<&str> = item["f"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|field| field["k"].as_str())
            .collect();
        if keys
            == [
                "frm1701q:txtBirthMonth",
                "frm1701q:txtBirthDay",
                "frm1701q:txtBirthYear",
            ]
        {
            item["one_date"] = Value::Bool(true);
        }
    }
}
