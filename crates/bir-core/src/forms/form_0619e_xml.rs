//! Typed XML contract for exact identity `0619Ev2018`.
//!
//! The reviewed evidence is a plain one-line pseudo-XML save and its decrypted
//! encrypted companion.  Their union adds `frm0619E:txtAddress2`; their only
//! conflicting value is `txtFinalFlag` (`1` vs `0`).

use super::FormValidator;
use super::form_0619e::{
    ATC_CODE, Form0619EDraft, Form0619EPaymentDetails, Form0619EPaymentRow, Form0619EXmlFinalFlag,
    TAX_TYPE_CODE, WithholdingAgentCategory,
};
use std::collections::BTreeMap;

const REVIEWED_NON_FILING_XML_STATE: [(&str, &str); 5] = [
    ("driveSelectTPExport", ""),
    ("ebirOnlineConfirmUsername", ""),
    ("ebirOnlineSecret", ""),
    ("ebirOnlineUsername", ""),
    ("txtEnroll", "Y"),
];

impl Form0619EDraft {
    /// Canonical TIN segments for printable 0619-E output.
    ///
    /// The raw draft value remains unchanged.  This uses the same branch-code
    /// left-padding as the XML boundary and returns `None` when the draft TIN
    /// does not satisfy the supported 12-to-14-digit shape.
    pub fn printable_tin_segments(&self) -> Option<[String; 4]> {
        let digit_count = self
            .tin
            .chars()
            .filter(|character| character.is_ascii_digit())
            .count();
        if !(12..=14).contains(&digit_count)
            || self
                .tin
                .chars()
                .any(|character| !character.is_ascii_digit() && character != '-')
        {
            return None;
        }

        let (segment_1, segment_2, segment_3, branch) = split_tin(&self.tin);
        Some([segment_1, segment_2, segment_3, branch])
    }

    /// Serialize every modeled field and preserve any unmodeled source keys.
    /// Modeled values always win, preventing preserved transport data from
    /// mutating tax truth.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.preserved_unmodeled_xml_fields.clone();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        let (due_month, due_year) = self.due_month_and_year();

