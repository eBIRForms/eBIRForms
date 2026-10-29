//! PR 11: the excise returns 2200A, 2200AN, 2200C, 2200M, 2200P, 2200S and
//! 2200T. Their page layout lives in app/forms/schema-overrides.json; this
//! module holds what the host does for them alone.
//!
//! - **Penalties from the typed date (decision D8, 2026-10-18).** The model
//!   measures their penalties from Item 1 (`PenaltyItems::typed_due_date`).
//!   A draft stored before that (penalty_model_version 1) keeps what it
//!   holds: its penalty boxes are "kept as entered before penalties were
//!   computed" until the filer chooses Edit penalties, and blanking them
//!   ("Use the computed amounts") puts the return on the typed due date.
//!   Nothing changes on open, so a stored draft saves back byte for byte.
//!   Maintainer decision (1800/1801 penalties run from the due date,
//!   2026-10-19) puts 1800 and 1801 on the same model (their
//!   due date: 30 days after the donation, one year after the death), so
//!   the kept-as-entered helpers (bir-core's `agent::draft_locks::excise`,
//!   with [`typed_due`]) take them too.
//! - **Printed item numbers (decision D6).** Where BIR's message names its
//!   program's item number and the sheet prints another, the page shows the
//!   printed one ([`PRINTED_MESSAGES`]); the checks and the filed data are
//!   unchanged.

use bir_core::forms::agent::draft_locks::excise::{before_d8, excise, typed_due};
use bir_core::forms::agent::{AgentFormKind, Binding};
use bir_core::forms::queueable::{QueueableForm, QueueableKind};
use bir_core::forms::{form_1800, form_1801};
use bir_core::forms::{form_2200a, form_2200an, form_2200c, form_2200m};
use bir_core::forms::{form_2200p, form_2200s, form_2200t};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

/// The lock of an excise return's penalty box while the draft predates
/// decision D8: the amounts are the ones it was stored with. bir-core's
/// (`agent::draft_locks::excise`, which also says which returns run their
/// penalties from a typed date), so the page and the assistant lock alike.
pub(super) const KEPT_AS_ENTERED: &str =
    bir_core::forms::agent::draft_locks::excise::KEPT_AS_ENTERED;

// [`KEPT_AS_ENTERED`] for a penalty box of an excise return stored before
// maintainer decision (excise penalties run from the typed date), or of a
// 1800 or 1801 return stored before maintainer decision (1800/1801
// penalties run from the due date); `None` for every other box and form
// (bir-core's).
pub(super) use bir_core::forms::agent::draft_locks::excise::penalty_lock;

/// Put the draft on its typed due date (`PenaltyItems::join_typed_due_date`).
/// The typed form only decides the new model version; the draft's JSON is
/// changed at that one key, so every other key it holds (one this build
/// does not know included) stays as it is.
#[inline(never)]
fn join_typed_due_date<F: QueueableForm + DeserializeOwned>(draft: &mut Value) {
    let Ok(mut form) = serde_json::from_value::<F>(draft.clone()) else {
        return;
    };
    for items in F::penalty_items() {
        items.join_typed_due_date(&mut form);
    }
    let Ok(joined) = serde_json::to_value(&form) else {
        return;
    };
    if let (Some(fields), Some(version)) =
        (draft.as_object_mut(), joined.get("penalty_model_version"))
    {
        fields.insert("penalty_model_version".to_string(), version.clone());
    }
}

/// Whether a penalty group's boxes are kept as entered: the page offers
/// "Use the computed amounts" as soon as Edit opens them.
pub(super) fn kept_as_entered(kind: AgentFormKind, keys: &[String], draft: &Value) -> bool {
    keys.iter()
        .any(|key| penalty_lock(kind, draft, key).is_some())
}

