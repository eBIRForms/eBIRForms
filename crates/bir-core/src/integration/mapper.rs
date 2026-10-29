//! Form Mapper Engine — translates `UniversalTaxPayload` into strongly-typed form drafts.
//!
//! Each BIR form has a dedicated mapper that understands how to convert
//! generalized financial data into the specific form's data model.

use crate::forms::atc::{AtcRateResolution, find_atc, resolve_2551q_atc_rate};
use crate::forms::form_2551q::{Form2551QDraft, Item13Election, Schedule1Row};
use crate::integration::models::{IncomeCategory, IncomeSource, UniversalTaxPayload};
use crate::profile::TaxpayerProfile;
use chrono::{Datelike, NaiveDate};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MapperError {
    #[error("No applicable form for this payload and profile combination")]
    NoApplicableForm,
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("Invalid period: Form 2551Q imports must cover one exact calendar quarter")]
    InvalidPeriod,
    #[error("ATC code not found: {0}")]
    AtcNotFound(String),
    #[error("Profile TIN mismatch: expected {expected}, got {actual}")]
    TinMismatch { expected: String, actual: String },
    #[error("Validation failed: {0:?}")]
    ValidationFailed(Vec<(String, String)>),
}

/// Output of a form mapper — a strongly-typed draft ready for computation and persistence.
#[derive(Debug, Clone)]
pub enum FormDraftOutput {
    /// A populated 2551Q (Quarterly Percentage Tax Return) draft.
    Form2551Q(Form2551QDraft),
    // Future variants:
    // Form1701Q(Form1701QDraft),
    // Form1701(Form1701Draft),
    // Form2550M(Form2550MDraft),
}

/// Trait for form-specific mappers.
///
/// Each mapper takes a `UniversalTaxPayload` and a `TaxpayerProfile`,
/// and produces a strongly-typed form draft with all derived values computed.
pub trait FormMapper {
    /// The form schema ID this mapper targets (e.g., "2551Qv2018").
    fn target_schema_id(&self) -> &'static str;

    /// The form code this mapper targets (e.g., "2551Q").
    fn target_form_code(&self) -> &'static str;

    /// Map a universal payload into a form draft output.
    ///
    /// The mapper:
    /// 1. Creates a draft from the profile (pre-filling header fields)
    /// 2. Overlays income sources as schedule rows
    /// 3. Calls `recompute()` to derive all calculated fields
    /// 4. Returns the populated draft
    fn map(
        &self,
        payload: &UniversalTaxPayload,
        profile: &TaxpayerProfile,
    ) -> Result<FormDraftOutput, MapperError>;
}

/// Mapper for BIR Form 2551Q — Quarterly Percentage Tax Return.
///
/// Maps `UniversalTaxPayload` income sources to Schedule 1 ATC rows,
/// applies the official January 2018 ATC registry rates and creditable
/// withholdings, and triggers auto-computation of tax due, penalties, and total
/// amount payable.
pub struct Mapper2551Q;

impl Mapper2551Q {
    fn calendar_quarter_period(payload: &UniversalTaxPayload) -> Result<(u16, u8), MapperError> {
        let quarter = payload.quarter().ok_or(MapperError::InvalidPeriod)?;
        let calendar_year = payload.period_end.year();
        let taxable_year = u16::try_from(calendar_year).map_err(|_| MapperError::InvalidPeriod)?;
        let (start_month, end_month, end_day) = match quarter {
            1 => (1, 3, 31),
            2 => (4, 6, 30),
            3 => (7, 9, 30),
            4 => (10, 12, 31),
            _ => return Err(MapperError::InvalidPeriod),
        };
        let expected_start = NaiveDate::from_ymd_opt(calendar_year, start_month, 1)
            .ok_or(MapperError::InvalidPeriod)?;
        let expected_end = NaiveDate::from_ymd_opt(calendar_year, end_month, end_day)
            .ok_or(MapperError::InvalidPeriod)?;
        if payload.period_start != expected_start || payload.period_end != expected_end {
            return Err(MapperError::InvalidPeriod);
        }
        Ok((taxable_year, quarter))
    }

