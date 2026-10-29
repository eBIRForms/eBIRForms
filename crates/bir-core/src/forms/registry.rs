//! Form type registry — static metadata for all supported BIR forms.
//!
//! # Status
//!
//! This module is a **static data file**. Its primary active consumers are:
//! - `news_fetcher::normalize_notice()` — matches article text against `FORM_REGISTRY` form codes
//! - `dashboard.rs` — filters `FormArtifact` lists using `FilingFrequency`
//!
//! The eligibility helper functions (`forms_for_taxpayer`, `forms_for_profile`) are
//! **deprecated** — use `integration::applicable_forms_for_profile_and_year()` instead.

use crate::profile::TaxpayerType;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilingFrequency {
    /// Filed 4 times per year — one per quarter. Counter: 0/4.
    Quarterly,
    /// Filed once per year. Counter: 0/1.
    Annual,
    /// Filed 12 times per year — one per month. Counter: 0/12.
    Monthly,
    /// Filed as many times as needed (e.g. payment forms). No counter.
    OpenEnded,
}

#[derive(Debug, Clone)]
pub struct FormDefinition {
    pub code: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub frequency: FilingFrequency,
    pub taxpayer_types: &'static [TaxpayerType],
    /// None = applies regardless of VAT status.
    /// Some(true) = only for VAT-registered taxpayers.
    /// Some(false) = only for non-VAT taxpayers.
    pub requires_vat: Option<bool>,
    /// If true, this form only applies to taxpayers with employees
    /// (withholding agent forms).
    pub requires_employees: bool,
    pub is_deprecated: bool,
}

impl FormDefinition {
    pub fn deprecation_year(&self) -> Option<u16> {
        if self.code == "1704" {
            return Some(2021);
        }
        // Monthly VAT declarations stopped being a required recurring filing
        // beginning in 2023. Keep the definition for historical returns, but
        // do not place it in current-year obligation sets.
        if self.code == "2550M" {
            return Some(2023);
        }
        if self.is_deprecated {
            match self.code {
                "1601E" | "2551M" => Some(2018),
                _ => Some(2018),
            }
        } else {
            None
        }
    }
}