/// Put an excise return stored before D8 on the typed due date when this
/// batch blanked every box of one of its penalty groups ("Use the computed
/// amounts"), so the amounts computed from Item 1 file. One blank alone
/// keeps the stored model: that box files 0.00, as entered.
pub(super) fn join_if_group_blanked(
    kind: AgentFormKind,
    draft: &mut Value,
    groups: &[super::penalties::Group],
    blanked: &[String],
    rejected: &Map<String, Value>,
) {
    let Some(queueable) = typed_due(kind) else {
        return;
    };
    let whole = groups.iter().any(|group| {
        group
            .keys
            .iter()
            .all(|key| blanked.contains(key) && !rejected.contains_key(key))
    });
    if !whole || !before_d8(queueable, draft) {
        return;
    }
    match queueable {
        QueueableKind::Form1800 => join_typed_due_date::<form_1800::Form1800Draft>(draft),
        QueueableKind::Form1801 => join_typed_due_date::<form_1801::Form1801Draft>(draft),
        QueueableKind::Form2200A => join_typed_due_date::<form_2200a::Form2200ADraft>(draft),
        QueueableKind::Form2200AN => join_typed_due_date::<form_2200an::Form2200ANDraft>(draft),
        QueueableKind::Form2200C => join_typed_due_date::<form_2200c::Form2200CDraft>(draft),
        QueueableKind::Form2200M => join_typed_due_date::<form_2200m::Form2200MDraft>(draft),
        QueueableKind::Form2200P => join_typed_due_date::<form_2200p::Form2200PDraft>(draft),
        QueueableKind::Form2200S => join_typed_due_date::<form_2200s::Form2200SDraft>(draft),
        QueueableKind::Form2200T => join_typed_due_date::<form_2200t::Form2200TDraft>(draft),
        _ => {}
    }
}

/// A write to a penalty group that is not part of the return as it stands
/// (2200S Items 23A-23C off a prepayment): refused with that reason, since
/// Edit opens nothing there and the form would clear what was typed (a
/// blank says the same, rather than "choose Edit penalties"). `None` for
/// every other form and group.
pub(super) fn closed_penalty(
    kind: AgentFormKind,
    draft: &Value,
    group: &super::penalties::Group,
    _value: &Value,
) -> Option<&'static str> {
    excise(kind)?;
    (!super::penalties::applies(kind, group, draft)).then_some(super::penalties::NOT_ON_RETURN)
}

/// 2200S item 17 (Other Similar Schemes) is a tick box of its own on the
/// official page, grouped with 15 and 16 by the probe: unticking it writes
/// its own false option (other scheme off) rather than blanking the group.
pub(super) fn untick_writes_its_option(kind: AgentFormKind, key: &str) -> bool {
    kind.form_code() == "2200S" && key == "frm2200S:chkPymntManner_3"
}