    /// Resolve an explicit official ATC. Universal income categories are not
    /// legal evidence that a source belongs to PT010 (or any other 2551Q ATC),
    /// so imports without a code fail closed for user classification.
    fn resolve_atc_code(
        source: &IncomeSource,
        source_index: usize,
    ) -> Result<&'static str, MapperError> {
        if let Some(override_code) = &source.atc_code_override {
            return find_atc(override_code)
                .map(|entry| entry.code)
                .ok_or_else(|| MapperError::AtcNotFound(override_code.clone()));
        }

        Err(MapperError::MissingField(format!(
            "income_sources[{source_index}].atc_code_override (an official 2551Q ATC is required)"
        )))
    }

    /// Creates Schedule 1 rows from the payload's income sources.
    fn build_schedule_rows(
        sources: &[IncomeSource],
        taxable_year: u16,
        quarter: u8,
        year_end_month: u8,
    ) -> Result<Vec<Schedule1Row>, MapperError> {
        if sources.is_empty() {
            // Default to an empty PT010 row so the draft is valid
            let mut row = Schedule1Row::default_pt010();
            match resolve_2551q_atc_rate("PT010", taxable_year, quarter, year_end_month) {
                Some(AtcRateResolution::Single(rate)) => row.tax_rate = rate,
                Some(AtcRateResolution::RequiresPeriodSplit) => {
                    return Err(MapperError::ValidationFailed(vec![(
                        "tax_period".to_string(),
                        "PT010 receipts span a July statutory rate boundary and must be split before mapping"
                            .to_string(),
                    )]));
                }
                None => unreachable!("PT010 is part of the official 2551Q ATC registry"),
            }
            row.recompute();
            return Ok(vec![row]);
        }

        let mut rows = Vec::with_capacity(sources.len());

        for (index, source) in sources.iter().enumerate() {
            if !source.gross_amount.is_finite()
                || source.gross_amount < 0.0
                || ((source.gross_amount * 100.0) - (source.gross_amount * 100.0).round()).abs()
                    >= 1e-7
            {
                return Err(MapperError::ValidationFailed(vec![(
                    format!("income_sources[{index}].gross_amount"),
                    "Gross amount must be finite, non-negative, and have at most two decimal places"
                        .to_string(),
                )]));
            }
            let atc_code = Self::resolve_atc_code(source, index)?;
            let mut row = Schedule1Row::new(atc_code)
                .ok_or_else(|| MapperError::AtcNotFound(atc_code.to_string()))?;

            let expected_rate = match resolve_2551q_atc_rate(
                atc_code,
                taxable_year,
                quarter,
                year_end_month,
            ) {
                Some(AtcRateResolution::Single(rate)) => rate,
                Some(AtcRateResolution::RequiresPeriodSplit) => {
                    return Err(MapperError::ValidationFailed(vec![(
                        format!("income_sources[{index}].gross_amount"),
                        "PT010 receipts span a July statutory rate boundary and must be split before mapping"
                            .to_string(),
                    )]));
                }
                None => unreachable!("resolved ATC code must remain registered"),
            };

            row.taxable_amount = source.gross_amount;
            row.tax_rate = expected_rate;

            // Form 2551Q rates are fixed by ATC. A universal-payload override
            // may repeat the official rate, but may not replace it.
            if let Some(rate) = source.tax_rate_override
                && (!rate.is_finite() || (rate - expected_rate).abs() > 1e-12)
            {
                return Err(MapperError::ValidationFailed(vec![(
                    format!("income_sources[{index}].tax_rate_override"),
                    format!(
                        "Tax rate override for {atc_code} must match the official rate of {:.2}%",
                        expected_rate * 100.0
                    ),
                )]));
            }

            row.recompute();
            rows.push(row);
        }

        Ok(rows)
    }
}