        // Both locked sources use these exact non-filing values. Rust never
        // persists or replays eBIRForms login, enrollment, or export-device
        // state through an editable 0619-E draft.
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
            "frm0619E:optAmend:Y",
            "frm0619E:optAmend:N",
            self.is_amended,
        );
        insert_bool_pair(
            &mut fields,
            "frm0619E:optWithheld:Y",
            "frm0619E:optWithheld:N",
            self.any_taxes_withheld,
        );
        insert_bool_pair(
            &mut fields,
            "frm0619E:optCategory:G",
            "frm0619E:optCategory:P",
            matches!(
                self.withholding_agent_category,
                WithholdingAgentCategory::Government
            ),
        );

        insert(&mut fields, "frm0619E:txtAtc", ATC_CODE);
        insert(&mut fields, "frm0619E:txtTaxTypeCode", TAX_TYPE_CODE);
        insert(
            &mut fields,
            "frm0619E:txtMonth",
            format!("{:02}", self.month),
        );
        insert(
            &mut fields,
            "frm0619E:txtYear",
            self.taxable_year.to_string(),
        );
        insert(
            &mut fields,
            "frm0619E:txtDueMonth",
            format!("{due_month:02}"),
        );
        insert(
            &mut fields,
            "frm0619E:txtDueDay",
            self.due_day
                .map(|day| format!("{day:02}"))
                .unwrap_or_default(),
        );
        insert(&mut fields, "frm0619E:txtDueYear", due_year.to_string());
        insert(&mut fields, "frm0619E:txtTIN1", tin1);
        insert(&mut fields, "frm0619E:txtTIN2", tin2);
        insert(&mut fields, "frm0619E:txtTIN3", tin3);
        insert(&mut fields, "frm0619E:txtBranchCode", branch);
        insert(&mut fields, "frm0619E:txtRDOCode", self.rdo_code.clone());
        insert(
            &mut fields,
            "frm0619E:txtTaxpayerName",
            self.taxpayer_name.clone(),
        );
        insert(
            &mut fields,
            "frm0619E:txtLineBus",
            self.line_of_business.clone(),
        );
        insert(
            &mut fields,
            "frm0619E:txtAddress",
            self.registered_address.clone(),
        );
        insert(
            &mut fields,
            "frm0619E:txtAddress2",
            self.registered_address_2.clone(),
        );
        insert(&mut fields, "frm0619E:txtZipCode", self.zip_code.clone());
        insert(
            &mut fields,
            "frm0619E:txtTelNum",
            self.contact_number.clone(),
        );

        insert_money(
            &mut fields,
            "frm0619E:txtTax14",
            self.item_14_amount_of_remittance,
        );
        insert_money(
            &mut fields,
            "frm0619E:txtTax15",
            self.item_15_amount_remitted_previously,
        );
        insert_money(
            &mut fields,
            "frm0619E:txtTax16",
            self.item_16_net_amount_of_remittance,
        );
        insert_money(&mut fields, "frm0619E:txtTax17A", self.item_17a_surcharge);
        insert_money(&mut fields, "frm0619E:txtTax17B", self.item_17b_interest);
        insert_money(&mut fields, "frm0619E:txtTax17C", self.item_17c_compromise);
        insert_money(
            &mut fields,
            "frm0619E:txtTax17D",
            self.item_17d_total_penalties,
        );
        insert_money(
            &mut fields,
            "frm0619E:txtTax18",
            self.item_18_total_amount_of_remittance,
        );

        insert_payment_row(
            &mut fields,
            19,
            &self.payment_details.cash_or_bank_debit_memo,
        );
        insert_payment_row(&mut fields, 20, &self.payment_details.check);
        insert_payment_row(&mut fields, 21, &self.payment_details.tax_debit_memo);
        insert_payment_row(&mut fields, 22, &self.payment_details.others);
        insert(
            &mut fields,
            "txtParticular22",
            self.payment_details.others_description.clone(),
        );

        fields
    }

    pub fn to_bir_xml_payload(&self) -> String {
        crate::bir_xml::generate_bir_xml(&self.to_bir_field_map())
    }

    /// Produce an editable/save XML payload only when the typed model is valid.
    /// This is not a submission API; queue transport remains disabled.
    pub fn try_to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = self.validate();
        if errors.is_empty() {
            Ok(self.to_bir_xml_payload())
        } else {
            Err(errors)
        }
    }

    /// Parse BIR pseudo-XML even when every `<div>` appears on one physical
    /// line, as in both reviewed 0619-E source payloads.
    pub fn from_bir_xml_payload(xml: &str) -> Result<Self, Vec<(String, String)>> {
        let fields = crate::bir_xml::parse_bir_xml_checked(xml).map_err(|error| {
            vec![(
                "xml_payload".to_string(),
                format!("Invalid 0619-E pseudo-XML: {error}"),
            )]
        })?;
        Self::from_bir_field_map(&fields)
    }

    pub fn from_bir_field_map(
        fields: &BTreeMap<String, String>,
    ) -> Result<Self, Vec<(String, String)>> {
        let mut errors = Vec::new();
        let month = parse_required::<u8>(fields, "frm0619E:txtMonth", &mut errors);
        let taxable_year = parse_required::<u16>(fields, "frm0619E:txtYear", &mut errors);
        let is_amended = parse_bool_pair(
            fields,
            "frm0619E:optAmend:Y",
            "frm0619E:optAmend:N",
            "is_amended",
            &mut errors,
        );
        let any_taxes_withheld = parse_bool_pair(
            fields,
            "frm0619E:optWithheld:Y",
            "frm0619E:optWithheld:N",
            "any_taxes_withheld",
            &mut errors,
        );
        let category_government = parse_bool_pair(
            fields,
            "frm0619E:optCategory:G",
            "frm0619E:optCategory:P",
            "withholding_agent_category",
            &mut errors,
        );

        verify_fixed_code(fields, "frm0619E:txtAtc", ATC_CODE, &mut errors);
        verify_fixed_code(
            fields,
            "frm0619E:txtTaxTypeCode",
            TAX_TYPE_CODE,
            &mut errors,
        );
        validate_reviewed_non_filing_state(fields, &mut errors);

        let xml_final_flag = match field(fields, "txtFinalFlag") {
            "0" => Form0619EXmlFinalFlag::Zero,
            "1" => Form0619EXmlFinalFlag::One,
            "" => Form0619EXmlFinalFlag::Missing,
            value => Form0619EXmlFinalFlag::Unknown(value.to_string()),
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        let mut draft = Form0619EDraft {
            penalty: crate::forms::penalty_items::PenaltyState::imported(),
            id: None,
            tin: format!(
                "{}{}{}{}",
                field(fields, "frm0619E:txtTIN1"),
                field(fields, "frm0619E:txtTIN2"),
                field(fields, "frm0619E:txtTIN3"),
                field(fields, "frm0619E:txtBranchCode")
            ),
            taxable_year: taxable_year.unwrap_or_default(),
            month: month.unwrap_or_default(),
            is_amended: is_amended.unwrap_or(false),
            any_taxes_withheld: any_taxes_withheld.unwrap_or(false),
            withholding_agent_category: if category_government.unwrap_or(false) {
                WithholdingAgentCategory::Government
            } else {
                WithholdingAgentCategory::Private
            },
            due_day: parse_optional::<u8>(fields, "frm0619E:txtDueDay", &mut errors),
            rdo_code: semantic_text(fields, "frm0619E:txtRDOCode"),
            taxpayer_name: semantic_text(fields, "frm0619E:txtTaxpayerName"),
            line_of_business: semantic_text(fields, "frm0619E:txtLineBus"),
            registered_address: semantic_text(fields, "frm0619E:txtAddress"),
            registered_address_2: semantic_text(fields, "frm0619E:txtAddress2"),
            zip_code: semantic_text(fields, "frm0619E:txtZipCode"),
            contact_number: semantic_text(fields, "frm0619E:txtTelNum"),
            email: semantic_text(fields, "txtEmail"),
            item_14_amount_of_remittance: parse_money(
                fields,
                "frm0619E:txtTax14",
                false,
                &mut errors,
            )
            .unwrap_or_default(),
            item_15_amount_remitted_previously: parse_money(
                fields,
                "frm0619E:txtTax15",
                false,
                &mut errors,
            )
            .unwrap_or_default(),
            item_16_net_amount_of_remittance: 0.0,
            item_17a_surcharge: parse_money(fields, "frm0619E:txtTax17A", false, &mut errors)
                .unwrap_or_default(),
            item_17b_interest: parse_money(fields, "frm0619E:txtTax17B", false, &mut errors)
                .unwrap_or_default(),
            item_17c_compromise: parse_money(fields, "frm0619E:txtTax17C", false, &mut errors)
                .unwrap_or_default(),
            item_17d_total_penalties: 0.0,
            item_18_total_amount_of_remittance: 0.0,
            tax_agent_accreditation_number: semantic_text(fields, "txtTaxAgentNo"),
            tax_agent_date_of_issue: semantic_text(fields, "txtDateIssue"),
            tax_agent_date_of_expiry: semantic_text(fields, "txtDateExpiry"),
            payment_details: Form0619EPaymentDetails {
                cash_or_bank_debit_memo: parse_payment_row(fields, 19, &mut errors),
                check: parse_payment_row(fields, 20, &mut errors),
                tax_debit_memo: parse_payment_row(fields, 21, &mut errors),
                others: parse_payment_row(fields, 22, &mut errors),
                others_description: semantic_text(fields, "txtParticular22"),
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

        let source_16 = parse_money(fields, "frm0619E:txtTax16", false, &mut errors);
        let source_17d = parse_money(fields, "frm0619E:txtTax17D", false, &mut errors);
        let source_18 = parse_money(fields, "frm0619E:txtTax18", false, &mut errors);
        // An imported return carries the penalties it was prepared with:
        // keep each one as an override so the source amounts round-trip.
        Form0619EDraft::penalty_declaration().adopt_imported(&mut draft);
        draft.recompute();
        verify_computed_source(
            "frm0619E:txtTax16",
            source_16,
            draft.item_16_net_amount_of_remittance,
            &mut errors,
        );
        verify_computed_source(
            "frm0619E:txtTax17D",
            source_17d,
            draft.item_17d_total_penalties,
            &mut errors,
        );
        verify_computed_source(
            "frm0619E:txtTax18",
            source_18,
            draft.item_18_total_amount_of_remittance,
            &mut errors,
        );

        let (expected_due_month, expected_due_year) = draft.due_month_and_year();
        verify_due_component(
            fields,
            "frm0619E:txtDueMonth",
            u16::from(expected_due_month),
            &mut errors,
        );
        verify_due_component(
            fields,
            "frm0619E:txtDueYear",
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
    // One reviewed plain field is double encoded (`SOFTWARE%2520DEVELOPMENT`),
    // while its encrypted companion contains the once-encoded semantic value.
    // Decode until stable, with a small fixed bound, only for modeled text.
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
                    "0619-E import requires the reviewed non-filing value {key}={expected:?}; credential, enrollment, and export-device state is never persisted"
                ),
            )),
            None => errors.push((
                key.to_string(),
                format!("Required reviewed 0619-E non-filing field {key} is missing"),
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
            format!("Required 0619-E field {key} is missing"),
        ));
        return None;
    }
    value.parse::<T>().map(Some).unwrap_or_else(|_| {
        errors.push((
            key.to_string(),
            format!("0619-E field {key} has invalid value {value:?}"),
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
            format!("0619-E field {key} has invalid value {value:?}"),
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
                format!("0619-E choice {semantic_field} must have exactly one selected value"),
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
                format!("0619-E boolean field {key} has invalid value {value:?}"),
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
                format!("0619-E money field {key} has invalid value {raw:?}"),
            ));
            None
        }
    }
}

