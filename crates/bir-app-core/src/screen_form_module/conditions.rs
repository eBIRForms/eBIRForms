//! Boxes another answer turns off (rule 6): the page fades them and says
//! what turns them on; they come alive when the condition holds. The checks
//! themselves are not touched.
//!
//! Which boxes another answer can turn off is the official corpus's
//! `enabled_when` (`rules/forms/<package>/fields.json`, read only), copied
//! for the boxes the page shows into `app/forms/conditions.json` by the
//! ignored test `write_conditions`; `conditions_match_the_rules_corpus`
//! keeps the copy in step. The corpus text is for developers (it records
//! official defects too), so the page never shows it: each condition has a
//! short reviewed tag in `app/forms/condition-labels.json` ("Needs item 13 =
//! Yes"), which says what the form's own computation needs where that is
//! more than the corpus says (0619F's items also need taxes withheld).
//!
//! Whether a box is off right now is the form's own computation: the host
//! types a value into it on a copy of the draft, recomputes, and the box is
//! off when the computation clears it again (1601C clears Item 28 unless the
//! return is amended). A box that holds a value, or one that takes none of
//! the probe values, is never reported off.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use bir_core::forms::agent::{self, AgentFormKind, Binding, DatePart};
use serde_json::{Map, Value, json};

/// `app/forms/conditions.json`: form code -> key -> the corpus condition.
fn conditions() -> &'static Value {
    static CONDITIONS: OnceLock<Value> = OnceLock::new();
    CONDITIONS.get_or_init(|| {
        serde_json::from_str(&bir_core::packed::inflate(bir_core::packed_bytes!(
            "assets/forms/conditions.json"
        )))
        .expect("app/forms/conditions.json is a JSON object")
    })
}

/// `app/forms/condition-labels.json`: form code -> corpus condition -> the
/// tag the page shows.
fn labels() -> &'static Value {
    static LABELS: OnceLock<Value> = OnceLock::new();
    LABELS.get_or_init(|| {
        serde_json::from_str(&bir_core::packed::inflate(bir_core::packed_bytes!(
            "assets/forms/condition-labels.json"
        )))
        .expect("app/forms/condition-labels.json is a JSON object")
    })
}

/// A value shown as nothing: blank, all zeros (`0.00`, `00`) or unticked.
fn is_blank(shown: &str) -> bool {
    let shown = shown.trim();
    shown == "false" || shown.chars().all(|c| matches!(c, '0' | '.' | ',' | '-'))
}

/// The binding a box writes through: its own, or its radio group's.
fn binding_of(kind: AgentFormKind, key: &str) -> Option<&'static Binding> {
    let def = kind.definition();
    let entry = def.box_def(key)?;
    def.bindings
        .get(key)
        .or_else(|| {
            entry
                .group
                .as_deref()
                .and_then(|group| def.bindings.get(group))
        })
        .map(|bound| &bound.binding)
}

/// The values tried on a box, in order, to see whether the form keeps one:
/// the first one the box takes decides. The tests use the same list.
pub(super) fn probes(kind: AgentFormKind, key: &str, shown: &str) -> Vec<Value> {
    let grouped = kind
        .definition()
        .box_def(key)
        .is_some_and(|entry| entry.group.is_some());
    match binding_of(kind, key) {
        None => Vec::new(),
        Some(Binding::Amount { .. } | Binding::Whole { .. }) => vec![json!("1"), json!("1000")],
        Some(Binding::Text {
            part: Some(part), ..
        }) => vec![json!(match part {
            DatePart::Month => "01",
            DatePart::Day => "15",
            DatePart::Year => "2020",
        })],
        Some(Binding::Text { date: Some(_), .. }) => vec![json!("01/15/2020")],
        Some(Binding::Text { .. }) => vec![json!("A"), json!("01"), json!("2020")],
        // A radio of a group is ticked by its own key.
        Some(Binding::Choice { .. }) if grouped => vec![json!("true")],
        Some(Binding::Choice { options }) => options
            .iter()
            .map(|option| option.value.as_str())
            .filter(|value| *value != shown && !is_blank(value))
            .take(2)
            .map(|value| json!(value))
            .collect(),
    }
}

/// The tag for a box another answer turns off: the box's own where a
/// condition covers columns that need different answers (1701Q's taxpayer
/// and spouse columns), else its condition's.
fn label(code: &str, key: &str, condition: &Value) -> Value {
    let text = condition.as_str().unwrap_or_default();
    labels()["keys"][code][key]
        .as_str()
        .or_else(|| labels()["conditions"][code][text].as_str())
        .map_or_else(
            || json!("Needs another answer on this form"),
            |label| json!(label),
        )
}

/// The tag of `code`'s box `key` when the corpus gives it a condition
/// (PR 9's and PR 11's host gates refuse a write with it).
pub(super) fn tag(code: &str, key: &str) -> Option<String> {
    let condition = conditions()[code].get(key)?;
    label(code, key, condition).as_str().map(str::to_string)
}

/// Whether the form clears the first probe value typed into `key` alone
/// (PR 11's host gates).
pub(super) fn cleared(kind: AgentFormKind, draft: &Value, key: &str, shown: &str) -> bool {
    off_alone(kind, draft, key, shown)
}

