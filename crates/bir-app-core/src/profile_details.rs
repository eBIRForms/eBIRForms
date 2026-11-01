//! The taxpayer's business and filing facts beyond name, type and the
//! branch facts: line of business, business start, EOPT tier, government
//! withholding, tax classification, the year's income-tax election and the
//! flags that decide which forms apply. bir-core stores them in every
//! branch's profile ([`bir_core::db::Database::update_profile_details`]).
//!
//! Like every [`BirApp`] method that takes `&self`, these read or write the
//! database: call them from a worker thread.

use bir_core::profile::{EoptTier, IncomeTaxElection, TaxClassification};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AppError, AppResult, BirApp, Choice};

/// What the profile's Business and Taxpayer sections show and save. Choice
/// fields hold a value of their list ([`BirApp::eopt_tiers`],
/// [`BirApp::tax_classifications`], [`BirApp::income_tax_elections`]);
/// dates are `YYYY-MM-DD`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDetails {
    pub line_of_business: String,
    pub business_start_date: Option<String>,
    pub eopt_tier: Option<String>,
    pub government_withholding: bool,
    pub tax_classification: Option<String>,
    /// The year [`ProfileDetails::income_tax_election`] is for.
    pub election_year: u16,
    pub income_tax_election: Option<String>,
    pub has_employees: bool,
    pub withholds_compensation: bool,
    pub withholds_expanded: bool,
    pub withholds_final: bool,
    pub vat_registered: bool,
    pub dormant: bool,
    pub single_employer: bool,
}

/// The branch facts a form box can save back to (`set_branch_fact`).
const BRANCH_FACTS: [&str; 5] = ["rdo_code", "registered_address", "zip_code", "phone", "email"];

fn call_error(error: impl std::fmt::Display) -> AppError {
    AppError::Call(error.to_string())
}

/// A choice list value (the serde name of an enum variant) as that enum.
fn parse_choice<T: serde::de::DeserializeOwned>(value: &Option<String>, what: &str) -> AppResult<Option<T>> {
    match value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(raw) => serde_json::from_value(Value::String(raw.to_owned()))
            .map(Some)
            .map_err(|_| AppError::Call(format!("`{raw}` is not a {what}."))),
    }
}

/// An enum variant as its choice list value.
fn choice_value<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        _ => String::new(),
    }
}

fn parse_date(value: &Option<String>, what: &str) -> AppResult<Option<chrono::NaiveDate>> {
    match value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(raw) => raw.parse().map(Some).map_err(|_| AppError::Call(format!("Enter the {what} as a real date."))),
    }
}

fn choices(list: &[(&str, &str, &str)]) -> Vec<Choice> {
    list.iter()
        .map(|(value, title, detail)| Choice { value: (*value).to_owned(), title: (*title).to_owned(), detail: (*detail).to_owned() })
        .collect()
}

impl BirApp {
    fn with_tax_profile<R>(&self, f: impl FnOnce(&bir_core::db::Database) -> Result<R, bir_core::tax_profile::TaxProfileError>) -> AppResult<R> {
        let db = self.db.lock().map_err(|_| AppError::Call("the database lock is poisoned".into()))?;
        f(&db).map_err(call_error)
    }

    /// The EOPT tiers, smallest first.
    pub fn eopt_tiers() -> Vec<Choice> {
        choices(&[
            ("Micro", "Micro", "Gross sales under ₱3 million"),
            ("Small", "Small", "₱3 million to under ₱20 million"),
            ("Medium", "Medium", "₱20 million to under ₱1 billion"),
            ("Large", "Large", "₱1 billion and up"),
        ])
    }

    /// The tax classifications a taxpayer of `taxpayer_type` may pick (an
    /// individual's income, a cooperative's exemption); empty for a type
    /// whose classification follows from the type itself.
    pub fn tax_classifications(taxpayer_type: &str) -> Vec<Choice> {
        match taxpayer_type {
            "Individual" => choices(&[
                ("PurelyCompensation", "Purely compensation", "Salary only"),
                ("SelfEmployed", "Self-employed", "Business or profession"),
                ("MixedIncome", "Mixed income", "Salary and business"),
            ]),
            "Cooperative" => choices(&[
                ("CooperativeTaxable", "Taxable cooperative", ""),
                ("CooperativeExempt", "Tax-exempt cooperative", ""),
                ("CooperativeMixed", "Cooperative with mixed income", ""),
            ]),
            _ => Vec::new(),
        }
    }

