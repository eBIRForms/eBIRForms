//! Typed XML contract for exact identity `0619Fv2018`.
//!
//! The reviewed evidence is a 59-field editable pseudo-XML save and its
//! decrypted 60-field encrypted companion. The union adds
//! `frm0619F:txtAddress2`; the only conflicting value is `txtFinalFlag`
//! (`1` in the editable save and `0` in the encrypted companion).

use std::collections::BTreeMap;

use super::FormValidator;
use super::form_0619f::{
    Form0619FDraft, Form0619FPaymentDetails, Form0619FPaymentRow, Form0619FTaxType,
    Form0619FXmlFinalFlag, WithholdingAgentCategory,
};

const REVIEWED_NON_FILING_XML_STATE: [(&str, &str); 5] = [
    ("driveSelectTPExport", ""),
    ("ebirOnlineConfirmUsername", ""),
    ("ebirOnlineSecret", ""),
    ("ebirOnlineUsername", ""),
    ("txtEnroll", "Y"),
];

impl Form0619FDraft {
    /// Serialize all modeled fields and retain unmodeled source keys. Modeled
    /// values always win, so preserved transport data cannot alter tax truth.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.preserved_unmodeled_xml_fields.clone();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        let (due_month, due_year) = self.due_month_and_year();

        // Both locked sources use these exact non-filing values. Rust never
        // persists or replays eBIRForms login, enrollment, or export-device
        // state through an editable 0619-F draft.
        for (key, value) in REVIEWED_NON_FILING_XML_STATE {
            insert(&mut fields, key, value);
        }

        insert(&mut fields, "txtEmail", self.email.clone());
        insert(
            &mut fields,
            "txtFinalFlag",
            self.xml_final_flag.as_xml_value(),
        );
        insert(
            &mut fields,
            "txtTaxAgentNo",
            self.tax_agent_accreditation_number.clone(),
        );
        insert(
            &mut fields,
            "txtDateIssue",
            self.tax_agent_date_of_issue.clone(),
        );
        insert(
            &mut fields,
            "txtDateExpiry",
            self.tax_agent_date_of_expiry.clone(),
        );

        insert_bool_pair(
            &mut fields,
            "frm0619F:optAmend:Y",
            "frm0619F:optAmend:N",
            self.is_amended,
        );
        insert_bool_pair(
            &mut fields,
            "frm0619F:optWithheld:Y",
            "frm0619F:optWithheld:N",
            self.any_taxes_withheld,
        );
        insert_bool_pair(
            &mut fields,
            "frm0619F:optCategory:G",
            "frm0619F:optCategory:P",
            matches!(
                self.withholding_agent_category,
                WithholdingAgentCategory::Government
            ),
        );

        insert(&mut fields, "frm0619F:txtTaxTypeCode", self.tax_type.code());
        insert(
            &mut fields,
            "frm0619F:txtMonth",
            format!("{:02}", self.month),
        );
        insert(
            &mut fields,
            "frm0619F:txtYear",
            self.taxable_year.to_string(),
        );
        insert(
            &mut fields,
            "frm0619F:txtDueMonth",
            format!("{due_month:02}"),
        );
        insert(
            &mut fields,
            "frm0619F:txtDueDay",
            self.due_day
                .map(|day| format!("{day:02}"))
                .unwrap_or_default(),
        );
        insert(&mut fields, "frm0619F:txtDueYear", due_year.to_string());
        insert(&mut fields, "frm0619F:txtTIN1", tin1);
        insert(&mut fields, "frm0619F:txtTIN2", tin2);
        insert(&mut fields, "frm0619F:txtTIN3", tin3);
        insert(&mut fields, "frm0619F:txtBranchCode", branch);
        insert(&mut fields, "frm0619F:txtRDOCode", self.rdo_code.clone());
        insert(
            &mut fields,
            "frm0619F:txtTaxpayerName",
            self.taxpayer_name.clone(),
        );
        insert(
            &mut fields,
            "frm0619F:txtLineBus",
            self.line_of_business.clone(),
        );
        insert(
            &mut fields,
            "frm0619F:txtAddress",
            self.registered_address.clone(),
        );
        insert(
            &mut fields,
            "frm0619F:txtAddress2",
            self.registered_address_2.clone(),
        );
        insert(&mut fields, "frm0619F:txtZipCode", self.zip_code.clone());
        insert(
            &mut fields,
            "frm0619F:txtTelNum",
            self.contact_number.clone(),
        );

