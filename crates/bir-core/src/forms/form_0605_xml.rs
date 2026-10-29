//! Checked 235-field editable-save contract for exact form `0605v1999`.
//!
//! The contract is the intersection of the official July 1999 form and two
//! reviewed plain saves paired with encrypted application outputs. The plain
//! saves establish editable persistence only; they do not authorize queueing.

use std::collections::{BTreeMap, BTreeSet};

use super::FormValidator;
use super::form_0605::{
    Form0605ApprovalSelection, Form0605Date, Form0605Draft, Form0605FilingBasis,
    Form0605IndexedCode, Form0605MannerOfPayment, Form0605PaymentDetails, Form0605SignatureDetails,
    Form0605TaxpayerClassification, Form0605TypeOfPayment,
};

const ATC_INDEX_COUNT: u16 = 142;
const TAX_TYPE_INDEX_COUNT: u16 = 37;
const EXACT_SOURCE_FIELD_COUNT: usize = 235;

impl Form0605Draft {
    /// Serialize every reviewed source key. Unknown imported keys are retained,
    /// while modeled values always replace their source counterparts.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.preserved_unmodeled_xml_fields.clone();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);

        for (key, default) in [
            ("driveSelectTPExport", "0"),
            ("ebirOnlineConfirmUsername", ""),
            ("ebirOnlineSecret", ""),
            ("ebirOnlineUsername", ""),
            ("txtEnroll", "Y"),
            ("txtFinalFlag", "1"),
        ] {
            fields
                .entry(key.to_string())
                .or_insert_with(|| default.to_string());
        }
        insert(&mut fields, "txtEmail", self.email.clone());

        insert_bool_pair(
            &mut fields,
            "frm0605:itemFiscalStartMonth:_1",
            "frm0605:itemFiscalStartMonth:_2",
            matches!(self.filing_basis, Form0605FilingBasis::Calendar),
        );
        insert_one_of_four(&mut fields, "itemQuarter_", self.quarter);
        insert_date(&mut fields, "frm0605:txtDueDate", self.due_date);
        insert(
            &mut fields,
            "frm0605:txtNoOfSheets",
            self.number_of_sheets.to_string(),
        );
        insert(
            &mut fields,
            "txtATCCode",
            self.atc
                .as_ref()
                .map(Form0605IndexedCode::code)
                .unwrap_or_default(),
        );
        insert_index_matrix(
            &mut fields,
            "AtcCode",
            ATC_INDEX_COUNT,
            self.atc.as_ref().map(Form0605IndexedCode::xml_index),
        );
        insert(
            &mut fields,
            "frm0605:itemYearEndMonth",
            format!("{:02}", self.year_end_month),
        );
        insert(
            &mut fields,
            "frm0605:txtYearEnded",
            self.taxable_year.to_string(),
        );
        insert_date(&mut fields, "frm0605:txtReturnPeriod", self.return_period);
        insert(
            &mut fields,
            "txtTaxTypeCode",
            self.tax_type
                .as_ref()
                .map(Form0605IndexedCode::code)
                .unwrap_or_default(),
        );
        insert_index_matrix(
            &mut fields,
            "TaxTypeCode",
            TAX_TYPE_INDEX_COUNT,
            self.tax_type.as_ref().map(Form0605IndexedCode::xml_index),
        );

        insert(&mut fields, "frm0605:txtTIN1", tin1);
        insert(&mut fields, "frm0605:txtTIN2", tin2);
        insert(&mut fields, "frm0605:txtTIN3", tin3);
        insert(&mut fields, "frm0605:txtBranchCode", branch);
        insert(&mut fields, "frm0605:txtRDOCode", self.rdo_code.clone());
        insert_bool_pair(
            &mut fields,
            "frm0605:txtClassification:_1",
            "frm0605:txtClassification:_2",
            matches!(
                self.classification,
                Form0605TaxpayerClassification::Individual
            ),
        );
        insert(
            &mut fields,
            "frm0605:txtLineBus",
            self.line_of_business.clone(),
        );
        insert(
            &mut fields,
            "frm0605:txtTaxPayerName",
            self.taxpayer_name.clone(),
        );
        insert(
            &mut fields,
            "frm0605:txtTelNum",
            self.contact_number.clone(),
        );
        insert(
            &mut fields,
            "frm0605:txtAddress",
            self.registered_address.clone(),
        );
        insert(&mut fields, "frm0605:txtZipCode", self.zip_code.clone());

        let payment_mode_index = self.type_of_payment.map(|value| match value {
            Form0605TypeOfPayment::Installment => 1,
            Form0605TypeOfPayment::PartialPayment => 2,
            Form0605TypeOfPayment::FullPayment => 3,
        });
        insert_index_matrix(
            &mut fields,
            "frm0605:itemModeOfPayment:_",
            3,
            payment_mode_index,
        );
        insert(
            &mut fields,
            "frm0605:txtNumOfInstallment",
            self.number_of_installments
                .map(|value| value.to_string())
                .unwrap_or_default(),
        );

