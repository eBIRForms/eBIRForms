//! Checked editable-save mapping for exact form `1601Cv2018`.
//!
//! The reviewed plaintext save is a 100-field editable snapshot. Its binary
//! encrypted companion decrypts to the same semantic values plus one explicit
//! empty second-address field. Those files prove XML mapping, not submission
//! on their own; queue authority is gated separately by the capability
//! registry and the dedicated immutable 1601C queue path.

use super::FormValidator;
use super::form_1601c::{Form1601CDraft, Form1601CSchedule1Row, MAX_SCHEDULE_1_ROWS};
use std::collections::BTreeMap;

/// The official form id of this rule package and layout.
pub const FORM_1601C_FORM_ID: &str = "1601c-v2018";

impl Form1601CDraft {
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);

        // Header
        insert(
            &mut fields,
            "frm1601c:txtMonth",
            format!("{:02}", self.month),
        );
        insert(
            &mut fields,
            "frm1601c:txtYear",
            self.taxable_year.to_string(),
        );

        insert_bool_1_2(&mut fields, "frm1601c:AmendedRtn", self.is_amended);
        match self.taxes_withheld_answer() {
            Some(value) => insert_bool_1_2(&mut fields, "frm1601c:TaxWithheld", value),
            // Unanswered Item 3: both radios unchecked, as the page opens.
            None => {
                insert(&mut fields, "frm1601c:TaxWithheld_1", "false".to_string());
                insert(&mut fields, "frm1601c:TaxWithheld_2", "false".to_string());
            }
        }

        insert(
            &mut fields,
            "frm1601c:txtSheets",
            self.number_of_sheets.to_string(),
        );
        insert(&mut fields, "frm1601c:txtATC", self.atc.clone());

        // Identity
        insert(&mut fields, "frm1601c:txtTIN1", tin1.clone());
        insert(&mut fields, "frm1601c:txtTIN2", tin2.clone());
        insert(&mut fields, "frm1601c:txtTIN3", tin3.clone());
        // Page 1's `maxlength="3"` only limits typing: official fills both
        // branch fields from the profile `tin4` (HTA L3779–3780), which
        // `padZeros` pads to 5 digits, so both carry the same 5-wide value.
        insert(&mut fields, "frm1601c:txtBranchCode", branch.clone());
        insert(&mut fields, "frm1601c:txtRDOCode", self.rdo_code.clone());

        // Use encoded spaces if requested by original XML format, but text should be fine
        // Name, address and line of business are uppercased to mirror the
        // official `capital()` onblur handler. Profile: official-match. The
        // email is not: the effective `capital()` (string-util.js L236) skips
        // `txtEmail`, so it is submitted as entered and only printed in
        // capitals by the renderer.
        insert(
            &mut fields,
            "frm1601c:txtTaxpayerName",
            self.taxpayer_name.to_uppercase(),
        );
        insert(
            &mut fields,
            "frm1601c:txtAddress",
            self.registered_address.to_uppercase(),
        );
        // The upload writes the Address 2 control even when it is blank.
        insert(
            &mut fields,
            "frm1601c:txtAddress2",
            self.registered_address_2.to_uppercase(),
        );
        insert(&mut fields, "frm1601c:txtZipCode", self.zip_code.clone());
        insert(
            &mut fields,
            "frm1601c:txtTelNum",
            self.contact_number.clone(),
        );

        insert_bool(
            &mut fields,
            "frm1601c:CatAgent_P",
            self.category_of_agent == "P",
        );
        insert_bool(
            &mut fields,
            "frm1601c:CatAgent_G",
            self.category_of_agent == "G",
        );

        insert(&mut fields, "txtEmail", self.email_address.clone());

        // Item 13 — Tax Relief / Treaty
        insert_bool_1_2(&mut fields, "frm1601c:SpecialTax", self.tax_relief);
        insert(
            &mut fields,
            "frm1601c:selTreaty",
            if self.tax_relief {
                // `validate` rejects unmapped values; if one reaches here
                // anyway, keep it verbatim rather than emit "0" (no relief)
                // under SpecialTax = Yes.
                crate::validation::official_tax_relief_code(&self.tax_relief_specification, true)
                    .unwrap_or_else(|| self.tax_relief_specification.trim().to_string())
            } else {
                "0".to_string()
            },
        );

        // Part II - Computation
        insert_money(
            &mut fields,
            "frm1601c:txtTax14",
            self.tax_14_total_compensation,
        );
        insert_money(
            &mut fields,
            "frm1601c:txtTax15",
            self.tax_15_statutory_minimum_wage,
        );
        insert_money(&mut fields, "frm1601c:txtTax16", self.tax_16_holiday_pay);
        insert_money(&mut fields, "frm1601c:txtTax17", self.tax_17_13th_month_pay);
        insert_money(&mut fields, "frm1601c:txtTax18", self.tax_18_de_minimis);
        insert_money(&mut fields, "frm1601c:txtTax19", self.tax_19_sss_gsis);

        insert(
            &mut fields,
            "frm1601c:txt20Other",
            self.tax_20_other_name.clone(),
        );
        insert_money(&mut fields, "frm1601c:txtTax20", self.tax_20_other_amount);

        insert_money(
            &mut fields,
            "frm1601c:txtTax21",
            self.tax_21_total_non_taxable,
        );
        insert_money(&mut fields, "frm1601c:txtTax22", self.tax_22_total_taxable);
        insert_money(&mut fields, "frm1601c:txtTax23", self.tax_23_not_subject);
        insert_money(&mut fields, "frm1601c:txtTax24", self.tax_24_net_taxable);
        insert_money(
            &mut fields,
            "frm1601c:txtTax25",
            self.tax_25_total_taxes_withheld,
        );
        insert_money(&mut fields, "frm1601c:txtTax26", self.tax_26_adjustment);
        insert_money(
            &mut fields,
            "frm1601c:txtTax27",
            self.tax_27_taxes_withheld_for_remittance,
        );
        // `1601c-input-011`: on a non-amended return the official handler
        // disables Item 28 and resets it to 0.00; the serialized field mirrors
        // that even when validate() was bypassed. Profile: official-match.
        insert_money(
            &mut fields,
            "frm1601c:txtTax28",
            if self.is_amended {
                self.tax_28_tax_remitted_previously
            } else {
                0.0
            },
        );

        insert(
            &mut fields,
            "frm1601c:txt29Other",
            self.tax_29_other_remittances_name.clone(),
        );
        insert_money(
            &mut fields,
            "frm1601c:txtTax29",
            self.tax_29_other_remittances_amount,
        );

        insert_money(
            &mut fields,
            "frm1601c:txtTax30",
            self.tax_30_total_tax_remittances,
        );
        insert_money(&mut fields, "frm1601c:txtTax31", self.tax_31_tax_still_due);
        insert_money(&mut fields, "frm1601c:txtTax32", self.tax_32_surcharge);
        insert_money(&mut fields, "frm1601c:txtTax33", self.tax_33_interest);
        insert_money(&mut fields, "frm1601c:txtTax34", self.tax_34_compromise);
        insert_money(
            &mut fields,
            "frm1601c:txtTax35",
            self.tax_35_total_penalties,
        );
        insert_money(
            &mut fields,
            "frm1601c:txtTax36",
            self.tax_36_total_amount_payable,
        );

        // Part III - Details of Payment
        insert(&mut fields, "txtTaxAgentNo", "");
        insert(&mut fields, "txtDateIssue", "");
        insert(&mut fields, "txtDateExpiry", "");

        for i in 37..=40 {
            if i != 39 {
                insert(&mut fields, &format!("frm1601c:txtAgency{}", i), "");
            }
            insert(&mut fields, &format!("frm1601c:txtNumber{}", i), "");
            insert(&mut fields, &format!("frm1601c:txtDate{}", i), "");
            insert(&mut fields, &format!("frm1601c:txtAmount{}", i), "");
        }
        insert(&mut fields, "frm1601c:txtParticular40", "");

        // Page 2 Header
        insert(&mut fields, "frm1601c:txtPg2TIN1", tin1);
        insert(&mut fields, "frm1601c:txtPg2TIN2", tin2);
        insert(&mut fields, "frm1601c:txtPg2TIN3", tin3);
        insert(&mut fields, "frm1601c:txtPg2BranchCode", branch);
        insert(
            &mut fields,
            "frm1601c:txtPg2TaxpayerName",
            self.taxpayer_name.to_uppercase(),
        );

        // Schedule I — the verified 1601-C payload exposes three rows.
        for i in 0..MAX_SCHEDULE_1_ROWS {
            let row = self.schedule_1.get(i);
            insert(&mut fields, &format!("chkScheduleDelete{}", i), "false");
            insert(
                &mut fields,
                &format!("frm1601c:sched1:txtMonthYear{}", i),
                row.map(|value| value.previous_month.as_str()).unwrap_or(""),
            );
            insert(
                &mut fields,
                &format!("frm1601c:sched1:txtDatePaid{}", i),
                row.map(|value| value.date_paid.as_str()).unwrap_or(""),
            );
            insert(
                &mut fields,
                &format!("frm1601c:sched1:txtBankCode{}", i),
                row.map(|value| value.drawee_bank_code_or_agency.as_str())
                    .unwrap_or(""),
            );
            insert(
                &mut fields,
                &format!("frm1601c:sched1:txtNumber{}", i),
                row.map(|value| value.payment_number.as_str()).unwrap_or(""),
            );
            insert_money(
                &mut fields,
                &format!("frm1601c:sched1:txtTaxPaid{}", i),
                row.map(|value| value.tax_paid).unwrap_or(0.0),
            );
            insert_money(
                &mut fields,
                &format!("frm1601c:sched1:txtShouldTaxDue{}", i),
                row.map(|value| value.should_be_tax_due).unwrap_or(0.0),
            );
            insert_money(
                &mut fields,
                &format!("frm1601c:sched1:txtAdjustments{}", i),
                row.map(|value| value.adjustment).unwrap_or(0.0),
            );
        }
        insert_money(
            &mut fields,
            "frm1601c:sched1:txtTotal1",
            self.tax_26_adjustment,
        );

        // Pagination
        insert(&mut fields, "frm1601c:txtCurrentPage", "1");
        insert(&mut fields, "frm1601c:txtMaxPage", "2");
        insert(
            &mut fields,
            "frm1601c:txtLineBus",
            self.line_of_business.to_uppercase(),
        );

        fields
    }

    /// The official submit plaintext for this draft, byte for byte as the
    /// official `saveXMLsubmit()` writes it (see [`crate::official_xml`]).
    pub fn to_bir_xml_payload(&self) -> String {
        let layout = crate::official_xml::layout(FORM_1601C_FORM_ID)
            .expect("the 1601C official layout is packaged");
        crate::official_xml::write(layout, &self.to_bir_field_map())
            .expect("the 1601C field map holds only official controls")
    }

    /// Generate XML only when the draft satisfies the verified model and
    /// three-row Schedule I capacity. Submission remains separately governed
    /// by the form capability registry.
    pub fn try_to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = self.validate();
        if errors.is_empty() {
            Ok(self.to_bir_xml_payload())
        } else {
            Err(errors)
        }
    }

    /// Parse a 1601-C January 2018 save payload into typed Rust state.
    pub fn from_bir_xml_payload(xml: &str) -> Result<Self, Vec<(String, String)>> {
        let fields = crate::bir_xml::parse_bir_xml_checked(xml).map_err(|error| {
            vec![(
                "xml_payload".to_string(),
                format!("invalid 1601-C save payload: {error}"),
            )]
        })?;
        Self::from_bir_field_map(&fields)
    }

    /// Parse the reviewed 1601-C field contract and verify every official
    /// computed value represented by the source payload.
    pub fn from_bir_field_map(
        fields: &BTreeMap<String, String>,
    ) -> Result<Self, Vec<(String, String)>> {
        let mut errors = Vec::new();
        let month = parse_required::<u8>(fields, "frm1601c:txtMonth", &mut errors);
        let taxable_year = parse_required::<u16>(fields, "frm1601c:txtYear", &mut errors);
        let is_amended = parse_bool_pair(fields, "frm1601c:AmendedRtn", &mut errors);
        // Both Item 3 radios unchecked is the page's unanswered state.
        let any_taxes_withheld_unanswered = matches!(
            (
                field(fields, "frm1601c:TaxWithheld_1"),
                field(fields, "frm1601c:TaxWithheld_2"),
            ),
            ("false", "false")
        );
        let any_taxes_withheld = if any_taxes_withheld_unanswered {
            Some(false)
        } else {
            parse_bool_pair(fields, "frm1601c:TaxWithheld", &mut errors)
        };
        let tax_relief = parse_bool_pair(fields, "frm1601c:SpecialTax", &mut errors);

        let category_private = parse_bool(fields, "frm1601c:CatAgent_P", &mut errors);
        let category_government = parse_bool(fields, "frm1601c:CatAgent_G", &mut errors);
        let category_of_agent = match (category_private, category_government) {
            (Some(true), Some(false)) => Some("P".to_string()),
            (Some(false), Some(true)) => Some("G".to_string()),
            // Unanswered Item 11, as the page opens.
            (Some(false), Some(false)) => Some(String::new()),
            (Some(_), Some(_)) => {
                errors.push((
                    "category_of_agent".to_string(),
                    "1601-C requires exactly one category of withholding agent".to_string(),
                ));
                None
            }
            _ => None,
        };

        let mut schedule_1 = Vec::new();
        let mut source_adjustments = Vec::new();
        for index in 0..MAX_SCHEDULE_1_ROWS {
            let previous_month = field(fields, &format!("frm1601c:sched1:txtMonthYear{index}"));
            let date_paid = field(fields, &format!("frm1601c:sched1:txtDatePaid{index}"));
            let bank = field(fields, &format!("frm1601c:sched1:txtBankCode{index}"));
            let number = field(fields, &format!("frm1601c:sched1:txtNumber{index}"));
            let tax_paid = parse_optional_money(
                fields,
                &format!("frm1601c:sched1:txtTaxPaid{index}"),
                &mut errors,
            );
            let should_be_tax_due = parse_optional_money(
                fields,
                &format!("frm1601c:sched1:txtShouldTaxDue{index}"),
                &mut errors,
            );
            let source_adjustment = parse_optional_money(
                fields,
                &format!("frm1601c:sched1:txtAdjustments{index}"),
                &mut errors,
            );

            let occupied = [previous_month, date_paid, bank, number]
                .iter()
                .any(|value| !value.trim().is_empty())
                || tax_paid != 0.0
                || should_be_tax_due != 0.0
                || source_adjustment != 0.0;
            if occupied {
                schedule_1.push(Form1601CSchedule1Row {
                    previous_month: previous_month.to_string(),
                    date_paid: date_paid.to_string(),
                    drawee_bank_code_or_agency: bank.to_string(),
                    payment_number: number.to_string(),
                    tax_paid,
                    should_be_tax_due,
                    adjustment: source_adjustment,
                });
                source_adjustments.push(source_adjustment);
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        let now = chrono::Utc::now().to_rfc3339();
        let treaty = field(fields, "frm1601c:selTreaty");
        let mut draft = Form1601CDraft {
            id: None,
            tin: format!(
                "{}{}{}{}",
                field(fields, "frm1601c:txtTIN1"),
                field(fields, "frm1601c:txtTIN2"),
                field(fields, "frm1601c:txtTIN3"),
                field(fields, "frm1601c:txtBranchCode")
            ),
            taxable_year: taxable_year.unwrap_or_default(),
            month: month.unwrap_or_default(),
            is_amended: is_amended.unwrap_or(false),
            any_taxes_withheld: any_taxes_withheld.unwrap_or(false),
            any_taxes_withheld_unanswered,
            number_of_sheets: parse_optional::<u32>(fields, "frm1601c:txtSheets", &mut errors),
            atc: field(fields, "frm1601c:txtATC").to_string(),
            rdo_code: field(fields, "frm1601c:txtRDOCode").to_string(),
            line_of_business: field(fields, "frm1601c:txtLineBus").to_string(),
            taxpayer_name: field(fields, "frm1601c:txtTaxpayerName").to_string(),
            contact_number: field(fields, "frm1601c:txtTelNum").to_string(),
            registered_address: field(fields, "frm1601c:txtAddress").to_string(),
            registered_address_2: field(fields, "frm1601c:txtAddress2").to_string(),
            zip_code: field(fields, "frm1601c:txtZipCode").to_string(),
            category_of_agent: category_of_agent.unwrap_or_default(),
            email_address: field(fields, "txtEmail").to_string(),
            tax_relief: tax_relief.unwrap_or(false),
            tax_relief_specification: if tax_relief.unwrap_or(false) && treaty != "0" {
                treaty.to_string()
            } else {
                String::new()
            },
            tax_14_total_compensation: parse_optional_money(
                fields,
                "frm1601c:txtTax14",
                &mut errors,
            ),
            tax_15_statutory_minimum_wage: parse_optional_money(
                fields,
                "frm1601c:txtTax15",
                &mut errors,
            ),
            tax_16_holiday_pay: parse_optional_money(fields, "frm1601c:txtTax16", &mut errors),
            tax_17_13th_month_pay: parse_optional_money(fields, "frm1601c:txtTax17", &mut errors),
            tax_18_de_minimis: parse_optional_money(fields, "frm1601c:txtTax18", &mut errors),
            tax_19_sss_gsis: parse_optional_money(fields, "frm1601c:txtTax19", &mut errors),
            tax_20_other_name: field(fields, "frm1601c:txt20Other").to_string(),
            tax_20_other_amount: parse_optional_money(fields, "frm1601c:txtTax20", &mut errors),
            tax_21_total_non_taxable: 0.0,
            tax_22_total_taxable: 0.0,
            tax_23_not_subject: parse_optional_money(fields, "frm1601c:txtTax23", &mut errors),
            tax_24_net_taxable: 0.0,
            tax_25_total_taxes_withheld: parse_optional_money(
                fields,
                "frm1601c:txtTax25",
                &mut errors,
            ),
            tax_26_adjustment: 0.0,
            schedule_1,
            tax_27_taxes_withheld_for_remittance: 0.0,
            tax_28_tax_remitted_previously: parse_optional_money(
                fields,
                "frm1601c:txtTax28",
                &mut errors,
            ),
            tax_29_other_remittances_name: field(fields, "frm1601c:txt29Other").to_string(),
            tax_29_other_remittances_amount: parse_optional_money(
                fields,
                "frm1601c:txtTax29",
                &mut errors,
            ),
            tax_30_total_tax_remittances: 0.0,
            tax_31_tax_still_due: 0.0,
            auto_compute_penalties: false,
            tax_32_surcharge: parse_optional_money(fields, "frm1601c:txtTax32", &mut errors),
            tax_33_interest: parse_optional_money(fields, "frm1601c:txtTax33", &mut errors),
            tax_34_compromise: parse_optional_money(fields, "frm1601c:txtTax34", &mut errors),
            penalty: crate::forms::penalty_items::PenaltyState::imported(),
            tax_35_total_penalties: 0.0,
            tax_36_total_amount_payable: 0.0,
            status: super::FilingStatus::Draft,
            created_at: now.clone(),
            updated_at: now,
            submitted_at: None,
            confirmed_at: None,
            submission_filename: None,
            receipt_id: None,
            submitted_payload: None,
            queued_submission_fingerprint: None,
            queue_authorization: None,
            submission_claim_token: None,
            submission_claimed_at: None,
            submission_claim_lease_until: None,
            submission_put_started_at: None,
            submission_attempts: 0,
            submission_error: None,
            next_retry_at: None,
            profile_edits: Vec::new(),
            profile_baseline: std::collections::BTreeMap::new(),
        };
        if !errors.is_empty() {
            return Err(errors);
        }

        // An imported return carries the penalties it was prepared with:
        // keep each one as an override so the source amounts round-trip.
        Form1601CDraft::penalty_items().adopt_imported(&mut draft);
        draft.compute();
        for (index, source_adjustment) in source_adjustments.into_iter().enumerate() {
            if (draft.schedule_1[index].adjustment - source_adjustment).abs() > 0.001 {
                errors.push((
                    format!("schedule_1_row_{}", index + 1),
                    format!(
                        "Schedule I row {} source adjustment does not equal Item 6 less Item 5",
                        index + 1
                    ),
                ));
            }
        }
        verify_source_money(
            fields,
            "frm1601c:sched1:txtTotal1",
            draft.tax_26_adjustment,
            &mut errors,
        );
        for (key, computed) in [
            ("frm1601c:txtTax21", draft.tax_21_total_non_taxable),
            ("frm1601c:txtTax22", draft.tax_22_total_taxable),
            ("frm1601c:txtTax24", draft.tax_24_net_taxable),
            ("frm1601c:txtTax26", draft.tax_26_adjustment),
            (
                "frm1601c:txtTax27",
                draft.tax_27_taxes_withheld_for_remittance,
            ),
            ("frm1601c:txtTax30", draft.tax_30_total_tax_remittances),
            ("frm1601c:txtTax31", draft.tax_31_tax_still_due),
            ("frm1601c:txtTax35", draft.tax_35_total_penalties),
            ("frm1601c:txtTax36", draft.tax_36_total_amount_payable),
        ] {
            verify_source_money(fields, key, computed, &mut errors);
        }

        errors.extend(draft.validate());

        if errors.is_empty() {
            Ok(draft)
        } else {
            Err(errors)
        }
    }
}

fn field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    fields.get(key).map(String::as_str).unwrap_or("")
}

