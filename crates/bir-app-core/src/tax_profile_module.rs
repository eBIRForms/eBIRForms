//! Tax-profile host functions: taxpayers, branches, tax years, yearly forms,
//! starter packs and form templates. Every rule (TIN shape, tax-year bound,
//! one template per form, D8 setup-only snapshots, the one-transaction TIN
//! move) lives in bir-core; this file only converts JSON in and out.

use std::collections::{BTreeMap, BTreeSet};

use crate::host::{HostArguments, HostError, HostModule, HostObject, HostResult, HostValue};
use bir_core::db::Database;
use bir_core::forms::registry::canonical_form_code;
use bir_core::profile::TaxpayerType;
use bir_core::tax_profile::{
    BranchFacts, BranchField, BranchUpdate, FieldSetting, FormTemplate, NewBranch, NewTaxpayer,
    ResolvedBranch, StarterPack, TaxProfileError, TaxpayerUpdate, TaxpayerWithBranches,
    TemplateField, format_tin_root, tax_year_bounds,
};
use serde_json::Value;

use crate::database::{to_json, with_database};

/// TypeScript appended to the `bir` module's declarations.
/// The JSON shapes of the functions below, in TypeScript notation (kept from
/// the old JS UI as the spec for typing them).
#[allow(dead_code)]
pub const DECLARATIONS: &str = r#"
export type BranchFieldName = "rdo_code" | "registered_address" | "zip_code" | "phone" | "email";
export interface BranchFacts {
    rdo_code: string;
    registered_address: string;
    zip_code: string;
    phone: string;
    email: string;
}
export interface Branch {
    tin: string;
    tin_display: string;
    tin_root: string;
    code: string;
    name: string;
    is_head_office: boolean;
    facts: BranchFacts;
    inherited: BranchFieldName[];
}
export interface Taxpayer {
    tin_root: string;
    tin_display: string;
    name: string;
    taxpayer_type: "Individual" | "Corporation" | "Partnership" | "Cooperative" | "Estate" | "Trust";
    birth_date: string | null;
    needs_grouping_review: boolean;
    /** Every branch is archived (set_taxpayer_archived). Archived taxpayers stay listed. */
    archived: boolean;
    branches: Branch[];
}
export interface NewTaxpayerInput {
    tin_root: string;
    name: string;
    taxpayer_type: Taxpayer["taxpayer_type"];
    birth_date?: string | null;
    head_office: Partial<BranchFacts>;
}
export interface TaxpayerUpdateInput {
    name?: string;
    taxpayer_type?: Taxpayer["taxpayer_type"];
}
export interface NewBranchInput {
    branch_code: string;
    name: string;
    fields?: Partial<Record<BranchFieldName, string | null>>;
}
export interface BranchUpdateInput {
    name?: string;
    fields?: Partial<Record<BranchFieldName, string | null>>;
}
export interface TaxYearBounds { min: number; max: number; }
export interface AddedTaxYear { year: number; copied_from: number | null; forms: string[]; }
export interface YearlyFormsChange { added: string[]; removed: string[]; forms: string[]; }
export interface StarterPackInfo { id: string; label: string; forms: string[]; }
export interface FormTemplateSummary {
    form_code: string;
    updated_at: string;
    answers_kept: number;
    marked: number;
    source: string;
}
export function list_taxpayers(): Taxpayer[];
export function get_taxpayer(tin_root: string): Taxpayer | null;
export function create_taxpayer(input: NewTaxpayerInput): Taxpayer;
export function update_taxpayer(tin_root: string, update: TaxpayerUpdateInput): Taxpayer;
export function move_taxpayer_tin(old_root: string, new_root: string): Taxpayer;
export function create_branch(tin_root: string, input: NewBranchInput): Branch;
export function update_branch(tin: string, update: BranchUpdateInput): Branch;
export function move_branch_code(tin: string, new_code: string): Branch;
export function suggest_next_branch_code(tin_root: string): string;
export function tax_year_bounds(): TaxYearBounds;
export function list_tax_years(tin: string): number[];
export function add_tax_year(tin: string, year: number, copy_forms: boolean): AddedTaxYear;
export function yearly_forms(tin: string, year: number): string[];
export function set_yearly_forms(tin: string, year: number, form_codes: string[]): YearlyFormsChange;
export function starter_packs(tin: string): StarterPackInfo[];
export function list_form_templates(tin: string): FormTemplateSummary[];
export function create_template_from_latest(tin: string, form_code: string): FormTemplateSummary;
export function create_template_from_return(draft_id: number): FormTemplateSummary;
export function delete_form_template(tin: string, form_code: string): boolean;
export interface SendGrant { tin: string; form: string; granted_at: string; }
/** The assistant's standing OKs to send for this branch (14-digit TIN). */
export function list_send_grants(tin: string): SendGrant[];
/** Let the assistant send `form` for this branch without asking again. */
export function grant_send(tin: string, form: string): SendGrant;
export function revoke_send(tin: string, form: string): null;
export interface RdoOption { code: string; description: string; }
export function list_rdos(): RdoOption[];
export function list_zip_codes(): string[];