pub const FORM_REGISTRY: &[FormDefinition] = &[
    // ═══════════════════════════════════════════
    // Payment
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "0605",
        title: "Payment Form",
        category: "Payment",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1905",
        title: "Application for Registration Information Update / Correction / Cancellation",
        category: "Registration",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
            TaxpayerType::Cooperative,
            TaxpayerType::Estate,
            TaxpayerType::Trust,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Withholding Tax (employer / withholding agent forms)
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "1600",
        title: "Monthly Remittance Return of VAT and Other Percentage Taxes Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1600PT",
        title: "Monthly Remittance Return of Other Percentage Taxes Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1600VT",
        title: "Monthly Remittance Return of Value-Added Tax Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1600WP",
        title: "Remittance Return of Percentage Tax on Winnings and Prizes",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1601C",
        title: "Monthly Remittance Return of Income Taxes Withheld on Compensation",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1601E",
        title: "Monthly Remittance Return of Creditable Income Taxes Withheld (Expanded)",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: true,
    },
    FormDefinition {
        code: "1601F",
        title: "Monthly Remittance Return of Final Income Tax Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "0619F",
        title: "Monthly Remittance Form for Final Income Taxes Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1601FQ",
        title: "Quarterly Remittance Return of Final Income Taxes Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1602",
        title: "Monthly Remittance Return of Final Income Taxes Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1602Q",
        title: "Quarterly Remittance Return of Final Taxes Withheld on Interest Paid on Deposits and Yield on Deposit Substitutes / Trusts / Etc.",
        category: "Withholding Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1603",
        title: "Quarterly Remittance Return of Final Income Taxes Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1603Q",
        title: "Quarterly Remittance Return of Final Income Taxes Withheld on Fringe Benefits Paid to Employees Other Than Rank and File",
        category: "Withholding Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    // Since the January 2018 revision BIR files 1604-C and 1604-F separately.
    FormDefinition {
        code: "1604C",
        title: "Annual Information Return of Income Taxes Withheld on Compensation",
        category: "Withholding Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1604F",
        title: "Annual Information Return of Income Payments Subjected to Final Withholding Taxes",
        category: "Withholding Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    // Legacy pre-2018 combined code. Kept only so Forms Sets saved with
    // `1604CF` still resolve; hidden from the picker (see `LEGACY_FORM_CODES`).
    FormDefinition {
        code: "1604CF",
        title: "Annual Information Return of Income Taxes Withheld on Compensation and Final Withholding Taxes (pre-2018)",
        category: "Withholding Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1604E",
        title: "Annual Information Return of Creditable Income Taxes Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1606",
        title: "Withholding Tax Remittance Return for Onerous Transfer of Real Property Other Than Capital Asset (Including Taxable and Exempt)",
        category: "Withholding Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "0620",
        title: "Monthly Remittance Form of Tax Withheld on the Amount Withdrawn from the Decedent's Deposit Account",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2316",
        title: "Certificate of Compensation Payment / Tax Withheld",
        category: "Withholding Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Income Tax — Individual
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "1700",
        title: "Annual Income Tax Return (Purely Compensation)",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[TaxpayerType::Individual],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1701Q",
        title: "Quarterly Income Tax Return for Individuals, Estates and Trusts",
        category: "Income Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Estate,
            TaxpayerType::Trust,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1701",
        title: "Annual Income Tax Return for Individuals, Estates and Trusts",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Estate,
            TaxpayerType::Trust,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1701A",
        title: "Annual Income Tax Return (8% / OSD)",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[TaxpayerType::Individual],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Income Tax — Corporation / Partnership
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "1702Q",
        title: "Quarterly Income Tax Return for Corporations, Partnerships and Cooperatives",
        category: "Income Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
            TaxpayerType::Cooperative,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1702",
        title: "Annual Income Tax Return for Corporations, Partnerships and Cooperatives",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
            TaxpayerType::Cooperative,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1702RT",
        title: "Annual Income Tax Return — Regular Taxable",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
            TaxpayerType::Cooperative,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1702EX",
        title: "Annual Income Tax Return — Tax-Exempt",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
            TaxpayerType::Cooperative,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1702MX",
        title: "Annual Income Tax Return — Mixed Income",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
            TaxpayerType::Cooperative,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1704",
        title: "Improperly Accumulated Earnings Tax Return",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[TaxpayerType::Corporation],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Value-Added Tax
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "2550M",
        title: "Monthly Value-Added Tax Declaration",
        category: "Value-Added Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: Some(true),
        requires_employees: false,
        is_deprecated: false, // Optional per RMC 52-2023; year-aware filtering handles the transition
    },
    FormDefinition {
        code: "2550Q",
        title: "Quarterly Value-Added Tax Return",
        category: "Value-Added Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: Some(true),
        requires_employees: false,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Percentage Tax
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "2551Q",
        title: "Quarterly Percentage Tax Return",
        category: "Percentage Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: Some(false),
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2551M",
        title: "Monthly Percentage Tax Return",
        category: "Percentage Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: Some(false),
        requires_employees: false,
        is_deprecated: true, // Abolished by TRAIN Law RA 10963 (2018) — percentage tax is quarterly-only via 2551Q
    },
    FormDefinition {
        code: "2552",
        title: "Percentage Tax Return on Transactions Involving Shares of Stock",
        category: "Percentage Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2553",
        title: "Percentage Tax Payable Under Special Laws",
        category: "Percentage Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Documentary Stamp Tax
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "2000",
        title: "Documentary Stamp Tax Declaration/Return",
        category: "Documentary Stamp Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2000OT",
        title: "Documentary Stamp Tax Declaration/Return (One-Time Transactions)",
        category: "Documentary Stamp Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Excise Tax
    // ═══════════════════════════════════════════
    // Excise Tax — activity-based, applies to any person (individual or non-individual)
    FormDefinition {
        code: "2200A",
        title: "Excise Tax Return for Alcohol Products",
        category: "Excise Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2200AN",
        title: "Excise Tax Return for Automobiles and Non-Essential Goods",
        category: "Excise Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2200M",
        title: "Excise Tax Return for Mineral Products",
        category: "Excise Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2200P",
        title: "Excise Tax Return for Petroleum Products",
        category: "Excise Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2200T",
        title: "Excise Tax Return for Tobacco Products",
        category: "Excise Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2200C",
        title: "Excise Tax Return for Cosmetic Procedures",
        category: "Excise Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "2200S",
        title: "Excise Tax Return for Sweetened Beverages",
        category: "Excise Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "0619E",
        title: "Monthly Remittance Form for Creditable Income Taxes Withheld (Expanded)",
        category: "Withholding Tax",
        frequency: FilingFrequency::Monthly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1601EQ",
        title: "Quarterly Remittance Return of Creditable Income Taxes Withheld (Expanded)",
        category: "Withholding Tax",
        frequency: FilingFrequency::Quarterly,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: true,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1701MS",
        title: "Annual Income Tax Return for Micro and Small Taxpayers",
        category: "Income Tax",
        frequency: FilingFrequency::Annual,
        taxpayer_types: &[TaxpayerType::Individual],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Capital Gains Tax
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "1706",
        title: "Capital Gains Tax Return (Real Properties)",
        category: "Capital Gains Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1707",
        title: "Capital Gains Tax Return (For Onerous Transfer of Shares of Stock Not Traded Through the Local Stock Exchange)",
        category: "Capital Gains Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1707A",
        title: "Annual Capital Gains Tax Return (Shares of Stock Not Traded)",
        category: "Capital Gains Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    // ═══════════════════════════════════════════
    // Estate and Donor's Tax
    // ═══════════════════════════════════════════
    FormDefinition {
        code: "1800",
        title: "Donor's Tax Return",
        category: "Estate and Donor's Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Corporation,
            TaxpayerType::Partnership,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
    FormDefinition {
        code: "1801",
        title: "Estate Tax Return",
        category: "Estate and Donor's Tax",
        frequency: FilingFrequency::OpenEnded,
        taxpayer_types: &[
            TaxpayerType::Individual,
            TaxpayerType::Estate,
            TaxpayerType::Trust,
        ],
        requires_vat: None,
        requires_employees: false,
        is_deprecated: false,
    },
];

/// Returns forms available to a given taxpayer type.
///
/// # Deprecated
/// This function performs a static registry lookup and is not era-aware.
/// Use [`crate::integration::applicable_forms_for_profile_and_year`] instead.
#[deprecated(
    since = "0.1.0",
    note = "Use integration::applicable_forms_for_profile_and_year() instead"
)]
pub fn forms_for_taxpayer(taxpayer_type: &TaxpayerType) -> Vec<&'static FormDefinition> {
    FORM_REGISTRY
        .iter()
        .filter(|f| f.taxpayer_types.contains(taxpayer_type))
        .collect()
}

/// Registry codes kept only so previously saved Forms Sets still resolve.
/// They are not offered in the Forms & Elections picker.
pub const LEGACY_FORM_CODES: &[&str] = &["1604CF"];

impl FormDefinition {
    /// Whether the Forms & Elections picker should offer this form.
    pub fn is_picker_selectable(&self) -> bool {
        !LEGACY_FORM_CODES.contains(&self.code)
    }
}

/// Forms offered by the Forms & Elections picker (registry minus legacy codes).
pub fn picker_forms() -> impl Iterator<Item = &'static FormDefinition> {
    FORM_REGISTRY.iter().filter(|f| f.is_picker_selectable())
}