        // Control order of the official BIR-Form0605.hta (Item 17, left
        // column): _1 Self-Assessment, _2 Penalties, _3 Tax Deposit/Advance
        // Payment, _4 Income Tax Second Installment, _5 Others; then B _1/_2.
        let manner_index = self.manner_of_payment.map(|value| match value {
            Form0605MannerOfPayment::SelfAssessment => 1,
            Form0605MannerOfPayment::Penalties => 2,
            Form0605MannerOfPayment::TaxDepositOrAdvancePayment => 3,
            Form0605MannerOfPayment::IncomeTaxSecondInstallmentIndividual => 4,
            Form0605MannerOfPayment::Others => 5,
            Form0605MannerOfPayment::PreliminaryOrFinalAssessmentOrDeficiencyTax => 6,
            Form0605MannerOfPayment::AccountsReceivableOrDelinquentAccount => 7,
        });
        insert_index_matrix(
            &mut fields,
            "frm0605:itemMannerOfPayment:_",
            5,
            manner_index.filter(|index| *index <= 5),
        );
        insert_index_matrix(
            &mut fields,
            "frm0605:itemMannerOfPaymentB:_",
            2,
            manner_index.and_then(|index| index.checked_sub(5)),
        );
        insert(
            &mut fields,
            "frm0605:txtOthersName",
            self.other_manner_description.clone(),
        );

        insert_money(
            &mut fields,
            "frm0605:txtTax19",
            self.item_19_basic_tax_or_payment,
        );
        insert_money(&mut fields, "frm0605:txtTax20A", self.item_20a_surcharge);
        insert_money(&mut fields, "frm0605:txtTax20B", self.item_20b_interest);
        insert_money(&mut fields, "frm0605:txtTax20C", self.item_20c_compromise);
        insert_money(
            &mut fields,
            "frm0605:txtTax20D",
            self.item_20d_total_penalties,
        );
        insert_money(
            &mut fields,
            "frm0605:txtTax21",
            self.item_21_total_amount_payable,
        );

        let approval_index = match self.approval_selection {
            Form0605ApprovalSelection::None => None,
            Form0605ApprovalSelection::XmlOption1 => Some(1),
            Form0605ApprovalSelection::XmlOption2 => Some(2),
        };
        insert_index_matrix(&mut fields, "frm0605:itemApprovedYN:_", 2, approval_index);