"#;

pub fn register(module: HostModule) -> HostModule {
    module
        .function("list_taxpayers", |_| list_taxpayers())
        .function("get_taxpayer", get_taxpayer)
        .function("create_taxpayer", create_taxpayer)
        .function("update_taxpayer", update_taxpayer)
        .function("move_taxpayer_tin", move_taxpayer_tin)
        .function("create_branch", create_branch)
        .function("update_branch", update_branch)
        .function("move_branch_code", move_branch_code)
        .function("suggest_next_branch_code", suggest_next_branch_code)
        .function("tax_year_bounds", |_| tax_year_bounds_fn())
        .function("list_tax_years", list_tax_years)
        .function("add_tax_year", add_tax_year)
        .function("yearly_forms", yearly_forms)
        .function("set_yearly_forms", set_yearly_forms)
        .function("starter_packs", starter_packs)
        .function("list_form_templates", list_form_templates)
        .function("create_template_from_latest", create_template_from_latest)
        .function("create_template_from_return", create_template_from_return)
        .function("delete_form_template", delete_form_template)
        .function("list_send_grants", list_send_grants)
        .function("grant_send", grant_send)
        .function("revoke_send", revoke_send)
        .function("list_rdos", |_| list_rdos())
        .function("list_zip_codes", |_| list_zip_codes())
}

// ---------- reference lists ----------

fn list_rdos() -> HostResult {
    Ok(HostValue::Array(
        bir_core::reference::get_all_rdos()
            .into_iter()
            .map(|rdo| {
                HostObject::new()
                    .field("code", rdo.code)
                    .field("description", rdo.description)
                    .into()
            })
            .collect(),
    ))
}

/// Each entry reads "1000 - Manila CPO - Ermita": the code, then the place.
fn list_zip_codes() -> HostResult {
    Ok(strings(bir_core::reference::get_all_zipcodes()))
}

// ---------- conversion helpers ----------

fn host_error(error: TaxProfileError) -> HostError {
    HostError::new(error.to_string())
}

fn strings(values: impl IntoIterator<Item = String>) -> HostValue {
    HostValue::Array(values.into_iter().map(HostValue::Str).collect())
}

fn field_name(field: BranchField) -> String {
    match serde_json::to_value(field) {
        Ok(Value::String(name)) => name,
        _ => format!("{field:?}"),
    }
}

fn parse_field(name: &str) -> Result<BranchField, HostError> {
    serde_json::from_value(Value::String(name.to_string()))
        .map_err(|_| HostError::new(format!("`{name}` is not a branch field")))
}

fn year_argument(arguments: &HostArguments, index: usize) -> Result<u16, HostError> {
    let year = arguments.integer(index)?;
    u16::try_from(year).map_err(|_| HostError::new(format!("{year} is not a tax year")))
}

fn object_argument(arguments: &HostArguments, index: usize) -> Result<Value, HostError> {
    match to_json(arguments.value(index)?) {
        value @ Value::Object(_) => Ok(value),
        _ => Err(HostError::new(format!(
            "argument {} must be an object",
            index + 1
        ))),
    }
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>, HostError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(HostError::new(format!("`{key}` must be a string"))),
    }
}

fn taxpayer_type(value: &Value, key: &str) -> Result<Option<TaxpayerType>, HostError> {
    optional_string(value, key)?
        .map(|raw| {
            serde_json::from_value(Value::String(raw.clone()))
                .map_err(|_| HostError::new(format!("`{raw}` is not a taxpayer type")))
        })
        .transpose()
}

