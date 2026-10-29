//! Form templates: exactly one per form per branch (D7), storing setup
//! answers only (D8). Validation lives here so every write path (create,
//! update, create-from-filed, bulk copy) enforces the same rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::TaxProfileError;
use super::field_kinds::{FieldKind, FormFieldKinds, ResolvedPeriod, TemplatableKind};

/// One templatable field in a template. Absent fields are "off".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemplateField {
    /// The value is stored and prefilled.
    Kept { value: serde_json::Value },
    /// No value; a guided-mode stop (D13/D14).
    Marked,
}

/// How the template came to exist. Persisted as `filed:<draft id>`,
/// `return:<draft id>`, `blank` or `copy:<from full TIN>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSource {
    Filed {
        draft_id: i64,
    },
    /// Snapshot of a return that was not filed (Draft or Queued) at the time.
    Return {
        draft_id: i64,
    },
    Blank,
    Copy {
        from_tin: String,
    },
}

impl TemplateSource {
    pub fn to_db(&self) -> String {
        match self {
            Self::Filed { draft_id } => format!("filed:{draft_id}"),
            Self::Return { draft_id } => format!("return:{draft_id}"),
            Self::Blank => "blank".to_string(),
            Self::Copy { from_tin } => format!("copy:{from_tin}"),
        }
    }

    pub fn from_db(value: &str) -> Result<Self, TaxProfileError> {
        if value == "blank" {
            return Ok(Self::Blank);
        }
        if let Some(id) = value.strip_prefix("filed:")
            && let Ok(draft_id) = id.parse()
        {
            return Ok(Self::Filed { draft_id });
        }
        if let Some(id) = value.strip_prefix("return:")
            && let Ok(draft_id) = id.parse()
        {
            return Ok(Self::Return { draft_id });
        }
        if let Some(from_tin) = value.strip_prefix("copy:")
            && !from_tin.is_empty()
        {
            return Ok(Self::Copy {
                from_tin: from_tin.to_string(),
            });
        }
        Err(TaxProfileError::Invalid(format!(
            "Unknown template source `{value}`"
        )))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FormTemplate {
    pub id: i64,
    /// Full TIN of the branch.
    pub tin: String,
    pub form_code: String,
    pub fields: BTreeMap<String, TemplateField>,
    pub source: TemplateSource,
    pub created_at: String,
    pub updated_at: String,
}

impl FormTemplate {
    pub fn has_marked_fields(&self) -> bool {
        self.fields
            .values()
            .any(|field| matches!(field, TemplateField::Marked))
    }
}

/// What the user chose for a field of a filed return in template mode
/// (D15: keep value -> mark -> off; "off" is simply not listed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateChoice {
    Keep,
    Mark,
}

/// Why a field cannot be stored in a template.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateFieldError {
    #[error("Form {0} has no field classification yet; templates are unavailable for it")]
    FormNotClassified(String),
    #[error("`{field}` is not a field of form {form_code}")]
    UnknownField { form_code: String, field: String },
    #[error("`{0}` is filled automatically and cannot be kept or marked")]
    AutoField(String),
    #[error("`{0}` is an amount; amounts can be marked but never kept")]
    AmountKept(String),
    #[error("A blank template can only mark fields; `{0}` has a kept value")]
    KeptOnBlankTemplate(String),
}

/// Enforce D8/D18 for one form: only registered templatable fields, never a
/// kept amount, and a blank-source template only marks.
pub fn validate_template_fields(
    form_code: &str,
    fields: &BTreeMap<String, TemplateField>,
    source: &TemplateSource,
) -> Result<&'static FormFieldKinds, TemplateFieldError> {
    let kinds = super::field_kinds::field_kinds_for(form_code)
        .ok_or_else(|| TemplateFieldError::FormNotClassified(form_code.to_string()))?;
    for (field, value) in fields {
        match kinds.kind_of(field) {
            None => {
                return Err(TemplateFieldError::UnknownField {
                    form_code: kinds.form_code.to_string(),
                    field: field.clone(),
                });
            }
            Some(FieldKind::Auto(_)) => return Err(TemplateFieldError::AutoField(field.clone())),
            Some(FieldKind::Templatable(kind)) => {
                if let TemplateField::Kept { .. } = value {
                    if kind == TemplatableKind::Amount {
                        return Err(TemplateFieldError::AmountKept(field.clone()));
                    }
                    if *source == TemplateSource::Blank {
                        return Err(TemplateFieldError::KeptOnBlankTemplate(field.clone()));
                    }
                }
            }
        }
    }
    Ok(kinds)
}