        fields
    }

    pub fn to_bir_xml_payload(&self) -> String {
        crate::bir_xml::generate_bir_xml(&self.to_bir_field_map())
    }

    /// Produce an editable-save payload only after all semantic and evidence
    /// checks pass. This is not a submission API.
    pub fn try_to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = self.validate();
        if errors.is_empty() {
            Ok(self.to_bir_xml_payload())
        } else {
            Err(errors)
        }
    }

    /// Parse the non-standard BIR div payload and reject malformed, duplicate,
    /// missing, or contradictory fields.
    pub fn from_bir_xml_payload(xml: &str) -> Result<Self, Vec<(String, String)>> {
        let fields = crate::bir_xml::parse_bir_xml_checked(xml).map_err(|error| {
            vec![(
                "xml_payload".to_string(),
                format!("Invalid 0605 pseudo-XML: {error}"),
            )]
        })?;
        Self::from_bir_field_map(&fields)
    }

    pub fn from_bir_field_map(
        fields: &BTreeMap<String, String>,
    ) -> Result<Self, Vec<(String, String)>> {
        let mut errors = Vec::new();
        let expected_keys = expected_xml_keys();
        for key in &expected_keys {
            if !fields.contains_key(key) {
                errors.push((
                    key.clone(),
                    format!("Required 0605 source field {key} is missing"),
                ));
            }
        }

        let filing_basis = parse_required_bool_pair(
            fields,
            "frm0605:itemFiscalStartMonth:_1",
            "frm0605:itemFiscalStartMonth:_2",
            "filing_basis",
            &mut errors,
        )
        .map(|first| {
            if first {
                Form0605FilingBasis::Calendar
            } else {
                Form0605FilingBasis::Fiscal
            }
        });
        let quarter = parse_required_one_index(fields, "itemQuarter_", 4, "quarter", &mut errors);
        let due_date = parse_required_date(fields, "frm0605:txtDueDate", &mut errors);
        let number_of_sheets = parse_required::<u16>(fields, "frm0605:txtNoOfSheets", &mut errors);
        let atc_index =
            parse_required_one_index(fields, "AtcCode", ATC_INDEX_COUNT, "atc", &mut errors);
        let atc_code = required_text(fields, "txtATCCode", &mut errors);
        let year_end_month = parse_required::<u8>(fields, "frm0605:itemYearEndMonth", &mut errors);
        let taxable_year = parse_required::<u16>(fields, "frm0605:txtYearEnded", &mut errors);
        let return_period = parse_required_date(fields, "frm0605:txtReturnPeriod", &mut errors);
        let tax_type_index = parse_required_one_index(
            fields,
            "TaxTypeCode",
            TAX_TYPE_INDEX_COUNT,
            "tax_type",
            &mut errors,
        );
        let tax_type_code = required_text(fields, "txtTaxTypeCode", &mut errors);
        let classification = parse_required_bool_pair(
            fields,
            "frm0605:txtClassification:_1",
            "frm0605:txtClassification:_2",
            "classification",
            &mut errors,
        )
        .map(|first| {
            if first {
                Form0605TaxpayerClassification::Individual
            } else {
                Form0605TaxpayerClassification::NonIndividual
            }
        });

        let payment_mode_index = parse_required_one_index(
            fields,
            "frm0605:itemModeOfPayment:_",
            3,
            "type_of_payment",
            &mut errors,
        );
        let type_of_payment = payment_mode_index.map(|index| match index {
            1 => Form0605TypeOfPayment::Installment,
            2 => Form0605TypeOfPayment::PartialPayment,
            _ => Form0605TypeOfPayment::FullPayment,
        });

        let manner_a = parse_true_indexes(fields, "frm0605:itemMannerOfPayment:_", 5, &mut errors);
        let manner_b = parse_true_indexes(fields, "frm0605:itemMannerOfPaymentB:_", 2, &mut errors);
        let manner_of_payment = match manner_a.len() + manner_b.len() {
            1 => {
                let index = manner_a
                    .first()
                    .copied()
                    .or_else(|| manner_b.first().copied().map(|index| index + 5));
                index.map(|index| match index {
                    1 => Form0605MannerOfPayment::SelfAssessment,
                    2 => Form0605MannerOfPayment::Penalties,
                    3 => Form0605MannerOfPayment::TaxDepositOrAdvancePayment,
                    4 => Form0605MannerOfPayment::IncomeTaxSecondInstallmentIndividual,
                    5 => Form0605MannerOfPayment::Others,
                    6 => Form0605MannerOfPayment::PreliminaryOrFinalAssessmentOrDeficiencyTax,
                    _ => Form0605MannerOfPayment::AccountsReceivableOrDelinquentAccount,
                })
            }
            0 => {
                errors.push((
                    "manner_of_payment".to_string(),
                    "Exactly one Item 17 Manner of Payment flag must be true".to_string(),
                ));
                None
            }
            _ => {
                errors.push((
                    "manner_of_payment".to_string(),
                    "Item 17 contains conflicting true flags".to_string(),
                ));
                None
            }
        };

        let approval_indexes =
            parse_true_indexes(fields, "frm0605:itemApprovedYN:_", 2, &mut errors);
        let approval_selection = match approval_indexes.as_slice() {
            [] => Form0605ApprovalSelection::None,
            [1] => Form0605ApprovalSelection::XmlOption1,
            [2] => Form0605ApprovalSelection::XmlOption2,
            _ => {
                errors.push((
                    "approval_selection".to_string(),
                    "The two BIR approval flags cannot both be true".to_string(),
                ));
                Form0605ApprovalSelection::None
            }
        };

        let number_of_installments =
            parse_optional::<u16>(fields, "frm0605:txtNumOfInstallment", &mut errors);
        let item_19_basic_tax_or_payment = parse_money(fields, "frm0605:txtTax19", &mut errors);
        let item_20a_surcharge = parse_money(fields, "frm0605:txtTax20A", &mut errors);
        let item_20b_interest = parse_money(fields, "frm0605:txtTax20B", &mut errors);
        let item_20c_compromise = parse_money(fields, "frm0605:txtTax20C", &mut errors);
        let source_20d = parse_money(fields, "frm0605:txtTax20D", &mut errors);
        let source_21 = parse_money(fields, "frm0605:txtTax21", &mut errors);

        if !errors.is_empty() {
            return Err(errors);
        }

        let mut draft = Form0605Draft {
            penalty: crate::forms::penalty_items::PenaltyState::imported(),
            id: None,
            tin: format!(
                "{}{}{}{}",
                field(fields, "frm0605:txtTIN1"),
                field(fields, "frm0605:txtTIN2"),
                field(fields, "frm0605:txtTIN3"),
                field(fields, "frm0605:txtBranchCode")
            ),
            taxable_year: taxable_year.unwrap_or_default(),
            month: return_period.map(|date| date.month).unwrap_or(1),
            filing_basis: filing_basis.unwrap_or_default(),
            quarter: quarter
                .and_then(|value| u8::try_from(value).ok())
                .unwrap_or(1),
            year_end_month: year_end_month.unwrap_or_default(),
            due_date,
            return_period,
            number_of_sheets: number_of_sheets.unwrap_or_default(),
            atc: atc_index.map(|index| Form0605IndexedCode::imported_atc(atc_code.clone(), index)),
            tax_type: tax_type_index
                .map(|index| Form0605IndexedCode::imported_tax_type(tax_type_code.clone(), index)),
            rdo_code: field(fields, "frm0605:txtRDOCode").to_string(),
            taxpayer_name: field(fields, "frm0605:txtTaxPayerName").to_string(),
            classification: classification.unwrap_or_default(),
            line_of_business: field(fields, "frm0605:txtLineBus").to_string(),
            registered_address: field(fields, "frm0605:txtAddress").to_string(),
            zip_code: field(fields, "frm0605:txtZipCode").to_string(),
            contact_number: field(fields, "frm0605:txtTelNum").to_string(),
            email: field(fields, "txtEmail").to_string(),
            manner_of_payment,
            other_manner_description: field(fields, "frm0605:txtOthersName").to_string(),
            type_of_payment,
            number_of_installments,
            item_19_basic_tax_or_payment: item_19_basic_tax_or_payment.unwrap_or_default(),
            item_20a_surcharge: item_20a_surcharge.unwrap_or_default(),
            item_20b_interest: item_20b_interest.unwrap_or_default(),
            item_20c_compromise: item_20c_compromise.unwrap_or_default(),
            item_20d_total_penalties: 0.0,
            item_21_total_amount_payable: 0.0,
            approval_selection,
            signatures: Form0605SignatureDetails::default(),
            payment_details: Form0605PaymentDetails::default(),
            preserved_unmodeled_xml_fields: fields
                .iter()
                .filter(|(key, _)| !is_modeled_xml_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            last_error: None,
            lifecycle: super::queueable::SubmissionLifecycle::default(),
        };

        // An imported return carries the penalties it was prepared with:
        // keep each one as an override so the source amounts round-trip.
        Form0605Draft::penalty_declaration().adopt_imported(&mut draft);
        draft.recompute();
        verify_computed_source(
            "frm0605:txtTax20D",
            source_20d,
            draft.item_20d_total_penalties,
            &mut errors,
        );
        verify_computed_source(
            "frm0605:txtTax21",
            source_21,
            draft.item_21_total_amount_payable,
            &mut errors,
        );

        let imported_atc_requires_review = draft
            .atc
            .as_ref()
            .is_some_and(Form0605IndexedCode::requires_review);
        let imported_tax_type_requires_review = draft
            .tax_type
            .as_ref()
            .is_some_and(Form0605IndexedCode::requires_review);
        errors.extend(draft.validate().into_iter().filter(|(field, _)| {
            !(field == "atc" && imported_atc_requires_review
                || field == "tax_type" && imported_tax_type_requires_review)
        }));

        if errors.is_empty() {
            Ok(draft)
        } else {
            Err(errors)
        }
    }
}

