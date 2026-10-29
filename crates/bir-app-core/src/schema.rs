//! A form's on-screen layout, typed: parts -> items -> boxes, as the
//! compiled-in `forms-schema.json` holds it ([`crate::BirApp::form_layout`]).
//!
//! The JSON keeps its short names (`k`, `t`, `l`, ...); the structs here
//! name them. Hidden boxes (`t: "hidden"`) are left out of the layout:
//! nothing draws them.

use serde::{Deserialize, Deserializer, Serialize};

use crate::FieldOption;

/// One form's layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormLayout {
    #[serde(rename = "c")]
    pub code: String,
    #[serde(rename = "t")]
    pub title: String,
    #[serde(rename = "rev", default)]
    pub revision: String,
    /// `Monthly`, `Quarterly`, `Annual` or `As needed`.
    #[serde(rename = "freq", default)]
    pub frequency: String,
    #[serde(default)]
    pub pages: u32,
    /// How many boxes the form has.
    #[serde(rename = "n", default)]
    pub box_count: u32,
    #[serde(default)]
    pub parts: Vec<LayoutPart>,
}

/// A part of the form (`Part I · Background Information`, a schedule).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutPart {
    #[serde(rename = "n")]
    pub name: String,
    #[serde(default)]
    pub items: Vec<LayoutItem>,
}

/// One numbered item: its boxes, or a schedule table of rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutItem {
    /// The BIR item number (`"26"`, `"7A"`), empty when it has none.
    #[serde(default, deserialize_with = "null_default")]
    pub no: String,
    #[serde(rename = "l")]
    pub label: String,
    /// The boxes of a plain item.
    #[serde(rename = "f", default, deserialize_with = "null_default")]
    pub fields: Vec<LayoutField>,
    /// A schedule's columns; `keys` holds each row's box keys in this order.
    #[serde(default, deserialize_with = "null_default")]
    pub cols: Vec<LayoutColumn>,
    #[serde(default, deserialize_with = "null_default")]
    pub keys: Vec<Vec<String>>,
    /// The first row's number (schedules numbered from elsewhere).
    #[serde(default)]
    pub start: Option<u32>,
    #[serde(default)]
    pub row_labels: Option<RowLabels>,
    /// Rows are numbered (default true; false for labelled rows).
    #[serde(default)]
    pub numbered: Option<bool>,
    /// Month, day and year boxes typed as one date.
    #[serde(default)]
    pub one_date: bool,
    /// Why the item has no boxes.
    #[serde(default)]
    pub note: Option<String>,
    /// An amount elsewhere on the form this item can take.
    #[serde(default)]
    pub from: Option<FromHint>,
}

impl LayoutItem {
    /// A schedule table rather than a row of boxes.
    pub fn is_schedule(&self) -> bool {
        !self.cols.is_empty() && !self.keys.is_empty()
    }

    /// Every box key the item writes.
    pub fn box_keys(&self) -> Vec<String> {
        if self.is_schedule() {
            return self.keys.iter().flatten().cloned().collect();
        }
        self.fields
            .iter()
            .flat_map(|field| match &field.pool {
                Some(pool) => pool.keys.clone(),
                None => vec![field.key.clone()],
            })
            .collect()
    }
}

/// One box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutField {
    #[serde(rename = "k")]
    pub key: String,
    #[serde(rename = "l", default)]
    pub label: String,
    /// `money`, `int`, `pct`, `text`, `email`, `phone`, `date`, `sel`,
    /// `radio` or `chk`.
    #[serde(rename = "t")]
    pub kind: String,
    /// The most characters it takes.
    #[serde(rename = "m", default)]
    pub max_len: Option<u32>,
    /// Computed by the form, never typed.
    #[serde(rename = "c", default, deserialize_with = "flag")]
    pub computed: bool,
    #[serde(rename = "r", default, deserialize_with = "flag")]
    pub required: bool,
    /// Whole pesos only.
    #[serde(rename = "w", default, deserialize_with = "flag")]
    pub whole: bool,
    /// A drop-down's listed choices (the open return's `options` win).
    #[serde(rename = "o", default, deserialize_with = "options")]
    pub options: Vec<FieldOption>,
    /// A radio's group: picking one clears the others of the group.
    #[serde(rename = "grp", default)]
    pub group: Option<String>,
    /// How many radios the group has.
    #[serde(rename = "rg", default)]
    pub group_size: Option<u32>,
    /// One text spread over several boxes (an address).
    #[serde(default)]
    pub pool: Option<BoxPool>,
    /// Why the app leaves it blank.
    #[serde(default)]
    pub why: Option<String>,
}

/// A schedule column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutColumn {
    #[serde(rename = "l")]
    pub label: String,
    #[serde(rename = "t")]
    pub kind: String,
    #[serde(rename = "c", default, deserialize_with = "flag")]
    pub computed: bool,
    #[serde(rename = "w", default, deserialize_with = "flag")]
    pub whole: bool,
    #[serde(rename = "o", default, deserialize_with = "options")]
    pub options: Vec<FieldOption>,
}

/// The labels of a schedule's rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RowLabels {
    pub caption: String,
    pub rows: Vec<String>,
}

/// An amount another item computes that this item can take.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FromHint {
    pub k: String,
    pub l: String,
    pub to: String,
}

/// The boxes one pooled text fills, in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoxPool {
    pub keys: Vec<String>,
}

impl FormLayout {
    /// The layout without hidden boxes, empty items or empty parts.
    pub(crate) fn without_hidden(mut self) -> Self {
        for part in &mut self.parts {
            for item in &mut part.items {
                item.fields.retain(|field| field.kind != "hidden");
            }
            part.items.retain(|item| {
                (item.is_schedule() && item.cols.iter().any(|col| col.kind != "hidden"))
                    || !item.fields.is_empty()
                    || item.note.is_some()
            });
        }
        self.parts.retain(|part| !part.items.is_empty());
        self
    }
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `1`, `true` or a non-empty string: set.
fn flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Bool(on) => on,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        _ => false,
    })
}

/// A choice is `{value, label}` or a string: `"01 - January"` is value
/// `01`; a quoted string is a note, not a choice.
fn options<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<FieldOption>, D::Error> {
    let raw = Option::<Vec<serde_json::Value>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(raw
        .into_iter()
        .filter_map(|option| match option {
            serde_json::Value::String(text) => option_of_text(&text),
            other => serde_json::from_value(other).ok(),
        })
        .collect())
}

fn option_of_text(text: &str) -> Option<FieldOption> {
    if text.starts_with('"') {
        return None;
    }
    let value = match text.split_once(" - ") {
        Some((code, _)) if !code.contains(char::is_whitespace) && !code.is_empty() => code,
        _ => text,
    };
    Some(FieldOption {
        value: value.to_string(),
        label: text.to_string(),
        selected: None,
    })
}