/// `{field: "value" | null}`: a string overrides, null inherits.
fn field_settings(value: &Value) -> Result<BTreeMap<BranchField, FieldSetting>, HostError> {
    let Some(fields) = value.get("fields").filter(|fields| !fields.is_null()) else {
        return Ok(BTreeMap::new());
    };
    let fields = fields
        .as_object()
        .ok_or_else(|| HostError::new("`fields` must be an object"))?;
    fields
        .iter()
        .map(|(name, setting)| {
            let setting = match setting {
                Value::Null => FieldSetting::Inherit,
                Value::String(text) => FieldSetting::Override(text.clone()),
                _ => {
                    return Err(HostError::new(format!(
                        "`{name}` must be a string, or null to use the head office's value"
                    )));
                }
            };
            Ok((parse_field(name)?, setting))
        })
        .collect()
}

fn branch_record(branch: &ResolvedBranch) -> HostValue {
    let mut facts = HostObject::new();
    let mut inherited = Vec::new();
    for field in BranchField::ALL {
        facts = facts.field(field_name(field), branch.value(field));
        if branch
            .facts
            .get(&field)
            .is_some_and(|resolved| resolved.inherited)
        {
            inherited.push(field_name(field));
        }
    }
    HostObject::new()
        .field("tin", branch.tin.as_str())
        .field(
            "tin_display",
            format!(
                "{}-{}",
                format_tin_root(&branch.tin_root),
                branch.branch_code
            ),
        )
        .field("tin_root", branch.tin_root.as_str())
        .field("code", branch.branch_code.as_str())
        .field("name", branch.name.as_str())
        .field("is_head_office", branch.is_head_office)
        .field("facts", facts)
        .field("inherited", strings(inherited))
        .into()
}

fn taxpayer_record(entry: &TaxpayerWithBranches) -> HostValue {
    let taxpayer = &entry.taxpayer;
    HostObject::new()
        .field("tin_root", taxpayer.tin_root.as_str())
        .field("tin_display", format_tin_root(&taxpayer.tin_root))
        .field("name", taxpayer.name.as_str())
        .field(
            "taxpayer_type",
            field_string(serde_json::to_value(&taxpayer.taxpayer_type).ok()),
        )
        .field(
            "birth_date",
            taxpayer
                .birth_date
                .map_or(HostValue::Null, |date| HostValue::Str(date.to_string())),
        )
        .field(
            "needs_grouping_review",
            taxpayer
                .grouping_review
                .as_ref()
                .is_some_and(|review| review.needs_review()),
        )
        .field("archived", taxpayer.archived)
        .field(
            "branches",
            HostValue::Array(entry.branches.iter().map(branch_record).collect()),
        )
        .into()
}

fn field_string(value: Option<Value>) -> String {
    match value {
        Some(Value::String(text)) => text,
        _ => String::new(),
    }
}

fn template_record(template: &FormTemplate) -> HostValue {
    let kept = template
        .fields
        .values()
        .filter(|field| matches!(field, TemplateField::Kept { .. }))
        .count();
    let marked = template.fields.len() - kept;
    HostObject::new()
        .field("form_code", template.form_code.as_str())
        .field("updated_at", template.updated_at.as_str())
        .field("answers_kept", kept as f64)
        .field("marked", marked as f64)
        .field("source", template.source.to_db())
        .into()
}

fn load_taxpayer(db: &Database, tin_root: &str) -> Result<TaxpayerWithBranches, HostError> {
    db.get_taxpayer(tin_root)
        .map_err(host_error)?
        .ok_or_else(|| HostError::new(format!("No taxpayer with TIN {tin_root}")))
}

// ---------- taxpayers ----------

fn list_taxpayers() -> HostResult {
    with_database(|db| {
        let taxpayers = db.list_taxpayers().map_err(host_error)?;
        Ok(HostValue::Array(
            taxpayers.iter().map(taxpayer_record).collect(),
        ))
    })
}

fn get_taxpayer(arguments: &HostArguments) -> HostResult {
    let tin_root = arguments.string(0)?;
    with_database(|db| {
        Ok(db
            .get_taxpayer(tin_root)
            .map_err(host_error)?
            .as_ref()
            .map_or(HostValue::Null, taxpayer_record))
    })
}

