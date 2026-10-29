//! Penalties (rule 8): computed, but the filer can override them.
//!
//! A form on the shared penalty model (`bir_core::forms::penalty_items`)
//! binds its Surcharge, Interest and Compromise boxes to the draft's
//! per-component overrides (`<party>/penalty_overrides/surcharge`, ...):
//! with no override the box shows what the model computed from the due
//! date, and an override files instead. This is the one hook every such
//! form shares, read from the bindings:
//!
//! - The three boxes of a party are one group. While no override is set the
//!   group is locked and shows the computed amounts; the page's one Edit
//!   opens all three (it sends them as `unlocked`).
//! - Once an override is set the group stays open, and its Total Penalties
//!   and amount payable recompute from what was entered (the model does
//!   that). A blank box clears its override: "Use the computed amounts"
//!   blanks all three.
//!
//! - 0605 (decided 2026-10-17) has no due date to compute a penalty from,
//!   so its three boxes (20A-20C) are plain open boxes: no lock and no
//!   Edit. They still write the overrides, and 20D Total Penalties still
//!   adds them up. Every other form keeps its locked group and Edit.
//!
//! The groups, their locks and reasons are bir-core's
//! (`agent::draft_locks::penalties`, decision P4-19): the page's groups are
//! pinned there from this page's schema (`tests/agent_locks.rs`), so the
//! desktop assistant refuses an unedited computed group as the page locks
//! it. This module keeps the page's own UX: the group JSON its Edit reads,
//! the kept flag, and where a model's penalty message lands.
//!
//! Validation and the XML are untouched; the XML carries whatever the filer
//! entered, which is the point of the rule.

use bir_core::forms::agent::AgentFormKind;
pub(super) use bir_core::forms::agent::draft_locks::penalties::{
    EIGHT_PERCENT, Group, KEPT_ON_EIGHT_PERCENT, LOCK, NOT_ON_RETURN, applies, computed_groups,
    computes, edited, eight_percent, groups, kept_amount_path, path_of,
};
use serde_json::{Value, json};

/// The boxes a model's penalty message names (`item_17a_surcharge`): the
/// control the form declares for that item.
pub(super) fn error_keys(kind: AgentFormKind, field: &str) -> Vec<String> {
    use bir_core::forms::queueable::QueueableForm;
    let items: Vec<([&'static str; 3], [&'static str; 3])> = match kind {
        AgentFormKind::Queueable(kind) => bir_core::with_queueable_kind!(kind, F => {
            <F as QueueableForm>::penalty_items()
                .iter()
                .map(|items| (items.fields, items.controls))
                .collect()
        }),
        AgentFormKind::Form1601C => {
            let items = bir_core::forms::form_1601c::Form1601CDraft::penalty_items();
            vec![(items.fields, items.controls)]
        }
        AgentFormKind::Form2551Q => Vec::new(),
    };
    let named = |name: &str| {
        field == name
            || field.ends_with(&format!(".{name}"))
            || field.ends_with(&format!("/{name}"))
    };
    items
        .iter()
        .flat_map(|(fields, controls)| fields.iter().zip(controls))
        .filter(|(name, _)| named(name))
        .map(|(_, control)| control.to_string())
        .collect()
}

/// What the page needs: each computed group's boxes, whether the filer
/// entered its amounts, and whether it is part of the return (Edit is
/// offered). A group the model does not compute is not listed: its boxes
/// are plain open boxes.
pub(super) fn json(kind: AgentFormKind, draft: &Value) -> Value {
    Value::Array(
        computed_groups(kind)
            .iter()
            .map(|group| {
                json!({
                    "keys": group.keys,
                    "edited": edited(group, draft),
                    // A kept 8% amount opens with Edit so a blank can clear it.
                    "applies": applies(kind, group, draft)
                        || group.keys.iter().any(|key| kept_amount_path(kind, draft, key).is_some()),
                    "kept": kept(kind, group, draft),
                })
            })
            .collect(),
    )
}

/// Whether the group's boxes are still on the amounts the draft was stored
/// with, before its penalties were computed: a 1701Q (PR 5) or 1701 (PR 7)
/// column, an excise return stored before maintainer decision (excise penalties
/// run from the typed date) (PR 11), or a 1800 or
/// 1801 return stored before maintainer decision (1800/1801 penalties run from the due date).
pub(super) fn kept(kind: AgentFormKind, group: &Group, draft: &Value) -> bool {
    super::rows_income_q::kept_as_entered(kind, &group.keys, draft)
        || super::rows_annual_individual::kept_as_entered(kind, &group.keys, draft)
        || super::rows_excise::kept_as_entered(kind, &group.keys, draft)
}
