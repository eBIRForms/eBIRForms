//! Taxpayer -> Branch -> yearly forms -> one template per form (tax-profile
//! redesign, `docs/tax-profile/REDESIGN_2026-10-14_SPEC.md`, Phase 1 data
//! layer). Persistence lives on [`crate::db::Database`]; this module holds the
//! domain types and the rules that do not need a connection.
//!
//! - A **taxpayer** is one 9-digit TIN root (`taxpayers` table). Name, type
//!   and birth date live here. Email, Security, Export and Calendar settings
//!   stay on the head-office profile row (D12); see
//!   [`crate::db::Database::taxpayer_settings_tin`].
//! - A **branch** is one `profiles` row, identified by its full TIN. Every
//!   existing TIN-keyed table keeps its key. A branch inherits RDO, address,
//!   ZIP, phone and email from the head office (`00000`) unless the field is
//!   overridden (D2). Inheritance resolves on read.

pub mod field_kinds;
pub mod templates;

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::db::DbError;
use crate::profile::TaxpayerType;

pub use field_kinds::{
    AutoSource, FieldKind, FieldSpec, FormFieldKinds, IdentitySource, PeriodSource, ResolvedPeriod,
    TemplatableKind, field_kinds_for,
};
pub use templates::{
    FormTemplate, GuidedStop, PrefillPlan, TemplateChoice, TemplateField, TemplateFieldError,
    TemplateSource, snapshot_setup_answers, validate_template_fields,
};

/// Default name of the `00000` branch.
pub const HEAD_OFFICE_NAME: &str = "Head office";

#[derive(Debug, thiserror::Error)]
pub enum TaxProfileError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Template(#[from] TemplateFieldError),
}

impl From<rusqlite::Error> for TaxProfileError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(error))
    }
}

impl From<serde_json::Error> for TaxProfileError {
    fn from(error: serde_json::Error) -> Self {
        Self::Db(DbError::Serialization(error))
    }
}

/// TIN root of the help desk admin account. It is not a real taxpayer, so no
/// taxpayer may ever be created under it or moved onto it.
pub const RESERVED_ADMIN_TIN_ROOT: &str = "000000000";