fn create_taxpayer(arguments: &HostArguments) -> HostResult {
    let input = object_argument(arguments, 0)?;
    let head = input.get("head_office").cloned().unwrap_or(Value::Null);
    let fact = |key: &str| -> Result<String, HostError> {
        Ok(optional_string(&head, key)?.unwrap_or_default())
    };
    let birth_date = optional_string(&input, "birth_date")?
        .filter(|raw| !raw.is_empty())
        .map(|raw| {
            raw.parse()
                .map_err(|_| HostError::new(format!("`{raw}` is not a date (YYYY-MM-DD)")))
        })
        .transpose()?;
    let new = NewTaxpayer {
        tin_root: optional_string(&input, "tin_root")?.unwrap_or_default(),
        name: optional_string(&input, "name")?.unwrap_or_default(),
        taxpayer_type: taxpayer_type(&input, "taxpayer_type")?.unwrap_or_default(),
        birth_date,
        head_office: BranchFacts {
            rdo_code: fact("rdo_code")?,
            registered_address: fact("registered_address")?,
            zip_code: fact("zip_code")?,
            phone: fact("phone")?,
            email: fact("email")?,
        },
    };
    with_database(|db| {
        let created = db.create_taxpayer(new).map_err(host_error)?;
        Ok(taxpayer_record(&created))
    })
}

fn update_taxpayer(arguments: &HostArguments) -> HostResult {
    let tin_root = arguments.string(0)?;
    let input = object_argument(arguments, 1)?;
    let update = TaxpayerUpdate {
        name: optional_string(&input, "name")?,
        taxpayer_type: taxpayer_type(&input, "taxpayer_type")?,
        birth_date: None,
    };
    with_database(|db| {
        let updated = db.update_taxpayer(tin_root, update).map_err(host_error)?;
        Ok(taxpayer_record(&load_taxpayer(db, &updated.tin_root)?))
    })
}

fn move_taxpayer_tin(arguments: &HostArguments) -> HostResult {
    let old_root = arguments.string(0)?;
    let new_root = arguments.string(1)?;
    with_database(|db| {
        let moved = db
            .move_taxpayer_tin(old_root, new_root)
            .map_err(host_error)?;
        Ok(taxpayer_record(&moved))
    })
}

// ---------- branches ----------

fn create_branch(arguments: &HostArguments) -> HostResult {
    let tin_root = arguments.string(0)?;
    let input = object_argument(arguments, 1)?;
    let new = NewBranch {
        branch_code: optional_string(&input, "branch_code")?.unwrap_or_default(),
        name: optional_string(&input, "name")?.unwrap_or_default(),
        fields: field_settings(&input)?,
    };
    with_database(|db| {
        let branch = db.create_branch(tin_root, new).map_err(host_error)?;
        Ok(branch_record(&branch))
    })
}

fn update_branch(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let input = object_argument(arguments, 1)?;
    let update = BranchUpdate {
        name: optional_string(&input, "name")?,
        fields: field_settings(&input)?,
    };
    with_database(|db| {
        let branch = db.update_branch(tin, update).map_err(host_error)?;
        Ok(branch_record(&branch))
    })
}

fn move_branch_code(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let new_code = arguments.string(1)?;
    with_database(|db| {
        let branch = db.move_branch_code(tin, new_code).map_err(host_error)?;
        Ok(branch_record(&branch))
    })
}

fn suggest_next_branch_code(arguments: &HostArguments) -> HostResult {
    let tin_root = arguments.string(0)?;
    with_database(|db| {
        db.suggest_next_branch_code(tin_root)
            .map(HostValue::Str)
            .map_err(host_error)
    })
}

// ---------- tax years and yearly forms ----------

fn tax_year_bounds_fn() -> HostResult {
    let (min, max) = tax_year_bounds();
    Ok(HostObject::new()
        .field("min", f64::from(min))
        .field("max", f64::from(max))
        .into())
}

fn list_tax_years(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    with_database(|db| {
        let years = db.list_tax_years(tin).map_err(host_error)?;
        Ok(HostValue::Array(
            years
                .into_iter()
                .map(|year| HostValue::Number(f64::from(year)))
                .collect(),
        ))
    })
}

fn add_tax_year(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let year = year_argument(arguments, 1)?;
    let copy_forms = arguments.boolean(2)?;
    with_database(|db| {
        let added = db.add_tax_year(tin, year, copy_forms).map_err(host_error)?;
        Ok(HostObject::new()
            .field("year", f64::from(added.year))
            .field(
                "copied_from",
                added
                    .copied_from
                    .map_or(HostValue::Null, |year| HostValue::Number(f64::from(year))),
            )
            .field("forms", strings(added.forms))
            .into())
    })
}