/// Fill `edits` on a copy of `draft` the way the page does, recompute, and
/// give each key's shown value, or `None` for a key the copy refused.
fn trial(
    kind: AgentFormKind,
    draft: &Value,
    edits: &[(String, Value)],
) -> BTreeMap<String, Option<String>> {
    let mut copy = draft.clone();
    let mut taken = BTreeMap::new();
    for (key, value) in edits {
        let mut next = copy.clone();
        let ok = super::catalogs::fill(kind, &mut next, key, value, super::catalogs::Lift::Nothing)
            .is_ok();
        if ok {
            copy = next;
        }
        taken.insert(key.clone(), ok);
    }
    let shown = agent::compute(kind, &mut copy)
        .ok()
        .and_then(|()| agent::field_map(kind, &copy).ok())
        .unwrap_or_default();
    taken
        .into_iter()
        .map(|(key, ok)| {
            let value = ok.then(|| shown.get(&key).cloned().unwrap_or_default());
            (key, value)
        })
        .collect()
}

/// Whether one box is off: the first probe it takes is cleared by the form.
fn off_alone(kind: AgentFormKind, draft: &Value, key: &str, shown: &str) -> bool {
    for probe in probes(kind, key, shown) {
        if let Some(Some(after)) = trial(kind, draft, &[(key.to_string(), probe)]).remove(key) {
            return is_blank(&after);
        }
    }
    false
}

/// The boxes of `draft` that another answer turns off right now, each with
/// its tag. `values` is what the page shows for the draft.
///
/// Typed boxes (amounts, numbers, text) are tried together in one trial
/// first: a box the form keeps there is on. Only the ones it clears, and
/// the choices (a choice may be the answer another box waits for), are
/// tried one at a time, which decides.
///
/// Where this could differ from trying each box alone (the tests check
/// that it does not, across many answers on every form):
/// - a typed box in the trial that is itself what another typed box waits
///   for (an amount that turns an item negative, say) would show the other
///   one on although alone it is off; today such boxes are choices, which
///   are never in the trial;
/// - a typed box that one of the trial's boxes turns off would be tried
///   again alone, which only costs time.
pub(super) fn off_boxes(kind: AgentFormKind, draft: &Value, values: &Value) -> Map<String, Value> {
    let mut off = Map::new();
    let code = kind.form_code();
    // 1800 and 1801 (PR 10, rows_donor_estate.rs): boxes the host itself
    // closes while their answer does not hold (it refuses what is typed
    // there) are off whatever they show, with their own tag.
    let donor_estate_gated = super::rows_donor_estate::gated_keys(kind);
    for (key, tag) in super::rows_donor_estate::closed(kind, draft) {
        off.insert(key, json!(tag));
    }
    let Some(list) = conditions()[code].as_object() else {
        // A form without corpus conditions may still have PR 9 or PR 11
        // host gates.
        off.extend(super::rows_as_needed::off(kind, draft, values));
        off.extend(super::rows_excise::off(kind, draft, values));
        return off;
    };
    // A box the host itself closes while its answer does not hold (it
    // refuses what is typed there: rows_withholding.rs, rows_monthly.rs,
    // rows_income_q.rs (1701Q's spouse part and 1702Q's schedules) and
    // rows_annual_individual.rs (PR 7: the annual individual returns' spouse
    // part)) is off whatever it shows: a legacy draft's value there is
    // faded, not hidden.
    let mut gated = std::collections::BTreeSet::new();
    for (key, condition) in list {
        if donor_estate_gated.contains(key) {
            gated.insert(key);
        } else if let Some(closed) = super::rows_withholding::gated(kind, draft, key)
            .or_else(|| super::rows_monthly::gated(kind, draft, key))
        {
            gated.insert(key);
            if closed {
                off.insert(key.clone(), label(code, key, condition));
            }
        } else if let Some(tag) = super::rows_income_q::gated(kind, draft, key) {
            gated.insert(key);
            off.insert(key.clone(), json!(tag));
        } else if super::rows_annual_individual::gated(kind, draft, key) {
            gated.insert(key);
            off.insert(key.clone(), label(code, key, condition));
        } else if super::rows_annual_individual::open_by_gate(kind, draft, key) {
            gated.insert(key);
        }
    }
    let blank: Vec<(&String, &Value, &str)> = list
        .iter()
        .filter(|(key, _)| !gated.contains(key))
        .map(|(key, condition)| (key, condition, values[key].as_str().unwrap_or_default()))
        .filter(|(_, _, shown)| is_blank(shown))
        .collect();
    let typed: Vec<(String, Value)> = blank
        .iter()
        .filter(|(key, _, _)| !matches!(binding_of(kind, key), Some(Binding::Choice { .. })))
        .filter_map(|(key, _, shown)| {
            probes(kind, key, shown)
                .into_iter()
                .next()
                .map(|probe| (key.to_string(), probe))
        })
        .collect();
    let together = trial(kind, draft, &typed);
    for (key, condition, shown) in blank {
        let kept_together = together
            .get(key)
            .is_some_and(|after| after.as_deref().is_some_and(|after| !is_blank(after)));
        if !kept_together && off_alone(kind, draft, key, shown) {
            off.insert(key.clone(), label(code, key, condition));
        }
    }
    // PR 9's host gates refuse a write while shut (the probe above sees a
    // refusal, not a clearing): rows_as_needed.rs says which are off.
    off.extend(super::rows_as_needed::off(kind, draft, values));
    // PR 11's host gates (rows_excise.rs), where the corpus names none.
    for (key, tag) in super::rows_excise::off(kind, draft, values) {
        off.entry(key).or_insert(tag);
    }
    // In the conditions' own order, gated boxes among the rest; a host-gated
    // box the conditions do not list (PR 9's group keys) follows them.
    let mut ordered: Map<String, Value> = list
        .keys()
        .filter_map(|key| off.remove(key).map(|tag| (key.clone(), tag)))
        .collect();
    ordered.extend(off);
    ordered
}