fn parse_required<T: std::str::FromStr>(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<T> {
    let value = field(fields, key);
    if value.trim().is_empty() {
        errors.push((
            key.to_string(),
            format!("Required 1601-C field {key} is missing"),
        ));
        return None;
    }
    match value.parse::<T>() {
        Ok(parsed) => Some(parsed),
        Err(_) => {
            errors.push((
                key.to_string(),
                format!("1601-C field {key} has invalid value {value:?}"),
            ));
            None
        }
    }
}

fn parse_optional<T: std::str::FromStr + Default>(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> T {
    let value = field(fields, key);
    if value.trim().is_empty() {
        return T::default();
    }
    value.parse::<T>().unwrap_or_else(|_| {
        errors.push((
            key.to_string(),
            format!("1601-C field {key} has invalid value {value:?}"),
        ));
        T::default()
    })
}

fn parse_optional_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> f64 {
    // Official amounts carry thousands separators: `1,234.50`.
    let Some(value) = crate::official_xml::parse_official_amount(field(fields, key)) else {
        errors.push((
            key.to_string(),
            format!(
                "1601-C field {key} has invalid value {:?}",
                field(fields, key)
            ),
        ));
        return 0.0;
    };
    if !value.is_finite() {
        errors.push((
            key.to_string(),
            format!("1601-C money field {key} must be finite"),
        ));
        0.0
    } else {
        value
    }
}

fn parse_bool(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    match field(fields, key).trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        value => {
            errors.push((
                key.to_string(),
                format!("1601-C boolean field {key} has invalid value {value:?}"),
            ));
            None
        }
    }
}

fn parse_bool_pair(
    fields: &BTreeMap<String, String>,
    base: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    let yes = parse_bool(fields, &format!("{base}_1"), errors);
    let no = parse_bool(fields, &format!("{base}_2"), errors);
    match (yes, no) {
        (Some(true), Some(false)) => Some(true),
        (Some(false), Some(true)) => Some(false),
        (Some(_), Some(_)) => {
            errors.push((
                base.to_string(),
                format!("1601-C choice {base} must contain exactly one selected value"),
            ));
            None
        }
        _ => None,
    }
}

fn verify_source_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    computed: f64,
    errors: &mut Vec<(String, String)>,
) {
    if !fields.contains_key(key) {
        return;
    }
    let source = parse_optional_money(fields, key, errors);
    if (source - computed).abs() > 0.001 {
        errors.push((
            key.to_string(),
            format!(
                "1601-C source value {source:.2} does not match the official computed value {computed:.2}"
            ),
        ));
    }
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    if tin.contains('-') {
        let parts: Vec<&str> = tin.split('-').collect();
        let branch = parts.get(3).copied().unwrap_or("00000");
        return (
            parts.first().copied().unwrap_or("").to_string(),
            parts.get(1).copied().unwrap_or("").to_string(),
            parts.get(2).copied().unwrap_or("").to_string(),
            format!("{:0>5}", branch),
        );
    }

    let digits: String = tin.chars().filter(|ch| ch.is_ascii_digit()).collect();
    let segment = |start: usize, end: usize| -> String {
        digits
            .get(start..end.min(digits.len()))
            .unwrap_or("")
            .to_string()
    };

    let branch_raw = digits.get(9..).filter(|s| !s.is_empty()).unwrap_or("00000");

    (
        segment(0, 3),
        segment(3, 6),
        segment(6, 9),
        format!("{:0>5}", branch_raw),
    )
}