/// Find a single form definition by code.
pub fn find_form(code: &str) -> Option<&'static FormDefinition> {
    let canonical = canonical_form_code(code);
    FORM_REGISTRY.iter().find(|f| f.code == canonical)
}

/// Normalize display variants and COR OCR output to the registry's canonical code.
pub fn canonical_form_code(code: &str) -> String {
    let normalized = code.trim().to_ascii_uppercase();
    let compact = normalized
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();

    match compact.as_str() {
        other if FORM_REGISTRY.iter().any(|form| form.code == other) => other.to_string(),
        _ => normalized,
    }
}

/// Returns forms filtered by taxpayer type, VAT status, and employee status.
///
/// # Deprecated
/// This function performs a static registry lookup and is not era-aware.
/// Use [`crate::integration::applicable_forms_for_profile_and_year`] instead.
#[deprecated(
    since = "0.1.0",
    note = "Use integration::applicable_forms_for_profile_and_year() instead"
)]
pub fn forms_for_profile(
    taxpayer_type: &TaxpayerType,
    is_vat_registered: bool,
    has_employees: bool,
) -> Vec<&'static FormDefinition> {
    FORM_REGISTRY
        .iter()
        .filter(|f| {
            f.taxpayer_types.contains(taxpayer_type)
                && match f.requires_vat {
                    None => true,
                    Some(v) => v == is_vat_registered,
                }
                && (!f.requires_employees || has_employees)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_code_normalizes_cor_variants() {
        assert_eq!(canonical_form_code("1604C"), "1604C");
        assert_eq!(canonical_form_code("1604-C"), "1604C");
        assert_eq!(canonical_form_code("1604-F"), "1604F");
        assert_eq!(canonical_form_code("1604CF"), "1604CF");
        assert_eq!(canonical_form_code("1707"), "1707");
        assert_eq!(canonical_form_code("1707-A"), "1707A");
        assert_eq!(canonical_form_code("0619-E"), "0619E");
        assert_eq!(canonical_form_code("1600-PT"), "1600PT");
        assert_eq!(canonical_form_code("1600-VT"), "1600VT");
        assert_eq!(canonical_form_code("1602Q"), "1602Q");
        assert_eq!(canonical_form_code("1603-Q"), "1603Q");
        assert_eq!(canonical_form_code("2000-OT"), "2000OT");
        assert_eq!(canonical_form_code("CUSTOM_FORM"), "CUSTOM_FORM");
    }

    #[test]
    fn registry_keeps_the_five_official_variants_as_distinct_forms() {
        assert_eq!(FORM_REGISTRY.len(), 55);

        for code in ["1600PT", "1600VT", "1602Q", "1603Q", "2000OT"] {
            assert_eq!(
                find_form(code).map(|form| form.code),
                Some(code),
                "{code} must remain independently selectable"
            );
        }

        assert_eq!(
            find_form("1600-PT").map(|form| &form.frequency),
            Some(&FilingFrequency::Monthly)
        );
        assert_eq!(
            find_form("1602Q").map(|form| &form.frequency),
            Some(&FilingFrequency::Quarterly)
        );
        assert_eq!(
            find_form("2000-OT").map(|form| &form.frequency),
            Some(&FilingFrequency::OpenEnded)
        );
    }

    #[test]
    fn registry_contains_open_ended_registration_forms() {
        assert_eq!(
            find_form("0605").map(|form| &form.frequency),
            Some(&FilingFrequency::OpenEnded)
        );
        assert_eq!(
            find_form("1905").map(|form| &form.frequency),
            Some(&FilingFrequency::OpenEnded)
        );
    }

    #[test]
    fn monthly_vat_is_not_a_required_2023_onward_obligation() {
        assert_eq!(
            find_form("2550M").and_then(FormDefinition::deprecation_year),
            Some(2023)
        );
    }

    #[test]
    fn estate_tax_form_applies_to_individual_estate_and_trust() {
        let taxpayer_types = &find_form("1801")
            .expect("1801 should be registered")
            .taxpayer_types;

        assert_eq!(
            *taxpayer_types,
            &[
                TaxpayerType::Individual,
                TaxpayerType::Estate,
                TaxpayerType::Trust,
            ]
        );
    }

    #[test]
    fn every_queueable_form_resolves_and_is_in_the_picker() {
        use crate::forms::queueable::QueueableKind;
        let picker: Vec<&str> = picker_forms().map(|f| f.code).collect();
        for kind in QueueableKind::ALL {
            let code = kind.form_code();
            assert_eq!(
                find_form(code).map(|f| f.code),
                Some(code),
                "{code} must resolve to its own FormDefinition"
            );
            assert!(picker.contains(&code), "{code} missing from picker");
        }
    }

    #[test]
    fn no_alias_maps_one_supported_form_onto_another() {
        use crate::forms::queueable::QueueableKind;
        let supported: Vec<&str> = QueueableKind::ALL.iter().map(|k| k.form_code()).collect();
        for code in &supported {
            assert_eq!(canonical_form_code(code), *code);
        }
        for form in FORM_REGISTRY {
            assert_eq!(canonical_form_code(form.code), form.code);
        }
    }

    #[test]
    fn legacy_1604cf_stays_readable_but_hidden_from_picker() {
        assert_eq!(find_form("1604CF").map(|f| f.code), Some("1604CF"));
        assert!(picker_forms().all(|f| f.code != "1604CF"));
    }

    #[test]
    fn per_transaction_forms_are_open_ended() {
        for code in ["1606", "1707"] {
            assert_eq!(
                find_form(code).map(|f| &f.frequency),
                Some(&FilingFrequency::OpenEnded)
            );
        }
        for code in ["1604C", "1604F"] {
            assert_eq!(
                find_form(code).map(|f| &f.frequency),
                Some(&FilingFrequency::Annual)
            );
        }
    }
}