/// Read `schedule_1[0].atc`-style paths out of a stored draft.
pub(crate) fn value_at_path<'a>(
    draft: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = draft;
    for segment in path.split('.') {
        let (name, index) = match segment.split_once('[') {
            Some((name, rest)) => (name, Some(rest.strip_suffix(']')?.parse::<usize>().ok()?)),
            None => (segment, None),
        };
        current = current.get(name)?;
        if let Some(index) = index {
            current = current.get(index)?;
        }
    }
    Some(current)
}

/// D8/D17 snapshot of a stored return: every **setup** answer the return
/// holds, kept with its value. Amounts, Auto (period, identity) and computed
/// fields are dropped, as are null and empty-string values.
pub fn snapshot_setup_answers(
    kinds: &FormFieldKinds,
    draft: &serde_json::Value,
) -> BTreeMap<String, TemplateField> {
    let mut fields = BTreeMap::new();
    for (id, kind) in kinds.templatable_fields() {
        if kind != TemplatableKind::Setup {
            continue;
        }
        let mut concrete = Vec::new();
        match id.split_once("[].") {
            Some((list, field)) => {
                let rows = draft
                    .get(list)
                    .and_then(serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                for index in 0..rows {
                    concrete.push(format!("{list}[{index}].{field}"));
                }
            }
            None => concrete.push(id.to_string()),
        }
        for field in concrete {
            let Some(value) = value_at_path(draft, &field) else {
                continue;
            };
            let blank =
                value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty());
            if !blank {
                fields.insert(
                    field,
                    TemplateField::Kept {
                        value: value.clone(),
                    },
                );
            }
        }
    }
    fields
}

/// One stop of guided mode, in order (D14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuidedStop {
    Marked(String),
    RequiredEmpty(String),
    Save,
}

/// What opening a form from the library applies, in order: Auto values, then
/// kept values, then guided mode (spec "Template mode and creation").
#[derive(Debug, Clone, PartialEq)]
pub struct PrefillPlan {
    pub tin: String,
    pub period: ResolvedPeriod,
    /// Period and identity values, typed as the draft stores them. Computed
    /// fields are not listed; the form computes them.
    pub auto_values: BTreeMap<String, serde_json::Value>,
    /// Field ids that are Auto and therefore locked on the form (D18),
    /// including computed ones.
    pub locked_fields: Vec<String>,
    pub kept_values: BTreeMap<String, serde_json::Value>,
    /// Marked fields in on-form order.
    pub marked_fields: Vec<String>,
    /// D13: guided mode starts on when the template marks anything.
    pub guided_default_on: bool,
}

impl PrefillPlan {
    /// D14 stop list: marked fields, then each required field the form still
    /// reports empty (the form-level hook supplies these, in its own order),
    /// then Save. A required field that is also marked stops only once.
    pub fn guided_stops<I>(&self, required_empty: I) -> Vec<GuidedStop>
    where
        I: IntoIterator<Item = String>,
    {
        let mut stops: Vec<GuidedStop> = self
            .marked_fields
            .iter()
            .cloned()
            .map(GuidedStop::Marked)
            .collect();
        for field in required_empty {
            if !self.marked_fields.contains(&field)
                && !stops.contains(&GuidedStop::RequiredEmpty(field.clone()))
            {
                stops.push(GuidedStop::RequiredEmpty(field));
            }
        }
        stops.push(GuidedStop::Save);
        stops
    }
}