fn insert(map: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    map.insert(key.to_string(), value.into());
}

fn insert_bool(map: &mut BTreeMap<String, String>, key: &str, value: bool) {
    insert(map, key, if value { "true" } else { "false" });
}

fn insert_bool_1_2(map: &mut BTreeMap<String, String>, key: &str, value: bool) {
    insert(
        map,
        &format!("{}_1", key),
        if value { "true" } else { "false" },
    );
    insert(
        map,
        &format!("{}_2", key),
        if !value { "true" } else { "false" },
    );
}

/// Official money text: `1,234.50` (see [`crate::official_xml::official_amount`]).
fn insert_money(map: &mut BTreeMap<String, String>, key: &str, value: f64) {
    insert(map, key, crate::official_xml::official_amount(value));
}

#[cfg(test)]
mod tests {
    use super::super::form_1601c::EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT as EXACT_REVIEWED_PLAIN_XML_FIELD_COUNT;
    use super::*;
    use crate::naming::Tin;
    use crate::profile::{TaxpayerProfile, TaxpayerType};

    fn test_profile() -> TaxpayerProfile {
        TaxpayerProfile {
            id: None,
            full_name: "Test Corp".into(),
            tin: Tin {
                segment1: "123".into(),
                segment2: "456".into(),
                segment3: "789".into(),
                branch: "00000".into(),
            },
            rdo_code: "043".into(),
            line_of_business: "Retail".into(),
            registered_address: "Manila".into(),
            zip_code: "1000".into(),
            phone: "09123456789".into(),
            email: "test@example.com".into(),
            default_form_type: "1601C".into(),
            taxpayer_type: TaxpayerType::Corporation,
            is_vat_registered: true,
            business_start_date: None,
            birth_date: None,
            is_archived: false,
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
            has_employees: true,
            is_dormant: false,
            has_single_employer: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            registration_activity_status: Default::default(),
            profile_pin_hash: None,
        }
    }