        insert_money(
            &mut fields,
            "frm0619F:txtTax13",
            self.item_13_interest_final_tax_withheld,
        );
        insert_money(
            &mut fields,
            "frm0619F:txtTax14",
            self.item_14_other_final_tax_withheld,
        );
        insert_money(&mut fields, "frm0619F:txtTax15", self.item_15_total);
        insert_money(
            &mut fields,
            "frm0619F:txtTax16",
            self.item_16_remitted_previously,
        );
        insert_money(
            &mut fields,
            "frm0619F:txtTax17",
            self.item_17_net_amount_of_remittance,
        );
        insert_money(&mut fields, "frm0619F:txtTax18A", self.item_18a_surcharge);
        insert_money(&mut fields, "frm0619F:txtTax18B", self.item_18b_interest);
        insert_money(&mut fields, "frm0619F:txtTax18C", self.item_18c_compromise);
        insert_money(
            &mut fields,
            "frm0619F:txtTax18D",
            self.item_18d_total_penalties,
        );
        insert_money(
            &mut fields,
            "frm0619F:txtTax19",
            self.item_19_total_amount_of_remittance,
        );

        insert_payment_row(
            &mut fields,
            20,
            &self.payment_details.cash_or_bank_debit_memo,
        );
        insert_payment_row(&mut fields, 21, &self.payment_details.check);
        insert_payment_row(&mut fields, 22, &self.payment_details.tax_debit_memo);
        insert_payment_row(&mut fields, 23, &self.payment_details.others);
        insert(
            &mut fields,
            "txtParticular23",
            self.payment_details.others_description.clone(),
        );