/// What a gated box holds in the draft.
#[derive(Clone, Copy)]
enum Track {
    /// One box (a text or an amount): one entry while it shows a value.
    One,
    /// Schedule rows: each row with a box showing a value is one entry.
    Rows(&'static [Table]),
    /// The penalty overrides (2200S Items 23A-23C): each non-zero one is one
    /// entry. Locked and refused by the penalty rules, not by a gate.
    Overrides,
}

/// A schedule table: `(draft list, box stems, first row, last row)`; box
/// `{stem}{n}` sits in row `n`.
type Table = (&'static str, &'static [&'static str], u32, u32);

/// 2200C Part V Schedule 1.
const PART_V_2200C: &[Table] = &[(
    "/schedule",
    &[
        "CPPexmpt",
        "CPPexcise",
        "CPPnonexcise",
        "GRnetVat",
        "GRexciseNet",
        "GRexciseExmpt",
        "GRnonexcise",
    ],
    1,
    10,
)];

/// 2200S Schedule 1: the fixed beverage rows, then the Others rows.
const SCHEDULE_1_2200S: &[Table] = &[
    ("/beverages", &["txtSalesValue", "txtVolumeRemovals"], 0, 9),
    (
        "/others",
        &[
            "txtsched1Atc",
            "txtsched1Desc",
            "txtsched1TaxBracket",
            "txtsched1AppTaxRate",
            "txtsched1SalesValue",
            "txtsched1VolumeRemovals",
        ],
        0,
        11,
    ),
];

/// Boxes the form clears while another answer does not hold (its own
/// computation, `recompute`), with what turns them on. The host refuses a
/// typed value there with the tag, the page fades them, and an answer that
/// clears what the filer typed in them asks first.
struct Gate {
    /// What the question names (`item 15`, `Part V Schedule 1`).
    item: &'static str,
    /// The tag (the page's Needs line and the refusal).
    tag: &'static str,
    keys: Vec<String>,
    track: Track,
}

/// The keys of `tables`, row by row (the first is the gate's probe).
fn table_keys(prefix: &str, tables: &[Table]) -> Vec<String> {
    tables
        .iter()
        .flat_map(|(_, stems, first, last)| {
            (*first..=*last)
                .flat_map(move |n| stems.iter().map(move |stem| format!("{prefix}{stem}{n}")))
        })
        .collect()
}

/// The row of `tables` box `key` sits in (`/others/3`).
fn row_of(tables: &[Table], key: &str) -> Option<String> {
    let tail = key.rsplit(':').next()?;
    tables.iter().find_map(|(list, stems, first, last)| {
        stems.iter().find_map(|stem| {
            let n: u32 = tail.strip_prefix(stem)?.parse().ok()?;
            (*first..=*last).contains(&n).then(|| format!("{list}/{n}"))
        })
    })
}

const NEEDS_12: &str = "Needs item 12 = Yes";
const NEEDS_15: &str = "Needs item 15 = Other Similar Schemes";
const NEEDS_AMENDED: &str = "Needs Amended Return = Yes";

fn gate(item: &'static str, tag: &'static str, keys: Vec<String>, track: Track) -> Gate {
    Gate {
        item,
        tag,
        keys,
        track,
    }
}

/// The gates of an excise return, read off each form's `recompute` (what
/// it clears and on which answer).
///
/// Three of them are enabled at load in the official program
/// (rules/forms/<form>/fixtures/runtime-control-inventory-v796.json):
/// 2200T item 15 (`paymentOther`), 2200C Part V Schedule 1 (`CPPexmpt1`
/// and the rest) and 2200S Schedule 1 (`txtsched1Atc0`,
/// `txtsched1SalesValue0`). The page there takes a value before the answer;
/// the port's recompute clears it unless that answer holds, so the host
/// keeps them shut until it does rather than take a value and lose it.
fn gates(code: &str) -> Vec<Gate> {
    let one = |prefix: &str, key: &str| vec![format!("{prefix}{key}")];
    match code {
        "2200A" | "2200T" => {
            let p = if code == "2200A" {
                "frm2200Av2020:"
            } else {
                "frm2200Tv2020:"
            };
            vec![
                gate("item 12A", NEEDS_12, one(p, "treatyY"), Track::One),
                // 2200T: enabled at load officially; the port's recompute
                // clears it unless item 15 = Other Similar Schemes.
                gate("item 15", NEEDS_15, one(p, "paymentOther"), Track::One),
                gate(
                    "item 19",
                    NEEDS_AMENDED,
                    one(p, "txtPrevReturn"),
                    Track::One,
                ),
            ]
        }
        "2200AN" => {
            let p = "frm2200ANv2018:";
            vec![
                gate("item 12A", NEEDS_12, one(p, "lstSpecialTax"), Track::One),
                gate(
                    "item 15",
                    NEEDS_15,
                    one(p, "MannerOfPaymentOthers"),
                    Track::One,
                ),
                gate("item 19", NEEDS_AMENDED, one(p, "txt19"), Track::One),
            ]
        }
        "2200C" => {
            let p = "frm2200C:";
            vec![
                gate(
                    "item 12A",
                    NEEDS_12,
                    one(p, "txtPg1I12TaxReliefSpecify"),
                    Track::One,
                ),
                gate(
                    "item 15",
                    NEEDS_15,
                    one(p, "txtPg1Pt2MOPOtherDesc"),
                    Track::One,
                ),
                gate(
                    "item 19",
                    NEEDS_AMENDED,
                    one(p, "txtPg1P3I19PmntOnRtrnPrevFiled"),
                    Track::One,
                ),
                // Enabled at load officially; the port's recompute clears it
                // unless item 13 or item 15 is ticked (resetPartFive).
                gate(
                    "Part V Schedule 1",
                    "Needs item 13 or item 15",
                    table_keys(p, PART_V_2200C),
                    Track::Rows(PART_V_2200C),
                ),
            ]
        }
        "2200M" | "2200P" => {
            let p = if code == "2200M" {
                "frm2200M:"
            } else {
                "frm2200P:"
            };
            vec![
                gate(
                    "item 12A",
                    NEEDS_12,
                    one(p, "txtPg1I12TaxReliefSpecify"),
                    Track::One,
                ),
                gate(
                    "item 15",
                    NEEDS_15,
                    one(p, "txtPg1Pt2MOPOtherDesc"),
                    Track::One,
                ),
                gate(
                    "item 19",
                    NEEDS_AMENDED,
                    one(p, "txtPg1P3I19PmntOnRtrnPrevFiled"),
                    Track::One,
                ),
            ]
        }
        "2200S" => {
            let p = "frm2200S:";
            vec![
                // The text is kept only with item 17 ticked, and item 17
                // only on a prepayment (item 16): recompute clears both
                // off it.
                gate(
                    "item 17",
                    "Needs item 16 = Prepayment and item 17 = Other Similar Schemes",
                    one(p, "txtOthMannerofPymnt"),
                    Track::One,
                ),
                gate("item 21", NEEDS_AMENDED, one(p, "txtTax19"), Track::One),
                // Enabled at load officially; the port's recompute clears it
                // unless item 15 = Payment on Actual Removal.
                gate(
                    "Schedule 1",
                    "Needs item 15 = Payment on Actual Removal",
                    table_keys(p, SCHEDULE_1_2200S),
                    Track::Rows(SCHEDULE_1_2200S),
                ),
                gate(
                    "items 23A-23C",
                    "",
                    vec![
                        format!("{p}txtTax21A"),
                        format!("{p}txtTax21B"),
                        format!("{p}txtTax21C"),
                    ],
                    Track::Overrides,
                ),
            ]
        }
        _ => Vec::new(),
    }
}

/// Whether a gated box takes amounts (its binding, or 2200S's Others rate,
/// which `catalogs` writes).
fn is_amount(kind: AgentFormKind, key: &str) -> bool {
    key.starts_with("frm2200S:txtsched1AppTaxRate")
        || matches!(
            kind.definition()
                .bindings
                .get(key)
                .map(|bound| &bound.binding),
            Some(Binding::Amount { .. } | Binding::Whole { .. })
        )
}

/// Whether `text` is the box's own empty value: blank, or on an amount box
/// zero (`0`, `0.00`, what it shows cleared). A text box's `0` is a value.
fn empty(kind: AgentFormKind, key: &str, text: &str) -> bool {
    let text = text.trim();
    text.is_empty()
        || (is_amount(kind, key) && bir_core::forms::agent::parse_amount(text) == Some(0.0))
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Whether `key` is a gated box of `kind` (the penalty group aside).
fn gated(kind: AgentFormKind, key: &str) -> Option<Gate> {
    gates(kind.form_code())
        .into_iter()
        .find(|gate| !matches!(gate.track, Track::Overrides) && gate.keys.iter().any(|k| k == key))
}

/// Gated boxes run after the answers they wait for in one batch.
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    gated(kind, key).map(|_| 1_000)
}

/// Why the host refuses `value` written to `key` (already applied in
/// `next`), if it does: 2200AN item 3 takes two digits (the official box's
/// maxlength), and a gated box whose answer does not hold keeps nothing
/// (the form's computation clears it), so the write is refused with what
/// turns it on, never taken and lost. Only the box's own empty value (a
/// blank; an amount box's zero) passes unasked. `None` for every other
/// form.
pub(super) fn refusal(
    kind: AgentFormKind,
    next: &Value,
    key: &str,
    value: &Value,
) -> Option<String> {
    excise(kind)?;
    let typed = text_of(value);
    if key == "frm2200ANv2018:txtSheets"
        && typed
            .trim()
            .parse::<u64>()
            .map_or(typed.trim().len() > 2, |sheets| sheets > 99)
    {
        return Some("Item 3 holds up to two digits".to_string());
    }
    let gate = gated(kind, key)?;
    if empty(kind, key, &typed) {
        return None;
    }
    let mut copy = next.clone();
    bir_core::forms::agent::compute(kind, &mut copy).ok()?;
    let shown = bir_core::forms::agent::field_map(kind, &copy).ok()?;
    empty(kind, key, shown.get(key).map_or("", String::as_str)).then(|| {
        super::conditions::tag(kind.form_code(), key).unwrap_or_else(|| gate.tag.to_string())
    })
}

/// The gated boxes off right now, each with its tag, for the page to fade
/// (rule 6). A gate is off when the form clears a probe value typed into
/// its first box; a box that holds a value is never reported off.
pub(super) fn off(kind: AgentFormKind, draft: &Value, values: &Value) -> Map<String, Value> {
    let mut off = Map::new();
    if excise(kind).is_none() {
        return off;
    }
    for gate in gates(kind.form_code()) {
        if matches!(gate.track, Track::Overrides) {
            continue;
        }
        let probe = &gate.keys[0];
        let shown = values[probe.as_str()].as_str().unwrap_or_default();
        if !empty(kind, probe, shown) || !super::conditions::cleared(kind, draft, probe, shown) {
            continue;
        }
        for key in &gate.keys {
            if empty(kind, key, values[key.as_str()].as_str().unwrap_or_default()) {
                off.insert(
                    key.clone(),
                    json!(
                        super::conditions::tag(kind.form_code(), key)
                            .unwrap_or_else(|| gate.tag.to_string())
                    ),
                );
            }
        }
    }
    off
}

/// Whether the penalty override behind `key` holds a typed amount.
fn override_typed(kind: AgentFormKind, draft: &Value, key: &str) -> bool {
    super::penalties::path_of(kind, key)
        .and_then(|path| draft.pointer(&path).and_then(Value::as_f64))
        .is_some_and(|amount| amount != 0.0)
}

/// The printed item a box sits under on the page (`21A`).
fn item_of(code: &str, key: &str) -> Option<String> {
    super::schema_entry(code)?["parts"]
        .as_array()?
        .iter()
        .flat_map(|part| part["items"].as_array().into_iter().flatten())
        .find(|item| {
            item["f"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|f| f["k"] == key))
        })
        .and_then(|item| item["no"].as_str().map(str::to_string))
}

fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What an edit removed or replaced that the filer typed, for the page to
/// ask first and offer Undo (the `removes` of the fill response):
/// - an answer that turns a gate off clears what was typed behind it
///   (`gates`: item 12A, 15 or 19, 2200S item 17 and Schedule 1, 2200C Part
///   V, 2200S Items 23A-23C off a prepayment). Every box that showed a value
///   and no longer does counts, except the boxes the batch writes itself (a
///   box the filer blanked is theirs); a schedule counts its rows;
/// - "Use the computed amounts" (a whole penalty group blanked) replaces the
///   amounts typed there, or kept from before decision D8 (the 0.00 says
///   nothing), as 1701 does.
pub(super) fn removes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    excise(kind)?;
    let code = kind.form_code();
    let shown = |draft: &Value| bir_core::forms::agent::field_map(kind, draft).unwrap_or_default();
    let (was, now) = (shown(before), shown(after));
    let holds = |map: &std::collections::BTreeMap<String, String>, key: &str| {
        !empty(kind, key, map.get(key).map_or("", String::as_str))
    };
    let mut removed: Vec<String> = Vec::new();
    let mut count = 0;
    for gate in gates(code) {
        let gone: Vec<&String> = gate
            .keys
            .iter()
            .filter(|key| !edits.contains_key(key.as_str()))
            .filter(|key| match gate.track {
                Track::Overrides => {
                    override_typed(kind, before, key) && !override_typed(kind, after, key)
                }
                _ => holds(&was, key) && !holds(&now, key),
            })
            .collect();
        let entries = match gate.track {
            Track::Rows(tables) => gone
                .iter()
                .filter_map(|key| row_of(tables, key))
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            _ => gone.len(),
        };
        if entries == 0 {
            continue;
        }
        count += entries;
        removed.push(match gate.track {
            Track::Rows(_) => format!(
                "{} ({entries} row{})",
                gate.item,
                if entries == 1 { "" } else { "s" }
            ),
            _ => gate.item.to_string(),
        });
    }
    let mut replaced: Vec<String> = Vec::new();
    for group in super::penalties::groups(kind) {
        let blanked = group.keys.iter().all(|key| {
            edits
                .get(key)
                .is_some_and(|value| text_of(value).trim().is_empty())
        });
        if !blanked {
            continue;
        }
        for key in &group.keys {
            let Some(path) = super::penalties::path_of(kind, key) else {
                continue;
            };
            let typed = before
                .pointer(&path)
                .and_then(Value::as_f64)
                .is_some_and(|amount| amount != 0.0);
            if typed && after.pointer(&path).is_none_or(Value::is_null) {
                count += 1;
                replaced.push(format!(
                    "item {}",
                    item_of(code, key).unwrap_or_else(|| key.clone())
                ));
            }
        }
    }
    if count == 0 {
        return None;
    }
    let verb = match (removed.is_empty(), replaced.is_empty()) {
        (false, true) => "removes",
        (true, false) => "replaces",
        _ => "removes and replaces",
    };
    replaced.sort();
    let names: Vec<String> = removed.into_iter().chain(replaced).collect();
    Some(json!({
        "message": format!("This answer {verb} what you typed in {}. Continue?", listed(&names)),
        "done": if count == 1 { "Removed 1 entry".to_string() } else { format!("Removed {count} entries") },
        "rows": count,
    }))
}

/// Decision D6: `(form, BIR's message, the same message naming the printed
/// item)` where the official program numbers an item differently from the
/// sheet (html-frozen/<slug>/writer-cells.json captions). 2200AN's checks
/// still use the 2013 layout's numbers; 2200M's and 2200P's are 2200C's.
pub(super) const PRINTED_MESSAGES: &[(&str, &str, &str)] = &[
    (
        "2200AN",
        "Please enter a valid TIN number on Item 5.",
        "Please enter a valid TIN number on Item 4.",
    ),
    (
        "2200AN",
        "Please enter a valid RDO Code on Item 6.",
        "Please enter a valid RDO Code on Item 5.",
    ),
    (
        "2200AN",
        "Please enter a valid Taxpayer Name on Item 8.",
        "Please enter a valid Taxpayer Name on Item 6.",
    ),
    (
        "2200AN",
        "Please enter Taxpayer's Registered Address on Item 10.",
        "Please enter Taxpayer's Registered Address on Item 7.",
    ),
    (
        "2200AN",
        "Please enter Taxpayer's Zip Code on Item 11.",
        "Please enter Taxpayer's Zip Code on Item 7A.",
    ),
    (
        "2200AN",
        "Please enter a valid Telephone Number on Item 9.",
        "Please enter a valid Telephone Number on Item 8.",
    ),
    (
        "2200M",
        "Please enter a valid RDO Code on Page 1 Item 6.",
        "Please enter a valid RDO Code on Page 1 Item 5.",
    ),
    (
        "2200M",
        "Name field on Page 1 Item 7 is required.",
        "Name field on Page 1 Item 6 is required.",
    ),
    (
        "2200M",
        "Registered Address field on Page 1 Item 8 is required.",
        "Registered Address field on Page 1 Item 7 is required.",
    ),
    (
        "2200M",
        "Zip Code field on Page 1 Item 8A is required.",
        "Zip Code field on Page 1 Item 7A is required.",
    ),
    (
        "2200M",
        "Contact Number field on Page 1 Item 9 is required.",
        "Contact Number field on Page 1 Item 8 is required.",
    ),
    (
        "2200M",
        "E-mail address on page 1 item 10 is required.",
        "E-mail address on page 1 item 9 is required.",
    ),
    (
        "2200M",
        "Please enter a valid e-mail address on page 1 item 10",
        "Please enter a valid e-mail address on page 1 item 9",
    ),
    (
        "2200P",
        "Please enter a valid RDO Code on Page 1 Item 6.",
        "Please enter a valid RDO Code on Page 1 Item 5.",
    ),
    (
        "2200P",
        "Name field on Page 1 Item 7 is required.",
        "Name field on Page 1 Item 6 is required.",
    ),
    (
        "2200P",
        "Registered Address field on Page 1 Item 8 is required.",
        "Registered Address field on Page 1 Item 7 is required.",
    ),
    (
        "2200P",
        "Zip Code field on Page 1 Item 8A is required.",
        "Zip Code field on Page 1 Item 7A is required.",
    ),
    (
        "2200P",
        "Contact Number field on Page 1 Item 9 is required.",
        "Contact Number field on Page 1 Item 8 is required.",
    ),
    (
        "2200P",
        "E-mail address on page 1 item 10 is required.",
        "E-mail address on page 1 item 9 is required.",
    ),
    (
        "2200P",
        "Please enter a valid e-mail address on page 1 item 10",
        "Please enter a valid e-mail address on page 1 item 9",
    ),
];

/// The printed-number wording of BIR's `message` on `kind`, if D6
/// rewrites it.
pub(super) fn printed_message(kind: AgentFormKind, message: &str) -> Option<&'static str> {
    let code = kind.form_code();
    PRINTED_MESSAGES
        .iter()
        .find(|(form, official, _)| *form == code && *official == message)
        .map(|(_, _, printed)| *printed)
}