fn expected_xml_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for key in [
        "driveSelectTPExport",
        "ebirOnlineConfirmUsername",
        "ebirOnlineSecret",
        "ebirOnlineUsername",
        "txtEmail",
        "txtEnroll",
        "txtFinalFlag",
        "frm0605:itemFiscalStartMonth:_1",
        "frm0605:itemFiscalStartMonth:_2",
        "frm0605:txtDueDateMonth",
        "frm0605:txtDueDateDay",
        "frm0605:txtDueDateYear",
        "frm0605:txtNoOfSheets",
        "txtATCCode",
        "frm0605:itemYearEndMonth",
        "frm0605:txtYearEnded",
        "frm0605:txtReturnPeriodMonth",
        "frm0605:txtReturnPeriodDay",
        "frm0605:txtReturnPeriodYear",
        "txtTaxTypeCode",
        "frm0605:txtTIN1",
        "frm0605:txtTIN2",
        "frm0605:txtTIN3",
        "frm0605:txtBranchCode",
        "frm0605:txtRDOCode",
        "frm0605:txtClassification:_1",
        "frm0605:txtClassification:_2",
        "frm0605:txtLineBus",
        "frm0605:txtTaxPayerName",
        "frm0605:txtTelNum",
        "frm0605:txtAddress",
        "frm0605:txtZipCode",
        "frm0605:txtNumOfInstallment",
        "frm0605:txtOthersName",
        "frm0605:txtTax19",
        "frm0605:txtTax20A",
        "frm0605:txtTax20B",
        "frm0605:txtTax20C",
        "frm0605:txtTax20D",
        "frm0605:txtTax21",
    ] {
        keys.insert(key.to_string());
    }
    for index in 1..=4 {
        keys.insert(format!("itemQuarter_{index}"));
    }
    for index in 1..=3 {
        keys.insert(format!("frm0605:itemModeOfPayment:_{index}"));
    }
    for index in 1..=5 {
        keys.insert(format!("frm0605:itemMannerOfPayment:_{index}"));
    }
    for index in 1..=2 {
        keys.insert(format!("frm0605:itemMannerOfPaymentB:_{index}"));
        keys.insert(format!("frm0605:itemApprovedYN:_{index}"));
    }
    for index in 1..=ATC_INDEX_COUNT {
        keys.insert(format!("AtcCode{index}"));
    }
    for index in 1..=TAX_TYPE_INDEX_COUNT {
        keys.insert(format!("TaxTypeCode{index}"));
    }
    debug_assert_eq!(keys.len(), EXACT_SOURCE_FIELD_COUNT);
    keys
}

fn is_modeled_xml_key(key: &str) -> bool {
    // Transport fields stay in the preserved map so imports reproduce their
    // exact values. Email is semantic and therefore modeled.
    if matches!(
        key,
        "driveSelectTPExport"
            | "ebirOnlineConfirmUsername"
            | "ebirOnlineSecret"
            | "ebirOnlineUsername"
            | "txtEnroll"
            | "txtFinalFlag"
    ) {
        return false;
    }
    expected_xml_keys().contains(key)
}