impl FormMapper for Mapper2551Q {
    fn target_schema_id(&self) -> &'static str {
        "2551Qv2018"
    }

    fn target_form_code(&self) -> &'static str {
        "2551Q"
    }

    fn map(
        &self,
        payload: &UniversalTaxPayload,
        profile: &TaxpayerProfile,
    ) -> Result<FormDraftOutput, MapperError> {
        // Validate TIN match
        let profile_tin = profile.tin.full();
        if profile_tin != payload.tin {
            return Err(MapperError::TinMismatch {
                expected: profile_tin,
                actual: payload.tin.clone(),
            });
        }

        // Derive period
        let (year, quarter) = Self::calendar_quarter_period(payload)?;

        for (field, value) in [
            ("creditable_withholdings", payload.creditable_withholdings),
            ("previous_tax_paid", payload.previous_tax_paid),
        ] {
            if !value.is_finite()
                || value < 0.0
                || ((value * 100.0) - (value * 100.0).round()).abs() >= 1e-7
            {
                return Err(MapperError::ValidationFailed(vec![(
                    field.to_string(),
                    "Amount must be finite, non-negative, and have at most two decimal places"
                        .to_string(),
                )]));
            }
        }

        // Bind imported data to the one confirmed profile segment that covers
        // the exact filing quarter. Missing, undated, overlapping, or
        // out-of-period ledgers remain visible as a blocking resolution error
        // on the draft instead of falling back to stale compatibility flags.
        let mut draft = Form2551QDraft::new_from_effective_profile(profile, year, quarter);

        // Overlay payload data
        draft.is_amended = payload.is_amended;
        draft.creditable_tax_withheld = payload.creditable_withholdings;

        if payload.is_amended {
            draft.tax_paid_previous = payload.previous_tax_paid;
        }

        // Build Schedule 1 from income sources
        draft.schedule_1 = Self::build_schedule_rows(
            &payload.income_sources,
            year,
            quarter,
            draft.year_end_month,
        )?;
        if draft.item_13_is_applicable() == Some(false) {
            draft.item_13_election = Item13Election::NotApplicable;
        }

        if profile.has_8_percent_election(year)
            && draft.schedule_1.iter().any(|row| {
                row.atc == "PT010"
                    && (row.taxable_amount.abs() >= 0.005 || row.tax_due.abs() >= 0.005)
            })
        {
            return Err(MapperError::ValidationFailed(vec![(
                "income_sources".to_string(),
                "PT010 must be NIL for a taxable year covered by the profile's 8% income-tax election"
                    .to_string(),
            )]));
        }

        // Trigger full recomputation (tax due, penalties, totals)
        let expected_sales = match payload.metadata.get("expected_sales") {
            Some(value) => {
                let parsed = value.parse::<f64>().map_err(|_| {
                    MapperError::ValidationFailed(vec![(
                        "metadata.expected_sales".to_string(),
                        "Expected sales must be a finite non-negative number".to_string(),
                    )])
                })?;
                if !parsed.is_finite() || parsed < 0.0 {
                    return Err(MapperError::ValidationFailed(vec![(
                        "metadata.expected_sales".to_string(),
                        "Expected sales must be a finite non-negative number".to_string(),
                    )]));
                }
                Some(parsed)
            }
            None => None,
        };

        draft.recompute(expected_sales);

        Ok(FormDraftOutput::Form2551Q(draft))
    }
}