        fields
    }

    pub fn to_bir_xml_payload(&self) -> String {
        crate::bir_xml::generate_bir_xml(&self.to_bir_field_map())
    }

    /// Produce an editable/save payload only when the typed model is valid.
    /// This is not a submission API; queue transport remains disabled.
    pub fn try_to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = self.validate();
        if errors.is_empty() {
            Ok(self.to_bir_xml_payload())
        } else {
            Err(errors)
        }
    }

    /// Parse checked BIR pseudo-XML, including the one-line layout used by both
    /// reviewed source payloads.
    pub fn from_bir_xml_payload(xml: &str) -> Result<Self, Vec<(String, String)>> {
        let fields = crate::bir_xml::parse_bir_xml_checked(xml).map_err(|error| {
            vec![(
                "xml_payload".to_string(),
                format!("Invalid 0619-F pseudo-XML: {error}"),
            )]
        })?;
        Self::from_bir_field_map(&fields)
    }

    pub fn from_bir_field_map(
        fields: &BTreeMap<String, String>,
    ) -> Result<Self, Vec<(String, String)>> {
        let mut errors = Vec::new();
        let month = parse_required::<u8>(fields, "frm0619F:txtMonth", &mut errors);
        let taxable_year = parse_required::<u16>(fields, "frm0619F:txtYear", &mut errors);
        let is_amended = parse_bool_pair(
            fields,
            "frm0619F:optAmend:Y",
            "frm0619F:optAmend:N",
            "is_amended",
            &mut errors,
        );
        let any_taxes_withheld = parse_bool_pair(
            fields,
            "frm0619F:optWithheld:Y",
            "frm0619F:optWithheld:N",
            "any_taxes_withheld",
            &mut errors,
        );
        let category_government = parse_bool_pair(
            fields,
            "frm0619F:optCategory:G",
            "frm0619F:optCategory:P",
            "withholding_agent_category",
            &mut errors,
        );

        let tax_type = Form0619FTaxType::from_code(field(fields, "frm0619F:txtTaxTypeCode").trim());
        if tax_type.is_none() {
            errors.push((
                "frm0619F:txtTaxTypeCode".to_string(),
                format!(
                    "Item 5 tax type must be WB or WF, found {:?}",
                    field(fields, "frm0619F:txtTaxTypeCode")
                ),
            ));
        }
        validate_reviewed_non_filing_state(fields, &mut errors);

        let xml_final_flag = match field(fields, "txtFinalFlag") {
            "0" => Form0619FXmlFinalFlag::Zero,
            "1" => Form0619FXmlFinalFlag::One,
            "" => Form0619FXmlFinalFlag::Missing,
            value => Form0619FXmlFinalFlag::Unknown(value.to_string()),
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        let mut draft = Form0619FDraft {
            penalty: crate::forms::penalty_items::PenaltyState::imported(),
            id: None,
            tin: format!(
                "{}{}{}{}",
                field(fields, "frm0619F:txtTIN1"),
                field(fields, "frm0619F:txtTIN2"),
                field(fields, "frm0619F:txtTIN3"),
                field(fields, "frm0619F:txtBranchCode")
            ),
            taxable_year: taxable_year.unwrap_or_default(),
            month: month.unwrap_or_default(),
            is_amended: is_amended.unwrap_or(false),
            any_taxes_withheld: any_taxes_withheld.unwrap_or(false),
            tax_type: tax_type.unwrap_or_default(),
            withholding_agent_category: if category_government.unwrap_or(false) {
                WithholdingAgentCategory::Government
            } else {
                WithholdingAgentCategory::Private
            },
            due_day: parse_optional::<u8>(fields, "frm0619F:txtDueDay", &mut errors),
            rdo_code: semantic_text(fields, "frm0619F:txtRDOCode"),
            taxpayer_name: semantic_text(fields, "frm0619F:txtTaxpayerName"),
            line_of_business: semantic_text(fields, "frm0619F:txtLineBus"),
            registered_address: semantic_text(fields, "frm0619F:txtAddress"),
            registered_address_2: semantic_text(fields, "frm0619F:txtAddress2"),
            zip_code: semantic_text(fields, "frm0619F:txtZipCode"),
            contact_number: semantic_text(fields, "frm0619F:txtTelNum"),
            email: semantic_text(fields, "txtEmail"),
            item_13_interest_final_tax_withheld: parse_money(
                fields,
                "frm0619F:txtTax13",
                false,
                &mut errors,
            )
            .unwrap_or_default(),
            item_14_other_final_tax_withheld: parse_money(
                fields,
                "frm0619F:txtTax14",
                false,
                &mut errors,
            )
            .unwrap_or_default(),
            item_15_total: 0.0,
            item_16_remitted_previously: parse_money(
                fields,
                "frm0619F:txtTax16",
                false,
                &mut errors,
            )
            .unwrap_or_default(),
            item_17_net_amount_of_remittance: 0.0,
            item_18a_surcharge: parse_money(fields, "frm0619F:txtTax18A", false, &mut errors)
                .unwrap_or_default(),
            item_18b_interest: parse_money(fields, "frm0619F:txtTax18B", false, &mut errors)
                .unwrap_or_default(),
            item_18c_compromise: parse_money(fields, "frm0619F:txtTax18C", false, &mut errors)
                .unwrap_or_default(),
            item_18d_total_penalties: 0.0,
            item_19_total_amount_of_remittance: 0.0,
            tax_agent_accreditation_number: semantic_text(fields, "txtTaxAgentNo"),
            tax_agent_date_of_issue: semantic_text(fields, "txtDateIssue"),
            tax_agent_date_of_expiry: semantic_text(fields, "txtDateExpiry"),
            payment_details: Form0619FPaymentDetails {
                cash_or_bank_debit_memo: parse_payment_row(fields, 20, &mut errors),
                check: parse_payment_row(fields, 21, &mut errors),
                tax_debit_memo: parse_payment_row(fields, 22, &mut errors),
                others: parse_payment_row(fields, 23, &mut errors),
                others_description: semantic_text(fields, "txtParticular23"),
            },
            xml_final_flag,
            preserved_unmodeled_xml_fields: fields
                .iter()
                .filter(|(key, _)| !is_modeled_xml_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            last_error: None,
            lifecycle: super::queueable::SubmissionLifecycle::default(),
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        let source_15 = parse_money(fields, "frm0619F:txtTax15", false, &mut errors);
        let source_17 = parse_money(fields, "frm0619F:txtTax17", false, &mut errors);
        let source_18d = parse_money(fields, "frm0619F:txtTax18D", false, &mut errors);
        let source_19 = parse_money(fields, "frm0619F:txtTax19", false, &mut errors);
        // An imported return carries the penalties it was prepared with:
        // keep each one as an override so the source amounts round-trip.
        Form0619FDraft::penalty_declaration().adopt_imported(&mut draft);
        draft.recompute();
        verify_computed_source(
            "frm0619F:txtTax15",
            source_15,
            draft.item_15_total,
            &mut errors,
        );
        verify_computed_source(
            "frm0619F:txtTax17",
            source_17,
            draft.item_17_net_amount_of_remittance,
            &mut errors,
        );
        verify_computed_source(
            "frm0619F:txtTax18D",
            source_18d,
            draft.item_18d_total_penalties,
            &mut errors,
        );
        verify_computed_source(
            "frm0619F:txtTax19",
            source_19,
            draft.item_19_total_amount_of_remittance,
            &mut errors,
        );

        let (expected_due_month, expected_due_year) = draft.due_month_and_year();
        verify_due_component(
            fields,
            "frm0619F:txtDueMonth",
            u16::from(expected_due_month),
            &mut errors,
        );
        verify_due_component(
            fields,
            "frm0619F:txtDueYear",
            expected_due_year,
            &mut errors,
        );

        // An imported save keeps its record even when its TIN fails the
        // official check digit; that check still blocks queueing.
        errors.extend(draft.validate().into_iter().filter(|(field, message)| {
            !(field == "tin" && message == crate::validation::OFFICIAL_INVALID_TIN_MESSAGE)
        }));
        if errors.is_empty() {
            Ok(draft)
        } else {
            Err(errors)
        }
    }
}

fn semantic_text(fields: &BTreeMap<String, String>, key: &str) -> String {
    // The editable Line of Business value has one more URL-encoding layer than
    // its encrypted companion. Decode modeled text until stable with a small
    // fixed bound, producing the same semantic value from both samples.
    let mut value = field(fields, key).to_string();
    for _ in 0..2 {
        let decoded = urlencoding::decode(&value)
            .unwrap_or(std::borrow::Cow::Borrowed(&value))
            .into_owned();
        if decoded == value {
            break;
        }
        value = decoded;
    }
    value
}

fn field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    fields.get(key).map(String::as_str).unwrap_or("")
}

fn validate_reviewed_non_filing_state(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) {
    for (key, expected) in REVIEWED_NON_FILING_XML_STATE {
        match fields.get(key) {
            Some(actual) if actual == expected => {}
            Some(_) => errors.push((
                key.to_string(),
                format!(
                    "0619-F import requires the reviewed non-filing value {key}={expected:?}; credential, enrollment, and export-device state is never persisted"
                ),
            )),
            None => errors.push((
                key.to_string(),
                format!("Required reviewed 0619-F non-filing field {key} is missing"),
            )),
        }
    }
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
            format!("Required 0619-F field {key} is missing"),
        ));
        return None;
    }
    value.parse::<T>().map(Some).unwrap_or_else(|_| {
        errors.push((
            key.to_string(),
            format!("0619-F field {key} has invalid value {value:?}"),
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
            format!("0619-F field {key} has invalid value {value:?}"),
        ));
        None
    })
}

