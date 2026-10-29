//! Universal Tax Payload — the standardized data model for external integrations.
//!
//! External tools push generalized financial summaries using these types.
//! The mapper engine translates them into strongly-typed BIR form drafts.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Category of income for tax mapping purposes.
///
/// Uses an enum rather than a raw String to prevent fragile string-matching
/// in mappers. The `Other` variant provides an escape hatch for edge cases.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IncomeCategory {
    /// Employment compensation income
    Compensation,
    /// Professional or freelancer services income
    ProfessionalServices,
    /// Business income — non-VAT registered (percentage tax)
    BusinessNonVat,
    /// Business income — VAT registered
    BusinessVat,
    /// Passive income (interest, dividends, royalties, etc.)
    PassiveIncome,
    /// Capital gains (sale of shares, real property, etc.)
    CapitalGains,
    /// Catch-all for categories not yet modeled
    Other(String),
}

/// A single income source within the payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomeSource {
    /// The category of this income — drives ATC code selection in mappers.
    pub category: IncomeCategory,
    /// Gross amount for this income source (in PHP).
    pub gross_amount: f64,
    /// Whether this income source is VAT-exempt.
    #[serde(default)]
    pub is_vat_exempt: bool,
    /// Optional ATC code override. If provided, the mapper will use this
    /// instead of auto-detecting from the category and taxpayer profile.
    #[serde(default)]
    pub atc_code_override: Option<String>,
    /// Optional tax rate override (e.g., 0.03 for 3%). If None, the mapper
    /// uses the rate from the ATC table.
    #[serde(default)]
    pub tax_rate_override: Option<f64>,
}

/// The standardized payload that external systems send to eBirForms.
///
/// This model focuses on accounting primitives (income, withholdings, periods)
/// rather than BIR-specific form field names. The mapper engine translates
/// this into the appropriate strongly-typed form drafts.
///
/// # Example (JSON)
///
/// ```json
/// {
///   "tin": "000234564000",
///   "target_form": "2551Q",
///   "period_start": "2026-01-01",
///   "period_end": "2026-03-31",
///   "is_amended": false,
///   "income_sources": [
///     {
///       "category": "BusinessNonVat",
///       "gross_amount": 500000.0,
///       "is_vat_exempt": true
///     }
///   ],
///   "creditable_withholdings": 15000.0,
///   "previous_tax_paid": 0.0
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalTaxPayload {
    /// Taxpayer Identification Number — identifies which profile to use.
    /// Must match an existing `TaxpayerProfile.tin.full()` in the database.
    pub tin: String,

    /// Optional target form code (e.g., "2551Q", "1701Q").
    /// If None, the mapper engine auto-detects applicable forms based on the
    /// taxpayer's profile classification and the income sources provided.
    #[serde(default)]
    pub target_form: Option<String>,

    /// Start of the taxable period (inclusive).
    pub period_start: NaiveDate,

    /// End of the taxable period (inclusive).
    pub period_end: NaiveDate,

    /// Whether this is an amended return. Controls `tax_paid_previous` behavior.
    #[serde(default)]
    pub is_amended: bool,

    /// Income sources to be mapped into form schedule rows.
    pub income_sources: Vec<IncomeSource>,

    /// Total creditable withholding tax (from BIR Form 2307, etc.).
    #[serde(default)]
    pub creditable_withholdings: f64,

    /// Tax already paid in a previously filed return (only relevant when `is_amended` is true).
    #[serde(default)]
    pub previous_tax_paid: f64,

    /// Extensibility escape hatch for integration-specific metadata.
    /// Mappers may inspect this for non-standard fields without breaking
    /// the core schema.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl UniversalTaxPayload {
    /// Derives the taxable year from the period end date.
    pub fn taxable_year(&self) -> u16 {
        self.period_end.year() as u16
    }

    /// Derives the quarter (1–4) from the period end date.
    /// Returns None if the period doesn't align to a standard calendar quarter.
    pub fn quarter(&self) -> Option<u8> {
        match self.period_end.month() {
            1..=3 => Some(1),
            4..=6 => Some(2),
            7..=9 => Some(3),
            10..=12 => Some(4),
            _ => None,
        }
    }

    /// Total gross income across all sources.
    pub fn total_gross_income(&self) -> f64 {
        self.income_sources.iter().map(|s| s.gross_amount).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_deserialization() {
        let json = r#"{
            "tin": "000234564000",
            "target_form": "2551Q",
            "period_start": "2026-01-01",
            "period_end": "2026-03-31",
            "is_amended": false,
            "income_sources": [
                {
                    "category": "BusinessNonVat",
                    "gross_amount": 500000.0,
                    "is_vat_exempt": true
                }
            ],
            "creditable_withholdings": 15000.0,
            "previous_tax_paid": 0.0
        }"#;

        let payload: UniversalTaxPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.tin, "000234564000");
        assert_eq!(payload.target_form, Some("2551Q".to_string()));
        assert_eq!(payload.taxable_year(), 2026);
        assert_eq!(payload.quarter(), Some(1));
        assert_eq!(payload.total_gross_income(), 500000.0);
        assert_eq!(
            payload.income_sources[0].category,
            IncomeCategory::BusinessNonVat
        );
        assert!(payload.metadata.is_empty());
    }

    #[test]
    fn test_payload_minimal() {
        let json = r#"{
            "tin": "123456789000",
            "period_start": "2026-04-01",
            "period_end": "2026-06-30",
            "income_sources": []
        }"#;

        let payload: UniversalTaxPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.target_form, None);
        assert!(!payload.is_amended);
        assert_eq!(payload.creditable_withholdings, 0.0);
        assert_eq!(payload.quarter(), Some(2));
    }

    #[test]
    fn test_income_category_other() {
        let json = r#"{
            "category": { "Other": "CustomCategory" },
            "gross_amount": 100000.0
        }"#;

        let source: IncomeSource = serde_json::from_str(json).unwrap();
        assert_eq!(
            source.category,
            IncomeCategory::Other("CustomCategory".to_string())
        );
        assert_eq!(source.atc_code_override, None);
    }
}