/// Resolves the appropriate mappers for a given payload and profile.
///
/// If `target_form` is specified, returns only that mapper.
/// Otherwise, auto-detects applicable forms based on income categories.
pub fn resolve_mappers(payload: &UniversalTaxPayload) -> Vec<Box<dyn FormMapper>> {
    let mut mappers: Vec<Box<dyn FormMapper>> = Vec::new();

    if let Some(ref target) = payload.target_form {
        if target.as_str() == "2551Q" {
            mappers.push(Box::new(Mapper2551Q));
            // Future: "1701Q" => mappers.push(Box::new(Mapper1701Q)),
        } // Unknown form — returns empty, caller handles the error
    } else {
        // Auto-detect only a plausible 2551Q candidate. Categories such as
        // passive income, capital gains, and professional services are legally
        // ambiguous without an explicit official 2551Q ATC; they must not
        // silently opt a payload into PT010.
        let has_percentage_tax_income = payload.income_sources.iter().any(|s| {
            matches!(s.category, IncomeCategory::BusinessNonVat)
                || s.atc_code_override
                    .as_deref()
                    .is_some_and(|code| find_atc(code).is_some())
        });

        if has_percentage_tax_income {
            mappers.push(Box::new(Mapper2551Q));
        }

        // Future: check for income tax eligibility
        // let has_income_tax_income = payload.income_sources.iter().any(|s| {
        //     matches!(s.category, IncomeCategory::Compensation | IncomeCategory::ProfessionalServices)
        // });
        // if has_income_tax_income { mappers.push(Box::new(Mapper1701Q)); }
    }

    mappers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::ATC_TABLE_2551Q;
    use crate::naming::Tin;

    fn test_profile() -> TaxpayerProfile {
        let mut profile = TaxpayerProfile {
            id: Some(1),
            full_name: "JUAN DELA CRUZ".to_string(),
            tin: Tin {
                segment1: "000".into(),
                segment2: "234".into(),
                segment3: "564".into(),
                branch: "000".into(),
            },
            rdo_code: "039".to_string(),
            line_of_business: "Consulting Services".to_string(),
            registered_address: "123 Rizal Street, Quezon City".to_string(),
            zip_code: "1100".to_string(),
            phone: "09170000100".to_string(),
            email: "juan@example.com".to_string(),
            default_form_type: "2551Q".to_string(),
            taxpayer_type: crate::profile::TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: NaiveDate::from_ymd_opt(2020, 1, 1),
            birth_date: None,
            is_archived: false,
            profile_pin_hash: None,
            email_tracking_enabled: false,
            test_notification_enabled: false,
            profile_versions: vec![],
            compliance_source_mode: Default::default(),
            per_year_forms: Default::default(),
            tax_classification: None,
            eopt_tier: None,
            is_bmbe: false,
            is_gpp_partner: false,
            is_create_msme: false,
            is_expanded_withholding_agent: false,
            atc_codes: vec![],
            excise_tax_categories: vec![],
            tax_elections: vec![],
            has_employees: false,
            is_dormant: false,
            has_single_employer: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            registration_activity_status: Default::default(),
        };
        profile.ensure_profile_version_ledger();
        profile
    }

    fn test_payload() -> UniversalTaxPayload {
        UniversalTaxPayload {
            tin: "000234564000".to_string(),
            target_form: Some("2551Q".to_string()),
            period_start: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            is_amended: false,
            income_sources: vec![IncomeSource {
                category: IncomeCategory::BusinessNonVat,
                gross_amount: 500_000.0,
                is_vat_exempt: true,
                atc_code_override: Some("PT010".to_string()),
                tax_rate_override: None,
            }],
            creditable_withholdings: 15_000.0,
            previous_tax_paid: 0.0,
            metadata: Default::default(),
        }
    }

    #[test]
    fn test_mapper_2551q_basic() {
        let mapper = Mapper2551Q;
        let profile = test_profile();
        let payload = test_payload();

        let result = mapper.map(&payload, &profile).unwrap();

        match result {
            FormDraftOutput::Form2551Q(draft) => {
                assert_eq!(draft.tin, "000234564000");
                assert_eq!(draft.taxable_year, 2026);
                assert_eq!(draft.quarter, 1);
                assert_eq!(draft.taxpayer_name, "JUAN DELA CRUZ");
                assert_eq!(draft.rdo_code, "039");
                assert!(!draft.is_amended);

                // Schedule 1 should have one PT010 row
                assert_eq!(draft.schedule_1.len(), 1);
                assert_eq!(draft.schedule_1[0].atc, "PT010");
                assert_eq!(draft.schedule_1[0].taxable_amount, 500_000.0);
                assert_eq!(draft.schedule_1[0].tax_rate, 0.03);
                assert_eq!(draft.schedule_1[0].tax_due, 15_000.0);

                // Creditable withholdings
                assert_eq!(draft.creditable_tax_withheld, 15_000.0);

                // Auto-computed totals
                assert_eq!(draft.total_tax_due, 15_000.0);
                // tax_payable = 15000 - 15000 = 0
                assert_eq!(draft.tax_payable, 0.0);
            }
        }
    }

    #[test]
    fn test_mapper_2551q_keeps_unresolved_profile_import_fail_closed() {
        let mapper = Mapper2551Q;
        let mut profile = test_profile();
        profile.profile_versions.clear();

        let FormDraftOutput::Form2551Q(draft) = mapper.map(&test_payload(), &profile).unwrap();

        assert!(
            draft
                .profile_resolution_error
                .as_deref()
                .is_some_and(|message| message.contains("No confirmed"))
        );
        assert!(draft.taxpayer_name.is_empty());
        assert!(draft.rdo_code.is_empty());
    }

    #[test]
    fn test_mapper_2551q_amended() {
        let mapper = Mapper2551Q;
        let profile = test_profile();
        let mut payload = test_payload();
        payload.is_amended = true;
        payload.previous_tax_paid = 5_000.0;

        let result = mapper.map(&payload, &profile).unwrap();

        match result {
            FormDraftOutput::Form2551Q(draft) => {
                assert!(draft.is_amended);
                assert_eq!(draft.tax_paid_previous, 5_000.0);
            }
        }
    }

    #[test]
    fn test_mapper_2551q_tin_mismatch() {
        let mapper = Mapper2551Q;
        let profile = test_profile();
        let mut payload = test_payload();
        payload.tin = "999999999000".to_string();

        let result = mapper.map(&payload, &profile);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MapperError::TinMismatch { .. }
        ));
    }

    #[test]
    fn test_mapper_2551q_multiple_sources() {
        let mapper = Mapper2551Q;
        let profile = test_profile();
        let mut payload = test_payload();
        payload.income_sources = vec![
            IncomeSource {
                category: IncomeCategory::BusinessNonVat,
                gross_amount: 300_000.0,
                is_vat_exempt: true,
                atc_code_override: Some("PT010".to_string()),
                tax_rate_override: None,
            },
            IncomeSource {
                category: IncomeCategory::ProfessionalServices,
                gross_amount: 200_000.0,
                is_vat_exempt: false,
                atc_code_override: Some("PT010".to_string()),
                tax_rate_override: None,
            },
        ];

        let result = mapper.map(&payload, &profile).unwrap();

        match result {
            FormDraftOutput::Form2551Q(draft) => {
                assert_eq!(draft.schedule_1.len(), 2);
                // Both should map to PT010 (3%)
                let total_tax: f64 = draft.schedule_1.iter().map(|r| r.tax_due).sum();
                assert_eq!(total_tax, 15_000.0); // (300k + 200k) * 3%
            }
        }
    }

    #[test]
    fn test_mapper_2551q_atc_override() {
        let mapper = Mapper2551Q;
        let profile = test_profile();
        let mut payload = test_payload();
        payload.income_sources = vec![IncomeSource {
            category: IncomeCategory::BusinessNonVat,
            gross_amount: 100_000.0,
            is_vat_exempt: true,
            atc_code_override: Some("PT060".to_string()), // Gas/water utility at 2%
            tax_rate_override: None,
        }];

        let result = mapper.map(&payload, &profile).unwrap();

        match result {
            FormDraftOutput::Form2551Q(draft) => {
                assert_eq!(draft.schedule_1[0].atc, "PT060");
                assert_eq!(draft.schedule_1[0].tax_rate, 0.02);
                assert_eq!(draft.schedule_1[0].tax_due, 2_000.0); // 100k * 2%
            }
        }
    }

    #[test]
    fn mapper_uses_the_period_specific_pt010_rate() {
        let mapper = Mapper2551Q;
        let profile = test_profile();
        let mut payload = test_payload();
        payload.period_start = NaiveDate::from_ymd_opt(2021, 7, 1).unwrap();
        payload.period_end = NaiveDate::from_ymd_opt(2021, 9, 30).unwrap();
        payload.income_sources[0].gross_amount = 100_000.0;
        payload.income_sources[0].tax_rate_override = Some(0.01);

        let FormDraftOutput::Form2551Q(draft) = mapper
            .map(&payload, &profile)
            .expect("Q3 2021 PT010 must use the temporary statutory rate");
        assert_eq!(draft.taxable_year, 2021);
        assert_eq!(draft.quarter, 3);
        assert_eq!(draft.schedule_1[0].tax_rate, 0.01);
        assert_eq!(draft.schedule_1[0].tax_due, 1_000.0);

        payload.income_sources[0].tax_rate_override = Some(0.03);
        assert!(matches!(
            mapper.map(&payload, &profile),
            Err(MapperError::ValidationFailed(errors))
                if errors.iter().any(|(field, _)| field.ends_with("tax_rate_override"))
        ));
    }

    #[test]
    fn mapper_requires_one_exact_calendar_quarter() {
        let mapper = Mapper2551Q;
        let profile = test_profile();
        for (start, end) in [
            ((2026, 1, 2), (2026, 3, 31)),
            ((2026, 1, 1), (2026, 3, 30)),
            ((2026, 1, 1), (2026, 6, 30)),
            ((2020, 6, 1), (2020, 9, 30)),
        ] {
            let mut payload = test_payload();
            payload.period_start = NaiveDate::from_ymd_opt(start.0, start.1, start.2).unwrap();
            payload.period_end = NaiveDate::from_ymd_opt(end.0, end.1, end.2).unwrap();

            assert!(
                matches!(
                    mapper.map(&payload, &profile),
                    Err(MapperError::InvalidPeriod)
                ),
                "{start:?} through {end:?} must not be mapped as one 2551Q quarter"
            );
        }
    }

    #[test]
    fn mapper_rejects_sub_cent_input_amounts() {
        let mapper = Mapper2551Q;
        let profile = test_profile();

        let mut gross = test_payload();
        gross.income_sources[0].gross_amount = 100.004;
        assert!(matches!(
            mapper.map(&gross, &profile),
            Err(MapperError::ValidationFailed(errors))
                if errors.iter().any(|(field, _)| field.ends_with("gross_amount"))
        ));

        for field in ["creditable_withholdings", "previous_tax_paid"] {
            let mut payload = test_payload();
            payload.is_amended = true;
            if field == "creditable_withholdings" {
                payload.creditable_withholdings = 0.004;
            } else {
                payload.previous_tax_paid = 0.004;
            }
            assert!(matches!(
                mapper.map(&payload, &profile),
                Err(MapperError::ValidationFailed(errors))
                    if errors.iter().any(|(error_field, _)| error_field == field)
            ));
        }
    }

    #[test]
    fn mapper_parses_expected_sales_strictly_and_persists_the_basis() {
        let mapper = Mapper2551Q;
        let profile = test_profile();
        for value in ["100,000", "NaN", "-1"] {
            let mut payload = test_payload();
            payload
                .metadata
                .insert("expected_sales".into(), value.into());
            assert!(matches!(
                mapper.map(&payload, &profile),
                Err(MapperError::ValidationFailed(errors))
                    if errors.iter().any(|(field, _)| field == "metadata.expected_sales")
            ));
        }

        let mut payload = test_payload();
        payload
            .metadata
            .insert("expected_sales".into(), "750000.25".into());
        let FormDraftOutput::Form2551Q(draft) = mapper
            .map(&payload, &profile)
            .expect("a finite non-negative expected-sales basis must map");
        assert_eq!(draft.expected_sales_for_penalties, Some(750_000.25));
        let persisted = serde_json::to_string(&draft).expect("mapped draft must serialize");
        let restored: Form2551QDraft =
            serde_json::from_str(&persisted).expect("mapped draft must deserialize");
        assert_eq!(restored.expected_sales_for_penalties, Some(750_000.25));
    }

    #[test]
    fn mapper_blocks_only_positive_pt010_for_an_eight_percent_year() {
        let mapper = Mapper2551Q;
        let mut profile = test_profile();
        profile
            .tax_elections
            .push(crate::profile::TaxElectionHistory {
                taxable_year: 2026,
                election: crate::profile::IncomeTaxElection::EightPercent,
                elected_at: chrono::NaiveDateTime::default(),
                source_form: "2551Qv2018".into(),
            });

        let mut pt010 = test_payload();
        pt010.target_form = None;
        assert!(matches!(
            mapper.map(&pt010, &profile),
            Err(MapperError::ValidationFailed(errors))
                if errors.iter().any(|(_, message)| message.contains("PT010 must be NIL"))
        ));

        let mut pt040 = test_payload();
        pt040.target_form = None;
        pt040.income_sources[0].atc_code_override = Some("PT040".into());
        let FormDraftOutput::Form2551Q(other_activity) = mapper
            .map(&pt040, &profile)
            .expect("the 8% election must not erase unrelated PT040 liability");
        assert_eq!(other_activity.schedule_1[0].atc, "PT040");
        assert_eq!(
            other_activity.item_13_election,
            Item13Election::NotApplicable
        );

        let mut mixed = test_payload();
        mixed.target_form = None;
        mixed.income_sources = vec![
            IncomeSource {
                category: IncomeCategory::BusinessNonVat,
                gross_amount: 0.0,
                is_vat_exempt: true,
                atc_code_override: Some("PT010".into()),
                tax_rate_override: None,
            },
            IncomeSource {
                category: IncomeCategory::BusinessNonVat,
                gross_amount: 100_000.0,
                is_vat_exempt: true,
                atc_code_override: Some("PT040".into()),
                tax_rate_override: None,
            },
        ];
        let FormDraftOutput::Form2551Q(mixed_activity) = mapper
            .map(&mixed, &profile)
            .expect("NIL PT010 plus taxable PT040 must remain mappable");
        assert_eq!(
            mixed_activity.item_13_election,
            Item13Election::EightPercent
        );
        assert_eq!(mixed_activity.schedule_1[0].tax_due, 0.0);
        assert_eq!(mixed_activity.schedule_1[1].tax_due, 3_000.0);
    }

    #[test]
    fn mapper_rejects_a_fiscal_pt010_quarter_that_crosses_a_rate_boundary() {
        let source = IncomeSource {
            category: IncomeCategory::BusinessNonVat,
            gross_amount: 10_000.0,
            is_vat_exempt: true,
            atc_code_override: Some("PT010".to_string()),
            tax_rate_override: None,
        };

        assert!(matches!(
            Mapper2551Q::build_schedule_rows(&[source], 2020, 4, 8),
            Err(MapperError::ValidationFailed(errors))
                if errors.iter().any(|(_, message)| message.contains("must be split"))
        ));
    }

    #[test]
    fn mapper_round_trips_all_official_2551q_atcs_and_rates() {
        for entry in ATC_TABLE_2551Q {
            let source = IncomeSource {
                category: IncomeCategory::BusinessNonVat,
                gross_amount: 10_000.0,
                is_vat_exempt: true,
                atc_code_override: Some(entry.code.to_string()),
                tax_rate_override: Some(entry.rate),
            };

            let rows = Mapper2551Q::build_schedule_rows(&[source], 2026, 1, 12)
                .expect("official registry ATC must map");
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].atc, entry.code);
            assert_eq!(rows[0].atc_description, entry.description);
            assert!((rows[0].tax_rate - entry.rate).abs() < f64::EPSILON);
            assert_eq!(
                rows[0].tax_due,
                (10_000.0 * entry.rate * 100.0).round() / 100.0
            );
        }
    }

    #[test]
    fn mapper_rejects_retired_invented_and_unknown_atc_overrides() {
        for atc in [
            "PT011", "PT019", "PT050", "PT080", "PT100", "PT110", "PT999",
        ] {
            let source = IncomeSource {
                category: IncomeCategory::BusinessNonVat,
                gross_amount: 10_000.0,
                is_vat_exempt: true,
                atc_code_override: Some(atc.to_string()),
                tax_rate_override: None,
            };

            assert!(matches!(
                Mapper2551Q::build_schedule_rows(&[source], 2026, 1, 12),
                Err(MapperError::AtcNotFound(code)) if code == atc
            ));
        }
    }

    #[test]
    fn mapper_rejects_income_sources_without_an_explicit_atc() {
        for category in [
            IncomeCategory::BusinessNonVat,
            IncomeCategory::ProfessionalServices,
            IncomeCategory::PassiveIncome,
            IncomeCategory::CapitalGains,
        ] {
            let source = IncomeSource {
                category,
                gross_amount: 10_000.0,
                is_vat_exempt: true,
                atc_code_override: None,
                tax_rate_override: None,
            };

            assert!(matches!(
                Mapper2551Q::build_schedule_rows(&[source], 2026, 1, 12),
                Err(MapperError::MissingField(field))
                    if field == "income_sources[0].atc_code_override (an official 2551Q ATC is required)"
            ));
        }
    }

    #[test]
    fn mapper_rejects_a_tax_rate_override_that_differs_from_the_registry() {
        let source = IncomeSource {
            category: IncomeCategory::BusinessNonVat,
            gross_amount: 10_000.0,
            is_vat_exempt: true,
            atc_code_override: Some("PT060".to_string()),
            tax_rate_override: Some(0.03),
        };

        assert!(matches!(
            Mapper2551Q::build_schedule_rows(&[source], 2026, 1, 12),
            Err(MapperError::ValidationFailed(errors))
                if errors.iter().any(|(field, _)| field.ends_with("tax_rate_override"))
        ));
    }

    #[test]
    fn test_resolve_mappers_targeted() {
        let payload = test_payload();
        let mappers = resolve_mappers(&payload);
        assert_eq!(mappers.len(), 1);
        assert_eq!(mappers[0].target_form_code(), "2551Q");
    }

    #[test]
    fn test_resolve_mappers_auto_detect() {
        let mut payload = test_payload();
        payload.target_form = None; // Auto-detect

        let mappers = resolve_mappers(&payload);
        assert_eq!(mappers.len(), 1);
        assert_eq!(mappers[0].target_form_code(), "2551Q");
    }

    #[test]
    fn ambiguous_categories_without_atcs_do_not_auto_select_2551q() {
        for category in [
            IncomeCategory::ProfessionalServices,
            IncomeCategory::PassiveIncome,
            IncomeCategory::CapitalGains,
        ] {
            let mut payload = test_payload();
            payload.target_form = None;
            payload.income_sources[0].category = category;
            payload.income_sources[0].atc_code_override = None;

            assert!(resolve_mappers(&payload).is_empty());
        }
    }

    #[test]
    fn test_resolve_mappers_unknown_form() {
        let mut payload = test_payload();
        payload.target_form = Some("9999X".to_string());

        let mappers = resolve_mappers(&payload);
        assert!(mappers.is_empty());
    }
}