fn parse_bool_pair(
    fields: &BTreeMap<String, String>,
    yes_key: &str,
    no_key: &str,
    semantic_field: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    let yes = parse_bool(fields, yes_key, errors);
    let no = parse_bool(fields, no_key, errors);
    match (yes, no) {
        (Some(true), Some(false)) => Some(true),
        (Some(false), Some(true)) => Some(false),
        (Some(_), Some(_)) => {
            errors.push((
                semantic_field.to_string(),
                format!("0619-F choice {semantic_field} must have exactly one selected value"),
            ));
            None
        }
        _ => None,
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
                format!("0619-F boolean field {key} has invalid value {value:?}"),
            ));
            None
        }
    }
}

fn parse_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    optional: bool,
    errors: &mut Vec<(String, String)>,
) -> Option<f64> {
    let raw = field(fields, key).trim();
    if raw.is_empty() {
        return if optional { None } else { Some(0.0) };
    }
    let normalized = raw.replace(',', "");
    match normalized.parse::<f64>() {
        Ok(value) if value.is_finite() => Some(value),
        _ => {
            errors.push((
                key.to_string(),
                format!("0619-F money field {key} has invalid value {raw:?}"),
            ));
            None
        }
    }
}

fn parse_payment_row(
    fields: &BTreeMap<String, String>,
    item: u8,
    errors: &mut Vec<(String, String)>,
) -> Form0619FPaymentRow {
    Form0619FPaymentRow {
        drawee_bank_or_agency: semantic_text(fields, &format!("txtAgency{item}")),
        number: semantic_text(fields, &format!("txtNumber{item}")),
        date: semantic_text(fields, &format!("txtDate{item}")),
        amount: parse_money(fields, &format!("txtAmount{item}"), true, errors),
    }
}

fn verify_computed_source(
    key: &str,
    source: Option<f64>,
    computed: f64,
    errors: &mut Vec<(String, String)>,
) {
    if let Some(source) = source
        && (source - computed).abs() > 0.001
    {
        errors.push((
            key.to_string(),
            format!(
                "0619-F source value {source:.2} does not match the official computed value {computed:.2}"
            ),
        ));
    }
}