fn yearly_forms(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let year = year_argument(arguments, 1)?;
    with_database(|db| {
        db.active_form_codes_for_year(tin, year)
            .map(strings)
            .map_err(|error| HostError::new(error.to_string()))
    })
}

fn set_yearly_forms(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let year = year_argument(arguments, 1)?;
    let codes = arguments
        .value(2)?
        .as_array()
        .ok_or_else(|| HostError::new("argument 3 must be a list of form codes"))?
        .iter()
        .map(|code| {
            code.as_str()
                .map(str::to_owned)
                .ok_or_else(|| HostError::new("every form code must be a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    with_database(|db| {
        let canonical = |codes: Vec<String>| -> BTreeSet<String> {
            codes.iter().map(|code| canonical_form_code(code)).collect()
        };
        let before = canonical(
            db.active_form_codes_for_year(tin, year)
                .map_err(|error| HostError::new(error.to_string()))?,
        );
        let set = db.set_yearly_forms(tin, year, &codes).map_err(host_error)?;
        let after = canonical(set.active_form_codes());
        Ok(HostObject::new()
            .field("added", strings(after.difference(&before).cloned()))
            .field("removed", strings(before.difference(&after).cloned()))
            .field("forms", strings(after))
            .into())
    })
}

fn starter_packs(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    with_database(|db| {
        let branch = db
            .get_branch(tin)
            .map_err(host_error)?
            .ok_or_else(|| HostError::new(format!("No branch with TIN {tin}")))?;
        let taxpayer = load_taxpayer(db, &branch.tin_root)?.taxpayer;
        Ok(HostValue::Array(
            StarterPack::ALL
                .into_iter()
                .map(|pack| {
                    HostObject::new()
                        .field("id", field_string(serde_json::to_value(pack).ok()))
                        .field("label", pack.label())
                        .field(
                            "forms",
                            strings(
                                pack.form_codes(&taxpayer.taxpayer_type)
                                    .iter()
                                    .map(|code| code.to_string()),
                            ),
                        )
                        .into()
                })
                .collect(),
        ))
    })
}

// ---------- templates ----------

fn list_form_templates(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    with_database(|db| {
        let templates = db.list_form_templates(tin).map_err(host_error)?;
        Ok(HostValue::Array(
            templates.iter().map(template_record).collect(),
        ))
    })
}

fn create_template_from_latest(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let form_code = arguments.string(1)?;
    with_database(|db| {
        let template = db
            .snapshot_template_from_latest_return(tin, form_code)
            .map_err(host_error)?;
        Ok(template_record(&template))
    })
}

fn create_template_from_return(arguments: &HostArguments) -> HostResult {
    let draft_id = arguments.integer(0)?;
    with_database(|db| {
        let template = db
            .snapshot_template_from_return(draft_id)
            .map_err(host_error)?;
        Ok(template_record(&template))
    })
}

fn delete_form_template(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let form_code = arguments.string(1)?;
    with_database(|db| {
        db.delete_form_template(tin, form_code)
            .map(HostValue::Bool)
            .map_err(host_error)
    })
}

// ---------- send grants (the assistant's standing OK to send) ----------

fn grant_record(grant: &bir_core::db::SendGrant) -> HostValue {
    HostObject::new()
        .field("tin", grant.tin.as_str())
        .field("form", grant.form.as_str())
        .field("granted_at", grant.granted_at.as_str())
        .into()
}

fn list_send_grants(arguments: &HostArguments) -> HostResult {
    let digits: String = arguments.string(0)?.chars().filter(char::is_ascii_digit).collect();
    with_database(|db| {
        let grants = db.list_send_grants().map_err(|error| HostError::new(error.to_string()))?;
        Ok(HostValue::Array(
            grants.iter().filter(|grant| grant.tin == digits).map(grant_record).collect(),
        ))
    })
}

fn grant_send(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let form = arguments.string(1)?;
    with_database(|db| {
        db.grant_send(tin, form)
            .map(|grant| grant_record(&grant))
            .map_err(|error| HostError::new(error.to_string()))
    })
}

fn revoke_send(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let form = arguments.string(1)?;
    with_database(|db| {
        db.revoke_send(tin, form)
            .map(|()| HostValue::Null)
            .map_err(|error| HostError::new(error.to_string()))
    })
}