fn parse_payment_row(
    fields: &BTreeMap<String, String>,
    item: u8,
    errors: &mut Vec<(String, String)>,
) -> Form0619EPaymentRow {
    Form0619EPaymentRow {
        drawee_bank_or_agency: semantic_text(fields, &format!("txtAgency{item}")),
        number: semantic_text(fields, &format!("txtNumber{item}")),
        date: semantic_text(fields, &format!("txtDate{item}")),
        amount: parse_money(fields, &format!("txtAmount{item}"), true, errors),
    }
}

fn verify_fixed_code(
    fields: &BTreeMap<String, String>,
    key: &str,
    expected: &str,
    errors: &mut Vec<(String, String)>,
) {
    let actual = field(fields, key).trim();
    if actual != expected {
        errors.push((
            key.to_string(),
            format!("Exact 0619-E revision requires fixed {key}={expected}, found {actual:?}"),
        ));
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
                "0619-E source value {source:.2} does not match the official computed value {computed:.2}"
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
            format!("0619-E {key} must be {expected}, found {actual}"),
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
            | "txtParticular22"
            | "frm0619E:optAmend:Y"
            | "frm0619E:optAmend:N"
            | "frm0619E:optWithheld:Y"
            | "frm0619E:optWithheld:N"
            | "frm0619E:optCategory:G"
            | "frm0619E:optCategory:P"
            | "frm0619E:txtAtc"
            | "frm0619E:txtTaxTypeCode"
            | "frm0619E:txtMonth"
            | "frm0619E:txtYear"
            | "frm0619E:txtDueMonth"
            | "frm0619E:txtDueDay"
            | "frm0619E:txtDueYear"
            | "frm0619E:txtTIN1"
            | "frm0619E:txtTIN2"
            | "frm0619E:txtTIN3"
            | "frm0619E:txtBranchCode"
            | "frm0619E:txtRDOCode"
            | "frm0619E:txtTaxpayerName"
            | "frm0619E:txtLineBus"
            | "frm0619E:txtAddress"
            | "frm0619E:txtAddress2"
            | "frm0619E:txtZipCode"
            | "frm0619E:txtTelNum"
            | "frm0619E:txtTax14"
            | "frm0619E:txtTax15"
            | "frm0619E:txtTax16"
            | "frm0619E:txtTax17A"
            | "frm0619E:txtTax17B"
            | "frm0619E:txtTax17C"
            | "frm0619E:txtTax17D"
            | "frm0619E:txtTax18"
            | "txtAgency19"
            | "txtNumber19"
            | "txtDate19"
            | "txtAmount19"
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
    )
}