fn verify_due_component(
    fields: &BTreeMap<String, String>,
    key: &str,
    expected: u16,
    errors: &mut Vec<(String, String)>,
) {
    let Some(actual) = parse_required::<u16>(fields, key, errors) else {
        return;
    };
    if actual != expected {
        errors.push((
            key.to_string(),
            format!("0619-F {key} must be {expected}, found {actual}"),
        ));
    }
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    let segment = |start: usize, end: usize| {
        digits
            .get(start..end.min(digits.len()))
            .unwrap_or("")
            .to_string()
    };
    let branch = digits
        .get(9..)
        .filter(|value| !value.is_empty())
        .unwrap_or("00000");
    (
        segment(0, 3),
        segment(3, 6),
        segment(6, 9),
        format!("{branch:0>5}"),
    )
}

fn is_modeled_xml_key(key: &str) -> bool {
    matches!(
        key,
        "driveSelectTPExport"
            | "ebirOnlineConfirmUsername"
            | "ebirOnlineSecret"
            | "ebirOnlineUsername"
            | "txtEnroll"
            | "txtEmail"
            | "txtFinalFlag"
            | "txtTaxAgentNo"
            | "txtDateIssue"
            | "txtDateExpiry"
            | "txtParticular23"
            | "frm0619F:optAmend:Y"
            | "frm0619F:optAmend:N"
            | "frm0619F:optWithheld:Y"
            | "frm0619F:optWithheld:N"
            | "frm0619F:optCategory:G"
            | "frm0619F:optCategory:P"
            | "frm0619F:txtTaxTypeCode"
            | "frm0619F:txtMonth"
            | "frm0619F:txtYear"
            | "frm0619F:txtDueMonth"
            | "frm0619F:txtDueDay"
            | "frm0619F:txtDueYear"
            | "frm0619F:txtTIN1"
            | "frm0619F:txtTIN2"
            | "frm0619F:txtTIN3"
            | "frm0619F:txtBranchCode"
            | "frm0619F:txtRDOCode"
            | "frm0619F:txtTaxpayerName"
            | "frm0619F:txtLineBus"
            | "frm0619F:txtAddress"
            | "frm0619F:txtAddress2"
            | "frm0619F:txtZipCode"
            | "frm0619F:txtTelNum"
            | "frm0619F:txtTax13"
            | "frm0619F:txtTax14"
            | "frm0619F:txtTax15"
            | "frm0619F:txtTax16"
            | "frm0619F:txtTax17"
            | "frm0619F:txtTax18A"
            | "frm0619F:txtTax18B"
            | "frm0619F:txtTax18C"
            | "frm0619F:txtTax18D"
            | "frm0619F:txtTax19"
            | "txtAgency20"
            | "txtNumber20"
            | "txtDate20"
            | "txtAmount20"
            | "txtAgency21"
            | "txtNumber21"
            | "txtDate21"
            | "txtAmount21"
            | "txtAgency22"
            | "txtNumber22"
            | "txtDate22"
            | "txtAmount22"
            | "txtAgency23"
            | "txtNumber23"
            | "txtDate23"
            | "txtAmount23"
    )
}

fn insert_payment_row(fields: &mut BTreeMap<String, String>, item: u8, row: &Form0619FPaymentRow) {
    insert(
        fields,
        &format!("txtAgency{item}"),
        row.drawee_bank_or_agency.clone(),
    );
    insert(fields, &format!("txtNumber{item}"), row.number.clone());
    insert(fields, &format!("txtDate{item}"), row.date.clone());
    insert_optional_money(fields, &format!("txtAmount{item}"), row.amount);
}

fn insert(map: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    map.insert(key.to_string(), value.into());
}

fn insert_bool_pair(map: &mut BTreeMap<String, String>, yes_key: &str, no_key: &str, value: bool) {
    insert(map, yes_key, if value { "true" } else { "false" });
    insert(map, no_key, if value { "false" } else { "true" });
}

fn insert_money(map: &mut BTreeMap<String, String>, key: &str, value: f64) {
    insert(map, key, format!("{value:.2}"));
}

fn insert_optional_money(map: &mut BTreeMap<String, String>, key: &str, value: Option<f64>) {
    insert(
        map,
        key,
        value
            .map(|amount| format!("{amount:.2}"))
            .unwrap_or_default(),
    );
}