/// Refuse the reserved help desk admin root (`root` already parsed).
pub fn reject_reserved_tin_root(root: &str) -> Result<(), TaxProfileError> {
    if root == RESERVED_ADMIN_TIN_ROOT {
        Err(TaxProfileError::Invalid(
            "TIN 000-000-000 is reserved for the help desk admin.".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// A 9-digit TIN root, or an error naming the bad input.
pub fn parse_tin_root(raw: &str) -> Result<String, TaxProfileError> {
    let root: String = raw.chars().filter(|ch| !matches!(ch, '-' | ' ')).collect();
    if root.len() == 9 && root.chars().all(|ch| ch.is_ascii_digit()) {
        Ok(root)
    } else {
        Err(TaxProfileError::Invalid(format!(
            "A TIN root is exactly 9 digits; got `{raw}`"
        )))
    }
}

/// A branch code normalized to 5 digits (legacy 3-digit codes are
/// zero-padded: `001` -> `00001`).
pub fn normalize_branch_code(raw: &str) -> Result<String, TaxProfileError> {
    if !(3..=5).contains(&raw.len()) || !raw.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(TaxProfileError::Invalid(format!(
            "A branch code is 3 to 5 digits; got `{raw}`"
        )));
    }
    Ok(format!("{raw:0>5}"))
}

pub fn is_head_office_code(code: &str) -> bool {
    !code.is_empty() && code.chars().all(|ch| ch == '0')
}

/// One-time review recorded by the v20 grouping (D11). Nothing is renamed or
/// invented; the UI asks once and then calls
/// [`crate::db::Database::resolve_taxpayer_grouping_review`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupingReview {
    /// Every branch's stored name, when they differ under one root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differing_names: Vec<BranchName>,
    /// The root has no zero-branch profile; none was created.
    #[serde(default)]
    pub missing_head_office: bool,
    /// More than one zero-branch profile (`000` and `00000`); the lowest
    /// profile id is used as head office.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub head_office_candidates: Vec<String>,
    #[serde(default)]
    pub resolved_at: Option<String>,
}

impl GroupingReview {
    pub fn needs_review(&self) -> bool {
        self.resolved_at.is_none()
            && (!self.differing_names.is_empty()
                || self.missing_head_office
                || !self.head_office_candidates.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchName {
    pub tin: String,
    pub name: String,
}

/// `taxpayers.data_json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaxpayerData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birth_date: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grouping_review: Option<GroupingReview>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Taxpayer {
    pub tin_root: String,
    pub name: String,
    pub taxpayer_type: TaxpayerType,
    pub birth_date: Option<NaiveDate>,
    pub grouping_review: Option<GroupingReview>,
    pub created_at: String,
    pub updated_at: String,
    /// Every branch profile is archived (`profiles.data_json.is_archived`).
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaxpayerWithBranches {
    pub taxpayer: Taxpayer,
    /// Head office first, then by branch code.
    pub branches: Vec<ResolvedBranch>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewTaxpayer {
    pub tin_root: String,
    pub name: String,
    pub taxpayer_type: TaxpayerType,
    pub birth_date: Option<NaiveDate>,
    /// Head-office facts; they cannot inherit.
    pub head_office: BranchFacts,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaxpayerUpdate {
    pub name: Option<String>,
    pub taxpayer_type: Option<TaxpayerType>,
    /// `Some(None)` clears the birth date.
    pub birth_date: Option<Option<NaiveDate>>,
}

/// The taxpayer's business and filing facts the forms read from the stored
/// profile (`profiles.data_json`, keys of [`crate::profile::TaxpayerProfile`]).
/// Shared by every branch: [`crate::db::Database::update_profile_details`]
/// writes them to each branch's profile, and
/// [`crate::db::Database::profile_details`] reads the head office's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDetails {
    /// What the forms print as the line of business (a PSIC code and its
    /// description, as the filer writes them).
    #[serde(default)]
    pub line_of_business: String,
    /// When the business started (or the corporation was incorporated).
    #[serde(default)]
    pub business_start_date: Option<NaiveDate>,
    #[serde(default)]
    pub eopt_tier: Option<crate::profile::EoptTier>,
    #[serde(default)]
    pub is_government_withholding_entity: bool,
    #[serde(default)]
    pub tax_classification: Option<crate::profile::TaxClassification>,
    #[serde(default)]
    pub has_employees: bool,
    #[serde(default)]
    pub withholds_compensation: bool,
    #[serde(default)]
    pub withholds_expanded: bool,
    #[serde(default)]
    pub withholds_final: bool,
    #[serde(default)]
    pub is_vat_registered: bool,
    #[serde(default)]
    pub is_dormant: bool,
    #[serde(default)]
    pub has_single_employer: bool,
}

/// Facts a branch may inherit from the head office (D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchField {
    RdoCode,
    RegisteredAddress,
    ZipCode,
    Phone,
    Email,
}

impl BranchField {
    pub const ALL: [Self; 5] = [
        Self::RdoCode,
        Self::RegisteredAddress,
        Self::ZipCode,
        Self::Phone,
        Self::Email,
    ];
}

/// Plain values for every inheritable fact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchFacts {
    pub rdo_code: String,
    pub registered_address: String,
    pub zip_code: String,
    pub phone: String,
    pub email: String,
}

impl BranchFacts {
    pub fn get(&self, field: BranchField) -> &str {
        match field {
            BranchField::RdoCode => &self.rdo_code,
            BranchField::RegisteredAddress => &self.registered_address,
            BranchField::ZipCode => &self.zip_code,
            BranchField::Phone => &self.phone,
            BranchField::Email => &self.email,
        }
    }

    pub(crate) fn set(&mut self, field: BranchField, value: String) {
        match field {
            BranchField::RdoCode => self.rdo_code = value,
            BranchField::RegisteredAddress => self.registered_address = value,
            BranchField::ZipCode => self.zip_code = value,
            BranchField::Phone => self.phone = value,
            BranchField::Email => self.email = value,
        }
    }
}

/// "Same as head office" toggle state for one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldSetting {
    Inherit,
    Override(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBranch {
    /// 5 digits, not all zeros; a free code can be suggested by
    /// [`crate::db::Database::suggest_next_branch_code`].
    pub branch_code: String,
    pub name: String,
    /// Fields not listed inherit from the head office.
    pub fields: BTreeMap<BranchField, FieldSetting>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchUpdate {
    pub name: Option<String>,
    /// Fields not listed are unchanged.
    pub fields: BTreeMap<BranchField, FieldSetting>,
}

/// `profiles.branch_json`. NULL (legacy rows) is the default: every fact is
/// the branch's own and the name is the default for its code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BranchSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub inherited: BTreeSet<BranchField>,
    /// Tax years the user added to this branch, kept even while a year has
    /// no forms (a `per_year_forms` set only exists once a form is listed).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tax_years: BTreeSet<u16>,
}

/// One effective branch fact and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedField {
    pub value: String,
    pub inherited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBranch {
    /// Full TIN as stored: the key of every TIN-keyed table.
    pub tin: String,
    pub tin_root: String,
    /// 5-digit normalized code.
    pub branch_code: String,
    pub name: String,
    pub is_head_office: bool,
    pub facts: BTreeMap<BranchField, ResolvedField>,
}

impl ResolvedBranch {
    pub fn value(&self, field: BranchField) -> &str {
        self.facts
            .get(&field)
            .map(|resolved| resolved.value.as_str())
            .unwrap_or_default()
    }
}

pub(crate) fn default_branch_name(branch_code: &str) -> String {
    if is_head_office_code(branch_code) {
        HEAD_OFFICE_NAME.to_string()
    } else {
        format!("Branch {branch_code}")
    }
}

/// D2 resolution: an inherited field takes the head office's current value;
/// with no head office, or on the head office itself, the branch's own value.
pub(crate) fn resolve_facts(
    own: &BranchFacts,
    inherited: &BTreeSet<BranchField>,
    head_office: Option<&BranchFacts>,
) -> BTreeMap<BranchField, ResolvedField> {
    BranchField::ALL
        .into_iter()
        .map(|field| {
            let resolved = match head_office {
                Some(head) if inherited.contains(&field) => ResolvedField {
                    value: head.get(field).to_string(),
                    inherited: true,
                },
                _ => ResolvedField {
                    value: own.get(field).to_string(),
                    inherited: false,
                },
            };
            (field, resolved)
        })
        .collect()
}

/// The earliest tax year a branch may add or edit.
pub const MIN_TAX_YEAR: u16 = 2010;

/// Allowed tax years: 2010 through one year after the current calendar year.
pub fn tax_year_bounds() -> (u16, u16) {
    use chrono::Datelike;
    tax_year_bounds_for(chrono::Local::now().year())
}

pub(crate) fn tax_year_bounds_for(current_year: i32) -> (u16, u16) {
    let max = u16::try_from(current_year.saturating_add(1)).unwrap_or(u16::MAX);
    (MIN_TAX_YEAR, max.max(MIN_TAX_YEAR))
}

/// Refuse a tax year outside [`tax_year_bounds`]. Every yearly-forms write
/// path calls this, so no caller can store a year the UI would not offer.
pub fn check_tax_year(year: u16) -> Result<(), TaxProfileError> {
    use chrono::Datelike;
    check_tax_year_for(year, chrono::Local::now().year())
}

pub(crate) fn check_tax_year_for(year: u16, current_year: i32) -> Result<(), TaxProfileError> {
    let (min, max) = tax_year_bounds_for(current_year);
    if (min..=max).contains(&year) {
        Ok(())
    } else {
        Err(TaxProfileError::Invalid(format!(
            "Choose a tax year from {min} to {max}."
        )))
    }
}

/// `9 digits` -> `000-000-000`.
pub fn format_tin_root(root: &str) -> String {
    if root.len() == 9 && root.is_ascii() {
        format!("{}-{}-{}", &root[0..3], &root[3..6], &root[6..9])
    } else {
        root.to_string()
    }
}

/// Result of [`crate::db::Database::add_tax_year`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedTaxYear {
    pub year: u16,
    /// The branch's latest earlier-added year whose forms were copied, when
    /// copying was asked for and the branch had a year.
    pub copied_from: Option<u16>,
    /// Active form codes of the new year.
    pub forms: Vec<String>,
}

/// D5 starter packs. Each adds its forms to a branch's yearly list.
///
/// Contents for the maintainer to review (official codes from
/// `forms::registry::FORM_REGISTRY`):
///
/// | Pack | Individual / estate / trust | Corporation / partnership / cooperative |
/// | --- | --- | --- |
/// | Non-VAT business | 2551Q, 1701Q, 1701 | 2551Q, 1702Q, 1702RT |
/// | VAT business | 2550Q, 1701Q, 1701 | 2550Q, 1702Q, 1702RT |
/// | Employer | 1601C, 0619E, 1601EQ, 1604C, 1604E | same |
/// | Professional | 2551Q, 1701Q, 1701A | 2551Q, 1702Q, 1702RT |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StarterPack {
    NonVatBusiness,
    VatBusiness,
    Employer,
    Professional,
}

impl StarterPack {
    pub const ALL: [Self; 4] = [
        Self::NonVatBusiness,
        Self::VatBusiness,
        Self::Employer,
        Self::Professional,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::NonVatBusiness => "Non-VAT business",
            Self::VatBusiness => "VAT business",
            Self::Employer => "Employer",
            Self::Professional => "Professional",
        }
    }

    pub fn form_codes(self, taxpayer_type: &TaxpayerType) -> &'static [&'static str] {
        let individual = matches!(
            taxpayer_type,
            TaxpayerType::Individual | TaxpayerType::Estate | TaxpayerType::Trust
        );
        match (self, individual) {
            (Self::NonVatBusiness, true) => &["2551Q", "1701Q", "1701"],
            (Self::NonVatBusiness, false) | (Self::Professional, false) => {
                &["2551Q", "1702Q", "1702RT"]
            }
            (Self::VatBusiness, true) => &["2550Q", "1701Q", "1701"],
            (Self::VatBusiness, false) => &["2550Q", "1702Q", "1702RT"],
            (Self::Employer, _) => &["1601C", "0619E", "1601EQ", "1604C", "1604E"],
            (Self::Professional, true) => &["2551Q", "1701Q", "1701A"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_packs_name_current_registry_forms_for_every_taxpayer_type() {
        for pack in StarterPack::ALL {
            for taxpayer_type in [
                TaxpayerType::Individual,
                TaxpayerType::Corporation,
                TaxpayerType::Partnership,
                TaxpayerType::Cooperative,
                TaxpayerType::Estate,
                TaxpayerType::Trust,
            ] {
                for code in pack.form_codes(&taxpayer_type) {
                    let form = crate::forms::find_form(code)
                        .unwrap_or_else(|| panic!("{pack:?}: {code} is not in the registry"));
                    assert!(
                        form.deprecation_year().is_none(),
                        "{pack:?}: {code} is deprecated"
                    );
                }
            }
        }
    }

    #[test]
    fn tax_year_bound_is_2010_to_next_year() {
        assert_eq!(tax_year_bounds_for(2026), (2010, 2027));
        for ok in [2010, 2026, 2027] {
            assert!(check_tax_year_for(ok, 2026).is_ok(), "{ok}");
        }
        for bad in [2009, 2028, 0] {
            let error = check_tax_year_for(bad, 2026).unwrap_err();
            assert_eq!(error.to_string(), "Choose a tax year from 2010 to 2027.");
        }
        assert!(check_tax_year_for(2030, 2029).is_ok());
        assert_eq!(
            check_tax_year_for(2031, 2029).unwrap_err().to_string(),
            "Choose a tax year from 2010 to 2030."
        );
        assert_eq!(format_tin_root("123456788"), "123-456-788");
    }

    #[test]
    fn branch_codes_normalize_to_five_digits() {
        assert_eq!(normalize_branch_code("000").unwrap(), "00000");
        assert_eq!(normalize_branch_code("001").unwrap(), "00001");
        assert_eq!(normalize_branch_code("00012").unwrap(), "00012");
        for bad in ["", "01", "000000", "00a01"] {
            assert!(normalize_branch_code(bad).is_err(), "{bad}");
        }
    }
}