fn insert_payment_row(fields: &mut BTreeMap<String, String>, item: u8, row: &Form0619EPaymentRow) {
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
    use super::super::form_0619e::{
        CURRENT_RUST_REENCRYPTED_XML_SHA256, EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT,
        REVIEWED_DECRYPTED_XML_SHA256,
    };
    use super::*;

    fn reviewed_sample(final_flag: &str, include_address_2: bool) -> String {
        let mut fields = BTreeMap::from([
            ("frm0619E:txtMonth".to_string(), "04".to_string()),
            ("frm0619E:txtYear".to_string(), "2026".to_string()),
            ("frm0619E:txtDueMonth".to_string(), "05".to_string()),
            ("frm0619E:txtDueDay".to_string(), "10".to_string()),
            ("frm0619E:txtDueYear".to_string(), "2026".to_string()),
            ("frm0619E:optAmend:Y".to_string(), "false".to_string()),
            ("frm0619E:optAmend:N".to_string(), "true".to_string()),
            ("frm0619E:optWithheld:Y".to_string(), "true".to_string()),
            ("frm0619E:optWithheld:N".to_string(), "false".to_string()),
            ("frm0619E:txtAtc".to_string(), "WME10".to_string()),
            ("frm0619E:txtTaxTypeCode".to_string(), "WE".to_string()),
            ("frm0619E:txtTIN1".to_string(), "000".to_string()),
            ("frm0619E:txtTIN2".to_string(), "000".to_string()),
            ("frm0619E:txtTIN3".to_string(), "000".to_string()),
            ("frm0619E:txtBranchCode".to_string(), "00000".to_string()),
            ("frm0619E:txtRDOCode".to_string(), "018".to_string()),
            (
                "frm0619E:txtTaxpayerName".to_string(),
                "JUAN DELA CRUZ".to_string(),
            ),
            (
                "frm0619E:txtLineBus".to_string(),
                "SOFTWARE%20DEVELOPMENT".to_string(),
            ),
            ("frm0619E:txtAddress".to_string(), "OLONGAPO".to_string()),
            ("frm0619E:txtZipCode".to_string(), "2200".to_string()),
            ("frm0619E:txtTelNum".to_string(), "09123456789".to_string()),
            ("frm0619E:optCategory:P".to_string(), "true".to_string()),
            ("frm0619E:optCategory:G".to_string(), "false".to_string()),
            ("txtEmail".to_string(), "taxpayer@example.com".to_string()),
            ("frm0619E:txtTax14".to_string(), "1,000.00".to_string()),
            ("frm0619E:txtTax15".to_string(), "0.00".to_string()),
            ("frm0619E:txtTax16".to_string(), "1,000.00".to_string()),
            ("frm0619E:txtTax17A".to_string(), "100.00".to_string()),
            ("frm0619E:txtTax17B".to_string(), "30.00".to_string()),
            ("frm0619E:txtTax17C".to_string(), "100.00".to_string()),
            ("frm0619E:txtTax17D".to_string(), "230.00".to_string()),
            ("frm0619E:txtTax18".to_string(), "1,230.00".to_string()),
            ("txtFinalFlag".to_string(), final_flag.to_string()),
            ("txtEnroll".to_string(), "Y".to_string()),
            ("driveSelectTPExport".to_string(), "".to_string()),
            ("ebirOnlineConfirmUsername".to_string(), "".to_string()),
            ("ebirOnlineUsername".to_string(), "".to_string()),
            ("ebirOnlineSecret".to_string(), "".to_string()),
        ]);
        for item in 19..=22 {
            for prefix in ["txtAgency", "txtNumber", "txtDate", "txtAmount"] {
                fields.insert(format!("{prefix}{item}"), String::new());
            }
        }
        for key in [
            "txtTaxAgentNo",
            "txtDateIssue",
            "txtDateExpiry",
            "txtParticular22",
        ] {
            fields.insert(key.to_string(), String::new());
        }
        if include_address_2 {
            fields.insert("frm0619E:txtAddress2".to_string(), String::new());
        }
        crate::bir_xml::generate_bir_xml(&fields).replace("\t\n", "")
    }

    #[test]
    fn reviewed_plain_and_companion_final_flags_are_both_preserved() {
        let plain = Form0619EDraft::from_bir_xml_payload(&reviewed_sample("1", false))
            .expect("reviewed plain save must parse");
        let companion = Form0619EDraft::from_bir_xml_payload(&reviewed_sample("0", true))
            .expect("reviewed encrypted companion must parse after decryption");
        assert_eq!(plain.xml_final_flag, Form0619EXmlFinalFlag::One);
        assert_eq!(companion.xml_final_flag, Form0619EXmlFinalFlag::Zero);
        assert_eq!(plain.to_bir_field_map()["txtFinalFlag"], "1");
        assert_eq!(companion.to_bir_field_map()["txtFinalFlag"], "0");
    }

    #[test]
    fn one_line_payload_and_double_encoded_semantic_text_parse_correctly() {
        let draft = Form0619EDraft::from_bir_xml_payload(&reviewed_sample("1", false))
            .expect("one-line reviewed save must parse");
        assert_eq!(draft.line_of_business, "SOFTWARE DEVELOPMENT");
        assert_eq!(draft.taxpayer_name, "JUAN DELA CRUZ");
        assert_eq!(draft.item_18_total_amount_of_remittance, 1_230.0);
    }

    #[test]
    fn address_line_two_is_in_the_union_and_roundtrips() {
        let mut fields = crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("0", true))
            .expect("reviewed sample must parse");
        fields.insert(
            "frm0619E:txtAddress2".to_string(),
            "SECOND LINE".to_string(),
        );
        let draft = Form0619EDraft::from_bir_field_map(&fields).expect("union must parse");
        assert_eq!(draft.registered_address_2, "SECOND LINE");
        assert_eq!(
            draft.to_bir_field_map()["frm0619E:txtAddress2"],
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
            let errors = Form0619EDraft::from_bir_field_map(&fields)
                .expect_err("non-reviewed credential or UI state must fail closed");
            assert!(errors.iter().any(|(field, _)| field == key));
        }

        let mut fields =
            crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("1", true)).unwrap();
        fields.remove("ebirOnlineSecret");
        assert!(
            Form0619EDraft::from_bir_field_map(&fields)
                .unwrap_err()
                .iter()
                .any(|(field, _)| field == "ebirOnlineSecret")
        );

        let draft = Form0619EDraft::from_bir_xml_payload(&reviewed_sample("0", true)).unwrap();
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
        let draft = Form0619EDraft::from_bir_field_map(&fields).expect("unknown key is preserved");
        assert_eq!(
            draft.preserved_unmodeled_xml_fields["future:field"],
            "future-value"
        );
        assert_eq!(draft.to_bir_field_map()["future:field"], "future-value");
        assert!(
            draft
                .xml_evidence_warnings()
                .iter()
                .any(|warning| { warning.contains("future:field") })
        );
    }

    #[test]
    fn source_formula_mismatch_fails_closed() {
        let mut fields = crate::bir_xml::parse_bir_xml_checked(&reviewed_sample("1", true))
            .expect("reviewed sample must parse");
        fields.insert("frm0619E:txtTax18".to_string(), "999.00".to_string());
        let errors = Form0619EDraft::from_bir_field_map(&fields)
            .expect_err("mismatched computed source must fail");
        assert!(errors.iter().any(|(field, message)| {
            field == "frm0619E:txtTax18" && message.contains("does not match")
        }));
    }

    #[test]
    fn december_due_month_and_year_export_with_rollover() {
        let mut draft = Form0619EDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        draft.month = 12;
        draft.taxable_year = 2026;
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm0619E:txtDueMonth"], "01");
        assert_eq!(fields["frm0619E:txtDueYear"], "2027");
    }

    #[test]
    fn payment_rows_preserve_blank_vs_explicit_zero_amounts() {
        let mut draft = Form0619EDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        assert_eq!(draft.payment_details.check.amount, None);
        draft.payment_details.check.amount = Some(0.0);
        assert_eq!(draft.to_bir_field_map()["txtAmount20"], "0.00");
    }

    #[test]
    fn due_day_is_manual_but_invalid_calendar_dates_fail_validation() {
        let mut draft = Form0619EDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        draft.month = 1;
        draft.due_day = Some(30);
        assert!(draft.validate().iter().any(|(field, _)| field == "due_day"));
    }

    #[test]
    fn checked_in_submission_provenance_matches_the_fail_closed_boundary() {
        let provenance: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/official-source/0619e-2018-source.json"
        ))
        .unwrap();

        assert_eq!(
            provenance["official_package_evidence"]["package_sha256"],
            super::super::form_0619e::OFFICIAL_PACKAGE_SHA256
        );
        assert_eq!(
            provenance["official_package_evidence"]["source_resources"]["hta_decoded_sha256"],
            super::super::form_0619e::OFFICIAL_HTA_RESOURCE_DECODED_SHA256
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
        // Recorded before 0619-E moved to the generic queue (official
        // layout replay + Encrypt.exe parity); the registry now owns the gate.
        assert_eq!(
            provenance["official_package_evidence"]["submission_boundary"]["queue_submission_supported"],
            false
        );
        assert_eq!(
            super::super::form_0619e::OFFICIAL_PACKAGE_MANIFEST_RESOURCE_ID
                + super::super::form_0619e::OFFICIAL_HTA_MANIFEST_INDEX,
            super::super::form_0619e::OFFICIAL_HTA_RESOURCE_ID
        );
    }

    #[test]
    fn imported_penalties_are_kept_as_overrides() {
        // The return was prepared for a period now long past its deadline:
        // the imported penalty items are kept as overrides, so recomputing
        // today files exactly what the source carried.
        let mut draft = Form0619EDraft::from_bir_xml_payload(&reviewed_sample("1", true))
            .expect("reviewed save must parse");
        let fields = draft.to_bir_field_map();
        let items = Form0619EDraft::penalty_declaration();
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