#[cfg(test)]
mod tests {
    use super::super::form_0619f::{
        CURRENT_RUST_REENCRYPTED_XML_SHA256, EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT,
        EXACT_REVIEWED_PLAIN_XML_FIELD_COUNT, REVIEWED_DECRYPTED_XML_SHA256,
    };
    use super::*;

    fn reviewed_sample(final_flag: &str, include_address_2: bool) -> String {
        let mut fields = BTreeMap::from([
            ("frm0619F:txtMonth".to_string(), "04".to_string()),
            ("frm0619F:txtYear".to_string(), "2026".to_string()),
            ("frm0619F:txtDueMonth".to_string(), "05".to_string()),
            ("frm0619F:txtDueDay".to_string(), "10".to_string()),
            ("frm0619F:txtDueYear".to_string(), "2026".to_string()),
            ("frm0619F:optAmend:Y".to_string(), "false".to_string()),
            ("frm0619F:optAmend:N".to_string(), "true".to_string()),
            ("frm0619F:optWithheld:Y".to_string(), "true".to_string()),
            ("frm0619F:optWithheld:N".to_string(), "false".to_string()),
            ("frm0619F:txtTaxTypeCode".to_string(), "WB".to_string()),
            ("frm0619F:txtTIN1".to_string(), "000".to_string()),
            ("frm0619F:txtTIN2".to_string(), "000".to_string()),
            ("frm0619F:txtTIN3".to_string(), "000".to_string()),
            ("frm0619F:txtBranchCode".to_string(), "00000".to_string()),
            ("frm0619F:txtRDOCode".to_string(), "018".to_string()),
            (
                "frm0619F:txtTaxpayerName".to_string(),
                "JUAN DELA CRUZ".to_string(),
            ),
            (
                "frm0619F:txtLineBus".to_string(),
                "SOFTWARE%2520DEVELOPMENT".to_string(),
            ),
            ("frm0619F:txtAddress".to_string(), "OLONGAPO".to_string()),
            ("frm0619F:txtZipCode".to_string(), "2200".to_string()),
            ("frm0619F:txtTelNum".to_string(), "09123456789".to_string()),
            ("frm0619F:optCategory:P".to_string(), "true".to_string()),
            ("frm0619F:optCategory:G".to_string(), "false".to_string()),
            ("txtEmail".to_string(), "taxpayer@example.com".to_string()),
            ("frm0619F:txtTax13".to_string(), "1,000.00".to_string()),
            ("frm0619F:txtTax14".to_string(), "0.00".to_string()),
            ("frm0619F:txtTax15".to_string(), "1,000.00".to_string()),
            ("frm0619F:txtTax16".to_string(), "0.00".to_string()),
            ("frm0619F:txtTax17".to_string(), "1,000.00".to_string()),
            ("frm0619F:txtTax18A".to_string(), "1,000.00".to_string()),
            ("frm0619F:txtTax18B".to_string(), "1,000.00".to_string()),
            ("frm0619F:txtTax18C".to_string(), "1,000.00".to_string()),
            ("frm0619F:txtTax18D".to_string(), "3,000.00".to_string()),
            ("frm0619F:txtTax19".to_string(), "4,000.00".to_string()),
            ("txtFinalFlag".to_string(), final_flag.to_string()),
            ("txtEnroll".to_string(), "Y".to_string()),
            ("driveSelectTPExport".to_string(), String::new()),
            ("ebirOnlineConfirmUsername".to_string(), String::new()),
            ("ebirOnlineUsername".to_string(), String::new()),
            ("ebirOnlineSecret".to_string(), String::new()),
        ]);
        for item in 20..=23 {
            for prefix in ["txtAgency", "txtNumber", "txtDate", "txtAmount"] {
                fields.insert(format!("{prefix}{item}"), String::new());
            }
        }
        for key in [
            "txtTaxAgentNo",
            "txtDateIssue",
            "txtDateExpiry",
            "txtParticular23",
        ] {
            fields.insert(key.to_string(), String::new());
        }
        if include_address_2 {
            fields.insert("frm0619F:txtAddress2".to_string(), String::new());
        }
        crate::bir_xml::generate_bir_xml(&fields).replace("\t\n", "")
    }