    /// The income-tax elections a year may record.
    pub fn income_tax_elections() -> Vec<Choice> {
        choices(&[
            ("EightPercent", "8% flat rate", "Instead of graduated rates and percentage tax"),
            ("GraduatedOsd", "Graduated, optional standard deduction", "40% of gross sales or receipts"),
            ("GraduatedItemized", "Graduated, itemized deductions", ""),
            ("GraduatedUnspecified", "Graduated, deductions not chosen yet", ""),
        ])
    }

    /// The taxpayer `tin_root`'s business and filing facts, with its
    /// income-tax election for `election_year`.
    pub fn profile_details(&self, tin_root: &str, election_year: u16) -> AppResult<ProfileDetails> {
        let (stored, election) = self.with_tax_profile(|db| {
            Ok((db.profile_details(tin_root)?, db.income_tax_election(tin_root, election_year)?))
        })?;
        Ok(ProfileDetails {
            line_of_business: stored.line_of_business,
            business_start_date: stored.business_start_date.map(|d| d.to_string()),
            eopt_tier: stored.eopt_tier.as_ref().map(choice_value),
            government_withholding: stored.is_government_withholding_entity,
            tax_classification: stored.tax_classification.as_ref().map(choice_value),
            election_year,
            income_tax_election: election.as_ref().map(choice_value),
            has_employees: stored.has_employees,
            withholds_compensation: stored.withholds_compensation,
            withholds_expanded: stored.withholds_expanded,
            withholds_final: stored.withholds_final,
            vat_registered: stored.is_vat_registered,
            dormant: stored.is_dormant,
            single_employer: stored.has_single_employer,
        })
    }

    /// Save `details` to every branch of `tin_root` (the election to its
    /// year); answers what is stored now.
    pub fn update_profile_details(&self, tin_root: &str, details: &ProfileDetails) -> AppResult<ProfileDetails> {
        let stored = bir_core::tax_profile::ProfileDetails {
            line_of_business: details.line_of_business.trim().to_owned(),
            business_start_date: parse_date(&details.business_start_date, "business start date")?,
            eopt_tier: parse_choice::<EoptTier>(&details.eopt_tier, "EOPT tier")?,
            is_government_withholding_entity: details.government_withholding,
            tax_classification: parse_choice::<TaxClassification>(&details.tax_classification, "tax classification")?,
            has_employees: details.has_employees,
            withholds_compensation: details.withholds_compensation,
            withholds_expanded: details.withholds_expanded,
            withholds_final: details.withholds_final,
            is_vat_registered: details.vat_registered,
            is_dormant: details.dormant,
            has_single_employer: details.single_employer,
        };
        let election = parse_choice::<IncomeTaxElection>(&details.income_tax_election, "income-tax election")?;
        self.with_tax_profile(|db| {
            db.update_profile_details(tin_root, &stored)?;
            if db.income_tax_election(tin_root, details.election_year)? != election {
                db.set_income_tax_election(tin_root, details.election_year, election)?;
            }
            Ok(())
        })?;
        self.profile_details(tin_root, details.election_year)
    }

    /// "Save to profile too" on a return's profile box: write `value` into
    /// the profile fact the box shows (`fact`, from
    /// [`crate::OpenedReturn::profile_facts`]) of the branch `tin`, so later
    /// returns prefill it. A branch fact goes to that branch; the line of
    /// business to every branch.
    pub fn save_box_to_profile(&self, tin: &str, fact: &str, value: &str) -> AppResult<()> {
        if BRANCH_FACTS.contains(&fact) {
            return self.set_branch_fact(tin, fact, value.trim());
        }
        if fact == "line_of_business" {
            let root = tin.get(..9).unwrap_or(tin).to_owned();
            return self.with_tax_profile(|db| {
                let mut stored = db.profile_details(&root)?;
                stored.line_of_business = value.trim().to_owned();
                db.update_profile_details(&root, &stored).map(drop)
            });
        }
        Err(AppError::Call(format!("`{fact}` is not a profile fact a return can save.")))
    }
}