/// Sort key putting concrete field ids in on-form order, rows by index.
pub(crate) fn form_order_key(kinds: &FormFieldKinds, field: &str) -> (usize, Vec<usize>, String) {
    let indices = field
        .split('[')
        .skip(1)
        .filter_map(|part| part.split(']').next()?.parse().ok())
        .collect();
    (
        kinds.position_of(field).unwrap_or(usize::MAX),
        indices,
        field.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kept(value: serde_json::Value) -> TemplateField {
        TemplateField::Kept { value }
    }

    #[test]
    fn validation_enforces_field_kinds() {
        let filed = TemplateSource::Filed { draft_id: 1 };
        let cases: Vec<(
            &str,
            TemplateField,
            &TemplateSource,
            Option<TemplateFieldError>,
        )> = vec![
            ("item_13_election", kept(json!("graduated")), &filed, None),
            ("schedule_1[1].atc", kept(json!("PT010")), &filed, None),
            (
                "schedule_1[0].taxable_amount",
                TemplateField::Marked,
                &filed,
                None,
            ),
            (
                "schedule_1[0].taxable_amount",
                kept(json!(100.0)),
                &filed,
                Some(TemplateFieldError::AmountKept(
                    "schedule_1[0].taxable_amount".into(),
                )),
            ),
            (
                "rdo_code",
                TemplateField::Marked,
                &filed,
                Some(TemplateFieldError::AutoField("rdo_code".into())),
            ),
            (
                "quarter",
                kept(json!(2)),
                &filed,
                Some(TemplateFieldError::AutoField("quarter".into())),
            ),
            (
                "total_tax_due",
                TemplateField::Marked,
                &filed,
                Some(TemplateFieldError::AutoField("total_tax_due".into())),
            ),
            (
                "status",
                TemplateField::Marked,
                &filed,
                Some(TemplateFieldError::UnknownField {
                    form_code: "2551Q".into(),
                    field: "status".into(),
                }),
            ),
            (
                "item_13_election",
                kept(json!("graduated")),
                &TemplateSource::Blank,
                Some(TemplateFieldError::KeptOnBlankTemplate(
                    "item_13_election".into(),
                )),
            ),
        ];
        for (field, value, source, expected) in cases {
            let fields = BTreeMap::from([(field.to_string(), value)]);
            assert_eq!(
                validate_template_fields("2551Q", &fields, source).err(),
                expected,
                "{field}"
            );
        }
        assert_eq!(
            validate_template_fields("1701", &BTreeMap::new(), &filed).err(),
            Some(TemplateFieldError::FormNotClassified("1701".into()))
        );
    }

    #[test]
    fn fields_json_shape_is_the_documented_contract() {
        let fields = BTreeMap::from([
            ("a".to_string(), kept(json!("x"))),
            ("b".to_string(), TemplateField::Marked),
        ]);
        assert_eq!(
            serde_json::to_value(&fields).unwrap(),
            json!({"a": {"kind": "kept", "value": "x"}, "b": {"kind": "marked"}})
        );
    }

    #[test]
    fn source_round_trips_and_rejects_garbage() {
        for source in [
            TemplateSource::Blank,
            TemplateSource::Filed { draft_id: 42 },
            TemplateSource::Return { draft_id: 7 },
            TemplateSource::Copy {
                from_tin: "12345678900001".into(),
            },
        ] {
            assert_eq!(TemplateSource::from_db(&source.to_db()).unwrap(), source);
        }
        for bad in ["", "filed:", "filed:x", "return:", "copy:", "kept"] {
            assert!(TemplateSource::from_db(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn snapshot_keeps_setup_answers_and_drops_amounts_auto_and_blanks() {
        let kinds = super::super::field_kinds_for("2551Q").unwrap();
        let draft = json!({
            "tin": "12345678800000",
            "quarter": 2,
            "item_13_election": "graduated",
            "tax_relief": false,
            "tax_relief_specification": "",
            "other_tax_credit_description": null,
            "creditable_tax_withheld": 500.0,
            "total_tax_due": 4500.0,
            "schedule_1": [
                {"atc": "PT010", "taxable_amount": 150000.0, "tax_due": 4500.0},
                {"atc": "", "taxable_amount": 0.0}
            ]
        });
        let fields = snapshot_setup_answers(kinds, &draft);
        assert_eq!(
            fields,
            BTreeMap::from([
                ("item_13_election".to_string(), kept(json!("graduated"))),
                ("schedule_1[0].atc".to_string(), kept(json!("PT010"))),
                ("tax_relief".to_string(), kept(json!(false))),
            ])
        );
        assert!(
            validate_template_fields("2551Q", &fields, &TemplateSource::Return { draft_id: 1 })
                .is_ok()
        );
    }

    #[test]
    fn guided_stops_follow_d14_order_without_duplicates() {
        let plan = PrefillPlan {
            tin: "12345678900000".into(),
            period: ResolvedPeriod::from_library_selection("2551Q", 2026, 1).unwrap(),
            auto_values: BTreeMap::new(),
            locked_fields: vec![],
            kept_values: BTreeMap::new(),
            marked_fields: vec![
                "item_13_election".into(),
                "schedule_1[0].taxable_amount".into(),
            ],
            guided_default_on: true,
        };
        assert_eq!(
            plan.guided_stops(["item_13_election".to_string(), "tax_relief".to_string()]),
            vec![
                GuidedStop::Marked("item_13_election".into()),
                GuidedStop::Marked("schedule_1[0].taxable_amount".into()),
                GuidedStop::RequiredEmpty("tax_relief".into()),
                GuidedStop::Save,
            ]
        );
    }
}