    #[test]
    fn test_1601c_xml_generation() {
        let profile = test_profile();

        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 5);
        draft.auto_compute_penalties = false;
        draft.tax_14_total_compensation = 100_000.0;
        draft.tax_17_13th_month_pay = 20_000.0;
        draft.tax_25_total_taxes_withheld = 15_000.0;
        // Pin Items 32-34 at zero: the period is past its deadline, and this
        // test is not about penalties, so they must not grow with the clock.
        for component in crate::penalties::PenaltyComponent::ALL {
            draft.set_penalty_override(component, Some(0.0));
        }
        draft.compute();

        let field_map = draft.to_bir_field_map();

        // Official amounts carry thousands separators (formatCurrency/round).
        assert_eq!(field_map["frm1601c:txtTax14"], "100,000.00");
        assert_eq!(field_map["frm1601c:txtTax17"], "20,000.00");
        assert_eq!(field_map["frm1601c:txtTax21"], "20,000.00"); // 15 to 20 = 20k
        assert_eq!(field_map["frm1601c:txtTax22"], "80,000.00"); // 14 - 21 = 80k
        assert_eq!(field_map["frm1601c:txtTax27"], "15,000.00"); // Taxes withheld
        assert_eq!(field_map["frm1601c:txtTax36"], "15,000.00"); // Total payable