fn parse_required_date(
    fields: &BTreeMap<String, String>,
    prefix: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<Form0605Date> {
    let month = parse_required::<u8>(fields, &format!("{prefix}Month"), errors);
    let day = parse_required::<u8>(fields, &format!("{prefix}Day"), errors);
    let year = parse_required::<u16>(fields, &format!("{prefix}Year"), errors);
    match (year, month, day) {
        (Some(year), Some(month), Some(day)) => match Form0605Date::new(year, month, day) {
            Ok(date) => Some(date),
            Err(message) => {
                errors.push((prefix.to_string(), message));
                None
            }
        },
        _ => None,
    }
}

fn parse_required_bool_pair(
    fields: &BTreeMap<String, String>,
    first_key: &str,
    second_key: &str,
    semantic_field: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    let first = parse_bool(fields, first_key, errors);
    let second = parse_bool(fields, second_key, errors);
    match (first, second) {
        (Some(true), Some(false)) => Some(true),
        (Some(false), Some(true)) => Some(false),
        (Some(false), Some(false)) => {
            errors.push((
                semantic_field.to_string(),
                format!("Exactly one {semantic_field} flag must be true"),
            ));
            None
        }
        (Some(true), Some(true)) => {
            errors.push((
                semantic_field.to_string(),
                format!("{semantic_field} contains conflicting true flags"),
            ));
            None
        }
        _ => None,
    }
}

fn parse_required_one_index(
    fields: &BTreeMap<String, String>,
    prefix: &str,
    count: u16,
    semantic_field: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<u16> {
    let selected = parse_true_indexes(fields, prefix, count, errors);
    match selected.as_slice() {
        [index] => Some(*index),
        [] => {
            errors.push((
                semantic_field.to_string(),
                format!("Exactly one {semantic_field} flag must be true"),
            ));
            None
        }
        _ => {
            errors.push((
                semantic_field.to_string(),
                format!("{semantic_field} contains conflicting true flags"),
            ));
            None
        }
    }
}

fn parse_true_indexes(
    fields: &BTreeMap<String, String>,
    prefix: &str,
    count: u16,
    errors: &mut Vec<(String, String)>,
) -> Vec<u16> {
    (1..=count)
        .filter_map(|index| {
            parse_bool(fields, &format!("{prefix}{index}"), errors)
                .filter(|selected| *selected)
                .map(|_| index)
        })
        .collect()
}

fn parse_bool(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    match fields.get(key).map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(value) => {
            errors.push((
                key.to_string(),
                format!("0605 boolean field {key} has invalid value {value:?}"),
            ));
            None
        }
        None => None,
    }
}

fn required_text(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> String {
    let value = field(fields, key).trim();
    if value.is_empty() {
        errors.push((
            key.to_string(),
            format!("Required 0605 field {key} is empty"),
        ));
    }
    value.to_string()
}

fn parse_required<T: std::str::FromStr>(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<T> {
    let value = field(fields, key).trim();
    if value.is_empty() {
        errors.push((
            key.to_string(),
            format!("Required 0605 field {key} is empty"),
        ));
        return None;
    }
    value.parse::<T>().map(Some).unwrap_or_else(|_| {
        errors.push((
            key.to_string(),
            format!("0605 field {key} has invalid value {value:?}"),
        ));
        None
    })
}

fn parse_optional<T: std::str::FromStr>(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<T> {
    let value = field(fields, key).trim();
    if value.is_empty() {
        return None;
    }
    value.parse::<T>().map(Some).unwrap_or_else(|_| {
        errors.push((
            key.to_string(),
            format!("0605 field {key} has invalid value {value:?}"),
        ));
        None
    })
}

fn parse_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<f64> {
    let normalized = field(fields, key).replace(',', "");
    let value = normalized.trim();
    if value.is_empty() {
        errors.push((
            key.to_string(),
            format!("Required 0605 amount {key} is empty"),
        ));
        return None;
    }
    value.parse::<f64>().map(Some).unwrap_or_else(|_| {
        errors.push((
            key.to_string(),
            format!("0605 amount {key} has invalid value {value:?}"),
        ));
        None
    })
}

fn verify_computed_source(
    field: &str,
    source: Option<f64>,
    computed: f64,
    errors: &mut Vec<(String, String)>,
) {
    if source.is_some_and(|source| (source - computed).abs() > 0.001) {
        errors.push((
            field.to_string(),
            format!("Source value does not equal the official computed amount {computed:.2}"),
        ));
    }
}

fn field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    fields.get(key).map(String::as_str).unwrap_or("")
}

fn insert_date(fields: &mut BTreeMap<String, String>, prefix: &str, date: Option<Form0605Date>) {
    insert(
        fields,
        &format!("{prefix}Month"),
        date.map(|value| format!("{:02}", value.month))
            .unwrap_or_default(),
    );
    insert(
        fields,
        &format!("{prefix}Day"),
        date.map(|value| format!("{:02}", value.day))
            .unwrap_or_default(),
    );
    insert(
        fields,
        &format!("{prefix}Year"),
        date.map(|value| value.year.to_string()).unwrap_or_default(),
    );
}

fn insert_bool_pair(
    fields: &mut BTreeMap<String, String>,
    first_key: &str,
    second_key: &str,
    first_selected: bool,
) {
    insert_bool(fields, first_key, first_selected);
    insert_bool(fields, second_key, !first_selected);
}

fn insert_one_of_four(fields: &mut BTreeMap<String, String>, prefix: &str, selected: u8) {
    insert_index_matrix(fields, prefix, 4, Some(u16::from(selected)));
}

fn insert_index_matrix(
    fields: &mut BTreeMap<String, String>,
    prefix: &str,
    count: u16,
    selected: Option<u16>,
) {
    for index in 1..=count {
        insert_bool(fields, &format!("{prefix}{index}"), selected == Some(index));
    }
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    let segment = |start: usize, end: usize| digits.get(start..end).unwrap_or_default().to_string();
    (
        segment(0, 3),
        segment(3, 6),
        segment(6, 9),
        digits.get(9..).unwrap_or_default().to_string(),
    )
}

fn insert(fields: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    fields.insert(key.to_string(), value.into());
}

fn insert_bool(fields: &mut BTreeMap<String, String>, key: &str, value: bool) {
    insert(fields, key, if value { "true" } else { "false" });
}

fn insert_money(fields: &mut BTreeMap<String, String>, key: &str, value: f64) {
    insert(fields, key, format_money(value));
}

fn format_money(value: f64) -> String {
    let formatted = format!("{value:.2}");
    let (integer, decimal) = formatted.split_once('.').unwrap_or((&formatted, "00"));
    let (sign, digits) = integer
        .strip_prefix('-')
        .map(|digits| ("-", digits))
        .unwrap_or(("", integer));
    let mut grouped_reversed = String::new();
    for (index, character) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped_reversed.push(',');
        }
        grouped_reversed.push(character);
    }
    let grouped: String = grouped_reversed.chars().rev().collect();
    format!("{sign}{grouped}.{decimal}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::form_0605::{
        Form0605CodeEvidence, Form0605ReviewedAtc, Form0605ReviewedTaxType, Form0605TypeOfPayment,
    };

    /// Item 17 follows the official BIR-Form0605.hta control order:
    /// `_1` Self-Assessment, `_2` Penalties, `_3` Tax Deposit/Advance Payment,
    /// `_4` Income Tax Second Installment, `_5` Others, then B `_1`, `_2`.
    #[test]
    fn manner_of_payment_follows_the_official_control_order() {
        use crate::forms::form_0605::Form0605MannerOfPayment as M;
        let official = [
            ("frm0605:itemMannerOfPayment:_1", M::SelfAssessment),
            ("frm0605:itemMannerOfPayment:_2", M::Penalties),
            (
                "frm0605:itemMannerOfPayment:_3",
                M::TaxDepositOrAdvancePayment,
            ),
            (
                "frm0605:itemMannerOfPayment:_4",
                M::IncomeTaxSecondInstallmentIndividual,
            ),
            ("frm0605:itemMannerOfPayment:_5", M::Others),
            (
                "frm0605:itemMannerOfPaymentB:_1",
                M::PreliminaryOrFinalAssessmentOrDeficiencyTax,
            ),
            (
                "frm0605:itemMannerOfPaymentB:_2",
                M::AccountsReceivableOrDelinquentAccount,
            ),
        ];
        // The B options (6, 7) need approval answers to parse; their order was
        // never in question, so the round trip covers the five A options.
        for (key, manner) in &official[..5] {
            let (key, manner) = (*key, *manner);
            let mut fields = reviewed_sample_fields(ReviewedSample::January2026);
            for (other, _) in official {
                fields.insert(other.to_string(), "false".to_string());
            }
            fields.insert(key.to_string(), "true".to_string());
            if manner == M::Others {
                fields.insert("frm0605:txtOthersName".to_string(), "EXAMPLE".to_string());
            }
            let mut draft = Form0605Draft::from_bir_field_map(&fields)
                .unwrap_or_else(|errors| panic!("{key}: {errors:?}"));
            assert_eq!(draft.manner_of_payment, Some(manner), "{key} reads back");
            draft.manner_of_payment = Some(manner);
            let written = draft.to_bir_field_map();
            for (other, _) in official {
                let expected = if other == key { "true" } else { "false" };
                assert_eq!(
                    written.get(other).map(String::as_str),
                    Some(expected),
                    "{manner:?} -> {other}"
                );
            }
        }
    }

    #[derive(Clone, Copy)]
    enum ReviewedSample {
        January2026,
        December2025,
    }

    fn reviewed_sample_fields(sample: ReviewedSample) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        for (key, value) in [
            ("driveSelectTPExport", "0"),
            ("ebirOnlineConfirmUsername", ""),
            ("ebirOnlineSecret", ""),
            ("ebirOnlineUsername", ""),
            ("txtEmail", "taxpayer@example.com"),
            ("txtEnroll", "Y"),
            ("txtFinalFlag", "1"),
            ("frm0605:itemFiscalStartMonth:_1", "true"),
            ("frm0605:itemFiscalStartMonth:_2", "false"),
            ("itemQuarter_1", "true"),
            ("itemQuarter_2", "false"),
            ("itemQuarter_3", "false"),
            ("itemQuarter_4", "false"),
            ("frm0605:txtTIN1", "123"),
            ("frm0605:txtTIN2", "456"),
            ("frm0605:txtTIN3", "788"),
            ("frm0605:txtBranchCode", "00000"),
            ("frm0605:txtRDOCode", "018"),
            ("frm0605:txtLineBus", "SOFTWARE DEVELOPMENT"),
            ("frm0605:txtTaxPayerName", "JUAN DELA CRUZ"),
            ("frm0605:txtTelNum", "09123456789"),
            ("frm0605:txtAddress", "OLONGAPO"),
            ("frm0605:txtZipCode", "2200"),
            ("frm0605:itemMannerOfPaymentB:_1", "false"),
            ("frm0605:itemMannerOfPaymentB:_2", "false"),
            ("frm0605:itemApprovedYN:_1", "false"),
            ("frm0605:itemApprovedYN:_2", "false"),
        ] {
            insert(&mut fields, key, value);
        }

        for index in 1..=ATC_INDEX_COUNT {
            insert_bool(&mut fields, &format!("AtcCode{index}"), false);
        }
        for index in 1..=TAX_TYPE_INDEX_COUNT {
            insert_bool(&mut fields, &format!("TaxTypeCode{index}"), false);
        }
        for index in 1..=3 {
            insert_bool(
                &mut fields,
                &format!("frm0605:itemModeOfPayment:_{index}"),
                false,
            );
        }
        for index in 1..=5 {
            insert_bool(
                &mut fields,
                &format!("frm0605:itemMannerOfPayment:_{index}"),
                false,
            );
        }

        match sample {
            ReviewedSample::January2026 => {
                for (key, value) in [
                    ("frm0605:txtDueDateMonth", "01"),
                    ("frm0605:txtDueDateDay", "01"),
                    ("frm0605:txtDueDateYear", "2026"),
                    ("frm0605:txtNoOfSheets", "0"),
                    ("txtATCCode", "FP010"),
                    ("frm0605:itemYearEndMonth", "12"),
                    ("frm0605:txtYearEnded", "2026"),
                    ("frm0605:txtReturnPeriodMonth", "01"),
                    ("frm0605:txtReturnPeriodDay", "31"),
                    ("frm0605:txtReturnPeriodYear", "2026"),
                    ("txtTaxTypeCode", "DO"),
                    ("frm0605:txtClassification:_1", "true"),
                    ("frm0605:txtClassification:_2", "false"),
                    ("frm0605:txtNumOfInstallment", ""),
                    ("frm0605:txtOthersName", ""),
                    ("frm0605:txtTax19", "10.00"),
                    ("frm0605:txtTax20A", "0.00"),
                    ("frm0605:txtTax20B", "0.00"),
                    ("frm0605:txtTax20C", "0.00"),
                    ("frm0605:txtTax20D", "0.00"),
                    ("frm0605:txtTax21", "10.00"),
                ] {
                    insert(&mut fields, key, value);
                }
                insert_bool(&mut fields, "AtcCode1", true);
                insert_bool(&mut fields, "TaxTypeCode4", true);
                insert_bool(&mut fields, "frm0605:itemModeOfPayment:_3", true);
                insert_bool(&mut fields, "frm0605:itemMannerOfPayment:_1", true);
            }
            ReviewedSample::December2025 => {
                for (key, value) in [
                    ("frm0605:txtDueDateMonth", "12"),
                    ("frm0605:txtDueDateDay", "31"),
                    ("frm0605:txtDueDateYear", "2025"),
                    ("frm0605:txtNoOfSheets", "10"),
                    ("txtATCCode", "II011"),
                    ("frm0605:itemYearEndMonth", "12"),
                    ("frm0605:txtYearEnded", "2025"),
                    ("frm0605:txtReturnPeriodMonth", "12"),
                    ("frm0605:txtReturnPeriodDay", "31"),
                    ("frm0605:txtReturnPeriodYear", "2025"),
                    ("txtTaxTypeCode", "IT"),
                    ("frm0605:txtClassification:_1", "false"),
                    ("frm0605:txtClassification:_2", "true"),
                    ("frm0605:txtNumOfInstallment", "10"),
                    (
                        "frm0605:txtOthersName",
                        "CANT CHOOSE PRELIMINARY OR ACCOUNT RECEIVABLE",
                    ),
                    ("frm0605:txtTax19", "1,000.00"),
                    ("frm0605:txtTax20A", "10.00"),
                    ("frm0605:txtTax20B", "20.00"),
                    ("frm0605:txtTax20C", "1,000.00"),
                    ("frm0605:txtTax20D", "1,030.00"),
                    ("frm0605:txtTax21", "2,030.00"),
                ] {
                    insert(&mut fields, key, value);
                }
                insert_bool(&mut fields, "AtcCode24", true);
                insert_bool(&mut fields, "TaxTypeCode9", true);
                insert_bool(&mut fields, "frm0605:itemModeOfPayment:_1", true);
                insert_bool(&mut fields, "frm0605:itemMannerOfPayment:_5", true);
            }
        }

        assert_eq!(fields.len(), EXACT_SOURCE_FIELD_COUNT);
        fields
    }

    #[test]
    fn expected_contract_contains_exactly_235_source_keys() {
        assert_eq!(expected_xml_keys().len(), EXACT_SOURCE_FIELD_COUNT);
    }

    #[test]
    fn january_source_round_trips_all_fields_and_proven_indexes() {
        let source = reviewed_sample_fields(ReviewedSample::January2026);
        let draft = Form0605Draft::from_bir_field_map(&source).unwrap();
        assert_eq!(draft.atc.as_ref().unwrap().code(), "FP010");
        assert_eq!(draft.atc.as_ref().unwrap().xml_index(), 1);
        assert_eq!(draft.tax_type.as_ref().unwrap().code(), "DO");
        assert_eq!(draft.tax_type.as_ref().unwrap().xml_index(), 4);
        assert_eq!(draft.to_bir_field_map(), source);
    }

    #[test]
    fn december_source_round_trips_all_fields_formulas_and_independent_quarter() {
        let source = reviewed_sample_fields(ReviewedSample::December2025);
        let draft = Form0605Draft::from_bir_field_map(&source).unwrap();
        assert_eq!(draft.quarter, 1);
        assert_eq!(draft.return_period.unwrap().month, 12);
        assert_eq!(draft.year_end_month, 12);
        assert_eq!(draft.atc.as_ref().unwrap().xml_index(), 24);
        assert_eq!(draft.tax_type.as_ref().unwrap().xml_index(), 9);
        assert_eq!(draft.item_20d_total_penalties, 1_030.0);
        assert_eq!(draft.item_21_total_amount_payable, 2_030.0);
        assert_eq!(draft.to_bir_field_map(), source);
    }

    #[test]
    fn checked_xml_parser_decodes_and_reencodes_semantic_text_once() {
        let source = reviewed_sample_fields(ReviewedSample::December2025);
        let xml = crate::bir_xml::generate_bir_xml(&source);
        assert!(xml.contains("SOFTWARE%20DEVELOPMENT"));
        let decoded = crate::bir_xml::parse_bir_xml_with_codec_checked(
            &xml,
            bir_rules::serialization::BodyCodec::Utf8PercentRfc3986Unreserved,
        )
        .unwrap();
        let draft = Form0605Draft::from_bir_field_map(&decoded).unwrap();
        assert_eq!(draft.line_of_business, "SOFTWARE DEVELOPMENT");
        assert_eq!(
            crate::bir_xml::parse_bir_xml_with_codec_checked(
                &draft.to_bir_xml_payload(),
                bir_rules::serialization::BodyCodec::Utf8PercentRfc3986Unreserved,
            )
            .unwrap(),
            source
        );
    }

    #[test]
    fn export_is_byte_deterministic() {
        let source = reviewed_sample_fields(ReviewedSample::January2026);
        let draft = Form0605Draft::from_bir_field_map(&source).unwrap();
        assert_eq!(draft.to_bir_xml_payload(), draft.to_bir_xml_payload());
    }

    #[test]
    fn parser_rejects_duplicate_or_missing_selections() {
        let mut duplicate = reviewed_sample_fields(ReviewedSample::January2026);
        duplicate.insert("AtcCode24".to_string(), "true".to_string());
        let duplicate_errors = Form0605Draft::from_bir_field_map(&duplicate).unwrap_err();
        assert!(duplicate_errors.iter().any(|(field, _)| field == "atc"));

        let mut missing = reviewed_sample_fields(ReviewedSample::January2026);
        missing.remove("TaxTypeCode37");
        let missing_errors = Form0605Draft::from_bir_field_map(&missing).unwrap_err();
        assert!(
            missing_errors
                .iter()
                .any(|(field, _)| field == "TaxTypeCode37")
        );
    }

    #[test]
    fn parser_rejects_invalid_dates_negative_amounts_and_formula_mismatch() {
        let mut invalid = reviewed_sample_fields(ReviewedSample::December2025);
        invalid.insert("frm0605:txtReturnPeriodDay".to_string(), "32".to_string());
        invalid.insert("frm0605:txtTax19".to_string(), "-1.00".to_string());
        invalid.insert("frm0605:txtTax20D".to_string(), "999.00".to_string());
        let errors = Form0605Draft::from_bir_field_map(&invalid).unwrap_err();
        assert!(errors.iter().any(|(field, _)| {
            field == "frm0605:txtReturnPeriod" || field == "item_19_basic_tax_or_payment"
        }));
    }

    #[test]
    fn unknown_imported_pair_round_trips_but_safe_export_fails_closed() {
        let mut source = reviewed_sample_fields(ReviewedSample::January2026);
        source.insert("AtcCode1".to_string(), "false".to_string());
        source.insert("AtcCode77".to_string(), "true".to_string());
        source.insert("txtATCCode".to_string(), "UNKNOWN".to_string());
        let draft = Form0605Draft::from_bir_field_map(&source).unwrap();
        assert_eq!(
            draft.atc.as_ref().unwrap().evidence(),
            Form0605CodeEvidence::ImportedExact
        );
        assert_eq!(draft.to_bir_field_map(), source);
        assert!(draft.try_to_bir_xml_payload().is_err());
    }

    #[test]
    fn reviewed_choices_export_only_the_four_proven_code_index_pairs() {
        let source = reviewed_sample_fields(ReviewedSample::January2026);
        let mut draft = Form0605Draft::from_bir_field_map(&source).unwrap();
        draft.select_reviewed_atc(Form0605ReviewedAtc::Ii011);
        draft.select_reviewed_tax_type(Form0605ReviewedTaxType::It);
        draft.type_of_payment = Some(Form0605TypeOfPayment::FullPayment);
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["AtcCode24"], "true");
        assert_eq!(fields["TaxTypeCode9"], "true");
    }

    #[test]
    fn twelve_digit_legacy_tin_preserves_three_digit_branch() {
        let mut source = reviewed_sample_fields(ReviewedSample::January2026);
        source.insert("frm0605:txtBranchCode".to_string(), "000".to_string());

        let draft = Form0605Draft::from_bir_field_map(&source).unwrap();

        assert_eq!(draft.tin, "123456788000");
        assert_eq!(draft.to_bir_field_map(), source);
    }
}