    #[test]
    fn reviewed_key_counts_match_plain_and_encrypted_union() {
        let plain = crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("1", false))
            .expect("plain sample must parse");
        let encrypted = crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("0", true))
            .expect("encrypted companion must parse");
        assert_eq!(plain.len(), EXACT_REVIEWED_PLAIN_XML_FIELD_COUNT);
        assert_eq!(encrypted.len(), EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT);
    }

    #[test]
    fn reviewed_plain_and_companion_final_flags_are_both_preserved() {
        let plain = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("1", false))
            .expect("reviewed plain save must parse");
        let companion = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("0", true))
            .expect("reviewed encrypted companion must parse");
        assert_eq!(plain.xml_final_flag, Form0619FXmlFinalFlag::One);
        assert_eq!(companion.xml_final_flag, Form0619FXmlFinalFlag::Zero);
        assert_eq!(plain.to_bir_field_map()["txtFinalFlag"], "1");
        assert_eq!(companion.to_bir_field_map()["txtFinalFlag"], "0");
    }

    #[test]
    fn one_line_payload_and_extra_encoded_semantic_text_parse_correctly() {
        let draft = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("1", false))
            .expect("one-line reviewed save must parse");
        assert_eq!(draft.line_of_business, "SOFTWARE DEVELOPMENT");
        assert_eq!(draft.taxpayer_name, "JUAN DELA CRUZ");
        assert_eq!(draft.item_19_total_amount_of_remittance, 4_000.0);
    }

    #[test]
    fn address_line_two_is_in_the_union_and_roundtrips() {
        let mut fields = crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("0", true))
            .expect("reviewed sample must parse");
        fields.insert(
            "frm0619F:txtAddress2".to_string(),
            "SECOND LINE".to_string(),
        );
        let draft = Form0619FDraft::from_bir_field_map(&fields).expect("union must parse");
        assert_eq!(draft.registered_address_2, "SECOND LINE");
        assert_eq!(
            draft.to_bir_field_map()["frm0619F:txtAddress2"],
            "SECOND LINE"
        );
    }

    #[test]
    fn credential_enrollment_and_export_state_fail_closed_and_do_not_persist() {
        for (key, unsafe_value) in [
            ("ebirOnlineConfirmUsername", "registered-user"),
            ("ebirOnlineUsername", "registered-user"),
            ("ebirOnlineSecret", "must-not-persist"),
            ("txtEnroll", "N"),
            ("driveSelectTPExport", "C:"),
        ] {
            let mut fields =
                crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("1", true)).unwrap();
            fields.insert(key.to_string(), unsafe_value.to_string());
            let errors = Form0619FDraft::from_bir_field_map(&fields)
                .expect_err("non-reviewed credential or UI state must fail closed");
            assert!(errors.iter().any(|(field, _)| field == key));
        }

        let mut fields =
            crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("1", true)).unwrap();
        fields.remove("ebirOnlineSecret");
        assert!(
            Form0619FDraft::from_bir_field_map(&fields)
                .unwrap_err()
                .iter()
                .any(|(field, _)| field == "ebirOnlineSecret")
        );

        let draft = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("0", true)).unwrap();
        for (key, expected) in REVIEWED_NON_FILING_XML_STATE {
            assert!(!draft.preserved_unmodeled_xml_fields.contains_key(key));
            assert_eq!(draft.to_bir_field_map()[key], expected);
        }
        let persisted = serde_json::to_string(&draft).unwrap();
        assert!(!persisted.contains("ebirOnlineSecret"));
        assert!(!persisted.contains("registered-user"));
        assert!(!persisted.contains("must-not-persist"));
    }

    #[test]
    fn unknown_keys_are_preserved_and_reported_without_overriding_modeled_truth() {
        let mut fields = crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("1", true))
            .expect("reviewed sample must parse");
        fields.insert("future:field".to_string(), "future-value".to_string());
        let draft = Form0619FDraft::from_bir_field_map(&fields).expect("unknown key is preserved");
        assert_eq!(
            draft.preserved_unmodeled_xml_fields["future:field"],
            "future-value"
        );
        assert_eq!(draft.to_bir_field_map()["future:field"], "future-value");
        assert!(
            draft
                .xml_evidence_warnings()
                .iter()
                .any(|warning| warning.contains("future:field"))
        );
    }

    #[test]
    fn source_formula_mismatch_fails_closed() {
        let mut fields = crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("1", true))
            .expect("reviewed sample must parse");
        fields.insert("frm0619F:txtTax19".to_string(), "999.00".to_string());
        let errors = Form0619FDraft::from_bir_field_map(&fields)
            .expect_err("mismatched computed source must fail");
        assert!(errors.iter().any(|(field, message)| {
            field == "frm0619F:txtTax19" && message.contains("does not match")
        }));
    }

    #[test]
    fn december_due_month_and_year_export_with_rollover() {
        let mut draft = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        draft.month = 12;
        draft.taxable_year = 2026;
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm0619F:txtDueMonth"], "01");
        assert_eq!(fields["frm0619F:txtDueYear"], "2027");
    }

    #[test]
    fn payment_rows_preserve_blank_vs_explicit_zero_amounts() {
        let mut draft = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        assert_eq!(draft.payment_details.check.amount, None);
        draft.payment_details.check.amount = Some(0.0);
        assert_eq!(draft.to_bir_field_map()["txtAmount21"], "0.00");
    }

    #[test]
    fn malformed_boolean_pair_is_rejected() {
        let mut fields = crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("1", true))
            .expect("reviewed sample must parse");
        fields.insert("frm0619F:optAmend:Y".to_string(), "true".to_string());
        fields.insert("frm0619F:optAmend:N".to_string(), "true".to_string());
        let errors = Form0619FDraft::from_bir_field_map(&fields)
            .expect_err("ambiguous yes/no pair must fail");
        assert!(errors.iter().any(|(field, _)| field == "is_amended"));
    }

    #[test]
    fn due_day_is_manual_but_invalid_calendar_dates_fail_validation() {
        let mut draft = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        draft.month = 1;
        draft.due_day = Some(30);
        assert!(draft.validate().iter().any(|(field, _)| field == "due_day"));
    }

    #[test]
    fn deterministic_contract_export_contains_no_generated_clock_value() {
        let draft = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        let first = draft.to_bir_xml_payload();
        let second = draft.to_bir_xml_payload();
        assert_eq!(first, second);
        assert_eq!(draft.to_bir_field_map()["txtDateIssue"], "");
    }

    #[test]
    fn checked_in_submission_provenance_matches_the_fail_closed_boundary() {
        let provenance: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/official-source/0619f-2018-source.json"
        ))
        .unwrap();

        assert_eq!(
            provenance["official_package_evidence"]["package_sha256"],
            super::super::form_0619f::OFFICIAL_PACKAGE_SHA256
        );
        assert_eq!(
            provenance["official_package_evidence"]["source_resources"]["hta_decoded_sha256"],
            super::super::form_0619f::OFFICIAL_HTA_RESOURCE_DECODED_SHA256
        );
        assert_eq!(
            provenance["official_package_evidence"]["reviewed_outbound_candidate"]["decrypted_sha256"],
            REVIEWED_DECRYPTED_XML_SHA256
        );
        assert_eq!(
            provenance["official_package_evidence"]["reviewed_outbound_candidate"]["decrypted_field_count"],
            EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT
        );
        assert_eq!(
            provenance["official_package_evidence"]["reviewed_outbound_candidate"]["rust_ciphertext_replay"]
                ["reencrypted_sha256"],
            CURRENT_RUST_REENCRYPTED_XML_SHA256
        );
        assert_eq!(
            provenance["official_package_evidence"]["reviewed_outbound_candidate"]["rust_ciphertext_replay"]
                ["matches_locked_ciphertext"],
            false
        );
        // Recorded before 0619-F moved to the generic queue (official
        // layout replay + Encrypt.exe parity); the registry now owns the gate.
        assert_eq!(
            provenance["official_package_evidence"]["submission_boundary"]["queue_submission_supported"],
            false
        );
        assert_eq!(
            super::super::form_0619f::OFFICIAL_PACKAGE_MANIFEST_RESOURCE_ID
                + super::super::form_0619f::OFFICIAL_HTA_MANIFEST_INDEX,
            super::super::form_0619f::OFFICIAL_HTA_RESOURCE_ID
        );
    }

    #[test]
    fn imported_penalties_are_kept_as_overrides() {
        // The return was prepared for a period now long past its deadline:
        // the imported penalty items are kept as overrides, so recomputing
        // today files exactly what the source carried.
        let mut draft = Form0619FDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        let fields = draft.to_bir_field_map();
        let items = Form0619FDraft::penalty_declaration();
        let state = &draft.penalty.penalty_overrides;
        assert_eq!(
            [state.surcharge, state.interest, state.compromise],
            items.filed(&draft).map(Some)
        );
        crate::forms::penalty_items::with_clock(
            chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap(),
            || draft.recompute(),
        );
        assert_eq!(draft.to_bir_field_map(), fields);
    }
}