        let xml = draft.to_bir_xml_payload();
        assert!(xml.contains("<div>frm1601c:txtTax22=80,000.00frm1601c:txtTax22=</div>"));
    }

    fn valid_schedule_row(index: usize) -> Form1601CSchedule1Row {
        Form1601CSchedule1Row {
            previous_month: format!("{:02}/2026", index + 1),
            date_paid: format!("{:02}/10/2026", index + 1),
            drawee_bank_code_or_agency: format!("AAB-{}", index + 1),
            payment_number: format!("REF-{}", index + 1),
            tax_paid: (index as f64 + 1.0) * 100.0,
            should_be_tax_due: (index as f64 + 1.0) * 125.0,
            adjustment: 0.0,
        }
    }

    #[test]
    fn item_13_and_three_schedule_rows_round_trip_through_xml() {
        let mut profile = test_profile();
        profile.registered_address = "First address line".into();
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 4);
        draft.registered_address_2 = "Second address line".into();
        draft.tax_relief = true;
        draft.tax_relief_specification = "International Tax Treaty".into();
        draft.auto_compute_penalties = false;
        draft.tax_14_total_compensation = 100_000.0;
        draft.tax_25_total_taxes_withheld = 1_000.0;
        draft.schedule_1 = (0..MAX_SCHEDULE_1_ROWS).map(valid_schedule_row).collect();
        draft.compute();

        let xml = draft
            .try_to_bir_xml_payload()
            .expect("a valid three-row draft should generate XML");
        let parsed = Form1601CDraft::from_bir_xml_payload(&xml)
            .expect("generated 1601-C XML should parse back into typed state");

        assert!(parsed.tax_relief);
        assert_eq!(parsed.tax_relief_specification, "2");
        // The upload (saveEncryptedProfile) writes txtAddress and txtAddress2
        // as separate controls, so both lines round-trip.
        assert_eq!(parsed.registered_address, "FIRST ADDRESS LINE");
        assert_eq!(parsed.registered_address_2, "SECOND ADDRESS LINE");
        assert_eq!(parsed.schedule_1, draft.schedule_1);
        assert_eq!(parsed.tax_26_adjustment, 150.0);
        assert_eq!(parsed.tax_27_taxes_withheld_for_remittance, 1_150.0);
    }

    #[test]
    fn government_category_round_trips_and_rejects_both_xboxes() {
        let profile = test_profile();
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 4);
        draft.any_taxes_withheld = false;
        draft.category_of_agent = "G".to_string();
        draft.compute();

        let xml = draft
            .try_to_bir_xml_payload()
            .expect("government category draft should generate XML");
        let parsed = Form1601CDraft::from_bir_xml_payload(&xml)
            .expect("government category XML should parse");
        assert_eq!(parsed.category_of_agent, "G");

        let mut fields = draft.to_bir_field_map();
        fields.insert("frm1601c:CatAgent_P".to_string(), "true".to_string());
        fields.insert("frm1601c:CatAgent_G".to_string(), "true".to_string());
        let errors = Form1601CDraft::from_bir_field_map(&fields)
            .expect_err("both category xboxes must be rejected");
        assert!(errors.iter().any(|(field, message)| {
            field == "category_of_agent" && message.contains("exactly one")
        }));
    }

    #[test]
    fn checked_xml_generation_rejects_schedule_overflow_instead_of_truncating() {
        let profile = test_profile();
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 4);
        draft.any_taxes_withheld = false;
        draft.auto_compute_penalties = false;
        draft.schedule_1 = (0..=MAX_SCHEDULE_1_ROWS).map(valid_schedule_row).collect();
        draft.compute();

        let errors = draft
            .try_to_bir_xml_payload()
            .expect_err("a fourth XML row must be rejected");
        assert!(errors.iter().any(|(field, _)| field == "schedule_1"));
    }

    #[test]
    fn xml_boundaries_reject_a_non_official_item_5_atc() {
        let profile = test_profile();
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 4);
        draft.any_taxes_withheld = false;
        draft.auto_compute_penalties = false;
        draft.compute();
        draft.atc = "WC010".to_string();

        let export_errors = draft
            .try_to_bir_xml_payload()
            .expect_err("XML export must reject a non-WW010 Item 5 ATC");
        assert!(export_errors.iter().any(|(field, message)| {
            field == "atc" && message.contains("Item 5 ATC must be WW010")
        }));

        let mut fields = Form1601CDraft::new_from_profile(&profile, 2026, 4).to_bir_field_map();
        fields.insert("frm1601c:txtATC".to_string(), "WC010".to_string());
        let import_errors = Form1601CDraft::from_bir_field_map(&fields)
            .expect_err("XML import must reject a non-WW010 Item 5 ATC");
        assert!(import_errors.iter().any(|(field, message)| {
            field == "atc" && message.contains("Item 5 ATC must be WW010")
        }));
    }

    #[test]
    fn generated_map_covers_the_reviewed_plain_schema_and_optional_address_line() {
        const PLAIN_SAMPLE_KEYS: &str = r#"
frm1601c:txtMonth
frm1601c:txtYear
frm1601c:AmendedRtn_1
frm1601c:AmendedRtn_2
frm1601c:TaxWithheld_1
frm1601c:TaxWithheld_2
frm1601c:txtSheets
frm1601c:txtATC
frm1601c:txtTIN1
frm1601c:txtTIN2
frm1601c:txtTIN3
frm1601c:txtBranchCode
frm1601c:txtRDOCode
frm1601c:txtTaxpayerName
frm1601c:txtAddress
frm1601c:txtZipCode
frm1601c:txtTelNum
frm1601c:CatAgent_P
frm1601c:CatAgent_G
txtEmail
frm1601c:SpecialTax_1
frm1601c:SpecialTax_2
frm1601c:selTreaty
frm1601c:txtTax14
frm1601c:txtTax15
frm1601c:txtTax16
frm1601c:txtTax17
frm1601c:txtTax18
frm1601c:txtTax19
frm1601c:txt20Other
frm1601c:txtTax20
frm1601c:txtTax21
frm1601c:txtTax22
frm1601c:txtTax23
frm1601c:txtTax24
frm1601c:txtTax25
frm1601c:txtTax26
frm1601c:txtTax27
frm1601c:txtTax28
frm1601c:txt29Other
frm1601c:txtTax29
frm1601c:txtTax30
frm1601c:txtTax31
frm1601c:txtTax32
frm1601c:txtTax33
frm1601c:txtTax34
frm1601c:txtTax35
frm1601c:txtTax36
txtTaxAgentNo
txtDateIssue
txtDateExpiry
frm1601c:txtAgency37
frm1601c:txtNumber37
frm1601c:txtDate37
frm1601c:txtAmount37
frm1601c:txtAgency38
frm1601c:txtNumber38
frm1601c:txtDate38
frm1601c:txtAmount38
frm1601c:txtNumber39
frm1601c:txtDate39
frm1601c:txtAmount39
frm1601c:txtParticular40
frm1601c:txtAgency40
frm1601c:txtNumber40
frm1601c:txtDate40
frm1601c:txtAmount40
frm1601c:txtPg2TIN1
frm1601c:txtPg2TIN2
frm1601c:txtPg2TIN3
frm1601c:txtPg2BranchCode
frm1601c:txtPg2TaxpayerName
chkScheduleDelete0
frm1601c:sched1:txtMonthYear0
frm1601c:sched1:txtDatePaid0
frm1601c:sched1:txtBankCode0
frm1601c:sched1:txtNumber0
frm1601c:sched1:txtTaxPaid0
chkScheduleDelete1
frm1601c:sched1:txtMonthYear1
frm1601c:sched1:txtDatePaid1
frm1601c:sched1:txtBankCode1
frm1601c:sched1:txtNumber1
frm1601c:sched1:txtTaxPaid1
chkScheduleDelete2
frm1601c:sched1:txtMonthYear2
frm1601c:sched1:txtDatePaid2
frm1601c:sched1:txtBankCode2
frm1601c:sched1:txtNumber2
frm1601c:sched1:txtTaxPaid2
frm1601c:sched1:txtShouldTaxDue0
frm1601c:sched1:txtAdjustments0
frm1601c:sched1:txtShouldTaxDue1
frm1601c:sched1:txtAdjustments1
frm1601c:sched1:txtShouldTaxDue2
frm1601c:sched1:txtAdjustments2
frm1601c:sched1:txtTotal1
frm1601c:txtCurrentPage
frm1601c:txtMaxPage
frm1601c:txtLineBus
"#;

        let profile = test_profile();
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 4);
        let fields = draft.to_bir_field_map();
        let reviewed_keys = PLAIN_SAMPLE_KEYS
            .lines()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .collect::<Vec<_>>();

        let missing = reviewed_keys
            .iter()
            .copied()
            .filter(|key| !fields.contains_key(*key))
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "missing source sample keys: {missing:?}"
        );
        // The upload adds Address 2 to the reviewed editable-save keys.
        assert_eq!(
            reviewed_keys.len() + 1,
            EXACT_REVIEWED_PLAIN_XML_FIELD_COUNT
        );
        assert_eq!(fields.len(), EXACT_REVIEWED_PLAIN_XML_FIELD_COUNT);
        assert_eq!(fields["frm1601c:txtAddress2"], "");

        draft.registered_address_2 = "Reviewed second address line".to_string();
        let fields_with_optional_address = draft.to_bir_field_map();
        assert_eq!(
            fields_with_optional_address.len(),
            EXACT_REVIEWED_PLAIN_XML_FIELD_COUNT
        );
        assert_eq!(
            fields_with_optional_address["frm1601c:txtAddress2"],
            "REVIEWED SECOND ADDRESS LINE"
        );
    }

    #[test]
    fn editable_map_is_deterministic_and_contains_no_unreviewed_transport_metadata() {
        let draft = Form1601CDraft::new_from_profile(&test_profile(), 2026, 5);

        let first = draft.to_bir_field_map();
        let second = draft.to_bir_field_map();

        assert_eq!(first, second);
        assert!(!first.contains_key("newOfflineForm"));
        assert!(!first.contains_key("txtFileName"));
    }
}
